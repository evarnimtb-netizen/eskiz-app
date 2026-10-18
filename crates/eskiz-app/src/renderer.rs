//! Which renderer the window uses, and what happens when it cannot start.
//!
//! * **Default:** eframe's wgpu renderer (Metal on macOS, DX12 or Vulkan on Windows, Vulkan or
//!   OpenGL on Linux), and the GPU compositor of `eskiz-gpu` for the canvas.
//! * **`ESKIZ_RENDERER`** picks the path by hand:
//!   `gpu` or unset: wgpu window, GPU compositor (falls back to the CPU compositor for a
//!   document it cannot show exactly, or if the device cannot run it);
//!   `cpu`: wgpu window, the CPU compositor and an egui texture, exactly the old display path;
//!   `gl`: eframe's glow (OpenGL) window and the CPU compositor. Needs the `gl-fallback` cargo
//!   feature, which is on by default.
//! * **Fallback:** if the wgpu window cannot be created (no adapter, no surface) and the `gl-fallback`
//!   feature is on, Eskiz retries once with glow. This is only done before the application was created, so
//!   it can never throw away a document. Without the feature, or if glow fails too, `run_native`
//!   returns an error that says what to try.

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui_wgpu::WgpuSetup;
use eframe::{wgpu, CreationContext, NativeOptions};

use crate::startup::{self, Ladder, Stage};

/// What `ESKIZ_RENDERER` asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Not set: the GPU compositor, with every fallback.
    Auto,
    /// `gpu`: the same as `Auto`, spelled out.
    Gpu,
    /// `cpu`: wgpu window, CPU compositor.
    Cpu,
    /// `gl`: glow window, CPU compositor.
    Gl,
    /// `dx12`: wgpu restricted to DirectX 12 (falls back to glow if it cannot start).
    Dx12,
    /// `vulkan`: wgpu restricted to Vulkan (falls back to glow if it cannot start).
    Vulkan,
}

impl Mode {
    /// Parse the value of `ESKIZ_RENDERER` (case-insensitive; unknown values mean `Auto`).
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("cpu") => Self::Cpu,
            Some("gl" | "glow" | "opengl") => Self::Gl,
            Some("gpu" | "wgpu") => Self::Gpu,
            Some("dx12" | "d3d12") => Self::Dx12,
            Some("vulkan" | "vk") => Self::Vulkan,
            _ => Self::Auto,
        }
    }

    /// Parse the value of `--renderer`: `gl`, `dx12`, `vulkan` or `auto`; anything else is an error.
    pub fn parse_flag(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "gl" | "glow" | "opengl" => Ok(Self::Gl),
            "dx12" | "d3d12" => Ok(Self::Dx12),
            "vulkan" | "vk" => Ok(Self::Vulkan),
            other => Err(format!("--renderer takes gl, dx12, vulkan or auto, not '{other}'")),
        }
    }

    /// Which request wins, and where it came from: `--renderer`, then `ESKIZ_RENDERER`, then the
    /// Settings > Graphics choice (Compatibility means OpenGL), else automatic.
    pub fn resolve(cli: Option<Self>, env: Option<&str>, compat_pref: bool) -> (Self, &'static str) {
        if let Some(m) = cli {
            return (m, "--renderer");
        }
        if let Some(v) = env.filter(|v| !v.trim().is_empty()) {
            return (Self::parse(Some(v)), "ESKIZ_RENDERER");
        }
        if compat_pref {
            return (Self::Gl, "Settings > Graphics > Compatibility");
        }
        (Self::Auto, "default")
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
            Self::Gl => "gl",
            Self::Dx12 => "dx12",
            Self::Vulkan => "vulkan",
        }
    }

    /// The mode for this run, resolved from the command line, the environment and the saved choice.
    pub fn from_env() -> Self {
        Self::requested().0
    }

    /// Like [`Mode::from_env`], with the source of the choice.
    pub fn requested() -> (Self, &'static str) {
        Self::resolve(
            CLI_REQUEST.get().copied(),
            crate::compat::env_var("ESKIZ_RENDERER").as_deref(),
            read_compat_pref(),
        )
    }

    /// Should the canvas be composited on the GPU?
    pub fn gpu_compositing(self) -> bool {
        matches!(self, Self::Auto | Self::Gpu | Self::Dx12 | Self::Vulkan)
    }
}

/// What `--renderer` asked for, set once by [`set_cli_request`].
static CLI_REQUEST: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();

/// Remember the `--renderer` flag (the first call wins).
pub fn set_cli_request(mode: Mode) {
    let _ = CLI_REQUEST.set(mode);
}

/// The file Settings > Graphics writes next to the app data, read before the window exists
/// (the preferences themselves live in eframe's storage, which opens later).
const GRAPHICS_FILE: &str = "graphics.pref";

