//! Adjustment mode.
//!
//! Choosing an adjustment from the panel enters adjustment mode on the active layer:
//!
//! * a banner at the top shows the name, the parameters and the Layer or Pencil switch (see
//!   `panels/adjustments.rs` and `panels/adjust_extras.rs`);
//! * in Layer mode, dragging left or right anywhere on the canvas changes the selected
//!   parameter; a drag that starts on an origin handle moves the handle instead;
//! * in Pencil mode, dragging paints a mask with a round dab, and the effect applies
//!   only through that mask. Clone always paints, from the source point it was given;
//! * the canvas shows a live preview in place of the layer. The preview runs on a worker thread
//!   (or the GPU for Opacity, Gaussian blur and HSB in Layer mode), and the layer's stored pixels
//!   are not touched until the commit;
//! * tapping the adjustments button again, Enter, or Apply commits the result as one undo step
//!   named after the adjustment. Cancel, Escape or switching layer leaves no trace.
//!
//! The effective mask of a render is the painted Pencil mask multiplied by the selection, so an
//! adjustment only lands inside the active selection.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use eframe::egui::{self, Pos2};
use eskiz_core::filters::{keep_alpha, keep_alpha_t, proxy_factor, render_preview, AdjustKind, Adjustment, Progress, Proxy, Quality};
use eskiz_core::pixel::Chan;

use crate::app::{EskizApp, FrameInput};
use crate::input::{sample_from, PointerKind, RawPointer};
use crate::paint::CoverageStroke;

/// A layer this big or bigger (pixels) is committed on a worker thread.
const BACKGROUND_PIXELS: usize = 250_000;

/// What the next canvas tap samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    None,
    /// The clone source point (sets the normalised source position).
    CloneSource,
    /// The colour under the tap becomes the recolor source colour.
    RecolorSource,
}

/// The result of a final render, at the document's depth.
enum Rendered {
    U8(Vec<u8>),
    U16(Vec<u16>),
}

/// A commit running on a worker thread.
struct ApplyJob {
    rx: Receiver<Rendered>,
    progress: Arc<Progress>,
}

/// A live preview running on a worker thread.
struct PreviewJob {
    rx: Receiver<PreviewResult>,
}

/// A finished preview: the canvas-sized layer to show, and the proxy it was rendered from (kept for
/// the next one). The layer is built on the worker, so the UI thread only swaps it in.
struct PreviewResult {
    layer: eskiz_core::Layer,
    proxy: Option<Arc<Proxy>>,
}

/// Render one preview. With `k > 1` the adjustment runs on a proxy of the layer (built here unless
/// `cached` already has one for this `k`) and the result is scaled up to the canvas; the commit
/// never takes this path. `k == 1` renders every pixel.
#[allow(clippy::too_many_arguments)]
fn run_preview(
    adj: &Adjustment,
    source: &[u8],
    paint: Option<&[f32]>,
    selection: Option<&[f32]>,
    cached: Option<Arc<Proxy>>,
    w: usize,
    h: usize,
    k: usize,
) -> PreviewResult {
    if k <= 1 {
        let pixels = render_masked(adj, source, w, h, Quality::Preview, None, paint, selection);
        return PreviewResult { layer: eskiz_core::Layer::from_rgba(w, h, pixels), proxy: None };
    }
    let proxy = cached.filter(|p| p.k == k).unwrap_or_else(|| Arc::new(Proxy::build(source, selection, w, h, k)));
    let layer = render_preview(adj, &proxy, paint, w, h);
    PreviewResult { layer, proxy: Some(proxy) }
}

