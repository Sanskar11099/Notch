//! System stats for the notch's second page: battery, CPU, memory, disk. Plain Win32 calls only. The
//! caller samples (about once a second) while that page is visible and never otherwise.

use crate::{
    devices::{self, BtDevice},
    netlink::{self, NetLink},
};
use std::{collections::HashMap, time::Instant};
use windows::{
    Win32::{
        Foundation::FILETIME,
        Globalization::{GetTimeFormatEx, TIME_NOSECONDS},
        NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2},
        Storage::FileSystem::GetDiskFreeSpaceExW,
        System::{
            Performance::{
                PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
                PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
            },
            Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS},
            SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
            Threading::GetSystemTimes,
        },
    },
    core::{PCWSTR, w},
};

// SYSTEM_POWER_STATUS.BatteryFlag bits; 255 (unknown) also has the NO_BATTERY bit set.
const NO_BATTERY: u8 = 128;
const CHARGING: u8 = 8;
const AC_ONLINE: u8 = 1;
// MIB_IF_ROW2: interface type for loopback, oper status "up", and bit 0 of the flag byte = physical NIC.
const IF_TYPE_LOOPBACK: u32 = 24;
const OPER_STATUS_UP: i32 = 1;
const HARDWARE_INTERFACE_BIT: u8 = 1;
const PDH_MORE_DATA: u32 = 0x800007D2;
const PDH_STATUS_VALID: u32 = 0;
const PDH_STATUS_NEW_DATA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

#[derive(Clone, Default)]
pub struct Stats {
    pub battery: Option<Battery>, // None on a desktop, or when Windows can't tell
    pub on_ac: bool,
    pub cpu: f32,  // all percentages 0..=100
    pub mem: f32,
    pub disk: f32, // C: used
    pub net_down: f32, // bytes per second, all connected physical adapters combined
    pub net_up: f32,
    pub gpu: Option<f32>, // None until the second reading, or if Windows has no GPU counters
    pub temp_c: Option<f32>, // hottest ACPI thermal zone; None if Windows exposes no thermal zones
    pub bluetooth: Vec<BtDevice>, // connected devices only
    pub link: Option<NetLink>, // connection in use (Wi-Fi / Ethernet ...); None = offline
}

/// GPU busy %, the way Task Manager computes it: per adapter and engine type (3D, copy, video...) add up
/// every process's utilization, then report the busiest engine. Instance names look like
/// `pid_1234_luid_0x00000000_0x0000F3C2_phys_0_eng_0_engtype_3D`.
pub fn gpu_percent<'a>(instances: impl IntoIterator<Item = (&'a str, f64)>) -> f32 {
    let mut busy: HashMap<(&str, &str), f64> = HashMap::new();
    for (name, util) in instances {
        let (Some(luid), Some(engine)) = (name.split("luid_").nth(1), name.split("engtype_").nth(1)) else { continue };
        let luid = luid.split("_phys").next().unwrap_or(luid);
        if engine.is_empty() {
            continue; // unnamed engines carry no useful type
        }
        *busy.entry((luid, engine)).or_default() += util;
    }
    busy.values().fold(0.0f64, |m, v| m.max(*v)).clamp(0.0, 100.0) as f32
}

