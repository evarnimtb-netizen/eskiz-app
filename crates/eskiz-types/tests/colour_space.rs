//! sRGB and Display P3 conversion: the exact converter, the 8-bit pixel converter and profile names.

use eskiz_types::colour::Rgb;
use eskiz_types::colour_space::{convert, ColourConverter, ColourSpace, PixelConverter};

#[test]
fn srgb_red_is_the_known_display_p3_value() {
    // sRGB pure red expressed in Display P3 is about (234, 51, 35) in 8-bit.
    let p3 = convert(
        Rgb::new(1.0, 0.0, 0.0),
        ColourSpace::Srgb,
        ColourSpace::DisplayP3,
    );
    assert!((p3.r - 234.0 / 255.0).abs() < 0.004, "{p3:?}");
    assert!((p3.g - 51.0 / 255.0).abs() < 0.004, "{p3:?}");
    assert!((p3.b - 35.0 / 255.0).abs() < 0.004, "{p3:?}");
}

#[test]
fn white_and_grey_map_to_themselves_between_spaces() {
    let white = convert(
        Rgb::new(1.0, 1.0, 1.0),
        ColourSpace::Srgb,
        ColourSpace::DisplayP3,
    );
    assert!(
        (white.r - 1.0).abs() < 1e-3
            && (white.g - 1.0).abs() < 1e-3
            && (white.b - 1.0).abs() < 1e-3
    );
    let grey = convert(
        Rgb::new(0.5, 0.5, 0.5),
        ColourSpace::DisplayP3,
        ColourSpace::Srgb,
    );
    assert!((grey.r - grey.g).abs() < 1e-5 && (grey.g - grey.b).abs() < 1e-5);
}

#[test]
fn srgb_to_p3_and_back_is_identity() {
    let forward = ColourConverter::new(ColourSpace::Srgb, ColourSpace::DisplayP3);
    let back = ColourConverter::new(ColourSpace::DisplayP3, ColourSpace::Srgb);
    for ri in 0..=4 {
        for gi in 0..=4 {
            for bi in 0..=4 {
                let c = Rgb::new(
                    f64::from(ri) / 4.0,
                    f64::from(gi) / 4.0,
                    f64::from(bi) / 4.0,
                );
                let round = back.apply_unclamped(forward.apply_unclamped(c));
                assert!((round.r - c.r).abs() < 1e-9);
                assert!((round.g - c.g).abs() < 1e-9);
                assert!((round.b - c.b).abs() < 1e-9);
            }
        }
    }
}

#[test]
fn p3_to_srgb_and_back_is_identity_for_srgb_gamut_colours() {
    // Colours that came from sRGB lie inside both gamuts, so P3 -> sRGB -> P3 must
    // return them unchanged, with no clamping involved.
    let srgb_to_p3 = ColourConverter::new(ColourSpace::Srgb, ColourSpace::DisplayP3);
    let p3_to_srgb = ColourConverter::new(ColourSpace::DisplayP3, ColourSpace::Srgb);
    for c in [
        Rgb::new(0.9, 0.1, 0.3),
        Rgb::new(0.2, 0.7, 0.4),
        Rgb::new(0.05, 0.5, 0.95),
    ] {
        let p3 = srgb_to_p3.apply(c);
        let back = p3_to_srgb.apply_unclamped(p3);
        let round = srgb_to_p3.apply_unclamped(back);
        assert!((round.r - p3.r).abs() < 1e-9);
        assert!((round.g - p3.g).abs() < 1e-9);
        assert!((round.b - p3.b).abs() < 1e-9);
    }
}

#[test]
fn p3_green_clips_when_shown_in_srgb() {
    // Saturated P3 green is outside sRGB. Unclamped it has negative channels, and the
    // clamped result stays displayable.
    let p = Rgb::new(0.0, 1.0, 0.0);
    let raw = ColourConverter::new(ColourSpace::DisplayP3, ColourSpace::Srgb).apply_unclamped(p);
    assert!(raw.r < 0.0 || raw.b < 0.0);
    let shown = convert(p, ColourSpace::DisplayP3, ColourSpace::Srgb);
    assert!(shown.r >= 0.0 && shown.g <= 1.0 && shown.b >= 0.0);
}

// ---- 8-bit pixel conversion and the display transform ------------------------

#[test]
fn srgb_to_p3_to_srgb_is_within_one_level() {
    // Exact (float) conversion: every 8-bit sRGB colour returns to within 1/255, in fact far less.
    let forward = ColourConverter::new(ColourSpace::Srgb, ColourSpace::DisplayP3);
    let back = ColourConverter::new(ColourSpace::DisplayP3, ColourSpace::Srgb);
    for r in (0..=255u16).step_by(5).chain([255]) {
        for g in (0..=255u16).step_by(5).chain([255]) {
            for b in (0..=255u16).step_by(5).chain([255]) {
                let c = Rgb::new(f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0);
                let round = back.apply(forward.apply(c));
                for (a, e) in [(round.r, c.r), (round.g, c.g), (round.b, c.b)] {
                    assert!((a - e).abs() <= 1.0 / 255.0, "{r},{g},{b}: {a} vs {e}");
                }
            }
        }
    }
}

