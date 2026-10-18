//! Start-up diagnostics for the desktop window: a log, a crash marker and a visible error.
//!
//! The Windows release is a GUI-subsystem program, so when it fails before its window shows
//! (a panic, a driver that kills the process, no usable graphics) nothing is visible: no
//! window, no dialog, no process left in Task Manager. This module makes that visible:
//!
//! * **Log.** `%LOCALAPPDATA%\Eskiz\logs\startup.log` (the previous run's log is kept as
//!   `startup.prev.log`) lists the steps: command line, renderer attempts, the adapter and
//!   backend wgpu chose, "first frames drawn". `ESKIZ_LOG_DIR` picks another folder on any OS.
//!   Other platforms have a terminal and only log there unless `ESKIZ_LOG_DIR` is set.
//! * **Panic report.** The panic hook writes `panic-<unix time>.txt` (message, location,
//!   backtrace) next to the log, and remembers the text for the dialog.
//! * **Message box.** [`report_failure`] shows the reason and the log path (Windows only;
//!   elsewhere it prints to stderr).
//! * **Crash marker.** `starting.marker` holds the renderer being tried while the first frames
//!   are not drawn yet. A driver crash kills the process without a panic, so the next start
//!   finds the marker and escalates ([`Ladder`]): wgpu with all backends, then wgpu on DX12
//!   only (Windows), then OpenGL (glow). If even that crashed it shows a message and resets.
//!   A second instance that starts while the first is still starting does not count as a crash:
//!   on Windows the first holds the marker open exclusively.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// How the window is created, in the order tried after crashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// wgpu with every backend it supports on this OS.
    Wgpu,
    /// wgpu restricted to DirectX 12 (Windows): skips a broken Vulkan driver.
    WgpuDx12,
    /// eframe's OpenGL renderer.
    Glow,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wgpu => "wgpu",
            Self::WgpuDx12 => "wgpu-dx12",
            Self::Glow => "glow",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "wgpu" => Some(Self::Wgpu),
            "wgpu-dx12" => Some(Self::WgpuDx12),
            "glow" => Some(Self::Glow),
            _ => None,
        }
    }
}

/// What to try, given the stage a previous start never finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ladder {
    Try(Stage),
    /// Every renderer crashed the process: tell the user instead of looping.
    GiveUp,
}

impl Ladder {
    /// `crashed` is the stage in the leftover marker, `None` after a clean start.
    pub fn next(crashed: Option<Stage>, windows: bool) -> Self {
        match crashed {
            None => Self::Try(Stage::Wgpu),
            Some(Stage::Wgpu) if windows => Self::Try(Stage::WgpuDx12),
            Some(Stage::Wgpu | Stage::WgpuDx12) => Self::Try(Stage::Glow),
            Some(Stage::Glow) => Self::GiveUp,
        }
    }
}

/// The folder for the logs: `ESKIZ_LOG_DIR`, else `%LOCALAPPDATA%\Eskiz\logs` on Windows.
pub fn log_dir_from(env_dir: Option<&str>, local_app_data: Option<&str>) -> Option<PathBuf> {
    match (env_dir.filter(|d| !d.is_empty()), local_app_data.filter(|d| !d.is_empty())) {
        (Some(d), _) => Some(PathBuf::from(d)),
        (None, Some(l)) if cfg!(windows) => Some(Path::new(l).join("Eskiz").join("logs")),
        _ => None,
    }
}

/// The text of a panic: message, location and a backtrace.
pub fn panic_text(info: &std::panic::PanicHookInfo<'_>, backtrace: &str) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned());
    let place = info.location().map_or_else(String::new, |l| format!(" at {}:{}", l.file(), l.line()));
    format!(
        "Eskiz {} panicked on thread '{}'{place}\n{message}\n\nBacktrace:\n{backtrace}\n",
        env!("CARGO_PKG_VERSION"),
        std::thread::current().name().unwrap_or("?"),
    )
}

/// The dialog text for a failure.
pub fn failure_message(reason: &str, log: Option<&Path>) -> String {
    let mut text = format!("Eskiz hit a problem.\n\n{reason}\n");
    match log {
        Some(p) => text.push_str(&format!(
            "\nDetails were saved in:\n{}\n\nPlease send that folder's startup.log (and any panic-*.txt) when you report this.",
            p.display()
        )),
        None => text.push_str("\nRun eskiz --diagnose in a terminal for details."),
    }
    text
}

struct State {
    dir: Option<PathBuf>,
    started: Instant,
    /// The marker file, held open so a second instance can tell a live start from a crash.
    marker: Option<fs::File>,
    last_panic: Option<String>,
    /// The stage the previous start died in, read by [`init`].
    crashed: Option<Stage>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { dir: None, started: Instant::now(), marker: None, last_panic: None, crashed: None }))
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = state().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g)
}

const LOG: &str = "startup.log";
const PREV: &str = "startup.prev.log";
const MARKER: &str = "starting.marker";

