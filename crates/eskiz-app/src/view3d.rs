//! 3D painting in the app.
//!
//! * [`ThreeD`] is the open 3D document and its view state: the orbit camera, the screen-to-UV
//!   lookup, the shaded image in the viewport, and the stroke in progress.
//! * Models arrive through [`import_job`] on a worker thread (glTF/GLB, OBJ or USD/USDZ, with the model
//!   checks run by `eskiz-3d`). A fatal check puts its messages in an error sheet; warnings
//!   become a dismissible banner in the view.
//! * Painting maps the screen dabs through the lookup with [`PaintSession`] into the colour map
//!   of the surface's texture set. The lookup is rebuilt when the camera, size or paint-through
//!   changes. While the camera moves the view renders at half resolution.
//! * Storage: a 3D document is the folder `<id>.eskiz3d` in the library. It holds the model as a
//!   GLB, one PNG per map per texture set under `layers/` (these are authoritative on load), and
//!   `document.json` with the set names, visibility, lock and opacity. The gallery thumbnail and
//!   metadata sit beside it, as for `.procreate` packages.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use eskiz_3d::export::encode_png;
use eskiz_3d::lookup::{render_lookup, Hit, LookupBuffer, RenderOptions, Side};
use eskiz_3d::model::{srgb_to_linear, MapKind, Model, SubMesh};
use eskiz_3d::shade::{encode_output, shade, Light, SurfaceInput};
use eskiz_3d::{
    export_glb, export_map_pngs, export_usdz, load_gltf, load_gltf_slice, load_obj, write_glb_bytes, ImportError, ImportOptions,
    ImportedModel, OrbitCamera, PaintSession, PaintSettings, TexelChange, DEFAULT_DILATION, restore_texels,
};
use eskiz_core::geometry::{Stroke, StrokeOptions};
use eskiz_core::{Brush, Dab, Layer, PaintOptions};
use eframe::egui::{self, Align2, Color32, ColorImage, Id, Pos2, Rect, TextureOptions, Vec2};
use glam::{Vec2 as UvVec2, Vec3};
use serde_json::{json, Value};

use crate::app::{over_ui, EskizApp, FIT_TOP};
use crate::export::premultiply;
use crate::chrome::small_label;
use crate::gallery::Screen;
use crate::input::{sample_from, PointerKind, RawPointer};
use crate::paint::StrokeSettings;
use crate::storage::{clean_title, new_id, DocEntry, Library, AUTOSAVE_SECS};

/// Stored light intensity that the shader scales by 50: 0.02 gives radiance 1.0.
const DEFAULT_LIGHT: f32 = 0.02;
/// Viewport resolution scale while the camera moves (fraction of full device resolution).
const LOW_RES: f32 = 0.5;
/// How long the low-resolution view lasts after the last camera move, seconds.
const LOW_RES_HOLD: f64 = 0.25;
/// Orbit speed, radians per point of drag.
const ORBIT_SPEED: f32 = 0.008;
/// Background behind the model.
const BACKDROP: [u8; 3] = [36, 36, 40];
/// Dots per inch written with exported texture PNGs.
const EXPORT_DPI: u32 = 300;
/// Name of the document metadata file inside a 3D document folder.
const DOCUMENT_FILE: &str = "document.json";
const FORMAT_NAME: &str = "eskiz-3d";

// ---------------------------------------------------------------- files and models

/// Whether `path` is a model this module imports (glTF, GLB, OBJ, and with the `usd` feature USDZ/USDA/USDC).
pub fn is_model_file(path: &Path) -> bool {
    let ext = ext_lower(path);
    matches!(ext.as_deref(), Some("glb" | "gltf" | "obj"))
        || (cfg!(feature = "usd") && matches!(ext.as_deref(), Some("usdz" | "usda" | "usdc")))
}

fn ext_lower(path: &Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
}

/// Load a model file, dispatching on its extension. A fatal check comes back as
/// [`ImportError::Rejected`].
pub fn load_model_file(path: &Path) -> Result<ImportedModel, ImportError> {
    let opts = ImportOptions::default();
    match ext_lower(path).as_deref() {
        Some("glb" | "gltf") => load_gltf(path, &opts),
        Some("obj") => load_obj(path, &opts),
        #[cfg(feature = "usd")]
        Some("usdz" | "usda" | "usdc") => eskiz_3d::load_usd_file(path, &opts),
        _ => Err(ImportError::Parse(format!("{} is not a 3D model", path.display()))),
    }
}

/// Name of the PNG for one map of one texture set inside a document folder.
fn layer_file(set: usize, kind: MapKind) -> String {
    format!("{set}_{}.png", kind.file_stem())
}

// ---------------------------------------------------------------- settings and document

/// Shading of the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shade {
    /// Albedo only, unlit.
    Flat,
    /// Cook-Torrance with a GGX distribution, one directional light and ambient.
    Ggx,
}

/// Lighting and preview settings of the 3D tab. These are view settings, not document data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSettings {
    pub shade: Shade,
    /// Direction towards the light: yaw around +Y and elevation, degrees.
    pub light_yaw: f32,
    pub light_elevation: f32,
    /// Multiplier of the default light (1 = default).
    pub light_intensity: f32,
    /// Constant ambient light, 0 to 1.
    pub ambient: f32,
    /// Show the roughness map as grey instead of shading.
    pub show_roughness: bool,
    /// Show the metallic map as grey instead of shading.
    pub show_metallic: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            shade: Shade::Ggx,
            light_yaw: 35.0,
            light_elevation: 45.0,
            light_intensity: 1.0,
            ambient: 0.15,
            show_roughness: false,
            show_metallic: false,
        }
    }
}

impl ViewSettings {
    /// Unit vector towards the light.
    fn light_direction(&self) -> Vec3 {
        let (y, e) = (self.light_yaw.to_radians(), self.light_elevation.to_radians());
        Vec3::new(e.cos() * y.sin(), e.sin(), e.cos() * y.cos())
    }
}

