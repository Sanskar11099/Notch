#![windows_subsystem = "windows"]
// Stage 1: notch window (idle <-> expanded on hover). No render loop at idle: the 16ms timer
// only runs while the spring is moving, and the window is redrawn only on timer ticks.

mod art;
mod autostart;
mod clipboard;
mod clippage;
mod devices;
mod dnd;
mod media;
mod netlink;
mod notice;
mod privacy;
mod server;
mod spring;
mod sysinfo;
mod volume;
mod watchers;
mod weather;

use media::{Cmd, MediaInfo};
use server::{Event, Status};
use netlink::LinkKind;
use sysinfo::{Sampler, Stats};
use spring::Spring;
use std::{cell::RefCell, collections::HashMap, time::{Duration, Instant}};
use windows_numerics::{Matrix3x2, Vector2};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_WORD_WRAPPING_NO_WRAP, DWriteCreateFactory,
                IDWriteFactory, IDWriteTextFormat,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::*,
        },
        Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC},
        System::{
            DataExchange::AddClipboardFormatListener,
            LibraryLoader::GetModuleHandleW,
            Ole::OleInitialize,
            Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification},
            Threading::{GetCurrentProcess, SetProcessWorkingSetSize},
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::*,
        },
    },
    core::*,
};

// Logical (96 dpi) sizes; scaled by system DPI at startup.
const WIN_SIZE: (f32, f32) = (440.0, 160.0);
const IDLE_SIZE: (f32, f32) = (170.0, 32.0);
const COMPACT_W: f32 = 220.0; // collapsed-pill width while music plays (room for thumbnail + bars)
const OPEN_SIZE: (f32, f32) = (420.0, 140.0);
const ART_SIZE: f32 = 64.0;
const THUMB_SIZE: f32 = 22.0;
// Transport buttons: pitch = centre-to-centre spacing; top is measured from the screen edge.
const BTN_PITCH: f32 = 56.0;
const BTN_W: f32 = 48.0;
const BTN_TOP: f32 = 96.0;
const BTN_H: f32 = 32.0;
// Music page, transport row: the buttons sit on the left, mute + volume slider on the right (offsets from the
// pill's left edge; the slider ends VOL_TRACK_RIGHT_PAD short of the right edge, where the page dots live).
const TRANSPORT_CX: f32 = 100.0;
const VOL_ICON_L: f32 = 214.0;
const VOL_ICON_R: f32 = 242.0;
const VOL_TRACK_L: f32 = 252.0;
const VOL_TRACK_RIGHT_PAD: f32 = 38.0;
// Music page, progress row: under the artist, right of the album art. Elapsed time | bar | total time.
const PROG_CY: f32 = 78.0;
const PROG_LABEL_L: f32 = 98.0;
const PROG_BAR_L: f32 = 138.0;
const PROG_BAR_R: f32 = 350.0;
const SEEK_HOLD: Duration = Duration::from_millis(1500);
const IDLE_RADIUS: f32 = 14.0;
const OPEN_RADIUS: f32 = 30.0;
const FRAME_MS: u32 = 16;
const TIMER_ID: usize = 1;
const HOLD_TIMER_ID: usize = 2;
const EQ_TIMER_ID: usize = 3;
const SYS_TIMER_ID: usize = 4;
const SYS_FIRST_MS: u32 = 300; // first CPU reading needs a second sample, so take it soon after opening
const SYS_MS: u32 = 1000;
const DEVICES_MS: u32 = 3000; // temperature and Bluetooth batteries change slowly: no need to read them every second
const ROTATE_TIMER_ID: usize = 10; // collapsed pill: step to the next running agent
const ROTATE_MS: u32 = 3000;
const WEATHER_TIMER_ID: usize = 7; // refresh the weather: every 30 min, every 2 min until the first success
const WEATHER_MS: u32 = 30 * 60 * 1000;
const WEATHER_RETRY_MS: u32 = 2 * 60 * 1000; // no network yet at login, or location off: try again soon
const GLANCE_W: f32 = 290.0; // peek-pill width when the glance strip also shows the weather
const BT_TIMER_ID: usize = 5; // debounce: device changes come in bursts, compare Bluetooth once they settle
const NET_TIMER_ID: usize = 6; // debounce for network interface changes
const BT_DEBOUNCE_MS: u32 = 800;
const NET_DEBOUNCE_MS: u32 = 1500;
const DBT_DEVNODES_CHANGED: usize = 0x0007; // WM_DEVICECHANGE: the device tree changed (includes Bluetooth connects)
const PBT_APMPOWERSTATUSCHANGE: usize = 0x000A; // WM_POWERBROADCAST: AC/battery state or level changed
const PBT_POWERSETTINGCHANGE: usize = 0x8013; // WM_POWERBROADCAST: a power setting we registered for changed
// GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS: notifies (payload = percent) whenever the current monitor brightness changes.
const GUID_BRIGHTNESS: GUID = GUID::from_u128(0x8FFEE2C6_2D01_46BE_ADB9_398ADDC5B4FF);
const GLANCE_PAGE: u8 = 250; // pseudo page id: the sys timer is sampling for the peek view's glance strip
const GLANCE_MS: u32 = 2000;
const WARN_PCT: f32 = 85.0; // CPU/memory/disk bars turn orange at this load
const TEMP_WARN_C: f32 = 85.0; // thermometer and number turn orange at this temperature
// Devices page widget layout (logical px inside the 420x140 pill): three tiles, leaving the right edge for the dots.
const TILE_LEFT: f32 = 16.0;
const TILE_TOP: f32 = 12.0;
const TILE_H: f32 = 116.0;
const TILE_GAP: f32 = 8.0;
const TEMP_TILE_W: f32 = 96.0;
const NET_TILE_W: f32 = 118.0;
const BT_TILE_W: f32 = 144.0;
const BT_RINGS: usize = 3; // battery rings shown before "+N"
const WEAK_SIGNAL_PCT: u8 = 30; // Wi-Fi signal quality below this tints the icon orange
// Hidden mode: with nothing to show the notch slides above the screen edge, leaving only this nearly
// invisible strip at the top-centre (still mouse-hit-testable) so hovering it can bring the notch back.
const HOT_STRIP_W: f32 = 200.0;
const HOT_STRIP_H: f32 = 6.0;
const HOT_STRIP_ALPHA: f32 = 0.008; // ~2/255: enough for Windows to route the mouse to us, too faint to see
const ROW_H: f32 = 19.0; // system page row height (logical px): six rows must fit in the 140 px pill
const ROW_TOP: f32 = 12.0;
// 0 = music, 1 = widgets, 2 = system, 3 = agents (live preview of running work), 4 = clipboard history
const PAGES: u8 = 5;
const AGENTS_PAGE: u8 = 3;
const CLIP_PAGE: u8 = 4;
const DOT_RIGHT: f32 = 12.0; // page dots: distance from the pill's right edge, first dot's y, spacing
const DOT_TOP: f32 = 38.0; // five dots centred on the 140 px pill: 38, 54, 70, 86, 102
const DOT_GAP: f32 = 16.0;
const EQ_MS: u32 = 167; // ~6 fps: still reads as motion; measured only ~16% cheaper than 10 fps (most cost is fixed per tick)
// Equalizer bars: height swings between MIN and MAX logical px; each bar has its own speed (Hz) and phase.
const EQ_MIN: f32 = 4.0;
const EQ_MAX: f32 = 14.0;
const EQ_HZ: [f32; 3] = [1.3, 1.9, 1.6];
const EQ_PHASE: [f32; 3] = [0.0, 2.1, 4.2];
const HOLD_MS: u32 = 3000; // how long a done/attention event keeps the notch open
const TEXT_FADE_START: f32 = 0.6; // text appears once the pill is 60% open
const MAX_SOURCES: usize = 64;
const ID_QUIT: usize = 1;
const ID_AUTOSTART: usize = 2;
// An agent that goes quiet without reporting done/attention (crashed, killed) stops counting as working this long
// after its last sign of life.
const WORKING_TTL: Duration = Duration::from_secs(30 * 60);
// Glass: a dark blue-grey tint the desktop faintly shows through (alpha), a white sheen fading from the
// top, and a pale hairline edge. Raise GLASS_TINT.a for legibility over bright windows, lower it for more glass.
const GLASS_TINT: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.05, g: 0.06, b: 0.09, a: 0.70 };
const SHEEN_TOP: f32 = 0.16;
const EDGE_ALPHA: f32 = 0.22;
const GOOD: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.30, g: 0.85, b: 0.50, a: 1.0 }; // battery bar while power is connected
const ACCENT: D2D1_COLOR_F = D2D1_COLOR_F { r: 1.0, g: 0.62, b: 0.2, a: 1.0 };
const WM_MOUSELEAVE: u32 = 0x02A3; // lives in Win32_UI_Controls; not worth a feature for one constant

struct Working {
    label: String, // agent, project and app for this run, e.g. "claude · my-app · VS Code" (agents page)
    short: String, // what the narrow collapsed pill shows, e.g. "claude · VS Code"
    since: Instant,
    last_activity: Instant, // when the newest step arrived (picks which run the agents page follows)
    waiting: bool,        // finished its turn and needs the user's review/input (its dot turns green)
}

struct App {
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    white: ID2D1SolidColorBrush,
    text: IDWriteTextFormat,
    small: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    event: Option<Event>,
    show_event: bool, // event text takes over the open pill until it collapses (or the user hovers on)
    accent: ID2D1SolidColorBrush,
    good: ID2D1SolidColorBrush,
    sheen: ID2D1LinearGradientBrush,
    badge: IDWriteTextFormat,
    factory: ID2D1Factory,        // for the ring arcs
    ring_style: ID2D1StrokeStyle, // round caps for the battery rings
    big: IDWriteTextFormat,       // widget numbers (temperature)
    center: IDWriteTextFormat,    // centred in the battery rings
    tiny: IDWriteTextFormat,      // device names under the rings
    label: IDWriteTextFormat, // system page: row names (left, vertically centred)
    value: IDWriteTextFormat, // system page: row values (right-aligned)
    row: IDWriteTextFormat,   // clipboard page: one-line list text (no wrapping, so long copies are clipped, not stacked)
    stamp: IDWriteTextFormat, // clipboard page: when it was copied (small, right-aligned)
    clip: clipboard::View,    // clipboard history and what the user is doing with it
    page: u8,                 // expanded view: 0 = music, 1 = widgets, 2 = system info, 3 = agents, 4 = clipboard (popups override all)
    page_spring: Spring,      // animated scroll position between pages (settles on `page`)
    stats: Stats,
    sampler: Sampler,
    sys_page: Option<u8>,     // the stats page SYS_TIMER_ID is sampling for; None = timer off
    volume: Option<volume::VolumeState>, // last system volume we read (None = no audio output)
    vol_drag: bool,           // the slider is being dragged (mouse captured)
    privacy: privacy::Privacy, // microphone / camera currently in use (any app)
    notice: Option<notice::Notice>, // a system popup being shown (replaces the agent popup text)
    power_prev: notice::Power,      // last readings, kept to spot what changed
    bt_prev: Vec<devices::BtDevice>,
    net_prev: Option<netlink::NetLink>,
    vol_prev: Option<volume::VolumeState>,
    brightness_prev: Option<u8>, // None until the first notification, which is only a baseline (no popup)
    weather: Option<weather::Weather>, // last good reading, kept if a refresh fails
    icon_big: IDWriteTextFormat, // large Segoe MDL2 icon in popups
    timeline: Option<media::Timeline>, // where the current track is (None = nothing playing / no timeline)
    timeline_at: Instant,     // when `timeline` was read, so the bar can keep moving between reads
    seek_hold: Instant,       // after a click-to-seek, ignore re-reads until then (the player applies it a moment later)
    working: HashMap<String, Working>, // runs currently working, keyed by Event::key() (agent + session)
    sources: HashMap<String, isize>,   // run key -> window to focus (captured at its last `working`)
    focus: isize,                      // window the current event's click should focus
    media: Option<MediaInfo>,
    art: Option<ID2D1Bitmap>,
    dc: HDC,
    size: (i32, i32),
    scale: f32,
    spring: Spring,
    target: f32,
    compact: Spring, // 0 = plain idle width, 1 = widened for the compact music state
    compact_target: f32,
    reveal: Spring, // 0 = hidden above the screen edge, 1 = slid into view
    reveal_target: f32,
    rotate: usize,        // which running agent the collapsed pill names (counts up; taken modulo how many are running)
    rotate_running: bool, // ROTATE_TIMER_ID is set
    eq_running: bool, // EQ_TIMER_ID is set; tracked so we only (re)arm it on a real state change
    eq_start: Instant,
    tracking: bool,
    last_tick: Instant,
}