/// The adjustment that is open.
pub struct AdjustState {
    pub adj: Adjustment,
    /// The layer being adjusted.
    pub layer: u64,
    /// Pixels of the layer when the mode opened. Every render starts from these.
    source: Arc<Vec<u8>>,
    /// The layer's exact 16-bit pixels when the document is 16-bit. The preview runs on `source`
    /// (8 bits is plenty for the screen); the final render runs on these, so committing an
    /// adjustment never quantises the layer.
    source16: Option<Arc<Vec<u16>>>,
    /// The selection mask at open (`width * height` values), when a selection is active.
    selection: Option<Arc<Vec<f32>>>,
    /// Pencil paint coverage (`width * height` values). Present in Pencil mode and for Clone.
    paint: Option<Arc<Vec<f32>>>,
    /// Which parameter the slider and the canvas drag change.
    pub param: usize,
    /// The next canvas tap samples a point or colour.
    pub pick: Pick,
    /// A value drag is in progress.
    pub dragging: bool,
    /// A paint stroke is in progress.
    pub painting: bool,
    /// An origin or source handle is being dragged.
    pub handle_drag: bool,
    /// Canvas position (document pixels) of the last dab of the stroke in progress.
    last: Option<(f64, f64)>,
    /// The brush stroke whose coverage is being merged into the Pencil paint.
    cover: Option<CoverageStroke>,
    /// Time of the next Pencil sample, seconds (the pointer does not carry one here).
    paint_t: f64,
    /// The preview is out of date.
    pub dirty: bool,
    /// `Session::edits` when the mode opened. Any other change to the document ends the mode.
    edits: u64,
    job: Option<ApplyJob>,
    preview: Option<PreviewJob>,
    /// The layer (and selection) downscaled for proxy previews, for the factor it was built for.
    proxy: Option<Arc<Proxy>>,
}

impl AdjustState {
    /// Is the final render running?
    pub fn applying(&self) -> bool {
        self.job.is_some()
    }

    /// Progress of the final render, 0..=1.
    pub fn progress(&self) -> f32 {
        self.job.as_ref().map_or(0.0, |j| j.progress.fraction())
    }

    /// Does the Pencil or Clone paint cover anything yet?
    pub fn painted(&self) -> bool {
        self.paint.as_ref().is_some_and(|p| p.iter().any(|&v| v > 0.0))
    }

    /// Does this adjustment paint its mask (Pencil mode, or Clone)?
    pub fn paints(&self) -> bool {
        self.paint.is_some()
    }
}

/// The value as the banner shows it: a percentage, pixels, a degree angle or a choice name.
pub fn format_value(kind: AdjustKind, param: usize, value: f64) -> String {
    let params = kind.params();
    let spec = params[param.min(params.len() - 1)];
    if !spec.choices.is_empty() {
        let i = (value.round().max(0.0) as usize).min(spec.choices.len() - 1);
        return spec.choices[i].to_string();
    }
    if spec.periodic {
        return format!("{:+.0}\u{b0}", value * 180.0);
    }
    if spec.name.contains("Vector") || spec.name.contains("Offset") {
        return format!("{value:+.0} px");
    }
    if spec.min >= -1.0 && spec.max <= 1.0 {
        return if spec.min < 0.0 { format!("{:+.0}%", value * 100.0) } else { format!("{:.0}%", value * 100.0) };
    }
    format!("{value:.2}")
}

/// Render `adj` with the Pencil paint and the selection combined into one mask.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_masked<T: Chan>(
    adj: &Adjustment,
    source: &[T],
    w: usize,
    h: usize,
    quality: Quality,
    progress: Option<&Progress>,
    paint: Option<&[f32]>,
    selection: Option<&[f32]>,
) -> Vec<T> {
    match (paint, selection) {
        (None, None) => adj.render(source, w, h, quality, progress, None),
        (Some(p), None) => adj.render(source, w, h, quality, progress, Some(p)),
        (None, Some(s)) => adj.render(source, w, h, quality, progress, Some(s)),
        (Some(p), Some(s)) => {
            let combined: Vec<f32> = p.iter().zip(s).map(|(a, b)| a * b).collect();
            adj.render(source, w, h, quality, progress, Some(&combined))
        }
    }
}

fn slice(o: &Option<Arc<Vec<f32>>>) -> Option<&[f32]> {
    o.as_ref().map(|v| v.as_slice())
}

impl EskizApp {
    /// Is an adjustment open? Painting, undo and layer edits wait until it closes.
    pub fn adjusting(&self) -> bool {
        self.adjust.is_some()
    }

    /// The adjustment that is open, if any.
    pub fn adjustment(&self) -> Option<&Adjustment> {
        self.adjust.as_ref().map(|s| &s.adj)
    }

    /// Open `kind` on the active layer in Layer mode.
    pub fn enter_adjustment(&mut self, kind: AdjustKind) -> bool {
        self.enter_adjustment_mode(kind, false)
    }

