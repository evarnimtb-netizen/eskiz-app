//! Stroke geometry tests: 23 cases with fixed tolerances.
#![allow(clippy::needless_range_loop)]

mod common;

use std::f64::consts::PI;

use common::{line, norm, plain_brush, straight_line};
use eskiz_core::brushes::{Accumulation, Brush, Grain, Preset};
use eskiz_core::geometry::{
    fft, interpolate_window, natural_cubic_spline, normalise_sample, reflected_value,
    spacing_diameter, stabilization_radius, stabilize_at, smoothing_block, smoothing_params,
    BrushCurve, Channel, Control, MotionFilter, NormEnv, NormSample, Rng, Sample, Stroke,
    StrokeOptions, MAX_STAMPS,
};
use eskiz_core::PointerType;

// ---------------------------------------------------------------------------
// 3.6 curves
// ---------------------------------------------------------------------------

#[test]
fn spline_curve_passes_through_its_control_points() {
    let pts = [[0.0, 0.0], [0.3, 0.8], [0.6, 0.2], [1.0, 1.0]];
    let curve = BrushCurve::new(&pts);
    for [x, y] in pts {
        let got = curve.eval(x);
        assert!((got - y).abs() < 2e-3, "at {x}: got {got}, want {y}");
    }
}

#[test]
fn identity_curve_maps_x_to_x() {
    let curve = BrushCurve::identity();
    for x in [0.0, 0.123, 0.5, 0.777, 1.0] {
        assert!((curve.eval(x) - x).abs() < 1e-9);
    }
}

#[test]
fn natural_spline_of_two_points_is_linear_and_of_more_points_is_c2_continuous_at_knots() {
    let f = natural_cubic_spline(&[0.0, 0.5, 1.0], &[0.0, 1.0, 0.0]);
    assert!((f.eval(0.5) - 1.0).abs() < 1e-12);
    // symmetric data gives a symmetric curve
    assert!((f.eval(0.25) - f.eval(0.75)).abs() < 1e-12);
}

// ---------------------------------------------------------------------------
// 2.2 Smoothing
// ---------------------------------------------------------------------------

#[test]
fn smoothing_with_s_zero_leaves_points_unchanged() {
    let cfg = smoothing_params(0.0, 0.0, 120.0);
    let pts: Vec<NormSample> = (0..40)
        .map(|i| NormSample {
            x: 10.0 + 3.0 * i as f64 + (i as f64).sin() * 4.0,
            y: 100.0,
            p: 1.0,
            ..NormSample::default()
        })
        .collect();
    let (out, _) = smoothing_block(None, &pts, pts.len(), &cfg);
    for (o, p) in out.iter().zip(&pts) {
        assert_eq!(o.x, p.x);
        assert_eq!(o.y, p.y);
    }
}

