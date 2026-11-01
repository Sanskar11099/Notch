// The clipboard page: drawing, hit-testing, and the handlers for clicks, typing, drops and paste.
// The history and search rules are in clipboard.rs; the OLE plumbing is in dnd.rs.

use crate::{
    App, APP, CLIP_PAGE, FRAME_MS, HOLD_TIMER_ID, TIMER_ID, dnd, pill_left, rect, render, set_target, spring::Spring, text, text_in,
};
use crate::clipboard::{DateRange, Kind, Press, ROWS, Stamp, when_label};
use windows::Win32::{
    Foundation::*,
    Graphics::Direct2D::{Common::*, *},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

pub const CLIP_TIMER_ID: usize = 8; // debounce: the clipboard changed, read it once the copying app has let go
pub const PASTE_TIMER_ID: usize = 9; // send Ctrl+V a moment after we set the clipboard
pub const WM_CLIPBOARDUPDATE: u32 = 0x031D;
pub const ID_CLIP_DELETE: usize = 3;
pub const ID_CLIP_CLEAR: usize = 4;
const CLIP_DEBOUNCE_MS: u32 = 80;
const CLIP_RETRY_MS: u32 = 120;
const CLIP_RETRIES: u8 = 4;
const PASTE_DELAY_MS: u32 = 150; // the target window needs a moment to take focus back
const DROP_HOLD_MS: u32 = 2000; // after a drop, keep the notch open this long so the new item can be seen
const DRAG_START_PX: i32 = 4;
const OWN_WRITE_WINDOW: std::time::Duration = std::time::Duration::from_millis(600); // clipboard updates this soon after our own write are ours

// Layout in logical px: (left, top, right, bottom) from the pill's left edge. The right 30 px belong to the page dots.
const SEARCH: (f32, f32, f32, f32) = (16.0, 10.0, 266.0, 34.0);
const CHIP: (f32, f32, f32, f32) = (272.0, 10.0, 386.0, 34.0);
const LIST_TOP: f32 = 40.0;
const LIST_L: f32 = 12.0;
const LIST_R: f32 = 388.0;
const ROW_H: f32 = 22.0;
const QUERY_TAIL_CHARS: usize = 30; // longer queries show their end, where the caret is

enum Hit {
    Search,
    Chip,
    Row(usize), // visible row, 0 = first on screen
    Nothing,
}

fn box_rect(x: f32, s: f32, b: (f32, f32, f32, f32)) -> D2D_RECT_F {
    rect(x + b.0 * s, b.1 * s, x + b.2 * s, b.3 * s)
}

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }
}

fn tail(s: &str, n: usize) -> String {
    let skip = s.chars().count().saturating_sub(n);
    s.chars().skip(skip).collect()
}

/// The page is on screen, settled, and nothing (an agent popup) covers it.
fn live(app: &App) -> bool {
    app.spring.pos >= 0.9 && !app.show_event && app.page == CLIP_PAGE && app.page_spring.pos >= CLIP_PAGE as f32 - 0.05
}

fn list_scroll(app: &App, len: usize) -> usize {
    app.clip.scroll.min(len.saturating_sub(ROWS))
}

/// The id of the item shown in visible row `row`.
fn row_item(app: &App, row: usize) -> Option<u64> {
    let list = app.clip.store.matches(&app.clip.search.text, app.clip.range, Stamp::now().days());
    list.get(list_scroll(app, list.len()) + row).map(|i| i.id)
}

fn first_match(app: &App) -> Option<u64> {
    app.clip.store.matches(&app.clip.search.text, app.clip.range, Stamp::now().days()).first().map(|i| i.id)
}