/// A 3D document: the model, its name and its camera.
#[derive(Clone)]
pub struct ModelDoc {
    pub id: String,
    pub title: String,
    pub model: Model,
    /// The framed camera. "Reset view" returns here.
    pub home: OrbitCamera,
    pub camera: OrbitCamera,
    pub paint_through: bool,
    /// The map that strokes paint into (chosen in the Layers panel).
    pub active_map: MapKind,
    /// Changed since the last save.
    pub dirty: bool,
}

impl ModelDoc {
    /// A new document framed on the model's bounds, at a three-quarter view.
    pub fn new(id: String, title: String, model: Model) -> Self {
        let mut home = OrbitCamera::default();
        if let Some((lo, hi)) = model.bounds() {
            home.frame(lo, hi);
        }
        home.yaw = 0.6;
        home.pitch = 0.35;
        Self {
            id,
            title,
            model,
            home,
            camera: home,
            paint_through: false,
            active_map: MapKind::Colour,
            dirty: true,
        }
    }

    /// Return the camera to the framed view.
    pub fn reset_view(&mut self) {
        self.camera = self.home;
    }
}

/// Undo steps kept for 3D strokes. Each step holds only the texture tiles its stroke changed.
const TEXEL_UNDO_STEPS: usize = 30;

/// A stroke in progress: the dab generator and the texture-space paint session.
struct ActiveStroke {
    stroke: Stroke,
    session: PaintSession,
    /// Committed dabs already given to the session.
    sent: usize,
}

/// The lookup buffer and the view it was built for.
struct LookupCache {
    camera: OrbitCamera,
    width: usize,
    height: usize,
    through: bool,
    buf: LookupBuffer,
}

/// The open 3D document and its view state.
pub struct ThreeD {
    pub doc: ModelDoc,
    pub settings: ViewSettings,
    /// Dismissible warning banner (model check warnings).
    pub banner: Option<String>,
    texture: Option<egui::TextureHandle>,
    lookup: Option<LookupCache>,
    /// The shaded image needs redrawing.
    view_dirty: bool,
    stroke: Option<ActiveStroke>,
    /// Low-resolution rendering until this app time (seconds).
    low_until: f64,
    last_save: f64,
    saving: Option<JoinHandle<Result<(), String>>>,
    export: Option<Receiver<Result<String, String>>>,
    /// Finished 3D strokes, oldest first, each as the texture maps it changed (see [`ThreeD::push_stroke`]).
    undo_steps: Vec<Vec<TexelChange>>,
    redo_steps: Vec<Vec<TexelChange>>,
}

impl ThreeD {
    pub fn new(doc: ModelDoc, banner: Option<String>) -> Self {
        Self {
            doc,
            settings: ViewSettings::default(),
            banner,
            texture: None,
            lookup: None,
            view_dirty: true,
            stroke: None,
            low_until: 0.0,
            last_save: 0.0,
            saving: None,
            export: None,
            undo_steps: Vec::new(),
            redo_steps: Vec::new(),
        }
    }

    /// Mark the view for redrawing (after a change made outside the viewport).
    pub fn touch(&mut self) {
        self.view_dirty = true;
    }

    /// Record a finished 3D stroke as one undo step. A new stroke ends the redo steps.
    pub fn push_stroke(&mut self, changes: Vec<TexelChange>) {
        if changes.is_empty() {
            return;
        }
        self.redo_steps.clear();
        self.undo_steps.push(changes);
        if self.undo_steps.len() > TEXEL_UNDO_STEPS {
            self.undo_steps.remove(0);
        }
        self.doc.dirty = true;
        self.view_dirty = true;
    }

    /// Whether a 3D stroke can be undone.
    pub fn can_undo(&self) -> bool {
        !self.undo_steps.is_empty()
    }

    /// Whether an undone 3D stroke can be redone.
    pub fn can_redo(&self) -> bool {
        !self.redo_steps.is_empty()
    }

    /// Undo the last 3D stroke: its maps go back to how they were before it. False when there is
    /// nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(changes) = self.undo_steps.pop() else { return false };
        restore_texels(&mut self.doc.model, &changes, false);
        self.redo_steps.push(changes);
        self.doc.dirty = true;
        self.view_dirty = true;
        true
    }

    /// Redo the last undone 3D stroke. False when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(changes) = self.redo_steps.pop() else { return false };
        restore_texels(&mut self.doc.model, &changes, true);
        self.undo_steps.push(changes);
        self.doc.dirty = true;
        self.view_dirty = true;
        true
    }

    /// Rebuild the lookup if the camera, size or paint-through changed. Returns whether it did.
    fn ensure_lookup(&mut self, width: usize, height: usize) -> bool {
        let through = self.doc.paint_through;
        let camera = self.doc.camera;
        let fresh = self
            .lookup
            .as_ref()
            .is_some_and(|l| l.camera == camera && l.width == width && l.height == height && l.through == through);
        if fresh {
            return false;
        }
        let opts = RenderOptions { back_layer: through, cull_backfaces: false };
        let buf = render_lookup(&self.doc.model, &camera, width, height, opts);
        self.lookup = Some(LookupCache { camera, width, height, through, buf });
        self.view_dirty = true;
        true
    }
}

// ---------------------------------------------------------------- storage

