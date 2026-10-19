//! `eskiz --diagnose [--window]`: a plain-text (Markdown) report of everything that tends to go
//! wrong on a machine Eskiz has not run on before: which GPU backends and adapters exist, which
//! one Eskiz picks, whether `WGPU_BACKEND` is honoured, DPI awareness and scale factor, the pen
//! backend, the data folders, a save/load round trip and the MP4 encoder.
//!
//! The report is built in two steps so that the second can be tested without a GPU or a window:
//!
//! 1. [`collect`] (and [`collect_window`] for `--window`) runs the probes and returns a
//!    [`DiagnoseData`]. Every probe is guarded: a failure or a panic becomes an `error: ...`
//!    row, never a crash, and one failing probe does not stop the others.
//! 2. [`render`] turns a [`DiagnoseData`] into the Markdown table text.
//!
//! [`run`] does both, prints the report and writes `eskiz-diagnose.md` in the current directory,
//! so that a user on another machine can paste the file back.

use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use eframe::wgpu;
use eskiz_gpu::{AdapterProbe, GpuContext};

/// The file the report is written to, in the current directory.
pub const REPORT_FILE: &str = "eskiz-diagnose.md";

/// Frames the `--window` mode runs before it closes the window again.
pub const WINDOW_FRAMES: u32 = 30;

/// A probe result: a value, or the text of what went wrong.
pub type Probe<T> = Result<T, String>;

