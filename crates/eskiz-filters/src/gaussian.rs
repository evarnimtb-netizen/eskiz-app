//! Gaussian blur: two separable passes, vertical first, premultiplied,
//! clamp-to-edge. Used directly by Layer mode and by bloom and transverse chromatic
//! aberration. The brush kernel is not used here.

use crate::image::{mix_masked, Image};

/// Name of the undo entry. Undo itself lives in the app.
pub const UNDO_NAME: &str = "Gaussian Blur";

/// Peak of the unnormalised Gaussian.
const PEAK: f32 = 0.3989423;

/// Parameters of the Gaussian blur.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaussianParams {
    /// Amount in [0, 1]. Default 0.6.
    pub amount: f32,
}

impl Default for GaussianParams {
    fn default() -> Self {
        Self { amount: 0.6 }
    }
}

/// Tap count `N = floor(63 sqrt(a)) + 1`, from 1 to 64.
pub fn tap_count(amount: f32) -> usize {
    let a = amount.clamp(0.0, 1.0);
    (63.0 * a.sqrt()).floor() as usize + 1
}

/// Effective sigma.
pub fn sigma(amount: f32) -> f32 {
    let a = amount.clamp(0.0, 1.0);
    let x = 32.0 * a;
    let n = x.floor();
    let f = x - n;
    if n >= 2.0 {
        f + (1.0 - f) * (1.0 - 1.0 / n)
    } else {
        f
    }
}

/// Normalised weights `w_0 .. w_{N-1}`. They sum to 1 over the full
/// symmetric kernel, so a flat region stays flat.
pub fn weights(amount: f32) -> Vec<f32> {
    let n = tap_count(amount);
    let s = sigma(amount) * n as f32 / 3.0;
    let g: Vec<f32> = (0..n)
        .map(|i| {
            if s > 0.0 {
                PEAK * (-0.5 * (i as f32 / s).powi(2)).exp()
            } else if i == 0 {
                PEAK
            } else {
                0.0
            }
        })
        .collect();
    let total = g[0] + 2.0 * g[1..].iter().sum::<f32>();
    if total <= 0.0 {
        let mut w = vec![0.0; n];
        w[0] = 1.0;
        w
    } else {
        g.iter().map(|v| v / total).collect()
    }
}

/// Blur an image with the Gaussian at the given amount. No mask is applied.
pub fn blur(src: &Image, amount: f32) -> Image {
    let w = weights(amount);
    if w.len() == 1 {
        return src.clone();
    }
    let vertical = pass(src, &w, true);
    pass(&vertical, &w, false)
}

fn pass(src: &Image, w: &[f32], vertical: bool) -> Image {
    Image::generate(src.width, src.height, |x, y| {
        let centre = src.px(x, y);
        let mut acc = centre.map(|v| v * w[0]);
        let (xi, yi) = (x as isize, y as isize);
        for (i, &wi) in w.iter().enumerate().skip(1) {
            let d = i as isize;
            let (a, b) = if vertical {
                (src.px_clamped(xi, yi + d), src.px_clamped(xi, yi - d))
            } else {
                (src.px_clamped(xi + d, yi), src.px_clamped(xi - d, yi))
            };
            for ((o, &av), &bv) in acc.iter_mut().zip(&a).zip(&b) {
                *o += wi * (av + bv);
            }
        }
        acc
    })
}

/// Layer-mode Gaussian blur, mixed with the source by the mask.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &GaussianParams) -> Image {
    let blurred = blur(src, params.amount);
    mix_masked(src, &blurred, mask)
}

/// Preview scale factor `floor(32 a)` when that is above 1, otherwise 1,
/// verified). The preview works at `1 / factor` of the layer size.
pub fn preview_scale(amount: f32) -> usize {
    let k = (32.0 * amount.clamp(0.0, 1.0)).floor() as usize;
    if k > 1 {
        k
    } else {
        1
    }
}

/// Live-preview variant: blur at reduced scale, then upsample. The commit
/// pass is [`apply`], which runs at full resolution.
pub fn preview(src: &Image, mask: Option<&[f32]>, params: &GaussianParams) -> Image {
    let k = preview_scale(params.amount);
    if k == 1 {
        return apply(src, mask, params);
    }
    let small = src.downscale_box(k);
    let blurred = blur(&small, params.amount).resize_bilinear(src.width, src.height);
    mix_masked(src, &blurred, mask)
}
