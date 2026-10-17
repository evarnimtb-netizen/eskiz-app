//! Deterministic inputs: canvas patterns, documents in the layer configurations the benchmark
//! compares, stroke paths and the brushes.

use std::sync::Arc;

use eskiz_app::brush_library::engine_for;
use eskiz_app::input::{sample_from, PointerKind, RawPointer};
use eskiz_app::paint::StrokeSettings;
use eskiz_app::session::Session;
use eskiz_core::brushes::{Brush, Preset};
use eskiz_core::document::{Document, Props};
use eskiz_core::geometry::Sample;
use eskiz_core::raster::ShapeTexture;
use eskiz_core::{BlendMode, Layer};

/// Bytes one fully painted 8-bit layer of `n x n` pixels takes in tiles.
pub fn layer_bytes(n: usize) -> u64 {
    (n as u64) * (n as u64) * 4
}

/// A cheap deterministic premultiplied RGBA8 pattern that allocates every tile: a gradient with
/// soft bands and some semi-transparent pixels, varied by `seed`.
pub fn pattern(n: usize, seed: u32) -> Vec<u8> {
    let mut v = vec![0u8; n * n * 4];
    let s = seed.wrapping_mul(0x9E37_79B9);
    for y in 0..n {
        let row = &mut v[y * n * 4..(y + 1) * n * 4];
        let yb = ((y as u32).wrapping_mul(7).wrapping_add(s) >> 4) as u8;
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let xb = ((x as u32).wrapping_mul(5).wrapping_add(s >> 8) >> 4) as u8;
            // Mostly opaque, a few translucent diagonal bands, never fully transparent.
            let a = if ((x + y) / 97) % 7 == 0 { 140u16 } else { 255u16 };
            let r = (x * 255 / n) as u16;
            let g = (y * 255 / n) as u16;
            let b = (xb ^ yb) as u16;
            *px = [(r * a / 255) as u8, (g * a / 255) as u8, (b * a / 255) as u8, a as u8];
        }
    }
    v
}

/// A fully painted layer.
pub fn full_layer(n: usize, seed: u32) -> Layer {
    Layer::from_rgba(n, n, pattern(n, seed))
}

/// Which layers a document has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// One fully painted layer.
    OneFull,
    /// 20 layers with three short ink strokes each.
    SparseN(usize),
    /// `n` fully painted layers, all Normal.
    FullN(usize),
    /// 20 fully painted layers with non-Normal blend modes, opacity, a clip and two groups.
    MixedFull20,
}

impl Layout {
    pub fn label(self) -> String {
        match self {
            Layout::OneFull => "1 full".into(),
            Layout::SparseN(n) => format!("{n} sparse"),
            Layout::FullN(n) => format!("{n} full"),
            Layout::MixedFull20 => "20 full, mixed blend + groups".into(),
        }
    }

    /// Pixel bytes the layers need (sparse strokes are counted roughly).
    pub fn bytes(self, n: usize) -> u64 {
        match self {
            Layout::OneFull => layer_bytes(n),
            Layout::SparseN(k) => (k as u64) * layer_bytes(n) / 12,
            Layout::FullN(k) => (k as u64) * layer_bytes(n),
            Layout::MixedFull20 => 20 * layer_bytes(n),
        }
    }

    pub fn layers(self) -> usize {
        match self {
            Layout::OneFull => 1,
            Layout::SparseN(k) | Layout::FullN(k) => k,
            Layout::MixedFull20 => 20,
        }
    }
}

/// Short ink strokes on a layer, for sparse documents. `k` picks the placement.
fn sparse_strokes(layer: &mut Layer, n: usize, k: usize) {
    let mut brush = Brush::preset(Preset::InkWithTaper);
    brush.size = (n as f64 / 80.0).max(6.0);
    for s in 0..3 {
        let mut stroke = eskiz_core::Stroke::new(brush.clone(), eskiz_core::StrokeOptions { seed: (k * 3 + s) as u32 + 1, ..Default::default() });
        let (cx, cy) = (n as f64 * (0.15 + 0.7 * ((k * 7 + s * 3) % 10) as f64 / 10.0), n as f64 * (0.15 + 0.7 * ((k * 3 + s * 5) % 10) as f64 / 10.0));
        let len = n as f64 * 0.25;
        for i in 0..60 {
            let p = i as f64 / 59.0;
            let a = p * 2.5 + s as f64;
            stroke.add_sample(Sample {
                x: cx + len * p * a.cos(),
                y: cy + len * p * a.sin(),
                t: i as f64 / 120.0,
                pressure: Some(0.3 + 0.6 * (p * std::f64::consts::PI).sin()),
                has_pressure: Some(true),
                ..Default::default()
            });
        }
        stroke.finish();
        layer.paint_stroke(&stroke, [0.1 * s as f64, 0.2, 0.7]);
    }
}

