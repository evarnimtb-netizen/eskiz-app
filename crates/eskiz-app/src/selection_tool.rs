//! The Selection and Transform tools: their state, the operations they perform on the
//! session, the clipboard commands and the pointer handling for both modes.
//!
//! The pure parts (masks, clipping, transform results, clipboard) are headless and tested
//! here; the toolbars and overlays are in [`crate::panels::selection`] and
//! [`crate::panels::transform`]. Selection changes and transform commits are undo steps named
//! "Selection", "Transform", "Cut", "Paste" or "Duplicate".

use eframe::egui::{Pos2, Rect as ScreenRect};
use eskiz_core::canvas::Layer;
use eskiz_select::clipboard::{copy_all, copy_selection, cut_selection, paste, Clip};
use eskiz_select::feather::feather;
use eskiz_select::geom::{Mat3, Point, Quad, Rect};
use eskiz_select::outline::{outline, Polyline, OUTLINE_LEVEL};
use eskiz_select::raster::{self, FreehandStroke};
use eskiz_select::selection::SavedSelections;
use eskiz_select::transform::{
    distort_move_corner, free_drag, normalise_angle, snap_angle, translate_quad, uniform_drag, Handle, SnapMode,
};
use eskiz_select::wand::{flood_select, threshold_after_drag};
use eskiz_core::filters::upscale_layer_over;
use eskiz_select::{CombineMode, Image, Interp, Mask, Selection, TransformSession};

use crate::app::{EskizApp, FrameInput};
use crate::document::Document;
use crate::input::PointerKind;
use crate::session::{Edit, Session};
use crate::view::View;

/// Default Automatic threshold, as a fraction of the colour range.
pub const DEFAULT_THRESHOLD: f32 = 0.4;
/// Horizontal drag, in screen points, that counts as a move rather than a tap.
const TAP_SLOP_PT: f32 = 4.0;
/// Screen points within which a handle or a polygon's first vertex is picked.
const HANDLE_RADIUS_PT: f32 = 14.0;
/// Screen points within which a warp control point is picked.
pub(crate) const WARP_PICK_PT: f32 = 24.0;
/// Distance of the rotate handle from the top edge, screen points.
pub(crate) const ROTATE_HANDLE_PT: f32 = 36.0;

/// Which edit mode the bottom toolbar belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditMode {
    /// Paint as usual.
    #[default]
    Off,
    Select,
    Transform,
}

/// How a new selection is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Flood fill from the tapped pixel. Drag horizontally to change the threshold.
    Automatic,
    Freehand,
    /// Tap to add vertices. Tap near the first vertex to close the shape.
    Polygon,
    Rectangle,
    Ellipse,
}

/// A selection gesture in progress.
pub enum Pending {
    Idle,
    /// Automatic: the tapped pixel and the screen x and threshold when the press began.
    Auto { seed: (i64, i64), start_x: f32, base: f32 },
    Freehand(FreehandStroke),
    /// Polygon vertices placed so far, canvas points.
    Polygon(Vec<Point>),
    /// Rectangle or ellipse from `a` to `b`, canvas points.
    Drag { shape: Shape, a: Point, b: Point, rot: f32 },
}

/// Selection tool state. The selection itself lives in the session (it is undoable).
pub struct SelectionTool {
    pub shape: Shape,
    /// How the next shape combines with the selection: Replace, Add or Subtract.
    pub combine: CombineMode,
    /// Feather radius in canvas pixels, applied to new selections.
    pub feather_px: f32,
    pub threshold: f32,
    pub saved: SavedSelections,
    /// The document the saved selections belong to, and the ids and names last written for it
    /// (see [`crate::saved_store`]).
    pub store_key: Option<String>,
    pub store_sig: Vec<(u64, String)>,
    /// The clipboard: what Paste puts down.
    pub clip: Option<Clip>,
    pub pending: Pending,
    /// Automatic: marching ants for the threshold being dragged, before the commit.
    pub preview: Option<Vec<Polyline>>,
    /// The threshold readout shown in the top banner while Automatic is dragged.
    pub banner: Option<String>,
    /// Automatic: the active layer's pixels, taken when the press began.
    pub auto_source: Option<Image>,
    /// Automatic: the threshold the preview was last computed for during this drag.
    pub auto_last: Option<f32>,
    /// Marching ants for the committed selection, keyed by the session's edit counter.
    pub ants: Option<(u64, Vec<Polyline>)>,
}

impl Default for SelectionTool {
    fn default() -> Self {
        Self {
            shape: Shape::Rectangle,
            combine: CombineMode::Replace,
            feather_px: 0.0,
            threshold: DEFAULT_THRESHOLD,
            saved: SavedSelections::default(),
            store_key: None,
            store_sig: Vec::new(),
            clip: None,
            pending: Pending::Idle,
            preview: None,
            banner: None,
            auto_source: None,
            auto_last: None,
            ants: None,
        }
    }
}

/// Copy, cut, copy all, paste and duplicate: the clipboard commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipAction {
    Copy,
    Cut,
    CopyAll,
    Paste,
    Duplicate,
}

// ----- pure helpers -----

/// The active layer's pixels as a straight-alpha image, or `None` for a group.
pub fn layer_image(session: &Session, id: u64) -> Option<Image> {
    let layer = session.doc.layer(id)?;
    Image::from_premul_rgba8(layer.width, layer.height, &layer.to_rgba())
}

