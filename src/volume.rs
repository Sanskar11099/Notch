//! System master volume and mute for the default output device: the same control the keyboard volume keys and
//! the taskbar speaker icon use (not a per-app volume). Each call looks up the current default device, so
//! switching to headphones or a Bluetooth speaker is followed automatically.

use std::ptr::null;
use windows::Win32::{
    Media::Audio::{Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender},
    System::Com::{CLSCTX_ALL, CoCreateInstance},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeState {
    pub level: f32, // 0.0 ..= 1.0
    pub muted: bool,
}

pub(crate) fn endpoint() -> Option<IAudioEndpointVolume> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
    }
}

/// Current level and mute state; `None` if there is no audio output.
pub fn read() -> Option<VolumeState> {
    let ep = endpoint()?;
    unsafe { Some(VolumeState { level: ep.GetMasterVolumeLevelScalar().ok()?, muted: ep.GetMute().ok()?.as_bool() }) }
}

/// Set the level. Raising it above zero also unmutes, as the Windows volume UI does.
pub fn set_level(level: f32) {
    let Some(ep) = endpoint() else { return };
    let level = level.clamp(0.0, 1.0);
    unsafe {
        let _ = ep.SetMasterVolumeLevelScalar(level, null());
        if level > 0.0 {
            let _ = ep.SetMute(false, null());
        }
    }
}

/// Flip mute; returns the new state.
pub fn toggle_mute() -> Option<bool> {
    let ep = endpoint()?;
    unsafe {
        let now = !ep.GetMute().ok()?.as_bool();
        ep.SetMute(now, null()).ok()?;
        Some(now)
    }
}

/// Slider position under the cursor: where `x` falls between `left` and `right`, clamped to 0..=1.
pub fn slider_level(x: f32, left: f32, right: f32) -> f32 {
    if right <= left {
        return 0.0;
    }
    ((x - left) / (right - left)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_maps_position_to_level_and_clamps() {
        assert_eq!(slider_level(100.0, 100.0, 300.0), 0.0);
        assert_eq!(slider_level(200.0, 100.0, 300.0), 0.5);
        assert_eq!(slider_level(300.0, 100.0, 300.0), 1.0);
        assert_eq!(slider_level(50.0, 100.0, 300.0), 0.0); // dragged past the left end
        assert_eq!(slider_level(999.0, 100.0, 300.0), 1.0); // dragged past the right end
        assert_eq!(slider_level(5.0, 10.0, 10.0), 0.0); // degenerate track
    }

    // The notch initializes COM on its UI thread at startup; a bare test thread has to do it itself.
    fn init_com() {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    #[test]
    fn live_read_is_sane() {
        init_com();
        if let Some(v) = read() {
            assert!((0.0..=1.0).contains(&v.level), "{v:?}");
        }
    }

    // Manual check: `cargo test print_volume -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_volume() {
        init_com();
        println!("VOLUME {:?}", read());
    }
}