fn graphics_path() -> Option<std::path::PathBuf> {
    crate::storage::app_data_root().map(|d| d.join(GRAPHICS_FILE))
}

/// Does `text` (the contents of the graphics file) choose Compatibility?
pub fn parse_compat_pref(text: &str) -> bool {
    text.trim() == "compat"
}

/// Settings > Graphics: is Compatibility (OpenGL) saved?
pub fn read_compat_pref() -> bool {
    graphics_path().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|t| parse_compat_pref(&t))
}

/// Save the Settings > Graphics choice for the next start.
pub fn write_compat_pref(compat: bool) {
    if let Some(p) = graphics_path() {
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, if compat { "compat\n" } else { "auto\n" });
    }
}

/// What an app creator returns.
pub type AppResult = Result<Box<dyn eframe::App>, Box<dyn Error + Send + Sync>>;

/// Ask for the device limits of the adapter instead of wgpu's defaults, so that canvases larger
/// than 8192 px can still be composited on a GPU that supports them.
fn tune_wgpu(options: &mut NativeOptions) {
    if let WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup {
        create.device_descriptor = Arc::new(|adapter: &wgpu::Adapter| wgpu::DeviceDescriptor {
            label: Some("eskiz device"),
            required_limits: adapter.limits(),
            ..Default::default()
        });
    }
}

// Android is Vulkan only. `eskiz-gpu` needs compute shaders and storage textures, which wgpu's
// GLES backend (GLES 3.0 on most phones) cannot give it, and a glow window with the CPU
// compositor would be far too slow on a phone. Phones that cannot run Vulkan 1.1 are not
// supported, so the fallback is compiled out rather than left half-working.
#[cfg(all(target_os = "android", feature = "gl-fallback"))]
compile_error!("Android builds are Vulkan only: build with --no-default-features --features android");

/// What the graphics start-up error tells the user the platform needs.
fn requirements(android: bool) -> &'static str {
    if android {
        "Eskiz needs a GPU with Vulkan 1.1 (Android 10 or newer on most phones from 2018)."
    } else {
        "It needs Metal (macOS), DX12 or Vulkan (Windows), or Vulkan or OpenGL (Linux). Update the \
         graphics driver; on a build with the `gl-fallback` feature, `ESKIZ_RENDERER=gl` selects OpenGL."
    }
}

fn explain(e: &eframe::Error) -> String {
    format!(
        "{e}\nEskiz could not start its graphics renderer. {}",
        requirements(cfg!(target_os = "android"))
    )
}

/// Set once the app creator has run, that is once the window and its app exist. A panic before
/// this point is a startup failure, which `open_window` reports.
static WINDOW_CREATED: AtomicBool = AtomicBool::new(false);

/// Whether the app has been created in this process.
pub fn window_created() -> bool {
    WINDOW_CREATED.load(Ordering::SeqCst)
}

