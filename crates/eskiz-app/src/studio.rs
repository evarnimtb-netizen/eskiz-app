//! Brush Bench state and editing rules. The sheet itself is drawn by
//! `panels::studio`; this module holds what it edits and the rules that go with it.
//!
//! * The Studio edits a **working copy** of a library brush, an imported brush, or one of My
//!   Brushes: the engine settings (`eskiz_core::Brush`), the tip and the grain image. Nothing
//!   outside the Studio changes while it is open.
//! * **Save** writes the working copy to My Brushes as a `.brush` file (see `brush_settings` for
//!   how every setting is kept) and makes it the tool's brush. Saving a built-in or imported
//!   brush makes a new My Brushes entry and leaves the original alone; saving one of My Brushes
//!   writes over it.
//! * **Duplicate** saves the working copy as a *new* entry and carries on editing that copy.
//! * **Reset** puts the working copy back as it was when it was opened, or last saved.
//! * The **drawing pad** is a small private layer painted through the same stroke engine as the
//!   canvas, so what it shows is what the canvas will do.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use eskiz_brushlib::grains::GrainKind;
use eskiz_brushlib::tips::TipKind;
use eskiz_brushlib::{generated_grain, generated_tip, LibraryBrush};
use eskiz_core::brushes::{Brush, Grain, Shape};
use eskiz_core::canvas::Layer;
use eskiz_core::geometry::Sample;
use eskiz_core::raster::DabRaster;
use eskiz_format::brushes::ImportedBrush;
use image::GrayImage;

use crate::app::{EskizApp, Tool, MAX_SIZE};
use crate::brush_import::{encode_gray_png, MY_BRUSHES_SET};
use crate::brush_library::{engine_for, BrushId};
use crate::brush_settings;
use crate::paint::{PaintEngine, StrokeSettings};
use crate::studio_settings::Section;

/// Ink colour of the drawing pad: a near-black on its paper.
pub const PAD_INK: [f64; 3] = [0.09, 0.09, 0.11];

/// Depth a grain gets when it is first picked on a brush that had none, so the choice shows.
const FRESH_GRAIN_DEPTH: f64 = 0.6;

/// What the Studio edits: the engine settings and the two images of a brush.
#[derive(Clone, Debug, PartialEq)]
pub struct StudioBrush {
    pub brush: Brush,
    /// Grey tip, white is paint.
    pub tip: GrayImage,
    /// Tileable grey grain, if the brush has one.
    pub grain: Option<GrayImage>,
    /// The generator of `tip` (`None` for a tip that came from a file).
    pub tip_kind: Option<TipKind>,
    /// The generator of `grain`.
    pub grain_kind: Option<GrainKind>,
}

impl StudioBrush {
    pub fn from_library(lb: &LibraryBrush) -> Self {
        Self {
            brush: lb.brush.clone(),
            tip: lb.tip.clone(),
            grain: lb.grain.clone(),
            tip_kind: lb.tip_kind,
            grain_kind: lb.grain_kind,
        }
    }

    /// The working copy as a library brush (for previews and the pad).
    pub fn library_brush(&self) -> LibraryBrush {
        LibraryBrush {
            id: "",
            name: "",
            brush: self.brush.clone(),
            tip: self.tip.clone(),
            grain: self.grain.clone(),
            tip_kind: self.tip_kind,
            grain_kind: self.grain_kind,
        }
    }

    /// Use the generated tip `kind`. The soft round tip is also the engine's soft shape.
    pub fn set_tip(&mut self, kind: TipKind) {
        self.tip = generated_tip(kind);
        self.tip_kind = Some(kind);
        self.brush.shape = if kind == TipKind::RoundSoft { Shape::SoftRound } else { Shape::HardRound };
    }

    /// Use the generated grain `kind`, or no grain. A grain with no depth does nothing, so a
    /// fresh choice gets some.
    pub fn set_grain(&mut self, kind: Option<GrainKind>) {
        match kind {
            None => {
                self.grain = None;
                self.grain_kind = None;
                self.brush.grain = Grain::None;
            }
            Some(k) => {
                self.grain = Some(generated_grain(k));
                self.grain_kind = Some(k);
                self.brush.grain = if k == GrainKind::PaperTooth { Grain::Paper } else { Grain::Noise };
                if self.brush.grain_depth == 0.0 {
                    self.brush.grain_depth = FRESH_GRAIN_DEPTH;
                }
            }
        }
    }

