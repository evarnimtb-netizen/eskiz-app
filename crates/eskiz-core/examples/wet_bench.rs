//! Wet-mix cost on a 2048x2048 layer with a 100 px brush:
//! `cargo run --release -p eskiz-core --example wet_bench`.
//!
//! Reports milliseconds per dab for the whole wet pipeline (pickup, carry, mix, splat,
//! composite), for a plain brush on the same stroke, and for a wet brush with wetness and bleed.

use std::time::Instant;

use eskiz_core::brushes::{Brush, Shape, Wet};
use eskiz_core::geometry::{PointerType, Sample, Stroke, StrokeOptions};
use eskiz_core::raster::{CoverageBuffer, DabRaster};
use eskiz_core::wet::{ComposeCtx, Rect, WetStroke};
use eskiz_core::{Layer, PaintOptions};

const W: usize = 2048;
const H: usize = 2048;

fn base_layer() -> Layer {
    // A fully painted, textured base so the pickup reads real tiles.
    let mut buf = vec![0u8; W * H * 4];
    for (i, px) in buf.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % W, i / W);
        px.copy_from_slice(&[(x / 8 % 256) as u8, (y / 8 % 256) as u8, ((x ^ y) % 256) as u8, 255]);
    }
    Layer::from_rgba(W, H, buf)
}

fn stroke(brush: &Brush) -> Stroke {
    let mut s = Stroke::new(brush.clone(), StrokeOptions { seed: 5, ..StrokeOptions::default() });
    for i in 0..400 {
        let p = f64::from(i) / 400.0;
        s.add_sample(Sample {
            x: 150.0 + 1750.0 * p,
            y: 1024.0 + 600.0 * (p * 9.0).sin(),
            t: f64::from(i) / 120.0,
            pressure: Some(0.5 + 0.5 * (p * std::f64::consts::PI).sin()),
            has_pressure: Some(true),
            pointer: PointerType::Pen,
            ..Sample::default()
        });
    }
    s.finish();
    s
}

fn brush(wet: Option<Wet>) -> Brush {
    Brush {
        size: 100.0,
        shape: Shape::SoftRound,
        smoothing: 0.0,
        spacing: 0.06,
        wet: wet.unwrap_or_default(),
        ..Brush::default()
    }
}

fn time_whole(b: &Brush, base: &Layer) -> (f64, usize) {
    let s = stroke(b);
    let n = s.dabs().len();
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut l = base.clone();
        let t = Instant::now();
        l.paint_stroke_with(&s, PaintOptions::new([0.9, 0.2, 0.1]));
        std::hint::black_box(&l);
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    (best, n)
}

/// The live-engine shape of the work: per dab, apply then recomposite the dab's rectangle.
fn time_live(b: &Brush, base: &Layer) -> f64 {
    let s = stroke(b);
    let raster = DabRaster::new(b);
    let colour = [0.9, 0.2, 0.1];
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut layer = base.clone();
        let before = layer.clone();
        let mut cov = CoverageBuffer::new(0, 0, W, H);
        let mut wet = WetStroke::new(0, 0, W, H, &b.wet, 1);
        let ctx = ComposeCtx { base: colour, brush: b, raster: &raster, alpha_lock: false, clip: None };
        let t = Instant::now();
        for d in s.dabs() {
            if let Some(r) = wet.apply_dab(0, d, colour, &raster, &mut cov, &before) {
                let r: Rect = r;
                wet.composite(&mut layer, &before, &cov, r, &ctx);
            }
        }
        std::hint::black_box(&layer);
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    best / s.dabs().len() as f64
}

fn main() {
    let base = base_layer();
    let mixing = Wet { enabled: true, dilution: 0.4, pull: 0.6, load_amount: 0.3, falloff_amount: 0.3, ..Wet::default() };
    let soft = Wet { wetness: 0.5, bleed: 0.4, ..mixing };

    for (name, b) in [("plain", brush(None)), ("wet mix", brush(Some(mixing))), ("wet mix + wetness/bleed", brush(Some(soft)))] {
        let (ms, n) = time_whole(&b, &base);
        println!("{name:<26} {n} dabs: whole stroke {ms:7.1} ms, {:.3} ms/dab", ms / n as f64);
    }
    println!("live engine shape (per-dab composite):");
    for (name, b) in [("wet mix", brush(Some(mixing))), ("wet mix + wetness/bleed", brush(Some(soft)))] {
        println!("{name:<26} {:.3} ms/dab", time_live(&b, &base));
    }
}
