//! Acceptance tests for the adjustments.
//! Each test is named after its spec ID. Tests for unverified items check behaviour class only.

use eskiz_filters::balance::{self, BalanceParams};
use eskiz_filters::bloom::{self, BloomParams};
use eskiz_filters::chromatic::{self, AxialParams, TransverseParams};
use eskiz_filters::clone::{self as clone_fx, CloneParams};
use eskiz_filters::colour::{luma, rgb_from_yuv, yuv};
use eskiz_filters::curves::{self, Curve, CurveSet, CurvesParams};
use eskiz_filters::gaussian::{self, GaussianParams};
use eskiz_filters::glitch::{self, GlitchParams};
use eskiz_filters::gradient_map::{self, GradientMapParams};
use eskiz_filters::halftone::{self, HalftoneParams};
use eskiz_filters::hsb::{self, HsbParams};
use eskiz_filters::motion::{self, MotionParams};
use eskiz_filters::noise::{self, CombineMode, NoiseParams};
use eskiz_filters::opacity::{self, OpacityParams};
use eskiz_filters::perspective::{self, PerspectiveParams};
use eskiz_filters::pyramid::{self, PyramidParams};
use eskiz_filters::recolor::{self, RecolorParams};
use eskiz_filters::sharpen::{self, SharpenParams};
use eskiz_filters::{blur_for_mode, drag_to_value, Image, InteractionMode};

/// One 8-bit step, the tolerance the design notes use for round trips.
const ONE_255: f32 = 1.0 / 255.0;

fn assert_px_close(a: [f32; 4], b: [f32; 4], eps: f32, what: &str) {
    for k in 0..4 {
        assert!(
            (a[k] - b[k]).abs() <= eps,
            "{what}: channel {k} differs: {a:?} vs {b:?} (eps {eps})"
        );
    }
}

fn assert_images_close(a: &Image, b: &Image, eps: f32, what: &str) {
    assert_eq!((a.width, a.height), (b.width, b.height), "{what}: size");
    for y in 0..a.height {
        for x in 0..a.width {
            assert_px_close(a.px(x, y), b.px(x, y), eps, &format!("{what} at ({x},{y})"));
        }
    }
}

/// A deterministic, varied opaque image.
fn varied(w: usize, h: usize) -> Image {
    Image::generate(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        [
            0.2 + 0.6 * fx,
            0.2 + 0.6 * fy,
            0.5 + 0.4 * ((x * 7 + y * 3) % 5) as f32 / 4.0 - 0.2,
            1.0,
        ]
    })
}

// ---------------------------------------------------------------------------
// Gaussian blur (section 2)
// ---------------------------------------------------------------------------

#[test]
fn g1_tap_count_is_49_and_sigma_0958_at_amount_0_6() {
    assert_eq!(gaussian::tap_count(0.6), 49);
    assert!((gaussian::sigma(0.6) - 0.9579).abs() < 1e-3);
}

#[test]
fn g2_amount_zero_is_identity() {
    let src = varied(9, 7);
    let out = gaussian::apply(&src, None, &GaussianParams { amount: 0.0 });
    assert_images_close(&out, &src, 1e-6, "G2");
}

#[test]
fn g3_amount_one_has_64_taps() {
    assert_eq!(gaussian::tap_count(1.0), 64);
}

#[test]
fn g4_amount_quarter_has_32_taps_and_sigma_0875() {
    assert_eq!(gaussian::tap_count(0.25), 32);
    assert!((gaussian::sigma(0.25) - 0.875).abs() < 1e-6);
}

#[test]
fn g5_flat_region_is_unchanged_and_weights_sum_to_one() {
    for &a in &[0.1f32, 0.6, 1.0] {
        let w = gaussian::weights(a);
        let total = w[0] + 2.0 * w[1..].iter().sum::<f32>();
        assert!(
            (total - 1.0).abs() < 1e-5,
            "weights sum {total} at amount {a}"
        );
        let flat = Image::filled(12, 10, [0.3, 0.5, 0.7, 0.8]);
        let out = gaussian::apply(&flat, None, &GaussianParams { amount: a });
        assert_images_close(&out, &flat, 1e-5, "G5");
    }
}

