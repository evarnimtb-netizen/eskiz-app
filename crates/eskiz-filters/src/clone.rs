//! Clone: copies pixels from a source offset.

use crate::image::{mask_at, mix_px, Image};

/// Parameters of the clone source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloneParams {
    /// Source offset in pixels: the source for pixel `q` is `q + offset`. Fixed per stroke.
    pub offset: [f32; 2],
    /// Opacity in [0, 1]. Default 1.
    pub opacity: f32,
}

impl Default for CloneParams {
    fn default() -> Self {
        Self {
            offset: [0.0, 0.0],
            opacity: 1.0,
        }
    }
}

/// Clone with a fixed offset: `out(q) = mix(out(q), layer(q + o), m(q) opacity)`.
/// The mask acts as the brush alpha. Bilinear sampling, and pixels outside the layer are
/// transparent.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &CloneParams) -> Image {
    Image::generate(src.width, src.height, |x, y| {
        let i = y * src.width + x;
        let fx = x as f32 + params.offset[0];
        let fy = y as f32 + params.offset[1];
        let sampled = src.sample_texel_or_zero(fx, fy);
        let weight = mask_at(mask, i) * params.opacity.clamp(0.0, 1.0);
        mix_px(src.px(x, y), sampled, weight)
    })
}
