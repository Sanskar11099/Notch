//! Event sources for the system popups that are not plain window messages: network interface changes and system
//! volume changes. Both callbacks run on Windows' own threads and do nothing but post a message to the UI thread,
//! so they cost nothing while nothing changes. (Power and Bluetooth changes arrive as ordinary window messages.)

use std::ffi::c_void;
use windows::{
    core::implement,
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, WPARAM},
        Media::Audio::{
            AUDIO_VOLUME_NOTIFICATION_DATA,
            Endpoints::{IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl},
        },
        NetworkManagement::IpHelper::{MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE, NotifyIpInterfaceChange},
        Networking::WinSock::AF_UNSPEC,
        UI::WindowsAndMessaging::{PostMessageW, WM_APP},
    },
};

/// A network interface changed (the UI waits a moment, then compares the connection in use with the last one).
pub const WM_NETCHANGE: u32 = WM_APP + 4;
/// The system volume or mute state changed (from the keyboard keys, the taskbar, another app...).
pub const WM_VOLUME: u32 = WM_APP + 5;

fn post(hwnd: isize, msg: u32) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
    }
}

unsafe extern "system" fn on_net_change(ctx: *const c_void, _row: *const MIB_IPINTERFACE_ROW, _kind: MIB_NOTIFICATION_TYPE) {
    post(ctx as isize, WM_NETCHANGE);
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatcher {
    hwnd: isize,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeWatcher_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        post(self.hwnd, WM_VOLUME);
        Ok(())
    }
}

/// Register both watchers. Call on the UI thread after COM is initialized. Registrations live for the whole
/// process (their handles are deliberately never released).
pub fn start(hwnd: HWND) {
    unsafe {
        let mut handle = HANDLE::default();
        let _ = NotifyIpInterfaceChange(AF_UNSPEC, Some(on_net_change), Some(hwnd.0 as *const c_void), false, &mut handle);
        // Volume: bound to the output device that is the default right now; if you switch devices later the HUD
        // stops following until the notch restarts.
        if let Some(endpoint) = crate::volume::endpoint() {
            let callback: IAudioEndpointVolumeCallback = VolumeWatcher { hwnd: hwnd.0 as isize }.into();
            if endpoint.RegisterControlChangeNotify(&callback).is_ok() {
                std::mem::forget(endpoint);
                std::mem::forget(callback);
            }
        }
    }
}