/// A PDH counter with wildcard instances, e.g. every GPU engine or every thermal zone. Opened on demand and
/// closed when dropped; a fresh one is used each time a page opens so newly started processes are picked up.
struct PdhArray {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl PdhArray {
    fn open(path: PCWSTR) -> Option<Self> {
        let (mut query, mut counter) = (PDH_HQUERY::default(), PDH_HCOUNTER::default());
        unsafe {
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            if PdhAddEnglishCounterW(query, path, 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            PdhCollectQueryData(query); // baseline: rate counters (GPU) need two collections
        }
        Some(Self { query, counter })
    }

    /// (instance name, value) for every instance.
    fn read(&self) -> Option<Vec<(String, f64)>> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let (mut size, mut count) = (0u32, 0u32);
            let rc = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if rc != PDH_MORE_DATA || size == 0 {
                return None;
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)]; // u64s keep the item array aligned
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return None;
            }
            let items = std::slice::from_raw_parts(items, count as usize);
            Some(
                items
                    .iter()
                    .filter(|i| matches!(i.FmtValue.CStatus, PDH_STATUS_VALID | PDH_STATUS_NEW_DATA))
                    .filter_map(|i| Some((i.szName.to_string().ok()?, i.FmtValue.Anonymous.doubleValue)))
                    .collect(),
            )
        }
    }
}

/// Hottest thermal zone in Celsius. The "High Precision Temperature" counter is in tenths of a Kelvin;
/// readings outside a sane range (a zone that reports 0 or garbage) are ignored.
pub fn hottest_zone_celsius(zones: &[(String, f64)]) -> Option<f32> {
    zones
        .iter()
        .map(|(_, tenths_kelvin)| (tenths_kelvin / 10.0 - 273.15) as f32)
        .filter(|c| (0.0..150.0).contains(c))
        .reduce(f32::max)
}

impl Drop for PdhArray {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Elapsed time for the agents preview: "45s", "2m 14s", "1h 05m".
pub fn format_elapsed(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Human-readable speed: "0 B/s", "812 B/s", "12.3 KB/s", "1.4 MB/s", "2.0 GB/s".
pub fn format_speed(bytes_per_sec: f32) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let (mut v, mut unit) = (bytes_per_sec.max(0.0), 0);
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{} {}", v.round() as u32, UNITS[0]) } else { format!("{:.1} {}", v, UNITS[unit]) }
}

/// Bytes/second between two counter readings. A counter that went backwards (adapter reset or removed)
/// counts as zero instead of a huge bogus spike.
pub fn rate(prev: u64, now: u64, secs: f32) -> f32 {
    if now < prev || secs <= 0.0 { 0.0 } else { (now - prev) as f32 / secs }
}

/// Total received/sent bytes across physical, connected, non-loopback adapters.
fn net_octets() -> Option<(u64, u64)> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    unsafe {
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return None;
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let totals = rows
            .iter()
            .filter(|r| {
                r.Type != IF_TYPE_LOOPBACK
                    && r.OperStatus.0 == OPER_STATUS_UP
                    && r.InterfaceAndOperStatusFlags._bitfield & HARDWARE_INTERFACE_BIT != 0
            })
            .fold((0u64, 0u64), |(i, o), r| (i + r.InOctets, o + r.OutOctets));
        FreeMibTable(table as *const _);
        Some(totals)
    }
}

pub const LOW_BATTERY_PCT: u8 = 20;

/// How the battery bar should look: green whenever power is connected (charging, or full on the charger),
/// orange when low and unplugged, otherwise normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PowerLook {
    Normal,
    Low,
    Plugged,
}

pub fn power_look(battery: Option<Battery>, on_ac: bool) -> PowerLook {
    let plugged = on_ac || battery.is_some_and(|b| b.charging);
    match battery {
        _ if plugged => PowerLook::Plugged,
        Some(b) if b.percent <= LOW_BATTERY_PCT => PowerLook::Low,
        _ => PowerLook::Normal,
    }
}

pub fn decode_battery(flag: u8, percent: u8) -> Option<Battery> {
    if flag & NO_BATTERY != 0 || percent > 100 {
        return None;
    }
    Some(Battery { percent, charging: flag & CHARGING != 0 })
}