thread_local!(static APP: RefCell<Option<App>> = const { RefCell::new(None) });

// Hands idle pages back to the OS; they fault back in on demand. Cuts the visible RAM footprint.
fn trim_memory() {
    unsafe { let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX); }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn render(hwnd: HWND, app: &App) -> Result<()> {
    render_with(hwnd, app, false)
}

// `partial` limits drawing and compositing to the pill's own rectangle. Only valid when the pill
// size is unchanged since the last full render (equalizer ticks), otherwise old pixels would linger.
fn render_with(hwnd: HWND, app: &App, partial: bool) -> Result<()> {
    let t = app.spring.pos.clamp(0.0, 1.06);
    let s = app.scale;
    let (win_w, win_h) = (app.size.0 as f32, app.size.1 as f32);
    let compact_w = if glance_wanted(app) && app.weather.is_some() { GLANCE_W } else { COMPACT_W };
    let idle_w = lerp(IDLE_SIZE.0, compact_w, app.compact.pos.clamp(0.0, 1.06));
    let w = (lerp(idle_w, OPEN_SIZE.0, t) * s).min(win_w);
    let h = (lerp(IDLE_SIZE.1, OPEN_SIZE.1, t) * s).min(win_h);
    let r = lerp(IDLE_RADIUS, OPEN_RADIUS, t.min(1.0)) * s;
    let x = (win_w - w) / 2.0;

    unsafe {
        let bounds = RECT { left: 0, top: 0, right: app.size.0, bottom: app.size.1 };
        app.rt.BindDC(app.dc, &bounds)?;
        app.rt.BeginDraw();
        let area = rect(
            (x - 2.0).floor().max(0.0), 0.0,
            (x + w + 2.0).ceil().min(win_w), (h + 2.0).ceil().min(win_h),
        );
        if partial {
            app.rt.PushAxisAlignedClip(&area, D2D1_ANTIALIAS_MODE_ALIASED);
        }
        app.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        // Hidden mode: everything below is drawn shifted up by `lift` (negative), so the notch rides up out of
        // the top of the screen as `reveal` falls to 0 and comes back down as it rises to 1.
        let lift = -(1.0 - app.reveal.pos.clamp(0.0, 1.06)) * (h + 4.0 * s);
        app.rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: lift });
        // Top edge sits r above the screen so only the bottom corners read as rounded (flush notch).
        let pill = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: -r, right: x + w, bottom: h },
            radiusX: r,
            radiusY: r,
        };
        draw_glass(app, &pill, s, h);
        let _ = draw_working(app, x, w, h, s, t);
        let _ = draw_compact_music(app, x, w, h, s, t);
        draw_privacy(app, x, w, h, s, t);
        draw_glance(app, x, h, s, t);
        if t > TEXT_FADE_START {
            let alpha = ((t - TEXT_FADE_START) / (1.0 - TEXT_FADE_START)).min(1.0);
            let _ = draw_content(app, x, w, h, s, alpha, lift);
        }
        app.rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
        if app.reveal.pos < 0.02 {
            // Fully hidden: leave a barely-there strip so the cursor reaching the top edge can find us.
            app.white.SetOpacity(HOT_STRIP_ALPHA);
            let half = HOT_STRIP_W * s / 2.0;
            app.rt.FillRectangle(&rect(win_w / 2.0 - half, 0.0, win_w / 2.0 + half, HOT_STRIP_H * s), &app.white);
        }
        if partial {
            app.rt.PopAxisAlignedClip();
        }
        app.rt.EndDraw(None, None)?;

        let dst = POINT { x: (GetSystemMetrics(SM_CXSCREEN) - app.size.0) / 2, y: 0 };
        let size = SIZE { cx: app.size.0, cy: app.size.1 };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if partial {
            let dirty = RECT { left: area.left as i32, top: 0, right: area.right as i32, bottom: area.bottom as i32 };
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: HDC::default(),
                pptDst: &dst,
                psize: &size,
                hdcSrc: app.dc,
                pptSrc: &src,
                crKey: COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: &dirty,
            };
            return UpdateLayeredWindowIndirect(hwnd, &info).ok();
        }
        UpdateLayeredWindow(
            hwnd, None, Some(&dst), Some(&size), Some(app.dc), Some(&src),
            COLORREF(0), Some(&blend), ULW_ALPHA,
        )
    }
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

unsafe fn text(app: &App, fmt: &IDWriteTextFormat, s: &str, area: &D2D_RECT_F, opacity: f32) {
    unsafe { text_in(app, &app.white, fmt, s, area, opacity) }
}

unsafe fn text_in(app: &App, brush: &ID2D1SolidColorBrush, fmt: &IDWriteTextFormat, s: &str, area: &D2D_RECT_F, opacity: f32) {
    unsafe {
        brush.SetOpacity(opacity);
        let wide: Vec<u16> = s.encode_utf16().collect();
        app.rt.DrawText(&wide, fmt, area, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }
}

fn button_rects(cx: f32, s: f32) -> [D2D_RECT_F; 3] {
    [-1.0f32, 0.0, 1.0].map(|i| {
        let c = cx + i * BTN_PITCH * s;
        rect(c - BTN_W * s / 2.0, BTN_TOP * s, c + BTN_W * s / 2.0, (BTN_TOP + BTN_H) * s)
    })
}

/// Left edge of the fully open pill, in window coordinates.
fn pill_left(app: &App) -> f32 {
    app.size.0 as f32 / 2.0 - OPEN_SIZE.0 * app.scale / 2.0
}

/// The music page's controls (transport, mute, volume) are live only while that page is fully in place.
fn music_controls_live(app: &App) -> bool {
    app.spring.pos >= 0.9 && !app.show_event && app.media.is_some() && app.page == 0 && app.page_spring.pos <= 0.05
}

/// Left and right ends of the volume slider track, in window coordinates.
fn volume_track(app: &App) -> (f32, f32) {
    let (s, left) = (app.scale, pill_left(app));
    (left + VOL_TRACK_L * s, left + (OPEN_SIZE.0 - VOL_TRACK_RIGHT_PAD) * s)
}

fn in_music_row(app: &App, y: i32) -> bool {
    let (top, bottom) = (BTN_TOP * app.scale, (BTN_TOP + BTN_H) * app.scale);
    (y as f32) >= top && (y as f32) < bottom
}

fn hit_volume_track(app: &App, x: i32, y: i32) -> bool {
    let (l, r) = volume_track(app);
    music_controls_live(app) && app.volume.is_some() && in_music_row(app, y) && (x as f32) >= l - 8.0 * app.scale && (x as f32) <= r + 8.0 * app.scale
}

fn hit_mute(app: &App, x: i32, y: i32) -> bool {
    let (s, left) = (app.scale, pill_left(app));
    music_controls_live(app)
        && app.volume.is_some()
        && in_music_row(app, y)
        && (x as f32) >= left + VOL_ICON_L * s
        && (x as f32) < left + VOL_ICON_R * s
}

/// Apply the slider position under the cursor as the system volume (which also unmutes, as Windows does).
fn set_volume_from_x(app: &mut App, x: f32) {
    let (l, r) = volume_track(app);
    let level = volume::slider_level(x, l, r);
    volume::set_level(level);
    let muted = app.volume.is_some_and(|v| v.muted) && level <= 0.0;
    app.volume = Some(volume::VolumeState { level, muted });
}

/// Track position to show now: the last read plus the time since, while playing (so the bar keeps moving
/// smoothly between the 1 s reads and during redraws caused by hovering).
fn shown_position(app: &App) -> u64 {
    let Some(tl) = app.timeline else { return 0 };
    let playing = app.media.as_ref().is_some_and(|m| m.playing);
    let extra = if playing { app.timeline_at.elapsed().as_millis() as u64 } else { 0 };
    (tl.position_ms + extra).min(if tl.duration_ms > 0 { tl.duration_ms } else { u64::MAX })
}

/// Re-read the track position. True if it differs from what is drawn (so a redraw is needed).
fn refresh_timeline(app: &mut App) -> bool {
    if Instant::now() < app.seek_hold {
        return true; // keep redrawing from our own (optimistic) position while the player catches up
    }
    let now = media::timeline();
    let changed = now != app.timeline;
    app.timeline = now;
    app.timeline_at = Instant::now();
    changed
}

/// Where along the progress bar (0..=1) the cursor is, if it is on a bar the player lets us seek in.
fn hit_progress(app: &App, x: i32, y: i32) -> Option<f32> {
    let tl = app.timeline.filter(|t| t.duration_ms > 0 && t.seekable)?;
    let _ = tl;
    let s = app.scale;
    let (l, r) = (pill_left(app) + PROG_BAR_L * s, pill_left(app) + PROG_BAR_R * s);
    let (px, py) = (x as f32, y as f32);
    let on_bar = music_controls_live(app) && py >= (PROG_CY - 9.0) * s && py <= (PROG_CY + 9.0) * s && px >= l - 4.0 * s && px <= r + 4.0 * s;
    on_bar.then(|| volume::slider_level(px, l, r))
}

/// Re-read the system volume (it can change from the keyboard keys). True if it differs from what is drawn.
fn refresh_volume(app: &mut App) -> bool {
    if app.vol_drag {
        return false; // don't fight the user's finger
    }
    let now = volume::read();
    let changed = now != app.volume;
    app.volume = now;
    changed
}

fn hit_button(app: &App, x: i32, y: i32) -> Option<Cmd> {
    if !music_controls_live(app) {
        return None;
    }
    let (px, py) = (x as f32, y as f32);
    button_rects(pill_left(app) + TRANSPORT_CX * app.scale, app.scale)
        .iter()
        .zip([Cmd::Prev, Cmd::Toggle, Cmd::Next])
        .find(|(r, _)| px >= r.left && px < r.right && py >= r.top && py < r.bottom)
        .map(|(_, cmd)| cmd)
}

unsafe fn draw_glass(app: &App, pill: &D2D1_ROUNDED_RECT, s: f32, h: f32) {
    unsafe {
        app.rt.FillRoundedRectangle(pill, &app.brush); // translucent tint (brush colour carries the alpha)
        app.sheen.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 });
        app.sheen.SetEndPoint(Vector2 { X: 0.0, Y: h });
        app.rt.FillRoundedRectangle(pill, &app.sheen);
        // Hairline edge, inset half a pixel so the stroke isn't clipped at the pill boundary.
        let (r, inset) = (pill.rect, 0.5 * s);
        let edge = D2D1_ROUNDED_RECT {
            rect: rect(r.left + inset, r.top, r.right - inset, r.bottom - inset),
            radiusX: pill.radiusX - inset,
            radiusY: pill.radiusY - inset,
        };
        app.white.SetOpacity(EDGE_ALPHA);
        app.rt.DrawRoundedRectangle(&edge, &app.white, s, None);
    }
}

