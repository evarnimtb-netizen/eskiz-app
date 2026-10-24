//! Text layers.
//!
//! A text layer is an ordinary raster node whose attributed source is kept in
//! [`eskiz_core::document::Document::texts`]. Its pixels are the last rendering of that source,
//! so every other part of the app (compositing, export, thumbnails) treats it as a raster
//! layer. Each edit re-renders the source with [`eskiz_text`] and stores both as one live change.
//! The live change becomes one undo step when the edit ends ([`Session::text_commit`]).
//! "Rasterize" drops the source and keeps the pixels.
//!
//! The on-canvas editor (caret, selection, typing, box width) is [`TextTool`] plus the input
//! hooks on [`EskizApp`]. The Edit Style window is in [`crate::panels::text_style`].

use std::ops::Range;
use std::path::Path;

use eframe::egui::{self, Color32, Event, Key, Modifiers, Pos2, Stroke};
use eskiz_core::canvas::Layer;
use eskiz_format::{TextRunSpec, TextSpec, TextValue};
use eskiz_text::model::DEFAULT_SIZE_PT;
use eskiz_text::storage::{self as text_storage, SavedValue};
use eskiz_text::{Affine, Attrs, Color, FontLibrary, Layout, Raster, Run, TextLayer, TextLayerExt};

use crate::app::{EskizApp, FrameInput};
use crate::command::{Change, Command, Placement, Refresh};
use crate::session::{Edit, Session};

/// Undo step names. Each one is a single entry in the undo history.
pub const ADD_STEP: &str = "Add Text";
pub const EDIT_STEP: &str = "Edit Text";
pub const STYLE_STEP: &str = "Text Style";
pub const WIDTH_STEP: &str = "Text Box Width";
/// Screen points between the box edge and the centre of the width knob.
const KNOB_GAP: f32 = 10.0;
/// A press this close (screen points) to the knob's centre grabs it.
const KNOB_GRAB: f32 = 8.0;
pub const RASTERIZE_STEP: &str = "Rasterize Text";

/// Caret and line geometry as fractions of the em size: the ascent above the baseline and the
/// descent below it.
const ASCENT: f32 = 0.8;
const DESCENT: f32 = 0.25;

/// A text edit or style change that is on the canvas but not yet a history step.
pub struct PendingText {
    pub(crate) id: u64,
    pub(crate) name: &'static str,
    /// The source before the first live change of the session (`None` for a new layer).
    before_text: Option<TextLayer>,
    /// The pixels before the first live change.
    before_pixels: Layer,
}

/// What a pointer drag on the canvas is doing while a text layer is being edited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Drag {
    #[default]
    None,
    /// Extending a selection from the caret.
    Select,
    /// Dragging the box width handle.
    Width,
}

/// Editor state. The document holds the text itself; this holds where the caret is.
#[derive(Default)]
pub struct TextTool {
    /// The text layer being typed into, if any.
    pub editing: Option<u64>,
    /// Caret position: a byte offset into the text on a character boundary.
    pub caret: usize,
    /// The other end of the selection, when there is one.
    pub anchor: Option<usize>,
    drag: Drag,
    /// The font library, with system fonts and the imported fonts folder. Created on first use.
    fonts: Option<FontLibrary>,
    /// The Edit Style font search field.
    pub style_search: String,
    /// The Actions panel's font import path.
    pub font_path: String,
}

impl TextTool {
    /// The font library, loading system fonts and the app's fonts folder on first use.
    pub fn fonts(&mut self) -> &mut FontLibrary {
        self.fonts.get_or_insert_with(load_fonts)
    }

    /// The selected byte range, empty when there is no selection.
    pub fn selection(&self) -> Range<usize> {
        match self.anchor {
            Some(a) if a != self.caret => a.min(self.caret)..a.max(self.caret),
            _ => self.caret..self.caret,
        }
    }

    fn begin(&mut self, id: u64, caret: usize) {
        self.editing = Some(id);
        self.caret = caret;
        self.anchor = None;
        self.drag = Drag::None;
    }
}

/// The system fonts plus every font in the app's fonts folder. A missing folder is not an error.
fn load_fonts() -> FontLibrary {
    let mut fonts = FontLibrary::new();
    let _ = fonts.load_dir(&crate::storage::fonts_dir());
    fonts
}

// ----- rendering and conversion -----

/// Renders a text layer at its own scale and blits it onto a canvas-sized premultiplied RGBA8
/// buffer (source-over, clipped to the canvas).
pub fn render_text(fonts: &mut FontLibrary, text: &TextLayer, w: usize, h: usize) -> Vec<u8> {
    blit(&text.rasterise(fonts), w, h)
}

/// Source-over of a premultiplied raster onto a blank canvas-sized buffer, clipped to it.
pub fn blit(raster: &Raster, w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let (ox, oy) = raster.origin;
    let rw = raster.width as i64;
    for ry in 0..raster.height as i64 {
        let y = i64::from(oy) + ry;
        if y < 0 || y >= h as i64 {
            continue;
        }
        for rx in 0..rw {
            let x = i64::from(ox) + rx;
            if x < 0 || x >= w as i64 {
                continue;
            }
            let s = ((ry * rw + rx) * 4) as usize;
            let sa = u32::from(raster.pixels[s + 3]);
            if sa == 0 {
                continue;
            }
            let d = (y as usize * w + x as usize) * 4;
            let inv = 255 - sa;
            for c in 0..4 {
                let dst = u32::from(out[d + c]) * inv / 255;
                out[d + c] = (u32::from(raster.pixels[s + c]) + dst).min(255) as u8;
            }
        }
    }
    out
}

