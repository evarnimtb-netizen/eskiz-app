//! Shared helpers for the integration tests (mirrors the JS test helpers).
#![allow(dead_code)]

use eskiz_core::brushes::Brush;
use eskiz_core::geometry::{normalise_sample, NormEnv, NormSample, PointerType, Sample};

/// Pen sample with pressure `pressure` at `(x, y)` and time `t`.
pub fn pen(x: f64, y: f64, t: f64, pressure: f64) -> Sample {
    Sample {
        x,
        y,
        t,
        pressure: Some(pressure),
        pointer: PointerType::Pen,
        ..Sample::default()
    }
}

/// JS `straightLine(n, { step, y, pressure, x0 })` at 120 Hz.
pub fn straight_line(n: usize, step: f64, y: f64, pressure: f64, x0: f64) -> Vec<Sample> {
    (0..n)
        .map(|i| pen(x0 + i as f64 * step, y, i as f64 / 120.0, pressure))
        .collect()
}

/// JS `straightLine(n, { step })` with the default y = 100, pressure = 1, x0 = 10.
pub fn line(n: usize, step: f64) -> Vec<Sample> {
    straight_line(n, step, 100.0, 1.0, 10.0)
}

/// Normalise raw samples with default settings.
pub fn norm(samples: &[Sample]) -> Vec<NormSample> {
    samples
        .iter()
        .map(|s| normalise_sample(s, &NormEnv::default()))
        .collect()
}

/// JS `plainBrush()`: size 20, no smoothing, no taper.
pub fn plain_brush() -> Brush {
    let mut b = Brush {
        size: 20.0,
        smoothing: 0.0,
        ..Brush::default()
    };
    b.taper.pencil.start_length = 0.0;
    b.taper.pencil.end_length = 0.0;
    b.taper.finger.start_length = 0.0;
    b.taper.finger.end_length = 0.0;
    b
}
