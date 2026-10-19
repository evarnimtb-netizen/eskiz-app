//! Showing the GPU composite in egui, and the app glue for it.
//!
//! [`GpuDisplay`] wraps eframe's wgpu device with `eskiz_gpu::GpuContext::from_parts`, owns the
//! [`GpuCanvas`], and registers the compositor's output texture with the egui-wgpu renderer
//! (`register_native_texture`), so `draw_canvas` draws it exactly like the CPU texture it
//! replaces. The CPU path (`flatten_rect_display` into a `TextureHandle`) stays in `app.rs` as the
//! ground truth and the fallback; [`EskizApp::gpu_in_use`] says which one is on screen.
//!
//! A stroke's dirty rectangles are collected by `EskizApp::refresh` and uploaded and composited
//! once per frame, at the end of `ui()` ([`EskizApp::gpu_flush`]). That is before egui's render
//! pass on the same queue, so the frame that follows the pointer event already shows the stroke.
//!
//! # Latency report
//!
//! In debug builds, or with `ESKIZ_LATENCY=1`, a stroke prints one line to stderr when it is
//! over: how long the display update took on the UI thread (`update`: GPU upload plus composite
//! submit, or the CPU flatten plus egui texture delta), how long from the first pointer sample
//! of a frame until `ui()` returned (`frame`), and for the GPU path how long until the GPU had
//! finished the composite (`done`). All in milliseconds, mean / max over the stroke's frames.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, TextureId};
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use eskiz_gpu::{block_on, GpuContext};

use crate::app::EskizApp;
use crate::gpu_canvas::{self, FlushStats, GpuCanvas};
use crate::history::PixelRect;

/// What [`Backlog::gate`] decides for a frame's GPU work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// The GPU has caught up: submit this frame's composite.
    Go,
    /// It is still busy with earlier frames: leave the work queued in the canvas and try again soon.
    Defer,
    /// It has been busy for the whole bound without finishing anything: stop using it.
    Stalled,
}

/// How many composites the GPU may still be working on before a frame adds another. Without a
/// limit a slow adapter (a software rasteriser, an integrated GPU) takes a new full-canvas
/// composite every frame, the queue grows, and the swap chain's acquire and present end up
/// waiting for all of it on the UI thread.
pub const MAX_PENDING: u32 = 2;

/// Composites submitted and not yet reported done by the GPU.
#[derive(Default)]
pub struct Backlog {
    pending: Arc<AtomicU32>,
    busy_since: Option<Instant>,
}

impl Backlog {
    /// Decide whether a frame may submit more work. `bound` is how long the GPU may stay at the
    /// limit before it counts as stalled.
    pub fn gate(&mut self, now: Instant, bound: Duration) -> Gate {
        if self.pending.load(Ordering::Acquire) < MAX_PENDING {
            self.busy_since = None;
            return Gate::Go;
        }
        let since = *self.busy_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= bound {
            Gate::Stalled
        } else {
            Gate::Defer
        }
    }

    /// One composite was submitted. The returned closure runs when the GPU finishes it.
    pub fn submitted(&self) -> impl FnOnce() + Send + 'static {
        self.pending.fetch_add(1, Ordering::AcqRel);
        let pending = Arc::clone(&self.pending);
        move || {
            let _ = pending.try_update(Ordering::AcqRel, Ordering::Acquire, |n| Some(n.saturating_sub(1)));
        }
    }

    pub fn pending(&self) -> u32 {
        self.pending.load(Ordering::Acquire)
    }
}

/// The GPU half of the canvas display.
pub struct GpuDisplay {
    backlog: Backlog,
    /// The last [`GpuDisplay::flush`] left work queued because the GPU was behind.
    deferred: bool,
    canvas: GpuCanvas,
    state: RenderState,
    tex: Option<TextureId>,
    /// The first resize and flush run inside wgpu error scopes, so a device that cannot run
    /// the shaders turns into an error (and the CPU display) instead of a panic.
    validated: bool,
}

