//! Brush files: `.brush` bundles, `.brushset` bundles and Photoshop `.abr` brush presets.
//!
//! The reader output is the neutral [`ImportedBrush`]: a name, a map of named settings
//! ([`SettingValue`]), and optional shape and grain PNG bytes. eskiz-format does not depend on
//! eskiz-core. [`engine_params`] turns the settings into the brush-engine values.
//! and [`ImportedBrush::map`] lets the caller build any
//! other target.
//!
//! The container and key names are:
//! `Brush.archive` (keyed archive, root class `SilicaBrush`), `Shape.png`, `Grain.png` and
//! `QuickLook/Thumbnail.png` in each brush, and `brushset.plist` for a set. The entries for
//! the set manifest and the member names are unverified. The `brushset.plist` key
//! names used here (`name`, `brushes`) are a guess, see [`read_brushset`].
//!
//! ABR: layouts follow public descriptions of the
//! Photoshop descriptor format. The ABR v6 sample and preset-section layouts are not verified
//! against real files.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek};

use zip::ZipArchive;

use crate::keyedarchive::{self, ArchivedObject, NsObject, NsValue, Object};
use crate::limits::{self, MAX_BRUSHES, MAX_BRUSH_TIP_SIDE, MAX_DECODED_BYTES, MAX_DESCRIPTOR_DEPTH};
use crate::palettes::zip_members;
use crate::{Error, Result};

/// Setting names: the §6 `SilicaBrush` keys used by the importers and by [`engine_params`].
/// Keys that are not in the §6 list are marked as Eskiz-only.
pub mod keys {
    pub const NAME: &str = "name";
    pub const MAX_SIZE: &str = "maxSize";
    pub const MIN_SIZE: &str = "minSize";
    pub const PLOT_SPACING: &str = "plotSpacing";
    pub const PLOT_SMOOTHING: &str = "plotSmoothing";
    pub const SHAPE_ROTATION: &str = "shapeRotation";
    pub const SHAPE_ROUNDNESS: &str = "shapeRoundness";
    pub const TAPER_START_LENGTH: &str = "taperStartLength";
    pub const TAPER_END_LENGTH: &str = "taperEndLength";
    pub const TAPER_SIZE: &str = "taperSize";
    pub const TAPER_OPACITY: &str = "taperOpacity";
    pub const TAPER_SHAPE: &str = "taperShape";
    pub const TAPER_PRESSURE: &str = "taperPressure";
    pub const PENCIL_TAPER_START_LENGTH: &str = "pencilTaperStartLength";
    pub const PENCIL_TAPER_END_LENGTH: &str = "pencilTaperEndLength";
    pub const PENCIL_TAPER_SIZE: &str = "pencilTaperSize";
    pub const PENCIL_TAPER_OPACITY: &str = "pencilTaperOpacity";
    pub const PENCIL_TAPER_SHAPE: &str = "pencilTaperShape";
    pub const BUNDLED_SHAPE_PATH: &str = "bundledShapePath";
    pub const BUNDLED_GRAIN_PATH: &str = "bundledGrainPath";
    /// Eskiz-only: hardness of a computed tip (ABR `Hrdn`). Not in the §6 key list.
    pub const HARDNESS: &str = "hardness";
}

/// Root class of a brush archive.
const SILICA_CLASS: &str = "SilicaBrush";
/// Class of a response curve: key `points`, strings `"{x, y}"`.
const CURVE_CLASS: &str = "ValkyrieMagnitudinalCurve";
/// Class of roll parameters.
const ROLL_CLASS: &str = "ValkyrieBrushRollParameters";
/// Archive member holding the brush object.
const BRUSH_ARCHIVE: &str = "Brush.archive";
/// Embedded tip texture member.
const SHAPE_MEMBER: &str = "Shape.png";
/// Embedded grain texture member.
const GRAIN_MEMBER: &str = "Grain.png";
/// Set manifest member of a `.brushset` (§6, unverified there).
const SET_MANIFEST: &str = "brushset.plist";

/// Keys that are integers in brush files. They are written as integers so other readers accept them.
const INTEGER_KEYS: [&str; 21] = [
    "plotSpacingVersion",
    "blendMode",
    "burntEdgesBlendMode",
    "burntEdgesBlendModeExtended",
    "dualBlendMode",
    "dualBlendModeExtended",
    "extendedBlend",
    "extendedBlend2",
    "grainBlendMode",
    "grainBlendModeExtended",
    "grainOrientation",
    "hoverFill",
    "hoverOutline",
    "shapeFilterMode",
    "shapeOrientation",
    "shapeRollMode",
    "taperVersion",
    "textureApplication",
    "textureFilterMode",
    "textureOrientation",
    "version",
];

/// One value of a brush setting.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Number(f64),
    Bool(bool),
    Text(String),
    /// A response curve: control points in [0, 1] x [0, 1]. The identity is `[[0,0],[1,1]]`.
    Curve(Vec<[f64; 2]>),
    /// Roll parameters: activation falloff, origin and range (radians), and the integer mode.
    Roll {
        falloff: f64,
        origin: f64,
        range: f64,
        mode: i64,
    },
    /// A 16-byte colour, four float32 RGBA values (little-endian in the file).
    Color([f32; 4]),
    /// A numeric array, such as the saved per-tool sizes and opacities.
    Array(Vec<f64>),
}

/// A brush in neutral form, as read from a file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportedBrush {
    pub name: String,
    /// Named settings, sorted by name. Unknown settings are kept so they survive a re-export.
    pub settings: BTreeMap<String, SettingValue>,
    /// Tip PNG (`Shape.png`), or `None` for the default round tip.
    pub shape_png: Option<Vec<u8>>,
    /// Grain PNG (`Grain.png`), or `None` for no grain.
    pub grain_png: Option<Vec<u8>>,
    /// Import summary lines: dropped settings, approximations and missing textures.
    pub notes: Vec<String>,
}

