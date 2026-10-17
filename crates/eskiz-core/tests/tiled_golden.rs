//! Golden hashes captured from the flat-buffer implementation, before tiled storage. The
//! tiled layer, stroke composite and compositor must reproduce them bit for bit.

use eskiz_core::brushes::{Brush, Preset};
use eskiz_core::composite::{composite, composite_rect, composite_to_layer, CompositeOpts};
use eskiz_core::document::{Document, Mask, Props};
use eskiz_core::geometry::{PointerType, Sample, Stroke, StrokeOptions};
use eskiz_core::{BlendMode, Layer, PaintOptions};

const W: usize = 700;
const H: usize = 530;

fn fnv(bytes: impl Iterator<Item = u8>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn layer_hash(l: &Layer) -> u64 {
    fnv((0..l.height).flat_map(|y| (0..l.width).map(move |x| (x, y))).flat_map(|(x, y)| l.pixel(x, y)))
}

/// Deterministic premultiplied pixels with transparent holes and a non-aligned pattern.
fn pattern(seed: usize) -> Vec<u8> {
    let mut v = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            if (x / 37 + y / 29 + seed).is_multiple_of(3) || (x > 300 && x < 520 && y < 200 && seed == 1) {
                continue;
            }
            let a = (40 + ((x * 5 + y * 3 + seed * 17) % 216)) as u8;
            let c = |k: usize| (((x * (k + 1) + y * (k + 2) + seed * 31) % 256) as u32 * u32::from(a) / 255) as u8;
            let i = (y * W + x) * 4;
            v[i..i + 4].copy_from_slice(&[c(0), c(1), c(2), a]);
        }
    }
    v
}

fn doc() -> Document {
    let mut d = Document::empty(W, H);
    d.background = [0.9, 0.85, 0.7];
    d.push_layer(None, Props::new("base"), None, pattern(0));
    let mut p = Props::new("mul");
    p.blend = BlendMode::Multiply;
    p.opacity = 0.8;
    let mask = Mask {
        data: (0..W * H).map(|i| ((i * 7) % 256) as u8).collect(),
    };
    d.push_layer(None, p, Some(mask), pattern(1));
    let mut c = Props::new("clip");
    c.clipped = true;
    c.blend = BlendMode::SoftLight;
    d.push_layer(None, c, None, pattern(2));
    let g = d.push_group(None, Props::new("g"), None);
    let mut s = Props::new("inner");
    s.blend = BlendMode::Screen;
    s.opacity = 0.6;
    d.push_layer(Some(g), s, None, pattern(3));
    d
}

fn stroke() -> Stroke {
    let mut brush = Brush::preset(Preset::InkWithTaper);
    brush.size = 40.0;
    let mut s = Stroke::new(
        brush,
        StrokeOptions {
            seed: 5,
            ..StrokeOptions::default()
        },
    );
    for i in 0..120 {
        let p = f64::from(i) / 120.0;
        s.add_sample(Sample {
            x: 20.0 + 660.0 * p,
            y: 265.0 + 180.0 * (p * 9.0).sin(),
            t: f64::from(i) / 120.0,
            pressure: Some(0.2 + 0.8 * (p * std::f64::consts::PI).sin()),
            has_pressure: Some(true),
            pointer: PointerType::Pen,
            ..Sample::default()
        });
    }
    s.finish();
    s
}

#[test]
fn golden_hashes() {
    let d = doc();
    let full = composite(&d, CompositeOpts::default());
    let h_full = fnv(full.iter().copied());
    // Unaligned sub-rectangle spanning several 256-pixel tile edges.
    let part = composite_rect(&d, 100, 90, 480, 400, CompositeOpts::default());
    let h_part = fnv(part.iter().copied());
    let h_layer = fnv(composite_to_layer(&d, false).iter().copied());

    let mut l = Layer::new(W, H);
    l.paint_stroke_with(&stroke(), PaintOptions::new([0.1, 0.4, 0.8]));
    let h_stroke = layer_hash(&l);
    let mut e = PaintOptions::new([0.0; 3]);
    e.erase = true;
    l.paint_stroke_with(&stroke(), e);
    let h_erase = layer_hash(&l);
    println!("GOLDEN {h_full:#x} {h_part:#x} {h_layer:#x} {h_stroke:#x} {h_erase:#x}");
    assert_eq!(
        [h_full, h_part, h_layer, h_stroke, h_erase],
        GOLDEN,
        "composite or stroke output changed"
    );
}

const GOLDEN: [u64; 5] = [
    0x8d82_49b1_3412_2116,
    0x8dfe_8609_a7a7_0ee9,
    0x748f_6a4b_df83_5359,
    0x6ae3_21f0_c8e1_0edc,
    0x3a6c_598a_2ed6_5c67,
];
