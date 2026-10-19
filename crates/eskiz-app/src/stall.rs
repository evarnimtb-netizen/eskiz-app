//! Freeze detection: breadcrumbs, a watchdog thread and the stall marker.
//!
//! A frozen window ("Not Responding") leaves no panic and no log. This module makes it leave
//! evidence, on every platform:
//!
//! * **Breadcrumbs.** The UI thread stores the name of the phase it is in (an atomic byte, a
//!   few nanoseconds): input, gallery, canvas composite, GPU readback, then "egui paint + present"
//!   once `ui` has returned (eframe has no hook between paint and present).
//! * **Heartbeat.** The start of every `ui` call proves the previous frame, present included,
//!   finished. A watchdog thread pokes the event loop (`Context::request_repaint`) when the
//!   heartbeat is a second old, so an idle window still beats; a window whose UI thread is stuck
//!   does not answer.
//! * **Stall.** No heartbeat for [`STALL_AFTER`] while the window is visible: the watchdog appends
//!   the last breadcrumb to `startup.log` and writes `stalled.marker` (renderer and phase). If the
//!   thread recovers, the marker is removed again (a long import is not a driver fault).
//! * **Next start.** A marker left behind (the user killed the frozen window) makes
//!   [`crate::renderer::plan`] skip that renderer and start one rung down the ladder
//!   ([`crate::startup::order`]). The skip is remembered in `renderer-skip.txt` until
//!   Try normal graphics or `--renderer auto` clears it. A stall in the present phase first
//!   switches to a non-blocking present mode before a renderer is given up.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::startup::Stage;

/// How long the UI thread may go without finishing a frame.
pub const STALL_AFTER: Duration = Duration::from_secs(3);
/// The watchdog asks for a repaint when the last heartbeat is this old.
const POKE_AFTER: Duration = Duration::from_secs(1);

const MARKER: &str = "stalled.marker";
const SKIP_FILE: &str = "renderer-skip.txt";

/// What the UI thread is doing, as far as the breadcrumbs go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    /// Between frames.
    Idle = 0,
    Input = 1,
    Gallery = 2,
    CanvasComposite = 3,
    GpuReadback = 4,
    /// After `ui` returned: egui tessellation, paint and the swap-chain present.
    PaintPresent = 5,
    /// A phase used by tests.
    Fake = 6,
    /// The compositor is queueing tile uploads (`Queue::write_texture`, staging hand-over).
    GpuUpload = 7,
    /// The compositor is recording and submitting its compute passes.
    GpuComposite = 8,
    /// egui-wgpu has prepared the frame; the next step is `Surface::get_current_texture`.
    SurfaceAcquire = 9,
    /// The surface texture was acquired and the render pass is recorded; what follows is the
    /// encoder finish, `Queue::submit` and `Queue::present`.
    SubmitPresent = 10,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Input => "input",
            Self::Gallery => "gallery",
            Self::CanvasComposite => "canvas composite",
            Self::GpuReadback => "GPU readback",
            Self::PaintPresent => "egui paint + present",
            Self::Fake => "fake phase",
            Self::GpuUpload => "GPU upload",
            Self::GpuComposite => "GPU composite submit",
            Self::SurfaceAcquire => "surface acquire",
            Self::SubmitPresent => "queue submit + present",
        }
    }

    /// Phases from the end of `ui` to the swap-chain present: where a driver that never
    /// returns from a vsync wait or a present leaves the thread.
    pub fn is_present_path(self) -> bool {
        matches!(self, Self::PaintPresent | Self::SurfaceAcquire | Self::SubmitPresent)
    }

    /// Phases that are the GPU compositor's own work.
    pub fn is_gpu_work(self) -> bool {
        matches!(self, Self::GpuReadback | Self::GpuUpload | Self::GpuComposite)
    }

    fn from_name(name: &str) -> Option<Self> {
        [
            Self::PaintPresent,
            Self::GpuReadback,
            Self::GpuUpload,
            Self::GpuComposite,
            Self::SurfaceAcquire,
            Self::SubmitPresent,
        ]
        .into_iter()
        .find(|p| p.name() == name)
    }

    fn from_u8(n: u8) -> Self {
        match n {
            1 => Self::Input,
            2 => Self::Gallery,
            3 => Self::CanvasComposite,
            4 => Self::GpuReadback,
            5 => Self::PaintPresent,
            6 => Self::Fake,
            7 => Self::GpuUpload,
            8 => Self::GpuComposite,
            9 => Self::SurfaceAcquire,
            10 => Self::SubmitPresent,
            _ => Self::Idle,
        }
    }

}

