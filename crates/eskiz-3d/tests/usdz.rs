//! USDZ export: zip layout, USDA text, UV orientation, maps, names, rejection paths.
mod common;

use common::{asymmetric_quad, cube_model, triangle_model};
use eskiz_3d::usd::zip::{crc32, write_stored_zip};
use eskiz_3d::usd::{material_idents, texture_entry_name};
use eskiz_3d::{
    export_usdz, write_usda, write_usdz_bytes, ExportError, MapKind, Model, SubMesh, TextureSet, UsdzOptions,
};
use glam::{Vec2, Vec3};

fn u16_at(b: &[u8], o: usize) -> usize {
    u16::from_le_bytes([b[o], b[o + 1]]) as usize
}
fn u32_at(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize
}

struct Entry {
    name: String,
    method: usize,
    data_offset: usize,
    data: Vec<u8>,
    crc: u32,
}

/// Parse the zip by hand (local headers walked from offset 0, cross-checked with the central directory).
fn parse_zip(z: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut off = 0;
    while u32_at(z, off) == 0x0403_4b50 {
        let method = u16_at(z, off + 8);
        let crc = u32_at(z, off + 14) as u32;
        let size = u32_at(z, off + 18);
        assert_eq!(size, u32_at(z, off + 22), "stored: compressed == uncompressed");
        assert_eq!(u16_at(z, off + 6) & 8, 0, "no data descriptor");
        let n = u16_at(z, off + 26);
        let x = u16_at(z, off + 28);
        let name = String::from_utf8(z[off + 30..off + 30 + n].to_vec()).unwrap();
        let data_offset = off + 30 + n + x;
        out.push(Entry { name, method, data_offset, data: z[data_offset..data_offset + size].to_vec(), crc });
        off = data_offset + size;
    }
    // Central directory agrees.
    assert_eq!(u32_at(z, off), 0x0201_4b50);
    let eocd = z.len() - 22;
    assert_eq!(u32_at(z, eocd), 0x0605_4b50);
    assert_eq!(u16_at(z, eocd + 10), out.len());
    assert_eq!(u32_at(z, eocd + 16), off, "directory starts where the data ended");
    let mut c = off;
    for e in &out {
        assert_eq!(u32_at(z, c), 0x0201_4b50);
        let n = u16_at(z, c + 28);
        assert_eq!(&z[c + 46..c + 46 + n], e.name.as_bytes());
        assert_eq!(u32_at(z, c + 16) as u32, e.crc);
        c += 46 + n + u16_at(z, c + 30) + u16_at(z, c + 32);
    }
    assert_eq!(c, eocd);
    out
}

fn decode_png(b: &[u8]) -> (usize, usize, Vec<u8>) {
    let dec = png::Decoder::new(std::io::Cursor::new(b));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba);
    buf.truncate(info.buffer_size());
    (info.width as usize, info.height as usize, buf)
}

fn usdz(model: &Model) -> Vec<Entry> {
    parse_zip(&write_usdz_bytes(model, &UsdzOptions::default()).unwrap())
}

fn root_text(entries: &[Entry]) -> &str {
    std::str::from_utf8(&entries[0].data).unwrap()
}

// ---- zip layout ----

#[test]
fn zip_is_stored_aligned_crc_checked_root_first() {
    let model = cube_model(&["Skin", "Metal"], 3);
    let entries = usdz(&model);
    assert_eq!(entries[0].name, "model.usda", "root layer first");
    assert_eq!(entries.len(), 1 + 2 * 5, "root plus five maps for each of two materials");
    for e in &entries {
        assert_eq!(e.method, 0, "{} is stored", e.name);
        assert_eq!(e.data_offset % 64, 0, "{} data offset {}", e.name, e.data_offset);
        assert_eq!(crc32(&e.data), e.crc, "{} crc", e.name);
    }
}

