//! Deterministic noise primitives for the procedural textures.
//!
//! Everything here is defined on the unit square and is periodic in both axes:
//! lattice indices wrap modulo the cell count, so any texture built from these
//! functions tiles seamlessly. No external noise crate is used.

use image::{GrayImage, Luma};
use std::f64::consts::TAU;

/// SplitMix64 finaliser: a cheap, well-mixed 64-bit bijection.
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform value in `[0, 1)` for lattice point `(x, y)` under `seed`.
pub fn hash01(x: i64, y: i64, seed: u64) -> f64 {
    let h = mix64(seed ^ mix64((x as u64) ^ mix64((y as u64) ^ 0x243F_6A88_85A3_08D3)));
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Hermite smoothstep, clamped to `[0, 1]`.
pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Centre of pixel `i` along an axis of `n` pixels, mapped to `(0, 1)`.
pub fn unit(n: u32, i: u32) -> f64 {
    (f64::from(i) + 0.5) / f64::from(n)
}

/// Periodic value noise in `[0, 1]` with `cells.0 x cells.1` lattice cells
/// across the unit square.
pub fn value_noise(u: f64, v: f64, cells: (u32, u32), seed: u64) -> f64 {
    let (cx, cy) = (i64::from(cells.0), i64::from(cells.1));
    let x = u * cx as f64;
    let y = v * cy as f64;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (fade(x - x0), fade(y - y0));
    let (ix, iy) = (x0 as i64, y0 as i64);
    let corner =
        |dx: i64, dy: i64| hash01((ix + dx).rem_euclid(cx), (iy + dy).rem_euclid(cy), seed);
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * fx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * fx;
    top + (bottom - top) * fy
}

/// Fractal (fBm) sum of value noise: `octaves` layers, each at double the
/// frequency and half the amplitude of the last. Result is in `[0, 1]`.
pub fn fbm(u: f64, v: f64, base: (u32, u32), octaves: u32, seed: u64) -> f64 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let mut cells = base;
    for o in 0..octaves {
        let layer_seed = seed.wrapping_add(u64::from(o).wrapping_mul(0x9E37_79B9));
        sum += amp * value_noise(u, v, cells, layer_seed);
        norm += amp;
        amp *= 0.5;
        cells = (cells.0 * 2, cells.1 * 2);
    }
    sum / norm
}

/// Cellular (Worley) noise on a periodic grid of `cells` feature points.
/// Returns the distances to the nearest and second-nearest feature point, in
/// cell units. `f2 - f1` is small along the boundaries between cells.
pub fn worley(u: f64, v: f64, cells: (u32, u32), seed: u64) -> (f64, f64) {
    let (cx, cy) = (i64::from(cells.0), i64::from(cells.1));
    let x = u * cx as f64;
    let y = v * cy as f64;
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    let (mut f1, mut f2) = (f64::MAX, f64::MAX);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (gx, gy) = (ix + dx, iy + dy);
            let (wx, wy) = (gx.rem_euclid(cx), gy.rem_euclid(cy));
            let px = gx as f64 + hash01(wx, wy, seed);
            let py = gy as f64 + hash01(wx, wy, seed ^ 0xA5A5_5A5A_1234_5678);
            let d = (px - x).hypot(py - y);
            if d < f1 {
                f2 = f1;
                f1 = d;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2)
}

/// Small deterministic generator for scattering dots when building tips.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self {
            state: mix64(seed ^ 0x9E37_79B9_7F4A_7C15),
        }
    }

    /// Uniform value in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        (mix64(self.state) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal sample (Box-Muller).
    pub fn gauss(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-12);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

/// Build a square grayscale image from `n * n` values in row-major order.
/// Values are already in `[0, 1]`.
pub fn gray_from_unit(n: u32, values: &[f64]) -> GrayImage {
    GrayImage::from_fn(n, n, |x, y| {
        let v = values[(y * n + x) as usize].clamp(0.0, 1.0);
        Luma([(v * 255.0).round() as u8])
    })
}
