//! Canvas operations: Crop & Resize, Flip canvas and Canvas information.
//!
//! * [`plan`] works out what a crop and resize does to the canvas size, every layer, every mask
//!   and the selection. It is pure, so its rules are tested without a window.
//! * [`CanvasOp`] is the document-wide change. It keeps the canvas before and after, so undo and
//!   redo are exact. The operation and the selection change it implies are one undo step.
//! * [`EskizApp::crop_frame`] draws the crop rectangle, with its handles, over the canvas and the
//!   Crop & Resize sheet. [`EskizApp::canvas_info_window`] is the Canvas information sheet.

use eframe::egui::{self, Align2, Color32, Id, Order, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use eskiz_core::canvas::Layer;
use eskiz_core::document::{CanvasPixels, Document};
use eskiz_select::canvas::{anchor_offset, resize_image, resize_mask, Anchor, MAX_SIDE, MIN_SIDE};
use eskiz_select::image::Image;
use eskiz_select::mask::Mask as SelMask;
use eskiz_select::resample::Interp;
use eskiz_select::Selection;

use crate::app::EskizApp;
use crate::command::{DocOp, Refresh};
use crate::session::Session;

/// The canvas as it was or as it becomes: size, resolution and every layer's and mask's pixels.
#[derive(Clone)]
pub struct CanvasState {
    pub width: usize,
    pub height: usize,
    pub dpi: f64,
    pub pixels: CanvasPixels,
}

/// A whole-canvas change. Undo restores `before`, redo restores `after`.
pub struct CanvasOp {
    before: CanvasState,
    after: CanvasState,
}

impl DocOp for CanvasOp {
    fn apply(&self, doc: &mut Document) {
        restore(doc, &self.after);
    }

    fn revert(&self, doc: &mut Document) {
        restore(doc, &self.before);
    }

    fn bytes(&self) -> usize {
        let tiles = |px: &CanvasPixels| px.layers.iter().map(|(_, l)| l.tile_bytes()).sum::<usize>();
        tiles(&self.before.pixels) + tiles(&self.after.pixels)
    }
}

fn restore(doc: &mut Document, s: &CanvasState) {
    doc.restore_canvas(s.width, s.height, s.dpi, &s.pixels);
}

/// What a Crop & Resize does, as read from the sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropSpec {
    /// The kept region in canvas pixels: x, y, width, height. It may reach past the canvas, which
    /// adds transparent pixels there.
    pub rect: [i64; 4],
    /// The canvas size after the operation.
    pub width: usize,
    pub height: usize,
    /// Where the kept region sits on the new canvas when it is not resampled.
    pub anchor: Anchor,
    /// Resample the kept region to the new size with this filter, instead of anchoring it.
    pub resample: Option<Interp>,
    /// Resolution after the operation, in dots per inch.
    pub dpi: f64,
}

impl CropSpec {
    /// The design notes that keeps a `width` x `height` canvas as it is.
    pub fn identity(width: usize, height: usize, dpi: f64) -> Self {
        Self {
            rect: [0, 0, width as i64, height as i64],
            width,
            height,
            anchor: Anchor::Centre,
            resample: None,
            dpi,
        }
    }
}

/// Most bytes of intermediate pixel buffers a crop or resize may need (4 GiB).
const MAX_PLAN_BYTES: u64 = 4 << 30;

