//! Now-playing via Windows GSMTC. Fully event-driven: a parked worker thread owns the manager, and
//! WinRT change events (session / track / play state) post a `MediaInfo` to the UI thread.
//! Nothing polls, so idle cost is zero.

use std::{sync::Mutex, thread};
use windows::{
    Foundation::TypedEventHandler,
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    },
    Storage::Streams::{DataReader, IRandomAccessStreamReference},
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx},
        UI::WindowsAndMessaging::{PostMessageW, WM_APP},
    },
    core::Result,
};

/// Posted to the UI thread; LPARAM is a `Box<Option<MediaInfo>>` the receiver owns (None = nothing playing).
pub const WM_MEDIA: u32 = WM_APP + 1;
const MAX_ART_BYTES: u32 = 4 * 1024 * 1024;

pub struct MediaInfo {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub art: Option<Vec<u8>>, // encoded PNG/JPEG; the UI decodes and drops it
}

#[derive(Clone, Copy)]
pub enum Cmd {
    Prev,
    Toggle,
    Next,
}

struct Bound {
    session: Session,
    tokens: (i64, i64),
}

static CURRENT: Mutex<Option<Bound>> = Mutex::new(None);

fn read_art(r: &IRandomAccessStreamReference) -> Result<Vec<u8>> {
    let stream = r.OpenReadAsync()?.join()?;
    let size = stream.Size()? as u32;
    if size == 0 || size > MAX_ART_BYTES {
        return Err(windows::core::Error::empty());
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    reader.LoadAsync(size)?.join()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf)?;
    Ok(buf)
}

fn read(session: &Session) -> Result<MediaInfo> {
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
    let art = props.Thumbnail().ok().and_then(|t| read_art(&t).ok());
    Ok(MediaInfo { title: props.Title()?.to_string(), artist: props.Artist()?.to_string(), playing, art })
}

fn publish(hwnd: isize, info: Option<MediaInfo>) {
    let ptr = Box::into_raw(Box::new(info));
    let posted = unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_MEDIA, WPARAM(0), LPARAM(ptr as isize)) };
    if posted.is_err() {
        drop(unsafe { Box::from_raw(ptr) });
    }
}

fn update(hwnd: isize) {
    let session = CURRENT.lock().ok().and_then(|g| g.as_ref().map(|b| b.session.clone()));
    publish(hwnd, session.and_then(|s| read(&s).ok()));
}

fn rebind(mgr: &Manager, hwnd: isize) -> Result<()> {
    let new = mgr.GetCurrentSession().ok();
    let bound = match new {
        Some(session) => {
            let t1 = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                update(hwnd);
                Ok(())
            }))?;
            let t2 = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                update(hwnd);
                Ok(())
            }))?;
            Some(Bound { session, tokens: (t1, t2) })
        }
        None => None,
    };
    if let Ok(mut guard) = CURRENT.lock() {
        if let Some(old) = guard.take() {
            let _ = old.session.RemoveMediaPropertiesChanged(old.tokens.0);
            let _ = old.session.RemovePlaybackInfoChanged(old.tokens.1);
        }
        *guard = bound;
    }
    update(hwnd);
    Ok(())
}

pub fn spawn(hwnd: HWND) {
    let target = hwnd.0 as isize;
    thread::spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let Ok(mgr) = Manager::RequestAsync().and_then(|op| op.join()) else { return };
        let watcher = mgr.clone();
        let handler = TypedEventHandler::new(move |_, _| {
            let _ = rebind(&watcher, target);
            Ok(())
        });
        let _ = mgr.CurrentSessionChanged(&handler);
        let _ = rebind(&mgr, target);
        // Keep the MTA (and so the event registrations) alive for the life of the process.
        loop {
            thread::park();
        }
    });
}

pub fn command(cmd: Cmd) {
    let Ok(guard) = CURRENT.lock() else { return };
    let Some(bound) = guard.as_ref() else { return };
    let _ = match cmd {
        Cmd::Prev => bound.session.TrySkipPreviousAsync().map(drop),
        Cmd::Toggle => bound.session.TryTogglePlayPauseAsync().map(drop),
        Cmd::Next => bound.session.TrySkipNextAsync().map(drop),
    };
}

