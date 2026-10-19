//! Start-up diagnostics for the desktop window: a log, a crash marker and a visible error.
//!
//! The Windows release is a GUI-subsystem program, so when it fails before its window shows
//! (a panic, a driver that kills the process, no usable graphics) nothing is visible: no
//! window, no dialog, no process left in Task Manager. This module makes that visible:
//!
//! * **Log.** `startup.log` (the previous run's log is kept as `startup.prev.log`) lists the
//!   steps: command line, renderer attempts, every adapter wgpu found and the one it chose,
//!   "first frames drawn". It lives in `%LOCALAPPDATA%\Eskiz\logs` on Windows and in `logs`
//!   under the app data folder elsewhere; `ESKIZ_LOG_DIR` picks another folder on any OS.
//! * **Panic report.** The panic hook writes `panic-<unix time>.txt` (message, location,
//!   backtrace) next to the log, and remembers the text for the dialog.
//! * **Message box.** [`report_failure`] shows the reason and the log path (Windows only;
//!   elsewhere it prints to stderr).
//! * **Crash marker.** `starting.marker` holds the renderer being tried while the first frames
//!   are not drawn yet. A driver crash or a frozen start kills the process without a panic, so the
//!   next start finds the marker and steps one rung down the ladder ([`order`], [`Ladder`]). The
//!   rung that crashed joins the skip list (`renderer-skip.txt`, shared with [`crate::stall`]),
//!   so Eskiz stays in safe graphics mode until the user picks Try normal graphics
//!   ([`retry_normal_graphics`]). If the last rung crashed it shows a message and starts over.
//!   A second instance that starts while the first is still starting does not count as a crash:
//!   on Windows the first holds the marker open exclusively.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// How the window is created: one rung of the renderer ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// wgpu with the backends and adapter wgpu picks itself (`WGPU_BACKEND` is honoured). Android's
    /// only rung, and what `WGPU_BACKEND` selects on the desktop.
    Wgpu,
    /// wgpu restricted to DirectX 12 (Windows).
    WgpuDx12,
    /// wgpu restricted to Vulkan.
    WgpuVulkan,
    /// wgpu restricted to Metal (macOS).
    WgpuMetal,
    /// wgpu restricted to its OpenGL backend (Linux).
    WgpuGl,
    /// eframe's OpenGL renderer with the CPU compositor.
    Glow,
    /// wgpu on a software adapter (WARP on Windows, lavapipe or llvmpipe on Linux), CPU compositor.
    WgpuSoftware,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wgpu => "wgpu",
            Self::WgpuDx12 => "wgpu-dx12",
            Self::WgpuVulkan => "wgpu-vulkan",
            Self::WgpuMetal => "wgpu-metal",
            Self::WgpuGl => "wgpu-gl",
            Self::Glow => "glow",
            Self::WgpuSoftware => "wgpu-software",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "wgpu" => Some(Self::Wgpu),
            "wgpu-dx12" => Some(Self::WgpuDx12),
            "wgpu-vulkan" => Some(Self::WgpuVulkan),
            "wgpu-metal" => Some(Self::WgpuMetal),
            "wgpu-gl" => Some(Self::WgpuGl),
            "glow" => Some(Self::Glow),
            "wgpu-software" => Some(Self::WgpuSoftware),
            _ => None,
        }
    }

    /// The name people see (Settings > Graphics, the safe-mode notice).
    pub fn label(self) -> &'static str {
        match self {
            Self::Wgpu => "wgpu (automatic)",
            Self::WgpuDx12 => "DirectX 12",
            Self::WgpuVulkan => "Vulkan",
            Self::WgpuMetal => "Metal",
            Self::WgpuGl => "OpenGL (wgpu)",
            Self::Glow => "OpenGL (compatibility)",
            Self::WgpuSoftware => "Software",
        }
    }

    /// Glow and the software adapter composite the canvas on the CPU.
    pub fn cpu_compositor(self) -> bool {
        matches!(self, Self::Glow | Self::WgpuSoftware)
    }
}

/// The operating system families that have different ladders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Mac,
    /// Linux and the BSDs.
    Linux,
    Android,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "android") {
            Self::Android
        } else {
            Self::Linux
        }
    }
}

