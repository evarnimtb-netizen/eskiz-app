//! A session is the document plus everything that changes it: the stroke engine, the
//! smudge engine and the undo history. The window and `--selftest` both drive this type.
//!
//! Every mutating method is one undoable step with a user-facing name (`undo_name`). Items
//! (layers and groups) are addressed by id. Methods return what the app must redraw, as a
//! [`Refresh`], or `None` when the request was refused (a stroke is running, the layer is
//! locked, the id does not exist, ...).

use std::sync::Arc;

use eskiz_core::blend::BlendMode;
use eskiz_core::canvas::Layer;
use eskiz_core::pixel::Chan;
use eskiz_core::Depth;
use eskiz_core::composite::{composite, composite_rect, composite_rect_premul, composite_to_tiled, CompositeOpts};
use eskiz_core::document::{Document, Location, Mask, Node, NodeKind, Props};
use eskiz_core::geometry::Sample;
use eskiz_core::raster::ShapeTexture;
use eskiz_guides::{DrawingAssist, StrokeAssist};
use eskiz_select::Selection;

use crate::command::{Change, Command, DocOp, FlipCanvas, Placement, Refresh, SelectionStep};
use crate::floodfill::{flood_fill, flood_fill16};
use crate::history::{History, PixelRect};
use crate::paint::{PaintEngine, StrokeSettings};
use crate::smudge::SmudgeEngine;

/// The result of an edit: the id it produced (a new layer or group, if any) and what to redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    /// The new item for add, duplicate and group; the item that moved or changed otherwise.
    pub id: Option<u64>,
    pub refresh: Refresh,
}

/// Initial content of a new layer mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskFill {
    /// All 1: the layer shows fully.
    Reveal,
    /// All 0: the layer is hidden.
    Hide,
}

/// What a property drag is changing (see [`Session::drag_opacity`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragKind {
    Opacity,
    Background,
}

pub struct Session {
    pub doc: Document,
    engine: PaintEngine,
    smudge: SmudgeEngine,
    history: History,
    stroke_count: u32,
    /// The layer of the stroke in progress.
    stroke_layer: Option<u64>,
    /// An open slider drag, so its many changes become one undo step.
    drag: Option<(u64, DragKind)>,
    /// Counts every committed change to the document. Storage compares it with the value at
    /// the last save to know whether there are unsaved edits.
    edits: u64,
    /// Undo steps taken so far (the welcome tour waits for the first).
    undos: u64,
    /// A Pour Fill that is showing on the layer but not yet committed.
    drop_preview: Option<DropPreview>,
    /// Display-only pixels shown in place of one layer (adjustment preview). The layer's stored
    /// pixels are not touched; only [`Session::flatten_display`] and
    /// [`Session::flatten_rect_display`] see this.
    preview: Option<(u64, Layer)>,
    /// Bumped whenever the preview is set or cleared, so a GPU mirror knows to re-upload it.
    preview_gen: u64,
    /// Per item: how many times its pixels or mask changed. See [`Session::layer_epoch`].
    epochs: std::collections::HashMap<u64, u64>,
    /// Bumped when every layer may have changed (new document, canvas-wide operations).
    epoch_all: u64,
    /// Guided Layer for the stroke in progress, when it was started with
    /// [`Session::begin_assisted_stroke`].
    assist: Option<StrokeAssist<'static>>,
    /// The document selection. Shared, so a stroke can hold it without copying the mask.
    selection: Arc<Selection>,
    /// A text edit or style change that is live on the canvas but not yet a history step.
    pub(crate) pending_text: Option<crate::text_tool::PendingText>,
    /// Strokes committed to this document (see [`Session::strokes_done`]).
    strokes_done: u32,
    /// Seconds spent on this document (see [`Session::add_time_spent`]).
    time_spent: f64,
}

/// A Pour Fill applied to the layer for live preview. `before` is the pristine layer (a
/// copy-on-write snapshot, so it holds only the tiles the fill changed as separate memory).
struct DropPreview {
    layer_id: u64,
    rect: PixelRect,
    before: Layer,
}

impl Session {
    pub fn new(doc: Document) -> Self {
        let history = History::for_canvas(doc.width, doc.height, doc.depth);
        Self {
            doc,
            engine: PaintEngine::default(),
            smudge: SmudgeEngine::default(),
            history,
            stroke_count: 0,
            stroke_layer: None,
            drag: None,
            edits: 0,
            undos: 0,
            drop_preview: None,
            preview: None,
            preview_gen: 0,
            epochs: std::collections::HashMap::new(),
            epoch_all: 0,
            assist: None,
            selection: Arc::new(Selection::none()),
            pending_text: None,
            strokes_done: 0,
            time_spent: 0.0,
        }
    }