#[test]
fn g6_impulse_spreads_along_both_axes() {
    let mut img = Image::new(15, 15);
    let c = (7 * 15 + 7) * 4;
    img.data[c..c + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
    let out = gaussian::blur(&img, 0.6);
    let at = |x: usize, y: usize| out.px(x, y)[0];
    assert!(at(8, 7) > 0.0 && at(6, 7) > 0.0, "horizontal spread");
    assert!(at(7, 8) > 0.0 && at(7, 6) > 0.0, "vertical spread");
    assert!(
        at(8, 8) > 0.0,
        "diagonal spread implies two separable passes"
    );
    assert!(at(8, 7) > at(9, 7), "weight decays with distance");
}

#[test]
fn g7_undo_entry_is_named_gaussian_blur() {
    // Undo itself belongs to the app layer. This crate only owns the entry name.
    assert_eq!(gaussian::UNDO_NAME, "Gaussian Blur");
}

#[test]
fn g_preview_scale_matches_spec_section_1_5() {
    assert_eq!(gaussian::preview_scale(0.02), 1);
    assert_eq!(gaussian::preview_scale(0.6), 19);
    assert_eq!(gaussian::preview_scale(1.0), 32);
}

#[test]
fn a3_preview_equals_commit_when_preview_scale_is_one() {
    let src = varied(10, 6);
    let p = GaussianParams { amount: 0.02 };
    assert_images_close(
        &gaussian::preview(&src, None, &p),
        &gaussian::apply(&src, None, &p),
        1e-6,
        "A3",
    );
}

// ---------------------------------------------------------------------------
// Pyramid blur (section 3)
// ---------------------------------------------------------------------------

#[test]
fn p1_zero_mask_returns_source() {
    let src = varied(16, 12);
    let mask = vec![0.0f32; 16 * 12];
    let out = pyramid::apply(&src, Some(&mask), &PyramidParams { amount: 0.6 });
    assert_images_close(&out, &src, 1e-6, "P1");
}

#[test]
fn p2_full_amount_and_mask_give_lod_3_and_mix_1_with_three_extra_levels() {
    let (lod, mix) = pyramid::select_level(1.0, 1.0, 3);
    assert!((lod - 3.0).abs() < 1e-6, "lod {lod}");
    assert!((mix - 1.0).abs() < 1e-6, "mix {mix}");
    assert_eq!(pyramid::level_count(8, 8), 3);
}

#[test]
fn p3_pencil_mode_blurs_only_the_painted_area() {
    let (w, h) = (16, 8);
    let src = Image::generate(w, h, |x, _| {
        let v = if x % 2 == 0 { 1.0 } else { 0.0 };
        [v, v, v, 1.0]
    });
    let mask: Vec<f32> = (0..w * h)
        .map(|i| if i % w < w / 2 { 1.0 } else { 0.0 })
        .collect();
    let out = blur_for_mode(InteractionMode::Pencil, &src, Some(&mask), 0.6);
    for y in 0..h {
        for x in w / 2..w {
            assert_px_close(out.px(x, y), src.px(x, y), 1e-6, "P3 unpainted");
        }
    }
    let changed =
        (0..h).any(|y| (0..w / 2).any(|x| (out.px(x, y)[0] - src.px(x, y)[0]).abs() > 0.01));
    assert!(changed, "P3: painted half should blur");
}

// ---------------------------------------------------------------------------
// Motion blur (section 4)
// ---------------------------------------------------------------------------

#[test]
fn m1_zero_vector_returns_source() {
    let src = varied(11, 9);
    let out = motion::apply(
        &src,
        None,
        &MotionParams {
            vector: [0.0, 0.0],
            amount: 0.6,
        },
    );
    assert_images_close(&out, &src, 1e-6, "M1");
}

#[test]
fn m2_horizontal_vector_blurs_only_horizontally() {
    let (w, h) = (20, 6);
    // Each row is constant, so horizontal blur must leave it unchanged.
    let src = Image::generate(w, h, |_, y| {
        let v = 0.1 + 0.15 * y as f32;
        [v, v, v, 1.0]
    });
    let out = motion::apply(
        &src,
        None,
        &MotionParams {
            vector: [40.0, 0.0],
            amount: 0.3,
        },
    );
    assert_images_close(&out, &src, 1e-4, "M2");
}

#[test]
fn m3_tap_positions_have_mean_half_a_step_before_the_pixel() {
    let n = 7;
    let offsets = motion::tap_offsets(n);
    let mean = offsets.iter().sum::<f32>() / n as f32;
    assert!((mean + 0.5).abs() < 1e-6, "mean {mean}");
    let st = motion::step([30.0, 0.0], 0.6, 100, 100);
    assert!((st[0] - 6.0 * 0.6 * 30.0 / 100.0).abs() < 1e-6);
}

#[test]
fn motion_tap_count_matches_spec_formula() {
    assert_eq!(motion::tap_count(0.6, 1.0), 153);
    assert_eq!(motion::tap_count(0.0, 1.0), 1);
}

// ---------------------------------------------------------------------------
// Perspective blur (section 5)
// ---------------------------------------------------------------------------

#[test]
fn r1_zero_direction_blurs_radially_and_keeps_origin_pixel() {
    let (w, h) = (5, 5);
    let src = varied(w, h);
    let params = PerspectiveParams {
        origin: [0.5, 0.5],
        direction: [0.0, 0.0],
        amount: 0.6,
    };
    let out = perspective::apply(&src, None, &params);
    assert_px_close(out.px(2, 2), src.px(2, 2), 1e-6, "R1 origin pixel");
    let moved = (0..h).any(|y| {
        (0..w).any(|x| (x, y) != (2, 2) && (out.px(x, y)[0] - src.px(x, y)[0]).abs() > 1e-4)
    });
    assert!(moved, "R1: non-origin pixels should blur radially");
}

#[test]
fn r2_strength_is_largest_where_w_is_zero_and_smallest_where_w_is_one() {
    let size = perspective::size_length(100, 100);
    let dir = [1.0, 0.0];
    // Radius towards -x: alpha = 0, so w = 0 and k = 1.
    let along = perspective::step_for([0.3, 0.5], [0.5, 0.5], dir, size);
    // Radius perpendicular to the direction: w = 1, and with |d| = 1, k = 0.
    let across = perspective::step_for([0.5, 0.7], [0.5, 0.5], dir, size);
    let mag = |s: [f32; 2]| s[0].hypot(s[1]);
    assert!(mag(along) > mag(across));
    assert!(mag(across) < 1e-6, "k = 0 gives a zero step");
    // Spec step: r 2 k / S.
    assert!((along[0] - (-0.2) * 2.0 / size).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// Sharpen (section 6)
// ---------------------------------------------------------------------------

#[test]
fn s1_amount_zero_returns_source() {
    let src = varied(8, 8);
    let out = sharpen::apply(&src, None, &SharpenParams { amount: 0.0 });
    assert_images_close(&out, &src, 1e-6, "S1");
}

#[test]
fn s2_flat_region_is_unchanged_at_any_amount() {
    let flat = Image::filled(6, 6, [0.25, 0.4, 0.6, 1.0]);
    for &k in &[0.3f32, 0.5, 1.0] {
        let out = sharpen::apply(&flat, None, &SharpenParams { amount: k });
        assert_images_close(&out, &flat, 1e-5, "S2");
    }
}

#[test]
fn s3_bright_centre_gets_brighter_and_neighbours_darker() {
    let mut img = Image::filled(5, 5, [0.5, 0.5, 0.5, 1.0]);
    let c = (2 * 5 + 2) * 4;
    img.data[c..c + 4].copy_from_slice(&[0.8, 0.8, 0.8, 1.0]);
    let out = sharpen::apply(&img, None, &SharpenParams { amount: 0.5 });
    assert!(out.px(2, 2)[0] > 0.8, "center {}", out.px(2, 2)[0]);
    assert!(out.px(2, 1)[0] < 0.5, "neighbor {}", out.px(2, 1)[0]);
}

// ---------------------------------------------------------------------------
// Noise (section 9)
// ---------------------------------------------------------------------------

#[test]
fn n1_amount_zero_leaves_image_unchanged() {
    let src = varied(12, 9);
    let params = NoiseParams {
        amount: 0.0,
        combine: CombineMode::Additive,
        ..NoiseParams::default()
    };
    assert_images_close(&noise::apply(&src, None, &params), &src, 1e-6, "N1");
}

#[test]
fn n2_output_is_deterministic_for_a_fixed_seed() {
    let src = varied(16, 16);
    let p = NoiseParams {
        amount: 0.5,
        ..NoiseParams::default()
    };
    let a = noise::apply(&src, None, &p);
    let b = noise::apply(&src, None, &p);
    assert_eq!(a, b, "N2: same seed must give identical output");
    let other = noise::apply(&src, None, &NoiseParams { seed: 9, ..p });
    assert_ne!(a, other, "N2: a different seed should change the output");
}

#[test]
fn n3_multichannel_produces_different_channels() {
    let flat = Image::filled(16, 16, [0.5, 0.5, 0.5, 1.0]);
    let p = NoiseParams {
        amount: 0.5,
        multichannel: true,
        ..NoiseParams::default()
    };
    let out = noise::apply(&flat, None, &p);
    let differs = out
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .any(|px| (px[0] - px[1]).abs() > 0.05 || (px[1] - px[2]).abs() > 0.05);
    assert!(differs, "N3: channels should differ visibly");
}

#[test]
fn noise_octave_count_and_ranges_match_spec() {
    assert_eq!(noise::octave_count(0.0), 1);
    assert_eq!(noise::octave_count(1.0), 5);
    let (lo, hi) = noise::normalisation_range(noise::NoiseKind::Signed, 1);
    assert!((lo + 1.00455).abs() < 1e-6 && (hi - 1.004689).abs() < 1e-6);
    let (lo_abs, _) = noise::normalisation_range(noise::NoiseKind::Absolute, 10);
    assert_eq!(lo_abs, 0.0);
}

// ---------------------------------------------------------------------------
// Chromatic aberration (section 10)
// ---------------------------------------------------------------------------

#[test]
fn c1_axial_zero_amount_returns_input_within_one_255() {
    let src = varied(14, 10);
    let params = AxialParams {
        amount: [0.0, 0.0],
        ..AxialParams::default()
    };
    assert_images_close(
        &chromatic::apply_axial(&src, None, &params),
        &src,
        ONE_255,
        "C1 axial",
    );
}

#[test]
fn c1_transverse_zero_offset_returns_input_within_one_255() {
    let src = varied(14, 10);
    let params = TransverseParams {
        offset: [0.0, 0.0],
        ..TransverseParams::default()
    };
    assert_images_close(
        &chromatic::apply_transverse(&src, None, &params),
        &src,
        ONE_255,
        "C1 transverse",
    );
}

#[test]
fn c2_transverse_red_and_blue_move_in_opposite_directions() {
    let (w, h) = (31, 31);
    let mut img = Image::filled(w, h, [0.0, 0.0, 0.0, 1.0]);
    let c = (15 * w + 15) * 4;
    img.data[c..c + 4].copy_from_slice(&[1.0, 0.0, 1.0, 1.0]);
    let params = TransverseParams {
        offset: [0.1, 0.0],
        ..TransverseParams::default()
    };
    let out = chromatic::apply_transverse(&img, None, &params);
    let centroid = |ch: usize| {
        let (mut sx, mut sw) = (0.0f32, 0.0f32);
        for y in 0..h {
            for x in 0..w {
                let v = out.px(x, y)[ch];
                sx += x as f32 * v;
                sw += v;
            }
        }
        sx / sw
    };
    let red = centroid(0);
    let blue = centroid(2);
    assert!(red > 15.0, "red centroid {red} should move right");
    assert!(blue < 15.0, "blue centroid {blue} should move left");
}

#[test]
fn c2_axial_offsets_are_opposite_on_opposite_sides_of_origin() {
    let params = AxialParams::default();
    let a = chromatic::axial_offset(&params, [0.7, 0.5], 100, 100);
    let b = chromatic::axial_offset(&params, [0.3, 0.5], 100, 100);
    assert!((a[0] + b[0]).abs() < 1e-6 && a[0].abs() > 1e-4);
}

#[test]
fn c3_alpha_is_average_of_three_alphas() {
    let src = Image::filled(8, 8, [0.4, 0.4, 0.4, 0.8]);
    let params = TransverseParams {
        offset: [0.0, 0.0],
        ..TransverseParams::default()
    };
    let out = chromatic::apply_transverse(&src, None, &params);
    // 0.333252 * 3 * 0.8 = 0.79980, which is the average to within the design notes constant.
    assert!(
        (out.px(3, 3)[3] - 0.8).abs() < 1e-3,
        "alpha {}",
        out.px(3, 3)[3]
    );
}

// ---------------------------------------------------------------------------
// Glitch (section 18)
// ---------------------------------------------------------------------------

#[test]
fn gl1_amount_zero_leaves_image_unchanged() {
    // At amount 0 the snap scales out too, so the default cell count gives the identity.
    let src = varied(13, 11);
    let params = GlitchParams {
        amount: 0.0,
        ..GlitchParams::default()
    };
    assert_images_close(&glitch::apply(&src, None, &params), &src, 1e-6, "GL1");
}

#[test]
fn gl2_output_alpha_lies_within_source_alpha_range() {
    let src = Image::generate(16, 16, |x, _| {
        [0.5, 0.5, 0.5, if x % 3 == 0 { 0.4 } else { 1.0 }]
    });
    let params = GlitchParams {
        amount: 0.8,
        seed: 3.0,
        ..GlitchParams::default()
    };
    let out = glitch::apply(&src, None, &params);
    for px in out.data.as_chunks::<4>().0.iter() {
        assert!(
            px[3] >= 0.4 - 1e-6 && px[3] <= 1.0 + 1e-6,
            "alpha {}",
            px[3]
        );
    }
}

// ---------------------------------------------------------------------------
// Halftone (section 11)
// ---------------------------------------------------------------------------

#[test]
fn h1_flat_mid_grey_gives_a_regular_dot_grid() {
    let src = Image::filled(64, 64, [0.5, 0.5, 0.5, 1.0]);
    let out = halftone::apply(&src, None, &HalftoneParams::default());
    let alphas: Vec<f32> = out.data.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    assert!(alphas.iter().any(|&a| a > 0.9), "dot cores expected");
    assert!(alphas.iter().any(|&a| a < 0.1), "paper gaps expected");
    let dotted = alphas.iter().filter(|&&a| a > 0.5).count() as f32 / alphas.len() as f32;
    assert!((0.3..0.7).contains(&dotted), "dot coverage {dotted}");
}

#[test]
fn h2_pitch_and_softness_change_dot_spacing_and_edges() {
    assert!(halftone::pitch(0.5) > halftone::pitch(0.12));
    assert!((halftone::pitch(0.12) - 13.52).abs() < 1e-3);
    let src = Image::filled(48, 48, [0.5, 0.5, 0.5, 1.0]);
    let sharp = halftone::apply(
        &src,
        None,
        &HalftoneParams {
            softness: 0.0,
            ..HalftoneParams::default()
        },
    );
    let soft = halftone::apply(
        &src,
        None,
        &HalftoneParams {
            softness: 2.0,
            ..HalftoneParams::default()
        },
    );
    let edges = |img: &Image| {
        img.data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 0.05 && p[3] < 0.95)
            .count()
    };
    assert_ne!(
        edges(&sharp),
        edges(&soft),
        "softness should change edge pixels"
    );
}

// ---------------------------------------------------------------------------
// Bloom (section 12)
// ---------------------------------------------------------------------------

#[test]
fn b1_thresholds_above_layer_max_luma_add_nothing() {
    let src = Image::generate(12, 12, |x, y| {
        let v = 0.1 + 0.3 * ((x + y) % 4) as f32 / 3.0;
        [v, v * 0.8, v * 0.6, 1.0]
    });
    let params = BloomParams {
        upper: 0.9,
        lower: 0.8,
        ..BloomParams::default()
    };
    assert_images_close(&bloom::apply(&src, None, &params), &src, 1e-6, "B1");
    // Semi-transparent pixels (premultiplied) come back unchanged too, not darkened.
    let soft = Image::generate(12, 12, |x, y| {
        let a = 0.25 + 0.5 * ((x + y) % 3) as f32 / 2.0;
        [0.4 * a, 0.3 * a, 0.2 * a, a]
    });
    assert_images_close(&bloom::apply(&soft, None, &params), &soft, 1e-6, "B1 soft");
}

#[test]
fn b2_bright_dot_halo_radius_grows_with_spread() {
    // Second moment of the glow (its mean squared radius, excluding the saturated core).
    // A wider spread gives a larger halo radius.
    let (w, h) = (41, 41);
    let bg = 0.05;
    let mut img = Image::filled(w, h, [bg, bg, bg, 1.0]);
    let c = (20 * w + 20) * 4;
    img.data[c..c + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
    let halo_radius_sq = |spread: f32| {
        let p = BloomParams {
            spread,
            amount: 1.0,
            ..BloomParams::default()
        };
        let out = bloom::apply(&img, None, &p);
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for y in 0..h {
            for x in 0..w {
                let v = out.px(x, y)[0];
                if v >= 0.999 || v <= bg {
                    continue;
                }
                let d = f64::from(v - bg);
                let (dx, dy) = (x as f64 - 20.0, y as f64 - 20.0);
                num += d * (dx * dx + dy * dy);
                den += d;
            }
        }
        num / den
    };
    let small = halo_radius_sq(0.1);
    let large = halo_radius_sq(0.5);
    assert!(large > small, "halo radius^2 {large} should exceed {small}");
}

// ---------------------------------------------------------------------------
// Hue, saturation, brightness (section 8)
// ---------------------------------------------------------------------------

const SAMPLE_COLOURS: [[f32; 3]; 4] = [
    [0.8, 0.2, 0.1],
    [0.1, 0.6, 0.9],
    [0.3, 0.3, 0.3],
    [0.95, 0.9, 0.05],
];

#[test]
fn hsb1_all_zero_returns_input() {
    let zero = HsbParams::default();
    for c in SAMPLE_COLOURS {
        let out = hsb::transform_rgb(c, &zero);
        for k in 0..3 {
            assert!((out[k] - c[k]).abs() <= ONE_255, "HSB1 {c:?} -> {out:?}");
        }
    }
}

#[test]
fn hsb2_hue_half_turn_rotates_red_towards_green() {
    let out = hsb::transform_rgb(
        [1.0, 0.0, 0.0],
        &HsbParams {
            hue: 0.5,
            ..HsbParams::default()
        },
    );
    assert!((out[0] - 0.06).abs() < 0.01, "r {}", out[0]);
    assert!((out[1] - 0.59).abs() < 0.01, "g {}", out[1]);
    assert_eq!(out[2], 0.0, "blue clamps to zero");
}

#[test]
fn hsb3_saturation_minus_one_is_greyscale_at_the_input_luma() {
    let params = HsbParams {
        saturation: -1.0,
        ..HsbParams::default()
    };
    for c in SAMPLE_COLOURS {
        let out = hsb::transform_rgb(c, &params);
        let y = luma(c);
        for v in out {
            assert!((v - y).abs() < 1e-5, "HSB3 {out:?} vs luma {y}");
        }
    }
}

#[test]
fn hsb4_brightness_plus_one_pushes_to_white() {
    let params = HsbParams {
        brightness: 1.0,
        ..HsbParams::default()
    };
    for c in [[0.0, 0.0, 0.0], [0.2, 0.2, 0.2], [0.5, 0.5, 0.5]] {
        let out = hsb::transform_rgb(c, &params);
        for v in out {
            assert!(v > 1.0 - 1e-3, "HSB4 {c:?} -> {out:?}");
        }
    }
}

#[test]
fn hsb5_round_trip_rgb_yuv_rgb_within_one_255() {
    for c in SAMPLE_COLOURS {
        let back = rgb_from_yuv(yuv(c));
        for k in 0..3 {
            assert!((back[k] - c[k]).abs() <= ONE_255, "HSB5 {c:?} -> {back:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// Colour balance (section 13)
// ---------------------------------------------------------------------------

#[test]
fn cb1_all_nine_values_zero_returns_input_within_one_255() {
    let zero = BalanceParams::default();
    for c in SAMPLE_COLOURS {
        let out = balance::balance_rgb(c, &zero);
        for k in 0..3 {
            assert!((out[k] - c[k]).abs() <= ONE_255, "CB1 {c:?} -> {out:?}");
        }
    }
}

#[test]
fn cb2_output_luma_equals_input_luma() {
    let params = BalanceParams {
        shadows: [0.3, -0.2, 0.1],
        midtones: [0.1, 0.1, -0.1],
        highlights: [-0.1, 0.2, 0.0],
    };
    for c in [[0.2, 0.3, 0.4], [0.5, 0.45, 0.6], [0.7, 0.8, 0.75]] {
        let out = balance::balance_rgb(c, &params);
        assert!((luma(out) - luma(c)).abs() < 0.01, "CB2 {c:?} -> {out:?}");
    }
}

#[test]
fn cb3_shadows_only_leaves_midtones_unchanged() {
    let params = BalanceParams {
        shadows: [1.0, 1.0, 1.0],
        ..BalanceParams::default()
    };
    let mid = [0.5, 0.5, 0.5];
    let out = balance::balance_rgb(mid, &params);
    for k in 0..3 {
        assert!((out[k] - mid[k]).abs() <= ONE_255, "CB3 {out:?}");
    }
}

// ---------------------------------------------------------------------------
// Curves (section 14)
// ---------------------------------------------------------------------------

#[test]
fn cu1_identity_curves_return_input_within_one_255() {
    let params = CurvesParams::default();
    for c in SAMPLE_COLOURS {
        let out = curves::curves_rgb(c, &params);
        for k in 0..3 {
            assert!((out[k] - c[k]).abs() <= ONE_255, "CU1 {c:?} -> {out:?}");
        }
    }
}

#[test]
fn cu2_blend_zero_returns_blend_source_and_blend_one_returns_target() {
    let mut inverted = [0.0f32; 256];
    for (i, v) in inverted.iter_mut().enumerate() {
        *v = 1.0 - i as f32 / 256.0;
    }
    let mut target = CurveSet::identity();
    target.red = Curve::from_table(inverted);
    let c = [0.3, 0.6, 0.9];
    let at_zero = curves::curves_rgb(
        c,
        &CurvesParams {
            target: target.clone(),
            blend: 0.0,
            blend_source: Some(CurveSet::identity()),
        },
    );
    assert!(
        (at_zero[0] - 0.3).abs() <= ONE_255,
        "blend 0 should give the blend-source curve"
    );
    let at_one = curves::curves_rgb(
        c,
        &CurvesParams {
            target,
            blend: 1.0,
            blend_source: Some(CurveSet::identity()),
        },
    );
    assert!(
        (at_one[0] - 0.7).abs() <= 2.0 * ONE_255,
        "blend 1 should give the target curve: {}",
        at_one[0]
    );
}

// ---------------------------------------------------------------------------
// Gradient map (section 7)
// ---------------------------------------------------------------------------

#[test]
fn gm1_preview_blend_eases_at_quarter_and_three_quarter_points() {
    let d = gradient_map::BLEND_SECONDS;
    assert!((gradient_map::blend_at(0.25 * d, d) - 0.125).abs() < 1e-6);
    assert!((gradient_map::blend_at(0.75 * d, d) - 0.875).abs() < 1e-6);
    assert_eq!(
        gradient_map::blend_at(2.0 * d, d),
        1.0,
        "after the window blend is 1"
    );
}

#[test]
fn gm2_commit_blend_one_gives_target_table_exactly() {
    let start = gradient_map::ramp([0.0; 3], [1.0; 3]);
    let target = gradient_map::ramp([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let params = GradientMapParams::commit(start, target.clone());
    let src = Image::filled(4, 4, [0.5, 0.5, 0.5, 1.0]);
    let out = gradient_map::apply(&src, None, &params);
    let expected = gradient_map::lookup(&target, luma([0.5, 0.5, 0.5]));
    for k in 0..3 {
        assert!(
            (out.px(1, 1)[k] - expected[k]).abs() < 1e-5,
            "GM2 {:?} vs {expected:?}",
            out.px(1, 1)
        );
    }
}

#[test]
fn gm3_mix_factor_is_mask_times_amount() {
    let start = gradient_map::ramp([0.0; 3], [1.0; 3]);
    let target = gradient_map::ramp([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    let params = GradientMapParams {
        start,
        target: target.clone(),
        amount: 0.5,
        blend: 1.0,
    };
    let src = Image::filled(3, 3, [0.5, 0.5, 0.5, 1.0]);
    let mask = vec![0.5f32; 9];
    let out = gradient_map::apply(&src, Some(&mask), &params);
    let adjusted = gradient_map::lookup(&target, luma([0.5, 0.5, 0.5]));
    let t = 0.25;
    for (k, &adj) in adjusted.iter().enumerate() {
        let expected = 0.5 * (1.0 - t) + adj * t;
        assert!((out.px(1, 1)[k] - expected).abs() < 1e-5, "GM3 channel {k}");
    }
}

// ---------------------------------------------------------------------------
// Recolor (section 15) and clone (section 16)
// ---------------------------------------------------------------------------

#[test]
fn rc1_pixels_far_from_source_colour_are_unchanged() {
    let src = Image::filled(4, 4, [0.0, 0.0, 1.0, 1.0]);
    let out = recolor::apply(&src, None, &RecolorParams::default());
    assert_images_close(&out, &src, 1e-6, "RC1");
}

#[test]
fn rc2_source_pixels_take_target_hue_and_saturation_and_keep_luma() {
    let src = Image::filled(2, 2, [1.0, 0.0, 0.0, 1.0]);
    let out = recolor::apply(&src, None, &RecolorParams::default());
    let [h, s, l] = eskiz_filters::colour::rgb_to_hsl(out.px(0, 0)[..3].try_into().unwrap());
    let [ht, st, _] = eskiz_filters::colour::rgb_to_hsl([0.0, 0.0, 1.0]);
    assert!((h - ht).abs() < 1e-3, "hue {h} vs {ht}");
    assert!((s - st).abs() < 1e-3, "sat {s} vs {st}");
    // The HSL lightness of the result is the Rec.601 luma of the source.
    assert!((l - luma([1.0, 0.0, 0.0])).abs() < 1e-3, "lightness {l}");
}

#[test]
fn cl1_clone_copies_pixels_from_the_fixed_offset_source() {
    let (w, h) = (6, 4);
    let src = Image::generate(w, h, |x, y| [x as f32 / 10.0, y as f32 / 10.0, 0.5, 1.0]);
    let out = clone_fx::apply(
        &src,
        None,
        &CloneParams {
            offset: [2.0, 1.0],
            opacity: 1.0,
        },
    );
    for y in 0..h - 1 {
        for x in 0..w - 2 {
            assert_px_close(out.px(x, y), src.px(x + 2, y + 1), 1e-5, "CL1");
        }
    }
    assert_eq!(
        out.px(w - 1, 0)[3],
        0.0,
        "source outside the layer is transparent"
    );
}

// ---------------------------------------------------------------------------
// Opacity (section 17), framework (section 1), and interaction (sections 1.3, 1.4, 1.5)
// ---------------------------------------------------------------------------

#[test]
fn o1_amount_one_returns_input_and_amount_zero_is_transparent() {
    let src = varied(5, 5);
    assert_images_close(
        &opacity::apply(&src, None, &OpacityParams { amount: 1.0 }),
        &src,
        1e-6,
        "O1 one",
    );
    let zero = opacity::apply(&src, None, &OpacityParams { amount: 0.0 });
    assert!(zero.data.iter().all(|&v| v == 0.0), "O1 zero");
}

#[test]
fn i1_layer_mode_applies_to_the_whole_layer_limited_by_the_selection() {
    let src = varied(10, 4);
    let mut mask = vec![1.0f32; 40];
    for y in 0..4 {
        for x in 5..10 {
            mask[y * 10 + x] = 0.0;
        }
    }
    let out = blur_for_mode(InteractionMode::Layer, &src, Some(&mask), 0.6);
    for y in 0..4 {
        for x in 5..10 {
            assert_px_close(out.px(x, y), src.px(x, y), 1e-6, "I1 unselected");
        }
    }
    assert!((0..4).any(|y| (0..5).any(|x| (out.px(x, y)[0] - src.px(x, y)[0]).abs() > 1e-4)));
}

#[test]
fn a1_preview_does_not_change_the_source_pixels() {
    let src = varied(8, 8);
    let before = src.clone();
    let _ = gaussian::preview(&src, None, &GaussianParams { amount: 0.6 });
    assert_eq!(src, before, "A1");
}

#[test]
fn d1_dragging_right_increases_left_decreases_and_value_clamps() {
    assert!((drag_to_value(0.5, 100.0, 200.0, (0.0, 1.0)) - 1.0).abs() < 1e-6);
    assert!((drag_to_value(0.5, -50.0, 200.0, (0.0, 1.0)) - 0.25).abs() < 1e-6);
    assert_eq!(drag_to_value(0.9, 1000.0, 200.0, (0.0, 1.0)), 1.0);
}

#[test]
fn shared_mask_rule_is_out_equals_mix_of_source_and_adjusted() {
    let src = varied(4, 4);
    let adj = Image::filled(4, 4, [0.0, 0.0, 0.0, 0.0]);
    let mask = vec![0.25f32; 16];
    let out = eskiz_filters::mix_masked(&src, &adj, Some(&mask));
    assert_px_close(
        out.px(1, 1),
        [
            src.px(1, 1)[0] * 0.75,
            src.px(1, 1)[1] * 0.75,
            src.px(1, 1)[2] * 0.75,
            src.px(1, 1)[3] * 0.75,
        ],
        1e-6,
        "mask rule",
    );
}