    /// Open `kind` on the active layer. `pencil` opens it in Pencil mode. Refused (with
    /// a toast) for groups, locked layers and while a stroke is running or Liquify is open.
    pub fn enter_adjustment_mode(&mut self, kind: AdjustKind, pencil: bool) -> bool {
        if self.refuse_in_3d() {
            return false;
        }
        if self.liquify.is_some() {
            return false;
        }
        if self.session.is_stroking() {
            return false;
        }
        let id = self.active;
        if !self.session.can_edit_pixels(id) {
            let msg = if self.session.doc.node(id).is_none_or(|n| n.is_group()) {
                "Select a layer to adjust."
            } else {
                "The layer is locked."
            };
            self.show_toast(msg, 2.0);
            return false;
        }
        if self.adjust.as_ref().is_some_and(AdjustState::applying) {
            return false;
        }
        self.adjust_cancel();
        let Some(layer) = self.session.doc.layer(id) else {
            return false;
        };
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let source = Arc::new(layer.to_rgba());
        let source16 = (layer.depth() == eskiz_core::Depth::U16).then(|| Arc::new(layer.to_rgba16()));
        let selection = self
            .session
            .selection()
            .mask()
            .filter(|m| m.width == w && m.height == h && self.session.selection().is_active())
            .map(|m| Arc::new(m.data.clone()));
        self.commit_stroke("an adjustment opened");
        self.popovers.close();
        let mut adj = Adjustment::new(kind);
        // Clone always paints; Pencil mode paints for every type.
        adj.pencil = pencil || kind == AdjustKind::Clone;
        let paint = adj.pencil.then(|| Arc::new(vec![0.0f32; w * h]));
        self.adjust = Some(AdjustState {
            adj,
            layer: id,
            source,
            source16,
            selection,
            paint,
            param: 0,
            pick: Pick::None,
            dragging: false,
            painting: false,
            handle_drag: false,
            last: None,
            cover: None,
            paint_t: 0.0,
            dirty: true,
            edits: self.session.edits(),
            job: None,
            preview: None,
            proxy: None,
        });
        true
    }

    /// Switch the open adjustment between Layer and Pencil mode. Switching clears the
    /// paint. Clone stays painted.
    pub fn adjust_set_pencil(&mut self, pencil: bool) {
        let n = self.session.doc.width * self.session.doc.height;
        if let Some(st) = self.adjust.as_mut().filter(|s| !s.applying() && s.adj.kind != AdjustKind::Clone) {
            st.adj.pencil = pencil;
            st.paint = pencil.then(|| Arc::new(vec![0.0f32; n]));
            st.last = None;
            st.dirty = true;
        }
    }

    /// Arm (or disarm, when pressed again) the next canvas tap to sample a point or colour.
    pub fn adjust_toggle_pick(&mut self, pick: Pick) {
        if let Some(st) = self.adjust.as_mut().filter(|s| !s.applying()) {
            st.pick = if st.pick == pick { Pick::None } else { pick };
        }
    }

    /// Set parameter `i` of the open adjustment (clamped) and refresh the preview.
    pub fn adjust_set(&mut self, i: usize, value: f64) {
        if let Some(st) = self.adjust.as_mut().filter(|s| !s.applying()) {
            let before = st.adj.clone();
            st.adj.set(i, value);
            if st.adj != before {
                st.dirty = true;
            }
        }
    }

    /// Replace the whole adjustment (used by the curve and gradient editors). The kind must match.
    pub fn adjust_store(&mut self, adj: Adjustment) {
        if let Some(st) = self.adjust.as_mut().filter(|s| !s.applying() && s.adj.kind == adj.kind) {
            if st.adj != adj {
                st.adj = adj;
                st.dirty = true;
            }
        }
    }

    /// Choose which parameter the slider edits.
    pub fn adjust_select_param(&mut self, i: usize) {
        if let Some(st) = self.adjust.as_mut() {
            st.param = i.min(st.adj.kind.params().len().saturating_sub(1));
        }
    }

    /// Change the selected parameter by a horizontal drag of `dx` points: a drag across the
    /// canvas width covers the parameter's whole range. Right increases.
    pub fn adjust_drag_by(&mut self, dx: f32) {
        let canvas_width = (self.session.doc.width as f32 * self.view.zoom).max(1.0);
        let Some(st) = self.adjust.as_ref() else {
            return;
        };
        let visible = st.adj.visible_params();
        let param = if visible.contains(&st.param) { st.param } else { visible.first().copied().unwrap_or(0) };
        // Curves has no sliders: a drag on the canvas changes nothing.
        let Some(&spec) = st.adj.kind.params().get(param) else {
            return;
        };
        if !spec.choices.is_empty() || spec.hidden {
            return;
        }
        let dv = f64::from(dx / canvas_width) * (spec.max - spec.min);
        let next = st.adj.value(param) + dv;
        self.adjust_set(param, next);
    }

