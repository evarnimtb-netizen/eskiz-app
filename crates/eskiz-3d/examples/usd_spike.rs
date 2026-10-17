//! U0 spike: what does the `openusd` crate (dev-dependency only) do with the USD subset Eskiz writes?
//!
//! Run: `cargo run -p eskiz-3d --example usd_spike`. Prints one line per question.

use std::io::Cursor;

use openusd::sdf::{AbstractData, Path, Value};
use openusd::usdc::CrateWriter;
use openusd::usdz::{Archive, ArchiveWriter};
use openusd::{usda, usdc};

const SAMPLE: &str = r#"#usda 1.0
(
    defaultPrim = "Model"
    upAxis = "Y"
    metersPerUnit = 1
)

def Xform "Model" (
    kind = "component"
)
{
    def Xform "Moved"
    {
        double3 xformOp:translate = (1, 2, 3)
        uniform token[] xformOpOrder = ["xformOp:translate"]

        def Mesh "Panel"
        {
            point3f[] points = [(-1, 0, -1), (1, 0, -1), (1, 0, 1), (-1, 0, 1)]
            int[] faceVertexCounts = [3, 3]
            int[] faceVertexIndices = [0, 1, 2, 0, 2, 3]
            normal3f[] normals = [(0, 1, 0), (0, 1, 0), (0, 1, 0), (0, 1, 0)] (
                interpolation = "vertex"
            )
            texCoord2f[] primvars:st = [(0, 1), (1, 1), (1, 0), (0, 0)] (
                interpolation = "vertex"
            )
            uniform token subdivisionScheme = "none"
            rel material:binding = </Model/Looks/Skin>
        }
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
                token outputs:surface
            }

            def Shader "Colour"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @textures/Skin_color.png@
                token inputs:sourceColorSpace = "sRGB"
                float3 outputs:rgb
            }
        }
    }
}
"#;

fn field(d: &dyn AbstractData, path: &str, f: &str) -> Option<Value> {
    d.try_field(&Path::new(path).unwrap(), f)
        .ok()
        .flatten()
        .map(|v| v.into_owned())
}

fn main() {
    let t0 = std::time::Instant::now();
    // (1) read a USDA with Mesh, material, Xform
    let data = usda::parse(SAMPLE).expect("parse usda");
    println!("Q1 parse usda: ok, {} specs", data.spec_paths().len());
    let mesh = "/Model/Moved/Panel";
    for (p, f) in [
        (".points", "default"),
        (".faceVertexCounts", "default"),
        (".faceVertexIndices", "default"),
        (".normals", "default"),
        (".primvars:st", "default"),
    ] {
        let path = format!("{mesh}{p}");
        let v = field(&data, &path, f);
        println!("Q1 {path}: {}", v.map_or("MISSING".to_string(), |v| format!("{v:?}")));
    }
    println!(
        "Q1 shader id: {:?}",
        field(&data, "/Model/Looks/Skin/Colour.info:id", "default")
    );
    println!(
        "Q1 texture file: {:?}",
        field(&data, "/Model/Looks/Skin/Colour.inputs:file", "default")
    );
    println!(
        "Q1 xform translate: {:?}",
        field(&data, "/Model/Moved.xformOp:translate", "default")
    );

    // (2) usdc write then read back through its own reader
    let mut buf = Cursor::new(Vec::new());
    CrateWriter::write(&data, &mut buf).expect("write usdc");
    let crate_bytes = buf.into_inner();
    let back = usdc::CrateData::open(crate_bytes.clone(), true).expect("read usdc back");
    let mut mismatches = 0;
    for p in data.spec_paths() {
        for f in data.list_fields(&p).unwrap_or_default() {
            let a = field(&data, &p.to_string(), &f);
            let b = field(&back, &p.to_string(), &f);
            if format!("{a:?}") != format!("{b:?}") {
                mismatches += 1;
                println!("Q2 mismatch {p} {f}: {a:?} vs {b:?}");
            }
        }
    }
    println!("Q2 usdc round trip: {} bytes, {mismatches} field mismatches", crate_bytes.len());

    // (3) ArchiveWriter alignment
    let mut w = ArchiveWriter::new(Cursor::new(Vec::new()));
    w.add_layer("model.usdc", &crate_bytes).unwrap();
    w.add_layer("textures/Skin_color.png", &[1, 2, 3, 4, 5]).unwrap();
    let zip = w.finish().unwrap().into_inner();
    let mut ar = Archive::from_bytes(zip.clone()).expect("open usdz");
    println!(
        "Q3 usdz: {} bytes, first layer {:?}, reads back root: {}",
        zip.len(),
        ar.first_layer_name(),
        ar.read_first_layer().is_ok()
    );
    // Local header: name_len at 26, extra_len at 28, data after 30 + name + extra.
    let mut off = 0usize;
    while zip[off..].starts_with(&[0x50, 0x4b, 3, 4]) {
        let n = u16::from_le_bytes([zip[off + 26], zip[off + 27]]) as usize;
        let x = u16::from_le_bytes([zip[off + 28], zip[off + 29]]) as usize;
        let size = u32::from_le_bytes(zip[off + 18..off + 22].try_into().unwrap()) as usize;
        let data_off = off + 30 + n + x;
        println!("Q3 entry at {off}: data offset {data_off} (% 64 = {})", data_off % 64);
        off = data_off + size;
    }
    println!("spike elapsed {:?}", t0.elapsed());
}