impl ImportedBrush {
    pub fn number(&self, key: &str) -> Option<f64> {
        match self.settings.get(key) {
            Some(SettingValue::Number(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.settings.get(key) {
            Some(SettingValue::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        match self.settings.get(key) {
            Some(SettingValue::Text(t)) => Some(t),
            _ => None,
        }
    }

    /// Builds a target value from the settings. `assign` is called once per setting, in name
    /// order, and sets the target's fields.
    pub fn map<T: Default>(&self, mut assign: impl FnMut(&mut T, &str, &SettingValue)) -> T {
        let mut target = T::default();
        for (key, value) in &self.settings {
            assign(&mut target, key, value);
        }
        target
    }
}

// ------------------------------------------------------------------ engine values

/// One taper, with the engine defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct TaperParams {
    pub start_length: f64,
    pub end_length: f64,
    pub size: f64,
    pub opacity: f64,
    pub shape: f64,
    pub pressure: f64,
}

impl Default for TaperParams {
    fn default() -> Self {
        Self {
            start_length: 0.1,
            end_length: 0.1,
            size: 1.0,
            opacity: 0.0,
            shape: 0.0,
            pressure: 1.0,
        }
    }
}

/// Brush-engine values read from a brush's settings. Missing keys keep the §8 defaults, since
/// older brushes omit newer keys.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineParams {
    /// Base size (`maxSize`). Units follow the file; px is assumed, unverified.
    pub size: f64,
    /// Lower size clamp as a fraction of `size` (`minSize / maxSize`).
    pub min_size: f64,
    /// Spacing as a fraction of the diameter (`plotSpacing`).
    pub spacing: f64,
    /// Smoothing smoothing in [0, 1] (`plotSmoothing`).
    pub smoothing: f64,
    /// Finger taper (`taper*`).
    pub taper: TaperParams,
    /// Pencil taper (`pencilTaper*`). The file has no pencil pressure key, so pressure is 1.
    pub pencil_taper: TaperParams,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            size: 20.0,
            min_size: 0.0,
            spacing: 0.1,
            smoothing: 0.3,
            taper: TaperParams::default(),
            pencil_taper: TaperParams::default(),
        }
    }
}

/// Reads the engine values from a brush's settings, as the file format notes
/// map them onto the engine.
pub fn engine_params(brush: &ImportedBrush) -> EngineParams {
    let defaults = EngineParams::default();
    let max = brush.number(keys::MAX_SIZE);
    let min_size = match (brush.number(keys::MIN_SIZE), max) {
        (Some(min), Some(max)) if max > 0.0 => min / max,
        _ => defaults.min_size,
    };
    EngineParams {
        size: max.unwrap_or(defaults.size),
        min_size,
        spacing: brush.number(keys::PLOT_SPACING).unwrap_or(defaults.spacing),
        smoothing: brush
            .number(keys::PLOT_SMOOTHING)
            .unwrap_or(defaults.smoothing),
        taper: taper_from(
            brush,
            [
                keys::TAPER_START_LENGTH,
                keys::TAPER_END_LENGTH,
                keys::TAPER_SIZE,
                keys::TAPER_OPACITY,
                keys::TAPER_SHAPE,
            ],
            Some(keys::TAPER_PRESSURE),
        ),
        pencil_taper: taper_from(
            brush,
            [
                keys::PENCIL_TAPER_START_LENGTH,
                keys::PENCIL_TAPER_END_LENGTH,
                keys::PENCIL_TAPER_SIZE,
                keys::PENCIL_TAPER_OPACITY,
                keys::PENCIL_TAPER_SHAPE,
            ],
            None,
        ),
    }
}

/// Reads one taper. `names` are start length, end length, size, opacity and shape.
fn taper_from(brush: &ImportedBrush, names: [&str; 5], pressure: Option<&str>) -> TaperParams {
    let d = TaperParams::default();
    let get = |key: &str, fallback: f64| brush.number(key).unwrap_or(fallback);
    TaperParams {
        start_length: get(names[0], d.start_length),
        end_length: get(names[1], d.end_length),
        size: get(names[2], d.size),
        opacity: get(names[3], d.opacity),
        shape: get(names[4], d.shape),
        pressure: pressure.map_or(d.pressure, |key| get(key, d.pressure)),
    }
}

// ------------------------------------------------------------------ zip helpers

fn read_member<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<Option<Vec<u8>>> {
    limits::read_entry(zip, name, limits::MAX_ENTRY_BYTES)
}

fn zip_owned(members: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let refs: Vec<(&str, &[u8])> = members
        .iter()
        .map(|(name, data)| (name.as_str(), data.as_slice()))
        .collect();
    zip_members(&refs)
}

// ------------------------------------------------------------------ setting conversion

fn color_from_bytes(b: &[u8; 16]) -> [f32; 4] {
    std::array::from_fn(|i| {
        f32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]])
    })
}

fn number_of(value: &Object) -> Option<f64> {
    match value {
        Object::Real(v) => Some(*v),
        Object::Int(v) => Some(*v as f64),
        _ => None,
    }
}

fn int_of(value: Option<&Object>) -> Option<i64> {
    match value? {
        Object::Int(v) => Some(*v),
        Object::Real(v) if v.fract() == 0.0 => Some(*v as i64),
        _ => None,
    }
}

/// Parses one `"{x, y}"` point.
fn parse_point(text: &str) -> Option<[f64; 2]> {
    let inner = text.trim().strip_prefix('{')?.strip_suffix('}')?;
    let mut parts = inner.split(',');
    let x = parts.next()?.trim().parse().ok()?;
    let y = parts.next()?.trim().parse().ok()?;
    parts.next().is_none().then_some([x, y])
}

fn setting_from_object(key: &str, value: &Object, notes: &mut Vec<String>) -> Option<SettingValue> {
    match value {
        Object::Null => None,
        Object::Real(v) => Some(SettingValue::Number(*v)),
        Object::Int(v) => Some(SettingValue::Number(*v as f64)),
        Object::Bool(v) => Some(SettingValue::Bool(*v)),
        Object::String(s) => Some(SettingValue::Text(s.clone())),
        Object::Data(d) => match <&[u8; 16]>::try_from(d.as_slice()) {
            Ok(bytes) => Some(SettingValue::Color(color_from_bytes(bytes))),
            Err(_) => {
                notes.push(format!(
                    "setting {key} is {} bytes of data; it is dropped",
                    d.len()
                ));
                None
            }
        },
        Object::Array(items) => {
            let numbers: Option<Vec<f64>> = items.iter().map(number_of).collect();
            match numbers {
                Some(v) => Some(SettingValue::Array(v)),
                None => {
                    notes.push(format!(
                        "setting {key} is not a number array; it is dropped"
                    ));
                    None
                }
            }
        }
        Object::Archived(object) => archived_setting(key, object, notes),
        _ => {
            notes.push(format!(
                "setting {key} has a type that is not kept; it is dropped"
            ));
            None
        }
    }
}

fn archived_setting(
    key: &str,
    object: &ArchivedObject,
    notes: &mut Vec<String>,
) -> Option<SettingValue> {
    match object.class_name.as_str() {
        CURVE_CLASS => {
            let points: Option<Vec<[f64; 2]>> = match object.get("points") {
                Some(Object::Array(items)) => items
                    .iter()
                    .map(|item| match item {
                        Object::String(s) => parse_point(s),
                        _ => None,
                    })
                    .collect(),
                _ => None,
            };
            match points {
                Some(p) => Some(SettingValue::Curve(p)),
                None => {
                    notes.push(format!("curve {key} has malformed points; it is dropped"));
                    None
                }
            }
        }
        ROLL_CLASS => {
            let real = |k: &str| object.get(k).and_then(number_of);
            match (
                real("activationFalloff"),
                real("activationOrigin"),
                real("activationRange"),
                int_of(object.get("mode")),
            ) {
                (Some(falloff), Some(origin), Some(range), Some(mode)) => {
                    Some(SettingValue::Roll {
                        falloff,
                        origin,
                        range,
                        mode,
                    })
                }
                _ => {
                    notes.push(format!(
                        "roll parameters {key} are incomplete; they are dropped"
                    ));
                    None
                }
            }
        }
        other => {
            notes.push(format!("setting {key} is a {other} object; it is not kept"));
            None
        }
    }
}