    /// The `.brush` contents of this brush under `name`. `base` is the file the brush came from,
    /// if any: settings Eskiz does not edit are kept from it.
    pub fn to_imported(&self, name: &str, base: Option<&ImportedBrush>) -> Result<ImportedBrush, String> {
        let mut ib = ImportedBrush { name: name.to_owned(), ..ImportedBrush::default() };
        if let Some(base) = base {
            ib.settings = base.settings.clone();
        }
        ib.settings.extend(brush_settings::to_settings(&self.brush));
        let mut text = |key: &str, value: Option<&str>| match value {
            Some(v) => {
                ib.settings.insert(key.to_owned(), eskiz_format::brushes::SettingValue::Text(v.to_owned()));
            }
            None => {
                ib.settings.remove(key);
            }
        };
        text(brush_settings::TIP_KIND, self.tip_kind.map(TipKind::key));
        text(brush_settings::GRAIN_KIND, self.grain_kind.map(GrainKind::key));
        ib.settings.insert(brush_settings::GRAY_PAINT.to_owned(), eskiz_format::brushes::SettingValue::Bool(true));
        ib.shape_png = Some(encode_gray_png(&self.tip)?);
        ib.grain_png = self.grain.as_ref().map(encode_gray_png).transpose()?;
        Ok(ib)
    }
}

/// "Name copy", then "Name copy 2" and so on: the name of a copy of `name`.
pub fn copy_name(name: &str) -> String {
    let n = name.trim();
    if let Some((base, k)) = n.rsplit_once(" copy ") {
        let digits = !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit());
        if let Some(next) = k.parse::<u32>().ok().filter(|_| digits).and_then(|k| k.checked_add(1)) {
            return format!("{base} copy {next}");
        }
    }
    if n.ends_with(" copy") {
        return format!("{n} 2");
    }
    format!("{n} copy")
}

/// The drawing pad: a private layer, the stroke in progress and its on-screen texture.
#[derive(Default)]
pub struct Pad {
    layer: Option<Layer>,
    engine: PaintEngine,
    /// Layer pixels per point.
    scale: u32,
    texture: Option<TextureHandle>,
    dirty: bool,
    /// A stroke is in progress.
    pub drawing: bool,
    /// Strokes finished since the pad was last cleared.
    pub strokes: u32,
    /// The last pointer position fed to the stroke (screen points), to skip repeats.
    pub last: Option<egui::Pos2>,
    seed: u32,
}

impl Pad {
    /// Make the layer `w x h` pixels (`scale` pixels per point). A different size starts a
    /// blank pad.
    pub fn ensure(&mut self, w: usize, h: usize, scale: u32) {
        let same = self.layer.as_ref().is_some_and(|l| l.width == w && l.height == h) && self.scale == scale;
        if !same {
            self.layer = Some(Layer::new(w.max(1), h.max(1)));
            self.engine = PaintEngine::default();
            self.scale = scale;
            self.drawing = false;
            self.strokes = 0;
            self.dirty = true;
        }
    }

    /// Pixels per point of the layer.
    pub fn scale(&self) -> u32 {
        self.scale.max(1)
    }

    /// Wipe the pad.
    pub fn clear(&mut self) {
        if let Some(l) = self.layer.as_mut() {
            *l = Layer::new(l.width, l.height);
        }
        self.engine = PaintEngine::default();
        self.drawing = false;
        self.strokes = 0;
        self.dirty = true;
    }

    /// Start a stroke with `brush` (sizes in layer pixels) and its tip and grain `raster`.
    pub fn begin(&mut self, brush: &Brush, raster: Arc<DabRaster>) -> bool {
        let Some(layer) = self.layer.as_ref() else {
            return false;
        };
        self.seed = self.seed.wrapping_add(0x9E37_79B9) | 1;
        let settings = StrokeSettings { brush: brush.clone(), colour: PAD_INK, erase: false, raster: Some(raster) };
        self.drawing = self.engine.begin(layer, 0, &settings, self.seed, false);
        self.drawing
    }

