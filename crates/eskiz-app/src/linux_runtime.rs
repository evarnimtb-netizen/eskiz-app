//! Linux runtime-library check, run before the window is created.
//!
//! winit opens some system libraries with dlopen when the event loop starts. On a system that
//! lacks them (for example `libxkbcommon-x11.so.0` on a minimal Debian install) it panics deep
//! inside winit's loader. This module checks the libraries for the session winit will use, so
//! Eskiz can name the packages to install and exit with status 2 instead.
//!
//! The package table and the message builder are plain Rust and compile on every platform, so
//! they are tested everywhere. Only the dlopen probe is Linux-specific.

use std::fmt::Write as _;

/// A shared library that winit needs at run time, with the packages that provide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Library {
    /// The soname the dynamic loader looks up.
    pub soname: &'static str,
    /// Package name on Debian and Ubuntu.
    pub debian: &'static str,
    /// Package name on Fedora.
    pub fedora: &'static str,
}

const XKBCOMMON_X11: Library = Library {
    soname: "libxkbcommon-x11.so.0",
    debian: "libxkbcommon-x11-0",
    fedora: "libxkbcommon-x11",
};
const XKBCOMMON: Library = Library {
    soname: "libxkbcommon.so.0",
    debian: "libxkbcommon0",
    fedora: "libxkbcommon",
};
const X11_XCB: Library = Library {
    soname: "libX11-xcb.so.1",
    debian: "libx11-xcb1",
    fedora: "libX11-xcb",
};
const WAYLAND_CLIENT: Library = Library {
    soname: "libwayland-client.so.0",
    debian: "libwayland-client0",
    fedora: "libwayland-client",
};

/// The display session winit will use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    X11,
    Wayland,
}

impl Session {
    /// The name shown in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::X11 => "X11",
            Self::Wayland => "Wayland",
        }
    }

    /// The libraries this session needs at run time.
    pub fn libraries(self) -> &'static [Library] {
        match self {
            Self::X11 => &[XKBCOMMON_X11, XKBCOMMON, X11_XCB],
            Self::Wayland => &[XKBCOMMON, WAYLAND_CLIENT],
        }
    }
}

/// The session winit picks from the environment. Wayland wins when `WAYLAND_DISPLAY` is set,
/// even if `DISPLAY` is also set (XWayland), because winit prefers Wayland. `None` means neither
/// is set; winit then reports the missing display itself.
pub fn session_from_env(wayland_display: bool, x11_display: bool) -> Option<Session> {
    if wayland_display {
        Some(Session::Wayland)
    } else if x11_display {
        Some(Session::X11)
    } else {
        None
    }
}

/// The message for a session whose libraries are missing. It names each library, the package
/// that provides it on Debian/Ubuntu and on Fedora, and the install commands for the missing
/// packages. `missing` is expected to be non-empty.
pub fn missing_message(session: Session, missing: &[Library]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "eskiz: missing system libraries that the {} window system needs:",
        session.name()
    );
    for lib in missing {
        let _ = writeln!(
            out,
            "  {}  (Debian/Ubuntu: {}, Fedora: {})",
            lib.soname, lib.debian, lib.fedora
        );
    }
    let debian: Vec<&str> = missing.iter().map(|l| l.debian).collect();
    let fedora: Vec<&str> = missing.iter().map(|l| l.fedora).collect();
    let _ = writeln!(out, "\nInstall them, then start Eskiz again:");
    let _ = writeln!(out, "  Debian/Ubuntu: sudo apt install {}", debian.join(" "));
    let _ = writeln!(out, "  Fedora:        sudo dnf install {}", fedora.join(" "));
    out.push_str(
        "\nEskiz also needs libGL and libvulkan with Mesa drivers to draw the window \
         (Debian/Ubuntu: libgl1 libvulkan1 libgl1-mesa-dri mesa-vulkan-drivers; \
         Fedora: mesa-libGL vulkan-loader mesa-dri-drivers mesa-vulkan-drivers).\n",
    );
    out
}

/// The libraries of `session` that the dynamic loader cannot open.
#[cfg(target_os = "linux")]
pub fn missing_libraries(session: Session) -> Vec<Library> {
    session
        .libraries()
        .iter()
        .copied()
        .filter(|lib| !loadable(lib.soname))
        .collect()
}

/// Whether the dynamic loader can open `soname`. The library is closed again at once.
#[cfg(target_os = "linux")]
fn loadable(soname: &str) -> bool {
    // SAFETY: opening a system library runs its initialisers. These are the libraries winit
    // opens itself a moment later, so nothing new is loaded into the process.
    unsafe { libloading::Library::new(soname) }.is_ok()
}

/// Checks the libraries for the session winit will use. `Err` holds the message to print.
#[cfg(target_os = "linux")]
pub fn preflight() -> Result<(), String> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    check_sessions(set("WAYLAND_DISPLAY"), set("DISPLAY"), missing_libraries)
}

