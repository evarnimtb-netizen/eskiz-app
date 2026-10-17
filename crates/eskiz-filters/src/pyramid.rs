//! Pyramid blur, the Pencil-mode replacement for the Gaussian blur.
//!
//! A mip chain is built by progressive blurring; each pixel blends between two levels
//! chosen from the amount and the painted mask.

use crate::colour::sat;
use crate::gaussian;
use crate::image::{mask_at, mix_px, Image};

/// Number of mip levels stored (indexed 0 to 5.
pub const MAX_LEVELS: usize = 6;

/// Blur amount applied to each progressive level. Not recovered by the design notes (section 3.1,
/// unverified); this is a placeholder that gives monotonically blurrier levels.
pub const LEVEL_AMOUNT: f32 = 0.3;

/// Parameters of the pyramid blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PyramidParams {
    /// Amount in [0, 1]. Default 0.6.
    pub amount: f32,
}

impl Default for PyramidParams {
    fn default() -> Self {
        Self { amount: 0.6 }
    }
}

/// Level count `L = clamp(floor(log2(max(width, height))), 1, 6)`.
pub fn level_count(width: usize, height: usize) -> usize {
    let m = width.max(height).max(1);
    (m.ilog2() as usize).clamp(1, MAX_LEVELS)
}

/// Level selection. Returns `(lod, mix_factor)`.
pub fn select_level(amount: f32, mask_value: f32, levels: usize) -> (f32, f32) {
    let sat_v = sat(amount * mask_value);
    let scaled = (levels as f32 + 1.0) * sat_v;
    ((scaled - 1.0).max(0.0), scaled.min(1.0))
}

/// Build the mip chain: level 0 is the source and each level is a progressive blur of the
/// one before.
pub fn build_levels(src: &Image) -> Vec<Image> {
    let mut levels = Vec::with_capacity(MAX_LEVELS);
    levels.push(src.clone());
    for _ in 1..MAX_LEVELS {
        let next = gaussian::blur(&levels[levels.len() - 1], LEVEL_AMOUNT);
        levels.push(next);
    }
    levels
}

/// Pyramid blur. The mask is the painted mask of Pencil mode. The output is mixed with the
/// source by `mix_factor`, not by the default mask rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &PyramidParams) -> Image {
    let levels = build_levels(src);
    let count = level_count(src.width, src.height);
    Image::generate(src.width, src.height, |x, y| {
        let i = y * src.width + x;
        let (lod, mix) = select_level(params.amount, mask_at(mask, i), count);
        let lo = (lod.floor() as usize).min(MAX_LEVELS - 1);
        let hi = (lo + 1).min(MAX_LEVELS - 1);
        let t = (lod - lod.floor()).clamp(0.0, 1.0);
        let sampled = mix_px(levels[lo].px(x, y), levels[hi].px(x, y), t);
        mix_px(src.px(x, y), sampled, mix)
    })
}
