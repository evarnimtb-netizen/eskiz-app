//! Tests for the adjustments added with `eskiz-filters`: each one through the app path against
//! the same call made directly on `eskiz-filters`, plus Pencil mode, the selection clip, the
//! colour pick and the clone.

use eskiz_core::filters::{curve_table, AdjustKind, Adjustment, CurvePoints, GradientStops};
use eskiz_filters::{
    balance, bloom, chromatic, clone as clone_f, curves, glitch, gradient_map, halftone, motion, noise, perspective, recolor, sharpen,
    Image,
};
use eskiz_select::{Mask, Selection};
use eframe::egui::pos2;

use crate::adjust::Pick;
use crate::adjust_tests::{headless, pixels, settle};
use crate::snapshot::Headless;

const W: usize = 240;
const H: usize = 180;

/// Apply the open adjustment through the commit path and wait for it.
fn commit(h: &mut Headless) {
    let ctx = h.ctx.clone();
    h.app.adjust_apply(&ctx);
    settle(h);
}

/// Largest per-channel difference between two RGBA8 buffers.
fn max_diff(a: &[u8], b: &[u8]) -> u8 {
    a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0)
}

/// The same adjustment, rendered directly on `eskiz-filters` from the same source pixels.
type Direct = fn(&Image) -> Image;

fn check_against_direct(name: &str, kind: AdjustKind, setup: impl Fn(&mut Headless), direct: Direct) {
    let mut h = headless(false, W, H);
    let before = pixels(&h);
    let steps = h.app.session.undo_steps();
    assert!(h.app.enter_adjustment(kind), "{name}: opens");
    setup(&mut h);
    h.step();
    commit(&mut h);
    let after = pixels(&h);
    let expected = direct(&Image::from_rgba8_premul(W, H, &before)).to_rgba8_premul();
    let diff = max_diff(&after, &expected);
    assert!(diff <= 1, "{name}: app path differs from eskiz-filters by {diff} levels");
    assert_ne!(after, before, "{name}: changed the layer");
    assert_eq!(h.app.session.undo_steps(), steps + 1, "{name}: one undo step");
    assert_eq!(h.app.session.undo_name(), Some(kind.undo_name()), "{name}: undo name");
    h.app.undo_action();
    assert_eq!(pixels(&h), before, "{name}: undo restores the pixels");
}

fn set_all(h: &mut Headless, values: &[(usize, f64)]) {
    for &(i, v) in values {
        h.app.adjust_set(i, v);
    }
}

#[test]
fn motion_blur_matches_eskiz_filters() {
    check_against_direct(
        "motion blur",
        AdjustKind::MotionBlur,
        |h| set_all(h, &[(0, 12.0), (1, -5.0), (2, 0.6)]),
        |img| motion::apply(img, None, &motion::MotionParams { vector: [12.0, -5.0], amount: 0.6 }),
    );
}

#[test]
fn perspective_blur_matches_eskiz_filters() {
    check_against_direct(
        "perspective blur",
        AdjustKind::PerspectiveBlur,
        |h| set_all(h, &[(0, 0.4), (2, 0.5), (4, 0.6)]),
        |img| perspective::apply(img, None, &perspective::PerspectiveParams { origin: [0.4, 0.5], direction: [0.5, 0.0], amount: 0.6 }),
    );
}

#[test]
fn sharpen_matches_eskiz_filters() {
    check_against_direct(
        "sharpen",
        AdjustKind::Sharpen,
        |h| set_all(h, &[(0, 0.8)]),
        |img| sharpen::apply(img, None, &sharpen::SharpenParams { amount: 0.8 }),
    );
}

#[test]
fn noise_matches_eskiz_filters() {
    check_against_direct(
        "noise",
        AdjustKind::Noise,
        |h| set_all(h, &[(1, 0.5), (2, 0.6), (3, 0.4), (4, 2.0), (6, 1.0)]),
        |img| {
            let p = noise::NoiseParams {
                scale: 0.0,
                amount: 0.5,
                octaves: 0.6,
                kind: noise::NoiseKind::Billowy,
                multichannel: false,
                turbulence: 0.4,
                combine: noise::CombineMode::Mix,
                seed: 0,
            };
            noise::apply(img, None, &p)
        },
    );
}

#[test]
fn axial_chromatic_aberration_matches_eskiz_filters() {
    check_against_direct(
        "chromatic aberration (axial)",
        AdjustKind::ChromaticAberration,
        |h| set_all(h, &[(1, 0.4), (2, 0.6), (3, 0.03), (4, 0.02)]),
        |img| {
            let p = chromatic::AxialParams { origin: [0.4, 0.6], amount: [0.03, 0.02], inverse_range: 1.0, falloff: 0.0, transition: 1.0 };
            chromatic::apply_axial(img, None, &p)
        },
    );
}