fn hit(app: &App, x: i32, y: i32) -> Hit {
    if !live(app) {
        return Hit::Nothing;
    }
    let (s, left) = (app.scale, pill_left(app));
    let (px, py) = (x as f32, y as f32);
    let inside = |b: (f32, f32, f32, f32)| px >= left + b.0 * s && px < left + b.2 * s && py >= b.1 * s && py < b.3 * s;
    if inside(SEARCH) {
        Hit::Search
    } else if inside(CHIP) {
        Hit::Chip
    } else if inside((LIST_L, LIST_TOP, LIST_R, LIST_TOP + ROWS as f32 * ROW_H)) {
        Hit::Row(((py / s - LIST_TOP) / ROW_H) as usize)
    } else {
        Hit::Nothing
    }
}

pub unsafe fn draw(app: &App, x: f32, w: f32, s: f32, a: f32) {
    unsafe {
        let v = &app.clip;
        let today = Stamp::now().days();
        let list = v.store.matches(&v.search.text, v.range, today);

        // Search box.
        let b = box_rect(x, s, SEARCH);
        let field = rounded(b, 12.0 * s);
        app.white.SetOpacity(a * 0.08);
        app.rt.FillRoundedRectangle(&field, &app.white);
        app.white.SetOpacity(a * if v.typing { 0.55 } else { 0.12 });
        app.rt.DrawRoundedRectangle(&field, &app.white, s, None);
        text(app, &app.icons, "\u{E721}", &rect(b.left + 4.0 * s, b.top, b.left + 26.0 * s, b.bottom), a * 0.6); // magnifier
        let (shown, dim) = if v.typing {
            (format!("{}|", tail(&v.search.text, QUERY_TAIL_CHARS)), 1.0)
        } else if v.search.text.is_empty() {
            ("Search clipboard".to_string(), 0.45)
        } else {
            (tail(&v.search.text, QUERY_TAIL_CHARS), 1.0)
        };
        text(app, &app.row, &shown, &rect(b.left + 30.0 * s, b.top, b.right - 8.0 * s, b.bottom), a * dim);

        // Date filter chip: click to step through the ranges; tinted while a filter is on.
        let c = box_rect(x, s, CHIP);
        let chip = rounded(c, 12.0 * s);
        let filtered = v.range != DateRange::Any;
        let brush = if filtered { &app.accent } else { &app.white };
        brush.SetOpacity(a * if filtered { 0.22 } else { 0.08 });
        app.rt.FillRoundedRectangle(&chip, brush);
        brush.SetOpacity(a * if filtered { 0.7 } else { 0.12 });
        app.rt.DrawRoundedRectangle(&chip, brush, s, None);
        text_in(app, brush, &app.center, v.range.label(), &c, a);

        if list.is_empty() {
            let msg = if v.store.is_empty() { "Copy something and it shows up here" } else { "Nothing matches" };
            text(app, &app.small, msg, &rect(x + 24.0 * s, 64.0 * s, x + 380.0 * s, 100.0 * s), a * 0.5);
            return;
        }
        let scroll = list_scroll(app, list.len());
        for (i, item) in list.iter().skip(scroll).take(ROWS).enumerate() {
            let top = (LIST_TOP + ROW_H * i as f32) * s;
            let bottom = top + ROW_H * s;
            if v.hover == Some(i) {
                app.white.SetOpacity(a * 0.10);
                app.rt.FillRoundedRectangle(&rounded(rect(x + LIST_L * s, top, x + LIST_R * s, bottom), 8.0 * s), &app.white);
            }
            // Segoe MDL2: E8C8 copy, E8A5 page, E91B photo.
            let glyph = match item.kind {
                Kind::Files => "\u{E8A5}",
                Kind::Image => "\u{E91B}",
                Kind::Text | Kind::Other => "\u{E8C8}",
            };
            text(app, &app.icons, glyph, &rect(x + (LIST_L + 4.0) * s, top, x + (LIST_L + 26.0) * s, bottom), a * 0.7);
            text(app, &app.row, &item.preview, &rect(x + 40.0 * s, top, x + w - 130.0 * s, bottom), a);
            text(app, &app.stamp, &when_label(item.at, today), &rect(x + w - 128.0 * s, top, x + w - 34.0 * s, bottom), a * 0.5);
        }
        if list.len() > ROWS {
            // A thin scroll thumb in the gap before the page dots.
            let track = ROWS as f32 * ROW_H * s;
            let thumb = track * ROWS as f32 / list.len() as f32;
            let at = LIST_TOP * s + (track - thumb) * scroll as f32 / (list.len() - ROWS) as f32;
            app.white.SetOpacity(a * 0.3);
            app.rt.FillRoundedRectangle(&rounded(rect(x + w - 28.0 * s, at, x + w - 26.0 * s, at + thumb), s), &app.white);
        }
    }
}

