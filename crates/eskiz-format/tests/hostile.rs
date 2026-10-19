//! Hostile and damaged files: bombs, huge declared sizes, deep nesting, lying size prefixes and
//! truncation. Every case must end in an error (or a clean read), never a panic, a hang or an
//! allocation the file did not pay for. Crash inputs found by fuzzing (`eskiz/fuzz/`) are added
//! here as regression tests.

use std::io::{Cursor, Write};
use std::time::{Duration, Instant};

use eskiz_format::brushes::{read_abr, read_brush, read_brushset, write_brush, write_brushset, ImportedBrush};
use eskiz_format::keyedarchive::{self, NsObject, NsValue};
use eskiz_format::palettes::{read_aco, read_ase, read_swatches, write_aco, write_ase, write_swatches, Palette, Swatch};
use eskiz_format::psd::{self, PsdBlend, PsdDocument, PsdLayer, PsdNode};
use eskiz_format::{apple_lz4_decompress, limits, unarchive, Document, DocumentBuilder, Error, LayerSpec, Node};
use plist::{Dictionary, Uid, Value};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

// ----------------------------------------------------------------------------- helpers

fn package(entries: &[(&str, &[u8], CompressionMethod)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data, method) in entries {
        let options = SimpleFileOptions::default().compression_method(*method).large_file(data.len() > u32::MAX as usize);
        zip.start_file(*name, options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn layer(uuid: &str) -> NsValue {
    NsValue::Object(
        NsObject::new("SilicaLayer")
            .field("UUID", NsValue::String(uuid.into()))
            .field("name", NsValue::String(uuid.into())),
    )
}

/// `Document.archive` for a canvas of `size` (`"{w, h}"`) with these layers.
fn document_archive(size: &str, tile: i64, layers: Vec<NsValue>) -> Vec<u8> {
    let doc = NsObject::new("SilicaDocument")
        .field("size", NsValue::String(size.into()))
        .field("tileSize", NsValue::Int(tile))
        .field("layers", NsValue::Array { items: layers, mutable: true });
    keyedarchive::write(&NsValue::Object(doc)).unwrap()
}

fn open(bytes: Vec<u8>) -> eskiz_format::Result<Document<Cursor<Vec<u8>>>> {
    Document::from_reader(Cursor::new(bytes))
}

fn is_limit<T>(result: eskiz_format::Result<T>) -> bool {
    matches!(result, Err(Error::Limit(_)))
}

fn uid(i: usize) -> Value {
    Value::Uid(Uid::new(i as u64))
}

fn class(name: &str) -> Value {
    let mut d = Dictionary::new();
    d.insert("$classname".into(), Value::String(name.into()));
    d.insert("$classes".into(), Value::Array(vec![Value::String(name.into()), Value::String("NSObject".into())]));
    Value::Dictionary(d)
}

/// An NSKeyedArchiver property list with these `$objects`, rooted at object 1.
fn keyed(objects: Vec<Value>) -> Vec<u8> {
    let mut top = Dictionary::new();
    top.insert("root".into(), uid(1));
    let mut doc = Dictionary::new();
    doc.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
    doc.insert("$version".into(), Value::Integer(100_000.into()));
    doc.insert("$top".into(), Value::Dictionary(top));
    doc.insert("$objects".into(), Value::Array(objects));
    let mut buf = Vec::new();
    Value::Dictionary(doc).to_writer_binary(&mut buf).unwrap();
    buf
}

/// An `NSArray` object whose elements are `refs`; `class_at` is the index of the NSArray class.
fn ns_array(refs: &[usize], class_at: usize) -> Value {
    let mut d = Dictionary::new();
    d.insert("$class".into(), uid(class_at));
    d.insert("NS.objects".into(), Value::Array(refs.iter().map(|&r| uid(r)).collect()));
    Value::Dictionary(d)
}

/// A binary property list written by hand: object `k` is `children(k)` (an array of references)
/// for `k < containers`, and object `containers` is the integer 1.
fn bplist(containers: usize, children: impl Fn(usize) -> Vec<usize>) -> Vec<u8> {
    let count = containers + 1;
    let mut out = b"bplist00".to_vec();
    let mut offsets = Vec::with_capacity(count);
    for k in 0..containers {
        offsets.push(out.len() as u32);
        let refs = children(k);
        assert!(refs.len() < 15);
        out.push(0xA0 | refs.len() as u8);
        for r in refs {
            out.extend_from_slice(&(r as u32).to_be_bytes());
        }
    }
    offsets.push(out.len() as u32);
    out.extend_from_slice(&[0x10, 0x01]);
    let table = out.len() as u64;
    for o in offsets {
        out.extend_from_slice(&o.to_be_bytes());
    }
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 4, 4]);
    out.extend_from_slice(&(count as u64).to_be_bytes());
    out.extend_from_slice(&0u64.to_be_bytes());
    out.extend_from_slice(&table.to_be_bytes());
    out
}

fn small_document(bits: u32) -> Vec<u8> {
    let (w, h) = (20usize, 12usize);
    let mut px = vec![0u8; w * h * 4];
    for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        if i % 3 != 0 {
            p.copy_from_slice(&[10, 20, (i % 200) as u8, 220]);
        }
    }
    let mut b = DocumentBuilder::new(w as u32, h as u32);
    b.tile_size = 8;
    b.bits_per_channel = bits;
    b.tree = vec![Node::Layer(LayerSpec {
        name: "L".into(),
        pixels: (bits == 8).then(|| px.clone()),
        pixels16: (bits == 16).then(|| px.iter().map(|&v| u16::from(v) * 257).collect()),
        ..Default::default()
    })];
    b.to_bytes().unwrap()
}