/// Run a probe so that an error return and a panic both come back as `Err`.
pub fn guarded<T>(probe: impl FnOnce() -> Probe<T>) -> Probe<T> {
    match catch_unwind(AssertUnwindSafe(probe)) {
        Ok(result) => result,
        Err(payload) => Err(format!("panicked: {}", panic_text(payload.as_ref()))),
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

// ---------------------------------------------------------------------------------- data

/// One environment variable and its value, if set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvVar {
    pub name: &'static str,
    pub value: Option<String>,
    /// What the variable does, one short phrase.
    pub effect: &'static str,
}

/// Machine and build facts.
#[derive(Clone, Debug)]
pub struct SystemInfo {
    pub os: String,
    pub os_version: Probe<String>,
    pub arch: String,
    /// `msvc`, `gnu`, `musl` or empty: tells a windows-msvc build from a windows-gnu one.
    pub target_env: String,
    pub eskiz_version: String,
    pub profile: &'static str,
    /// Cargo features that matter at run time, with whether they are compiled in.
    pub features: Vec<(&'static str, bool)>,
    pub parallelism: Probe<usize>,
    /// Physical memory in bytes.
    pub memory_bytes: Probe<u64>,
}

/// What one wgpu backend offered.
#[derive(Clone, Debug)]
pub struct BackendResult {
    pub name: &'static str,
    pub adapters: Probe<Vec<AdapterProbe>>,
}

/// The GPU section: every backend probed, and what Eskiz's own selection picks.
#[derive(Clone, Debug)]
pub struct GpuInfo {
    pub backends: Vec<BackendResult>,
    /// `GpuContext::new()`: the adapter the headless GPU compositor takes (honours `WGPU_BACKEND`).
    pub picked: Probe<PickedGpu>,
}

/// The adapter `GpuContext::new` chose.
#[derive(Clone, Debug)]
pub struct PickedGpu {
    pub description: String,
    pub supports_compositing: bool,
    pub max_canvas_edge: u32,
}

/// One data folder.
#[derive(Clone, Debug)]
pub struct DirInfo {
    pub label: &'static str,
    pub path: PathBuf,
    /// The environment variable that overrides this folder, and whether it is set.
    pub override_var: Option<(&'static str, bool)>,
    pub exists: bool,
    /// Whether a file can be created in the folder, or in its nearest existing parent.
    pub writable: Probe<bool>,
    /// The existing folder the writable test used, when `path` itself does not exist yet.
    pub checked_parent: Option<PathBuf>,
}

/// What `--window` learned from a real window.
#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub frames: u32,
    /// `wgpu` or `OpenGL (glow)`.
    pub renderer: String,
    pub adapter: Option<WindowAdapter>,
    pub pixels_per_point: f32,
    pub native_pixels_per_point: Option<f32>,
    pub inner_points: Option<(f32, f32)>,
    pub monitor_points: Option<(f32, f32)>,
    pub maximized: Option<bool>,
    pub fullscreen: Option<bool>,
    pub dpi_awareness: Probe<String>,
    pub pen: Probe<String>,
}

/// The adapter of the window's own wgpu surface.
#[derive(Clone, Debug)]
pub struct WindowAdapter {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    /// Whether `WGPU_BACKEND` was honoured (`None` when it is unset).
    pub env_honoured: Option<bool>,
}

/// Everything the report shows.
#[derive(Clone, Debug)]
pub struct DiagnoseData {
    pub system: SystemInfo,
    pub env: Vec<EnvVar>,
    /// The renderer request (`--renderer`, `ESKIZ_BACKEND`, ...) and this OS's renderer ladder.
    pub renderer_mode: String,
    pub gpu: GpuInfo,
    pub dirs: Vec<DirInfo>,
    pub pen: String,
    pub dialogs: String,
    pub round_trip: Probe<String>,
    pub mp4: Probe<String>,
    /// `None` without `--window`. `Some(Err)` when the window could not be opened.
    pub window: Option<Probe<WindowInfo>>,
    /// When the report was made.
    pub timestamp: String,
}

// ---------------------------------------------------------------------------------- collect

/// Run every headless probe.
pub fn collect() -> DiagnoseData {
    let features = vec![
        ("tablet", cfg!(feature = "tablet")),
        ("gl-fallback", cfg!(feature = "gl-fallback")),
        ("native-dialogs", cfg!(feature = "native-dialogs")),
    ];
    let system = SystemInfo {
        os: std::env::consts::OS.to_owned(),
        os_version: guarded(os_version),
        arch: std::env::consts::ARCH.to_owned(),
        target_env: target_env().to_owned(),
        eskiz_version: env!("CARGO_PKG_VERSION").to_owned(),
        profile: if cfg!(debug_assertions) { "debug" } else { "release" },
        features,
        parallelism: guarded(|| {
            std::thread::available_parallelism().map(usize::from).map_err(|e| e.to_string())
        }),
        memory_bytes: guarded(physical_memory),
    };
    DiagnoseData {
        system,
        env: env_vars(),
        renderer_mode: {
            let (mode, source) = crate::renderer::Mode::requested();
            let ladder: Vec<&str> = crate::startup::this_order().iter().map(|s| s.name()).collect();
            format!("{} (from {source}); ladder: {}", mode.name(), ladder.join(" -> "))
        },
        gpu: collect_gpu(),
        dirs: collect_dirs(),
        pen: pen_support().to_owned(),
        dialogs: dialog_support(),
        round_trip: guarded(round_trip),
        mp4: guarded(mp4_probe),
        window: None,
        timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S %:z").to_string(),
    }
}

fn target_env() -> &'static str {
    if cfg!(target_env = "msvc") {
        "msvc"
    } else if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else {
        ""
    }
}

/// The variables that steer rendering, storage and media, in the order they appear in the report.
fn env_vars() -> Vec<EnvVar> {
    const VARS: &[(&str, &str)] = &[
        ("WGPU_BACKEND", "limits wgpu to these backends (vulkan, dx12, metal, gl); the window and the GPU compositor both read it"),
        ("WGPU_POWER_PREF", "wgpu power preference for the window (high, low)"),
        ("WGPU_ADAPTER_NAME", "wgpu picks the adapter whose name contains this"),
        ("ESKIZ_BACKEND", "renderer: auto, dx12, vulkan, metal, wgpu-gl, gl or software (like --renderer)"),
        ("ESKIZ_RENDERER", "older switch: gpu (default), cpu (CPU compositor) or gl"),
        ("ESKIZ_GPU_REQUIRE", "GPU tests fail instead of skipping when there is no adapter"),
        ("ESKIZ_LIBRARY_DIR", "document library folder"),
        ("ESKIZ_BRUSH_DIR", "imported brushes folder"),
        ("ESKIZ_FONT_DIR", "imported fonts folder"),
        ("ESKIZ_OPENH264_LIB", "runtime OpenH264 library (only with the libloading feature)"),
    ];
    VARS.iter()
        .map(|&(name, effect)| EnvVar {
            name,
            value: crate::compat::env_os(name).map(|v| v.to_string_lossy().into_owned()),
            effect,
        })
        .collect()
}

fn collect_gpu() -> GpuInfo {
    let backends = [
        ("Vulkan", wgpu::Backends::VULKAN),
        ("DX12", wgpu::Backends::DX12),
        ("GL", wgpu::Backends::GL),
        ("Metal", wgpu::Backends::METAL),
    ]
    .into_iter()
    .map(|(name, flag)| BackendResult {
        name,
        adapters: guarded(|| Ok(eskiz_gpu::probe_adapters(flag))),
    })
    .collect();
    let picked = guarded(|| {
        let ctx = GpuContext::new().map_err(|e| e.to_string())?;
        Ok(PickedGpu {
            description: ctx.describe(),
            supports_compositing: ctx.supports_compositing(),
            max_canvas_edge: ctx.max_canvas_edge(),
        })
    });
    GpuInfo { backends, picked }
}

fn collect_dirs() -> Vec<DirInfo> {
    let library = crate::storage::Library::default_location();
    let lib_set = std::env::var_os(crate::storage::LIBRARY_ENV).is_some_and(|v| !v.is_empty());
    let set = |name: &str| crate::compat::env_os(name).is_some_and(|v| !v.is_empty());
    let mut dirs = vec![
        dir_info("Library", library.root().to_owned(), Some((crate::storage::LIBRARY_ENV, lib_set))),
        dir_info("Recovery", library.recovery_root().to_owned(), None),
        dir_info("Brushes", crate::storage::brushes_dir(), Some(("ESKIZ_BRUSH_DIR", set("ESKIZ_BRUSH_DIR")))),
        dir_info("Fonts", crate::storage::fonts_dir(), Some(("ESKIZ_FONT_DIR", set("ESKIZ_FONT_DIR")))),
    ];
    // The OS-level folder the defaults hang off, for a "wrong place" report.
    if let Some(project) = crate::storage::app_data_root() {
        dirs.insert(0, dir_info("App data", project, None));
    }
    dirs
}

fn dir_info(label: &'static str, path: PathBuf, override_var: Option<(&'static str, bool)>) -> DirInfo {
    let exists = path.is_dir();
    let (writable, checked_parent) = if exists {
        (guarded(|| can_create_file(&path)), None)
    } else {
        match path.ancestors().skip(1).find(|p| p.is_dir()) {
            Some(parent) => (guarded(|| can_create_file(parent)), Some(parent.to_owned())),
            None => (Err("no existing parent folder".to_owned()), None),
        }
    };
    DirInfo { label, path, override_var, exists, writable, checked_parent }
}

