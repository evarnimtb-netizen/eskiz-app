//! Property tests: random inputs through the encoders and back through the readers.

use std::collections::BTreeMap;
use std::io::Cursor;

use eskiz_format::brushes::{read_brush, write_brush, ImportedBrush, SettingValue};
use eskiz_format::codecs::lz4_block_decompress;
use eskiz_format::keyedarchive::{self, NsObject, NsValue};
use eskiz_format::{
    apple_lz4_compress, apple_lz4_decompress, lz4_block_compress, unarchive, Document, DocumentBuilder, LayerSpec,
    Node, Object, TilePixels,
};
use proptest::collection::vec;
use proptest::prelude::*;

/// Bytes that compress (runs of repeated bytes, repeated phrases) or do not (noise).
fn tile_bytes() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        vec(any::<u8>(), 0..4096),
        vec((any::<u8>(), 1usize..600), 0..40).prop_map(|runs| {
            runs.into_iter().flat_map(|(b, n)| std::iter::repeat_n(b, n)).collect()
        }),
        (vec(any::<u8>(), 1..24), 1usize..400).prop_map(|(phrase, n)| phrase.repeat(n)),
    ]
}

proptest! {
    #[test]
    fn lz4_tiles_round_trip(data in tile_bytes()) {
        let framed = apple_lz4_compress(&data);
        prop_assert_eq!(&apple_lz4_decompress(&framed, data.len()).unwrap(), &data);
        // a cap one byte short of the payload is refused
        if !data.is_empty() {
            prop_assert!(apple_lz4_decompress(&framed, data.len() - 1).is_err());
        }
        let block = lz4_block_compress(&data);
        let mut out = Vec::new();
        lz4_block_decompress(&block, &mut out, data.len()).unwrap();
        prop_assert_eq!(out, data);
    }
}

/// A sparse premultiplied layer: a few opaque or translucent rectangles on transparency.
#[derive(Debug, Clone)]
struct Sparse {
    w: usize,
    h: usize,
    tile: u32,
    bits: u32,
    rects: Vec<(usize, usize, usize, usize, [u16; 4])>,
}

impl Sparse {
    /// Premultiplied RGBA16 samples.
    fn pixels16(&self) -> Vec<u16> {
        let mut px = vec![0u16; self.w * self.h * 4];
        for &(x, y, rw, rh, c) in &self.rects {
            for yy in y..(y + rh).min(self.h) {
                for xx in x..(x + rw).min(self.w) {
                    px[(yy * self.w + xx) * 4..][..4].copy_from_slice(&c);
                }
            }
        }
        px
    }
}

