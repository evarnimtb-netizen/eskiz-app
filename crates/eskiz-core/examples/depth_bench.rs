//! Cost of painting at 8 and 16 bits on a 2048x2048 layer:
//! `cargo run --release -p eskiz-core --example depth_bench`.
//!
//! Times a 300-sample dry stroke (60 px) and a 120-sample wet stroke (100 px) and reports
//! milliseconds per stroke sample, best of several runs, onto fresh tiles and onto tiles that
//! already exist.

use std::time::Instant;

use eskiz_core::brushes::{Brush, Preset, Shape, Wet};
use eskiz_core::geometry::{PointerType, Sample, Stroke, StrokeOptions};
use eskiz_core::{Layer, PaintOptions};

const N: usize = 2048;

fn stroke(brush: Brush, samples: usize) -> Stroke {
    let mut s = Stroke::new(brush, StrokeOptions { seed: 5, ..StrokeOptions::default() });
    for i in 0..samples {
        let p = i as f64 / samples as f64;
        s.add_sample(Sample {
            x: 100.0 + 1800.0 * p,
            y: 1024.0 + 500.0 * (p * 7.0).sin(),
            t: i as f64 / 120.0,
            pressure: Some(0.3 + 0.7 * (p * std::f64::consts::PI).sin()),
            has_pressure: Some(true),
            pointer: PointerType::Pen,
            ..Sample::default()
        });
    }
    s.finish();
    s
}

fn best(n: usize, mut f: impl FnMut()) -> f64 {
    (0..n)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::MAX, f64::min)
}

fn layer(sixteen: bool) -> Layer {
    Layer::new_depth(N, N, if sixteen { eskiz_core::Depth::U16 } else { eskiz_core::Depth::U8 })
}

fn main() {
    let mut ink = Brush::preset(Preset::InkWithTaper);
    ink.size = 60.0;
    let dry = stroke(ink, 300);
    let wet_brush = Brush { size: 100.0, shape: Shape::SoftRound, smoothing: 0.0, spacing: 0.06, wet: Wet { enabled: true, dilution: 0.6, pull: 0.6, ..Wet::default() }, ..Brush::default() };
    let wet = stroke(wet_brush, 120);
    let depths = [false, true];
    for sixteen in depths {
        let label = if sixteen { "16-bit" } else { " 8-bit" };
        let opts = PaintOptions::new([0.1, 0.4, 0.8]);
        let cold = best(7, || {
            let mut l = layer(sixteen);
            l.paint_stroke_with(&dry, opts);
        });
        let mut warm = layer(sixteen);
        warm.paint_stroke_with(&dry, opts);
        let warm_ms = best(7, || warm.paint_stroke_with(&dry, opts));
        let wet_ms = best(5, || {
            let mut l = layer(sixteen);
            l.paint_stroke_with(&wet, opts);
        });
        println!(
            "{label}: dry stroke {:.3} ms/sample (fresh tiles) {:.3} ms/sample (existing tiles) | wet stroke {:.3} ms/sample",
            cold / 300.0,
            warm_ms / 300.0,
            wet_ms / 120.0
        );
    }
}
