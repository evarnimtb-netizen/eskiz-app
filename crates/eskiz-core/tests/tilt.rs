//! Pen tilt and barrel roll: the tilt-to-size, tilt-to-opacity and tilt-to-roundness
//! curves, and azimuth- and roll-aligned tips.

use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use eskiz_core::brushes::Brush;
use eskiz_core::geometry::{
    dynamics_step, tilt_amount, tilt_factors, Cfg, DynState, NormSample, PointerType, Sample, Stroke, StrokeOptions,
};

/// A brush with no pressure response, so tilt is the only thing that can move size or opacity.
fn flat_brush() -> Brush {
    Brush {
        size: 40.0,
        opacity: 0.8,
        pressure_size: 0.0,
        pressure_opacity: 0.0,
        taper: eskiz_core::brushes::TaperPair {
            pencil: eskiz_core::brushes::Taper { start_length: 0.0, end_length: 0.0, size: 0.0, ..Default::default() },
            finger: eskiz_core::brushes::Taper { start_length: 0.0, end_length: 0.0, size: 0.0, ..Default::default() },
        },
        ..Brush::default()
    }
}

/// Size, minor size and opacity of the first control for a pen at `altitude_norm`.
fn control(brush: &Brush, altitude_norm: f64) -> (f64, f64, f64) {
    let cfg = Cfg::new(brush.clone(), 120.0, 1.0, 1.0);
    let mut st = DynState::default();
    let s = NormSample { x: 10.0, y: 10.0, p: 1.0, has_pressure: true, altitude_norm, ..NormSample::default() };
    let c = dynamics_step(&mut st, &s, &cfg, None, 0.0);
    (c.sx, c.sy, c.opacity)
}

#[test]
fn tilt_amount_runs_from_upright_to_flat() {
    assert_eq!(tilt_amount(1.0), 0.0);
    assert_eq!(tilt_amount(0.0), 1.0);
    assert!((tilt_amount(0.25) - 0.75).abs() < 1e-12);
    // Out-of-range input is clamped.
    assert_eq!(tilt_amount(1.4), 0.0);
}

#[test]
fn a_brush_without_tilt_settings_ignores_tilt() {
    let b = flat_brush();
    let upright = control(&b, 1.0);
    for alt in [0.8, 0.5, 0.1, 0.0] {
        assert_eq!(control(&b, alt), upright, "altitude {alt}");
    }
    assert_eq!(tilt_factors(&b, 0.2), (1.0, 1.0, 1.0));
}

#[test]
fn tilt_size_grows_and_shrinks_the_tip() {
    let grow = Brush { tilt_size: 1.0, ..flat_brush() };
    let (upright, _, _) = control(&grow, 1.0);
    let (half, _, _) = control(&grow, 0.5);
    let (flat, _, _) = control(&grow, 0.0);
    assert!((upright - 40.0).abs() < 1e-9, "upright is the nominal size ({upright})");
    assert!((half - 40.0 * 2.5).abs() < 1e-9, "half lean grows 2.5x ({half})");
    assert!((flat - 40.0 * 4.0).abs() < 1e-9, "flat grows 4x ({flat})");
    assert!(upright < half && half < flat);

    let shrink = Brush { tilt_size: -1.0, ..flat_brush() };
    let (s_half, _, _) = control(&shrink, 0.5);
    let (s_flat, _, _) = control(&shrink, 0.0);
    assert!((s_half - 20.0).abs() < 1e-9, "{s_half}");
    assert!(s_flat < 1e-9, "a flat pen vanishes ({s_flat})");
}

#[test]
fn tilt_opacity_follows_the_curve_in_both_directions() {
    let darker = Brush { tilt_opacity: 0.5, ..flat_brush() };
    let (_, _, up) = control(&darker, 1.0);
    let (_, _, flat) = control(&darker, 0.0);
    assert!((up - 0.8 * 0.5).abs() < 1e-9, "upright is half as dark ({up})");
    assert!((flat - 0.8).abs() < 1e-9, "flat keeps the brush opacity ({flat})");

    let lighter = Brush { tilt_opacity: -0.5, ..flat_brush() };
    let (_, _, up) = control(&lighter, 1.0);
    let (_, _, flat) = control(&lighter, 0.0);
    assert!((up - 0.8).abs() < 1e-9);
    assert!((flat - 0.8 * 0.5).abs() < 1e-9);
}

#[test]
fn tilt_roundness_flattens_the_tip() {
    let b = Brush { tilt_roundness: 1.0, ..flat_brush() };
    let (sx, sy, _) = control(&b, 1.0);
    assert!((sx - sy).abs() < 1e-9, "upright tip is round");
    let (sx, sy, _) = control(&b, 0.0);
    assert!((sy / sx - 0.1).abs() < 1e-9, "flat tip is 10% as tall as wide ({})", sy / sx);
}

fn pen_stroke(brush: Brush, altitude: f64, azimuth: f64, roll: f64) -> Vec<eskiz_core::Dab> {
    let mut s = Stroke::new(brush, StrokeOptions::default());
    for i in 0..30 {
        s.add_sample(Sample {
            x: 20.0 + f64::from(i) * 3.0,
            y: 40.0,
            t: f64::from(i) / 120.0,
            pressure: Some(0.8),
            tilt: Some(altitude),
            azimuth: Some(azimuth),
            roll,
            pointer: PointerType::Pen,
            has_pressure: Some(true),
            ..Sample::default()
        });
    }
    s.finish().dabs
}