/// The archive value of one setting. Keys listed in [`INTEGER_KEYS`] are written as integers.
fn object_for(key: &str, value: &SettingValue) -> NsValue {
    match value {
        SettingValue::Number(n) if INTEGER_KEYS.contains(&key) && n.fract() == 0.0 => {
            NsValue::Int(*n as i64)
        }
        SettingValue::Number(n) => NsValue::Real(*n),
        SettingValue::Bool(b) => NsValue::Bool(*b),
        SettingValue::Text(t) => NsValue::String(t.clone()),
        SettingValue::Curve(points) => NsValue::Object(
            NsObject::new(CURVE_CLASS).field(
                "points",
                NsValue::Array {
                    items: points
                        .iter()
                        .map(|[x, y]| NsValue::String(format!("{{{x}, {y}}}")))
                        .collect(),
                    mutable: false,
                },
            ),
        ),
        SettingValue::Roll {
            falloff,
            origin,
            range,
            mode,
        } => NsValue::Object(
            NsObject::new(ROLL_CLASS)
                .field("activationFalloff", NsValue::Real(*falloff))
                .field("activationOrigin", NsValue::Real(*origin))
                .field("activationRange", NsValue::Real(*range))
                .field("mode", NsValue::Int(*mode)),
        ),
        SettingValue::Color(c) => NsValue::Data(c.iter().flat_map(|v| v.to_le_bytes()).collect()),
        SettingValue::Array(values) => NsValue::Array {
            items: values.iter().map(|v| NsValue::Real(*v)).collect(),
            mutable: false,
        },
    }
}

// ------------------------------------------------------------------ .brush

/// Reads the brush whose members sit under `folder` (`""` for a `.brush`, `"Name/"` inside a
/// `.brushset`).
fn read_brush_at<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    folder: &str,
    name_hint: &str,
) -> Result<ImportedBrush> {
    let archive = read_member(zip, &format!("{folder}{BRUSH_ARCHIVE}"))?
        .ok_or_else(|| Error::Format(format!("no {BRUSH_ARCHIVE} in this brush")))?;
    let Object::Archived(object) = keyedarchive::unarchive(&archive)? else {
        return Err(Error::Format("Brush.archive root is not an object".into()));
    };

    let mut brush = ImportedBrush::default();
    for (key, value) in &object.fields {
        if key.as_str() == keys::NAME {
            if let Object::String(s) = value {
                brush.name = s.clone();
                continue;
            }
        }
        if let Some(setting) = setting_from_object(key, value, &mut brush.notes) {
            brush.settings.insert(key.clone(), setting);
        }
    }
    if brush.name.is_empty() {
        brush.name = name_hint.to_string();
    }

    brush.shape_png = read_member(zip, &format!("{folder}{SHAPE_MEMBER}"))?;
    brush.grain_png = read_member(zip, &format!("{folder}{GRAIN_MEMBER}"))?;
    // Built-in tips are stored by path instead of embedding them. This reader does
    // not ship those assets, so the default tip is used and the summary says so.
    if brush.shape_png.is_none() {
        if let Some(path) = brush.text(keys::BUNDLED_SHAPE_PATH).map(str::to_owned) {
            brush.notes.push(format!(
                "built-in tip {path:?} is not in this bundle; the default round tip is used"
            ));
        }
    }
    if brush.grain_png.is_none() {
        if let Some(path) = brush.text(keys::BUNDLED_GRAIN_PATH).map(str::to_owned) {
            brush.notes.push(format!(
                "built-in grain {path:?} is not in this bundle; no grain is used"
            ));
        }
    }
    Ok(brush)
}

/// Reads a `.brush` bundle: a ZIP holding `Brush.archive`, and optionally `Shape.png` and
/// `Grain.png`. A missing `Brush.archive` is an error. A brush without a `name` setting takes
/// `name_hint`.
pub fn read_brush(name_hint: &str, bytes: &[u8]) -> Result<ImportedBrush> {
    let mut zip = limits::open_zip(bytes)?;
    read_brush_at(&mut zip, "", name_hint)
}

/// The members of one brush under `folder`: `Brush.archive`, then the textures it has.
fn brush_members(brush: &ImportedBrush, folder: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let mut fields: BTreeMap<String, NsValue> = brush
        .settings
        .iter()
        .map(|(key, value)| (key.clone(), object_for(key, value)))
        .collect();
    fields.insert(keys::NAME.into(), NsValue::String(brush.name.clone()));

    let mut object = NsObject::new(SILICA_CLASS);
    object.fields = fields.into_iter().collect();
    let archive = keyedarchive::write(&NsValue::Object(object))?;

    let mut members = vec![(format!("{folder}{BRUSH_ARCHIVE}"), archive)];
    if let Some(png) = &brush.shape_png {
        members.push((format!("{folder}{SHAPE_MEMBER}"), png.clone()));
    }
    if let Some(png) = &brush.grain_png {
        members.push((format!("{folder}{GRAIN_MEMBER}"), png.clone()));
    }
    Ok(members)
}

/// Writes a `.brush` bundle: `Brush.archive`, plus `Shape.png` and `Grain.png` when present.
/// A `read_brush` of the output gives back the name, settings and textures. The thumbnail
/// `QuickLook/Thumbnail.png` is not written; the library then shows its placeholder.
pub fn write_brush(brush: &ImportedBrush) -> Result<Vec<u8>> {
    zip_owned(&brush_members(brush, "")?)
}

// ------------------------------------------------------------------ .brushset

/// The result of reading a `.brushset`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrushSet {
    /// The manifest name when the set has one, otherwise `name_hint`.
    pub name: String,
    /// Brushes in manifest order. Brushes the manifest does not list follow in archive order.
    pub brushes: Vec<ImportedBrush>,
    /// One line per brush folder that failed, naming the folder and the error.
    pub failures: Vec<String>,
}

/// What a `brushset.plist` supplies: the set name and the brush order.
#[derive(Debug, Default)]
struct Manifest {
    name: Option<String>,
    brushes: Vec<String>,
}

fn parse_manifest(data: &[u8]) -> Option<Manifest> {
    let value = limits::parse_plist(data).ok()?;
    let dict = value.as_dictionary()?;
    let name = dict
        .get("name")
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let brushes = dict
        .get("brushes")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_string().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Some(Manifest { name, brushes })
}