/// Create and remove a small file in `dir`.
fn can_create_file(dir: &Path) -> Probe<bool> {
    let probe = dir.join(format!(".eskiz-diagnose-{}", std::process::id()));
    match std::fs::write(&probe, b"eskiz") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// What the pen backend can do on this build, without a window. With `--window` the connection
/// itself is tried.
fn pen_support() -> &'static str {
    if !cfg!(feature = "tablet") {
        "not built in (cargo feature `tablet` is off)"
    } else if cfg!(windows) {
        "Windows Ink (octotablet 0.1, RealTimeStylus); Wintab and the Pointer API are not supported. The connection needs a window: use --window"
    } else if cfg!(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android", target_os = "redox")))) {
        "Wayland tablet protocol (octotablet 0.1); X11 is not supported. The connection needs a window: use --window"
    } else {
        "no tablet backend on this platform; mouse and touch only"
    }
}

fn dialog_support() -> String {
    if !cfg!(feature = "native-dialogs") {
        return "not built in (cargo feature `native-dialogs` is off); path fields only".to_owned();
    }
    let backend = if cfg!(windows) {
        "IFileDialog (Windows common dialogs)"
    } else if cfg!(target_os = "macos") {
        "NSOpenPanel / NSSavePanel"
    } else {
        "xdg-desktop-portal over D-Bus"
    };
    let mut text = format!("built in, `rfd` backend: {backend}");
    if cfg!(all(unix, not(target_os = "macos"))) {
        let bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
        let _ = write!(text, "; session bus address {}", if bus { "set" } else { "NOT set (a pick will answer 'unavailable')" });
    }
    text
}

/// Save a small document to a temporary library, load it back and compare pixels and layer
/// properties: the gallery's own code path, in a folder this probe creates and removes. (It does
/// not call `storage::selftest`, whose folder is named by process id only and would collide with
/// a concurrent run.)
fn round_trip() -> Probe<String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "eskiz-diagnose-library-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let started = std::time::Instant::now();
    let result = round_trip_in(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let layers = result?;
    Ok(format!(
        "ok: saved, listed and reloaded a 300x200 document with {layers} layers, pixels and layer properties identical, in {:.0} ms (temp folder {})",
        started.elapsed().as_secs_f64() * 1000.0,
        std::env::temp_dir().display()
    ))
}

fn round_trip_in(dir: &Path) -> Probe<usize> {
    use crate::storage::{meta_signature, new_id, sample_document, Library};
    let library = Library::at(dir);
    let doc = sample_document(300, 200);
    let id = new_id();
    library.save(&id, "Diagnose", &doc)?;
    if library.list().len() != 1 {
        return Err("the saved document is not listed".to_owned());
    }
    let back = library.load(&id)?;
    let same = back.layer_count() == doc.layer_count()
        && (0..doc.layer_count()).all(|i| back.layer_at(i) == doc.layer_at(i))
        && meta_signature(&back) == meta_signature(&doc);
    if !same {
        return Err("the document changed in the save/load round trip".to_owned());
    }
    library.delete(&id)?;
    Ok(doc.layer_count())
}

