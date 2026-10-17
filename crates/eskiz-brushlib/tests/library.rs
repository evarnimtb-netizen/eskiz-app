//! Behavioural tests for the built-in brush library: previews, naming, texture
//! determinism and normalisation, preset parameter ranges and edge softness.

use eskiz_brushlib::grains::{grain, GrainKind, GRAIN_SIZE};
use eskiz_brushlib::tips::{tip, TipKind, TIP_SIZE};
use eskiz_brushlib::{library, library_ref, preview_stroke, stroke_mask, LibraryBrush};
use eskiz_core::brushes::{Accumulation, Brush, Grain, Shape};
use image::GrayImage;
use std::collections::HashSet;

fn all_brushes() -> Vec<&'static LibraryBrush> {
    library_ref()
        .iter()
        .flat_map(|set| set.brushes.iter())
        .collect()
}

/// Mean alpha of the stroke's rim: painted pixels with at least one unpainted
/// 4-neighbour. Soft edges fade toward zero, so this is low for soft brushes.
fn edge_alpha(mask: &GrayImage) -> f64 {
    let (w, h) = mask.dimensions();
    let at = |x: u32, y: u32| mask.get_pixel(x, y).0[0];
    let (mut sum, mut count) = (0.0, 0usize);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let a = at(x, y);
            if a == 0 {
                continue;
            }
            let rim = [at(x - 1, y), at(x + 1, y), at(x, y - 1), at(x, y + 1)].contains(&0);
            if rim {
                sum += f64::from(a) / 255.0;
                count += 1;
            }
        }
    }
    assert!(count > 0, "stroke has no rim pixels");
    sum / count as f64
}

/// Mean value of pixels whose distance from the tip centre lies in
/// `[lo, hi]` times the tip radius (0.46 of the image size, as generated).
fn ring_mean(img: &GrayImage, lo: f64, hi: f64) -> f64 {
    let (w, h) = img.dimensions();
    let c = f64::from(w) / 2.0;
    let radius = 0.46 * f64::from(w);
    let (mut sum, mut count) = (0.0, 0usize);
    for y in 0..h {
        for x in 0..w {
            let d = (f64::from(x) + 0.5 - c).hypot(f64::from(y) + 0.5 - c) / radius;
            if (lo..=hi).contains(&d) {
                sum += f64::from(img.get_pixel(x, y).0[0]) / 255.0;
                count += 1;
            }
        }
    }
    sum / count as f64
}

#[test]
fn library_has_eight_sets_of_five_presets() {
    let sets = library_ref();
    let names: Vec<&str> = sets.iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        [
            "Graphite",
            "Inkwell",
            "Pigment",
            "Mist",
            "Surfaces",
            "Char",
            "Washes",
            "Nibs",
        ]
    );
    for set in sets {
        assert_eq!(set.brushes.len(), 5, "set {} size", set.name);
    }
    assert_eq!(all_brushes().len(), 40);
}

#[test]
fn every_preset_produces_a_non_empty_preview() {
    for lb in all_brushes() {
        let img = preview_stroke(lb, 256, 96);
        assert_eq!(img.dimensions(), (256, 96), "{}", lb.name);

        let mask = stroke_mask(lb, 256, 96);
        let painted = mask.pixels().filter(|p| p.0[0] > 0).count();
        let strongest = mask.pixels().map(|p| p.0[0]).max().unwrap_or(0);
        assert!(painted > 50, "{}: only {painted} painted pixels", lb.name);
        // Buildup and low-flow tools (Smudge Stick) are deliberately faint, peaking
        // near 10% coverage.
        assert!(strongest > 24, "{}: faint stroke ({strongest})", lb.name);
    }
}

#[test]
fn names_and_ids_are_unique() {
    let mut names = HashSet::new();
    let mut ids = HashSet::new();
    let mut set_names = HashSet::new();
    for set in library_ref() {
        assert!(set_names.insert(set.name), "duplicate set {}", set.name);
        for lb in &set.brushes {
            assert!(names.insert(lb.name), "duplicate name {}", lb.name);
            assert!(ids.insert(lb.id), "duplicate id {}", lb.id);
            assert_eq!(lb.brush.name, lb.name, "engine name mismatch");
        }
    }
    assert_eq!(names.len(), 40);
}

#[test]
fn library_is_cached_after_first_build() {
    let first = library_ref();
    let second = library_ref();
    assert!(
        std::ptr::eq(first, second),
        "library rebuilt on second call"
    );
    assert_eq!(library().len(), first.len());
}

#[test]
fn textures_are_deterministic_for_the_seed() {
    for kind in TipKind::ALL {
        let a = tip(kind, TIP_SIZE, 42);
        let b = tip(kind, TIP_SIZE, 42);
        assert_eq!(a, b, "tip {kind:?} differs for the same seed");
    }
    for kind in GrainKind::ALL {
        let a = grain(kind, GRAIN_SIZE, 42);
        let b = grain(kind, GRAIN_SIZE, 42);
        assert_eq!(a, b, "grain {kind:?} differs for the same seed");
    }
}