fn redraw(hwnd: HWND) {
    APP.with(|a| {
        if let Some(app) = a.borrow().as_ref() {
            let _ = render(hwnd, app);
        }
    });
}

/// Hover highlight, and the start of a drag-out once a pressed row has been pulled a few pixels.
/// True if a drag ran (the caller has nothing more to do for this mouse move).
pub fn on_mouse_move(hwnd: HWND, lp: LPARAM, left_down: bool) -> bool {
    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
    let dragged = APP.with(|a| {
        let mut a = a.borrow_mut();
        let app = a.as_mut()?;
        let now = match hit(app, x, y) {
            Hit::Row(i) if row_item(app, i).is_some() => Some(i),
            _ => None,
        };
        if app.clip.hover != now {
            app.clip.hover = now;
            let _ = render(hwnd, app);
        }
        let press = app.clip.press?;
        if !left_down {
            app.clip.press = None;
            return None;
        }
        if (x - press.x).abs() < DRAG_START_PX && (y - press.y).abs() < DRAG_START_PX {
            return None;
        }
        app.clip.press = None;
        app.clip.store.get(press.id).map(|i| i.formats.clone())
    });
    let Some(formats) = dragged else { return false };
    dnd::drag(formats); // modal; runs with no APP borrow held
    // The drop happened somewhere else: if the cursor is no longer on the notch, let it collapse.
    let mut at = POINT::default();
    let over_us = unsafe { GetCursorPos(&mut at).is_ok() && WindowFromPoint(at) == hwnd };
    if !over_us {
        APP.with(|a| a.borrow_mut().as_mut().map(|app| app.tracking = false));
        set_target(hwnd, 0.0);
    }
    true
}

pub fn on_button_down(x: i32, y: i32) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            if let Hit::Row(i) = hit(app, x, y) {
                app.clip.press = row_item(app, i).map(|id| Press { id, x, y });
            }
        }
    });
}

enum Click {
    Paste(u64),
    FocusSearch,
    Handled,
    Ignored,
}

/// A button release on the page. True if the click belonged to it.
pub fn on_click(hwnd: HWND, x: i32, y: i32) -> bool {
    let click = APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return Click::Ignored };
        let pressed = app.clip.press.take();
        match hit(app, x, y) {
            Hit::Row(i) => match (row_item(app, i), pressed) {
                (Some(id), Some(p)) if p.id == id => Click::Paste(id),
                _ => Click::Handled,
            },
            Hit::Search => Click::FocusSearch,
            Hit::Chip => {
                app.clip.range = app.clip.range.next();
                app.clip.scroll = 0;
                let _ = render(hwnd, app);
                Click::Handled
            }
            Hit::Nothing => Click::Ignored,
        }
    });
    match click {
        Click::Paste(id) => paste(hwnd, id),
        Click::FocusSearch => focus_search(hwnd),
        Click::Handled => {}
        Click::Ignored => return false,
    }
    true
}

/// Give the search box the keyboard. The notch window never takes focus on its own, so ask for it here.
fn focus_search(hwnd: HWND) {
    let prev = unsafe { GetForegroundWindow() };
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.clip.typing = true;
            app.clip.prev_fg = if prev == hwnd { app.clip.prev_fg } else { prev.0 as isize };
            let _ = render(hwnd, app);
        }
    });
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        if GetForegroundWindow() != hwnd {
            // Windows refused: don't pretend to be typing (nothing would ever clear it).
            APP.with(|a| a.borrow_mut().as_mut().map(|app| app.clip.typing = false));
            redraw(hwnd);
        }
    }
}

