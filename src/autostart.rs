//! Start-at-login via the per-user Run key (no admin needed). "Enabled" means the value points at
//! *this* exe, so a moved or rebuilt-elsewhere binary reads as disabled instead of lying.

use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
        },
    },
    core::{PCWSTR, w},
};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const NAME: &str = "Notch";
const MAX_VALUE_U16: usize = 1024;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn command() -> Option<String> {
    Some(format!("\"{}\"", std::env::current_exe().ok()?.display()))
}

fn get(name: &str) -> Option<String> {
    let name = wide(name);
    let mut buf = [0u16; MAX_VALUE_U16];
    let mut bytes = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER, RUN_KEY, PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None,
            Some(buf.as_mut_ptr().cast()), Some(&mut bytes),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn set(name: &str, value: &str) -> bool {
    let (name, value) = (wide(name), wide(value));
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER, RUN_KEY, PCWSTR(name.as_ptr()), REG_SZ.0,
            Some(value.as_ptr().cast()), (value.len() * 2) as u32,
        )
    };
    rc == ERROR_SUCCESS
}

fn remove(name: &str) -> bool {
    let name = wide(name);
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, PCWSTR(name.as_ptr())) == ERROR_SUCCESS }
}

pub fn is_enabled() -> bool {
    command().is_some_and(|c| get(NAME) == Some(c))
}

pub fn toggle() {
    if is_enabled() {
        remove(NAME);
    } else if let Some(c) = command() {
        set(NAME, &c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_roundtrip_with_spaces_in_path() {
        let (name, value) = ("NotchTest", r#""C:\Program Files\x y\a.exe""#);
        remove(name);
        assert!(set(name, value));
        assert_eq!(get(name).as_deref(), Some(value));
        assert!(remove(name));
        assert_eq!(get(name), None);
    }
}
