//! USD export: a USDA text root layer plus a stored, 64-byte-aligned USDZ package.
//!
//! Layout written by [`write_usda`]:
//!
//! ```text
//! /Model                      Xform, kind "component"; defaultPrim, upAxis Y, metersPerUnit 1
//! /Model/<name>_Mesh          Mesh, one per material (submeshes merged), subdivisionScheme "none",
//!                             vertex normals, primvars:st with t = 1 - v, bound to the material
//! /Model/Looks                Scope
//! /Model/Looks/<name>         Material -> PBR (UsdPreviewSurface), StReader (UsdPrimvarReader_float2) and one
//!                             UsdUVTexture per map: Colour (sRGB, rgb), Roughness/Metallic/AO (raw, r),
//!                             Normal (raw, rgb, scale 2 / bias -1)
//! ```
//!
//! Eskiz's UV origin is the top-left texel corner with `v` growing down. USD's `st` origin is the bottom-left of the
//! image, so `t = 1 - v`.

pub mod zip;
#[cfg(feature = "usd")]
pub mod import;

use std::fmt::Write as _;

use glam::Vec3;

use crate::export::sanitise_name;
use crate::model::{MapKind, Model, SubMesh};

/// Format of the root layer inside the package.
///
/// Only `.usda` is written. An early spike found no evidence that Apple tooling rejects a
/// `.usda` root, so the `openusd` binary (`.usdc`) writer was not adopted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RootFormat {
    #[default]
    Usda,
}

impl RootFormat {
    /// File name of the root layer in the package.
    pub fn file_name(self) -> &'static str {
        match self {
            RootFormat::Usda => "model.usda",
        }
    }
}

/// Make `s` a valid USD prim identifier: ASCII letters, digits and `_`, not starting with a digit.
fn identifier(s: &str) -> String {
    let mut out: String = sanitise_name(s)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

/// One identifier per texture set; collisions get a numeric suffix. Used for prim names and texture file names.
pub fn material_idents(model: &Model) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for s in &model.texture_sets {
        let base = identifier(&s.name);
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

/// Path of a map's PNG inside the package.
pub fn texture_entry_name(ident: &str, kind: MapKind) -> String {
    format!("textures/{ident}_{}.png", kind.file_stem())
}

/// Materials that have at least one triangle, in texture-set order.
pub fn used_materials(model: &Model) -> Vec<usize> {
    (0..model.texture_sets.len())
        .filter(|&m| {
            model
                .submeshes
                .iter()
                .any(|s| s.material == m && s.triangle_count() > 0)
        })
        .collect()
}

fn fmt_f(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "0".to_owned()
    }
}

fn fmt_v3(v: Vec3) -> String {
    format!("({}, {}, {})", fmt_f(v.x), fmt_f(v.y), fmt_f(v.z))
}

fn join<T>(items: impl Iterator<Item = T>, f: impl Fn(T) -> String) -> String {
    let mut s = String::new();
    for (i, it) in items.enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&f(it));
    }
    s
}

/// Merge a material's submeshes into one set of vertex arrays (indices offset).
struct Merged {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<glam::Vec2>,
    indices: Vec<u32>,
}

fn merge(parts: &[&SubMesh]) -> Merged {
    let mut m = Merged {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
    };
    let all_normals = parts.iter().all(|p| p.normals.len() == p.positions.len());
    let all_uvs = parts.iter().all(|p| p.uvs.len() == p.positions.len());
    for p in parts {
        let base = m.positions.len() as u32;
        m.positions.extend_from_slice(&p.positions);
        if all_normals {
            m.normals.extend_from_slice(&p.normals);
        }
        if all_uvs {
            m.uvs.extend_from_slice(&p.uvs);
        }
        let tris = p.triangle_count() * 3;
        m.indices.extend(p.indices[..tris].iter().map(|i| i + base));
    }
    m
}

