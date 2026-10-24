//! Brush Library data: the sets and brushes the panel lists, the engine
//! resources of each brush, and cached preview textures.
//!
//! The sets are the eight built-in sets of `eskiz_brushlib::library_ref()`, then the
//! "Imported" set of the user's own brushes (see `brush_import`). A brush is named by a
//! [`BrushId`]. Each tool keeps its own brush. [`BrushBank`] builds a brush's [`Engine`] (its
//! tip and grain as a [`DabRaster`]) on first use and keeps it for the session.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use eskiz_brushlib::{library_ref, preview_stroke, LibraryBrush};
use eskiz_core::brushes::Brush;
use eskiz_core::raster::{DabRaster, GrainTexture, ShapeTexture};
use eskiz_format::brushes::ImportedBrush;

use crate::brush_import::{self, BrushStore, IMPORTED_SET, MY_BRUSHES_SET};
use crate::icons::{self, IconFn};
use crate::icons_brush;

/// Preview size in points.
pub const PREVIEW_W: f32 = 208.0;
pub const PREVIEW_H: f32 = 52.0;
/// Previews rendered per frame at most, so opening the library never stalls a frame.
pub const RENDERS_PER_FRAME: usize = 2;

/// The default brush of each tool: Brush, Smudge and Eraser.
pub const DEFAULT_BRUSHES: [&str; 3] = ["inking.studio-ink", "painting.soft-oil-round", "inking.technical-fineliner"];

/// Names one brush. Built-in brushes are named by their library id, imported ones by the key of
/// their file in the brush store, and the user's own (made in the Brush Bench) by the key of
/// their file in the My Brushes store.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BrushId {
    Library(&'static str),
    Imported(String),
    User(String),
}

impl BrushId {
    /// Text form for prefs: `lib:<id>`, `imp:<key>` or `usr:<key>`.
    pub fn encode(&self) -> String {
        match self {
            BrushId::Library(id) => format!("lib:{id}"),
            BrushId::Imported(key) => format!("imp:{key}"),
            BrushId::User(key) => format!("usr:{key}"),
        }
    }

    /// Parses [`BrushId::encode`] output. A built-in id must be in the library.
    pub fn decode(text: &str) -> Option<Self> {
        if let Some(key) = text.strip_prefix("imp:") {
            return (!key.is_empty()).then(|| BrushId::Imported(key.to_owned()));
        }
        if let Some(key) = text.strip_prefix("usr:") {
            return (!key.is_empty()).then(|| BrushId::User(key.to_owned()));
        }
        let id = text.strip_prefix("lib:")?;
        library_ref()
            .iter()
            .flat_map(|s| &s.brushes)
            .find(|b| b.id == id)
            .map(|b| BrushId::Library(b.id))
    }
}

/// The brush of each tool, in prefs form: three entries separated by `|`, in tool order. An
/// entry that does not parse is `None`, so the tool keeps its default.
pub fn decode_choice(text: &str) -> [Option<BrushId>; 3] {
    let mut out = [None, None, None];
    for (slot, part) in out.iter_mut().zip(text.split('|')) {
        *slot = BrushId::decode(part);
    }
    out
}

pub fn encode_choice(ids: &[BrushId; 3]) -> String {
    ids.iter().map(BrushId::encode).collect::<Vec<_>>().join("|")
}

/// The built-in brush a tool starts with.
pub fn default_brush(tool: usize) -> BrushId {
    BrushId::Library(DEFAULT_BRUSHES[tool.min(2)])
}

