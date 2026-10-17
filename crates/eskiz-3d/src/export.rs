//! Export: GLB with embedded PNG textures, and one PNG per map.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::model::{MapKind, Model, TextureSet};
use crate::usd::{self, RootFormat};

/// Export failure.
#[derive(Debug)]
pub enum ExportError {
    Io(std::io::Error),
    Png(String),
    /// Nothing to export (no triangles).
    Empty,
    /// The model cannot be written in this format (message says why).
    Invalid(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::Io(e) => write!(f, "I/O error: {e}"),
            ExportError::Png(e) => write!(f, "PNG error: {e}"),
            ExportError::Empty => write!(f, "model has no triangles"),
            ExportError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(e: std::io::Error) -> Self {
        ExportError::Io(e)
    }
}

/// Encode straight RGBA8 as PNG. `dpi` writes a pixel-density chunk.
pub fn encode_png(
    rgba: &[u8],
    width: usize,
    height: usize,
    dpi: Option<u32>,
) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width as u32, height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        if let Some(d) = dpi {
            let ppm = (f64::from(d) / 0.0254).round() as u32;
            enc.set_pixel_dims(Some(png::PixelDimensions {
                xppu: ppm,
                yppu: ppm,
                unit: png::Unit::Meter,
            }));
        }
        let mut w = enc
            .write_header()
            .map_err(|e| ExportError::Png(e.to_string()))?;
        w.write_image_data(rgba)
            .map_err(|e| ExportError::Png(e.to_string()))?;
    }
    Ok(out)
}

/// File-name-safe version of a material name.
pub fn sanitise_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "material".to_string()
    } else {
        s
    }
}

/// Unique file prefixes per texture set (collisions get a numeric suffix).
pub(crate) fn unique_prefixes(sets: &[TextureSet]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for s in sets {
        let base = sanitise_name(&s.name);
        let mut cand = base.clone();
        let mut n = 1;
        while seen.contains(&cand) {
            n += 1;
            cand = format!("{base}_{n}");
        }
        seen.push(cand);
    }
    seen
}

