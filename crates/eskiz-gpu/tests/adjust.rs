//! GPU adjustments against the `eskiz-filters` CPU references on random images.
//!
//! Tolerance: 1/255 per channel for everything except Noise, which is documented at the test.
mod common;
use common::*;
use eskiz_filters::curves::{Curve, CurveSet, CurvesParams};
use eskiz_filters::gaussian::GaussianParams;
use eskiz_filters::gradient_map::{self, GradientMapParams};
use eskiz_filters::hsb::HsbParams;
use eskiz_filters::noise::{CombineMode, NoiseKind, NoiseParams};
use eskiz_filters::opacity::OpacityParams;
use eskiz_filters::sharpen::SharpenParams;
use eskiz_filters::{gaussian, hsb, noise, opacity, sharpen, Image};
use eskiz_gpu::{Adjuster, Adjustment, GpuContext, GpuLayer, GpuMask, Rect};

const W: u32 = 300;
const H: u32 = 270;

struct Case {
    px: Vec<u8>,
    layer: GpuLayer,
    mask_bytes: Vec<u8>,
    mask: GpuMask,
}

fn case(ctx: &GpuContext, seed: u64, w: u32, h: u32) -> Case {
    let mut rng = Rng::new(seed);
    let px = random_premul(&mut rng, w as usize, h as usize, false);
    let mut layer = GpuLayer::new(w, h);
    layer.upload_all(ctx, &px).unwrap();
    let mask_bytes = random_mask(&mut rng, w as usize, h as usize);
    let mut mask = GpuMask::new(w, h, 1.0);
    mask.upload_rect(ctx, Rect::new(0, 0, w, h), &mask_bytes).unwrap();
    Case {
        px,
        layer,
        mask_bytes,
        mask,
    }
}

fn mask_f32(c: &Case) -> Vec<f32> {
    c.mask_bytes.iter().map(|&b| f32::from(b) / 255.0).collect()
}

fn run(
    ctx: &GpuContext,
    adj: &mut Adjuster,
    c: &Case,
    use_mask: bool,
    a: &Adjustment,
) -> Vec<u8> {
    let out = adj
        .apply(ctx, &c.layer, use_mask.then_some(&c.mask), a)
        .unwrap();
    out.download_all(ctx).unwrap()
}

fn cpu(c: &Case, w: u32, h: u32, f: impl Fn(&Image, Option<&[f32]>) -> Image, use_mask: bool) -> Vec<u8> {
    let img = Image::from_rgba8_premul(w as usize, h as usize, &c.px);
    let m = mask_f32(c);
    f(&img, use_mask.then_some(m.as_slice())).to_rgba8_premul()
}

#[test]
fn gaussian_matches_cpu() {
    let Some(ctx) = gpu("adjust::gaussian") else { return };
    let mut adj = Adjuster::new(&ctx);
    let c = case(&ctx, 40, W, H);
    for amount in [0.0, 0.02, 0.1, 0.3, 0.6, 1.0] {
        for use_mask in [false, true] {
            let got = run(&ctx, &mut adj, &c, use_mask, &Adjustment::Gaussian(GaussianParams { amount }));
            let want = cpu(&c, W, H, |i, m| gaussian::apply(i, m, &GaussianParams { amount }), use_mask);
            let worst = assert_close(&format!("gaussian {amount} mask={use_mask}"), &got, &want, 1, 0.0, 1);
            eprintln!("  gaussian amount {amount:<5} mask {use_mask:<5} worst {worst}");
        }
    }
}

#[test]
fn gaussian_spreads_into_empty_neighbour_tiles() {
    let Some(ctx) = gpu("adjust::gaussian_sparse") else { return };
    let mut adj = Adjuster::new(&ctx);
    // A single opaque block in tile (1, 1) of a 1024 x 1024 layer, near its top-left corner.
    let (w, h) = (1024u32, 1024u32);
    let mut layer = GpuLayer::new(w, h);
    let block = vec![255u8; 30 * 30 * 4];
    layer.upload_rect(&ctx, Rect::new(256, 256, 30, 30), &block).unwrap();
    let mut full = vec![0u8; (w * h * 4) as usize];
    for y in 256..286usize {
        for x in 256..286usize {
            full[(y * w as usize + x) * 4..][..4].copy_from_slice(&[255; 4]);
        }
    }
    let p = GaussianParams { amount: 1.0 };
    let out = adj.apply(&ctx, &layer, None, &Adjustment::Gaussian(p)).unwrap();
    assert!(out.has_tile((0, 0)) && out.has_tile((1, 0)) && out.has_tile((0, 1)));
    assert!(out.has_tile((2, 2)), "one ring of tiles around the source");
    assert!(!out.has_tile((3, 3)) && !out.has_tile((0, 3)));
    let got = out.download_all(&ctx).unwrap();
    let want = gaussian::apply(&Image::from_rgba8_premul(w as usize, h as usize, &full), None, &p)
        .to_rgba8_premul();
    assert_close("sparse gaussian", &got, &want, 1, 0.0, 1);
    assert!(got.iter().any(|&b| b != 0));
}

