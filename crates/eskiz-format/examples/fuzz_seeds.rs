//! Writes the seed corpora for the fuzz targets in `eskiz/fuzz/`: small valid files made with
//! this crate's own writers, one folder per target.
//!
//! Run from `eskiz/`: `cargo run -p eskiz-format --example fuzz_seeds -- fuzz/seeds`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;

use eskiz_format::brushes::{write_brush, write_brushset, ImportedBrush, SettingValue};
use eskiz_format::palettes::{write_aco, write_ase, write_swatches, Palette, Swatch};
use eskiz_format::psd::{self, Compression16, PsdBlend, PsdDocument, PsdGroup, PsdLayer, PsdNode};
use eskiz_format::{apple_lz4_compress, lz4_block_compress, DocumentBuilder, GroupSpec, LayerSpec, Node};

fn pattern(w: usize, h: usize, seed: u8) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % w, i / w);
        if (x + y + usize::from(seed)) % 3 == 0 {
            continue; // leave some pixels transparent
        }
        let a = 128 + (x * 7 % 128) as u8;
        p.copy_from_slice(&[(x as u8).min(a), (y as u8).min(a), seed.min(a), a]);
    }
    px
}

fn procreate(bits: u32) -> Vec<u8> {
    let (w, h) = (40u32, 30u32);
    let mut b = DocumentBuilder::new(w, h);
    b.name = format!("Seed {bits}");
    b.tile_size = 16;
    b.bits_per_channel = bits;
    let layer = |name: &str, seed: u8| {
        let px = pattern(w as usize, h as usize, seed);
        LayerSpec {
            name: name.into(),
            pixels: (bits == 8).then(|| px.clone()),
            pixels16: (bits == 16).then(|| px.iter().map(|&v| u16::from(v) * 257).collect()),
            ..Default::default()
        }
    };
    b.tree = vec![
        Node::Layer(layer("Top", 1)),
        Node::Group(GroupSpec {
            name: "Group".into(),
            opacity: 0.5,
            children: vec![Node::Layer(LayerSpec { blend_mode: 1, opacity: 0.7, ..layer("Inner", 2) })],
            ..Default::default()
        }),
        Node::Layer(LayerSpec { hidden: true, ..layer("Bottom", 3) }),
    ];
    b.to_bytes().expect("seed document")
}

fn document_archive(package: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(Cursor::new(package)).expect("seed zip");
    let mut out = Vec::new();
    zip.by_name("Document.archive").expect("archive").read_to_end(&mut out).expect("read");
    out
}

/// One LZO1X literal run and the end marker.
fn lzo_literals(data: &[u8]) -> Vec<u8> {
    let mut s = vec![(17 + data.len()) as u8];
    s.extend_from_slice(data);
    s.extend_from_slice(&[0x11, 0x00, 0x00]);
    s
}

fn psd_doc() -> PsdDocument {
    let (w, h) = (24u32, 16u32);
    let layer = |name: &str, seed: u8| PsdLayer {
        name: name.into(),
        opacity: 0.8,
        blend: PsdBlend::Normal,
        hidden: false,
        clipped: false,
        alpha_locked: false,
        locked: false,
        pixels: pattern(w as usize, h as usize, seed),
    };
    PsdDocument {
        width: w,
        height: h,
        dpi: 144.0,
        colour_space: None,
        root: vec![
            PsdNode::Layer(layer("Back", 1)),
            PsdNode::Group(PsdGroup {
                name: "Folder".into(),
                opacity: 1.0,
                blend: PsdBlend::Multiply,
                hidden: false,
                collapsed: true,
                children: vec![PsdNode::Layer(layer("Inside", 2))],
            }),
        ],
    }
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_unicode(out: &mut Vec<u8>, s: &str) {
    let units: Vec<u16> = s.encode_utf16().collect();
    push_u32(out, units.len() as u32);
    units.into_iter().for_each(|u| push_u16(out, u));
}

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

/// A version 6 ABR with one sampled tip (raw or PackBits rows) and one preset that uses it.
fn abr(rle: bool) -> Vec<u8> {
    let mut out = Vec::new();
    push_u16(&mut out, 6);
    push_u16(&mut out, 2);
    let mut entry = b"tip-1\0".to_vec();
    for v in [0i32, 0, 2, 3] {
        entry.extend_from_slice(&v.to_be_bytes());
    }
    push_u16(&mut entry, 8);
    if rle {
        entry.push(1);
        push_u16(&mut entry, 2);
        push_u16(&mut entry, 2);
        entry.extend_from_slice(&[0xFE, 0x40, 0xFE, 0xC0]); // each row: one byte repeated 3 times
    } else {
        entry.push(0);
        entry.extend_from_slice(&[0, 255, 128, 64, 0, 255]);
    }
    let mut samp = Vec::new();
    push_u32(&mut samp, entry.len() as u32);
    samp.extend_from_slice(&entry);
    push_section(&mut out, "samp", &samp);

    let mut desc = Vec::new();
    push_u32(&mut desc, 16);
    push_unicode(&mut desc, "");
    push_key(&mut desc, "bsh ");
    push_u32(&mut desc, 5);
    push_key(&mut desc, "Nm  ");
    desc.extend_from_slice(b"TEXT");
    push_unicode(&mut desc, "Seed tip");
    for (key, unit, value) in [("Dmtr", "#Pxl", 40.0f64), ("Spcn", "#Prc", 25.0)] {
        push_key(&mut desc, key);
        desc.extend_from_slice(b"UntF");
        desc.extend_from_slice(unit.as_bytes());
        desc.extend_from_slice(&value.to_be_bytes());
    }
    push_key(&mut desc, "Brsh");
    desc.extend_from_slice(b"Objc");
    push_unicode(&mut desc, "");
    push_key(&mut desc, "computedBrush");
    push_u32(&mut desc, 1);
    push_key(&mut desc, "Hrdn");
    desc.extend_from_slice(b"VlLs");
    push_u32(&mut desc, 1);
    desc.extend_from_slice(b"doub");
    desc.extend_from_slice(&50.0f64.to_be_bytes());
    push_key(&mut desc, "tipI");
    desc.extend_from_slice(b"TEXT");
    push_unicode(&mut desc, "tip-1");
    push_section(&mut out, "desc", &desc);
    out
}

fn brush(name: &str) -> ImportedBrush {
    let mut settings = BTreeMap::new();
    settings.insert("maxSize".into(), SettingValue::Number(40.5));
    settings.insert("plotSpacing".into(), SettingValue::Number(0.1));
    settings.insert("shapeRotation".into(), SettingValue::Bool(true));
    settings.insert("bundledGrainPath".into(), SettingValue::Text("grain/paper".into()));
    settings.insert("pressureSize".into(), SettingValue::Curve(vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]]));
    ImportedBrush {
        name: name.into(),
        settings,
        shape_png: Some(b"\x89PNG\r\n\x1a\n not really".to_vec()),
        ..Default::default()
    }
}

