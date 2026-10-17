//! Hue, saturation and brightness, in the YUV-style space.
//!
//! For each pixel, with the unpremultiplied colour `c`: `(Y, U, V) = YUV(c)`, `Y' = Y + b`,
//! `U' = (1 + s)(cos t U - sin t V)`, `V' = (1 + s)(sin t U + cos t V)` with `t = pi h`, then
//! `rgb = clamp(RGB(Y', U', V'))`, and the result is `rgb * alpha` with the alpha unchanged.

use super::luma;
use crate::pixel::Chan;
use std::f32::consts::PI;

/// RGB to (Y, U, V).
pub fn to_yuv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let y = luma(r, g, b);
    (y, (b - y) * 0.564_941, (r - y) * 0.712_891)
}

/// (Y, U, V) back to RGB, (inverse constants).
pub fn to_rgb(y: f32, u: f32, v: f32) -> (f32, f32, f32) {
    (y + 1.403 * v, y - 0.344 * u - 0.714 * v, y + 1.770 * u)
}

/// One colour (unpremultiplied, 0..1) through the adjustment, clamped to 0..=1.
pub fn adjust_colour(c: [f32; 3], hue: f32, sat: f32, bright: f32) -> [f32; 3] {
    let (y, u, v) = to_yuv(c[0], c[1], c[2]);
    let theta = PI * hue;
    let (s, co) = theta.sin_cos();
    let m = 1.0 + sat;
    let u2 = m * (co * u - s * v);
    let v2 = m * (s * u + co * v);
    let (r, g, b) = to_rgb(y + bright, u2, v2);
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
}

/// `src` (premultiplied RGBA, 8 or 16 bits) through hue `h`, saturation `s` and brightness `b`, each in
/// -1..=1. With all three zero the input is returned exactly.
pub fn apply(src: &[u8], h: f64, s: f64, b: f64) -> Vec<u8> {
    apply_t(src, h, s, b)
}

/// [`apply`] for either depth.
pub fn apply_t<T: Chan>(src: &[T], h: f64, s: f64, b: f64) -> Vec<T> {
    let clean = |v: f64| if v.is_finite() { v.clamp(-1.0, 1.0) as f32 } else { 0.0 };
    let (h, s, b) = (clean(h), clean(s), clean(b));
    if h == 0.0 && s == 0.0 && b == 0.0 {
        return src.to_vec();
    }
    let theta = PI * h;
    let (sin_t, cos_t) = theta.sin_cos();
    let m = 1.0 + s;
    let mut out = vec![T::default(); src.len()];
    super::par_rows(src, &mut out, 4096 * 4, |_, s, dst| {
        for (px, o) in s.as_chunks::<4>().0.iter().zip(dst.as_chunks_mut::<4>().0.iter_mut()) {
            let a = px[3];
            if a.is_zero() {
                continue;
            }
            let af = a.to_f32() / T::MAXF;
            let c = [0, 1, 2].map(|i| (px[i].to_f32() / T::MAXF / af).min(1.0));
            let (y, u, v) = to_yuv(c[0], c[1], c[2]);
            let (r, g, bl) = to_rgb(y + b, m * (cos_t * u - sin_t * v), m * (sin_t * u + cos_t * v));
            for (dst, val) in o[..3].iter_mut().zip([r, g, bl]) {
                *dst = T::from_f32_round(val.clamp(0.0, 1.0) * af * T::MAXF);
            }
            o[3] = a;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HSB1: all parameters zero returns the input.
    #[test]
    fn zero_is_identity() {
        let mut src: Vec<u8> = (0..64u32).map(|i| (i * 37 % 256) as u8).collect();
        // A valid premultiplied buffer: colour never above alpha.
        for p in src.as_chunks_mut::<4>().0 {
            for c in 0..3 {
                p[c] = p[c].min(p[3]);
            }
        }
        assert_eq!(apply(&src, 0.0, 0.0, 0.0), src);
    }

    /// HSB2: hue 0.5 rotates chroma by 90 degrees; pure red goes to about (0.06, 0.59, 0).
    #[test]
    fn hue_half_turns_red_toward_green() {
        let out = adjust_colour([1.0, 0.0, 0.0], 0.5, 0.0, 0.0);
        assert!((out[0] - 0.06).abs() < 0.02, "{out:?}");
        assert!((out[1] - 0.59).abs() < 0.02, "{out:?}");
        assert!(out[2] < 0.01, "blue clamps to 0: {out:?}");
        // U and V swap with a sign change: U' = -V, V' = U.
        let (_, u, v) = to_yuv(1.0, 0.0, 0.0);
        let theta = PI * 0.5;
        let u2 = theta.cos() * u - theta.sin() * v;
        assert!((u2 + v).abs() < 1e-5);
    }

    /// HSB3: saturation -1 gives greyscale at the same luma.
    #[test]
    fn saturation_minus_one_is_grey_at_the_same_luma() {
        for c in [[1.0, 0.2, 0.1], [0.1, 0.8, 0.3], [0.3, 0.3, 0.9]] {
            let out = adjust_colour(c, 0.0, -1.0, 0.0);
            assert!((out[0] - out[1]).abs() < 2e-3 && (out[1] - out[2]).abs() < 2e-3, "{out:?}");
            assert!((luma(out[0], out[1], out[2]) - luma(c[0], c[1], c[2])).abs() < 2e-3);
        }
        let b = adjust_colour([0.5, 0.2, 0.2], 0.0, -1.0, 0.1);
        assert!((luma(b[0], b[1], b[2]) - (luma(0.5, 0.2, 0.2) + 0.1)).abs() < 2e-3);
    }

    /// HSB4: brightness +1 pushes every colour to white after clamping.
    #[test]
    fn brightness_plus_one_is_white() {
        for c in [[0.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.0, 0.0, 0.0]] {
            let out = adjust_colour(c, 0.0, 0.0, 1.0);
            assert!(out.iter().all(|&v| v > 0.999), "{c:?} -> {out:?}");
        }
        assert_eq!(apply(&[0, 0, 0, 255], 0.0, 0.0, 1.0), vec![255, 255, 255, 255]);
        // A strongly coloured pixel is at least pushed up in every channel.
        let out = adjust_colour([0.9, 0.1, 0.1], 0.0, 0.0, 1.0);
        assert!(out.iter().all(|&v| v > 0.9), "{out:?}");
    }

    /// HSB5: RGB to YUV and back returns the input within 1/255.
    #[test]
    fn yuv_round_trip() {
        for r in 0..=4 {
            for g in 0..=4 {
                for b in 0..=4 {
                    let c = [r as f32 / 4.0, g as f32 / 4.0, b as f32 / 4.0];
                    let (y, u, v) = to_yuv(c[0], c[1], c[2]);
                    let (r2, g2, b2) = to_rgb(y, u, v);
                    for (a, e) in [r2, g2, b2].iter().zip(c) {
                        assert!((a - e).abs() < 1.0 / 255.0, "{c:?} -> {r2} {g2} {b2}");
                    }
                }
            }
        }
    }

    #[test]
    fn alpha_is_unchanged_and_transparent_pixels_stay_empty() {
        let out = apply(&[100, 50, 20, 200, 0, 0, 0, 0], 0.3, 0.2, -0.1);
        assert_eq!(out[3], 200);
        assert_eq!(&out[4..], &[0, 0, 0, 0]);
        assert!(out[0] <= 200 && out[1] <= 200 && out[2] <= 200);
    }
}