/// The archived form of a text layer (see [`eskiz_format::TextSpec`]).
pub fn to_spec(text: &TextLayer) -> TextSpec {
    let t = text.transform;
    TextSpec {
        width: f64::from(text.box_width),
        text: text.text.clone(),
        runs: text
            .runs
            .iter()
            .map(|run| TextRunSpec {
                len: run.len,
                attributes: text_storage::attrs_to_saved(&run.attrs)
                    .into_iter()
                    .map(|(key, value)| {
                        let value = match value {
                            SavedValue::Text(s) => TextValue::Text(s),
                            SavedValue::Number(n) => TextValue::Number(n),
                            SavedValue::Flag(b) => TextValue::Flag(b),
                        };
                        (key.to_owned(), value)
                    })
                    .collect(),
            })
            .collect(),
        transform: [t.a, t.b, t.c, t.d, t.tx, t.ty].map(f64::from),
    }
}

/// Rebuilds a text layer from its archived form. Unknown attribute keys are dropped, and values
/// are clamped to their ranges. Returns `None` when the runs do not cover the text exactly, or
/// a run boundary falls inside a character.
pub fn from_spec(spec: &TextSpec) -> Option<TextLayer> {
    // Checked sums and boundaries: a forged length could wrap around and pass the total.
    let mut at = 0usize;
    for run in &spec.runs {
        at = at.checked_add(run.len)?;
        if !spec.text.is_char_boundary(at.min(spec.text.len() + 1)) || at > spec.text.len() {
            return None;
        }
    }
    if at != spec.text.len() {
        return None;
    }
    let mut runs: Vec<Run> = spec
        .runs
        .iter()
        .map(|run| {
            let mut attrs = Attrs::default();
            for (key, value) in &run.attributes {
                let value = match value {
                    TextValue::Text(s) => SavedValue::Text(s.clone()),
                    TextValue::Number(n) => SavedValue::Number(*n),
                    TextValue::Flag(b) => SavedValue::Flag(*b),
                };
                let _ = text_storage::apply_saved(&mut attrs, key, &value);
            }
            Run {
                len: run.len,
                attrs: attrs.clamped(),
            }
        })
        .collect();
    if runs.is_empty() {
        runs.push(Run {
            len: 0,
            attrs: Attrs::default(),
        });
    }
    let [a, b, c, d, tx, ty] = spec.transform.map(|v| if v.is_finite() { v as f32 } else { 0.0 });
    Some(TextLayer {
        text: spec.text.clone(),
        runs,
        box_width: if spec.width.is_finite() { spec.width.max(0.0) as f32 } else { 0.0 },
        transform: Affine::new(a, b, c, d, tx, ty),
    })
}

// ----- geometry -----

/// Layer units from canvas pixels, the inverse of the layer's transform.
pub fn to_layer(t: &Affine, x: f32, y: f32) -> (f32, f32) {
    let det = t.a * t.d - t.c * t.b;
    if det.abs() < 1e-9 {
        return (x, y);
    }
    let (dx, dy) = (x - t.tx, y - t.ty);
    ((t.d * dx - t.c * dy) / det, (-t.b * dx + t.a * dy) / det)
}

/// The caret for byte `at`: its x position and the top and bottom of its line, in layer units.
pub fn caret_geometry(layout: &Layout, at: usize, size: f32) -> (f32, f32, f32) {
    let band = |g: &eskiz_text::PlacedGlyph| (g.origin.1 - g.font_size * ASCENT, g.origin.1 + g.font_size * DESCENT);
    if let Some(g) = layout.glyphs.iter().find(|g| g.cluster == at) {
        let (top, bottom) = band(g);
        return (g.origin.0, top, bottom);
    }
    // After the last glyph before the caret: a line end, or a newline.
    if let Some(g) = layout.glyphs.iter().filter(|g| g.cluster < at).max_by_key(|g| g.cluster) {
        let (top, bottom) = band(g);
        return (g.origin.0 + g.advance, top, bottom);
    }
    (0.0, -size * ASCENT, size * DESCENT)
}

/// Length in bytes of the character starting at `at` (zero at the end of the text).
fn char_len(text: &str, at: usize) -> usize {
    text.get(at..).and_then(|s| s.chars().next()).map_or(0, char::len_utf8)
}