/// A brush as the library holds it: a built-in one borrowed for the whole run, or an imported or
/// user one shared with the bank.
#[derive(Clone)]
pub enum LibRef {
    Builtin(&'static LibraryBrush),
    /// Read from a brush file: an imported brush, or one of My Brushes.
    Imported(Arc<LibraryBrush>),
}

impl Deref for LibRef {
    type Target = LibraryBrush;
    fn deref(&self) -> &LibraryBrush {
        match self {
            LibRef::Builtin(b) => b,
            LibRef::Imported(b) => b,
        }
    }
}

/// One brush in the panel.
#[derive(Clone)]
pub struct CatalogBrush {
    pub id: BrushId,
    pub name: String,
    pub lb: LibRef,
}

/// One set in the panel's left column.
#[derive(Clone)]
pub struct CatalogSet {
    pub name: String,
    pub icon: IconFn,
    pub brushes: Vec<CatalogBrush>,
}

fn icon_for(set: &str) -> IconFn {
    match eskiz_brushlib::canonical_set_name(set) {
        MY_BRUSHES_SET => icons_brush::icon_set_mine,
        IMPORTED_SET => crate::icons_tools::icon_load,
        "Inkwell" => icons::icon_set_inking,
        "Graphite" => icons::icon_set_sketching,
        "Mist" => icons::icon_set_airbrushing,
        "Pigment" => icons::icon_set_painting,
        "Surfaces" => icons_brush::icon_set_textures,
        "Char" => icons_brush::icon_set_charcoal,
        "Nibs" => icons_brush::icon_set_calligraphy,
        "Washes" => icons_brush::icon_wet,
        _ => icons::icon_brush,
    }
}

fn builtin_sets() -> Vec<CatalogSet> {
    library_ref()
        .iter()
        .map(|set| CatalogSet {
            name: set.name.to_owned(),
            icon: icon_for(set.name),
            brushes: set
                .brushes
                .iter()
                .map(|lb| CatalogBrush {
                    id: BrushId::Library(lb.id),
                    name: lb.name.to_owned(),
                    lb: LibRef::Builtin(lb),
                })
                .collect(),
        })
        .collect()
}

/// Ink in a preview of `lb`, and the width of its horizontal extent, for a row of `PREVIEW_W` at
/// 2x. Ink is coverage of 8 or more: faint brushes (Smudge Stick peaks near 10%) still count.
pub fn preview_ink(lb: &LibraryBrush) -> (usize, usize) {
    let mask = eskiz_brushlib::stroke_mask(lb, PREVIEW_W as u32 * 2, PREVIEW_H as u32 * 2);
    let ink = mask.pixels().filter(|p| p[0] >= 8).count();
    let (mut lo, mut hi) = (usize::MAX, 0);
    for (x, _, p) in mask.enumerate_pixels() {
        if p[0] >= 8 {
            lo = lo.min(x as usize);
            hi = hi.max(x as usize);
        }
    }
    (ink, if lo == usize::MAX { 0 } else { hi - lo + 1 })
}

/// The engine resources of one brush: its settings, its tip and grain as a dab raster, and the
/// tip on its own (smudge uses it to weight the disc).
pub struct Engine {
    pub brush: Brush,
    pub raster: Arc<DabRaster>,
    pub tip: Arc<ShapeTexture>,
}

/// Builds the engine resources of a library brush from its grey tip and grain.
pub fn engine_for(lb: &LibraryBrush) -> Engine {
    let shape = ShapeTexture::from_gray(lb.tip.width() as usize, lb.tip.as_raw())
        .unwrap_or_else(|| ShapeTexture::new(lb.brush.shape, 128));
    let grain = lb
        .grain
        .as_ref()
        .and_then(|g| GrainTexture::from_gray(g.width() as usize, g.as_raw()));
    let raster = DabRaster::with_textures(&lb.brush, shape.clone(), grain);
    Engine {
        brush: lb.brush.clone(),
        raster: Arc::new(raster),
        tip: Arc::new(shape),
    }
}

/// The brushes the panel lists, and their engine resources built on demand.
pub struct BrushBank {
    /// Every set, built-in first, then Imported, then My Brushes. Shared with the panel each frame.
    pub sets: Arc<Vec<CatalogSet>>,
    builtin: Vec<CatalogSet>,
    imported: Vec<CatalogBrush>,
    user: Vec<CatalogBrush>,
    /// The file contents behind each imported and user brush, so a re-save keeps settings Eskiz
    /// does not edit.
    sources: HashMap<BrushId, ImportedBrush>,
    store: BrushStore,
    /// Where My Brushes are saved. `None` keeps them in memory only (headless tests).
    user_store: Option<BrushStore>,
    engines: HashMap<BrushId, Arc<Engine>>,
    /// Imported and My Brushes files that were found but could not be read, so are not listed.
    pub skipped: usize,
}

impl BrushBank {
    /// The bank with the built-in library and whatever the import store holds. My Brushes are
    /// kept in memory only; see [`BrushBank::with_stores`].
    pub fn new(store: BrushStore) -> Self {
        Self::build(store, None)
    }