/// Work out a crop and resize of `doc`: the new canvas, every layer and mask, and the new
/// selection (`None` when there is no selection to change). Errors say what is wrong with the
/// values on the sheet.
pub fn plan(doc: &Document, selection: &Selection, spec: &CropSpec) -> Result<(CanvasOp, Option<Selection>), String> {
    let [_, _, rw, rh] = spec.rect;
    if rw < 1 || rh < 1 {
        return Err("The crop area is empty.".to_owned());
    }
    if rw as usize > MAX_SIDE || rh as usize > MAX_SIDE {
        return Err(format!("The crop area is larger than {MAX_SIDE} pixels on a side."));
    }
    for side in [spec.width, spec.height] {
        if !(MIN_SIDE..=MAX_SIDE).contains(&side) {
            return Err(format!("The canvas must be between {MIN_SIDE} and {MAX_SIDE} pixels on each side."));
        }
    }
    if !(spec.dpi.is_finite() && (1.0..=10_000.0).contains(&spec.dpi)) {
        return Err("The resolution must be between 1 and 10000 DPI.".to_owned());
    }
    let (cw, ch) = (doc.width, doc.height);
    // Every layer is rebuilt at full size, so bound the total before allocating any of it.
    let buffers = doc.layer_ids().len() as u64 * 2 + 2;
    let largest = (spec.width as u64 * spec.height as u64).max(cw as u64 * ch as u64);
    if largest.saturating_mul(4).saturating_mul(buffers) > MAX_PLAN_BYTES {
        return Err(
            "The canvas is too large for this many layers. Use a smaller size or fewer layers."
                .to_owned(),
        );
    }
    let before = CanvasState {
        width: cw,
        height: ch,
        dpi: doc.dpi,
        pixels: doc.canvas_pixels(),
    };
    let mut after = CanvasState {
        width: spec.width,
        height: spec.height,
        dpi: spec.dpi,
        pixels: before.pixels.clone(),
    };
    for (_, layer) in &mut after.pixels.layers {
        let rgba = reshape_rgba(&layer.to_rgba(), cw, ch, spec)?;
        let mut next = Layer::from_rgba(spec.width, spec.height, rgba);
        next.visible = layer.visible;
        next.opacity = layer.opacity;
        *layer = next;
    }
    // A mask's padding reveals (1.0), as a new layer mask does.
    for (_, mask) in &mut after.pixels.masks {
        if let Some(m) = mask {
            let values: Vec<f32> = m.data.iter().map(|&v| f32::from(v) / 255.0).collect();
            let out = reshape_f32(&values, cw, ch, spec, 1.0)?;
            m.data = out.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
        }
    }
    // A new selection area is not selected.
    let new_selection = match selection.mask() {
        Some(m) => {
            let data = reshape_f32(&m.data, cw, ch, spec, 0.0)?;
            Some(Selection::active(SelMask {
                width: spec.width,
                height: spec.height,
                data,
            }))
        }
        None => None,
    };
    Ok((CanvasOp { before, after }, new_selection))
}

/// The kept region of a `width` x `height` buffer, with `fill` where it reaches past the edge.
fn cut<T: Copy>(src: &[T], width: usize, height: usize, rect: [i64; 4], fill: T) -> Vec<T> {
    let [x0, y0, w, h] = rect;
    let mut out = Vec::with_capacity(w as usize * h as usize);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let inside = (0..width as i64).contains(&x) && (0..height as i64).contains(&y);
            out.push(if inside { src[y as usize * width + x as usize] } else { fill });
        }
    }
    out
}

/// Copy a `sw` x `sh` region into a `dw` x `dh` buffer at `offset`, with `fill` everywhere else.
fn place<T: Copy>(src: &[T], sw: usize, sh: usize, dw: usize, dh: usize, offset: (i64, i64), fill: T) -> Vec<T> {
    let mut out = vec![fill; dw * dh];
    for y in 0..sh {
        let dy = y as i64 + offset.1;
        if !(0..dh as i64).contains(&dy) {
            continue;
        }
        for x in 0..sw {
            let dx = x as i64 + offset.0;
            if (0..dw as i64).contains(&dx) {
                out[dy as usize * dw + dx as usize] = src[y * sw + x];
            }
        }
    }
    out
}

fn region_size(rect: [i64; 4]) -> (usize, usize) {
    (rect[2] as usize, rect[3] as usize)
}

/// One layer's premultiplied RGBA8 pixels through the crop and resize.
fn reshape_rgba(rgba: &[u8], width: usize, height: usize, spec: &CropSpec) -> Result<Vec<u8>, String> {
    let pixels: Vec<[u8; 4]> = rgba.as_chunks::<4>().0.to_vec();
    let kept = cut(&pixels, width, height, spec.rect, [0; 4]);
    let (rw, rh) = region_size(spec.rect);
    if (spec.width, spec.height) == (rw, rh) {
        return Ok(kept.as_flattened().to_vec());
    }
    if let Some(interp) = spec.resample {
        let img = Image::from_premul_rgba8(rw, rh, kept.as_flattened())
            .ok_or("The layer pixels do not match the crop area.")?;
        let out = resize_image(&img, spec.width, spec.height, interp).map_err(|e| e.to_string())?;
        return Ok(out.to_premul_rgba8());
    }
    let offset = anchor_offset(spec.anchor, rw, rh, spec.width, spec.height);
    Ok(place(&kept, rw, rh, spec.width, spec.height, offset, [0; 4]).as_flattened().to_vec())
}

