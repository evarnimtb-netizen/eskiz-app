//! Whether the pen backend (Windows Ink) is connected at all.
//!
//! A pen backend that hangs must never freeze the window. Four things can keep it off: the
//! command line (`--no-tablet`), the environment (`ESKIZ_TABLET=off`), the setting
//! (Settings > Pen) and what an earlier run learned (the last run froze with the backend
//! connected). On Windows it is also skipped when the machine reports no pen digitizer. This
//! module only decides; `tablet_backend.rs` connects.

use std::sync::atomic::{AtomicBool, Ordering};

/// Why the backend stays off. The text goes to `startup.log`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Off {
    CommandLine,
    Environment,
    Setting,
    /// The last run froze with the pen backend connected.
    Learned,
    /// Windows reports no pen digitizer (a touch screen alone does not need the backend).
    NoDigitizer,
}

impl Off {
    pub fn reason(self) -> &'static str {
        match self {
            Self::CommandLine => "--no-tablet was given",
            Self::Environment => "ESKIZ_TABLET=off",
            Self::Setting => "Settings > Pen has it switched off",
            Self::Learned => "the last run froze while it was connected",
            Self::NoDigitizer => "this PC reports no pen digitizer",
        }
    }
}

/// What the machine says about pens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Digitizer {
    /// Not asked, or not Windows.
    Unknown,
    Pen,
    /// Touch only, or nothing.
    NoPen,
}

/// The inputs of the decision.
#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    pub cli_off: bool,
    pub env: EnvSwitch,
    pub setting_on: bool,
    pub learned_off: bool,
    pub digitizer: Digitizer,
}

/// What `ESKIZ_TABLET` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnvSwitch {
    #[default]
    Unset,
    Off,
    /// Connect even when no pen digitizer is reported.
    On,
}

impl EnvSwitch {
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("off" | "0" | "no" | "false" | "none") => Self::Off,
            Some("on" | "1" | "yes" | "true" | "force") => Self::On,
            _ => Self::Unset,
        }
    }
}

/// `None`: connect. `Some(why)`: stay off. The order is the order of strength: an explicit
/// switch beats a learned skip beats the hardware check.
pub fn decide(i: Inputs) -> Option<Off> {
    if i.cli_off {
        return Some(Off::CommandLine);
    }
    if i.env == EnvSwitch::Off {
        return Some(Off::Environment);
    }
    if !i.setting_on {
        return Some(Off::Setting);
    }
    if i.learned_off {
        return Some(Off::Learned);
    }
    if i.digitizer == Digitizer::NoPen && i.env != EnvSwitch::On {
        return Some(Off::NoDigitizer);
    }
    None
}

/// The digitizer bits of `GetSystemMetrics(SM_DIGITIZER)`: integrated pen (0x2) or external pen
/// (0x4). Touch (0x1, 0x8) alone is not a pen.
pub fn digitizer_from_metrics(sm_digitizer: u32) -> Digitizer {
    if sm_digitizer & 0x6 != 0 {
        Digitizer::Pen
    } else {
        Digitizer::NoPen
    }
}

/// Take `--no-tablet` out of the arguments. Returns whether it was there and the rest.
pub fn split_flag(args: &[String]) -> (bool, Vec<String>) {
    let off = args.iter().any(|a| a == "--no-tablet");
    (off, args.iter().filter(|a| *a != "--no-tablet").cloned().collect())
}

static CLI_OFF: AtomicBool = AtomicBool::new(false);

pub fn set_cli_off(off: bool) {
    CLI_OFF.store(off, Ordering::Relaxed);
}

pub fn cli_off() -> bool {
    CLI_OFF.load(Ordering::Relaxed)
}

pub fn env_switch() -> EnvSwitch {
    EnvSwitch::parse(crate::compat::env_var("ESKIZ_TABLET").as_deref())
}

/// Ask Windows whether a pen digitizer exists. Elsewhere: `Unknown`.
#[cfg(windows)]
pub fn digitizer() -> Digitizer {
    #[allow(unsafe_code)]
    // SAFETY: `GetSystemMetrics` takes an index and returns an int; no pointers.
    let metrics = unsafe { GetSystemMetrics(94) }; // SM_DIGITIZER
    digitizer_from_metrics(u32::try_from(metrics).unwrap_or(0))
}

#[cfg(not(windows))]
pub fn digitizer() -> Digitizer {
    Digitizer::Unknown
}

#[cfg(windows)]
#[allow(unsafe_code)]
extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Inputs {
        Inputs { cli_off: false, env: EnvSwitch::Unset, setting_on: true, learned_off: false, digitizer: Digitizer::Pen }
    }

    #[test]
    fn the_switches_parse() {
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(split_flag(&a(&["--no-tablet", "doc.procreate"])), (true, a(&["doc.procreate"])));
        assert_eq!(split_flag(&a(&["doc.procreate"])), (false, a(&["doc.procreate"])));
        for v in ["off", "OFF", " 0 ", "no", "false"] {
            assert_eq!(EnvSwitch::parse(Some(v)), EnvSwitch::Off, "{v}");
        }
        assert_eq!(EnvSwitch::parse(Some("on")), EnvSwitch::On);
        assert_eq!(EnvSwitch::parse(Some("whatever")), EnvSwitch::Unset);
        assert_eq!(EnvSwitch::parse(None), EnvSwitch::Unset);
    }

    #[test]
    fn a_pen_machine_with_nothing_against_it_connects() {
        assert_eq!(decide(base()), None);
        assert_eq!(decide(Inputs { digitizer: Digitizer::Unknown, ..base() }), None);
    }

    #[test]
    fn each_switch_keeps_the_backend_off() {
        assert_eq!(decide(Inputs { cli_off: true, ..base() }), Some(Off::CommandLine));
        assert_eq!(decide(Inputs { env: EnvSwitch::Off, ..base() }), Some(Off::Environment));
        assert_eq!(decide(Inputs { setting_on: false, ..base() }), Some(Off::Setting));
        assert_eq!(decide(Inputs { learned_off: true, ..base() }), Some(Off::Learned));
        // The command line wins over everything, even a forced ESKIZ_TABLET=on.
        assert_eq!(decide(Inputs { cli_off: true, env: EnvSwitch::On, ..base() }), Some(Off::CommandLine));
    }

    #[test]
    fn a_touch_screen_without_a_pen_does_not_need_the_backend() {
        assert_eq!(digitizer_from_metrics(0x1 | 0x80), Digitizer::NoPen);
        assert_eq!(digitizer_from_metrics(0), Digitizer::NoPen);
        assert_eq!(digitizer_from_metrics(0x2 | 0x80), Digitizer::Pen);
        assert_eq!(digitizer_from_metrics(0x4 | 0x1), Digitizer::Pen);
        assert_eq!(decide(Inputs { digitizer: Digitizer::NoPen, ..base() }), Some(Off::NoDigitizer));
        // ESKIZ_TABLET=on connects anyway; a learned freeze still wins.
        assert_eq!(decide(Inputs { digitizer: Digitizer::NoPen, env: EnvSwitch::On, ..base() }), None);
        assert_eq!(decide(Inputs { digitizer: Digitizer::NoPen, env: EnvSwitch::On, learned_off: true, ..base() }), Some(Off::Learned));
    }

    #[test]
    fn every_reason_has_text_for_the_log() {
        for o in [Off::CommandLine, Off::Environment, Off::Setting, Off::Learned, Off::NoDigitizer] {
            assert!(!o.reason().is_empty());
        }
    }
}