#[test]
fn crc32_known_values() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn alignment_holds_for_awkward_name_lengths_and_sizes() {
    let entries: Vec<(String, Vec<u8>)> = (0..40)
        .map(|i| (format!("{}{}.bin", "n".repeat(i), i), vec![i as u8; i * 7 + 1]))
        .collect();
    let z = write_stored_zip(&entries).unwrap();
    let parsed = parse_zip(&z);
    assert_eq!(parsed.len(), 40);
    for (p, (name, data)) in parsed.iter().zip(&entries) {
        assert_eq!(&p.name, name);
        assert_eq!(&p.data, data);
        assert_eq!(p.data_offset % 64, 0);
    }
}

#[test]
fn zip_guards() {
    assert!(write_stored_zip(&[]).is_err(), "no entries");
    for bad in ["", "/abs", "a/../b", "a\\b", "a//b", "dir/"] {
        assert!(write_stored_zip(&[(bad.to_owned(), vec![1])]).is_err(), "{bad:?}");
    }
    let many: Vec<(String, Vec<u8>)> = (0..65_536).map(|i| (format!("f{i}"), Vec::new())).collect();
    assert!(write_stored_zip(&many).is_err(), "over 65535 entries (no zip64)");
    let ok: Vec<(String, Vec<u8>)> = (0..65_535).map(|i| (format!("f{i}"), Vec::new())).collect();
    assert_eq!(parse_zip(&write_stored_zip(&ok).unwrap()).len(), 65_535);
}

#[test]
fn python_zipfile_agrees() {
    let Ok(dir) = std::env::var("CARGO_TARGET_TMPDIR").or_else(|_| Ok::<_, ()>(std::env::temp_dir().display().to_string()))
    else {
        return;
    };
    let path = std::path::Path::new(&dir).join("usdz_python_check.usdz");
    export_usdz(&cube_model(&["A"], 2), &path).unwrap();
    let script = "import sys,zipfile,struct\nraw=open(sys.argv[1],'rb').read()\nz=zipfile.ZipFile(sys.argv[1])\n\
        assert z.testzip() is None\ni=z.infolist()\nassert i[0].filename=='model.usda'\n\
        assert all(x.compress_type==0 for x in i)\n\
        for x in i:\n n,e=struct.unpack_from('<HH',raw,x.header_offset+26)\n assert (x.header_offset+30+n+e)%64==0\n\
        print(len(i))";
    let Ok(out) = std::process::Command::new("python3").args(["-I", "-c", script]).arg(&path).output() else {
        eprintln!("python3 not found; skipping");
        return;
    };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "6");
    let _ = std::fs::remove_file(path);
}

// ---- USDA text ----

const GOLDEN: &str = r#"#usda 1.0
(
    defaultPrim = "Model"
    upAxis = "Y"
    metersPerUnit = 1
    doc = "Exported from Eskiz"
)