/// One mask or selection (one value per pixel) through the crop and resize.
fn reshape_f32(data: &[f32], width: usize, height: usize, spec: &CropSpec, fill: f32) -> Result<Vec<f32>, String> {
    let kept = cut(data, width, height, spec.rect, fill);
    let (rw, rh) = region_size(spec.rect);
    if (spec.width, spec.height) == (rw, rh) {
        return Ok(kept);
    }
    if let Some(interp) = spec.resample {
        let mask = SelMask {
            width: rw,
            height: rh,
            data: kept,
        };
        let out = resize_mask(&mask, spec.width, spec.height, interp).map_err(|e| e.to_string())?;
        return Ok(out.data);
    }
    let offset = anchor_offset(spec.anchor, rw, rh, spec.width, spec.height);
    Ok(place(&kept, rw, rh, spec.width, spec.height, offset, fill))
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

// ---------------------------------------------------------------- the sheet and the handles

/// The crop rectangle's eight handles: side across (-1 left or top, 0 the middle, 1 right or
/// bottom) and side down.
const HANDLES: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];

/// Unit directions in which the canvas grows for `anchor`: away from the anchor on each axis.
/// A corner opens two ways, an edge three and the centre all four.
fn growth_dirs(anchor: Anchor) -> Vec<Vec2> {
    let i = ANCHORS.iter().position(|a| *a == anchor).unwrap_or(4);
    let (col, row) = ((i % 3) as i8 - 1, (i / 3) as i8 - 1);
    let mut dirs = Vec::new();
    if col <= 0 {
        dirs.push(Vec2::new(1.0, 0.0));
    }
    if col >= 0 {
        dirs.push(Vec2::new(-1.0, 0.0));
    }
    if row <= 0 {
        dirs.push(Vec2::new(0.0, 1.0));
    }
    if row >= 0 {
        dirs.push(Vec2::new(0.0, -1.0));
    }
    dirs
}

/// Rotates `v` by `deg` degrees (screen coordinates, y down).
fn rotate(v: Vec2, deg: f32) -> Vec2 {
    let (s, c) = deg.to_radians().sin_cos();
    Vec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

/// The 3 x 3 anchor picker. Each cell is a dot. The selected cell holds the anchor, with
/// arrows that point the way the canvas grows. Clicking a cell moves the anchor there.
fn anchor_picker(ui: &mut egui::Ui, pal: &crate::theme::Palette, anchor: &mut Anchor) {
    const CELL: f32 = 30.0;
    const GAP: f32 = 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(CELL * 3.0 + GAP * 2.0), Sense::hover());
    for (i, cell_anchor) in ANCHORS.iter().enumerate() {
        let origin = rect.min + Vec2::new((i % 3) as f32 * (CELL + GAP), (i / 3) as f32 * (CELL + GAP));
        let cell = Rect::from_min_size(origin, Vec2::splat(CELL));
        let resp = ui.interact(cell, ui.id().with(("crop_anchor", i)), Sense::click());
        let selected = *cell_anchor == *anchor;
        let fill = if selected {
            pal.accent.gamma_multiply(0.22)
        } else if resp.hovered() {
            pal.accent.gamma_multiply(0.10)
        } else {
            pal.row
        };
        let painter = ui.painter();
        painter.rect_filled(cell, egui::CornerRadius::same(6), fill);
        let centre = cell.center();
        if selected {
            let stroke = Stroke::new(1.75, pal.accent);
            for d in growth_dirs(*cell_anchor) {
                let tip = centre + d * 11.0;
                painter.line_segment([centre + d * 5.0, tip], stroke);
                for wing in [30.0, -30.0] {
                    painter.line_segment([tip, tip + rotate(-d, wing) * 4.0], stroke);
                }
            }
            painter.circle_filled(centre, 3.0, pal.accent);
        } else {
            painter.circle_filled(centre, 2.0, pal.text_dim);
        }
        if resp.clicked() {
            *anchor = *cell_anchor;
        }
    }
}

/// The nine anchors in reading order, for the sheet's grid.
const ANCHORS: [Anchor; 9] = [
    Anchor::TopLeft,
    Anchor::TopCentre,
    Anchor::TopRight,
    Anchor::MiddleLeft,
    Anchor::Centre,
    Anchor::MiddleRight,
    Anchor::BottomLeft,
    Anchor::BottomCentre,
    Anchor::BottomRight,
];

