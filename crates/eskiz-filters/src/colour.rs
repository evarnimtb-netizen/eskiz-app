//! Shared colour maths.

use crate::image::Px;

/// Rec. 601 luma weights.
pub const LUMA: [f32; 3] = [0.299, 0.586914, 0.114014];

/// Clamp to [0, 1].
#[inline]
pub fn sat(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// `smoothstep(t) = t^2 (3 - 2t)` for `t` clamped to [0, 1].
#[inline]
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Quadratic ease-in-out.
#[inline]
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - 2.0 * (1.0 - t) * (1.0 - t)
    }
}

/// Luma of an RGB triple.
#[inline]
pub fn luma(rgb: [f32; 3]) -> f32 {
    LUMA[0] * rgb[0] + LUMA[1] * rgb[1] + LUMA[2] * rgb[2]
}

/// `(Y, U, V)` from RGB: `U = (B - Y) 0.564941`, `V = (R - Y) 0.712891`.
#[inline]
pub fn yuv(rgb: [f32; 3]) -> [f32; 3] {
    let y = luma(rgb);
    [y, (rgb[2] - y) * 0.564941, (rgb[0] - y) * 0.712891]
}

/// RGB from `(Y, U, V)` with the inverse constants.
#[inline]
pub fn rgb_from_yuv(c: [f32; 3]) -> [f32; 3] {
    let [y, u, v] = c;
    [y + 1.403 * v, y - 0.344 * u - 0.714 * v, y + 1.770 * u]
}

/// Unpremultiply a pixel's colour. Yields black when alpha is 0.
#[inline]
pub fn unpremultiply(px: Px) -> [f32; 3] {
    let a = px[3];
    if a <= 0.0 {
        [0.0; 3]
    } else {
        [px[0] / a, px[1] / a, px[2] / a]
    }
}

/// Premultiply an RGB colour by alpha `a`, producing a pixel.
#[inline]
pub fn premultiply(rgb: [f32; 3], a: f32) -> Px {
    [rgb[0] * a, rgb[1] * a, rgb[2] * a, a]
}

/// HSL to RGB. `h` is in turns (1.0 = a full revolution), `s` and `l` in [0, 1].
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    if s <= 0.0 {
        return [l; 3];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let h = h.rem_euclid(1.0);
    [
        hue_to_channel(p, q, h + 1.0 / 3.0),
        hue_to_channel(p, q, h),
        hue_to_channel(p, q, h - 1.0 / 3.0),
    ]
}

fn hue_to_channel(p: f32, q: f32, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

/// RGB to `[h, s, l]` with `h` in turns.
pub fn rgb_to_hsl(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return [0.0, 0.0, l];
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    [h / 6.0, s, l]
}