fn sparse() -> impl Strategy<Value = Sparse> {
    let colour = (any::<u16>(), any::<u16>(), any::<u16>(), 1u16..).prop_map(|(r, g, b, a)| {
        [r % (a + 1), g % (a + 1), b % (a + 1), a] // premultiplied: no channel above alpha
    });
    (1usize..90, 1usize..90, prop::sample::select(vec![8u32, 16, 32, 64, 256]), prop::sample::select(vec![8u32, 16]))
        .prop_flat_map(move |(w, h, tile, bits)| {
            vec((0..w, 0..h, 1usize..40, 1usize..40, colour.clone()), 0..5)
                .prop_map(move |rects| Sparse { w, h, tile, bits, rects })
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn sparse_layers_round_trip_through_tiles(layer in sparse()) {
        let px16 = layer.pixels16();
        let px8: Vec<u8> = px16.iter().map(|&v| ((u32::from(v) + 128) / 257) as u8).collect();
        let mut b = DocumentBuilder::new(layer.w as u32, layer.h as u32);
        b.tile_size = layer.tile;
        b.bits_per_channel = layer.bits;
        b.tree = vec![Node::Layer(LayerSpec {
            name: "Sparse".into(),
            pixels: (layer.bits == 8).then(|| px8.clone()),
            pixels16: (layer.bits == 16).then(|| px16.clone()),
            ..Default::default()
        })];
        let bytes = b.to_bytes().unwrap();
        let mut doc = Document::from_reader(Cursor::new(bytes)).unwrap();
        let read = doc.layers[0].clone();
        if layer.bits == 8 {
            prop_assert_eq!(&doc.layer_rgba(&read).unwrap(), &px8);
        } else {
            prop_assert_eq!(&doc.layer_rgba16(&read).unwrap(), &px16);
        }

        // the tiles hold exactly the non-empty grid cells, and put back together give the layer
        let t = layer.tile as usize;
        let mut rebuilt = vec![0u16; layer.w * layer.h * 4];
        let tiles = doc.layer_tiles(&read, 16).unwrap();
        let mut cells = std::collections::HashSet::new();
        for tile in &tiles {
            let TilePixels::U16(samples) = &tile.pixels else { panic!("asked for 16 bits") };
            prop_assert!(samples.iter().any(|&v| v != 0));
            cells.insert((tile.col, tile.row));
            for y in 0..t {
                for x in 0..t {
                    let (cx, cy) = (tile.col * t + x, tile.row * t + y);
                    let src = &samples[(y * t + x) * 4..][..4];
                    if cx < layer.w && cy < layer.h {
                        rebuilt[(cy * layer.w + cx) * 4..][..4].copy_from_slice(src);
                    } else {
                        prop_assert_eq!(src, &[0u16; 4][..], "margin past the layer edge is zero");
                    }
                }
            }
        }
        let expected16: Vec<u16> = if layer.bits == 8 { px8.iter().map(|&v| u16::from(v) * 257).collect() } else { px16 };
        prop_assert_eq!(rebuilt, expected16.clone());
        let painted: std::collections::HashSet<(usize, usize)> = expected16
            .as_chunks::<4>().0.iter()
            .enumerate()
            .filter(|(_, p)| p.iter().any(|&v| v != 0))
            .map(|(i, _)| ((i % layer.w) / t, (i / layer.w) / t))
            .collect();
        prop_assert_eq!(cells, painted);
    }
}

/// A field value the keyed-archive writer stores and the reader gives back unchanged.
fn ns_value() -> impl Strategy<Value = NsValue> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(NsValue::Int),
        any::<f64>().prop_filter("finite", |f| f.is_finite()).prop_map(NsValue::Real),
        any::<bool>().prop_map(NsValue::Bool),
        "[ -~]{0,20}".prop_filter("not the null marker", |s| s != "$null").prop_map(NsValue::String),
        vec(any::<u8>(), 0..40).prop_map(NsValue::Data),
    ];
    leaf.prop_recursive(3, 24, 6, |inner| {
        vec(inner, 0..6).prop_map(|items| NsValue::Array { items, mutable: false })
    })
}

/// Checks that a decoded object equals the value that was written.
fn same(written: &NsValue, read: &Object) -> bool {
    match (written, read) {
        (NsValue::Int(a), Object::Int(b)) => a == b,
        (NsValue::Real(a), Object::Real(b)) => a.to_bits() == b.to_bits(),
        (NsValue::Bool(a), Object::Bool(b)) => a == b,
        (NsValue::String(a), Object::String(b)) => a == b,
        (NsValue::Data(a), Object::Data(b)) => a == b,
        (NsValue::Array { items, .. }, Object::Array(read)) => {
            items.len() == read.len() && items.iter().zip(read).all(|(w, r)| same(w, r))
        }
        _ => false,
    }
}

proptest! {
    #[test]
    fn keyed_archives_keep_unknown_keys(
        fields in prop::collection::btree_map("[a-zA-Z][a-zA-Z0-9_.]{0,15}", ns_value(), 0..12),
        class in "[A-Z][A-Za-z]{0,12}",
    ) {
        let mut object = NsObject::new(class.clone());
        for (key, value) in &fields {
            object = object.field(key.clone(), value.clone());
        }
        let bytes = keyedarchive::write(&NsValue::Object(object)).unwrap();
        let root = unarchive(&bytes).unwrap();
        let read = root.as_archived().expect("an archived object");
        prop_assert_eq!(&read.class_name, &class);
        prop_assert_eq!(read.fields.len(), fields.len());
        for (key, value) in &fields {
            let got = read.get(key);
            prop_assert!(got.is_some_and(|g| same(value, g)), "{key}: wrote {value:?}, read {got:?}");
        }
    }

    #[test]
    fn brush_settings_keep_unknown_keys(
        settings in prop::collection::btree_map(
            "zz[a-zA-Z0-9]{1,12}",
            prop_oneof![
                any::<f64>().prop_filter("finite", |f| f.is_finite()).prop_map(SettingValue::Number),
                any::<bool>().prop_map(SettingValue::Bool),
                "[ -~]{0,16}".prop_filter("not the null marker", |s| s != "$null").prop_map(SettingValue::Text),
            ],
            0..10,
        ),
    ) {
        let brush = ImportedBrush { name: "Prop".into(), settings: settings.clone(), ..Default::default() };
        let read = read_brush("hint", &write_brush(&brush).unwrap()).unwrap();
        prop_assert_eq!(read.name, "Prop");
        let read: BTreeMap<String, SettingValue> = read.settings;
        prop_assert_eq!(read, settings);
    }
}
