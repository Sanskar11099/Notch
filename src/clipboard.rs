// Session-only clipboard history: held in memory, gone when the notch exits (so at shutdown or restart).
// Pure data and rules (search, date filter, previews); the Windows plumbing lives in dnd.rs.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

pub const ROWS: usize = 4; // rows visible on the page
const MAX_ITEMS: usize = 200;
pub const MAX_ITEM_BYTES: usize = 32 << 20; // a bigger single copy is not kept
const MAX_TOTAL_BYTES: usize = 256 << 20; // oldest items go first when the history grows past this
const PREVIEW_CHARS: usize = 100;
// ponytail: search looks at the first 4000 chars of each item (it runs on every redraw); index lazily if that bites.
const HAYSTACK_CHARS: usize = 4000;

pub const CF_DIB: u16 = 8;
pub const CF_UNICODETEXT: u16 = 13;
pub const CF_HDROP: u16 = 15;

/// One clipboard format and its raw bytes (everything we keep is an HGLOBAL payload).
pub type Format = (u16, Vec<u8>);

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Local date and time of day, to the minute.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub minute: u16, // minutes since local midnight
}

impl Stamp {
    pub fn now() -> Stamp {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Stamp { year: t.wYear as i32, month: t.wMonth as u8, day: t.wDay as u8, minute: t.wHour * 60 + t.wMinute }
    }

    /// Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    pub fn days(self) -> i32 {
        let y = if self.month <= 2 { self.year - 1 } else { self.year };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (self.month as i32 + 9) % 12;
        let doy = (153 * mp + 2) / 5 + self.day as i32 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }
}

