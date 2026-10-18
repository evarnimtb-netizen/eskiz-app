//! Eskiz: a painting app built on `eskiz-core` (brush engine and compositing) and
//! `eskiz-format` (`.procreate` reader), with an egui/eframe front end.
//!
//! * [`run`] is the desktop entry point. It parses the command line, then either runs a
//!   headless command (`--selftest`, `--open FILE --export PNG`) or opens the window.
//! * With the `android` feature on an Android target, `android_main` is the entry point.
//!
//! Module layout: [`session`] is the document plus stroke engine plus undo history, and
//! it is what both the window and the headless paths drive. [`input`] maps pointer data
//! to samples. [`view`] is the pan/zoom transform. [`app`] is the egui front end.

pub mod a11y;
pub mod fonts;
#[cfg(test)]
mod identity_tests;
mod compat;
#[cfg(test)]
mod a11y_tests;
pub mod adjust;
pub mod animation_ui;
pub mod app;
pub mod bit_depth;
pub mod brush_import;
pub mod brush_library;
pub mod brush_settings;
pub mod canvas_ops;
pub mod chrome;
pub mod colour;
pub mod colour_management;
pub mod pour_fill;
pub mod curves_widget;
pub mod diagnose;
#[cfg(test)]
mod pour_fill_tests;
#[cfg(test)]
mod adjust_tests;
#[cfg(test)]
mod adjust_new_tests;
#[cfg(test)]
mod view3d_tests;
#[cfg(test)]
mod gpu_tests;
#[cfg(test)]
mod text_tool_tests;
pub mod command;
pub mod document;
pub mod export;
pub mod eyedropper;
pub mod file_access;
#[cfg(test)]
mod file_access_tests;
pub mod gallery;
pub mod floodfill;
pub mod gesture;
pub mod gpu_canvas;
pub mod gpu_display;
pub mod guide_ui;
pub mod guides;
pub mod history;
pub mod icons;
pub mod icons_actions;
pub mod icons_brush;
pub mod icons_guides;
pub mod icons_selection;
pub mod icons_tools;
pub mod import;
pub mod liquify_tool;
pub mod android_input;
pub mod android_queue;
pub mod android_source;
pub mod safe_area;
#[cfg(test)]
mod android_input_tests;
pub mod input;
pub mod linux_runtime;
pub mod paint;
pub mod panels;
pub mod popover;
pub mod pressure;
pub mod hold_shape;
pub mod reference;
pub mod renderer;
pub mod selftest;
pub mod saved_store;
pub mod selection_tool;
pub mod session;
pub mod shape_edit;
pub mod share;
pub mod smudge;
pub mod journal;
pub mod onboarding;
mod onboarding_sample;
#[cfg(test)]
mod onboarding_tests;
pub mod lifecycle;
mod lifecycle_host;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod android_fix_tests;
#[cfg(test)]
mod rc1_tests;
#[cfg(test)]
mod rc2_tests;
#[cfg(test)]
mod rc3_tests;
#[cfg(test)]
mod rc5_tests;
#[cfg(test)]
mod rc3_look_tests;
#[cfg(test)]
mod qa1_tests;
pub mod stroke_trace;
pub mod storage;
pub mod studio;
pub(crate) mod licenses;
pub mod sheets;
pub mod stacks;
pub mod studio_settings;
pub mod shortcuts;
pub mod user_guide;
#[cfg(test)]
mod studio_tests;
#[cfg(test)]
mod visual_tour_tests;
pub mod snapshot;
#[cfg(test)]
mod qa_capture;
pub mod tablet;
pub mod tablet_backend;
pub mod startup;
#[cfg(any(target_os = "macos", test))]
pub mod tablet_mac;
pub mod text_tool;
pub mod theme;
pub mod timelapse;
pub mod view;
pub mod view3d;

use std::path::PathBuf;

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Gui { file: Option<PathBuf> },
    SelfTest,
    Export { open: PathBuf, png: PathBuf },
    /// Open the icon review window.
    Icons,
    /// Print a report about this machine (GPU, DPI, paths, pen, MP4) and write `eskiz-diagnose.md`.
    /// With `window`, also open a window for a few frames and report what it saw.
    Diagnose { window: bool },
    Help,
    /// Print `Eskiz <version>`.
    Version,
}

pub const USAGE: &str = "\
Eskiz - painting app

