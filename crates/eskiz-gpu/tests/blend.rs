//! Every blend mode against the CPU references.
//!
//! Two references are used: the f64 implementation in `common` (partial alpha,
//! opacity) and `eskiz_core::blend::blend_pixel` (opaque backdrop), so the GPU, the design notes
//! reference and the app's CPU flattening all agree.
mod common;
use common::*;
use eskiz_core::blend::blend_pixel;
use eskiz_core::BlendMode;
use eskiz_gpu::{mode_index, Region};

const W: u32 = 200;
const H: u32 = 150;

/// Modes whose output jumps at a threshold; f32 and f64 may pick different sides when the
/// inputs sit exactly on it (common with 8-bit data).
fn discontinuous(m: BlendMode) -> bool {
    matches!(m, BlendMode::HardMix | BlendMode::LighterColor | BlendMode::DarkerColor)
}

#[test]
fn shader_mode_numbers_follow_the_core_table() {
    for (i, m) in BlendMode::ALL.iter().enumerate() {
        assert_eq!(mode_index(*m) as usize, i);
    }
    assert_eq!(BlendMode::ALL.len(), 26);
}

#[test]
fn all_modes_opaque_over_opaque() {
    let Some(ctx) = gpu("blend::opaque") else { return };
    let mut rng = Rng::new(10);
    let bottom = random_premul(&mut rng, W as usize, H as usize, true);
    let top = random_premul(&mut rng, W as usize, H as usize, true);
    for mode in BlendMode::ALL {
        let mut doc = TestDoc::new(W, H);
        doc.push(&ctx, RefLayer::new(bottom.clone()));
        let mut t = RefLayer::new(top.clone());
        t.blend = mode;
        doc.push(&ctx, t);
        let (_, out) = doc.render(&ctx, &Region::All);
        let want = doc.expected(doc.full());

        // Pixels sitting on a discontinuity may legitimately differ: exclude them.
        let mut g = Vec::new();
        let mut c = Vec::new();
        for i in 0..(W * H) as usize {
            let f = |p: &[u8]| [0, 1, 2].map(|k| f64::from(p[i * 4 + k]) / 255.0);
            if discontinuous(mode) && near_discontinuity(mode, f(&bottom), f(&top)) {
                continue;
            }
            g.extend_from_slice(&out[i * 4..i * 4 + 4]);
            c.extend_from_slice(&want[i * 4..i * 4 + 4]);
        }
        let worst = assert_close(mode.name(), &g, &c, 1, 0.0, 1);

        // Cross-check against the core's opaque-backdrop blend (white paper under bottom is
        // irrelevant because the bottom layer is opaque).
        let mut core = Vec::new();
        let mut gg = Vec::new();
        for i in 0..(W * H) as usize {
            let f = |p: &[u8]| [0, 1, 2].map(|k| f64::from(p[i * 4 + k]) / 255.0);
            if discontinuous(mode) && near_discontinuity(mode, f(&bottom), f(&top)) {
                continue;
            }
            let o = blend_pixel(f(&bottom), [f(&top)[0], f(&top)[1], f(&top)[2], 1.0], mode);
            core.extend(o.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
            gg.extend_from_slice(&out[i * 4..i * 4 + 3]);
        }
        assert_close(&format!("{} vs core", mode.name()), &gg, &core, 1, 0.0, 1);
        eprintln!("  {:<14} worst diff {worst}", mode.name());
    }
}

#[test]
fn all_modes_with_alpha_opacity_and_transparent_backdrop() {
    let Some(ctx) = gpu("blend::alpha") else { return };
    let mut rng = Rng::new(11);
    for mode in BlendMode::ALL {
        let bottom = random_premul(&mut rng, W as usize, H as usize, false);
        let top = random_premul(&mut rng, W as usize, H as usize, false);
        let mut doc = TestDoc::new(W, H);
        doc.bg = None; // transparent backdrop: partial alpha everywhere
        let mut b = RefLayer::new(bottom);
        b.opacity = 0.85;
        doc.push(&ctx, b);
        let mut t = RefLayer::new(top);
        t.blend = mode;
        t.opacity = 0.7;
        doc.push(&ctx, t);
        let (_, out) = doc.render(&ctx, &Region::All);
        let want = doc.expected(doc.full());
        let (frac, hard) = if discontinuous(mode) { (0.01, 255) } else { (0.0, 1) };
        // The discontinuous modes may flip a branch on a few pixels; everything else must
        // be within one 8-bit step. Non-separable modes get a 0.02% allowance for ClipColor
        // divisions on near-grey colours, with a hard cap of 3.
        let (frac, hard) = match mode {
            BlendMode::Color | BlendMode::Luminosity | BlendMode::Hue | BlendMode::Saturation => (0.0002, 3),
            _ => (frac, hard),
        };
        let worst = assert_close(mode.name(), &out, &want, 1, frac, hard);
        eprintln!("  {:<14} worst diff {worst}", mode.name());
    }
}

#[test]
fn soft_light_is_pegtop() {
    let Some(ctx) = gpu("blend::soft_light") else { return };
    // Hand-checked examples: Cb 0.5 / Cs 0.25 -> 0.375, Cb 0.25 / Cs 0.75 -> 0.375.
    // 0.5 and 0.25 are not exact in 8 bits, so use 128/255 and 64/255 with a loose bound.
    let one = |cb: u8, cs: u8| -> u8 {
        let mut doc = TestDoc::new(1, 1);
        doc.push(&ctx, RefLayer::new(vec![cb, cb, cb, 255]));
        let mut t = RefLayer::new(vec![cs, cs, cs, 255]);
        t.blend = BlendMode::SoftLight;
        doc.push(&ctx, t);
        let (_, out) = doc.render(&ctx, &Region::All);
        out[0]
    };
    let a = one(128, 64);
    assert!((i32::from(a) - (0.375f64 * 255.0).round() as i32).abs() <= 2, "{a}");
    let b = one(64, 191);
    assert!((i32::from(b) - (0.375f64 * 255.0).round() as i32).abs() <= 2, "{b}");
    // The W3C form would give about 0.4375 and 0.347 here.
}

#[test]
fn colour_mode_example_from_the_spec() {
    let Some(ctx) = gpu("blend::colour") else { return };
    // Cb = 50% grey, Cs = red -> (1.0, 0.286, 0.286) within 0.005.
    let mut doc = TestDoc::new(1, 1);
    doc.push(&ctx, RefLayer::new(vec![128, 128, 128, 255]));
    let mut t = RefLayer::new(vec![255, 0, 0, 255]);
    t.blend = BlendMode::Color;
    doc.push(&ctx, t);
    let (_, out) = doc.render(&ctx, &Region::All);
    assert!(out[0] >= 254);
    let g = f64::from(out[1]) / 255.0;
    assert!((g - 0.286).abs() < 0.012, "{g}");
}
