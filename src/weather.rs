//! Current weather for the glance strip: position from the Windows location service, conditions from Open-Meteo
//! (free, no key). A refresh runs on its own short-lived thread and posts the result to the UI thread, so nothing
//! blocks the window and nothing runs between refreshes (the UI timer fires every 30 minutes).

use std::{
    io::Read,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use windows::{
    Devices::Geolocation::Geolocator,
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Networking::WinHttp::{
            INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WinHttpCloseHandle,
            WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
            WinHttpSetTimeouts,
        },
        UI::WindowsAndMessaging::{PostMessageW, WM_APP},
    },
    core::{HSTRING, PCWSTR, w},
};

const MAX_RESPONSE: usize = 64 * 1024; // the reply is ~300 bytes; refuse anything absurd
const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Posted to the UI thread; LPARAM is a `Box<Option<Weather>>` the receiver owns (None = the refresh failed).
pub const WM_WEATHER: u32 = WM_APP + 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weather {
    pub temp_c: f32,
    pub code: u8, // WMO weather code
}

impl Weather {
    /// "☀ 18°": a condition symbol and the rounded temperature.
    pub fn short_text(&self) -> String {
        let degrees = self.temp_c.round() + 0.0; // + 0.0 turns -0 into 0
        format!("{} {degrees:.0}\u{b0}", symbol(self.code))
    }
}

/// A plain Unicode symbol for a WMO weather code (0 clear ... 99 thunderstorm hail).
fn symbol(code: u8) -> &'static str {
    match code {
        0 => "\u{2600}",                 // sun
        1 | 2 => "\u{26C5}",             // sun behind cloud
        3 | 45 | 48 => "\u{2601}",       // cloud, fog
        51..=67 | 80..=82 => "\u{2614}", // drizzle, rain, showers
        71..=77 | 85 | 86 => "\u{2744}", // snow
        95..=99 => "\u{26A1}",           // thunderstorm
        _ => "\u{2601}",
    }
}

/// Start a refresh in the background; the result arrives as `WM_WEATHER`.
pub fn refresh(hwnd: HWND) {
    let target = hwnd.0 as isize;
    thread::spawn(move || {
        let ptr = Box::into_raw(Box::new(fetch_in_child()));
        let posted = unsafe { PostMessageW(Some(HWND(target as *mut _)), WM_WEATHER, WPARAM(0), LPARAM(ptr as isize)) };
        if posted.is_err() {
            drop(unsafe { Box::from_raw(ptr) });
        }
    });
}

/// `notch.exe --weather`: look up the weather, print "temp code" and exit (status 1 on failure). Running the lookup
/// in its own short-lived process keeps the location, web and TLS libraries out of the notch's memory.
pub fn print_reading() -> i32 {
    match fetch() {
        Some(w) => {
            println!("{} {}", w.temp_c, w.code);
            0
        }
        None => 1,
    }
}

/// Run `print_reading` in a child copy of this exe and parse what it prints. Killed if it takes too long
/// (a location lookup can hang).
fn fetch_in_child() -> Option<Weather> {
    let mut child = Command::new(std::env::current_exe().ok()?)
        .arg("--weather")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() > CHILD_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(200));
    };
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    if status.success() { parse_reading(&out) } else { None }
}

fn parse_reading(line: &str) -> Option<Weather> {
    let (temp, code) = line.trim().split_once(' ')?;
    Some(Weather { temp_c: temp.parse().ok()?, code: code.parse().ok()? })
}

fn fetch() -> Option<Weather> {
    let (lat, lon) = position()?;
    // Two decimals is about 1 km: plenty for weather, and no more precise than it needs to be.
    let path = format!("/v1/forecast?latitude={lat:.2}&longitude={lon:.2}&current=temperature_2m,weather_code");
    parse(&https_get("api.open-meteo.com", &path)?)
}