#[test]
fn tilt_changes_the_size_and_opacity_of_real_dabs() {
    let b = Brush { tilt_size: 0.6, tilt_opacity: 0.6, ..flat_brush() };
    let upright = pen_stroke(b.clone(), FRAC_PI_2, 0.0, 0.0);
    let leaning = pen_stroke(b, FRAC_PI_4, 0.0, 0.0);
    let mid = |d: &[eskiz_core::Dab]| d[d.len() / 2];
    assert!(mid(&leaning).sx > mid(&upright).sx * 1.5, "{} vs {}", mid(&leaning).sx, mid(&upright).sx);
    assert!(mid(&leaning).opacity > mid(&upright).opacity);
}

#[test]
fn tips_follow_the_azimuth_only_when_asked() {
    let off = Brush { tilt_rotation: false, ..flat_brush() };
    let on = Brush { tilt_rotation: true, ..flat_brush() };
    for az in [0.0, 1.0, -2.0] {
        let plain = pen_stroke(off.clone(), FRAC_PI_4, az, 0.0);
        assert!(plain.iter().all(|d| d.angle.abs() < 1e-9), "azimuth {az} rotated a plain tip");
        let aligned = pen_stroke(on.clone(), FRAC_PI_4, az, 0.0);
        let mid = aligned[aligned.len() / 2].angle;
        let diff = (mid - az + PI).rem_euclid(2.0 * PI) - PI;
        assert!(diff.abs() < 1e-6, "tip angle {mid} should be the azimuth {az}");
    }
}

#[test]
fn a_turned_canvas_turns_the_azimuth_with_it() {
    // The app passes minus the view rotation, so a tip that follows the pen stays put on screen.
    let brush = Brush { tilt_rotation: true, ..flat_brush() };
    let mut s = Stroke::new(brush, StrokeOptions { canvas_rotation: -0.4, ..StrokeOptions::default() });
    for i in 0..30 {
        s.add_sample(Sample {
            x: 20.0 + f64::from(i) * 3.0,
            y: 40.0,
            t: f64::from(i) / 120.0,
            pressure: Some(0.8),
            tilt: Some(FRAC_PI_4),
            azimuth: Some(1.0),
            pointer: PointerType::Pen,
            has_pressure: Some(true),
            ..Sample::default()
        });
    }
    let dabs = s.finish().dabs;
    let mid = dabs[dabs.len() / 2].angle;
    assert!((mid - 0.6).abs() < 1e-6, "azimuth 1.0 on a canvas turned by 0.4 is 0.6 ({mid})");
}

#[test]
fn tips_follow_the_barrel_roll_only_when_asked() {
    let off = flat_brush();
    let on = Brush { roll_rotation: true, shape_rotation: 0.25, ..flat_brush() };
    let plain = pen_stroke(off, FRAC_PI_2, 0.0, 0.9);
    assert!(plain.iter().all(|d| d.angle.abs() < 1e-9));
    let rolled = pen_stroke(on, FRAC_PI_2, 0.0, 0.9);
    let mid = rolled[rolled.len() / 2].angle;
    assert!((mid - 1.15).abs() < 1e-6, "{mid}");
}

#[test]
fn a_mouse_stroke_has_no_tilt_and_is_unchanged_by_tilt_settings() {
    let mouse = |brush: Brush| {
        let mut s = Stroke::new(brush, StrokeOptions::default());
        for i in 0..30 {
            s.add_sample(Sample {
                x: 20.0 + f64::from(i) * 3.0,
                y: 40.0,
                t: f64::from(i) / 120.0,
                pressure: Some(0.5),
                pointer: PointerType::Mouse,
                has_pressure: Some(true),
                ..Sample::default()
            });
        }
        s.finish().dabs
    };
    let plain = mouse(flat_brush());
    let tilty = mouse(Brush { tilt_size: 0.8, tilt_opacity: -0.8, tilt_roundness: 0.5, ..flat_brush() });
    assert_eq!(plain.len(), tilty.len());
    for (a, b) in plain.iter().zip(&tilty) {
        assert!((a.sx - b.sx).abs() < 1e-9 && (a.opacity - b.opacity).abs() < 1e-9);
    }
}

#[test]
fn stroke_colour_is_untouched_without_jitter_and_shifts_with_it() {
    use eskiz_core::geometry::stroke_colour;
    let colour = [0.8, 0.3, 0.2];
    let plain = Stroke::new(flat_brush(), StrokeOptions { seed: 5, ..StrokeOptions::default() });
    assert_eq!(stroke_colour(plain.brush(), colour, plain.meta()), colour);
    let brush = Brush { hue_jitter: 0.5, lightness_jitter: 0.3, ..flat_brush() };
    let s = Stroke::new(brush, StrokeOptions { seed: 5, ..StrokeOptions::default() });
    let shifted = stroke_colour(s.brush(), colour, s.meta());
    assert_ne!(shifted, colour);
    // The same seed gives the same colour.
    let again = Stroke::new(s.brush().clone(), StrokeOptions { seed: 5, ..StrokeOptions::default() });
    assert_eq!(stroke_colour(again.brush(), colour, again.meta()), shifted);
}