static PHASE: AtomicU8 = AtomicU8::new(0);
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
static VISIBLE: AtomicBool = AtomicBool::new(true);
/// The GPU compositor is the one on screen (written by the UI thread, read by the watchdog).
static GPU_COMPOSITOR: AtomicBool = AtomicBool::new(false);

/// Tell the watchdog which compositor is in use, so a stall report names it.
pub fn set_gpu_compositor(on: bool) {
    GPU_COMPOSITOR.store(on, Ordering::Relaxed);
}

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    u64::try_from(epoch().elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Record the phase the UI thread enters. Cheap: one relaxed store.
pub fn crumb(phase: Phase) {
    PHASE.store(phase as u8, Ordering::Relaxed);
}

/// The last breadcrumb.
pub fn current_phase() -> Phase {
    Phase::from_u8(PHASE.load(Ordering::Relaxed))
}

/// A frame starts: the previous one is complete. `visible` is false while the window is minimised
/// (nothing redraws then, which is not a stall).
pub fn beat(visible: bool) {
    VISIBLE.store(visible, Ordering::Relaxed);
    LAST_BEAT_MS.store(now_ms(), Ordering::Relaxed);
    crumb(Phase::Input);
}

/// What the watchdog decides on one look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Healthy,
    /// Ask the event loop for a frame.
    Poke,
    /// No frame for `secs` while in `phase`. Reported once per stall.
    Stalled { phase: Phase, secs: f64 },
    /// The stalled thread came back after `secs`.
    Recovered { secs: f64 },
}

/// The watchdog's memory between looks.
#[derive(Default)]
pub struct Watch {
    reported: bool,
}

impl Watch {
    /// One look. `age` is the time since the last heartbeat.
    pub fn look(&mut self, age: Duration, phase: Phase, visible: bool) -> Verdict {
        if age < POKE_AFTER {
            if self.reported {
                self.reported = false;
                // The age counts from the beat that ended the stall, so the length is not known
                // here; the caller keeps the time it reported at.
                return Verdict::Recovered { secs: 0.0 };
            }
            return Verdict::Healthy;
        }
        if !visible {
            return Verdict::Healthy;
        }
        if age >= STALL_AFTER && !self.reported {
            self.reported = true;
            return Verdict::Stalled { phase, secs: age.as_secs_f64() };
        }
        Verdict::Poke
    }
}

/// Contents of `stalled.marker`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StallMarker {
    pub stage: Stage,
    pub phase: String,
    /// The GPU compositor was in use when the stall began.
    pub gpu_compositor: bool,
}

impl StallMarker {
    pub fn encode(&self) -> String {
        format!("renderer={}\nphase={}\ncompositor={}\n", self.stage.name(), self.phase, if self.gpu_compositor { "gpu" } else { "cpu" })
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut stage = None;
        let mut phase = String::new();
        let mut gpu_compositor = false;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("renderer", v)) => stage = Stage::parse(v),
                Some(("phase", v)) => phase = v.trim().to_owned(),
                Some(("compositor", v)) => gpu_compositor = v.trim() == "gpu",
                _ => {}
            }
        }
        Some(Self { stage: stage?, phase, gpu_compositor })
    }

    fn phase(&self) -> Option<Phase> {
        Phase::from_name(&self.phase)
    }

    pub fn is_present(&self) -> bool {
        self.phase().is_some_and(Phase::is_present_path)
    }

    fn is_gpu_work(&self) -> bool {
        self.phase().is_some_and(Phase::is_gpu_work)
    }
}

pub fn write_marker(dir: &Path, marker: &StallMarker) {
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(dir.join(MARKER), marker.encode());
}

pub fn remove_marker(dir: &Path) {
    let _ = fs::remove_file(dir.join(MARKER));
}

/// Read and remove a leftover marker.
pub fn take_marker(dir: &Path) -> Option<StallMarker> {
    let path = dir.join(MARKER);
    let text = fs::read_to_string(&path).ok()?;
    fs::remove_file(&path).ok()?;
    StallMarker::parse(&text)
}

/// What earlier stalls taught us, kept across starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Skips {
    /// Renderers not to try first.
    pub stages: Vec<Stage>,
    /// Present without waiting for the display (AutoNoVsync).
    pub no_vsync: bool,
    /// Composite on the CPU even on a wgpu renderer: the last freeze happened with the GPU
    /// compositor sharing the device with the window.
    pub cpu_compositor: bool,
}