/// The renderer ladder, top (fastest) to bottom (safest), for `platform`. `gl` is whether the
/// glow renderer is compiled in (the `gl-fallback` feature).
///
/// * Windows: DX12, Vulkan, OpenGL, software (WARP).
/// * macOS: Metal, OpenGL. wgpu has no software Metal adapter.
/// * Linux: Vulkan, wgpu's OpenGL backend, OpenGL (glow), software (lavapipe / llvmpipe).
/// * Android: wgpu's own choice (Vulkan) only.
pub fn order(platform: Platform, gl: bool) -> Vec<Stage> {
    use Stage::*;
    let all: &[Stage] = match platform {
        Platform::Windows => &[WgpuDx12, WgpuVulkan, Glow, WgpuSoftware],
        Platform::Mac => &[WgpuMetal, Glow],
        Platform::Linux => &[WgpuVulkan, WgpuGl, Glow, WgpuSoftware],
        Platform::Android => &[Wgpu],
    };
    all.iter().copied().filter(|s| gl || *s != Glow).collect()
}

/// This build's ladder on this OS.
pub fn this_order() -> Vec<Stage> {
    order(Platform::current(), cfg!(feature = "gl-fallback"))
}

/// What to try, given the stage a previous start never finished and the stages earlier
/// crashes and stalls ruled out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ladder {
    Try(Stage),
    /// The last rung crashed the process: tell the user instead of looping.
    GiveUp,
}

impl Ladder {
    /// `crashed` is the stage in the leftover marker (`None` after a clean start), `skipped` the
    /// rungs earlier crashes and stalls ruled out, `order` the ladder.
    ///
    /// * The last rung crashed: give up (unless the ladder has only one rung: nothing to step to).
    /// * Otherwise the first rung that is neither skipped nor the one that crashed.
    /// * Every rung ruled out: the last one, which never leaves Eskiz with nothing to try.
    pub fn next(crashed: Option<Stage>, skipped: &[Stage], order: &[Stage]) -> Self {
        let Some(&last) = order.last() else { return Self::GiveUp };
        if order.len() > 1 && crashed == Some(last) {
            return Self::GiveUp;
        }
        let first = order.iter().copied().find(|s| !skipped.contains(s) && crashed != Some(*s));
        Self::Try(first.unwrap_or(last))
    }
}

/// The folder for the logs and the start-up markers: `ESKIZ_LOG_DIR`, else
/// `%LOCALAPPDATA%\Eskiz\logs` on Windows, else `logs` in the app data folder (`app_data`).
pub fn log_dir_from(env_dir: Option<&str>, local_app_data: Option<&str>, app_data: Option<&Path>) -> Option<PathBuf> {
    match (env_dir.filter(|d| !d.is_empty()), local_app_data.filter(|d| !d.is_empty())) {
        (Some(d), _) => Some(PathBuf::from(d)),
        (None, Some(l)) if cfg!(windows) => Some(Path::new(l).join("Eskiz").join("logs")),
        _ => app_data.map(|d| d.join("logs")),
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
    /// Renderers and the present mode that earlier crashes and stalls ruled out.
    skips: crate::stall::Skips,
    /// A sentence for a one-time toast about what the last run's stall changed.
    notice: Option<String>,
    /// Why this start stepped down a rung (only on the first start after the problem).
    step_reason: Option<String>,
    /// Set by the renderer when this start runs below the top rung of the ladder.
    safe: Option<SafeMode>,
    /// The safe-mode banner was handed out (it shows once per run).
    banner_taken: bool,
    /// The renderer being tried (for the watchdog).
    current: Option<Stage>,
}

/// This start runs below the top rung because of earlier problems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SafeMode {
    /// The rung in use.
    pub stage: Stage,
    /// Why Eskiz stepped down, on the first start after the problem; `None` on later starts.
    pub reason: Option<String>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            dir: None,
            started: Instant::now(),
            marker: None,
            last_panic: None,
            crashed: None,
            skips: crate::stall::Skips::default(),
            notice: None,
            step_reason: None,
            safe: None,
            banner_taken: false,
            current: None,
        })
    })
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
        crate::storage::app_data_root().as_deref(),
    );
    init_in(dir, args, &this_order())
}

