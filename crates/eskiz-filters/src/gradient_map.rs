//! Gradient map: remaps luma through a starting table and a target table,
//! easing from one to the other during a live preview.

use crate::colour::{ease_in_out, luma, premultiply, unpremultiply};
use crate::image::{mask_at, mix_weighted, Image};

/// Duration of the live-preview blend in seconds.
pub const BLEND_SECONDS: f32 = 0.25;

/// Table length.
pub const TABLE_LEN: usize = 256;

/// Linear ramp between two colours over 256 entries.
pub fn ramp(from: [f32; 3], to: [f32; 3]) -> Vec<[f32; 3]> {
    (0..TABLE_LEN)
        .map(|i| {
            let t = i as f32 / (TABLE_LEN - 1) as f32;
            std::array::from_fn(|k| from[k] + (to[k] - from[k]) * t)
        })
        .collect()
}

/// Lookup with linear interpolation. `y` outside [0, 1] clamps to the end entries.
pub fn lookup(table: &[[f32; 3]], y: f32) -> [f32; 3] {
    let last = table.len() - 1;
    let pos = y.clamp(0.0, 1.0) * last as f32;
    let i0 = (pos.floor() as usize).min(last.saturating_sub(1));
    let i1 = (i0 + 1).min(last);
    let t = pos - i0 as f32;
    std::array::from_fn(|k| table[i0][k] + (table[i1][k] - table[i0][k]) * t)
}

/// Blend during a live preview. `elapsed` is measured from the
/// window start. Outside the window the blend is 1.
pub fn blend_at(elapsed: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    let t = elapsed / duration;
    if !(0.0..=1.0).contains(&t) {
        1.0
    } else {
        ease_in_out(t)
    }
}

/// Parameters of the gradient map.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientMapParams {
    /// Starting table (256 colours indexed by luma).
    pub start: Vec<[f32; 3]>,
    /// Target table (256 colours indexed by luma).
    pub target: Vec<[f32; 3]>,
    /// Amount in [0, 1]. Default 1 (unverified).
    pub amount: f32,
    /// Blend in [0, 1]. 1 on commit.
    pub blend: f32,
}

impl GradientMapParams {
    /// Parameters for a commit: blend 1, so the result equals the target table exactly.
    pub fn commit(start: Vec<[f32; 3]>, target: Vec<[f32; 3]>) -> Self {
        Self {
            start,
            target,
            amount: 1.0,
            blend: 1.0,
        }
    }
}

/// Apply the gradient map. The mix factor is `mask x amount`.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &GradientMapParams) -> Image {
    let blend = params.blend.clamp(0.0, 1.0);
    let amount = params.amount.clamp(0.0, 1.0);
    let adjusted = src.map(|_, _, px| {
        let y = luma(unpremultiply(px));
        let a = lookup(&params.start, y);
        let b = lookup(&params.target, y);
        let g: [f32; 3] = std::array::from_fn(|k| {
            (a[k] * a[k] * (1.0 - blend) + b[k] * b[k] * blend)
                .max(0.0)
                .sqrt()
        });
        premultiply(g, px[3])
    });
    mix_weighted(src, &adjusted, |i| mask_at(mask, i) * amount)
}
