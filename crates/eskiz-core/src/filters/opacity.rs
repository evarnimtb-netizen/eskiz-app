//! Opacity: multiplies the layer's premultiplied pixels, colour and alpha, by the
//! amount. Amount 1 is the identity and amount 0 makes the layer fully transparent. The mix rule
//! is not used; the result is written directly.

use crate::pixel::Chan;

/// `src` (premultiplied RGBA, 8 or 16 bits) scaled by `amount`, clamped to 0..=1.
pub fn apply(src: &[u8], amount: f64) -> Vec<u8> {
    apply_t(src, amount)
}

/// [`apply`] for either depth.
pub fn apply_t<T: Chan>(src: &[T], amount: f64) -> Vec<T> {
    let a = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 1.0 };
    if a >= 1.0 {
        return src.to_vec();
    }
    let mut out = vec![T::default(); src.len()];
    super::par_rows(src, &mut out, 4096 * 4, |_, s, d| {
        for (o, &v) in d.iter_mut().zip(s) {
            *o = T::from_f64_round(v.raw() * a);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O1: amount 1 returns the input; amount 0 makes the layer fully transparent.
    #[test]
    fn amount_one_is_identity_and_zero_is_transparent() {
        let src = vec![10, 20, 30, 40, 255, 128, 0, 255];
        assert_eq!(apply(&src, 1.0), src);
        assert!(apply(&src, 0.0).iter().all(|&v| v == 0));
    }

    #[test]
    fn half_halves_colour_and_alpha() {
        assert_eq!(apply(&[200, 100, 50, 255], 0.5), vec![100, 50, 25, 128]);
    }
}
