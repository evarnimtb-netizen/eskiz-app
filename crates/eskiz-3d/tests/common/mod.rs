#![allow(dead_code)]
//! Test fixtures: a generated cube as glTF and OBJ text, and a flat quad model.

use eskiz_3d::{Model, SubMesh, TextureSet};
use glam::{Vec2, Vec3};

#[derive(Clone, Copy, PartialEq)]
pub enum UvMode {
    /// Six distinct cells in a 3x2 atlas.
    Atlas,
    /// Every face uses the same cell (overlapping UVs).
    Overlap,
    /// Atlas shifted so some UVs exceed 1.
    Repeating,
    /// No UV attribute at all.
    None,
}

pub struct CubeData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
}

/// Unit cube centred on the origin. Face 0 is +Z, face 1 is -Z.
pub fn cube(mode: UvMode) -> CubeData {
    // (normal, right axis, up axis)
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0., 0., 1.], [1., 0., 0.], [0., 1., 0.]),
        ([0., 0., -1.], [-1., 0., 0.], [0., 1., 0.]),
        ([1., 0., 0.], [0., 0., -1.], [0., 1., 0.]),
        ([-1., 0., 0.], [0., 0., 1.], [0., 1., 0.]),
        ([0., 1., 0.], [1., 0., 0.], [0., 0., -1.]),
        ([0., -1., 0.], [1., 0., 0.], [0., 0., 1.]),
    ];
    let mut d = CubeData {
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        indices: vec![],
    };
    for (f, (n, r, u)) in faces.iter().enumerate() {
        let base = d.positions.len() as u16;
        let (col, row) = match mode {
            UvMode::Overlap => (0, 0),
            _ => (f % 3, f / 3),
        };
        let shift = if mode == UvMode::Repeating { 0.3 } else { 0.0 };
        let (u0, u1) = (
            col as f32 / 3.0 + 0.02 + shift,
            (col + 1) as f32 / 3.0 - 0.02 + shift,
        );
        let (v0, v1) = (row as f32 / 2.0 + 0.02, (row + 1) as f32 / 2.0 - 0.02);
        // Corners: bottom-left, bottom-right, top-right, top-left.
        for (sx, sy, uu, vv) in [
            (-1., -1., u0, v1),
            (1., -1., u1, v1),
            (1., 1., u1, v0),
            (-1., 1., u0, v0),
        ] {
            let p: Vec<f32> = (0..3)
                .map(|k| 0.5 * n[k] + 0.5 * sx * r[k] + 0.5 * sy * u[k])
                .collect();
            d.positions.push([p[0], p[1], p[2]]);
            d.normals.push(*n);
            d.uvs.push([uu, vv]);
        }
        d.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    d
}

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// glTF JSON (embedded base64 buffer) for a cube with one material. `mode` is the primitive mode (4 = triangles, 0 = points).
pub fn cube_gltf(uv: UvMode, prim_mode: u32) -> String {
    let d = cube(uv);
    let mut buf = Vec::new();
    let mut views = Vec::new();
    let mut accs = Vec::new();
    let mut add = |bytes: Vec<u8>, ctype: u32, count: usize, ty: &str, extra: &str| -> usize {
        views.push(format!(
            r#"{{"buffer":0,"byteOffset":{},"byteLength":{}}}"#,
            buf.len(),
            bytes.len()
        ));
        buf.extend(bytes);
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
        accs.push(format!(
            r#"{{"bufferView":{},"componentType":{ctype},"count":{count},"type":"{ty}"{extra}}}"#,
            views.len() - 1
        ));
        accs.len() - 1
    };
    let fb = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let pos: Vec<f32> = d.positions.iter().flatten().copied().collect();
    let p = add(
        fb(&pos),
        5126,
        d.positions.len(),
        "VEC3",
        r#","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]"#,
    );
    let nrm: Vec<f32> = d.normals.iter().flatten().copied().collect();
    let n = add(fb(&nrm), 5126, d.normals.len(), "VEC3", "");
    let mut attrs = format!(r#""POSITION":{p},"NORMAL":{n}"#);
    if uv != UvMode::None {
        let t: Vec<f32> = d.uvs.iter().flatten().copied().collect();
        let a = add(fb(&t), 5126, d.uvs.len(), "VEC2", "");
        attrs += &format!(r#","TEXCOORD_0":{a}"#);
    }
    let idx: Vec<u8> = d.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let i = add(idx, 5123, d.indices.len(), "SCALAR", "");
    format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0}}],
"meshes":[{{"name":"cube","primitives":[{{"attributes":{{{attrs}}},"indices":{i},"mode":{prim_mode},"material":0}}]}}],
"materials":[{{"name":"red","pbrMetallicRoughness":{{"baseColorFactor":[1,0,0,1],"metallicFactor":0.25,"roughnessFactor":0.5}}}}],
"accessors":[{}],"bufferViews":[{}],
"buffers":[{{"byteLength":{},"uri":"data:application/octet-stream;base64,{}"}}]}}"#,
        accs.join(","),
        views.join(","),
        buf.len(),
        b64(&buf)
    )
}

/// glTF with a scene but no mesh.
pub fn empty_gltf() -> String {
    r#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"name":"empty"}]}"#
        .to_string()
}