/// The Crop & Resize sheet while it is open.
#[derive(Clone, Debug)]
pub struct CropState {
    /// The kept region in canvas pixels: left, top, right and bottom (right and bottom exclusive).
    pub edges: [i64; 4],
    /// The output size and resolution, as typed.
    pub width: String,
    pub height: String,
    pub dpi: String,
    /// Keep the output's width to height ratio when one side is typed.
    pub lock_ratio: bool,
    pub anchor: Anchor,
    pub resample: bool,
    pub interp: Interp,
    /// The last valid output size; the locked ratio comes from it.
    pub out: (usize, usize),
    /// Why the last Apply was refused.
    pub message: Option<String>,
}

impl CropState {
    /// The sheet for a `width` x `height` canvas at `dpi`: the whole canvas kept, nothing changed.
    pub fn new(width: usize, height: usize, dpi: f64) -> Self {
        Self {
            edges: [0, 0, width as i64, height as i64],
            width: width.to_string(),
            height: height.to_string(),
            dpi: format!("{dpi}"),
            lock_ratio: false,
            anchor: Anchor::Centre,
            resample: false,
            interp: Interp::default(),
            out: (width, height),
            message: None,
        }
    }

    /// The kept region as `x, y, width, height`.
    pub(crate) fn rect(&self) -> [i64; 4] {
        let e = self.edges;
        [e[0], e[1], e[2] - e[0], e[3] - e[1]]
    }
}

/// Move the side(s) of the kept region that `handle` names to canvas position `(x, y)`. The
/// region keeps at least one pixel and at most [`MAX_SIDE`] on each side.
fn drag_edges(edges: &mut [i64; 4], handle: (i8, i8), x: i64, y: i64) {
    let (sx, sy) = handle;
    let max = MAX_SIDE as i64;
    if sx < 0 {
        edges[0] = x.min(edges[2] - 1).max(edges[2] - max);
    }
    if sx > 0 {
        edges[2] = x.max(edges[0] + 1).min(edges[0] + max);
    }
    if sy < 0 {
        edges[1] = y.min(edges[3] - 1).max(edges[3] - max);
    }
    if sy > 0 {
        edges[3] = y.max(edges[1] + 1).min(edges[1] + max);
    }
}

fn interp_name(i: Interp) -> &'static str {
    match i {
        Interp::Nearest => "Nearest",
        Interp::Bilinear => "Bilinear",
        Interp::Bicubic => "Bicubic",
    }
}

impl EskizApp {
    /// Open the Crop & Resize sheet on the canvas. Refused while a stroke, an adjustment or a 3D
    /// view is open.
    pub(crate) fn open_crop(&mut self) {
        if self.three_d.is_some() || self.adjust.is_some() || self.transform.is_some() || self.session.is_stroking() {
            self.show_toast("Finish what you are doing first.", 2.0);
            return;
        }
        let d = &self.session.doc;
        self.crop = Some(CropState::new(d.width, d.height, d.dpi));
    }

    /// The canvas changed size: refit the view and redraw everything.
    pub(crate) fn canvas_resized(&mut self, refresh: Refresh) {
        self.need_fit = true;
        self.needs_full = true;
        self.apply_refresh(refresh);
    }

    /// Redraw after an undo or redo. `before` is the canvas size before the step, so a step that
    /// changed the size (a crop or resize) refits the view.
    pub(crate) fn apply_history_refresh(&mut self, before: (usize, usize), refresh: Refresh) {
        self.clamp_text_caret();
        if before != (self.session.doc.width, self.session.doc.height) {
            self.canvas_resized(refresh);
        } else {
            self.apply_refresh(refresh);
        }
    }