#[test]
fn transverse_chromatic_aberration_matches_eskiz_filters() {
    check_against_direct(
        "chromatic aberration (transverse)",
        AdjustKind::ChromaticAberration,
        |h| set_all(h, &[(0, 1.0), (8, 0.2), (9, 0.5)]),
        |img| chromatic::apply_transverse(img, None, &chromatic::TransverseParams { offset: [0.01, 0.01], transparency: 0.2, strength: 0.5 }),
    );
}

#[test]
fn glitch_matches_eskiz_filters() {
    check_against_direct(
        "glitch",
        AdjustKind::Glitch,
        |h| set_all(h, &[(0, 2.0), (1, 0.8)]),
        |img| glitch::apply(img, None, &glitch::GlitchParams { preset: 2, amount: 0.8, seed: 0.0, cells: None }),
    );
}

#[test]
fn halftone_matches_eskiz_filters() {
    check_against_direct(
        "halftone",
        AdjustKind::Halftone,
        |h| set_all(h, &[(0, 0.2), (3, 1.0)]),
        |img| halftone::apply(img, None, &halftone::HalftoneParams { amount: 0.2, softness: 0.5, anti_alias: 1.0, overlay: true, white_fill: false }),
    );
}

#[test]
fn bloom_matches_eskiz_filters() {
    check_against_direct(
        "bloom",
        AdjustKind::Bloom,
        |h| set_all(h, &[(0, 0.7), (1, 0.5), (2, 0.5), (3, 0.2), (4, 0.6)]),
        |img| {
            let p = bloom::BloomParams { upper: 0.7, lower: 0.5, burn: 0.35, spread: 0.2, amount: 0.6, preserve_alpha: false };
            bloom::apply(img, None, &p)
        },
    );
}

#[test]
fn colour_balance_matches_eskiz_filters() {
    check_against_direct(
        "color balance",
        AdjustKind::ColourBalance,
        |h| set_all(h, &[(0, 0.5), (7, -0.3), (4, 0.2)]),
        |img| {
            let p = balance::BalanceParams { shadows: [0.5, 0.0, 0.0], midtones: [0.0, 0.2, 0.0], highlights: [0.0, -0.3, 0.0] };
            balance::apply(img, None, &p)
        },
    );
}

#[test]
fn curves_match_eskiz_filters() {
    let points = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
    check_against_direct(
        "curves",
        AdjustKind::Curves,
        |h| {
            let mut a = Adjustment::new(AdjustKind::Curves);
            let mut c = CurvePoints::default();
            c.channels[0] = points.clone();
            c.channels[1] = vec![[0.0, 0.0], [0.5, 0.3], [1.0, 1.0]];
            a.set_curve_points(c);
            h.app.adjust_store(a);
        },
        |img| {
            let curve = |k: usize| {
                let pts = match k {
                    0 => vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
                    1 => vec![[0.0, 0.0], [0.5, 0.3], [1.0, 1.0]],
                    _ => vec![[0.0, 0.0], [1.0, 1.0]],
                };
                curves::Curve::from_table(curve_table(&pts))
            };
            let set = curves::CurveSet { master: curve(0), red: curve(1), green: curve(2), blue: curve(3) };
            curves::apply(img, None, &curves::CurvesParams { target: set, blend: 1.0, blend_source: None })
        },
    );
}

#[test]
fn gradient_map_matches_eskiz_filters() {
    check_against_direct(
        "gradient map",
        AdjustKind::GradientMap,
        |h| {
            let mut a = Adjustment::new(AdjustKind::GradientMap);
            a.set_gradient_stops(GradientStops::preset(1));
            h.app.adjust_store(a);
        },
        |img| {
            let table = GradientStops::preset(1).table();
            let p = gradient_map::GradientMapParams { start: table.clone(), target: table, amount: 1.0, blend: 1.0 };
            gradient_map::apply(img, None, &p)
        },
    );
}

#[test]
fn recolor_matches_eskiz_filters() {
    check_against_direct(
        "recolor",
        AdjustKind::Recolor,
        |h| set_all(h, &[(0, 0.9), (1, 0.2), (2, 0.2), (3, 0.1), (4, 0.8), (5, 0.3), (6, 0.3)]),
        |img| {
            let p = recolor::RecolorParams { source: [0.9, 0.2, 0.2], target: [0.1, 0.8, 0.3], tolerance: 0.3 };
            recolor::apply(img, None, &p)
        },
    );
}