/// Encode three tiny frames to MP4 and check the container header.
fn mp4_probe() -> Probe<String> {
    use eskiz_media::{encode_mp4, Frame, Mp4Options};
    if !eskiz_media::encoder_available() {
        return Ok(format!("off: {}", eskiz_media::MP4_UNAVAILABLE));
    }
    let (w, h) = (64u32, 48u32);
    let frames = (0..3u8)
        .map(|i| {
            let px = [40 + 80 * i, 120, 200 - 60 * i, 255];
            let pixels = px.repeat((w * h) as usize);
            Frame::new(w, h, pixels, std::time::Duration::from_millis(100)).map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let started = std::time::Instant::now();
    let bytes = encode_mp4(&frames, &Mp4Options::default()).map_err(|e| e.to_string())?;
    let ftyp = bytes.get(4..8) == Some(b"ftyp");
    if !ftyp {
        return Err(format!("encoder returned {} bytes without an ftyp box", bytes.len()));
    }
    let lib = crate::compat::env_var("ESKIZ_OPENH264_LIB");
    Ok(format!(
        "ok: 3 frames of {w}x{h} became {} bytes of MP4 in {:.0} ms; OpenH264 from {}",
        bytes.len(),
        started.elapsed().as_secs_f64() * 1000.0,
        match lib {
            Some(path) => format!("ESKIZ_OPENH264_LIB={path} if the `libloading` feature is on, else the compiled-in source"),
            None => "the compiled-in source build".to_owned(),
        }
    ))
}

// ---------------------------------------------------------------------------------- OS probes

#[cfg(target_os = "macos")]
fn os_version() -> Probe<String> {
    command_output("sw_vers", &["-productVersion"]).map(|v| format!("macOS {v}"))
}

#[cfg(target_os = "linux")]
fn os_version() -> Probe<String> {
    let text = std::fs::read_to_string("/etc/os-release").map_err(|e| e.to_string())?;
    let pretty = text
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_owned());
    pretty.ok_or_else(|| "no PRETTY_NAME in /etc/os-release".to_owned())
}

#[cfg(windows)]
fn os_version() -> Probe<String> {
    // `ver` prints "Microsoft Windows [Version 10.0.22631.4317]". A GUI process must not flash a
    // console window for it, hence CREATE_NO_WINDOW.
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn os_version() -> Probe<String> {
    Err("not implemented for this OS".to_owned())
}

#[cfg(target_os = "macos")]
fn command_output(program: &str, args: &[&str]) -> Probe<String> {
    let out = std::process::Command::new(program).args(args).output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(target_os = "macos")]
fn physical_memory() -> Probe<u64> {
    command_output("sysctl", &["-n", "hw.memsize"])?.parse().map_err(|e| format!("hw.memsize: {e}"))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn physical_memory() -> Probe<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
    crate::history::parse_meminfo_total(&text).ok_or_else(|| "no MemTotal in /proc/meminfo".to_owned())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn physical_memory() -> Probe<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `status` is a correctly sized MEMORYSTATUSEX with `length` set, as the API requires.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err("GlobalMemoryStatusEx failed".to_owned());
    }
    Ok(status.total_phys)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android", windows)))]
fn physical_memory() -> Probe<u64> {
    Err("not implemented for this OS".to_owned())
}

/// The DPI awareness of the calling thread (Windows only): `unaware`, `system`, `per-monitor` or
/// `per-monitor v2`, plus the system DPI. Eskiz relies on winit to switch it on at start-up (no
/// manifest is embedded), so `unaware` would mean the UI is bitmap-scaled by Windows and blurry.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn dpi_awareness() -> Probe<String> {
    #[link(name = "user32")]
    extern "system" {
        fn GetThreadDpiAwarenessContext() -> isize;
        fn AreDpiAwarenessContextsEqual(a: isize, b: isize) -> i32;
        fn GetDpiForSystem() -> u32;
    }
    // The pseudo-handles of DPI_AWARENESS_CONTEXT_*.
    const CONTEXTS: [(isize, &str); 5] = [
        (-1, "unaware"),
        (-2, "system"),
        (-3, "per-monitor"),
        (-4, "per-monitor v2"),
        (-5, "unaware (GDI scaled)"),
    ];
    // SAFETY: these user32 calls take and return plain integers and have no preconditions.
    let (name, system_dpi) = unsafe {
        let current = GetThreadDpiAwarenessContext();
        let name = CONTEXTS
            .iter()
            .find(|(c, _)| AreDpiAwarenessContextsEqual(current, *c) != 0)
            .map_or("unknown", |(_, n)| *n);
        (name, GetDpiForSystem())
    };
    Ok(format!("{name} (system DPI {system_dpi} = {}%)", system_dpi * 100 / 96))
}

#[cfg(not(windows))]
pub fn dpi_awareness() -> Probe<String> {
    Ok("not applicable: DPI awareness is a Windows setting".to_owned())
}

// ---------------------------------------------------------------------------------- window

/// Whether the backend a window ended up on is one `WGPU_BACKEND` allowed. `None` when the
/// variable is unset or empty.
pub fn backend_honoured(env: Option<&str>, actual: wgpu::Backend) -> Option<bool> {
    let env = env.map(str::trim).filter(|v| !v.is_empty())?;
    Some(wgpu::Backends::from_comma_list(env).contains(wgpu::Backends::from(actual)))
}

/// Open a window, let it run [`WINDOW_FRAMES`] frames and report what it saw. Uses the same
/// renderer set-up and fallback as the real app.
pub fn collect_window() -> Probe<WindowInfo> {
    use std::sync::{Arc, Mutex};
    let slot: Arc<Mutex<Option<WindowInfo>>> = Arc::new(Mutex::new(None));
    let result = guarded(|| {
        crate::renderer::run_native(
            "Eskiz diagnose",
            || eframe::NativeOptions {
                viewport: eframe::egui::ViewportBuilder::default()
                    .with_title("Eskiz diagnose")
                    .with_inner_size([640.0, 360.0]),
                ..eframe::NativeOptions::default()
            },
            |cc, _mode| {
                let renderer = if cc.wgpu_render_state.is_some() { "wgpu" } else { "OpenGL (glow)" };
                let adapter = cc.wgpu_render_state.as_ref().map(|rs| {
                    let info = rs.adapter.get_info();
                    WindowAdapter {
                        name: info.name.clone(),
                        backend: format!("{:?}", info.backend),
                        device_type: format!("{:?}", info.device_type),
                        driver: format!("{} {}", info.driver, info.driver_info).trim().to_owned(),
                        env_honoured: backend_honoured(std::env::var("WGPU_BACKEND").ok().as_deref(), info.backend),
                    }
                });
                // Connect the pen now: the handles it needs are only valid while the window lives.
                let (pen, pen_text) = match crate::tablet_backend::connect(cc) {
                    Ok(source) => {
                        let text = format!("connected: {}", source.name());
                        (Some(source), Ok(text))
                    }
                    Err(e) => (None, Err(e)),
                };
                let app: Box<dyn eframe::App> = Box::new(DiagnoseApp {
                    frames: 0,
                    slot: Arc::clone(&slot),
                    info: WindowInfo {
                        frames: 0,
                        renderer: renderer.to_owned(),
                        adapter,
                        pixels_per_point: 1.0,
                        native_pixels_per_point: None,
                        inner_points: None,
                        monitor_points: None,
                        maximized: None,
                        fullscreen: None,
                        dpi_awareness: Err("not read yet".to_owned()),
                        pen: pen_text,
                    },
                    pen,
                });
                Ok(app)
            },
        )
    });
    result?;
    let taken = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    taken.ok_or_else(|| "the window closed before it drew a frame".to_owned())
}

/// The app that runs in the `--window` mode: draws a little text, reads the window facts on the
/// last frame and closes itself.
struct DiagnoseApp {
    frames: u32,
    slot: std::sync::Arc<std::sync::Mutex<Option<WindowInfo>>>,
    info: WindowInfo,
    /// Kept alive until the report is taken, then dropped before the window goes away.
    pen: Option<Box<dyn crate::tablet::PenSource>>,
}

impl eframe::App for DiagnoseApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        use eframe::egui;
        self.frames += 1;
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Eskiz diagnose");
            ui.label(format!("frame {} of {WINDOW_FRAMES}", self.frames));
            ui.label("This window closes by itself.");
        });
        if self.frames < WINDOW_FRAMES {
            ctx.request_repaint();
            return;
        }
        let viewport = ctx.input(|i| i.viewport().clone());
        let native = viewport.native_pixels_per_point;
        self.info.frames = self.frames;
        self.info.pixels_per_point = ctx.pixels_per_point();
        self.info.native_pixels_per_point = native;
        self.info.inner_points = viewport.inner_rect.map(|r| (r.width(), r.height()));
        self.info.monitor_points = viewport.monitor_size.map(|s| (s.x, s.y));
        self.info.maximized = viewport.maximized;
        self.info.fullscreen = viewport.fullscreen;
        self.info.dpi_awareness = dpi_awareness();
        *self.slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(self.info.clone());
        // The pen connection borrows the window's display, so it goes first.
        self.pen = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

