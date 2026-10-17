//! Cost of a large soft dab on a big canvas:
//! `cargo run --release -p eskiz-app --example dab_perf -- [canvas_edge] [dab_events] [brush_size]`.
//!
//! Paints a slow stroke with a 500 px soft round build-up brush (the airbrush case of
//! `eskiz-bench`) straight through `PaintEngine` and reports ms per dab-laying event and for
//! pen-up.

use std::time::Instant;

use eskiz_app::paint::{PaintEngine, StrokeSettings};
use eskiz_core::brushes::{Accumulation, Brush, Shape};
use eskiz_core::geometry::{PointerType, Sample};
use eskiz_core::Layer;

fn main() {
    let mut args = std::env::args().skip(1);
    let edge: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(4096);
    let events: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(120);
    let size_arg: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(500.0);
    let brush = Brush {
        size: size_arg,
        shape: Shape::SoftRound,
        spacing: 0.05,
        opacity: 0.7,
        flow: 0.1,
        accumulation: Accumulation::Buildup,
        ..Brush::default()
    };
    let settings = StrokeSettings { brush, colour: [0.15, 0.3, 0.8], erase: false, raster: None };
    let mut layer = Layer::new(edge, edge);
    let mut engine = PaintEngine::default();
    engine.begin(&layer, 1, &settings, 3, false);
    let mut times = Vec::new();
    let steps = events;
    for i in 0..steps {
        let p = i as f64 / steps as f64;
        let sample = Sample {
            x: 600.0 + 24.0 * i as f64,
            y: edge as f64 / 2.0 + 400.0 * (p * 8.0).sin(),
            t: i as f64 / 120.0,
            pressure: Some(0.8),
            has_pressure: Some(true),
            pointer: PointerType::Pen,
            ..Sample::default()
        };
        let t = Instant::now();
        let rect = engine.push(&mut layer, sample);
        if rect.is_some() {
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let t = Instant::now();
    engine.end(&mut layer);
    let up = t.elapsed().as_secs_f64() * 1000.0;
    times.sort_by(f64::total_cmp);
    let n = times.len().max(1);
    let rss = |key: &str| {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with(key)).map(|l| l.trim().to_owned()))
            .unwrap_or_default()
    };
    println!("{}  {}", rss("VmHWM"), rss("VmRSS"));
    println!("{edge}²: {} dab events, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms; pen-up {up:.1} ms", times.len(), times[n / 2], times[n * 95 / 100], times[n - 1]);
}