/// Agent runs currently working, oldest first (a stable order, so the preview doesn't reshuffle every frame).
fn live_runs(app: &App) -> Vec<&Working> {
    let mut runs: Vec<&Working> = app.working.values().filter(|w| w.last_activity.elapsed() < WORKING_TTL).collect();
    runs.sort_by_key(|w| w.since);
    runs
}

// Every agent that is working, one row each: "claude · Project Notch · VS Code" with how long it has been going.
// A 1 s tick keeps the elapsed times moving while this page is open.
const AGENT_ROWS: usize = 5;
const AGENT_ROW_H: f32 = 22.0;

unsafe fn draw_agents(app: &App, x: f32, w: f32, s: f32, a: f32) {
    unsafe {
        let runs = live_runs(app);
        if runs.is_empty() {
            text(app, &app.text, "No agents working", &rect(x + 24.0 * s, 18.0 * s, x + w - 30.0 * s, 50.0 * s), a * 0.6);
            return;
        }
        // The last row turns into "+N more" when there are too many to list.
        let listed = if runs.len() > AGENT_ROWS { AGENT_ROWS - 1 } else { runs.len() };
        for (i, run) in runs.iter().take(listed).enumerate() {
            let top = (ROW_TOP + AGENT_ROW_H * i as f32) * s;
            let bottom = top + AGENT_ROW_H * s;
            let dot = D2D1_ELLIPSE { point: Vector2 { X: x + 30.0 * s, Y: (top + bottom) / 2.0 }, radiusX: 3.5 * s, radiusY: 3.5 * s };
            let dot_brush = if run.waiting { &app.good } else { &app.accent };
            dot_brush.SetOpacity(a);
            app.rt.FillEllipse(&dot, dot_brush);
            text(app, &app.row, &run.label, &rect(x + 42.0 * s, top, x + w - 110.0 * s, bottom), a);
            let area = rect(x + w - 108.0 * s, top, x + w - 34.0 * s, bottom);
            if run.waiting {
                text_in(app, &app.good, &app.stamp, "needs review", &area, a);
            } else {
                text(app, &app.stamp, &sysinfo::format_elapsed(run.since.elapsed().as_secs()), &area, a * 0.6);
            }
        }
        if runs.len() > listed {
            let top = (ROW_TOP + AGENT_ROW_H * listed as f32) * s;
            text(app, &app.row, &format!("+{} more", runs.len() - listed), &rect(x + 42.0 * s, top, x + w - 110.0 * s, top + AGENT_ROW_H * s), a * 0.6);
        }
    }
}

/// The agent the collapsed pill names right now. With several running it steps through them (see update_rotate_timer).
fn shown_run(app: &App) -> Option<&Working> {
    let runs = live_runs(app);
    (!runs.is_empty()).then(|| runs[app.rotate % runs.len()])
}

fn working_label(app: &App) -> Option<String> {
    shown_run(app).map(|r| r.short.clone())
}

/// Step the collapsed pill's label through the running agents while there is more than one to show.
fn update_rotate_timer(hwnd: HWND, app: &mut App) {
    let want = app.target == 0.0 && app.reveal_target > 0.0 && live_runs(app).len() > 1;
    if want == app.rotate_running {
        return;
    }
    app.rotate_running = want;
    unsafe {
        if want {
            SetTimer(Some(hwnd), ROTATE_TIMER_ID, ROTATE_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), ROTATE_TIMER_ID);
        }
    }
}

fn on_rotate_tick(hwnd: HWND) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.rotate = app.rotate.wrapping_add(1);
            update_rotate_timer(hwnd, app); // an agent may have timed out: stop if only one is left
            let _ = render(hwnd, app);
        }
    });
}

// Static dot + name in the collapsed pill. Fades out as the pill opens. No animation, so no timer.
unsafe fn draw_working(app: &App, x: f32, w: f32, h: f32, s: f32, t: f32) -> Result<()> {
    unsafe {
        let alpha = (1.0 - t / 0.4).clamp(0.0, 1.0);
        let Some(run) = shown_run(app).filter(|_| alpha > 0.0) else { return Ok(()) };
        let dot_brush = if run.waiting { &app.good } else { &app.accent }; // green: waiting for the user's review
        dot_brush.SetOpacity(alpha);
        let dot = D2D1_ELLIPSE { point: Vector2 { X: x + 16.0 * s, Y: h / 2.0 }, radiusX: 4.0 * s, radiusY: 4.0 * s };
        app.rt.FillEllipse(&dot, dot_brush);
        // Leave room on the right for the equalizer bars and the microphone/camera icons.
        let privacy_icons = app.privacy.mic as u8 + app.privacy.camera as u8;
        let right_pad = if is_playing(app) { 38.0 } else { 10.0 } + 22.0 * privacy_icons as f32;
        text(app, &app.badge, &run.short, &rect(x + 28.0 * s, 0.0, x + w - right_pad * s, h), alpha * 0.9);
        Ok(())
    }
}

fn is_playing(app: &App) -> bool {
    app.media.as_ref().is_some_and(|m| m.playing)
}

// A small battery drawn from shapes: outline, terminal cap, and a fill for the charge (green while on power,
// orange when low and unplugged).
unsafe fn draw_battery_icon(app: &App, left: f32, cy: f32, s: f32, a: f32, percent: u8, fill: Fill) {
    unsafe {
        let body = D2D1_ROUNDED_RECT { rect: rect(left, cy - 5.5 * s, left + 21.0 * s, cy + 5.5 * s), radiusX: 3.0 * s, radiusY: 3.0 * s };
        app.white.SetOpacity(a * 0.75);
        app.rt.DrawRoundedRectangle(&body, &app.white, 1.2 * s, None);
        app.rt.FillRectangle(&rect(left + 22.0 * s, cy - 2.0 * s, left + 24.0 * s, cy + 2.0 * s), &app.white);
        let inner_w = 17.0 * s * (percent.min(100) as f32 / 100.0);
        if inner_w >= 1.0 {
            let brush = fill_brush(app, fill);
            brush.SetOpacity(a * 0.95);
            let level = D2D1_ROUNDED_RECT { rect: rect(left + 2.0 * s, cy - 3.5 * s, left + 2.0 * s + inner_w, cy + 3.5 * s), radiusX: 1.5 * s, radiusY: 1.5 * s };
            app.rt.FillRoundedRectangle(&level, brush);
        }
    }
}

// The peek view's glance strip: time, battery, connection, CPU. Shown only while peeking with nothing else going on.
unsafe fn draw_glance(app: &App, x: f32, h: f32, s: f32, t: f32) {
    unsafe {
        let alpha = (1.0 - t / 0.4).clamp(0.0, 1.0);
        if alpha <= 0.0 || !glance_wanted(app) {
            return;
        }
        let cy = h / 2.0;
        let st = &app.stats;
        text(app, &app.badge, &sysinfo::clock_text(), &rect(x + 16.0 * s, 0.0, x + 92.0 * s, h), alpha);
        if let Some(b) = st.battery {
            let fill = match sysinfo::power_look(st.battery, st.on_ac) {
                sysinfo::PowerLook::Plugged => Fill::Good,
                sysinfo::PowerLook::Low => Fill::Warn,
                sysinfo::PowerLook::Normal => Fill::Normal,
            };
            draw_battery_icon(app, x + 100.0 * s, cy, s, alpha, b.percent, fill);
            text(app, &app.badge, &format!("{}%", b.percent), &rect(x + 128.0 * s, 0.0, x + 160.0 * s, h), alpha * 0.9);
        }
        // Connection: Segoe MDL2 E701 Wi-Fi, E839 Ethernet; nothing when offline or another kind.
        let glyph = match st.link.as_ref().map(|l| l.kind) {
            Some(LinkKind::WiFi) => "\u{E701}",
            Some(LinkKind::Ethernet) => "\u{E839}",
            _ => "",
        };
        text(app, &app.icons, glyph, &rect(x + 172.0 * s, 0.0, x + 198.0 * s, h), alpha * 0.9);
        if let Some(weather) = &app.weather {
            text(app, &app.badge, &weather.short_text(), &rect(x + 206.0 * s, 0.0, x + 280.0 * s, h), alpha * 0.9);
        }
    }
}

// Microphone / camera in use: an orange mic and/or green camera icon at the right of the collapsed pill (like the
// privacy dot on a phone). Segoe MDL2: E720 microphone, E714 video camera.
unsafe fn draw_privacy(app: &App, x: f32, w: f32, h: f32, s: f32, t: f32) {
    unsafe {
        let alpha = (1.0 - t / 0.4).clamp(0.0, 1.0);
        if alpha <= 0.0 || !app.privacy.any() {
            return;
        }
        let mut right = x + w - if is_playing(app) { 38.0 } else { 12.0 } * s;
        for (active, glyph, brush) in [
            (app.privacy.mic, "\u{E720}", &app.accent),
            (app.privacy.camera, "\u{E714}", &app.good),
        ] {
            if active {
                text_in(app, brush, &app.icons, glyph, &rect(right - 20.0 * s, 0.0, right, h), alpha);
                right -= 22.0 * s;
            }
        }
    }
}

// The bars only show while collapsed, so animate only then; killing the timer is what keeps pause/hover free.
fn update_eq_timer(hwnd: HWND, app: &mut App) {
    // Only while the collapsed notch is actually on screen: a hidden notch has nothing to animate.
    let want = is_playing(app) && app.target == 0.0 && app.reveal_target > 0.0;
    if want == app.eq_running {
        return;
    }
    app.eq_running = want;
    unsafe {
        if want {
            SetTimer(Some(hwnd), EQ_TIMER_ID, EQ_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), EQ_TIMER_ID);
        }
    }
}

// Pages 1 (devices widgets) and 2 (system info) need live stats; each samples only its own data.
fn sample_page(app: &mut App) {
    // What to sample follows the timer's mode (a stats page, or the glance strip), not just the current page.
    match app.sys_page.unwrap_or(app.page) {
        0 => {
            refresh_volume(app);
            refresh_timeline(app);
        }
        1 => app.stats = app.sampler.sample_devices(),
        2 => app.stats = app.sampler.sample_system(),
        GLANCE_PAGE => app.stats = app.sampler.sample_glance(),
        _ => {}
    }
}

// Stats are only sampled while a stats page is on screen; any other state kills the timer. Moving between the
// two stats pages changes `sys_page`, which restarts sampling for the new page.
fn update_sys_timer(hwnd: HWND, app: &mut App) {
    // Pages 1+ sample live stats; the music page (0) re-reads the volume so the slider follows the keyboard keys.
    // The clipboard page has nothing live to sample: it redraws when something happens.
    let wants_ticks = app.page != CLIP_PAGE && (app.page >= 1 || app.media.is_some());
    let want = if app.target == 1.0 && wants_ticks && !app.show_event {
        Some(app.page)
    } else if glance_wanted(app) {
        Some(GLANCE_PAGE) // the peek view's clock / CPU / battery / connection
    } else {
        None
    };
    if want == app.sys_page {
        return;
    }
    app.sys_page = want;
    unsafe {
        if want.is_some() {
            app.sampler.reset();
            sample_page(app); // CPU reads 0 until the first tick provides an interval
            SetTimer(Some(hwnd), SYS_TIMER_ID, SYS_FIRST_MS, None);
        } else {
            let _ = KillTimer(Some(hwnd), SYS_TIMER_ID);
        }
    }
}