USAGE:
    eskiz [FILE.procreate]              open the window (optionally with a document)
    eskiz --open FILE --export OUT.png  export a document to PNG without a window
    eskiz --version                     print the version
    eskiz --selftest                    paint, undo and redo headlessly; writes selftest.png
    eskiz --icons                       open a window showing every icon, for review
    eskiz --diagnose [--window]         report GPU backends, DPI, data folders, pen and MP4 support
                                        (also saved as eskiz-diagnose.md); --window opens a window
                                        for a moment to read its scale factor and renderer

Drop a .procreate file onto the window to open it.";

/// The `--version` output.
pub fn version_line() -> String {
    format!("Eskiz {}", env!("CARGO_PKG_VERSION"))
}

/// Parse command-line arguments (without the program name).
pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut open = None;
    let mut export = None;
    let mut selftest = false;
    let mut icons = false;
    let mut diagnose = false;
    let mut window = false;
    let mut positional = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--selftest" => selftest = true,
            "--icons" => icons = true,
            "--diagnose" => diagnose = true,
            "--window" => window = true,
            "--open" => {
                open = Some(PathBuf::from(it.next().ok_or("--open needs a file")?));
            }
            "--export" => {
                export = Some(PathBuf::from(it.next().ok_or("--export needs a file")?));
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => {
                if positional.replace(PathBuf::from(s)).is_some() {
                    return Err("only one file can be given".into());
                }
            }
        }
    }
    if window && !diagnose {
        return Err("--window only goes with --diagnose".into());
    }
    if selftest {
        return Ok(Command::SelfTest);
    }
    if diagnose {
        return Ok(Command::Diagnose { window });
    }
    if icons {
        return Ok(Command::Icons);
    }
    match (open, export) {
        (Some(open), Some(png)) => Ok(Command::Export { open, png }),
        (None, Some(_)) => Err("--export needs --open".into()),
        (Some(file), None) => Ok(Command::Gui { file: Some(file) }),
        (None, None) => Ok(Command::Gui { file: positional }),
    }
}