/// A minimal HTTPS GET through WinHTTP (system proxy and certificate store, no extra dependency).
fn https_get(host: &str, path: &str) -> Option<String> {
    unsafe {
        let session = WinHttpOpen(w!("Notch"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            return None;
        }
        let _ = WinHttpSetTimeouts(session, 5_000, 5_000, 5_000, 10_000);
        let body = (|| {
            let connect = WinHttpConnect(session, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0);
            if connect.is_null() {
                return None;
            }
            let request = WinHttpOpenRequest(
                connect, w!("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE,
            );
            let text = (|| {
                if request.is_null()
                    || WinHttpSendRequest(request, None, None, 0, 0, 0).is_err()
                    || WinHttpReceiveResponse(request, std::ptr::null_mut()).is_err()
                {
                    return None;
                }
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let mut read = 0u32;
                    WinHttpReadData(request, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut read).ok()?;
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..read as usize]);
                    if bytes.len() > MAX_RESPONSE {
                        return None;
                    }
                }
                String::from_utf8(bytes).ok()
            })();
            let _ = WinHttpCloseHandle(request);
            let _ = WinHttpCloseHandle(connect);
            text
        })();
        let _ = WinHttpCloseHandle(session);
        body
    }
}

fn position() -> Option<(f64, f64)> {
    let point = Geolocator::new().ok()?.GetGeopositionAsync().ok()?.join().ok()?.Coordinate().ok()?.Point().ok()?;
    let p = point.Position().ok()?;
    Some((p.Latitude, p.Longitude))
}

fn parse(json: &str) -> Option<Weather> {
    let current = &json[json.find("\"current\":{")?..]; // not "current_units", which holds the unit strings
    Some(Weather { temp_c: number_after(current, "\"temperature_2m\":")?, code: number_after(current, "\"weather_code\":")? as u8 })
}

fn number_after(text: &str, key: &str) -> Option<f32> {
    let rest = &text[text.find(key)? + key.len()..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"latitude":12.5,"longitude":77.5,"current_units":{"time":"iso8601","temperature_2m":"°C","weather_code":"wmo code"},"current":{"time":"2026-10-31T08:15","interval":900,"temperature_2m":26.6,"weather_code":1}}"#;

    #[test]
    fn parses_the_current_block_not_the_units_block() {
        assert_eq!(parse(SAMPLE), Some(Weather { temp_c: 26.6, code: 1 }));
        assert_eq!(parse(r#"{"current":{"temperature_2m":-3.4,"weather_code":71}}"#), Some(Weather { temp_c: -3.4, code: 71 }));
        assert_eq!(parse("{}"), None);
        assert_eq!(parse(r#"{"current":{"temperature_2m":5.0}}"#), None); // a missing field is a failed refresh
    }

    #[test]
    fn short_text_rounds_and_never_shows_negative_zero() {
        assert_eq!(Weather { temp_c: 26.6, code: 0 }.short_text(), "\u{2600} 27\u{b0}");
        assert_eq!(Weather { temp_c: -0.2, code: 71 }.short_text(), "\u{2744} 0\u{b0}");
        assert_eq!(Weather { temp_c: -5.5, code: 61 }.short_text(), "\u{2614} -6\u{b0}");
    }

    #[test]
    fn parses_what_the_child_prints() {
        assert_eq!(parse_reading("27.3 0\r\n"), Some(Weather { temp_c: 27.3, code: 0 }));
        assert_eq!(parse_reading("-4 71"), Some(Weather { temp_c: -4.0, code: 71 }));
        assert_eq!(parse_reading(""), None);
        assert_eq!(parse_reading("hot 3"), None);
    }

    #[test]
    fn maps_weather_codes_to_symbols() {
        assert_eq!(symbol(0), "\u{2600}");
        assert_eq!(symbol(63), "\u{2614}");
        assert_eq!(symbol(96), "\u{26A1}");
        assert_eq!(symbol(200), "\u{2601}"); // unknown codes fall back to a cloud
    }

    // Manual check on a real machine: `cargo test print_live_weather -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_live_weather() {
        println!("WEATHER {:?} at {:?}", fetch(), position());
    }
}
