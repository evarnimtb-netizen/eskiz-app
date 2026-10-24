//! Model checks and UV island masks.

use std::collections::HashMap;

use glam::Vec2;

use crate::model::{Model, UvMask};

/// Resolution of the overlap-detection raster (samples per side over 0..1).
pub const OVERLAP_RESOLUTION: usize = 512;

/// A check failure that stops the import.
#[derive(Clone, Debug, PartialEq)]
pub enum FatalIssue {
    /// A submesh has no non-zero UV coordinates.
    MissingUvs { submesh: usize, name: String },
    /// Point geometry and no triangles.
    PointCloud,
    /// No mesh objects at all.
    NoMesh,
    /// A vertex position is infinite or not a number: the model cannot be framed or exported.
    NonFinitePositions { submesh: usize, name: String },
}

/// A check finding that lets the import continue.
#[derive(Clone, Debug, PartialEq)]
pub enum Warning {
    /// UV triangles overlap in texture space (paint on one spot lands on another).
    OverlappingUvs { texture_set: usize, texels: usize },
    /// UVs fall outside 0..1 (texture repeating).
    TextureRepeating { submesh: usize, name: String },
}

/// Result of [`Model::check`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckReport {
    pub fatal: Vec<FatalIssue>,
    pub warnings: Vec<Warning>,
}

impl CheckReport {
    /// True when the import must stop.
    pub fn is_fatal(&self) -> bool {
        !self.fatal.is_empty()
    }

    /// Human-readable one-line descriptions, fatal first.
    pub fn messages(&self) -> Vec<String> {
        let mut out = Vec::new();
        for f in &self.fatal {
            out.push(match f {
                FatalIssue::MissingUvs { name, .. } => {
                    format!("Mesh \"{name}\" has no UV coordinates.")
                }
                FatalIssue::PointCloud => {
                    "The file contains only points, not triangles.".to_string()
                }
                FatalIssue::NoMesh => "The file contains no mesh.".to_string(),
                FatalIssue::NonFinitePositions { name, .. } => {
                    format!("Mesh \"{name}\" has vertex positions that are not numbers.")
                }
            });
        }
        for w in &self.warnings {
            out.push(match w {
                Warning::OverlappingUvs {
                    texture_set,
                    texels,
                } => {
                    format!("Texture set {texture_set} has overlapping UVs ({texels} texels).")
                }
                Warning::TextureRepeating { name, .. } => {
                    format!("Mesh \"{name}\" has UVs outside 0 to 1 (texture repeating).")
                }
            });
        }
        out
    }
}

impl Model {
    /// Run the documented model checks.
    pub fn check(&self) -> CheckReport {
        let mut r = CheckReport::default();
        if self.triangle_count() == 0 {
            r.fatal.push(if self.point_primitives > 0 {
                FatalIssue::PointCloud
            } else {
                FatalIssue::NoMesh
            });
            return r;
        }
        for (i, s) in self.submeshes.iter().enumerate() {
            if s.triangle_count() == 0 {
                continue;
            }
            if s.uvs.iter().all(|uv| *uv == Vec2::ZERO) {
                r.fatal.push(FatalIssue::MissingUvs {
                    submesh: i,
                    name: s.name.clone(),
                });
            }
            if !s.positions.iter().all(|p| p.is_finite()) {
                r.fatal.push(FatalIssue::NonFinitePositions {
                    submesh: i,
                    name: s.name.clone(),
                });
            }
        }
        if r.is_fatal() {
            return r;
        }
        const EPS: f32 = 1e-4;
        for (i, s) in self.submeshes.iter().enumerate() {
            if s.uvs
                .iter()
                .any(|uv| uv.x < -EPS || uv.y < -EPS || uv.x > 1.0 + EPS || uv.y > 1.0 + EPS)
            {
                r.warnings.push(Warning::TextureRepeating {
                    submesh: i,
                    name: s.name.clone(),
                });
            }
        }
        for set in 0..self.texture_sets.len() {
            let texels = overlapping_texels(self, set, OVERLAP_RESOLUTION);
            if texels > 0 {
                r.warnings.push(Warning::OverlappingUvs {
                    texture_set: set,
                    texels,
                });
            }
        }
        r
    }
}

/// Triangles of material `set` as `(global triangle id, uv corners)`.
fn set_triangles(model: &Model, set: usize) -> Vec<(u32, [Vec2; 3])> {
    let mut out = Vec::new();
    for (si, s) in model.submeshes.iter().enumerate() {
        if s.material != set || s.uvs.is_empty() {
            continue;
        }
        for t in 0..s.triangle_count() {
            out.push((model.tri_offset(si) + t as u32, s.tri_uvs(t)));
        }
    }
    out
}

