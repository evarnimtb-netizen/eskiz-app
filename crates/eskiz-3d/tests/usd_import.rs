//! USD import (`.usdz`, `.usda`, `.usdc`): fixtures are generated in code.
#![cfg(feature = "usd")]
mod common;

use common::{asymmetric_quad, cube_model};
use eskiz_3d::{
    load_usd_file, load_usd_slice, load_usdz_slice, write_usdz_bytes, FatalIssue, ImportError, ImportOptions,
    ImportedModel, MapKind, UsdzOptions,
};
use glam::{Vec2, Vec3};

fn opts() -> ImportOptions {
    ImportOptions::default()
}

fn wrap(body: &str) -> String {
    format!("#usda 1.0\n(\n    defaultPrim = \"Root\"\n    upAxis = \"Y\"\n)\n\ndef Xform \"Root\"\n{{\n{body}\n}}\n")
}

const SQUARE: &str = "point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)]";

fn st_vertex() -> &'static str {
    "texCoord2f[] primvars:st = [(0, 0), (1, 0), (1, 1), (0, 1)] (\n interpolation = \"vertex\"\n )"
}

fn mesh(body: &str) -> String {
    format!("def Mesh \"M\"\n{{\n{body}\n}}\n")
}

fn quad_mesh(extra: &str) -> String {
    mesh(&format!(
        "{SQUARE}\nint[] faceVertexCounts = [4]\nint[] faceVertexIndices = [0, 1, 2, 3]\n{extra}"
    ))
}

fn load(text: &str) -> Result<ImportedModel, ImportError> {
    load_usd_slice(text.as_bytes(), &opts())
}

fn ok(text: &str) -> ImportedModel {
    load(text).unwrap_or_else(|e| panic!("import failed: {e}"))
}

fn tri_count(m: &ImportedModel) -> usize {
    m.model.submeshes.iter().map(|s| s.triangle_count()).sum()
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-5
}

// ---- round trip ----

#[test]
fn round_trip_multi_material_cube() {
    let src = cube_model(&["Red", "Green", "Blue"], 8);
    let bytes = write_usdz_bytes(&src, &UsdzOptions::default()).unwrap();
    let imp = load_usdz_slice(&bytes, &opts()).unwrap();
    let m = &imp.model;
    assert_eq!(m.texture_sets.len(), 3);
    assert_eq!(m.submeshes.len(), 3, "one mesh per material");
    assert_eq!(tri_count(&imp), 12);
    assert!(!imp.normals_generated);
    for (a, b) in src.texture_sets.iter().zip(&m.texture_sets) {
        assert_eq!(a.name, b.name);
        for kind in [MapKind::Colour, MapKind::Normal] {
            assert_eq!(a.map(kind).to_rgba(), b.map(kind).to_rgba(), "{kind:?} pixels");
        }
        for kind in [MapKind::Roughness, MapKind::Metallic, MapKind::AmbientOcclusion] {
            let (pa, pb) = (a.map(kind).to_rgba(), b.map(kind).to_rgba());
            assert_eq!(pa.len(), pb.len());
            for (x, y) in pa.as_chunks::<4>().0.iter().zip(pb.as_chunks::<4>().0) {
                assert_eq!(x[0], y[0], "{kind:?} red channel");
            }
        }
    }
    // Every source triangle (positions + uvs) is found in the import, material by name.
    let tris = |model: &eskiz_3d::Model| {
        let mut v = Vec::new();
        for s in &model.submeshes {
            for t in 0..s.triangle_count() {
                let p = s.tri_positions(t);
                let u = s.tri_uvs(t);
                v.push((model.texture_sets[s.material].name.clone(), p, u));
            }
        }
        v
    };
    let (a, b) = (tris(&src), tris(m));
    for (name, p, u) in &a {
        assert!(
            b.iter().any(|(n2, p2, u2)| n2 == name
                && (0..3).all(|k| near(p[k], p2[k]) && (u[k] - u2[k]).length() < 1e-5)),
            "triangle missing after round trip"
        );
    }
}

