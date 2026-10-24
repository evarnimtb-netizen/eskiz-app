//! Which renderer the window uses, and what happens when it cannot start.
//!
//! * **Ladder.** Each OS has a list of renderers, fastest first ([`crate::startup::order`]):
//!   Windows DX12, Vulkan, OpenGL, software (WARP); macOS Metal, OpenGL; Linux Vulkan, wgpu's
//!   OpenGL, OpenGL (glow), software. OpenGL (glow) and software composite the canvas on the CPU;
//!   the wgpu rungs use the GPU compositor of `eskiz-gpu` (which itself falls back to the CPU for a
//!   document it cannot show exactly).
//! * **Safe mode.** A start that crashed or froze (see [`crate::startup`], [`crate::stall`]) makes
//!   later starts begin one rung lower, until the user picks Try normal graphics.
//! * **Requests.** `--renderer`, then `ESKIZ_BACKEND`, then `ESKIZ_RENDERER` (older values,
//!   including `cpu`: wgpu window, CPU compositor), then `WGPU_BACKEND` (wgpu picks the backend
//!   and adapter itself), then Settings > Graphics > Renderer. A request wins over the markers,
//!   the skip list and the driver blocklist.
//! * **Fallback.** Each attempt runs under `catch_unwind`. One that fails before the window has
//!   drawn its first frames is followed by the next rung, so it can never throw away a document.
//!   If every rung fails, `run_native` returns an error that says what to try.
//! * **Adapter choice and logs.** On the wgpu rungs Eskiz picks the adapter itself (eframe's
//!   `native_adapter_selector`): every adapter is logged with vendor, driver and backend, the
//!   blocklist ([`crate::gpu_blocklist`]) is applied, and the software rung takes a CPU adapter.

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use eframe::egui_wgpu::WgpuSetup;
use eframe::{wgpu, CreationContext, NativeOptions};

use crate::gpu_blocklist::{self, Adapter};
use crate::stall;
use crate::startup::{self, Ladder, Stage};

/// What the user asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Nothing: the ladder, with every fallback.
    Auto,
    /// `ESKIZ_RENDERER=gpu`: the same as `Auto`, spelled out.
    Gpu,
    /// `ESKIZ_RENDERER=cpu`: the ladder, with the CPU compositor.
    Cpu,
    /// OpenGL (glow) window, CPU compositor.
    Gl,
    /// wgpu on DirectX 12.
    Dx12,
    /// wgpu on Vulkan.
    Vulkan,
    /// wgpu on Metal.
    Metal,
    /// wgpu on its OpenGL backend.
    WgpuGl,
    /// wgpu on a software adapter, CPU compositor.
    Software,
    /// `WGPU_BACKEND` is set: wgpu picks the backend and adapter.
    Wgpu,
}