/// A straight-alpha RGBA8 buffer (the composite) as an image.
fn image_from_straight(width: usize, height: usize, bytes: &[u8]) -> Image {
    Image::from_fn(width, height, |x, y| {
        let i = (y * width + x) * 4;
        [
            f32::from(bytes[i]) / 255.0,
            f32::from(bytes[i + 1]) / 255.0,
            f32::from(bytes[i + 2]) / 255.0,
            f32::from(bytes[i + 3]) / 255.0,
        ]
    })
}

/// A rectangle or ellipse mask from two canvas points.
pub fn shape_mask(shape: Shape, a: Point, b: Point, width: usize, height: usize) -> Mask {
    let bbox = Rect::from_corners(a, b);
    match shape {
        Shape::Ellipse => raster::ellipse(width, height, bbox),
        _ => raster::rectangle(width, height, bbox),
    }
}

/// Combines a new mask with `sel` in `mode`, feathering the new mask first.
pub fn combine_into(sel: &Selection, mask: &Mask, mode: CombineMode, feather_px: f32) -> Result<Selection, String> {
    let shaped = feather(mask, feather_px);
    sel.combine(&shaped, mode).map_err(|_| "The selection and the new shape differ in size.".to_owned())
}

/// Marching-ants outlines for a selection (none when nothing is selected).
pub fn ants_for(sel: &Selection) -> Vec<Polyline> {
    sel.mask().map(|m| outline(m, OUTLINE_LEVEL)).unwrap_or_default()
}

/// Copies the layer inside the selection (the whole layer when nothing is selected).
pub fn copy_clip(session: &Session, id: u64) -> Option<Clip> {
    copy_selection(&layer_image(session, id)?, session.selection())
}

/// Copies the composite of the whole canvas.
pub fn copy_composite(session: &Session) -> Clip {
    let (w, h) = (session.doc.width, session.doc.height);
    copy_all(&image_from_straight(w, h, &session.flatten()))
}

// ----- transform -----

/// Which transform the bounding box applies to a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformMode {
    Free,
    Uniform,
    Distort,
    Warp,
}

/// What a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Corner(usize),
    Edge(usize),
    Rotate,
    Inside,
    /// Warp control point `(i, j)`.
    Control(usize, usize),
}

/// A drag in progress. Quads are measured from the start of the drag, so each frame is exact.
#[derive(Clone, Debug)]
pub struct Drag {
    pub hit: Hit,
    pub start: Quad,
    /// Canvas point where the press began.
    pub origin: Point,
    /// Canvas point at the last frame (warp control points move by the difference).
    pub last: Point,
    pub centre: Point,
    /// Angle of the press about `centre`, degrees.
    pub start_angle: f32,
    /// Rotation already applied by this drag, degrees.
    pub applied_deg: f32,
}

/// Transform in progress on one layer: a [`TransformSession`] plus the selection it acts on.
pub struct TransformTool {
    pub layer: u64,
    pub session: TransformSession,
    /// The selection when the transform began. `None` transforms the whole layer.
    pub mask: Option<Mask>,
    pub mode: TransformMode,
    /// Snap rotation to 15 degree steps.
    pub snap: bool,
    pub interp: Interp,
    pub drag: Option<Drag>,
    /// The transform changed since the preview was last drawn.
    pub dirty: bool,
    /// The layer with the footprint cleared (8-bit), under the moved content in the preview.
    /// Built on the first preview; the footprint does not change during a session.
    base: Option<Layer>,
    /// The drawing it was started on (`Session::document_generation`): the snapshot is stale in
    /// any other. Flip, Convert and Crop wait until the transform is finished.
    doc: u64,
}

/// Longest side the live preview of a transform is rendered at; larger canvases preview from a
/// proxy.
pub const PREVIEW_EDGE: usize = 1600;

/// Longest side up to which the preview is rendered in full.
const PREVIEW_FULL_EDGE: usize = 2048;

/// Canvases wider than this preview from a half-size proxy while a gesture is in progress.
const PREVIEW_DRAG_EDGE: usize = 1200;

/// The block factor of the preview proxy for a `w x h` canvas (1: full resolution). Up to
/// [`PREVIEW_FULL_EDGE`] pixels the picture at rest is rendered in full, and in two-pixel blocks
/// while `dragging`; bigger canvases always use a proxy about [`PREVIEW_EDGE`] pixels across.
pub fn preview_factor(w: usize, h: usize, dragging: bool) -> usize {
    let edge = w.max(h);
    if edge > PREVIEW_FULL_EDGE {
        edge.div_ceil(PREVIEW_EDGE)
    } else if dragging && edge > PREVIEW_DRAG_EDGE {
        2
    } else {
        1
    }
}