/// [`init`] with the folder and the ladder given (tests use a temporary folder).
pub fn init_in(dir: Option<PathBuf>, args: &[String], order: &[Stage]) -> Option<Stage> {
    let crashed = dir.as_deref().and_then(take_marker);
    let stall = dir.as_deref().and_then(crate::stall::take_marker);
    let mut skips = dir.as_deref().map(crate::stall::Skips::load).unwrap_or_default();
    let before = skips.clone();
    let last = order.last().copied().unwrap_or(Stage::Wgpu);
    let mut step_reason = None;
    let notice = stall.as_ref().map(|m| {
        let had = skips.stages.len();
        let text = skips.learn(m, last);
        if skips.stages.len() > had {
            step_reason = Some(format!("The last run stopped responding while using {}.", m.stage.label()));
        }
        text
    });
    // A crash on any rung but the last is remembered: Eskiz stays below it until the user retries.
    // (A crash on the last rung makes the renderer give up and clear the list.)
    if let Some(c) = crashed.filter(|c| *c != last && order.contains(c)) {
        if !skips.stages.contains(&c) {
            skips.stages.push(c);
        }
        step_reason = Some(format!("The last start stopped before its window appeared while using {}.", c.label()));
    }
    if let Some(d) = &dir {
        if skips != before {
            skips.save(d);
        }
    }
    with(|s| {
        s.dir = dir.clone();
        s.started = Instant::now();
        s.crashed = crashed;
        s.skips = skips;
        s.notice = notice.clone();
        s.step_reason = step_reason;
        s.safe = None;
        s.banner_taken = false;
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
    if let (Some(m), Some(n)) = (&stall, &notice) {
        log(&format!("the previous run stalled (last phase '{}', renderer '{}'): {n}", m.phase, m.stage.name()));
    }
    let skipped = with(|s| s.skips.clone());
    if !skipped.stages.is_empty() {
        let names: Vec<&str> = skipped.stages.iter().map(|s| s.name()).collect();
        log(&format!("renderers skipped after earlier problems: {}", names.join(", ")));
    }
    crashed
}

/// The renderer runs `stage` below the top of the ladder: this start is in safe graphics mode.
pub fn enter_safe_mode(stage: Stage) {
    with(|s| s.safe = Some(SafeMode { stage, reason: s.step_reason.clone() }));
    log(&format!("safe graphics mode: starting on '{}' after earlier problems", stage.name()));
}

/// Whether this start is in safe graphics mode (Settings > Graphics shows it).
pub fn safe_mode() -> Option<SafeMode> {
    with(|s| s.safe.clone())
}

/// The safe-mode notice, once per run, and only on the first start after the step-down.
pub fn take_safe_banner() -> Option<SafeMode> {
    with(|s| {
        if s.banner_taken {
            return None;
        }
        let safe = s.safe.clone().filter(|m| m.reason.is_some())?;
        s.banner_taken = true;
        Some(safe)
    })
}

/// Try normal graphics: forget every renderer earlier crashes and stalls ruled out, so the next
/// start begins at the top of the ladder again.
pub fn retry_normal_graphics() {
    log("Try normal graphics: the skip list is cleared; the next start uses the top of the ladder");
    clear_skips();
    with(|s| s.safe = None);
}

/// What earlier crashes and stalls ruled out (set by [`init`]).
pub fn skips() -> crate::stall::Skips {
    with(|s| s.skips.clone())
}

/// Forget earlier crashes and stalls: `--renderer auto` and Try normal graphics start at the top again.
pub fn clear_skips() {
    with(|s| {
        s.skips = crate::stall::Skips::default();
        if let Some(d) = &s.dir {
            s.skips.save(d);
        }
    });
}

/// The one-time notice about a stall in the last run, once.
pub fn take_notice() -> Option<String> {
    with(|s| s.notice.take())
}

/// Queue a notice for the toast (a stall handled in this run).
pub fn set_notice(text: &str) {
    with(|s| s.notice = Some(text.to_owned()));
}

/// The renderer being tried now.
pub fn current_stage() -> Option<Stage> {
    with(|s| s.current)
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
        s.current = Some(stage);
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
    drop_marker();
}

/// An attempt ended with an error or a caught panic before it drew: the process is still alive, so
/// this was not a crash and the next start must not read it as one.
pub fn attempt_failed() {
    drop_marker();
}

fn drop_marker() {
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

    /// The tests that use the global start-up state run one at a time.
    fn global() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn ladder_steps_one_rung_down_after_each_crash_and_gives_up_on_the_last() {
        use Stage::*;
        let win = order(Platform::Windows, true);
        assert_eq!(win, [WgpuDx12, WgpuVulkan, Glow, WgpuSoftware]);
        // No marker: the top rung.
        assert_eq!(Ladder::next(None, &[], &win), Ladder::Try(WgpuDx12));
        // A marker for each rung: the next one down.
        assert_eq!(Ladder::next(Some(WgpuDx12), &[], &win), Ladder::Try(WgpuVulkan));
        assert_eq!(Ladder::next(Some(WgpuVulkan), &[WgpuDx12], &win), Ladder::Try(Glow));
        assert_eq!(Ladder::next(Some(Glow), &[WgpuDx12, WgpuVulkan], &win), Ladder::Try(WgpuSoftware));
        assert_eq!(Ladder::next(Some(WgpuSoftware), &[WgpuDx12, WgpuVulkan, Glow], &win), Ladder::GiveUp);
        // A crash on a lower rung with nothing skipped above it (a request crashed): the top.
        assert_eq!(Ladder::next(Some(WgpuVulkan), &[], &win), Ladder::Try(WgpuDx12));
        // Skips alone (stalls, or crashes remembered from earlier starts).
        assert_eq!(Ladder::next(None, &[WgpuDx12], &win), Ladder::Try(WgpuVulkan));
        assert_eq!(Ladder::next(None, &[WgpuDx12, WgpuVulkan, Glow], &win), Ladder::Try(WgpuSoftware));
        assert_eq!(Ladder::next(None, &[WgpuDx12, WgpuVulkan, Glow, WgpuSoftware], &win), Ladder::Try(WgpuSoftware), "never nothing");
        let mac = order(Platform::Mac, true);
        assert_eq!(Ladder::next(None, &[], &mac), Ladder::Try(WgpuMetal));
        assert_eq!(Ladder::next(Some(WgpuMetal), &[], &mac), Ladder::Try(Glow));
        assert_eq!(Ladder::next(Some(Glow), &[WgpuMetal], &mac), Ladder::GiveUp);
        let linux = order(Platform::Linux, true);
        assert_eq!(Ladder::next(Some(WgpuVulkan), &[], &linux), Ladder::Try(WgpuGl));
        assert_eq!(Ladder::next(Some(WgpuGl), &[WgpuVulkan], &linux), Ladder::Try(Glow));
        // One rung (Android): a crash has nowhere to go and is not a reason to stop.
        assert_eq!(Ladder::next(Some(Wgpu), &[], &order(Platform::Android, false)), Ladder::Try(Wgpu));
    }

    #[test]
    fn stage_names_round_trip() {
        use Stage::*;
        for s in [Wgpu, WgpuDx12, WgpuVulkan, WgpuMetal, WgpuGl, Glow, WgpuSoftware] {
            assert_eq!(Stage::parse(s.name()), Some(s));
            assert!(!s.label().is_empty());
        }
        assert_eq!(Stage::parse("junk"), None);
        assert_eq!(Stage::parse(" glow\n"), Some(Stage::Glow));
        assert!(Glow.cpu_compositor() && WgpuSoftware.cpu_compositor() && !WgpuDx12.cpu_compositor());
    }

    #[test]
    fn log_folder_is_local_app_data_on_windows_and_overridable() {
        let data = Path::new("/data/Eskiz");
        assert_eq!(log_dir_from(Some("/x"), Some("C:\\L"), Some(data)), Some(PathBuf::from("/x")));
        assert_eq!(log_dir_from(Some(""), None, None), None);
        assert_eq!(log_dir_from(None, None, Some(data)), Some(data.join("logs")), "macOS and Linux: the app data folder");
        let d = log_dir_from(None, Some("C:\\Users\\a\\AppData\\Local"), Some(data));
        if cfg!(windows) {
            assert!(d.unwrap().ends_with("Eskiz\\logs"));
        } else {
            assert_eq!(d, Some(data.join("logs")));
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
        let _g = global();
        let d = tmp("run");
        let win = order(Platform::Windows, true);
        fs::write(d.join(LOG), "old run\n").unwrap();
        let crashed = init_in(Some(d.clone()), &["--x".to_owned()], &win);
        assert_eq!(crashed, None);
        assert_eq!(fs::read_to_string(d.join(PREV)).unwrap(), "old run\n", "the previous log is kept");
        begin_attempt(Stage::WgpuDx12);
        assert!(d.join(MARKER).exists());
        // On Windows the marker is held open without sharing while the start is in progress.
        #[cfg(not(windows))]
        assert_eq!(fs::read_to_string(d.join(MARKER)).unwrap(), "wgpu-dx12");
        first_frames_ok();
        assert!(!d.join(MARKER).exists(), "a successful start removes the marker");
        let log_text = fs::read_to_string(d.join(LOG)).unwrap();
        assert!(log_text.contains("renderer attempt: wgpu-dx12") && log_text.contains("first frames drawn"), "{log_text}");
        // A crash leaves the marker; the next start sees it.
        begin_attempt(Stage::Glow);
        // A crashed process no longer holds the marker open.
        with(|s| s.marker = None);
        let again = init_in(Some(d.clone()), &[], &win);
        assert_eq!(again, Some(Stage::Glow));
        let _ = fs::remove_dir_all(&d);
    }

    /// The start-up sequence of `run_native` without a window: init, plan, attempt the first rung.
    fn start(dir: &Path, order: &[Stage]) -> crate::renderer::Plan {
        init_in(Some(dir.to_owned()), &[], order);
        let plan = crate::renderer::plan(crate::renderer::Mode::Auto, previous_crash(), &skips().stages, order).expect("a rung");
        if plan.safe {
            enter_safe_mode(plan.stages[0]);
        }
        begin_attempt(plan.stages[0]);
        plan
    }

    #[test]
    fn a_failed_attempt_does_not_leave_a_crash_marker() {
        let _g = global();
        let d = tmp("failed");
        let win = order(Platform::Windows, true);
        init_in(Some(d.clone()), &[], &win);
        begin_attempt(Stage::Glow);
        attempt_failed();
        assert!(!d.join(MARKER).exists());
        assert_eq!(init_in(Some(d.clone()), &[], &win), None);
        let _ = fs::remove_dir_all(&d);
    }

    /// The process dies (a driver crash, or the user ends a frozen start): the marker stays.
    fn crash() {
        with(|s| s.marker = None);
    }

    /// The window draws its first frames.
    fn draw() {
        first_frames_ok();
    }

    #[test]
    fn a_crashed_launch_moves_the_next_start_one_rung_down() {
        use Stage::*;
        let _g = global();
        let d = tmp("ladder");
        let win = order(Platform::Windows, true);
        // A clean first start: the top rung, no safe mode, no notice.
        let p = start(&d, &win);
        assert_eq!((p.stages[0], p.safe), (WgpuDx12, false));
        assert!(safe_mode().is_none() && take_safe_banner().is_none());
        draw();
        assert!(!d.join(MARKER).exists());
        // Same again: still the top (a clean start leaves nothing behind).
        assert_eq!(start(&d, &win).stages[0], WgpuDx12);
        // That start crashes before its first frames.
        crash();
        assert!(d.join(MARKER).exists(), "the launch marker survives the crash");
        // Next start: one rung down, in safe mode, with the notice.
        let p = start(&d, &win);
        assert_eq!((p.stages[0], p.safe), (WgpuVulkan, true));
        let banner = take_safe_banner().expect("the safe-mode notice");
        assert_eq!(banner.stage, WgpuVulkan);
        assert!(banner.reason.as_deref().unwrap_or("").contains("DirectX 12"), "{banner:?}");
        assert!(take_safe_banner().is_none(), "once per run");
        draw();
        // The start after a successful safe start stays down (no ping-pong back to the crash)...
        let p = start(&d, &win);
        assert_eq!((p.stages[0], p.safe), (WgpuVulkan, true));
        assert_eq!(safe_mode().map(|m| m.reason), Some(None), "safe mode, but no new notice");
        assert!(take_safe_banner().is_none());
        // ...and a crash there steps down once more.
        crash();
        assert_eq!(start(&d, &win).stages[0], Glow);
        crash();
        assert_eq!(start(&d, &win).stages[0], WgpuSoftware);
        draw();
        // Try normal graphics: back to the top on the next start.
        retry_normal_graphics();
        assert!(safe_mode().is_none());
        let p = start(&d, &win);
        assert_eq!((p.stages[0], p.safe), (WgpuDx12, false));
        draw();
        // A crash on the last rung gives up (the renderer clears the list and says so).
        for _ in 0..3 {
            start(&d, &win);
            crash();
        }
        init_in(Some(d.clone()), &[], &win);
        assert_eq!(skips().stages, [WgpuDx12, WgpuVulkan, Glow]);
        begin_attempt(WgpuSoftware);
        crash();
        init_in(Some(d.clone()), &[], &win);
        assert_eq!(previous_crash(), Some(WgpuSoftware));
        assert!(crate::renderer::plan(crate::renderer::Mode::Auto, previous_crash(), &skips().stages, &win).is_none());
        let log_text = fs::read_to_string(d.join(PREV)).unwrap_or_default() + &fs::read_to_string(d.join(LOG)).unwrap_or_default();
        assert!(log_text.contains("renderers skipped after earlier problems"), "{log_text}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_frozen_run_also_moves_the_next_start_down_and_says_why() {
        use Stage::*;
        let _g = global();
        let d = tmp("freeze");
        let mac = order(Platform::Mac, true);
        start(&d, &mac);
        draw();
        // The watchdog saw the window freeze; the user ended it.
        crate::stall::report_stall(Some(&d), WgpuMetal, crate::stall::Phase::Gallery, 3.5, false);
        let p = start(&d, &mac);
        assert_eq!((p.stages[0], p.safe), (Glow, true));
        let banner = take_safe_banner().expect("notice");
        assert!(banner.reason.unwrap().contains("stopped responding while using Metal"));
        let _ = fs::remove_dir_all(&d);
    }
}