    /// Put the open adjustment back to its defaults and clear any paint.
    pub fn adjust_reset(&mut self) {
        let n = self.session.doc.width * self.session.doc.height;
        if let Some(st) = self.adjust.as_mut().filter(|s| !s.applying()) {
            st.adj.reset();
            st.adj.pencil = st.paint.is_some();
            if st.paint.is_some() {
                st.paint = Some(Arc::new(vec![0.0f32; n]));
            }
            st.dirty = true;
        }
    }

    /// Leave the mode without changing anything.
    pub fn adjust_cancel(&mut self) {
        if self.adjust.take().is_some() {
            self.session.clear_preview();
            self.gpu_clear_preview();
            self.needs_full = true;
        }
    }

    /// Commit the open adjustment as one undo step. A slow filter on a big layer runs on a worker
    /// thread; the mode stays open (and input off) until it finishes.
    pub fn adjust_apply(&mut self, ctx: &egui::Context) {
        let Some(st) = self.adjust.as_mut().filter(|s| !s.applying()) else {
            return;
        };
        if st.adj.is_identity() {
            self.adjust_cancel();
            self.show_toast("No change to apply.", 1.5);
            return;
        }
        if st.paint.is_some() && !st.painted() {
            self.show_toast("Paint where the effect should apply first.", 2.0);
            return;
        }
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let (adj, source) = (st.adj.clone(), Arc::clone(&st.source));
        let source16 = st.source16.clone();
        let (paint, selection) = (st.paint.clone(), st.selection.clone());
        st.pick = Pick::None;
        // The final render works on the layer's own depth.
        let render = move |adj: &Adjustment, progress: Option<&Progress>, paint: Option<&[f32]>, selection: Option<&[f32]>| match &source16 {
            Some(px) => Rendered::U16(render_masked(adj, px, w, h, Quality::Full, progress, paint, selection)),
            None => Rendered::U8(render_masked(adj, &source, w, h, Quality::Full, progress, paint, selection)),
        };
        if w * h >= BACKGROUND_PIXELS {
            let progress = Arc::new(Progress::new());
            let (tx, rx) = channel();
            let (worker_progress, repaint) = (Arc::clone(&progress), ctx.clone());
            let (job_adj, job_paint, job_sel) = (adj.clone(), paint.clone(), selection.clone());
            let job_render = render.clone();
            let spawned = std::thread::Builder::new().name("eskiz-adjust".into()).spawn(move || {
                let out = job_render(&job_adj, Some(&worker_progress), slice(&job_paint), slice(&job_sel));
                let _ = tx.send(out);
                repaint.request_repaint();
            });
            if spawned.is_ok() {
                if let Some(st) = self.adjust.as_mut() {
                    st.job = Some(ApplyJob { rx, progress });
                }
                // The panel shows the one progress indicator (label and bar); no toast on top of its buttons.
                return;
            }
            // The worker could not start: render on this thread instead.
        }
        let out = render(&adj, None, slice(&paint), slice(&selection));
        self.adjust_finish(out);
    }

    /// Write the final pixels into the layer as one command and close the mode.
    fn adjust_finish(&mut self, out: Rendered) {
        let Some(st) = self.adjust.take() else {
            return;
        };
        self.session.clear_preview();
        self.gpu_clear_preview();
        self.needs_full = true;
        let alpha_lock = self.session.doc.node(st.layer).is_some_and(|n| n.props.alpha_lock);
        let name = st.adj.kind.undo_name();
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let edit = match (out, st.source16.as_ref()) {
            (Rendered::U16(mut out), Some(source)) => {
                if alpha_lock {
                    keep_alpha_t(source, &mut out);
                }
                if out.as_slice() == source.as_slice() {
                    self.show_toast("No change to apply.", 1.5);
                    return;
                }
                self.session.replace_layer(st.layer, eskiz_core::Layer::from_rgba16(w, h, out), name)
            }
            (Rendered::U8(mut out), _) => {
                if alpha_lock {
                    keep_alpha(&st.source, &mut out);
                }
                if out.as_slice() == st.source.as_slice() {
                    self.show_toast("No change to apply.", 1.5);
                    return;
                }
                // The open mode's copy of the layer is not needed any more: free it before the
                // layer is rebuilt from `out`, so the peak is the old layer, `out` and the new layer.
                let layer = st.layer;
                drop(st);
                self.session.replace_layer_pixels(layer, out, name)
            }
            (Rendered::U16(_), None) => None,
        };
        match edit {
            Some(edit) => {
                self.apply_refresh(edit.refresh);
                self.show_toast(name, 1.5);
            }
            None => self.show_toast("The adjustment could not be applied.", 2.5),
        }
    }

