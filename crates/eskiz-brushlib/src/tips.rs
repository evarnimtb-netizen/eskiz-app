//! Procedural brush tips. A tip is a grayscale alpha mask where white (255) is
//! paint and black is empty. Every tip is:
//!
//! - deterministic for a given `(kind, size, seed)`;
//! - normalised so its brightest pixel is exactly 255;
//! - centred, so the bounding box of its painted area is centred in the image.

use crate::noise::{fbm, gray_from_unit, smoothstep, unit, value_noise, Rng};
use image::GrayImage;
use std::f64::consts::{PI, TAU};

/// Default tip resolution in pixels (matches the eskiz-core shape texture size).
pub const TIP_SIZE: u32 = 128;

/// Tip families. Each is a distinct, original silhouette or edge treatment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipKind {
    /// Crisp anti-aliased disc.
    RoundHard,
    /// Disc with a smooth falloff to zero at the rim.
    RoundSoft,
    /// Square with a crisp edge.
    Square,
    /// Round tip whose rim is broken and speckled, like chalk on paper.
    ChalkEdge,
    /// Many small dots scattered inside a round footprint.
    Bristle,
    /// A dense core with a loose, heavy-tailed scatter of drops.
    Splatter,
    /// Pointed lens (leaf) with a slight bow along its midrib.
    Leaf,
    /// Chisel-edged flat with rounded corners and fine streaks along its length.
    Flat,
    /// Round tip with a noisy rim and a mottled interior.
    TexturedRound,
    /// Fine spray: many tiny dots with a centre-heavy distribution.
    SprayDots,
}

impl TipKind {
    /// Name shown in pickers.
    pub fn label(self) -> &'static str {
        match self {
            TipKind::RoundHard => "Hard round",
            TipKind::RoundSoft => "Soft round",
            TipKind::Square => "Square",
            TipKind::ChalkEdge => "Chalk",
            TipKind::Bristle => "Bristles",
            TipKind::Splatter => "Splatter",
            TipKind::Leaf => "Leaf",
            TipKind::Flat => "Flat",
            TipKind::TexturedRound => "Textured",
            TipKind::SprayDots => "Spray",
        }
    }

    /// Stable name used in brush files.
    pub fn key(self) -> &'static str {
        match self {
            TipKind::RoundHard => "round_hard",
            TipKind::RoundSoft => "round_soft",
            TipKind::Square => "square",
            TipKind::ChalkEdge => "chalk_edge",
            TipKind::Bristle => "bristle",
            TipKind::Splatter => "splatter",
            TipKind::Leaf => "leaf",
            TipKind::Flat => "flat",
            TipKind::TexturedRound => "textured_round",
            TipKind::SprayDots => "spray_dots",
        }
    }

    /// The kind whose [`key`](Self::key) is `key`.
    pub fn from_key(key: &str) -> Option<TipKind> {
        TipKind::ALL.into_iter().find(|k| k.key() == key)
    }

    /// Every tip kind, in declaration order.
    pub const ALL: [TipKind; 10] = [
        TipKind::RoundHard,
        TipKind::RoundSoft,
        TipKind::Square,
        TipKind::ChalkEdge,
        TipKind::Bristle,
        TipKind::Splatter,
        TipKind::Leaf,
        TipKind::Flat,
        TipKind::TexturedRound,
        TipKind::SprayDots,
    ];
}

/// Anti-aliased coverage of a half-plane edge at signed distance `d` (px, inside
/// negative): 1 well inside, 0 well outside, a one-pixel ramp at the boundary.
/// Width of the anti-aliased rim, in tip texels. Wider than one texel so that
/// small, rotated dabs do not show stair-stepped edges.
const AA_WIDTH: f64 = 3.0;

fn aa(d: f64) -> f64 {
    (0.5 - d / AA_WIDTH).clamp(0.0, 1.0)
}

/// Fill `buf` (row-major `n x n`) with `f(dx, dy, u, v)`, where `(dx, dy)` is the
/// offset of the pixel centre from the tip centre in pixels and `(u, v)` is the
/// unit-square position.
fn fill(buf: &mut [f64], n: u32, mut f: impl FnMut(f64, f64, f64, f64) -> f64) {
    let c = f64::from(n) / 2.0;
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let v = f(px - c, py - c, unit(n, x), unit(n, y));
            buf[(y * n + x) as usize] = v;
        }
    }
}

/// Max-combine a soft-edged dot of radius `rad` (px) and brightness `value` at
/// `(cx, cy)` into `buf`.
fn stamp(buf: &mut [f64], n: u32, (cx, cy): (f64, f64), rad: f64, value: f64) {
    let ni = i64::from(n);
    let lo_x = ((cx - rad - 1.0).floor() as i64).max(0);
    let hi_x = ((cx + rad + 1.0).ceil() as i64).min(ni - 1);
    let lo_y = ((cy - rad - 1.0).floor() as i64).max(0);
    let hi_y = ((cy + rad + 1.0).ceil() as i64).min(ni - 1);
    for y in lo_y..=hi_y {
        for x in lo_x..=hi_x {
            let d = (x as f64 + 0.5 - cx).hypot(y as f64 + 0.5 - cy);
            let a = (rad - d + 0.5).clamp(0.0, 1.0) * value;
            let idx = (y * ni + x) as usize;
            buf[idx] = buf[idx].max(a);
        }
    }
}