    /// The current selection (no selection means every pixel is selected).
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Replaces the selection as one undo step named `name`. Refused while stroking.
    pub fn set_selection(&mut self, name: &'static str, sel: Selection) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        if *self.selection == sel {
            return Self::unchanged(None);
        }
        let mut cmd = Command::new(name);
        cmd.selection = Some(self.swap_selection(sel));
        self.finish(cmd, None)
    }

    /// Makes `sel` the selection and returns the step that undoes it. Call once per command.
    fn swap_selection(&mut self, sel: Selection) -> SelectionStep {
        let before = std::mem::replace(&mut self.selection, Arc::new(sel));
        SelectionStep {
            before,
            after: self.selection.clone(),
        }
    }

    /// A number that changes whenever the pixels or the mask of item `id` change (a stroke,
    /// fill, undo, redo, a restored layer, ...). Thumbnails compare it with the value they
    /// were drawn at, so only the layers that really changed are redrawn.
    pub fn layer_epoch(&self, id: u64) -> u64 {
        self.epoch_all + self.epochs.get(&id).copied().unwrap_or(0)
    }

    pub(crate) fn touch(&mut self, id: u64) {
        *self.epochs.entry(id).or_insert(0) += 1;
    }

    fn touch_command(&mut self, cmd: &Command) {
        let mut ids = Vec::new();
        let all = !cmd.touched(&mut ids);
        self.touch_ids(&ids, all);
    }

    fn touch_ids(&mut self, ids: &[u64], all: bool) {
        if all {
            self.epoch_all += 1;
        }
        for &id in ids {
            self.touch(id);
        }
    }

    /// Number of changes made so far (strokes, undo, redo, layer operations, replacing the
    /// document). It only ever grows.
    pub fn edits(&self) -> u64 {
        self.edits
    }

    /// Number of undo steps taken so far.
    pub fn undo_count(&self) -> u64 {
        self.undos
    }

    /// Record a change made outside the session's own methods.
    pub fn mark_edited(&mut self) {
        self.edits += 1;
    }

    /// Replace the document. Any stroke in progress is discarded and undo history is cleared.
    pub fn replace_document(&mut self, doc: Document) {
        self.engine = PaintEngine::default();
        self.smudge = SmudgeEngine::default();
        self.history.clear();
        self.stroke_layer = None;
        self.drag = None;
        self.drop_preview = None;
        self.preview = None;
        self.preview_gen += 1;
        self.pending_text = None;
        self.doc = doc;
        self.fit_history_budget();
        self.strokes_done = 0;
        self.time_spent = 0.0;
        self.selection = Arc::new(Selection::none());
        self.epoch_all += 1;
        self.edits += 1;
    }

    pub fn is_stroking(&self) -> bool {
        self.engine.is_active() || self.smudge.is_active()
    }

    /// The layer the stroke in progress paints on (it can differ from the active layer if that
    /// changed after pen-down).
    pub fn stroke_layer(&self) -> Option<u64> {
        self.stroke_layer
    }

    // ----- painting -----

    /// Can this item take paint? It must be a raster layer, shown (with every group above it
    /// shown) and not locked (nor inside a locked group).
    pub fn can_paint(&self, id: u64) -> bool {
        self.doc.node(id).is_some_and(|n| !n.is_group())
            && !self.doc.texts.contains_key(&id)
            && self.doc.layer(id).is_some()
            && self.doc.effective_visible(id)
            && !self.doc.effective_locked(id)
    }

    /// Start a stroke on layer `id`. Returns false if the layer cannot be painted (see
    /// [`Session::can_paint`]) or a stroke is already running. The layer's lock transparency is honoured.
    pub fn begin_stroke(&mut self, id: u64, settings: &StrokeSettings) -> bool {
        if self.is_stroking() || !self.can_paint(id) {
            return false;
        }
        let alpha_lock = self.doc.node(id).is_some_and(|n| n.props.alpha_lock);
        let Some(layer) = self.doc.layer(id) else {
            return false;
        };
        self.stroke_count = self.stroke_count.wrapping_add(1);
        self.assist = None;
        let started = self.engine.begin(layer, id, settings, self.stroke_count, alpha_lock);
        if started {
            self.engine.set_clip(self.selection.clone());
            self.stroke_layer = Some(id);
            self.drag = None;
        }
        started
    }

    /// Start a stroke with Guided Layer. Each input sample is snapped and mirrored by
    /// `assist`, and every output copy paints into this one stroke, so the whole stroke is one
    /// undo step. Refused in the same cases as [`Session::begin_stroke`].
    pub fn begin_assisted_stroke(&mut self, id: u64, settings: &StrokeSettings, assist: &DrawingAssist) -> bool {
        if self.is_stroking() || !self.can_paint(id) {
            return false;
        }
        let alpha_lock = self.doc.node(id).is_some_and(|n| n.props.alpha_lock);
        let Some(layer) = self.doc.layer(id) else {
            return false;
        };
        self.stroke_count = self.stroke_count.wrapping_add(1);
        let paths = assist.output_count();
        let started = self.engine.begin_paths(layer, id, settings, self.stroke_count, alpha_lock, paths);
        if started {
            self.engine.set_clip(self.selection.clone());
            self.stroke_layer = Some(id);
            self.drag = None;
            self.assist = Some(assist.begin_stroke_owned());
        }
        started
    }

    /// Tell the engine how the canvas is turned on screen, so pen azimuth (tilt-aligned tips)
    /// is measured on the canvas. Pass minus the view rotation, in radians.
    pub fn set_canvas_rotation(&mut self, radians: f64) {
        self.engine.canvas_rotation = radians;
    }

    /// Start a smudge stroke. `radius` is in canvas pixels, `strength` (0..1) is the opacity.
    /// `tip` is the brush tip of a library brush, or `None`. The layer's lock transparency is honoured.
    pub fn begin_smudge(&mut self, id: u64, radius: f64, strength: f64, tip: Option<Arc<ShapeTexture>>) -> bool {
        if self.is_stroking() || !self.can_paint(id) {
            return false;
        }
        let alpha_lock = self.doc.node(id).is_some_and(|n| n.props.alpha_lock);
        let Some(layer) = self.doc.layer(id) else {
            return false;
        };
        self.smudge.set_clip(self.selection.clone());
        let started = self.smudge.begin_with_tip(layer, id, radius, strength, alpha_lock, tip);
        if started {
            self.stroke_layer = Some(id);
            self.drag = None;
        }
        started
    }

    /// Feed one sample of the stroke in progress. Returns the rectangle to refresh.
    pub fn push(&mut self, sample: Sample) -> Option<PixelRect> {
        let id = self.stroke_layer?;
        let layer = self.doc.layer_mut(id)?;
        if self.smudge.is_active() {
            return self.smudge.push(layer, sample.x, sample.y);
        }
        if let Some(assist) = self.assist.as_mut() {
            let guided = assist.push(crate::guides::to_guide_sample(&sample));
            let samples = crate::guides::to_core_samples(&sample, &guided);
            return self.engine.push_paths(layer, &samples);
        }
        self.engine.push(layer, sample)
    }

    /// Commit the stroke and record it in the undo history. Returns the rectangle to refresh.
    pub fn end_stroke(&mut self) -> Option<PixelRect> {
        self.assist = None;
        let id = self.stroke_layer.take()?;
        let layer = self.doc.layer_mut(id)?;
        let cmd = if self.smudge.is_active() {
            self.smudge.end(layer)?
        } else {
            self.engine.end(layer)?
        };
        let rect = match cmd.refresh() {
            Refresh::Rect(r) => Some(r),
            _ => None,
        };
        self.touch(id);
        self.strokes_done = self.strokes_done.saturating_add(1);
        self.push_history(cmd);
        self.edits += 1;
        rect
    }

    /// Abandon the stroke in progress. Returns the rectangle to refresh.
    pub fn cancel_stroke(&mut self) -> Option<PixelRect> {
        self.assist = None;
        let id = self.stroke_layer.take()?;
        let layer = self.doc.layer_mut(id)?;
        if self.smudge.is_active() {
            return self.smudge.cancel(layer);
        }
        self.engine.cancel(layer)
    }

    // ----- undo and redo -----

    /// Undo the newest step. Returns what to refresh (`None` when there is nothing to undo).
    pub fn undo(&mut self) -> Option<Refresh> {
        if self.is_stroking() {
            return None;
        }
        self.text_commit();
        self.drag = None;
        let cmd = self.history.undo()?;
        self.edits += 1;
        self.undos += 1;
        cmd.undo(&mut self.doc);
        if let Some(step) = &cmd.selection {
            self.selection = step.before.clone();
        }
        let refresh = cmd.refresh();
        let mut ids = Vec::new();
        let all = !cmd.touched(&mut ids);
        self.touch_ids(&ids, all);
        Some(refresh)
    }

    /// Redo the newest undone step. Returns what to refresh.
    pub fn redo(&mut self) -> Option<Refresh> {
        if self.is_stroking() {
            return None;
        }
        self.text_commit();
        self.drag = None;
        let cmd = self.history.redo()?;
        self.edits += 1;
        cmd.redo(&mut self.doc);
        if let Some(step) = &cmd.selection {
            self.selection = step.after.clone();
        }
        let refresh = cmd.refresh();
        let mut ids = Vec::new();
        let all = !cmd.touched(&mut ids);
        self.touch_ids(&ids, all);
        Some(refresh)
    }

    pub fn can_undo(&self) -> bool {
        !self.is_stroking() && self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        !self.is_stroking() && self.history.can_redo()
    }

    /// Name of the step that undo would revert ("Stroke", "Add Layer", ...).
    pub fn undo_name(&self) -> Option<&'static str> {
        self.history.next_undo().map(|c| c.name)
    }

    /// Name of the step that redo would re-apply.
    pub fn redo_name(&self) -> Option<&'static str> {
        self.history.next_redo().map(|c| c.name)
    }

    pub fn undo_steps(&self) -> usize {
        self.history.undo_len()
    }

    /// The undo byte budget for this document (see [`History::bytes_cap_for`]).
    pub fn undo_budget(&self) -> usize {
        self.history.max_bytes()
    }

    /// Pixel bytes the undo and redo stacks hold, each distinct tile counted once.
    pub fn undo_bytes(&self) -> usize {
        self.history.bytes()
    }

    // ----- command plumbing -----

    /// Size the undo byte budget for the current canvas (it changes with a resize or a depth
    /// conversion, and a new document brings its own).
    fn fit_history_budget(&mut self) {
        let cap = History::bytes_cap_for(self.doc.width, self.doc.height, self.doc.depth, crate::history::physical_memory());
        self.history.set_max_bytes(cap);
    }

    fn push_history(&mut self, cmd: Command) {
        self.fit_history_budget();
        self.history.push(cmd);
    }

    /// Record a finished command and report what it changed.
    pub(crate) fn finish(&mut self, cmd: Command, id: Option<u64>) -> Option<Edit> {
        self.drag = None;
        let refresh = cmd.refresh();
        self.touch_command(&cmd);
        if !cmd.is_empty() {
            self.push_history(cmd);
            self.edits += 1;
        }
        Some(Edit { id, refresh })
    }

    pub(crate) fn idle(&self) -> bool {
        !self.is_stroking()
    }

    /// A refused-nothing-changed result: the request was valid but changed nothing.
    fn unchanged(id: Option<u64>) -> Option<Edit> {
        Some(Edit {
            id,
            refresh: Refresh::None,
        })
    }

    // ----- layer operations -----

    /// Empty a layer. One undo step ("Clear Layer"). An alpha-locked layer keeps its alpha and
    /// only loses its colour. Refused for locked layers.
    pub fn clear_layer(&mut self, id: u64) -> Option<Edit> {
        if !self.idle() || !self.can_edit_pixels(id) {
            return None;
        }
        let before = self.doc.layer(id)?.clone();
        let mut after = if self.doc.node(id)?.props.alpha_lock {
            let mut a = before.clone();
            if a.depth() == Depth::U16 {
                a.map_allocated16(|p| [0, 0, 0, p[3]]);
            } else {
                a.map_allocated(|p| [0, 0, 0, p[3]]);
            }
            a.compact();
            a.share_unchanged_with(&before);
            a
        } else {
            self.doc.blank_layer()
        };
        if self.selection.is_active() {
            let (w, h) = (before.width, before.height);
            let full = PixelRect { x: 0, y: 0, w, h };
            after = if before.depth() == Depth::U16 {
                let mut px = after.to_rgba16();
                clip_to_selection(&before.to_rgba16(), &mut px, full, &self.selection);
                Layer::from_rgba16(w, h, px)
            } else {
                let mut px = after.to_rgba();
                clip_to_selection(&before.to_rgba(), &mut px, full, &self.selection);
                Layer::from_rgba(w, h, px)
            };
            after.share_unchanged_with(&before);
        }
        if before == after {
            return Self::unchanged(Some(id));
        }
        after.visible = before.visible;
        let mut cmd = Command::new("Clear Layer");
        cmd.apply(&mut self.doc, Change::LayerPixels { layer: id, before, after });
        self.finish(cmd, Some(id))
    }

    /// Fill every pixel of a layer with `colour`, as one "Fill Layer" step. An alpha-locked layer
    /// keeps its alpha and only takes the colour. Refused for locked layers.
    pub fn fill_layer(&mut self, id: u64, colour: [u8; 3]) -> Option<Edit> {
        if !self.idle() || !self.can_edit_pixels(id) {
            return None;
        }
        let layer = self.doc.layer(id)?;
        let alpha_lock = self.doc.node(id)?.props.alpha_lock;
        if layer.depth() == Depth::U16 {
            // Same rule at 16 bits: the 8-bit colour widens exactly (`v * 257`).
            let source = layer.to_rgba16();
            let mut pixels = vec![0u16; source.len()];
            for (dst, src) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(source.as_chunks::<4>().0.iter()) {
                let a = if alpha_lock { src[3] } else { 65535 };
                for c in 0..3 {
                    dst[c] = (u64::from(colour[c]) * 257 * u64::from(a) / 65535) as u16;
                }
                dst[3] = a;
            }
            if self.selection.is_active() {
                let full = PixelRect { x: 0, y: 0, w: layer.width, h: layer.height };
                clip_to_selection(&source, &mut pixels, full, &self.selection);
            }
            let after = Layer::from_rgba16(layer.width, layer.height, pixels);
            return self.replace_layer(id, after, "Fill Layer");
        }
        let source = layer.to_rgba();
        let mut pixels = vec![0u8; source.len()];
        for (dst, src) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(source.as_chunks::<4>().0.iter()) {
            let a = if alpha_lock { src[3] } else { 255 };
            for c in 0..3 {
                dst[c] = (u32::from(colour[c]) * u32::from(a) / 255) as u8;
            }
            dst[3] = a;
        }
        if self.selection.is_active() {
            let full = PixelRect { x: 0, y: 0, w: layer.width, h: layer.height };
            clip_to_selection(&source, &mut pixels, full, &self.selection);
        }
        self.replace_layer_pixels(id, pixels, "Fill Layer")
    }

    /// Replace a whole layer's pixels (filters, flood fill, transforms). `pixels` is
    /// premultiplied RGBA8 of the canvas size. The caller is responsible for alpha-lock rules.
    /// Refused for locked layers or a wrong-sized buffer. The undo step keeps only the tiles
    /// whose pixels changed. See [`Session::replace_layer`] to pass tiles instead of a buffer.
    pub fn replace_layer_pixels(&mut self, id: u64, pixels: Vec<u8>, name: &'static str) -> Option<Edit> {
        if !self.idle() || !self.can_edit_pixels(id) {
            return None;
        }
        let layer = self.doc.layer(id)?;
        if pixels.len() != layer.width * layer.height * 4 {
            return None;
        }
        let after = Layer::from_rgba(layer.width, layer.height, pixels);
        self.replace_layer(id, after, name)
    }

    /// [`Session::replace_layer_pixels`] with a ready-made tiled layer of the canvas size.
    pub fn replace_layer(&mut self, id: u64, after: Layer, name: &'static str) -> Option<Edit> {
        self.replace_layer_and_selection(id, after, name, None)
    }

    /// Replace a layer's pixels and, when `selection` is given, the selection too. One undo
    /// step named `name` (Transform, Cut, Paste, ...). Nothing changes if neither differs.
    pub fn replace_layer_and_selection(
        &mut self,
        id: u64,
        mut after: Layer,
        name: &'static str,
        selection: Option<Selection>,
    ) -> Option<Edit> {
        if !self.idle() || !self.can_edit_pixels(id) {
            return None;
        }
        let before = self.doc.layer(id)?.clone();
        if (after.width, after.height) != (before.width, before.height) {
            return None;
        }
        // Tools that work on 8-bit buffers hand back an 8-bit layer; the document keeps its depth.
        if after.depth() != before.depth() {
            after = after.converted(before.depth(), false);
        }
        after.share_unchanged_with(&before);
        let pixels_changed = before != after;
        if !pixels_changed && selection.is_none() {
            return Self::unchanged(Some(id));
        }
        let mut cmd = Command::new(name);
        if pixels_changed {
            cmd.apply(&mut self.doc, Change::LayerPixels { layer: id, before, after });
        }
        if let Some(sel) = selection {
            cmd.selection = Some(self.swap_selection(sel));
        }
        self.finish(cmd, Some(id))
    }

    /// A raster layer whose pixels may be edited: it exists and is not locked.
    pub fn can_edit_pixels(&self, id: u64) -> bool {
        self.doc.node(id).is_some_and(|n| !n.is_group())
            && !self.doc.texts.contains_key(&id)
            && self.doc.layer(id).is_some()
            && !self.doc.effective_locked(id)
    }

    /// Add an empty layer directly above item `above` (in the same group), or on top of the
    /// stack when `None`. Returns the new layer's id in `Edit::id`.
    pub fn add_layer(&mut self, above: Option<u64>) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let id = self.doc.add_layer_above(above);
        let placement = Placement::capture(&self.doc, id)?;
        let mut cmd = Command::new("Add Layer");
        cmd.changes.push(Change::Insert(placement));
        self.finish(cmd, Some(id))
    }

    /// Add a new layer above item `above` holding `pixels` (premultiplied RGBA8, canvas sized),
    /// named `name`. One undo step ("Import Image"). The new layer's id is in `Edit::id`.
    pub fn import_layer(&mut self, above: Option<u64>, name: &str, pixels: Vec<u8>) -> Option<Edit> {
        self.import_layer_named(above, name, pixels, "Import Image")
    }

    /// [`Session::import_layer`] with the undo step named `step` ("Paste", "Duplicate", ...).
    pub fn import_layer_named(&mut self, above: Option<u64>, name: &str, pixels: Vec<u8>, step: &'static str) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let expected = self.doc.width * self.doc.height * 4;
        if pixels.len() != expected {
            return None;
        }
        let id = self.doc.add_layer_above(above);
        if let Some(node) = self.doc.node_mut(id) {
            node.props.name = name.to_owned();
        }
        let (w, h) = (self.doc.width, self.doc.height);
        let depth = self.doc.depth;
        if let Some(layer) = self.doc.layer_mut(id) {
            *layer = Layer::from_rgba(w, h, pixels).converted(depth, false);
        }
        let placement = Placement::capture(&self.doc, id)?;
        let mut cmd = Command::new(step);
        cmd.changes.push(Change::Insert(placement));
        self.finish(cmd, Some(id))
    }

    /// Delete a layer or group (and its children). The last raster layer cannot be deleted.
    /// Undo restores it with its pixels, settings and position.
    pub fn delete_layer(&mut self, id: u64) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let node = self.doc.node(id)?;
        let mut ids = Vec::new();
        node.layer_ids(&mut ids);
        if ids.len() >= self.doc.layer_count() {
            return None;
        }
        let name = if node.is_group() { "Delete Group" } else { "Delete Layer" };
        let placement = Placement::capture(&self.doc, id)?;
        let mut cmd = Command::new(name);
        cmd.apply(&mut self.doc, Change::Delete(placement));
        self.finish(cmd, None)
    }

    /// Copy a layer or group (pixels, mask and settings) directly above the original.
    pub fn duplicate_layer(&mut self, id: u64) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let loc = self.doc.locate(id)?;
        let is_group = self.doc.node(id)?.is_group();
        let (mut node, pixels, texts) = self.doc.duplicate_subtree_with_texts(id)?;
        node.props.name = format!("{} copy", node.props.name);
        let new_id = node.id;
        let mut placement = Placement::new(loc.parent, loc.index + 1, node, pixels);
        placement.texts = texts;
        let mut cmd = Command::new(if is_group { "Duplicate Group" } else { "Duplicate Layer" });
        cmd.apply(&mut self.doc, Change::Insert(placement));
        self.finish(cmd, Some(new_id))
    }

    /// Swap an item with its neighbour above (`up`) or below, inside the same group.
    pub fn move_layer(&mut self, id: u64, up: bool) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let loc = self.doc.locate(id)?;
        let len = self.doc.children(loc.parent)?.len();
        let index = if up {
            (loc.index + 1 < len).then_some(loc.index + 1)?
        } else {
            loc.index.checked_sub(1)?
        };
        self.move_item(id, loc.parent, index)
    }

    /// Move an item to `index` inside `parent` (`None` = top level). `index` counts the
    /// destination's items with this item taken out; it is clamped. A group cannot move into
    /// itself or its own descendants.
    pub fn move_item(&mut self, id: u64, parent: Option<u64>, index: usize) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let from = self.doc.locate(id)?;
        if let Some(p) = parent {
            if p == id || self.doc.is_ancestor(id, p) || !self.doc.node(p)?.is_group() {
                return None;
            }
        }
        let mut len = self.doc.children(parent)?.len();
        if from.parent == parent {
            len -= 1;
        }
        let to = Location {
            parent,
            index: index.min(len),
        };
        if to == from {
            return Self::unchanged(Some(id));
        }
        let mut cmd = Command::new("Move Layer");
        cmd.apply(&mut self.doc, Change::Move { id, from, to });
        self.finish(cmd, Some(id))
    }

    /// Move an item to the top of group `group`.
    pub fn move_into_group(&mut self, id: u64, group: u64) -> Option<Edit> {
        let len = self.doc.children(Some(group))?.len();
        self.move_item(id, Some(group), len)
    }

    /// Merge layer `id` into the raster layer directly below it. The lower layer
    /// keeps its name and blend mode, and ends with opacity 1, visible and no mask. Refused when
    /// the item below is a group or missing, or when either layer is locked.
    pub fn merge_down(&mut self, id: u64) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let loc = self.doc.locate(id)?;
        let lower_index = loc.index.checked_sub(1)?;
        let lower_id = self.doc.children(loc.parent)?.get(lower_index)?.id;
        let upper = self.doc.node(id)?;
        let lower = self.doc.node(lower_id)?;
        if upper.is_group() || lower.is_group() || upper.props.locked || lower.props.locked {
            return None;
        }

        // Composite the pair on its own, without the background.
        let mut tmp = Document::empty(self.doc.width, self.doc.height);
        let mut lower_props = lower.props.clone();
        let mut upper_props = upper.props.clone();
        lower_props.visible = true;
        if lower_props.clipped {
            // The lower layer's own base is not part of the merge.
            upper_props.clipped = false;
        }
        lower_props.clipped = false;
        tmp.push_raster(None, lower_props, lower.mask.clone(), self.doc.layer(lower_id)?.clone());
        tmp.push_raster(None, upper_props, upper.mask.clone(), self.doc.layer(id)?.clone());
        let mut after_pixels = composite_to_tiled(&tmp, false);

        let before_pixels = self.doc.layer(lower_id)?.clone();
        after_pixels.share_unchanged_with(&before_pixels);
        let before_props = lower.props.clone();
        let mut after_props = before_props.clone();
        after_props.opacity = 1.0;
        after_props.visible = true;
        let before_mask = lower.mask.clone();
        let upper_placement = Placement::capture(&self.doc, id)?;

        let mut cmd = Command::new("Merge Down");
        cmd.apply(
            &mut self.doc,
            Change::LayerPixels {
                layer: lower_id,
                before: before_pixels,
                after: after_pixels,
            },
        );
        cmd.apply(
            &mut self.doc,
            Change::Props {
                id: lower_id,
                before: before_props,
                after: after_props,
            },
        );
        if before_mask.is_some() {
            cmd.apply(
                &mut self.doc,
                Change::Mask {
                    id: lower_id,
                    before: before_mask,
                    after: None,
                },
            );
        }
        cmd.apply(&mut self.doc, Change::Delete(upper_placement));
        self.finish(cmd, Some(lower_id))
    }

    /// Wrap the given items in a new group placed directly above the topmost of them. The items
    /// must share one parent; they keep their relative order. Returns the group id.
    pub fn group_items(&mut self, ids: &[u64]) -> Option<Edit> {
        self.group_named(ids, "Group")
    }

    /// Put item `id` and the item directly below it (same parent) into a new group, as one
    /// "Combine Down" step. The new group
    /// is selected. Refused for the bottom item.
    pub fn combine_down(&mut self, id: u64) -> Option<Edit> {
        let lower = self.item_below(id)?;
        self.group_named(&[lower, id], "Combine Down")
    }

    /// The item directly below `id` in the same parent.
    pub fn item_below(&self, id: u64) -> Option<u64> {
        let loc = self.doc.locate(id)?;
        let lower = loc.index.checked_sub(1)?;
        Some(self.doc.children(loc.parent)?.get(lower)?.id)
    }

    /// Would [`Session::merge_down`] accept `id`?
    pub fn can_merge_down(&self, id: u64) -> bool {
        let Some(lower) = self.item_below(id) else {
            return false;
        };
        match (self.doc.node(id), self.doc.node(lower)) {
            (Some(u), Some(l)) => !u.is_group() && !l.is_group() && !u.props.locked && !l.props.locked,
            _ => false,
        }
    }

    fn group_named(&mut self, ids: &[u64], name: &'static str) -> Option<Edit> {
        if !self.idle() || ids.is_empty() {
            return None;
        }
        let first = self.doc.locate(ids[0])?;
        let mut members: Vec<(usize, u64)> = Vec::new();
        for &id in ids {
            let loc = self.doc.locate(id)?;
            if loc.parent != first.parent || members.iter().any(|m| m.1 == id) {
                return None;
            }
            members.push((loc.index, id));
        }
        members.sort_unstable();
        let top = members.last()?.0;
        let gid = self.doc.alloc_id();
        let node = Node {
            id: gid,
            props: Props::new(format!("Group {gid}")),
            mask: None,
            kind: NodeKind::Group {
                children: Vec::new(),
                collapsed: false,
            },
        };
        let mut cmd = Command::new(name);
        cmd.apply(
            &mut self.doc,
            Change::Insert(Placement::new(first.parent, top + 1, node, Vec::new())),
        );
        for (k, (_, id)) in members.iter().enumerate() {
            let from = self.doc.locate(*id)?;
            cmd.apply(
                &mut self.doc,
                Change::Move {
                    id: *id,
                    from,
                    to: Location {
                        parent: Some(gid),
                        index: k,
                    },
                },
            );
        }
        self.finish(cmd, Some(gid))
    }

    /// Dissolve a group: its children move into the parent at the group's position. The group's
    /// own opacity, blend mode and mask are dropped. Returns the topmost child's id.
    pub fn ungroup(&mut self, id: u64) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let loc = self.doc.locate(id)?;
        let node = self.doc.node(id)?;
        if !node.is_group() {
            return None;
        }
        let children: Vec<u64> = node.children().iter().map(|c| c.id).collect();
        let mut cmd = Command::new("Ungroup");
        for child in &children {
            let from = self.doc.locate(*child)?;
            let group_index = self.doc.locate(id)?.index;
            cmd.apply(
                &mut self.doc,
                Change::Move {
                    id: *child,
                    from,
                    to: Location {
                        parent: loc.parent,
                        index: group_index,
                    },
                },
            );
        }
        let empty = Placement::capture(&self.doc, id)?;
        cmd.apply(&mut self.doc, Change::Delete(empty));
        self.finish(cmd, children.last().copied())
    }

    /// Expand or collapse a group in the layers panel. UI state: not undoable.
    pub fn set_collapsed(&mut self, id: u64, collapsed: bool) {
        if let Some(Node {
            kind: NodeKind::Group { collapsed: c, .. },
            ..
        }) = self.doc.node_mut(id)
        {
            *c = collapsed;
        }
    }

    // ----- Pour Fill -----

    /// Pour Fill in one step: flood-fill the region under `seed` on layer `id` and record one
    /// undo step named "Pour Fill". Returns the rectangle to refresh.
    pub fn pour_fill(&mut self, id: u64, seed: (i64, i64), threshold: f32, colour: [u8; 3]) -> Option<PixelRect> {
        let preview = self.pour_fill_preview(id, seed, threshold, colour);
        let committed = self.pour_fill_commit();
        committed.or(preview)
    }

    /// Show a Pour Fill on layer `id` without recording it. Any earlier preview is replaced.
    /// Refused (apart from removing the old preview) for hidden or locked layers. An
    /// alpha-locked layer keeps its alpha and only takes the colour. Returns the rectangle to
    /// refresh (the union of the old and new previews).
    pub fn pour_fill_preview(&mut self, id: u64, seed: (i64, i64), threshold: f32, colour: [u8; 3]) -> Option<PixelRect> {
        let restored = self.pour_fill_cancel();
        if self.is_stroking() || !self.can_paint(id) {
            return restored;
        }
        let alpha_lock = self.doc.node(id)?.props.alpha_lock;
        let layer = self.doc.layer_mut(id)?;
        let before = layer.clone();
        let rect = if layer.depth() == Depth::U16 {
            let Some(mut fill) = flood_fill16(layer, seed, threshold, colour) else {
                return restored;
            };
            let rect = fill.rect;
            if alpha_lock {
                eskiz_core::filters::keep_alpha_t(&layer.read_rect16(rect.x, rect.y, rect.w, rect.h), &mut fill.pixels);
            }
            if self.selection.is_active() {
                let old = layer.read_rect16(rect.x, rect.y, rect.w, rect.h);
                clip_to_selection(&old, &mut fill.pixels, rect, &self.selection);
            }
            layer.write_rect16(rect.x, rect.y, rect.w, rect.h, &fill.pixels);
            rect
        } else {
            let Some(mut fill) = flood_fill(layer, seed, threshold, colour) else {
                return restored;
            };
            let rect = fill.rect;
            if alpha_lock {
                keep_alpha(&layer.read_rect(rect.x, rect.y, rect.w, rect.h), &mut fill.pixels);
            }
            if self.selection.is_active() {
                let old = layer.read_rect(rect.x, rect.y, rect.w, rect.h);
                clip_to_selection(&old, &mut fill.pixels, rect, &self.selection);
            }
            layer.write_rect(rect.x, rect.y, rect.w, rect.h, &fill.pixels);
            rect
        };
        self.touch(id);
        self.drop_preview = Some(DropPreview {
            layer_id: id,
            rect,
            before,
        });
        Some(restored.map_or(rect, |r| r.union(rect)))
    }

    /// Turn the Pour Fill preview into one undo step ("Pour Fill").
    pub fn pour_fill_commit(&mut self) -> Option<PixelRect> {
        let p = self.drop_preview.take()?;
        let layer = self.doc.layer(p.layer_id)?;
        let change = Change::tiles_between(p.layer_id, &p.before, layer, p.rect);
        self.push_history(Command::single("Pour Fill", change));
        self.edits += 1;
        self.drag = None;
        Some(p.rect)
    }

    /// Remove the Pour Fill preview, restoring the layer. Returns the rectangle to refresh.
    pub fn pour_fill_cancel(&mut self) -> Option<PixelRect> {
        let p = self.drop_preview.take()?;
        let layer = self.doc.layer_mut(p.layer_id)?;
        layer.copy_tiles_in_rect_from(&p.before, p.rect.x, p.rect.y, p.rect.w, p.rect.h);
        self.touch(p.layer_id);
        Some(p.rect)
    }

    pub fn has_pour_fill_preview(&self) -> bool {
        self.drop_preview.is_some()
    }

    // ----- properties -----

    /// Change an item's properties as one named step. Nothing is recorded when nothing changed.
    pub fn set_props(&mut self, id: u64, name: &'static str, f: impl FnOnce(&mut Props)) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let before = self.doc.node(id)?.props.clone();
        let mut after = before.clone();
        f(&mut after);
        after.opacity = if after.opacity.is_finite() { after.opacity.clamp(0.0, 1.0) } else { before.opacity };
        after.name = after.name.chars().take(64).collect();
        if after == before {
            return Self::unchanged(Some(id));
        }
        let mut cmd = Command::new(name);
        cmd.apply(&mut self.doc, Change::Props { id, before, after });
        self.finish(cmd, Some(id))
    }

    pub fn set_name(&mut self, id: u64, name: &str) -> Option<Edit> {
        self.set_props(id, "Rename Layer", |p| p.name = name.to_owned())
    }

    pub fn set_visible(&mut self, id: u64, visible: bool) -> Option<Edit> {
        self.set_props(id, if visible { "Show Layer" } else { "Hide Layer" }, |p| p.visible = visible)
    }

    pub fn set_opacity(&mut self, id: u64, opacity: f64) -> Option<Edit> {
        self.set_props(id, "Layer Opacity", |p| p.opacity = opacity)
    }

    pub fn set_blend(&mut self, id: u64, blend: BlendMode) -> Option<Edit> {
        self.set_props(id, "Layer Blend Mode", |p| p.blend = blend)
    }

    pub fn set_clipped(&mut self, id: u64, clipped: bool) -> Option<Edit> {
        self.set_props(id, if clipped { "Clip to Layer Below" } else { "Release Clip to Layer Below" }, |p| p.clipped = clipped)
    }

    pub fn set_alpha_lock(&mut self, id: u64, on: bool) -> Option<Edit> {
        self.set_props(id, if on { "Lock Transparency" } else { "Unlock Transparency" }, |p| p.alpha_lock = on)
    }

    pub fn set_locked(&mut self, id: u64, locked: bool) -> Option<Edit> {
        self.set_props(id, if locked { "Lock Layer" } else { "Unlock Layer" }, |p| p.locked = locked)
    }

    pub fn set_private(&mut self, id: u64, private: bool) -> Option<Edit> {
        self.set_props(id, if private { "Private Layer" } else { "Public Layer" }, |p| p.private = private)
    }

    /// Change opacity continuously (a slider drag). The first call starts one "Layer Opacity"
    /// undo step; further calls for the same item update it in place until [`Session::end_drag`]
    /// or any other edit.
    pub fn drag_opacity(&mut self, id: u64, opacity: f64) -> Option<Edit> {
        let opacity = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { return None };
        if self.drag == Some((id, DragKind::Opacity)) {
            if let Some(Change::Props { after, .. }) = self.history.last_undo_mut().and_then(|c| c.changes.first_mut()) {
                after.opacity = opacity;
                self.edits += 1;
                if let Some(n) = self.doc.node_mut(id) {
                    n.props.opacity = opacity;
                }
                return Some(Edit {
                    id: Some(id),
                    refresh: Refresh::Everything,
                });
            }
        }
        let edit = self.set_opacity(id, opacity)?;
        if self.history.can_undo() && edit.refresh != Refresh::None {
            self.drag = Some((id, DragKind::Opacity));
        }
        Some(edit)
    }

    /// Change the background colour continuously (a colour picker drag). Like
    /// [`Session::drag_opacity`]: one "Background Colour" step until [`Session::end_drag`].
    pub fn drag_background(&mut self, colour: [f64; 3]) -> Option<Edit> {
        let colour = colour.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        if self.drag == Some((0, DragKind::Background)) {
            if let Some(Change::Background { after, .. }) = self.history.last_undo_mut().and_then(|c| c.changes.first_mut()) {
                after.0 = colour;
                self.edits += 1;
                self.doc.background = colour;
                return Some(Edit {
                    id: None,
                    refresh: Refresh::Everything,
                });
            }
        }
        let edit = self.set_background_colour(colour)?;
        if self.history.can_undo() && edit.refresh != Refresh::None {
            self.drag = Some((0, DragKind::Background));
        }
        Some(edit)
    }

    /// Close a property drag, so the next change starts a new step.
    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    // ----- masks -----

    /// Add a mask to a layer or group. Refused if it already has one.
    pub fn add_mask(&mut self, id: u64, fill: MaskFill) -> Option<Edit> {
        let value = match fill {
            MaskFill::Reveal => 255,
            MaskFill::Hide => 0,
        };
        let mask = Mask::filled(self.doc.width, self.doc.height, value);
        if self.doc.node(id)?.mask.is_some() {
            return None;
        }
        self.set_mask(id, Some(mask), "Add Mask")
    }

    /// Remove a layer's or group's mask. The layer's pixels are unchanged.
    pub fn remove_mask(&mut self, id: u64) -> Option<Edit> {
        self.doc.node(id)?.mask.as_ref()?;
        self.set_mask(id, None, "Delete Mask")
    }

    /// Replace a mask (or remove it with `None`) as one undo step called `name`.
    pub fn set_mask(&mut self, id: u64, mask: Option<Mask>, name: &'static str) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let before = self.doc.node(id)?.mask.clone();
        if let Some(m) = &mask {
            if m.data.len() != self.doc.width * self.doc.height {
                return None;
            }
        }
        if before == mask {
            return Self::unchanged(Some(id));
        }
        let mut cmd = Command::new(name);
        cmd.apply(&mut self.doc, Change::Mask { id, before, after: mask });
        self.finish(cmd, Some(id))
    }

    // ----- background and document-wide -----

    /// Change the background colour (RGB 0..1).
    pub fn set_background_colour(&mut self, colour: [f64; 3]) -> Option<Edit> {
        self.set_background(colour, self.doc.background_visible, "Background Color")
    }

    pub fn set_background_visible(&mut self, visible: bool) -> Option<Edit> {
        self.set_background(
            self.doc.background,
            visible,
            if visible { "Show Background" } else { "Hide Background" },
        )
    }

    fn set_background(&mut self, colour: [f64; 3], visible: bool, name: &'static str) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let colour = colour.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        let before = (self.doc.background, self.doc.background_visible);
        let after = (colour, visible);
        if before == after {
            return Self::unchanged(None);
        }
        let mut cmd = Command::new(name);
        cmd.apply(&mut self.doc, Change::Background { before, after });
        self.finish(cmd, None)
    }

    /// Apply a whole-canvas operation (crop, resize) and the selection change that goes with it
    /// as one undo step named `name`. `selection` is the new selection, or `None` to keep the
    /// current one. Refused while stroking.
    pub fn canvas_op(&mut self, name: &'static str, op: Box<dyn DocOp>, selection: Option<Selection>) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        self.text_commit();
        let _ = self.pour_fill_commit();
        let mut cmd = Command::new(name);
        if let Some(sel) = selection {
            cmd.selection = Some(self.swap_selection(sel));
        }
        cmd.apply(&mut self.doc, Change::Doc(op));
        self.finish(cmd, None)
    }

    /// Strokes committed to this document since it was opened or replaced (smudges included).
    pub fn strokes_done(&self) -> u32 {
        self.strokes_done
    }

    /// Seconds spent on this document since it was opened or replaced.
    pub fn time_spent(&self) -> f64 {
        self.time_spent
    }

    /// Add to the time spent on the document. Not part of undo.
    pub fn add_time_spent(&mut self, seconds: f64) {
        self.time_spent += seconds.max(0.0);
    }

    /// Mirror the whole canvas: every layer and mask. One undo step.
    pub fn flip_canvas(&mut self, horizontal: bool) -> Option<Edit> {
        let name = if horizontal { "Flip Canvas Horizontal" } else { "Flip Canvas Vertical" };
        self.apply_doc_op(name, Box::new(FlipCanvas { horizontal }))
    }

    /// Apply any document-wide operation as one undo step (crop, resize and the like).
    pub fn apply_doc_op(&mut self, name: &'static str, op: Box<dyn DocOp>) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        let mut cmd = Command::new(name);
        cmd.apply(&mut self.doc, Change::Doc(op));
        self.finish(cmd, None)
    }

    // ----- helpers for the app -----

    /// `preferred` if it still names an item, else the topmost raster layer. Call after undo,
    /// redo, delete or load so the active layer never dangles.
    pub fn valid_active(&self, preferred: Option<u64>) -> u64 {
        if let Some(id) = preferred.filter(|&id| self.doc.node(id).is_some()) {
            return id;
        }
        self.doc.layer_ids().last().copied().unwrap_or(0)
    }

    // ----- output -----

    /// The whole canvas composited (background included), straight RGBA8.
    pub fn flatten(&self) -> Vec<u8> {
        composite(&self.doc, CompositeOpts::default())
    }

    /// One rectangle of the composite, straight RGBA8.
    pub fn flatten_rect(&self, rect: PixelRect) -> Vec<u8> {
        composite_rect(&self.doc, rect.x, rect.y, rect.w, rect.h, CompositeOpts::default())
    }

    /// Show `pixels` (premultiplied RGBA8, canvas sized) in place of layer `id` in the display
    /// composite, without changing the layer. Ignored when the size is wrong.
    pub fn set_preview(&mut self, id: u64, pixels: Vec<u8>) {
        let (w, h) = (self.doc.width, self.doc.height);
        if self.doc.layer(id).is_some() && pixels.len() == w * h * 4 {
            self.preview = Some((id, Layer::from_rgba(w, h, pixels)));
            self.preview_gen += 1;
        }
    }

    /// Show a ready-made layer (canvas sized) in place of layer `id`, like [`Session::set_preview`].
    pub fn set_preview_layer(&mut self, id: u64, layer: Layer) {
        if self.doc.layer(id).is_some() && layer.width == self.doc.width && layer.height == self.doc.height {
            self.preview = Some((id, layer));
            self.preview_gen += 1;
        }
    }

    /// Stop showing a layer preview. Returns whether there was one.
    pub fn clear_preview(&mut self) -> bool {
        let had = self.preview.take().is_some();
        if had {
            self.preview_gen += 1;
        }
        had
    }

    /// The layer preview, if any: the layer it replaces, its pixels, and a counter that changes
    /// every time a preview is set or cleared.
    pub fn preview_layer(&self) -> Option<(u64, &Layer)> {
        self.preview.as_ref().map(|(id, l)| (*id, l))
    }

    /// Changes whenever the layer preview is set or cleared (see [`Session::preview_layer`]).
    pub fn preview_gen(&self) -> u64 {
        self.preview_gen
    }

    pub fn has_preview(&self) -> bool {
        self.preview.is_some()
    }

    /// Run `f` on the document with the preview pixels swapped into their layer (and swapped
    /// back before returning), so the stored pixels are never lost or changed.
    fn with_preview<R>(&mut self, f: impl FnOnce(&Document) -> R) -> R {
        if let Some((id, buf)) = self.preview.as_mut() {
            if let Some(layer) = self.doc.layer_mut(*id) {
                std::mem::swap(layer, buf);
                let out = f(&self.doc);
                if let Some(layer) = self.doc.layer_mut(*id) {
                    std::mem::swap(layer, buf);
                }
                return out;
            }
        }
        f(&self.doc)
    }

    /// The whole canvas as it is displayed: like [`Session::flatten`], with any layer preview, and
    /// mapped from the document's colour space to the screen's (sRGB).
    pub fn flatten_display(&mut self) -> Vec<u8> {
        let mut px = self.with_preview(|doc| composite(doc, CompositeOpts::default()));
        crate::colour_management::apply_display_rgba8(self.doc.colour_space, &mut px);
        px
    }

    /// One rectangle of the displayed composite, with any layer preview.
    pub fn flatten_rect_display(&mut self, rect: PixelRect) -> Vec<u8> {
        let mut px = self.with_preview(|doc| composite_rect(doc, rect.x, rect.y, rect.w, rect.h, CompositeOpts::default()));
        crate::colour_management::apply_display_rgba8(self.doc.colour_space, &mut px);
        px
    }

    /// One rectangle of the displayed composite as premultiplied RGBA8 pixels (a `u32` per pixel whose bytes are `[r, g, b, a]`),
    /// ready for `egui::Color32`. Exactly [`Session::flatten_rect_display`] followed by egui's
    /// `from_rgba_unmultiplied` for every pixel, but done block by block on the compositor's
    /// threads, with no whole-canvas straight RGBA buffer and no second conversion pass. `rect`
    /// must lie inside the canvas.
    pub fn display_pixels_rect(&mut self, rect: PixelRect) -> Vec<u32> {
        let transform = crate::colour_management::display_transform(self.doc.colour_space);
        let map = move |px: &mut [u8]| {
            if let Some(t) = &transform {
                t.convert_straight_rgba8(px);
            }
        };
        self.with_preview(|doc| composite_rect_premul(doc, rect.x, rect.y, rect.w, rect.h, CompositeOpts::default(), &map))
    }

    /// The whole displayed canvas: see [`Session::display_pixels_rect`].
    pub fn display_pixels(&mut self) -> Vec<u32> {
        let rect = PixelRect {
            x: 0,
            y: 0,
            w: self.doc.width,
            h: self.doc.height,
        };
        self.display_pixels_rect(rect)
    }

    /// The composite for a flattened export: private items left out.
    pub fn flatten_for_export(&self) -> Vec<u8> {
        composite(
            &self.doc,
            CompositeOpts {
                skip_private: true,
                background: true,
            },
        )
    }
}