impl Mode {
    /// Parse the value of `ESKIZ_RENDERER`: the older `cpu` and `gpu`, or anything [`Mode::parse_flag`]
    /// takes (case-insensitive; unknown values mean `Auto`).
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("cpu") => Self::Cpu,
            Some("gpu" | "wgpu") => Self::Gpu,
            Some(v) => Self::parse_flag(v).unwrap_or(Self::Auto),
            None => Self::Auto,
        }
    }

    /// Parse `--renderer`, `ESKIZ_BACKEND` and the saved Settings choice. Anything else is an error.
    pub fn parse_flag(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatic" => Ok(Self::Auto),
            "gl" | "glow" | "opengl" | "compat" => Ok(Self::Gl),
            "dx12" | "d3d12" => Ok(Self::Dx12),
            "vulkan" | "vk" => Ok(Self::Vulkan),
            "metal" | "mtl" => Ok(Self::Metal),
            "wgpu-gl" | "gles" => Ok(Self::WgpuGl),
            "software" | "warp" | "llvmpipe" | "lavapipe" => Ok(Self::Software),
            other => Err(format!(
                "--renderer and ESKIZ_BACKEND take auto, dx12, vulkan, metal, wgpu-gl, gl or software, not '{other}'"
            )),
        }
    }

    /// Which request wins, and where it came from.
    pub fn resolve(r: &Requests<'_>) -> (Self, &'static str) {
        let set = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned);
        if let Some(m) = r.cli {
            return (m, "--renderer");
        }
        if let Some(m) = set(r.backend_env).and_then(|v| Self::parse_flag(&v).ok()) {
            return (m, "ESKIZ_BACKEND");
        }
        if let Some(v) = set(r.renderer_env) {
            return (Self::parse(Some(&v)), "ESKIZ_RENDERER");
        }
        if set(r.wgpu_env).is_some() {
            return (Self::Wgpu, "WGPU_BACKEND");
        }
        if r.pref != Self::Auto {
            return (r.pref, "Settings > Graphics");
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
            Self::Metal => "metal",
            Self::WgpuGl => "wgpu-gl",
            Self::Software => "software",
            Self::Wgpu => "wgpu",
        }
    }

    /// The rung a request names; `None` for the ladder.
    pub fn stage(self) -> Option<Stage> {
        match self {
            Self::Auto | Self::Gpu | Self::Cpu => None,
            Self::Gl => Some(Stage::Glow),
            Self::Dx12 => Some(Stage::WgpuDx12),
            Self::Vulkan => Some(Stage::WgpuVulkan),
            Self::Metal => Some(Stage::WgpuMetal),
            Self::WgpuGl => Some(Stage::WgpuGl),
            Self::Software => Some(Stage::WgpuSoftware),
            Self::Wgpu => Some(Stage::Wgpu),
        }
    }

    /// The Settings choice that picks `stage`.
    pub fn for_stage(stage: Stage) -> Self {
        match stage {
            Stage::Wgpu => Self::Auto,
            Stage::WgpuDx12 => Self::Dx12,
            Stage::WgpuVulkan => Self::Vulkan,
            Stage::WgpuMetal => Self::Metal,
            Stage::WgpuGl => Self::WgpuGl,
            Stage::Glow => Self::Gl,
            Stage::WgpuSoftware => Self::Software,
        }
    }

    /// The mode for this run, resolved from the command line, the environment and the saved choice.
    pub fn from_env() -> Self {
        Self::requested().0
    }

    /// Like [`Mode::from_env`], with the source of the choice.
    pub fn requested() -> (Self, &'static str) {
        let backend = crate::compat::env_var("ESKIZ_BACKEND");
        let renderer = crate::compat::env_var("ESKIZ_RENDERER");
        let wgpu_env = std::env::var("WGPU_BACKEND").ok();
        Self::resolve(&Requests {
            cli: CLI_REQUEST.get().copied(),
            backend_env: backend.as_deref(),
            renderer_env: renderer.as_deref(),
            wgpu_env: wgpu_env.as_deref(),
            pref: read_graphics_pref(),
        })
    }

    /// Should the canvas be composited on the GPU?
    pub fn gpu_compositing(self) -> bool {
        !matches!(self, Self::Cpu | Self::Gl | Self::Software)
    }
}

/// Everything that can ask for a renderer, highest precedence first.
#[derive(Clone, Copy, Debug)]
pub struct Requests<'a> {
    pub cli: Option<Mode>,
    pub backend_env: Option<&'a str>,
    pub renderer_env: Option<&'a str>,
    pub wgpu_env: Option<&'a str>,
    pub pref: Mode,
}

impl Default for Requests<'_> {
    fn default() -> Self {
        Self { cli: None, backend_env: None, renderer_env: None, wgpu_env: None, pref: Mode::Auto }
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
    // Tests click through Settings > Graphics: they must not change the real saved choice.
    if cfg!(test) {
        return None;
    }
    crate::storage::app_data_root().map(|d| d.join(GRAPHICS_FILE))
}

/// The Settings choice in `text` (the contents of the graphics file). Older builds wrote
/// `compat` for OpenGL; anything unreadable is Automatic.
pub fn parse_graphics_pref(text: &str) -> Mode {
    match Mode::parse_flag(text) {
        Ok(Mode::Cpu | Mode::Gpu | Mode::Wgpu) | Err(_) => Mode::Auto,
        Ok(m) => m,
    }
}

/// Settings > Graphics > Renderer, as saved.
pub fn read_graphics_pref() -> Mode {
    graphics_path().and_then(|p| std::fs::read_to_string(p).ok()).map_or(Mode::Auto, |t| parse_graphics_pref(&t))
}

/// Save the Settings > Graphics choice for the next start (only when it changed).
pub fn write_graphics_pref(mode: Mode) {
    if let Some(p) = graphics_path() {
        let text = format!("{}\n", mode.name());
        if std::fs::read_to_string(&p).is_ok_and(|t| t == text) {
            return;
        }
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Whole or not at all: a half-written choice would be read as no choice.
        if let Err(e) = eskiz_format::fsutil::write_atomic(&p, text.as_bytes()) {
            crate::startup::log(&format!("renderer: could not save the choice: {e}"));
        }
    }
}

/// The renderers Settings > Graphics offers on this OS: Automatic, then each rung of the ladder.
pub fn choices() -> Vec<Mode> {
    std::iter::once(Mode::Auto)
        .chain(startup::this_order().into_iter().map(Mode::for_stage).filter(|m| *m != Mode::Auto))
        .collect()
}