/// Write the USDA root layer for `model`. Texture asset paths are `textures/<name>_<map>.png`, relative to the
/// layer (they live in the same package; see [`texture_entry_name`]). Submeshes without UVs are written without an
/// `st` primvar; [`crate::export::write_usdz_bytes`] rejects such models before calling this.
pub fn write_usda(model: &Model) -> String {
    let idents = material_idents(model);
    let mut s = String::new();
    s.push_str("#usda 1.0\n(\n    defaultPrim = \"Model\"\n    upAxis = \"Y\"\n    metersPerUnit = 1\n");
    s.push_str("    doc = \"Exported from Eskiz\"\n)\n\n");
    s.push_str("def Xform \"Model\" (\n    kind = \"component\"\n)\n{\n");
    let used = used_materials(model);
    for &mat in &used {
        let parts: Vec<&SubMesh> = model
            .submeshes
            .iter()
            .filter(|sm| sm.material == mat && sm.triangle_count() > 0)
            .collect();
        let m = merge(&parts);
        let id = &idents[mat];
        let (lo, hi) = m.positions.iter().fold((m.positions[0], m.positions[0]), |(l, h), p| {
            (l.min(*p), h.max(*p))
        });
        let _ = writeln!(s, "    def Mesh \"{id}_Mesh\" (");
        s.push_str("        prepend apiSchemas = [\"MaterialBindingAPI\"]\n    )\n    {\n");
        let _ = writeln!(s, "        float3[] extent = [{}, {}]", fmt_v3(lo), fmt_v3(hi));
        let _ = writeln!(s, "        int[] faceVertexCounts = [{}]", join((0..m.indices.len() / 3).map(|_| 3), |c| c.to_string()));
        let _ = writeln!(s, "        int[] faceVertexIndices = [{}]", join(m.indices.iter(), |i| i.to_string()));
        let _ = writeln!(s, "        point3f[] points = [{}]", join(m.positions.iter(), |p| fmt_v3(*p)));
        if m.normals.len() == m.positions.len() {
            let _ = writeln!(s, "        normal3f[] normals = [{}] (", join(m.normals.iter(), |p| fmt_v3(*p)));
            s.push_str("            interpolation = \"vertex\"\n        )\n");
        }
        if m.uvs.len() == m.positions.len() {
            let st = join(m.uvs.iter(), |uv| format!("({}, {})", fmt_f(uv.x), fmt_f(1.0 - uv.y)));
            let _ = writeln!(s, "        texCoord2f[] primvars:st = [{st}] (");
            s.push_str("            interpolation = \"vertex\"\n        )\n");
        }
        s.push_str("        uniform bool doubleSided = true\n");
        s.push_str("        uniform token orientation = \"rightHanded\"\n");
        s.push_str("        uniform token subdivisionScheme = \"none\"\n");
        let _ = writeln!(s, "        rel material:binding = </Model/Looks/{id}>");
        s.push_str("    }\n\n");
    }
    s.push_str("    def Scope \"Looks\"\n    {\n");
    for &mat in &used {
        let set = &model.texture_sets[mat];
        let id = &idents[mat];
        let base = format!("/Model/Looks/{id}");
        let translucent = set.map(MapKind::Colour).to_rgba().as_chunks::<4>().0.iter().any(|p| p[3] < 255);
        let _ = writeln!(s, "        def Material \"{id}\"\n        {{");
        let _ = writeln!(s, "            token outputs:surface.connect = <{base}/PBR.outputs:surface>\n");
        s.push_str("            def Shader \"PBR\"\n            {\n");
        s.push_str("                uniform token info:id = \"UsdPreviewSurface\"\n");
        let _ = writeln!(s, "                color3f inputs:diffuseColor.connect = <{base}/Colour.outputs:rgb>");
        if translucent {
            let _ = writeln!(s, "                float inputs:opacity.connect = <{base}/Colour.outputs:a>");
        }
        let _ = writeln!(s, "                float inputs:roughness.connect = <{base}/Roughness.outputs:r>");
        let _ = writeln!(s, "                float inputs:metallic.connect = <{base}/Metallic.outputs:r>");
        let _ = writeln!(s, "                float inputs:occlusion.connect = <{base}/AO.outputs:r>");
        let _ = writeln!(s, "                normal3f inputs:normal.connect = <{base}/Normal.outputs:rgb>");
        s.push_str("                token outputs:surface\n            }\n\n");
        s.push_str("            def Shader \"StReader\"\n            {\n");
        s.push_str("                uniform token info:id = \"UsdPrimvarReader_float2\"\n");
        s.push_str("                token inputs:varname = \"st\"\n");
        s.push_str("                float2 outputs:result\n            }\n");
        let maps = [
            (MapKind::Colour, "Colour", "sRGB", "float3 outputs:rgb"),
            (MapKind::Roughness, "Roughness", "raw", "float outputs:r"),
            (MapKind::Metallic, "Metallic", "raw", "float outputs:r"),
            (MapKind::Normal, "Normal", "raw", "float3 outputs:rgb"),
            (MapKind::AmbientOcclusion, "AO", "raw", "float outputs:r"),
        ];
        for (kind, shader, space, out) in maps {
            let _ = writeln!(s, "\n            def Shader \"{shader}\"\n            {{");
            s.push_str("                uniform token info:id = \"UsdUVTexture\"\n");
            let _ = writeln!(s, "                asset inputs:file = @{}@", texture_entry_name(id, kind));
            let _ = writeln!(s, "                float2 inputs:st.connect = <{base}/StReader.outputs:result>");
            let _ = writeln!(s, "                token inputs:sourceColorSpace = \"{space}\"");
            s.push_str("                token inputs:wrapS = \"repeat\"\n                token inputs:wrapT = \"repeat\"\n");
            if kind == MapKind::Normal {
                s.push_str("                float4 inputs:scale = (2, 2, 2, 1)\n");
                s.push_str("                float4 inputs:bias = (-1, -1, -1, 0)\n");
            }
            if kind == MapKind::Colour && translucent {
                s.push_str("                float outputs:a\n");
            }
            let _ = writeln!(s, "                {out}\n            }}");
        }
        s.push_str("        }\n");
    }
    s.push_str("    }\n}\n");
    s
}
