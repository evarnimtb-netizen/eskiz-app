//! Mesh, material and texture-set data model.

use eskiz_core::Layer;
use glam::{Vec2, Vec3};

/// The five maps of a texture set, in the documented binding order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapKind {
    Colour = 0,
    Roughness = 1,
    Metallic = 2,
    Normal = 3,
    AmbientOcclusion = 4,
}

impl MapKind {
    /// All maps in binding order.
    pub const ALL: [MapKind; 5] = [
        MapKind::Colour,
        MapKind::Roughness,
        MapKind::Metallic,
        MapKind::Normal,
        MapKind::AmbientOcclusion,
    ];

    /// Layers of the "base" group (colour, roughness, metallic).
    pub const BASE_GROUP: [MapKind; 3] = [MapKind::Colour, MapKind::Roughness, MapKind::Metallic];

    /// Index into [`TextureSet::maps`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Colour maps are sRGB; all others are linear data.
    pub fn is_srgb(self) -> bool {
        self == MapKind::Colour
    }

    /// File-name stem used for exports.
    pub fn file_stem(self) -> &'static str {
        match self {
            MapKind::Colour => "color",
            MapKind::Roughness => "roughness",
            MapKind::Metallic => "metallic",
            MapKind::Normal => "normal",
            MapKind::AmbientOcclusion => "ambient_occlusion",
        }
    }
}

/// sRGB byte to linear 0..1.
pub fn srgb_to_linear(b: u8) -> f32 {
    let c = f32::from(b) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear 0..1 to sRGB byte.
pub fn linear_to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round() as u8
}

/// One material's maps. Maps are premultiplied RGBA8 eskiz-core layers of the same size.
///
/// Colour is stored sRGB-encoded; the other maps store linear data as raw bytes.
/// Roughness, metallic and AO maps are greyscale (value replicated in RGB, alpha 255).
#[derive(Clone, Debug)]
pub struct TextureSet {
    pub name: String,
    pub width: usize,
    pub height: usize,
    /// Indexed by [`MapKind::index`].
    pub maps: [Layer; 5],
    /// Per-set visibility, lock and opacity. Opacity is 0..1.
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
}

impl TextureSet {
    /// A set filled with constants: `colour` is sRGB RGBA 0..255, `roughness` and
    /// `metallic` are 0..1. Normal is flat (0.5, 0.5, 1) and AO is white.
    pub fn from_constants(
        name: &str,
        width: usize,
        height: usize,
        colour: [u8; 4],
        roughness: f32,
        metallic: f32,
    ) -> Self {
        let fill = |px: [u8; 4]| Layer::from_rgba(width, height, px.repeat(width * height));
        let a = f32::from(colour[3]) / 255.0;
        let premul = |c: u8| (f32::from(c) * a).round() as u8;
        let grey = |v: f32| {
            let b = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            [b, b, b, 255]
        };
        Self {
            name: name.to_string(),
            width,
            height,
            maps: [
                fill([
                    premul(colour[0]),
                    premul(colour[1]),
                    premul(colour[2]),
                    colour[3],
                ]),
                fill(grey(roughness)),
                fill(grey(metallic)),
                fill([128, 128, 255, 255]),
                fill([255, 255, 255, 255]),
            ],
            visible: true,
            locked: false,
            opacity: 1.0,
        }
    }

    /// Layer for `kind`.
    pub fn map(&self, kind: MapKind) -> &Layer {
        &self.maps[kind.index()]
    }

    /// Mutable layer for `kind`.
    pub fn map_mut(&mut self, kind: MapKind) -> &mut Layer {
        &mut self.maps[kind.index()]
    }