/// "14:32" today, "Yesterday 14:32", otherwise "3 Nov 14:32".
pub fn when_label(at: Stamp, today: i32) -> String {
    let hm = format!("{:02}:{:02}", at.minute / 60, at.minute % 60);
    match today - at.days() {
        0 => hm,
        1 => format!("Yesterday {hm}"),
        _ => format!("{} {} {hm}", at.day, MONTHS[(at.month as usize).clamp(1, 12) - 1]),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DateRange {
    #[default]
    Any,
    Today,
    Yesterday,
    Week,
    Month,
}

impl DateRange {
    pub fn next(self) -> DateRange {
        match self {
            DateRange::Any => DateRange::Today,
            DateRange::Today => DateRange::Yesterday,
            DateRange::Yesterday => DateRange::Week,
            DateRange::Week => DateRange::Month,
            DateRange::Month => DateRange::Any,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DateRange::Any => "Any date",
            DateRange::Today => "Today",
            DateRange::Yesterday => "Yesterday",
            DateRange::Week => "Last 7 days",
            DateRange::Month => "Last 30 days",
        }
    }

    pub fn contains(self, day: i32, today: i32) -> bool {
        match self {
            DateRange::Any => true,
            DateRange::Today => day == today,
            DateRange::Yesterday => day == today - 1,
            DateRange::Week => (today - 6..=today).contains(&day),
            DateRange::Month => (today - 29..=today).contains(&day),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Files,
    Image,
    Other,
}

pub struct Item {
    pub id: u64,
    pub kind: Kind,
    pub preview: String, // one line, shown in the list
    haystack: String,    // lowercased, what the search looks at
    pub at: Stamp,
    pub formats: Vec<Format>,
    bytes: usize,
    hash: u64,
}

fn format_bytes(formats: &[Format], cf: u16) -> Option<&[u8]> {
    formats.iter().find(|(f, _)| *f == cf).map(|(_, b)| b.as_slice())
}

fn parse_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Paths in a CF_HDROP block (a DROPFILES header, then NUL-separated paths ending in a double NUL).
fn parse_files(bytes: &[u8]) -> Vec<String> {
    let u32_at = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(offset), Some(wide)) = (u32_at(0), u32_at(16)) else { return Vec::new() };
    let Some(body) = bytes.get(offset as usize..) else { return Vec::new() };
    if wide != 0 {
        let units: Vec<u16> = body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        units.split(|&u| u == 0).take_while(|p| !p.is_empty()).map(String::from_utf16_lossy).collect()
    } else {
        body.split(|&b| b == 0).take_while(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect()
    }
}

/// Width and height from a CF_DIB's BITMAPINFOHEADER.
fn dib_size(bytes: &[u8]) -> Option<(i32, i32)> {
    let w = i32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    let h = i32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?);
    Some((w, h.abs()))
}

fn one_line(text: &str) -> String {
    let head: String = text.chars().take(PREVIEW_CHARS * 4).collect();
    head.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(PREVIEW_CHARS).collect()
}

impl Item {
    /// None when there is nothing worth keeping (e.g. whitespace-only text).
    fn new(id: u64, formats: Vec<Format>, at: Stamp) -> Option<Item> {
        let (kind, preview, haystack) = if let Some(files) = format_bytes(&formats, CF_HDROP).map(parse_files).filter(|f| !f.is_empty()) {
            let name = |p: &String| p.rsplit(['\\', '/']).next().unwrap_or(p).to_string();
            let preview = match files.len() {
                1 => name(&files[0]),
                n => format!("{} (+{} more)", name(&files[0]), n - 1),
            };
            (Kind::Files, preview, files.join("\n"))
        } else if let Some(text) = format_bytes(&formats, CF_UNICODETEXT).map(parse_text) {
            if text.trim().is_empty() {
                return None;
            }
            (Kind::Text, one_line(&text), text)
        } else if let Some((w, h)) = format_bytes(&formats, CF_DIB).and_then(dib_size) {
            (Kind::Image, format!("Image {w} \u{d7} {h}"), "image picture screenshot".to_string())
        } else {
            (Kind::Other, "Copied data".to_string(), String::new())
        };
        // Identity is the main payload only: extras like a source URL differ between copies of the same thing.
        let mut hasher = DefaultHasher::new();
        match [CF_HDROP, CF_UNICODETEXT, CF_DIB].iter().find_map(|&cf| format_bytes(&formats, cf)) {
            Some(main) => main.hash(&mut hasher),
            None => formats.hash(&mut hasher),
        }
        let bytes = formats.iter().map(|(_, b)| b.len()).sum();
        Some(Item {
            id,
            kind,
            preview,
            haystack: haystack.chars().take(HAYSTACK_CHARS).collect::<String>().to_lowercase(),
            at,
            formats,
            bytes,
            hash: hasher.finish(),
        })
    }
}

/// Newest first.
#[derive(Default)]
pub struct Store {
    items: Vec<Item>,
    next_id: u64,
}

impl Store {
    /// Keep a copy. Copying something already in the history moves it to the top. False if it was not kept.
    pub fn add(&mut self, formats: Vec<Format>, at: Stamp) -> bool {
        if formats.is_empty() || formats.iter().map(|(_, b)| b.len()).sum::<usize>() > MAX_ITEM_BYTES {
            return false;
        }
        let Some(item) = Item::new(self.next_id, formats, at) else { return false };
        self.next_id += 1;
        self.items.retain(|i| i.hash != item.hash);
        self.items.insert(0, item);
        self.items.truncate(MAX_ITEMS);
        while self.items.len() > 1 && self.items.iter().map(|i| i.bytes).sum::<usize>() > MAX_TOTAL_BYTES {
            self.items.pop();
        }
        true
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|i| i.id != id);
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Delete everything `matches` would list for this search and date filter. Returns how many went.
    pub fn remove_matching(&mut self, query: &str, range: DateRange, today: i32) -> usize {
        let ids: Vec<u64> = self.matches(query, range, today).iter().map(|i| i.id).collect();
        self.items.retain(|i| !ids.contains(&i.id));
        ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Items matching every word of `query` (case-insensitive) that were copied within `range`.
    pub fn matches(&self, query: &str, range: DateRange, today: i32) -> Vec<&Item> {
        let query = query.to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        self.items
            .iter()
            .filter(|i| range.contains(i.at.days(), today) && words.iter().all(|w| i.haystack.contains(w)))
            .collect()
    }
}

/// What the user is typing in the search box. WM_CHAR delivers UTF-16 units, so emoji arrive as two halves.
#[derive(Default)]
pub struct Search {
    pub text: String,
    high: Option<u16>,
}

impl Search {
    pub fn push_unit(&mut self, unit: u16) {
        let units = self.high.take().into_iter().chain([unit]);
        let last = unit;
        for ch in char::decode_utf16(units) {
            match ch {
                Ok(c) => self.text.push(c),
                // Only a high surrogate that is the unit just typed waits for its other half; a stale one is dropped.
                Err(e) if e.unpaired_surrogate() == last && (0xD800..0xDC00).contains(&last) => self.high = Some(last),
                Err(_) => {}
            }
        }
    }

    pub fn backspace(&mut self) {
        self.high = None;
        self.text.pop();
    }

    pub fn clear(&mut self) {
        self.high = None;
        self.text.clear();
    }
}

/// A mouse press on a row, kept until the button comes up (a click) or the mouse moves far enough (a drag).
#[derive(Clone, Copy)]
pub struct Press {
    pub id: u64,
    pub x: i32,
    pub y: i32,
}

/// Everything the clipboard page remembers: the history plus what the user is doing with it.
#[derive(Default)]
pub struct View {
    pub store: Store,
    pub search: Search,
    pub range: DateRange,
    pub scroll: usize,
    pub hover: Option<usize>, // visible row under the cursor
    pub typing: bool,         // the search box has keyboard focus
    pub press: Option<Press>,
    pub prev_fg: isize, // window that had focus before the search box took it
    pub retries: u8,    // clipboard was busy: attempts made for the current change
    pub own_write: Option<std::time::Instant>, // when we last put an item on the clipboard ourselves
}

impl View {
    /// Scroll the list one row. False when it is already at that end (so the wheel can change page instead).
    pub fn scroll_by(&mut self, down: bool, len: usize) -> bool {
        let max = len.saturating_sub(ROWS);
        let next = if down { (self.scroll + 1).min(max) } else { self.scroll.saturating_sub(1) };
        std::mem::replace(&mut self.scroll, next) != next
    }

    /// Back to a fresh page (when the notch collapses).
    pub fn reset_view(&mut self) {
        self.search.clear();
        self.range = DateRange::Any;
        self.scroll = 0;
        self.hover = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(year: i32, month: u8, day: u8, minute: u16) -> Stamp {
        Stamp { year, month, day, minute }
    }

    fn text(s: &str) -> Vec<Format> {
        let mut bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes.extend([0, 0]);
        vec![(CF_UNICODETEXT, bytes)]
    }

    fn files(paths: &[&str]) -> Vec<Format> {
        let mut bytes = vec![0u8; 20];
        bytes[0] = 20; // offset of the file list
        bytes[16] = 1; // wide characters
        for p in paths {
            bytes.extend(p.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        }
        bytes.extend([0, 0]);
        vec![(CF_HDROP, bytes)]
    }

    #[test]
    fn days_match_known_dates() {
        assert_eq!(stamp(1970, 1, 1, 0).days(), 0);
        assert_eq!(stamp(2000, 3, 1, 0).days(), 11017);
        assert_eq!(stamp(2026, 11, 2, 0).days() - stamp(2026, 10, 31, 0).days(), 2);
        assert_eq!(stamp(2024, 3, 1, 0).days() - stamp(2024, 2, 28, 0).days(), 2); // leap year
    }

    #[test]
    fn date_ranges_pick_the_right_days() {
        let today = 1000;
        assert!(DateRange::Any.contains(5, today));
        assert!(DateRange::Today.contains(1000, today) && !DateRange::Today.contains(999, today));
        assert!(DateRange::Yesterday.contains(999, today) && !DateRange::Yesterday.contains(1000, today));
        assert!(DateRange::Week.contains(994, today) && !DateRange::Week.contains(993, today));
        assert!(DateRange::Month.contains(971, today) && !DateRange::Month.contains(970, today));
        assert!(!DateRange::Week.contains(1001, today)); // from the future (clock moved back)
        let mut r = DateRange::Any;
        for _ in 0..5 {
            r = r.next();
        }
        assert_eq!(r, DateRange::Any);
    }

    #[test]
    fn when_labels() {
        let today = stamp(2026, 11, 2, 0).days();
        assert_eq!(when_label(stamp(2026, 11, 2, 14 * 60 + 5), today), "14:05");
        assert_eq!(when_label(stamp(2026, 11, 1, 9 * 60), today), "Yesterday 09:00");
        assert_eq!(when_label(stamp(2026, 10, 3, 23 * 60 + 59), today), "3 Oct 23:59");
    }

    #[test]
    fn search_is_case_insensitive_and_matches_every_word() {
        let mut store = Store::default();
        let now = stamp(2026, 11, 2, 600);
        store.add(text("Hello World"), now);
        store.add(text("hello there"), now);
        store.add(files(&["C:\\docs\\Report Final.pdf"]), now);
        let today = now.days();
        assert_eq!(store.matches("", DateRange::Any, today).len(), 3);
        assert_eq!(store.matches("HELLO", DateRange::Any, today).len(), 2);
        assert_eq!(store.matches("hello world", DateRange::Any, today).len(), 1);
        assert_eq!(store.matches("report pdf", DateRange::Any, today).len(), 1); // file paths are searchable
        assert!(store.matches("nope", DateRange::Any, today).is_empty());
    }

    #[test]
    fn date_filter_applies_with_search() {
        let mut store = Store::default();
        store.add(text("old note"), stamp(2026, 10, 20, 600));
        store.add(text("new note"), stamp(2026, 11, 2, 600));
        let today = stamp(2026, 11, 2, 900).days();
        assert_eq!(store.matches("note", DateRange::Any, today).len(), 2);
        let today_only = store.matches("note", DateRange::Today, today);
        assert_eq!(today_only.len(), 1);
        assert_eq!(today_only[0].preview, "new note");
        assert!(store.matches("old", DateRange::Week, today).is_empty());
    }

    #[test]
    fn deleting_the_shown_items_keeps_the_rest() {
        let mut store = Store::default();
        store.add(text("keep me"), stamp(2026, 11, 2, 600));
        store.add(text("secret one"), stamp(2026, 11, 2, 601));
        store.add(text("secret two"), stamp(2026, 10, 1, 602));
        let today = stamp(2026, 11, 2, 900).days();
        assert_eq!(store.remove_matching("secret", DateRange::Today, today), 1); // only today's secret
        assert_eq!(store.matches("", DateRange::Any, today).len(), 2);
        assert_eq!(store.remove_matching("secret", DateRange::Any, today), 1);
        assert_eq!(store.matches("", DateRange::Any, today)[0].preview, "keep me");
        let id = store.matches("", DateRange::Any, today)[0].id;
        store.remove(id);
        assert!(store.is_empty());
    }

    #[test]
    fn copying_again_moves_to_top_without_duplicating() {
        let mut store = Store::default();
        let now = stamp(2026, 11, 2, 600);
        store.add(text("a"), now);
        store.add(text("b"), now);
        store.add(text("a"), now);
        let list = store.matches("", DateRange::Any, now.days());
        assert_eq!(list.iter().map(|i| i.preview.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn history_is_capped_and_oversize_or_blank_copies_are_ignored() {
        let mut store = Store::default();
        let now = stamp(2026, 11, 2, 600);
        for n in 0..MAX_ITEMS + 20 {
            store.add(text(&format!("item {n}")), now);
        }
        let list = store.matches("", DateRange::Any, now.days());
        assert_eq!(list.len(), MAX_ITEMS);
        assert_eq!(list[0].preview, format!("item {}", MAX_ITEMS + 19)); // newest kept, oldest dropped
        assert!(!store.add(text("   \n\t"), now));
        assert!(!store.add(vec![(CF_UNICODETEXT, vec![1; MAX_ITEM_BYTES + 1])], now));
    }

    #[test]
    fn previews_for_each_kind() {
        let now = stamp(2026, 11, 2, 600);
        let one = Item::new(0, text("line one\n\n  line   two"), now).unwrap();
        assert_eq!((one.kind, one.preview.as_str()), (Kind::Text, "line one line two"));
        let long = Item::new(1, text(&"x".repeat(500)), now).unwrap();
        assert_eq!(long.preview.chars().count(), PREVIEW_CHARS);
        let f = Item::new(2, files(&["C:\\a\\b.txt", "C:\\a\\c.txt", "D:\\d.txt"]), now).unwrap();
        assert_eq!((f.kind, f.preview.as_str()), (Kind::Files, "b.txt (+2 more)"));
        let mut header = vec![0u8; 40];
        header[4..8].copy_from_slice(&1920i32.to_le_bytes());
        header[8..12].copy_from_slice(&(-1080i32).to_le_bytes()); // top-down bitmaps have a negative height
        let img = Item::new(3, vec![(CF_DIB, header)], now).unwrap();
        assert_eq!((img.kind, img.preview.as_str()), (Kind::Image, "Image 1920 \u{d7} 1080"));
    }

    #[test]
    fn typed_emoji_arrive_as_two_halves() {
        let mut s = Search::default();
        for u in "a\u{1F600}b".encode_utf16() {
            s.push_unit(u);
        }
        assert_eq!(s.text, "a\u{1F600}b");
        s.backspace();
        s.backspace();
        assert_eq!(s.text, "a");
    }

    #[test]
    fn a_stray_high_surrogate_does_not_pair_with_a_later_low_one() {
        let mut s = Search::default();
        for u in [0xD83D, b'a' as u16, 0xDE00] {
            s.push_unit(u);
        }
        assert_eq!(s.text, "a"); // the stray halves are dropped, not glued into an emoji
    }

    #[test]
    fn wheel_scrolls_until_the_ends() {
        let mut v = View::default();
        assert!(!v.scroll_by(true, ROWS)); // everything fits: nothing to scroll
        assert!(v.scroll_by(true, ROWS + 2) && v.scroll_by(true, ROWS + 2));
        assert!(!v.scroll_by(true, ROWS + 2)); // at the bottom
        assert_eq!(v.scroll, 2);
        assert!(v.scroll_by(false, ROWS + 2) && v.scroll_by(false, ROWS + 2));
        assert!(!v.scroll_by(false, ROWS + 2)); // at the top: the caller changes page instead
    }
}