/// Write `doc` as the folder `<id>.eskiz3d` in `lib`, with the gallery thumbnail and metadata.
/// The new copy is written beside the old one and swapped in, so a failed save keeps the last
/// good copy.
pub fn save_model_doc(lib: &Library, doc: &ModelDoc) -> Result<DocEntry, String> {
    let dir = lib.model_dir(&doc.id);
    let tmp = dir.with_extension("writing");
    let _ = fs::remove_dir_all(&tmp);
    let io = |e: std::io::Error| format!("{}: {e}", tmp.display());
    fs::create_dir_all(tmp.join("layers")).map_err(io)?;
    let sets: Vec<Value> = doc
        .model
        .texture_sets
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "width": s.width,
                "height": s.height,
                "visible": s.visible,
                "locked": s.locked,
                "opacity": s.opacity,
            })
        })
        .collect();
    let meta = json!({
        "format": FORMAT_NAME,
        "version": 1,
        "title": doc.title,
        "paint_through": doc.paint_through,
        "texture_sets": sets,
    });
    fs::write(tmp.join(DOCUMENT_FILE), meta.to_string()).map_err(io)?;
    let glb = write_glb_bytes(&doc.model).map_err(|e| e.to_string())?;
    fs::write(tmp.join("model.glb"), glb).map_err(io)?;
    for (i, set) in doc.model.texture_sets.iter().enumerate() {
        for kind in MapKind::ALL {
            let png = encode_png(&set.straight_rgba(kind), set.width, set.height, None).map_err(|e| e.to_string())?;
            fs::write(tmp.join("layers").join(layer_file(i, kind)), png).map_err(io)?;
        }
    }
    let _ = fs::remove_dir_all(&dir);
    fs::rename(&tmp, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let (w, h, rgba) = match doc.model.texture_sets.first() {
        Some(s) => (s.width, s.height, s.straight_rgba(MapKind::Colour)),
        None => (1, 1, vec![128, 128, 128, 255]),
    };
    lib.write_model_meta(&doc.id, &doc.title, w, h, &rgba)?;
    lib.get(&doc.id).ok_or_else(|| "saved document is missing".to_owned())
}

/// Read a 3D document folder back. The geometry comes from the GLB. Each texture set's maps come
/// from its layer PNGs when the sizes agree.
pub fn load_model_doc(lib: &Library, id: &str) -> Result<ModelDoc, String> {
    let dir = lib.model_dir(id);
    let text = fs::read_to_string(dir.join(DOCUMENT_FILE)).map_err(|e| format!("{}: {e}", dir.display()))?;
    let meta: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if meta["format"].as_str() != Some(FORMAT_NAME) {
        return Err("not a 3D document".into());
    }
    let title = meta["title"].as_str().unwrap_or("Untitled").to_owned();
    let glb = fs::read(dir.join("model.glb")).map_err(|e| format!("model.glb: {e}"))?;
    let mut model = load_gltf_slice(&glb, &ImportOptions::default()).map_err(|e| e.to_string())?.model;
    let saved = meta["texture_sets"].as_array().cloned().unwrap_or_default();
    if saved.len() != model.texture_sets.len() {
        return Err("the saved layers do not match the model".into());
    }
    for (i, (set, info)) in model.texture_sets.iter_mut().zip(&saved).enumerate() {
        if let Some(name) = info["name"].as_str() {
            set.name = name.to_owned();
        }
        set.visible = info["visible"].as_bool().unwrap_or(true);
        set.locked = info["locked"].as_bool().unwrap_or(false);
        set.opacity = info["opacity"].as_f64().unwrap_or(1.0) as f32;
        for kind in MapKind::ALL {
            let path = dir.join("layers").join(layer_file(i, kind));
            let Ok(bytes) = fs::read(&path) else { continue };
            let (w, h, rgba) = decode_png(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            if w == set.width && h == set.height {
                *set.map_mut(kind) = Layer::from_rgba(w, h, premultiply(&rgba));
            }
        }
    }
    let mut doc = ModelDoc::new(id.to_owned(), title, model);
    doc.paint_through = meta["paint_through"].as_bool().unwrap_or(false);
    doc.dirty = false;
    Ok(doc)
}

/// Decode an 8-bit RGBA PNG (straight alpha) from memory.
fn decode_png(bytes: &[u8]) -> Result<(usize, usize, Vec<u8>), String> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().map_err(|e| e.to_string())?;
    let size = reader.output_buffer_size().ok_or("PNG too large")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("expected 8-bit RGBA".into());
    }
    buf.truncate(info.buffer_size());
    Ok((info.width as usize, info.height as usize, buf))
}

// ---------------------------------------------------------------- import and open jobs

/// The result of an import or open job, handed back to the UI thread.
pub enum Outcome {
    /// A fatal problem: the messages for the error sheet.
    Failed(Vec<String>),
    Opened {
        doc: Box<ModelDoc>,
        banner: Option<String>,
        /// The gallery entry, when the document was saved to the library.
        entry: Option<DocEntry>,
    },
}

/// Import a model file (worker thread): run the checks, build the document and, when there is a
/// library, save it there.
pub fn import_job(lib: Option<Library>, path: PathBuf) -> Outcome {
    let imported = match load_model_file(&path) {
        Ok(m) => m,
        Err(ImportError::Rejected(report)) => return Outcome::Failed(report.messages()),
        Err(e) => return Outcome::Failed(vec![e.to_string()]),
    };
    let title = clean_title(path.file_stem().and_then(|s| s.to_str()).unwrap_or("Model"));
    let mut doc = ModelDoc::new(new_id(), title, imported.model);
    let mut notes = imported.report.messages();
    notes.extend(imported.notes);
    let banner = (!notes.is_empty()).then(|| notes.join(" "));
    let entry = match lib {
        Some(lib) => match save_model_doc(&lib, &doc) {
            Ok(entry) => {
                doc.dirty = false;
                Some(entry)
            }
            Err(e) => return Outcome::Failed(vec![format!("Could not save the document: {e}")]),
        },
        None => None,
    };
    Outcome::Opened { doc: Box::new(doc), banner, entry }
}

/// Open a saved 3D document (worker thread).
fn open_job(lib: Library, id: String) -> Outcome {
    match load_model_doc(&lib, &id) {
        Ok(doc) => Outcome::Opened { doc: Box::new(doc), banner: None, entry: None },
        Err(e) => Outcome::Failed(vec![format!("Could not open the 3D document: {e}")]),
    }
}

/// The 3D import or open job in flight, and the error sheet.
#[derive(Default)]
pub struct ModelJobs {
    rx: Option<Receiver<Outcome>>,
    /// The error sheet's messages, while it is shown.
    pub error: Option<Vec<String>>,
}

// ---------------------------------------------------------------- shading