    /// Straight (un-premultiplied) RGBA8 of a map, as written to PNG.
    pub fn straight_rgba(&self, kind: MapKind) -> Vec<u8> {
        let mut out = self.map(kind).to_rgba();
        for p in out.chunks_exact_mut(4) {
            let a = u32::from(p[3]);
            if a != 0 && a != 255 {
                for c in p.iter_mut().take(3) {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }

    /// Sample a map at `(u, v)` with nearest filtering and repeat wrapping,
    /// returning straight RGBA 0..1 (colour stays sRGB-encoded).
    pub fn sample(&self, kind: MapKind, u: f32, v: f32) -> [f32; 4] {
        let l = self.map(kind);
        let x = ((u - u.floor()) * l.width as f32) as usize % l.width;
        let y = ((v - v.floor()) * l.height as f32) as usize % l.height;
        let p = l.pixel(x, y);
        let a = f32::from(p[3]) / 255.0;
        let un = |c: u8| {
            if p[3] == 0 {
                0.0
            } else {
                f32::from(c) / 255.0 / a
            }
        };
        [un(p[0]).min(1.0), un(p[1]).min(1.0), un(p[2]).min(1.0), a]
    }
}

/// One drawable chunk of a model: indexed triangles sharing a material.
#[derive(Clone, Debug, Default)]
pub struct SubMesh {
    pub name: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// Per-vertex UV, top-left origin. Empty when the source had none.
    pub uvs: Vec<Vec2>,
    /// Triangle list, three indices per triangle.
    pub indices: Vec<u32>,
    /// Index into [`Model::texture_sets`].
    pub material: usize,
}

impl SubMesh {
    /// Triangle count.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Corner positions of triangle `t`.
    pub fn tri_positions(&self, t: usize) -> [Vec3; 3] {
        let i = &self.indices[t * 3..t * 3 + 3];
        [
            self.positions[i[0] as usize],
            self.positions[i[1] as usize],
            self.positions[i[2] as usize],
        ]
    }

    /// Corner UVs of triangle `t` (zero when the mesh has no UVs).
    pub fn tri_uvs(&self, t: usize) -> [Vec2; 3] {
        if self.uvs.is_empty() {
            return [Vec2::ZERO; 3];
        }
        let i = &self.indices[t * 3..t * 3 + 3];
        [
            self.uvs[i[0] as usize],
            self.uvs[i[1] as usize],
            self.uvs[i[2] as usize],
        ]
    }

    /// Replace normals with flat per-face normals (unwelds vertices). This is the
    /// "crease threshold 0" normal generation the design notes ask for on import.
    pub fn generate_flat_normals(&mut self) {
        let mut positions = Vec::with_capacity(self.indices.len());
        let mut uvs = Vec::new();
        let mut normals = Vec::with_capacity(self.indices.len());
        let mut indices = Vec::with_capacity(self.indices.len());
        for t in 0..self.triangle_count() {
            let p = self.tri_positions(t);
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
            let uv = self.tri_uvs(t);
            for k in 0..3 {
                indices.push(positions.len() as u32);
                positions.push(p[k]);
                normals.push(n);
                if !self.uvs.is_empty() {
                    uvs.push(uv[k]);
                }
            }
        }
        self.positions = positions;
        self.normals = normals;
        self.uvs = uvs;
        self.indices = indices;
    }
}

/// Texel coverage of a texture set's UV triangles, with island labels.
///
/// `label` is 0 for empty (gutter) texels, an island id (>= 1) for texels
/// touched by exactly one island, and [`UvMask::SHARED`] where islands touch.
#[derive(Clone, Debug, Default)]
pub struct UvMask {
    pub width: usize,
    pub height: usize,
    pub label: Vec<u32>,
}

impl UvMask {
    /// Label for texels where two islands meet; accepts paint from either.
    pub const SHARED: u32 = u32::MAX;

    /// Whether texel `(x, y)` is covered by any island.
    pub fn covered(&self, x: usize, y: usize) -> bool {
        self.label[y * self.width + x] != 0
    }
}

/// A loaded model: submeshes plus one texture set per material.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub submeshes: Vec<SubMesh>,
    /// One per material, in the source file's material order.
    pub texture_sets: Vec<TextureSet>,
    /// Point primitives seen in the source (not drawable as triangles).
    pub point_primitives: usize,
    /// One mask per texture set, built by [`Model::rebuild_masks`].
    pub masks: Vec<UvMask>,
    /// First global triangle id of each submesh.
    tri_offsets: Vec<u32>,
}

impl Model {
    /// Assemble a model and build triangle offsets and UV masks.
    pub fn new(
        submeshes: Vec<SubMesh>,
        texture_sets: Vec<TextureSet>,
        point_primitives: usize,
    ) -> Self {
        let mut m = Self {
            submeshes,
            texture_sets,
            point_primitives,
            masks: Vec::new(),
            tri_offsets: Vec::new(),
        };
        m.rebuild_offsets();
        m.rebuild_masks();
        m
    }

    /// Recompute global triangle numbering after editing `submeshes`.
    pub fn rebuild_offsets(&mut self) {
        let mut acc = 0u32;
        self.tri_offsets = self
            .submeshes
            .iter()
            .map(|s| {
                let o = acc;
                acc += s.triangle_count() as u32;
                o
            })
            .collect();
    }

    /// Total triangles across all submeshes.
    pub fn triangle_count(&self) -> usize {
        self.submeshes.iter().map(SubMesh::triangle_count).sum()
    }

    /// First global triangle id of `submesh`.
    pub fn tri_offset(&self, submesh: usize) -> u32 {
        self.tri_offsets[submesh]
    }

    /// Resolve a global triangle id into `(submesh, local triangle)`.
    pub fn locate(&self, tri: u32) -> Option<(usize, usize)> {
        let s = self
            .tri_offsets
            .partition_point(|&o| o <= tri)
            .checked_sub(1)?;
        let local = (tri - self.tri_offsets[s]) as usize;
        (local < self.submeshes[s].triangle_count()).then_some((s, local))
    }

    /// Texture-set index (material) of a global triangle id.
    pub fn material_of(&self, tri: u32) -> Option<usize> {
        self.locate(tri).map(|(s, _)| self.submeshes[s].material)
    }

    /// Texture-set indices in the order the design notes list them: reverse of material order.
    pub fn texture_set_display_order(&self) -> Vec<usize> {
        (0..self.texture_sets.len()).rev().collect()
    }

    /// Axis-aligned bounds of all vertices, or `None` for an empty model.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self
            .submeshes
            .iter()
            .flat_map(|s| s.positions.iter().copied());
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// Rebuild per-texture-set UV island masks (call after changing UVs or texture sizes).
    pub fn rebuild_masks(&mut self) {
        self.masks = (0..self.texture_sets.len())
            .map(|i| crate::check::build_uv_mask(self, i))
            .collect();
    }
}