/// Count samples of a `res x res` grid over the unit square covered strictly
/// inside two or more different UV triangles of material `set`.
///
/// Triangles sharing an edge never count: samples on the shared edge fall
/// strictly inside neither.
pub fn overlapping_texels(model: &Model, set: usize, res: usize) -> usize {
    let tris = set_triangles(model, set);
    let mut owner = vec![u32::MAX; res * res];
    let mut overlapped = vec![false; res * res];
    let rf = res as f32;
    for (id, uv0) in tris {
        for shift in tile_shifts(&uv0) {
            let uv = [uv0[0] + shift, uv0[1] + shift, uv0[2] + shift];
            let c = [uv[0] * rf, uv[1] * rf, uv[2] * rf];
            let area = (c[1] - c[0]).perp_dot(c[2] - c[0]);
            if area.abs() < 1e-6 {
                continue;
            }
            let min = c[0].min(c[1]).min(c[2]);
            let max = c[0].max(c[1]).max(c[2]);
            let x0 = (min.x - 0.5).floor().max(0.0) as usize;
            let y0 = (min.y - 0.5).floor().max(0.0) as usize;
            let x1 = ((max.x + 0.5).ceil().max(0.0) as usize).min(res);
            let y1 = ((max.y + 0.5).ceil().max(0.0) as usize).min(res);
            let eps = 1e-4 * area.abs();
            for y in y0..y1 {
                for x in x0..x1 {
                    let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = (c[2] - c[1]).perp_dot(q - c[1]) / area;
                    let w1 = (c[0] - c[2]).perp_dot(q - c[2]) / area;
                    let w2 = (c[1] - c[0]).perp_dot(q - c[0]) / area;
                    let e = eps / area.abs();
                    if w0 > e && w1 > e && w2 > e {
                        let i = y * res + x;
                        if owner[i] == u32::MAX {
                            owner[i] = id;
                        } else if owner[i] != id {
                            overlapped[i] = true;
                        }
                    }
                }
            }
        }
    }
    overlapped.iter().filter(|&&b| b).count()
}

/// Whole-tile shifts (in UV units) that bring a triangle's parts into the 0..1 tile,
/// matching how the lookup wraps UVs. Non-finite triangles give none; the count is capped.
fn tile_shifts(uv: &[Vec2; 3]) -> Vec<Vec2> {
    const MAX_TILES: i64 = 16;
    if uv.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return Vec::new();
    }
    let min = uv[0].min(uv[1]).min(uv[2]);
    let max = uv[0].max(uv[1]).max(uv[2]);
    let span = |lo: f32, hi: f32| {
        let a = (lo.floor().clamp(-1e6, 1e6)) as i64;
        let b = ((hi.ceil().clamp(-1e6, 1e6)) as i64 - 1).max(a);
        (a, b.min(a + MAX_TILES - 1))
    };
    let (tx0, tx1) = span(min.x, max.x);
    let (ty0, ty1) = span(min.y, max.y);
    let mut out = Vec::new();
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            out.push(Vec2::new(-(tx as f32), -(ty as f32)));
        }
    }
    out
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Build the island-labelled coverage mask for texture set `set`.
///
/// Islands are sets of triangles joined through shared UV vertices. A texel
/// counts as covered when any triangle overlaps its box (conservative), so
/// every texel a lookup can reach is inside the mask.
pub fn build_uv_mask(model: &Model, set: usize) -> UvMask {
    let (w, h) = {
        let ts = &model.texture_sets[set];
        (ts.width, ts.height)
    };
    let mut mask = UvMask {
        width: w,
        height: h,
        label: vec![0; w * h],
    };
    let tris = set_triangles(model, set);
    let mut parent: Vec<usize> = (0..tris.len()).collect();
    let mut by_vertex: HashMap<(i64, i64), usize> = HashMap::new();
    for (ti, (_, uv)) in tris.iter().enumerate() {
        for c in uv {
            let key = ((c.x * 1e5).round() as i64, (c.y * 1e5).round() as i64);
            match by_vertex.get(&key) {
                Some(&other) => {
                    let (a, b) = (find(&mut parent, ti), find(&mut parent, other));
                    if a != b {
                        parent[a] = b;
                    }
                }
                None => {
                    by_vertex.insert(key, ti);
                }
            }
        }
    }
    let (wf, hf) = (w as f32, h as f32);
    for (ti, (_, uv0)) in tris.iter().enumerate() {
        let island = find(&mut parent, ti) as u32 + 1;
        for shift in tile_shifts(uv0) {
            let uv = [uv0[0] + shift, uv0[1] + shift, uv0[2] + shift];
            let mut c = [
                Vec2::new(uv[0].x * wf, uv[0].y * hf),
                Vec2::new(uv[1].x * wf, uv[1].y * hf),
                Vec2::new(uv[2].x * wf, uv[2].y * hf),
            ];
            let area = (c[1] - c[0]).perp_dot(c[2] - c[0]);
            if area.abs() < 1e-6 {
                continue;
            }
            if area < 0.0 {
                c.swap(1, 2);
            }
            let min = c[0].min(c[1]).min(c[2]);
            let max = c[0].max(c[1]).max(c[2]);
            let x0 = min.x.floor().max(0.0) as usize;
            let y0 = min.y.floor().max(0.0) as usize;
            let x1 = (max.x.ceil().max(0.0) as usize).min(w);
            let y1 = (max.y.ceil().max(0.0) as usize).min(h);
            for y in y0..y1 {
                for x in x0..x1 {
                    if box_hits_triangle(&c, x as f32, y as f32) {
                        let l = &mut mask.label[y * w + x];
                        if *l == 0 {
                            *l = island;
                        } else if *l != island {
                            *l = UvMask::SHARED;
                        }
                    }
                }
            }
        }
    }
    mask
}

/// Separating-axis test of the unit texel box at `(bx, by)` against a CCW triangle
/// (the box axes are covered by the caller's bounding-box loop).
fn box_hits_triangle(c: &[Vec2; 3], bx: f32, by: f32) -> bool {
    const EPS: f32 = 1e-3;
    for i in 0..3 {
        let a = c[i];
        let e = c[(i + 1) % 3] - a;
        // Inside is e.perp_dot(p - a) > 0; test the box corner that is most inside.
        let px = if e.y > 0.0 { bx } else { bx + 1.0 };
        let py = if e.x > 0.0 { by + 1.0 } else { by };
        if e.perp_dot(Vec2::new(px, py) - a) <= EPS {
            return false;
        }
    }
    true
}
