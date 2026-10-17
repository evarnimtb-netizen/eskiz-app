//! Recolor: pixels near a source colour take
//! the target hue and saturation and keep their luma.

use crate::colour::{hsl_to_rgb, luma, premultiply, rgb_to_hsl, sat, unpremultiply};
use crate::image::{mix_masked, Image};

/// Parameters of the recolor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecolorParams {
    /// Colour to replace. Default red.
    pub source: [f32; 3],
    /// Colour whose hue and saturation are used. Default blue.
    pub target: [f32; 3],
    /// RGB-distance tolerance in [0, 1]. Default 0.2. The soft edge has the same width.
    pub tolerance: f32,
}

impl Default for RecolorParams {
    fn default() -> Self {
        Self {
            source: [1.0, 0.0, 0.0],
            target: [0.0, 0.0, 1.0],
            tolerance: 0.2,
        }
    }
}

/// Selection weight: 1 inside the tolerance, falling to 0 over a band of the same width.
pub fn selection_weight(distance: f32, tolerance: f32) -> f32 {
    if tolerance <= 0.0 {
        return if distance <= 0.0 { 1.0 } else { 0.0 };
    }
    sat(1.0 - (distance - tolerance) / tolerance)
}

/// Apply the recolor and mix by the mask with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &RecolorParams) -> Image {
    let [ht, st, _] = rgb_to_hsl(params.target);
    let out = src.map(|_, _, px| {
        let c = unpremultiply(px);
        let d = (0..3)
            .map(|k| (c[k] - params.source[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        let w = selection_weight(d, params.tolerance);
        let recoloured = hsl_to_rgb(ht, st, luma(c));
        let out_rgb: [f32; 3] = std::array::from_fn(|k| c[k] + (recoloured[k] - c[k]) * w);
        premultiply(out_rgb, px[3])
    });
    mix_masked(src, &out, mask)
}