fn refocus(window: isize) {
    let h = HWND(window as *mut _);
    if window != 0 && unsafe { IsWindow(Some(h)).as_bool() } {
        unsafe { let _ = SetForegroundWindow(h); }
    }
}

/// Leave the search box. The notch collapses if the mouse is not on it.
fn stop_typing(hwnd: HWND, give_focus_back: bool) {
    let (prev, away) = APP.with(|a| match a.borrow_mut().as_mut() {
        Some(app) => {
            app.clip.typing = false;
            (std::mem::take(&mut app.clip.prev_fg), !app.tracking)
        }
        None => (0, false),
    });
    if give_focus_back {
        refocus(prev);
    }
    if away {
        set_target(hwnd, 0.0);
    } else {
        redraw(hwnd);
    }
}

pub fn is_typing() -> bool {
    APP.with(|a| a.borrow().as_ref().is_some_and(|app| app.clip.typing))
}

pub fn on_kill_focus(hwnd: HWND) {
    let typing = APP.with(|a| a.borrow().as_ref().is_some_and(|app| app.clip.typing));
    if typing {
        stop_typing(hwnd, false);
    }
}

/// A character typed while the search box has focus (WM_CHAR delivers UTF-16 units).
pub fn on_char(hwnd: HWND, unit: u16) {
    enum Next {
        Redraw,
        Paste(Option<u64>),
        Stop,
    }
    let next = APP.with(|a| {
        let mut a = a.borrow_mut();
        let app = a.as_mut()?;
        if !app.clip.typing {
            return None;
        }
        Some(match unit {
            0x1B => {
                app.clip.search.clear(); // Esc
                Next::Stop
            }
            0x0D => Next::Paste(first_match(app)), // Enter pastes the top result
            0x08 => {
                app.clip.search.backspace();
                app.clip.scroll = 0;
                Next::Redraw
            }
            u if u >= 0x20 && u != 0x7F => {
                app.clip.search.push_unit(u);
                app.clip.scroll = 0;
                Next::Redraw
            }
            _ => return None,
        })
    });
    match next {
        Some(Next::Redraw) => redraw(hwnd),
        Some(Next::Paste(Some(id))) => paste(hwnd, id),
        Some(Next::Paste(None)) | None => {}
        Some(Next::Stop) => stop_typing(hwnd, true),
    }
}

/// Wheel over the list. True if it scrolled (otherwise the wheel changes page).
pub fn on_wheel(hwnd: HWND, down: bool) -> bool {
    APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return false };
        if !live(app) {
            return false;
        }
        let len = app.clip.store.matches(&app.clip.search.text, app.clip.range, Stamp::now().days()).len();
        let moved = app.clip.scroll_by(down, len);
        if moved {
            app.clip.hover = None;
            let _ = render(hwnd, app);
        }
        moved
    })
}

/// Put an item on the system clipboard and press Ctrl+V in whatever window had focus.
fn paste(hwnd: HWND, id: u64) {
    let (formats, back_to, was_typing_away) = APP.with(|a| match a.borrow_mut().as_mut() {
        Some(app) => {
            let formats = app.clip.store.get(id).map(|i| i.formats.clone());
            let typing = std::mem::replace(&mut app.clip.typing, false);
            let back_to = if typing { std::mem::take(&mut app.clip.prev_fg) } else { 0 };
            (formats, back_to, typing && !app.tracking)
        }
        None => (None, 0, false),
    });
    let Some(formats) = formats else { return };
    refocus(back_to); // the search box had the focus: hand it back before pasting
    if dnd::set_clipboard(formats) {
        // Our own write must not come back as a new copy (it would jump to the top with a new time).
        APP.with(|a| a.borrow_mut().as_mut().map(|app| app.clip.own_write = Some(std::time::Instant::now())));
        unsafe { SetTimer(Some(hwnd), PASTE_TIMER_ID, PASTE_DELAY_MS, None) };
    }
    redraw(hwnd);
    if was_typing_away {
        set_target(hwnd, 0.0); // the mouse is not on the notch, so nothing else would close it
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 },
        },
    }
}