/// Reads everything a document offers, ignoring errors (only panics matter).
fn read_all(bytes: Vec<u8>) {
    let Ok(mut doc) = open(bytes) else { return };
    let _ = doc.thumbnail();
    let _ = doc.bits_per_channel();
    for layer in doc.layers.clone() {
        let _ = doc.layer_rgba(&layer);
        let _ = doc.layer_rgba16(&layer);
        let _ = doc.layer_tiles(&layer, 8);
        let _ = doc.mask_rgba(&layer);
    }
    let _ = doc.composite_rgba();
}

// ------------------------------------------------------------------------- ZIP bombs

#[test]
fn deflate_bomb_entry_is_rejected_when_the_package_opens() {
    let zeros = vec![0u8; 8 << 20];
    let archive = document_archive("{64, 64}", 256, vec![layer("A")]);
    let bytes = package(&[
        ("Document.archive", &archive, CompressionMethod::Deflated),
        ("bomb.bin", &zeros, CompressionMethod::Deflated),
    ]);
    assert!(bytes.len() < 100_000, "the bomb is small: {} bytes", bytes.len());
    let err = open(bytes).err().expect("rejected");
    assert!(matches!(err, Error::Limit(_)), "{err}");
    assert!(err.to_string().contains("bomb.bin"), "{err}");
}

#[test]
fn deflate_bomb_document_archive_is_rejected() {
    let zeros = vec![0u8; 8 << 20];
    let bytes = package(&[("Document.archive", &zeros, CompressionMethod::Deflated)]);
    assert!(is_limit(open(bytes)));
}

#[test]
fn entry_that_inflates_past_its_declared_size_is_an_error() {
    let archive = document_archive("{64, 64}", 256, vec![layer("A")]);
    let mut bytes = package(&[("Document.archive", &archive, CompressionMethod::Deflated)]);
    // claim 10 bytes in both the local header and the central directory
    let local = bytes.windows(4).position(|w| w == b"PK\x03\x04").unwrap();
    bytes[local + 22..local + 26].copy_from_slice(&10u32.to_le_bytes());
    let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    bytes[central + 24..central + 28].copy_from_slice(&10u32.to_le_bytes());
    assert!(open(bytes).is_err());
}