fn to_u8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// World position and normal of the surface a lookup hit shows.
///
/// The hit UV is perspective-correct and wrapped into 0..1. It is unwrapped next to the
/// triangle's own UVs, then inverted to barycentric weights.
fn surface_point(sm: &SubMesh, t: usize, u: f32, v: f32) -> Option<(Vec3, Vec3)> {
    let uv = sm.tri_uvs(t);
    let p = sm.tri_positions(t);
    let centre = (uv[0] + uv[1] + uv[2]) / 3.0;
    let q = UvVec2::new(u + (centre.x - u).round(), v + (centre.y - v).round());
    let (e1, e2, d) = (uv[1] - uv[0], uv[2] - uv[0], q - uv[0]);
    let det = e1.x * e2.y - e2.x * e1.y;
    if det.abs() < 1e-12 {
        return None;
    }
    let b1 = (d.x * e2.y - e2.x * d.y) / det;
    let b2 = (e1.x * d.y - d.x * e1.y) / det;
    let b0 = 1.0 - b1 - b2;
    let pos = p[0] * b0 + p[1] * b1 + p[2] * b2;
    let normal = if sm.normals.is_empty() {
        (p[1] - p[0]).cross(p[2] - p[0])
    } else {
        let i = &sm.indices[t * 3..t * 3 + 3];
        let n = |k: usize| sm.normals[i[k] as usize];
        n(0) * b0 + n(1) * b1 + n(2) * b2
    };
    Some((pos, normal.normalize_or_zero()))
}

/// Colour of the pixel a lookup hit shows, or `None` where the surface is hidden.
fn shade_hit(model: &Model, hit: &Hit, s: &ViewSettings, light: &Light, eye: Vec3) -> Option<[u8; 3]> {
    let (si, t) = model.locate(hit.tri)?;
    let sm = &model.submeshes[si];
    let ts = model.texture_sets.get(sm.material)?;
    if !ts.visible {
        return None;
    }
    let (position, normal) = surface_point(sm, t, hit.u, hit.v)?;
    let grey = |g: f32| {
        let b = to_u8(g);
        [b, b, b]
    };
    if s.show_roughness {
        return Some(grey(ts.sample(MapKind::Roughness, hit.u, hit.v)[0]));
    }
    if s.show_metallic {
        return Some(grey(ts.sample(MapKind::Metallic, hit.u, hit.v)[0]));
    }
    let colour = ts.sample(MapKind::Colour, hit.u, hit.v);
    match s.shade {
        Shade::Flat => Some([to_u8(colour[0]), to_u8(colour[1]), to_u8(colour[2])]),
        Shade::Ggx => {
            let input = SurfaceInput {
                position,
                normal,
                eye,
                base_colour: Vec3::new(
                    srgb_to_linear(to_u8(colour[0])),
                    srgb_to_linear(to_u8(colour[1])),
                    srgb_to_linear(to_u8(colour[2])),
                ),
                roughness: ts.sample(MapKind::Roughness, hit.u, hit.v)[0],
                metallic: ts.sample(MapKind::Metallic, hit.u, hit.v)[0],
                ao: ts.sample(MapKind::AmbientOcclusion, hit.u, hit.v)[0],
            };
            let out = encode_output(shade(&input, std::slice::from_ref(light), Vec3::splat(s.ambient)), 1.0);
            Some([to_u8(out.x), to_u8(out.y), to_u8(out.z)])
        }
    }
}