    /// Feed a sample (layer pixels).
    pub fn push(&mut self, sample: Sample) {
        if let Some(layer) = self.layer.as_mut() {
            if self.engine.push(layer, sample).is_some() {
                self.dirty = true;
            }
        }
    }

    /// Finish the stroke.
    pub fn end(&mut self) {
        if let Some(layer) = self.layer.as_mut() {
            if self.drawing {
                self.engine.end(layer);
                self.strokes += 1;
                self.dirty = true;
            }
        }
        self.drawing = false;
    }

    /// Ink pixels on the pad with alpha of at least `min_alpha` (for tests).
    pub fn ink_pixels(&self, min_alpha: u8) -> usize {
        self.layer.as_ref().map_or(0, |l| l.to_rgba().as_chunks::<4>().0.iter().filter(|p| p[3] >= min_alpha).count())
    }

    /// The pad's pixels, premultiplied RGBA (for tests).
    pub fn rgba(&self) -> Vec<u8> {
        self.layer.as_ref().map(Layer::to_rgba).unwrap_or_default()
    }

    /// The texture of the pad, uploaded again when it changed.
    pub fn texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        let layer = self.layer.as_ref()?;
        if self.dirty || self.texture.is_none() {
            let image = ColorImage::from_rgba_premultiplied([layer.width, layer.height], &layer.to_rgba());
            match self.texture.as_mut() {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("brush-studio-pad", image, TextureOptions::LINEAR)),
            }
            self.dirty = false;
        }
        self.texture.as_ref()
    }
}

/// Where the sheet drew its parts last frame, in screen points (for tests and hit checks).
#[derive(Clone, Debug, Default)]
pub struct StudioLayout {
    pub sheet: Option<egui::Rect>,
    pub pad: Option<egui::Rect>,
    pub strip: Option<egui::Rect>,
    pub sections: Vec<(Section, egui::Rect)>,
    pub done: Option<egui::Rect>,
    pub save: Option<egui::Rect>,
    pub duplicate: Option<egui::Rect>,
    pub reset: Option<egui::Rect>,
    pub clear: Option<egui::Rect>,
    pub name: Option<egui::Rect>,
    /// Sliders as `(label, rect)` in the order drawn.
    pub sliders: Vec<(&'static str, egui::Rect)>,
    /// Tip and grain picker cells as `(is_tip, index, rect)`. A grain's index 0 is "None".
    pub cells: Vec<(bool, usize, egui::Rect)>,
    /// Segments of two-way switches as `(label, rect)`.
    pub segments: Vec<(String, egui::Rect)>,
}

/// What the Studio is editing.
pub struct StudioState {
    /// The tool whose brush this is.
    pub tool: Tool,
    /// The brush the working copy came from, or the My Brushes entry it was last saved as.
    pub target: BrushId,
    /// The name in the name field.
    pub name: String,
    /// The working settings, shown in every control and drawn on the pad.
    pub working: StudioBrush,
    /// What Reset goes back to.
    pub saved: StudioBrush,
    pub saved_name: String,
    pub section: Section,
    /// Taper tab: touch (finger) instead of pencil.
    pub finger_taper: bool,
    /// Which curve chip is selected under Pressure and tilt, and under Wet mix.
    pub curve_pick: [usize; 2],
    pub confirm_delete: bool,
    /// Edits made since the Studio opened.
    pub edits: u32,
    /// Size or opacity was edited, so a save also sets the tool's sliders.
    pub props_edited: bool,
    /// Counts changes to the working copy, so caches know when to redraw.
    pub rev: u64,
    /// The last thing Save, Duplicate or Delete did, for the header.
    pub status: Option<String>,
    pub pad: Pad,
    /// The stroke preview strip: what it was drawn from, and its texture.
    pub strip: Option<((u64, u32, i32), TextureHandle)>,
    /// Tip and grain thumbnails, made when a picker first shows.
    pub thumbs: HashMap<(bool, usize), TextureHandle>,
    pub layout: StudioLayout,
    /// When Escape or Back last asked to close with unsaved changes (app time).
    pub close_asked: Option<f64>,
}

impl StudioState {
    fn new(tool: Tool, target: BrushId, name: String, brush: StudioBrush) -> Self {
        Self {
            tool,
            target,
            saved: brush.clone(),
            saved_name: name.clone(),
            name,
            working: brush,
            section: Section::StrokePath,
            finger_taper: false,
            curve_pick: [0, 0],
            confirm_delete: false,
            edits: 0,
            props_edited: false,
            rev: 0,
            status: None,
            pad: Pad::default(),
            strip: None,
            thumbs: HashMap::new(),
            layout: StudioLayout::default(),
            close_asked: None,
        }
    }