    /// Once a frame while the sheet is open: the crop rectangle and its handles over the canvas,
    /// and the sheet itself.
    pub(crate) fn crop_frame(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.crop.take() else { return };
        let (vp, view, pal) = (self.viewport, self.view, self.pal);
        let before = st.edges;
        let e = st.edges.map(|v| v as f64);
        // The kept region's corners on screen: a rotated rectangle when the view is rotated.
        let corners = view.rect_corners(vp, e);
        let at = |sx: i8, sy: i8| {
            let (x, y) = (
                match sx {
                    -1 => e[0],
                    1 => e[2],
                    _ => (e[0] + e[2]) / 2.0,
                },
                match sy {
                    -1 => e[1],
                    1 => e[3],
                    _ => (e[1] + e[3]) / 2.0,
                },
            );
            view.canvas_to_screen(vp, x, y)
        };
        egui::Area::new(Id::new("crop_handles"))
            .order(Order::Middle)
            .fixed_pos(vp.min)
            .show(ctx, |ui| {
                ui.set_clip_rect(vp);
                let painter = ui.painter();
                let upright = view.is_upright();
                if upright {
                    let frame = Rect::from_two_pos(corners[0], corners[2]);
                    painter.rect_stroke(frame, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
                } else {
                    painter.add(egui::Shape::closed_line(corners.to_vec(), Stroke::new(1.5, Color32::WHITE)));
                }
                for (i, &(sx, sy)) in HANDLES.iter().enumerate() {
                    let c = at(sx, sy);
                    // The grab area stays a screen square; the visible handle turns with the canvas.
                    let resp = ui.interact(Rect::from_center_size(c, Vec2::splat(18.0)), Id::new(("crop_handle", i)), Sense::drag());
                    let fill = if resp.hovered() || resp.dragged() { Color32::from_rgb(255, 210, 60) } else { Color32::WHITE };
                    if upright {
                        painter.rect_filled(Rect::from_center_size(c, Vec2::splat(10.0)), 2.0, fill);
                    } else {
                        let (sn, cs) = view.rotation.sin_cos();
                        let q: Vec<Pos2> = [(-5.0f32, -5.0f32), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)]
                            .iter()
                            .map(|&(x, y)| c + Vec2::new(x * cs - y * sn, x * sn + y * cs))
                            .collect();
                        painter.add(egui::Shape::convex_polygon(q, fill, Stroke::NONE));
                    }
                    if resp.dragged() {
                        if let Some(p) = resp.interact_pointer_pos() {
                            let (x, y) = view.screen_to_canvas(vp, p);
                            drag_edges(&mut st.edges, (sx, sy), x.round() as i64, y.round() as i64);
                        }
                    }
                }
            });
        if st.edges != before {
            // The output size follows the kept region after a drag.
            let r = st.rect();
            st.width = r[2].to_string();
            st.height = r[3].to_string();
            st.out = (r[2] as usize, r[3] as usize);
        }

        let (pre_w, pre_h) = (st.width.clone(), st.height.clone());
        let (mut apply, mut cancel, mut reset) = (false, false, false);
        egui::Window::new("Crop & Resize")
            .anchor(Align2::RIGHT_TOP, Vec2::new(-12.0, 64.0))
            .order(Order::Foreground)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let r = st.rect();
                ui.label(format!("Kept: {} \u{d7} {} px from ({}, {})", r[2], r[3], r[0], r[1]));
                egui::Grid::new("crop_size").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Width");
                    ui.add(egui::TextEdit::singleline(&mut st.width).desired_width(80.0));
                    ui.end_row();
                    ui.label("Height");
                    ui.add(egui::TextEdit::singleline(&mut st.height).desired_width(80.0));
                    ui.end_row();
                    ui.label("DPI");
                    ui.add(egui::TextEdit::singleline(&mut st.dpi).desired_width(80.0));
                    ui.end_row();
                });
                crate::theme::checkbox(ui, &mut st.lock_ratio, "Lock ratio");
                crate::theme::checkbox(ui, &mut st.resample, "Resample all layers");
                let resample = st.resample;
                ui.add_enabled_ui(resample, |ui| {
                    egui::ComboBox::from_id_salt("crop_interp")
                        .selected_text(interp_name(st.interp))
                        .show_ui(ui, |ui| {
                            for m in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
                                ui.selectable_value(&mut st.interp, m, interp_name(m));
                            }
                        });
                });
                ui.add_space(4.0);
                ui.add_enabled_ui(!resample, |ui| {
                    ui.label("Anchor (where the kept area sits)");
                    anchor_picker(ui, &pal, &mut st.anchor);
                });
                if let Some(msg) = &st.message {
                    ui.colored_label(Color32::from_rgb(230, 90, 80), msg);
                }
                ui.horizontal(|ui| {
                    if ui.button("Reset").clicked() {
                        reset = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui.button("Apply").clicked() {
                        apply = true;
                    }
                });
            });

        // With the ratio locked, typing one side sets the other.
        let (w_edit, h_edit) = (st.width != pre_w, st.height != pre_h);
        if st.lock_ratio && (w_edit ^ h_edit) {
            let (ow, oh) = (st.out.0.max(1) as f64, st.out.1.max(1) as f64);
            if w_edit {
                if let Ok(w) = st.width.trim().parse::<usize>() {
                    st.height = ((w as f64 * oh / ow).round().max(1.0) as usize).to_string();
                }
            } else if let Ok(h) = st.height.trim().parse::<usize>() {
                st.width = ((h as f64 * ow / oh).round().max(1.0) as usize).to_string();
            }
        }
        if let (Ok(w), Ok(h)) = (st.width.trim().parse::<usize>(), st.height.trim().parse::<usize>()) {
            st.out = (w, h);
        }

        let mut keep = true;
        if reset {
            let d = &self.session.doc;
            st = CropState::new(d.width, d.height, d.dpi);
        }
        if cancel {
            keep = false;
        }
        if apply {
            match self.apply_crop(&st) {
                Ok(()) => keep = false,
                Err(msg) => st.message = Some(msg),
            }
        }
        if keep {
            self.crop = Some(st);
        }
    }

    /// Apply the sheet's crop and resize as one undo step.
    pub(crate) fn apply_crop(&mut self, st: &CropState) -> Result<(), String> {
        let width = st
            .width
            .trim()
            .parse::<usize>()
            .map_err(|_| "Width must be a whole number of pixels.".to_owned())?;
        let height = st
            .height
            .trim()
            .parse::<usize>()
            .map_err(|_| "Height must be a whole number of pixels.".to_owned())?;
        let dpi = st.dpi.trim().parse::<f64>().map_err(|_| "DPI must be a number.".to_owned())?;
        let spec = CropSpec {
            rect: st.rect(),
            width,
            height,
            anchor: st.anchor,
            resample: st.resample.then_some(st.interp),
            dpi,
        };
        let (op, selection) = plan(&self.session.doc, self.session.selection(), &spec)?;
        let Some(edit) = self.session.canvas_op("Crop & Resize", Box::new(op), selection) else {
            return Err("Finish what you are doing first.".to_owned());
        };
        self.canvas_resized(edit.refresh);
        self.show_toast("Crop & Resize", 1.2);
        Ok(())
    }

    /// The Canvas information sheet: size, resolution, layers, time spent and strokes.
    pub(crate) fn canvas_info_window(&mut self, ctx: &egui::Context) {
        if !self.canvas_info_open {
            return;
        }
        let doc = &self.session.doc;
        let rows = [
            ("Size", format!("{} \u{d7} {} px", doc.width, doc.height)),
            ("Resolution", format!("{} DPI", doc.dpi)),
            ("Color space", doc.colour_space.label().to_owned()),
            ("Color depth", doc.depth.label().to_owned()),
            ("Layers", doc.layer_count().to_string()),
            ("Time spent", format_duration(self.session.time_spent())),
            ("Strokes", self.session.strokes_done().to_string()),
        ];
        let mut open = true;
        egui::Window::new("Canvas information")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::Grid::new("canvas_info").num_columns(2).spacing([28.0, 8.0]).show(ui, |ui| {
                    for (label, value) in rows {
                        ui.label(label);
                        ui.label(value);
                        ui.end_row();
                    }
                });
            });
        if !open {
            self.canvas_info_open = false;
        }
    }
}

