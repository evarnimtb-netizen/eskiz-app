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
}

impl Mode {
    /// Parse the value of `ESKIZ_RENDERER` (case-insensitive; unknown values mean `Auto`).
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("cpu") => Self::Cpu,
            Some("gl" | "glow" | "opengl") => Self::Gl,
            Some("gpu" | "wgpu") => Self::Gpu,
            _ => Self::Auto,
        }
    }

    pub fn from_env() -> Self {
        Self::parse(crate::compat::env_var("ESKIZ_RENDERER").as_deref())
    }

    /// Should the canvas be composited on the GPU?
    pub fn gpu_compositing(self) -> bool {
        matches!(self, Self::Auto | Self::Gpu)
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
/// * `ESKIZ_RENDERER=gl`: glow only.
/// * A previous start that never drew its first frames (see [`crate::startup`]) moves the first
///   attempt down the ladder: wgpu, wgpu on DX12 only (Windows), glow.
/// * Otherwise wgpu with every backend, and glow after it when the `gl-fallback` feature is on.
///
/// `None` when every renderer already crashed the process: the caller tells the user.
pub fn plan(mode: Mode, crashed: Option<Stage>, windows: bool, gl_fallback: bool) -> Option<Vec<Stage>> {
    if mode == Mode::Gl && gl_fallback {
        return Some(vec![Stage::Glow]);
    }
    let first = match Ladder::next(crashed, windows) {
        Ladder::Try(Stage::Glow) if !gl_fallback => Stage::Wgpu,
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
    let mode = Mode::from_env();
    #[cfg(not(feature = "gl-fallback"))]
    if mode == Mode::Gl {
        eprintln!("eskiz: this build has no OpenGL renderer (cargo feature `gl-fallback`); using wgpu");
    }
    let crashed = startup::previous_crash();
    let Some(stages) = plan(mode, crashed, cfg!(windows), cfg!(feature = "gl-fallback")) else {
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
        if stage == Stage::WgpuDx12 {
            if let WgpuSetup::CreateNew(create) = &mut o.wgpu_options.wgpu_setup {
                create.instance_descriptor.backends = wgpu::Backends::DX12;
            }
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
        assert_eq!(plan(Mode::Auto, None, true, true), Some(vec![Wgpu, Glow]));
        assert_eq!(plan(Mode::Auto, None, true, false), Some(vec![Wgpu]));
        assert_eq!(plan(Mode::Auto, Some(Wgpu), true, true), Some(vec![WgpuDx12, Glow]));
        assert_eq!(plan(Mode::Auto, Some(Wgpu), false, true), Some(vec![Glow]));
        assert_eq!(plan(Mode::Auto, Some(WgpuDx12), true, true), Some(vec![Glow]));
        assert_eq!(plan(Mode::Auto, Some(Glow), true, true), None);
        assert_eq!(plan(Mode::Gl, None, true, true), Some(vec![Glow]), "ESKIZ_RENDERER=gl wins");
        // No glow compiled in: wgpu is the only choice, whatever crashed.
        assert_eq!(plan(Mode::Auto, Some(WgpuDx12), true, false), Some(vec![Wgpu]));
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