/// Lock transparency for a pixel rectangle: keep the old alpha, move only the colour; transparent
/// pixels stay as they were. Both buffers are premultiplied RGBA8.
/// Blend `after` back towards `before` by how far each pixel lies outside the selection. Both
/// buffers are premultiplied RGBA8 covering `rect`. Pixels fully inside keep their new value and
/// pixels fully outside keep their old one.
fn clip_to_selection<T: Chan>(before: &[T], after: &mut [T], rect: PixelRect, sel: &Selection) {
    for row in 0..rect.h {
        for col in 0..rect.w {
            let coverage = sel.clip_coverage(rect.x + col, rect.y + row, 1.0);
            if coverage >= 1.0 {
                continue;
            }
            let i = (row * rect.w + col) * 4;
            for k in i..i + 4 {
                let (b, a) = (before[k].to_f32(), after[k].to_f32());
                after[k] = T::from_f32_round(b + (a - b) * coverage);
            }
        }
    }
}

fn keep_alpha(before: &[u8], after: &mut [u8]) {
    for (old, new) in before.as_chunks::<4>().0.iter().zip(after.as_chunks_mut::<4>().0.iter_mut()) {
        if old[3] == 0 {
            new.copy_from_slice(old);
        } else if new[3] == 0 {
            new[..3].fill(0);
            new[3] = old[3];
        } else {
            let k = f64::from(old[3]) / f64::from(new[3]);
            for c in &mut new[..3] {
                *c = (f64::from(*c) * k).round().min(f64::from(old[3])) as u8;
            }
            new[3] = old[3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{sample_from, PointerKind, RawPointer};
    use eskiz_core::brushes::{Brush, Preset};
    use std::collections::BTreeMap;

    fn settings() -> StrokeSettings {
        let mut brush = Brush::preset(Preset::Pencil);
        brush.size = 30.0;
        StrokeSettings {
            brush,
            colour: [0.9, 0.1, 0.1],
            erase: false,
            raster: None,
        }
    }

    fn stroke(session: &mut Session, layer: u64, y: f64) -> bool {
        if !session.begin_stroke(layer, &settings()) {
            return false;
        }
        for i in 0..40 {
            let raw = RawPointer {
                x: 40.0 + i as f64 * 6.0,
                y,
                time: i as f64 / 120.0,
                kind: PointerKind::Mouse,
                force: None,
            };
            session.push(sample_from(raw));
        }
        session.end_stroke().is_some()
    }

    fn fresh() -> Session {
        Session::new(Document::new(200, 120))
    }

    fn base(s: &Session) -> u64 {
        s.doc.layer_id(0).unwrap()
    }

    /// Everything undo must restore: the tree, every layer's pixels, the background.
    type Snap = (Vec<Node>, BTreeMap<u64, Vec<u8>>, [f64; 3], bool, Vec<u8>);

    fn snap(s: &Session) -> Snap {
        let pixels = s
            .doc
            .layer_ids()
            .into_iter()
            .map(|id| (id, s.doc.layer(id).unwrap().to_rgba()))
            .collect();
        (s.doc.root.clone(), pixels, s.doc.background, s.doc.background_visible, s.flatten())
    }

    /// Run `op`, then check undo restores the state before and redo the state after.
    fn round_trip(s: &mut Session, name: &str, op: impl FnOnce(&mut Session)) {
        let before = snap(s);
        let steps = s.undo_steps();
        op(s);
        let after = snap(s);
        assert_ne!(before, after, "{name}: the operation changed nothing");
        assert_eq!(s.undo_steps(), steps + 1, "{name}: one undo step");
        assert!(s.undo().is_some());
        assert_eq!(snap(s), before, "{name}: undo");
        assert!(s.redo().is_some());
        assert_eq!(snap(s), after, "{name}: redo");
        assert!(s.undo().is_some());
        assert_eq!(snap(s), before, "{name}: undo again");
        assert!(s.redo().is_some());
    }

    #[test]
    fn pour_fill_is_one_named_undo_step() {
        let mut s = fresh();
        let l = base(&s);
        let blank = s.flatten();
        let rect = s.pour_fill(l, (50, 50), 0.5, [255, 0, 0]).unwrap();
        assert_eq!((rect.w, rect.h), (200, 120));
        assert_ne!(s.flatten(), blank);
        assert_eq!(s.undo_steps(), 1);
        assert_eq!(s.undo_name(), Some("Pour Fill"));
        let filled = s.flatten();
        s.undo().unwrap();
        assert_eq!(s.flatten(), blank);
        s.redo().unwrap();
        assert_eq!(s.flatten(), filled);
    }

    #[test]
    fn pour_fill_preview_changes_do_not_pile_up() {
        let mut s = fresh();
        let l = base(&s);
        let blank = s.flatten();
        s.pour_fill_preview(l, (5, 5), 0.5, [255, 0, 0]).unwrap();
        s.pour_fill_preview(l, (5, 5), 0.2, [0, 255, 0]).unwrap();
        assert_eq!(s.undo_steps(), 0);
        s.pour_fill_cancel().unwrap();
        assert_eq!(s.flatten(), blank);
        assert!(!s.has_pour_fill_preview());
        // A seed outside the canvas does nothing.
        assert!(s.pour_fill(l, (-1, 5), 0.5, [0, 0, 0]).is_none());
        assert_eq!(s.undo_steps(), 0);
    }

    #[test]
    fn pour_fill_refuses_hidden_and_locked_layers_and_honours_alpha_lock() {
        let mut s = fresh();
        let l = base(&s);
        s.set_visible(l, false);
        assert!(s.pour_fill(l, (5, 5), 0.5, [255, 0, 0]).is_none());
        s.set_visible(l, true);
        s.set_locked(l, true);
        assert!(s.pour_fill(l, (5, 5), 0.5, [255, 0, 0]).is_none());
        s.set_locked(l, false);
        // Lock transparency on an empty layer: nothing can change, alpha stays 0.
        s.set_alpha_lock(l, true);
        s.pour_fill(l, (5, 5), 0.5, [255, 0, 0]);
        assert!(s.doc.layer(l).unwrap().is_blank());
        // Half-transparent pixels keep their alpha and take the colour.
        s.set_alpha_lock(l, false);
        s.doc.layer_mut(l).unwrap().write_rect(0, 0, 200, 120, &[0, 0, 0, 128].repeat(200 * 120));
        s.set_alpha_lock(l, true);
        s.pour_fill(l, (5, 5), 1.0, [255, 0, 0]).unwrap();
        let p = s.doc.layer(l).unwrap().pixel(5, 5);
        assert_eq!(p[3], 128);
        assert!(p[0] > 100 && p[1] < 5, "{p:?}");
    }

    #[test]
    fn every_commit_undo_and_redo_bumps_the_edit_counter() {
        let mut s = fresh();
        let l = base(&s);
        let mut last = s.edits();
        let mut bumped = |s: &Session| {
            let now = s.edits();
            assert!(now > last);
            last = now;
        };
        stroke(&mut s, l, 40.0);
        bumped(&s);
        s.add_layer(None);
        bumped(&s);
        s.set_opacity(l, 0.5);
        bumped(&s);
        s.undo();
        bumped(&s);
        s.redo();
        bumped(&s);
        s.pour_fill(l, (5, 5), 0.5, [1, 2, 3]);
        bumped(&s);
        s.mark_edited();
        bumped(&s);
        s.replace_document(Document::new(4, 4));
        bumped(&s);
    }

    #[test]
    fn stroke_undo_redo_round_trip() {
        let mut s = fresh();
        let l = base(&s);
        let blank = s.flatten();
        assert!(stroke(&mut s, l, 40.0));
        let one = s.flatten();
        assert_ne!(one, blank);
        assert!(stroke(&mut s, l, 80.0));
        let two = s.flatten();

        assert!(s.undo().is_some());
        assert_eq!(s.flatten(), one);
        assert!(s.undo().is_some());
        assert_eq!(s.flatten(), blank);
        assert!(!s.can_undo());
        assert!(s.redo().is_some());
        assert!(s.redo().is_some());
        assert_eq!(s.flatten(), two);
        assert!(!s.can_redo());
        assert_eq!(s.undo_name(), Some("Stroke"));
    }

    #[test]
    fn new_stroke_after_undo_drops_redo() {
        let mut s = fresh();
        let l = base(&s);
        stroke(&mut s, l, 40.0);
        stroke(&mut s, l, 80.0);
        s.undo();
        assert!(s.can_redo());
        stroke(&mut s, l, 60.0);
        assert!(!s.can_redo());
    }

    #[test]
    fn undo_history_is_capped() {
        let mut s = fresh();
        let l = base(&s);
        for i in 0..(History::DEFAULT_MAX_STEPS + 4) {
            stroke(&mut s, l, 10.0 + (i % 100) as f64);
        }
        assert_eq!(s.undo_steps(), History::DEFAULT_MAX_STEPS);
    }

    #[test]
    fn hidden_or_locked_layer_refuses_strokes() {
        let mut s = fresh();
        let l = base(&s);
        s.set_visible(l, false);
        assert!(!stroke(&mut s, l, 40.0));
        s.set_visible(l, true);
        s.set_locked(l, true);
        assert!(!stroke(&mut s, l, 40.0));
        assert!(!s.begin_smudge(l, 5.0, 1.0, None));
        assert!(s.clear_layer(l).is_none());
        s.set_locked(l, false);
        assert!(stroke(&mut s, l, 40.0));
    }

    #[test]
    fn locked_or_hidden_group_blocks_children() {
        let mut s = fresh();
        let l = base(&s);
        let g = s.group_items(&[l]).unwrap().id.unwrap();
        assert!(s.can_paint(l));
        s.set_locked(g, true);
        assert!(!s.can_paint(l));
        s.set_locked(g, false);
        s.set_visible(g, false);
        assert!(!s.can_paint(l));
    }

    #[test]
    fn alpha_lock_keeps_alpha_while_painting() {
        // Acceptance 33 and 34.
        let mut s = fresh();
        let l = base(&s);
        // A half-transparent black pixel block on the left, nothing elsewhere.
        {
            let layer = s.doc.layer_mut(l).unwrap();
            for y in 0..120 {
                for x in 0..100 {
                    layer.set_pixel(x, y, [0, 0, 0, 128]);
                }
            }
        }
        let original = s.doc.layer(l).unwrap().to_rgba();
        s.set_alpha_lock(l, true);
        assert!(stroke(&mut s, l, 60.0));
        let after = s.doc.layer(l).unwrap().to_rgba();
        let mut recoloured = false;
        for (i, (a, b)) in original.as_chunks::<4>().0.iter().zip(after.as_chunks::<4>().0.iter()).enumerate() {
            assert_eq!(a[3], b[3], "alpha changed at pixel {i}");
            if a[3] == 0 {
                assert_eq!(a, b);
            } else if a != b {
                recoloured = true;
            }
        }
        assert!(recoloured, "the stroke should have recolored the locked pixels");
        // Without the lock the stroke creates alpha on empty pixels (acceptance 32).
        s.undo();
        s.set_alpha_lock(l, false);
        assert!(stroke(&mut s, l, 60.0));
        let grew = original
            .as_chunks::<4>().0.iter()
            .zip(s.doc.layer(l).unwrap().to_rgba().as_chunks::<4>().0.iter())
            .any(|(a, b)| a[3] == 0 && b[3] > 0);
        assert!(grew);
    }

    #[test]
    fn alpha_locked_smudge_keeps_alpha() {
        let mut s = fresh();
        let l = base(&s);
        {
            let layer = s.doc.layer_mut(l).unwrap();
            for y in 0..120 {
                for x in 0..200 {
                    let px = if x < 100 { [200, 20, 20, 200] } else if x < 150 { [0, 0, 200, 200] } else { [0; 4] };
                    layer.set_pixel(x, y, px);
                }
            }
        }
        let original = s.doc.layer(l).unwrap().to_rgba();
        s.set_alpha_lock(l, true);
        assert!(s.begin_smudge(l, 12.0, 1.0, None));
        for i in 0..80 {
            s.push(sample_from(RawPointer {
                x: 60.0 + i as f64 * 1.5,
                y: 60.0,
                time: i as f64 / 120.0,
                kind: PointerKind::Mouse,
                force: None,
            }));
        }
        s.end_stroke();
        let after = s.doc.layer(l).unwrap().to_rgba();
        assert_ne!(original, after);
        for (a, b) in original.as_chunks::<4>().0.iter().zip(after.as_chunks::<4>().0.iter()) {
            assert_eq!(a[3], b[3]);
        }
    }

    #[test]
    fn layer_reorder_keeps_undo_pointing_at_the_right_layer() {
        let mut s = fresh();
        let l = base(&s);
        s.add_layer(None);
        let blank = s.flatten();
        stroke(&mut s, l, 50.0);
        // Move the painted layer to the top; undo of the stroke must still restore it.
        s.move_layer(l, true);
        assert!(s.undo().is_some()); // undoes the move
        assert!(s.undo().is_some()); // undoes the stroke
        assert_eq!(s.flatten(), blank);
    }

    #[test]
    fn deleting_a_layer_is_undoable_and_keeps_history() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        stroke(&mut s, b, 40.0);
        stroke(&mut s, a, 80.0);
        let painted = snap(&s);
        let steps = s.undo_steps();
        s.delete_layer(b).unwrap();
        assert!(s.doc.node(b).is_none());
        assert_eq!(s.undo_steps(), steps + 1);
        assert_eq!(s.undo_name(), Some("Delete Layer"));
        s.undo();
        assert_eq!(snap(&s), painted);
        // Strokes made before the delete can still be undone afterwards.
        s.undo();
        s.undo();
        assert!(s.doc.node(b).is_some());
        assert!(s.doc.layer(b).unwrap().is_blank());
        s.undo();
        assert!(s.doc.node(b).is_none(), "undoing the add removes the layer");
    }

    #[test]
    fn last_layer_cannot_be_deleted() {
        let mut s = fresh();
        let l = base(&s);
        assert!(s.delete_layer(l).is_none());
        let g = s.group_items(&[l]).unwrap().id.unwrap();
        assert!(s.delete_layer(g).is_none());
    }

    #[test]
    fn add_duplicate_delete_round_trips() {
        let mut s = fresh();
        let l = base(&s);
        stroke(&mut s, l, 40.0);
        round_trip(&mut s, "add", |s| {
            s.add_layer(Some(l));
        });
        round_trip(&mut s, "duplicate", |s| {
            s.duplicate_layer(l);
        });
        let dup = s.doc.layer_ids()[1];
        assert_eq!(s.doc.layer(dup).unwrap(), s.doc.layer(l).unwrap());
        assert!(s.doc.layer(dup).unwrap().tile_shared(s.doc.layer(l).unwrap(), 0, 0), "a duplicate shares tiles");
        assert_eq!(s.doc.locate(dup).unwrap().index, 1, "the copy sits directly above the original");
        round_trip(&mut s, "delete", |s| {
            s.delete_layer(dup);
        });
    }

    #[test]
    fn clear_and_replace_round_trip() {
        let mut s = fresh();
        let l = base(&s);
        stroke(&mut s, l, 40.0);
        round_trip(&mut s, "clear", |s| {
            s.clear_layer(l);
        });
        assert_eq!(s.undo_name(), Some("Clear Layer"));
        let len = s.doc.layer(l).unwrap().to_rgba().len();
        round_trip(&mut s, "replace", |s| {
            s.replace_layer_pixels(l, vec![200; len], "Fill");
        });
        assert_eq!(s.undo_name(), Some("Fill"));
        assert!(s.replace_layer_pixels(l, vec![0; 3], "Bad").is_none());
    }

    #[test]
    fn clearing_an_alpha_locked_layer_keeps_alpha() {
        let mut s = fresh();
        let l = base(&s);
        stroke(&mut s, l, 40.0);
        let alphas: Vec<u8> = s.doc.layer(l).unwrap().to_rgba().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        s.set_alpha_lock(l, true);
        s.clear_layer(l).unwrap();
        let px = s.doc.layer(l).unwrap().to_rgba();
        assert!(px.as_chunks::<4>().0.iter().zip(&alphas).all(|(p, &a)| p[3] == a && p[0] == 0 && p[1] == 0 && p[2] == 0));
        s.undo();
        s.undo();
    }

    #[test]
    fn move_round_trips_including_into_groups() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        let c = s.add_layer(None).unwrap().id.unwrap();
        round_trip(&mut s, "move up", |s| {
            s.move_layer(a, true);
        });
        assert_eq!(s.doc.layer_ids(), vec![b, a, c]);
        round_trip(&mut s, "move to index", |s| {
            s.move_item(c, None, 0);
        });
        assert_eq!(s.doc.layer_ids(), vec![c, b, a]);
        // The bottom item cannot move down and the top cannot move up.
        assert!(s.move_layer(c, false).is_none());
        let g = s.group_items(&[b]).unwrap().id.unwrap();
        round_trip(&mut s, "move into group", |s| {
            s.move_into_group(a, g);
        });
        assert_eq!(s.doc.locate(a).unwrap().parent, Some(g));
        // A group cannot go into itself.
        assert!(s.move_into_group(g, g).is_none());
    }

    #[test]
    fn group_and_ungroup_round_trip() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        let c = s.add_layer(None).unwrap().id.unwrap();
        stroke(&mut s, a, 30.0);
        stroke(&mut s, c, 70.0);
        let mut g = 0;
        round_trip(&mut s, "group", |s| {
            g = s.group_items(&[c, a]).unwrap().id.unwrap();
        });
        assert_eq!(s.doc.node(g).unwrap().children().iter().map(|n| n.id).collect::<Vec<_>>(), vec![a, c]);
        assert_eq!(s.doc.root.iter().map(|n| n.id).collect::<Vec<_>>(), vec![b, g]);
        assert_eq!(s.undo_name(), Some("Group"));
        round_trip(&mut s, "ungroup", |s| {
            s.ungroup(g);
        });
        assert_eq!(s.doc.layer_ids(), vec![b, a, c], "children take the group's place");
        assert!(s.doc.node(g).is_none());
        // Items from different parents cannot be grouped together.
        let g2 = s.group_items(&[a]).unwrap().id.unwrap();
        assert!(s.group_items(&[g2, c]).is_some());
        assert!(s.group_items(&[]).is_none());
    }

    #[test]
    fn grouping_changes_nothing_for_normal_layers() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        stroke(&mut s, a, 30.0);
        stroke(&mut s, b, 60.0);
        let before = s.flatten();
        s.group_items(&[a, b]).unwrap();
        assert_eq!(s.flatten(), before);
    }

    #[test]
    fn merge_down_round_trip_and_appearance() {
        // Acceptance 35.
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        stroke(&mut s, a, 30.0);
        stroke(&mut s, b, 60.0);
        s.set_opacity(b, 0.6);
        let appearance = s.flatten();
        round_trip(&mut s, "merge down", |s| {
            s.merge_down(b);
        });
        assert!(s.doc.node(b).is_none());
        let lower = s.doc.node(a).unwrap();
        assert_eq!(lower.props.opacity, 1.0);
        assert!(lower.mask.is_none());
        let after = s.flatten();
        let worst = appearance.iter().zip(&after).map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs()).max().unwrap();
        assert!(worst <= 2, "appearance changed by {worst}");
        assert_eq!(s.undo_name(), Some("Merge Down"));
    }

    #[test]
    fn merge_down_applies_lower_opacity_and_mask_and_refuses_groups_and_locks() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        stroke(&mut s, a, 30.0);
        s.set_opacity(a, 0.5);
        s.add_mask(a, MaskFill::Reveal);
        let before = s.flatten();
        s.merge_down(b).unwrap();
        let lower = s.doc.node(a).unwrap();
        assert_eq!((lower.props.opacity, lower.mask.is_some()), (1.0, false));
        // Lower opacity was baked into the pixels, so the picture is (nearly) unchanged.
        let worst = before.iter().zip(&s.flatten()).map(|(x, y)| (i32::from(*x) - i32::from(*y)).abs()).max().unwrap();
        assert!(worst <= 2, "{worst}");
        s.undo();
        s.set_locked(b, true);
        assert!(s.merge_down(b).is_none());
        s.set_locked(b, false);
        let g = s.group_items(&[a]).unwrap().id.unwrap();
        assert!(s.merge_down(b).is_none(), "the lower item is a group");
        assert!(s.merge_down(g).is_none(), "nothing below the bottom item");
    }

    #[test]
    fn property_changes_round_trip_with_names() {
        let mut s = fresh();
        let l = base(&s);
        stroke(&mut s, l, 40.0);
        type Case = (&'static str, Box<dyn Fn(&mut Session, u64)>);
        let cases: Vec<Case> = vec![
            ("Layer Opacity", Box::new(|s, l| { s.set_opacity(l, 0.4); })),
            ("Layer Blend Mode", Box::new(|s, l| { s.set_blend(l, BlendMode::Multiply); })),
            ("Hide Layer", Box::new(|s, l| { s.set_visible(l, false); })),
            ("Rename Layer", Box::new(|s, l| { s.set_name(l, "Sketch"); })),
            ("Clip to Layer Below", Box::new(|s, l| { s.set_clipped(l, true); })),
            ("Lock Transparency", Box::new(|s, l| { s.set_alpha_lock(l, true); })),
            ("Lock Layer", Box::new(|s, l| { s.set_locked(l, true); })),
            ("Private Layer", Box::new(|s, l| { s.set_private(l, true); })),
            ("Add Mask", Box::new(|s, l| { s.add_mask(l, MaskFill::Hide); })),
        ];
        for (name, op) in cases {
            round_trip(&mut s, name, |s| op(s, l));
            assert_eq!(s.undo_name(), Some(name));
            // Put it back so every case starts from the same state.
            s.undo();
            s.history.clear();
        }
        // Mask removal.
        s.add_mask(l, MaskFill::Reveal);
        round_trip(&mut s, "Delete Mask", |s| {
            s.remove_mask(l);
        });
        assert!(s.remove_mask(l).is_none(), "no mask left to remove");
    }

    #[test]
    fn unchanged_property_records_nothing() {
        let mut s = fresh();
        let l = base(&s);
        let steps = s.undo_steps();
        s.set_opacity(l, 1.0).unwrap();
        s.set_visible(l, true).unwrap();
        assert_eq!(s.undo_steps(), steps);
        // Out-of-range opacity is clamped, names are limited to 64 characters.
        s.set_opacity(l, 7.0);
        assert_eq!(s.doc.node(l).unwrap().props.opacity, 1.0);
        s.set_name(l, &"x".repeat(100));
        assert_eq!(s.doc.node(l).unwrap().props.name.chars().count(), 64);
    }

    #[test]
    fn opacity_drag_is_one_undo_step() {
        let mut s = fresh();
        let l = base(&s);
        stroke(&mut s, l, 40.0);
        let before = snap(&s);
        let steps = s.undo_steps();
        for v in [0.9, 0.8, 0.7, 0.5, 0.3] {
            s.drag_opacity(l, v).unwrap();
        }
        s.end_drag();
        assert_eq!(s.undo_steps(), steps + 1);
        assert_eq!(s.doc.node(l).unwrap().props.opacity, 0.3);
        s.undo();
        assert_eq!(snap(&s), before);
        s.redo();
        assert_eq!(s.doc.node(l).unwrap().props.opacity, 0.3);
        // A later drag is a new step.
        s.drag_opacity(l, 0.2);
        s.end_drag();
        assert_eq!(s.undo_steps(), steps + 2);
    }

    #[test]
    fn background_round_trips() {
        let mut s = fresh();
        round_trip(&mut s, "background color", |s| {
            s.set_background_colour([0.2, 0.4, 0.6]);
        });
        assert_eq!(s.undo_name(), Some("Background Color"));
        round_trip(&mut s, "background visibility", |s| {
            s.set_background_visible(false);
        });
        assert_eq!(s.undo_name(), Some("Hide Background"));
        assert_eq!(&s.flatten()[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn flip_canvas_round_trips_and_mirrors() {
        let mut s = fresh();
        let l = base(&s);
        stroke(&mut s, l, 40.0);
        s.add_mask(l, MaskFill::Reveal);
        let first = s.doc.layer(l).unwrap().to_rgba();
        round_trip(&mut s, "flip h", |s| {
            s.flip_canvas(true);
        });
        assert_eq!(s.undo_name(), Some("Flip Canvas Horizontal"));
        let flipped = s.doc.layer(l).unwrap().to_rgba();
        assert!(first.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        for y in 0..120 {
            for x in 0..200 {
                assert_eq!(&flipped[(y * 200 + x) * 4..][..4], &first[(y * 200 + 199 - x) * 4..][..4]);
            }
        }
        round_trip(&mut s, "flip v", |s| {
            s.flip_canvas(false);
        });
        s.undo();
        s.undo();
        assert_eq!(s.doc.layer(l).unwrap().to_rgba(), first);
        // Two flips of the same axis return the original (acceptance 68).
        s.flip_canvas(true);
        s.flip_canvas(true);
        assert_eq!(s.doc.layer(l).unwrap().to_rgba(), first);
    }

    #[test]
    fn mask_replace_round_trips_and_checks_size() {
        let mut s = fresh();
        let l = base(&s);
        assert!(s.set_mask(l, Some(Mask { data: vec![0; 3] }), "Bad").is_none());
        let m = Mask::filled(200, 120, 90);
        round_trip(&mut s, "set mask", |s| {
            s.set_mask(l, Some(m), "Edit Mask");
        });
    }

    #[test]
    fn mixed_history_unwinds_to_the_start() {
        let mut s = fresh();
        let a = base(&s);
        let start = snap(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        stroke(&mut s, b, 50.0);
        s.set_blend(b, BlendMode::Overlay);
        let g = s.group_items(&[a, b]).unwrap().id.unwrap();
        s.set_opacity(g, 0.5);
        s.duplicate_layer(b);
        s.merge_down(b);
        s.flip_canvas(false);
        s.set_background_colour([0.0, 0.5, 0.0]);
        s.clear_layer(a);
        s.delete_layer(b);
        let end = snap(&s);
        let steps = s.undo_steps();
        for _ in 0..steps {
            assert!(s.undo().is_some());
        }
        assert_eq!(snap(&s), start);
        for _ in 0..steps {
            assert!(s.redo().is_some());
        }
        assert_eq!(snap(&s), end);
    }

    #[test]
    fn edits_are_refused_while_stroking() {
        let mut s = fresh();
        let l = base(&s);
        assert!(s.begin_stroke(l, &settings()));
        assert!(s.add_layer(None).is_none());
        assert!(s.set_opacity(l, 0.5).is_none());
        assert!(s.flip_canvas(true).is_none());
        assert!(s.undo().is_none());
        s.cancel_stroke();
        assert!(s.add_layer(None).is_some());
    }

    #[test]
    fn valid_active_falls_back_to_the_top_layer() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        assert_eq!(s.valid_active(Some(a)), a);
        s.delete_layer(b);
        assert_eq!(s.valid_active(Some(b)), a);
        assert_eq!(s.valid_active(None), a);
    }

    #[test]
    fn fill_combine_and_background_drag_are_single_named_steps() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        round_trip(&mut s, "fill", |s| {
            s.fill_layer(b, [10, 20, 30]).unwrap();
        });
        assert_eq!(s.undo_name(), Some("Fill Layer"));
        s.fill_layer(b, [10, 20, 30]).unwrap();
        assert_eq!(s.doc.layer(b).unwrap().pixel(1, 1), [10, 20, 30, 255]);
        s.undo();
        // An alpha-locked fill keeps the (empty) alpha.
        s.set_alpha_lock(b, true);
        s.fill_layer(b, [10, 20, 30]).unwrap();
        assert!(s.doc.layer(b).unwrap().is_blank());
        s.set_locked(b, true);
        assert!(s.fill_layer(b, [1, 2, 3]).is_none(), "locked layers refuse the fill");
        s.set_locked(b, false);

        let g = s.combine_down(b).unwrap().id.unwrap();
        assert_eq!(s.undo_name(), Some("Combine Down"));
        assert_eq!(s.doc.node(g).unwrap().children().iter().map(|n| n.id).collect::<Vec<_>>(), vec![a, b]);
        assert!(s.combine_down(g).is_none(), "nothing below the group");
        s.undo();
        assert!(s.can_merge_down(b));
        assert!(!s.can_merge_down(a));

        let steps = s.undo_steps();
        for c in [[0.9, 0.8, 0.7], [0.5, 0.4, 0.3], [0.1, 0.2, 0.3]] {
            s.drag_background(c).unwrap();
        }
        s.end_drag();
        assert_eq!(s.undo_steps(), steps + 1);
        assert_eq!(s.doc.background, [0.1, 0.2, 0.3]);
        s.undo();
        assert_eq!(s.doc.background, [1.0; 3]);
    }

    #[test]
    fn layer_epochs_change_only_for_layers_whose_pixels_changed() {
        let mut s = fresh();
        let a = base(&s);
        let b = s.add_layer(None).unwrap().id.unwrap();
        let (ea, eb) = (s.layer_epoch(a), s.layer_epoch(b));
        s.set_opacity(a, 0.5);
        s.set_name(b, "x");
        s.move_layer(a, true);
        assert_eq!((s.layer_epoch(a), s.layer_epoch(b)), (ea, eb), "properties and order are not pixels");
        stroke(&mut s, a, 40.0);
        assert_ne!(s.layer_epoch(a), ea);
        assert_eq!(s.layer_epoch(b), eb);
        let e = s.layer_epoch(a);
        s.undo();
        assert_ne!(s.layer_epoch(a), e, "undo of a stroke changes the pixels");
        let e = s.layer_epoch(a);
        s.flip_canvas(true);
        assert_ne!(s.layer_epoch(a), e, "a canvas-wide operation touches every layer");
        assert_ne!(s.layer_epoch(b), eb);
    }

    #[test]
    fn rect_refresh_matches_full_flatten_after_a_stroke_inside_a_group() {
        let mut s = Session::new(Document::new(200, 120));
        let a = base(&s);
        let g = s.group_items(&[a]).unwrap().id.unwrap();
        s.set_opacity(g, 0.5);
        s.set_blend(g, BlendMode::Multiply);
        stroke(&mut s, a, 60.0);
        let full = s.flatten();
        let rect = PixelRect { x: 30, y: 20, w: 100, h: 70 };
        let part = s.flatten_rect(rect);
        for y in 0..rect.h {
            for x in 0..rect.w {
                let i = ((rect.y + y) * 200 + rect.x + x) * 4;
                assert_eq!(&part[(y * rect.w + x) * 4..][..4], &full[i..i + 4]);
            }
        }
    }

    // ----- tiled storage -----

    const TILE_BYTES: usize = eskiz_core::canvas::TILE_BYTES;

    fn big() -> Session {
        Session::new(Document::new(2048, 1536))
    }

    #[test]
    fn many_empty_layers_cost_no_pixel_memory() {
        let mut s = big();
        for _ in 0..19 {
            s.add_layer(None).unwrap();
        }
        assert_eq!(s.doc.layer_count(), 20);
        assert_eq!(s.doc.tile_bytes(), 0);
        assert_eq!(s.undo_bytes(), 0, "adding empty layers stores no pixels in history");
        // Duplicating, deleting and merging empty layers stay free too.
        let top = s.doc.layer_ids()[19];
        let dup = s.duplicate_layer(top).unwrap().id.unwrap();
        s.merge_down(dup).unwrap();
        s.delete_layer(top).unwrap();
        assert_eq!(s.doc.tile_bytes(), 0);
        assert_eq!(s.undo_bytes(), 0);
        s.undo();
        s.undo();
        s.undo();
        assert_eq!(s.doc.tile_bytes(), 0);
    }

    #[test]
    fn a_stroke_undo_step_holds_only_the_touched_tiles() {
        let mut s = big();
        let l = base(&s);
        assert!(stroke(&mut s, l, 300.0)); // x 40..280, y ~300: tiles (0..=1, 1)
        let layer = s.doc.layer(l).unwrap();
        let painted = layer.tile_count();
        assert!((1..=4).contains(&painted), "{painted} tiles painted");
        assert!(painted < layer.tiles_x() * layer.tiles_y() / 4);
        // The "before" side is empty here, so the step costs exactly the painted tiles.
        assert_eq!(s.undo_bytes(), painted * TILE_BYTES);
        assert_eq!(s.doc.tile_bytes(), painted * TILE_BYTES);
        let snapshot = layer.clone();
        s.undo().unwrap();
        assert_eq!(s.doc.layer(l).unwrap().tile_count(), 0);
        s.redo().unwrap();
        assert_eq!(*s.doc.layer(l).unwrap(), snapshot);

        // A second stroke over the same tiles: its "before" is the first one's "after", so the
        // history counts the shared tile once.
        let first = s.undo_bytes();
        assert!(stroke(&mut s, l, 310.0));
        let total = s.undo_bytes();
        // Step 1 holds N tiles, step 2 holds N shared ones plus N new ones: 2N, not 3N.
        assert_eq!(total, first + painted * TILE_BYTES, "shared tiles are not counted twice");
    }

    #[test]
    fn a_whole_layer_replace_only_stores_changed_tiles() {
        let mut s = big();
        let l = base(&s);
        assert!(stroke(&mut s, l, 300.0));
        let used = s.doc.tile_bytes();
        let before = s.undo_bytes();
        // A "filter" that changes one corner pixel and nothing else.
        let mut px = s.doc.layer(l).unwrap().to_rgba();
        px[3] = 255;
        s.replace_layer_pixels(l, px, "Filter").unwrap();
        // Only the touched tile (0,0) is new data: it was empty before.
        assert_eq!(s.undo_bytes(), before + TILE_BYTES);
        assert_eq!(s.doc.tile_bytes(), used + TILE_BYTES);
        // Clear keeps the tiles of the layer in history (shared, counted once).
        s.clear_layer(l).unwrap();
        assert_eq!(s.doc.tile_bytes(), 0);
        assert_eq!(s.undo_bytes(), before + TILE_BYTES);
        s.undo();
        s.undo();
        assert_eq!(s.doc.layer(l).unwrap().tile_count(), used / TILE_BYTES);
    }

    #[test]
    fn duplicate_shares_tiles_and_diverges_on_paint() {
        let mut s = big();
        let l = base(&s);
        assert!(stroke(&mut s, l, 300.0));
        let bytes = s.doc.tile_bytes();
        let dup = s.duplicate_layer(l).unwrap().id.unwrap();
        assert_eq!(s.doc.tile_bytes(), bytes, "a duplicate shares its tiles");
        assert!(stroke(&mut s, dup, 300.0));
        let (a, b) = (s.doc.layer(l).unwrap(), s.doc.layer(dup).unwrap());
        assert_ne!(a, b);
        // The original is untouched by painting on the copy.
        assert_eq!(a.tile_count() * TILE_BYTES, bytes);
    }

    #[test]
    fn a_document_clone_shares_all_pixels() {
        let mut s = big();
        let l = base(&s);
        assert!(stroke(&mut s, l, 300.0));
        let copy = s.doc.clone();
        assert_eq!(copy.tile_bytes(), s.doc.tile_bytes());
        let (a, b) = (copy.layer(l).unwrap(), s.doc.layer(l).unwrap());
        for ty in 0..a.tiles_y() {
            for tx in 0..a.tiles_x() {
                assert!(a.tile_shared(b, tx, ty));
            }
        }
        // Painting on after the clone does not touch the snapshot (gallery saves rely on it).
        let snap = a.clone();
        assert!(stroke(&mut s, l, 500.0));
        assert_eq!(*copy.layer(l).unwrap(), snap);
    }

    #[test]
    fn pour_fill_undo_round_trips_across_all_tiles() {
        let mut s = big();
        let l = base(&s);
        // A fill of the whole empty layer touches every tile.
        s.pour_fill(l, (10, 10), 0.5, [255, 0, 0]).unwrap();
        assert_eq!(s.doc.layer(l).unwrap().tile_count(), 8 * 6);
        let flat = s.flatten();
        s.undo().unwrap();
        assert_eq!(s.doc.layer(l).unwrap().tile_count(), 0);
        s.redo().unwrap();
        assert_eq!(s.flatten(), flat);
    }

    /// `Session::display_pixels_rect` is what `flatten_rect_display` + egui's
    /// `from_rgba_unmultiplied` gave before, pixel for pixel, with a layer preview, a
    /// non-sRGB document and odd rectangles.
    #[test]
    fn display_pixels_match_the_unmultiplied_conversion() {
        use eskiz_core::ColourSpace;
        for (i, space) in [ColourSpace::Srgb, ColourSpace::DisplayP3].into_iter().enumerate() {
            let mut s = Session::new(Document::new(317, 203));
            s.doc.colour_space = space;
            s.doc.background_visible = i == 0;
            let l = base(&s);
            for k in 0..3 {
                let top = if k == 0 { l } else { s.doc.push_layer(None, eskiz_core::document::Props::new("x"), None, Vec::new()) };
                assert!(stroke(&mut s, top, 40.0 + 50.0 * k as f64));
            }
            let id = s.doc.layer_id(1).unwrap();
            s.doc.node_mut(id).unwrap().props.opacity = 0.5;
            // A preview of the base layer: half transparent noise.
            let mut px = vec![0u8; 317 * 203 * 4];
            for (n, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let a = (n * 7 % 256) as u8;
                *p = [a / 2, (n % 200) as u8 % (a / 2 + 1), a / 3, a];
            }
            s.set_preview(l, px);
            for rect in [
                PixelRect { x: 0, y: 0, w: 317, h: 203 },
                PixelRect { x: 130, y: 7, w: 101, h: 190 },
                PixelRect { x: 316, y: 202, w: 1, h: 1 },
            ] {
                let want: Vec<u32> = eframe::egui::ColorImage::from_rgba_unmultiplied([rect.w, rect.h], &s.flatten_rect_display(rect))
                    .pixels
                    .iter()
                    .map(|c| u32::from_ne_bytes(c.to_array()))
                    .collect();
                assert!(s.display_pixels_rect(rect) == want, "{space:?} {rect:?}");
            }
            let all = eframe::egui::ColorImage::from_rgba_unmultiplied([317, 203], &s.flatten_display());
            assert!(s.display_pixels().iter().zip(&all.pixels).all(|(a, b)| a.to_ne_bytes() == b.to_array()));
            // The wrap into an egui image is a reinterpretation of the same pixels.
            let wrapped = crate::app::color_image([317, 203], s.display_pixels());
            assert!(wrapped == all, "{space:?}: color_image");
        }
    }

    /// The premultiplication kept in `eskiz-core` is egui's, for every colour and alpha.
    #[test]
    fn premultiply_is_eguis() {
        for a in 0..=255u8 {
            for v in 0..=255u8 {
                let want = eframe::egui::Color32::from_rgba_unmultiplied(v, 255 - v, v / 2, a).to_array();
                assert_eq!(eskiz_core::composite::premultiply_u8([v, 255 - v, v / 2, a]), want, "v {v} a {a}");
            }
        }
    }

    #[test]
    fn undo_budget_follows_the_canvas() {
        let small = Session::new(Document::new(200, 120));
        assert_eq!(small.undo_budget(), History::DEFAULT_MAX_BYTES);
        let big = Session::new(Document::new(8000, 8000));
        assert!(big.undo_budget() >= History::DEFAULT_MAX_BYTES);
        if crate::history::physical_memory().is_some_and(|m| m >= 8 * 1024 * 1024 * 1024) {
            assert!(big.undo_budget() > History::DEFAULT_MAX_BYTES, "an 8000 x 8000 canvas gets a larger budget");
        }
        // A new document brings its own budget; resizing the canvas through a document swap too.
        let mut s = Session::new(Document::new(200, 120));
        s.replace_document(Document::new(8000, 8000));
        assert_eq!(s.undo_budget(), big.undo_budget());
    }

    /// Undo and redo give the same layers whatever the budget: the same edits run under the
    /// default budget and under a much larger one, and every undo and redo is compared.
    #[test]
    fn undo_redo_are_independent_of_the_budget() {
        let run = |budget: usize| {
            let mut s = fresh();
            s.history.set_max_bytes(budget);
            let l = base(&s);
            let mut states = Vec::new();
            for i in 0..6 {
                assert!(stroke(&mut s, l, 20.0 + 15.0 * i as f64));
                states.push(snap(&s));
            }
            let mut trace = Vec::new();
            while s.undo().is_some() {
                trace.push(snap(&s));
            }
            while s.redo().is_some() {
                trace.push(snap(&s));
            }
            (states, trace)
        };
        let small = run(History::DEFAULT_MAX_BYTES);
        let large = run(History::DEFAULT_MAX_BYTES * 8);
        assert!(small == large);
        // And the undo really restores each earlier state.
        let (states, trace) = &large;
        assert_eq!(trace[0], states[4]);
    }
}
