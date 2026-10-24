//! Bloom / glow: threshold, Gaussian blur, additive burn.

use crate::colour::{luma, sat, smoothstep, unpremultiply};
use crate::gaussian;
use crate::image::{mix_masked, Image, Px};

/// Parameters of the bloom filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BloomParams {
    /// Upper threshold U in [0, 1]. Default 0.8 (unverified).
    pub upper: f32,
    /// Lower threshold L in [0, 1]. Default 0.6 (unverified).
    pub lower: f32,
    /// Burn adjustment b in [-0.3, 1]. Default 0.44 at paint defaults.
    pub burn: f32,
    /// Spread in [0, 1], used as the Gaussian amount (section 2). Default 0.14.
    pub spread: f32,
    /// Filter amount in [0, 1], which scales the glow. Default 0.5.
    /// The scaling is an implementation choice.
    pub amount: f32,
    /// Preserve alpha (section 1.6). Default off.
    pub preserve_alpha: bool,
}

impl Default for BloomParams {
    fn default() -> Self {
        Self {
            upper: 0.8,
            lower: 0.6,
            burn: 0.44,
            spread: 0.14,
            amount: 0.5,
            preserve_alpha: false,
        }
    }
}

/// Burn adjustment from a control `n` in [0, 1]: `b = 1.3 n - 0.3`.
pub fn burn_from_control(n: f32) -> f32 {
    1.3 * n.clamp(0.0, 1.0) - 0.3
}

/// Threshold of one pixel.
pub fn threshold_px(px: Px, upper: f32, lower: f32) -> Px {
    let y = luma(unpremultiply(px));
    let f = if lower < upper {
        smoothstep(((y - lower) / (upper - lower)).clamp(0.0, 1.0))
    } else if y > upper {
        1.0
    } else {
        0.0
    };
    px.map(|v| v * f)
}

/// Burn step for one pixel. The alpha takes the maximum of the
/// colour components and the standard alpha (medium confidence, as the design notes say).
/// `src`, `glow` and the result are premultiplied, so the colour is not multiplied by alpha again.
pub fn burn_px(src: Px, glow: Px, burn: f32, preserve: bool) -> Px {
    let mut rgb: [f32; 3] = std::array::from_fn(|k| sat(sat((1.0 + burn) * glow[k]) + src[k]));
    let standard = sat(src[3] * (1.0 - glow[3]) + glow[3]);
    if preserve {
        let alpha = src[3];
        if standard > 0.0 {
            rgb = rgb.map(|v| (v * src[3] / standard).min(alpha));
        }
        [rgb[0], rgb[1], rgb[2], alpha]
    } else {
        let alpha = standard.max(rgb[0]).max(rgb[1]).max(rgb[2]);
        [rgb[0], rgb[1], rgb[2], alpha]
    }
}

/// Apply bloom, then mix by the mask with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &BloomParams) -> Image {
    let thresholded = src.map(|_, _, px| threshold_px(px, params.upper, params.lower));
    let glow = gaussian::blur(&thresholded, params.spread).scale(params.amount);
    let out = Image::generate(src.width, src.height, |x, y| {
        burn_px(
            src.px(x, y),
            glow.px(x, y),
            params.burn,
            params.preserve_alpha,
        )
    });
    mix_masked(src, &out, mask)
}