/// Reads a `.brushset`: a ZIP of brush folders. Each folder `Name/` holds one brush's members
/// (`Name/Brush.archive`, and textures). A root-level `Brush.archive` also counts as a brush.
/// Other members are ignored. A folder that fails is listed in `failures`, and the rest still
/// import.
///
/// The manifest `brushset.plist` is read as a property list with an optional `name` string and an
/// optional `brushes` array of folder names giving the order. The key names are a guess
/// leaves the manifest unverified); a set without a manifest uses `name_hint` and archive order.
pub fn read_brushset(name_hint: &str, bytes: &[u8]) -> Result<BrushSet> {
    let mut zip = limits::open_zip(bytes)?;
    let names: Vec<String> = zip
        .file_names()
        .filter_map(|n| n.ok().map(|s| s.into_owned()))
        .collect();

    // Folder prefixes: "" for a root brush, "Name/" for each folder brush.
    let mut prefixes: Vec<String> = names
        .iter()
        .filter_map(|n| n.strip_suffix(BRUSH_ARCHIVE))
        .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))
        .map(str::to_owned)
        .collect();
    if prefixes.len() > MAX_BRUSHES {
        return Err(limits::exceeded(format!(
            "the set has {} brushes; the most Eskiz imports at once is {MAX_BRUSHES}",
            prefixes.len()
        )));
    }

    let manifest = match read_member(&mut zip, SET_MANIFEST)? {
        Some(data) => parse_manifest(&data).unwrap_or_default(),
        None => Manifest::default(),
    };
    let label_of = |prefix: &str| prefix.trim_end_matches('/').to_string();
    // the first position of each name in the manifest's order
    let mut order: HashMap<&str, usize> = HashMap::new();
    for (i, b) in manifest.brushes.iter().enumerate() {
        order.entry(b.as_str()).or_insert(i);
    }
    prefixes.sort_by_cached_key(|p| order.get(label_of(p).as_str()).copied().unwrap_or(usize::MAX));

    let mut set = BrushSet::default();
    for prefix in &prefixes {
        let label = label_of(prefix);
        let label = if label.is_empty() {
            name_hint.to_string()
        } else {
            label
        };
        match read_brush_at(&mut zip, prefix, &label) {
            Ok(brush) => set.brushes.push(brush),
            Err(e) => set.failures.push(format!("{prefix}{BRUSH_ARCHIVE}: {e}")),
        }
    }
    if set.brushes.is_empty() && set.failures.is_empty() {
        return Err(Error::InvalidInput("no brushes found in this file".into()));
    }
    set.name = manifest.name.unwrap_or_else(|| name_hint.to_string());
    Ok(set)
}

/// A folder name for a brush: its name with path separators and control characters replaced,
/// made unique among `used` by a numeric suffix.
fn unique_folder(used: &mut Vec<String>, name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let base = match cleaned.trim() {
        "" => "Brush".to_string(),
        s => s.to_string(),
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while used.contains(&candidate) {
        candidate = format!("{base} {n}");
        n += 1;
    }
    used.push(candidate.clone());
    candidate
}

/// Writes a `.brushset` with one folder per brush and a `brushset.plist` naming the set and
/// giving the brush order.
pub fn write_brushset(name: &str, brushes: &[ImportedBrush]) -> Result<Vec<u8>> {
    let mut folders: Vec<String> = Vec::new();
    let mut members: Vec<(String, Vec<u8>)> = Vec::new();
    for brush in brushes {
        let folder = unique_folder(&mut folders, &brush.name);
        members.extend(brush_members(brush, &format!("{folder}/"))?);
    }

    let mut dict = plist::Dictionary::new();
    dict.insert("name".into(), plist::Value::String(name.to_string()));
    dict.insert(
        "brushes".into(),
        plist::Value::Array(folders.into_iter().map(plist::Value::String).collect()),
    );
    let mut manifest = Vec::new();
    plist::Value::Dictionary(dict)
        .to_writer_xml(&mut manifest)
        .map_err(|e| Error::Format(e.to_string()))?;
    members.push((SET_MANIFEST.into(), manifest));
    zip_owned(&members)
}

// ------------------------------------------------------------------ ABR

/// The result of reading an ABR file.
#[derive(Debug, Clone, PartialEq)]
pub struct AbrImport {
    pub brushes: Vec<ImportedBrush>,
    /// Presets that were skipped, with the reason.
    pub warnings: Vec<String>,
}

/// Descriptor keys used for the basic preset fields. These are Photoshop's four-character codes.
mod abr_key {
    pub const NAME: &str = "Nm  ";
    pub const DIAMETER: &str = "Dmtr";
    pub const ANGLE: &str = "Angl";
    pub const ROUNDNESS: &str = "Rndn";
    pub const HARDNESS: &str = "Hrdn";
    pub const SPACING: &str = "Spcn";
    pub const MAPPED: [&str; 6] = [NAME, DIAMETER, ANGLE, ROUNDNESS, HARDNESS, SPACING];
}

/// Reads a Photoshop ABR file, version 6 and later.
///
/// Layout: a 16-bit version and a 16-bit sub-version, then `8BIM` sections, each a four-byte
/// key, a 32-bit length, and the data (padded to even). The `samp` section holds sampled tips.
/// The `desc` section holds a 32-bit version and then one descriptor per preset.
///
/// Versions 1 and 2 are rejected with "this brush file version is not supported",
/// Eskiz's choice). Sampled tips are linked to presets by a text value equal to the sample's id.
/// Sample values are inverted, so black in the file is paint in Eskiz. The preset fields map to
/// the §6 names as describes: `Dmtr` to `maxSize`, `Spcn` (percent) to `plotSpacing`,
/// `Angl` to `shapeRotation` (degrees), `Rndn` (percent) to `shapeRoundness`, and `Hrdn` to the
/// Eskiz-only `hardness`. Presets with neither a sampled tip nor a diameter are skipped with a
/// warning. Dual brushes are not imported in this version.
pub fn read_abr(bytes: &[u8]) -> Result<AbrImport> {
    let mut r = Reader::new(bytes);
    let version = r.u16()?;
    let _sub_version = r.u16()?;
    if version < 6 {
        return Err(Error::Unsupported(
            "this brush file version is not supported".into(),
        ));
    }

    let mut samples = Vec::new();
    let mut presets = Vec::new();
    let mut warnings = Vec::new();
    while r.remaining() > 0 {
        if r.take(4)? != b"8BIM" {
            return Err(Error::Format("expected an 8BIM section".into()));
        }
        let key = r.take(4)?.to_vec();
        let len = r.u32()? as usize;
        let data = r.take(len)?;
        if len % 2 == 1 && r.remaining() > 0 {
            r.take(1)?;
        }
        match key.as_slice() {
            b"samp" => samples.extend(parse_samples(data, &mut warnings)?),
            b"desc" => presets.extend(parse_presets(data)?),
            _ => {}
        }
        if samples.len() > MAX_BRUSHES || presets.len() > MAX_BRUSHES {
            return Err(limits::exceeded(format!(
                "the file has more than {MAX_BRUSHES} brushes; the most Eskiz imports at once is {MAX_BRUSHES}"
            )));
        }
        let tip_bytes: u64 = samples.iter().map(|s| s.gray.len() as u64).sum();
        if tip_bytes > MAX_DECODED_BYTES {
            return Err(limits::exceeded(format!("the brush tips decode to more than {MAX_DECODED_BYTES} bytes")));
        }
    }

    let mut brushes = Vec::new();
    // each sampled tip is encoded once, however many presets use it
    let mut pngs: HashMap<usize, std::result::Result<Vec<u8>, String>> = HashMap::new();
    for (index, desc) in presets.iter().enumerate() {
        if let Some(brush) = preset_brush(desc, index, &samples, &mut pngs, &mut warnings) {
            brushes.push(brush);
        }
    }
    if brushes.is_empty() {
        return Err(Error::InvalidInput("no brushes found in this file".into()));
    }
    Ok(AbrImport { brushes, warnings })
}

/// A sampled tip: 8-bit gray, where 0 is paint in the file.
#[derive(Debug, Clone)]
struct Sample {
    id: String,
    width: usize,
    height: usize,
    gray: Vec<u8>,
}

/// Each sample entry: a 32-bit length (after the length field), a null-terminated id, bounds as
/// four 32-bit values (top, left, bottom, right), a 16-bit depth (8 or 16), an 8-bit compression
/// code (0 raw, 1 PackBits per row with a 16-bit byte count per row), then the pixels.
fn parse_samples(data: &[u8], warnings: &mut Vec<String>) -> Result<Vec<Sample>> {
    let mut r = Reader::new(data);
    let mut samples = Vec::new();
    while r.remaining() > 0 {
        let len = r.u32()? as usize;
        let entry = r.take(len)?;
        match parse_sample(entry) {
            Ok(sample) => samples.push(sample),
            Err(e) => warnings.push(format!("sample skipped: {e}")),
        }
    }
    Ok(samples)
}

fn parse_sample(entry: &[u8]) -> Result<Sample> {
    let mut r = Reader::new(entry);
    let id = r.cstring()?;
    let top = r.i32()?;
    let left = r.i32()?;
    let bottom = r.i32()?;
    let right = r.i32()?;
    let depth = r.u16()?;
    let compression = r.u8()?;
    // i64: the difference of two i32 bounds can overflow i32
    let (w, h) = (i64::from(right) - i64::from(left), i64::from(bottom) - i64::from(top));
    if w <= 0 || h <= 0 {
        return Err(Error::Format(format!("sample {id:?} has invalid bounds")));
    }
    if w > i64::from(MAX_BRUSH_TIP_SIDE) || h > i64::from(MAX_BRUSH_TIP_SIDE) {
        return Err(limits::exceeded(format!(
            "sample {id:?} is {w}x{h}; the largest tip Eskiz imports is {MAX_BRUSH_TIP_SIDE} pixels"
        )));
    }
    let (width, height) = (w as usize, h as usize);
    let bytes_per_pixel = match depth {
        8 => 1,
        16 => 2,
        other => return Err(Error::Unsupported(format!("sample depth {other}"))),
    };
    let row_bytes = width * bytes_per_pixel;
    let packed = match compression {
        0 => r.take(row_bytes * height)?.to_vec(),
        1 => {
            let counts: Vec<usize> = (0..height)
                .map(|_| r.u16().map(usize::from))
                .collect::<Result<_>>()?;
            // reserve no more than the remaining input can expand to (PackBits: 64 bytes per byte)
            let mut out = Vec::with_capacity((row_bytes * height).min(r.remaining().saturating_mul(64)));
            for count in counts {
                out.extend(packbits_decode(r.take(count)?, row_bytes)?);
            }
            out
        }
        other => return Err(Error::Unsupported(format!("sample compression {other}"))),
    };
    let gray = if bytes_per_pixel == 1 {
        packed
    } else {
        // 16-bit samples are big-endian, so the first byte of each pair is the high byte.
        packed.iter().step_by(2).copied().collect()
    };
    Ok(Sample {
        id,
        width,
        height,
        gray,
    })
}

/// PackBits: a signed count byte. Non-negative `n` copies the next `n + 1` bytes. Negative `n`
/// (except -128) repeats the next byte `1 - n` times.
fn packbits_decode(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    let mut i = 0;
    while i < src.len() {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = n as usize + 1;
            let chunk = src
                .get(i..i + len)
                .ok_or_else(|| Error::Format("truncated PackBits row".into()))?;
            out.extend_from_slice(chunk);
            i += len;
        } else if n != -128 {
            let value = *src
                .get(i)
                .ok_or_else(|| Error::Format("truncated PackBits row".into()))?;
            i += 1;
            out.extend(std::iter::repeat_n(value, (1 - i32::from(n)) as usize));
        }
    }
    if out.len() != expected {
        return Err(Error::Format(format!(
            "PackBits row decodes to {} bytes, expected {expected}",
            out.len()
        )));
    }
    Ok(out)
}

