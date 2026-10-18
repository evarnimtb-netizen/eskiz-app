//! Turns raw pointer data into `eskiz_core::Sample`s.
//!
//! The same mapping is used by the window (egui pointer and touch events) and by
//! `--selftest`, so both go through one code path.

use eskiz_core::geometry::{PointerType, Sample};

use crate::pressure::PressureCurve;

/// Simulated pressure for mouse input. Eskiz uses 0.5 when
/// "simulate pressure for mouse" is on, which is the default.
pub const MOUSE_PRESSURE: f64 = 0.5;

/// Where a sample came from, as far as the window can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    Mouse,
    /// Touch or stylus. egui reports stylus pressure as touch force.
    Touch,
}

/// What a pen tablet reports for one sample. `None` fields mean "the device has no such axis".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenReading {
    /// Raw pressure in 0..1, before the global pressure curve.
    pub pressure: Option<f64>,
    /// Altitude in radians (pi/2 = perpendicular to the screen).
    pub altitude: Option<f64>,
    /// Azimuth in radians.
    pub azimuth: Option<f64>,
    /// Barrel rotation in radians, when the device reports it.
    pub roll: Option<f64>,
}

/// One raw pointer reading, with the position already in canvas pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawPointer {
    pub x: f64,
    pub y: f64,
    /// Seconds.
    pub time: f64,
    pub kind: PointerKind,
    /// Touch force in 0..1, when the device reports one.
    pub force: Option<f64>,
}

/// Convert a raw reading to a sample.
///
/// * Mouse: pressure 0.5 with `has_pressure` set (simulated, as in the JS app).
/// * Touch with a force reading: that force is the pressure, and the sample is treated
///   as a stylus (pen taper). A force of 0 or NaN means "not measured" in egui, so it
///   falls through to the finger case.
/// * Touch without force: a finger with no pressure sensor (`has_pressure` false).
pub fn sample_from(raw: RawPointer) -> Sample {
    let base = Sample {
        x: raw.x,
        y: raw.y,
        t: raw.time,
        ..Sample::default()
    };
    match raw.kind {
        PointerKind::Mouse => Sample {
            pressure: Some(MOUSE_PRESSURE),
            has_pressure: Some(true),
            pointer: PointerType::Mouse,
            ..base
        },
        PointerKind::Touch => match raw.force.filter(|f| f.is_finite() && *f > 0.0) {
            Some(force) => Sample {
                pressure: Some(force.clamp(0.0, 1.0)),
                has_pressure: Some(true),
                pointer: PointerType::Pen,
                ..base
            },
            None => Sample {
                pressure: None,
                has_pressure: Some(false),
                pointer: PointerType::Touch,
                ..base
            },
        },
    }
}

/// Convert a pen tablet reading (position in canvas pixels, time in seconds) to a sample.
/// Pressure, altitude and azimuth come straight from the device; an axis the device lacks
/// stays `None`. A pen without a pressure axis has `has_pressure` false.
pub fn sample_from_pen(x: f64, y: f64, time: f64, pen: PenReading) -> Sample {
    let pressure = pen.pressure.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0));
    Sample {
        x,
        y,
        t: time,
        pressure,
        has_pressure: Some(pressure.is_some()),
        tilt: pen.altitude.filter(|a| a.is_finite()),
        azimuth: pen.azimuth.filter(|a| a.is_finite()),
        roll: pen.roll.filter(|r| r.is_finite()).unwrap_or(0.0),
        pointer: PointerType::Pen,
        ..Sample::default()
    }
}