/// Write one PNG per map per texture set into `dir` as `<material>_<map>.png`
/// (no channel packing). Returns the written paths.
pub fn export_map_pngs(
    model: &Model,
    dir: &Path,
    dpi: Option<u32>,
) -> Result<Vec<PathBuf>, ExportError> {
    std::fs::create_dir_all(dir)?;
    let prefixes = unique_prefixes(&model.texture_sets);
    let mut paths = Vec::new();
    for (set, prefix) in model.texture_sets.iter().zip(&prefixes) {
        for kind in MapKind::ALL {
            let bytes = encode_png(&set.straight_rgba(kind), set.width, set.height, dpi)?;
            let path = dir.join(format!("{prefix}_{}.png", kind.file_stem()));
            std::fs::write(&path, bytes)?;
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Pack AO (R), roughness (G) and metallic (B) into one RGBA image, the usual glTF layout.
fn pack_orm(set: &TextureSet) -> Vec<u8> {
    let ao = set.map(MapKind::AmbientOcclusion).to_rgba();
    let ro = set.map(MapKind::Roughness).to_rgba();
    let me = set.map(MapKind::Metallic).to_rgba();
    let mut out = vec![255u8; set.width * set.height * 4];
    for i in 0..set.width * set.height {
        out[i * 4] = ao[i * 4];
        out[i * 4 + 1] = ro[i * 4];
        out[i * 4 + 2] = me[i * 4];
    }
    out
}

struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
}

impl Bin {
    fn add(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.data.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.data.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }
}

fn f32_bytes<'a>(it: impl Iterator<Item = &'a f32>) -> Vec<u8> {
    it.flat_map(|f| f.to_le_bytes()).collect()
}

/// Build a binary glTF (GLB) in memory with the painted textures embedded as PNG.
///
/// Each texture set becomes a material with base colour, a packed
/// occlusion/roughness/metallic image (R/G/B), and a normal map. Factors are 1
/// because the maps already hold the full values.
pub fn write_glb_bytes(model: &Model) -> Result<Vec<u8>, ExportError> {
    if model.triangle_count() == 0 {
        return Err(ExportError::Empty);
    }
    let mut bin = Bin {
        data: Vec::new(),
        views: Vec::new(),
    };
    let mut accessors: Vec<Value> = Vec::new();
    let mut primitives: Vec<Value> = Vec::new();

    for sm in &model.submeshes {
        if sm.triangle_count() == 0 {
            continue;
        }
        let (lo, hi) = sm
            .positions
            .iter()
            .fold((sm.positions[0], sm.positions[0]), |(l, h), p| {
                (l.min(*p), h.max(*p))
            });
        let pos: Vec<f32> = sm.positions.iter().flat_map(|p| p.to_array()).collect();
        let v = bin.add(&f32_bytes(pos.iter()), Some(34962));
        accessors.push(json!({"bufferView": v, "componentType": 5126, "count": sm.positions.len(), "type": "VEC3",
            "min": lo.to_array(), "max": hi.to_array()}));
        let mut attrs = json!({"POSITION": accessors.len() - 1});
        if sm.normals.len() == sm.positions.len() {
            let n: Vec<f32> = sm.normals.iter().flat_map(|p| p.to_array()).collect();
            let v = bin.add(&f32_bytes(n.iter()), Some(34962));
            accessors.push(json!({"bufferView": v, "componentType": 5126, "count": sm.normals.len(), "type": "VEC3"}));
            attrs["NORMAL"] = json!(accessors.len() - 1);
        }
        if sm.uvs.len() == sm.positions.len() {
            let t: Vec<f32> = sm.uvs.iter().flat_map(|p| p.to_array()).collect();
            let v = bin.add(&f32_bytes(t.iter()), Some(34962));
            accessors.push(json!({"bufferView": v, "componentType": 5126, "count": sm.uvs.len(), "type": "VEC2"}));
            attrs["TEXCOORD_0"] = json!(accessors.len() - 1);
        }
        let idx: Vec<u8> = sm.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let v = bin.add(&idx, Some(34963));
        accessors.push(json!({"bufferView": v, "componentType": 5125, "count": sm.indices.len(), "type": "SCALAR"}));
        primitives.push(json!({"attributes": attrs, "indices": accessors.len() - 1, "material": sm.material, "mode": 4}));
    }

    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut materials = Vec::new();
    for set in &model.texture_sets {
        let mut add_image = |bin: &mut Bin, rgba: &[u8]| -> Result<usize, ExportError> {
            let png = encode_png(rgba, set.width, set.height, None)?;
            let v = bin.add(&png, None);
            images.push(json!({"bufferView": v, "mimeType": "image/png", "name": set.name}));
            textures.push(json!({"sampler": 0, "source": images.len() - 1}));
            Ok(textures.len() - 1)
        };
        let colour = add_image(&mut bin, &set.straight_rgba(MapKind::Colour))?;
        let orm = add_image(&mut bin, &pack_orm(set))?;
        let normal = add_image(&mut bin, &set.straight_rgba(MapKind::Normal))?;
        let translucent = set
            .map(MapKind::Colour)
            .to_rgba()
            .chunks_exact(4)
            .any(|p| p[3] < 255);
        materials.push(json!({
            "name": set.name,
            "pbrMetallicRoughness": {
                "baseColorTexture": {"index": colour},
                "baseColorFactor": [1.0, 1.0, 1.0, 1.0],
                "metallicFactor": 1.0,
                "roughnessFactor": 1.0,
                "metallicRoughnessTexture": {"index": orm}
            },
            "occlusionTexture": {"index": orm},
            "normalTexture": {"index": normal},
            "alphaMode": if translucent { "BLEND" } else { "OPAQUE" },
            "doubleSided": true
        }));
    }

    let root = json!({
        "asset": {"version": "2.0", "generator": "eskiz-3d"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "model"}],
        "meshes": [{"name": "model", "primitives": primitives}],
        "materials": materials,
        "textures": textures,
        "images": images,
        "samplers": [{"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}],
        "accessors": accessors,
        "bufferViews": bin.views,
        "buffers": [{"byteLength": bin.data.len()}]
    });
    let mut json_bytes = serde_json::to_vec(&root).map_err(|e| ExportError::Png(e.to_string()))?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut data = bin.data;
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + data.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&data);
    Ok(out)
}

/// Write a GLB file.
pub fn export_glb(model: &Model, path: &Path) -> Result<(), ExportError> {
    let bytes = write_glb_bytes(model)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Options for [`write_usdz_bytes`].
#[derive(Clone, Copy, Debug, Default)]
pub struct UsdzOptions {
    /// Root layer format (only `.usda` today).
    pub root: RootFormat,
    /// Pixel density written into the texture PNGs.
    pub dpi: Option<u32>,
}

/// Build a USDZ package in memory: the root layer first, then one unpacked PNG per map per used material under
/// `textures/`. Every entry's data starts on a 64-byte boundary.
///
/// Errors: [`ExportError::Empty`] when there are no triangles, [`ExportError::Invalid`] when a mesh with triangles
/// has no UVs, a material index is out of range, or a position is not finite.
pub fn write_usdz_bytes(model: &Model, opts: &UsdzOptions) -> Result<Vec<u8>, ExportError> {
    if model.triangle_count() == 0 {
        return Err(ExportError::Empty);
    }
    for sm in model.submeshes.iter().filter(|s| s.triangle_count() > 0) {
        if sm.uvs.len() != sm.positions.len() {
            return Err(ExportError::Invalid(format!(
                "mesh \"{}\" has no UV coordinates; USDZ export needs UVs",
                sm.name
            )));
        }
        if sm.material >= model.texture_sets.len() {
            return Err(ExportError::Invalid(format!("mesh \"{}\" uses a material that does not exist", sm.name)));
        }
        if sm.positions.iter().any(|p| !p.is_finite()) {
            return Err(ExportError::Invalid(format!("mesh \"{}\" has non-finite positions", sm.name)));
        }
        if sm.indices[..sm.triangle_count() * 3].iter().any(|&i| i as usize >= sm.positions.len()) {
            return Err(ExportError::Invalid(format!("mesh \"{}\" has an out-of-range index", sm.name)));
        }
    }
    let idents = usd::material_idents(model);
    let mut entries: Vec<(String, Vec<u8>)> = vec![(
        opts.root.file_name().to_owned(),
        usd::write_usda(model).into_bytes(),
    )];
    for mat in usd::used_materials(model) {
        let set = &model.texture_sets[mat];
        for kind in MapKind::ALL {
            let png = encode_png(&set.straight_rgba(kind), set.width, set.height, opts.dpi)?;
            entries.push((usd::texture_entry_name(&idents[mat], kind), png));
        }
    }
    usd::zip::write_stored_zip(&entries)
}

/// Write a USDZ file.
pub fn export_usdz(model: &Model, path: &Path) -> Result<(), ExportError> {
    let bytes = write_usdz_bytes(model, &UsdzOptions::default())?;
    std::fs::write(path, bytes)?;
    Ok(())
}