/// Generate a tip of `size x size` pixels from `kind`, seeded by `seed`.
pub fn tip(kind: TipKind, size: u32, seed: u64) -> GrayImage {
    let n = size.max(16);
    let r = f64::from(n) * 0.46;
    let mut buf = vec![0.0f64; (n * n) as usize];
    let mut rng = Rng::new(seed);
    match kind {
        TipKind::RoundHard => fill(&mut buf, n, |dx, dy, _, _| aa(dx.hypot(dy) - r)),
        TipKind::RoundSoft => fill(&mut buf, n, |dx, dy, _, _| {
            1.0 - smoothstep(0.15, 1.0, dx.hypot(dy) / r)
        }),
        TipKind::Square => fill(&mut buf, n, |dx, dy, _, _| {
            let h = 0.84 * r;
            aa(dx.abs() - h) * aa(dy.abs() - h)
        }),
        TipKind::ChalkEdge => fill(&mut buf, n, |dx, dy, u, v| {
            let e = fbm(u, v, (5, 5), 4, seed);
            let edge = r * (0.78 + 0.24 * e);
            let speckle = 0.75 + 0.25 * value_noise(u, v, (64, 64), seed ^ 0x51);
            aa(dx.hypot(dy) - edge) * speckle
        }),
        TipKind::TexturedRound => fill(&mut buf, n, |dx, dy, u, v| {
            let e = fbm(u, v, (7, 7), 3, seed);
            let edge = r * (0.82 + 0.2 * e);
            let mottle = 0.8 + 0.2 * value_noise(u, v, (40, 40), seed ^ 0x77);
            aa(dx.hypot(dy) - edge) * mottle
        }),
        TipKind::Leaf => fill(&mut buf, n, |dx, dy, _, _| {
            let t = dy / r;
            if t.abs() >= 1.0 {
                return 0.0;
            }
            // Half-width vanishes at both tips, giving a pointed outline.
            let half = 0.42 * r * (PI * (t + 1.0) / 2.0).sin().powf(0.9);
            let x = dx - 0.12 * r * t * t;
            aa(x.abs() - half)
        }),
        TipKind::Flat => fill(&mut buf, n, |dx, dy, u, v| {
            let (hw, hh, rr) = (0.30 * r, 0.92 * r, 0.12 * r);
            let qx = dx.abs() - (hw - rr);
            let qy = dy.abs() - (hh - rr);
            let sd = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rr;
            let streak = 0.97 + 0.03 * value_noise(u, v, (3, 96), seed ^ 0x33);
            aa(sd) * streak
        }),
        TipKind::Bristle => {
            for _ in 0..110 {
                let a = TAU * rng.next_f64();
                let rr = 0.9 * r * rng.next_f64().sqrt();
                let c = f64::from(n) / 2.0;
                let rad = 0.9 + 1.4 * rng.next_f64();
                let value = 0.5 + 0.5 * rng.next_f64();
                stamp(
                    &mut buf,
                    n,
                    (c + rr * a.cos(), c + rr * a.sin()),
                    rad,
                    value,
                );
            }
        }
        TipKind::Splatter => {
            let core = r * 0.22;
            fill(&mut buf, n, |dx, dy, _, _| aa(dx.hypot(dy) - core));
            let c = f64::from(n) / 2.0;
            for _ in 0..380 {
                let rr = (rng.gauss().abs() * 0.42 * r).min(r * 0.96);
                let a = TAU * rng.next_f64();
                let rad = 0.5 + 3.2 * rng.next_f64().powi(3);
                let value = 0.7 + 0.3 * rng.next_f64();
                stamp(
                    &mut buf,
                    n,
                    (c + rr * a.cos(), c + rr * a.sin()),
                    rad,
                    value,
                );
            }
        }
        TipKind::SprayDots => {
            let c = f64::from(n) / 2.0;
            for _ in 0..2400 {
                let rr = (rng.gauss().abs() * 0.30 * r).min(r * 0.95);
                let a = TAU * rng.next_f64();
                let rad = 0.45 + 0.7 * rng.next_f64();
                let value = 0.5 + 0.5 * rng.next_f64();
                stamp(
                    &mut buf,
                    n,
                    (c + rr * a.cos(), c + rr * a.sin()),
                    rad,
                    value,
                );
            }
        }
    }
    finish(&buf, n)
}

/// Normalise `buf` so its maximum is 1, translate the painted bounding box to the
/// centre, and quantise to 8 bits.
fn finish(buf: &[f64], n: u32) -> GrayImage {
    let ni = i64::from(n);
    let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    let mut max = 0.0f64;
    for y in 0..ni {
        for x in 0..ni {
            let v = buf[(y * ni + x) as usize];
            if v > 0.0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
                max = max.max(v);
            }
        }
    }
    if max <= 0.0 {
        return GrayImage::new(n, n);
    }

    let target = (f64::from(n) - 1.0) / 2.0;
    let shift_x = (target - (x0 + x1) as f64 / 2.0).round() as i64;
    let shift_y = (target - (y0 + y1) as f64 / 2.0).round() as i64;
    let mut out = vec![0.0f64; (n * n) as usize];
    for y in 0..ni {
        for x in 0..ni {
            let (sx, sy) = (x - shift_x, y - shift_y);
            if (0..ni).contains(&sx) && (0..ni).contains(&sy) {
                out[(y * ni + x) as usize] = buf[(sy * ni + sx) as usize] / max;
            }
        }
    }
    gray_from_unit(n, &out)
}