/// Apply the global pressure curve to a sample's measured pressure (touch force and pen
/// pressure). Simulated mouse pressure and samples without pressure are left alone.
pub fn apply_pressure_curve(mut s: Sample, curve: &PressureCurve) -> Sample {
    if s.pointer != PointerType::Mouse && s.has_pressure == Some(true) && !curve.is_identity() {
        s.pressure = s.pressure.map(|p| f64::from(curve.eval(p as f32)));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use eskiz_core::geometry::{normalise_sample, NormEnv};

    fn raw(kind: PointerKind, force: Option<f64>) -> RawPointer {
        RawPointer {
            x: 10.0,
            y: 20.0,
            time: 1.5,
            kind,
            force,
        }
    }

    #[test]
    fn position_and_time_carry_over() {
        let s = sample_from(raw(PointerKind::Mouse, None));
        assert_eq!((s.x, s.y, s.t), (10.0, 20.0, 1.5));
        assert!(!s.predicted);
    }

    #[test]
    fn mouse_gets_simulated_pressure() {
        let s = sample_from(raw(PointerKind::Mouse, None));
        assert_eq!(s.pointer, PointerType::Mouse);
        assert_eq!(s.has_pressure, Some(true));
        let n = normalise_sample(&s, &NormEnv::default());
        assert!(n.has_pressure);
        assert!((n.p - MOUSE_PRESSURE).abs() < 1e-12);
    }

    #[test]
    fn touch_force_becomes_pressure() {
        let s = sample_from(raw(PointerKind::Touch, Some(0.8)));
        assert_eq!(s.pointer, PointerType::Pen);
        let n = normalise_sample(&s, &NormEnv::default());
        assert!(n.has_pressure);
        assert!((n.p - 0.8).abs() < 1e-12);
    }

    #[test]
    fn force_is_clamped_to_unit_range() {
        let s = sample_from(raw(PointerKind::Touch, Some(3.0)));
        assert_eq!(s.pressure, Some(1.0));
    }

    #[test]
    fn zero_or_nan_force_means_finger() {
        for force in [Some(0.0), Some(f64::NAN), None] {
            let s = sample_from(raw(PointerKind::Touch, force));
            assert_eq!(s.pointer, PointerType::Touch);
            assert_eq!(s.has_pressure, Some(false));
            assert_eq!(s.pressure, None);
            let n = normalise_sample(&s, &NormEnv::default());
            assert!(!n.has_pressure);
            assert_eq!(n.p, 1.0);
        }
    }

    #[test]
    fn pen_reading_fills_pressure_tilt_and_azimuth() {
        let pen = PenReading { pressure: Some(0.25), altitude: Some(0.9), azimuth: Some(2.0), roll: Some(0.5) };
        let s = sample_from_pen(3.0, 4.0, 0.5, pen);
        assert_eq!(s.pointer, PointerType::Pen);
        assert_eq!(s.has_pressure, Some(true));
        assert_eq!((s.x, s.y, s.t), (3.0, 4.0, 0.5));
        assert_eq!((s.pressure, s.tilt, s.azimuth), (Some(0.25), Some(0.9), Some(2.0)));
        assert_eq!(s.roll, 0.5);
        let n = normalise_sample(&s, &NormEnv::default());
        assert!(n.has_pressure && (n.p - 0.25).abs() < 1e-12);
    }

    #[test]
    fn pen_without_pressure_axis_has_no_pressure() {
        let s = sample_from_pen(0.0, 0.0, 0.0, PenReading::default());
        assert_eq!(s.has_pressure, Some(false));
        assert_eq!((s.pressure, s.tilt, s.azimuth), (None, None, None));
        let s = sample_from_pen(0.0, 0.0, 0.0, PenReading { pressure: Some(9.0), ..PenReading::default() });
        assert_eq!(s.pressure, Some(1.0));
    }

    #[test]
    fn curve_applies_to_pen_and_touch_but_not_mouse() {
        let mut curve = PressureCurve::default();
        curve.set_point(0, 0.2, 0.6);
        curve.set_point(1, 0.7, 0.9);
        let pen = sample_from_pen(0.0, 0.0, 0.0, PenReading { pressure: Some(0.2), ..PenReading::default() });
        let curved = apply_pressure_curve(pen, &curve).pressure.unwrap();
        assert!((curved - 0.6).abs() < 1e-4, "{curved}");
        let touch = apply_pressure_curve(sample_from(raw(PointerKind::Touch, Some(0.2))), &curve);
        assert!((touch.pressure.unwrap() - 0.6).abs() < 1e-4);
        let mouse = apply_pressure_curve(sample_from(raw(PointerKind::Mouse, None)), &curve);
        assert_eq!(mouse.pressure, Some(MOUSE_PRESSURE));
        let finger = apply_pressure_curve(sample_from(raw(PointerKind::Touch, None)), &curve);
        assert_eq!(finger.pressure, None);
        assert_eq!(apply_pressure_curve(pen, &PressureCurve::default()).pressure, Some(0.2));
    }
}