def Xform "Model" (
    kind = "component"
)
{
    def Mesh "Skin_Mesh" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        float3[] extent = [(0, 0, 0), (1, 1, 0)]
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0, 1, 2]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        normal3f[] normals = [(0, 0, 1), (0, 0, 1), (0, 0, 1)] (
            interpolation = "vertex"
        )
        texCoord2f[] primvars:st = [(0, 0), (1, 0), (0, 0.75)] (
            interpolation = "vertex"
        )
        uniform bool doubleSided = true
        uniform token orientation = "rightHanded"
        uniform token subdivisionScheme = "none"
        rel material:binding = </Model/Looks/Skin>
    }

    def Scope "Looks"
    {
        def Material "Skin"
        {
            token outputs:surface.connect = </Model/Looks/Skin/PBR.outputs:surface>

            def Shader "PBR"
            {
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor.connect = </Model/Looks/Skin/Colour.outputs:rgb>
                float inputs:roughness.connect = </Model/Looks/Skin/Roughness.outputs:r>
                float inputs:metallic.connect = </Model/Looks/Skin/Metallic.outputs:r>
                float inputs:occlusion.connect = </Model/Looks/Skin/AO.outputs:r>
                normal3f inputs:normal.connect = </Model/Looks/Skin/Normal.outputs:rgb>
                token outputs:surface
            }

            def Shader "StReader"
            {
                uniform token info:id = "UsdPrimvarReader_float2"
                token inputs:varname = "st"
                float2 outputs:result
            }

            def Shader "Colour"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @textures/Skin_color.png@
                float2 inputs:st.connect = </Model/Looks/Skin/StReader.outputs:result>
                token inputs:sourceColorSpace = "sRGB"
                token inputs:wrapS = "repeat"
                token inputs:wrapT = "repeat"
                float3 outputs:rgb
            }

            def Shader "Roughness"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @textures/Skin_roughness.png@
                float2 inputs:st.connect = </Model/Looks/Skin/StReader.outputs:result>
                token inputs:sourceColorSpace = "raw"
                token inputs:wrapS = "repeat"
                token inputs:wrapT = "repeat"
                float outputs:r
            }

            def Shader "Metallic"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @textures/Skin_metallic.png@
                float2 inputs:st.connect = </Model/Looks/Skin/StReader.outputs:result>
                token inputs:sourceColorSpace = "raw"
                token inputs:wrapS = "repeat"
                token inputs:wrapT = "repeat"
                float outputs:r
            }

            def Shader "Normal"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @textures/Skin_normal.png@
                float2 inputs:st.connect = </Model/Looks/Skin/StReader.outputs:result>
                token inputs:sourceColorSpace = "raw"
                token inputs:wrapS = "repeat"
                token inputs:wrapT = "repeat"
                float4 inputs:scale = (2, 2, 2, 1)
                float4 inputs:bias = (-1, -1, -1, 0)
                float3 outputs:rgb
            }

            def Shader "AO"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @textures/Skin_ambient_occlusion.png@
                float2 inputs:st.connect = </Model/Looks/Skin/StReader.outputs:result>
                token inputs:sourceColorSpace = "raw"
                token inputs:wrapS = "repeat"
                token inputs:wrapT = "repeat"
                float outputs:r
            }
        }
    }
}
"#;

#[test]
fn usda_golden() {
    assert_eq!(write_usda(&triangle_model()), GOLDEN);
}