    /// Like [`BrushBank::new`], and My Brushes are saved in, and read from, `user_store`.
    pub fn with_stores(store: BrushStore, user_store: BrushStore) -> Self {
        Self::build(store, Some(user_store))
    }

    fn build(store: BrushStore, user_store: Option<BrushStore>) -> Self {
        let mut sources = HashMap::new();
        let mut skipped = 0;
        let mut read = |entries: Vec<(String, ImportedBrush)>, make: fn(String) -> BrushId| -> Vec<CatalogBrush> {
            entries
                .into_iter()
                .filter_map(|(key, ib)| {
                    let Ok(lb) = brush_import::library_brush(&ib) else {
                        skipped += 1;
                        return None;
                    };
                    let id = make(key);
                    let cat = CatalogBrush { id: id.clone(), name: display_name(&ib.name), lb: LibRef::Imported(Arc::new(lb)) };
                    sources.insert(id, ib);
                    Some(cat)
                })
                .collect()
        };
        let imported = read(store.load(), BrushId::Imported);
        let user = user_store.as_ref().map(|s| read(s.load(), BrushId::User)).unwrap_or_default();
        let mut bank = Self {
            sets: Arc::new(Vec::new()),
            builtin: builtin_sets(),
            imported,
            user,
            sources,
            store,
            user_store,
            engines: HashMap::new(),
            skipped,
        };
        bank.rebuild();
        bank
    }

    fn rebuild(&mut self) {
        let mut sets = self.builtin.clone();
        sets.push(CatalogSet {
            name: IMPORTED_SET.to_owned(),
            icon: icon_for(IMPORTED_SET),
            brushes: self.imported.clone(),
        });
        sets.push(CatalogSet {
            name: MY_BRUSHES_SET.to_owned(),
            icon: icon_for(MY_BRUSHES_SET),
            brushes: self.user.clone(),
        });
        self.sets = Arc::new(sets);
    }

    /// Stores an imported brush and adds it to the Imported set. Nothing is written when the
    /// brush cannot be mapped.
    pub fn add_imported(&mut self, ib: &ImportedBrush) -> Result<BrushId, String> {
        let lb = brush_import::library_brush(ib)?;
        let key = self.store.save(ib)?;
        let id = BrushId::Imported(key);
        self.imported.push(CatalogBrush {
            id: id.clone(),
            name: display_name(&ib.name),
            lb: LibRef::Imported(Arc::new(lb)),
        });
        self.sources.insert(id.clone(), ib.clone());
        self.rebuild();
        Ok(id)
    }

    /// The file contents behind an imported or user brush (`None` for a built-in one).
    pub fn source(&self, id: &BrushId) -> Option<&ImportedBrush> {
        self.sources.get(id)
    }

    /// A key for a new user brush named `name` that no other user brush has.
    fn fresh_user_key(&self, name: &str) -> String {
        let base = brush_import::safe_name(name);
        let taken = |k: &str| {
            self.user.iter().any(|b| b.id == BrushId::User(k.to_owned()))
                || self.user_store.as_ref().is_some_and(|s| s.contains(k))
        };
        let mut key = base.clone();
        let mut n = 2;
        while taken(&key) {
            key = format!("{base} {n}");
            n += 1;
        }
        key
    }

    /// A name for a new user brush that no other user brush shows: `name`, else "name 2", ...
    fn fresh_user_name(&self, name: &str) -> String {
        let taken = |n: &str| self.user.iter().any(|b| b.name == display_name(n));
        let base = display_name(name);
        let mut out = base.clone();
        let mut n = 2;
        while taken(&out) {
            out = format!("{base} {n}");
            n += 1;
        }
        out
    }