pub fn on_paste_timer(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(Some(hwnd), PASTE_TIMER_ID);
        let keys = [key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)];
        SendInput(&keys, size_of::<INPUT>() as i32);
    }
}

/// Something was copied somewhere: read it shortly, once the app that copied it has let go of the clipboard.
pub fn on_clipboard_update(hwnd: HWND) {
    let ours = APP.with(|a| {
        a.borrow_mut().as_mut().is_some_and(|app| {
            app.clip.retries = 0;
            app.clip.own_write.is_some_and(|t| t.elapsed() < OWN_WRITE_WINDOW)
        })
    });
    if ours {
        return;
    }
    unsafe { SetTimer(Some(hwnd), CLIP_TIMER_ID, CLIP_DEBOUNCE_MS, None) };
}

pub fn on_clip_timer(hwnd: HWND) {
    unsafe { let _ = KillTimer(Some(hwnd), CLIP_TIMER_ID); }
    let snapshot = dnd::clipboard_snapshot(); // may call into the copying app: no APP borrow held
    let retry = APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return false };
        match snapshot {
            Some(formats) => {
                if app.clip.store.add(formats, Stamp::now()) {
                    app.clip.scroll = 0;
                    if live(app) {
                        let _ = render(hwnd, app);
                    }
                }
                false
            }
            // Busy, empty or private: look again a few times, then give up on this change.
            None => {
                app.clip.retries += 1;
                app.clip.retries <= CLIP_RETRIES
            }
        }
    });
    if retry {
        unsafe { SetTimer(Some(hwnd), CLIP_TIMER_ID, CLIP_RETRY_MS, None) };
    }
}

/// Something is being dragged over the notch (enter) or left it without dropping (leave).
pub fn drag_hover(hwnd: HWND, entering: bool) {
    if !entering {
        let away = APP.with(|a| a.borrow().as_ref().is_some_and(|app| !app.tracking));
        if away {
            set_target(hwnd, 0.0);
        }
        return;
    }
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.show_event = false; // show the page, not a popup
            app.notice = None;
            app.page = CLIP_PAGE;
            app.page_spring = Spring { pos: CLIP_PAGE as f32, vel: 0.0 }; // open already in place
        }
    });
    unsafe { let _ = KillTimer(Some(hwnd), HOLD_TIMER_ID); }
    set_target(hwnd, 1.0);
}

/// Something was dropped on the notch: it goes into the history (and onto the page) like a copy.
pub fn dropped(hwnd: HWND, formats: Vec<crate::clipboard::Format>) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.clip.store.add(formats, Stamp::now());
            app.clip.scroll = 0;
            app.last_tick = std::time::Instant::now();
            unsafe { SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None) };
            let _ = render(hwnd, app);
        }
    });
    unsafe { SetTimer(Some(hwnd), HOLD_TIMER_ID, DROP_HOLD_MS, None) }; // then it collapses like any popup
}

/// What a right-click at (x, y) offers: the item under it, and whether "clear all" makes sense.
pub fn menu_context(x: i32, y: i32) -> (Option<u64>, bool) {
    APP.with(|a| match a.borrow().as_ref() {
        Some(app) => {
            let item = match hit(app, x, y) {
                Hit::Row(i) => row_item(app, i),
                _ => None,
            };
            (item, live(app) && !app.clip.store.is_empty())
        }
        None => (None, false),
    })
}

pub fn delete(hwnd: HWND, id: u64) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.clip.store.remove(id);
            app.clip.hover = None;
            let _ = render(hwnd, app);
        }
    });
}

pub fn clear(hwnd: HWND) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.clip.store.clear();
            app.clip.scroll = 0;
            app.clip.hover = None;
            let _ = render(hwnd, app);
        }
    });
}
