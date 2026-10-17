//! Colour maths: HSB, hex, the colour disc, harmonies, history, palettes and Display P3.

use eskiz_guides::colour::{hsb_to_rgb, rgb_to_hsb, ColourHistory, Hsb, Rgb, Rgb8};
use eskiz_guides::colour_space::{convert, ColourConverter, ColourSpace};
use eskiz_guides::disc::{ColourDisc, DiscHit};
use eskiz_guides::harmony::{harmony, Harmony};
use eskiz_guides::palette::{extract_palette, MAX_PALETTE_COLOURS};
use eskiz_guides::Vec2;

const TOL: f64 = 1e-9;

/// Signed difference between two hues, folded into `(-180, 180]`.
fn hue_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

#[test]
fn hsb_round_trips_through_rgb() {
    for h in (0..360).step_by(7) {
        for si in 0..=8 {
            for bi in 0..=8 {
                let s = f64::from(si) / 8.0;
                let b = f64::from(bi) / 8.0;
                let hsb = Hsb::new(f64::from(h), s, b);
                let back = rgb_to_hsb(hsb_to_rgb(hsb));
                assert!((back.b - b).abs() < TOL, "brightness {hsb:?} -> {back:?}");
                // At zero brightness the colour is black, so saturation and hue are
                // not recoverable.
                if b > 0.0 {
                    assert!((back.s - s).abs() < TOL, "saturation {hsb:?} -> {back:?}");
                    if s > 0.0 {
                        assert!(
                            hue_diff(back.h, hsb.h).abs() < 1e-7,
                            "hue {hsb:?} -> {back:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rgb_to_hsb_round_trips_for_all_8_bit_primaries_and_mixes() {
    for r in (0..=255).step_by(51) {
        for g in (0..=255).step_by(51) {
            for b in (0..=255).step_by(51) {
                let rgb = Rgb8::new(r, g, b).to_rgb();
                let back = hsb_to_rgb(rgb_to_hsb(rgb));
                assert!((back.r - rgb.r).abs() < TOL);
                assert!((back.g - rgb.g).abs() < TOL);
                assert!((back.b - rgb.b).abs() < TOL);
            }
        }
    }
}

#[test]
fn hsb_known_primaries() {
    let red = hsb_to_rgb(Hsb::new(0.0, 1.0, 1.0));
    assert_eq!((red.r, red.g, red.b), (1.0, 0.0, 0.0));
    let green = hsb_to_rgb(Hsb::new(120.0, 1.0, 1.0));
    assert!((green.g - 1.0).abs() < TOL && green.r.abs() < TOL);
    let blue = hsb_to_rgb(Hsb::new(240.0, 1.0, 1.0));
    assert!((blue.b - 1.0).abs() < TOL && blue.r.abs() < TOL);
    let grey = rgb_to_hsb(Rgb::new(0.2, 0.2, 0.2));
    assert_eq!(grey.s, 0.0);
    assert!((grey.b - 0.2).abs() < TOL);
}

#[test]
fn hex_parsing_and_formatting() {
    let c: Rgb8 = "#1C1C1E".parse().unwrap();
    assert_eq!(c, Rgb8::new(28, 28, 30));
    assert_eq!(c.to_hex(), "#1C1C1E");
    assert_eq!("fff".parse::<Rgb8>().unwrap(), Rgb8::new(255, 255, 255));
    assert_eq!("#0a0".parse::<Rgb8>().unwrap(), Rgb8::new(0, 170, 0));
    assert_eq!("3b82f6".parse::<Rgb8>().unwrap().to_hex(), "#3B82F6");
    assert!("#12345".parse::<Rgb8>().is_err());
    assert!("#GG0000".parse::<Rgb8>().is_err());
    assert!("+F0000".parse::<Rgb8>().is_err());
    assert!("".parse::<Rgb8>().is_err());
}

#[test]
fn rgb_quantises_with_clamping() {
    assert_eq!(Rgb::new(1.5, -0.2, 0.5).to_rgb8(), Rgb8::new(255, 0, 128));
}

#[test]
fn disc_ring_round_trips_hue() {
    let disc = ColourDisc::new(100.0);
    for hue in (0..360).step_by(5) {
        let p = disc.hue_point(f64::from(hue));
        match disc.hit_test(p) {
            DiscHit::Ring { hue: got } => {
                assert!(hue_diff(got, f64::from(hue)).abs() < 1e-7, "{hue} -> {got}");
            }
            other => panic!("expected ring hit, got {other:?}"),
        }
    }
}

#[test]
fn disc_inner_round_trips_hue_and_saturation() {
    let disc = ColourDisc::new(100.0);
    for h in (0..360).step_by(15) {
        for si in 1..=10 {
            let s = f64::from(si) / 10.0;
            let colour = Hsb::new(f64::from(h), s, 0.8);
            match disc.hit_test(disc.point_for(colour)) {
                DiscHit::Inner { hue, saturation } => {
                    assert!(hue_diff(hue, colour.h).abs() < 1e-7);
                    assert!((saturation - s).abs() < 1e-9);
                }
                other => panic!("expected inner hit, got {other:?}"),
            }
        }
    }
}

#[test]
fn disc_colour_at_uses_the_separate_brightness() {
    let disc = ColourDisc::new(100.0);
    let p = disc.point_for(Hsb::new(200.0, 0.5, 1.0));
    let c = disc.colour_at(p, 0.25).unwrap();
    assert!(hue_diff(c.h, 200.0).abs() < 1e-7);
    assert!((c.s - 0.5).abs() < 1e-9);
    assert!((c.b - 0.25).abs() < TOL);
    assert!(disc.colour_at(Vec2::new(1000.0, 0.0), 0.5).is_none());
    assert_eq!(disc.hit_test(Vec2::new(1000.0, 0.0)), DiscHit::Outside);
}

#[test]
fn complementary_is_180_degrees() {
    let base = Hsb::new(30.0, 0.6, 0.9);
    let out = harmony(base, Harmony::Complementary);
    assert_eq!(out.len(), 1);
    assert!(hue_diff(out[0].h, base.h).abs() - 180.0 < TOL);
    assert!((out[0].s - base.s).abs() < TOL && (out[0].b - base.b).abs() < TOL);
}

#[test]
fn triadic_companions_are_plus_and_minus_120_degrees() {
    let base = Hsb::new(10.0, 1.0, 1.0);
    let out = harmony(base, Harmony::Triadic);
    let diffs: Vec<f64> = out.iter().map(|c| hue_diff(c.h, base.h)).collect();
    assert_eq!(diffs.len(), 2);
    assert!((diffs[0] - 120.0).abs() < TOL);
    assert!((diffs[1] + 120.0).abs() < TOL);
}

#[test]
fn other_harmony_angles() {
    let base = Hsb::new(10.0, 1.0, 1.0);
    let hues = |mode| -> Vec<f64> { harmony(base, mode).iter().map(|c| c.h).collect() };
    assert_eq!(hues(Harmony::SplitComplementary), vec![160.0, 220.0]);
    assert_eq!(hues(Harmony::Analogous), vec![340.0, 40.0]);
    assert_eq!(hues(Harmony::Tetradic), vec![70.0, 190.0, 250.0]);
}

#[test]
fn history_keeps_last_ten_without_duplicates() {
    let mut history = ColourHistory::new();
    for i in 0..12u8 {
        history.push(Rgb8::new(i, 0, 0));
    }
    assert_eq!(history.items().len(), 10);
    assert_eq!(history.items()[0], Rgb8::new(11, 0, 0));
    assert_eq!(history.items()[9], Rgb8::new(2, 0, 0));

    let mut h = ColourHistory::new();
    let a = Rgb8::new(1, 2, 3);
    let b = Rgb8::new(4, 5, 6);
    h.push(a);
    h.push(b);
    h.push(a);
    assert_eq!(h.items(), &[a, b]);
}

#[test]
fn palette_small_image_is_exact_and_ordered_by_frequency() {
    let red = [200, 0, 0, 255];
    let green = [0, 200, 0, 255];
    let blue = [0, 0, 200, 255];
    let transparent = [9, 9, 9, 0];
    let mut pixels = Vec::new();
    pixels.extend(std::iter::repeat_n(blue, 1));
    pixels.extend(std::iter::repeat_n(red, 5));
    pixels.extend(std::iter::repeat_n(green, 3));
    pixels.extend(std::iter::repeat_n(transparent, 50));
    let palette = extract_palette(&pixels, 8);
    assert_eq!(
        palette,
        vec![
            Rgb8::new(200, 0, 0),
            Rgb8::new(0, 200, 0),
            Rgb8::new(0, 0, 200)
        ]
    );
}

#[test]
fn palette_clusters_noisy_colours_and_caps_at_thirty() {
    // Two noisy clusters, far apart: k = 2 should find one reddish and one bluish colour.
    let mut pixels = Vec::new();
    for i in 0..400u32 {
        let n = (i % 5) as u8;
        pixels.push([240 - n, 10 + n, 5, 255]);
        pixels.push([5, 12 + n, 230 - n, 255]);
    }
    let palette = extract_palette(&pixels, 2);
    assert_eq!(palette.len(), 2);
    assert!(palette.iter().any(|c| c.r > 200 && c.b < 50));
    assert!(palette.iter().any(|c| c.b > 200 && c.r < 50));

    // Many distinct colours, asking for 100: capped at 30 and deterministic.
    let many: Vec<[u8; 4]> = (0..3000u32)
        .map(|i| {
            [
                (i * 7 % 256) as u8,
                (i * 13 % 256) as u8,
                (i * 29 % 256) as u8,
                255,
            ]
        })
        .collect();
    let a = extract_palette(&many, 100);
    let b = extract_palette(&many, 100);
    assert_eq!(a, b);
    assert!(!a.is_empty() && a.len() <= MAX_PALETTE_COLOURS);
    assert!(extract_palette(&many, 0).is_empty());
    assert!(extract_palette(&[[0, 0, 0, 0]], 5).is_empty());
}

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

use eskiz_guides::colour_space::PixelConverter;

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
