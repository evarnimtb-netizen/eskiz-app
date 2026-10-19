//! Colour maths: HSB, hex, the colour disc, harmonies, history and palettes. The Display P3
//! conversion tests are in `eskiz-types` (`tests/colour_space.rs`).

use eskiz_guides::colour::{hsb_to_rgb, rgb_to_hsb, ColourHistory, Hsb, Rgb, Rgb8};
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
