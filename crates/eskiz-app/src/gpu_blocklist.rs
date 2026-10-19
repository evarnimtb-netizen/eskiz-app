//! A small, data-driven list of graphics drivers Eskiz will not use, in the style of Blender's.
//!
//! A [`Rule`] matches an adapter by PCI vendor id and, optionally, the OS, the wgpu backend, a
//! piece of the adapter name and a half-open driver-version range (`driver_from <= v <
//! driver_below`). A matching adapter is not chosen on that rung of the renderer ladder; if no
//! other adapter is left, the rung fails before the window opens and the next one is tried
//! ([`crate::renderer`]). A renderer the user picks (`--renderer`, `ESKIZ_BACKEND`, Settings)
//! ignores the list.
//!
//! [`RULES`] ships empty: add a rule only for a driver that is known to crash or freeze, with the
//! bug report in `reason`. Example (not active):
//!
//! ```text
//! Rule { vendor: INTEL, os: Some("windows"), backend: Some(wgpu::Backend::Vulkan), name_contains: None,
//!        driver_from: None, driver_below: Some("31.0.101.2000"), reason: "freezes on the first present" }
//! ```

use eframe::wgpu;

pub const INTEL: u32 = 0x8086;
pub const NVIDIA: u32 = 0x10de;
pub const AMD: u32 = 0x1002;

/// The parts of `wgpu::AdapterInfo` the ladder looks at (plain data, so tests can build them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub device_type: wgpu::DeviceType,
    pub backend: wgpu::Backend,
    pub driver: String,
    pub driver_info: String,
}

impl Adapter {
    pub fn from_info(info: &wgpu::AdapterInfo) -> Self {
        Self {
            name: info.name.clone(),
            vendor: info.vendor,
            device: info.device,
            device_type: info.device_type,
            backend: info.backend,
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
        }
    }

    /// The driver version as numbers, from `driver` (DX12 puts it there) or `driver_info`
    /// (Vulkan and GL put it there, often after a name: "Mesa 24.0.5").
    pub fn driver_version(&self) -> Option<Vec<u32>> {
        parse_version(&self.driver).or_else(|| parse_version(&self.driver_info))
    }

    /// One line for the start-up log.
    pub fn describe(&self) -> String {
        format!(
            "'{}' vendor=0x{:04x} ({}) device=0x{:04x} type={:?} backend={:?} driver='{}' driver_info='{}'",
            self.name,
            self.vendor,
            vendor_name(self.vendor),
            self.device,
            self.device_type,
            self.backend,
            self.driver,
            self.driver_info
        )
    }
}

/// The vendor behind a PCI vendor id, for the log.
pub fn vendor_name(id: u32) -> &'static str {
    match id {
        INTEL => "Intel",
        NVIDIA => "NVIDIA",
        AMD => "AMD",
        0x106b => "Apple",
        0x1414 => "Microsoft",
        0x5143 => "Qualcomm",
        0x13b5 => "Arm",
        0x1010 => "Imagination",
        0x15ad => "VMware",
        0x1af4 => "virtio",
        0x10005 => "Mesa",
        _ => "unknown",
    }
}

/// The first dotted number with at least two parts in `text` ("31.0.101.2130", "Mesa 24.0.5").
pub fn parse_version(text: &str) -> Option<Vec<u32>> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map(|t| t.trim_matches('.'))
        .filter(|t| t.contains('.'))
        .find_map(|t| t.split('.').map(|n| n.parse::<u32>().ok()).collect::<Option<Vec<_>>>())
}

/// Compare versions, missing parts counting as 0.
fn cmp_version(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| a.get(i).copied().unwrap_or(0).cmp(&b.get(i).copied().unwrap_or(0)))
        .find(|o| o.is_ne())
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// One blocklist entry. `None` fields match anything.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub vendor: u32,
    /// `std::env::consts::OS`: "windows", "macos", "linux".
    pub os: Option<&'static str>,
    pub backend: Option<wgpu::Backend>,
    /// A substring of the adapter name, compared case-insensitively.
    pub name_contains: Option<&'static str>,
    /// First blocked driver version (inclusive).
    pub driver_from: Option<&'static str>,
    /// First driver version that works again (exclusive).
    pub driver_below: Option<&'static str>,
    pub reason: &'static str,
}