/// OBJ text for the cube (v is up in OBJ, so v is flipped back from the top-left convention).
pub fn cube_obj(uv: UvMode, with_mtl: bool) -> String {
    let d = cube(uv);
    let mut s = String::new();
    if with_mtl {
        s += "mtllib cube.mtl\n";
    }
    s += "o cube\n";
    for p in &d.positions {
        s += &format!("v {} {} {}\n", p[0], p[1], p[2]);
    }
    if uv != UvMode::None {
        for t in &d.uvs {
            s += &format!("vt {} {}\n", t[0], 1.0 - t[1]);
        }
    }
    if with_mtl {
        s += "usemtl blue\n";
    }
    for t in d.indices.chunks(3) {
        let f = |i: u16| {
            let i = i + 1;
            if uv != UvMode::None {
                format!("{i}/{i}")
            } else {
                format!("{i}")
            }
        };
        s += &format!("f {} {} {}\n", f(t[0]), f(t[1]), f(t[2]));
    }
    s
}

pub const CUBE_MTL: &str = "newmtl blue\nKd 0 0 1\nNs 100\n";

/// A flat quad in the z = 0 plane from (-1,-1) to (1,1), UVs spanning `uv_min..uv_max` (top-left origin).
pub fn quad_model(uv_min: Vec2, uv_max: Vec2, size: usize) -> Model {
    let sm = SubMesh {
        name: "quad".into(),
        positions: vec![
            Vec3::new(-1., -1., 0.),
            Vec3::new(1., -1., 0.),
            Vec3::new(1., 1., 0.),
            Vec3::new(-1., 1., 0.),
        ],
        normals: vec![Vec3::Z; 4],
        uvs: vec![
            Vec2::new(uv_min.x, uv_max.y),
            Vec2::new(uv_max.x, uv_max.y),
            Vec2::new(uv_max.x, uv_min.y),
            Vec2::new(uv_min.x, uv_min.y),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        material: 0,
    };
    Model::new(
        vec![sm],
        vec![TextureSet::from_constants(
            "quad",
            size,
            size,
            [255, 255, 255, 0],
            0.5,
            0.0,
        )],
        0,
    )
}

/// One triangle with one 1x1 material, for the USDA golden test. UVs use a quarter-height `v` so the flip shows.
pub fn triangle_model() -> Model {
    let sm = SubMesh {
        name: "tri".into(),
        positions: vec![Vec3::new(0., 0., 0.), Vec3::new(1., 0., 0.), Vec3::new(0., 1., 0.)],
        normals: vec![Vec3::Z; 3],
        uvs: vec![Vec2::new(0., 1.), Vec2::new(1., 1.), Vec2::new(0., 0.25)],
        indices: vec![0, 1, 2],
        material: 0,
    };
    Model::new(
        vec![sm],
        vec![TextureSet::from_constants("Skin", 1, 1, [200, 100, 50, 255], 0.5, 0.0)],
        0,
    )
}

/// The cube from [`cube`] (atlas UVs) as a model with `sets` materials named by `names`; faces alternate materials.
/// Every map of every set holds a distinct, non-uniform pattern so unpacked export is checkable.
pub fn cube_model(names: &[&str], size: usize) -> Model {
    let c = cube(UvMode::Atlas);
    let mut subs: Vec<SubMesh> = Vec::new();
    for f in 0..6usize {
        let r = f * 4..f * 4 + 4;
        let base = (f * 4) as u16;
        subs.push(SubMesh {
            name: format!("face{f}"),
            positions: c.positions[r.clone()].iter().map(|p| Vec3::from(*p)).collect(),
            normals: c.normals[r.clone()].iter().map(|p| Vec3::from(*p)).collect(),
            uvs: c.uvs[r].iter().map(|p| Vec2::from(*p)).collect(),
            indices: c.indices[f * 6..f * 6 + 6].iter().map(|i| u32::from(i - base)).collect(),
            material: f % names.len(),
        });
    }
    let mut sets: Vec<TextureSet> = names
        .iter()
        .map(|n| TextureSet::from_constants(n, size, size, [255, 255, 255, 255], 0.5, 0.0))
        .collect();
    for (si, set) in sets.iter_mut().enumerate() {
        for (ki, kind) in eskiz_3d::MapKind::ALL.iter().enumerate() {
            let mut px = Vec::with_capacity(size * size * 4);
            for y in 0..size {
                for x in 0..size {
                    px.extend([
                        (x * 40 + ki * 7) as u8,
                        (y * 40 + si * 11) as u8,
                        (x * 13 + y * 29 + ki * 3) as u8,
                        255,
                    ]);
                }
            }
            *set.map_mut(*kind) = eskiz_core::Layer::from_rgba(size, size, px);
        }
    }
    Model::new(subs, sets, 0)
}

/// A flat quad with a 4x4 asymmetric texture: a red texel at the top-left, a green texel at the top-right, a
/// blue texel at the bottom-left, everything else white. UVs cover the whole image, top-left origin.
pub fn asymmetric_quad() -> Model {
    let mut m = quad_model(Vec2::ZERO, Vec2::ONE, 4);
    let mut px = [255u8; 4 * 4 * 4].to_vec();
    let mut put = |x: usize, y: usize, c: [u8; 3]| px[(y * 4 + x) * 4..(y * 4 + x) * 4 + 3].copy_from_slice(&c);
    put(0, 0, [255, 0, 0]);
    put(3, 0, [0, 255, 0]);
    put(0, 3, [0, 0, 255]);
    *m.texture_sets[0].map_mut(eskiz_3d::MapKind::Colour) = eskiz_core::Layer::from_rgba(4, 4, px);
    m
}