fn parse_presets(data: &[u8]) -> Result<Vec<Desc>> {
    let mut r = Reader::new(data);
    let _version = r.u32()?;
    let mut presets = Vec::new();
    while r.remaining() > 0 {
        presets.push(r.descriptor()?);
    }
    Ok(presets)
}

fn preset_brush(
    desc: &Desc,
    index: usize,
    samples: &[Sample],
    pngs: &mut HashMap<usize, std::result::Result<Vec<u8>, String>>,
    warnings: &mut Vec<String>,
) -> Option<ImportedBrush> {
    let name = match find(desc, abr_key::NAME) {
        Some(Value::Text(t)) if !t.is_empty() => t.clone(),
        _ => format!("Brush {}", index + 1),
    };
    let mut texts = Vec::new();
    collect_text(desc, &mut texts);
    let sample = samples.iter().position(|s| texts.contains(&s.id));
    let diameter = find(desc, abr_key::DIAMETER).and_then(pixels);
    if sample.is_none() && diameter.is_none() {
        warnings.push(format!("preset {name:?} has no usable tip; skipped"));
        return None;
    }

    let mut brush = ImportedBrush {
        name: name.clone(),
        ..ImportedBrush::default()
    };
    set_number(&mut brush, keys::MAX_SIZE, diameter);
    set_number(
        &mut brush,
        keys::PLOT_SPACING,
        find(desc, abr_key::SPACING).and_then(percent),
    );
    set_number(
        &mut brush,
        keys::SHAPE_ROTATION,
        find(desc, abr_key::ANGLE).and_then(degrees),
    );
    set_number(
        &mut brush,
        keys::SHAPE_ROUNDNESS,
        find(desc, abr_key::ROUNDNESS).and_then(percent),
    );
    match sample {
        Some(sample) => {
            let png = pngs.entry(sample).or_insert_with(|| {
                let sample = &samples[sample];
                let inverted: Vec<u8> = sample.gray.iter().map(|v| 255 - v).collect();
                encode_gray_png(sample.width, sample.height, &inverted).map_err(|e| e.to_string())
            });
            match png {
                Ok(png) => brush.shape_png = Some(png.clone()),
                Err(e) => {
                    warnings.push(format!("preset {name:?}: sampled tip not encoded ({e})"));
                    return None;
                }
            }
            if find(desc, abr_key::HARDNESS).is_some() {
                brush
                    .notes
                    .push("hardness is ignored for a sampled tip".into());
            }
        }
        None => set_number(
            &mut brush,
            keys::HARDNESS,
            find(desc, abr_key::HARDNESS).and_then(percent),
        ),
    }
    for (key, value) in &desc.items {
        if !abr_key::MAPPED.contains(&key.as_str()) && !matches!(value, Value::Text(_)) {
            brush
                .notes
                .push(format!("{key} is not imported in this version"));
        }
    }
    Some(brush)
}

fn set_number(brush: &mut ImportedBrush, key: &str, value: Option<f64>) {
    if let Some(v) = value {
        brush
            .settings
            .insert(key.to_string(), SettingValue::Number(v));
    }
}

