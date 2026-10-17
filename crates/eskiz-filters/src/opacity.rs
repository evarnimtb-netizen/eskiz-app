//! Opacity: multiplies both colour and alpha by the amount. Amount 1 is the
//! identity.

use crate::image::{mix_masked, Image};

/// Parameters of the opacity adjustment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpacityParams {
    /// Amount in [0, 1]. Default 1 (unverified).
    pub amount: f32,
}

impl Default for OpacityParams {
    fn default() -> Self {
        Self { amount: 1.0 }
    }
}

/// `result = pixel x amount`, mixed by the mask with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &OpacityParams) -> Image {
    let scaled = src.scale(params.amount.clamp(0.0, 1.0));
    mix_masked(src, &scaled, mask)
}
