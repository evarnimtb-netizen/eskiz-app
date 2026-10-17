//! Procedural grain textures. A grain is a tileable grayscale image: white is
//! where paint catches the surface, black is where it skips. Every grain is
//! deterministic for a given `(kind, size, seed)` and is stretched to the full
//! 0..255 range.

use crate::noise::{fbm, gray_from_unit, hash01, smoothstep, unit, value_noise, worley};

use image::GrayImage;
use std::f64::consts::PI;

/// Default grain resolution in pixels (matches the eskiz-core grain texture size).
pub const GRAIN_SIZE: u32 = 256;

/// Grain families. Each is an original surface treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrainKind {
    /// Layered fractal noise: the tooth of a watercolour or drawing paper.
    PaperTooth,
    /// Over-under thread crossings of a woven canvas.
    CanvasWeave,
    /// Streaky, high-contrast grit that breaks up like compressed charcoal.
    CharcoalNoise,
    /// Cellular pigment pooling along the edges of irregular cells.
    WatercolourBloom,
    /// Mottled fractal surface with small pits.
    Concrete,
    /// Two families of fine diagonal hatch lines, broken up irregularly.
    CrossHatch,
}

impl GrainKind {
    /// Name shown in pickers.
    pub fn label(self) -> &'static str {
        match self {
            GrainKind::PaperTooth => "Paper tooth",
            GrainKind::CanvasWeave => "Canvas",
            GrainKind::CharcoalNoise => "Charcoal",
            GrainKind::WatercolourBloom => "Bloom",
            GrainKind::Concrete => "Concrete",
            GrainKind::CrossHatch => "Crosshatch",
        }
    }

    /// Stable name used in brush files.
    pub fn key(self) -> &'static str {
        match self {
            GrainKind::PaperTooth => "paper_tooth",
            GrainKind::CanvasWeave => "canvas_weave",
            GrainKind::CharcoalNoise => "charcoal_noise",
            GrainKind::WatercolourBloom => "watercolour_bloom",
            GrainKind::Concrete => "concrete",
            GrainKind::CrossHatch => "cross_hatch",
        }
    }

    /// The kind whose [`key`](Self::key) is `key`.
    pub fn from_key(key: &str) -> Option<GrainKind> {
        GrainKind::ALL.into_iter().find(|k| k.key() == key)
    }

    /// Every grain kind, in declaration order.
    pub const ALL: [GrainKind; 6] = [
        GrainKind::PaperTooth,
        GrainKind::CanvasWeave,
        GrainKind::CharcoalNoise,
        GrainKind::WatercolourBloom,
        GrainKind::Concrete,
        GrainKind::CrossHatch,
    ];
}

/// Generate a tileable grain of `size x size` pixels.
pub fn grain(kind: GrainKind, size: u32, seed: u64) -> GrayImage {
    let n = size.max(8);
    let values: Vec<f64> = (0..n * n)
        .map(|i| {
            let (x, y) = (i % n, i / n);
            sample(kind, unit(n, x), unit(n, y), seed)
        })
        .collect();
    stretch(n, &values)
}

/// Raw grain value at unit-square position `(u, v)`, before stretching.
fn sample(kind: GrainKind, u: f64, v: f64, seed: u64) -> f64 {
    match kind {
        GrainKind::PaperTooth => fbm(u, v, (8, 8), 5, seed),
        GrainKind::CanvasWeave => weave(u, v, seed),
        GrainKind::CharcoalNoise => {
            // Anisotropic fBm (stretched along x) gives horizontal streaks; a fine
            // grit layer and a steep smoothstep make the contrast harsh.
            let streak = fbm(u, v, (3, 40), 4, seed);
            let grit = value_noise(u, v, (128, 128), seed ^ 0x51);
            smoothstep(0.38, 0.66, 0.7 * streak + 0.3 * grit)
        }
        GrainKind::WatercolourBloom => {
            let (f1, f2) = worley(u, v, (9, 9), seed);
            // Wide, gentle ramp so pigment pools softly instead of drawing cracks.
            let pool = 1.0 - smoothstep(0.0, 0.6, f2 - f1);
            let blotch = fbm(u, v, (3, 3), 4, seed ^ 0x33);
            0.35 * pool + 0.65 * blotch
        }
        GrainKind::Concrete => {
            let base = fbm(u, v, (16, 16), 5, seed);
            let (f1, _) = worley(u, v, (20, 20), seed ^ 0x77);
            let pit = smoothstep(0.05, 0.3, f1);
            (0.8 * base + 0.2) * (0.5 + 0.5 * pit)
        }
        GrainKind::CrossHatch => {
            // Integer frequency along both diagonals keeps the pattern periodic.
            let k = 22.0;
            let s1 = (PI * k * (u + v)).cos().abs();
            let s2 = (PI * k * (u - v)).cos().abs();
            let line = smoothstep(0.88, 0.995, s1).max(smoothstep(0.88, 0.995, s2));
            let breakup = smoothstep(0.25, 0.75, value_noise(u, v, (48, 48), seed));
            line * breakup
        }
    }
}

/// Over-under weave: threads alternate between horizontal and vertical on a
/// checkerboard of crossings, each with a rounded cross-section and a slight
/// per-thread variation.
fn weave(u: f64, v: f64, seed: u64) -> f64 {
    const THREADS: f64 = 24.0;
    let (fu, fv) = (u * THREADS, v * THREADS);
    let (iu, iv) = (fu.floor(), fv.floor());
    let (pu, pv) = (fu - iu, fv - iv);
    let (cu, cv) = (iu as i64, iv as i64);
    let horizontal_on_top = (cu + cv).rem_euclid(2) == 0;
    let profile = if horizontal_on_top {
        (PI * pv).sin().sqrt()
    } else {
        (PI * pu).sin().sqrt()
    };
    let per_thread = 0.9 + 0.1 * hash01(cu, cv, seed);
    let fibre = value_noise(u, v, (64, 64), seed ^ 0x1F);
    profile * per_thread * 0.9 + 0.1 * fibre
}

/// Stretch `values` linearly so the minimum maps to 0 and the maximum to 1. A
/// constant field maps to mid-grey.
fn stretch(n: u32, values: &[f64]) -> GrayImage {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = hi - lo;
    if span <= f64::EPSILON {
        return gray_from_unit(n, &vec![0.5; values.len()]);
    }
    let out: Vec<f64> = values.iter().map(|v| (v - lo) / span).collect();
    gray_from_unit(n, &out)
}