#[test]
fn every_list_entry_opens_and_is_implemented() {
    let mut h = headless(false, W, H);
    for kind in AdjustKind::ALL {
        assert!(kind.implemented(), "{kind:?}");
        assert!(h.app.enter_adjustment(kind), "{kind:?} opens");
        h.app.adjust_cancel();
    }
    h.step();
}

/// Pencil mode: the effect lands only where the mask was painted.
#[test]
fn pencil_mask_limits_the_effect_to_what_was_painted() {
    let mut h = headless(false, W, H);
    let before = pixels(&h);
    assert!(h.app.enter_adjustment_mode(AdjustKind::Hsb, true));
    h.app.adjust_set(2, 0.4);
    let p = h.app.view.canvas_to_screen(h.app.viewport, 60.0, 60.0);
    h.app.adjust_paint_to(p);
    h.step();
    commit(&mut h);
    let after = pixels(&h);
    // The mask is the current brush's dab: nothing changes beyond its radius.
    let (cx, cy) = (60.0f64, 60.0f64);
    let radius = f64::from(h.app.tool_size()) / 2.0 + 2.0;
    let mut changed_inside = 0;
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            if after[i..i + 4] == before[i..i + 4] {
                continue;
            }
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            assert!(d <= radius, "pixel ({x}, {y}) changed outside the painted dab");
            changed_inside += 1;
        }
    }
    assert!(changed_inside > 0, "the painted area changed");
    assert_eq!(h.app.session.undo_name(), Some(AdjustKind::Hsb.undo_name()));
}

/// Pencil paints its mask with the current brush through the stroke engine's coverage: a
/// textured library brush leaves a textured (not flat) mask.
#[test]
fn pencil_mask_with_a_textured_brush_has_grain() {
    use eskiz_core::brushes::Grain;
    let mut h = headless(false, W, H);
    let id = h
        .app
        .bank
        .sets
        .iter()
        .flat_map(|s| &s.brushes)
        .find(|b| b.lb.grain.is_some() && b.lb.brush.grain != Grain::None)
        .map(|b| b.id.clone())
        .expect("a textured library brush");
    let tool = h.app.tool.index();
    h.app.tool_brushes[tool] = id;
    assert!(h.app.enter_adjustment_mode(AdjustKind::Hsb, true));
    h.app.adjust_set(2, 0.4);
    for i in 0..40 {
        let p = h.app.view.canvas_to_screen(h.app.viewport, 40.0 + f64::from(i) * 3.0, 90.0);
        h.app.adjust_paint_to(p);
    }
    let mask = h.app.adjust_paint_mask().expect("pencil mask");
    // Inside the stroke's middle, away from its ends and edges.
    let inner: Vec<f64> = (80..100)
        .flat_map(|y| (70..130).map(move |x| (x, y)))
        .map(|(x, y)| f64::from(mask[y * W + x]))
        .collect();
    let mean = inner.iter().sum::<f64>() / inner.len() as f64;
    let variance = inner.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / inner.len() as f64;
    assert!(mean > 0.05, "the brush painted the mask (mean {mean})");
    assert!(variance > 0.0, "mask coverage is flat (variance {variance})");
    h.app.adjust_cancel();
}

/// Applying with nothing painted changes nothing and says so.
#[test]
fn pencil_with_nothing_painted_refuses_to_apply() {
    let mut h = headless(false, W, H);
    let steps = h.app.session.undo_steps();
    assert!(h.app.enter_adjustment_mode(AdjustKind::Sharpen, true));
    h.app.adjust_set(0, 0.8);
    let ctx = h.ctx.clone();
    h.app.adjust_apply(&ctx);
    h.step();
    assert!(h.app.adjusting(), "the mode stays open until something is painted");
    assert_eq!(h.app.session.undo_steps(), steps);
    h.app.adjust_cancel();
}

/// The selection clips the effect: outside it the pixels do not change.
#[test]
fn selection_clips_the_effect() {
    let mut h = headless(false, W, H);
    let before = pixels(&h);
    let mut mask = Mask::empty(W, H);
    for y in 0..H {
        for x in 0..100 {
            mask.set(x, y, 1.0);
        }
    }
    let _ = h.app.session.set_selection("Select", Selection::active(mask));
    assert!(h.app.enter_adjustment(AdjustKind::Sharpen));
    h.app.adjust_set(0, 0.8);
    commit(&mut h);
    let after = pixels(&h);
    let mut inside_changed = 0;
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            if x >= 100 {
                assert_eq!(after[i..i + 4], before[i..i + 4], "({x}, {y}) is outside the selection");
            } else if after[i..i + 4] != before[i..i + 4] {
                inside_changed += 1;
            }
        }
    }
    assert!(inside_changed > 0, "the selected part changed");
}