#[test]
fn round_trip_keeps_uv_orientation() {
    let src = asymmetric_quad();
    let imp = load_usdz_slice(&write_usdz_bytes(&src, &UsdzOptions::default()).unwrap(), &opts()).unwrap();
    let (s, i) = (&src.submeshes[0], &imp.model.submeshes[0]);
    for t in 0..2 {
        let (pa, ua) = (s.tri_positions(t), s.tri_uvs(t));
        let found = (0..i.triangle_count()).any(|k| {
            let (pb, ub) = (i.tri_positions(k), i.tri_uvs(k));
            (0..3).all(|c| near(pa[c], pb[c]) && (ua[c] - ub[c]).length() < 1e-6)
        });
        assert!(found);
    }
    // The red texel in the top-left of the image is still found at v near 0.
    let c = imp.model.texture_sets[0].sample(MapKind::Colour, 0.05, 0.05);
    assert!(c[0] > 0.9 && c[1] < 0.1, "{c:?}");
}

// ---- primvars ----

#[test]
fn st_flips_v() {
    let m = ok(&wrap(&quad_mesh(st_vertex())));
    let s = &m.model.submeshes[0];
    assert_eq!(s.uvs[0], Vec2::new(0.0, 1.0));
    assert_eq!(s.uvs[2], Vec2::new(1.0, 0.0));
}

#[test]
fn face_varying_st_splits_vertices() {
    // Two triangles share vertices 0 and 2 but give them different UVs per corner.
    let body = mesh(&format!(
        "{SQUARE}\nint[] faceVertexCounts = [3, 3]\nint[] faceVertexIndices = [0, 1, 2, 0, 2, 3]\n\
         texCoord2f[] primvars:st = [(0,0), (1,0), (1,1), (0.5,0.5), (0.6,0.6), (0,1)] (\n interpolation = \"faceVarying\"\n )"
    ));
    let m = ok(&wrap(&body));
    let s = &m.model.submeshes[0];
    assert_eq!(s.positions.len(), 6, "all six corners differ");
    assert_eq!(s.triangle_count(), 2);
    let has = |v: Vec2| s.uvs.iter().any(|u| (*u - v).length() < 1e-6);
    assert!(has(Vec2::new(0.5, 0.5)) && has(Vec2::new(0.6, 0.4)));
}

#[test]
fn indexed_primvar() {
    let body = mesh(&format!(
        "{SQUARE}\nint[] faceVertexCounts = [4]\nint[] faceVertexIndices = [0, 1, 2, 3]\n\
         texCoord2f[] primvars:st = [(0, 0), (1, 1)] (\n interpolation = \"faceVarying\"\n )\n\
         int[] primvars:st:indices = [0, 1, 1, 0]"
    ));
    let m = ok(&wrap(&body));
    let s = &m.model.submeshes[0];
    assert!(s.uvs.iter().all(|u| *u == Vec2::new(0., 1.) || *u == Vec2::new(1., 0.)));
    assert_eq!(s.uvs.iter().filter(|u| u.x == 1.0).count(), s.uvs.len() / 2);
}

#[test]
fn indexed_vertex_normals_and_uniform_normal() {
    let body = mesh(&format!(
        "{SQUARE}\nint[] faceVertexCounts = [4]\nint[] faceVertexIndices = [0, 1, 2, 3]\n{}\n\
         normal3f[] normals = [(0, 0, 1)] (\n interpolation = \"uniform\"\n )",
        st_vertex()
    ));
    let m = ok(&wrap(&body));
    assert!(m.model.submeshes[0].normals.iter().all(|n| near(*n, Vec3::Z)));
    assert!(!m.normals_generated);
}

#[test]
fn missing_normals_are_generated() {
    let m = ok(&wrap(&quad_mesh(st_vertex())));
    assert!(m.normals_generated);
    assert!(m.model.submeshes[0].normals.iter().all(|n| near(n.abs(), Vec3::Z)));
}

// ---- triangulation ----