/// Build a document of `n x n` pixels in `layout`.
pub fn document(n: usize, layout: Layout) -> Document {
    let mut doc = Document::empty(n, n);
    match layout {
        Layout::OneFull => {
            doc.push_raster(None, Props::new("Layer 1"), None, full_layer(n, 1));
        }
        Layout::SparseN(k) => {
            for i in 0..k {
                let mut l = Layer::new(n, n);
                sparse_strokes(&mut l, n, i);
                doc.push_raster(None, Props::new(format!("Layer {}", i + 1)), None, l);
            }
        }
        Layout::FullN(k) => {
            for i in 0..k {
                let mut p = Props::new(format!("Layer {}", i + 1));
                p.opacity = 0.9;
                doc.push_raster(None, p, None, full_layer(n, i as u32 + 1));
            }
        }
        Layout::MixedFull20 => {
            let modes = [
                BlendMode::Normal,
                BlendMode::Multiply,
                BlendMode::Screen,
                BlendMode::Overlay,
                BlendMode::SoftLight,
                BlendMode::Color,
                BlendMode::Luminosity,
                BlendMode::Add,
            ];
            // Two groups (Normal, opacity 1, as the GPU compositor supports) holding 5 layers each,
            // then 10 top-level layers.
            let mut parents = vec![None; 20];
            for g in 0..2 {
                let gid = doc.push_group(None, Props::new(format!("Group {}", g + 1)), None);
                for i in 0..5 {
                    parents[g * 5 + i] = Some(gid);
                }
            }
            for i in 0..20 {
                let mut p = Props::new(format!("Layer {}", i + 1));
                p.blend = modes[i % modes.len()];
                p.opacity = 0.6 + 0.02 * i as f64;
                p.clipped = i % 5 == 4;
                doc.push_raster(parents[i], p, None, full_layer(n, i as u32 + 1));
            }
        }
    }
    doc
}

/// The bottom-most layer id of a document (the layer the benchmarks paint on).
pub fn first_layer(doc: &Document) -> u64 {
    doc.layer_ids()[0]
}

/// The top-most layer id.
pub fn top_layer(doc: &Document) -> u64 {
    *doc.layer_ids().last().expect("document has layers")
}

/// A long stylus path: `count` samples at 120 Hz along a loose spiral that crosses most of the
/// canvas, with pressure rising and falling. Deterministic.
pub fn stroke_path(n: usize, count: usize) -> Vec<Sample> {
    let c = n as f64 / 2.0;
    let r_max = n as f64 * 0.42;
    (0..count)
        .map(|i| {
            let p = i as f64 / (count - 1) as f64;
            let a = p * 4.0 * std::f64::consts::PI;
            let r = r_max * (0.15 + 0.85 * p);
            let raw = RawPointer {
                x: c + r * a.cos(),
                y: c + r * a.sin() * 0.85,
                time: i as f64 / 120.0,
                kind: PointerKind::Touch,
                force: Some(0.25 + 0.7 * (p * std::f64::consts::PI).sin() * (0.8 + 0.2 * (p * 40.0).sin())),
            };
            sample_from(raw)
        })
        .collect()
}

/// A brush the painting benchmark uses, with the engine resources the app builds for it.
pub struct BenchBrush {
    pub label: &'static str,
    pub settings: StrokeSettings,
    /// The tip, for the smudge tool.
    pub tip: Arc<ShapeTexture>,
}

fn library_brush(id: &str) -> eskiz_brushlib::LibraryBrush {
    eskiz_brushlib::library_ref()
        .iter()
        .flat_map(|s| s.brushes.iter())
        .find(|b| b.id == id)
        .unwrap_or_else(|| panic!("library brush {id} missing"))
        .clone()
}

fn bench_brush(label: &'static str, id: &str, size: f64) -> BenchBrush {
    let lb = library_brush(id);
    let engine = engine_for(&lb);
    let mut brush = engine.brush.clone();
    brush.size = size;
    BenchBrush {
        label,
        settings: StrokeSettings { brush, colour: [0.15, 0.3, 0.8], erase: false, raster: Some(engine.raster.clone()) },
        tip: engine.tip.clone(),
    }
}

/// The representative brushes: hard ink, a very large soft airbrush, a textured (grain) brush and
/// a wet-mixing watercolour, all from the built-in library with their real tip and grain textures.
pub fn brushes() -> Vec<BenchBrush> {
    vec![
        bench_brush("hard ink 30 px", "inking.studio-ink", 30.0),
        bench_brush("soft airbrush 500 px", "airbrushing.broad-aerograph", 500.0),
        bench_brush("textured dry brush 80 px", "texture.canvas-dry-brush", 80.0),
        bench_brush("wet watercolor 120 px", "watercolour.wet-bloom-round", 120.0),
    ]
}

/// A session over a fresh document.
pub fn session(n: usize, layout: Layout) -> Session {
    Session::new(document(n, layout))
}