    /// Saves `ib` as one of My Brushes: over the brush `key` when it is given and exists, else as
    /// a new brush (renamed "Name 2", ... when the name is already shown in My Brushes, so the
    /// rows can be told apart). Writes the file first, so nothing changes when that fails.
    /// Returns the id.
    pub fn save_user(&mut self, key: Option<&str>, ib: &ImportedBrush) -> Result<BrushId, String> {
        let renamed;
        let ib = match key {
            Some(_) => ib,
            None => {
                renamed = ImportedBrush { name: self.fresh_user_name(&ib.name), ..ib.clone() };
                &renamed
            }
        };
        let lb = brush_import::library_brush(ib)?;
        let key = match key {
            Some(k) => k.to_owned(),
            None => self.fresh_user_key(&ib.name),
        };
        if let Some(store) = &self.user_store {
            store.write(&key, ib)?;
        }
        let id = BrushId::User(key);
        let cat = CatalogBrush { id: id.clone(), name: display_name(&ib.name), lb: LibRef::Imported(Arc::new(lb)) };
        match self.user.iter_mut().find(|b| b.id == id) {
            Some(slot) => *slot = cat,
            None => self.user.push(cat),
        }
        self.sources.insert(id.clone(), ib.clone());
        self.engines.remove(&id);
        self.rebuild();
        Ok(id)
    }

    /// Deletes one of My Brushes. Other ids are left alone.
    pub fn delete_user(&mut self, id: &BrushId) -> Result<(), String> {
        let BrushId::User(key) = id else {
            return Ok(());
        };
        if let Some(store) = &self.user_store {
            store.remove(key)?;
        }
        self.user.retain(|b| &b.id != id);
        self.sources.remove(id);
        self.engines.remove(id);
        self.rebuild();
        Ok(())
    }

    /// The library brush for `id`, if the bank has it.
    pub fn find(&self, id: &BrushId) -> Option<LibRef> {
        self.sets
            .iter()
            .flat_map(|s| &s.brushes)
            .find(|b| &b.id == id)
            .map(|b| b.lb.clone())
    }

    /// Engine resources for `id`, built on first use. `None` when the bank does not have it.
    pub fn engine(&mut self, id: &BrushId) -> Option<Arc<Engine>> {
        if let Some(e) = self.engines.get(id) {
            return Some(e.clone());
        }
        let lb = self.find(id)?;
        let e = Arc::new(engine_for(&lb));
        self.engines.insert(id.clone(), e.clone());
        Some(e)
    }

    /// Engine resources for `id`, or the tool's default brush when `id` is unknown.
    pub fn engine_or_default(&mut self, id: &BrushId, tool: usize) -> Arc<Engine> {
        match self.engine(id) {
            Some(e) => e,
            None => self
                .engine(&default_brush(tool))
                .expect("the built-in default brushes are always in the library"),
        }
    }
}

fn display_name(name: &str) -> String {
    if name.trim().is_empty() {
        "Brush".to_owned()
    } else {
        name.to_owned()
    }
}

/// Index of the set that lists `id` (0 when no set does).
pub fn set_of(sets: &[CatalogSet], id: &BrushId) -> usize {
    sets.iter()
        .position(|s| s.brushes.iter().any(|b| &b.id == id))
        .unwrap_or(0)
}

/// Where the panel drew its set and brush rows last frame, in screen points (for tests).
#[derive(Default)]
pub struct LibraryLayout {
    pub sets: Vec<egui::Rect>,
    pub rows: Vec<(BrushId, egui::Rect)>,
    /// The Edit button, which opens the Brush Bench on the tool's brush.
    pub edit: Option<egui::Rect>,
}

/// The import field of the Brush Library: open or closed, and the path typed in it.
#[derive(Default)]
pub struct BrushUi {
    pub adding: bool,
    pub path: String,
}

/// Lazily rendered, cached preview textures: one per brush and display scale.
#[derive(Default)]
pub struct BrushPreviews {
    textures: HashMap<(BrushId, u32), TextureHandle>,
    /// How many previews have been rendered so far (for tests and diagnostics).
    pub renders: usize,
}

impl BrushPreviews {
    /// Pixels per point to render at for a display scale.
    pub fn scale_for(pixels_per_point: f32) -> u32 {
        (pixels_per_point.ceil() as u32).clamp(2, 3)
    }