impl Skips {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("skip", v)) => {
                    if let Some(st) = Stage::parse(v) {
                        if !s.stages.contains(&st) {
                            s.stages.push(st);
                        }
                    }
                }
                Some(("novsync", v)) => s.no_vsync = v.trim() == "1",
                Some(("cpucomp", v)) => s.cpu_compositor = v.trim() == "1",
                _ => {}
            }
        }
        s
    }

    pub fn encode(&self) -> String {
        let mut out: String = self.stages.iter().map(|s| format!("skip={}\n", s.name())).collect();
        if self.no_vsync {
            out.push_str("novsync=1\n");
        }
        if self.cpu_compositor {
            out.push_str("cpucomp=1\n");
        }
        out
    }

    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(SKIP_FILE)).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) {
        if self.stages.is_empty() && !self.no_vsync && !self.cpu_compositor {
            let _ = fs::remove_file(dir.join(SKIP_FILE));
        } else {
            let _ = fs::write(dir.join(SKIP_FILE), self.encode());
        }
    }

    /// Fold in a stall from the last run. A stall while presenting first switches to the
    /// non-blocking present mode; a second one (or any other phase) gives the renderer up, unless
    /// it is `last`, the bottom of the ladder. Returns a sentence for the log and the toast.
    pub fn learn(&mut self, m: &StallMarker, last: Stage) -> String {
        // The compositor and the window share one device there: first take the compositor away
        // (the CPU compositor on the same renderer is what a software adapter runs fine).
        if m.gpu_compositor && !self.cpu_compositor && (m.is_present() || m.is_gpu_work()) {
            self.cpu_compositor = true;
            return format!(
                "The last run froze in '{}' ({}) with the GPU compositor. Eskiz now composites on the CPU and keeps the renderer.",
                m.phase,
                m.stage.name()
            );
        }
        if m.is_present() && !self.no_vsync {
            self.no_vsync = true;
            return format!(
                "The last run froze while presenting a frame ({}). Eskiz now presents without waiting for the display.",
                m.stage.name()
            );
        }
        if m.stage == last {
            return format!(
                "The last run froze with the {} renderer; it is still used because nothing else is left.",
                m.stage.label()
            );
        }
        if !self.stages.contains(&m.stage) {
            self.stages.push(m.stage);
        }
        format!(
            "The last run froze in '{}' ({}). Eskiz skips the {} renderer now.",
            m.phase,
            m.stage.name(),
            m.stage.name()
        )
    }
}

/// Frames in a row for which the swap chain gave no texture (acquire timed out, surface lost)
/// before the session gives the GPU compositor up.
pub const SURFACE_TROUBLE_AFTER: u32 = 5;

/// Counts consecutive frames the swap chain would not hand out a texture. wgpu bounds the
/// acquire itself (about a second, then `Timeout`), so a driver that stops presenting shows up
/// as a run of skipped frames before it shows up as a freeze.
pub struct SurfaceTrouble {
    bad: AtomicU32,
    tripped: AtomicBool,
}

impl SurfaceTrouble {
    pub const fn new() -> Self {
        Self { bad: AtomicU32::new(0), tripped: AtomicBool::new(false) }
    }

    /// A frame was skipped. Returns the length of the run.
    pub fn problem(&self) -> u32 {
        let n = self.bad.fetch_add(1, Ordering::Relaxed) + 1;
        if n >= SURFACE_TROUBLE_AFTER {
            self.tripped.store(true, Ordering::Relaxed);
        }
        n
    }

    /// A frame got its texture: the run is over.
    pub fn ok(&self) {
        self.bad.store(0, Ordering::Relaxed);
    }

    /// True once after the run reached [`SURFACE_TROUBLE_AFTER`].
    pub fn take_tripped(&self) -> bool {
        self.tripped.swap(false, Ordering::Relaxed)
    }
}

impl Default for SurfaceTrouble {
    fn default() -> Self {
        Self::new()
    }
}

static SURFACE: SurfaceTrouble = SurfaceTrouble::new();

/// The window's swap chain skipped a frame. Returns the length of the run.
pub fn surface_problem() -> u32 {
    SURFACE.problem()
}

/// The window's swap chain gave a texture.
pub fn surface_ok() {
    SURFACE.ok();
}

/// Has the swap chain failed often enough in a row to leave the GPU compositor? Once per run.
pub fn take_surface_trouble() -> bool {
    SURFACE.take_tripped()
}