#[test]
fn overlapping_entries_are_rejected() {
    // a package whose directory lists the same compressed data under three names
    let archive = document_archive("{64, 64}", 256, (0..40).map(|i| layer(&format!("L{i:03}"))).collect());
    let bytes = package(&[("Document.archive", &archive, CompressionMethod::Stored)]);
    let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    let end = bytes.windows(4).rposition(|w| w == b"PK\x05\x06").unwrap();
    let entry = bytes[central..end].to_vec();
    let mut out = bytes[..central].to_vec();
    let cd_start = out.len() as u32;
    out.extend_from_slice(&entry);
    for name in ["Copy1.archive.", "Copy2.archive."] {
        let mut copy = entry.clone();
        let name_at = 46;
        copy[name_at..name_at + name.len() + 2].copy_from_slice(format!("{name}xx").as_bytes());
        out.extend_from_slice(&copy);
    }
    let cd_size = out.len() as u32 - cd_start;
    let mut eocd = bytes[end..].to_vec();
    eocd[8..10].copy_from_slice(&3u16.to_le_bytes());
    eocd[10..12].copy_from_slice(&3u16.to_le_bytes());
    eocd[12..16].copy_from_slice(&cd_size.to_le_bytes());
    eocd[16..20].copy_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&eocd);
    let err = open(out).err().expect("rejected");
    assert!(matches!(err, Error::Limit(_)), "{err}");
}

// ------------------------------------------------------------------- declared sizes

#[test]
fn huge_canvas_is_rejected_before_any_pixels_are_read() {
    for size in ["{100000, 100000}", "{20000, 20000}", "{40000, 8}"] {
        let archive = document_archive(size, 256, vec![layer("A")]);
        let bytes = package(&[("Document.archive", &archive, CompressionMethod::Deflated)]);
        assert!(is_limit(open(bytes)), "{size}");
    }
    // the largest real canvases still open
    let archive = document_archive("{16384, 16384}", 256, vec![layer("A")]);
    let doc = open(package(&[("Document.archive", &archive, CompressionMethod::Deflated)])).unwrap();
    assert_eq!((doc.width, doc.height), (16384, 16384));
}

#[test]
fn huge_layer_size_is_rejected() {
    let big = NsValue::Object(
        NsObject::new("SilicaLayer")
            .field("UUID", NsValue::String("BIG".into()))
            .field("sizeWidth", NsValue::Int(1 << 30))
            .field("sizeHeight", NsValue::Int(1 << 30)),
    );
    let archive = document_archive("{64, 64}", 256, vec![big]);
    assert!(is_limit(open(package(&[("Document.archive", &archive, CompressionMethod::Deflated)]))));
}

#[test]
fn too_many_layers_are_rejected() {
    let layers = (0..=limits::MAX_LAYERS).map(|i| layer(&format!("L{i}"))).collect();
    let archive = document_archive("{8, 8}", 256, layers);
    assert!(is_limit(open(package(&[("Document.archive", &archive, CompressionMethod::Deflated)]))));
}

#[test]
fn tile_size_above_the_limit_is_rejected() {
    let archive = document_archive("{64, 64}", i64::from(limits::MAX_TILE_SIDE) * 2, vec![layer("A")]);
    assert!(open(package(&[("Document.archive", &archive, CompressionMethod::Deflated)])).is_err());
}

// --------------------------------------------------------------------- LZ4 prefixes

fn document_with_tile(tile: &[u8]) -> Document<Cursor<Vec<u8>>> {
    let archive = document_archive("{64, 64}", 64, vec![layer("A")]);
    open(package(&[
        ("Document.archive", &archive, CompressionMethod::Deflated),
        ("A/0~0.lz4", tile, CompressionMethod::Stored),
    ]))
    .unwrap()
}

