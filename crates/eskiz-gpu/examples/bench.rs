//! Compositing benchmark: 20 layers at 4096 x 4096, printing milliseconds per frame.
//!
//! Run with `cargo run --release -p eskiz-gpu --example bench`. The numbers include the GPU
//! wait, because each frame ends by waiting for the queue to drain.

use eskiz_core::BlendMode;
use eskiz_filters::gaussian::GaussianParams;
use eskiz_filters::hsb::HsbParams;
use eskiz_gpu::{
    Adjuster, Adjustment, Compositor, GpuContext, GpuLayer, GpuMask, Rect, Region, Scene, SceneLayer,
};
use std::time::Instant;

const SIZE: u32 = 4096;
const LAYERS: usize = 20;
const FRAMES: usize = 10;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Cheap deterministic pattern: a smooth gradient with soft shapes, mostly opaque so every
/// tile exists, with a bit of transparency.
fn pattern(seed: u32) -> Vec<u8> {
    let n = SIZE as usize;
    let mut v = vec![0u8; n * n * 4];
    let (fx, fy) = (0.003 + seed as f32 * 0.0007, 0.002 + seed as f32 * 0.0005);
    for y in 0..n {
        for x in 0..n {
            let s = ((x as f32 * fx + seed as f32).sin() * (y as f32 * fy).cos() + 1.0) * 0.5;
            let a = (0.35 + 0.65 * s).clamp(0.0, 1.0);
            let c = [
                (x * 255 / n) as f32 / 255.0,
                (y * 255 / n) as f32 / 255.0,
                s,
            ];
            let o = (y * n + x) * 4;
            for k in 0..3 {
                v[o + k] = (c[k] * a * 255.0).round() as u8;
            }
            v[o + 3] = (a * 255.0).round() as u8;
        }
    }
    v
}

fn main() {
    let ctx = match GpuContext::new() {
        Ok(c) => c,
        Err(e) => {
            println!("no GPU adapter available: {e}");
            return;
        }
    };
    println!("adapter: {}", ctx.describe());
    println!("canvas {SIZE} x {SIZE}, {LAYERS} layers, {} tiles per layer", (SIZE / 256) * (SIZE / 256));

    let t = Instant::now();
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
    let mut layers = Vec::new();
    for i in 0..LAYERS {
        let mut l = GpuLayer::new(SIZE, SIZE);
        l.upload_all(&ctx, &pattern(i as u32)).unwrap();
        layers.push(l);
    }
    ctx.wait_idle();
    let mb: f64 = layers.iter().map(|l| l.allocated_bytes() as f64).sum::<f64>() / (1024.0 * 1024.0);
    println!("upload of {LAYERS} layers: {:.0} ms ({mb:.0} MiB of tiles)", ms(t));
    let mut mask = GpuMask::new(SIZE, SIZE, 1.0);
    mask.upload_rect(
        &ctx,
        Rect::new(0, 0, 1024, 1024),
        &vec![128u8; 1024 * 1024],
    )
    .unwrap();

    let scene = |blend_mix: bool| {
        let mut s = Scene::new();
        for (i, l) in layers.iter().enumerate() {
            let mut sl = SceneLayer::new(l).with_opacity(0.6 + 0.02 * i as f32);
            if blend_mix {
                sl = sl.with_blend(modes[i % modes.len()]);
            }
            if i % 7 == 3 {
                sl = sl.with_mask(&mask);
            }
            if i % 5 == 4 && blend_mix {
                sl = sl.clipped();
            }
            s.push_layer(sl);
        }
        s
    };

    let mut comp = Compositor::new(&ctx, SIZE, SIZE);
    for (label, mix) in [("all Normal", false), ("mixed modes + masks + clips", true)] {
        let sc = scene(mix);
        // Warm up pipelines and driver caches.
        comp.composite(&ctx, &sc, &Region::All).unwrap();
        ctx.wait_idle();
        let t = Instant::now();
        let mut dispatches = 0;
        for _ in 0..FRAMES {
            dispatches = comp.composite(&ctx, &sc, &Region::All).unwrap().dispatches;
            ctx.wait_idle();
        }
        println!(
            "full recomposite ({label}): {:.1} ms/frame over {FRAMES} frames ({dispatches} dispatches)",
            ms(t) / FRAMES as f64
        );
    }

    // Dirty rectangle: one brush-stroke-sized region, as during painting.
    let sc = scene(true);
    for (label, r) in [
        ("256 x 256 (1 tile)", Rect::new(1024, 1024, 256, 256)),
        ("1024 x 1024 (16 tiles)", Rect::new(512, 512, 1024, 1024)),
    ] {
        comp.composite(&ctx, &sc, &Region::Rect(r)).unwrap();
        ctx.wait_idle();
        let t = Instant::now();
        for _ in 0..FRAMES * 5 {
            comp.composite(&ctx, &sc, &Region::Rect(r)).unwrap();
            ctx.wait_idle();
        }
        println!("dirty rect {label}: {:.2} ms/frame", ms(t) / (FRAMES * 5) as f64);
    }

    // Read-back of the whole output (export path).
    let t = Instant::now();
    let out = comp.download_output_all(&ctx).unwrap();
    println!("download of the 4096 x 4096 output: {:.0} ms ({} MiB)", ms(t), out.len() >> 20);

    // Adjustments on one full layer.
    let mut adj = Adjuster::new(&ctx);
    for (label, a) in [
        ("HSB", Adjustment::Hsb(HsbParams { hue: 0.2, saturation: 0.3, brightness: 0.0 })),
        ("Gaussian amount 0.6", Adjustment::Gaussian(GaussianParams { amount: 0.6 })),
        ("Gaussian amount 1.0", Adjustment::Gaussian(GaussianParams { amount: 1.0 })),
    ] {
        adj.apply(&ctx, &layers[0], None, &a).unwrap();
        ctx.wait_idle();
        let t = Instant::now();
        for _ in 0..3 {
            let r = adj.apply(&ctx, &layers[0], None, &a).unwrap();
            ctx.wait_idle();
            drop(r);
        }
        println!("adjustment {label}: {:.1} ms per 4096 x 4096 layer", ms(t) / 3.0);
    }
}
