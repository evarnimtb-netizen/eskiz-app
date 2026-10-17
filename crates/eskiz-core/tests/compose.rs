//! Compositing tests: 11 cases with fixed tolerances.

use eskiz_core::compose::{
    contrast_ratio, display_pixel, over_paper, stroke_composite_pixel, PixelOpts,
};

const WHITE: [f64; 3] = [1.0, 1.0, 1.0];
const INK: [f64; 3] = [
    0x1d as f64 / 255.0,
    0x1d as f64 / 255.0,
    0x1f as f64 / 255.0,
];
const CLEAR: [f64; 4] = [0.0, 0.0, 0.0, 0.0];

fn near(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() < eps
}

fn assert_colour(got: [f64; 4], want: [f64; 3], eps: f64) {
    for i in 0..3 {
        assert!(
            near(got[i], want[i], eps),
            "channel {i}: {} vs {}",
            got[i],
            want[i]
        );
    }
}

fn opts(layer: [f64; 4], coverage: f64, colour: [f64; 3]) -> PixelOpts {
    PixelOpts {
        layer,
        coverage,
        colour,
        ..PixelOpts::default()
    }
}

#[test]
fn empty_layer_with_no_stroke_shows_the_paper_colour() {
    let c = display_pixel(&PixelOpts {
        paper: Some(WHITE),
        ..opts(CLEAR, 0.0, INK)
    });
    assert_colour(c, WHITE, 1e-6);
    assert_eq!(c[3], 1.0, "display output is opaque");
}

#[test]
fn default_colour_1d1d1f_is_clearly_visible_on_white_paper() {
    let c = display_pixel(&PixelOpts {
        paper: Some(WHITE),
        ..opts(CLEAR, 1.0, INK)
    });
    assert_colour(c, INK, 1e-6);
    // WCAG contrast well above the 4.5 needed for text
    let cr = contrast_ratio(INK, WHITE);
    assert!(cr > 12.0, "contrast {cr:.1}");
}

#[test]
fn erase_reveals_paper_underneath_paper_is_not_baked_into_the_layer() {
    let painted = [INK[0], INK[1], INK[2], 1.0]; // opaque ink already on the layer
    let erased = stroke_composite_pixel(&PixelOpts {
        erase: true,
        ..opts(painted, 1.0, INK)
    });
    let rounded: Vec<f64> = erased.iter().map(|v| (v * 1e6).round() / 1e6).collect();
    assert_eq!(rounded, vec![0.0, 0.0, 0.0, 0.0]);
    let shown = display_pixel(&PixelOpts {
        erase: true,
        paper: Some(WHITE),
        ..opts(painted, 1.0, INK)
    });
    assert_colour(shown, WHITE, 1e-6);
}

#[test]
fn partial_erase_lets_paper_show_through_in_proportion() {
    let painted = [INK[0], INK[1], INK[2], 1.0];
    let half = display_pixel(&PixelOpts {
        erase: true,
        paper: Some(WHITE),
        ..opts(painted, 0.5, INK)
    });
    // result alpha is 0.5 before paper; paper fills the rest
    let expected = INK[0] * 0.5 + 0.5;
    assert!(
        near(half[0], expected, 1e-6),
        "got {} expected {expected}",
        half[0]
    );
}

#[test]
fn normal_paint_over_the_layer_uses_source_over_with_alpha_f() {
    let out = stroke_composite_pixel(&opts(CLEAR, 0.5, INK));
    assert!(near(out[3], 0.5, 1e-6));
    assert!(near(out[0], INK[0] * 0.5, 1e-6));
}

#[test]
fn alpha_lock_never_paints_onto_transparent_pixels() {
    let out = stroke_composite_pixel(&PixelOpts {
        alpha_lock: true,
        ..opts(CLEAR, 1.0, INK)
    });
    assert_eq!(out, [0.0, 0.0, 0.0, 0.0]);
    let painted = [0.2, 0.2, 0.2, 0.5];
    let locked = stroke_composite_pixel(&PixelOpts {
        alpha_lock: true,
        ..opts(painted, 1.0, INK)
    });
    assert!(near(locked[3], 0.5, 1e-6), "alpha is kept");
}

#[test]
fn wet_edges_follow_mix_a_1_minus_abs_sin_4_5a_over_4_5a_with_wet_e() {
    let a = 1.0f64;
    let wet = 1.0 - (f64::sin(4.5 * a) / (4.5 * a)).abs();
    let out = stroke_composite_pixel(&PixelOpts {
        wet_edges: 1.0,
        ..opts(CLEAR, a, INK)
    });
    assert!(near(out[3], wet, 1e-9), "alpha {} vs {wet}", out[3]);
    let none = stroke_composite_pixel(&PixelOpts {
        wet_edges: 0.0,
        ..opts(CLEAR, a, INK)
    });
    assert!(near(none[3], 1.0, 1e-9));
}

#[test]
fn alpha_threshold_drops_coverage_below_the_threshold() {
    let below = stroke_composite_pixel(&PixelOpts {
        threshold: 0.5,
        ..opts(CLEAR, 0.2, INK)
    });
    assert_eq!(below, [0.0, 0.0, 0.0, 0.0]);
    let above = stroke_composite_pixel(&PixelOpts {
        threshold: 0.5,
        ..opts(CLEAR, 0.8, INK)
    });
    assert!(near(above[3], 0.8, 1e-6));
}

#[test]
fn burnt_edges_darken_the_colour_in_the_low_coverage_band_only() {
    let white = [1.0, 1.0, 1.0];
    let edge = stroke_composite_pixel(&PixelOpts {
        burnt: 1.0,
        ..opts(CLEAR, 0.05, white)
    });
    let core = stroke_composite_pixel(&PixelOpts {
        burnt: 1.0,
        ..opts(CLEAR, 1.0, white)
    });
    assert!(edge[0] / edge[3] < 0.5, "edge is darker");
    assert!(near(core[0] / core[3], 1.0, 1e-9), "core keeps the color");
}

#[test]
fn over_paper_makes_any_premultiplied_pixel_opaque() {
    let out = over_paper([0.25, 0.0, 0.0, 0.5], WHITE);
    assert_eq!(out, [0.25 + 0.5, 0.5, 0.5, 1.0]);
}

#[test]
fn contrast_ratio_is_symmetric_and_21_for_black_on_white() {
    assert!(near(contrast_ratio([0.0, 0.0, 0.0], WHITE), 21.0, 1e-9));
    assert!(near(
        contrast_ratio(INK, WHITE),
        contrast_ratio(WHITE, INK),
        1e-6
    ));
}