/// The caret position nearest to a point in layer units.
pub fn hit_test(layout: &Layout, text: &str, x: f32, y: f32) -> usize {
    let mut best: Option<(f32, usize)> = None;
    for g in &layout.glyphs {
        let top = g.origin.1 - g.font_size * ASCENT;
        let bottom = g.origin.1 + g.font_size * DESCENT;
        let dy = if y < top {
            top - y
        } else if y > bottom {
            y - bottom
        } else {
            0.0
        };
        let dx = (x - g.origin.0).abs().min((x - (g.origin.0 + g.advance)).abs());
        let score = dy * 4.0 + dx;
        let at = if x < g.origin.0 + g.advance * 0.5 {
            g.cluster
        } else {
            g.cluster + char_len(text, g.cluster)
        };
        let better = match best {
            None => true,
            Some((s, _)) => score < s,
        };
        if better {
            best = Some((score, at));
        }
    }
    best.map_or(0, |(_, at)| at.min(text.len()))
}

/// Layer-unit rectangles `(x0, y0, x1, y1)` that cover the glyphs of a byte range.
pub fn selection_rects(layout: &Layout, range: Range<usize>) -> Vec<[f32; 4]> {
    layout
        .glyphs
        .iter()
        .filter(|g| g.cluster >= range.start && g.cluster < range.end)
        .map(|g| {
            [
                g.origin.0,
                g.origin.1 - g.font_size * ASCENT,
                g.origin.0 + g.advance,
                g.origin.1 + g.font_size * DESCENT,
            ]
        })
        .collect()
}

/// Removes the bytes of `range` from the text and from the runs that hold them.
pub fn delete_range(t: &mut TextLayer, range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    let mut pos = 0;
    for run in &mut t.runs {
        let (s, e) = (pos, pos + run.len);
        pos = e;
        let lo = range.start.max(s);
        let hi = range.end.min(e);
        if hi > lo {
            run.len -= hi - lo;
        }
    }
    t.runs.retain(|r| r.len > 0);
    if t.runs.is_empty() {
        t.runs.push(Run {
            len: 0,
            attrs: Attrs::default(),
        });
    }
    t.text.replace_range(range, "");
}

/// Inserts `s` at byte `at`. The new text takes the attributes of the character before it (of
/// the first run at the start).
pub fn insert_at(t: &mut TextLayer, at: usize, s: &str) {
    if t.runs.is_empty() {
        t.runs.push(Run {
            len: 0,
            attrs: Attrs::default(),
        });
    }
    let mut idx = 0;
    if at > 0 {
        let mut pos = 0;
        let mut found = false;
        for (i, run) in t.runs.iter().enumerate() {
            if at - 1 < pos + run.len {
                idx = i;
                found = true;
                break;
            }
            pos += run.len;
        }
        if !found {
            idx = t.runs.len() - 1;
        }
    }
    t.runs[idx].len += s.len();
    t.text.insert_str(at, s);
}

// ----- the Session side: undo steps for text -----

impl Session {
    /// Adds a text layer above `above` holding `text`, drawn as `pixels` (canvas-sized
    /// premultiplied RGBA8). One undo step, "Add Text". The new layer's id is in `Edit::id`.
    pub fn add_text_layer(&mut self, above: Option<u64>, text: TextLayer, pixels: Vec<u8>) -> Option<Edit> {
        if !self.idle() || pixels.len() != self.doc.width * self.doc.height * 4 {
            return None;
        }
        self.text_commit();
        let id = self.doc.add_layer_above(above);
        if let Some(node) = self.doc.node_mut(id) {
            node.props.name = "Text".to_owned();
        }
        let (w, h) = (self.doc.width, self.doc.height);
        if let Some(layer) = self.doc.layer_mut(id) {
            *layer = Layer::from_rgba(w, h, pixels);
        }
        self.doc.texts.insert(id, text);
        let placement = Placement::capture(&self.doc, id)?;
        let mut cmd = Command::new(ADD_STEP);
        cmd.changes.push(Change::Insert(placement));
        self.finish(cmd, Some(id))
    }

    /// Makes `text` and its rendering `pixels` the state of text layer `id`, live on the canvas.
    /// Changes under the same `name` in a row are one step; a different name, or another layer,
    /// first records the pending step (see [`Session::text_commit`]).
    pub fn text_live(&mut self, id: u64, name: &'static str, text: TextLayer, pixels: Vec<u8>) -> Option<Edit> {
        let (w, h) = (self.doc.width, self.doc.height);
        // A locked text layer (or one in a locked group) is not edited, like any other layer.
        if !self.idle() || !self.doc.texts.contains_key(&id) || pixels.len() != w * h * 4 || self.doc.effective_locked(id) {
            return None;
        }
        if self.pending_text.as_ref().is_some_and(|p| p.id != id || p.name != name) {
            self.text_commit();
        }
        if self.pending_text.is_none() {
            self.pending_text = Some(PendingText {
                id,
                name,
                before_text: self.doc.texts.get(&id).cloned(),
                before_pixels: self.doc.layer(id)?.clone(),
            });
        }
        self.doc.texts.insert(id, text);
        if let Some(layer) = self.doc.layer_mut(id) {
            *layer = Layer::from_rgba(w, h, pixels);
        }
        self.touch(id);
        self.mark_edited();
        Some(Edit {
            id: Some(id),
            refresh: Refresh::Everything,
        })
    }