/// `layer` as 8-bit tiles with the selection's footprint cleared: pixels fade by `1 - selection`,
/// and fully faded ones go transparent. Tiles the selection does not reach are shared with the
/// layer, not copied.
fn cleared_base(layer: &Layer, footprint: &Mask) -> Layer {
    let mut base = if layer.depth() == eskiz_core::Depth::U8 { layer.clone() } else { layer.converted(eskiz_core::Depth::U8, false) };
    let w = layer.width;
    for ty in 0..base.tiles_y() {
        for tx in 0..base.tiles_x() {
            let (x0, y0, tw, th) = base.tile_rect(tx, ty);
            let reached = (y0..y0 + th).any(|y| footprint.data[y * w + x0..y * w + x0 + tw].iter().any(|&f| f > 0.0));
            if !reached || base.is_empty_tile(tx, ty) {
                continue;
            }
            let tile = base.tile_mut(tx, ty).data_mut();
            for r in 0..th {
                for c in 0..tw {
                    let keep = (1.0 - footprint.data[(y0 + r) * w + x0 + c]).clamp(0.0, 1.0);
                    let px = &mut tile[(r * eskiz_core::canvas::TILE + c) * 4..][..4];
                    let a = (f32::from(px[3]) * keep).round() as u8;
                    if a == 0 {
                        px.fill(0);
                    } else {
                        for v in px.iter_mut() {
                            *v = (f32::from(*v) * keep).round() as u8;
                        }
                    }
                }
            }
        }
    }
    base
}


/// Interpolation the transform starts with.
pub const DEFAULT_INTERP: Interp = Interp::Bicubic;

impl TransformTool {
    /// Starts a transform of `layer`. It acts on the selection when there is one (the selected
    /// pixels move, the rest of the layer stays), else on the whole layer.
    ///
    /// The session keeps the layer as the RGBA8 it is stored in (a quarter of the float image it
    /// used to hold), converts pixels as they are sampled, and the commit never builds a
    /// canvas-sized float image, so a transform of an 8000 px canvas fits in memory.
    pub fn start(session: &Session, layer: u64, interp: Interp) -> Result<Self, String> {
        let l = session.doc.layer(layer).ok_or("Select a layer to transform.")?;
        let mask = session.selection().mask().cloned();
        let transform = TransformSession::new_bytes(l.width, l.height, l.to_rgba(), mask.clone(), Mat3::IDENTITY, interp)
            .map_err(|_| "Nothing to transform on this layer.")?;
        Ok(Self {
            layer,
            session: transform,
            mask,
            mode: TransformMode::Free,
            snap: true,
            interp,
            drag: None,
            dirty: true,
            base: None,
            doc: session.document_generation(),
        })
    }

    /// The layer as it will be after the commit (premultiplied, canvas sized).
    pub fn result(&self, session: &Session) -> Option<Layer> {
        let layer = session.doc.layer(self.layer)?;
        // The pixels that stay put are only needed when a selection leaves part of the layer.
        let base = self.mask.is_some().then(|| layer.to_rgba());
        let out = self.session.commit_premul(base.as_deref()).ok()?;
        Some(Layer::from_rgba(layer.width, layer.height, out))
    }

    /// The live preview: the layer with the footprint cleared and the moved content over it. A
    /// big canvas is shown from a proxy (the source averaged over blocks, sampled bilinearly,
    /// repeated into tiles), at most [`PREVIEW_EDGE`] pixels across; the commit renders at full
    /// resolution with the session's kernel.
    pub fn preview(&mut self, session: &Session) -> Option<Layer> {
        let layer = session.doc.layer(self.layer)?;
        let (w, h) = (layer.width, layer.height);
        let dragging = self.drag.is_some();
        let k = preview_factor(w, h, dragging);
        // Bilinear while a gesture is in progress or the preview is a proxy; the chosen kernel
        // once the picture is at rest at full size.
        let interp = if dragging || k > 1 { Interp::Bilinear } else { self.interp };
        let (pw, ph, pixels) = self.session.preview_premul(k, interp).ok()?;
        if self.base.is_none() {
            self.base = Some(match &self.mask {
                Some(m) => cleared_base(layer, m),
                None => Layer::new(w, h),
            });
        }
        Some(upscale_layer_over(&pixels, pw, ph, w, h, k, self.base.as_ref()))
    }

    /// The selection after the commit: the marquee carried through the transform.
    pub fn selection_after(&self) -> Selection {
        match &self.mask {
            Some(m) => Selection::active(self.session.marquee(m)),
            None => Selection::none(),
        }
    }

    /// Commits the transform as one undo step named `name`.
    pub fn commit(&self, session: &mut Session, name: &'static str) -> Option<Edit> {
        let layer = self.result(session)?;
        session.replace_layer_and_selection(self.layer, layer, name, Some(self.selection_after()))
    }
}

// ----- the operations on the app -----

impl EskizApp {
    /// Leaves Selection or Transform. A transform in progress is cancelled.
    pub(crate) fn exit_edit_mode(&mut self) {
        self.cancel_transform();
        self.edit_mode = EditMode::Off;
        self.sel.pending = Pending::Idle;
        self.sel.preview = None;
        self.sel.banner = None;
        self.sel.auto_source = None;
    }

    /// The top-bar Selection button.
    pub(crate) fn toggle_select_mode(&mut self) {
        if self.refuse_in_3d() {
            return;
        }
        if self.transform.is_some() {
            self.show_toast("Finish or cancel the transform first.", 2.0);
        } else if self.edit_mode == EditMode::Select {
            self.exit_edit_mode();
        } else {
            self.popovers.close();
            self.edit_mode = EditMode::Select;
        }
    }

    /// Starts a transform of the active layer (acting on the selection if there is one).
    pub(crate) fn begin_transform(&mut self) {
        if self.refuse_in_3d() {
            return;
        }
        if self.transform.is_some() {
            return;
        }
        if self.adjust.is_some() {
            return;
        }
        if !self.session.can_edit_pixels(self.active) {
            self.show_toast("Select an editable layer to transform.", 2.0);
            return;
        }
        match TransformTool::start(&self.session, self.active, DEFAULT_INTERP) {
            Ok(t) => {
                self.popovers.close();
                self.sel.pending = Pending::Idle;
                self.transform = Some(t);
                self.edit_mode = EditMode::Transform;
                self.refresh_transform_preview_now();
            }
            Err(msg) => self.show_toast(msg, 2.0),
        }
    }