/// Start the log for this run. Returns the stage a previous run died in, if any. Call once,
/// first thing, for a window start.
pub fn init(args: &[String]) -> Option<Stage> {
    let dir = log_dir_from(
        crate::compat::env_var("ESKIZ_LOG_DIR").as_deref(),
        std::env::var("LOCALAPPDATA").ok().as_deref(),
    );
    let crashed = dir.as_deref().and_then(take_marker);
    with(|s| {
        s.dir = dir.clone();
        s.started = Instant::now();
        s.crashed = crashed;
    });
    if let Some(d) = &dir {
        let _ = fs::create_dir_all(d);
        let _ = fs::rename(d.join(LOG), d.join(PREV));
    }
    log(&format!(
        "Eskiz {} starting; os={} arch={} args={args:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    if let Some(c) = crashed {
        log(&format!("the previous start never drew its first frames while using '{}'", c.name()));
    }
    crashed
}

/// Read and remove a leftover marker. `None` when there is none, or when another instance
/// still holds it (it is starting right now).
fn take_marker(dir: &Path) -> Option<Stage> {
    let path = dir.join(MARKER);
    let text = fs::read_to_string(&path).ok()?;
    // Removal fails while a live instance holds the file open exclusively (Windows).
    fs::remove_file(&path).ok()?;
    Stage::parse(&text)
}

/// The stage the previous start died in (set by [`init`]).
pub fn previous_crash() -> Option<Stage> {
    with(|s| s.crashed)
}

/// Where the logs go, if anywhere.
pub fn log_dir() -> Option<PathBuf> {
    with(|s| s.dir.clone())
}

/// Append a line to the start-up log (and stderr).
pub fn log(line: &str) {
    with(|s| {
        let stamp = format!("[{:7.3}s] {line}", s.started.elapsed().as_secs_f64());
        eprintln!("eskiz: {line}");
        if let Some(d) = &s.dir {
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(d.join(LOG)) {
                let _ = writeln!(f, "{stamp}");
            }
        }
    });
}

/// Record that `stage` is about to be tried: the marker stays until the first frames are drawn.
pub fn begin_attempt(stage: Stage) {
    log(&format!("renderer attempt: {}", stage.name()));
    with(|s| {
        s.marker = None;
        let Some(d) = s.dir.clone() else { return };
        let path = d.join(MARKER);
        let mut options = fs::OpenOptions::new();
        options.create(true).write(true).truncate(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // No sharing: a second instance cannot read or delete it while this one starts.
            options.share_mode(0);
        }
        if let Ok(mut f) = options.open(&path) {
            let _ = f.write_all(stage.name().as_bytes());
            let _ = f.flush();
            s.marker = Some(f);
        }
    });
}

static FRAMES: AtomicU32 = AtomicU32::new(0);
/// Frames drawn before the start counts as successful.
const FRAMES_OK: u32 = 3;

/// Called once per drawn frame. After a few frames the start is a success and the marker goes.
pub fn frame_drawn() {
    // The headless test harness draws many "windows" in one process; they are not a start-up.
    if cfg!(test) {
        return;
    }
    if FRAMES.fetch_add(1, Ordering::Relaxed) + 1 == FRAMES_OK {
        first_frames_ok();
    }
}

fn first_frames_ok() {
    log("first frames drawn");
    with(|s| {
        if s.marker.take().is_some() {
            if let Some(d) = &s.dir {
                let _ = fs::remove_file(d.join(MARKER));
            }
        }
    });
}

/// True once the window has drawn its first frames (a failure after that is a runtime error,
/// not a start-up failure, and must not restart with another renderer).
pub fn started_ok() -> bool {
    FRAMES.load(Ordering::Relaxed) >= FRAMES_OK
}

/// Install a panic hook that writes the report file. Chains to the previous hook.
pub fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let text = panic_text(info, &bt);
            record_panic(&text);
            previous(info);
        }));
    });
}

fn record_panic(text: &str) {
    with(|s| {
        if let Some(d) = &s.dir {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |t| t.as_secs());
            let _ = fs::write(d.join(format!("panic-{secs}.txt")), text);
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(d.join(LOG)) {
                let first = text.lines().next().unwrap_or("");
                let _ = writeln!(f, "[{:7.3}s] PANIC: {first} (details in panic-{secs}.txt)", s.started.elapsed().as_secs_f64());
            }
        }
        s.last_panic = Some(text.to_owned());
    });
}

/// The first line of the last panic (message only), for the dialog.
pub fn last_panic_summary() -> Option<String> {
    with(|s| s.last_panic.as_ref().map(|t| t.lines().take(2).collect::<Vec<_>>().join(": ")))
}

