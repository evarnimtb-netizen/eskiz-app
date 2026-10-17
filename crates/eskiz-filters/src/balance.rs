//! Colour balance: shadow, midtone and highlight tints, luma preserved.

use crate::colour::{luma, rgb_from_yuv, sat, unpremultiply, yuv};
use crate::image::{mix_masked, Image};

/// Nine balance values in [-1, 1]: one RGB vector each for shadows, midtones and highlights.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BalanceParams {
    pub shadows: [f32; 3],
    pub midtones: [f32; 3],
    pub highlights: [f32; 3],
}

/// Balance one unpremultiplied colour.
pub fn balance_rgb(c: [f32; 3], params: &BalanceParams) -> [f32; 3] {
    let clampv = |v: [f32; 3]| v.map(|x| x.clamp(-1.0, 1.0));
    let s = clampv(params.shadows);
    let m = clampv(params.midtones);
    let h = clampv(params.highlights);
    let balanced: [f32; 3] = std::array::from_fn(|k| {
        let v = c[k];
        let t1 = (v - 0.333008) * 4.0;
        let t2 = (v - 0.666992) * 4.0;
        let shadow = sat(0.5 - t1);
        let mid_up = sat(t1 + 0.5);
        let mid_down = sat(0.5 - t2);
        let high_up = sat(t2 + 0.5);
        let bal = shadow * s[k] + mid_up * mid_down * m[k] + high_up * h[k];
        sat(0.700_195 * bal + v)
    });
    let y = luma(c);
    let [_, u, v] = yuv(balanced);
    rgb_from_yuv([y, u, v]).map(sat)
}

/// Apply colour balance and mix by the mask with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &BalanceParams) -> Image {
    let out = src.map(|_, _, px| {
        let rgb = balance_rgb(unpremultiply(px), params);
        [rgb[0] * px[3], rgb[1] * px[3], rgb[2] * px[3], px[3]]
    });
    mix_masked(src, &out, mask)
}