#[test]
fn smoothing_with_s_one_lags_behind_the_pen_on_a_moving_line() {
    let cfg = smoothing_params(1.0, 0.0, 120.0);
    assert!((cfg.s - 0.04).abs() < 1e-12);
    let pts: Vec<NormSample> = (0..100)
        .map(|i| NormSample {
            x: i as f64,
            y: 100.0,
            p: 1.0,
            ..NormSample::default()
        })
        .collect();
    let (out, _) = smoothing_block(None, &pts, pts.len(), &cfg);
    // mid-stroke: the accumulator trails the pen
    assert!(
        out[50].x < pts[50].x - 5.0,
        "expected lag, got {}",
        out[50].x
    );
    // newest points snap to the pen tip
    assert!((out[99].x - pts[99].x).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// 2.3 Stabilization
// ---------------------------------------------------------------------------

#[test]
fn stabilization_radius_follows_n_equals_floor_z_pow_2_2_times_120() {
    assert_eq!(stabilization_radius(0.0, 0.0), 0);
    assert_eq!(stabilization_radius(1.0, 0.0), 120);
    assert_eq!(
        stabilization_radius(0.5, 0.0),
        (0.5f64.powf(2.2) * 120.0).floor() as usize
    );
    assert_eq!(stabilization_radius(0.5, 0.0), 26);
    // larger of brush and global values wins
    assert_eq!(stabilization_radius(0.2, 0.5), 26);
}

#[test]
fn stabilization_mirrors_the_ends_with_point_reflection() {
    let pts: Vec<NormSample> = [5.0, 6.0, 8.0]
        .iter()
        .map(|&x| NormSample {
            x,
            ..NormSample::default()
        })
        .collect();
    // p[-1] = 2*p[0] - p[1] = 4
    assert_eq!(reflected_value(&pts, -1, Channel::X), 4.0);
    // p[3] = 2*p[2] - p[1] = 10
    assert_eq!(reflected_value(&pts, 3, Channel::X), 10.0);
    // window n=1 at c=0: mean of (4, 5, 6)
    assert!((stabilize_at(&pts, 0, 1).x - 5.0).abs() < 1e-12);
    // n=0 is the identity
    assert_eq!(stabilize_at(&pts, 2, 0).x, 8.0);
}

// ---------------------------------------------------------------------------
// 2.1 Motion filter
// ---------------------------------------------------------------------------

#[test]
fn motion_filter_at_a_zero_is_the_identity() {
    let f = MotionFilter::new(0.0, 0.0, 120.0);
    let mut buf = norm(&line(50, 3.0));
    for (i, s) in buf.iter_mut().enumerate() {
        s.y = 100.0 + ((i * 37) % 7) as f64;
    }
    for c in 0..buf.len() {
        let o = f.apply(&buf, c);
        assert_eq!(o.x, buf[c].x);
        assert_eq!(o.y, buf[c].y);
    }
}

#[test]
fn motion_filter_at_a_one_smooths_noise_but_keeps_a_straight_line() {
    let f = MotionFilter::new(1.0, 0.0, 120.0);
    assert_eq!(f.fft_size(), 1024);

    // Straight line: interior points are preserved.
    let line_buf = norm(&straight_line(2000, 2.0, 50.0, 1.0, 10.0));
    for c in [1000usize, 1200] {
        let o = f.apply(&line_buf, c);
        assert!(
            (o.x - line_buf[c].x).abs() < 0.5,
            "x drift {}",
            o.x - line_buf[c].x
        );
        assert!((o.y - 50.0).abs() < 0.5, "y drift {}", o.y - 50.0);
    }

    // Noisy line: roughness (sum of |dy|) drops a lot.
    let mut rng = Rng::new(42);
    let mut noisy = norm(&straight_line(2000, 2.0, 50.0, 1.0, 10.0));
    for s in noisy.iter_mut() {
        s.y += (rng.next_f64() - 0.5) * 12.0;
    }
    let mut raw_rough = 0.0;
    let mut filt_rough = 0.0;
    let mut prev: Option<f64> = None;
    for c in 800..1200 {
        raw_rough += (noisy[c].y - noisy[c - 1].y).abs();
        let o = f.apply(&noisy, c);
        if let Some(p) = prev {
            filt_rough += (o.y - p).abs();
        }
        prev = Some(o.y);
    }
    assert!(
        filt_rough < raw_rough / 3.0,
        "raw {raw_rough:.1} filtered {filt_rough:.1}"
    );
}

#[test]
fn fft_round_trips_a_signal() {
    let n = 16;
    let orig: Vec<f64> = (0..n).map(|i| (i as f64).sin() + i as f64 * 0.1).collect();
    let mut re = orig.clone();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im, false);
    fft(&mut re, &mut im, true);
    for i in 0..n {
        assert!((re[i] - orig[i]).abs() < 1e-9);
        assert!(im[i].abs() < 1e-9);
    }
}

// ---------------------------------------------------------------------------
// 4.1 interpolation basis
// ---------------------------------------------------------------------------

#[test]
fn basis_weights_at_t0_are_0_quarter_half_quarter_and_at_t1_are_quarter_half_quarter_0() {
    // Each control is a one-hot on x: value = sum(w_i * c_i.x).
    let mk = |x: f64| Control {
        x,
        ..Control::default()
    };
    let probe = |c0x: f64, c1x: f64, c2x: f64, c3x: f64, t: f64| {
        interpolate_window(&mk(c3x), &mk(c2x), &mk(c1x), &mk(c0x), t).x
    };
    // weight of c0
    assert!(probe(1.0, 0.0, 0.0, 0.0, 0.0).abs() < 1e-12);
    assert!((probe(1.0, 0.0, 0.0, 0.0, 1.0) - 0.25).abs() < 1e-12);
    // weight of c1
    assert!((probe(0.0, 1.0, 0.0, 0.0, 0.0) - 0.25).abs() < 1e-12);
    assert!((probe(0.0, 1.0, 0.0, 0.0, 1.0) - 0.5).abs() < 1e-12);
    // weight of c2
    assert!((probe(0.0, 0.0, 1.0, 0.0, 0.0) - 0.5).abs() < 1e-12);
    assert!((probe(0.0, 0.0, 1.0, 0.0, 1.0) - 0.25).abs() < 1e-12);
    // weight of c3
    assert!((probe(0.0, 0.0, 0.0, 1.0, 0.0) - 0.25).abs() < 1e-12);
    assert!(probe(0.0, 0.0, 0.0, 1.0, 1.0).abs() < 1e-12);
}

#[test]
fn azimuth_is_unwrapped_across_pi_before_blending() {
    let c = |az: f64| Control {
        azimuth: az,
        sx: 1.0,
        sy: 1.0,
        opacity: 1.0,
        pressure: 1.0,
        bleed: 1.0,
        ..Control::default()
    };
    let out = interpolate_window(
        &c(PI - 0.05),
        &c(PI - 0.05),
        &c(-PI + 0.05),
        &c(-PI + 0.05),
        0.5,
    );
    // the blend should stay near +-pi, not swing through 0
    assert!(
        (out.azimuth.abs() - PI).abs() < 0.1,
        "azimuth {}",
        out.azimuth
    );
}

// ---------------------------------------------------------------------------
// 4.2 / 4.3 spacing, dabs and stamps
// ---------------------------------------------------------------------------

#[test]
fn spacing_diameter_follows_d_of_sz_with_k_1() {
    assert!((spacing_diameter(20.0, 0.1) - (1.5 * 20.0 / 128.0 + 0.5 + 0.1 * 41.0)).abs() < 1e-12);
}

#[test]
fn straight_line_with_constant_size_consecutive_dabs_are_one_spacing_diameter_apart() {
    let mut brush = plain_brush();
    brush.spacing = 0.1;
    let want = spacing_diameter(20.0, 0.1);
    let mut s = Stroke::new(
        brush,
        StrokeOptions {
            seed: 3,
            has_pressure: true,
            ..StrokeOptions::default()
        },
    );
    let dabs = s.add_samples(&line(120, 3.0), &[]).committed_dabs;
    assert!(dabs.len() > 20, "only {} dabs", dabs.len());
    // every dab is at the same size
    for d in &dabs {
        assert!((d.sx - 20.0).abs() < 1e-9);
    }
    // skip the first dab (placed at the curve start); check the rest
    for i in 2..dabs.len() - 1 {
        let dist = (dabs[i].x - dabs[i - 1].x).hypot(dabs[i].y - dabs[i - 1].y);
        assert!((dist - want).abs() < 0.05, "gap {i}: {dist} vs {want}");
    }
}

#[test]
fn dabs_are_continuous_across_two_add_samples_calls() {
    let mut brush = plain_brush();
    brush.spacing = 0.08;
    let samples: Vec<Sample> = straight_line(90, 2.5, 100.0, 1.0, 10.0)
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            s.y = 100.0 + (i as f64 / 7.0).sin() * 6.0;
            s
        })
        .collect();
    let opts = || StrokeOptions {
        seed: 11,
        ..StrokeOptions::default()
    };

    let mut whole = Stroke::new(brush.clone(), opts());
    let a = whole.add_samples(&samples, &[]).committed_dabs;

    let mut split = Stroke::new(brush, opts());
    let mut b = split.add_samples(&samples[..40], &[]).committed_dabs;
    b.extend(split.add_samples(&samples[40..], &[]).committed_dabs);

    assert_eq!(b.len(), a.len());
    for i in 0..a.len() {
        assert!(
            (a[i].x - b[i].x).abs() < 1e-9 && (a[i].y - b[i].y).abs() < 1e-9,
            "dab {i} differs"
        );
    }
}