#[test]
fn lz4_size_prefix_larger_than_a_tile_is_rejected_before_allocating() {
    let mut tile = b"bv41".to_vec();
    tile.extend_from_slice(&0xFFFF_FFF0u32.to_le_bytes()); // decoded size: 4 GiB
    tile.extend_from_slice(&4u32.to_le_bytes());
    tile.extend_from_slice(&[0x1F, 0, 1, 0]);
    tile.extend_from_slice(b"bv4$");
    let mut doc = document_with_tile(&tile);
    let layer = doc.layers[0].clone();
    let err = doc.layer_rgba(&layer).unwrap_err();
    assert!(matches!(err, Error::Corrupt(_)), "{err}");
    assert!(err.to_string().contains("declares"), "{err}");
    assert!(doc.layer_tiles(&layer, 8).is_err());
}

#[test]
fn lz4_stored_block_longer_than_its_data_is_an_error() {
    let mut tile = b"bv4-".to_vec();
    tile.extend_from_slice(&1000u32.to_le_bytes());
    tile.extend_from_slice(&[1, 2, 3]);
    let mut doc = document_with_tile(&tile);
    let layer = doc.layers[0].clone();
    assert!(doc.layer_rgba(&layer).is_err());
}

#[test]
fn lz4_block_that_expands_past_its_prefix_stops_at_the_cap() {
    // one literal, then a match of 4 + 15 + 255 * 200 bytes: far more than the 16 declared
    let mut block = vec![0x1F, 0xAA, 0x01, 0x00];
    block.extend(std::iter::repeat_n(255, 200));
    block.push(0);
    let mut tile = b"bv41".to_vec();
    tile.extend_from_slice(&16u32.to_le_bytes());
    tile.extend_from_slice(&(block.len() as u32).to_le_bytes());
    tile.extend_from_slice(&block);
    assert!(apple_lz4_decompress(&tile, 1 << 20).is_err());
}

// ----------------------------------------------------------------- property lists

#[test]
fn deep_plist_nesting_is_rejected_without_recursing() {
    // 100 000 nested arrays: a recursive reader would overflow the stack
    let deep = bplist(100_000, |k| vec![k + 1]);
    assert!(is_limit(limits::check_plist(&deep)));
    assert!(is_limit(unarchive(&deep)));

    let xml = format!(
        "<?xml version=\"1.0\"?><plist version=\"1.0\">{}<integer>1</integer>{}</plist>",
        "<array>".repeat(100_000),
        "</array>".repeat(100_000)
    );
    assert!(is_limit(limits::check_plist(xml.as_bytes())));

    // nesting up to the limit is fine
    let ok = bplist(limits::MAX_PLIST_DEPTH, |k| vec![k + 1]);
    limits::check_plist(&ok).unwrap();
}

#[test]
fn exponential_plist_graph_is_rejected_quickly() {
    // each array holds the next one twice: 2^60 values once expanded
    let bomb = bplist(60, |k| vec![k + 1, k + 1]);
    let start = Instant::now();
    assert!(is_limit(limits::check_plist(&bomb)));
    assert!(is_limit(unarchive(&bomb)));
    assert!(start.elapsed() < Duration::from_secs(5), "took {:?}", start.elapsed());
}

#[test]
fn self_referencing_plist_is_an_error() {
    let cycle = bplist(2, |k| vec![(k + 1) % 2]);
    assert!(limits::check_plist(&cycle).is_err());
    assert!(unarchive(&cycle).is_err());
}

#[test]
fn keyed_archive_reference_bomb_is_rejected_quickly() {
    // object 1 is an NSArray of object 2 twice, which holds object 3 twice, and so on
    let levels = 60;
    let class_at = levels + 1;
    let mut objects = vec![Value::String("$null".into())];
    for k in 1..levels {
        objects.push(ns_array(&[k + 1, k + 1], class_at));
    }
    objects.push(Value::Integer(7.into()));
    objects.push(class("NSArray"));
    let start = Instant::now();
    assert!(is_limit(unarchive(&keyed(objects))));
    assert!(start.elapsed() < Duration::from_secs(10), "took {:?}", start.elapsed());
}