/// What to do about a frame the swap chain did not give a texture for: egui-wgpu's own choices,
/// plus a count of the frames lost in a row (an acquire that times out blocks the UI thread for
/// about a second each time; [`stall::take_surface_trouble`] ends the GPU compositor after a run).
fn surface_action(status: &wgpu::CurrentSurfaceTexture) -> eframe::egui_wgpu::SurfaceErrorAction {
    use eframe::egui_wgpu::SurfaceErrorAction as Act;
    use wgpu::CurrentSurfaceTexture as S;
    let (action, counts) = match status {
        S::Outdated => (Act::Reconfigure, false),
        S::Lost => (Act::RecreateSurface, true),
        S::Occluded => (Act::SkipFrame, false),
        _ => (Act::SkipFrame, true),
    };
    if counts {
        let n = stall::surface_problem();
        if n == 1 || n.is_multiple_of(5) {
            startup::log(&format!("surface: frame skipped ({status:?}), {n} in a row"));
        }
        stall::crumb(stall::Phase::Idle);
    }
    action
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
         graphics driver; `--renderer gl` (or `ESKIZ_BACKEND=gl`) selects OpenGL and `--renderer software` a \
         software renderer."
    }
}

fn explain(e: &eframe::Error) -> String {
    format!(
        "{e}\nEskiz could not start its graphics renderer. {}",
        requirements(cfg!(target_os = "android"))
    )
}

/// Set when the saved Settings choice crashed the last start and was reset to Automatic.
static SAVED_CHOICE_RESET: AtomicBool = AtomicBool::new(false);

/// Whether this start reset the saved renderer choice (the app then shows Automatic too, so
/// it does not save the old choice again).
pub fn saved_choice_was_reset() -> bool {
    SAVED_CHOICE_RESET.load(Ordering::SeqCst)
}

/// Set once the app creator has run, that is once the window and its app exist. A panic before
/// this point is a startup failure, which `open_window` reports.
static WINDOW_CREATED: AtomicBool = AtomicBool::new(false);

/// Whether the app has been created in this process.
pub fn window_created() -> bool {
    WINDOW_CREATED.load(Ordering::SeqCst)
}

/// Which renderers to try, in order, for this start, and whether that is safe mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub stages: Vec<Stage>,
    /// The ladder starts below its top rung because of earlier problems.
    pub safe: bool,
}