/// Desktop entry point. Parses `std::env::args`, runs the requested mode, and exits
/// with status 1 on error.
pub fn run() {
    journal::install_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = parse_args(&args);
    // Commands that print: on Windows the release exe is a GUI program with no console of its own.
    if matches!(parsed, Ok(Command::Gui { .. } | Command::Icons)) {
        startup::install_panic_hook();
    } else {
        attach_parent_console();
    }
    let gui = matches!(parsed, Ok(Command::Gui { .. } | Command::Icons));
    let result = match parsed {
        Ok(Command::Help) => {
            println!("{USAGE}");
            Ok(())
        }
        Ok(Command::Version) => {
            println!("{}", version_line());
            Ok(())
        }
        Ok(Command::SelfTest) => selftest::run().map(|report| println!("{report}")),
        Ok(Command::Export { open, png }) => selftest::export(&open, &png).map(|line| println!("{line}")),
        Ok(Command::Diagnose { window }) => diagnose::run(window).map(|report| println!("{report}")),
        Ok(Command::Icons) => open_window(icons::run_viewer),
        Ok(Command::Gui { file }) => open_window(|| app::run_desktop(file)),
        Err(e) => {
            eprintln!("eskiz: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("eskiz: {e}");
        if gui {
            startup::report_failure(&e);
        }
        std::process::exit(1);
    }
}

/// Windows release builds are GUI-subsystem programs (no console window), so `println!` goes
/// nowhere when `eskiz.exe --selftest` or `--diagnose` is started from a terminal. Attach to the
/// parent's console and point stdout and stderr at it, unless they were already redirected to a
/// file or pipe. Does nothing when there is no parent console (double click) or on failure.
#[cfg(windows)]
#[allow(unsafe_code)]
fn attach_parent_console() {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn GetStdHandle(which: u32) -> isize;
        fn SetStdHandle(which: u32, handle: isize) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: isize,
        ) -> isize;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const INVALID: isize = -1;
    // SAFETY: plain kernel32 calls with integer arguments and a NUL-terminated UTF-16 name that
    // outlives the call. Failures are ignored: the worst case is that nothing is printed.
    unsafe {
        let missing = |which: u32| {
            let h = GetStdHandle(which);
            h == 0 || h == INVALID
        };
        let (need_out, need_err) = (missing(STD_OUTPUT_HANDLE), missing(STD_ERROR_HANDLE));
        if !(need_out || need_err) || AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        // GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING.
        let console = CreateFileW(name.as_ptr(), 0xC000_0000, 3, std::ptr::null(), 3, 0, 0);
        if console == INVALID {
            return;
        }
        if need_out {
            SetStdHandle(STD_OUTPUT_HANDLE, console);
        }
        if need_err {
            SetStdHandle(STD_ERROR_HANDLE, console);
        }
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

/// Runs a function that opens the window. On Linux the system libraries winit needs are checked
/// first, so a missing package gives a message and exit status 2, not a panic. A panic before
/// the app has been created is reported the same way; a panic after that is passed on unchanged.
fn open_window(start: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    startup::init(&std::env::args().skip(1).collect::<Vec<_>>());
    #[cfg(target_os = "linux")]
    {
        if let Err(message) = linux_runtime::preflight() {
            eprintln!("{message}");
            std::process::exit(2);
        }
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(start)) {
        Ok(result) => result,
        Err(payload) if renderer::window_created() => {
            // The panic hook has written the report. Say so, then let the process end.
            let summary = startup::last_panic_summary().unwrap_or_else(|| "unknown panic".to_owned());
            startup::report_failure(&format!("Eskiz stopped unexpectedly: {summary}"));
            std::panic::resume_unwind(payload)
        }
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            eprintln!("eskiz: could not start the window: {detail}");
            startup::report_failure(&format!("The window could not be created: {detail}"));
            #[cfg(target_os = "linux")]
            {
                if let Err(message) = linux_runtime::preflight() {
                    eprintln!("{message}");
                }
            }
            std::process::exit(2);
        }
    }
}

/// Android entry point, used by the `android` feature. It runs the same setup as the desktop
/// (renderer selection, library, prefs, GPU compositor) with the app's internal data path as the
/// data folder and the preferences file.
#[cfg(all(target_os = "android", feature = "android"))]
#[no_mangle]
fn android_main(android_app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_tag("eskiz").with_max_level(log::LevelFilter::Info),
    );
    journal::install_panic_hook();
    // Phones throttle hard on many threads: the global rayon pool gets the same cap as the
    // hand-rolled worker pools (`eskiz_core::parallelism`).
    let _ = rayon::ThreadPoolBuilder::new().num_threads(eskiz_core::parallelism::cpus()).build_global();
    let data = android_app.internal_data_path();
    if let Some(dir) = &data {
        storage::set_app_data_dir(dir);
    }
    let prefs_file = data.map(|d| storage::DataPaths::under(d).prefs_file());
    let result = app::run_shared(None, move |options| {
        options.android_app = Some(android_app.clone());
        options.persistence_path = prefs_file.clone();
    });
    if let Err(e) = result {
        log::error!("eskiz: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_modes() {
        assert_eq!(parse_args(&args(&[])), Ok(Command::Gui { file: None }));
        assert_eq!(
            parse_args(&args(&["a.procreate"])),
            Ok(Command::Gui { file: Some("a.procreate".into()) })
        );
        assert_eq!(parse_args(&args(&["--selftest"])), Ok(Command::SelfTest));
        assert_eq!(parse_args(&args(&["--version"])), Ok(Command::Version));
        assert_eq!(parse_args(&args(&["-V"])), Ok(Command::Version));
        assert_eq!(version_line(), format!("Eskiz {}", env!("CARGO_PKG_VERSION")));
        assert_eq!(parse_args(&args(&["--icons"])), Ok(Command::Icons));
        assert_eq!(parse_args(&args(&["--diagnose"])), Ok(Command::Diagnose { window: false }));
        assert_eq!(parse_args(&args(&["--diagnose", "--window"])), Ok(Command::Diagnose { window: true }));
        assert_eq!(parse_args(&args(&["--window", "--diagnose"])), Ok(Command::Diagnose { window: true }));
        assert_eq!(
            parse_args(&args(&["--open", "a.procreate", "--export", "o.png"])),
            Ok(Command::Export { open: "a.procreate".into(), png: "o.png".into() })
        );
        assert_eq!(
            parse_args(&args(&["--open", "a.procreate"])),
            Ok(Command::Gui { file: Some("a.procreate".into()) })
        );
    }

    #[test]
    fn rejects_bad_combinations() {
        assert!(parse_args(&args(&["--export", "o.png"])).is_err());
        assert!(parse_args(&args(&["--open"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());
        assert!(parse_args(&args(&["--window"])).is_err());
        assert!(parse_args(&args(&["a", "b"])).is_err());
        assert_eq!(parse_args(&args(&["--help"])), Ok(Command::Help));
    }
}
