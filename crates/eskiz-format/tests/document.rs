//! Integration tests for the reader.
//!
//! Synthetic `.procreate` packages are built in memory: a minimal NSKeyedArchiver writer,
//! a ZIP writer, and literal-only LZ4 and LZO1X encoders. Their output is valid, but nothing is
//! actually compressed, so these tests check the container and tile logic, not the codecs.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Cursor, Write};

use eskiz_format::{Document, Error, Item};
use plist::{Dictionary, Uid, Value};
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

const W: usize = 300;
const H: usize = 200;
const T: usize = 128;

fn uid(i: usize) -> Value {
    Value::Uid(Uid::new(i as u64))
}

// ------------------------------------------------------------ codec encoders (literal only)

/// One LZ4 block made only of literals.
fn lz4_literal_block(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let lit = data.len();
    if lit < 15 {
        out.push((lit as u8) << 4);
    } else {
        out.push(0xF0);
        let mut rem = lit - 15;
        while rem >= 255 {
            out.push(255);
            rem -= 255;
        }
        out.push(rem as u8);
    }
    out.extend_from_slice(data);
    out
}

/// Apple framed LZ4: one `bv41` block and the `bv4$` end marker.
fn lz4_frame(raw: &[u8]) -> Vec<u8> {
    let block = lz4_literal_block(raw);
    let mut f = b"bv41".to_vec();
    f.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    f.extend_from_slice(&(block.len() as u32).to_le_bytes());
    f.extend_from_slice(&block);
    f.extend_from_slice(b"bv4$");
    f
}

/// An LZO1X stream of one literal run and the end-of-stream marker.
fn lzo_literal_stream(data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut s = Vec::new();
    if (1..=238).contains(&n) {
        s.push((17 + n) as u8);
    } else if n > 238 {
        let rem = n - 18;
        let last = (rem - 1) % 255 + 1;
        let zeros = (rem - last) / 255;
        s.push(0);
        s.extend(std::iter::repeat_n(0, zeros));
        s.push(last as u8);
    }
    s.extend_from_slice(data);
    s.extend_from_slice(&[0x11, 0x00, 0x00]);
    s
}

// ------------------------------------------------------------ NSKeyedArchiver writer

#[derive(Clone)]
enum V {
    Int(i64),
    Real(f64),
    Bool(bool),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<V>),
    Obj(&'static str, Vec<(&'static str, V)>),
}

fn s(text: &str) -> V {
    V::Str(text.to_owned())
}

/// Port of the Python `_Archiver`: objects are appended to `$objects` as they are encoded.
struct Archiver {
    objects: Vec<Value>,
    classes: HashMap<&'static str, usize>,
}

impl Archiver {
    fn new() -> Self {
        Self {
            objects: vec![Value::String("$null".into())],
            classes: HashMap::new(),
        }
    }

    fn push(&mut self, value: Value) -> usize {
        self.objects.push(value);
        self.objects.len() - 1
    }

    fn class(&mut self, name: &'static str) -> usize {
        if let Some(&idx) = self.classes.get(name) {
            return idx;
        }
        let mut d = Dictionary::new();
        d.insert("$classname".into(), Value::String(name.into()));
        d.insert(
            "$classes".into(),
            Value::Array(vec![Value::String(name.into()), Value::String("NSObject".into())]),
        );
        let idx = self.push(Value::Dictionary(d));
        self.classes.insert(name, idx);
        idx
    }

    fn encode(&mut self, v: &V) -> Value {
        match v {
            V::Int(n) => Value::Integer((*n).into()),
            V::Real(f) => Value::Real(*f),
            V::Bool(b) => Value::Boolean(*b),
            V::Str(text) => uid(self.push(Value::String(text.clone()))),
            V::Bytes(bytes) => {
                let cls = self.class("NSData");
                let mut d = Dictionary::new();
                d.insert("NS.data".into(), Value::Data(bytes.clone()));
                d.insert("$class".into(), uid(cls));
                uid(self.push(Value::Dictionary(d)))
            }
            V::List(items) => {
                let idx = self.push(Value::Boolean(false));
                let encoded: Vec<Value> = items.iter().map(|x| self.encode(x)).collect();
                let cls = self.class("NSArray");
                let mut d = Dictionary::new();
                d.insert("NS.objects".into(), Value::Array(encoded));
                d.insert("$class".into(), uid(cls));
                self.objects[idx] = Value::Dictionary(d);
                uid(idx)
            }
            V::Obj(name, fields) => {
                let idx = self.push(Value::Boolean(false));
                let mut d = Dictionary::new();
                for (key, x) in fields {
                    let encoded = self.encode(x);
                    d.insert((*key).into(), encoded);
                }
                let cls = self.class(name);
                d.insert("$class".into(), uid(cls));
                self.objects[idx] = Value::Dictionary(d);
                uid(idx)
            }
        }
    }