/// The attempts for this start. `crashed` is the rung a previous start never finished, `skipped`
/// the rungs earlier crashes and stalls ruled out, `order` the ladder ([`startup::order`]).
///
/// * A request (`mode.stage()`): that renderer, then the CPU-compositor rungs of the ladder
///   (OpenGL, software) in case it cannot start. Markers and skips do not change it.
/// * Otherwise the rung [`Ladder::next`] picks and the rungs below it that are not skipped
///   (the last rung always stays). `safe` when that is not the top rung.
///
/// `None` when the last rung crashed the process: the caller tells the user.
pub fn plan(mode: Mode, crashed: Option<Stage>, skipped: &[Stage], order: &[Stage]) -> Option<Plan> {
    if let Some(stage) = mode.stage() {
        let mut stages = vec![stage];
        stages.extend(order.iter().copied().filter(|s| s.cpu_compositor() && *s != stage));
        return Some(Plan { stages, safe: false });
    }
    let first = match Ladder::next(crashed, skipped, order) {
        Ladder::Try(stage) => stage,
        Ladder::GiveUp => return None,
    };
    let at = order.iter().position(|s| *s == first).unwrap_or(0);
    let last = order.last().copied();
    let stages: Vec<Stage> = order[at..]
        .iter()
        .copied()
        .filter(|s| *s == first || Some(*s) == last || !(skipped.contains(s) || crashed == Some(*s)))
        .collect();
    Some(Plan { safe: order.first() != Some(&first), stages })
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Did the renderer chosen in Settings > Graphics crash the last start or freeze the last run? Then
/// the choice goes back to Automatic. A command-line or environment request is kept.
fn saved_choice_failed(source: &str, mode: Mode, crashed: Option<Stage>, skipped: &[Stage]) -> bool {
    let Some(stage) = mode.stage() else { return false };
    source == "Settings > Graphics" && (crashed == Some(stage) || skipped.contains(&stage))
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
    let (mut mode, source) = Mode::requested();
    startup::log(&format!("renderer requested: {} (from {source})", mode.name()));
    if let Some(bad) = crate::compat::env_var("ESKIZ_BACKEND").filter(|v| !v.trim().is_empty()) {
        if let Err(e) = Mode::parse_flag(&bad) {
            startup::log(&format!("ESKIZ_BACKEND ignored: {e}"));
        }
    }
    if CLI_REQUEST.get() == Some(&Mode::Auto) {
        startup::clear_skips();
    }
    let crashed = startup::previous_crash();
    // A saved choice that crashed the last start goes back to Automatic: the user may not be
    // able to reach Settings to change it. (A command-line or environment request is kept.)
    let skips = startup::skips();
    if saved_choice_failed(source, mode, crashed, &skips.stages) {
        startup::log(&format!("the saved renderer '{}' crashed or froze the last run; Settings > Graphics is back to Automatic", mode.name()));
        mode = Mode::Auto;
        write_graphics_pref(Mode::Auto);
        SAVED_CHOICE_RESET.store(true, Ordering::SeqCst);
    }
    let order = startup::this_order();
    #[cfg(not(feature = "gl-fallback"))]
    if mode == Mode::Gl {
        eprintln!("eskiz: this build has no OpenGL renderer (cargo feature `gl-fallback`); using wgpu");
        mode = Mode::Auto;
    }
    let Some(plan) = plan(mode, crashed, &skips.stages, &order) else {
        // The last rung killed the process too. Say so once and start over next time.
        startup::clear_skips();
        let reason = "Eskiz stopped during start-up with every graphics renderer it can use. Update the graphics \
                      driver, or run eskiz.exe --diagnose from a terminal and send the output with the logs.";
        return Err(reason.to_owned());
    };
    let names: Vec<&str> = plan.stages.iter().map(|s| s.name()).collect();
    startup::log(&format!("renderer plan: {}", names.join(" -> ")));
    // The blocklist is for the automatic ladder; a renderer the user picked is used as asked.
    let use_blocklist = mode.stage().is_none();
    let attempt = |stage: Stage| -> Result<(), String> {
        startup::begin_attempt(stage);
        let mut o = options();
        o.renderer = renderer_for(stage);
        if stage != Stage::Glow {
            tune_wgpu(&mut o);
        }
        if let WgpuSetup::CreateNew(create) = &mut o.wgpu_options.wgpu_setup {
            if let Some(backends) = backends_for(stage) {
                create.instance_descriptor.backends = backends;
                let low_power = wgpu::PowerPreference::from_env() == Some(wgpu::PowerPreference::LowPower);
                create.native_adapter_selector = Some(Arc::new(move |adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>| {
                    select_adapter(adapters, surface, stage, use_blocklist, low_power)
                }));
            }
        }
        o.wgpu_options.on_surface_status = Arc::new(surface_action);
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
        // The software rung composites on the CPU, like OpenGL; so does a renderer whose last
        // freeze happened with the GPU compositor sharing its device (unless the user chose a
        // compositor explicitly on the command line or in Settings).
        let learned_cpu = skips.cpu_compositor && mode == Mode::Auto;
        if learned_cpu && stage != Stage::Glow {
            startup::log("compositor: CPU (the last run froze with the GPU compositor)");
        }
        let app_mode = if (stage.cpu_compositor() || learned_cpu) && mode.gpu_compositing() { Mode::Cpu } else { mode };
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            eframe::run_native(
                title,
                o,
                Box::new(|cc| {
                    WINDOW_CREATED.store(true, Ordering::SeqCst);
                    log_renderer(cc, stage);
                    if plan.safe {
                        startup::enter_safe_mode(stage);
                    }
                    creator(cc, app_mode)
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
    for &stage in &plan.stages {
        match attempt(stage) {
            Ok(()) => return Ok(()),
            // The window ran and drew: this is a runtime failure, not a start-up one.
            Err(e) if startup::started_ok() => return Err(e),
            Err(e) => {
                startup::attempt_failed();
                startup::log(&format!("renderer '{}' failed to start: {e}", stage.name()));
                eprintln!("eskiz: the {} renderer failed to start ({e}); trying the next one", stage.name());
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "no renderer could be started".to_owned()))
}

/// The wgpu backends a stage is limited to; `None` leaves wgpu's own choice (and adapter
/// selection) alone, as `WGPU_BACKEND` and Android expect.
fn backends_for(stage: Stage) -> Option<wgpu::Backends> {
    match stage {
        Stage::WgpuDx12 => Some(wgpu::Backends::DX12),
        Stage::WgpuVulkan => Some(wgpu::Backends::VULKAN),
        Stage::WgpuMetal => Some(wgpu::Backends::METAL),
        Stage::WgpuGl => Some(wgpu::Backends::GL),
        Stage::WgpuSoftware => Some(wgpu::Backends::DX12 | wgpu::Backends::VULKAN | wgpu::Backends::GL),
        Stage::Wgpu | Stage::Glow => None,
    }
}

/// An adapter wgpu found, as the selector sees it.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub adapter: Adapter,
    /// Can it present to the window's surface?
    pub surface_ok: bool,
}

/// Lower is better: the order adapters are preferred in on a hardware rung.
fn type_rank(t: wgpu::DeviceType, low_power: bool) -> u8 {
    match (t, low_power) {
        (wgpu::DeviceType::DiscreteGpu, false) | (wgpu::DeviceType::IntegratedGpu, true) => 0,
        (wgpu::DeviceType::IntegratedGpu, false) | (wgpu::DeviceType::DiscreteGpu, true) => 1,
        (wgpu::DeviceType::VirtualGpu, _) => 2,
        (wgpu::DeviceType::Other, _) => 3,
        (wgpu::DeviceType::Cpu, _) => 4,
    }
}

/// Pick the adapter for `stage` from `cands`, or say why none fits. The software rung takes only
/// CPU adapters; the others prefer a discrete GPU (an integrated one with `low_power`, as
/// `WGPU_POWER_PREF=low` asks), and take a CPU adapter only when nothing else can present.
/// Adapters the `rules` block are left out when `use_blocklist`. Each verdict is passed to `note`.
pub fn choose_adapter(
    cands: &[Candidate],
    stage: Stage,
    rules: &[gpu_blocklist::Rule],
    os: &str,
    use_blocklist: bool,
    low_power: bool,
    mut note: impl FnMut(usize, &str),
) -> Result<usize, String> {
    let mut best: Option<(u8, usize)> = None;
    for (i, c) in cands.iter().enumerate() {
        let verdict = if !c.surface_ok {
            "cannot present to the window".to_owned()
        } else if stage == Stage::WgpuSoftware && c.adapter.device_type != wgpu::DeviceType::Cpu {
            "not a software adapter".to_owned()
        } else if let Some(rule) = gpu_blocklist::blocked(rules, &c.adapter, os).filter(|_| use_blocklist) {
            format!("blocked: {}", rule.reason)
        } else {
            let rank = type_rank(c.adapter.device_type, low_power);
            if best.is_none_or(|(r, _)| rank < r) {
                best = Some((rank, i));
            }
            "usable".to_owned()
        };
        note(i, &verdict);
    }
    best.map(|(_, i)| i).ok_or_else(|| {
        if cands.is_empty() {
            format!("no {} adapter was found", stage.name())
        } else {
            format!("none of the {} {} adapters can be used (see startup.log)", cands.len(), stage.name())
        }
    })
}

/// eframe's adapter hook: log every adapter, then [`choose_adapter`].
fn select_adapter(
    adapters: &[wgpu::Adapter],
    surface: Option<&wgpu::Surface<'_>>,
    stage: Stage,
    use_blocklist: bool,
    low_power: bool,
) -> Result<wgpu::Adapter, String> {
    let cands: Vec<Candidate> = adapters
        .iter()
        .map(|a| Candidate { adapter: Adapter::from_info(&a.get_info()), surface_ok: surface.is_none_or(|s| a.is_surface_supported(s)) })
        .collect();
    startup::log(&format!("{} adapters for {}:", cands.len(), stage.name()));
    let chosen = choose_adapter(&cands, stage, gpu_blocklist::RULES, std::env::consts::OS, use_blocklist, low_power, |i, verdict| {
        startup::log(&format!("  adapter {i}: {} -> {verdict}", cands[i].adapter.describe()));
    })?;
    startup::log(&format!("  chosen: adapter {chosen}"));
    Ok(adapters[chosen].clone())
}

#[cfg(feature = "gl-fallback")]
fn renderer_for(stage: Stage) -> eframe::Renderer {
    if stage == Stage::Glow {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    }
}

/// Without the `gl-fallback` feature there is only wgpu (the ladder has no glow rung then).
#[cfg(not(feature = "gl-fallback"))]
fn renderer_for(_stage: Stage) -> eframe::Renderer {
    eframe::Renderer::Wgpu
}

/// Log the adapter, vendor, driver and backend the window ended up with.
fn log_renderer(cc: &CreationContext<'_>, stage: Stage) {
    if let Some(state) = cc.wgpu_render_state.as_ref() {
        if stage == Stage::Wgpu {
            // wgpu chose by itself: list what it could choose from.
            for (i, a) in state.available_adapters.iter().enumerate() {
                startup::log(&format!("  adapter {i}: {}", Adapter::from_info(&a.get_info()).describe()));
            }
        }
        let info = state.adapter.get_info();
        startup::log(&format!("renderer: wgpu ({}) adapter={}", stage.name(), Adapter::from_info(&info).describe()));
        return;
    }
    #[cfg(feature = "gl-fallback")]
    if let Some(gl) = cc.gl.as_ref() {
        startup::log(&format!("renderer: {} (OpenGL window, CPU compositor) {}", stage.name(), gl_strings(gl)));
        return;
    }
    startup::log(&format!("renderer: {} (OpenGL window, CPU compositor)", stage.name()));
}

/// `GL_VENDOR`, `GL_RENDERER` and `GL_VERSION`, for the log.
#[cfg(feature = "gl-fallback")]
#[allow(unsafe_code)]
fn gl_strings(gl: &eframe::glow::Context) -> String {
    use eframe::glow::HasContext as _;
    // SAFETY: called on the thread that owns the current GL context (eframe's app creator), with
    // valid enum names; glGetString has no other preconditions.
    let get = |name| unsafe { gl.get_parameter_string(name) };
    format!(
        "vendor='{}' renderer='{}' version='{}'",
        get(eframe::glow::VENDOR),
        get(eframe::glow::RENDERER),
        get(eframe::glow::VERSION)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::{order, Platform};

    #[test]
    fn startup_error_names_the_platforms_graphics_needs() {
        assert!(requirements(true).contains("Vulkan 1.1"));
        assert!(!requirements(true).contains("OpenGL"), "no GLES fallback is promised on Android");
        assert!(requirements(false).contains("Metal") && requirements(false).contains("OpenGL"));
    }

    #[test]
    fn a_saved_renderer_that_froze_is_reset_but_a_command_line_one_is_kept() {
        let dx12 = Mode::Dx12;
        assert!(saved_choice_failed("Settings > Graphics", dx12, None, &[Stage::WgpuDx12]));
        assert!(saved_choice_failed("Settings > Graphics", dx12, Some(Stage::WgpuDx12), &[]));
        assert!(!saved_choice_failed("Settings > Graphics", dx12, None, &[Stage::Glow]));
        assert!(!saved_choice_failed("--renderer", dx12, None, &[Stage::WgpuDx12]));
        assert!(!saved_choice_failed("Settings > Graphics", Mode::Auto, None, &[Stage::WgpuDx12]));
    }

    fn stages(mode: Mode, crashed: Option<Stage>, skipped: &[Stage], platform: Platform) -> Option<Vec<Stage>> {
        plan(mode, crashed, skipped, &order(platform, true)).map(|p| p.stages)
    }

    #[test]
    fn plan_walks_each_rung_of_the_windows_ladder() {
        use Stage::*;
        let win = Platform::Windows;
        assert_eq!(stages(Mode::Auto, None, &[], win), Some(vec![WgpuDx12, WgpuVulkan, Glow, WgpuSoftware]));
        assert_eq!(stages(Mode::Auto, Some(WgpuDx12), &[], win), Some(vec![WgpuVulkan, Glow, WgpuSoftware]));
        assert_eq!(stages(Mode::Auto, Some(WgpuVulkan), &[WgpuDx12], win), Some(vec![Glow, WgpuSoftware]));
        assert_eq!(stages(Mode::Auto, Some(Glow), &[WgpuDx12, WgpuVulkan], win), Some(vec![WgpuSoftware]));
        assert_eq!(stages(Mode::Auto, Some(WgpuSoftware), &[WgpuDx12, WgpuVulkan, Glow], win), None, "the last rung crashed");
        // Skipped rungs below the start are not retried in the same run either.
        assert_eq!(stages(Mode::Auto, None, &[WgpuVulkan], win), Some(vec![WgpuDx12, Glow, WgpuSoftware]));
        // Everything ruled out by freezes: the last rung is still tried.
        assert_eq!(stages(Mode::Auto, None, &[WgpuDx12, WgpuVulkan, Glow, WgpuSoftware], win), Some(vec![WgpuSoftware]));
        // Older marker values (the old automatic rung) are not on the ladder: start at the top.
        assert_eq!(stages(Mode::Auto, Some(Wgpu), &[Wgpu], win), Some(vec![WgpuDx12, WgpuVulkan, Glow, WgpuSoftware]));
    }

    #[test]
    fn plan_on_macos_linux_and_android() {
        use Stage::*;
        assert_eq!(stages(Mode::Auto, None, &[], Platform::Mac), Some(vec![WgpuMetal, Glow]));
        assert_eq!(stages(Mode::Auto, Some(WgpuMetal), &[], Platform::Mac), Some(vec![Glow]));
        assert_eq!(stages(Mode::Auto, Some(Glow), &[WgpuMetal], Platform::Mac), None);
        assert_eq!(stages(Mode::Auto, None, &[], Platform::Linux), Some(vec![WgpuVulkan, WgpuGl, Glow, WgpuSoftware]));
        assert_eq!(stages(Mode::Auto, Some(WgpuVulkan), &[], Platform::Linux), Some(vec![WgpuGl, Glow, WgpuSoftware]));
        // Android has one rung: a crash cannot step anywhere, and never gives up.
        assert_eq!(stages(Mode::Auto, Some(Wgpu), &[], Platform::Android), Some(vec![Wgpu]));
        // No OpenGL compiled in: the ladder has no glow rung.
        assert_eq!(plan(Mode::Auto, Some(WgpuVulkan), &[WgpuDx12], &order(Platform::Windows, false)).unwrap().stages, vec![WgpuSoftware]);
        assert_eq!(order(Platform::Mac, false), vec![WgpuMetal]);
    }

    #[test]
    fn plan_reports_safe_mode_only_below_the_top_rung() {
        let win = order(Platform::Windows, true);
        assert!(!plan(Mode::Auto, None, &[], &win).unwrap().safe);
        assert!(plan(Mode::Auto, Some(Stage::WgpuDx12), &[], &win).unwrap().safe);
        assert!(plan(Mode::Auto, None, &[Stage::WgpuDx12], &win).unwrap().safe);
        assert!(!plan(Mode::Gl, Some(Stage::WgpuDx12), &[Stage::WgpuDx12], &win).unwrap().safe, "a request is not safe mode");
    }

    #[test]
    fn a_user_request_wins_over_markers_and_skips() {
        use Stage::*;
        let win = Platform::Windows;
        assert_eq!(stages(Mode::Dx12, Some(WgpuDx12), &[WgpuDx12], win), Some(vec![WgpuDx12, Glow, WgpuSoftware]));
        assert_eq!(stages(Mode::Vulkan, None, &[], win), Some(vec![WgpuVulkan, Glow, WgpuSoftware]));
        assert_eq!(stages(Mode::Gl, Some(Glow), &[], win), Some(vec![Glow, WgpuSoftware]));
        assert_eq!(stages(Mode::Software, Some(WgpuSoftware), &[], win), Some(vec![WgpuSoftware, Glow]));
        assert_eq!(stages(Mode::Metal, None, &[], Platform::Mac), Some(vec![WgpuMetal, Glow]));
        // WGPU_BACKEND: wgpu's own choice, then the safe rungs.
        assert_eq!(stages(Mode::Wgpu, None, &[], Platform::Linux), Some(vec![Wgpu, Glow, WgpuSoftware]));
        // ESKIZ_RENDERER=cpu keeps the ladder (with the CPU compositor).
        assert_eq!(stages(Mode::Cpu, Some(WgpuDx12), &[], win), Some(vec![WgpuVulkan, Glow, WgpuSoftware]));
    }

    #[test]
    fn flag_parsing() {
        assert_eq!(Mode::parse_flag("gl"), Ok(Mode::Gl));
        assert_eq!(Mode::parse_flag("DX12"), Ok(Mode::Dx12));
        assert_eq!(Mode::parse_flag("vulkan"), Ok(Mode::Vulkan));
        assert_eq!(Mode::parse_flag("metal"), Ok(Mode::Metal));
        assert_eq!(Mode::parse_flag("wgpu-gl"), Ok(Mode::WgpuGl));
        assert_eq!(Mode::parse_flag(" Software "), Ok(Mode::Software));
        assert_eq!(Mode::parse_flag("warp"), Ok(Mode::Software));
        assert_eq!(Mode::parse_flag("auto"), Ok(Mode::Auto));
        assert!(Mode::parse_flag("directx9").is_err());
        assert!(Mode::parse_flag("cpu").is_err(), "cpu is a compositor, not a backend");
        for m in choices() {
            assert_eq!(Mode::parse_flag(m.name()), Ok(m), "every Settings choice round-trips");
        }
    }

    #[test]
    fn precedence_cli_then_eskiz_backend_then_eskiz_renderer_then_wgpu_backend_then_settings() {
        let all = Requests {
            cli: Some(Mode::Dx12),
            backend_env: Some("vulkan"),
            renderer_env: Some("gl"),
            wgpu_env: Some("metal"),
            pref: Mode::Software,
        };
        assert_eq!(Mode::resolve(&all), (Mode::Dx12, "--renderer"));
        assert_eq!(Mode::resolve(&Requests { cli: None, ..all }), (Mode::Vulkan, "ESKIZ_BACKEND"));
        let r = Requests { cli: None, backend_env: None, ..all };
        assert_eq!(Mode::resolve(&r), (Mode::Gl, "ESKIZ_RENDERER"));
        let r = Requests { renderer_env: None, ..r };
        assert_eq!(Mode::resolve(&r), (Mode::Wgpu, "WGPU_BACKEND"));
        let r = Requests { wgpu_env: Some(" "), ..r };
        assert_eq!(Mode::resolve(&r), (Mode::Software, "Settings > Graphics"));
        assert_eq!(Mode::resolve(&Requests::default()), (Mode::Auto, "default"));
        // A bad ESKIZ_BACKEND is ignored (and logged), not fatal.
        let r = Requests { backend_env: Some("bogus"), ..Requests::default() };
        assert_eq!(Mode::resolve(&r), (Mode::Auto, "default"));
        // ESKIZ_RENDERER keeps its old values.
        assert_eq!(Mode::resolve(&Requests { renderer_env: Some("cpu"), ..Requests::default() }).0, Mode::Cpu);
        assert_eq!(Mode::resolve(&Requests { renderer_env: Some("whatever"), ..Requests::default() }).0, Mode::Auto);
    }

    #[test]
    fn graphics_pref_reads_old_and_new_values() {
        assert_eq!(parse_graphics_pref("compat\n"), Mode::Gl, "older builds wrote compat");
        assert_eq!(parse_graphics_pref("auto\n"), Mode::Auto);
        assert_eq!(parse_graphics_pref("dx12\n"), Mode::Dx12);
        assert_eq!(parse_graphics_pref("software"), Mode::Software);
        assert_eq!(parse_graphics_pref("junk"), Mode::Auto);
        assert_eq!(parse_graphics_pref(""), Mode::Auto);
    }

    #[test]
    fn parses_the_env_values() {
        assert_eq!(Mode::parse(None), Mode::Auto);
        assert_eq!(Mode::parse(Some("cpu")), Mode::Cpu);
        assert_eq!(Mode::parse(Some(" CPU ")), Mode::Cpu);
        assert_eq!(Mode::parse(Some("gl")), Mode::Gl);
        assert_eq!(Mode::parse(Some("glow")), Mode::Gl);
        assert_eq!(Mode::parse(Some("gpu")), Mode::Gpu);
        assert_eq!(Mode::parse(Some("software")), Mode::Software);
        assert_eq!(Mode::parse(Some("whatever")), Mode::Auto);
        assert!(Mode::Auto.gpu_compositing() && Mode::Gpu.gpu_compositing() && Mode::Wgpu.gpu_compositing());
        assert!(!Mode::Cpu.gpu_compositing() && !Mode::Gl.gpu_compositing() && !Mode::Software.gpu_compositing());
    }

    fn cand(name: &str, t: wgpu::DeviceType, vendor: u32, surface_ok: bool) -> Candidate {
        Candidate {
            adapter: Adapter {
                name: name.into(),
                vendor,
                device: 1,
                device_type: t,
                backend: wgpu::Backend::Dx12,
                driver: "31.0.101.2130".into(),
                driver_info: String::new(),
            },
            surface_ok,
        }
    }

    fn pick(cands: &[Candidate], stage: Stage, rules: &[gpu_blocklist::Rule], blocklist: bool, low: bool) -> Result<usize, String> {
        choose_adapter(cands, stage, rules, "windows", blocklist, low, |_, _| {})
    }

    #[test]
    fn selector_prefers_hardware_and_the_software_rung_takes_a_cpu_adapter() {
        use wgpu::DeviceType::*;
        let laptop = [
            cand("Intel(R) UHD Graphics", IntegratedGpu, gpu_blocklist::INTEL, true),
            cand("NVIDIA GeForce", DiscreteGpu, gpu_blocklist::NVIDIA, true),
            cand("Microsoft Basic Render Driver", Cpu, 0x1414, true),
        ];
        assert_eq!(pick(&laptop, Stage::WgpuDx12, &[], true, false), Ok(1), "discrete first");
        assert_eq!(pick(&laptop, Stage::WgpuDx12, &[], true, true), Ok(0), "WGPU_POWER_PREF=low: integrated first");
        assert_eq!(pick(&laptop, Stage::WgpuSoftware, &[], true, false), Ok(2), "the software rung takes WARP");
        assert!(pick(&laptop[..2], Stage::WgpuSoftware, &[], true, false).is_err(), "no software adapter: the rung fails cleanly");
        // A VM with only WARP still gets a window on a hardware rung.
        assert_eq!(pick(&laptop[2..], Stage::WgpuDx12, &[], true, false), Ok(0));
        // An adapter that cannot present is never chosen.
        let headless = [cand("NVIDIA GeForce", DiscreteGpu, gpu_blocklist::NVIDIA, false), laptop[0].clone()];
        assert_eq!(pick(&headless, Stage::WgpuDx12, &[], true, false), Ok(1));
        assert!(pick(&[], Stage::WgpuVulkan, &[], true, false).unwrap_err().contains("no wgpu-vulkan adapter"));
    }

    #[test]
    fn selector_applies_the_blocklist_unless_the_user_chose_the_renderer() {
        use wgpu::DeviceType::*;
        let rule = gpu_blocklist::Rule {
            vendor: gpu_blocklist::INTEL,
            os: Some("windows"),
            backend: Some(wgpu::Backend::Dx12),
            name_contains: None,
            driver_from: None,
            driver_below: Some("31.0.101.4000"),
            reason: "test freeze",
        };
        let intel_only = [cand("Intel(R) UHD Graphics", IntegratedGpu, gpu_blocklist::INTEL, true)];
        let mut notes = Vec::new();
        let r = choose_adapter(&intel_only, Stage::WgpuDx12, &[rule], "windows", true, false, |i, v| notes.push((i, v.to_owned())));
        assert!(r.is_err(), "the rung fails, so the next rung is tried");
        assert_eq!(notes, [(0, "blocked: test freeze".to_owned())]);
        assert_eq!(pick(&intel_only, Stage::WgpuDx12, &[rule], false, false), Ok(0), "a user request ignores the list");
        let two = [intel_only[0].clone(), cand("NVIDIA GeForce", DiscreteGpu, gpu_blocklist::NVIDIA, true)];
        assert_eq!(pick(&two, Stage::WgpuDx12, &[rule], true, true), Ok(1), "the other adapter is used");
    }
}