impl GpuDisplay {
    /// Wrap eframe's device. `Err` explains why the GPU compositor cannot be used.
    pub fn new(state: &RenderState) -> Result<Self, String> {
        let ctx = GpuContext::from_parts(state.adapter.clone(), state.device.clone(), state.queue.clone());
        if !ctx.supports_compositing() {
            return Err(format!("{} lacks compute shaders or the limits Eskiz needs", ctx.describe()));
        }
        Ok(Self {
            backlog: Backlog::default(),
            deferred: false,
            canvas: GpuCanvas::new(ctx),
            state: state.clone(),
            tex: None,
            validated: false,
        })
    }

    /// `Apple M3 (Metal, hardware)` and the like.
    /// Use another GPU wait bound (tests on slow software adapters).
    #[cfg(test)]
    pub(crate) fn set_wait_bound(&mut self, bound: std::time::Duration) {
        self.canvas.set_wait_bound(bound);
    }

    pub fn describe(&self) -> String {
        self.canvas.context().describe()
    }

    /// The id to draw the canvas with.
    pub fn tex_id(&self) -> Option<TextureId> {
        self.tex
    }

    pub fn canvas(&self) -> &GpuCanvas {
        &self.canvas
    }

    pub fn canvas_mut(&mut self) -> &mut GpuCanvas {
        &mut self.canvas
    }

    /// Make the output texture match the document and (re)register it with egui.
    pub fn prepare(&mut self, w: usize, h: usize) -> Result<(), String> {
        if !self.canvas.fits(w, h) {
            return Err(format!("a {w} x {h} canvas is larger than the GPU allows"));
        }
        let replaced = if self.validated {
            self.canvas.resize(w, h)
        } else {
            let dev = self.state.device.clone();
            guarded(&dev, || self.canvas.resize(w, h))?
        };
        if replaced || self.tex.is_none() {
            let Some(view) = self.canvas.output_view() else {
                return Err("no output texture".into());
            };
            let mut r = self.state.renderer.write();
            match self.tex {
                Some(id) => r.update_egui_texture_from_wgpu_texture(&self.state.device, view, wgpu::FilterMode::Linear, id),
                None => self.tex = Some(r.register_native_texture(&self.state.device, view, wgpu::FilterMode::Linear)),
            }
        }
        Ok(())
    }

    /// Upload what changed and recomposite. See [`GpuCanvas::flush`]. Never waits for the GPU: when
    /// it is more than [`MAX_PENDING`] composites behind the work stays queued in the canvas
    /// ([`GpuDisplay::deferred`]), and after the context's wait bound of that the call fails so the
    /// app moves to the CPU compositor.
    pub fn flush(&mut self, session: &crate::session::Session) -> Result<FlushStats, String> {
        self.deferred = false;
        if self.canvas.context().is_stalled() {
            return Err("the graphics driver did not answer in time".to_owned());
        }
        // Let the driver retire what finished (this runs the completion callbacks); never waits.
        self.canvas.context().retire_finished();
        let bound = self.canvas.context().wait_bound();
        match self.backlog.gate(Instant::now(), bound) {
            Gate::Go => {}
            Gate::Defer => {
                self.deferred = true;
                return Ok(FlushStats::default());
            }
            Gate::Stalled => {
                self.canvas.context().mark_stalled();
                return Err(format!("the graphics driver did not answer in time ({} composites unfinished)", self.backlog.pending()));
            }
        }
        let stats = if self.validated {
            self.canvas.flush(session).map_err(|e| e.to_string())?
        } else {
            let dev = self.state.device.clone();
            let stats = guarded(&dev, || self.canvas.flush(session))?.map_err(|e| e.to_string())?;
            if stats.composited {
                self.validated = true;
            }
            stats
        };
        if stats.composited {
            self.state.queue.on_submitted_work_done(self.backlog.submitted());
        }
        Ok(stats)
    }

    /// Composites the GPU has not reported done yet (after letting the driver retire finished ones).
    pub fn unfinished(&self) -> u32 {
        self.canvas.context().retire_finished();
        self.backlog.pending()
    }

    /// The last flush left work queued because the GPU was still busy: ask for another frame.
    pub fn deferred(&self) -> bool {
        self.deferred
    }