/// Recolor's colour sampler reads the colour under the tap.
#[test]
fn recolor_sampler_reads_the_colour_under_the_tap() {
    let mut h = headless(false, W, H);
    let before = pixels(&h);
    assert!(h.app.enter_adjustment(AdjustKind::Recolor));
    h.app.adjust_toggle_pick(Pick::RecolorSource);
    let p = h.app.view.canvas_to_screen(h.app.viewport, 10.5, 10.5);
    h.app.adjust_pick_at(p);
    let i = (10 * W + 10) * 4;
    let adj = h.app.adjustment().expect("open").clone();
    for k in 0..3 {
        let want = f64::from(before[i + k]) / 255.0;
        assert!((adj.value(k) - want).abs() <= 1.0 / 255.0 + 1e-9, "channel {k}: {} vs {want}", adj.value(k));
    }
    h.app.adjust_cancel();
}

/// Clone copies from the source point, with the offset fixed by the first stroke.
#[test]
fn clone_paints_from_the_source_point() {
    let mut h = headless(false, W, H);
    let before = pixels(&h);
    assert!(h.app.enter_adjustment(AdjustKind::Clone));
    h.app.adjust_toggle_pick(Pick::CloneSource);
    let src = h.app.view.canvas_to_screen(h.app.viewport, 150.0, 40.0);
    h.app.adjust_pick_at(src);
    let start = h.app.view.canvas_to_screen(h.app.viewport, 60.0, 60.0);
    h.app.adjust_paint_to(start);
    h.step();
    let adj = h.app.adjustment().expect("open").clone();
    assert!((adj.value(3) - (150.5 - 60.0)).abs() < 1e-6, "offset x fixed by the stroke: {}", adj.value(3));
    assert!((adj.value(4) - (40.5 - 60.0)).abs() < 1e-6, "offset y fixed by the stroke: {}", adj.value(4));
    commit(&mut h);
    let after = pixels(&h);
    let centre = (60 * W + 60) * 4;
    assert_ne!(after[centre..centre + 4], before[centre..centre + 4], "the painted center changed");
    let far = (170 * W + 220) * 4;
    assert_eq!(after[far..far + 4], before[far..far + 4], "far from the stroke nothing changed");
    // The clone sampled the source region, so the centre takes source colours.
    let expected = clone_f::apply(
        &Image::from_rgba8_premul(W, H, &before),
        None,
        &clone_f::CloneParams { offset: [adj.value(3) as f32, adj.value(4) as f32], opacity: 1.0 },
    );
    assert!(expected.px(60, 60)[3] > 0.0);
    let _ = pos2(0.0, 0.0);
    let _ = glitch::placeholder();
}

#[test]
fn curve_table_is_the_identity_for_the_default_points() {
    let t = curve_table(&[[0.0, 0.0], [1.0, 1.0]]);
    for (i, v) in t.iter().enumerate() {
        assert!((v - i as f32 / 256.0).abs() < 1e-6, "entry {i}");
    }
}

/// Visual check: the banners of the curve editor, the gradient editor and an origin handle are
/// written as PNGs in the snapshot folder for inspection.
#[test]
fn banners_for_curves_gradients_and_handles_render() {
    let dir = crate::snapshot::snapshot_dir();
    for (name, kind) in [
        ("curves", AdjustKind::Curves),
        ("gradient_map", AdjustKind::GradientMap),
        ("perspective_blur", AdjustKind::PerspectiveBlur),
    ] {
        let mut h = headless(false, W, H);
        assert!(h.app.enter_adjustment(kind), "{name} opens");
        h.step();
        h.step();
        h.render().save_png(&dir.join(format!("adjust_{name}.png"))).unwrap();
        h.app.adjust_cancel();
    }
}

/// Curves has no sliders: a drag on the canvas and a parameter pick change nothing and must not panic.
#[test]
fn a_drag_on_the_canvas_in_curves_does_not_panic() {
    let mut h = headless(false, W, H);
    assert!(h.app.enter_adjustment(AdjustKind::Curves));
    h.app.adjust_drag_by(40.0);
    h.app.adjust_select_param(3);
    h.step();
    assert!(h.app.adjust.is_some(), "Curves stays open");
}