    /// Records the pending text change, if any, as one undo step. The live state is already
    /// applied, so the step only remembers the before and after.
    pub fn text_commit(&mut self) -> Option<Edit> {
        let PendingText {
            id,
            name,
            before_text,
            before_pixels,
        } = self.pending_text.take()?;
        let after_pixels = self.doc.layer(id)?.clone();
        let after_text = self.doc.texts.get(&id).cloned();
        let mut cmd = Command::new(name);
        if before_text != after_text {
            cmd.changes.push(Change::Text {
                id,
                before: before_text,
                after: after_text,
            });
        }
        if before_pixels != after_pixels {
            cmd.changes.push(Change::LayerPixels {
                layer: id,
                before: before_pixels,
                after: after_pixels,
            });
        }
        self.finish(cmd, Some(id))
    }

    /// Turns text layer `id` into an ordinary raster layer: the source is dropped and the pixels
    /// stay. One undo step, "Rasterize Text". Undo brings the source back.
    pub fn rasterize_text(&mut self, id: u64) -> Option<Edit> {
        if !self.idle() {
            return None;
        }
        self.text_commit();
        let before = self.doc.texts.get(&id)?.clone();
        let mut cmd = Command::new(RASTERIZE_STEP);
        cmd.apply(
            &mut self.doc,
            Change::Text {
                id,
                before: Some(before),
                after: None,
            },
        );
        self.finish(cmd, Some(id))
    }
}

// ----- the app side -----

impl EskizApp {
    /// The text layer the editor and the Edit Style window work on: the one being typed into,
    /// else the active layer when it is text.
    pub(crate) fn text_target(&self) -> Option<u64> {
        self.text
            .editing
            .or_else(|| self.session.doc.texts.contains_key(&self.active).then_some(self.active))
    }

    /// Actions > Add > Add text: a new empty text layer at the canvas centre, ready for typing.
    pub(crate) fn add_text_action(&mut self) {
        if self.three_d.is_some() {
            self.show_toast("Leave the 3D view to add text.", 3.0);
            return;
        }
        self.end_text_edit();
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let attrs = Attrs {
            color: Color::rgba(self.colour[0], self.colour[1], self.colour[2], 255),
            ..Attrs::default()
        };
        let mut layer = TextLayer::new("", attrs, 0.0);
        layer.transform = Affine::translation(w as f32 * 0.5, h as f32 * 0.5 - DEFAULT_SIZE_PT * 0.5);
        let pixels = render_text(self.text.fonts(), &layer, w, h);
        match self.session.add_text_layer(Some(self.active), layer, pixels) {
            Some(edit @ Edit { id: Some(id), .. }) => {
                self.apply_refresh(edit.refresh);
                self.active = id;
                self.text.begin(id, 0);
                self.show_toast("Type to add text. Escape finishes.", 2.5);
            }
            _ => self.show_toast("Could not add text here.", 2.5),
        }
    }

    /// After undo or redo, the text being typed into may be shorter than before: keep the caret
    /// and the selection anchor inside it, on character boundaries.
    pub(crate) fn clamp_text_caret(&mut self) {
        let Some(id) = self.text.editing else { return };
        let Some(layer) = self.session.doc.texts.get(&id) else { return };
        fn fit(text: &str, at: usize) -> usize {
            let mut at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        }
        self.text.caret = fit(&layer.text, self.text.caret);
        self.text.anchor = self.text.anchor.map(|a| fit(&layer.text, a)).filter(|&a| a != self.text.caret);
    }

    /// Ends the typing session, recording its changes as one step.
    pub(crate) fn end_text_edit(&mut self) {
        self.text.editing = None;
        self.text.anchor = None;
        self.text.drag = Drag::None;
        self.commit_text();
    }

    /// Records the pending text change, if any, and redraws what it changed.
    fn commit_text(&mut self) {
        if let Some(edit) = self.session.text_commit() {
            self.apply_refresh(edit.refresh);
        }
    }

    /// Per-frame housekeeping: drop an edit whose layer went away or is no longer active, and
    /// record a style or width change once the pointer is released.
    pub(crate) fn text_tick(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.text.editing {
            if !self.session.doc.texts.contains_key(&id) || self.active != id {
                self.end_text_edit();
            }
        }
        let style_pending = self.session.pending_text.as_ref().is_some_and(|p| p.name != EDIT_STEP);
        if style_pending && !ctx.input(|i| i.pointer.any_down()) {
            self.commit_text();
        }
    }

    /// Re-renders text layer `id` from `layer` and makes the change live under `name`.
    fn text_update(&mut self, id: u64, name: &'static str, layer: TextLayer) -> bool {
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let pixels = render_text(self.text.fonts(), &layer, w, h);
        match self.session.text_live(id, name, layer, pixels) {
            Some(edit) => {
                self.apply_refresh(edit.refresh);
                true
            }
            None => false,
        }
    }

    /// Replaces the bytes in `range` with `s` in the layer being typed into, and moves the caret
    /// after the new text.
    pub(crate) fn text_replace(&mut self, range: Range<usize>, s: &str) {
        let Some(id) = self.text.editing else { return };
        let Some(mut layer) = self.session.doc.texts.get(&id).cloned() else { return };
        let start = range.start;
        delete_range(&mut layer, range);
        insert_at(&mut layer, start, s);
        self.text.caret = start + s.len();
        self.text.anchor = None;
        self.text_update(id, EDIT_STEP, layer);
    }