fn on_sys_tick(hwnd: HWND) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            // The music page redraws only when the volume or the track position moved (a paused song costs
            // nothing); the other pages redraw every tick.
            let on_glance = app.sys_page == Some(GLANCE_PAGE);
            let redraw = if on_glance {
                sample_page(app);
                true
            } else if app.page == 0 {
                refresh_volume(app) | refresh_timeline(app)
            } else {
                sample_page(app);
                true
            };
            if redraw {
                let _ = render(hwnd, app);
            }
            // Settle into the steady cadence: 1 s for the live system page, slower for temperature/Bluetooth and
            // for the glance strip (its clock only changes by the minute).
            let ms = if on_glance { GLANCE_MS } else if app.page == 1 { DEVICES_MS } else { SYS_MS };
            unsafe { SetTimer(Some(hwnd), SYS_TIMER_ID, ms, None) };
        }
    });
}

// Scroll to a page: the spring animates the slide on the shared frame timer.
fn set_page(hwnd: HWND, page: u8) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            if app.show_event || page >= PAGES || page == app.page {
                return; // an agent popup owns the pill, or nothing to do
            }
            app.page = page;
            update_sys_timer(hwnd, app);
            app.last_tick = Instant::now();
            unsafe { SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None) };
        }
    });
}

// Which page dot is under the cursor; the hit area is wider than the dots themselves.
fn hit_dots(app: &App, x: i32, y: i32) -> Option<u8> {
    if app.spring.pos < 0.9 || app.show_event {
        return None;
    }
    let s = app.scale;
    let right = app.size.0 as f32 / 2.0 + OPEN_SIZE.0 * s / 2.0;
    let (px, py) = (x as f32, y as f32);
    if px < right - 28.0 * s || px >= right {
        return None;
    }
    (0..PAGES).find(|&i| (py - dot_y(i) * s).abs() <= DOT_GAP * s / 2.0)
}

fn on_eq_tick(hwnd: HWND) {
    APP.with(|a| {
        if let Some(app) = a.borrow().as_ref() {
            let _ = render_with(hwnd, app, true);
        }
    });
}

// Compact music: equalizer bars on the right, plus a thumbnail on the left when no agent label is
// using that space. Static on purpose: animating it would need a timer running for as long as music plays.
unsafe fn draw_compact_music(app: &App, x: f32, w: f32, h: f32, s: f32, t: f32) -> Result<()> {
    unsafe {
        let alpha = (1.0 - t / 0.4).clamp(0.0, 1.0);
        if alpha <= 0.0 || !is_playing(app) {
            return Ok(());
        }
        let cy = h / 2.0;
        let bars_left = x + w - 22.0 * s;
        app.white.SetOpacity(alpha * 0.9);
        let secs = app.eq_start.elapsed().as_secs_f32();
        for i in 0..EQ_HZ.len() {
            let swing = 0.5 + 0.5 * (secs * EQ_HZ[i] * std::f32::consts::TAU + EQ_PHASE[i]).sin();
            let bar_h = &lerp(EQ_MIN, EQ_MAX, swing);
            let l = bars_left + i as f32 * 5.0 * s;
            let bar = D2D1_ROUNDED_RECT {
                rect: rect(l, cy - bar_h * s / 2.0, l + 3.0 * s, cy + bar_h * s / 2.0),
                radiusX: 1.5 * s,
                radiusY: 1.5 * s,
            };
            app.rt.FillRoundedRectangle(&bar, &app.white);
        }
        if working_label(app).is_some() {
            return Ok(());
        }
        let size = THUMB_SIZE * s;
        let (left, top) = (x + 12.0 * s, cy - size / 2.0);
        let thumb = D2D1_ROUNDED_RECT { rect: rect(left, top, left + size, top + size), radiusX: 6.0 * s, radiusY: 6.0 * s };
        match &app.art {
            Some(bmp) => {
                let b = app.rt.CreateBitmapBrush(bmp, None, None)?;
                b.SetOpacity(alpha);
                // The cached art is ART_SIZE logical px; shrink it to the thumbnail.
                let k = THUMB_SIZE / ART_SIZE;
                b.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: left, M32: top });
                app.rt.FillRoundedRectangle(&thumb, &b);
            }
            None => {
                app.white.SetOpacity(alpha * 0.15);
                app.rt.FillRoundedRectangle(&thumb, &app.white);
            }
        }
        Ok(())
    }
}

fn focus_source(own: HWND, target: isize) -> bool {
    let t = HWND(target as *mut _);
    unsafe {
        if target == 0 || t == own || !IsWindow(Some(t)).as_bool() {
            return false;
        }
        if IsIconic(t).as_bool() {
            let _ = ShowWindow(t, SW_RESTORE);
        }
        SetForegroundWindow(t).as_bool()
    }
}

// Page indicator: a vertical strip on the right edge, matching the vertical scrolling. The dot
// nearest the current scroll position glows, blending smoothly while the pages slide.
fn dot_y(page: u8) -> f32 {
    DOT_TOP + DOT_GAP * page as f32
}

unsafe fn draw_dots(app: &App, x: f32, w: f32, s: f32, a: f32) {
    unsafe {
        for i in 0..PAGES {
            let dot = D2D1_ELLIPSE {
                point: Vector2 { X: x + w - DOT_RIGHT * s, Y: dot_y(i) * s },
                radiusX: 3.0 * s,
                radiusY: 3.0 * s,
            };
            let weight = (1.0 - (app.page_spring.pos - i as f32).abs()).clamp(0.0, 1.0);
            app.white.SetOpacity(a * (0.3 + 0.6 * weight));
            app.rt.FillEllipse(&dot, &app.white);
        }
    }
}

#[derive(Clone, Copy)]
enum Fill {
    Normal,
    Warn, // orange
    Good, // green
}

fn load_fill(pct: f32) -> Fill {
    if pct >= WARN_PCT { Fill::Warn } else { Fill::Normal }
}

// One row of the system page: name on the left, a bar in the middle, value on the right.
unsafe fn draw_row(app: &App, x: f32, w: f32, s: f32, a: f32, row: usize, name: &str, pct: f32, value: &str, fill: Fill) {
    unsafe {
        let top = (ROW_TOP + ROW_H * row as f32) * s;
        let bottom = top + ROW_H * s;
        text(app, &app.label, name, &rect(x + 24.0 * s, top, x + 112.0 * s, bottom), a * 0.75);
        text(app, &app.value, value, &rect(x + w - 104.0 * s, top, x + w - 24.0 * s, bottom), a);
        draw_bar(app, x + 116.0 * s, x + w - 112.0 * s, (top + bottom) / 2.0, s, a, pct, fill);
    }
}

// A thin rounded bar: a faint track with a coloured fill covering `pct` percent of it.
unsafe fn draw_bar(app: &App, left: f32, right: f32, cy: f32, s: f32, a: f32, pct: f32, fill: Fill) {
    unsafe {
        let half = 3.0 * s;
        let track = D2D1_ROUNDED_RECT { rect: rect(left, cy - half, right, cy + half), radiusX: half, radiusY: half };
        app.white.SetOpacity(a * 0.14);
        app.rt.FillRoundedRectangle(&track, &app.white);
        let fill_w = (right - left) * (pct / 100.0).clamp(0.0, 1.0);
        if fill_w >= 1.0 {
            let bar = D2D1_ROUNDED_RECT { rect: rect(left, cy - half, left + fill_w, cy + half), radiusX: half, radiusY: half };
            let brush = fill_brush(app, fill);
            brush.SetOpacity(a * 0.9);
            app.rt.FillRoundedRectangle(&bar, brush);
        }
    }
}

fn fill_brush(app: &App, fill: Fill) -> &ID2D1SolidColorBrush {
    match fill {
        Fill::Normal => &app.white,
        Fill::Warn => &app.accent,
        Fill::Good => &app.good,
    }
}

// Thermometer icon drawn from shapes (a tube and a bulb) so it needs no icon font.
unsafe fn draw_thermometer(app: &App, cx: f32, cy: f32, s: f32, a: f32, fill: Fill) {
    unsafe {
        let brush = fill_brush(app, fill);
        brush.SetOpacity(a * 0.9);
        let tube = D2D1_ROUNDED_RECT {
            rect: rect(cx - 2.5 * s, cy - 8.5 * s, cx + 2.5 * s, cy + 3.0 * s),
            radiusX: 2.5 * s,
            radiusY: 2.5 * s,
        };
        app.rt.FillRoundedRectangle(&tube, brush);
        let bulb = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy + 5.2 * s }, radiusX: 4.4 * s, radiusY: 4.4 * s };
        app.rt.FillEllipse(&bulb, brush);
    }
}

// A widget tile: a softly lit rounded card on the glass.
unsafe fn draw_tile(app: &App, left: f32, top: f32, w: f32, h: f32, s: f32, a: f32) {
    unsafe {
        let card = D2D1_ROUNDED_RECT { rect: rect(left, top, left + w, top + h), radiusX: 14.0 * s, radiusY: 14.0 * s };
        app.white.SetOpacity(a * 0.07);
        app.rt.FillRoundedRectangle(&card, &app.white);
        app.white.SetOpacity(a * 0.10);
        app.rt.DrawRoundedRectangle(&card, &app.white, s, None);
    }
}

// A circular progress ring: faint track plus a round-capped arc from 12 o'clock, clockwise, for `pct` percent.
unsafe fn draw_ring(app: &App, cx: f32, cy: f32, radius: f32, stroke: f32, pct: f32, a: f32, fill: Fill) -> Result<()> {
    unsafe {
        let ring = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        app.white.SetOpacity(a * 0.14);
        app.rt.DrawEllipse(&ring, &app.white, stroke, None);
        let pct = pct.clamp(0.0, 100.0);
        if pct <= 0.0 {
            return Ok(());
        }
        let brush = fill_brush(app, fill);
        brush.SetOpacity(a * 0.95);
        if pct >= 99.5 {
            app.rt.DrawEllipse(&ring, brush, stroke, None); // an arc can't close on itself: draw the full circle
            return Ok(());
        }
        let angle = pct / 100.0 * std::f32::consts::TAU;
        let end = Vector2 { X: cx + radius * angle.sin(), Y: cy - radius * angle.cos() };
        let path = app.factory.CreatePathGeometry()?;
        let sink = path.Open()?;
        sink.BeginFigure(Vector2 { X: cx, Y: cy - radius }, D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: end,
            size: D2D_SIZE_F { width: radius, height: radius },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: if pct > 50.0 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
        });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        app.rt.DrawGeometry(&path, brush, stroke, &app.ring_style);
        Ok(())
    }
}

