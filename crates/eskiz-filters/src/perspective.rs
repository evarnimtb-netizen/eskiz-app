//! Perspective (radial) blur: a box average along a per-pixel step that
//! depends on the angle between the pixel's radius and a stored direction.

use crate::image::{mix_masked, Image};
use std::f32::consts::{PI, TAU};

/// Parameters of the perspective blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerspectiveParams {
    /// Origin in normalised coordinates. Default (0.5, 0.5).
    pub origin: [f32; 2],
    /// Direction, components in [-1, 1]. Default (0, 0).
    pub direction: [f32; 2],
    /// Amount in [0, 1]. Default 0.6.
    pub amount: f32,
}

impl Default for PerspectiveParams {
    fn default() -> Self {
        Self {
            origin: [0.5, 0.5],
            direction: [0.0, 0.0],
            amount: 0.6,
        }
    }
}

/// Size length `S = sqrt(W^2 + H^2)` in pixels.
pub fn size_length(width: usize, height: usize) -> f32 {
    ((width * width + height * height) as f32).sqrt()
}

/// Wrap an angle into `(-pi, pi]`.
pub fn wrap_angle(a: f32) -> f32 {
    let mut x = a.rem_euclid(TAU);
    if x > PI {
        x -= TAU;
    }
    x
}

/// Per-pixel tap step in normalised units. The
/// `k = 1 - |d| w` factor can go negative for `|d| > 1`; the design notes do not clamp it.
pub fn step_for(uv: [f32; 2], origin: [f32; 2], direction: [f32; 2], size_len: f32) -> [f32; 2] {
    let r = [uv[0] - origin[0], uv[1] - origin[1]];
    let rl = r[0].hypot(r[1]);
    let theta = if rl == 0.0 { PI } else { r[1].atan2(r[0]) + PI };
    let dl = direction[0].hypot(direction[1]);
    let w = if dl == 0.0 {
        0.0
    } else {
        let phi = direction[1].atan2(direction[0]);
        let alpha = wrap_angle(theta - phi);
        (alpha.abs() * 2.0 / PI).clamp(0.0, 1.0)
    };
    let k = 1.0 - dl * w;
    let f = 2.0 * k / size_len;
    [r[0] * f, r[1] * f]
}

/// Blur with an explicit tap factor `p`. The commit uses `p = 1`.
pub fn apply_with_factor(
    src: &Image,
    mask: Option<&[f32]>,
    params: &PerspectiveParams,
    p: f32,
) -> Image {
    let n = crate::motion::tap_count(params.amount, p);
    let offsets = crate::motion::tap_offsets(n);
    let inv = 1.0 / n as f32;
    let (w, h) = (src.width, src.height);
    let size_len = size_length(w, h);
    let blurred = Image::generate(w, h, |x, y| {
        let uv = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
        let st = step_for(uv, params.origin, params.direction, size_len);
        let mut acc = [0.0f32; 4];
        for &o in &offsets {
            let s = src.sample_uv(uv[0] + o * st[0], uv[1] + o * st[1]);
            for (a, sv) in acc.iter_mut().zip(s) {
                *a += sv;
            }
        }
        acc.map(|a| a * inv)
    });
    mix_masked(src, &blurred, mask)
}

/// Commit pass (`p = 1`).
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &PerspectiveParams) -> Image {
    apply_with_factor(src, mask, params, 1.0)
}

/// Live-preview pass with `p = min(1, 1 / (4a))`.
pub fn preview(src: &Image, mask: Option<&[f32]>, params: &PerspectiveParams) -> Image {
    let a = params.amount.clamp(0.0, 1.0);
    let p = if a <= 0.0 {
        1.0
    } else {
        (1.0 / (4.0 * a)).min(1.0)
    };
    apply_with_factor(src, mask, params, p)
}