    /// Applies a style change to the selection, or to the whole text when nothing is selected.
    /// It is one live change named `name`.
    pub(crate) fn text_restyle(&mut self, name: &'static str, f: impl Fn(&mut Attrs)) {
        let Some(id) = self.text_target() else { return };
        let Some(mut layer) = self.session.doc.texts.get(&id).cloned() else { return };
        let selection = if self.text.editing == Some(id) { self.text.selection() } else { 0..0 };
        let whole = selection.is_empty() || (selection.start == 0 && selection.end == layer.text.len());
        if whole {
            for run in &mut layer.runs {
                f(&mut run.attrs);
                run.attrs = run.attrs.clone().clamped();
            }
        } else {
            layer.set_attrs(selection, |a| f(a));
        }
        self.text_update(id, name, layer);
    }

    /// Deletes the selection, or the character before (or after) the caret.
    fn text_delete(&mut self, forward: bool) {
        let Some(id) = self.text.editing else { return };
        let Some(layer) = self.session.doc.texts.get(&id) else { return };
        let sel = self.text.selection();
        let range = if !sel.is_empty() {
            sel
        } else if forward {
            self.text.caret..self.text.caret + char_len(&layer.text, self.text.caret)
        } else {
            let before = layer.text[..self.text.caret].chars().next_back().map_or(0, char::len_utf8);
            self.text.caret - before..self.text.caret
        };
        if !range.is_empty() {
            self.text_replace(range, "");
        }
    }

    /// Moves the caret, extending the selection when `extend` is set.
    fn text_move(&mut self, to: usize, extend: bool) {
        let t = &mut self.text;
        if extend {
            t.anchor.get_or_insert(t.caret);
        } else {
            t.anchor = None;
        }
        t.caret = to;
        if t.anchor == Some(t.caret) {
            t.anchor = None;
        }
    }

    /// Up and down: the caret moves to the nearest glyph one line away, keeping its x position.
    fn text_line_move(&mut self, down: bool, extend: bool) {
        let Some(id) = self.text.editing else { return };
        let Some(layer) = self.session.doc.texts.get(&id).cloned() else { return };
        let layout = layer.layout(self.text.fonts());
        let size = layer.attrs_at(self.text.caret).map_or(DEFAULT_SIZE_PT, |a| a.size_pt);
        let (x, top, bottom) = caret_geometry(&layout, self.text.caret, size);
        let step = (bottom - top) * if down { 1.0 } else { -1.0 };
        let y = (top + bottom) * 0.5 + step * 1.1;
        let to = hit_test(&layout, &layer.text, x, y);
        self.text_move(to, extend);
    }