// Four ascending bars, lit by Wi-Fi signal quality (0-100).
unsafe fn draw_signal_bars(app: &App, right: f32, bottom: f32, s: f32, a: f32, quality: u8) {
    unsafe {
        let lit = (quality as usize).div_ceil(25).min(4); // 1-25 -> 1 bar ... 76-100 -> 4 bars
        for i in 0..4usize {
            let bar_h = (5.0 + 3.0 * i as f32) * s;
            let l = right - (4 - i) as f32 * 5.0 * s;
            let bar = D2D1_ROUNDED_RECT { rect: rect(l, bottom - bar_h, l + 3.0 * s, bottom), radiusX: 1.0 * s, radiusY: 1.0 * s };
            app.white.SetOpacity(a * if i < lit { 0.9 } else { 0.2 });
            app.rt.FillRoundedRectangle(&bar, &app.white);
        }
    }
}

// Third page, widget style: temperature, the connection in use, and Bluetooth battery rings.
unsafe fn draw_devices(app: &App, x: f32, s: f32, a: f32) {
    unsafe {
        let st = &app.stats;
        let top = TILE_TOP * s;
        let h = TILE_H * s;
        let mut left = x + TILE_LEFT * s;
        let mut tile = |width: f32| {
            let l = left;
            left += (width + TILE_GAP) * s;
            draw_tile(app, l, top, width * s, h, s, a);
            l
        };

        // Temperature tile: thermometer, big number, label.
        let l = tile(TEMP_TILE_W);
        let (hot, number) = match st.temp_c {
            Some(t) => (t >= TEMP_WARN_C, format!("{t:.0}\u{b0}")),
            None => (false, "\u{2014}".to_string()),
        };
        let fill = if hot { Fill::Warn } else { Fill::Normal };
        draw_thermometer(app, l + 20.0 * s, top + 24.0 * s, s, a, fill);
        text_in(app, fill_brush(app, fill), &app.big, &number, &rect(l + 14.0 * s, top + 44.0 * s, l + (TEMP_TILE_W - 6.0) * s, top + 84.0 * s), a);
        text(app, &app.small, "Temperature", &rect(l + 14.0 * s, top + 88.0 * s, l + (TEMP_TILE_W - 4.0) * s, top + 108.0 * s), a * 0.6);

        // Connection tile: icon + type, signal bars (Wi-Fi), network name. A weak signal tints the icon.
        let l = tile(NET_TILE_W);
        let (glyph, kind, name, signal) = match &st.link {
            Some(link) => {
                let (glyph, kind) = match link.kind {
                    LinkKind::WiFi => ("\u{E701}", "Wi-Fi"),
                    LinkKind::Ethernet => ("\u{E839}", "Ethernet"),
                    LinkKind::Mobile => ("", "Mobile"),
                    LinkKind::Other => ("", "Network"),
                };
                (glyph, kind, link.name.clone(), link.signal)
            }
            None => ("", "Offline", String::new(), None),
        };
        let weak = signal.is_some_and(|q| q < WEAK_SIGNAL_PCT);
        let icon_brush = if weak { &app.accent } else { &app.white };
        text_in(app, icon_brush, &app.icons, glyph, &rect(l + 8.0 * s, top + 12.0 * s, l + 34.0 * s, top + 36.0 * s), a);
        text(app, &app.small, kind, &rect(l + 36.0 * s, top + 12.0 * s, l + (NET_TILE_W - 40.0) * s, top + 36.0 * s), a * 0.6);
        if let Some(q) = signal {
            draw_signal_bars(app, l + (NET_TILE_W - 12.0) * s, top + 30.0 * s, s, a, q);
        }
        // The name gets the rest of the tile (long network names wrap onto a second line).
        text(app, &app.text, &name, &rect(l + 14.0 * s, top + 44.0 * s, l + (NET_TILE_W - 8.0) * s, top + 106.0 * s), a);

        // Bluetooth tile: header, then a battery ring per connected device (percentage inside, name below).
        let l = tile(BT_TILE_W);
        text(app, &app.icons, "\u{E702}", &rect(l + 8.0 * s, top + 12.0 * s, l + 34.0 * s, top + 36.0 * s), a);
        let total = st.bluetooth.len();
        let title = match total {
            0 => "Bluetooth".to_string(),
            n if n > BT_RINGS => format!("Bluetooth \u{b7} +{}", n - BT_RINGS),
            _ => "Bluetooth".to_string(),
        };
        text(app, &app.small, &title, &rect(l + 36.0 * s, top + 12.0 * s, l + (BT_TILE_W - 8.0) * s, top + 36.0 * s), a * 0.6);
        if total == 0 {
            text(app, &app.small, "No device connected", &rect(l + 14.0 * s, top + 52.0 * s, l + (BT_TILE_W - 10.0) * s, top + 90.0 * s), a * 0.5);
        }
        let slot = (BT_TILE_W - 12.0) / BT_RINGS as f32; // width of one device's column
        for (i, dev) in st.bluetooth.iter().take(BT_RINGS).enumerate() {
            let cx = l + (6.0 + slot * (i as f32 + 0.5)) * s;
            let cy = top + 62.0 * s;
            let low = dev.battery.is_some_and(|b| b <= sysinfo::LOW_BATTERY_PCT);
            let ring_fill = if low { Fill::Warn } else { Fill::Normal };
            let _ = draw_ring(app, cx, cy, 15.0 * s, 3.5 * s, dev.battery.unwrap_or(0) as f32, a, ring_fill);
            let inside = dev.battery.map_or("\u{2014}".to_string(), |b| b.to_string()); // dash = connected, no battery reported
            text(app, &app.center, &inside, &rect(cx - 17.0 * s, cy - 10.0 * s, cx + 17.0 * s, cy + 10.0 * s), a);
            text(app, &app.tiny, &dev.name, &rect(cx - slot / 2.0 * s + 1.0 * s, top + 82.0 * s, cx + slot / 2.0 * s - 1.0 * s, top + 110.0 * s), a * 0.7);
        }
    }
}

unsafe fn draw_system(app: &App, x: f32, w: f32, s: f32, a: f32) {
    unsafe {
        let st = &app.stats;
        // Battery: green while power is connected (charging, or full on the charger), orange when low and
        // unplugged, plain white on battery.
        let (name, pct, value) = match st.battery {
            Some(b) => (
                if b.charging { "Charging" } else if st.on_ac { "Plugged in" } else { "Battery" },
                b.percent as f32,
                format!("{}%", b.percent),
            ),
            None => ("Power", 0.0, (if st.on_ac { "Plugged in" } else { "\u{2014}" }).to_string()),
        };
        let battery_fill = match sysinfo::power_look(st.battery, st.on_ac) {
            sysinfo::PowerLook::Plugged => Fill::Good,
            sysinfo::PowerLook::Low => Fill::Warn,
            sysinfo::PowerLook::Normal => Fill::Normal,
        };
        draw_row(app, x, w, s, a, 0, name, pct, &value, battery_fill);
        draw_row(app, x, w, s, a, 1, "CPU", st.cpu, &format!("{:.0}%", st.cpu), load_fill(st.cpu));
        // GPU has no value for the first fraction of a second, or when Windows exposes no GPU counters.
        let (gpu, gpu_text) = st.gpu.map_or((0.0, "\u{2014}".to_string()), |g| (g, format!("{g:.0}%")));
        draw_row(app, x, w, s, a, 2, "GPU", gpu, &gpu_text, load_fill(gpu));
        draw_row(app, x, w, s, a, 3, "Memory", st.mem, &format!("{:.0}%", st.mem), load_fill(st.mem));
        draw_row(app, x, w, s, a, 4, "Disk C:", st.disk, &format!("{:.0}%", st.disk), load_fill(st.disk));
        // Network has no meaningful 0-100 scale, so no bar: download on the left, upload on the right.
        let top = (ROW_TOP + ROW_H * 5.0) * s;
        let bottom = top + ROW_H * s;
        text(app, &app.label, "Network", &rect(x + 24.0 * s, top, x + 112.0 * s, bottom), a * 0.75);
        let down = format!("\u{2193} {}", sysinfo::format_speed(st.net_down));
        let up = format!("\u{2191} {}", sysinfo::format_speed(st.net_up));
        text(app, &app.value, &down, &rect(x + 116.0 * s, top, x + w - 112.0 * s, bottom), a);
        text(app, &app.value, &up, &rect(x + w - 112.0 * s, top, x + w - 24.0 * s, bottom), a);
    }
}

// A system popup: a large icon on the left, a title and detail line, and (for the volume HUD) a level bar.
unsafe fn draw_notice(app: &App, x: f32, w: f32, s: f32, a: f32, n: &notice::Notice) {
    unsafe {
        let cy = 70.0 * s;
        match &n.icon {
            notice::Icon::Battery { percent, plugged } => {
                let fill = if *plugged { Fill::Good } else if *percent <= sysinfo::LOW_BATTERY_PCT { Fill::Warn } else { Fill::Normal };
                draw_battery_icon(app, x + 30.0 * s, cy, s * 2.0, a, *percent, fill); // the small icon at double size
            }
            notice::Icon::Glyph(g) => text(app, &app.icon_big, g, &rect(x + 24.0 * s, cy - 28.0 * s, x + 92.0 * s, cy + 28.0 * s), a),
        }
        let tx = x + 108.0 * s;
        let (title_top, detail_top) = if n.level.is_some() { (28.0, 54.0) } else { (42.0, 68.0) };
        text(app, &app.text, &n.title, &rect(tx, title_top * s, x + w - 40.0 * s, (title_top + 24.0) * s), a);
        text(app, &app.small, &n.detail, &rect(tx, detail_top * s, x + w - 40.0 * s, (detail_top + 20.0) * s), a * 0.65);
        if let Some(level) = n.level {
            draw_bar(app, tx, x + w - 44.0 * s, 98.0 * s, s, a, level * 100.0, Fill::Normal);
        }
    }
}

// Pages are stacked vertically and slide with page_spring: page i sits at (i - pos) * page_height.
unsafe fn draw_content(app: &App, x: f32, w: f32, h: f32, s: f32, a: f32, lift: f32) -> Result<()> {
    unsafe {
        if let (Some(n), true) = (&app.notice, app.show_event) {
            draw_notice(app, x, w, s, a, n);
            return Ok(());
        }
        if let (Some(ev), true) = (&app.event, app.show_event) {
            text(app, &app.text, &event_label(ev), &rect(x + 24.0 * s, 18.0 * s, x + w - 24.0 * s, 120.0 * s), a);
            return Ok(());
        }
        let page_h = OPEN_SIZE.1 * s;
        app.rt.PushAxisAlignedClip(&rect(x, 0.0, x + w, h), D2D1_ANTIALIAS_MODE_ALIASED);
        let mut result = Ok(());
        for page in 0..PAGES {
            let dy = (page as f32 - app.page_spring.pos) * page_h;
            if dy.abs() >= page_h {
                continue; // fully off-screen
            }
            // `lift` keeps the whole notch's hide/reveal slide applied while the pages slide inside it.
            app.rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: dy + lift });
            match page {
                0 => result = draw_music(app, x, w, s, a),
                1 => draw_devices(app, x, s, a), // the widget page comes first...
                2 => draw_system(app, x, w, s, a), // ...then the system info page
                3 => draw_agents(app, x, w, s, a),
                _ => clippage::draw(app, x, w, s, a),
            }
        }
        app.rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: lift });
        app.rt.PopAxisAlignedClip();
        draw_dots(app, x, w, s, a);
        result
    }
}