    /// Redraws the canvas with the transform's live preview.
    pub(crate) fn refresh_transform_preview_now(&mut self) {
        let Some(t) = self.transform.as_mut() else {
            return;
        };
        t.dirty = false;
        if let Some(layer) = t.preview(&self.session) {
            self.session.set_preview_layer(t.layer, layer);
            self.needs_full = true;
        }
    }

    /// Once per frame: end a transform whose snapshot is stale (another drawing was opened, or
    /// the canvas was flipped, cropped or converted), so Apply never writes old pixels.
    pub(crate) fn transform_guard(&mut self) {
        let Some(t) = self.transform.as_ref() else {
            return;
        };
        if self.session.document_generation() != t.doc || self.session.doc.layer(t.layer).is_none() {
            self.cancel_transform();
            self.show_toast("The transform was cancelled because the drawing changed.", 2.5);
        }
    }

    /// Commits the transform: one "Transform" undo step.
    pub(crate) fn apply_transform(&mut self) {
        let Some(t) = self.transform.take() else {
            return;
        };
        self.session.clear_preview();
        self.edit_mode = EditMode::Off;
        self.needs_full = true;
        match t.commit(&mut self.session, "Transform") {
            Some(edit) => self.apply_refresh(edit.refresh),
            None => self.show_toast("The transform could not be applied.", 2.0),
        }
    }

    /// Drops the transform without changing the layer.
    pub(crate) fn cancel_transform(&mut self) {
        if self.transform.take().is_some() {
            self.session.clear_preview();
            self.needs_full = true;
        }
        if self.edit_mode == EditMode::Transform {
            self.edit_mode = EditMode::Off;
        }
    }

    /// Runs one clipboard command on the active layer and the selection.
    pub(crate) fn clip_action(&mut self, action: ClipAction) {
        if self.refuse_in_3d() {
            return;
        }
        if self.transform.is_some() {
            self.show_toast("Finish or cancel the transform first.", 2.0);
            return;
        }
        match action {
            ClipAction::Copy => match copy_clip(&self.session, self.active) {
                Some(clip) => {
                    self.sel.clip = Some(clip);
                    self.clip_to_system();
                    self.show_toast("Copied", 1.0);
                }
                None => self.show_toast("Nothing to copy.", 1.5),
            },
            ClipAction::CopyAll => {
                self.sel.clip = Some(copy_composite(&self.session));
                self.clip_to_system();
                self.show_toast("Copied all", 1.0);
            }
            ClipAction::Cut => self.cut_selection_pixels(),
            // An image another app copied wins over Eskiz's own clip (the system clipboard no
            // longer holds Eskiz's copy then). Desktop has no system clipboard: always the own clip.
            ClipAction::Paste if self.start_foreign_paste() => {}
            ClipAction::Paste => match self.sel.clip.clone() {
                Some(clip) => {
                    if let Some(id) = self.paste_clip(&clip, "Paste") {
                        self.active = id;
                        self.begin_transform();
                    }
                }
                None => self.show_toast("Nothing copied yet.", 1.5),
            },
            ClipAction::Duplicate => match copy_clip(&self.session, self.active) {
                Some(clip) => {
                    self.paste_clip(&clip, "Duplicate");
                }
                None => self.show_toast("Nothing to duplicate.", 1.5),
            },
        }
    }

    /// Cut: the selected pixels go to the clipboard and leave the layer ("Cut").
    fn cut_selection_pixels(&mut self) {
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let Some(image) = layer_image(&self.session, self.active) else {
            self.show_toast("Select a layer to cut from.", 1.5);
            return;
        };
        let Some((clip, cleared)) = cut_selection(&image, self.session.selection()) else {
            self.show_toast("Nothing to cut.", 1.5);
            return;
        };
        let layer = Layer::from_rgba(w, h, cleared.to_premul_rgba8());
        match self.session.replace_layer_and_selection(self.active, layer, "Cut", None) {
            Some(edit) => {
                self.apply_refresh(edit.refresh);
                self.sel.clip = Some(clip);
                self.clip_to_system();
            }
            None => self.show_toast("The layer cannot be edited.", 1.5),
        }
    }

    /// Puts `clip` at its original place on a new layer above the active one.
    fn paste_clip(&mut self, clip: &Clip, step: &'static str) -> Option<u64> {
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let pixels = paste(clip, w, h, None).to_premul_rgba8();
        let edit = self.session.import_layer_named(Some(self.active), step, pixels, step);
        match edit {
            Some(edit) => {
                self.apply_refresh(edit.refresh);
                self.needs_full = true;
                edit.id
            }
            None => {
                self.show_toast("Could not paste here.", 1.5);
                None
            }
        }
    }

    /// Replaces the selection with `mask`, combined with the current one, as one undo step.
    fn commit_selection_mask(&mut self, mask: Mask) {
        match combine_into(self.session.selection(), &mask, self.sel.combine, self.sel.feather_px) {
            Ok(sel) => {
                if self.session.set_selection("Selection", sel).is_none() {
                    self.show_toast("The selection cannot change while painting.", 1.5);
                }
            }
            Err(msg) => self.show_toast(msg, 2.0),
        }
    }