    /// Handles the keyboard while a text layer is being typed into. Returns without acting on
    /// keys that a text field or a shortcut should get (those are checked by the caller).
    pub(crate) fn text_keys(&mut self, ctx: &egui::Context) {
        if self.text.editing.is_none() {
            return;
        }
        let typed: Vec<String> = ctx.input_mut(|i| {
            let mut out = Vec::new();
            i.events.retain(|e| match e {
                Event::Text(t) => {
                    out.push(t.clone());
                    false
                }
                // Pasted text keeps its line breaks and loses other control characters.
                Event::Paste(t) => {
                    let lines = t.replace("\r\n", "\n").replace('\r', "\n");
                    out.push(lines.chars().filter(|c| *c == '\n' || !c.is_control()).collect());
                    false
                }
                _ => true,
            });
            out
        });
        let press = |key: Key, shift: bool| {
            ctx.input(|i| i.key_pressed(key) && i.modifiers.shift == shift && !i.modifiers.command && !i.modifiers.alt)
        };
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            if let Some(id) = self.text.editing {
                let len = self.session.doc.texts.get(&id).map_or(0, |t| t.text.len());
                self.text.anchor = Some(0);
                self.text.caret = len;
            }
        }
        if press(Key::Escape, false) || press(Key::Escape, true) {
            self.end_text_edit();
            return;
        }
        if press(Key::Backspace, false) || press(Key::Backspace, true) {
            self.text_delete(false);
        }
        if press(Key::Delete, false) {
            self.text_delete(true);
        }
        if press(Key::Enter, false) {
            self.text_replace(self.text.selection(), "\n");
        }
        for shift in [false, true] {
            if press(Key::ArrowLeft, shift) {
                let text = self.session.doc.texts.get(&self.text.editing.unwrap_or(0)).map(|t| t.text.clone()).unwrap_or_default();
                let to = text[..self.text.caret].chars().next_back().map_or(0, |c| self.text.caret - c.len_utf8());
                self.text_move(to, shift);
            }
            if press(Key::ArrowRight, shift) {
                let text = self.session.doc.texts.get(&self.text.editing.unwrap_or(0)).map(|t| t.text.clone()).unwrap_or_default();
                let to = self.text.caret + char_len(&text, self.text.caret);
                self.text_move(to, shift);
            }
            if press(Key::ArrowUp, shift) {
                self.text_line_move(false, shift);
            }
            if press(Key::ArrowDown, shift) {
                self.text_line_move(true, shift);
            }
            if press(Key::Home, shift) || press(Key::End, shift) {
                let text = self.session.doc.texts.get(&self.text.editing.unwrap_or(0)).map(|t| t.text.clone()).unwrap_or_default();
                let at = self.text.caret;
                let to = if press(Key::Home, shift) {
                    text[..at].rfind('\n').map_or(0, |i| i + 1)
                } else {
                    text[at..].find('\n').map_or(text.len(), |i| at + i)
                };
                self.text_move(to, shift);
            }
        }
        for s in typed {
            if s.chars().all(|c| c == '\n' || !c.is_control()) && !s.is_empty() {
                self.text_replace(self.text.selection(), &s);
            }
        }
    }

    /// Pointer input while a text layer is being typed into: a press places the caret (or starts
    /// a selection), a press on the width handle drags the box width, and a press elsewhere
    /// finishes the edit.
    pub(crate) fn text_pointer_frame(&mut self, input: &FrameInput, press_on_ui: bool) {
        let Some(id) = self.text.editing else { return };
        let Some(layer) = self.session.doc.texts.get(&id).cloned() else {
            self.end_text_edit();
            return;
        };
        let layout = layer.layout(self.text.fonts());
        let knob = self.width_knob(&layer, &layout);
        // Pointer positions in layer units, worked out before anything changes the document.
        let press = input.press_origin.filter(|_| input.primary_pressed && !press_on_ui).map(|p| (p, self.canvas_to_layer(p)));
        let pointer = input.pointer.map(|p| self.canvas_to_layer(p));

        if let Some((p, (lx, ly))) = press {
            // Only the visible knob, which sits outside the text, starts a width drag; a press on
            // the glyphs (however short the word) places the caret.
            let near_handle = knob.is_some_and(|(k, _)| k.distance(p) < KNOB_GRAB);
            let b = layout.bounds.outset(4.0);
            let inside = lx >= b.x0 && lx <= b.x1 && ly >= b.y0 && ly <= b.y1;
            if near_handle {
                self.text.drag = Drag::Width;
            } else if inside {
                let at = hit_test(&layout, &layer.text, lx, ly);
                self.text.caret = at;
                self.text.anchor = Some(at);
                self.text.drag = Drag::Select;
            } else {
                self.end_text_edit();
                return;
            }
        }
        if input.primary_down {
            if let Some((lx, ly)) = pointer {
                match self.text.drag {
                    Drag::Select => self.text.caret = hit_test(&layout, &layer.text, lx, ly),
                    Drag::Width => {
                        // The knob is drawn beside the box edge; the edge follows the pointer.
                        let edge = match (knob, input.pointer) {
                            (Some((_, off)), Some(ptr)) => self.canvas_to_layer(ptr - off).0,
                            _ => lx,
                        };
                        let mut next = layer.clone();
                        next.box_width = edge.max(1.0);
                        self.text_update(id, WIDTH_STEP, next);
                    }
                    Drag::None => {}
                }
            }
        } else {
            if self.text.drag == Drag::Width {
                self.commit_text();
            }
            if self.text.anchor == Some(self.text.caret) {
                self.text.anchor = None;
            }
            self.text.drag = Drag::None;
        }
    }

    /// Canvas pixels to the editing layer's units, through the layer transform.
    fn canvas_to_layer(&self, p: Pos2) -> (f32, f32) {
        let c = crate::selection_tool::canvas_pt(&self.view, self.viewport, p);
        let t = self.text_target().and_then(|id| self.session.doc.texts.get(&id)).map_or(Affine::IDENTITY, |l| l.transform);
        to_layer(&t, c.x, c.y)
    }

    /// Where the box width handle sits, in layer units: at the right edge of the box, halfway
    /// down the first line.
    pub(crate) fn width_handle(&self, layer: &TextLayer, layout: &Layout) -> Option<[f32; 2]> {
        let right = if layer.box_width > 0.0 {
            layer.box_width
        } else {
            layout.bounds.x1.max(0.0)
        };
        let size = layer.runs.first().map_or(DEFAULT_SIZE_PT, |r| r.attrs.size_pt);
        Some([right, -size * 0.3])
    }

    /// The visible width knob: its screen position and its offset from the box edge. It sits
    /// `KNOB_GAP` screen points outside the edge, along the layer's x direction, so it never covers
    /// the last glyph.
    pub(crate) fn width_knob(&self, layer: &TextLayer, layout: &Layout) -> Option<(Pos2, egui::Vec2)> {
        let h = self.width_handle(layer, layout)?;
        let t = layer.transform;
        let edge = self.layer_to_screen(&t, h[0], h[1]);
        let a = self.layer_to_screen(&t, 0.0, 0.0);
        let d = self.layer_to_screen(&t, 1.0, 0.0) - a;
        let u = if d.length() > 1e-6 { d.normalized() } else { egui::vec2(1.0, 0.0) };
        let off = u * KNOB_GAP;
        Some((edge + off, off))
    }

    /// Layer units to screen pixels.
    pub(crate) fn layer_to_screen(&self, t: &Affine, lx: f32, ly: f32) -> Pos2 {
        let (cx, cy) = t.apply(lx, ly);
        self.view.canvas_to_screen(self.viewport, f64::from(cx), f64::from(cy))
    }

    /// A small handle square around the screen point `p`, as a quad that turns with the view and the layer.
    /// `half` is its half side in screen points; the handle keeps its screen size.
    fn handle_quad(&self, t: &Affine, p: Pos2, half: f32) -> [Pos2; 4] {
        // Axis of the layer's x direction on screen, from two nearby layer points.
        let a = self.layer_to_screen(t, 0.0, 0.0);
        let b = self.layer_to_screen(t, 1.0, 0.0);
        let d = b - a;
        let u = if d.length() > 1e-6 { d.normalized() } else { egui::vec2(1.0, 0.0) };
        let v = egui::vec2(-u.y, u.x);
        [p - u * half - v * half, p + u * half - v * half, p + u * half + v * half, p - u * half + v * half]
    }

    /// Draws the text box of the target layer and, while typing, the caret and selection.
    pub(crate) fn draw_text_overlay(&mut self, ui: &egui::Ui) {
        let Some(id) = self.text_target() else { return };
        let Some(layer) = self.session.doc.texts.get(&id).cloned() else { return };
        let painter = ui.painter_at(self.viewport);
        let layout = layer.layout(self.text.fonts());
        let t = layer.transform;
        let accent = Color32::from_rgb(0, 122, 255);
        let editing = self.text.editing == Some(id);

        let b = layout.bounds.outset(2.0);
        let corners = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)];
        let pts: Vec<Pos2> = corners.iter().map(|&(x, y)| self.layer_to_screen(&t, x, y)).collect();
        for i in 0..4 {
            painter.line_segment([pts[i], pts[(i + 1) % 4]], Stroke::new(1.0, accent));
        }
        if let Some((p, _)) = self.width_knob(&layer, &layout) {
            let q = self.handle_quad(&t, p, 4.0);
            fill_quad(&painter, q, 1.0, accent);
        }
        if editing {
            let sel = self.text.selection();
            for [x0, y0, x1, y1] in selection_rects(&layout, sel) {
                let q = [
                    self.layer_to_screen(&t, x0, y0),
                    self.layer_to_screen(&t, x1, y0),
                    self.layer_to_screen(&t, x1, y1),
                    self.layer_to_screen(&t, x0, y1),
                ];
                fill_quad(&painter, q, 0.0, accent.gamma_multiply(0.35));
            }
            let size = layer.attrs_at(self.text.caret).map_or(DEFAULT_SIZE_PT, |a| a.size_pt);
            let (x, top, bottom) = caret_geometry(&layout, self.text.caret, size);
            let blink_on = ui.input(|i| i.time) % 1.0 < 0.5;
            if blink_on {
                let a = self.layer_to_screen(&t, x, top);
                let c = self.layer_to_screen(&t, x, bottom);
                painter.line_segment([a, c], Stroke::new(1.5, Color32::BLACK));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        }
    }

    /// Actions > Add > Import font: copies a `.ttf`, `.otf` or `.ttc` file into the fonts folder
    /// and loads it. Its families appear in the Edit Style font list at once.
    pub(crate) fn import_font_action(&mut self, path: &Path) {
        let Some(name) = path.file_name() else {
            self.show_toast("Choose a font file.", 2.5);
            return;
        };
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if !matches!(ext.as_str(), "ttf" | "otf" | "ttc") {
            self.show_toast("Fonts must be .ttf, .otf or .ttc.", 2.5);
            return;
        }
        // Load it from where it is first: a file that does not load is not copied.
        let families = match self.text.fonts().import_file(path) {
            Ok(families) => families,
            Err(e) => {
                self.show_toast(format!("Could not load the font: {e}"), 3.0);
                return;
            }
        };
        let names = families.join(", ");
        match keep_font(path, name) {
            Ok(()) => self.show_toast(format!("Imported {names}"), 2.5),
            Err(e) => self.show_toast(format!("Imported {names} for now; it could not be kept: {e}"), 3.5),
        }
    }

    /// Layers menu > Rasterize: ends any typing, then turns the text layer into pixels.
    pub(crate) fn rasterize_text_action(&mut self, id: u64) {
        self.end_text_edit();
        match self.session.rasterize_text(id) {
            Some(_) => self.show_toast("Text rasterized. Undo brings it back.", 2.5),
            None => self.show_toast("Could not rasterize this layer.", 2.5),
        }
    }
}