    /// Whether the brush being edited is one of My Brushes.
    pub fn is_user_brush(&self) -> bool {
        matches!(self.target, BrushId::User(_))
    }

    /// Whether the working copy differs from what was saved or opened.
    pub fn dirty(&self) -> bool {
        self.working != self.saved || self.name != self.saved_name
    }

    /// The raster of the working copy: its tip and grain with its settings.
    pub fn raster(&self) -> Arc<DabRaster> {
        engine_for(&self.working.library_brush()).raster
    }
}

impl EskizApp {
    /// Set a tool's size and opacity sliders to a brush's own values.
    pub(crate) fn apply_brush_defaults(&mut self, tool: Tool, brush: &Brush) {
        let p = &mut self.tool_params[tool.index()];
        p.size = (brush.size as f32).clamp(1.0, MAX_SIZE);
        p.opacity = (brush.opacity as f32).clamp(0.0, 1.0);
    }

    /// Open the Studio on the brush `id` for `tool`, closing any popover. Does nothing when the
    /// bank does not have the brush.
    pub(crate) fn open_studio(&mut self, tool: Tool, id: BrushId) -> bool {
        let Some(lb) = self.bank.find(&id) else {
            return false;
        };
        let name = self
            .bank
            .sets
            .iter()
            .flat_map(|s| &s.brushes)
            .find(|b| b.id == id)
            .map_or_else(|| lb.name.to_owned(), |b| b.name.clone());
        self.popovers.close();
        self.studio = Some(StudioState::new(tool, id, name, StudioBrush::from_library(&lb)));
        true
    }

    /// Open the Studio on the brush the library is open for (the Edit button).
    pub(crate) fn open_studio_for_library_tool(&mut self) -> bool {
        let tool = self.library_tool;
        let id = self.tool_brushes[tool.index()].clone();
        self.open_studio(tool, id)
    }

    /// Close the Studio. Unsaved edits are dropped.
    pub(crate) fn close_studio(&mut self) {
        self.studio = None;
    }

    /// Escape and Back. With unsaved changes the Studio closes only on a second press within
    /// 3 seconds: one stray press must not drop a long editing session.
    pub(crate) fn request_close_studio(&mut self) {
        let now = self.now;
        let Some(st) = self.studio.as_mut() else { return };
        if st.dirty() && st.close_asked.is_none_or(|t| now - t > 3.0) {
            st.close_asked = Some(now);
            self.show_toast("Unsaved changes. Press again to close without saving.", 3.0);
            return;
        }
        self.close_studio();
    }

    /// Apply one edit to the working copy.
    pub(crate) fn studio_edit(&mut self, edit: impl FnOnce(&mut StudioBrush)) {
        let Some(st) = self.studio.as_mut() else {
            return;
        };
        let mut after = st.working.clone();
        edit(&mut after);
        if after != st.working {
            if (after.brush.size - st.working.brush.size).abs() > 1e-9
                || (after.brush.opacity - st.working.brush.opacity).abs() > 1e-9
            {
                st.props_edited = true;
            }
            st.edits += 1;
            st.rev += 1;
            st.working = after;
        }
    }

    /// Edit the engine settings of the working copy.
    pub(crate) fn studio_edit_brush(&mut self, edit: impl FnOnce(&mut Brush)) {
        self.studio_edit(|w| edit(&mut w.brush));
    }

    /// Rename the working copy.
    pub(crate) fn studio_rename(&mut self, name: String) {
        if let Some(st) = self.studio.as_mut() {
            if st.name != name {
                st.name = name;
                st.rev += 1;
            }
        }
    }