/// Shade the whole lookup into an image.
fn shade_view(three: &ThreeD) -> Option<ColorImage> {
    let lookup = &three.lookup.as_ref()?.buf;
    let s = &three.settings;
    let light = Light::directional(s.light_direction(), Vec3::ONE, s.light_intensity * DEFAULT_LIGHT);
    let eye = three.doc.camera.eye();
    let mut rgba = Vec::with_capacity(lookup.width * lookup.height * 4);
    for y in 0..lookup.height {
        for x in 0..lookup.width {
            let hit = lookup.hit(x as f32 + 0.5, y as f32 + 0.5, Side::Front);
            let rgb = hit.and_then(|h| shade_hit(&three.doc.model, &h, s, &light, eye)).unwrap_or(BACKDROP);
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
    Some(ColorImage::from_rgba_unmultiplied([lookup.width, lookup.height], &rgba))
}

// ---------------------------------------------------------------- painting

/// Dabs for a stroke through `points` (lookup pixels), made the way the 2D brush engine makes them.
pub(crate) fn stroke_dabs(brush: &Brush, points: &[(f64, f64)]) -> Vec<Dab> {
    let mut stroke = Stroke::new(brush.clone(), StrokeOptions::default());
    for (i, &(x, y)) in points.iter().enumerate() {
        let raw = RawPointer { x, y, time: i as f64 / 120.0, kind: PointerKind::Mouse, force: None };
        let _ = stroke.add_sample(sample_from(raw));
    }
    stroke.finish().dabs
}

/// Paint `dabs` (lookup pixels) into `doc` through `lookup`, and finish the stroke.
pub(crate) fn paint_dabs(doc: &mut ModelDoc, lookup: &LookupBuffer, brush: Brush, colour: [f64; 3], erase: bool, dabs: &[Dab]) -> Vec<TexelChange> {
    let opts = PaintOptions { colour, erase, alpha_lock: false };
    let settings = PaintSettings { paint_through: doc.paint_through, dilation: DEFAULT_DILATION, map: doc.active_map };
    let mut session = PaintSession::new(brush, opts, settings);
    session.add_dabs(&doc.model, lookup, dabs);
    let changes = session.finish(&mut doc.model);
    doc.dirty = true;
    changes
}

/// Begin a stroke. The brush is scaled from points to lookup pixels.
fn begin_stroke(three: &mut ThreeD, settings: &StrokeSettings, ppp: f32) {
    let mut brush = settings.brush.clone();
    brush.size *= f64::from(ppp);
    let ps = PaintSettings {
        paint_through: three.doc.paint_through,
        dilation: DEFAULT_DILATION,
        map: three.doc.active_map,
    };
    let opts = PaintOptions { colour: settings.colour, erase: settings.erase, alpha_lock: false };
    three.stroke = Some(ActiveStroke {
        stroke: Stroke::new(brush.clone(), StrokeOptions::default()),
        session: PaintSession::new(brush, opts, ps),
        sent: 0,
    });
}

/// Send one lookup-pixel position to the stroke in progress and paint what it committed.
fn feed_stroke(three: &mut ThreeD, x: f64, y: f64, now: f64) {
    let raw = RawPointer { x, y, time: now, kind: PointerKind::Mouse, force: None };
    let ThreeD { doc, lookup, stroke, view_dirty, .. } = three;
    let (Some(active), Some(lookup)) = (stroke.as_mut(), lookup.as_ref()) else { return };
    let update = active.stroke.add_sample(sample_from(raw));
    if update.committed_dabs.is_empty() {
        return;
    }
    active.sent += update.committed_dabs.len();
    active.session.add_dabs(&doc.model, &lookup.buf, &update.committed_dabs);
    active.session.apply(&mut doc.model);
    doc.dirty = true;
    *view_dirty = true;
}

/// Viewport position to lookup pixels: relative to the viewport origin, scaled by device pixels.
fn to_lookup_px(p: Pos2, vp: Rect, ppp: f32) -> (f64, f64) {
    (f64::from((p.x - vp.min.x) * ppp), f64::from((p.y - vp.min.y) * ppp))
}

// ---------------------------------------------------------------- EskizApp glue

impl EskizApp {
    /// Import a model file in the background, save it to the library, then show it.
    pub(crate) fn start_model_import(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.ensure_storage(ctx);
        let lib = self.gal.library.clone();
        self.spawn_model_job("eskiz-3d-import", move || import_job(lib, path));
    }

    /// Open a 3D document from the gallery, in the background.
    pub(crate) fn start_model_open(&mut self, id: String) {
        let Some(lib) = self.gal.library.clone() else { return };
        self.spawn_model_job("eskiz-3d-open", move || open_job(lib, id));
    }

    fn spawn_model_job(&mut self, name: &str, job: impl FnOnce() -> Outcome + Send + 'static) {
        if self.model_jobs.rx.is_some() {
            self.show_toast("A 3D model is already loading.", 2.0);
            return;
        }
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
            let _ = tx.send(job());
        });
        if spawned.is_err() {
            self.show_toast("Could not start the 3D job.", 2.5);
            return;
        }
        self.model_jobs.rx = Some(rx);
        self.show_toast("Loading the 3D model...", 30.0);
    }

    /// Collect a finished import or open, and draw the error sheet. Called once per frame.
    pub(crate) fn model_frame(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.model_jobs.rx.as_ref() {
            match rx.try_recv() {
                Ok(outcome) => {
                    self.model_jobs.rx = None;
                    self.finish_model_job(outcome);
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(50)),
                Err(TryRecvError::Disconnected) => {
                    self.model_jobs.rx = None;
                    self.show_toast("The 3D job stopped unexpectedly.", 3.0);
                }
            }
        }
        if let Some(lines) = self.model_jobs.error.clone() {
            let mut open = true;
            egui::Window::new("Could not open the model")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    for line in &lines {
                        ui.label(line);
                    }
                    if ui.button("OK").clicked() {
                        open = false;
                    }
                });
            if !open {
                self.model_jobs.error = None;
            }
        }
    }

    fn finish_model_job(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Failed(lines) => {
                self.toast = None;
                self.model_jobs.error = Some(lines);
            }
            Outcome::Opened { doc, banner, entry } => {
                if entry.is_some() {
                    self.gal.refresh();
                }
                let title = doc.title.clone();
                self.open_three_d(*doc, banner);
                self.show_toast(format!("Opened {title}"), 2.5);
            }
        }
    }

    /// Show a 3D document in the canvas area, in place of the 2D canvas.
    fn open_three_d(&mut self, doc: ModelDoc, banner: Option<String>) {
        self.cancel_stroke();
        self.adjust_cancel();
        self.exit_edit_mode();
        // The text tool works on the 2D canvas only: finish any typing before the view changes.
        self.end_text_edit();
        self.gal.current = None;
        self.gal.screen = Screen::Canvas;
        self.three_d = Some(Box::new(ThreeD::new(doc, banner)));
    }

    /// Leave the 3D view: finish any stroke, wait for a running autosave, and save if changed.
    pub(crate) fn close_three_d(&mut self) {
        self.finish_three_d_stroke();
        let Some(mut three) = self.three_d.take() else { return };
        if let Some(job) = three.saving.take() {
            let _ = job.join();
        }
        if three.doc.dirty {
            if let Some(lib) = self.gal.library.clone() {
                if let Err(e) = save_model_doc(&lib, &three.doc) {
                    self.show_toast(format!("Could not save the 3D document: {e}"), 4.0);
                }
            }
        }
    }

    /// Save the open 3D document now if it changed (app exit and periodic saves).
    pub(crate) fn save_three_d_now(&mut self) {
        self.finish_three_d_stroke();
        let Some(three) = self.three_d.as_mut() else { return };
        if let Some(job) = three.saving.take() {
            let _ = job.join();
        }
        if !three.doc.dirty {
            return;
        }
        let Some(lib) = self.gal.library.clone() else { return };
        if save_model_doc(&lib, &three.doc).is_ok() {
            three.doc.dirty = false;
        }
    }

    /// Return the open 3D document's camera to the framed view.
    pub(crate) fn reset_three_d_view(&mut self) {
        if let Some(three) = self.three_d.as_mut() {
            three.doc.reset_view();
            three.low_until = 0.0;
        }
    }

    /// Stop a stroke in progress and keep the paint it has made.
    fn finish_three_d_stroke(&mut self) {
        let Some(three) = self.three_d.as_mut() else { return };
        let Some(mut active) = three.stroke.take() else { return };
        let end = active.stroke.finish();
        let rest: Vec<Dab> = end.dabs.get(active.sent..).unwrap_or(&[]).to_vec();
        if let Some(lookup) = three.lookup.as_ref() {
            active.session.add_dabs(&three.doc.model, &lookup.buf, &rest);
        }
        let changes = active.session.finish(&mut three.doc.model);
        three.push_stroke(changes);
        three.doc.dirty = true;
        three.view_dirty = true;
    }

    /// Undo or redo the last 3D stroke. The stroke in progress is finished first. While the 3D
    /// view is open the 2D history is not used.
    pub(crate) fn step_three_d(&mut self, redo: bool) {
        self.finish_three_d_stroke();
        let done = self.three_d.as_mut().is_some_and(|t| if redo { t.redo() } else { t.undo() });
        if done {
            self.show_toast(if redo { "Redo 3D Stroke" } else { "Undo 3D Stroke" }, 1.0);
        }
    }

    /// Export the model as a GLB, in the background.
    pub(crate) fn start_glb_export(&mut self, path: PathBuf) {
        let Some(three) = self.three_d.as_mut() else { return };
        let model = three.doc.model.clone();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("eskiz-3d-export".into()).spawn(move || {
            let result = export_glb(&model, &path).map(|()| format!("GLB to {}", path.display()));
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
        if spawned.is_ok() {
            three.export = Some(rx);
        }
    }

    /// Export the model as a USDZ package, in the background.
    pub(crate) fn start_usdz_export(&mut self, path: PathBuf) {
        let Some(three) = self.three_d.as_mut() else { return };
        let model = three.doc.model.clone();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("eskiz-3d-export".into()).spawn(move || {
            let result = export_usdz(&model, &path).map(|()| format!("USDZ to {}", path.display()));
            let _ = tx.send(result.map_err(|e| e.to_string()));
        });
        if spawned.is_ok() {
            three.export = Some(rx);
        }
    }

    /// Export one PNG per map per texture set into `dir`, in the background.
    pub(crate) fn start_texture_export(&mut self, dir: PathBuf) {
        let Some(three) = self.three_d.as_mut() else { return };
        let model = three.doc.model.clone();
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("eskiz-3d-export".into()).spawn(move || {
            let result = export_map_pngs(&model, &dir, Some(EXPORT_DPI))
                .map(|paths| format!("{} texture PNGs to {}", paths.len(), dir.display()))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        if spawned.is_ok() {
            three.export = Some(rx);
        }
    }

    /// Report a finished export as a toast.
    fn poll_three_d_export(&mut self) {
        let Some(three) = self.three_d.as_mut() else { return };
        let Some(rx) = three.export.as_ref() else { return };
        let message = match rx.try_recv() {
            Ok(Ok(done)) => Some(format!("Exported {done}")),
            Ok(Err(e)) => Some(format!("Export failed: {e}")),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some("Export failed.".to_owned()),
        };
        if let Some(text) = message {
            three.export = None;
            self.show_toast(text, 3.5);
        }
    }

    /// Autosave the 3D document on a worker thread every [`AUTOSAVE_SECS`] while it has changes.
    fn autosave_three_d(&mut self, now: f64) {
        let Some(lib) = self.gal.library.clone() else { return };
        let Some(three) = self.three_d.as_mut() else { return };
        if three.saving.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(job) = three.saving.take() {
                if !matches!(job.join(), Ok(Ok(()))) {
                    three.doc.dirty = true;
                }
            }
        }
        let due = now - three.last_save > AUTOSAVE_SECS;
        if three.doc.dirty && three.saving.is_none() && three.stroke.is_none() && due {
            let snap = three.doc.clone();
            three.doc.dirty = false;
            three.last_save = now;
            three.saving = std::thread::Builder::new()
                .name("eskiz-3d-save".into())
                .spawn(move || save_model_doc(&lib, &snap).map(|_| ()))
                .ok();
        }
    }

    /// The 3D viewport: orbit, pan, zoom, paint, the overlay controls and the banner.
    pub(crate) fn three_d_frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let outer = ui.max_rect();
        // The model sits clear of the top bar and the left tool column, as the 2D canvas does.
        let vp = Rect::from_min_max(outer.min + Vec2::new(16.0, FIT_TOP), outer.max - Vec2::new(16.0, 16.0));
        let ppp = ctx.pixels_per_point();
        let input = self.read_input(ui);
        let now = input.time;
        let (secondary, sec_delta) =
            ctx.input(|i| (i.pointer.button_down(egui::PointerButton::Secondary), i.pointer.delta()));
        let pan = input.middle || (input.space && input.primary_down);
        let two_fingers = input.multi.filter(|m| m.num_touches >= 2);
        let pressed_on_view = input.press_origin.is_some_and(|p| vp.contains(p) && !over_ui(&ctx, p));
        let start = pressed_on_view && input.primary_pressed && !pan && !secondary && two_fingers.is_none();
        // Read the stroke settings before the 3D document is borrowed.
        let stroke_settings = start.then(|| self.stroke_settings());
        let Some(three) = self.three_d.as_mut() else { return };

        // Gestures: orbit with two fingers or right-drag, pan with middle or space drag, and
        // zoom with pinch or scroll over the view. The camera does not move during a stroke.
        if three.stroke.is_none() {
            let before = three.doc.camera;
            let hover_in = input.pointer.is_some_and(|p| vp.contains(p) && !over_ui(&ctx, p));
            if let Some(mt) = two_fingers {
                let d = mt.translation_delta;
                three.doc.camera.orbit(-d.x * ORBIT_SPEED, d.y * ORBIT_SPEED);
                three.doc.camera.zoom_by(mt.zoom_delta);
            } else if secondary && sec_delta != Vec2::ZERO {
                three.doc.camera.orbit(-sec_delta.x * ORBIT_SPEED, sec_delta.y * ORBIT_SPEED);
            } else if pan && input.pointer_delta != Vec2::ZERO {
                let d = input.pointer_delta;
                three.doc.camera.pan(d.x / vp.height(), d.y / vp.height());
            }
            if hover_in {
                if input.zoom != 1.0 {
                    three.doc.camera.zoom_by(input.zoom);
                }
                if input.scroll.y != 0.0 {
                    three.doc.camera.zoom_by((-input.scroll.y * 0.002).exp());
                }
            }
            if three.doc.camera != before {
                three.low_until = now + LOW_RES_HOLD;
            }
        }

        // The lookup is full resolution while a stroke starts or runs, and when the camera is idle.
        let full = start || three.stroke.is_some() || now >= three.low_until;
        let scale = if full { ppp } else { ppp * LOW_RES };
        let width = ((vp.width() * scale).round() as usize).max(1);
        let height = ((vp.height() * scale).round() as usize).max(1);
        let rebuilt = three.ensure_lookup(width, height);

        if let (Some(settings), Some(p)) = (stroke_settings, input.press_origin) {
            begin_stroke(three, &settings, ppp);
            let (x, y) = to_lookup_px(p, vp, ppp);
            feed_stroke(three, x, y, now);
        } else if three.stroke.is_some() && input.primary_down {
            if let Some(p) = input.pointer {
                let (x, y) = to_lookup_px(p, vp, ppp);
                feed_stroke(three, x, y, now);
            }
        }
        let releasing = three.stroke.is_some() && !input.primary_down;
        if releasing {
            self.finish_three_d_stroke();
        }
        let Some(three) = self.three_d.as_mut() else { return };

        if rebuilt || three.view_dirty {
            if let Some(image) = shade_view(three) {
                let size = image.size;
                match three.texture.as_mut() {
                    Some(tex) if tex.size() == size => tex.set(image, TextureOptions::LINEAR),
                    _ => three.texture = Some(ctx.load_texture("model-3d", image, TextureOptions::LINEAR)),
                }
            }
            three.view_dirty = false;
        }
        let moving = now < three.low_until || three.stroke.is_some();
        let painter = ui.painter_at(outer);
        painter.rect_filled(outer, 0.0, Color32::from_rgb(BACKDROP[0], BACKDROP[1], BACKDROP[2]));
        if let Some(tex) = three.texture.as_ref() {
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(tex.id(), vp, uv, Color32::WHITE);
        }
        if moving {
            ctx.request_repaint();
        } else if !full {
            ctx.request_repaint_after(Duration::from_secs_f64((three.low_until - now).max(0.02)));
        }

        self.poll_three_d_export();
        self.autosave_three_d(now);
        self.three_d_overlay(&ctx, vp);
    }

    /// The overlay controls (reset view, paint-through) and the warning banner.
    fn three_d_overlay(&mut self, ctx: &egui::Context, vp: Rect) {
        let mut reset = false;
        let mut dismiss = false;
        let Some(three) = self.three_d.as_mut() else { return };
        let mut through = three.doc.paint_through;
        let banner = three.banner.clone();
        egui::Area::new(Id::new("three-d-overlay"))
            .fixed_pos(vp.min + Vec2::new(48.0, 0.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_black_alpha(150))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            reset |= ui.button("Reset view").clicked();
                            crate::theme::checkbox(ui, &mut through, "Paint through");
                        });
                    });
                if let Some(text) = banner.as_ref() {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(120, 90, 20, 230))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.colored_label(Color32::WHITE, text);
                                dismiss |= ui.small_button("Dismiss").clicked();
                            });
                        });
                }
            });
        if through != three.doc.paint_through {
            three.doc.paint_through = through;
            three.view_dirty = true;
        }
        if dismiss {
            three.banner = None;
        }
        if reset {
            self.reset_three_d_view();
        }
    }

    /// The 3D tab of the Actions panel: lighting, previews, painting and the view.
    pub(crate) fn three_d_settings_body(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let Some(three) = self.three_d.as_mut() else {
            small_label(ui, &pal, "NO 3D DOCUMENT");
            ui.label(if cfg!(feature = "usd") {
                "Import a .glb, .gltf, .obj, .usdz, .usda or .usdc from the Gallery to paint in 3D."
            } else {
                "Import a .glb, .gltf or .obj from the Gallery to paint in 3D."
            });
            return;
        };
        let s = &mut three.settings;
        let mut changed = false;
        small_label(ui, &pal, "LIGHT");
        changed |= ui.add(egui::Slider::new(&mut s.light_yaw, -180.0..=180.0).text("Direction")).changed();
        changed |= ui.add(egui::Slider::new(&mut s.light_elevation, 0.0..=90.0).text("Elevation")).changed();
        changed |= ui.add(egui::Slider::new(&mut s.light_intensity, 0.0..=2.0).text("Intensity")).changed();
        changed |= ui.add(egui::Slider::new(&mut s.ambient, 0.0..=1.0).text("Ambient")).changed();
        small_label(ui, &pal, "PREVIEW");
        let mut ggx = s.shade == Shade::Ggx;
        if crate::theme::checkbox(ui, &mut ggx, "GGX shading (off = flat)").changed() {
            s.shade = if ggx { Shade::Ggx } else { Shade::Flat };
            changed = true;
        }
        changed |= crate::theme::checkbox(ui, &mut s.show_roughness, "Roughness").changed();
        changed |= crate::theme::checkbox(ui, &mut s.show_metallic, "Metallic").changed();
        small_label(ui, &pal, "PAINTING");
        let mut through = three.doc.paint_through;
        if crate::theme::checkbox(ui, &mut through, "Paint through (surfaces behind the front)").changed() {
            three.doc.paint_through = through;
            changed = true;
        }
        if changed {
            three.view_dirty = true;
        }
        if ui.button("Reset view").clicked() {
            self.reset_three_d_view();
        }
    }
}