/// Shared state of the running watchdog.
struct Running {
    dir: Option<PathBuf>,
    stage: Stage,
}

/// Start the watchdog thread (once). `poke` asks the event loop for a frame.
pub fn spawn_watchdog(dir: Option<PathBuf>, stage: Stage, poke: impl Fn() + Send + 'static) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let run = Running { dir, stage };
        let _ = std::thread::Builder::new().name("eskiz-watchdog".into()).spawn(move || watch_loop(&run, &poke));
    });
}

fn watch_loop(run: &Running, poke: &dyn Fn()) {
    let mut watch = Watch::default();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let age = Duration::from_millis(now_ms().saturating_sub(LAST_BEAT_MS.load(Ordering::Relaxed)));
        // A slow close is not a driver fault: it must not make the next start skip a renderer.
        let visible = VISIBLE.load(Ordering::Relaxed) && !crate::exit::closing();
        match watch.look(age, current_phase(), visible) {
            Verdict::Healthy => {}
            Verdict::Poke => poke(),
            Verdict::Stalled { phase, secs } => {
                report_stall(run.dir.as_deref(), run.stage, phase, secs, GPU_COMPOSITOR.load(Ordering::Relaxed));
            }
            Verdict::Recovered { .. } => {
                if let Some(d) = &run.dir {
                    remove_marker(d);
                    append_log(d, "the UI thread is responding again");
                }
            }
        }
    }
}

fn append_log(dir: &Path, line: &str) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("startup.log")) {
        let _ = writeln!(f, "[{:7.3}s] {line}", epoch().elapsed().as_secs_f64());
    }
}

