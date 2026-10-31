//! Short system-event popups: "Charger connected", "AMK XS Flow1 connected · 50%", "Wi-Fi disconnected", the volume
//! HUD. This module only decides *whether* a change deserves a popup and what it says (pure functions, unit
//! tested); `main.rs` shows the result and `watchers.rs` supplies the events that trigger the comparisons.

use crate::{
    devices::BtDevice,
    netlink::{LinkKind, NetLink},
    sysinfo::{Battery, LOW_BATTERY_PCT},
};

pub const CRITICAL_BATTERY_PCT: u8 = 10;
const NOTICE_HOLD_MS: u32 = 2600;
const HUD_HOLD_MS: u32 = 1400; // the volume HUD should be quick: it re-arms on every key press

#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    Battery { percent: u8, plugged: bool },
    Glyph(&'static str), // a Segoe MDL2 Assets codepoint
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub icon: Icon,
    pub title: String,
    pub detail: String,
    pub level: Option<f32>, // 0..=1: draws a bar (the volume HUD)
    pub chime: bool,
    pub hold_ms: u32,
}

impl Notice {
    fn new(icon: Icon, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { icon, title: title.into(), detail: detail.into(), level: None, chime: false, hold_ms: NOTICE_HOLD_MS }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Power {
    pub battery: Option<Battery>,
    pub on_ac: bool,
}

impl Power {
    fn plugged(self) -> bool {
        self.on_ac || self.battery.is_some_and(|b| b.charging)
    }

    fn percent(self) -> Option<u8> {
        self.battery.map(|b| b.percent)
    }
}

/// A popup for a power change: charger connected/removed, battery low or critical (once each, on the way down),
/// or fully charged.
pub fn power_notice(prev: Power, now: Power) -> Option<Notice> {
    let (pct_prev, pct_now) = (prev.percent(), now.percent());
    let icon = Icon::Battery { percent: pct_now.unwrap_or(100), plugged: now.plugged() };
    let pct_text = pct_now.map_or(String::new(), |p| format!("{p}%"));

    if !prev.plugged() && now.plugged() {
        let detail = if now.battery.is_some_and(|b| b.charging) { format!("Charging \u{b7} {pct_text}") } else { pct_text };
        return Some(Notice::new(icon, "Charger connected", detail.trim_end_matches(" \u{b7} ")));
    }
    if prev.plugged() && !now.plugged() {
        return Some(Notice::new(icon, "On battery", pct_text));
    }
    if let (Some(p), Some(n)) = (pct_prev, pct_now) {
        if !now.plugged() && p > CRITICAL_BATTERY_PCT && n <= CRITICAL_BATTERY_PCT {
            return Some(Notice { chime: true, ..Notice::new(icon, "Battery critical", format!("{n}% \u{2014} plug in soon")) });
        }
        if !now.plugged() && p > LOW_BATTERY_PCT && n <= LOW_BATTERY_PCT {
            return Some(Notice { chime: true, ..Notice::new(icon, "Battery low", format!("{n}% remaining")) });
        }
        if now.plugged() && p < 100 && n >= 100 {
            return Some(Notice::new(icon, "Fully charged", "100%"));
        }
    }
    None
}

/// A popup for the first newly connected Bluetooth device, else the first one that disconnected.
pub fn bluetooth_notice(prev: &[BtDevice], now: &[BtDevice]) -> Option<Notice> {
    const GLYPH: Icon = Icon::Glyph("\u{E702}");
    if let Some(d) = now.iter().find(|d| !prev.iter().any(|p| p.name == d.name)) {
        let detail = d.battery.map_or("Connected".to_string(), |b| format!("Connected \u{b7} {b}%"));
        return Some(Notice::new(GLYPH, d.name.clone(), detail));
    }
    prev.iter()
        .find(|p| !now.iter().any(|d| d.name == p.name))
        .map(|p| Notice::new(GLYPH, p.name.clone(), "Disconnected"))
}

fn link_glyph(kind: LinkKind) -> &'static str {
    match kind {
        LinkKind::Ethernet => "\u{E839}",
        _ => "\u{E701}",
    }
}

fn link_name(link: &NetLink) -> String {
    match link.kind {
        LinkKind::WiFi => link.name.clone(),
        LinkKind::Ethernet => "Ethernet".to_string(),
        LinkKind::Mobile => "Mobile data".to_string(),
        LinkKind::Other => link.name.clone(),
    }
}

/// A popup when the connection in use appears, disappears or changes network. Speed and signal changes are ignored.
pub fn network_notice(prev: &Option<NetLink>, now: &Option<NetLink>) -> Option<Notice> {
    match (prev, now) {
        (None, Some(n)) => Some(Notice::new(Icon::Glyph(link_glyph(n.kind)), "Connected", link_name(n))),
        (Some(p), None) => Some(Notice::new(Icon::Glyph(link_glyph(p.kind)), "Disconnected", "No network")),
        (Some(p), Some(n)) if p.kind != n.kind || p.name != n.name => {
            Some(Notice::new(Icon::Glyph(link_glyph(n.kind)), "Switched network", link_name(n)))
        }
        _ => None,
    }
}

/// The volume HUD: a bar at the new level, or "Muted".
pub fn volume_notice(level: f32, muted: bool) -> Notice {
    let silent = muted || level <= 0.0;
    Notice {
        level: Some(if silent { 0.0 } else { level.clamp(0.0, 1.0) }),
        hold_ms: HUD_HOLD_MS,
        ..Notice::new(
            Icon::Glyph(if silent { "\u{E74F}" } else { "\u{E767}" }),
            if muted { "Muted" } else { "Volume" },
            format!("{:.0}%", level.clamp(0.0, 1.0) * 100.0),
        )
    }
}

/// The brightness HUD, built like the volume one: a bar at the new level.
pub fn brightness_notice(percent: u8) -> Notice {
    let percent = percent.min(100);
    Notice {
        level: Some(percent as f32 / 100.0),
        hold_ms: HUD_HOLD_MS,
        ..Notice::new(Icon::Glyph("\u{E706}"), "Brightness", format!("{percent}%")) // Segoe MDL2 E706 = brightness
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_hud_shows_the_level_and_clamps() {
        let n = brightness_notice(40);
        assert_eq!((n.title.as_str(), n.detail.as_str(), n.level), ("Brightness", "40%", Some(0.4)));
        assert_eq!(brightness_notice(250).detail, "100%"); // a bogus value can't overflow the bar
        assert!(brightness_notice(10).hold_ms < NOTICE_HOLD_MS);
        assert!(!brightness_notice(10).chime);
    }

    fn power(percent: u8, charging: bool, on_ac: bool) -> Power {
        Power { battery: Some(Battery { percent, charging }), on_ac }
    }

    #[test]
    fn charger_connected_and_removed() {
        let n = power_notice(power(79, false, false), power(79, true, true)).unwrap();
        assert_eq!((n.title.as_str(), n.detail.as_str()), ("Charger connected", "Charging \u{b7} 79%"));
        let n = power_notice(power(79, false, false), power(79, false, true)).unwrap(); // on mains, not charging
        assert_eq!((n.title.as_str(), n.detail.as_str()), ("Charger connected", "79%"));
        let n = power_notice(power(79, true, true), power(78, false, false)).unwrap();
        assert_eq!((n.title.as_str(), n.detail.as_str()), ("On battery", "78%"));
    }

    #[test]
    fn low_and_critical_battery_fire_once_on_the_way_down() {
        let low = power_notice(power(21, false, false), power(20, false, false)).unwrap();
        assert!(low.chime && low.title == "Battery low", "{low:?}");
        assert!(power_notice(power(20, false, false), power(19, false, false)).is_none()); // already low: quiet
        let critical = power_notice(power(11, false, false), power(10, false, false)).unwrap();
        assert!(critical.chime && critical.title == "Battery critical", "{critical:?}");
        assert!(power_notice(power(21, true, true), power(20, true, true)).is_none()); // charging: not alarming
    }

    #[test]
    fn fully_charged_and_no_change() {
        let n = power_notice(power(99, true, true), power(100, true, true)).unwrap();
        assert_eq!(n.title, "Fully charged");
        assert!(power_notice(power(79, false, false), power(78, false, false)).is_none()); // just drifting down
        assert!(power_notice(Power { battery: None, on_ac: true }, Power { battery: None, on_ac: true }).is_none());
    }

    fn bt(name: &str, battery: Option<u8>) -> BtDevice {
        BtDevice { name: name.into(), battery }
    }

    #[test]
    fn bluetooth_connect_with_battery_and_disconnect() {
        let n = bluetooth_notice(&[], &[bt("Buds", Some(50))]).unwrap();
        assert_eq!((n.title.as_str(), n.detail.as_str()), ("Buds", "Connected \u{b7} 50%"));
        let n = bluetooth_notice(&[], &[bt("Speaker", None)]).unwrap();
        assert_eq!(n.detail, "Connected");
        let n = bluetooth_notice(&[bt("Buds", Some(50))], &[]).unwrap();
        assert_eq!((n.title.as_str(), n.detail.as_str()), ("Buds", "Disconnected"));
        assert!(bluetooth_notice(&[bt("Buds", Some(50))], &[bt("Buds", Some(49))]).is_none()); // battery drift is not news
    }

    fn wifi(name: &str) -> Option<NetLink> {
        Some(NetLink { kind: LinkKind::WiFi, name: name.into(), signal: Some(80), speed_bps: 866_000_000 })
    }

    #[test]
    fn network_changes() {
        let n = network_notice(&None, &wifi("Home")).unwrap();
        assert_eq!((n.title.as_str(), n.detail.as_str()), ("Connected", "Home"));
        let n = network_notice(&wifi("Home"), &None).unwrap();
        assert_eq!(n.title, "Disconnected");
        let eth = Some(NetLink { kind: LinkKind::Ethernet, name: "Ethernet 3".into(), signal: None, speed_bps: 1_000_000_000 });
        let n = network_notice(&wifi("Home"), &eth).unwrap();
        assert_eq!((n.title.as_str(), n.detail.as_str()), ("Switched network", "Ethernet"));
        assert!(network_notice(&wifi("Home"), &wifi("Home")).is_none());
        assert!(network_notice(&None, &None).is_none());
    }

    #[test]
    fn volume_hud() {
        let n = volume_notice(0.52, false);
        assert_eq!((n.title.as_str(), n.detail.as_str(), n.level), ("Volume", "52%", Some(0.52)));
        let n = volume_notice(0.52, true);
        assert_eq!((n.title.as_str(), n.level), ("Muted", Some(0.0)));
        assert!(volume_notice(0.3, false).hold_ms < NOTICE_HOLD_MS); // the HUD is short
    }
}