impl Rule {
    pub fn matches(&self, a: &Adapter, os: &str) -> bool {
        if a.vendor != self.vendor || self.os.is_some_and(|o| o != os) || self.backend.is_some_and(|b| b != a.backend) {
            return false;
        }
        if let Some(part) = self.name_contains {
            if !a.name.to_ascii_lowercase().contains(&part.to_ascii_lowercase()) {
                return false;
            }
        }
        if self.driver_from.is_none() && self.driver_below.is_none() {
            return true;
        }
        // A rule about driver versions never matches a driver whose version cannot be read.
        let Some(v) = a.driver_version() else { return false };
        let bound = |b: Option<&str>| b.and_then(parse_version);
        let from_ok = bound(self.driver_from).is_none_or(|f| cmp_version(&v, &f).is_ge());
        let below_ok = bound(self.driver_below).is_none_or(|b| cmp_version(&v, &b).is_lt());
        from_ok && below_ok
    }
}

/// The drivers Eskiz avoids. Empty for now; see the module comment for the format.
pub const RULES: &[Rule] = &[];

/// The first rule in `rules` that blocks `adapter` on `os`.
pub fn blocked<'a>(rules: &'a [Rule], adapter: &Adapter, os: &str) -> Option<&'a Rule> {
    rules.iter().find(|r| r.matches(adapter, os))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(vendor: u32, backend: wgpu::Backend, driver: &str, driver_info: &str) -> Adapter {
        Adapter {
            name: "Intel(R) UHD Graphics 620".into(),
            vendor,
            device: 0x5917,
            device_type: wgpu::DeviceType::IntegratedGpu,
            backend,
            driver: driver.into(),
            driver_info: driver_info.into(),
        }
    }

    #[test]
    fn versions_are_read_from_driver_or_driver_info() {
        assert_eq!(parse_version("31.0.101.2130"), Some(vec![31, 0, 101, 2130]));
        assert_eq!(parse_version("Mesa 24.0.5-1ubuntu1"), Some(vec![24, 0, 5]));
        assert_eq!(parse_version("4.6 (Core Profile) Mesa 23.2.1"), Some(vec![4, 6]));
        assert_eq!(parse_version("Intel Corporation"), None);
        assert_eq!(parse_version("v.1"), None);
        let a = adapter(INTEL, wgpu::Backend::Vulkan, "Intel Corporation", "Intel driver 101.2130");
        assert_eq!(a.driver_version(), Some(vec![101, 2130]));
        let d = adapter(INTEL, wgpu::Backend::Dx12, "31.0.101.2130", "");
        assert_eq!(d.driver_version(), Some(vec![31, 0, 101, 2130]));
    }

    #[test]
    fn rules_match_vendor_os_backend_and_a_driver_range() {
        let rule = Rule {
            vendor: INTEL,
            os: Some("windows"),
            backend: Some(wgpu::Backend::Dx12),
            name_contains: Some("uhd"),
            driver_from: Some("31.0.101.2000"),
            driver_below: Some("31.0.101.4000"),
            reason: "test",
        };
        let hit = adapter(INTEL, wgpu::Backend::Dx12, "31.0.101.2130", "");
        assert!(rule.matches(&hit, "windows"));
        assert!(!rule.matches(&hit, "linux"), "another OS");
        assert!(!rule.matches(&adapter(NVIDIA, wgpu::Backend::Dx12, "31.0.101.2130", ""), "windows"), "another vendor");
        assert!(!rule.matches(&adapter(INTEL, wgpu::Backend::Vulkan, "31.0.101.2130", ""), "windows"), "another backend");
        assert!(!rule.matches(&adapter(INTEL, wgpu::Backend::Dx12, "31.0.101.4000", ""), "windows"), "fixed driver");
        assert!(rule.matches(&adapter(INTEL, wgpu::Backend::Dx12, "31.0.101.2000", ""), "windows"), "lower bound is inclusive");
        assert!(!rule.matches(&adapter(INTEL, wgpu::Backend::Dx12, "30.0.100.9000", ""), "windows"), "older driver");
        assert!(!rule.matches(&adapter(INTEL, wgpu::Backend::Dx12, "", ""), "windows"), "unknown version");
        let any = Rule { vendor: AMD, os: None, backend: None, name_contains: None, driver_from: None, driver_below: None, reason: "all" };
        assert!(any.matches(&adapter(AMD, wgpu::Backend::Gl, "", ""), "linux"), "no version bound matches every driver");
        assert_eq!(blocked(&[rule, any], &hit, "windows").map(|r| r.reason), Some("test"));
        assert!(blocked(RULES, &hit, "windows").is_none(), "the shipped list is empty");
    }

    #[test]
    fn the_log_line_names_the_vendor() {
        let a = adapter(INTEL, wgpu::Backend::Dx12, "31.0.101.2130", "");
        let line = a.describe();
        assert!(line.contains("0x8086 (Intel)") && line.contains("31.0.101.2130") && line.contains("Dx12"), "{line}");
    }
}