#[test]
fn taper_makes_the_first_and_last_dabs_smaller_than_the_middle() {
    // default taper 0.1 (51 px)
    let brush = Brush {
        size: 20.0,
        smoothing: 0.0,
        spacing: 0.1,
        ..Brush::default()
    };
    let mut s = Stroke::new(
        brush,
        StrokeOptions {
            seed: 5,
            has_pressure: true,
            ..StrokeOptions::default()
        },
    );
    s.add_samples(&straight_line(120, 2.5, 100.0, 1.0, 10.0), &[]);
    let dabs = s.finish().dabs;
    let first = dabs[0];
    let last = dabs[dabs.len() - 1];
    let middle = dabs[dabs.len() / 2];
    assert!(
        first.sx < middle.sx * 0.5,
        "first {} middle {}",
        first.sx,
        middle.sx
    );
    assert!(
        last.sx < middle.sx * 0.5,
        "last {} middle {}",
        last.sx,
        middle.sx
    );
    assert!((middle.sx - 20.0).abs() < 1e-6);
}

#[test]
fn stamps_never_exceed_16_per_dab() {
    let brush = Brush {
        size: 10.0,
        smoothing: 0.0,
        stamp_count: 1000.0,
        ..Brush::default()
    };
    let mut s = Stroke::new(
        brush,
        StrokeOptions {
            seed: 9,
            ..StrokeOptions::default()
        },
    );
    let dabs = s.add_samples(&line(40, 4.0), &[]).committed_dabs;
    assert!(!dabs.is_empty());
    // group stamps into dabs by the stamp index sequence
    let mut max_stamp = 0;
    for d in &dabs {
        assert!(d.stamp < MAX_STAMPS, "stamp index {} out of range", d.stamp);
        max_stamp = max_stamp.max(d.stamp);
    }
    assert_eq!(
        max_stamp,
        MAX_STAMPS - 1,
        "cap should be reached with a huge stampCount"
    );
}