// ---------------------------------------------------------------- headless checks

/// Crop to half then undo, and flip the canvas then undo (`--selftest`).
fn crop_and_flip_check() -> Result<String, String> {
    let (w, h) = (100usize, 60usize);
    let mut s = Session::new(Document::new(w, h));
    let id = s.doc.layer_ids()[0];
    let pattern: Vec<[u8; 4]> = (0..w * h)
        .map(|i| if i % w < 50 { [255, 0, 0, 255] } else { [0, 0, 255, 255] })
        .collect();
    s.replace_layer_pixels(id, pattern.as_flattened().to_vec(), "Pattern")
        .ok_or("canvas: pattern refused")?;
    let original = s.doc.layer(id).ok_or("canvas: no layer")?.to_rgba();

    // Flip: the left edge becomes blue, and one undo puts red back.
    s.flip_canvas(true).ok_or("canvas: flip refused")?;
    if s.doc.layer(id).ok_or("canvas: no layer")?.pixel(0, 0) != [0, 0, 255, 255] {
        return Err("canvas: flip did not mirror the layer".to_owned());
    }
    s.undo().ok_or("canvas: flip undo unavailable")?;
    if s.doc.layer(id).ok_or("canvas: no layer")?.to_rgba() != original {
        return Err("canvas: undo of flip did not restore the layer".to_owned());
    }

    // Crop to the left half, check the kept pixels, then undo and redo.
    let spec = CropSpec {
        rect: [0, 0, 50, 30],
        width: 50,
        height: 30,
        anchor: Anchor::TopLeft,
        resample: None,
        dpi: s.doc.dpi,
    };
    let (op, sel) = plan(&s.doc, s.selection(), &spec)?;
    s.canvas_op("Crop & Resize", Box::new(op), sel).ok_or("canvas: crop refused")?;
    if (s.doc.width, s.doc.height) != (50, 30) {
        return Err(format!("canvas: crop gave {}x{}", s.doc.width, s.doc.height));
    }
    let cropped = s.doc.layer(id).ok_or("canvas: no layer")?.to_rgba();
    let src: &[[u8; 4]] = original.as_chunks::<4>().0;
    let expected: Vec<u8> = (0..30)
        .flat_map(move |y| (0..50).flat_map(move |x| src[y * w + x]))
        .collect();
    if cropped != expected {
        return Err("canvas: cropped pixels do not match the kept region".to_owned());
    }
    s.undo().ok_or("canvas: crop undo unavailable")?;
    if (s.doc.width, s.doc.height) != (w, h) || s.doc.layer(id).ok_or("canvas: no layer")?.to_rgba() != original {
        return Err("canvas: undo of crop did not restore the canvas".to_owned());
    }
    s.redo().ok_or("canvas: crop redo unavailable")?;
    if (s.doc.width, s.doc.height) != (50, 30) || s.doc.layer(id).ok_or("canvas: no layer")?.to_rgba() != cropped {
        return Err("canvas: redo of crop did not reapply it".to_owned());
    }
    Ok("  crop to 50x30 then undo: canvas, pixels and redo ok; flip canvas then undo: ok".to_owned())
}