/// CPU busy % between two (idle, kernel, user) readings. Kernel time already includes idle time.
pub fn cpu_percent(prev: (u64, u64, u64), now: (u64, u64, u64)) -> f32 {
    let total = now.1.saturating_sub(prev.1) + now.2.saturating_sub(prev.2);
    if total == 0 {
        return 0.0;
    }
    let idle = now.0.saturating_sub(prev.0);
    (total.saturating_sub(idle) as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
}

fn filetime(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn cpu_times() -> Option<(u64, u64, u64)> {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    Some((filetime(idle), filetime(kernel), filetime(user)))
}

fn memory_percent() -> f32 {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_ok() { m.dwMemoryLoad as f32 } else { 0.0 }
}

fn disk_percent() -> f32 {
    let (mut free, mut total) = (0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(w!("C:\\"), Some(&mut free), Some(&mut total), None) }.is_ok();
    if ok && total > 0 { (total - free) as f32 / total as f32 * 100.0 } else { 0.0 }
}

/// CPU % and network speed are deltas, so the sampler remembers the previous readings. Each page samples only
/// what it shows (`sample_system` / `sample_devices`); the other page's last values are kept, not refreshed.
#[derive(Default)]
pub struct Sampler {
    prev: Option<(u64, u64, u64)>,
    prev_net: Option<(u64, u64, Instant)>,
    gpu: Option<PdhArray>,
    thermal: Option<PdhArray>,
    last: Stats,
}

impl Sampler {
    /// Forget the last readings (CPU and network speed start a fresh interval and read 0 until then).
    pub fn reset(&mut self) {
        self.prev = None;
        self.prev_net = None;
        self.gpu = None; // dropping closes the PDH query; the next sample opens a fresh one
        self.thermal = None;
    }

    /// Temperature, the connection in use, and connected Bluetooth devices.
    pub fn sample_devices(&mut self) -> Stats {
        // Temperature is a plain gauge, so the baseline collection made on open is enough for a first reading.
        if self.thermal.is_none() {
            self.thermal = PdhArray::open(w!(r"\Thermal Zone Information(*)\High Precision Temperature"));
        }
        self.last.temp_c = self.thermal.as_ref().and_then(|t| t.read()).and_then(|z| hottest_zone_celsius(&z));
        self.last.bluetooth = devices::connected();
        self.last.link = netlink::active();
        self.last.clone()
    }

    /// Battery, CPU, GPU, memory, disk and network.
    pub fn sample_system(&mut self) -> Stats {
        let cpu = self.cpu_now();
        let net_now = net_octets().map(|(i, o)| (i, o, Instant::now()));
        let (net_down, net_up) = match (self.prev_net, net_now) {
            (Some((pi, po, pt)), Some((i, o, t))) => {
                let secs = (t - pt).as_secs_f32();
                (rate(pi, i, secs), rate(po, o, secs))
            }
            _ => (0.0, 0.0),
        };
        self.prev_net = net_now;
        let gpu = match &self.gpu {
            Some(query) => query.read().map(|v| gpu_percent(v.iter().map(|(n, x)| (n.as_str(), *x)))),
            None => {
                self.gpu = PdhArray::open(w!(r"\GPU Engine(*)\Utilization Percentage")); // baseline: no value yet
                None
            }
        };
        let (battery, on_ac) = power_status();
        let last = &mut self.last;
        (last.battery, last.on_ac) = (battery, on_ac);
        (last.cpu, last.mem, last.disk) = (cpu, memory_percent(), disk_percent());
        (last.net_down, last.net_up, last.gpu) = (net_down, net_up, gpu);
        last.clone()
    }

    /// Just what the peek view's glance strip shows: battery and the connection in use.
    pub fn sample_glance(&mut self) -> Stats {
        let (battery, on_ac) = power_status();
        let last = &mut self.last;
        (last.battery, last.on_ac) = (battery, on_ac);
        last.link = netlink::active();
        last.clone()
    }

    /// CPU busy % since the previous call (0 for the first call after a reset).
    fn cpu_now(&mut self) -> f32 {
        let now = cpu_times();
        let cpu = match (self.prev, now) {
            (Some(p), Some(n)) => cpu_percent(p, n),
            _ => 0.0,
        };
        self.prev = now;
        cpu
    }
}

/// Battery (None on a desktop or when Windows can't tell) and whether the charger is connected.
pub fn power_status() -> (Option<Battery>, bool) {
    let mut power = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut power) }.is_err() {
        return (None, false);
    }
    (decode_battery(power.BatteryFlag, power.BatteryLifePercent), power.ACLineStatus == AC_ONLINE)
}

