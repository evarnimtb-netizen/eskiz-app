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
//!   [`crate::renderer::plan`] skip that renderer: wgpu auto, wgpu-dx12, wgpu-vulkan, then OpenGL
//!   with the CPU compositor. The skip is remembered in `renderer-skip.txt` until
//!   `--renderer auto` clears it. A stall in the present phase first switches to a
//!   non-blocking present mode before a renderer is given up.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
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
        }
    }

    fn from_u8(n: u8) -> Self {
        match n {
            1 => Self::Input,
            2 => Self::Gallery,
            3 => Self::CanvasComposite,
            4 => Self::GpuReadback,
            5 => Self::PaintPresent,
            6 => Self::Fake,
            _ => Self::Idle,
        }
    }

}

static PHASE: AtomicU8 = AtomicU8::new(0);
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
static VISIBLE: AtomicBool = AtomicBool::new(true);

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
}

impl StallMarker {
    pub fn encode(&self) -> String {
        format!("renderer={}\nphase={}\n", self.stage.name(), self.phase)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut stage = None;
        let mut phase = String::new();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("renderer", v)) => stage = Stage::parse(v),
                Some(("phase", v)) => phase = v.trim().to_owned(),
                _ => {}
            }
        }
        Some(Self { stage: stage?, phase })
    }

    pub fn is_present(&self) -> bool {
        self.phase == Phase::PaintPresent.name()
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
}

impl Skips {
    pub fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("skip", v)) => {
                    if let Some(st) = Stage::parse(v) {
                        if st != Stage::Glow && !s.stages.contains(&st) {
                            s.stages.push(st);
                        }
                    }
                }
                Some(("novsync", v)) => s.no_vsync = v.trim() == "1",
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
        out
    }

    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(SKIP_FILE)).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) {
        if self.stages.is_empty() && !self.no_vsync {
            let _ = fs::remove_file(dir.join(SKIP_FILE));
        } else {
            let _ = fs::write(dir.join(SKIP_FILE), self.encode());
        }
    }

    /// Fold in a stall from the last run. A stall while presenting first switches to the
    /// non-blocking present mode; a second one (or any other phase) gives the renderer up.
    /// Returns a sentence for the log and the toast.
    pub fn learn(&mut self, m: &StallMarker) -> String {
        if m.is_present() && !self.no_vsync && m.stage != Stage::Glow {
            self.no_vsync = true;
            return format!(
                "The last run froze while presenting a frame ({}). Eskiz now presents without waiting for the display.",
                m.stage.name()
            );
        }
        if m.stage == Stage::Glow {
            return "The last run froze with the compatible (OpenGL) renderer; it is still used because nothing else is left.".to_owned();
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
        match watch.look(age, current_phase(), VISIBLE.load(Ordering::Relaxed)) {
            Verdict::Healthy => {}
            Verdict::Poke => poke(),
            Verdict::Stalled { phase, secs } => {
                report_stall(run.dir.as_deref(), run.stage, phase, secs);
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
pub fn report_stall(dir: Option<&Path>, stage: Stage, phase: Phase, secs: f64) {
    let line = format!("STALL: the UI thread has not finished a frame for {secs:.1}s; last phase: {}; renderer: {}", phase.name(), stage.name());
    eprintln!("eskiz: {line}");
    if let Some(d) = dir {
        append_log(d, &line);
        write_marker(d, &StallMarker { stage, phase: phase.name().to_owned() });
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
        report_stall(Some(&d), Stage::WgpuDx12, Phase::Fake, 3.2);
        let log = fs::read_to_string(d.join("startup.log")).unwrap();
        assert!(log.contains("STALL") && log.contains("fake phase") && log.contains("wgpu-dx12"), "{log}");
        let m = take_marker(&d).expect("marker");
        assert_eq!(m, StallMarker { stage: Stage::WgpuDx12, phase: "fake phase".into() });
        assert!(take_marker(&d).is_none(), "read once");
        report_stall(Some(&d), Stage::Wgpu, Phase::Gallery, 4.0);
        remove_marker(&d);
        assert!(take_marker(&d).is_none(), "a recovered stall leaves no marker");
    }

    #[test]
    fn a_stall_teaches_the_skip_list() {
        let mut s = Skips::default();
        let m = StallMarker { stage: Stage::Wgpu, phase: Phase::Gallery.name().into() };
        let msg = s.learn(&m);
        assert!(msg.contains("skips") && s.stages == [Stage::Wgpu]);
        // Present stalls try the non-blocking present mode first.
        let p = StallMarker { stage: Stage::WgpuDx12, phase: Phase::PaintPresent.name().into() };
        s.learn(&p);
        assert!(s.no_vsync && s.stages == [Stage::Wgpu]);
        s.learn(&p);
        assert_eq!(s.stages, [Stage::Wgpu, Stage::WgpuDx12]);
        let again = Skips::parse(&s.encode());
        assert_eq!(again, s);
        let d = tmp("skips");
        s.save(&d);
        assert_eq!(Skips::load(&d), s);
        Skips::default().save(&d);
        assert_eq!(Skips::load(&d), Skips::default());
    }
}