/// The decision behind `preflight`, with the library probe passed in. Wayland is tried first.
/// When its libraries are missing but `DISPLAY` is set and the X11 libraries are there, winit
/// falls back to X11, so that is fine. The error names the first session tried.
pub fn check_sessions(
    wayland_display: bool,
    x11_display: bool,
    missing: impl Fn(Session) -> Vec<Library>,
) -> Result<(), String> {
    let Some(session) = session_from_env(wayland_display, x11_display) else {
        return Ok(());
    };
    let gone = missing(session);
    if gone.is_empty() {
        return Ok(());
    }
    if session == Session::Wayland && x11_display && missing(Session::X11).is_empty() {
        return Ok(());
    }
    Err(missing_message(session, &gone))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib_set(gone: &'static [&'static str]) -> impl Fn(Session) -> Vec<Library> {
        move |s| {
            s.libraries()
                .iter()
                .copied()
                .filter(|l| gone.contains(&l.soname))
                .collect()
        }
    }

    #[test]
    fn missing_wayland_libraries_fall_back_to_x11_when_display_is_set() {
        let gone = lib_set(&["libwayland-client.so.0"]);
        assert!(check_sessions(true, true, &gone).is_ok());
        // No X11 display: nothing to fall back to.
        assert!(check_sessions(true, false, &gone).is_err());
    }

    #[test]
    fn both_sessions_broken_reports_the_wayland_set() {
        let gone = lib_set(&["libwayland-client.so.0", "libxkbcommon-x11.so.0"]);
        let msg = check_sessions(true, true, gone).unwrap_err();
        assert!(msg.contains("Wayland"));
    }

    #[test]
    fn working_wayland_and_missing_x11_is_fine() {
        let gone = lib_set(&["libxkbcommon-x11.so.0"]);
        assert!(check_sessions(true, true, gone).is_ok());
    }

    #[test]
    fn wayland_display_wins_over_x11() {
        assert_eq!(session_from_env(true, true), Some(Session::Wayland));
        assert_eq!(session_from_env(true, false), Some(Session::Wayland));
        assert_eq!(session_from_env(false, true), Some(Session::X11));
        assert_eq!(session_from_env(false, false), None);
    }

    #[test]
    fn x11_needs_xkbcommon_x11_xkbcommon_and_x11_xcb() {
        let sonames: Vec<&str> = Session::X11.libraries().iter().map(|l| l.soname).collect();
        assert_eq!(sonames, ["libxkbcommon-x11.so.0", "libxkbcommon.so.0", "libX11-xcb.so.1"]);
    }

    #[test]
    fn wayland_needs_xkbcommon_and_wayland_client() {
        let sonames: Vec<&str> = Session::Wayland.libraries().iter().map(|l| l.soname).collect();
        assert_eq!(sonames, ["libxkbcommon.so.0", "libwayland-client.so.0"]);
    }

    #[test]
    fn message_names_the_missing_library_and_its_packages() {
        let msg = missing_message(Session::X11, &[XKBCOMMON_X11]);
        assert!(msg.contains("missing system libraries that the X11 window system needs"));
        assert!(msg.contains(
            "libxkbcommon-x11.so.0  (Debian/Ubuntu: libxkbcommon-x11-0, Fedora: libxkbcommon-x11)"
        ));
        assert!(msg.contains("Debian/Ubuntu: sudo apt install libxkbcommon-x11-0\n"));
        assert!(msg.contains("Fedora:        sudo dnf install libxkbcommon-x11\n"));
    }

    #[test]
    fn message_lists_only_the_missing_packages_in_order() {
        let msg = missing_message(Session::X11, &[X11_XCB, XKBCOMMON]);
        assert!(msg.contains("Debian/Ubuntu: sudo apt install libx11-xcb1 libxkbcommon0\n"));
        assert!(msg.contains("Fedora:        sudo dnf install libX11-xcb libxkbcommon\n"));
        assert!(!msg.contains("libxkbcommon-x11-0\n"));
    }

    #[test]
    fn wayland_message_uses_the_wayland_client_packages() {
        let msg = missing_message(Session::Wayland, &[WAYLAND_CLIENT]);
        assert!(msg.contains("Wayland window system"));
        assert!(msg.contains("libwayland-client.so.0  (Debian/Ubuntu: libwayland-client0, Fedora: libwayland-client)"));
        assert!(msg.contains("Debian/Ubuntu: sudo apt install libwayland-client0\n"));
        assert!(msg.contains("Fedora:        sudo dnf install libwayland-client\n"));
    }

    #[test]
    fn every_library_has_both_package_names() {
        for session in [Session::X11, Session::Wayland] {
            for lib in session.libraries() {
                assert!(lib.soname.contains(".so"), "{lib:?}");
                assert!(!lib.debian.is_empty() && !lib.fedora.is_empty(), "{lib:?}");
            }
        }
    }
}