/// Which renderer attempts to make, in order, for this start.
///
/// * `--renderer gl` / `ESKIZ_RENDERER=gl` / Settings > Graphics > Compatibility: glow only.
/// * `--renderer dx12|vulkan`: that backend, then glow.
/// * Otherwise the first renderer in `wgpu auto -> wgpu-dx12 -> wgpu-vulkan -> glow` that no
///   earlier crash (`crashed`) or stall (`skipped`) ruled out, and glow after it when the
///   `gl-fallback` feature is on. See [`crate::startup::Ladder`] and [`crate::stall`].
///
/// `None` when every renderer already crashed the process: the caller tells the user.
pub fn plan(mode: Mode, crashed: Option<Stage>, skipped: &[Stage], windows: bool, gl_fallback: bool) -> Option<Vec<Stage>> {
    if gl_fallback {
        match mode {
            Mode::Gl => return Some(vec![Stage::Glow]),
            Mode::Dx12 => return Some(vec![Stage::WgpuDx12, Stage::Glow]),
            Mode::Vulkan => return Some(vec![Stage::WgpuVulkan, Stage::Glow]),
            _ => {}
        }
    }
    if !gl_fallback {
        // Without OpenGL there is nothing to step down to: wgpu with every backend, whatever happened.
        return Some(vec![Stage::Wgpu]);
    }
    let first = match Ladder::next(crashed, skipped, windows) {
        Ladder::Try(stage) => stage,
        Ladder::GiveUp => return None,
    };
    let mut stages = vec![first];
    if first != Stage::Glow && gl_fallback {
        stages.push(Stage::Glow);
    }
    Some(stages)
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Open the window. `options` builds fresh [`NativeOptions`] (it is called once per attempt) and
/// `creator` builds the app; it is told the requested [`Mode`], and finds out whether wgpu is in
/// use from `cc.wgpu_render_state`.
///
/// Each attempt runs under `catch_unwind`. An attempt that returns an error or panics before the
/// window has drawn its first frames ([`startup::started_ok`]) is followed by the next renderer
/// in the plan; nothing the user did can be lost by then. Later failures are reported as they are.
pub fn run_native(
    title: &str,
    options: impl Fn() -> NativeOptions,
    creator: impl Fn(&CreationContext<'_>, Mode) -> AppResult,
) -> Result<(), String> {
    let (mode, source) = Mode::requested();
    startup::log(&format!("renderer requested: {} (from {source})", mode.name()));
    if CLI_REQUEST.get() == Some(&Mode::Auto) {
        startup::clear_skips();
    }
    let skips = startup::skips();
    #[cfg(not(feature = "gl-fallback"))]
    if mode == Mode::Gl {
        eprintln!("eskiz: this build has no OpenGL renderer (cargo feature `gl-fallback`); using wgpu");
    }
    let crashed = startup::previous_crash();
    let Some(stages) = plan(mode, crashed, &skips.stages, cfg!(windows), cfg!(feature = "gl-fallback")) else {
        // Every renderer killed the process on an earlier start. Say so once and start over next time.
        let reason = "Eskiz stopped during start-up with every graphics renderer it can use. Update the graphics \
                      driver, or run eskiz.exe --diagnose from a terminal and send the output with the logs.";
        return Err(reason.to_owned());
    };
    let attempt = |stage: Stage| -> Result<(), String> {
        startup::begin_attempt(stage);
        let mut o = options();
        o.renderer = renderer_for(stage);
        if stage != Stage::Glow {
            tune_wgpu(&mut o);
        }
        if let Some(backends) = backends_for(stage) {
            if let WgpuSetup::CreateNew(create) = &mut o.wgpu_options.wgpu_setup {
                create.instance_descriptor.backends = backends;
            }
        }
        if skips.no_vsync {
            // An earlier run froze while presenting: do not wait for the display.
            o.wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
            #[cfg(feature = "gl-fallback")]
            {
                o.glow_options.vsync = false;
            }
        }
        if stage == Stage::Glow {
            #[cfg(feature = "gl-fallback")]
            startup::log(&format!("present mode: {}", if o.glow_options.vsync { "vsync (OpenGL swap interval 1)" } else { "no vsync" }));
        } else {
            startup::log(&format!("present mode requested: {:?}", o.wgpu_options.surface.present_mode));
        }
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            eframe::run_native(
                title,
                o,
                Box::new(|cc| {
                    WINDOW_CREATED.store(true, Ordering::SeqCst);
                    log_renderer(cc, stage);
                    creator(cc, mode)
                }),
            )
        }));
        match run {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(explain(&e)),
            Err(payload) if startup::started_ok() => std::panic::resume_unwind(payload),
            Err(payload) => Err(format!("the {} renderer panicked while starting: {}", stage.name(), panic_text(&*payload))),
        }
    };
    let mut first_error: Option<String> = None;
    for stage in stages {
        match attempt(stage) {
            Ok(()) => return Ok(()),
            // The window ran and drew: this is a runtime failure, not a start-up one.
            Err(e) if startup::started_ok() => return Err(e),
            Err(e) => {
                startup::log(&format!("renderer '{}' failed to start: {e}", stage.name()));
                eprintln!("eskiz: the {} renderer failed to start ({e}); trying the next one", stage.name());
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "no renderer could be started".to_owned()))
}

/// The wgpu backends a stage is limited to; `None` leaves wgpu's own choice.
fn backends_for(stage: Stage) -> Option<wgpu::Backends> {
    match stage {
        Stage::WgpuDx12 => Some(wgpu::Backends::DX12),
        Stage::WgpuVulkan => Some(wgpu::Backends::VULKAN),
        Stage::Wgpu | Stage::Glow => None,
    }
}

#[cfg(feature = "gl-fallback")]
fn renderer_for(stage: Stage) -> eframe::Renderer {
    if stage == Stage::Glow {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    }
}

/// Without the `gl-fallback` feature there is only wgpu (`plan` never returns glow then).
#[cfg(not(feature = "gl-fallback"))]
fn renderer_for(_stage: Stage) -> eframe::Renderer {
    eframe::Renderer::Wgpu
}