#[test]
fn eight_bit_round_trip_is_exact_for_greys_and_bounded_for_saturated_colours() {
    // Storing the intermediate P3 colour in 8 bits quantises it, and going back to the narrower
    // sRGB gamut amplifies that error for saturated colours (pure green moves 8 levels in red).
    // Neutral colours, which are the common case, return to within one level.
    let forward = PixelConverter::new(ColourSpace::Srgb, ColourSpace::DisplayP3);
    let back = PixelConverter::new(ColourSpace::DisplayP3, ColourSpace::Srgb);
    for v in 0..=255u8 {
        let round = back.convert(forward.convert([v, v, v]));
        assert!(round.iter().all(|&c| (i32::from(c) - i32::from(v)).abs() <= 1), "gray {v}: {round:?}");
    }
    let mut worst = 0i32;
    for r in (0..=255u16).step_by(5).chain([255]) {
        for g in (0..=255u16).step_by(5).chain([255]) {
            for b in (0..=255u16).step_by(5).chain([255]) {
                let c = [r as u8, g as u8, b as u8];
                let round = back.convert(forward.convert(c));
                for i in 0..3 {
                    worst = worst.max((i32::from(round[i]) - i32::from(c[i])).abs());
                }
            }
        }
    }
    assert!(worst <= 10, "worst 8-bit round trip error {worst}");
}

#[test]
fn pixel_converter_matches_the_exact_converter() {
    let lut = PixelConverter::new(ColourSpace::DisplayP3, ColourSpace::Srgb);
    let exact = ColourConverter::new(ColourSpace::DisplayP3, ColourSpace::Srgb);
    for r in (0..=255u16).step_by(15) {
        for g in (0..=255u16).step_by(15) {
            for b in (0..=255u16).step_by(15) {
                let got = lut.convert([r as u8, g as u8, b as u8]);
                let want = exact.apply(Rgb::new(f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0));
                for (i, w) in [want.r, want.g, want.b].into_iter().enumerate() {
                    assert!((f64::from(got[i]) - w * 255.0).abs() <= 1.0, "{r},{g},{b} channel {i}");
                }
            }
        }
    }
}

#[test]
fn display_matrix_maps_p3_red_outside_srgb_and_the_pixels_clip() {
    // The linear matrix takes P3 (1, 0, 0) to roughly (1.2249, -0.0420, -0.0197) in linear sRGB:
    // red above 1 and a small negative green and blue. Those are outside the sRGB gamut.
    let t = ColourSpace::DisplayP3.display_transform().expect("P3 needs a transform");
    let m = t.matrix_f32();
    let col = |i: usize| [m[0][i], m[1][i], m[2][i]];
    let red = col(0);
    assert!((red[0] - 1.2249).abs() < 1e-3, "{red:?}");
    assert!((red[1] + 0.0420).abs() < 1e-3, "{red:?}");
    assert!((red[2] + 0.0197).abs() < 1e-3, "{red:?}");
    // The 8-bit conversion clips each channel to 0..=1: red saturates, green and blue go to 0.
    assert_eq!(t.convert([255, 0, 0]), [255, 0, 0]);
    // White and grey stay neutral.
    assert_eq!(t.convert([255, 255, 255]), [255, 255, 255]);
    let g = t.convert([128, 128, 128]);
    assert!(g.iter().all(|&c| (i32::from(c) - 128).abs() <= 1), "{g:?}");
    // sRGB documents need no transform.
    assert!(ColourSpace::Srgb.display_transform().is_none());
}

#[test]
fn profile_names_round_trip() {
    for space in ColourSpace::ALL {
        assert_eq!(ColourSpace::from_profile_name(space.profile_name()), space);
    }
    assert_eq!(ColourSpace::from_profile_name("Generic RGB Profile"), ColourSpace::Srgb);
    assert_eq!(ColourSpace::from_profile_name("Display P3"), ColourSpace::DisplayP3);
}

#[test]
fn premultiplied_pixels_convert_through_straight_colour() {
    let t = PixelConverter::new(ColourSpace::Srgb, ColourSpace::DisplayP3);
    assert_eq!(t.convert_premultiplied([0, 0, 0, 0]), [0, 0, 0, 0]);
    let opaque = t.convert_premultiplied([200, 40, 40, 255]);
    assert_eq!(&opaque[..3], &t.convert([200, 40, 40]));
    // Half alpha: the straight colour (200, 40, 40) is converted, then scaled by alpha again.
    let half = t.convert_premultiplied([100, 20, 20, 128]);
    let want = t.convert([198, 40, 40]);
    assert_eq!(half[3], 128);
    assert!((i32::from(half[0]) - i32::from(want[0]) * 128 / 255).abs() <= 2);
}