#[test]
fn keyed_archive_data_amplification_is_rejected() {
    // one 1 MiB blob referenced 600 times would expand to 600 MiB
    let mut objects = vec![Value::String("$null".into())];
    objects.push(ns_array(&vec![2; 600], 3));
    objects.push(Value::Data(vec![0xAB; 1 << 20]));
    objects.push(class("NSArray"));
    assert!(is_limit(unarchive(&keyed(objects))));
}

#[test]
fn keyed_archive_reference_chain_depth_is_limited() {
    let chain = |depth: usize| {
        let class_at = depth + 2;
        let mut objects = vec![Value::String("$null".into())];
        for k in 1..=depth {
            objects.push(ns_array(&[k + 1], class_at));
        }
        objects.push(Value::Integer(1.into()));
        objects.push(class("NSArray"));
        keyed(objects)
    };
    // runs on a test thread (2 MiB of stack): the limit keeps recursion well inside it
    assert!(unarchive(&chain(limits::MAX_ARCHIVE_DEPTH - 2)).is_ok());
    assert!(is_limit(unarchive(&chain(limits::MAX_ARCHIVE_DEPTH + 10))));
    assert!(is_limit(unarchive(&chain(5000))));
}

// --------------------------------------------------------------------------- brushes

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn abr_with(section: &str, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 6, 0, 2];
    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(section.as_bytes());
    push_u32(&mut out, data.len() as u32);
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// A preset descriptor with a name and a diameter (enough to import without a sampled tip).
fn abr_preset() -> Vec<u8> {
    let mut desc = Vec::new();
    push_u32(&mut desc, 16);
    push_u32(&mut desc, 0);
    push_u32(&mut desc, 0);
    desc.extend_from_slice(b"brsh");
    push_u32(&mut desc, 1);
    push_u32(&mut desc, 0);
    desc.extend_from_slice(b"Dmtr");
    desc.extend_from_slice(b"UntF#Pxl");
    desc.extend_from_slice(&12.0f64.to_be_bytes());
    desc
}

#[test]
fn abr_tip_larger_than_the_limit_is_skipped_without_allocating() {
    // a 30000 x 30000 PackBits tip: the old reader reserved 900 MB for it from 60 KB of input
    let side = 30_000i32;
    let mut entry = b"big\0".to_vec();
    for v in [0, 0, side, side] {
        entry.extend_from_slice(&v.to_be_bytes());
    }
    entry.extend_from_slice(&[0, 8, 1]); // 8-bit, PackBits
    entry.extend(std::iter::repeat_n(0, side as usize * 2)); // row byte counts
    let mut samp = Vec::new();
    push_u32(&mut samp, entry.len() as u32);
    samp.extend_from_slice(&entry);
    let mut abr = abr_with("samp", &samp);
    let preset = abr_with("desc", &abr_preset());
    abr.extend_from_slice(&preset[4..]);
    let import = read_abr(&abr).unwrap();
    assert_eq!(import.brushes.len(), 1);
    assert!(import.warnings.iter().any(|w| w.contains("largest tip")), "{:?}", import.warnings);
}

#[test]
fn abr_descriptor_nesting_is_limited() {
    // `Objc` inside `Objc` 10 000 times: a recursive reader without a limit overflows the stack
    let mut desc = Vec::new();
    push_u32(&mut desc, 16);
    for _ in 0..10_000 {
        push_u32(&mut desc, 0); // name: empty
        push_u32(&mut desc, 0);
        desc.extend_from_slice(b"null"); // class id
        push_u32(&mut desc, 1); // one item
        push_u32(&mut desc, 0);
        desc.extend_from_slice(b"Kid "); // key
        desc.extend_from_slice(b"Objc");
    }
    assert!(is_limit(read_abr(&abr_with("desc", &desc))));

    let mut lists = Vec::new();
    push_u32(&mut lists, 16);
    push_u32(&mut lists, 0);
    push_u32(&mut lists, 0);
    lists.extend_from_slice(b"null");
    push_u32(&mut lists, 1);
    push_u32(&mut lists, 0);
    lists.extend_from_slice(b"List");
    lists.extend_from_slice(b"VlLs");
    for _ in 0..10_000 {
        push_u32(&mut lists, 1);
        lists.extend_from_slice(b"VlLs");
    }
    assert!(is_limit(read_abr(&abr_with("desc", &lists))));
}