/// Log the adapter and backend the window ended up with.
fn log_renderer(cc: &CreationContext<'_>, stage: Stage) {
    match cc.wgpu_render_state.as_ref() {
        Some(state) => {
            let info = state.adapter.get_info();
            startup::log(&format!(
                "renderer: wgpu ({}) adapter='{}' backend={:?} type={:?} driver='{}' {}",
                stage.name(),
                info.name,
                info.backend,
                info.device_type,
                info.driver,
                info.driver_info
            ));
        }
        None => startup::log(&format!("renderer: {} (OpenGL window, CPU compositor)", stage.name())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_error_names_the_platforms_graphics_needs() {
        assert!(requirements(true).contains("Vulkan 1.1"));
        assert!(!requirements(true).contains("OpenGL"), "no GLES fallback is promised on Android");
        assert!(requirements(false).contains("Metal") && requirements(false).contains("OpenGL"));
    }

    #[test]
    fn plan_follows_the_crash_ladder() {
        use Stage::*;
        assert_eq!(plan(Mode::Auto, None, &[], true, true), Some(vec![Wgpu, Glow]));
        assert_eq!(plan(Mode::Auto, None, &[], true, false), Some(vec![Wgpu]));
        assert_eq!(plan(Mode::Auto, Some(Wgpu), &[], true, true), Some(vec![WgpuDx12, Glow]));
        assert_eq!(plan(Mode::Auto, Some(Wgpu), &[], false, true), Some(vec![Glow]));
        assert_eq!(plan(Mode::Auto, Some(WgpuDx12), &[], true, true), Some(vec![WgpuVulkan, Glow]));
        assert_eq!(plan(Mode::Auto, Some(WgpuVulkan), &[], true, true), Some(vec![Glow]));
        assert_eq!(plan(Mode::Auto, Some(Glow), &[], true, true), None);
        assert_eq!(plan(Mode::Gl, None, &[], true, true), Some(vec![Glow]), "ESKIZ_RENDERER=gl wins");
        // No glow compiled in: wgpu is the only choice, whatever crashed.
        assert_eq!(plan(Mode::Auto, Some(WgpuDx12), &[], true, false), Some(vec![Wgpu]));
    }

    #[test]
    fn plan_skips_stalled_renderers_in_order() {
        use Stage::*;
        // wgpu auto -> wgpu-dx12 -> wgpu-vulkan -> glow, one stall at a time.
        assert_eq!(plan(Mode::Auto, None, &[Wgpu], true, true), Some(vec![WgpuDx12, Glow]));
        assert_eq!(plan(Mode::Auto, None, &[Wgpu, WgpuDx12], true, true), Some(vec![WgpuVulkan, Glow]));
        assert_eq!(plan(Mode::Auto, None, &[Wgpu, WgpuDx12, WgpuVulkan], true, true), Some(vec![Glow]));
        // A stalled glow is not given up: nothing else is left.
        assert_eq!(plan(Mode::Auto, None, &[Glow], false, true), Some(vec![Wgpu, Glow]));
        // An explicit request ignores the skip list.
        assert_eq!(plan(Mode::Dx12, None, &[WgpuDx12], true, true), Some(vec![WgpuDx12, Glow]));
        assert_eq!(plan(Mode::Vulkan, None, &[], true, true), Some(vec![WgpuVulkan, Glow]));
    }

    #[test]
    fn flag_parsing_and_precedence() {
        assert_eq!(Mode::parse_flag("gl"), Ok(Mode::Gl));
        assert_eq!(Mode::parse_flag("DX12"), Ok(Mode::Dx12));
        assert_eq!(Mode::parse_flag("vulkan"), Ok(Mode::Vulkan));
        assert_eq!(Mode::parse_flag("auto"), Ok(Mode::Auto));
        assert!(Mode::parse_flag("metal").is_err());
        assert_eq!(Mode::resolve(Some(Mode::Dx12), Some("gl"), true), (Mode::Dx12, "--renderer"));
        assert_eq!(Mode::resolve(None, Some("cpu"), true), (Mode::Cpu, "ESKIZ_RENDERER"));
        assert_eq!(Mode::resolve(None, None, true).0, Mode::Gl);
        assert_eq!(Mode::resolve(None, Some(""), false), (Mode::Auto, "default"));
        assert!(parse_compat_pref("compat\n") && !parse_compat_pref("auto\n"));
    }

    #[test]
    fn parses_the_env_values() {
        assert_eq!(Mode::parse(None), Mode::Auto);
        assert_eq!(Mode::parse(Some("cpu")), Mode::Cpu);
        assert_eq!(Mode::parse(Some(" CPU ")), Mode::Cpu);
        assert_eq!(Mode::parse(Some("gl")), Mode::Gl);
        assert_eq!(Mode::parse(Some("glow")), Mode::Gl);
        assert_eq!(Mode::parse(Some("gpu")), Mode::Gpu);
        assert_eq!(Mode::parse(Some("whatever")), Mode::Auto);
        assert!(Mode::Auto.gpu_compositing() && Mode::Gpu.gpu_compositing());
        assert!(!Mode::Cpu.gpu_compositing() && !Mode::Gl.gpu_compositing());
    }
}
