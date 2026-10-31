//! Is anything using the microphone or camera right now? Windows records, per app, when it last started and
//! stopped using each (the data behind its own privacy indicator): an app is using the device while its
//! "last used start" is later than its "last used stop". A registry change notification wakes the watcher thread
//! only when something changes, so it costs nothing in between.

use std::thread;
use windows::{
    Win32::{
        Foundation::{ERROR_SUCCESS, HWND, LPARAM, WPARAM},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME,
            RRF_RT_REG_QWORD, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW,
        },
        UI::WindowsAndMessaging::{PostMessageW, WM_APP},
    },
    core::{PCWSTR, PWSTR, w},
};

/// Posted to the UI thread when the state changes; WPARAM bit 0 = microphone, bit 1 = camera.
pub const WM_PRIVACY: u32 = WM_APP + 3;

const CONSENT_STORE: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore");
const NON_PACKAGED: &str = "NonPackaged"; // desktop (Win32) apps live one level down, under this key
const MAX_KEY_NAME: usize = 512;

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Privacy {
    pub mic: bool,
    pub camera: bool,
}

impl Privacy {
    pub fn any(self) -> bool {
        self.mic || self.camera
    }

    pub fn bits(self) -> usize {
        self.mic as usize | (self.camera as usize) << 1
    }
}

/// An app is using the device while its last start is newer than its last stop (stop is 0 while active).
pub fn in_use(start: u64, stop: u64) -> bool {
    start > stop
}

fn read_qword(key: HKEY, name: PCWSTR) -> u64 {
    let (mut value, mut size) = (0u64, size_of::<u64>() as u32);
    let rc = unsafe { RegGetValueW(key, None, name, RRF_RT_REG_QWORD, None, Some((&raw mut value).cast()), Some(&mut size)) };
    if rc == ERROR_SUCCESS { value } else { 0 }
}

fn subkey_names(key: HKEY) -> Vec<String> {
    let mut names = Vec::new();
    for index in 0.. {
        let mut buf = [0u16; MAX_KEY_NAME];
        let mut len = buf.len() as u32;
        let rc = unsafe { RegEnumKeyExW(key, index, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None) };
        if rc != ERROR_SUCCESS {
            break;
        }
        names.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    names
}

fn open(parent: HKEY, path: &str) -> Option<HKEY> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut key = HKEY::default();
    (unsafe { RegOpenKeyExW(parent, PCWSTR(wide.as_ptr()), None, KEY_READ, &mut key) } == ERROR_SUCCESS).then_some(key)
}

/// True if any app under `capability` (e.g. "microphone") is using it. `key` is that capability's open key.
fn any_active(key: HKEY) -> bool {
    let mut active = false;
    for name in subkey_names(key) {
        let Some(app) = open(key, &name) else { continue };
        if name == NON_PACKAGED {
            active |= subkey_names(app).iter().filter_map(|n| open(app, n)).any(|k| {
                let on = in_use(read_qword(k, w!("LastUsedTimeStart")), read_qword(k, w!("LastUsedTimeStop")));
                unsafe { let _ = RegCloseKey(k); }
                on
            });
        } else {
            active |= in_use(read_qword(app, w!("LastUsedTimeStart")), read_qword(app, w!("LastUsedTimeStop")));
        }
        unsafe { let _ = RegCloseKey(app); }
        if active {
            break;
        }
    }
    active
}

/// Current state of both devices.
pub fn read() -> Privacy {
    let mut state = Privacy::default();
    let mut root = HKEY::default();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, CONSENT_STORE, None, KEY_READ, &mut root) } != ERROR_SUCCESS {
        return state;
    }
    if let Some(k) = open(root, "microphone") {
        state.mic = any_active(k);
        unsafe { let _ = RegCloseKey(k); }
    }
    if let Some(k) = open(root, "webcam") {
        state.camera = any_active(k);
        unsafe { let _ = RegCloseKey(k); }
    }
    unsafe { let _ = RegCloseKey(root); }
    state
}

/// Start the watcher: posts `WM_PRIVACY` to `hwnd` once at startup and again on every change.
pub fn spawn(hwnd: HWND) {
    let target = hwnd.0 as isize;
    thread::spawn(move || {
        let post = |state: Privacy| unsafe {
            let _ = PostMessageW(Some(HWND(target as *mut _)), WM_PRIVACY, WPARAM(state.bits()), LPARAM(0));
        };
        let mut watched = HKEY::default();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, CONSENT_STORE, None, KEY_NOTIFY | KEY_READ, &mut watched) } != ERROR_SUCCESS {
            return; // no consent store (unusual): the dot just never appears
        }
        let mut last = read();
        post(last);
        loop {
            // Blocks until anything under the store changes, then re-reads; only a real change is posted.
            let rc = unsafe {
                RegNotifyChangeKeyValue(watched, true, REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME, None, false)
            };
            if rc != ERROR_SUCCESS {
                return;
            }
            let now = read();
            if now != last {
                last = now;
                post(now);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_use_means_started_after_last_stop() {
        assert!(in_use(200, 0)); // started, never stopped
        assert!(in_use(200, 100)); // restarted after an earlier stop
        assert!(!in_use(100, 200)); // stopped after it started
        assert!(!in_use(0, 0)); // never used
        assert!(!in_use(150, 150));
    }

    #[test]
    fn bits_round_trip() {
        assert_eq!(Privacy { mic: true, camera: false }.bits(), 1);
        assert_eq!(Privacy { mic: false, camera: true }.bits(), 2);
        assert_eq!(Privacy { mic: true, camera: true }.bits(), 3);
        assert!(!Privacy::default().any());
    }

    #[test]
    fn live_read_does_not_panic() {
        let _ = read(); // value depends on what is running; just prove the registry walk works
    }

    // Manual check: `cargo test print_privacy -- --ignored --nocapture` while an app records.
    #[test]
    #[ignore]
    fn print_privacy() {
        println!("PRIVACY {:?}", read());
    }
}