/// Smudge, Fill, Clear and Pour Fill stay inside an active selection (`--selftest`).
fn selection_clip_check() -> Result<String, String> {
    let (w, h) = (64usize, 48usize);
    let mut s = Session::new(Document::new(w, h));
    let id = s.doc.layer_ids()[0];
    let pattern: Vec<[u8; 4]> = (0..w * h)
        .map(|i| match i % w {
            c if c < 16 => [200, 40, 40, 255],
            c if c < 40 => [40, 180, 40, 255],
            _ => [40, 40, 200, 255],
        })
        .collect();
    s.replace_layer_pixels(id, pattern.as_flattened().to_vec(), "Pattern")
        .ok_or("clip: pattern refused")?;
    let base = s.doc.layer(id).ok_or("clip: no layer")?.to_rgba();
    // The selection is the left 24 columns.
    let mask = SelMask {
        width: w,
        height: h,
        data: (0..w * h).map(|i| if i % w < 24 { 1.0 } else { 0.0 }).collect(),
    };
    s.set_selection("Select", Selection::active(mask)).ok_or("clip: select refused")?;
    // Outside the selection (columns 24 and beyond) the pixels must not change.
    let outside_unchanged = |rgba: &[u8]| {
        rgba.as_chunks::<4>().0.iter().zip(base.as_chunks::<4>().0.iter()).enumerate().all(|(i, (a, b))| i % w < 24 || a == b)
    };
    let inside_changed = |rgba: &[u8]| {
        rgba.as_chunks::<4>().0.iter().zip(base.as_chunks::<4>().0.iter()).enumerate().any(|(i, (a, b))| i % w < 24 && a != b)
    };

    s.fill_layer(id, [0, 255, 0]).ok_or("clip: fill refused")?;
    let filled = s.doc.layer(id).ok_or("clip: no layer")?.to_rgba();
    if !outside_unchanged(&filled) || !inside_changed(&filled) {
        return Err("clip: fill changed pixels outside the selection".to_owned());
    }
    s.undo().ok_or("clip: fill undo unavailable")?;

    s.clear_layer(id).ok_or("clip: clear refused")?;
    let cleared = s.doc.layer(id).ok_or("clip: no layer")?.to_rgba();
    if !outside_unchanged(&cleared) || !inside_changed(&cleared) {
        return Err("clip: clear changed pixels outside the selection".to_owned());
    }
    s.undo().ok_or("clip: clear undo unavailable")?;

    // Pour Fill over the whole layer, seeded on the left and set to a wide threshold.
    s.pour_fill(id, (2, 2), 1.0, [255, 255, 0]).ok_or("clip: pour fill refused")?;
    let dropped = s.doc.layer(id).ok_or("clip: no layer")?.to_rgba();
    if !outside_unchanged(&dropped) || !inside_changed(&dropped) {
        return Err("clip: Pour Fill changed pixels outside the selection".to_owned());
    }
    s.undo().ok_or("clip: pour fill undo unavailable")?;

    // Smudge a drag that crosses the selection edge. The layer is back to the pattern here.
    if s.doc.layer(id).ok_or("clip: no layer")?.to_rgba() != base {
        return Err("clip: undo did not restore the pattern before the smudge".to_owned());
    }
    if !s.begin_smudge(id, 6.0, 1.0, None) {
        return Err("clip: smudge refused".to_owned());
    }
    for i in 0..30 {
        s.push(crate::input::sample_from(crate::input::RawPointer {
            x: 34.0 - i as f64,
            y: 24.0,
            time: i as f64 / 120.0,
            kind: crate::input::PointerKind::Mouse,
            force: None,
        }));
    }
    s.end_stroke();
    let smudged = s.doc.layer(id).ok_or("clip: no layer")?.to_rgba();
    if !outside_unchanged(&smudged) {
        return Err("clip: smudge changed pixels outside the selection".to_owned());
    }
    if !inside_changed(&smudged) {
        return Err("clip: smudge did not change the selected pixels".to_owned());
    }
    Ok("  selection clip: fill, clear, Pour Fill and smudge leave the outside untouched".to_owned())
}