#[test]
fn brushset_with_too_many_brushes_is_rejected() {
    let names: Vec<String> = (0..=limits::MAX_BRUSHES).map(|i| format!("B{i}/Brush.archive")).collect();
    let entries: Vec<(&str, &[u8], CompressionMethod)> =
        names.iter().map(|n| (n.as_str(), &b"x"[..], CompressionMethod::Stored)).collect();
    assert!(is_limit(read_brushset("set", &package(&entries))));
}

// ------------------------------------------------------------------------------- PSD

fn psd_bytes(layers: usize) -> Vec<u8> {
    let layer = PsdLayer {
        name: "L".into(),
        opacity: 1.0,
        blend: PsdBlend::Normal,
        hidden: false,
        clipped: false,
        alpha_locked: false,
        locked: false,
        pixels: vec![255; 4 * 4 * 4],
    };
    let doc = PsdDocument {
        width: 4,
        height: 4,
        dpi: 72.0,
        colour_space: None,
        root: vec![PsdNode::Layer(layer); layers],
    };
    psd::to_bytes(&doc).unwrap()
}

fn with_canvas(mut psd: Vec<u8>, w: u32, h: u32) -> Vec<u8> {
    psd[14..18].copy_from_slice(&h.to_be_bytes());
    psd[18..22].copy_from_slice(&w.to_be_bytes());
    psd
}

#[test]
fn psd_with_a_huge_canvas_is_rejected() {
    assert!(is_limit(psd::from_bytes(&with_canvas(psd_bytes(1), 20_000, 20_000))));
}

#[test]
fn psd_layers_that_would_decode_past_the_budget_are_rejected_first() {
    // 20 layers on a 16384 x 16384 canvas would be 20 GiB of pixels
    let psd = with_canvas(psd_bytes(20), 16_384, 16_384);
    assert!(is_limit(psd::from_bytes(&psd)));
    assert!(is_limit(psd::from_bytes16(&psd)));
}

// -------------------------------------------------------------------------- palettes

#[test]
fn swatches_json_bomb_is_rejected() {
    let spaces = vec![b' '; 20 << 20];
    let bytes = package(&[("Swatches.json", &spaces, CompressionMethod::Deflated)]);
    assert!(is_limit(read_swatches(&bytes)));
}

// -------------------------------------------------------------------------- truncation

#[test]
fn truncated_files_are_errors_not_panics() {
    let palette = Palette {
        name: "P".into(),
        swatches: vec![Swatch { name: Some("Red".into()), rgba: [1.0, 0.0, 0.0, 1.0] }],
    };
    let brush = ImportedBrush { name: "B".into(), shape_png: Some(vec![1, 2, 3]), ..Default::default() };
    let docs = [small_document(8), small_document(16)];
    let psd8 = psd_bytes(2);
    let brush_bytes = write_brush(&brush).unwrap();
    let set = write_brushset("S", &[brush.clone(), brush]).unwrap();
    let swatches = write_swatches(&palette).unwrap();
    let aco = write_aco(&palette).unwrap();
    let ase = write_ase(&palette).unwrap();

    let cuts = |len: usize| (0..len).step_by((len / 97).max(1)).chain([len.saturating_sub(1)]);
    for doc in &docs {
        read_all(doc.clone());
        for cut in cuts(doc.len()) {
            read_all(doc[..cut].to_vec());
        }
    }
    for cut in cuts(psd8.len()) {
        let _ = psd::from_bytes(&psd8[..cut]);
        let _ = psd::from_bytes16(&psd8[..cut]);
    }
    for cut in cuts(set.len()) {
        let _ = read_brush("b", &brush_bytes[..cut.min(brush_bytes.len())]);
        let _ = read_brushset("s", &set[..cut]);
    }
    for bytes in [&swatches, &aco, &ase] {
        for cut in cuts(bytes.len()) {
            let _ = read_swatches(&bytes[..cut]);
            let _ = read_aco(&bytes[..cut]);
            let _ = read_ase(&bytes[..cut]);
        }
    }
}