    /// Routes the pointer while Selection or Transform is on.
    pub(crate) fn edit_pointer(&mut self, input: &FrameInput, pressed_on_ui: bool) {
        match self.edit_mode {
            EditMode::Select => self.select_pointer(input, pressed_on_ui),
            EditMode::Transform => self.transform_pointer(input, pressed_on_ui),
            EditMode::Off => {}
        }
    }

    /// The selection gestures: tap, drag and polygon taps.
    fn select_pointer(&mut self, input: &FrameInput, pressed_on_ui: bool) {
        let vp = self.viewport;
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let zoom = self.view.zoom;
        if input.primary_pressed && !pressed_on_ui {
            if let Some(p) = input.press_origin {
                let c = canvas_pt(&self.view, vp, p);
                match self.sel.shape {
                    Shape::Automatic => {
                        self.sel.auto_source = layer_image(&self.session, self.active);
                        self.sel.auto_last = None;
                        if self.sel.auto_source.is_some() {
                            let seed = (c.x.floor() as i64, c.y.floor() as i64);
                            self.sel.pending = Pending::Auto {
                                seed,
                                start_x: p.x,
                                base: self.sel.threshold,
                            };
                        }
                    }
                    Shape::Freehand => {
                        let mut stroke = FreehandStroke::with_spacing(2.0);
                        stroke.push(c);
                        self.sel.pending = Pending::Freehand(stroke);
                    }
                    Shape::Polygon => {
                        let mut pts = match std::mem::replace(&mut self.sel.pending, Pending::Idle) {
                            Pending::Polygon(pts) => pts,
                            _ => Vec::new(),
                        };
                        let close = pts.len() >= 3 && pts[0].distance(c) * zoom <= HANDLE_RADIUS_PT;
                        if close {
                            self.commit_selection_mask(raster::polygon(w, h, &pts));
                        } else {
                            pts.push(c);
                            self.sel.pending = Pending::Polygon(pts);
                        }
                    }
                    shape @ (Shape::Rectangle | Shape::Ellipse) => {
                        self.sel.pending = Pending::Drag { shape, a: c, b: c, rot: self.view.rotation };
                    }
                }
            }
        }

        let releasing = input.primary_released || !input.primary_down;
        if input.primary_down {
            if let Some(p) = input.pointer {
                let c = canvas_pt(&self.view, vp, p);
                let mut auto_update = None;
                match &mut self.sel.pending {
                    Pending::Freehand(stroke) => stroke.push(c),
                    Pending::Drag { b, .. } => *b = c,
                    Pending::Auto { seed, start_x, base } => {
                        let dx = p.x - *start_x;
                        if dx.abs() > TAP_SLOP_PT {
                            auto_update = Some((*seed, threshold_after_drag(*base, dx, 1.0)));
                        }
                    }
                    _ => {}
                }
                if let Some((seed, threshold)) = auto_update {
                    self.sel.threshold = threshold;
                    self.sel.banner = Some(format!("Threshold {:.0}%", threshold * 100.0));
                    // A pen that rests keeps the same threshold: the preview is already right.
                    let unchanged = self.sel.auto_last == Some(threshold);
                    if let (false, Some(src)) = (unchanged, &self.sel.auto_source) {
                        self.sel.auto_last = Some(threshold);
                        let mask = flood_select(src, seed, threshold);
                        self.sel.preview = Some(outline(&mask, OUTLINE_LEVEL));
                    }
                }
            }
        }

        let finishing = releasing && !matches!(self.sel.pending, Pending::Idle | Pending::Polygon(_));
        if finishing {
            let pending = std::mem::replace(&mut self.sel.pending, Pending::Idle);
            match pending {
                Pending::Auto { seed, .. } => {
                    if let Some(src) = self.sel.auto_source.take() {
                        let mask = flood_select(&src, seed, self.sel.threshold);
                        self.commit_selection_mask(mask);
                    }
                }
                Pending::Freehand(stroke) => self.commit_selection_mask(stroke.finish(w, h)),
                Pending::Drag { shape, a, b, rot } => self.commit_selection_mask(drag_mask(shape, a, b, rot, w, h)),
                Pending::Idle | Pending::Polygon(_) => {}
            }
            self.sel.preview = None;
            self.sel.banner = None;
            self.sel.auto_source = None;
        }
    }

    /// Marching ants for the committed selection, and the shape being drawn.
    pub(crate) fn selection_outlines(&mut self) {
        let key = self.session.edits();
        if self.sel.ants.as_ref().map(|a| a.0) != Some(key) {
            let lines = ants_for(self.session.selection());
            self.sel.ants = Some((key, lines));
        }
    }
}

/// Screen point to canvas point.
pub(crate) fn canvas_pt(view: &View, vp: ScreenRect, p: Pos2) -> Point {
    let (x, y) = view.screen_to_canvas(vp, p);
    Point::new(x as f32, y as f32)
}

/// Rotation of a drag about `centre`, in degrees, for a pointer at `p`.
pub(crate) fn angle_about(centre: Point, p: Point) -> f32 {
    (p.y - centre.y).atan2(p.x - centre.x).to_degrees()
}