    /// Show `adj` on layer `id` as the live preview, computed on the GPU. `Err` leaves the
    /// display as it was; the caller then renders the preview on the CPU.
    pub fn adjust_preview(
        &mut self,
        session: &crate::session::Session,
        id: u64,
        adj: &eskiz_gpu::Adjustment,
    ) -> Result<(), String> {
        let dev = self.state.device.clone();
        guarded(&dev, || self.canvas.set_gpu_preview(session, id, adj))?.map_err(|e| e.to_string())
    }

    /// Run `f` when the GPU has finished the work submitted so far.
    pub fn on_done(&self, f: impl FnOnce() + Send + 'static) {
        self.state.queue.on_submitted_work_done(f);
    }
}

/// Run `f`, turning wgpu validation and out-of-memory errors inside it into an `Err`.
fn guarded<T>(dev: &wgpu::Device, f: impl FnOnce() -> T) -> Result<T, String> {
    let oom = dev.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let val = dev.push_error_scope(wgpu::ErrorFilter::Validation);
    let out = f();
    let e_val = block_on(val.pop());
    let e_oom = block_on(oom.pop());
    match e_val.or(e_oom) {
        Some(e) => Err(e.to_string()),
        None => Ok(out),
    }
}

/// Is the latency report on? Debug builds, or `ESKIZ_LATENCY` set.
pub fn latency_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| cfg!(debug_assertions) || crate::compat::env_os("ESKIZ_LATENCY").is_some())
}

/// Per-stroke latency samples, in milliseconds.
#[derive(Default)]
pub struct Latency {
    update: Vec<f64>,
    frame: Vec<f64>,
    done: Arc<Mutex<Vec<f64>>>,
    /// First pointer sample of the frame in progress.
    frame_start: Option<Instant>,
    /// Frames since the last sample, to delay the report until the GPU callbacks arrived.
    idle: u32,
}

fn stats(v: &[f64]) -> String {
    if v.is_empty() {
        return "-".into();
    }
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let max = v.iter().copied().fold(0.0, f64::max);
    format!("{mean:.2} / {max:.2}")
}

impl Latency {
    /// A pointer sample changed the canvas.
    pub fn note_input(&mut self) {
        if latency_enabled() {
            self.frame_start.get_or_insert_with(Instant::now);
        }
    }

    pub fn record_update(&mut self, d: Duration) {
        if latency_enabled() {
            self.update.push(d.as_secs_f64() * 1e3);
        }
    }

    /// The frame's display work is submitted. `gpu` is where the GPU completion is awaited.
    pub fn end_frame(&mut self, gpu: Option<&GpuDisplay>) {
        let Some(t0) = self.frame_start.take() else {
            return;
        };
        self.frame.push(t0.elapsed().as_secs_f64() * 1e3);
        if let Some(g) = gpu {
            let done = Arc::clone(&self.done);
            g.on_done(move || {
                if let Ok(mut d) = done.lock() {
                    d.push(t0.elapsed().as_secs_f64() * 1e3);
                }
            });
        }
        self.idle = 0;
    }

    /// Whether a report is waiting (so the app should repaint once more to deliver it).
    pub fn pending(&self) -> bool {
        !self.frame.is_empty()
    }

    /// Print and clear the stroke's numbers once the stroke is over and two quiet frames passed.
    pub fn maybe_report(&mut self, stroking: bool, path: &str) {
        if self.frame.is_empty() {
            return;
        }
        if stroking {
            self.idle = 0;
            return;
        }
        self.idle += 1;
        if self.idle < 3 {
            return;
        }
        let done = self.done.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default();
        eprintln!(
            "[eskiz latency] {path}: {} frames; ms mean/max: update {}, frame {}, gpu-done {}",
            self.frame.len(),
            stats(&self.update),
            stats(&self.frame),
            stats(&done),
        );
        self.update.clear();
        self.frame.clear();
        self.idle = 0;
    }
}

impl EskizApp {
    /// Use the GPU compositor on eframe's wgpu device. Does nothing (and the CPU display stays)
    /// when the device cannot run it.
    pub fn attach_gpu(&mut self, ctx: &egui::Context, state: &RenderState) {
        match GpuDisplay::new(state) {
            Ok(g) => {
                if latency_enabled() {
                    eprintln!("[eskiz] GPU compositor on {}", g.describe());
                }
                crate::startup::log(&format!("compositor: GPU ({})", g.describe()));
                self.gpu = Some(g);
                self.sync_full(ctx);
            }
            Err(e) => {
                eprintln!("eskiz: GPU compositor unavailable, compositing on the CPU: {e}");
            }
        }
    }