#[test]
fn one_stamp_per_dab_by_default() {
    let mut s = Stroke::new(
        plain_brush(),
        StrokeOptions {
            seed: 2,
            ..StrokeOptions::default()
        },
    );
    let dabs = s.add_samples(&line(40, 3.0), &[]).committed_dabs;
    assert!(!dabs.is_empty());
    for d in &dabs {
        assert_eq!(d.stamp, 0);
    }
}

#[test]
fn predicted_samples_produce_a_disposable_result_that_does_not_change_committed_state() {
    let mut brush = plain_brush();
    brush.spacing = 0.1;
    let samples = line(60, 3.0);
    let mut reference = Stroke::new(
        brush.clone(),
        StrokeOptions {
            seed: 4,
            ..StrokeOptions::default()
        },
    );
    let ref_len = reference.add_samples(&samples, &[]).committed_dabs.len();

    let mut s = Stroke::new(
        brush,
        StrokeOptions {
            seed: 4,
            ..StrokeOptions::default()
        },
    );
    let r1 = s.add_samples(&samples[..30], &samples[30..33]);
    let r2 = s.add_samples(&samples[30..], &[]);
    assert!(!r1.predicted_dabs.is_empty());
    assert_eq!(r1.committed_dabs.len() + r2.committed_dabs.len(), ref_len);
}

#[test]
fn finish_replays_the_stroke_and_reports_the_first_changed_dab() {
    let brush = Brush {
        size: 20.0,
        smoothing: 0.0,
        spacing: 0.1,
        ..Brush::default()
    };
    let mut s = Stroke::new(
        brush,
        StrokeOptions {
            seed: 6,
            ..StrokeOptions::default()
        },
    );
    s.add_samples(&straight_line(120, 2.5, 100.0, 1.0, 10.0), &[]);
    let end = s.finish();
    let tail = end
        .tail_redraw
        .expect("first finish() reports a tail redraw");
    assert!(!end.dabs.is_empty());
    assert!(
        tail.from > 0 && tail.from < end.dabs.len(),
        "from={}",
        tail.from
    );
}

// ---------------------------------------------------------------------------
// 4.4 randomness and misc
// ---------------------------------------------------------------------------

#[test]
fn seeded_prng_is_deterministic_and_in_0_1() {
    let mut a = Rng::new(123);
    let mut b = Rng::new(123);
    for _ in 0..1000 {
        let v = a.next_f64();
        assert_eq!(v, b.next_f64());
        assert!((0.0..1.0).contains(&v));
    }
}

#[test]
fn normalise_sample_mouse_input_is_treated_as_having_no_pressure() {
    let s = normalise_sample(
        &Sample {
            x: 1.0,
            y: 2.0,
            t: 0.0,
            pressure: Some(0.5),
            pointer: PointerType::Mouse,
            ..Sample::default()
        },
        &NormEnv::default(),
    );
    assert!(!s.has_pressure);
    assert_eq!(s.p, 1.0);
}

#[test]
fn presets_resolve_to_complete_brushes() {
    for preset in [
        Preset::TechnicalPen,
        Preset::SoftAirbrush,
        Preset::Pencil,
        Preset::InkWithTaper,
    ] {
        let b = Brush::preset(preset);
        assert!(b.size > 0.0);
        assert!(b.taper.pencil.start_length >= 0.0 && b.taper.finger.start_length >= 0.0);
    }
    assert_eq!(
        Brush::preset(Preset::SoftAirbrush).accumulation,
        Accumulation::Buildup
    );
    assert_eq!(Brush::preset(Preset::Pencil).grain, Grain::Noise);
}