#[test]
fn garbage_after_a_valid_document_still_reads_or_fails_cleanly() {
    let mut doc = small_document(8);
    let mid = doc.len() / 2;
    for b in &mut doc[mid..mid + 64] {
        *b ^= 0x5A;
    }
    read_all(doc);
}

// ------------------------------------------------------------------ fuzz regressions

/// `abr` fuzz target, 2026-10-19: a sample whose bounds are far apart (`right - left` overflowed
/// `i32`) panicked in builds with overflow checks.
#[test]
fn fuzz_abr_sample_bounds_overflow() {
    const CRASH: &[u8] = &[
    0x00, 0x06, 0x00, 0x02, 0x38, 0x42, 0x49, 0x4D, 0x73, 0x61, 0x6D, 0x70, 0x00, 0x00, 0x00, 0x23, 0x00, 0x00, 0x00, 0x1F, 0x74, 0x69, 0x00, 0x08,
    0x00, 0x00, 0xFF, 0x80, 0x40, 0x00, 0xFF, 0x00, 0x38, 0x42, 0x49, 0x4D, 0x64, 0x65, 0x71, 0x63, 0x00, 0x00, 0x00, 0xBF, 0x00, 0x00, 0x00, 0x10,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x72, 0x73, 0x68, 0x4F, 0x62, 0x6A, 0x63, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x63, 0x6F,
    0x6D, 0x70, 0x75, 0x74, 0x65, 0x64, 0x42, 0x72, 0x75, 0x73, 0x68, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x48, 0x72, 0x64, 0x6E, 0x56,
    0x6C, 0x4C, 0x73, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x62, 0x73, 0x68, 0x20, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x4E, 0x6D,
    0x20, 0x20, 0x54, 0x45, 0x58, 0x54, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x74, 0x00, 0x69, 0x00, 0x70, 0x00, 0x2D, 0x00, 0x31, 0x00,
    ];
    let _ = read_abr(CRASH);
}

#[test]
fn many_full_size_layers_are_rejected_by_the_total_budget() {
    // Each layer is a legal 16384 x 16384 canvas, but 40 of them need 40 GiB at full size.
    let layers = (0..40).map(|i| layer(&format!("L{i}"))).collect();
    let archive = document_archive("{16384, 16384}", 256, layers);
    assert!(is_limit(open(package(&[(
        "Document.archive",
        &archive,
        CompressionMethod::Deflated
    )]))));
}

#[test]
fn abr_repeat_runs_cannot_expand_past_the_decode_budget() {
    // 8192 x 8192 PackBits tips made of repeat runs: about 1 MB of input becomes 64 MB each.
    let side = 8192usize;
    let row: Vec<u8> = (0..side / 128).flat_map(|_| [0x81u8, 0]).collect();
    let mut entry = b"rep\0".to_vec();
    for v in [0i32, 0, side as i32, side as i32] {
        entry.extend_from_slice(&v.to_be_bytes());
    }
    entry.extend_from_slice(&[0, 8, 1]);
    for _ in 0..side {
        entry.extend_from_slice(&(row.len() as u16).to_be_bytes());
    }
    for _ in 0..side {
        entry.extend_from_slice(&row);
    }
    let mut samp = Vec::new();
    for _ in 0..20 {
        push_u32(&mut samp, entry.len() as u32);
        samp.extend_from_slice(&entry);
    }
    let mut abr = abr_with("samp", &samp);
    let preset = abr_with("desc", &abr_preset());
    abr.extend_from_slice(&preset[4..]);
    assert!(is_limit(read_abr(&abr)));
}