    /// Is the canvas on screen the GPU composite? False for the CPU path, and for documents the
    /// GPU compositor cannot show exactly.
    pub fn gpu_in_use(&self) -> bool {
        self.gpu_active
    }

    /// Decide between the GPU and the CPU display for the current document. Called by
    /// `sync_full`. On `true` the GPU canvas is marked for a full recomposite.
    pub(crate) fn gpu_sync_full(&mut self, ctx: &egui::Context) -> bool {
        let Some(g) = self.gpu.as_mut() else {
            self.gpu_active = false;
            return false;
        };
        let doc = &self.session.doc;
        if !gpu_canvas::supports(doc) {
            self.gpu_active = false;
            return false;
        }
        match g.prepare(doc.width, doc.height) {
            Ok(()) => {
                g.canvas_mut().mark_all();
                self.gpu_active = true;
                self.texture = None;
                true
            }
            Err(e) => {
                eprintln!("eskiz: GPU compositor failed, compositing on the CPU: {e}");
                self.gpu = None;
                self.gpu_active = false;
                ctx.request_repaint();
                false
            }
        }
    }

    /// The adjustment in the GPU's terms, for the kinds that have a shader.
    fn gpu_adjustment(adj: &eskiz_core::filters::Adjustment) -> Option<eskiz_gpu::Adjustment> {
        use eskiz_core::filters::AdjustKind;
        let v = |i| adj.value(i) as f32;
        match adj.kind {
            AdjustKind::Opacity => Some(eskiz_gpu::Adjustment::Opacity(eskiz_gpu::OpacityParams { amount: v(0) })),
            AdjustKind::GaussianBlur => Some(eskiz_gpu::Adjustment::Gaussian(eskiz_gpu::GaussianParams { amount: v(0) })),
            AdjustKind::Hsb => Some(eskiz_gpu::Adjustment::Hsb(eskiz_gpu::HsbParams {
                hue: v(0),
                saturation: v(1),
                brightness: v(2),
            })),
            _ => None,
        }
    }

    /// Live preview of an adjustment on the GPU. Returns false when the GPU cannot do it (CPU
    /// display, an adjustment without a shader, a failure), and the caller renders on the CPU.
    pub(crate) fn gpu_adjust_preview(&mut self, layer: u64, adj: &eskiz_core::filters::Adjustment) -> bool {
        if !self.gpu_active {
            return false;
        }
        let Some(gpu_adj) = Self::gpu_adjustment(adj) else {
            return false;
        };
        let Some(g) = self.gpu.as_mut() else {
            return false;
        };
        match g.adjust_preview(&self.session, layer, &gpu_adj) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("eskiz: GPU adjustment preview failed, using the CPU: {e}");
                false
            }
        }
    }

    /// Drop the GPU adjustment preview (the adjustment ended).
    pub(crate) fn gpu_clear_preview(&mut self) {
        if let Some(g) = self.gpu.as_mut() {
            g.canvas_mut().clear_gpu_preview();
        }
    }

    /// A stroke (or pour fill) changed `rect` of the active layer.
    pub(crate) fn gpu_mark_rect(&mut self, rect: PixelRect) {
        self.latency.note_input();
        if let Some(g) = self.gpu.as_mut() {
            g.canvas_mut().mark_rect(rect, self.active);
        }
    }

    /// End of `ui()`: upload and composite what changed, then settle the latency numbers.
    pub(crate) fn gpu_flush(&mut self, ctx: &egui::Context) {
        let stroking = self.session.is_stroking();
        crate::stall::set_gpu_compositor(self.gpu_active);
        // The swap chain skipped several frames in a row: the driver has stopped presenting.
        // Leave the GPU compositor before it becomes a freeze (the same exit as a stalled wait).
        if self.gpu_active && (self.trouble_probe)() {
            if let Some(g) = self.gpu.as_ref() {
                g.canvas().context().mark_stalled();
            }
        }
        if self.gpu_active {
            let t = Instant::now();
            let result = match self.gpu.as_mut() {
                Some(g) => g.flush(&self.session),
                None => Ok(FlushStats::default()),
            };
            let stalled = self.gpu.as_ref().is_some_and(|g| g.canvas().context().is_stalled());
            let result = if stalled { Err("the graphics driver did not answer in time".to_owned()) } else { result };
            match result {
                Ok(stats) => {
                    if self.gpu.as_ref().is_some_and(GpuDisplay::deferred) {
                        ctx.request_repaint_after(Duration::from_millis(16));
                    }
                    if stats.composited {
                        self.latency.record_update(t.elapsed());
                    }
                    self.latency.end_frame(self.gpu.as_ref());
                }
                Err(e) => {
                    eprintln!("eskiz: GPU compositor failed, compositing on the CPU: {e}");
                    if stalled || e.contains("did not answer") {
                        crate::startup::log(&format!("GPU compositor stopped: {e}; compositing on the CPU for this session"));
                        self.show_toast("Graphics driver is slow; switched to compatible mode", 6.0);
                    }
                    self.gpu = None;
                    self.gpu_active = false;
                    self.needs_full = true;
                    self.latency.end_frame(None);
                    ctx.request_repaint();
                }
            }
        } else {
            self.latency.end_frame(None);
        }
        if self.latency.pending() {
            if !stroking {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            let path = if self.gpu_active { "gpu" } else { "cpu" };
            self.latency.maybe_report(stroking, path);
        }
    }

    /// The texture `draw_canvas` should draw: the Frames view while that is on, else the
    /// GPU composite when it is in use, else the CPU texture.
    pub(crate) fn canvas_texture_id(&self) -> Option<TextureId> {
        if self.session.doc.animation.frames_enabled && self.three_d.is_none() {
            if let Some(tex) = self.anim.canvas() {
                return Some(tex.id());
            }
        }
        if self.gpu_active {
            return self.gpu.as_ref().and_then(GpuDisplay::tex_id);
        }
        self.texture.as_ref().map(|t| t.id())
    }
}