#[test]
fn usda_has_required_stage_and_material_tokens() {
    let text = write_usda(&cube_model(&["Skin", "Metal"], 2));
    for needle in [
        "defaultPrim = \"Model\"",
        "upAxis = \"Y\"",
        "metersPerUnit = 1",
        "uniform token subdivisionScheme = \"none\"",
        "\"UsdPreviewSurface\"",
        "\"UsdUVTexture\"",
        "\"UsdPrimvarReader_float2\"",
        "def Mesh \"Skin_Mesh\"",
        "def Mesh \"Metal_Mesh\"",
        "rel material:binding = </Model/Looks/Metal>",
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
    assert_eq!(text.matches("def Mesh").count(), 2, "one mesh per material");
}

#[test]
fn translucent_colour_adds_opacity() {
    let mut m = triangle_model();
    assert!(!write_usda(&m).contains("inputs:opacity"));
    *m.texture_sets[0].map_mut(MapKind::Colour) = eskiz_core::Layer::from_rgba(1, 1, vec![100, 50, 25, 128]);
    let t = write_usda(&m);
    assert!(t.contains("float inputs:opacity.connect = </Model/Looks/Skin/Colour.outputs:a>"));
    assert!(t.contains("float outputs:a"));
}

// ---- UV orientation ----

#[test]
fn st_is_one_minus_v_including_near_the_top_edge() {
    let mut m = triangle_model();
    m.submeshes[0].uvs = vec![Vec2::new(0., 0.0), Vec2::new(1., 0.001), Vec2::new(0.5, 1.0)];
    let text = write_usda(&m);
    assert!(text.contains("primvars:st = [(0, 1), (1, 0.999), (0.5, 0)]"), "{text}");
}

/// Decode `primvars:st` from the exported text of the first mesh.
fn st_of(text: &str) -> Vec<(f32, f32)> {
    let line = text.lines().find(|l| l.contains("primvars:st")).unwrap();
    let body = &line[line.find("= [").unwrap() + 3..line.rfind(']').unwrap()];
    body.split("), (")
        .map(|p| {
            let p = p.trim_matches(|c| c == '(' || c == ')');
            let (a, b) = p.split_once(", ").unwrap();
            (a.parse().unwrap(), b.parse().unwrap())
        })
        .collect()
}

#[test]
fn asymmetric_texture_lands_on_the_same_texel_through_usd_st() {
    let model = asymmetric_quad();
    let entries = usdz(&model);
    let text = root_text(&entries).to_owned();
    let png = entries.iter().find(|e| e.name == "textures/quad_color.png").unwrap();
    let (w, h, px) = decode_png(&png.data);
    assert_eq!((w, h), (4, 4));
    // Exported image keeps Eskiz's row order: row 0 is the top row.
    assert_eq!(&px[0..3], &[255, 0, 0], "top-left red");
    assert_eq!(&px[3 * 4..3 * 4 + 3], &[0, 255, 0], "top-right green");
    assert_eq!(&px[(3 * 4) * 4..(3 * 4) * 4 + 3], &[0, 0, 255], "bottom-left blue");
    // A USD reader samples image row (1 - t) * h counted from the top of the stored image (t = 1 is the top row).
    // For each vertex, that texel must equal what Eskiz's own (u, v) sampling gives.
    let sm = &model.submeshes[0];
    for ((s, t), uv) in st_of(&text).into_iter().zip(&sm.uvs) {
        let x = ((s * w as f32) as usize).min(w - 1);
        let y = (((1.0 - t) * h as f32) as usize).min(h - 1);
        let usd = &px[(y * w + x) * 4..(y * w + x) * 4 + 3];
        let want = model.texture_sets[0].sample(MapKind::Colour, uv.x.min(0.999), uv.y.min(0.999));
        let want: Vec<u8> = want[..3].iter().map(|c| (c * 255.0).round() as u8).collect();
        assert_eq!(usd, &want[..], "vertex uv {uv:?} -> st ({s}, {t})");
    }
    // Quad corners: bottom-left vertex 0 has uv (0, 1) -> st (0, 0) -> bottom-left texel (blue).
    let st = st_of(&text);
    assert_eq!(st[0], (0.0, 0.0));
    assert_eq!(st[2], (1.0, 1.0));
}

// ---- maps, names ----

#[test]
fn maps_are_unpacked_one_png_each() {
    let model = cube_model(&["Skin", "Metal"], 3);
    let entries = usdz(&model);
    let idents = material_idents(&model);
    for (mat, id) in idents.iter().enumerate() {
        for kind in MapKind::ALL {
            let name = texture_entry_name(id, kind);
            let e = entries.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("{name} missing"));
            let (w, h, px) = decode_png(&e.data);
            assert_eq!((w, h), (3, 3));
            assert_eq!(px, model.texture_sets[mat].straight_rgba(kind), "{name} is the raw map, not packed");
        }
    }
    let colour = &decode_png(&entries.iter().find(|e| e.name == "textures/Skin_color.png").unwrap().data).2;
    let rough = &decode_png(&entries.iter().find(|e| e.name == "textures/Skin_roughness.png").unwrap().data).2;
    assert_ne!(colour, rough);
}

#[test]
fn names_are_sanitised_and_unique() {
    let mut m = cube_model(&["a b", "a/b", "9lives", "\u{e9}t\u{e9}"], 1);
    m.texture_sets[3].name = "\u{e9}t\u{e9}".into();
    let ids = material_idents(&m);
    assert_eq!(ids, ["a_b", "a_b_2", "_9lives", "_t_"]);
    let text = write_usda(&m);
    for id in &ids {
        assert!(text.contains(&format!("def Mesh \"{id}_Mesh\"")), "{id}");
        assert!(text.contains(&format!("def Material \"{id}\"")), "{id}");
    }
    // Every name in the package is plain ASCII with no spaces.
    for e in usdz(&m) {
        assert!(e.name.is_ascii() && !e.name.contains(' '), "{}", e.name);
    }
}

// ---- round trip of mesh counts through an independent reader ----