/// Where the current track is, for the progress bar. All in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeline {
    pub position_ms: u64,
    pub duration_ms: u64, // 0 = the player reports no length (live stream, or it doesn't publish a timeline)
    pub seekable: bool,
}

const TICKS_PER_MS: i64 = 10_000; // WinRT TimeSpan / DateTime tick = 100 ns
const UNIX_TO_WINDOWS_EPOCH_TICKS: i64 = 116_444_736_000_000_000; // 1601-01-01 -> 1970-01-01

fn windows_now_ticks() -> i64 {
    let since_unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    UNIX_TO_WINDOWS_EPOCH_TICKS + since_unix.as_secs() as i64 * 10_000_000 + since_unix.subsec_nanos() as i64 / 100
}

/// The position a player last reported plus the time that has passed since (at normal speed), clamped to the
/// track. Players publish the position only when it changes discontinuously (play, pause, seek), not every second.
pub fn extrapolate(position_ms: u64, age_ms: i64, playing: bool, duration_ms: u64) -> u64 {
    let now = if playing && age_ms > 0 { position_ms + age_ms as u64 } else { position_ms };
    if duration_ms > 0 { now.min(duration_ms) } else { now }
}

/// "3:07", or "1:02:09" for an hour or more.
pub fn format_clock(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Current track position for the session being shown. A plain property read: cheap enough for a 1 s tick.
pub fn timeline() -> Option<Timeline> {
    let guard = CURRENT.lock().ok()?;
    let session = &guard.as_ref()?.session;
    let tl = session.GetTimelineProperties().ok()?;
    let info = session.GetPlaybackInfo().ok()?;
    let ms = |ticks: i64| ticks.max(0) as u64 / TICKS_PER_MS as u64;
    let (start, end, pos) = (tl.StartTime().ok()?.Duration, tl.EndTime().ok()?.Duration, tl.Position().ok()?.Duration);
    let duration_ms = ms(end - start);
    let age_ms = (windows_now_ticks() - tl.LastUpdatedTime().ok()?.UniversalTime) / TICKS_PER_MS;
    let playing = info.PlaybackStatus().ok()? == PlaybackStatus::Playing;
    Some(Timeline {
        position_ms: extrapolate(ms(pos - start), age_ms, playing, duration_ms),
        duration_ms,
        seekable: info.Controls().ok()?.IsPlaybackPositionEnabled().unwrap_or(false),
    })
}

/// Jump to `position_ms` in the current track (the player decides whether it allows it).
pub fn seek(position_ms: u64) {
    let Ok(guard) = CURRENT.lock() else { return };
    let Some(bound) = guard.as_ref() else { return };
    let start = bound.session.GetTimelineProperties().ok().and_then(|t| t.StartTime().ok()).map_or(0, |t| t.Duration);
    let _ = bound.session.TryChangePlaybackPositionAsync(start + position_ms as i64 * TICKS_PER_MS).map(drop);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_clock_times() {
        assert_eq!(format_clock(0), "0:00");
        assert_eq!(format_clock(7_000), "0:07");
        assert_eq!(format_clock(187_000), "3:07");
        assert_eq!(format_clock(3_729_000), "1:02:09");
    }

    #[test]
    fn extrapolates_position_only_while_playing_and_clamps() {
        assert_eq!(extrapolate(10_000, 2_500, true, 200_000), 12_500); // 2.5 s since the player's last report
        assert_eq!(extrapolate(10_000, 2_500, false, 200_000), 10_000); // paused: stays put
        assert_eq!(extrapolate(10_000, -50, true, 200_000), 10_000); // clock skew: never goes backwards
        assert_eq!(extrapolate(199_000, 5_000, true, 200_000), 200_000); // can't pass the end
        assert_eq!(extrapolate(5_000, 1_000, true, 0), 6_000); // unknown length: no clamp
    }
}