    fn archive(mut self, root: &V) -> Vec<u8> {
        let top = self.encode(root);
        let mut top_dict = Dictionary::new();
        top_dict.insert("root".into(), top);
        let mut doc = Dictionary::new();
        doc.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        doc.insert("$version".into(), Value::Integer(100_000.into()));
        doc.insert("$top".into(), Value::Dictionary(top_dict));
        doc.insert("$objects".into(), Value::Array(self.objects));
        let mut buf = Vec::new();
        Value::Dictionary(doc).to_writer_binary(&mut buf).unwrap();
        buf
    }
}

// ------------------------------------------------------------ package builders

/// Port of `_tile_pixels`: a deterministic, premultiplied-valid RGBA pattern.
fn tile_pixels(c: usize, r: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let a = (x * 7 + y * 3 + c * 50 + r * 90) % 256;
            out.push((a * ((x + c) % 256) / 255) as u8);
            out.push((a * ((y + r) % 256) / 255) as u8);
            out.push((a / 2) as u8);
            out.push(a as u8);
        }
    }
    out
}

/// A `SilicaLayer` object of the canvas size. `extra` replaces default fields.
fn layer_obj(uuid: &str, name: &str, blend: i64, extra: Vec<(&'static str, V)>) -> V {
    let mut fields: Vec<(&'static str, V)> = vec![
        ("UUID", s(uuid)),
        ("name", s(name)),
        ("sizeWidth", V::Int(W as i64)),
        ("sizeHeight", V::Int(H as i64)),
        ("opacity", V::Real(0.75)),
        ("blend", V::Int(0)),
        ("extendedBlend", V::Int(blend)),
        ("extendedBlend2", V::Int(blend)),
        ("hidden", V::Bool(false)),
    ];
    for (key, value) in extra {
        fields.retain(|(k, _)| *k != key);
        fields.push((key, value));
    }
    V::Obj("SilicaLayer", fields)
}

/// A ZIP package from `(name, bytes, method)` entries.
fn write_zip(entries: Vec<(String, Vec<u8>, CompressionMethod)>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data, method) in entries {
        let options = SimpleFileOptions::default().compression_method(method);
        zip.start_file(name, options).unwrap();
        zip.write_all(&data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn background_bytes() -> Vec<u8> {
    [1.0f32, 0.5, 0.25, 1.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

/// Port of `build_procreate`: two layers in a group plus a composite. Layer 1 has one empty tile,
/// and `BBBB-2` has a stale revision 0 next to the real revision 2 of its first tile.
/// Returns the package and the expected RGBA8 pixels per layer UUID.
fn build_procreate(prefix: &str) -> (Vec<u8>, HashMap<&'static str, Vec<u8>>) {
    let mut expected = HashMap::new();
    let mut files: Vec<(String, Vec<u8>, CompressionMethod)> = Vec::new();
    for uuid in ["AAAA-1", "BBBB-2", "COMP-0"] {
        let mut img = vec![0u8; W * H * 4];
        for r in 0..H.div_ceil(T) {
            for c in 0..W.div_ceil(T) {
                if uuid == "AAAA-1" && (c, r) == (1, 1) {
                    continue;
                }
                let (tw, th) = (T.min(W - c * T), T.min(H - r * T));
                let px = tile_pixels(c + uuid.len(), r, tw, th);
                let frame = lz4_frame(&px);
                if uuid == "BBBB-2" && (c, r) == (0, 0) {
                    files.push((format!("{prefix}BBBB-2/0~0.lz4.2"), frame, CompressionMethod::Stored));
                    let stale = lz4_frame(&vec![0u8; T * T * 4]);
                    files.push((format!("{prefix}BBBB-2/0~0.lz4"), stale, CompressionMethod::Stored));
                } else {
                    files.push((format!("{prefix}{uuid}/{c}~{r}.lz4"), frame, CompressionMethod::Stored));
                }
                for y in 0..th {
                    let o = ((r * T + y) * W + c * T) * 4;
                    img[o..o + tw * 4].copy_from_slice(&px[y * tw * 4..(y + 1) * tw * 4]);
                }
            }
        }
        expected.insert(uuid, img);
    }

    let l1 = layer_obj("AAAA-1", "Sketch", 1, vec![("hidden", V::Bool(true))]);
    let l2 = layer_obj("BBBB-2", "Ink", 26, vec![]);
    let group = V::Obj(
        "SilicaGroup",
        vec![
            ("name", s("Group 1")),
            ("children", V::List(vec![l1.clone(), l2.clone()])),
            ("opacity", V::Real(1.0)),
            ("isHidden", V::Bool(false)),
        ],
    );
    let doc = V::Obj(
        "SilicaDocument",
        vec![
            ("name", s("Test Doc")),
            ("tileSize", V::Int(T as i64)),
            ("version", V::Int(2)),
            ("size", s(&format!("{{{W}, {H}}}"))),
            ("unwrappedLayers", V::List(vec![group])),
            ("layers", V::List(vec![l1, l2])),
            ("composite", layer_obj("COMP-0", "", 0, vec![])),
            ("backgroundColor", V::Bytes(background_bytes())),
            ("SilicaDocumentArchiveDPIKey", V::Real(132.0)),
            ("strokeCount", V::Int(42)),
            ("orientation", V::Int(1)),
        ],
    );

    let mut entries = vec![(
        format!("{prefix}Document.archive"),
        Archiver::new().archive(&doc),
        CompressionMethod::Deflated,
    )];
    entries.push((
        format!("{prefix}QuickLook/Thumbnail.png"),
        b"\x89PNG fake".to_vec(),
        CompressionMethod::Stored,
    ));
    entries.extend(files);
    (write_zip(entries), expected)
}

/// A package with one legacy `.chunk` (LZO) layer, a clipped edge tile, a full-size edge tile
/// and a stale `.chunk` revision 0. Canvas 200x130 with 128-pixel tiles.
fn build_legacy() -> (Vec<u8>, Vec<u8>) {
    const LW: usize = 200;
    const LH: usize = 130;
    let uuid = "LEGACY-7";
    let mut expected = vec![0u8; LW * LH * 4];
    let mut files: Vec<(String, Vec<u8>, CompressionMethod)> = Vec::new();
    for r in 0..LH.div_ceil(T) {
        for c in 0..LW.div_ceil(T) {
            let (tw, th) = (T.min(LW - c * T), T.min(LH - r * T));
            let full = tile_pixels(c, r, T, T);
            if (c, r) == (0, 0) {
                // stale revision 0 holds zeros; revision 1 is the real tile
                let stale = lzo_literal_stream(&vec![0u8; T * T * 4]);
                files.push((format!("{uuid}/0~0.chunk"), stale, CompressionMethod::Stored));
                files.push((format!("{uuid}/0~0.chunk.1"), lzo_literal_stream(&full), CompressionMethod::Deflated));
            } else if (c, r) == (1, 1) {
                // a full-size payload for an edge tile: the reader must clip it
                files.push((format!("{uuid}/1~1.chunk"), lzo_literal_stream(&full), CompressionMethod::Stored));
            } else {
                let clipped: Vec<u8> = (0..th)
                    .flat_map(|y| full[y * T * 4..y * T * 4 + tw * 4].iter().copied())
                    .collect();
                files.push((format!("{uuid}/{c}~{r}.chunk"), lzo_literal_stream(&clipped), CompressionMethod::Stored));
            }
            for y in 0..th {
                let o = ((r * T + y) * LW + c * T) * 4;
                expected[o..o + tw * 4].copy_from_slice(&full[y * T * 4..y * T * 4 + tw * 4]);
            }
        }
    }
    let doc = V::Obj(
        "SilicaDocument",
        vec![
            ("name", s("Legacy")),
            ("tileSize", V::Int(T as i64)),
            ("size", s(&format!("{{{LW}, {LH}}}"))),
            (
                "layers",
                V::List(vec![V::Obj(
                    "SilicaLayer",
                    vec![
                        ("UUID", s(uuid)),
                        ("name", s("Legacy")),
                        ("sizeWidth", V::Int(LW as i64)),
                        ("sizeHeight", V::Int(LH as i64)),
                        ("opacity", V::Real(1.0)),
                        ("extendedBlend2", V::Int(0)),
                    ],
                )]),
            ),
        ],
    );
    let mut entries = vec![(
        "Document.archive".to_string(),
        Archiver::new().archive(&doc),
        CompressionMethod::Deflated,
    )];
    entries.extend(files);
    (write_zip(entries), expected)
}

fn open(bytes: Vec<u8>) -> Document<Cursor<Vec<u8>>> {
    Document::from_reader(Cursor::new(bytes)).expect("document opens")
}

// ------------------------------------------------------------ tests

#[test]
fn metadata_tree_and_layers() {
    let (bytes, _) = build_procreate("");
    let mut doc = open(bytes);
    assert_eq!(
        (doc.name.as_deref(), doc.width, doc.height, doc.tile_size),
        (Some("Test Doc"), 300, 200, 128)
    );
    assert_eq!(doc.stroke_count, Some(42));
    assert_eq!(doc.orientation, Some(1));
    assert!((doc.dpi.unwrap() - 132.0).abs() < 1e-9);
    let color = doc.background_color.expect("background color");
    assert!((color[1] - 0.5).abs() < 1e-6);

    let [Item::Group(group)] = doc.tree.as_slice() else {
        panic!("expected a single group at the top level");
    };
    assert_eq!(group.name, "Group 1");
    let children: Vec<&str> = group
        .children
        .iter()
        .map(|item| match item {
            Item::Layer(layer) => layer.name.as_str(),
            Item::Group(_) => "group",
        })
        .collect();
    assert_eq!(children, ["Sketch", "Ink"]);

    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Sketch", "Ink"]);
    let (sketch, ink) = (&doc.layers[0], &doc.layers[1]);
    assert!(sketch.hidden);
    assert_eq!(sketch.blend_name().as_deref(), Some("Multiply"));
    assert_eq!(ink.blend_name().as_deref(), Some("Divide"));
    assert!((ink.opacity - 0.75).abs() < 1e-9);
    assert_eq!(doc.tile_count("AAAA-1"), 5);
    assert_eq!(doc.tile_count("BBBB-2"), 6);

    assert_eq!(doc.thumbnail().unwrap().as_deref(), Some(&b"\x89PNG fake"[..]));
}

#[test]
fn layer_pixels_clip_missing_tiles_and_pick_highest_revision() {
    let (bytes, expected) = build_procreate("");
    let mut doc = open(bytes);
    let (sketch, ink) = (doc.layers[0].clone(), doc.layers[1].clone());

    // the skipped tile (1, 1) must read back as transparent
    let mut want_sketch = expected["AAAA-1"].clone();
    for y in T..H {
        let o = (y * W + T) * 4;
        want_sketch[o..o + T * 4].fill(0);
    }
    assert_eq!(doc.layer_rgba(&sketch).unwrap(), want_sketch);
    // revision 2 wins over the stale revision 0
    assert_eq!(doc.layer_rgba(&ink).unwrap(), expected["BBBB-2"]);
    assert_eq!(doc.composite_rgba().unwrap().as_deref(), Some(&expected["COMP-0"][..]));
}

#[test]
fn legacy_lzo_tiles_clip_full_edge_payloads_and_use_revisions() {
    let (bytes, expected) = build_legacy();
    let mut doc = open(bytes);
    let layer = doc.layers[0].clone();
    assert_eq!(layer.uuid, "LEGACY-7");
    assert_eq!(doc.tile_count("LEGACY-7"), 4);
    assert_eq!(doc.layer_rgba(&layer).unwrap(), expected);
}

#[test]
fn nested_package_folder_is_found() {
    let (bytes, expected) = build_procreate("Sample.procreate/");
    let mut doc = open(bytes);
    assert_eq!(doc.layers.len(), 2);
    let ink = doc.layers[1].clone();
    assert_eq!(doc.layer_rgba(&ink).unwrap(), expected["BBBB-2"]);
    assert_eq!(doc.thumbnail().unwrap().as_deref(), Some(&b"\x89PNG fake"[..]));
}

#[test]
fn open_reads_a_file_from_disk() {
    let (bytes, _) = build_procreate("");
    let path = env::temp_dir().join(format!("eskiz-format-{}.procreate", std::process::id()));
    fs::write(&path, &bytes).unwrap();
    let result = Document::open(&path);
    fs::remove_file(&path).ok();
    let doc = result.expect("opens from path");
    assert_eq!(doc.name.as_deref(), Some("Test Doc"));
}

#[test]
fn rejects_files_that_are_not_procreate_documents() {
    assert!(Document::from_reader(Cursor::new(b"not a zip at all".to_vec())).is_err());

    let other = write_zip(vec![("readme.txt".into(), b"hi".to_vec(), CompressionMethod::Stored)]);
    let err = Document::from_reader(Cursor::new(other)).err().expect("rejected");
    assert!(matches!(err, Error::NotProcreate(_)), "got {err}");
}

#[test]
fn corrupt_tile_is_an_error_not_a_panic() {
    let doc = V::Obj(
        "SilicaDocument",
        vec![
            ("tileSize", V::Int(128)),
            ("size", s("{128, 128}")),
            ("layers", V::List(vec![layer_obj("BAD-1", "Bad", 0, vec![])])),
        ],
    );
    let bytes = write_zip(vec![
        ("Document.archive".into(), Archiver::new().archive(&doc), CompressionMethod::Deflated),
        ("BAD-1/0~0.lz4".into(), b"bv41\x10\x00\x00\x00\x02\x00\x00\x00zz".to_vec(), CompressionMethod::Stored),
    ]);
    let mut doc = open(bytes);
    let layer = doc.layers[0].clone();
    assert!(doc.layer_rgba(&layer).is_err());
}