unsafe fn draw_music(app: &App, x: f32, w: f32, s: f32, a: f32) -> Result<()> {
    unsafe {
        let Some(m) = &app.media else {
            text(app, &app.text, "Nothing playing", &rect(x + 24.0 * s, 18.0 * s, x + w - 24.0 * s, 50.0 * s), a * 0.6);
            return Ok(());
        };
        let art_area = rect(x + 20.0 * s, 20.0 * s, x + (20.0 + ART_SIZE) * s, (20.0 + ART_SIZE) * s);
        let rounded = D2D1_ROUNDED_RECT { rect: art_area, radiusX: 10.0 * s, radiusY: 10.0 * s };
        match &app.art {
            Some(bmp) => {
                let b = app.rt.CreateBitmapBrush(bmp, None, None)?;
                b.SetOpacity(a);
                b.SetTransform(&Matrix3x2 {
                    M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: art_area.left, M32: art_area.top,
                });
                app.rt.FillRoundedRectangle(&rounded, &b);
            }
            None => {
                app.white.SetOpacity(a * 0.12);
                app.rt.FillRoundedRectangle(&rounded, &app.white);
            }
        }
        let tx = x + (20.0 + ART_SIZE + 14.0) * s;
        text(app, &app.text, &m.title, &rect(tx, 22.0 * s, x + w - 20.0 * s, 44.0 * s), a);
        text(app, &app.small, &m.artist, &rect(tx, 46.0 * s, x + w - 20.0 * s, 66.0 * s), a * 0.6);
        // Progress: elapsed time, a thin bar (with a small knob when the player allows seeking), total time.
        if let Some(tl) = app.timeline.filter(|t| t.duration_ms > 0) {
            let cy = PROG_CY * s;
            let elapsed = shown_position(app);
            let (bar_l, bar_r) = (x + PROG_BAR_L * s, x + PROG_BAR_R * s);
            text(app, &app.label, &media::format_clock(elapsed), &rect(x + PROG_LABEL_L * s, cy - 9.0 * s, bar_l - 4.0 * s, cy + 9.0 * s), a * 0.6);
            text(app, &app.value, &media::format_clock(tl.duration_ms), &rect(bar_r + 4.0 * s, cy - 9.0 * s, bar_r + 40.0 * s, cy + 9.0 * s), a * 0.6);
            let half = 1.5 * s;
            let track = D2D1_ROUNDED_RECT { rect: rect(bar_l, cy - half, bar_r, cy + half), radiusX: half, radiusY: half };
            app.white.SetOpacity(a * 0.18);
            app.rt.FillRoundedRectangle(&track, &app.white);
            let filled = bar_l + (bar_r - bar_l) * (elapsed as f32 / tl.duration_ms as f32).clamp(0.0, 1.0);
            if filled - bar_l >= 1.0 {
                let fill = D2D1_ROUNDED_RECT { rect: rect(bar_l, cy - half, filled, cy + half), radiusX: half, radiusY: half };
                app.white.SetOpacity(a * 0.85);
                app.rt.FillRoundedRectangle(&fill, &app.white);
            }
            if tl.seekable {
                let knob = D2D1_ELLIPSE { point: Vector2 { X: filled, Y: cy }, radiusX: 3.5 * s, radiusY: 3.5 * s };
                app.white.SetOpacity(a);
                app.rt.FillEllipse(&knob, &app.white);
            }
        }
        // Segoe MDL2 Assets: E892 previous, E768 play, E769 pause, E893 next.
        let glyphs = ["\u{E892}", if m.playing { "\u{E769}" } else { "\u{E768}" }, "\u{E893}"];
        for (g, r) in glyphs.iter().zip(button_rects(x + TRANSPORT_CX * s, s)) {
            text(app, &app.icons, g, &r, a);
        }
        if let Some(v) = app.volume {
            // Mute button + volume slider on the right of the same row. Segoe MDL2: E767 volume, E74F muted.
            let (top, bottom) = (BTN_TOP * s, (BTN_TOP + BTN_H) * s);
            let cy = (top + bottom) / 2.0;
            let silent = v.muted || v.level <= 0.0;
            let glyph = if silent { "\u{E74F}" } else { "\u{E767}" };
            text(app, &app.icons, glyph, &rect(x + VOL_ICON_L * s, top, x + VOL_ICON_R * s, bottom), if silent { a * 0.55 } else { a });
            let (left, right) = (x + VOL_TRACK_L * s, x + w - VOL_TRACK_RIGHT_PAD * s);
            let half = 2.0 * s;
            let track = D2D1_ROUNDED_RECT { rect: rect(left, cy - half, right, cy + half), radiusX: half, radiusY: half };
            app.white.SetOpacity(a * 0.18);
            app.rt.FillRoundedRectangle(&track, &app.white);
            let shown = if v.muted { 0.0 } else { v.level };
            let knob_x = left + (right - left) * shown;
            if knob_x - left >= 1.0 {
                let fill = D2D1_ROUNDED_RECT { rect: rect(left, cy - half, knob_x, cy + half), radiusX: half, radiusY: half };
                app.white.SetOpacity(a * 0.9);
                app.rt.FillRoundedRectangle(&fill, &app.white);
            }
            let knob = D2D1_ELLIPSE { point: Vector2 { X: knob_x, Y: cy }, radiusX: 6.0 * s, radiusY: 6.0 * s };
            app.white.SetOpacity(a);
            app.rt.FillEllipse(&knob, &app.white);
        }
        Ok(())
    }
}

fn on_media(hwnd: HWND, info: Option<MediaInfo>) {
    APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return };
        let mut info = info;
        let px = (ART_SIZE * app.scale) as u32;
        app.art = info.as_mut().and_then(|m| m.art.take()).and_then(|b| art::decode(&app.rt, &b, px).ok());
        app.media = info;
        // Widen/narrow the collapsed pill when playback starts/stops.
        app.compact_target = compact_wanted(app);
        update_eq_timer(hwnd, app);
        update_reveal(hwnd, app); // music started or stopped: show or hide the notch
        update_sys_timer(hwnd, app); // the music page needs its volume refresh once a player exists
        refresh_timeline(app); // new track or play/pause: don't wait for the next tick to move the bar
        app.last_tick = Instant::now();
        unsafe { SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None) };
        let _ = render(hwnd, app);
    });
    trim_memory();
}

fn event_label(ev: &Event) -> String {
    let state = match ev.status {
        Status::Working | Status::Activity => "Working", // Activity never reaches a popup: on_event handles it first
        Status::Done => "Done",
        Status::Attention => "Needs attention",
        Status::Clear => "Cleared", // never displayed: on_event handles Clear before any UI
    };
    format!("{} \u{2014} {state}\n{}", ev.agent, joined([&ev.project, &ev.place, &ev.title]))
}

/// The non-empty parts, separated by a middle dot.
fn joined<'a>(parts: impl IntoIterator<Item = &'a String>) -> String {
    parts.into_iter().filter(|p| !p.is_empty()).map(String::as_str).collect::<Vec<_>>().join(" \u{b7} ")
}

/// "claude · Project Notch · VS Code": which agent, on what project, in which app.
fn run_label(ev: &Event) -> String {
    joined([&ev.agent, &ev.project, &ev.place])
}

/// The collapsed pill is narrow: agent and app, or agent and project when the app is unknown.
fn short_label(ev: &Event) -> String {
    joined([&ev.agent, if ev.place.is_empty() { &ev.project } else { &ev.place }])
}

fn on_event(hwnd: HWND, ev: Event) {
    if ev.status == Status::Clear {
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().as_mut() {
                app.working.remove(&ev.key());
                update_reveal(hwnd, app); // the last run may have just gone away
                let _ = render(hwnd, app);
            }
        });
        return;
    }
    if ev.status == Status::Activity {
        // Progress for the hover preview: update the run's current step. No popup, no chime.
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().as_mut() {
                let key = ev.key();
                let is_new = !app.working.contains_key(&key); // notch started mid-run: begin tracking it
                let was_waiting = app.working.get(&key).is_some_and(|r| r.waiting);
                let run = app.working.entry(key).or_insert_with(|| Working {
                    label: run_label(&ev),
                    short: short_label(&ev),
                    since: Instant::now(),
                    last_activity: Instant::now(),
                    waiting: false,
                });
                run.last_activity = Instant::now();
                run.waiting = false; // it is working again
                update_reveal(hwnd, app); // a run we only just learned about makes the notch visible
                if is_new || was_waiting || app.target == 1.0 {
                    let _ = render(hwnd, app); // only worth a frame if the pill is open or its label changed
                }
            }
        });
        return;
    }
    let opens = ev.status != Status::Working;
    APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return };
        let key = ev.key();
        if opens {
            if ev.status == Status::Attention {
                // Waiting for the user: stays in the list with a green dot until it works again or is cleared.
                let run = app.working.entry(key.clone()).or_insert_with(|| Working {
                    label: run_label(&ev),
                    short: short_label(&ev),
                    since: Instant::now(),
                    last_activity: Instant::now(),
                    waiting: false,
                });
                run.waiting = true;
                run.last_activity = Instant::now();
            } else {
                app.working.remove(&key);
            }
            // Prefer the window captured when this run started working: the foreground window
            // at completion time is often something else by then.
            app.focus = app.sources.get(&key).copied().unwrap_or(ev.hwnd);
        } else {
            app.working.insert(
                key.clone(),
                Working {
                    label: run_label(&ev),
                    short: short_label(&ev),
                    since: Instant::now(),
                    last_activity: Instant::now(),
                    waiting: false,
                },
            );
            if ev.hwnd != 0 {
                if app.sources.len() >= MAX_SOURCES {
                    app.sources.clear(); // sessions come and go; a rare reset only loses click-to-focus
                }
                app.sources.insert(key, ev.hwnd);
            }
        }
        app.event = Some(ev);
        if opens {
            unsafe { let _ = PlaySoundW(w!("Notification.Default"), None, SND_ALIAS | SND_ASYNC); }
            app.notice = None; // an agent popup replaces any system popup
            app.show_event = true;
            app.target = 1.0;
            update_eq_timer(hwnd, app);
            update_sys_timer(hwnd, app);
            app.last_tick = Instant::now();
            unsafe {
                SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None);
                SetTimer(Some(hwnd), HOLD_TIMER_ID, HOLD_MS, None);
            }
        } else {
            let _ = render(hwnd, app);
        }
        update_reveal(hwnd, app); // a new run or popup brings the notch out; the last run finishing lets it hide
    });
}

// Hold expired: collapse unless the user is hovering (leaving then collapses it).
fn on_hold_expired(hwnd: HWND) {
    unsafe { let _ = KillTimer(Some(hwnd), HOLD_TIMER_ID); }
    let hovering = APP.with(|a| {
        a.borrow_mut().as_mut().is_some_and(|app| {
            if app.tracking {
                app.show_event = false; // user is still here: hand the pill over to the music view
                app.notice = None;
            }
            app.tracking
        })
    });
    if hovering {
        APP.with(|a| {
            if let Some(app) = a.borrow_mut().as_mut() {
                update_sys_timer(hwnd, app); // the popup yielded to the page the user was on
                let _ = render(hwnd, app);
            }
        });
    } else {
        set_target(hwnd, 0.0);
    }
}

/// Show a system popup (charger, Bluetooth, network, volume HUD). It never takes over a notch the user has open.
fn show_notice(hwnd: HWND, app: &mut App, n: notice::Notice) {
    if app.target > 0.0 && !app.show_event {
        return; // the user is hovering the open notch: don't hijack it
    }
    if n.chime {
        unsafe { let _ = PlaySoundW(w!("Notification.Default"), None, SND_ALIAS | SND_ASYNC); }
    }
    let hold = n.hold_ms;
    app.notice = Some(n);
    app.event = None;
    app.show_event = true;
    app.target = 1.0;
    update_eq_timer(hwnd, app);
    update_sys_timer(hwnd, app);
    update_reveal(hwnd, app);
    app.last_tick = Instant::now();
    unsafe {
        SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None);
        SetTimer(Some(hwnd), HOLD_TIMER_ID, hold, None);
    }
}