#[test]
fn textures_change_with_the_seed() {
    // Stochastic generators must vary with the seed.
    for kind in [TipKind::Bristle, TipKind::Splatter, TipKind::SprayDots] {
        assert_ne!(tip(kind, TIP_SIZE, 1), tip(kind, TIP_SIZE, 2), "{kind:?}");
    }
    for kind in [GrainKind::PaperTooth, GrainKind::CharcoalNoise] {
        assert_ne!(
            grain(kind, GRAIN_SIZE, 1),
            grain(kind, GRAIN_SIZE, 2),
            "{kind:?}"
        );
    }
}

#[test]
fn tips_are_normalised_and_centred() {
    for kind in TipKind::ALL {
        let img = tip(kind, TIP_SIZE, 9);
        let (w, h) = img.dimensions();
        assert_eq!((w, h), (TIP_SIZE, TIP_SIZE));

        let max = img.pixels().map(|p| p.0[0]).max().unwrap_or(0);
        assert_eq!(max, 255, "{kind:?} is not normalized to a max of 1");

        // Bounding box of the painted area, centred to within half a pixel.
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (x, y, p) in img.enumerate_pixels() {
            if p.0[0] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        assert!(x1 >= x0 && y1 >= y0, "{kind:?} is empty");
        let target = f64::from(TIP_SIZE - 1) / 2.0;
        let cx = (f64::from(x0) + f64::from(x1)) / 2.0;
        let cy = (f64::from(y0) + f64::from(y1)) / 2.0;
        assert!((cx - target).abs() <= 0.5, "{kind:?} center x {cx}");
        assert!((cy - target).abs() <= 0.5, "{kind:?} center y {cy}");
    }
}

#[test]
fn grains_span_the_full_range_and_wrap_smoothly() {
    for kind in GrainKind::ALL {
        let img = grain(kind, GRAIN_SIZE, 5);
        let values: Vec<u8> = img.pixels().map(|p| p.0[0]).collect();
        assert_eq!(*values.iter().min().unwrap(), 0, "{kind:?} min");
        assert_eq!(*values.iter().max().unwrap(), 255, "{kind:?} max");
    }

    // Seam check for the smooth, noise-based grains. Weave and hatch have hard
    // thread and line edges by design, so their seams are not comparable and they
    // are covered by the range check above.
    for kind in [
        GrainKind::PaperTooth,
        GrainKind::CharcoalNoise,
        GrainKind::WatercolourBloom,
        GrainKind::Concrete,
    ] {
        let img = grain(kind, GRAIN_SIZE, 5);
        let n = GRAIN_SIZE;
        let col_diff = |a: u32, b: u32| -> f64 {
            (0..n)
                .map(|y| {
                    (f64::from(img.get_pixel(a, y).0[0]) - f64::from(img.get_pixel(b, y).0[0]))
                        .abs()
                })
                .sum::<f64>()
                / f64::from(n)
        };
        let inner: f64 = (0..n - 1).map(|x| col_diff(x, x + 1)).sum::<f64>() / f64::from(n - 1);
        let seam = col_diff(n - 1, 0);
        assert!(
            seam <= 3.0 * inner + 8.0,
            "{kind:?} seam {seam:.2} vs inner {inner:.2}"
        );
    }
}

#[test]
fn presets_stay_within_the_spec_parameter_ranges() {
    let in_unit = |v: f64| (0.0..=1.0).contains(&v);
    let in_signed = |v: f64| (-1.0..=1.0).contains(&v);
    for lb in all_brushes() {
        let b: &Brush = &lb.brush;
        let n = lb.name;
        assert!(b.size > 0.0 && b.size <= 300.0, "{n}: size {}", b.size);
        assert!(in_unit(b.opacity), "{n}: opacity {}", b.opacity);
        assert!(in_unit(b.flow), "{n}: flow {}", b.flow);
        assert!(
            b.spacing >= 0.01 && b.spacing <= 1.0,
            "{n}: spacing {}",
            b.spacing
        );
        assert!(in_unit(b.spacing_jitter), "{n}: spacing jitter");
        assert!(in_unit(b.smoothing), "{n}: smoothing {}", b.smoothing);
        assert!(in_unit(b.stabilization), "{n}: stabilization");
        assert!(
            in_signed(b.pressure_size),
            "{n}: pressure size {}",
            b.pressure_size
        );
        assert!(in_signed(b.speed_size), "{n}: speed size {}", b.speed_size);
        assert!(in_signed(b.pressure_opacity), "{n}: pressure opacity");
        assert!(in_signed(b.speed_opacity), "{n}: speed opacity");
        assert!(
            in_unit(b.min_size) && in_unit(b.max_size),
            "{n}: size range"
        );
        assert!(b.min_size <= b.max_size, "{n}: min above max");
        assert!(in_unit(b.wet_edges) && in_unit(b.burnt_edges), "{n}: edges");
        assert!(in_unit(b.scatter) && in_unit(b.size_jitter), "{n}: jitter");
        assert!(
            in_unit(b.opacity_jitter) && in_unit(b.rotation_jitter),
            "{n}: jitter"
        );
        assert!(in_unit(b.grain_depth), "{n}: grain depth {}", b.grain_depth);
        assert!(in_signed(b.grain_contrast), "{n}: grain contrast");
        assert!(in_signed(b.grain_brightness), "{n}: grain brightness");
        assert!(
            b.grain_scale > 0.0 && b.grain_scale <= 4.0,
            "{n}: grain scale"
        );
        assert!(
            b.shape_roundness > 0.0 && b.shape_roundness <= 1.0,
            "{n}: roundness"
        );
        for (label, pair) in [("pencil", &b.taper.pencil), ("finger", &b.taper.finger)] {
            assert!(in_unit(pair.start_length), "{n}: {label} taper start");
            assert!(in_unit(pair.end_length), "{n}: {label} taper end");
            assert!(in_signed(pair.shape), "{n}: {label} taper shape");
            assert!(
                in_unit(pair.size) && in_unit(pair.opacity),
                "{n}: {label} taper"
            );
            assert!(in_unit(pair.pressure), "{n}: {label} taper pressure");
        }
        if b.wet.enabled {
            for v in [
                b.wet.attack,
                b.wet.falloff_amount,
                b.wet.load_amount,
                b.wet.dilution,
                b.wet.tilt_angle,
            ] {
                assert!(in_unit(v), "{n}: wet value {v}");
            }
        }
        for curve in [&b.curves.size, &b.curves.opacity, &b.curves.overall] {
            assert!(curve.len() >= 2, "{n}: curve needs two points");
            assert!(
                curve.windows(2).all(|w| w[0][0] < w[1][0]),
                "{n}: curve x not increasing"
            );
        }
    }
}

#[test]
fn textured_presets_have_a_grain_and_plain_ones_do_not() {
    for lb in all_brushes() {
        let has_grain = lb.brush.grain != Grain::None;
        assert_eq!(has_grain, lb.grain.is_some(), "{}", lb.name);
        if has_grain {
            assert!(lb.brush.grain_depth > 0.0, "{} grain has no depth", lb.name);
        }
        if lb.brush.accumulation == Accumulation::Buildup {
            assert!(lb.brush.flow < 1.0, "{} builds up at full flow", lb.name);
        }
    }
}

#[test]
fn tips_are_rendered_for_every_kind() {
    for kind in TipKind::ALL {
        let img = tip(kind, TIP_SIZE, 3);
        assert!(
            img.pixels().any(|p| p.0[0] > 200),
            "{kind:?} has no solid core"
        );
    }
}

#[test]
fn soft_tip_has_lower_edge_alpha_than_hard_tip() {
    let hard = tip(TipKind::RoundHard, TIP_SIZE, 1);
    let soft = tip(TipKind::RoundSoft, TIP_SIZE, 1);
    let hard_rim = ring_mean(&hard, 0.9, 0.95);
    let soft_rim = ring_mean(&soft, 0.9, 0.95);
    assert!(hard_rim > 0.8, "hard rim too faint: {hard_rim:.3}");
    assert!(
        soft_rim < 0.25 && soft_rim < 0.5 * hard_rim,
        "soft rim {soft_rim:.3} vs hard {hard_rim:.3}"
    );
}

#[test]
fn soft_brush_stroke_has_lower_edge_alpha_than_hard_brush_stroke() {
    // Same preset settings, only the tip changes, so the comparison isolates the
    // tip's edge profile.
    let hard_base = library_ref()
        .iter()
        .flat_map(|s| s.brushes.iter())
        .find(|b| b.id == "inking.studio-ink")
        .expect("studio ink preset");

    let mut hard = hard_base.clone();
    hard.grain = None;
    hard.brush.grain = Grain::None;

    let mut soft = hard.clone();
    soft.tip = tip(TipKind::RoundSoft, TIP_SIZE, 1);
    soft.brush.shape = Shape::SoftRound;
    soft.brush.size = 24.0;
    hard.brush.size = 24.0;

    let hard_edge = edge_alpha(&stroke_mask(&hard, 256, 96));
    let soft_edge = edge_alpha(&stroke_mask(&soft, 256, 96));
    assert!(
        soft_edge < hard_edge,
        "soft edge alpha {soft_edge:.3} should be below hard {hard_edge:.3}"
    );
}