#[test]
fn openusd_reads_back_counts() {
    use openusd::sdf::{AbstractData, Path, Value};
    let model = cube_model(&["Skin", "Metal"], 2);
    let zip = write_usdz_bytes(&model, &UsdzOptions::default()).unwrap();
    let mut ar = openusd::usdz::Archive::from_bytes(zip).unwrap();
    assert_eq!(ar.first_layer_name().as_deref(), Some("model.usda"));
    let layer = ar.read_first_layer().unwrap();
    let get = |p: &str| layer.try_field(&Path::new(p).unwrap(), "default").unwrap().map(|v| v.into_owned());
    let mut points = 0;
    let mut indices = 0;
    let mut meshes = 0;
    for id in material_idents(&model) {
        let base = format!("/Model/{id}_Mesh");
        let Some(Value::Vec3fVec(p)) = get(&format!("{base}.points")) else { panic!("{base} points") };
        let Some(Value::IntVec(i)) = get(&format!("{base}.faceVertexIndices")) else { panic!("{base} indices") };
        let Some(Value::IntVec(c)) = get(&format!("{base}.faceVertexCounts")) else { panic!("{base} counts") };
        let Some(Value::Vec2fVec(st)) = get(&format!("{base}.primvars:st")) else { panic!("{base} st") };
        assert_eq!(st.len(), p.len());
        assert_eq!(c.len() * 3, i.len());
        assert!(i.iter().all(|&x| (x as usize) < p.len()));
        points += p.len();
        indices += i.len();
        meshes += 1;
    }
    assert_eq!(meshes, model.texture_sets.len());
    assert_eq!(points, model.submeshes.iter().map(|s| s.positions.len()).sum::<usize>());
    assert_eq!(indices / 3, model.triangle_count());
    let Some(Value::Token(id)) =
        get("/Model/Looks/Skin/Colour.info:id")
    else {
        panic!("shader id")
    };
    assert_eq!(id.as_str(), "UsdUVTexture");
    // Texture bytes are reachable through the package.
    assert!(ar.contains("textures/Skin_color.png"));
}

// ---- rejection paths ----

fn model_with(f: impl FnOnce(&mut SubMesh)) -> Model {
    let mut m = triangle_model();
    f(&mut m.submeshes[0]);
    m
}

#[test]
fn rejects_without_panicking() {
    let empty = Model::new(vec![], vec![], 0);
    assert!(matches!(write_usdz_bytes(&empty, &UsdzOptions::default()), Err(ExportError::Empty)));
    let points_only = Model::new(vec![], vec![TextureSet::from_constants("m", 1, 1, [0; 4], 0.5, 0.0)], 12);
    assert!(matches!(write_usdz_bytes(&points_only, &UsdzOptions::default()), Err(ExportError::Empty)));
    let no_tris = model_with(|s| s.indices.clear());
    assert!(matches!(write_usdz_bytes(&no_tris, &UsdzOptions::default()), Err(ExportError::Empty)));

    let no_uvs = model_with(|s| s.uvs.clear());
    let err = write_usdz_bytes(&no_uvs, &UsdzOptions::default()).unwrap_err();
    assert!(matches!(err, ExportError::Invalid(_)));
    assert!(err.to_string().contains("UV"), "{err}");

    let bad_mat = model_with(|s| s.material = 7);
    assert!(matches!(write_usdz_bytes(&bad_mat, &UsdzOptions::default()), Err(ExportError::Invalid(_))));
    let nan = model_with(|s| s.positions[0] = Vec3::NAN);
    assert!(matches!(write_usdz_bytes(&nan, &UsdzOptions::default()), Err(ExportError::Invalid(_))));
    let oob = model_with(|s| s.indices = vec![0, 1, 9]);
    assert!(matches!(write_usdz_bytes(&oob, &UsdzOptions::default()), Err(ExportError::Invalid(_))));
}

#[test]
fn export_usdz_writes_a_file_and_leaves_glb_alone() {
    let dir = std::env::temp_dir().join(format!("eskiz_usdz_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = cube_model(&["Skin"], 2);
    let path = dir.join("m.usdz");
    export_usdz(&model, &path).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), write_usdz_bytes(&model, &UsdzOptions::default()).unwrap());
    // GLB path unchanged by this feature.
    let glb = eskiz_3d::write_glb_bytes(&model).unwrap();
    assert_eq!(&glb[..4], b"glTF");
    let _ = std::fs::remove_dir_all(dir);
}