/// Selftest: create "Eskiz", check its ink, change the size, check the ink grows, undo.
pub fn selftest_checks() -> Result<Vec<String>, String> {
    let mut fonts = FontLibrary::new();
    if fonts.families().is_empty() {
        return Ok(vec!["  text layers: skipped (no fonts installed)".to_owned()]);
    }
    let (w, h) = (320usize, 200usize);
    let mut session = Session::new(eskiz_core::document::Document::new(w, h));
    let above = session.doc.layer_ids().last().copied();

    let attrs = Attrs {
        size_pt: 24.0,
        ..Attrs::default()
    };
    let layer = TextLayer::new("Eskiz", attrs, 0.0);
    let pixels = render_text(&mut fonts, &layer, w, h);
    let added = session.add_text_layer(above, layer, pixels).ok_or("text: add refused")?;
    let id = added.id.ok_or("text: no id")?;
    let before = ink_of(&session, id)?;
    if before.is_none() {
        return Err("text: ink bounds are empty after Add Text".to_owned());
    }
    let (bw, bh) = before.map_or((0, 0), |b| (b.2 - b.0, b.3 - b.1));

    let mut grown = session.doc.texts[&id].clone();
    grown.set_attrs(0..grown.text.len(), |a| a.size_pt = 48.0);
    let pixels = render_text(&mut fonts, &grown, w, h);
    session.text_live(id, STYLE_STEP, grown, pixels).ok_or("text: restyle refused")?;
    session.text_commit();
    let after = ink_of(&session, id)?.ok_or("text: ink bounds empty after resize")?;
    let (aw, ah) = (after.2 - after.0, after.3 - after.1);
    if aw <= bw || ah <= bh {
        return Err(format!("text: bounds did not grow ({bw}x{bh} -> {aw}x{ah})"));
    }

    if session.undo_name() != Some(STYLE_STEP) {
        return Err(format!("text: undo names {:?}", session.undo_name()));
    }
    session.undo().ok_or("text: undo refused")?;
    let restored = ink_of(&session, id)?;
    if restored != before {
        return Err("text: undo did not restore the size".to_owned());
    }
    // Duplicating a text layer keeps its text source, so the copy can be restyled as well.
    let copy = session.duplicate_layer(id).and_then(|e| e.id).ok_or("text: duplicate refused")?;
    let source = session.doc.texts.get(&copy).cloned().ok_or("text: duplicate lost its text source")?;
    if source.text != session.doc.texts[&id].text {
        return Err("text: the duplicate has different text".to_owned());
    }
    let mut restyled = source;
    restyled.set_attrs(0..restyled.text.len(), |a| a.size_pt = 36.0);
    let pixels = render_text(&mut fonts, &restyled, w, h);
    session.text_live(copy, STYLE_STEP, restyled, pixels).ok_or("text: the duplicate is not editable")?;
    session.text_commit();
    if ink_of(&session, copy)?.is_none() {
        return Err("text: the duplicate renders nothing after restyling".to_owned());
    }
    Ok(vec![
        format!("  text layer \"Eskiz\": ink {bw}x{bh} at 24 pt, {aw}x{ah} at 48 pt, undo restores: ok"),
        "  text layer duplicate keeps its text source and restyles: ok".to_owned(),
    ])
}

