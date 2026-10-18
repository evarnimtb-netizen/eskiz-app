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

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{self, TextureId};
use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use eskiz_gpu::{block_on, GpuContext};

use crate::app::EskizApp;
use crate::gpu_canvas::{self, FlushStats, GpuCanvas};
use crate::history::PixelRect;

/// The GPU half of the canvas display.
pub struct GpuDisplay {
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
            canvas: GpuCanvas::new(ctx),
            state: state.clone(),
            tex: None,
            validated: false,
        })
    }

    /// `Apple M3 (Metal, hardware)` and the like.
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

    /// Upload what changed and recomposite. See [`GpuCanvas::flush`].
    pub fn flush(&mut self, session: &crate::session::Session) -> Result<FlushStats, String> {
        if self.validated {
            return self.canvas.flush(session).map_err(|e| e.to_string());
        }
        let dev = self.state.device.clone();
        let stats = guarded(&dev, || self.canvas.flush(session))?.map_err(|e| e.to_string())?;
        if stats.composited {
            self.validated = true;
        }
        Ok(stats)
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