/// The free-transform drag result for a handle, the quad the drag produces.
pub(crate) fn quad_for_drag(mode: TransformMode, hit: Hit, start: Quad, delta: Point) -> Option<Quad> {
    let handle = match hit {
        Hit::Corner(i) => Handle::Corner(i),
        Hit::Edge(i) => Handle::Edge(i),
        _ => return None,
    };
    Some(match (mode, hit) {
        (TransformMode::Uniform, _) => uniform_drag(start, handle, delta),
        (TransformMode::Distort, Hit::Corner(i)) => {
            distort_move_corner(start, i, Point::new(start[i].x + delta.x, start[i].y + delta.y))
        }
        _ => free_drag(start, handle, delta),
    })
}

/// Moves a quad by `delta` (press inside the box).
pub(crate) fn moved_quad(start: Quad, delta: Point) -> Quad {
    translate_quad(start, delta)
}

/// Rotation step for a drag, with snapping. `total` is the drag's angle so far.
pub(crate) fn snapped(total: f32, snap: bool) -> f32 {
    let t = normalise_angle(total);
    if snap {
        snap_angle(t, SnapMode::Nearest)
    } else {
        t
    }
}

/// Button presses on the Transform toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformAction {
    Mode(TransformMode),
    Snap,
    FlipH,
    FlipV,
    Rotate45,
    Fit,
    Interp(Interp),
    Apply,
    Cancel,
}

/// Is `rot` (radians) a whole number of quarter turns, so a screen-aligned box is also canvas-aligned?
fn is_quarter_turn(rot: f32) -> bool {
    let q = std::f32::consts::FRAC_PI_2;
    ((rot / q).round() * q - rot).abs() < 1e-4
}

/// Shape of a rectangle or ellipse as a closed canvas polyline, for the preview. The box is the
/// one the pointer drew on screen: on a view rotated by `rot` radians it is a rotated box on the
/// canvas (a whole number of quarter turns keeps it canvas-aligned).
pub(crate) fn preview_polyline(shape: Shape, a: Point, b: Point, rot: f32) -> Vec<Point> {
    if is_quarter_turn(rot) {
        return match shape {
            Shape::Ellipse => {
                let c = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                let (rx, ry) = ((b.x - a.x).abs() / 2.0, (b.y - a.y).abs() / 2.0);
                ellipse_points(|u, v| Point::new(c.x + u * rx, c.y + v * ry))
            }
            _ => vec![a, Point::new(b.x, a.y), b, Point::new(a.x, b.y)],
        };
    }
    let (sn, cs) = rot.sin_cos();
    // Canvas vector to the screen frame and back (rotation only; the zoom is uniform).
    let to_screen = |x: f32, y: f32| (x * cs - y * sn, x * sn + y * cs);
    let to_canvas = |x: f32, y: f32| (x * cs + y * sn, -x * sn + y * cs);
    let (dx, dy) = to_screen(b.x - a.x, b.y - a.y);
    let at = |u: f32, v: f32| {
        let (x, y) = to_canvas(u, v);
        Point::new(a.x + x, a.y + y)
    };
    match shape {
        Shape::Ellipse => ellipse_points(|u, v| at(dx / 2.0 + u * dx.abs() / 2.0, dy / 2.0 + v * dy.abs() / 2.0)),
        _ => vec![at(0.0, 0.0), at(dx, 0.0), at(dx, dy), at(0.0, dy)],
    }
}

/// 96 points on the unit circle, mapped through `f`.
fn ellipse_points(f: impl Fn(f32, f32) -> Point) -> Vec<Point> {
    (0..96)
        .map(|i| {
            let t = std::f32::consts::TAU * i as f32 / 96.0;
            f(t.cos(), t.sin())
        })
        .collect()
}

/// The mask of a rectangle or ellipse dragged from `a` to `b` on a view rotated by `rot` radians.
pub fn drag_mask(shape: Shape, a: Point, b: Point, rot: f32, width: usize, height: usize) -> Mask {
    if is_quarter_turn(rot) {
        shape_mask(shape, a, b, width, height)
    } else {
        raster::polygon(width, height, &preview_polyline(shape, a, b, rot))
    }
}

// ----- headless checks, shared by the unit tests and `--selftest` -----

/// Paint across the edge of a selected rectangle. Nothing may land outside it. Returns the
/// number of painted pixels inside.
pub fn check_clip_to_selection() -> Result<usize, String> {
    use crate::input::{sample_from, RawPointer};
    use crate::paint::StrokeSettings;
    use eskiz_core::brushes::{Brush, Preset};

    let (w, h) = (300usize, 200usize);
    let mut s = Session::new(Document::new(w, h));
    let id = s.doc.layer_id(0).ok_or("no layer")?;
    let mask = raster::rectangle(w, h, Rect::from_corners(Point::new(100.0, 50.0), Point::new(200.0, 150.0)));
    s.set_selection("Selection", Selection::active(mask)).ok_or("could not select")?;

    let mut brush = Brush::preset(Preset::TechnicalPen);
    brush.size = 40.0;
    let settings = StrokeSettings { brush, colour: [0.0; 3], erase: false, raster: None };
    if !s.begin_stroke(id, &settings) {
        return Err("could not start stroke".into());
    }
    for i in 0..60 {
        let x = 10.0 + 280.0 * f64::from(i) / 59.0;
        let raw = RawPointer { x, y: 100.0, time: f64::from(i) / 120.0, kind: PointerKind::Mouse, force: None };
        s.push(sample_from(raw));
    }
    s.end_stroke().ok_or("stroke painted nothing")?;

    let px = s.doc.layer(id).ok_or("no layer")?.to_rgba();
    let mut inside = 0usize;
    for y in 0..h {
        for x in 0..w {
            let alpha = px[(y * w + x) * 4 + 3];
            let in_rect = (100..200).contains(&x) && (50..150).contains(&y);
            match (in_rect, alpha) {
                (true, a) if a > 0 => inside += 1,
                (false, 0) => {}
                (false, a) => return Err(format!("paint landed outside the selection at ({x}, {y}): alpha {a}")),
                _ => {}
            }
        }
    }
    if inside == 0 {
        return Err("nothing was painted inside the selection".into());
    }
    Ok(inside)
}