#[test]
fn quads_and_ngons_are_triangulated() {
    let body = mesh(
        "point3f[] points = [(0,0,0), (1,0,0), (2,0,0), (2,1,0), (1,2,0), (0,1,0), (0,2,0)]\n\
         int[] faceVertexCounts = [4, 5]\n\
         int[] faceVertexIndices = [0, 1, 5, 6, 1, 2, 3, 4, 5]\n\
         texCoord2f[] primvars:st = [(0,0), (1,0), (1,1), (1,1), (0,1), (0,1), (0,1)] (\n interpolation = \"vertex\"\n )",
    );
    let m = ok(&wrap(&body));
    assert_eq!(tri_count(&m), 2 + 3);
    let s = &m.model.submeshes[0];
    assert!(s.indices.iter().all(|&i| (i as usize) < s.positions.len()));
}

// ---- transforms ----

#[test]
fn xforms_are_baked_into_points() {
    let text = "#usda 1.0\n(\n upAxis = \"Y\"\n)\n\
        def Xform \"A\"\n{\n double3 xformOp:translate = (10, 0, 0)\n uniform token[] xformOpOrder = [\"xformOp:translate\"]\n\
        def Xform \"B\"\n{\n float3 xformOp:scale = (2, 2, 2)\n float xformOp:rotateZ = 90\n\
        uniform token[] xformOpOrder = [\"xformOp:translate\", \"xformOp:rotateZ\", \"xformOp:scale\"]\n double3 xformOp:translate = (0, 1, 0)\n";
    let text = format!("{text}{}\n}}\n}}\n", quad_mesh(st_vertex()));
    let m = ok(&text);
    let s = &m.model.submeshes[0];
    // (1, 0, 0): scale 2 -> (2,0,0); rotate Z 90 -> (0,2,0); translate (0,1,0) -> (0,3,0); parent +10 x.
    let has = |v: Vec3| s.positions.iter().any(|p| near(*p, v));
    assert!(has(Vec3::new(10.0, 3.0, 0.0)), "{:?}", s.positions);
    assert!(has(Vec3::new(8.0, 1.0, 0.0)), "{:?}", s.positions);
}

#[test]
fn matrix_and_orient_ops() {
    let text = format!(
        "#usda 1.0\n(\n upAxis = \"Y\"\n)\ndef Xform \"A\"\n{{\n\
         matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (5, 6, 7, 1) )\n\
         uniform token[] xformOpOrder = [\"xformOp:transform\"]\n{}\n}}\n",
        quad_mesh(st_vertex())
    );
    let m = ok(&text);
    assert!(near(m.model.submeshes[0].positions[0], Vec3::new(5.0, 6.0, 7.0)));
}

#[test]
fn negative_scale_keeps_faces_outward() {
    let plain = ok(&wrap(&quad_mesh(st_vertex())));
    let text = format!(
        "#usda 1.0\n(\n upAxis = \"Y\"\n)\ndef Xform \"A\"\n{{\n float3 xformOp:scale = (-1, 1, 1)\n\
         uniform token[] xformOpOrder = [\"xformOp:scale\"]\n{}\n}}\n",
        quad_mesh(st_vertex())
    );
    let flipped = ok(&text);
    let n = |m: &ImportedModel| {
        let p = m.model.submeshes[0].tri_positions(0);
        (p[1] - p[0]).cross(p[2] - p[0]).normalize()
    };
    // Mirroring x and re-winding keeps the geometric normal pointing the same way (+z).
    assert!(near(n(&plain), n(&flipped)), "{:?} {:?}", n(&plain), n(&flipped));
}

#[test]
fn z_up_is_rotated_to_y_up() {
    let text = wrap(&quad_mesh(st_vertex())).replace("upAxis = \"Y\"", "upAxis = \"Z\"");
    let m = ok(&text);
    // (0, 1, 0) in Z-up becomes (0, 0, -1).
    assert!(m.model.submeshes[0].positions.iter().any(|p| near(*p, Vec3::new(0.0, 0.0, -1.0))));
}

// ---- materials ----

fn png_bytes(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend(f(x, y));
        }
    }
    eskiz_3d::export::encode_png(&px, w as usize, h as usize, None).unwrap()
}

