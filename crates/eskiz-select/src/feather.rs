//! Feathering: a normalised Gaussian-like blur of a selection mask.

use crate::mask::Mask;

/// Largest feather amount accepted, in pixels.
pub const MAX_FEATHER_PX: f32 = 100.0;

/// Feathers `mask` with a separable Gaussian of standard deviation `amount / 2`, truncated at
/// three standard deviations with weights normalised to sum to 1.
/// Samples outside the mask count as 0. A feather of 0 returns the mask unchanged.
pub fn feather(mask: &Mask, amount: f32) -> Mask {
    let amount = if amount.is_nan() {
        0.0
    } else {
        amount.clamp(0.0, MAX_FEATHER_PX)
    };
    if amount <= 0.0 {
        return mask.clone();
    }
    let sigma = amount / 2.0;
    let half = (3.0 * sigma).ceil() as i64;
    if half < 1 {
        return mask.clone();
    }
    let mut kernel: Vec<f32> = (-half..=half)
        .map(|i| {
            let t = i as f32;
            (-(t * t) / (2.0 * sigma * sigma)).exp()
        })
        .collect();
    let total: f32 = kernel.iter().sum();
    for k in &mut kernel {
        *k /= total;
    }
    let (w, h) = (mask.width, mask.height);
    let horizontal = blur_pass(&mask.data, w, h, &kernel, half, true);
    let data = blur_pass(&horizontal, w, h, &kernel, half, false);
    Mask {
        width: w,
        height: h,
        data,
    }
}

fn blur_pass(
    src: &[f32],
    w: usize,
    h: usize,
    kernel: &[f32],
    half: i64,
    horizontal: bool,
) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for (k, &weight) in kernel.iter().enumerate() {
                let offset = k as i64 - half;
                let (sx, sy) = if horizontal {
                    (x as i64 + offset, y as i64)
                } else {
                    (x as i64, y as i64 + offset)
                };
                if sx < 0 || sy < 0 || sx >= w as i64 || sy >= h as i64 {
                    continue;
                }
                acc += weight * src[sy as usize * w + sx as usize];
            }
            out[y * w + x] = acc.clamp(0.0, 1.0);
        }
    }
    out
}