    /// Put the working copy back as it was when the Studio opened or last saved.
    pub(crate) fn studio_reset(&mut self) {
        if let Some(st) = self.studio.as_mut() {
            st.working = st.saved.clone();
            st.name = st.saved_name.clone();
            st.rev += 1;
            st.props_edited = false;
            st.status = None;
        }
    }

    /// The name to save the working copy under: the name field, or "My brush" when it is blank.
    fn studio_save_name(st: &StudioState) -> String {
        let n = st.name.trim();
        if n.is_empty() {
            "My brush".to_owned()
        } else {
            n.to_owned()
        }
    }

    /// Save the working copy to My Brushes. One of My Brushes is written over; anything else
    /// becomes a new entry. The saved brush becomes the tool's brush and the Studio goes on
    /// editing it. Returns the id, or `None` after a toast when it could not be written.
    pub(crate) fn studio_save(&mut self) -> Option<BrushId> {
        self.studio_store(false)
    }

    /// Save the working copy as a new My Brushes entry named "<name> copy", and edit that.
    pub(crate) fn studio_duplicate(&mut self) -> Option<BrushId> {
        self.studio_store(true)
    }

    fn studio_store(&mut self, copy: bool) -> Option<BrushId> {
        let st = self.studio.as_ref()?;
        let mut name = Self::studio_save_name(st);
        let key = match (&st.target, copy) {
            (BrushId::User(k), false) => Some(k.clone()),
            _ => None,
        };
        if copy {
            name = copy_name(&name);
        }
        let base = self.bank.source(&st.target).cloned();
        let saved = st
            .working
            .to_imported(&name, base.as_ref())
            .and_then(|ib| self.bank.save_user(key.as_deref(), &ib));
        let id = match saved {
            Ok(id) => id,
            Err(e) => {
                self.show_toast(format!("Could not save the brush: {e}"), 4.0);
                return None;
            }
        };
        self.previews.invalidate(&id);
        let st = self.studio.as_mut()?;
        let (tool, props) = (st.tool, st.props_edited);
        st.target = id.clone();
        st.name = name.clone();
        st.saved = st.working.clone();
        st.saved_name = name.clone();
        st.props_edited = false;
        st.rev += 1;
        st.status = Some(if copy { format!("Copied to {MY_BRUSHES_SET}") } else { format!("Saved to {MY_BRUSHES_SET}") });
        let brush = st.working.brush.clone();
        self.tool_brushes[tool.index()] = id.clone();
        if props && tool != Tool::Smudge {
            self.apply_brush_defaults(tool, &brush);
        }
        self.show_toast(format!("Saved \u{201c}{name}\u{201d} to {MY_BRUSHES_SET}"), 2.0);
        Some(id)
    }

