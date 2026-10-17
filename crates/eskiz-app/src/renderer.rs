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

use std::cell::Cell;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui_wgpu::WgpuSetup;
use eframe::{wgpu, CreationContext, NativeOptions};

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

/// Open the window. `options` builds fresh [`NativeOptions`] (it is called once per attempt) and
/// `creator` builds the app; it is told the requested [`Mode`], and finds out whether wgpu is in
/// use from `cc.wgpu_render_state`.
pub fn run_native(
    title: &str,
    options: impl Fn() -> NativeOptions,
    creator: impl Fn(&CreationContext<'_>, Mode) -> AppResult,
) -> Result<(), String> {
    let mode = Mode::from_env();
    let started = Cell::new(false);
    let attempt = |renderer: eframe::Renderer| {
        let mut o = options();
        o.renderer = renderer;
        if renderer == eframe::Renderer::Wgpu {
            tune_wgpu(&mut o);
        }
        eframe::run_native(
            title,
            o,
            Box::new(|cc| {
                started.set(true);
                WINDOW_CREATED.store(true, Ordering::SeqCst);
                creator(cc, mode)
            }),
        )
    };
    #[cfg(feature = "gl-fallback")]
    if mode == Mode::Gl {
        return attempt(eframe::Renderer::Glow).map_err(|e| explain(&e));
    }
    #[cfg(not(feature = "gl-fallback"))]
    if mode == Mode::Gl {
        eprintln!("eskiz: this build has no OpenGL renderer (cargo feature `gl-fallback`); using wgpu");
    }
    match attempt(eframe::Renderer::Wgpu) {
        Ok(()) => Ok(()),
        #[cfg(feature = "gl-fallback")]
        Err(e) if !started.get() => {
            eprintln!("eskiz: the wgpu renderer failed to start ({e}); trying OpenGL");
            attempt(eframe::Renderer::Glow).map_err(|e2| format!("{}\n(wgpu failed first: {e})", explain(&e2)))
        }
        Err(e) => Err(explain(&e)),
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