fn pixels(v: &Value) -> Option<f64> {
    match v {
        Value::Unit(u, x) if u.as_str() == "#Pxl" => Some(*x),
        Value::Double(x) => Some(*x),
        _ => None,
    }
}

fn degrees(v: &Value) -> Option<f64> {
    match v {
        Value::Unit(u, x) if u.as_str() == "#Ang" => Some(*x),
        Value::Double(x) => Some(*x),
        _ => None,
    }
}

/// A percentage, returned as a fraction (50 % gives 0.5).
fn percent(v: &Value) -> Option<f64> {
    match v {
        Value::Unit(u, x) if u.as_str() == "#Prc" => Some(x / 100.0),
        Value::Double(x) => Some(x / 100.0),
        _ => None,
    }
}

/// Finds a key in a descriptor: its own items first, then nested descriptors depth first.
fn find<'a>(desc: &'a Desc, key: &str) -> Option<&'a Value> {
    if let Some((_, v)) = desc.items.iter().find(|(k, _)| k == key) {
        return Some(v);
    }
    desc.items.iter().find_map(|(_, v)| match v {
        Value::Desc(child) => find(child, key),
        _ => None,
    })
}

fn collect_text(desc: &Desc, out: &mut Vec<String>) {
    for (_, v) in &desc.items {
        collect_value_text(v, out);
    }
}

fn collect_value_text(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Text(t) => out.push(t.clone()),
        Value::Desc(d) => collect_text(d, out),
        Value::List(items) => items.iter().for_each(|i| collect_value_text(i, out)),
        _ => {}
    }
}

fn encode_gray_png(width: usize, height: usize, data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| Error::Encode(e.to_string()))?;
        writer
            .write_image_data(data)
            .map_err(|e| Error::Encode(e.to_string()))?;
        writer.finish().map_err(|e| Error::Encode(e.to_string()))?;
    }
    Ok(out)
}

// ------------------------------------------------------------------ descriptors

/// A decoded Photoshop action descriptor value. Every type is decoded so that a whole descriptor
/// can be walked, even when the importer reads only some of them.
#[allow(dead_code)]
#[derive(Debug, Clone)]
enum Value {
    Double(f64),
    /// A unit value: the four-character unit code (for example `#Pxl`) and the number.
    Unit(String, f64),
    Int(i64),
    Bool(bool),
    Text(String),
    Enum(String),
    Raw(Vec<u8>),
    Desc(Desc),
    List(Vec<Value>),
}

/// A descriptor: its items in order.
#[derive(Debug, Clone)]
struct Desc {
    items: Vec<(String, Value)>,
}