fn on_power_change(hwnd: HWND) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            let (battery, on_ac) = sysinfo::power_status();
            let now = notice::Power { battery, on_ac };
            if let Some(n) = notice::power_notice(app.power_prev, now) {
                show_notice(hwnd, app, n);
            }
            app.power_prev = now;
        }
    });
}

fn on_bt_timer(hwnd: HWND) {
    unsafe { let _ = KillTimer(Some(hwnd), BT_TIMER_ID); }
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            let now = devices::connected();
            if let Some(n) = notice::bluetooth_notice(&app.bt_prev, &now) {
                show_notice(hwnd, app, n);
            }
            app.bt_prev = now;
        }
    });
}

fn on_net_timer(hwnd: HWND) {
    unsafe { let _ = KillTimer(Some(hwnd), NET_TIMER_ID); }
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            let now = netlink::active();
            if let Some(n) = notice::network_notice(&app.net_prev, &now) {
                show_notice(hwnd, app, n);
            }
            app.net_prev = now;
        }
    });
}

/// A power-setting notification arrived (lp points at a POWERBROADCAST_SETTING). We only registered for the
/// current monitor brightness, whose payload is the new percentage as a DWORD.
fn on_power_setting(hwnd: HWND, lp: LPARAM) {
    if lp.0 == 0 {
        return;
    }
    let percent = unsafe {
        let setting = &*(lp.0 as *const POWERBROADCAST_SETTING);
        if setting.PowerSetting != GUID_BRIGHTNESS || setting.DataLength < 4 {
            return;
        }
        std::ptr::read_unaligned(setting.Data.as_ptr().cast::<u32>()).min(100) as u8
    };
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            let changed = app.brightness_prev.is_some_and(|prev| prev != percent);
            app.brightness_prev = Some(percent);
            if changed {
                show_notice(hwnd, app, notice::brightness_notice(percent)); // the HUD for the brightness keys
            }
        }
    });
}

fn on_volume_change(hwnd: HWND) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            let now = volume::read();
            let changed = now != app.vol_prev;
            app.vol_prev = now;
            if let (true, false, Some(v)) = (changed, app.vol_drag, now) {
                show_notice(hwnd, app, notice::volume_notice(v.level, v.muted)); // the HUD for the volume keys
            }
        }
    });
}

/// True while there is something worth showing without the user asking: a popup, music playing, or an agent working.
fn has_content(app: &App) -> bool {
    // Music alone does not bring the notch out any more: it is reachable by hovering the top edge. A coding agent
    // working, a popup, or the microphone/camera being in use makes the notch show itself.
    app.show_event || app.privacy.any() || !live_runs(app).is_empty()
}

/// Width state of the collapsed pill: 1 = the wider "compact" size (needed for the music art + bars and the
/// microphone/camera icons), 0 = the plain narrow size.
fn compact_wanted(app: &App) -> f32 {
    if is_playing(app) || app.privacy.any() || glance_wanted(app) { 1.0 } else { 0.0 }
}

/// The peek view: the user touched the top edge, the notch is collapsed and has nothing else to say, so it shows
/// the glance strip (time, battery, connection, CPU). Not shown while music plays, an agent works or a popup is up.
fn glance_wanted(app: &App) -> bool {
    app.tracking && app.target == 0.0 && !has_content(app) && !is_playing(app)
}

/// Slide the notch into view or hide it. Visible while it has content, while the cursor is on it, and while it
/// is open or still collapsing. Returns true if the target changed (so the caller must keep animating).
fn update_reveal(hwnd: HWND, app: &mut App) -> bool {
    let visible = has_content(app) || app.tracking || app.target > 0.0 || app.spring.pos > 0.001;
    let target = if visible { 1.0 } else { 0.0 };
    let mut changed = false;
    if target != app.reveal_target {
        app.reveal_target = target;
        update_eq_timer(hwnd, app); // the equalizer bars only run while the notch is visible
        changed = true;
    }
    // Showing or hiding the glance strip changes the pill width and whether the clock/CPU/battery are sampled.
    let compact = compact_wanted(app);
    if compact != app.compact_target {
        app.compact_target = compact;
        changed = true;
    }
    update_sys_timer(hwnd, app);
    update_rotate_timer(hwnd, app);
    if changed {
        app.last_tick = Instant::now();
        unsafe { SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None) };
    }
    changed
}

fn set_target(hwnd: HWND, target: f32) {
    APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.target = target;
            update_eq_timer(hwnd, app);
            update_sys_timer(hwnd, app);
            update_reveal(hwnd, app);
            app.last_tick = Instant::now();
            unsafe { SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None) };
        }
    });
}

fn on_timer(hwnd: HWND) {
    APP.with(|a| {
        let mut a = a.borrow_mut();
        let Some(app) = a.as_mut() else { return };
        let now = Instant::now();
        let dt = (now - app.last_tick).as_secs_f32();
        app.spring.step(app.target, dt);
        app.compact.step(app.compact_target, dt);
        app.page_spring.step(app.page as f32, dt);
        app.reveal.step(app.reveal_target, dt);
        app.last_tick = now;
        if app.spring.settled(app.target)
            && app.compact.settled(app.compact_target)
            && app.page_spring.settled(app.page as f32)
            && app.reveal.settled(app.reveal_target)
        {
            app.spring = Spring { pos: app.target, vel: 0.0 };
            app.compact = Spring { pos: app.compact_target, vel: 0.0 };
            app.page_spring = Spring { pos: app.page as f32, vel: 0.0 };
            app.reveal = Spring { pos: app.reveal_target, vel: 0.0 };
            if app.target == 0.0 {
                app.show_event = false; // fully collapsed: the next hover shows music again
                app.notice = None;
                app.clip.reset_view(); // the next visit starts with an empty search and no date filter
                if app.clip.typing {
                    // A popup closed the notch while the search box had focus: leave it like Esc would (handled
                    // after this borrow ends, because handing focus back sends messages to this window).
                    unsafe { let _ = PostMessageW(Some(hwnd), WM_CHAR, WPARAM(0x1B), LPARAM(0)); }
                }
            }
            if update_reveal(hwnd, app) {
                // Collapse finished with nothing left to show: start sliding away, so keep ticking.
                let _ = render(hwnd, app);
                return;
            }
            unsafe { let _ = KillTimer(Some(hwnd), TIMER_ID); }
            let _ = render(hwnd, app);
            trim_memory();
            return;
        }
        let _ = render(hwnd, app);
    });
}