    /// Once per frame, before the canvas: end the mode when the document moved on under it,
    /// collect a finished commit or preview, handle Enter and Escape, and start the next preview.
    pub(crate) fn adjust_frame(&mut self, ctx: &egui::Context) {
        let Some(st) = self.adjust.as_mut() else {
            return;
        };
        let applying = st.applying();
        // Another drawing opened, or the layer changed (Clear, Fill, Merge Down...): the render
        // would write old pixels over it. While Apply is rendering, the layer is the one it started
        // on whatever is selected now.
        let stale = self.session.edits() != st.edits || self.session.doc.layer(st.layer).is_none();
        if stale || (!applying && self.active != st.layer) {
            self.adjust_cancel();
            if applying {
                self.show_toast("The adjustment was not applied: the drawing changed while it was rendering.", 3.0);
            }
            return;
        }
        if let Some(job) = st.job.as_ref() {
            match job.rx.try_recv() {
                Ok(out) => self.adjust_finish(out),
                Err(TryRecvError::Empty) => ctx.request_repaint(),
                Err(TryRecvError::Disconnected) => {
                    self.adjust_cancel();
                    self.show_toast("The adjustment failed.", 2.5);
                }
            }
            return;
        }
        // A finished preview replaces the one on screen; a still-running one is waited for.
        let mut finished = None;
        if let Some(job) = st.preview.as_ref() {
            match job.rx.try_recv() {
                Ok(out) => finished = Some(Some(out)),
                Err(TryRecvError::Empty) => ctx.request_repaint(),
                Err(TryRecvError::Disconnected) => finished = Some(None),
            }
        }
        if let Some(out) = finished {
            st.preview = None;
            let layer = st.layer;
            if let Some(out) = out {
                if let Some(st) = self.adjust.as_mut() {
                    st.proxy = out.proxy.or(st.proxy.take());
                }
                self.session.set_preview_layer(layer, out.layer);
                self.needs_full = true;
            }
        }
        if !ctx.egui_wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.adjust_apply(ctx);
                return;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.adjust_cancel();
                return;
            }
        }
        self.adjust_refresh_preview(ctx);
    }

    /// Start the next preview when the adjustment changed and no preview is running.
    fn adjust_refresh_preview(&mut self, ctx: &egui::Context) {
        let Some(st) = self.adjust.as_mut() else {
            return;
        };
        if !st.dirty || st.preview.is_some() {
            return;
        }
        st.dirty = false;
        let (adj, layer) = (st.adj.clone(), st.layer);
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        self.needs_full = true;
        if adj.is_identity() {
            self.session.clear_preview();
            self.gpu_clear_preview();
            return;
        }
        // The GPU shows Opacity, Layer-mode Gaussian blur and HSB. It knows nothing of masks, so
        // it is used only when there is no paint and no selection.
        let (no_paint, no_selection) = match self.adjust.as_ref() {
            Some(st) => (st.paint.is_none(), st.selection.is_none()),
            None => return,
        };
        if no_paint && no_selection && !adj.pencil && self.gpu_adjust_preview(layer, &adj) {
            self.session.clear_preview();
            return;
        }
        self.gpu_clear_preview();
        let Some(st) = self.adjust.as_mut() else {
            return;
        };
        let (source, paint, selection) = (Arc::clone(&st.source), st.paint.clone(), st.selection.clone());
        // Big canvases preview on a proxy about as fine as the screen shows them.
        let k = proxy_factor(adj.kind, adj.pencil, w, h, self.view.zoom);
        let cached = st.proxy.clone();
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let (adj_for_thread, cached_for_thread) = (adj.clone(), cached.clone());
        let (src_t, paint_t, sel_t) = (Arc::clone(&source), paint.clone(), selection.clone());
        let spawned = std::thread::Builder::new().name("eskiz-adjust-preview".into()).spawn(move || {
            let out = run_preview(&adj_for_thread, &src_t, slice(&paint_t), slice(&sel_t), cached_for_thread, w, h, k);
            let _ = tx.send(out);
            repaint.request_repaint();
        });
        match spawned {
            Ok(_) => {
                if let Some(st) = self.adjust.as_mut() {
                    st.preview = Some(PreviewJob { rx });
                }
            }
            Err(_) => {
                // No thread: render the preview here.
                let out = run_preview(&adj, &source, slice(&paint), slice(&selection), cached, w, h, k);
                if let Some(st) = self.adjust.as_mut() {
                    st.proxy = out.proxy.or(st.proxy.take());
                }
                self.session.set_preview_layer(layer, out.layer);
            }
        }
    }

    /// Is a live preview being computed (or about to be)? Tests wait on it.
    #[cfg(test)]
    pub(crate) fn adjust_preview_busy(&self) -> bool {
        self.adjust.as_ref().is_some_and(|s| s.preview.is_some() || s.dirty)
    }

    /// Canvas position of the origin or source handle, in screen points, when the type has one.
    pub(crate) fn adjust_handle_screen(&self) -> Option<Pos2> {
        let st = self.adjust.as_ref()?;
        let (hx, hy) = st.adj.handle_indices()?;
        let (w, h) = (self.session.doc.width as f64, self.session.doc.height as f64);
        let (x, y) = (st.adj.value(hx) * w, st.adj.value(hy) * h);
        Some(self.view.canvas_to_screen(self.viewport, x, y))
    }

    /// A tap while a sampler is armed: the clone source point, or the recolor source colour.
    pub(crate) fn adjust_pick_at(&mut self, p: Pos2) {
        let (cx, cy) = self.view.screen_to_canvas(self.viewport, p);
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let Some(st) = self.adjust.as_mut() else {
            return;
        };
        let (ix, iy) = (cx.floor().clamp(0.0, (w - 1) as f64) as usize, cy.floor().clamp(0.0, (h - 1) as f64) as usize);
        match st.pick {
            Pick::CloneSource => {
                let (sx, sy) = ((ix as f64 + 0.5) / w as f64, (iy as f64 + 0.5) / h as f64);
                st.adj.set(0, sx);
                st.adj.set(1, sy);
            }
            Pick::RecolorSource => {
                let i = (iy * w + ix) * 4;
                let px = &st.source[i..i + 4];
                if px[3] > 0 {
                    let a = f64::from(px[3]);
                    for (k, &c) in px[..3].iter().enumerate() {
                        st.adj.set(k, (f64::from(c) / a).clamp(0.0, 1.0));
                    }
                }
            }
            Pick::None => return,
        }
        st.pick = Pick::None;
        st.dirty = true;
    }

    /// Move the origin or source handle to the canvas point under `p`.
    fn adjust_handle_to(&mut self, p: Pos2) {
        let (cx, cy) = self.view.screen_to_canvas(self.viewport, p);
        let (w, h) = (self.session.doc.width as f64, self.session.doc.height as f64);
        let Some(st) = self.adjust.as_mut() else {
            return;
        };
        let Some((hx, hy)) = st.adj.handle_indices() else {
            return;
        };
        st.adj.set(hx, (cx / w).clamp(0.0, 1.0));
        st.adj.set(hy, (cy / h).clamp(0.0, 1.0));
        st.dirty = true;
    }

    /// Paint to `p` (screen points) with the current brush: the Pencil mask takes the coverage
    /// the brush would lay down (size, opacity, tip and grain), merged by maximum.
    pub(crate) fn adjust_paint_to(&mut self, p: Pos2) {
        let (cx, cy) = self.view.screen_to_canvas(self.viewport, p);
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        if !self.adjust.as_ref().is_some_and(|st| st.paint.is_some()) {
            return;
        }
        let needs_stroke = self.adjust.as_ref().is_some_and(|st| st.cover.is_none());
        let settings = needs_stroke.then(|| self.stroke_settings());
        let Some(st) = self.adjust.as_mut() else {
            return;
        };
        if st.last.is_none() && st.adj.kind == AdjustKind::Clone && !st.painted() {
            // The first stroke fixes the clone offset: source minus stroke start.
            let (sx, sy) = (st.adj.value(0) * w as f64, st.adj.value(1) * h as f64);
            st.adj.set(3, sx - cx);
            st.adj.set(4, sy - cy);
        }
        if let Some(settings) = settings {
            st.cover = Some(CoverageStroke::new(&settings, w, h, 1));
            st.paint_t = 0.0;
        }
        st.paint_t += 1.0 / 120.0;
        let sample = sample_from(RawPointer { x: cx, y: cy, time: st.paint_t, kind: PointerKind::Mouse, force: None });
        let rect = st.cover.as_mut().and_then(|c| c.push(sample));
        if let (Some(rect), Some(cover)) = (rect, st.cover.as_ref()) {
            let paint = Arc::make_mut(st.paint.as_mut().expect("checked above"));
            cover.merge_into(paint, rect);
        }
        st.last = Some((cx, cy));
        st.dirty = true;
    }

    /// The Pencil mask painted so far (`width * height` coverage values), if Pencil is open.
    #[cfg(test)]
    pub(crate) fn adjust_paint_mask(&self) -> Option<&[f32]> {
        self.adjust.as_ref()?.paint.as_ref().map(|p| p.as_slice())
    }

    /// End the Pencil stroke: apply the brush's end taper to the mask.
    fn adjust_end_paint(&mut self) {
        let Some(st) = self.adjust.as_mut() else {
            return;
        };
        let Some(mut cover) = st.cover.take() else {
            return;
        };
        if let (Some(rect), Some(paint)) = (cover.finish(), st.paint.as_mut()) {
            let mask: &mut Vec<f32> = Arc::make_mut(paint);
            cover.merge_into(mask, rect);
            st.dirty = true;
        }
    }

    /// Pointer input while the mode is open. A press on the canvas picks, moves a handle, paints
    /// (Pencil and Clone) or starts a value drag (Layer mode). Always returns after handling, so
    /// nothing paints.
    pub(crate) fn adjust_pointer_frame(&mut self, ctx: &egui::Context, input: &FrameInput) {
        let on_canvas = |app: &Self, p: Pos2| app.viewport.contains(p) && !crate::app::over_ui(ctx, p);
        let Some(st) = self.adjust.as_ref() else {
            return;
        };
        if st.applying() {
            return;
        }
        if !input.primary_down || input.primary_released {
            self.adjust_end_paint();
            if let Some(st) = self.adjust.as_mut() {
                st.dragging = false;
                st.painting = false;
                st.handle_drag = false;
                st.last = None;
            }
            return;
        }
        if input.primary_pressed {
            let origin = input.press_origin.filter(|p| on_canvas(self, *p));
            let Some(p) = origin else {
                return;
            };
            let pick = self.adjust.as_ref().map_or(Pick::None, |s| s.pick);
            if pick != Pick::None {
                self.adjust_pick_at(p);
                return;
            }
            let near_handle = self.adjust_handle_screen().is_some_and(|h| (h - p).length() <= 18.0);
            let paints = self.adjust.as_ref().is_some_and(AdjustState::paints);
            if near_handle {
                if let Some(st) = self.adjust.as_mut() {
                    st.handle_drag = true;
                }
                self.adjust_handle_to(p);
            } else if paints {
                if let Some(st) = self.adjust.as_mut() {
                    st.painting = true;
                    st.last = None;
                }
                self.adjust_paint_to(p);
            } else if let Some(st) = self.adjust.as_mut() {
                st.dragging = true;
            }
            return;
        }
        let Some(current) = input.pointer else {
            return;
        };
        let (handle_drag, painting, dragging) = match self.adjust.as_ref() {
            Some(st) => (st.handle_drag, st.painting, st.dragging),
            None => return,
        };
        if handle_drag {
            self.adjust_handle_to(current);
        } else if painting {
            self.adjust_paint_to(current);
        } else if dragging && input.pointer_delta.x != 0.0 {
            self.adjust_drag_by(input.pointer_delta.x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_shown_as_percent_pixels_choices_or_degrees() {
        assert_eq!(format_value(AdjustKind::GaussianBlur, 0, 0.6), "60%");
        assert_eq!(format_value(AdjustKind::Opacity, 0, 1.0), "100%");
        assert_eq!(format_value(AdjustKind::Hsb, 1, 0.25), "+25%");
        assert_eq!(format_value(AdjustKind::Hsb, 2, -0.4), "-40%");
        assert_eq!(format_value(AdjustKind::Hsb, 0, 0.5), "+90\u{b0}");
        assert_eq!(format_value(AdjustKind::MotionBlur, 0, 12.0), "+12 px");
        assert_eq!(format_value(AdjustKind::Noise, 4, 2.0), "Billowy");
    }
}