    /// Forget the previews of `id`, so the next frame draws them again (the brush was edited).
    pub fn invalidate(&mut self, id: &BrushId) {
        self.textures.retain(|(b, _), _| b != id);
    }

    pub fn is_cached(&self, id: &BrushId, scale: u32) -> bool {
        self.textures.contains_key(&(id.clone(), scale))
    }

    /// The texture for `id`, rendering it first if it is missing and `budget` allows. Each render
    /// spends one unit of `budget`. Returns `None` while it is pending.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        id: &BrushId,
        lb: &LibraryBrush,
        scale: u32,
        budget: &mut usize,
    ) -> Option<&TextureHandle> {
        let key = (id.clone(), scale);
        if !self.textures.contains_key(&key) {
            if *budget == 0 {
                return None;
            }
            *budget -= 1;
            let (w, h) = ((PREVIEW_W as u32) * scale, (PREVIEW_H as u32) * scale);
            let img = preview_stroke(lb, w, h);
            self.renders += 1;
            let image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
            let name = format!("brush-preview-{}-{scale}x", id.encode());
            self.textures.insert(key.clone(), ctx.load_texture(name, image, TextureOptions::LINEAR));
        }
        self.textures.get(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::popover::Panel;
    use crate::app::Tool;

    fn temp_store(tag: &str) -> BrushStore {
        let dir = std::env::temp_dir().join(format!("eskiz-brushlib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        BrushStore::at(dir)
    }

    fn headless(light: bool) -> crate::snapshot::Headless {
        use crate::app::Prefs;
        use crate::document::Document;
        let prefs = Prefs { light, ..Prefs::default() };
        crate::snapshot::Headless::new(egui::Vec2::new(1280.0, 860.0), Document::new(2048, 1536), prefs)
    }

    /// The panel's rows as ids, in order.
    fn row_ids(h: &crate::snapshot::Headless) -> Vec<BrushId> {
        h.app.library_layout.rows.iter().map(|(id, _)| id.clone()).collect()
    }

    #[test]
    fn library_has_eight_sets_and_forty_brushes_then_imported_and_my_brushes() {
        let bank = BrushBank::new(temp_store("sets"));
        assert_eq!(bank.sets.len(), 10);
        let builtin: usize = bank.sets[..8].iter().map(|s| s.brushes.len()).sum();
        assert_eq!(builtin, 40);
        assert_eq!(bank.sets[8].name, IMPORTED_SET);
        assert!(bank.sets[8].brushes.is_empty());
        assert_eq!(bank.sets[9].name, MY_BRUSHES_SET);
        assert!(bank.sets[9].brushes.is_empty());
        let mut ids: Vec<&BrushId> = bank.sets.iter().flat_map(|s| &s.brushes).map(|b| &b.id).collect();
        ids.sort_by_key(|id| id.encode());
        ids.dedup();
        assert_eq!(ids.len(), 40, "every built-in id is unique");
        for id in DEFAULT_BRUSHES {
            assert!(bank.find(&BrushId::Library(id)).is_some(), "default {id} is in the library");
        }
    }

    #[test]
    fn brush_ids_and_tool_choices_round_trip_through_prefs() {
        for lb in library_ref().iter().flat_map(|s| &s.brushes) {
            let id = BrushId::Library(lb.id);
            assert_eq!(BrushId::decode(&id.encode()), Some(id));
        }
        let imp = BrushId::Imported("Sumi ink".into());
        assert_eq!(BrushId::decode(&imp.encode()), Some(imp.clone()));
        let usr = BrushId::User("My pen".into());
        assert_eq!(BrushId::decode(&usr.encode()), Some(usr));
        assert_eq!(BrushId::decode("usr:"), None);
        assert_eq!(BrushId::decode("lib:no.such.brush"), None);
        assert_eq!(BrushId::decode("imp:"), None);
        let choice = [imp.clone(), default_brush(1), default_brush(2)];
        let back = decode_choice(&encode_choice(&choice));
        assert_eq!(back, [Some(imp), Some(default_brush(1)), Some(default_brush(2))]);
        assert_eq!(decode_choice("junk|lib:inking.studio-ink"), [None, Some(default_brush(0)), None]);
    }

    #[test]
    fn every_library_preview_has_visible_ink_across_the_row() {
        for lb in library_ref().iter().flat_map(|s| &s.brushes) {
            let (ink, span) = preview_ink(lb);
            assert!(ink > 150, "{} is nearly blank: {ink} ink pixels", lb.name);
            assert!(span > 208, "{} does not span the row: {span}", lb.name);
        }
    }

    #[test]
    fn engine_uses_the_brush_tip_and_grain_textures() {
        let lb = library_ref()
            .iter()
            .flat_map(|s| &s.brushes)
            .find(|b| b.grain.is_some() && b.brush.grain != eskiz_core::brushes::Grain::None)
            .expect("the library has a textured brush");
        let e = engine_for(lb);
        assert_eq!(e.tip.size(), lb.tip.width() as usize, "the tip is the generated texture");
        let (u, v) = (0.5, 0.5);
        let expect = f64::from(lb.tip.get_pixel(lb.tip.width() / 2, lb.tip.height() / 2)[0]) / 255.0;
        assert!((e.tip.sample(u, v) - expect).abs() < 0.02, "center sample {} vs {expect}", e.tip.sample(u, v));
    }

    #[test]
    fn library_snapshots_cache_previews_and_select_on_click() {
        let dir = std::env::temp_dir();
        for light in [false, true] {
            let mut h = headless(light);
            h.open_panel(Panel::Brush);
            let theme = if light { "light" } else { "dark" };
            h.render().save_png(&dir.join(format!("eskiz-brush-library-{theme}.png"))).unwrap();
            let rows = h.app.library_layout.rows.clone();
            assert_eq!(rows.len(), h.app.bank.sets[set_of(&h.app.bank.sets, &h.app.tool_brushes[0])].brushes.len());
            // Every visible row has its preview cached, and drawing again renders nothing.
            let renders = h.app.previews.renders;
            assert_eq!(renders, rows.len());
            h.advance(0.3);
            assert_eq!(h.app.previews.renders, renders, "previews re-rendered on a later frame");

            // Close and reopen: the cache serves it.
            h.open_panel(Panel::Brush);
            h.open_panel(Panel::Brush);
            assert_eq!(h.app.previews.renders, renders, "reopening re-rendered previews");

            // Click a row other than the current brush: it becomes the brush for the Brush tool.
            let (id, rect) = rows.iter().find(|(p, _)| *p != h.app.tool_brushes[0]).cloned().unwrap();
            h.click(rect.center());
            assert_eq!(h.app.tool_brushes[0], id);
            h.advance(0.1);
            h.render().save_png(&dir.join(format!("eskiz-brush-library-selected-{theme}.png"))).unwrap();
        }
    }

    #[test]
    fn choosing_a_set_lists_its_brushes_and_each_tool_keeps_its_own_brush() {
        let mut h = headless(false);
        h.open_panel(Panel::Brush);
        let sets = h.app.bank.sets.clone();
        h.click(h.app.library_layout.sets[2].center());
        h.advance(0.1);
        let shown = row_ids(&h);
        let expect: Vec<BrushId> = sets[2].brushes.iter().map(|b| b.id.clone()).collect();
        assert_eq!(shown, expect);
        for i in 0..sets.len() {
            let r = h.app.library_layout.sets[i];
            h.click(r.center());
            h.advance(0.1);
        }
        // The last set is Imported, whose rows are the user's imported brushes (none on a fresh install).
        assert_eq!(sets.len(), 10);
        h.click(h.app.library_layout.sets[8].center());
        h.advance(0.1);
        let imported: Vec<BrushId> = sets[8].brushes.iter().map(|b| b.id.clone()).collect();
        assert_eq!(row_ids(&h), imported);
        h.click(h.app.library_layout.sets[2].center());
        h.advance(0.1);
        // Smudge opens the same library for its own brush.
        let before = h.app.tool_brushes.clone();
        h.app.library_tool = Tool::Smudge;
        h.advance(0.1);
        let (id, rect) = h.app.library_layout.rows.iter().find(|(p, _)| *p != before[1]).cloned().unwrap();
        h.click(rect.center());
        assert_eq!(h.app.tool_brushes[1], id);
        assert_eq!(h.app.tool_brushes[0], before[0]);
        assert_eq!(h.app.tool_brushes[2], before[2]);
    }

    #[test]
    fn library_panel_snapshot_is_written_to_the_temp_dir() {
        let mut h = headless(false);
        h.open_panel(Panel::Brush);
        let path = std::env::temp_dir().join("eskiz-brush-library-panel.png");
        h.render().save_png(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        eprintln!("brush library panel: {}", path.display());
    }

    /// Writes `brush_sets.png`: the set column, with a glyph beside each set name.
    #[test]
    fn brush_sets_snapshot_shows_a_glyph_for_every_set() {
        let mut h = headless(false);
        h.open_panel(Panel::Brush);
        // The panel sizes itself on the frame after it appears.
        h.render();
        let path = crate::snapshot::snapshot_dir().join("brush_sets.png");
        h.render().save_png(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
    }

    #[test]
    fn importing_brush_files_adds_them_to_the_imported_set_and_selects_the_first() {
        use eskiz_format::brushes::{self as fb, keys, ImportedBrush, SettingValue};
        let store = temp_store("import");
        let dir = store.clone();
        let mut h = headless(false);
        // Keep the test's imports out of the user's brush folder.
        h.app.bank = BrushBank::new(store);
        let mut one = ImportedBrush { name: "Soft Gouache".into(), ..ImportedBrush::default() };
        one.settings.insert(keys::PLOT_SPACING.into(), SettingValue::Number(0.3));
        let set = ImportedBrush { name: "Set A".into(), ..ImportedBrush::default() };
        let set2 = ImportedBrush { name: "Set B".into(), ..ImportedBrush::default() };
        let files = std::env::temp_dir().join(format!("eskiz-brush-files-{}", std::process::id()));
        std::fs::create_dir_all(&files).unwrap();
        let brush_path = files.join("Soft Gouache.brush");
        std::fs::write(&brush_path, fb::write_brush(&one).unwrap()).unwrap();
        let set_path = files.join("Pair.brushset");
        std::fs::write(&set_path, fb::write_brushset("Pair", &[set, set2]).unwrap()).unwrap();

        assert!(h.app.import_brush_file(&brush_path));
        assert_eq!(h.app.bank.sets[8].brushes.len(), 1);
        assert!(matches!(h.app.tool_brushes[0], BrushId::Imported(_)), "the import is selected for the library tool");
        assert!(h.app.stroke_settings().raster.is_some(), "an imported brush paints through its raster");

        assert!(h.app.import_brush_file(&set_path));
        assert_eq!(h.app.bank.sets[8].brushes.len(), 3, "a set adds each brush");

        // The Imported set survives a restart: a new bank on the same folder lists all three.
        let reloaded = BrushBank::new(BrushStore::at(dir.root()));
        assert_eq!(reloaded.sets[8].brushes.len(), 3);
        assert!(reloaded.sets[8].brushes.iter().any(|b| b.name == "Soft Gouache"));
        let _ = std::fs::remove_dir_all(&files);
    }

    #[test]
    fn previews_are_deterministic() {
        let lb = &library_ref()[0].brushes[0];
        assert_eq!(preview_stroke(lb, 416, 104).as_raw(), preview_stroke(lb, 416, 104).as_raw());
    }
}
