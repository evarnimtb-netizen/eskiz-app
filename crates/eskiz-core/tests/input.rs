//! Input handling tests: 7 cases with fixed tolerances.

use eskiz_core::brushes::Brush;
use eskiz_core::geometry::{
    dynamics_step, normalise_sample, Cfg, DynState, NormEnv, NormSample, Sample,
    NO_PRESSURE_MIN_FACTOR,
};
use eskiz_core::PointerType;

fn cfg_for(brush: Brush) -> Cfg {
    Cfg::new(brush, 120.0, 1.0, 1.0)
}

fn input(pointer: PointerType, pressure: Option<f64>, has_pressure: Option<bool>) -> Sample {
    Sample {
        x: 1.0,
        y: 2.0,
        t: 0.0,
        pressure,
        pointer,
        has_pressure,
        ..Sample::default()
    }
}

#[test]
fn mouse_with_simulated_pressure_0_5_is_treated_as_having_pressure() {
    let s = normalise_sample(
        &input(PointerType::Mouse, Some(0.5), Some(true)),
        &NormEnv::default(),
    );
    assert!(s.has_pressure);
    assert_eq!(s.p, 0.5);
}

#[test]
fn mouse_without_simulated_pressure_stays_a_no_pressure_sample() {
    let s = normalise_sample(
        &input(PointerType::Mouse, Some(0.5), Some(false)),
        &NormEnv::default(),
    );
    assert!(!s.has_pressure);
    assert_eq!(s.p, 1.0);
}

#[test]
fn pen_samples_keep_their_pressure_when_has_pressure_is_declared() {
    let s = normalise_sample(
        &Sample {
            x: 0.0,
            y: 0.0,
            ..input(PointerType::Pen, Some(0.3), Some(true))
        },
        &NormEnv::default(),
    );
    assert!(s.has_pressure);
    assert_eq!(s.p, 0.3);
}

#[test]
fn a_declared_has_pressure_of_true_still_needs_a_finite_pressure() {
    let s = normalise_sample(
        &input(PointerType::Pen, None, Some(true)),
        &NormEnv::default(),
    );
    assert!(!s.has_pressure);
}

#[test]
fn no_pressure_opacity_is_floored_not_collapsed_to_0_pressure_opacity_1() {
    let cfg = cfg_for(Brush {
        pressure_opacity: 1.0,
        opacity: 1.0,
        ..Brush::default()
    });
    let mut st = DynState::default();
    let sample = NormSample {
        x: 0.0,
        y: 0.0,
        t: 0.0,
        p: 1.0,
        has_pressure: false,
        altitude_norm: 1.0,
        azimuth: 0.0,
        roll: 0.0,
        predicted: false,
    };
    let c = dynamics_step(&mut st, &sample, &cfg, None, 0.0);
    assert!(
        (c.opacity - NO_PRESSURE_MIN_FACTOR).abs() < 1e-9,
        "opacity {}",
        c.opacity
    );
}

fn pressure_sample(x: f64, t: f64, p: f64) -> NormSample {
    NormSample {
        x,
        y: 0.0,
        t,
        p,
        has_pressure: true,
        altitude_norm: 1.0,
        azimuth: 0.0,
        roll: 0.0,
        predicted: false,
    }
}

#[test]
fn pressure_softening_lerps_previous_and_current_with_clamped_w() {
    let cfg = cfg_for(Brush {
        pressure_softening: true,
        ..Brush::default()
    });
    let mut st = DynState::default();
    dynamics_step(&mut st, &pressure_sample(0.0, 0.0, 0.2), &cfg, None, 0.0);
    // segLen = 1 px, dt = (1/120) * 60 = 0.5, so w = clamp(0.5, 0.2, 1) = 0.5
    let c = dynamics_step(
        &mut st,
        &pressure_sample(1.0, 1.0 / 120.0, 0.8),
        &cfg,
        None,
        0.0,
    );
    assert!((c.pressure - 0.5).abs() < 1e-9, "pressure {}", c.pressure);
}

#[test]
fn pressure_softening_with_a_long_segment_keeps_the_current_value_w_1() {
    let cfg = cfg_for(Brush {
        pressure_softening: true,
        ..Brush::default()
    });
    let mut st = DynState::default();
    dynamics_step(&mut st, &pressure_sample(0.0, 0.0, 0.2), &cfg, None, 0.0);
    // segLen = 1 px but dt = 1.0 frames -> w = clamp(1/1, 0.2, 1) = 1 -> current value
    let c = dynamics_step(
        &mut st,
        &pressure_sample(1.0, 1.0 / 60.0, 0.8),
        &cfg,
        None,
        0.0,
    );
    assert!((c.pressure - 0.8).abs() < 1e-9, "pressure {}", c.pressure);
}