/// Show the user why Eskiz failed: log it, then a message box on Windows, stderr elsewhere.
pub fn report_failure(reason: &str) {
    log(&format!("FAILED: {reason}"));
    let text = failure_message(reason, log_dir().as_deref());
    show_message(&text);
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn show_message(text: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (t, c) = (wide(text), wide("Eskiz"));
    // MB_OK (0) | MB_ICONERROR (0x10) | MB_SETFOREGROUND (0x10000)
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; no owner window.
    unsafe {
        MessageBoxW(0, t.as_ptr(), c.as_ptr(), 0x10 | 0x1_0000);
    }
}

#[cfg(not(windows))]
fn show_message(text: &str) {
    eprintln!("{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("eskiz-startup-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn ladder_escalates_after_each_crash_and_gives_up() {
        assert_eq!(Ladder::next(None, true), Ladder::Try(Stage::Wgpu));
        assert_eq!(Ladder::next(Some(Stage::Wgpu), true), Ladder::Try(Stage::WgpuDx12));
        assert_eq!(Ladder::next(Some(Stage::WgpuDx12), true), Ladder::Try(Stage::Glow));
        assert_eq!(Ladder::next(Some(Stage::Wgpu), false), Ladder::Try(Stage::Glow));
        assert_eq!(Ladder::next(Some(Stage::Glow), true), Ladder::GiveUp);
    }

    #[test]
    fn stage_names_round_trip() {
        for s in [Stage::Wgpu, Stage::WgpuDx12, Stage::Glow] {
            assert_eq!(Stage::parse(s.name()), Some(s));
        }
        assert_eq!(Stage::parse("junk"), None);
        assert_eq!(Stage::parse(" glow\n"), Some(Stage::Glow));
    }

    #[test]
    fn log_folder_is_local_app_data_on_windows_and_overridable() {
        assert_eq!(log_dir_from(Some("/x"), Some("C:\\L")), Some(PathBuf::from("/x")));
        assert_eq!(log_dir_from(Some(""), None), None);
        let d = log_dir_from(None, Some("C:\\Users\\a\\AppData\\Local"));
        if cfg!(windows) {
            assert!(d.unwrap().ends_with("Eskiz\\logs"));
        } else {
            assert_eq!(d, None);
        }
    }

    #[test]
    fn leftover_marker_is_read_once_and_removed() {
        let d = tmp("marker");
        assert_eq!(take_marker(&d), None);
        fs::write(d.join(MARKER), "wgpu-dx12").unwrap();
        assert_eq!(take_marker(&d), Some(Stage::WgpuDx12));
        assert!(!d.join(MARKER).exists());
        assert_eq!(take_marker(&d), None);
        fs::write(d.join(MARKER), "garbage").unwrap();
        assert_eq!(take_marker(&d), None, "an unreadable marker is no crash evidence, but it is removed");
        assert!(!d.join(MARKER).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn message_names_the_reason_and_the_log_folder() {
        let m = failure_message("No graphics adapter was found.", Some(Path::new("C:\\Users\\a\\AppData\\Local\\Eskiz\\logs")));
        assert!(m.contains("No graphics adapter was found."));
        assert!(m.contains("Eskiz\\logs") && m.contains("startup.log") && m.contains("panic-"));
        let m = failure_message("x", None);
        assert!(m.contains("--diagnose"));
    }

    #[test]
    fn panic_report_has_message_location_and_backtrace() {
        let seen = std::sync::Arc::new(Mutex::new(String::new()));
        let sink = seen.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            *sink.lock().unwrap() = panic_text(info, "frame 0\nframe 1");
        }));
        let _ = std::panic::catch_unwind(|| panic!("boom {}", 7));
        std::panic::set_hook(previous);
        let text = seen.lock().unwrap().clone();
        assert!(text.contains("boom 7") && text.contains("startup.rs") && text.contains("frame 1"), "{text}");
    }

    #[test]
    fn a_full_start_writes_the_log_and_the_marker_until_the_first_frames() {
        // One test owns the global state (the other tests only use pure helpers).
        let d = tmp("run");
        std::env::set_var("ESKIZ_LOG_DIR", &d);
        fs::write(d.join(LOG), "old run\n").unwrap();
        let crashed = init(&["--x".to_owned()]);
        assert_eq!(crashed, None);
        assert_eq!(fs::read_to_string(d.join(PREV)).unwrap(), "old run\n", "the previous log is kept");
        begin_attempt(Stage::Wgpu);
        assert!(d.join(MARKER).exists());
        // On Windows the marker is held open without sharing while the start is in progress.
        #[cfg(not(windows))]
        assert_eq!(fs::read_to_string(d.join(MARKER)).unwrap(), "wgpu");
        first_frames_ok();
        assert!(!d.join(MARKER).exists(), "a successful start removes the marker");
        let log_text = fs::read_to_string(d.join(LOG)).unwrap();
        assert!(log_text.contains("renderer attempt: wgpu") && log_text.contains("first frames drawn"), "{log_text}");
        // A crash leaves the marker; the next start sees it.
        begin_attempt(Stage::Glow);
        // A crashed process no longer holds the marker open.
        with(|s| s.marker = None);
        let again = init(&[]);
        assert_eq!(again, Some(Stage::Glow));
        std::env::remove_var("ESKIZ_LOG_DIR");
        let _ = fs::remove_dir_all(&d);
    }
}