// ---------------------------------------------------------------- self-test

/// `--selftest` check: import a generated cube, paint at the centre of the view and check that
/// the colour map changed at the UV the centre shows. Then check that the GLB export re-imports.
pub fn selftest() -> Result<String, String> {
    let model = load_gltf_slice(&cube_glb(true), &ImportOptions::default())
        .map_err(|e| format!("cube import: {e}"))?
        .model;
    let mut doc = ModelDoc::new("selftest".into(), "Cube".into(), model);
    doc.camera = doc.home;
    doc.camera.yaw = 0.0;
    doc.camera.pitch = 0.0;
    let size = 256;
    let lookup = render_lookup(&doc.model, &doc.camera, size, size, RenderOptions::default());
    let c = size as f32 / 2.0;
    let (u, v) = lookup.remap(c, c).ok_or("the center of the view misses the cube")?;
    let set = doc.model.texture_sets.first().ok_or("cube has no texture set")?;
    let (tw, th) = (set.width, set.height);
    let (tx, ty) = ((u * tw as f32) as usize, (v * th as f32) as usize);
    let before = set.map(MapKind::Colour).pixel(tx, ty);
    let brush = Brush { size: 40.0, ..Brush::default() };
    let points: Vec<(f64, f64)> = (0..21).map(|i| (f64::from(c) - 10.0 + f64::from(i), f64::from(c))).collect();
    let dabs = stroke_dabs(&brush, &points);
    // A hidden texture set takes no paint.
    let mut hidden = doc.clone();
    hidden.model.texture_sets[0].visible = false;
    let hidden_changes = paint_dabs(&mut hidden, &lookup, brush.clone(), [1.0, 0.0, 0.0], false, &dabs);
    if !hidden_changes.is_empty() || hidden.model.texture_sets[0].map(MapKind::Colour).pixel(tx, ty) != before {
        return Err("painting changed a hidden texture set".into());
    }
    let changes = paint_dabs(&mut doc, &lookup, brush, [1.0, 0.0, 0.0], false, &dabs);
    let after = doc.model.texture_sets[0].map(MapKind::Colour).pixel(tx, ty);
    if after == before || after[0] <= after[1] {
        return Err(format!("painting the center did not change texel ({tx}, {ty}): {before:?} -> {after:?}"));
    }
    // The stroke is one 3D undo step: undo puts the texel back and redo paints it again.
    let mut three = ThreeD::new(doc.clone(), None);
    three.push_stroke(changes);
    if !three.undo() || three.doc.model.texture_sets[0].map(MapKind::Colour).pixel(tx, ty) != before {
        return Err("3D undo did not restore the painted texel".into());
    }
    if !three.redo() || three.doc.model.texture_sets[0].map(MapKind::Colour).pixel(tx, ty) != after {
        return Err("3D redo did not paint the texel again".into());
    }
    // A 3D document copies and renames in the library, with its folder and title.
    let root = std::env::temp_dir().join(format!("eskiz-3d-selftest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let lib = Library::at(&root);
    let saved = save_model_doc(&lib, &doc)?;
    let copy = lib.duplicate(&saved.id)?;
    if !lib.is_model_doc(&copy.id) || copy.title != format!("{} copy", doc.title) {
        return Err(format!("3D duplicate gave {:?} / {:?}", copy.id, copy.title));
    }
    lib.rename(&copy.id, "Renamed cube")?;
    let reopened = load_model_doc(&lib, &copy.id)?;
    if reopened.title != "Renamed cube" {
        return Err(format!("3D rename did not reach the document: {:?}", reopened.title));
    }
    let _ = fs::remove_dir_all(&root);
    let glb = write_glb_bytes(&doc.model).map_err(|e| e.to_string())?;
    let back = load_gltf_slice(&glb, &ImportOptions::default()).map_err(|e| format!("GLB re-import: {e}"))?;
    Ok(format!(
        "  3D cube: center paints UV ({u:.2}, {v:.2}), texel ({tx}, {ty}) of {tw}x{th}; GLB re-imports {} triangles; stroke undo and redo, hidden set, gallery copy and rename: ok",
        back.model.triangle_count()
    ))
}

// ---------------------------------------------------------------- test fixtures

/// A cube of side 2 centred on the origin, as a GLB with one grey material. The six faces sit in
/// a 3 x 2 UV atlas with a margin, so no two faces overlap. With `with_uvs` false the texture
/// coordinates are left out, which is a fatal check.
pub fn cube_glb(with_uvs: bool) -> Vec<u8> {
    // (normal, a, b) per face, with a x b = normal so the corners run counter-clockwise outside.
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
    ];
    let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let mut positions: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();
    let mut uvs: Vec<f32> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    for (face, (n, a, b)) in faces.into_iter().enumerate() {
        let base = (positions.len() / 3) as u16;
        for (s1, s2) in corners {
            for k in 0..3 {
                positions.push(n[k] + a[k] * s1 + b[k] * s2);
            }
            normals.extend_from_slice(&n);
            let (cx, cy) = ((face % 3) as f32, (face / 3) as f32);
            uvs.extend_from_slice(&[(cx + 0.02 + 0.48 * (s1 + 1.0)) / 3.0, (cy + 0.02 + 0.48 * (s2 + 1.0)) / 2.0]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let mut bin: Vec<u8> = Vec::new();
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let f32_bytes = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let mut push_view = |bin: &mut Vec<u8>, bytes: &[u8], target: u32| -> usize {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len(), "target": target}));
        views.len() - 1
    };
    let pos_view = push_view(&mut bin, &f32_bytes(&positions), 34962);
    accessors.push(json!({"bufferView": pos_view, "componentType": 5126, "count": 24, "type": "VEC3",
        "min": [-1.0, -1.0, -1.0], "max": [1.0, 1.0, 1.0]}));
    let norm_view = push_view(&mut bin, &f32_bytes(&normals), 34962);
    accessors.push(json!({"bufferView": norm_view, "componentType": 5126, "count": 24, "type": "VEC3"}));
    let mut attributes = json!({"POSITION": 0, "NORMAL": 1});
    if with_uvs {
        let uv_view = push_view(&mut bin, &f32_bytes(&uvs), 34962);
        accessors.push(json!({"bufferView": uv_view, "componentType": 5126, "count": 24, "type": "VEC2"}));
        attributes["TEXCOORD_0"] = json!(2);
    }
    let idx_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let idx_view = push_view(&mut bin, &idx_bytes, 34963);
    accessors.push(json!({"bufferView": idx_view, "componentType": 5123, "count": 36, "type": "SCALAR"}));
    let index_accessor = accessors.len() - 1;
    let doc = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": attributes, "indices": index_accessor, "material": 0}]}],
        "materials": [{"pbrMetallicRoughness": {"baseColorFactor": [0.8, 0.8, 0.8, 1.0],
            "metallicFactor": 0.0, "roughnessFactor": 0.5}}],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": views,
        "accessors": accessors,
    });
    let mut json_bytes = doc.to_string().into_bytes();
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}