fn palette() -> Palette {
    Palette {
        name: "Seed".into(),
        swatches: vec![
            Swatch { name: Some("Red".into()), rgba: [1.0, 0.0, 0.0, 1.0] },
            Swatch { name: None, rgba: [0.2, 0.4, 0.6, 1.0] },
        ],
    }
}

fn write(dir: &Path, target: &str, name: &str, bytes: &[u8]) {
    let folder = dir.join(target);
    fs::create_dir_all(&folder).expect("create seed folder");
    fs::write(folder.join(name), bytes).expect("write seed");
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: fuzz_seeds <out-dir>");
    let out = Path::new(&out);

    let doc8 = procreate(8);
    let doc16 = procreate(16);
    write(out, "procreate", "doc8.procreate", &doc8);
    write(out, "procreate", "doc16.procreate", &doc16);
    write(out, "keyed_archive", "document8.archive", &document_archive(&doc8));
    write(out, "keyed_archive", "document16.archive", &document_archive(&doc16));

    let ramp: Vec<u8> = (0..1024u32).map(|i| (i * 7 % 251) as u8).collect();
    for (name, data) in [("zeros", vec![0u8; 16 * 16 * 4]), ("ramp", ramp.clone()), ("short", b"abcabcabcabc".to_vec())] {
        write(out, "lz4_tile", &format!("{name}.lz4"), &apple_lz4_compress(&data));
        write(out, "lz4_tile", &format!("{name}.block"), &lz4_block_compress(&data));
        write(out, "lzo_tile", &format!("{name}.lzo"), &lzo_literals(&data[..data.len().min(200)]));
    }

    let doc = psd_doc();
    write(out, "psd", "rgba8.psd", &psd::to_bytes(&doc).expect("psd"));
    let layers16: Vec<Vec<u16>> = [1u8, 2]
        .iter()
        .map(|&seed| pattern(24, 16, seed).iter().map(|&v| u16::from(v) * 257).collect())
        .collect();
    for (name, c) in [("rgba16-rle.psd", Compression16::Rle), ("rgba16-raw.psd", Compression16::Raw)] {
        write(out, "psd", name, &psd::to_bytes16(&doc, &layers16, c).expect("psd16"));
    }

    write(out, "abr", "raw.abr", &abr(false));
    write(out, "abr", "rle.abr", &abr(true));

    write(out, "brush", "one.brush", &write_brush(&brush("Seed")).expect("brush"));
    write(out, "brush", "set.brushset", &write_brushset("Seeds", &[brush("A"), brush("B/C")]).expect("brushset"));

    let p = palette();
    write(out, "palette", "seed.swatches", &write_swatches(&p).expect("swatches"));
    write(out, "palette", "seed.aco", &write_aco(&p).expect("aco"));
    write(out, "palette", "seed.ase", &write_ase(&p).expect("ase"));
}
