//! Hue, saturation and brightness.

use crate::colour::{premultiply, rgb_from_yuv, sat, unpremultiply, yuv};
use crate::image::{mix_masked, Image};
use std::f32::consts::PI;

/// Parameters of the HSB adjustment. All three are in [-1, 1]. Hue is periodic.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HsbParams {
    /// Hue. Default 0.
    pub hue: f32,
    /// Saturation. Default 0.
    pub saturation: f32,
    /// Brightness. Default 0.
    pub brightness: f32,
}

/// Wrap a hue into [-1, 1] by adding or subtracting 2.
pub fn wrap_hue(h: f32) -> f32 {
    let mut x = h.rem_euclid(2.0);
    if x > 1.0 {
        x -= 2.0;
    }
    x
}

/// Transform one unpremultiplied RGB colour.
/// The hue is an angle of `pi h`. The clamp is applied after the YUV inverse.
pub fn transform_rgb(c: [f32; 3], params: &HsbParams) -> [f32; 3] {
    let theta = PI * wrap_hue(params.hue);
    let (sin_t, cos_t) = theta.sin_cos();
    let ms = 1.0 + params.saturation.clamp(-1.0, 1.0);
    let bri = params.brightness.clamp(-1.0, 1.0);
    let [y, u, v] = yuv(c);
    let y2 = y + bri;
    let u2 = ms * (cos_t * u - sin_t * v);
    let v2 = ms * (sin_t * u + cos_t * v);
    rgb_from_yuv([y2, u2, v2]).map(sat)
}

/// Apply HSB to an image and mix by the mask with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &HsbParams) -> Image {
    let out = src.map(|_, _, px| premultiply(transform_rgb(unpremultiply(px), params), px[3]));
    mix_masked(src, &out, mask)
}