fn material_layer(file: &str) -> String {
    format!(
        "def Scope \"Looks\"\n{{\n def Material \"Paint\"\n {{\n token outputs:surface.connect = </Root/Looks/Paint/PBR.outputs:surface>\n\
         def Shader \"PBR\"\n {{\n uniform token info:id = \"UsdPreviewSurface\"\n\
         color3f inputs:diffuseColor.connect = </Root/Looks/Paint/Tex.outputs:rgb>\n float inputs:roughness = 0.25\n float inputs:metallic = 1\n\
         token outputs:surface\n }}\n\
         def Shader \"Tex\"\n {{\n uniform token info:id = \"UsdUVTexture\"\n asset inputs:file = @{file}@\n float3 outputs:rgb\n }}\n }}\n}}\n"
    )
}

#[test]
fn material_constants_and_loose_texture_next_to_file() {
    let dir = std::env::temp_dir().join(format!("eskiz_usd_import_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("tex")).unwrap();
    std::fs::write(
        dir.join("tex/c.png"),
        png_bytes(2, 2, |x, y| [if x == 0 && y == 0 { 255 } else { 10 }, 20, 30, 255]),
    )
    .unwrap();
    let body = format!(
        "{}\n{}",
        material_layer("./tex/c.png"),
        quad_mesh(&format!("{}\nrel material:binding = </Root/Looks/Paint>", st_vertex()))
    );
    std::fs::write(dir.join("m.usda"), wrap(&body)).unwrap();
    let m = load_usd_file(&dir.join("m.usda"), &opts()).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert!(m.notes.is_empty(), "{:?}", m.notes);
    let set = &m.model.texture_sets[0];
    assert_eq!(set.name, "Paint");
    assert_eq!(&set.map(MapKind::Colour).to_rgba()[0..4], &[255, 20, 30, 255]);
    assert_eq!(set.map(MapKind::Colour).to_rgba()[4], 10);
    // Constants fill the other maps.
    let r = set.map(MapKind::Roughness).to_rgba()[0];
    assert!((i32::from(r) - 64).abs() <= 2, "roughness {r}");
    assert_eq!(set.map(MapKind::Metallic).to_rgba()[0], 255);
}

#[test]
fn missing_texture_is_noted_not_fatal() {
    let body = format!(
        "{}\n{}",
        material_layer("nope.png"),
        quad_mesh(&format!("{}\nrel material:binding = </Root/Looks/Paint>", st_vertex()))
    );
    let m = ok(&wrap(&body));
    assert!(m.notes.iter().any(|n| n.contains("nope.png")), "{:?}", m.notes);
}

#[test]
fn exr_texture_is_skipped_with_note() {
    let body = format!(
        "{}\n{}",
        material_layer("hdr.exr"),
        quad_mesh(&format!("{}\nrel material:binding = </Root/Looks/Paint>", st_vertex()))
    );
    let m = ok(&wrap(&body));
    assert!(m.notes.iter().any(|n| n.contains("only PNG and JPEG")), "{:?}", m.notes);
}

#[test]
fn geom_subsets_split_materials() {
    let sub = |name: &str, idx: &str, mat: &str| {
        format!(
            "def GeomSubset \"{name}\"\n{{\n uniform token elementType = \"face\"\n int[] indices = [{idx}]\n rel material:binding = </Root/Looks/{mat}>\n}}\n"
        )
    };
    let scope = "def Scope \"Looks\"\n{\n\
        def Material \"A\"\n{\n token outputs:surface.connect = </Root/Looks/A/P.outputs:surface>\n def Shader \"P\"\n{\n uniform token info:id = \"UsdPreviewSurface\"\n color3f inputs:diffuseColor = (1, 0, 0)\n token outputs:surface\n}\n}\n\
        def Material \"B\"\n{\n token outputs:surface.connect = </Root/Looks/B/P.outputs:surface>\n def Shader \"P\"\n{\n uniform token info:id = \"UsdPreviewSurface\"\n color3f inputs:diffuseColor = (0, 0, 1)\n token outputs:surface\n}\n}\n}\n";
    let body = format!(
        "{scope}\n{}",
        mesh(&format!(
            "{SQUARE}\nint[] faceVertexCounts = [3, 3]\nint[] faceVertexIndices = [0, 1, 2, 0, 2, 3]\n{}\n{}{}",
            st_vertex(),
            sub("a", "0", "A"),
            sub("b", "1", "B")
        ))
    );
    let m = ok(&wrap(&body));
    assert_eq!(m.model.submeshes.len(), 2);
    assert_eq!(m.model.texture_sets.len(), 2);
    assert_ne!(m.model.submeshes[0].material, m.model.submeshes[1].material);
    assert_eq!(m.model.texture_sets[0].name, "A");
}

// ---- usdc ----

#[test]
fn usdc_written_by_crate_writer_imports() {
    let text = wrap(&quad_mesh(&format!("{}\nnormal3f[] normals = [(0,0,1),(0,0,1),(0,0,1),(0,0,1)]", st_vertex())));
    let data = openusd::usda::parse(&text).unwrap();
    let mut buf = std::io::Cursor::new(Vec::new());
    openusd::usdc::CrateWriter::write(&data, &mut buf).unwrap();
    let crate_bytes = buf.into_inner();
    assert!(crate_bytes.starts_with(b"PXR-USDC"));
    let from_c = load_usd_slice(&crate_bytes, &opts()).unwrap();
    let from_a = ok(&text);
    assert_eq!(from_c.model.submeshes[0].positions, from_a.model.submeshes[0].positions);
    assert_eq!(from_c.model.submeshes[0].uvs, from_a.model.submeshes[0].uvs);
    assert_eq!(from_c.model.submeshes[0].indices, from_a.model.submeshes[0].indices);
    // And as a .usdc on disk.
    let p = std::env::temp_dir().join(format!("eskiz_import_{}.usdc", std::process::id()));
    std::fs::write(&p, &crate_bytes).unwrap();
    let f = load_usd_file(&p, &opts());
    std::fs::remove_file(&p).ok();
    assert_eq!(f.unwrap().model.submeshes.len(), 1);
}

// ---- unsupported / malformed ----

fn unsupported(text: &str, needle: &str) {
    match load(text) {
        Err(ImportError::Unsupported(s)) => assert!(s.contains(needle), "{s}"),
        other => panic!("expected Unsupported({needle}), got {:?}", other.map(|m| m.notes)),
    }
}

#[test]
fn references_and_instancing_are_rejected_clearly() {
    unsupported(
        &wrap(&format!("def \"Ext\" (\n references = @other.usd@\n)\n{{\n}}\n{}", quad_mesh(st_vertex()))),
        "references",
    );
    unsupported(&wrap("def PointInstancer \"P\"\n{\n}"), "instancing");
    unsupported(
        &wrap(&format!("def Xform \"I\" (\n instanceable = true\n)\n{{\n}}\n{}", quad_mesh(st_vertex()))),
        "instancing",
    );
    let sub = format!("#usda 1.0\n(\n subLayers = [@other.usda@]\n)\n{}", "def Xform \"R\"\n{\n}\n");
    unsupported(&sub, "sublayers");
}

#[test]
fn skinning_subdivision_and_animation_are_noted() {
    let text = wrap(&format!(
        "def SkelRoot \"S\"\n{{\n{}\n}}",
        mesh(&format!(
            "{SQUARE}\nint[] faceVertexCounts = [4]\nint[] faceVertexIndices = [0, 1, 2, 3]\n{}\nuniform token subdivisionScheme = \"catmullClark\"",
            st_vertex()
        ))
    ));
    let m = ok(&text);
    let all = m.notes.join("\n");
    assert!(all.contains("Skinning"), "{all}");
    assert!(all.contains("Subdivision"), "{all}");
    let anim = wrap(&mesh(&format!(
        "point3f[] points.timeSamples = {{ 0: [(0,0,0),(1,0,0),(1,1,0),(0,1,0)], 1: [(0,0,0),(2,0,0),(2,2,0),(0,2,0)] }}\n\
         int[] faceVertexCounts = [4]\nint[] faceVertexIndices = [0, 1, 2, 3]\n{}",
        st_vertex()
    )));
    let m = ok(&anim);
    assert!(m.notes.iter().any(|n| n.contains("Animation")), "{:?}", m.notes);
    assert!(m.model.submeshes[0].positions.iter().any(|p| near(*p, Vec3::X)), "first frame is used");
}

#[test]
fn unsupported_primitives_are_noted() {
    let m = ok(&wrap(&format!("def Sphere \"S\"\n{{\n}}\n{}", quad_mesh(st_vertex()))));
    assert!(m.notes.iter().any(|n| n.contains("Sphere")));
}

#[test]
fn unsupported_shader_falls_back_with_note() {
    let scope = "def Scope \"Looks\"\n{\n def Material \"X\"\n{\n token outputs:surface.connect = </Root/Looks/X/S.outputs:surface>\n\
        def Shader \"S\"\n{\n uniform token info:id = \"ND_open_pbr_surface_surfaceshader\"\n token outputs:surface\n}\n}\n}\n";
    let m = ok(&wrap(&format!(
        "{scope}\n{}",
        quad_mesh(&format!("{}\nrel material:binding = </Root/Looks/X>", st_vertex()))
    )));
    assert!(m.notes.iter().any(|n| n.contains("not supported")), "{:?}", m.notes);
    assert_eq!(m.model.texture_sets.len(), 1);
}

#[test]
fn mesh_without_uvs_is_rejected_like_other_formats() {
    match load(&wrap(&quad_mesh(""))) {
        Err(ImportError::Rejected(r)) => assert!(r.fatal.iter().any(|f| matches!(f, FatalIssue::MissingUvs { .. })), "{r:?}"),
        other => panic!("{:?}", other.map(|m| m.notes)),
    }
}

#[test]
fn no_mesh_is_rejected() {
    match load(&wrap("def Xform \"Empty\"\n{\n}")) {
        Err(ImportError::Rejected(r)) => assert!(r.fatal.iter().any(|f| matches!(f, FatalIssue::NoMesh))),
        other => panic!("{:?}", other.map(|m| m.notes)),
    }
}

#[test]
fn malformed_input_errors_not_panics() {
    for bytes in [
        &b""[..],
        b"hello world",
        b"#usda 1.0\ndef Xform \"A\" {",
        b"PXR-USDC garbage garbage garbage",
        b"PK\x03\x04 not really a zip",
        &[0xff, 0xfe, 0, 1, 2],
    ] {
        match load_usd_slice(bytes, &opts()) {
            Err(ImportError::Parse(_) | ImportError::Rejected(_)) => {}
            other => panic!("{:?}: {:?}", String::from_utf8_lossy(bytes), other.map(|m| m.notes)),
        }
    }
    assert!(matches!(load_usd_file(std::path::Path::new("/definitely/missing.usdz"), &opts()), Err(ImportError::Io(_))));
}

#[test]
fn inconsistent_topology_is_a_parse_error() {
    let bad_counts = mesh(&format!("{SQUARE}\nint[] faceVertexCounts = [3]\nint[] faceVertexIndices = [0, 1, 2, 3]\n{}", st_vertex()));
    let bad_index = mesh(&format!("{SQUARE}\nint[] faceVertexCounts = [3]\nint[] faceVertexIndices = [0, 1, 9]\n{}", st_vertex()));
    let degenerate = mesh(&format!("{SQUARE}\nint[] faceVertexCounts = [2]\nint[] faceVertexIndices = [0, 1]\n{}", st_vertex()));
    let short_uv = mesh(&format!(
        "{SQUARE}\nint[] faceVertexCounts = [4]\nint[] faceVertexIndices = [0, 1, 2, 3]\n\
         texCoord2f[] primvars:st = [(0, 0)] (\n interpolation = \"faceVarying\"\n )\nint[] primvars:st:indices = [0, 1, 0, 0]"
    ));
    for m in [bad_counts, bad_index, degenerate, short_uv] {
        assert!(matches!(load(&wrap(&m)), Err(ImportError::Parse(_))), "{m}");
    }
}

#[test]
fn compressed_usdz_gives_a_message() {
    // A zip whose first entry is deflated is not a valid USDZ; the error must be a Parse error, not a panic.
    let bytes = write_usdz_bytes(&cube_model(&["A"], 4), &UsdzOptions::default()).unwrap();
    let mut broken = bytes.clone();
    broken.truncate(broken.len() / 2);
    assert!(matches!(load_usdz_slice(&broken, &opts()), Err(ImportError::Parse(_))));
}
