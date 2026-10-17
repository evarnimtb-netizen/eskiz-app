//! Quick timing of the hot paths on a 2048x1536 canvas:
//! `cargo run --release -p eskiz-core --example tile_bench`.

use std::time::Instant;

use eskiz_core::brushes::{Brush, Preset};
use eskiz_core::composite::{composite, composite_rect, CompositeOpts};
use eskiz_core::document::{Document, Props};
use eskiz_core::geometry::{PointerType, Sample, Stroke, StrokeOptions};
use eskiz_core::{Layer, PaintOptions};

const W: usize = 2048;
const H: usize = 1536;

fn stroke() -> Stroke {
    let mut brush = Brush::preset(Preset::InkWithTaper);
    brush.size = 60.0;
    let mut s = Stroke::new(
        brush,
        StrokeOptions {
            seed: 5,
            ..StrokeOptions::default()
        },
    );
    for i in 0..300 {
        let p = f64::from(i) / 300.0;
        s.add_sample(Sample {
            x: 100.0 + 1800.0 * p,
            y: 768.0 + 500.0 * (p * 7.0).sin(),
            t: f64::from(i) / 120.0,
            pressure: Some(0.3 + 0.7 * (p * std::f64::consts::PI).sin()),
            has_pressure: Some(true),
            pointer: PointerType::Pen,
            ..Sample::default()
        });
    }
    s.finish();
    s
}

fn best<R>(n: usize, mut f: impl FnMut() -> R) -> f64 {
    (0..n)
        .map(|_| {
            let t = Instant::now();
            std::hint::black_box(f());
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::MAX, f64::min)
}

fn main() {
    let s = stroke();
    let stroke_ms = best(5, || {
        let mut l = Layer::new(W, H);
        l.paint_stroke_with(&s, PaintOptions::new([0.1, 0.4, 0.8]));
        l
    });
    // The same stroke onto tiles that already exist: separates allocation from compositing.
    let mut warm = Layer::new(W, H);
    warm.paint_stroke_with(&s, PaintOptions::new([0.1, 0.4, 0.8]));
    let warm_ms = best(5, || warm.paint_stroke_with(&s, PaintOptions::new([0.1, 0.4, 0.8])));
    println!("warm stroke {warm_ms:.1} ms");
    // Three layers, two of them painted, over a background.
    let mut d = Document::empty(W, H);
    for k in 0..3 {
        let mut l = Layer::new(W, H);
        if k > 0 {
            l.paint_stroke_with(&s, PaintOptions::new([0.2 * k as f64, 0.4, 0.8]));
        }
        let mut buf = vec![0u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                buf[(y * W + x) * 4..(y * W + x) * 4 + 4].copy_from_slice(&l.pixel(x, y));
            }
        }
        if k == 0 {
            // A dense, fully painted base layer.
            for (i, px) in buf.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                px.copy_from_slice(&[(i % 200) as u8, 120, 90, 255]);
            }
        }
        d.push_layer(None, Props::new("l"), None, buf);
    }
    let rect_ms = best(10, || composite_rect(&d, 300, 300, 512, 512, CompositeOpts::default()));
    let full_ms = best(5, || composite(&d, CompositeOpts::default()));
    println!("stroke {stroke_ms:.1} ms | rect512 {rect_ms:.1} ms | full {full_ms:.1} ms");
}