/// Headless checks for the canvas operations and the selection clip (`--selftest`).
pub fn selftest_checks() -> Result<Vec<String>, String> {
    Ok(vec![crop_and_flip_check()?, selection_clip_check()?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_arrows_point_away_from_the_anchor() {
        let right = Vec2::new(1.0, 0.0);
        let down = Vec2::new(0.0, 1.0);
        let top_left = growth_dirs(Anchor::TopLeft);
        assert_eq!(top_left.len(), 2);
        assert!(top_left.contains(&right) && top_left.contains(&down));
        assert_eq!(growth_dirs(Anchor::TopCentre).len(), 3);
        assert_eq!(growth_dirs(Anchor::Centre).len(), 4);
        let bottom_right = growth_dirs(Anchor::BottomRight);
        assert!(bottom_right.contains(&-right) && bottom_right.contains(&-down));
    }

    #[test]
    fn crop_refuses_a_canvas_too_large_for_the_layer_count() {
        let mut doc = Document::new(64, 64);
        let first = doc.layer_ids()[0];
        doc.add_layer_above(Some(first));
        let spec = CropSpec::identity(16384, 16384, 132.0);
        let err = plan(&doc, &Selection::default(), &spec)
            .err()
            .expect("refused");
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn canvas_operations_selftest() {
        let lines = selftest_checks().expect("canvas checks");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn anchored_resize_places_content_by_anchor() {
        let mut s = Session::new(Document::new(4, 4));
        let id = s.doc.layer_ids()[0];
        s.fill_layer(id, [255, 255, 255]).expect("fill");
        let spec = CropSpec {
            rect: [0, 0, 4, 4],
            width: 6,
            height: 4,
            anchor: Anchor::TopRight,
            resample: None,
            dpi: s.doc.dpi,
        };
        let (op, sel) = plan(&s.doc, s.selection(), &spec).expect("plan");
        s.canvas_op("Resize", Box::new(op), sel).expect("resize");
        assert_eq!((s.doc.width, s.doc.height), (6, 4));
        // Top-right anchor: the old columns start at x = 2 and the new columns on the left are empty.
        let layer = s.doc.layer(id).expect("layer");
        assert_eq!(layer.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(layer.pixel(2, 0), [255, 255, 255, 255]);
        s.undo().expect("undo");
        assert_eq!((s.doc.width, s.doc.height), (4, 4));
    }

    #[test]
    fn crop_edges_keep_one_pixel_and_clamp_to_the_limit() {
        let mut e = [0, 0, 10, 10];
        drag_edges(&mut e, (-1, -1), 50, 50);
        assert_eq!((e[0], e[1]), (9, 9));
        drag_edges(&mut e, (1, 1), -40, -40);
        assert_eq!((e[2], e[3]), (10, 10));
    }

    #[test]
    fn duration_reads_minutes_and_hours() {
        assert_eq!(format_duration(65.0), "1:05");
        assert_eq!(format_duration(3725.0), "1:02:05");
    }
}