#[cfg(test)]
mod backlog_tests {
    use super::*;

    const BOUND: Duration = Duration::from_millis(1800);

    #[test]
    fn a_gpu_that_keeps_up_is_never_held_back() {
        let mut b = Backlog::default();
        let t0 = Instant::now();
        for i in 0..100 {
            assert_eq!(b.gate(t0 + Duration::from_millis(16 * i), BOUND), Gate::Go);
            let done = b.submitted();
            done();
        }
        assert_eq!(b.pending(), 0);
    }

    #[test]
    fn a_slow_gpu_defers_work_instead_of_queueing_more_and_resumes_when_it_catches_up() {
        let mut b = Backlog::default();
        let t0 = Instant::now();
        let first = b.submitted();
        let second = b.submitted();
        assert_eq!(b.pending(), MAX_PENDING);
        assert_eq!(b.gate(t0, BOUND), Gate::Defer);
        assert_eq!(b.gate(t0 + Duration::from_millis(900), BOUND), Gate::Defer);
        first();
        assert_eq!(b.gate(t0 + Duration::from_millis(1000), BOUND), Gate::Go);
        // The busy clock restarts after a Go.
        let third = b.submitted();
        assert_eq!(b.gate(t0 + Duration::from_millis(1100), BOUND), Gate::Defer);
        assert_eq!(b.gate(t0 + Duration::from_millis(2000), BOUND), Gate::Defer, "only 900 ms busy");
        second();
        third();
    }

    #[test]
    fn a_gpu_that_never_finishes_is_given_up_within_the_bound() {
        // The fake hanging step: the completion callback is never run.
        let mut b = Backlog::default();
        let t0 = Instant::now();
        let _lost: Vec<_> = (0..MAX_PENDING).map(|_| b.submitted()).collect();
        let mut verdict = Gate::Go;
        let mut at = Duration::ZERO;
        while at <= BOUND + Duration::from_millis(100) {
            verdict = b.gate(t0 + at, BOUND);
            if verdict == Gate::Stalled {
                break;
            }
            at += Duration::from_millis(16);
        }
        assert_eq!(verdict, Gate::Stalled);
        assert!(at >= BOUND && at <= BOUND + Duration::from_millis(50), "{at:?}");
    }
}