/// A bounds-checked big-endian cursor.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Nesting of descriptors and lists being read.
    depth: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, depth: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::Format("brush data is truncated".into()));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_be_bytes(self.array()?))
    }

    /// A null-terminated ASCII string. The terminator is consumed.
    fn cstring(&mut self) -> Result<String> {
        let rest = &self.data[self.pos..];
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::Format("unterminated sample id".into()))?;
        let text = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Ok(text)
    }

    /// A descriptor Unicode string: a 32-bit count of UTF-16 units, then the units.
    fn unicode(&mut self) -> Result<String> {
        let count = self.u32()? as usize;
        let bytes = count.checked_mul(2).ok_or_else(|| Error::Format("brush data is truncated".into()))?;
        let units: Vec<u16> = self
            .take(bytes)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        Ok(String::from_utf16_lossy(&units)
            .trim_end_matches('\0')
            .to_string())
    }

    /// A class or key identifier: a 32-bit length, where zero means a four-byte code follows.
    fn id(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let len = if len == 0 { 4 } else { len };
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    fn descriptor(&mut self) -> Result<Desc> {
        let _name = self.unicode()?;
        let _class = self.id()?;
        let count = self.u32()?;
        let mut items = Vec::new();
        for _ in 0..count {
            let key = self.id()?;
            let kind = self.array::<4>()?;
            let value = self.value(&kind)?;
            items.push((key, value));
        }
        Ok(Desc { items })
    }

    /// Runs `read` one nesting level deeper, failing past [`MAX_DESCRIPTOR_DEPTH`].
    fn nested<T>(&mut self, read: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.depth >= MAX_DESCRIPTOR_DEPTH {
            return Err(limits::exceeded(format!(
                "brush descriptors nest more than {MAX_DESCRIPTOR_DEPTH} levels deep"
            )));
        }
        self.depth += 1;
        let result = read(self);
        self.depth -= 1;
        result
    }

    fn value(&mut self, kind: &[u8; 4]) -> Result<Value> {
        Ok(match kind {
            b"Objc" => Value::Desc(self.nested(Self::descriptor)?),
            b"VlLs" => self.nested(|r| {
                let count = r.u32()?;
                let mut items = Vec::new();
                for _ in 0..count {
                    let item_kind = r.array::<4>()?;
                    items.push(r.value(&item_kind)?);
                }
                Ok(Value::List(items))
            })?,
            b"doub" => Value::Double(self.f64()?),
            b"UntF" => {
                let unit = String::from_utf8_lossy(&self.array::<4>()?).into_owned();
                Value::Unit(unit, self.f64()?)
            }
            b"long" => Value::Int(i64::from(self.i32()?)),
            b"comp" => Value::Int(self.i64()?),
            b"bool" => Value::Bool(self.u8()? != 0),
            b"TEXT" => Value::Text(self.unicode()?),
            b"enum" => {
                let _class = self.id()?;
                let _enum_type = self.id()?;
                Value::Enum(self.id()?)
            }
            b"tdta" | b"alis" => {
                let len = self.u32()? as usize;
                Value::Raw(self.take(len)?.to_vec())
            }
            other => {
                return Err(Error::Format(format!(
                    "unsupported descriptor type {:?}",
                    String::from_utf8_lossy(other)
                )))
            }
        })
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn push_u16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_be_bytes());
    }

    fn push_u32(out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&v.to_be_bytes());
    }

    fn push_i32(out: &mut Vec<u8>, v: i32) {
        out.extend_from_slice(&v.to_be_bytes());
    }

    fn push_unicode(out: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        push_u32(out, units.len() as u32);
        for u in units {
            push_u16(out, u);
        }
    }

    /// A descriptor key: a zero length followed by a four-byte code.
    fn push_key(out: &mut Vec<u8>, code: &str) {
        push_u32(out, 0);
        out.extend_from_slice(code.as_bytes());
    }

    fn push_section(out: &mut Vec<u8>, key: &str, data: &[u8]) {
        out.extend_from_slice(b"8BIM");
        out.extend_from_slice(key.as_bytes());
        push_u32(out, data.len() as u32);
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
    }

    fn decode_png_gray(bytes: &[u8]) -> Vec<u8> {
        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        buf
    }

    /// A v6 ABR with one sampled tip (a 3 x 2 gray sample, black is paint) and one preset that
    /// refers to it by id.
    fn abr_v6_one_sampled_tip() -> Vec<u8> {
        let mut out = Vec::new();
        push_u16(&mut out, 6);
        push_u16(&mut out, 2);

        // samp: one entry, raw 8-bit pixels.
        let mut entry = Vec::new();
        entry.extend_from_slice(b"tip-1\0");
        push_i32(&mut entry, 0); // top
        push_i32(&mut entry, 0); // left
        push_i32(&mut entry, 2); // bottom
        push_i32(&mut entry, 3); // right
        push_u16(&mut entry, 8); // depth
        entry.push(0); // raw
        entry.extend_from_slice(&[0, 255, 128, 64, 0, 255]);
        let mut samp = Vec::new();
        push_u32(&mut samp, entry.len() as u32);
        samp.extend_from_slice(&entry);
        push_section(&mut out, "samp", &samp);

        // desc: version, then one preset descriptor.
        let mut desc = Vec::new();
        push_u32(&mut desc, 16);
        push_unicode(&mut desc, "");
        push_key(&mut desc, "bsh "); // class, written as a four-byte code
        push_u32(&mut desc, 7);
        push_key(&mut desc, "Nm  ");
        desc.extend_from_slice(b"TEXT");
        push_unicode(&mut desc, "Test tip");
        push_key(&mut desc, "Dmtr");
        desc.extend_from_slice(b"UntF");
        desc.extend_from_slice(b"#Pxl");
        desc.extend_from_slice(&40.0f64.to_be_bytes());
        push_key(&mut desc, "Spcn");
        desc.extend_from_slice(b"UntF");
        desc.extend_from_slice(b"#Prc");
        desc.extend_from_slice(&25.0f64.to_be_bytes());
        push_key(&mut desc, "Angl");
        desc.extend_from_slice(b"UntF");
        desc.extend_from_slice(b"#Ang");
        desc.extend_from_slice(&30.0f64.to_be_bytes());
        push_key(&mut desc, "Rndn");
        desc.extend_from_slice(b"UntF");
        desc.extend_from_slice(b"#Prc");
        desc.extend_from_slice(&50.0f64.to_be_bytes());
        push_key(&mut desc, "Hrdn");
        desc.extend_from_slice(b"UntF");
        desc.extend_from_slice(b"#Prc");
        desc.extend_from_slice(&100.0f64.to_be_bytes());
        push_key(&mut desc, "tipI");
        desc.extend_from_slice(b"TEXT");
        push_unicode(&mut desc, "tip-1");
        push_section(&mut out, "desc", &desc);
        out
    }

    #[test]
    fn abr_v6_sampled_tip_maps_settings_and_inverts_sample() {
        let import = read_abr(&abr_v6_one_sampled_tip()).unwrap();
        assert!(import.warnings.is_empty(), "{:?}", import.warnings);
        assert_eq!(import.brushes.len(), 1);
        let brush = &import.brushes[0];
        assert_eq!(brush.name, "Test tip");
        assert_eq!(brush.number(keys::MAX_SIZE), Some(40.0));
        assert_eq!(brush.number(keys::PLOT_SPACING), Some(0.25));
        assert_eq!(brush.number(keys::SHAPE_ROTATION), Some(30.0));
        assert_eq!(brush.number(keys::SHAPE_ROUNDNESS), Some(0.5));
        assert_eq!(brush.number(keys::HARDNESS), None);
        assert!(brush.notes.iter().any(|n| n.contains("hardness")));
        let png = brush.shape_png.as_ref().expect("sampled tip");
        assert_eq!(decode_png_gray(png), vec![255, 0, 127, 191, 255, 0]);
    }

    #[test]
    fn abr_v1_and_v2_are_rejected() {
        for version in [1u8, 2] {
            let bytes = [0, version, 0, 1, 0, 0];
            match read_abr(&bytes) {
                Err(Error::Unsupported(msg)) => {
                    assert_eq!(msg, "this brush file version is not supported");
                }
                other => panic!("expected unsupported, got {other:?}"),
            }
        }
    }

    #[test]
    fn packbits_decodes_runs_and_literals() {
        // Repeat 7 three times, then a literal run of 1, 2, 3.
        let src = [0xFE, 7, 0x02, 1, 2, 3];
        assert_eq!(packbits_decode(&src, 6).unwrap(), vec![7, 7, 7, 1, 2, 3]);
        assert!(packbits_decode(&src, 5).is_err());
    }

    #[test]
    fn abr_without_usable_presets_is_rejected() {
        let mut out = Vec::new();
        push_u16(&mut out, 6);
        push_u16(&mut out, 2);
        let mut desc = Vec::new();
        push_u32(&mut desc, 16);
        push_unicode(&mut desc, "");
        push_key(&mut desc, "bsh ");
        push_u32(&mut desc, 1);
        push_key(&mut desc, "Nm  ");
        desc.extend_from_slice(b"TEXT");
        push_unicode(&mut desc, "Empty");
        push_section(&mut out, "desc", &desc);
        assert!(matches!(
            read_abr(&out),
            Err(Error::InvalidInput(msg)) if msg == "no brushes found in this file"
        ));
    }

    /// A brush using the §6 keys for size, spacing, Smoothing and taper, plus a curve, a roll
    /// parameter set, a colour and an array.
    fn sample_brush() -> ImportedBrush {
        let mut settings = BTreeMap::new();
        settings.insert(keys::MAX_SIZE.into(), SettingValue::Number(40.5));
        settings.insert(keys::PLOT_SPACING.into(), SettingValue::Number(0.05));
        settings.insert(keys::PLOT_SMOOTHING.into(), SettingValue::Number(0.5));
        settings.insert("blendMode".into(), SettingValue::Number(3.0));
        settings.insert("shapeInverted".into(), SettingValue::Bool(true));
        settings.insert("authorName".into(), SettingValue::Text("ink".into()));
        settings.insert(
            "dynamicsPressureOpacityCurve".into(),
            SettingValue::Curve(vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]]),
        );
        settings.insert(
            "dynamicsRollSizeParameters".into(),
            SettingValue::Roll {
                falloff: 0.3,
                origin: 0.1,
                range: 1.5,
                mode: 2,
            },
        );
        settings.insert("color".into(), SettingValue::Color([0.25, 0.5, 0.75, 1.0]));
        settings.insert(
            "savedPaintSizes".into(),
            SettingValue::Array(vec![20.0, 35.5]),
        );
        ImportedBrush {
            name: "Round ink".into(),
            settings,
            shape_png: Some(b"\x89PNG shape bytes".to_vec()),
            grain_png: Some(b"grain bytes".to_vec()),
            notes: Vec::new(),
        }
    }

    #[test]
    fn brush_round_trip_keeps_curves_roll_colour_and_arrays() {
        let brush = sample_brush();
        let bytes = write_brush(&brush).unwrap();

        // The members are the §6 names, and the root class is SilicaBrush.
        let mut zip = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        assert!(zip.by_name(BRUSH_ARCHIVE).is_ok());
        assert!(zip.by_name(SHAPE_MEMBER).is_ok());
        assert!(zip.by_name(GRAIN_MEMBER).is_ok());
        let archive = read_member(&mut zip, BRUSH_ARCHIVE).unwrap().unwrap();
        match keyedarchive::unarchive(&archive).unwrap() {
            Object::Archived(root) => assert_eq!(root.class_name, SILICA_CLASS),
            other => panic!("root is not an object: {other:?}"),
        }

        let read = read_brush("fallback", &bytes).unwrap();
        assert_eq!(read.name, brush.name);
        assert_eq!(read.settings, brush.settings);
        assert_eq!(read.shape_png, brush.shape_png);
        assert_eq!(read.grain_png, brush.grain_png);
        assert!(read.notes.is_empty(), "{:?}", read.notes);
    }

    #[test]
    fn hand_built_brush_with_section6_keys_maps_engine_params() {
        let mut fields: BTreeMap<String, NsValue> = BTreeMap::new();
        fields.insert("name".into(), NsValue::String("Hand built".into()));
        fields.insert("maxSize".into(), NsValue::Real(40.0));
        fields.insert("minSize".into(), NsValue::Real(8.0));
        fields.insert("plotSpacing".into(), NsValue::Real(0.05));
        fields.insert("plotSmoothing".into(), NsValue::Real(0.5));
        fields.insert("taperStartLength".into(), NsValue::Real(0.3));
        fields.insert("taperEndLength".into(), NsValue::Real(0.4));
        fields.insert("taperSize".into(), NsValue::Real(0.8));
        fields.insert("taperOpacity".into(), NsValue::Real(0.2));
        fields.insert("taperShape".into(), NsValue::Real(-0.5));
        fields.insert("taperPressure".into(), NsValue::Real(0.75));
        fields.insert("pencilTaperStartLength".into(), NsValue::Real(0.1));
        fields.insert("pencilTaperEndLength".into(), NsValue::Real(0.2));
        fields.insert("pencilTaperSize".into(), NsValue::Real(0.5));
        fields.insert("pencilTaperOpacity".into(), NsValue::Real(0.0));
        fields.insert("pencilTaperShape".into(), NsValue::Real(0.25));
        fields.insert("blendMode".into(), NsValue::Int(3));
        fields.insert(
            "bundledShapePath".into(),
            NsValue::String("Shapes/Round.png".into()),
        );
        let mut object = NsObject::new(SILICA_CLASS);
        object.fields = fields.into_iter().collect();
        let archive = keyedarchive::write(&NsValue::Object(object)).unwrap();
        let bytes = zip_members(&[(BRUSH_ARCHIVE, &archive)]).unwrap();

        let brush = read_brush("fallback", &bytes).unwrap();
        assert_eq!(brush.name, "Hand built");
        assert_eq!(brush.number("blendMode"), Some(3.0));
        assert!(brush.shape_png.is_none());
        assert!(brush.notes.iter().any(|n| n.contains("Shapes/Round.png")));

        let params = engine_params(&brush);
        assert_eq!(params.size, 40.0);
        assert!((params.min_size - 0.2).abs() < 1e-12);
        assert_eq!(params.spacing, 0.05);
        assert_eq!(params.smoothing, 0.5);
        assert_eq!(
            params.taper,
            TaperParams {
                start_length: 0.3,
                end_length: 0.4,
                size: 0.8,
                opacity: 0.2,
                shape: -0.5,
                pressure: 0.75,
            }
        );
        assert_eq!(
            params.pencil_taper,
            TaperParams {
                start_length: 0.1,
                end_length: 0.2,
                size: 0.5,
                opacity: 0.0,
                shape: 0.25,
                pressure: 1.0,
            }
        );
    }

    #[test]
    fn engine_params_use_spec_defaults_for_missing_keys() {
        let params = engine_params(&ImportedBrush::default());
        assert_eq!(params, EngineParams::default());
        assert_eq!(params.size, 20.0);
        assert_eq!(params.spacing, 0.1);
        assert_eq!(params.smoothing, 0.3);
        assert_eq!(params.taper.start_length, 0.1);
    }

    #[test]
    fn brush_missing_texture_falls_back_with_note() {
        let mut brush = sample_brush();
        brush.shape_png = None;
        brush.settings.insert(
            keys::BUNDLED_SHAPE_PATH.into(),
            SettingValue::Text("Shapes/Gone.png".into()),
        );
        let bytes = write_brush(&brush).unwrap();
        let read = read_brush("fallback", &bytes).unwrap();
        assert_eq!(read.shape_png, None);
        assert!(read.notes.iter().any(|n| n.contains("Shapes/Gone.png")));
    }

    #[test]
    fn brush_without_archive_is_rejected() {
        let bytes = zip_members(&[("other.txt", b"hi")]).unwrap();
        assert!(matches!(read_brush("x", &bytes), Err(Error::Format(_))));
    }

    #[test]
    fn brush_map_calls_closure_per_setting() {
        #[derive(Default, Debug, PartialEq)]
        struct Target {
            size: f64,
            flag: bool,
        }
        let brush = sample_brush();
        let target = brush.map::<Target>(|t, key, value| match (key, value) {
            (keys::MAX_SIZE, SettingValue::Number(n)) => t.size = *n,
            ("shapeInverted", SettingValue::Bool(b)) => t.flag = *b,
            _ => {}
        });
        assert_eq!(
            target,
            Target {
                size: 40.5,
                flag: true
            }
        );
    }

    #[test]
    fn brushset_round_trip_keeps_name_and_order() {
        let soft = ImportedBrush {
            name: "Soft".into(),
            ..ImportedBrush::default()
        };
        let hard = ImportedBrush {
            name: "Hard/Edge".into(),
            ..ImportedBrush::default()
        };
        let bytes = write_brushset("Set A", &[soft.clone(), hard.clone()]).unwrap();
        let set = read_brushset("fallback", &bytes).unwrap();
        assert_eq!(set.name, "Set A");
        assert!(set.failures.is_empty(), "{:?}", set.failures);
        let names: Vec<&str> = set.brushes.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["Soft", "Hard/Edge"]);
    }

    #[test]
    fn brushset_manifest_orders_folders_and_failures_are_listed() {
        let brush = |name: &str| ImportedBrush {
            name: name.into(),
            ..ImportedBrush::default()
        };
        let mut members = brush_members(&brush("B"), "B/").unwrap();
        members.extend(brush_members(&brush("A"), "A/").unwrap());
        // A folder whose archive is corrupt fails and the others still import.
        members.push(("Broken/Brush.archive".into(), b"not a bplist".to_vec()));
        members.push(("notes.txt".into(), b"ignored".to_vec()));
        let mut manifest = plist::Dictionary::new();
        manifest.insert("name".into(), plist::Value::String("Ordered".into()));
        manifest.insert(
            "brushes".into(),
            plist::Value::Array(vec![
                plist::Value::String("B".into()),
                plist::Value::String("Broken".into()),
                plist::Value::String("A".into()),
            ]),
        );
        let mut xml = Vec::new();
        plist::Value::Dictionary(manifest)
            .to_writer_xml(&mut xml)
            .unwrap();
        members.push((SET_MANIFEST.into(), xml));
        let bytes = zip_owned(&members).unwrap();

        let set = read_brushset("fallback", &bytes).unwrap();
        assert_eq!(set.name, "Ordered");
        let names: Vec<&str> = set.brushes.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["B", "A"]);
        assert_eq!(set.failures.len(), 1);
        assert!(set.failures[0].starts_with("Broken/"));
    }
}
