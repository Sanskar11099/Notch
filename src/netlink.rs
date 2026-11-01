//! Which network the PC is actually using (Wi-Fi, Ethernet, mobile...), its name and its link speed.
//! "Actually using" = the interface Windows would route to the internet over, so a VPN or a virtual switch
//! never masks the real adapter, and with both Wi-Fi and Ethernet up it picks the one in use.

use std::{ffi::c_void, ptr::null_mut};
use windows::Win32::{
    Foundation::HANDLE,
    NetworkManagement::{
        IpHelper::{FreeMibTable, GetBestInterface, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2},
        WiFi::{
            WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces,
            WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, wlan_interface_state_connected,
            wlan_intf_opcode_current_connection,
        },
    },
};

const IF_TYPE_ETHERNET: u32 = 6;
const IF_TYPE_WIFI: u32 = 71;
const IF_TYPE_WWAN: [u32; 2] = [243, 244]; // mobile broadband
const OPER_STATUS_UP: i32 = 1;
const HARDWARE_INTERFACE_BIT: u8 = 1;
const ROUTE_PROBE: u32 = 0x0101_0101; // 1.1.1.1: only used for a local route lookup, no packet is sent
const WLAN_CLIENT_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinkKind {
    WiFi,
    Ethernet,
    Mobile,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetLink {
    pub kind: LinkKind,
    pub name: String,        // Wi-Fi network name, or the adapter's name
    pub signal: Option<u8>,  // Wi-Fi signal quality 0-100
}

pub fn classify(if_type: u32) -> LinkKind {
    match if_type {
        IF_TYPE_ETHERNET => LinkKind::Ethernet,
        IF_TYPE_WIFI => LinkKind::WiFi,
        t if IF_TYPE_WWAN.contains(&t) => LinkKind::Mobile,
        _ => LinkKind::Other,
    }
}

/// A Wi-Fi network name from the fixed 32-byte buffer + length Windows gives.
pub fn ssid_from(len: u32, bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..(len as usize).min(bytes.len())]).trim_matches(['\0', ' ']).to_string()
}

fn alias_of(row: &MIB_IF_ROW2) -> String {
    let end = row.Alias.iter().position(|&c| c == 0).unwrap_or(row.Alias.len());
    String::from_utf16_lossy(&row.Alias[..end])
}

fn is_usable_nic(row: &MIB_IF_ROW2) -> bool {
    row.OperStatus.0 == OPER_STATUS_UP && row.InterfaceAndOperStatusFlags._bitfield & HARDWARE_INTERFACE_BIT != 0
}

/// (network name, signal quality) of the connected Wi-Fi interface.
fn wifi_details() -> Option<(String, u8)> {
    unsafe {
        let (mut negotiated, mut handle) = (0u32, HANDLE::default());
        if WlanOpenHandle(WLAN_CLIENT_VERSION, None, &mut negotiated, &mut handle) != 0 {
            return None; // WLAN service not running (e.g. no Wi-Fi adapter)
        }
        let mut found = None;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = null_mut();
        if WlanEnumInterfaces(handle, None, &mut list) == 0 && !list.is_null() {
            let infos = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
            for info in infos.iter().filter(|i| i.isState == wlan_interface_state_connected) {
                let (mut size, mut data) = (0u32, null_mut::<c_void>());
                let rc = WlanQueryInterface(
                    handle, &info.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None,
                );
                if rc == 0 && !data.is_null() {
                    let assoc = &(*(data as *const WLAN_CONNECTION_ATTRIBUTES)).wlanAssociationAttributes;
                    let ssid = ssid_from(assoc.dot11Ssid.uSSIDLength, &assoc.dot11Ssid.ucSSID);
                    found = Some((ssid, assoc.wlanSignalQuality.min(100) as u8));
                    WlanFreeMemory(data);
                    break;
                }
            }
            WlanFreeMemory(list as *const c_void);
        }
        WlanCloseHandle(handle, None);
        found
    }
}

/// The connection in use, or `None` when there is no route out (offline).
pub fn active() -> Option<NetLink> {
    let mut best = 0u32;
    if unsafe { GetBestInterface(ROUTE_PROBE, &mut best) } != 0 {
        return None;
    }
    let mut table: *mut MIB_IF_TABLE2 = null_mut();
    let link = unsafe {
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return None;
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        // The routed interface, unless that is a VPN/virtual adapter: then the first connected real NIC.
        let row = rows
            .iter()
            .find(|r| r.InterfaceIndex == best && is_usable_nic(r))
            .or_else(|| rows.iter().find(|r| is_usable_nic(r) && classify(r.Type) != LinkKind::Other));
        let link = row.map(|r| NetLink {
            kind: classify(r.Type),
            name: alias_of(r),
            signal: None,
        });
        FreeMibTable(table as *const c_void);
        link
    }?;
    Some(match link.kind {
        LinkKind::WiFi => match wifi_details() {
            Some((ssid, signal)) => NetLink {
                name: if ssid.is_empty() { link.name } else { ssid },
                signal: Some(signal),
                ..link
            },
            None => link,
        },
        _ => link,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_interface_types() {
        assert_eq!(classify(6), LinkKind::Ethernet);
        assert_eq!(classify(71), LinkKind::WiFi);
        assert_eq!(classify(243), LinkKind::Mobile);
        assert_eq!(classify(244), LinkKind::Mobile);
        assert_eq!(classify(24), LinkKind::Other); // loopback
        assert_eq!(classify(131), LinkKind::Other); // tunnel / VPN
    }

    #[test]
    fn decodes_ssids_from_fixed_buffers() {
        let mut buf = [0u8; 32];
        buf[..7].copy_from_slice(b"Home-5G");
        assert_eq!(ssid_from(7, &buf), "Home-5G");
        assert_eq!(ssid_from(0, &buf), "");
        assert_eq!(ssid_from(99, &buf).len(), 7); // a bad length can't read past the buffer
    }

    #[test]
    fn live_link_is_sane() {
        if let Some(l) = active() {
            assert!(l.signal.is_none_or(|s| s <= 100));
        }
    }

    // Manual check: `cargo test print_active_link -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_active_link() {
        println!("LINK {:?}", active());
    }
}
