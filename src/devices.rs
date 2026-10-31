//! Connected Bluetooth devices and their battery level, read from the PnP properties Windows already keeps
//! for every device (the same data Settings > Bluetooth shows). No pairing and no radio access.
//! Classic headsets only have a battery value if they report one to Windows; then we show a dash.

use std::iter::once;
use windows::{
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                CM_GETIDLIST_FILTER_ENUMERATOR, CM_LOCATE_DEVNODE_NORMAL, CM_Get_DevNode_PropertyW,
                CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_Locate_DevNodeW,
            },
            Properties::DEVPROPTYPE,
        },
        Foundation::DEVPROPKEY,
    },
    core::{GUID, PCWSTR, w},
};

#[derive(Clone, Debug, PartialEq)]
pub struct BtDevice {
    pub name: String,
    pub battery: Option<u8>,
}

// Property keys (DEVPKEY_Device_IsConnected, the Bluetooth battery level, DEVPKEY_NAME).
const KEY_IS_CONNECTED: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x83DA6326_97A6_4088_9453_A1923F573B29), pid: 15 };
const KEY_BATTERY: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5), pid: 2 };
const KEY_NAME: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC), pid: 10 };

/// A device's own node (not one of its many service nodes): `BTHLE\DEV_<addr>` or `BTHENUM\DEV_<addr>`.
pub fn is_device_node(id: &str) -> bool {
    let id = id.to_ascii_uppercase();
    id.starts_with("BTHLE\\DEV_") || id.starts_with("BTHENUM\\DEV_")
}

/// Battery property bytes: a single byte, or a 32-bit little-endian value; anything above 100 is not a percentage.
pub fn parse_battery(bytes: &[u8]) -> Option<u8> {
    let value = match bytes.len() {
        1 => bytes[0] as u32,
        4 => u32::from_le_bytes(bytes.try_into().ok()?),
        _ => return None,
    };
    (value <= 100).then_some(value as u8)
}

/// UTF-16LE property bytes -> text, without the trailing NUL.
pub fn parse_name(bytes: &[u8]) -> String {
    let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&wide).trim_end_matches('\0').trim().to_string()
}

/// One device often appears as both a classic and an LE node; keep one entry, preferring a known battery.
pub fn merge(list: &mut Vec<BtDevice>, device: BtDevice) {
    match list.iter_mut().find(|d| d.name == device.name) {
        Some(existing) => existing.battery = existing.battery.max(device.battery),
        None => list.push(device),
    }
}

fn property(devinst: u32, key: &DEVPROPKEY) -> Option<Vec<u8>> {
    let (mut kind, mut size) = (DEVPROPTYPE::default(), 0u32);
    unsafe {
        let _ = CM_Get_DevNode_PropertyW(devinst, key, &mut kind, None, &mut size, 0); // first call reports the size
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if CM_Get_DevNode_PropertyW(devinst, key, &mut kind, Some(buf.as_mut_ptr()), &mut size, 0).0 != 0 {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

fn device_ids(enumerator: PCWSTR) -> Vec<String> {
    unsafe {
        let mut len = 0u32;
        if CM_Get_Device_ID_List_SizeW(&mut len, enumerator, CM_GETIDLIST_FILTER_ENUMERATOR).0 != 0 || len == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if CM_Get_Device_ID_ListW(enumerator, &mut buf, CM_GETIDLIST_FILTER_ENUMERATOR).0 != 0 {
            return Vec::new();
        }
        // The list is NUL-separated ids ending in a double NUL.
        buf.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
    }
}

/// Currently connected devices, sorted by name.
pub fn connected() -> Vec<BtDevice> {
    let mut found = Vec::new();
    for enumerator in [w!("BTHLE"), w!("BTHENUM")] {
        for id in device_ids(enumerator).into_iter().filter(|id| is_device_node(id)) {
            let wide: Vec<u16> = id.encode_utf16().chain(once(0)).collect();
            let mut devinst = 0u32;
            if unsafe { CM_Locate_DevNodeW(&mut devinst, PCWSTR(wide.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) }.0 != 0 {
                continue;
            }
            let is_connected = property(devinst, &KEY_IS_CONNECTED).is_some_and(|b| b.first().is_some_and(|v| *v != 0));
            if !is_connected {
                continue;
            }
            let name = property(devinst, &KEY_NAME).map(|b| parse_name(&b)).filter(|n| !n.is_empty());
            let battery = property(devinst, &KEY_BATTERY).and_then(|b| parse_battery(&b));
            merge(&mut found, BtDevice { name: name.unwrap_or_else(|| "Bluetooth device".into()), battery });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_only_a_devices_own_node() {
        assert!(is_device_node(r"BTHLE\DEV_E4034A1C26A4\8&A08C6A1&0&E4034A1C26A4"));
        assert!(is_device_node(r"BTHENUM\DEV_081287195D17\8&2EEE4740&0&BLUETOOTHDEVICE_081287195D17"));
        assert!(!is_device_node(r"BTHLEDEVICE\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&0232C2\9&288946F&0&0017"));
        assert!(!is_device_node(r"BTHENUM\{0000110E-0000-1000-8000-00805F9B34FB}_VID&000102B0_PID&0000\8&2EEE4740"));
    }

    #[test]
    fn parses_battery_bytes() {
        assert_eq!(parse_battery(&[50]), Some(50));
        assert_eq!(parse_battery(&[90, 0, 0, 0]), Some(90));
        assert_eq!(parse_battery(&[101]), None);
        assert_eq!(parse_battery(&[255]), None); // "unknown" sentinel is not a percentage
        assert_eq!(parse_battery(&[1, 2, 3]), None);
        assert_eq!(parse_battery(&[]), None);
    }

    #[test]
    fn parses_names() {
        let bytes: Vec<u8> = "AMK XS Flow1\0".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        assert_eq!(parse_name(&bytes), "AMK XS Flow1");
    }

    #[test]
    fn merging_dedupes_and_prefers_a_known_battery() {
        let mut list = Vec::new();
        merge(&mut list, BtDevice { name: "Phone".into(), battery: None });
        merge(&mut list, BtDevice { name: "Phone".into(), battery: Some(70) }); // same device over LE
        merge(&mut list, BtDevice { name: "Buds".into(), battery: Some(40) });
        merge(&mut list, BtDevice { name: "Buds".into(), battery: None }); // must not erase the known value
        assert_eq!(list, vec![
            BtDevice { name: "Phone".into(), battery: Some(70) },
            BtDevice { name: "Buds".into(), battery: Some(40) },
        ]);
    }

    // Manual check: `cargo test print_connected -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_connected() {
        for d in connected() {
            println!("BT {:?} battery={:?}", d.name, d.battery);
        }
    }

    #[test]
    fn live_enumeration_is_sane() {
        for d in connected() {
            assert!(!d.name.is_empty());
            assert!(d.battery.is_none_or(|b| b <= 100));
        }
    }
}
