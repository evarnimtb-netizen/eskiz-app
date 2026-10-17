//! Motion blur: a uniform box average of taps along a straight step.
//!
//! The step and tap count are implemented as described. Ordinary vectors give a very long streak, so
//! the extent has not been confirmed against output.

use crate::image::{mix_masked, Image};

/// Each motion-vector component is clamped to this range.
pub const VECTOR_LIMIT: f32 = 512.0;

/// Parameters of the motion blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionParams {
    /// Motion vector in pixels. Components are in [-512, 512]. Default (0, 0).
    pub vector: [f32; 2],
    /// Amount in [0, 1]. Default 0.6.
    pub amount: f32,
}

impl Default for MotionParams {
    fn default() -> Self {
        Self {
            vector: [0.0, 0.0],
            amount: 0.6,
        }
    }
}

/// Tap count `max(1, floor(256 a p))`. `p` is 1 for the commit.
pub fn tap_count(amount: f32, p: f32) -> usize {
    ((256.0 * amount.clamp(0.0, 1.0) * p).floor() as usize).max(1)
}

/// Normalised per-tap step `(6 a v_x / W, 6 a v_y / H)`.
pub fn step(vector: [f32; 2], amount: f32, width: usize, height: usize) -> [f32; 2] {
    let a = amount.clamp(0.0, 1.0);
    let vx = vector[0].clamp(-VECTOR_LIMIT, VECTOR_LIMIT);
    let vy = vector[1].clamp(-VECTOR_LIMIT, VECTOR_LIMIT);
    [6.0 * a * vx / width as f32, 6.0 * a * vy / height as f32]
}

/// Tap offsets `k - N/2` for `k` in `0..N`. Their mean is `-1/2`, so the
/// taps sit half a step before the pixel.
pub fn tap_offsets(n: usize) -> Vec<f32> {
    (0..n).map(|k| k as f32 - n as f32 / 2.0).collect()
}

/// Preview tap factor `min(1, 1 / (3a))`.
pub fn preview_factor(amount: f32) -> f32 {
    let a = amount.clamp(0.0, 1.0);
    if a <= 0.0 {
        1.0
    } else {
        (1.0 / (3.0 * a)).min(1.0)
    }
}

/// Blur with an explicit tap factor `p`. Shared by the commit (`p = 1`) and the preview.
pub fn apply_with_factor(
    src: &Image,
    mask: Option<&[f32]>,
    params: &MotionParams,
    p: f32,
) -> Image {
    let st = step(params.vector, params.amount, src.width, src.height);
    if st == [0.0, 0.0] {
        // A zero vector returns the source.
        return mix_masked(src, src, mask);
    }
    let n = tap_count(params.amount, p);
    let offsets = tap_offsets(n);
    let inv = 1.0 / n as f32;
    let (w, h) = (src.width as f32, src.height as f32);
    let blurred = Image::generate(src.width, src.height, |x, y| {
        let u = (x as f32 + 0.5) / w;
        let v = (y as f32 + 0.5) / h;
        let mut acc = [0.0f32; 4];
        for &o in &offsets {
            let s = src.sample_uv(u + o * st[0], v + o * st[1]);
            for (a, sv) in acc.iter_mut().zip(s) {
                *a += sv;
            }
        }
        acc.map(|a| a * inv)
    });
    mix_masked(src, &blurred, mask)
}

/// Commit pass of the motion blur (`p = 1`).
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &MotionParams) -> Image {
    apply_with_factor(src, mask, params, 1.0)
}

/// Live-preview pass, which uses the reduced tap count `p = min(1, 1 / (3a))`.
pub fn preview(src: &Image, mask: Option<&[f32]>, params: &MotionParams) -> Image {
    apply_with_factor(src, mask, params, preview_factor(params.amount))
}