/// Ink bounds `(x0, y0, x1, y1)` of a layer's pixels, or `None` when it is blank.
fn ink_of(session: &Session, id: u64) -> Result<Option<(usize, usize, usize, usize)>, String> {
    let layer = session.doc.layer(id).ok_or("text: no pixels")?;
    let rgba = layer.to_rgba();
    let w = session.doc.width;
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for (i, p) in rgba.as_chunks::<4>().0.iter().enumerate() {
        if p[3] == 0 {
            continue;
        }
        let (x, y) = (i % w, i / w);
        bounds = Some(match bounds {
            None => (x, y, x + 1, y + 1),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
        });
    }
    Ok(bounds)
}


impl EskizApp {
    /// Sets the wrap width of the target text layer (zero means no wrapping), as one live change.
    pub(crate) fn text_set_box_width(&mut self, width: f32) {
        let Some(id) = self.text_target() else { return };
        let Some(mut layer) = self.session.doc.texts.get(&id).cloned() else { return };
        layer.box_width = width.max(0.0);
        self.text_update(id, WIDTH_STEP, layer);
    }
}

/// Fill a quad given by its screen corners: a plain rectangle when it is axis-aligned (the upright
/// view), otherwise a polygon that follows the rotation.
fn fill_quad(painter: &egui::Painter, q: [Pos2; 4], rounding: f32, colour: Color32) {
    let aligned = (q[0].y - q[1].y).abs() < 1e-3 && (q[1].x - q[2].x).abs() < 1e-3 && (q[2].y - q[3].y).abs() < 1e-3;
    if aligned {
        painter.rect_filled(egui::Rect::from_two_pos(q[0], q[2]), rounding, colour);
    } else {
        painter.add(egui::Shape::convex_polygon(q.to_vec(), colour, Stroke::NONE));
    }
}

/// Copy an imported font into the fonts folder so it loads at the next start. A font already
/// there with the same bytes is left as it is (importing it from that folder must not copy a
/// file onto itself, which empties it), and another font with the same file name keeps its file:
/// this one gets a free name.
fn keep_font(path: &Path, name: &std::ffi::OsStr) -> std::io::Result<()> {
    let dir = crate::storage::fonts_dir();
    std::fs::create_dir_all(&dir)?;
    let data = std::fs::read(path)?;
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = Path::new(name).extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut dest = dir.join(name);
    let mut n = 2;
    while dest.exists() {
        if std::fs::read(&dest).is_ok_and(|have| have == data) {
            return Ok(());
        }
        dest = dir.join(format!("{stem} ({n}).{ext}"));
        n += 1;
    }
    crate::storage::write_atomic(&dest, &data).map_err(std::io::Error::other)
}