/// The current local time in the user's own format, without seconds ("10:42 AM", "22:42", ...).
pub fn clock_text() -> String {
    let mut buf = [0u16; 32];
    let written = unsafe { GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, None, PCWSTR::null(), Some(&mut buf)) };
    if written > 1 { String::from_utf16_lossy(&buf[..(written - 1) as usize]) } else { String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_battery_flags() {
        assert_eq!(decode_battery(0, 82), Some(Battery { percent: 82, charging: false }));
        assert_eq!(decode_battery(8, 40), Some(Battery { percent: 40, charging: true }));
        assert_eq!(decode_battery(128, 100), None); // no system battery (desktop)
        assert_eq!(decode_battery(255, 255), None); // unknown
        assert_eq!(decode_battery(0, 200), None);
    }

    #[test]
    fn battery_look_green_when_plugged_orange_when_low_and_unplugged() {
        let b = |percent, charging| Some(Battery { percent, charging });
        assert_eq!(power_look(b(80, true), true), PowerLook::Plugged); // charging
        assert_eq!(power_look(b(100, false), true), PowerLook::Plugged); // full, still on the charger
        assert_eq!(power_look(b(15, true), true), PowerLook::Plugged); // low but charging: not alarming
        assert_eq!(power_look(b(80, false), false), PowerLook::Normal); // on battery
        assert_eq!(power_look(b(20, false), false), PowerLook::Low); // threshold is inclusive
        assert_eq!(power_look(b(21, false), false), PowerLook::Normal);
        assert_eq!(power_look(None, true), PowerLook::Plugged); // desktop on mains
        assert_eq!(power_look(None, false), PowerLook::Normal);
    }

    #[test]
    fn converts_thermal_zone_readings_to_celsius() {
        // 3181 tenths of a Kelvin = 318.1 K = 44.95 C (what the counter reports on this PC)
        let z = |v: f64| ("\\_tz.thrm".to_string(), v);
        let c = hottest_zone_celsius(&[z(3181.0)]).unwrap();
        assert!((c - 44.95).abs() < 0.05, "{c}");
        // the hottest sane zone wins; 0 K and absurd values are ignored
        let c = hottest_zone_celsius(&[z(3181.0), z(3331.0), z(0.0), z(90000.0)]).unwrap();
        assert!((c - 59.95).abs() < 0.05, "{c}");
        assert_eq!(hottest_zone_celsius(&[]), None);
        assert_eq!(hottest_zone_celsius(&[z(0.0)]), None);
    }

    #[test]
    fn device_sample_is_sane() {
        let mut s = Sampler::default();
        let st = s.sample_devices();
        if let Some(t) = st.temp_c {
            assert!((0.0..150.0).contains(&t), "{t}");
        }
        for d in &st.bluetooth {
            assert!(d.battery.is_none_or(|b| b <= 100));
        }
    }

    #[test]
    fn computes_cpu_percent_from_time_deltas() {
        // 1000 ticks passed (kernel 600 incl. 400 idle, user 400): busy = 600 -> 60%
        assert!((cpu_percent((0, 0, 0), (400, 600, 400)) - 60.0).abs() < 0.01); // f32 rounding, not exact
        assert_eq!(cpu_percent((5, 5, 5), (5, 5, 5)), 0.0); // no time elapsed
        assert_eq!(cpu_percent((0, 0, 0), (1000, 1000, 0)), 0.0); // fully idle
    }

    // Manual check: `cargo test print_live_network_rates -- --ignored --nocapture` while transferring data.
    #[test]
    #[ignore]
    fn print_live_network_rates() {
        let mut s = Sampler::default();
        s.sample_system();
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let st = s.sample_system();
            println!("RATE down={} up={}", st.net_down as u64, st.net_up as u64);
        }
    }

    #[test]
    fn gpu_busiest_engine_per_adapter_wins() {
        let inst = [
            ("pid_1_luid_0x0_0xA_phys_0_eng_0_engtype_3D", 30.0),
            ("pid_2_luid_0x0_0xA_phys_0_eng_0_engtype_3D", 25.0), // same adapter+engine: adds to 55
            ("pid_1_luid_0x0_0xA_phys_0_eng_3_engtype_Copy", 70.0),
            ("pid_3_luid_0x0_0xB_phys_0_eng_0_engtype_3D", 10.0), // other adapter, kept separate
            ("pid_4_luid_0x0_0xA_phys_0_eng_9_engtype_", 99.0),    // unnamed engine ignored
            ("garbage", 99.0),
        ];
        assert_eq!(gpu_percent(inst), 70.0);
        assert_eq!(gpu_percent([("pid_1_luid_0x0_0xA_phys_0_eng_0_engtype_3D", 250.0)]), 100.0); // clamped
        assert_eq!(gpu_percent([]), 0.0);
    }

    // Manual check: `cargo test print_live_gpu -- --ignored --nocapture` while the GPU is busy.
    #[test]
    #[ignore]
    fn print_live_gpu() {
        let mut s = Sampler::default();
        s.sample_system();
        for _ in 0..6 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            println!("GPU {:?}", s.sample_system().gpu);
        }
    }

    #[test]
    fn clock_text_looks_like_a_time() {
        let t = clock_text();
        assert!(t.chars().filter(char::is_ascii_digit).count() >= 3, "{t:?}"); // e.g. "9:05 AM", "22:42"
        assert!(t.len() < 16, "{t:?}");
    }

    #[test]
    fn power_status_is_consistent() {
        let (battery, _on_ac) = power_status();
        if let Some(b) = battery {
            assert!(b.percent <= 100);
        }
    }

    #[test]
    fn formats_elapsed_time() {
        assert_eq!(format_elapsed(0), "0s");
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(60), "1m 00s");
        assert_eq!(format_elapsed(134), "2m 14s");
        assert_eq!(format_elapsed(3600), "1h 00m");
        assert_eq!(format_elapsed(3900), "1h 05m");
    }

    #[test]
    fn formats_speeds() {
        assert_eq!(format_speed(0.0), "0 B/s");
        assert_eq!(format_speed(812.0), "812 B/s");
        assert_eq!(format_speed(12.3 * 1024.0), "12.3 KB/s");
        assert_eq!(format_speed(1.4 * 1024.0 * 1024.0), "1.4 MB/s");
        assert_eq!(format_speed(2.0 * 1024.0 * 1024.0 * 1024.0), "2.0 GB/s");
        assert_eq!(format_speed(-5.0), "0 B/s");
    }

    #[test]
    fn rate_ignores_counter_resets_and_zero_intervals() {
        assert_eq!(rate(1000, 3000, 2.0), 1000.0);
        assert_eq!(rate(3000, 1000, 1.0), 0.0); // counter went backwards
        assert_eq!(rate(1000, 2000, 0.0), 0.0);
    }

    #[test]
    fn live_sample_is_sane() {
        let mut s = Sampler::default();
        s.sample_system();
        let st = s.sample_system();
        for v in [st.cpu, st.mem, st.disk] {
            assert!((0.0..=100.0).contains(&v), "{v}");
        }
        assert!(st.mem > 0.0 && st.disk > 0.0);
        assert!(st.net_down >= 0.0 && st.net_up >= 0.0);
    }
}