/// Write the log line and the marker for a stall. Also callable from tests.
pub fn report_stall(dir: Option<&Path>, stage: Stage, phase: Phase, secs: f64, gpu_compositor: bool) {
    let line = format!(
        "STALL: the UI thread has not finished a frame for {secs:.1}s; last phase: {}; renderer: {}; compositor: {}",
        phase.name(),
        stage.name(),
        if gpu_compositor { "gpu" } else { "cpu" }
    );
    eprintln!("eskiz: {line}");
    if let Some(d) = dir {
        append_log(d, &line);
        write_marker(d, &StallMarker { stage, phase: phase.name().to_owned(), gpu_compositor });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("eskiz-stall-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn breadcrumbs_record_the_phase() {
        crumb(Phase::Fake);
        assert_eq!(current_phase(), Phase::Fake);
        crumb(Phase::Idle);
        assert_eq!(current_phase(), Phase::Idle);
    }

    #[test]
    fn watchdog_pokes_then_reports_a_fake_stalled_phase_once() {
        let mut w = Watch::default();
        assert_eq!(w.look(Duration::from_millis(200), Phase::Idle, true), Verdict::Healthy);
        assert_eq!(w.look(Duration::from_millis(1500), Phase::Gallery, true), Verdict::Poke);
        let v = w.look(Duration::from_millis(3200), Phase::Fake, true);
        assert!(matches!(v, Verdict::Stalled { phase: Phase::Fake, .. }), "{v:?}");
        // Not reported twice; still poking.
        assert_eq!(w.look(Duration::from_millis(4000), Phase::Fake, true), Verdict::Poke);
        assert!(matches!(w.look(Duration::from_millis(10), Phase::Input, true), Verdict::Recovered { .. }));
        assert_eq!(w.look(Duration::from_millis(10), Phase::Input, true), Verdict::Healthy);
    }

    #[test]
    fn a_minimised_window_is_not_a_stall() {
        let mut w = Watch::default();
        assert_eq!(w.look(Duration::from_secs(60), Phase::Idle, false), Verdict::Healthy);
    }

    #[test]
    fn a_stall_writes_log_and_marker_that_the_next_start_reads() {
        let d = tmp("marker");
        report_stall(Some(&d), Stage::WgpuDx12, Phase::Fake, 3.2, false);
        let log = fs::read_to_string(d.join("startup.log")).unwrap();
        assert!(log.contains("STALL") && log.contains("fake phase") && log.contains("wgpu-dx12"), "{log}");
        let m = take_marker(&d).expect("marker");
        assert_eq!(m, StallMarker { stage: Stage::WgpuDx12, phase: "fake phase".into(), gpu_compositor: false });
        assert!(take_marker(&d).is_none(), "read once");
        report_stall(Some(&d), Stage::Wgpu, Phase::Gallery, 4.0, false);
        remove_marker(&d);
        assert!(take_marker(&d).is_none(), "a recovered stall leaves no marker");
    }

    #[test]
    fn a_stall_teaches_the_skip_list() {
        let mut s = Skips::default();
        let m = StallMarker { stage: Stage::Wgpu, phase: Phase::Gallery.name().into(), gpu_compositor: false };
        let last = Stage::WgpuSoftware;
        let msg = s.learn(&m, last);
        assert!(msg.contains("skips") && s.stages == [Stage::Wgpu]);
        // Present stalls try the non-blocking present mode first.
        let p = StallMarker { stage: Stage::WgpuDx12, phase: Phase::PaintPresent.name().into(), gpu_compositor: false };
        s.learn(&p, last);
        assert!(s.no_vsync && s.stages == [Stage::Wgpu]);
        s.learn(&p, last);
        assert_eq!(s.stages, [Stage::Wgpu, Stage::WgpuDx12]);
        // OpenGL is not the bottom on Windows and Linux: a frozen glow is skipped too.
        s.learn(&StallMarker { stage: Stage::Glow, phase: Phase::Gallery.name().into(), gpu_compositor: false }, last);
        assert_eq!(s.stages, [Stage::Wgpu, Stage::WgpuDx12, Stage::Glow]);
        // The bottom rung is never given up.
        let msg = s.learn(&StallMarker { stage: last, phase: Phase::Gallery.name().into(), gpu_compositor: false }, last);
        assert!(msg.contains("nothing else is left") && !s.stages.contains(&last), "{msg}");
        let again = Skips::parse(&s.encode());
        assert_eq!(again, s);
        let d = tmp("skips");
        s.save(&d);
        assert_eq!(Skips::load(&d), s);
        Skips::default().save(&d);
        assert_eq!(Skips::load(&d), Skips::default());
    }

    #[test]
    fn a_freeze_with_the_gpu_compositor_takes_the_compositor_away_first() {
        let last = Stage::WgpuSoftware;
        for phase in [Phase::SubmitPresent, Phase::SurfaceAcquire, Phase::PaintPresent, Phase::GpuComposite, Phase::GpuUpload, Phase::GpuReadback] {
            let mut s = Skips::default();
            let m = StallMarker { stage: Stage::WgpuDx12, phase: phase.name().into(), gpu_compositor: true };
            let msg = s.learn(&m, last);
            assert!(s.cpu_compositor && !s.no_vsync && s.stages.is_empty(), "{phase:?}: {msg}");
            // It survives the file, and a second freeze (now on the CPU compositor) moves on.
            assert_eq!(Skips::parse(&s.encode()), s);
            let again = StallMarker { gpu_compositor: false, ..m };
            s.learn(&again, last);
            assert!(s.cpu_compositor);
            assert!(s.no_vsync || s.stages == [Stage::WgpuDx12], "{phase:?}");
        }
        // A freeze in the UI itself says nothing about the compositor.
        let mut s = Skips::default();
        s.learn(&StallMarker { stage: Stage::Wgpu, phase: Phase::Gallery.name().into(), gpu_compositor: true }, last);
        assert!(!s.cpu_compositor);
    }

    #[test]
    fn the_marker_names_the_compositor_and_old_markers_still_read() {
        let m = StallMarker { stage: Stage::WgpuDx12, phase: Phase::SubmitPresent.name().into(), gpu_compositor: true };
        assert_eq!(StallMarker::parse(&m.encode()), Some(m.clone()));
        assert!(m.is_present());
        let old = StallMarker::parse("renderer=wgpu-dx12\nphase=egui paint + present\n").unwrap();
        assert!(old.is_present() && !old.gpu_compositor);
        for n in 0..=10u8 {
            let p = Phase::from_u8(n);
            assert_eq!(p as u8, n, "{p:?}");
        }
    }

    #[test]
    fn a_run_of_skipped_frames_trips_once_and_a_good_frame_ends_the_run() {
        let t = SurfaceTrouble::new();
        for _ in 0..SURFACE_TROUBLE_AFTER - 1 {
            t.problem();
        }
        assert!(!t.take_tripped());
        t.ok();
        for _ in 0..SURFACE_TROUBLE_AFTER - 1 {
            t.problem();
        }
        assert!(!t.take_tripped(), "the good frame restarted the count");
        assert_eq!(t.problem(), SURFACE_TROUBLE_AFTER);
        assert!(t.take_tripped());
        assert!(!t.take_tripped(), "reported once");
    }
}