#[test]
fn hsb_opacity_sharpen_match_cpu() {
    let Some(ctx) = gpu("adjust::point_ops") else { return };
    let mut adj = Adjuster::new(&ctx);
    let c = case(&ctx, 41, W, H);
    for use_mask in [false, true] {
        for p in [
            HsbParams { hue: 0.0, saturation: 0.0, brightness: 0.0 },
            HsbParams { hue: 0.37, saturation: 0.4, brightness: -0.2 },
            HsbParams { hue: -0.9, saturation: -0.7, brightness: 0.35 },
            HsbParams { hue: 1.6, saturation: 1.0, brightness: 1.0 },
        ] {
            let got = run(&ctx, &mut adj, &c, use_mask, &Adjustment::Hsb(p));
            let want = cpu(&c, W, H, |i, m| hsb::apply(i, m, &p), use_mask);
            assert_close(&format!("hsb {p:?}"), &got, &want, 1, 0.0, 1);
        }
        for amount in [0.0, 0.33, 1.0] {
            let p = OpacityParams { amount };
            let got = run(&ctx, &mut adj, &c, use_mask, &Adjustment::Opacity(p));
            let want = cpu(&c, W, H, |i, m| opacity::apply(i, m, &p), use_mask);
            assert_close(&format!("opacity {amount}"), &got, &want, 1, 0.0, 1);
        }
        for amount in [0.0, 0.25, 0.8, 1.0] {
            let p = SharpenParams { amount };
            let got = run(&ctx, &mut adj, &c, use_mask, &Adjustment::Sharpen(p));
            let want = cpu(&c, W, H, |i, m| sharpen::apply(i, m, &p), use_mask);
            assert_close(&format!("sharpen {amount}"), &got, &want, 1, 0.0, 1);
        }
    }
}

fn random_curve(rng: &mut Rng) -> Curve {
    // A monotone-ish curve with a few kinks.
    let knots: Vec<(f32, f32)> = (0..5)
        .map(|k| (k as f32 / 4.0, (k as f32 / 4.0 + (rng.f32() - 0.5) * 0.4).clamp(0.0, 1.0)))
        .collect();
    let mut table = [0.0f32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let x = i as f32 / 255.0;
        let seg = ((x * 4.0).floor() as usize).min(3);
        let (x0, y0) = knots[seg];
        let (x1, y1) = knots[seg + 1];
        *t = y0 + (y1 - y0) * ((x - x0) / (x1 - x0));
    }
    Curve::from_table(table)
}

fn random_set(rng: &mut Rng) -> CurveSet {
    CurveSet {
        master: random_curve(rng),
        red: random_curve(rng),
        green: random_curve(rng),
        blue: random_curve(rng),
    }
}

#[test]
fn curves_match_cpu() {
    let Some(ctx) = gpu("adjust::curves") else { return };
    let mut adj = Adjuster::new(&ctx);
    let c = case(&ctx, 42, W, H);
    let mut rng = Rng::new(420);
    let params = [
        CurvesParams::default(),
        CurvesParams {
            target: random_set(&mut rng),
            blend: 1.0,
            blend_source: None,
        },
        CurvesParams {
            target: random_set(&mut rng),
            blend: 0.35,
            blend_source: Some(random_set(&mut rng)),
        },
    ];
    for use_mask in [false, true] {
        for p in &params {
            let got = run(&ctx, &mut adj, &c, use_mask, &Adjustment::Curves(Box::new(p.clone())));
            let want = cpu(&c, W, H, |i, m| eskiz_filters::curves::apply(i, m, p), use_mask);
            assert_close("curves", &got, &want, 1, 0.0, 1);
        }
    }
}