    /// Delete the My Brushes entry in the Studio and close it. Tools that used it go back to
    /// their default brush.
    pub(crate) fn studio_delete(&mut self) {
        let Some(st) = self.studio.take() else {
            return;
        };
        if !st.is_user_brush() {
            self.studio = Some(st);
            return;
        }
        if let Err(e) = self.bank.delete_user(&st.target) {
            self.show_toast(format!("Could not delete the brush: {e}"), 4.0);
            self.studio = Some(st);
            return;
        }
        self.previews.invalidate(&st.target);
        for tool in Tool::ALL {
            if self.tool_brushes[tool.index()] == st.target {
                self.tool_brushes[tool.index()] = crate::brush_library::default_brush(tool.index());
            }
        }
    }
}

/// `--selftest`: open the Studio on a library brush from the Brush Library, change spacing and
/// tilt-size, save it as one of My Brushes, load it again from disk in a fresh bank, and check
/// that the settings (and the tilt response on the pad) persisted.
pub fn selftest_check() -> Result<String, String> {
    use crate::app::Prefs;
    use crate::brush_import::BrushStore;
    use crate::brush_library::BrushBank;
    use crate::document::Document;
    use crate::popover::Panel;
    use crate::snapshot::Headless;
    use eskiz_core::geometry::PointerType;

    let root = std::env::temp_dir().join(format!("eskiz-selftest-studio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (imports, mine) = (BrushStore::at(root.join("imports")), BrushStore::at(root.join("mine")));
    let result = (|| -> Result<String, String> {
        let mut h = Headless::new(egui::Vec2::new(1280.0, 860.0), Document::new(512, 384), Prefs::default());
        h.app.bank = BrushBank::with_stores(imports.clone(), mine.clone());

        // Open the Studio from the Brush Library's Edit button.
        h.open_panel(Panel::Brush);
        let edit = h.app.library_layout.edit.ok_or("studio: the Brush Library has no Edit button")?;
        h.click(edit.center());
        h.advance(0.3);
        let source = h.app.studio.as_ref().map(|s| s.target.clone()).ok_or("studio: Edit did not open the Studio")?;
        let original = h.app.bank.find(&source).ok_or("studio: the library brush is gone")?;
        if !matches!(source, BrushId::Library(_)) {
            return Err("studio: expected to open on a library brush".into());
        }

        // Change spacing and tilt-size, then save as a user brush.
        h.app.studio_edit_brush(|b| {
            b.spacing = 0.42;
            b.tilt_size = 0.7;
        });
        h.app.studio_rename("Selftest pen".into());
        let id = h.app.studio_save().ok_or("studio: Save failed")?;
        let BrushId::User(key) = &id else {
            return Err("studio: Save did not make a My Brushes entry".into());
        };
        if !mine.contains(key) {
            return Err("studio: Save wrote no .brush file".into());
        }
        if h.app.tool_brushes[0] != id || (h.app.stroke_settings().brush.spacing - 0.42).abs() > 1e-9 {
            return Err("studio: the saved brush is not the tool's brush".into());
        }
        if h.app.bank.find(&source).is_some_and(|b| b.brush != original.brush) {
            return Err("studio: saving changed the library brush".into());
        }

        // Reload from disk in a new bank and compare.
        let fresh = BrushBank::with_stores(imports.clone(), mine.clone());
        let back = fresh.find(&id).ok_or("studio: the saved brush was not reloaded")?;
        if (back.brush.spacing, back.brush.tilt_size) != (0.42, 0.7) {
            return Err(format!("studio: reloaded spacing {} and tilt-size {}", back.brush.spacing, back.brush.tilt_size));
        }
        if back.tip != original.tip || back.grain != original.grain {
            return Err("studio: the tip or grain changed in the file".into());
        }

        // The reloaded brush leans: a tilted pen paints a fatter pad stroke than an upright one.
        let raster = engine_for(&back).raster;
        let mut pad = Pad::default();
        pad.ensure(320, 160, 1);
        let mut brush = back.brush.clone();
        brush.size = 14.0;
        let mut ink_for = |altitude: f64| {
            pad.clear();
            pad.begin(&brush, raster.clone());
            for i in 0..40 {
                pad.push(Sample {
                    x: 30.0 + f64::from(i) * 6.0,
                    y: 80.0,
                    t: f64::from(i) / 120.0,
                    pressure: Some(0.8),
                    tilt: Some(altitude),
                    azimuth: Some(0.0),
                    pointer: PointerType::Pen,
                    has_pressure: Some(true),
                    ..Sample::default()
                });
            }
            pad.end();
            pad.ink_pixels(64)
        };
        let (upright, leaning) = (ink_for(std::f64::consts::FRAC_PI_2), ink_for(0.6));
        if leaning <= upright + upright / 2 {
            return Err(format!("studio: tilt-size does nothing on the pad ({leaning} leaning vs {upright} upright)"));
        }
        Ok(format!(
            "  brush bench: opened on a library brush, spacing and tilt-size saved to My Brushes and reloaded; pad ink {upright} upright, {leaning} tilted"
        ))
    })();
    let _ = std::fs::remove_dir_all(&root);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_named_in_order() {
        assert_eq!(copy_name("Pen"), "Pen copy");
        assert_eq!(copy_name("Pen copy"), "Pen copy 2");
        assert_eq!(copy_name("Pen copy 2"), "Pen copy 3");
        assert_eq!(copy_name("  Pen  "), "Pen copy");
    }
}