/// The right-click menu. On the clipboard page it also offers to delete the item under the cursor or clear the history.
fn show_menu(hwnd: HWND, clip_item: Option<u64>, can_clear: bool) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        if clip_item.is_some() {
            let _ = AppendMenuW(menu, MF_STRING, clippage::ID_CLIP_DELETE, w!("Delete this item"));
        }
        if can_clear {
            let _ = AppendMenuW(menu, MF_STRING, clippage::ID_CLIP_CLEAR, w!("Clear clipboard history"));
        }
        if clip_item.is_some() || can_clear {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        }
        let login_flags = if autostart::is_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
        let _ = AppendMenuW(menu, login_flags, ID_AUTOSTART, w!("Start at login"));
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // The menu only dismisses on outside clicks if our window is foreground first.
        let _ = SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        match picked.0 as usize {
            ID_QUIT => { let _ = DestroyWindow(hwnd); }
            ID_AUTOSTART => autostart::toggle(),
            clippage::ID_CLIP_DELETE => {
                if let Some(id) = clip_item {
                    clippage::delete(hwnd, id);
                }
            }
            clippage::ID_CLIP_CLEAR => clippage::clear(hwnd),
            _ => {}
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            // Three modes. Hidden: touching the top-edge strip only slides the notch into view. Revealed (or
            // visible because it has content): moving onto the notch itself expands it.
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32;
            // Dragging the volume slider: follow the cursor (even outside the window; the mouse is captured).
            let x = (lp.0 & 0xFFFF) as i16 as f32;
            let dragging = APP.with(|a| match a.borrow_mut().as_mut() {
                Some(app) if app.vol_drag => {
                    set_volume_from_x(app, x);
                    let _ = render(hwnd, app);
                    true
                }
                _ => false,
            });
            if dragging {
                return LRESULT(0);
            }
            // Clipboard page: row hover, and pulling a row off the notch starts a drag.
            if clippage::on_mouse_move(hwnd, lp, wp.0 & 1 != 0) {
                return LRESULT(0);
            }
            let (first, expand) = APP.with(|a| {
                let mut a = a.borrow_mut();
                let Some(app) = a.as_mut() else { return (false, false) };
                let first = !app.tracking;
                app.tracking = true;
                if first {
                    update_reveal(hwnd, app); // cursor reached the strip or the notch: bring it into view
                }
                let expand = app.target == 0.0 && (has_content(app) || y >= HOT_STRIP_H * app.scale);
                if expand {
                    // Open on whichever page is useful: running work first, then music, else system stats.
                    app.page = if !live_runs(app).is_empty() { AGENTS_PAGE } else if app.media.is_some() { 0 } else { 1 };
                    app.page_spring = Spring { pos: app.page as f32, vel: 0.0 }; // open already in place
                }
                (first, expand)
            });
            if first {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe { let _ = TrackMouseEvent(&mut tme); }
            }
            if expand {
                set_target(hwnd, 1.0);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            let typing = APP.with(|a| {
                a.borrow_mut().as_mut().is_some_and(|app| {
                    app.tracking = false;
                    app.clip.typing // typing in the search box keeps the notch open until it loses focus
                })
            });
            if !typing {
                set_target(hwnd, 0.0);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            clippage::on_button_down(x, y);
            let grabbed = APP.with(|a| {
                let mut a = a.borrow_mut();
                let Some(app) = a.as_mut() else { return false };
                if !hit_volume_track(app, x, y) {
                    return false;
                }
                app.vol_drag = true;
                set_volume_from_x(app, x as f32); // a click on the track jumps there, then follows the drag
                let _ = render(hwnd, app);
                true
            });
            if grabbed {
                unsafe { SetCapture(hwnd) };
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            // Finishing a slider drag: release the mouse and stop; it must not also count as a click elsewhere.
            if APP.with(|a| a.borrow_mut().as_mut().is_some_and(|app| std::mem::replace(&mut app.vol_drag, false))) {
                unsafe { let _ = ReleaseCapture(); }
                return LRESULT(0);
            }
            if clippage::on_click(hwnd, x, y) {
                return LRESULT(0);
            }
            // The OS calls below (volume, media session) run with no APP borrow held: an outgoing COM call can pump
            // messages, and an incoming drag-and-drop callback would then try to borrow it again.
            if APP.with(|a| a.borrow().as_ref().is_some_and(|app| hit_mute(app, x, y))) {
                let toggled = volume::toggle_mute().is_some();
                APP.with(|a| {
                    if let (true, Some(app)) = (toggled, a.borrow_mut().as_mut()) {
                        app.volume = volume::read();
                        let _ = render(hwnd, app);
                    }
                });
                return LRESULT(0);
            }
            // Click on the progress bar: jump there (and show it at once rather than waiting for the next read).
            let seek = APP.with(|a| {
                let a = a.borrow();
                let app = a.as_ref()?;
                let (frac, tl) = (hit_progress(app, x, y)?, app.timeline?);
                Some((tl, (tl.duration_ms as f32 * frac) as u64))
            });
            if let Some((tl, target)) = seek {
                media::seek(target);
                APP.with(|a| {
                    if let Some(app) = a.borrow_mut().as_mut() {
                        app.timeline = Some(media::Timeline { position_ms: target, ..tl });
                        app.timeline_at = Instant::now();
                        app.seek_hold = Instant::now() + SEEK_HOLD;
                        let _ = render(hwnd, app);
                    }
                });
                return LRESULT(0);
            }
            if let Some(page) = APP.with(|a| a.borrow().as_ref().and_then(|app| hit_dots(app, x, y))) {
                set_page(hwnd, page);
                return LRESULT(0);
            }
            let (cmd, focus) = APP.with(|a| {
                let a = a.borrow();
                let Some(app) = a.as_ref() else { return (None, 0) };
                (hit_button(app, x, y), if app.show_event { app.focus } else { 0 })
            });
            if let Some(cmd) = cmd {
                media::command(cmd);
            } else if focus_source(hwnd, focus) {
                unsafe { let _ = KillTimer(Some(hwnd), HOLD_TIMER_ID); }
                set_target(hwnd, 0.0);
            }
            LRESULT(0)
        }
        WM_POWERBROADCAST => {
            match wp.0 {
                PBT_APMPOWERSTATUSCHANGE => on_power_change(hwnd),
                PBT_POWERSETTINGCHANGE => on_power_setting(hwnd, lp),
                _ => {}
            }
            LRESULT(1) // TRUE: the message was handled
        }
        WM_DEVICECHANGE => {
            if wp.0 == DBT_DEVNODES_CHANGED {
                // Devices (Bluetooth included) come and go in bursts: compare once things have settled.
                unsafe { SetTimer(Some(hwnd), BT_TIMER_ID, BT_DEBOUNCE_MS, None) };
            }
            LRESULT(1)
        }
        watchers::WM_NETCHANGE => {
            unsafe { SetTimer(Some(hwnd), NET_TIMER_ID, NET_DEBOUNCE_MS, None) };
            LRESULT(0)
        }
        watchers::WM_VOLUME => {
            on_volume_change(hwnd);
            LRESULT(0)
        }
        weather::WM_WEATHER => {
            let fetched = *unsafe { Box::from_raw(lp.0 as *mut Option<weather::Weather>) };
            APP.with(|a| {
                if let Some(app) = a.borrow_mut().as_mut() {
                    app.weather = fetched.or(app.weather); // a failed refresh keeps the last reading
                    let next = if fetched.is_some() { WEATHER_MS } else { WEATHER_RETRY_MS };
                    unsafe { SetTimer(Some(hwnd), WEATHER_TIMER_ID, next, None) };
                    if glance_wanted(app) {
                        let _ = render(hwnd, app);
                    }
                }
            });
            LRESULT(0)
        }
        privacy::WM_PRIVACY => {
            APP.with(|a| {
                if let Some(app) = a.borrow_mut().as_mut() {
                    app.privacy = privacy::Privacy { mic: wp.0 & 1 != 0, camera: wp.0 & 2 != 0 };
                    app.compact_target = compact_wanted(app); // the pill widens to fit the icons
                    app.last_tick = Instant::now();
                    unsafe { SetTimer(Some(hwnd), TIMER_ID, FRAME_MS, None) };
                    update_reveal(hwnd, app); // mic/camera in use brings the notch out; stopping lets it hide
                    let _ = render(hwnd, app);
                }
            });
            LRESULT(0)
        }
        media::WM_MEDIA => {
            // Ownership of the boxed info was handed over by the media thread.
            on_media(hwnd, *unsafe { Box::from_raw(lp.0 as *mut Option<MediaInfo>) });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Scroll down = next page below, scroll up = previous page above (like any scrollable list).
            let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
            let current = APP.with(|a| a.borrow().as_ref().filter(|app| app.spring.pos > 0.9).map(|app| app.page));
            if let Some(page) = current {
                // On the clipboard page the wheel scrolls the list first, and only changes page at the ends.
                if page == CLIP_PAGE && clippage::on_wheel(hwnd, delta < 0) {
                    return LRESULT(0);
                }
                set_page(hwnd, if delta < 0 { (page + 1).min(PAGES - 1) } else { page.saturating_sub(1) });
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            let (item, can_clear) = clippage::menu_context(x, y);
            show_menu(hwnd, item, can_clear);
            LRESULT(0)
        }
        WM_CHAR => {
            clippage::on_char(hwnd, wp.0 as u16);
            LRESULT(0)
        }
        WM_SYSCOMMAND if (wp.0 & 0xFFF0) == SC_CLOSE as usize && clippage::is_typing() => LRESULT(0), // Alt+F4 in the search box must not quit the notch
        WM_KILLFOCUS => {
            clippage::on_kill_focus(hwnd);
            LRESULT(0)
        }
        clippage::WM_CLIPBOARDUPDATE => {
            clippage::on_clipboard_update(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            match wp.0 {
                clippage::CLIP_TIMER_ID => clippage::on_clip_timer(hwnd),
                clippage::PASTE_TIMER_ID => clippage::on_paste_timer(hwnd),
                HOLD_TIMER_ID => on_hold_expired(hwnd),
                EQ_TIMER_ID => on_eq_tick(hwnd),
                ROTATE_TIMER_ID => on_rotate_tick(hwnd),
                SYS_TIMER_ID => on_sys_tick(hwnd),
                BT_TIMER_ID => on_bt_timer(hwnd),
                WEATHER_TIMER_ID => weather::refresh(hwnd),
                NET_TIMER_ID => on_net_timer(hwnd),
                _ => on_timer(hwnd),
            }
            LRESULT(0)
        }
        WM_APP => {
            // Ownership of the boxed event was handed over by the server thread.
            on_event(hwnd, *unsafe { Box::from_raw(lp.0 as *mut Event) });
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn create_app(hwnd: HWND) -> Result<App> {
    unsafe {
        let scale = GetDpiForSystem() as f32 / 96.0;
        let size = ((WIN_SIZE.0 * scale) as i32, (WIN_SIZE.1 * scale) as i32);

        // Top-down 32-bit DIB: Direct2D draws premultiplied alpha, UpdateLayeredWindow blends it.
        let dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.0,
                biHeight: -size.1,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        SelectObject(dc, dib.into());

        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            // Software, not the GPU: the surface is tiny, and a GPU device reset (sleep/wake, driver update) would
            // otherwise leave every later EndDraw failing with "recreate target" while the rest of the app lives on.
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = factory.CreateDCRenderTarget(&props)?;
        let brush = rt.CreateSolidColorBrush(&GLASS_TINT, None)?;
        let sheen_stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: SHEEN_TOP } },
            D2D1_GRADIENT_STOP { position: 1.0, color: D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.0 } },
        ];
        let sheen_stops = rt.CreateGradientStopCollection(&sheen_stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        let sheen = rt.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 { X: 0.0, Y: 0.0 },
                endPoint: Vector2 { X: 0.0, Y: 1.0 },
            },
            None,
            &sheen_stops,
        )?;

        let white = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let format = |family: PCWSTR, weight: DWRITE_FONT_WEIGHT, px: f32| {
            dwrite.CreateTextFormat(
                family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL,
                px * scale, w!(""),
            )
        };
        let text = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0)?;
        let small = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 13.0)?;
        let icons = format(w!("Segoe MDL2 Assets"), DWRITE_FONT_WEIGHT_NORMAL, 18.0)?;
        icons.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        icons.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let badge = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0)?;
        badge.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let accent = rt.CreateSolidColorBrush(&ACCENT, None)?;
        let good = rt.CreateSolidColorBrush(&GOOD, None)?;
        let label = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 13.0)?;
        label.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let value = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0)?;
        value.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        value.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        let row = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 13.0)?;
        row.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        row.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        let stamp = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 11.0)?;
        stamp.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        stamp.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
        stamp.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        let big = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 28.0)?;
        let icon_big = format(w!("Segoe MDL2 Assets"), DWRITE_FONT_WEIGHT_NORMAL, 34.0)?;
        icon_big.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        icon_big.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let center = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_SEMI_BOLD, 12.0)?;
        center.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        center.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let tiny = format(w!("Segoe UI"), DWRITE_FONT_WEIGHT_NORMAL, 11.0)?;
        tiny.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        let ring_style = factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            },
            None,
        )?;

        let app = App {
            rt, brush, white, text, small, icons, event: None, show_event: false, media: None, art: None,
            label, value, row, stamp, clip: clipboard::View::default(), page: 0, page_spring: Spring::default(), stats: Stats::default(), sampler: Sampler::default(), sys_page: None, volume: None, vol_drag: false,
            privacy: privacy::Privacy::default(),
            notice: None,
            power_prev: {
                let (battery, on_ac) = sysinfo::power_status();
                notice::Power { battery, on_ac }
            },
            bt_prev: devices::connected(),
            net_prev: netlink::active(),
            vol_prev: volume::read(),
            brightness_prev: None,
            weather: None,
            icon_big,
            timeline: None, timeline_at: Instant::now(), seek_hold: Instant::now(),
            accent, good, sheen, badge, factory, ring_style, big, center, tiny, working: HashMap::new(), sources: HashMap::new(), focus: 0,
            dc, size, scale,
            spring: Spring::default(),
            target: 0.0,
            compact: Spring::default(),
            compact_target: 0.0,
            reveal: Spring::default(), // starts hidden: nothing to show yet
            reveal_target: 0.0,
            rotate: 0,
            rotate_running: false,
            eq_running: false,
            eq_start: Instant::now(),
            tracking: false,
            last_tick: Instant::now(),
        };
        render(hwnd, &app)?;
        Ok(app)
    }
}

fn main() -> Result<()> {
    // `notch done "title" --agent claude` acts as the CLI client; no args runs the notch.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--weather") {
        std::process::exit(weather::print_reading()); // the notch runs this as a short-lived child
    }
    if !args.is_empty() {
        std::process::exit(server::send_from_args(&args));
    }
    let Some(listener) = server::bind() else { return Ok(()) }; // already running
    server::announce_running();

    unsafe {
        // Without this Windows bitmap-scales the window on high-DPI screens (blurry, wrong size).
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None); // COM for WIC (album art), plus OLE for the clipboard and drag and drop
        let hinst = GetModuleHandleW(None)?;
        let class = w!("NotchWindow");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class, w!("Notch"), WS_POPUP,
            0, 0, 1, 1, None, None, Some(hinst.into()), None,
        )?;
        let app = create_app(hwnd)?;
        APP.with(|a| *a.borrow_mut() = Some(app));
        dnd::register_drop_target(hwnd); // dropping files, text or images on the notch adds them to the clipboard page
        let _ = AddClipboardFormatListener(hwnd); // WM_CLIPBOARDUPDATE whenever anything is copied
        server::spawn(listener, hwnd);
        media::spawn(hwnd);
        privacy::spawn(hwnd);
        watchers::start(hwnd);
        weather::refresh(hwnd); // the first reading; WM_WEATHER then schedules the next refresh
        // Brightness HUD source: Windows posts WM_POWERBROADCAST when the panel brightness changes.
        let _ = RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_BRIGHTNESS, DEVICE_NOTIFY_WINDOW_HANDLE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        trim_memory();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
