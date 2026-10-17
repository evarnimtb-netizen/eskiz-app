//! Sharpen: an unsharp mask built from the four neighbours, on
//! unpremultiplied colour.

use crate::colour::{premultiply, sat, unpremultiply};
use crate::image::{mix_masked, Image};

/// Parameters of the sharpen filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharpenParams {
    /// Amount k in [0, 1]. Default 0 (unverified).
    pub amount: f32,
}

impl Default for SharpenParams {
    fn default() -> Self {
        Self { amount: 0.0 }
    }
}

/// Sharpen: `out = c (4k + 1) - k (L + R + U + D)`, clamped per channel.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &SharpenParams) -> Image {
    let k = params.amount.clamp(0.0, 1.0);
    let sharpened = Image::generate(src.width, src.height, |x, y| {
        let (xi, yi) = (x as isize, y as isize);
        let px = src.px(x, y);
        let c = unpremultiply(px);
        let l = unpremultiply(src.px_clamped(xi - 1, yi));
        let r = unpremultiply(src.px_clamped(xi + 1, yi));
        let u = unpremultiply(src.px_clamped(xi, yi - 1));
        let d = unpremultiply(src.px_clamped(xi, yi + 1));
        let rgb: [f32; 3] = std::array::from_fn(|ch| {
            sat(c[ch] * (4.0 * k + 1.0) - k * (l[ch] + r[ch] + u[ch] + d[ch]))
        });
        premultiply(rgb, px[3])
    });
    mix_masked(src, &sharpened, mask)
}