/// Rotate a 8x8 block 90 degrees and check it moved, then undo and check the layer is back.
/// Returns the number of pixels in the block after the rotation.
pub fn check_rotate_and_undo() -> Result<usize, String> {
    let n = 64usize;
    let mut s = Session::new(Document::new(n, n));
    let id = s.doc.layer_id(0).ok_or("no layer")?;
    let mut pixels = vec![0u8; n * n * 4];
    for y in 4..12 {
        for x in 4..12 {
            let i = (y * n + x) * 4;
            pixels[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    s.replace_layer_pixels(id, pixels, "Fill").ok_or("could not fill")?;
    let before = s.doc.layer(id).ok_or("no layer")?.to_rgba();

    let mut t = TransformTool::start(&s, id, Interp::Nearest)?;
    t.session.apply_gesture(eskiz_select::transform::rotate_about(Point::new(32.0, 32.0), 90.0));
    t.commit(&mut s, "Transform").ok_or("commit refused")?;

    let after = s.doc.layer(id).ok_or("no layer")?.to_rgba();
    let alpha = |x: usize, y: usize| after[(y * n + x) * 4 + 3];
    if alpha(8, 8) != 0 {
        return Err("the block did not leave its place".into());
    }
    let block = |xs: std::ops::Range<usize>, ys: std::ops::Range<usize>| xs.clone().all(|x| ys.clone().all(|y| alpha(x, y) > 0));
    if !(block(52..60, 4..12) || block(4..12, 52..60)) {
        return Err("the block did not land at its rotated place".into());
    }
    let count = after.chunks(4).filter(|p| p[3] > 0).count();
    s.undo().ok_or("undo unavailable")?;
    if s.doc.layer(id).ok_or("no layer")?.to_rgba() != before {
        return Err("undo did not restore the layer".into());
    }
    Ok(count)
}

/// Selftest lines for the Selection and Transform tools.
pub fn selftest_checks() -> Result<Vec<String>, String> {
    let inside = check_clip_to_selection()?;
    let moved = check_rotate_and_undo()?;
    Ok(vec![
        format!("  selection: paint across a selected edge stays inside ({inside} px painted, 0 outside): ok"),
        format!("  transform: rotate 90 moves the block ({moved} px), undo restores the layer: ok"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_clips_paint_to_its_edge() {
        let inside = check_clip_to_selection().unwrap();
        assert!(inside > 1000);
    }

    /// A document whose first layer holds seeded random (valid premultiplied) pixels in a margin-free
    /// block, with an optional feathered selection.
    fn noisy_session(w: usize, h: usize, selected: bool) -> (Session, u64) {
        let mut s = Session::new(Document::new(w, h));
        let id = s.doc.layer_id(0).expect("layer");
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 24
        };
        let mut px = vec![0u8; w * h * 4];
        for y in 10..h - 10 {
            for x in 12..w - 12 {
                let a = [255u64, 255, 0, next() % 256][(next() % 4) as usize] as u8;
                let i = (y * w + x) * 4;
                for c in 0..3 {
                    px[i + c] = if a == 0 { 0 } else { (next() % (u64::from(a) + 1)) as u8 };
                }
                px[i + 3] = a;
            }
        }
        s.replace_layer_pixels(id, px, "Fill").expect("fill");
        if selected {
            let data = (0..w * h).map(|i| {
                let (x, y) = (i % w, i / w);
                if (40..w - 60).contains(&x) && (30..h - 50).contains(&y) { 0.25 + 0.75 * (((x + y) % 7) as f32 / 6.0) } else { 0.0 }
            });
            let mask = Mask { width: w, height: h, data: data.collect() };
            s.set_selection("Selection", Selection::active(mask)).expect("selection");
        }
        (s, id)
    }

    /// The byte-backed tool commits the same layer the float image path did: same pixels, with and
    /// without a (feathered) selection, for a rotation with scaling.
    #[test]
    fn transform_result_equals_the_float_image_path() {
        let (w, h) = (300usize, 240usize);
        for selected in [false, true] {
            for interp in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
                let (s, id) = noisy_session(w, h, selected);
                let mut t = TransformTool::start(&s, id, interp).expect("start");
                t.session.apply_gesture((Mat3::rotate_deg(21.0) * Mat3::scale(1.15, 0.9)).about(Point::new(150.0, 120.0)));
                let got = t.result(&s).expect("result").to_rgba();
                // The float path, as the tool was before.
                let image = layer_image(&s, id).expect("image");
                let (source, footprint) = match s.selection().mask() {
                    Some(m) => {
                        let mut src = image.clone();
                        for (px, &v) in src.pixels.iter_mut().zip(&m.data) {
                            px[3] *= v;
                        }
                        (src, m.clone())
                    }
                    None => (image.clone(), Mask::full(w, h)),
                };
                let mut float = TransformSession::new(source, footprint, Mat3::IDENTITY, interp).expect("session");
                float.apply_gesture((Mat3::rotate_deg(21.0) * Mat3::scale(1.15, 0.9)).about(Point::new(150.0, 120.0)));
                let want = float.commit(&image).expect("commit").to_premul_rgba8();
                assert!(got == want, "selected {selected} {interp:?}");
            }
        }
    }

    /// The live preview keeps the part of the layer outside the footprint, clears the footprint
    /// where the content left, and shows the moved content.
    #[test]
    fn transform_preview_shows_base_and_moved_content() {
        let (w, h) = (300usize, 240usize);
        let (s, id) = noisy_session(w, h, true);
        let before = s.doc.layer(id).expect("layer").to_rgba();
        let mut t = TransformTool::start(&s, id, Interp::Bicubic).expect("start");
        t.session.apply_gesture(Mat3::translate(0.0, 100.0));
        let preview = t.preview(&s).expect("preview");
        assert_eq!((preview.width, preview.height), (w, h));
        let px = preview.to_rgba();
        // Outside the selection, away from the moved content, the layer is as it was.
        let (x, y) = (20usize, 20usize);
        assert_eq!(px[(y * w + x) * 4..][..4], before[(y * w + x) * 4..][..4]);
        // Inside the selection the content moved down by 100: the top of the footprint (rows
        // 30..130) shows no more than the faded-out remainder, the rows it moved to show content.
        let alpha = |x: usize, y: usize| px[(y * w + x) * 4 + 3];
        let moved: u32 = (140..180).map(|y| (60..200).map(|x| u32::from(alpha(x, y))).sum::<u32>()).sum();
        assert!(moved > 0, "moved content is visible");
        // An unselected transform of the whole layer leaves nothing at the old place.
        let (s2, id2) = noisy_session(w, h, false);
        let mut t2 = TransformTool::start(&s2, id2, Interp::Bicubic).expect("start");
        t2.session.apply_gesture(Mat3::translate(0.0, 200.0));
        let p2 = t2.preview(&s2).expect("preview").to_rgba();
        assert!((20..60).all(|y| (20..280).all(|x| p2[(y * w + x) * 4 + 3] == 0)), "the old place is empty");
    }

    #[test]
    fn preview_factor_follows_the_canvas() {
        assert_eq!(preview_factor(2048, 2048, false), 1);
        assert_eq!(preview_factor(2048, 2048, true), 2, "a drag on a 2048 px canvas previews in 2 px blocks");
        assert_eq!(preview_factor(1024, 768, true), 1);
        assert_eq!(preview_factor(4096, 4096, false), 3);
        assert_eq!(preview_factor(8000, 8000, true), 5);
    }

    #[test]
    fn transform_commit_moves_pixels_and_undo_restores() {
        assert_eq!(check_rotate_and_undo().unwrap(), 64);
    }

    #[test]
    fn selection_changes_are_one_named_undo_step() {
        let mut s = Session::new(Document::new(40, 40));
        let mask = raster::rectangle(40, 40, Rect::from_corners(Point::new(4.0, 4.0), Point::new(20.0, 20.0)));
        s.set_selection("Selection", Selection::active(mask)).unwrap();
        assert_eq!(s.undo_name(), Some("Selection"));
        s.undo().unwrap();
        assert!(!s.selection().is_active());
        s.redo().unwrap();
        assert!(s.selection().is_active());
    }

    #[test]
    fn paste_makes_a_named_layer_that_undo_removes() {
        let mut s = Session::new(Document::new(40, 40));
        let id = s.doc.layer_id(0).unwrap();
        let mut px = vec![0u8; 40 * 40 * 4];
        px[..4].copy_from_slice(&[10, 20, 30, 255]);
        s.replace_layer_pixels(id, px, "Fill").unwrap();
        let clip = copy_clip(&s, id).unwrap();
        let layers = s.doc.layer_count();
        let edit = s.import_layer_named(Some(id), "Paste", paste(&clip, 40, 40, None).to_premul_rgba8(), "Paste").unwrap();
        assert_eq!(s.doc.layer_count(), layers + 1);
        assert_eq!(s.undo_name(), Some("Paste"));
        assert!(edit.id.is_some());
        s.undo().unwrap();
        assert_eq!(s.doc.layer_count(), layers);
    }

    #[test]
    fn combine_modes_follow_the_selection_rules() {
        let a = raster::rectangle(20, 20, Rect::from_corners(Point::new(0.0, 0.0), Point::new(10.0, 20.0)));
        let b = raster::rectangle(20, 20, Rect::from_corners(Point::new(5.0, 0.0), Point::new(20.0, 20.0)));
        let sel = combine_into(&Selection::none(), &a, CombineMode::Replace, 0.0).unwrap();
        let added = combine_into(&sel, &b, CombineMode::Add, 0.0).unwrap();
        assert!(added.value_at(15, 10) > 0.5);
        let cut = combine_into(&added, &b, CombineMode::Subtract, 0.0).unwrap();
        // Subtracting `b` removes what it covers: only the left strip (x < 5) remains.
        assert!(cut.value_at(2, 10) > 0.5 && cut.value_at(7, 10) < 0.5);
    }
}