#[test]
fn gradient_map_matches_cpu() {
    let Some(ctx) = gpu("adjust::gradient_map") else { return };
    let mut adj = Adjuster::new(&ctx);
    let c = case(&ctx, 43, W, H);
    let start = gradient_map::ramp([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
    let target = gradient_map::ramp([0.1, 0.0, 0.4], [1.0, 0.8, 0.2]);
    for use_mask in [false, true] {
        for (amount, blend) in [(1.0, 1.0), (0.7, 1.0), (1.0, 0.4), (0.5, 0.25)] {
            let p = GradientMapParams {
                start: start.clone(),
                target: target.clone(),
                amount,
                blend,
            };
            let got = run(&ctx, &mut adj, &c, use_mask, &Adjustment::GradientMap(p.clone()));
            let want = cpu(&c, W, H, |i, m| gradient_map::apply(i, m, &p), use_mask);
            assert_close(&format!("gradient map {amount}/{blend}"), &got, &want, 1, 0.0, 1);
        }
    }
    // Tables of other lengths work as well.
    let p = GradientMapParams {
        start: vec![[0.0; 3], [1.0; 3]],
        target: vec![[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        amount: 1.0,
        blend: 1.0,
    };
    let got = run(&ctx, &mut adj, &c, false, &Adjustment::GradientMap(p.clone()));
    let want = cpu(&c, W, H, |i, m| gradient_map::apply(i, m, &p), false);
    assert_close("gradient map short tables", &got, &want, 1, 0.0, 1);
    let bad = GradientMapParams {
        start: vec![],
        ..p
    };
    assert!(adj.apply(&ctx, &c.layer, None, &Adjustment::GradientMap(bad)).is_err());
}

/// Noise. The simplex sampler floors a skewed coordinate, and f32 evaluation order differs
/// between Rust and the GPU shader compiler (fused multiply-add, fast-math). A sample that lies
/// within a few ulps of a cell boundary can land in the neighbouring cell, which changes the
/// value by a lot; none occurred in these seeds. The tolerances are documented at the call.
#[test]
fn noise_matches_cpu() {
    let Some(ctx) = gpu("adjust::noise") else { return };
    let mut adj = Adjuster::new(&ctx);
    let (w, h) = (300u32, 280u32);
    let c = case(&ctx, 44, w, h);
    let base = NoiseParams::default();
    let configs = [
        NoiseParams { scale: 0.3, amount: 0.3, ..base },
        NoiseParams { scale: 0.0, amount: 0.23, octaves: 0.0, ..base },
        NoiseParams { scale: 0.6, amount: 0.5, octaves: 1.0, kind: NoiseKind::Absolute, ..base },
        NoiseParams { scale: 0.4, amount: 0.4, octaves: 0.5, kind: NoiseKind::Billowy, multichannel: true, ..base },
        NoiseParams { scale: 0.5, amount: 0.6, combine: CombineMode::Mix, seed: 7, ..base },
        NoiseParams { scale: 0.5, amount: 0.15, turbulence: 0.0, multichannel: true, seed: 3, ..base },
    ];
    for (k, p) in configs.iter().enumerate() {
        for use_mask in [false, true] {
            let got = run(&ctx, &mut adj, &c, use_mask, &Adjustment::Noise(*p));
            let img = Image::from_rgba8_premul(w as usize, h as usize, &c.px);
            let m = mask_f32(&c);
            let want = noise::apply(&img, use_mask.then_some(m.as_slice()), p).to_rgba8_premul();
            // Multichannel noise samples at origins up to ~4100, where an f32 ulp is 5e-4, so
            // fused multiply-adds in the GPU compiler move the sample by an ulp or two and the
            // value by up to ~12/255. Single-field noise stays within 2/255 everywhere.
            let (frac, hard) = if p.multichannel { (0.02, 32) } else { (0.0, 2) };
            let worst = assert_close(&format!("noise config {k} mask={use_mask}"), &got, &want, if p.multichannel { 1 } else { 2 }, frac, hard);
            eprintln!("  noise config {k} mask {use_mask:<5} worst {worst}");
        }
    }
}

#[test]
fn transparent_layers_stay_sparse() {
    let Some(ctx) = gpu("adjust::sparse") else { return };
    let mut adj = Adjuster::new(&ctx);
    let layer = GpuLayer::new(512, 512);
    for a in [
        Adjustment::Hsb(HsbParams { hue: 0.5, saturation: 0.2, brightness: 0.1 }),
        Adjustment::Gaussian(GaussianParams { amount: 0.5 }),
        Adjustment::Noise(NoiseParams::default()),
    ] {
        let out = adj.apply(&ctx, &layer, None, &a).unwrap();
        assert_eq!(out.tile_count(), 0);
    }
    // Mismatched mask size is an error, not a crash.
    let m = GpuMask::new(100, 100, 1.0);
    assert!(adj
        .apply(&ctx, &layer, Some(&m), &Adjustment::Opacity(OpacityParams { amount: 0.5 }))
        .is_err());
}