// ---------------------------------------------------------------------------------- render

/// Escape a value for a Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\n', '\r'], " ")
}

fn table(out: &mut String, rows: &[(String, String)]) {
    out.push_str("| Item | Value |\n|---|---|\n");
    for (k, v) in rows {
        let _ = writeln!(out, "| {} | {} |", cell(k), cell(v));
    }
    out.push('\n');
}

fn probe_text<T>(p: &Probe<T>, ok: impl FnOnce(&T) -> String) -> String {
    match p {
        Ok(v) => ok(v),
        Err(e) => format!("error: {e}"),
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn opt_bool(b: Option<bool>) -> &'static str {
    b.map_or("unknown", yes_no)
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// The section headings, in order. `--selftest` checks the rendered report for each of them.
pub const SECTIONS: [&str; 8] = [
    "## System",
    "## Environment",
    "## GPU backends",
    "## Data folders",
    "## Pen and file dialogs",
    "## Save and load",
    "## MP4 encoder",
    "## Window",
];

/// Render the report.
pub fn render(data: &DiagnoseData) -> String {
    let mut out = String::new();
    let s = &data.system;
    let _ = writeln!(out, "# Eskiz diagnose report\n");
    let _ = writeln!(out, "Made {}. Paste this whole file back when asking for help.\n", data.timestamp);

    out.push_str("## System\n\n");
    let features = s
        .features
        .iter()
        .map(|(n, on)| format!("{n}={}", if *on { "on" } else { "off" }))
        .collect::<Vec<_>>()
        .join(", ");
    table(
        &mut out,
        &[
            ("Eskiz".into(), format!("{} ({} build, {})", s.eskiz_version, s.profile, features)),
            ("OS".into(), format!("{} - {}", s.os, probe_text(&s.os_version, Clone::clone))),
            (
                "Architecture".into(),
                format!("{}{}", s.arch, if s.target_env.is_empty() { String::new() } else { format!(" ({} target)", s.target_env) }),
            ),
            ("CPU threads".into(), probe_text(&s.parallelism, |n| n.to_string())),
            ("Physical memory".into(), probe_text(&s.memory_bytes, |b| mib(*b))),
        ],
    );

    out.push_str("## Environment\n\n");
    let mut rows: Vec<(String, String)> = data
        .env
        .iter()
        .map(|e| {
            let value = e.value.as_deref().map_or("(not set)".to_owned(), |v| format!("`{v}`"));
            (e.name.to_owned(), format!("{value} - {}", e.effect))
        })
        .collect();
    rows.push(("Renderer request".into(), data.renderer_mode.clone()));
    table(&mut out, &rows);

    out.push_str("## GPU backends\n\n");
    let mut rows = Vec::new();
    for b in &data.gpu.backends {
        match &b.adapters {
            Err(e) => rows.push((b.name.to_owned(), format!("error: {e}"))),
            Ok(list) if list.is_empty() => rows.push((
                b.name.to_owned(),
                "no adapters (backend not compiled in, or no driver for it)".to_owned(),
            )),
            Ok(list) => {
                for a in list {
                    rows.push((format!("{} adapter", b.name), adapter_text(a)));
                }
            }
        }
    }
    rows.push((
        "Eskiz's own pick (GpuContext::new)".into(),
        probe_text(&data.gpu.picked, |p| {
            format!(
                "{}; compositing {}; largest canvas edge {} px",
                p.description,
                if p.supports_compositing { "supported" } else { "NOT supported" },
                p.max_canvas_edge
            )
        }),
    ));
    rows.push((
        "Compositor needs".into(),
        "compute shaders, >= 4 storage buffers and >= 1 storage texture per stage, >= 64 texture array layers, >= 4096 px textures".into(),
    ));
    table(&mut out, &rows);

    out.push_str("## Data folders\n\n");
    out.push_str("| Folder | Path | Exists | Writable | Override |\n|---|---|---|---|---|\n");
    for d in &data.dirs {
        let writable = match (&d.writable, &d.checked_parent) {
            (Ok(w), None) => yes_no(*w).to_owned(),
            (Ok(w), Some(p)) => format!("{} (checked the parent {})", yes_no(*w), p.display()),
            (Err(e), _) => format!("error: {e}"),
        };
        let over = match d.override_var {
            Some((name, true)) => format!("{name} is set"),
            Some((name, false)) => format!("{name} not set"),
            None => "-".to_owned(),
        };
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {} |",
            cell(d.label),
            cell(&d.path.display().to_string()),
            yes_no(d.exists),
            cell(&writable),
            cell(&over)
        );
    }
    out.push('\n');

    out.push_str("## Pen and file dialogs\n\n");
    let pen_now = match &data.window {
        Some(Ok(w)) => match &w.pen {
            Ok(text) => text.clone(),
            Err(e) => format!("not connected: {e}"),
        },
        Some(Err(_)) => "not tried (the window failed)".to_owned(),
        None => "not tried (run with --window)".to_owned(),
    };
    table(
        &mut out,
        &[
            ("Pen backend".into(), data.pen.clone()),
            ("Pen connection".into(), pen_now),
            ("File dialogs".into(), data.dialogs.clone()),
        ],
    );

    out.push_str("## Save and load\n\n");
    table(&mut out, &[("Library round trip".into(), probe_text(&data.round_trip, Clone::clone))]);

    out.push_str("## MP4 encoder\n\n");
    table(&mut out, &[("OpenH264".into(), probe_text(&data.mp4, Clone::clone))]);

    out.push_str("## Window\n\n");
    match &data.window {
        None => out.push_str("Not requested. Run `eskiz --diagnose --window` to open a window and report its scale factor, size, renderer and DPI awareness.\n\n"),
        Some(Err(e)) => {
            let _ = writeln!(out, "error: {e}\n");
        }
        Some(Ok(w)) => table(&mut out, &window_rows(w)),
    }
    out
}

fn adapter_text(a: &AdapterProbe) -> String {
    let i = &a.info;
    let driver = format!("{} {}", i.driver, i.driver_info).trim().to_owned();
    format!(
        "{} - {:?}, driver {}; vendor 0x{:04x} device 0x{:04x}; limits: compute {}, storage buffers {}, storage textures {}, array layers {}, 2D {} px -> {}; device: {}",
        i.name,
        i.device_type,
        if driver.is_empty() { "unknown".to_owned() } else { driver },
        i.vendor,
        i.device,
        yes_no(a.compute_shaders),
        a.max_storage_buffers_per_shader_stage,
        a.max_storage_textures_per_shader_stage,
        a.max_texture_array_layers,
        a.max_texture_dimension_2d,
        if a.meets_requirements { "meets the compositor's needs" } else { "TOO LOW for the compositor" },
        match &a.device {
            Ok(()) => "created".to_owned(),
            Err(e) => format!("error: {e}"),
        }
    )
}

fn window_rows(w: &WindowInfo) -> Vec<(String, String)> {
    let pair = |p: Option<(f32, f32)>| p.map_or("unknown".to_owned(), |(a, b)| format!("{a:.0} x {b:.0}"));
    let physical = |p: Option<(f32, f32)>, scale: Option<f32>| match (p, scale) {
        (Some((a, b)), Some(s)) => format!("{:.0} x {:.0}", a * s, b * s),
        _ => "unknown".to_owned(),
    };
    let native = w.native_pixels_per_point;
    let mut rows = vec![
        ("Renderer".to_owned(), w.renderer.clone()),
        ("Frames drawn".to_owned(), w.frames.to_string()),
        (
            "Scale factor (egui pixels_per_point)".to_owned(),
            format!(
                "{:.3} (the OS says {})",
                w.pixels_per_point,
                native.map_or("unknown".to_owned(), |n| format!("{n:.3}"))
            ),
        ),
        ("Window size, points".to_owned(), pair(w.inner_points)),
        ("Window size, physical pixels".to_owned(), physical(w.inner_points, native)),
        ("Monitor size, points".to_owned(), pair(w.monitor_points)),
        ("Monitor size, physical pixels".to_owned(), physical(w.monitor_points, native)),
        (
            "Other monitors".to_owned(),
            "not listed: eframe exposes only the monitor the window is on. Drag the window to each monitor and rerun.".to_owned(),
        ),
        (
            "Maximized / fullscreen".to_owned(),
            format!("{} / {}", opt_bool(w.maximized), opt_bool(w.fullscreen)),
        ),
        ("DPI awareness".to_owned(), probe_text(&w.dpi_awareness, Clone::clone)),
    ];
    match &w.adapter {
        Some(a) => {
            rows.push((
                "Window adapter".to_owned(),
                format!("{} - {}, {}, driver {}", a.name, a.backend, a.device_type, if a.driver.is_empty() { "unknown" } else { &a.driver }),
            ));
            rows.push((
                "WGPU_BACKEND honoured by the window".to_owned(),
                match a.env_honoured {
                    None => "WGPU_BACKEND is not set".to_owned(),
                    Some(true) => format!("yes (the window uses {})", a.backend),
                    Some(false) => format!("NO: the window uses {} although WGPU_BACKEND asks for something else", a.backend),
                },
            ));
        }
        None => rows.push(("Window adapter".to_owned(), "none: the window runs on OpenGL (glow), not wgpu".to_owned())),
    }
    rows
}

// ---------------------------------------------------------------------------------- run

/// The `--diagnose` command: collect, print and save. Returns the report text.
pub fn run(window: bool) -> Result<String, String> {
    let mut data = collect();
    if window {
        data.window = Some(collect_window());
    }
    let report = render(&data);
    let path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(REPORT_FILE);
    match std::fs::write(&path, &report) {
        Ok(()) => Ok(format!("{report}\nWritten to {}", path.display())),
        Err(e) => Ok(format!("{report}\nCould not write {}: {e}", path.display())),
    }
}

/// The `--selftest` line: the no-window report is built and has every section.
pub fn selftest() -> Result<String, String> {
    let report = render(&collect());
    for heading in SECTIONS {
        if !report.contains(heading) {
            return Err(format!("diagnose: report has no '{heading}' section"));
        }
    }
    let errors = report.lines().filter(|l| l.contains("error:")).count();
    Ok(format!(
        "  diagnose: no-window report has all {} sections ({} lines, {errors} probe error rows)",
        SECTIONS.len(),
        report.lines().count()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DiagnoseData {
        let ok_dir = DirInfo {
            label: "Library",
            path: PathBuf::from("/data/Eskiz|x"),
            override_var: Some(("ESKIZ_LIBRARY_DIR", false)),
            exists: false,
            writable: Ok(true),
            checked_parent: Some(PathBuf::from("/data")),
        };
        let bad_dir = DirInfo {
            label: "Fonts",
            path: PathBuf::from("/nowhere/Fonts"),
            override_var: Some(("ESKIZ_FONT_DIR", true)),
            exists: false,
            writable: Err("no existing parent folder".into()),
            checked_parent: None,
        };
        DiagnoseData {
            system: SystemInfo {
                os: "windows".into(),
                os_version: Ok("Microsoft Windows [Version 10.0.22631.4317]".into()),
                arch: "x86_64".into(),
                target_env: "msvc".into(),
                eskiz_version: "0.1.0".into(),
                profile: "release",
                features: vec![("tablet", true), ("gl-fallback", false)],
                parallelism: Ok(8),
                memory_bytes: Err("denied".into()),
            },
            env: vec![
                EnvVar { name: "WGPU_BACKEND", value: Some("dx12".into()), effect: "limits wgpu" },
                EnvVar { name: "ESKIZ_RENDERER", value: None, effect: "gpu, cpu or gl" },
            ],
            renderer_mode: "Auto".into(),
            gpu: GpuInfo {
                backends: vec![
                    BackendResult { name: "Vulkan", adapters: Ok(vec![]) },
                    BackendResult { name: "DX12", adapters: Err("panicked: boom".into()) },
                ],
                picked: Err("no usable GPU adapter is available: none".into()),
            },
            dirs: vec![ok_dir, bad_dir],
            pen: "Windows Ink".into(),
            dialogs: "built in".into(),
            round_trip: Ok("ok".into()),
            mp4: Err("encoder unavailable".into()),
            window: Some(Err("no display".into())),
            timestamp: "2026-10-15 12:00:00 +00:00".into(),
        }
    }

    #[test]
    fn report_has_every_section_and_shows_errors_as_rows() {
        let text = render(&sample());
        for heading in SECTIONS {
            assert!(text.contains(heading), "missing {heading}\n{text}");
        }
        assert!(text.contains("| Physical memory | error: denied |"), "{text}");
        assert!(text.contains("| DX12 | error: panicked: boom |"), "{text}");
        assert!(text.contains("| Vulkan | no adapters"), "{text}");
        assert!(text.contains("error: no usable GPU adapter"), "{text}");
        assert!(text.contains("| OpenH264 | error: encoder unavailable |"), "{text}");
        assert!(text.contains("error: no display"), "{text}");
        assert!(text.contains("(msvc target)"), "{text}");
        assert!(text.contains("ESKIZ_FONT_DIR is set"), "{text}");
        assert!(text.contains("checked the parent /data"), "{text}");
        // A pipe in a path must not break the table.
        assert!(text.contains("/data/Eskiz\\|x"), "{text}");
    }

    #[test]
    fn window_rows_show_scale_and_the_wgpu_backend_check() {
        let mut data = sample();
        data.window = Some(Ok(WindowInfo {
            frames: 30,
            renderer: "wgpu".into(),
            adapter: Some(WindowAdapter {
                name: "Test GPU".into(),
                backend: "Vulkan".into(),
                device_type: "DiscreteGpu".into(),
                driver: "nvidia 555".into(),
                env_honoured: Some(false),
            }),
            pixels_per_point: 1.5,
            native_pixels_per_point: Some(1.5),
            inner_points: Some((1280.0, 860.0)),
            monitor_points: Some((1280.0, 720.0)),
            maximized: Some(false),
            fullscreen: Some(false),
            dpi_awareness: Ok("per-monitor v2 (system DPI 144 = 150%)".into()),
            pen: Ok("connected: Windows Ink".into()),
        }));
        let text = render(&data);
        assert!(text.contains("1.500 (the OS says 1.500)"), "{text}");
        assert!(text.contains("1920 x 1290"), "{text}");
        assert!(text.contains("per-monitor v2"), "{text}");
        assert!(text.contains("NO: the window uses Vulkan"), "{text}");
        assert!(text.contains("connected: Windows Ink"), "{text}");
    }

    #[test]
    fn wgpu_backend_env_check() {
        assert_eq!(backend_honoured(None, wgpu::Backend::Vulkan), None);
        assert_eq!(backend_honoured(Some("  "), wgpu::Backend::Vulkan), None);
        assert_eq!(backend_honoured(Some("vulkan"), wgpu::Backend::Vulkan), Some(true));
        assert_eq!(backend_honoured(Some("vk,gl"), wgpu::Backend::Gl), Some(true));
        assert_eq!(backend_honoured(Some("dx12"), wgpu::Backend::Vulkan), Some(false));
        assert_eq!(backend_honoured(Some("d3d12"), wgpu::Backend::Dx12), Some(true));
    }

    #[test]
    fn a_panicking_probe_becomes_an_error() {
        let r: Probe<u8> = guarded(|| panic!("probe blew up"));
        assert_eq!(r, Err("panicked: probe blew up".to_owned()));
        let r: Probe<u8> = guarded(|| Err("plain".into()));
        assert_eq!(r, Err("plain".to_owned()));
        assert_eq!(guarded(|| Ok(3)), Ok(3));
    }

    #[test]
    fn dir_probe_finds_the_nearest_existing_parent() {
        let base = std::env::temp_dir().join(format!("eskiz-diagnose-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let missing = base.join("a").join("b");
        let info = dir_info("Test", missing.clone(), None);
        assert!(!info.exists);
        assert_eq!(info.checked_parent.as_deref(), Some(base.as_path()));
        assert_eq!(info.writable, Ok(true));
        let here = dir_info("Test", base.clone(), None);
        assert!(here.exists && here.checked_parent.is_none());
        assert_eq!(here.writable, Ok(true));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The real probes, with no window: must not panic, and every section is there. A machine
    /// without a GPU shows 'no adapters' or an error row, which is also fine.
    #[test]
    fn headless_collect_renders_every_section() {
        let data = collect();
        let text = render(&data);
        for heading in SECTIONS {
            assert!(text.contains(heading), "missing {heading}\n{text}");
        }
        assert!(data.round_trip.is_ok(), "{:?}", data.round_trip);
        assert!(data.mp4.is_ok(), "{:?}", data.mp4);
        if !eskiz_media::encoder_available() {
            assert!(data.mp4.as_ref().unwrap().contains("not available in this build"));
        }
        assert_eq!(data.gpu.backends.len(), 4);
        assert!(data.window.is_none());
        assert!(text.contains("Not requested"), "{text}");
    }

    #[test]
    fn selftest_line_reports_the_sections() {
        let line = selftest().unwrap();
        assert!(line.contains("all 8 sections"), "{line}");
    }
}
