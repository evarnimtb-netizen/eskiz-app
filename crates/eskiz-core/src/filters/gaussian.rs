//! Gaussian blur: a one-dimensional Gaussian applied twice, vertically then
//! horizontally, on premultiplied colour with clamp-to-edge sampling.
//!
//! The amount `a` in 0..=1 gives the tap count `N = floor(63 sqrt(a)) + 1` and an effective sigma.
//! The kernel's sigma in taps is `s = sigma * N / 3` and the weights are normalised
//! so a flat region never changes. Samples sit at whole-pixel offsets `i` for
//! `i = 0..N`, mirrored to both sides.

use super::{downscale, upscale, Progress, Quality};
use crate::pixel::Chan;

/// Prefactor of the unnormalised Gaussian. It cancels in the normalisation.
const G0: f64 = 0.398_942_3;

/// A normalised one-sided kernel: `weights[0]` is the centre and `weights[i]` the weight of
/// each of the samples at `+i` and `-i`.
#[derive(Clone, Debug, PartialEq)]
pub struct Kernel {
    /// Number of taps `N` (1 to 64). One tap means the identity.
    pub taps: usize,
    /// Effective sigma (0 for amount 0).
    pub sigma: f64,
    /// Sigma of the Gaussian, in taps (`sigma * N / 3`).
    pub s: f64,
    pub weights: Vec<f32>,
}

/// Tap count and effective sigma for an amount.
pub fn taps_and_sigma(amount: f64) -> (usize, f64) {
    let a = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
    let taps = (63.0 * a.sqrt()).floor() as usize + 1;
    let x = 32.0 * a;
    let n = x.floor();
    let f = x - n;
    let sigma = if n >= 2.0 { f + (1.0 - f) * (1.0 - 1.0 / n) } else { f };
    (taps, sigma)
}

/// The kernel for an amount.
pub fn kernel(amount: f64) -> Kernel {
    let (taps, sigma) = taps_and_sigma(amount);
    let mut k = kernel_from(taps, sigma * taps as f64 / 3.0);
    k.sigma = sigma;
    if sigma <= 0.0 {
        k = kernel_from(1, 0.0);
    }
    k
}

/// A normalised kernel of `taps` samples whose Gaussian has sigma `s` (in taps).
pub fn kernel_from(taps: usize, s: f64) -> Kernel {
    let taps = taps.max(1);
    let g: Vec<f64> = (0..taps)
        .map(|i| {
            if s > 0.0 {
                let t = i as f64 / s;
                G0 * (-0.5 * t * t).exp()
            } else if i == 0 {
                G0
            } else {
                0.0
            }
        })
        .collect();
    let total = g[0] + 2.0 * g[1..].iter().sum::<f64>();
    let weights = if total > 0.0 {
        g.iter().map(|v| (v / total) as f32).collect()
    } else {
        let mut w = vec![0.0; taps];
        w[0] = 1.0;
        w
    };
    Kernel { taps, sigma: 0.0, s, weights }
}

/// Downscale factor for a preview of this size and kernel: 1 (full resolution) when the layer is
/// small or the blur is slight, up to 4.
pub fn preview_scale(width: usize, height: usize, k: &Kernel) -> usize {
    if width * height <= 400_000 || k.taps <= 4 {
        return 1;
    }
    ((k.s / 2.0).floor() as usize).clamp(1, 4)
}

/// Blur with an amount. Preview quality may run on a downscaled proxy (kernel scaled to match)
/// and scale the result back up; full quality is exactly.
pub fn blur_amount<T: Chan>(src: &[T], width: usize, height: usize, amount: f64, quality: Quality, progress: Option<&Progress>) -> Vec<T> {
    let k = kernel(amount);
    if k.taps == 1 || width == 0 || height == 0 {
        return src.to_vec();
    }
    let scale = if quality == Quality::Preview { preview_scale(width, height, &k) } else { 1 };
    if scale > 1 {
        let (small, pw, ph) = downscale(src, width, height, scale);
        let pk = kernel_from(k.taps.div_ceil(scale), k.s / scale as f64);
        let blurred = blur(&small, pw, ph, &pk, progress);
        return upscale(&blurred, pw, ph, width, height, scale);
    }
    blur(src, width, height, &k, progress)
}

/// Threads to use for `rows` rows of work with `taps` taps.
/// The Gaussian blur of `amount` as seen on a proxy made by [`super::downscale`] with factor `k`:
/// the kernel shrinks with the image. Mixed with `src` by `mask` like [`super::Adjustment::render`].
pub fn blur_scaled<T: Chan>(src: &[T], width: usize, height: usize, amount: f64, k: usize, mask: Option<&[f32]>) -> Vec<T> {
    let kern = kernel(amount);
    let mut out = if kern.taps == 1 {
        src.to_vec()
    } else {
        let pk = kernel_from(kern.taps.div_ceil(k.max(1)), kern.s / k.max(1) as f64);
        blur(src, width, height, &pk, None)
    };
    super::mix_bytes(src, &mut out, mask);
    out
}

pub(super) fn workers(rows: usize, width: usize, taps: usize) -> usize {
    let work = rows * width * taps;
    if work < 4_000_000 {
        return 1;
    }
    crate::parallelism::cpus().clamp(1, 8).min(rows)
}

/// The two-pass blur of premultiplied RGBA8 `src` with `k`.
pub fn blur<T: Chan>(src: &[T], width: usize, height: usize, k: &Kernel, progress: Option<&Progress>) -> Vec<T> {
    if k.taps == 1 || width == 0 || height == 0 {
        return src.to_vec();
    }
    if let Some(p) = progress {
        p.add_total(2 * height);
    }
    let row = width * 4;
    let n_workers = workers(height, width, k.taps);
    let rows_per = height.div_ceil(n_workers);
    let mut done = vec![T::default(); src.len()];
    // Each worker takes a run of rows and handles it in bands: the band's source rows (plus the
    // kernel's reach above and below) go to `f32`, the vertical pass fills the band's middle
    // rows, the horizontal pass writes the output. The sums are the ones of a whole-image pass,
    // so the result is the same; the `f32` working set is a band, not the layer.
    std::thread::scope(|scope| {
        for (chunk_index, out) in done.chunks_mut(rows_per * row).enumerate() {
            scope.spawn(move || {
                let first = chunk_index * rows_per;
                let rows = out.len() / row;
                let reach = k.taps - 1;
                let mut input: Vec<f32> = Vec::new();
                let mut mid: Vec<f32> = Vec::new();
                let mut padded = Vec::new();
                let mut y0 = first;
                while y0 < first + rows {
                    let y1 = (y0 + BAND_ROWS).min(first + rows);
                    let (lo, hi) = (y0.saturating_sub(reach), (y1 + reach).min(height));
                    input.clear();
                    input.extend(src[lo * row..hi * row].iter().map(|&v| v.to_f32()));
                    mid.resize((y1 - y0) * row, 0.0);
                    for (r, dst) in mid.chunks_mut(row).enumerate() {
                        vertical_row(&input, lo, dst, y0 + r, width, height, k);
                    }
                    for (r, dst) in out[(y0 - first) * row..(y1 - first) * row].chunks_mut(row).enumerate() {
                        horizontal_row(&mid[r * row..(r + 1) * row], dst, width, k, &mut padded);
                    }
                    if let Some(p) = progress {
                        p.add_done(2 * (y1 - y0));
                    }
                    y0 = y1;
                }
            });
        }
    });
    done
}

/// Output rows per band of the blur's working set.
const BAND_ROWS: usize = 32;

/// `input` holds the rows from `base` on.
fn vertical_row(input: &[f32], base: usize, dst: &mut [f32], y: usize, width: usize, height: usize, k: &Kernel) {
    let row = width * 4;
    let w0 = k.weights[0];
    for (d, s) in dst.iter_mut().zip(&input[(y - base) * row..(y - base + 1) * row]) {
        *d = w0 * s;
    }
    for (i, &w) in k.weights.iter().enumerate().skip(1) {
        let up = &input[(y.saturating_sub(i) - base) * row..][..row];
        let down = &input[((y + i).min(height - 1) - base) * row..][..row];
        for ((d, a), b) in dst.iter_mut().zip(up).zip(down) {
            *d += w * (a + b);
        }
    }
}

pub(super) fn horizontal_row<T: Chan>(mid: &[f32], dst: &mut [T], width: usize, k: &Kernel, padded: &mut Vec<[f32; 4]>) {
    let reach = k.taps - 1;
    padded.clear();
    let px = |x: usize| [mid[x * 4], mid[x * 4 + 1], mid[x * 4 + 2], mid[x * 4 + 3]];
    padded.resize(reach, px(0));
    padded.extend((0..width).map(px));
    padded.resize(reach + width + reach, px(width - 1));
    let w0 = k.weights[0];
    for (x, out) in dst.chunks_mut(4).enumerate() {
        let c = x + reach;
        let centre = padded[c];
        let mut acc = [w0 * centre[0], w0 * centre[1], w0 * centre[2], w0 * centre[3]];
        for (i, &w) in k.weights.iter().enumerate().skip(1) {
            let (a, b) = (padded[c - i], padded[c + i]);
            for ch in 0..4 {
                acc[ch] += w * (a[ch] + b[ch]);
            }
        }
        for (o, v) in out.iter_mut().zip(acc) {
            *o = T::from_f32_round(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// G1: at amount 0.6 the kernel has 49 taps and sigma about 0.958.
    #[test]
    fn g1_amount_point_six() {
        let (n, sigma) = taps_and_sigma(0.6);
        assert_eq!(n, 49);
        assert!((sigma - 0.9579).abs() < 1e-3, "{sigma}");
        assert_eq!(kernel(0.6).taps, 49);
    }

    /// G2: at amount 0 the output equals the input exactly.
    #[test]
    fn g2_amount_zero_is_identity() {
        let src: Vec<u8> = (0..(9 * 7 * 4)).map(|i| (i * 13 % 251) as u8).collect();
        assert_eq!(kernel(0.0).taps, 1);
        assert_eq!(blur_amount(&src, 9, 7, 0.0, Quality::Full, None), src);
    }

    /// G3: at amount 1 the kernel has 64 taps.
    #[test]
    fn g3_amount_one_has_64_taps() {
        assert_eq!(kernel(1.0).taps, 64);
        assert_eq!(kernel(1.0).weights.len(), 64);
    }

    /// G4: at amount 0.25 the kernel has 32 taps and sigma 0.875.
    #[test]
    fn g4_amount_quarter() {
        let (n, sigma) = taps_and_sigma(0.25);
        assert_eq!(n, 32);
        assert!((sigma - 0.875).abs() < 1e-12);
    }

    /// G5: a flat colour region is unchanged by any amount (the weights sum to 1).
    #[test]
    fn g5_flat_regions_do_not_change() {
        for amount in [0.05, 0.25, 0.6, 1.0] {
            let k = kernel(amount);
            let sum = f64::from(k.weights[0]) + 2.0 * k.weights[1..].iter().map(|&w| f64::from(w)).sum::<f64>();
            assert!((sum - 1.0).abs() < 1e-5, "amount {amount}: weights sum to {sum}");
            let src = [200u8, 120, 40, 230].repeat(20 * 16);
            assert_eq!(blur_amount(&src, 20, 16, amount, Quality::Full, None), src, "amount {amount}");
        }
    }

    /// G6: two separable passes; a one-pixel impulse spreads along both axes, symmetrically.
    #[test]
    fn g6_impulse_spreads_in_both_axes() {
        let (w, h) = (41, 41);
        let mut src = vec![0u8; w * h * 4];
        let c = (20 * w + 20) * 4;
        src[c..c + 4].copy_from_slice(&[255, 255, 255, 255]);
        let out = blur_amount(&src, w, h, 0.05, Quality::Full, None);
        let at = |x: usize, y: usize| out[(y * w + x) * 4];
        assert!(at(20, 20) > at(21, 20) && at(21, 20) > 0, "spreads along x");
        assert!(at(20, 21) > 0 && at(21, 21) > 0, "spreads along y and diagonally");
        for d in 1..6 {
            assert_eq!(at(20 - d, 20), at(20 + d, 20));
            assert_eq!(at(20, 20 - d), at(20, 20 + d));
            assert_eq!(at(20 + d, 20), at(20, 20 + d), "separable kernel is square-symmetric");
        }
    }

    /// Premultiplied blur: a transparent neighbour does not darken the colour at an edge.
    #[test]
    fn transparent_pixels_do_not_darken_edges() {
        let (w, h) = (16, 4);
        let mut src = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..8 {
                src[(y * w + x) * 4..][..4].copy_from_slice(&[200, 0, 0, 200]);
            }
        }
        let out = blur_amount(&src, w, h, 0.5, Quality::Full, None);
        for p in out.as_chunks::<4>().0 {
            if p[3] > 20 {
                // Unpremultiplied red stays the same: colour and alpha blur together.
                let r = f32::from(p[0]) / f32::from(p[3]);
                assert!((r - 1.0).abs() < 0.03, "{p:?}");
            }
        }
    }

    /// Clamp-to-edge: a gradient keeps its end values instead of fading towards black.
    #[test]
    fn edges_clamp() {
        let (w, h) = (24, 3);
        let src = [90u8, 90, 90, 255].repeat(w * h);
        let out = blur_amount(&src, w, h, 0.8, Quality::Full, None);
        assert_eq!(&out[..4], &[90, 90, 90, 255]);
    }

    /// Splitting the rows over threads does not change the result.
    #[test]
    fn threaded_result_matches_the_single_thread_one() {
        let (w, h) = (500, 300);
        let src: Vec<u8> = (0..w * h * 4).map(|i| ((i * 7919) % 251) as u8).collect();
        let k = kernel(0.5);
        let threaded = blur(&src, w, h, &k, None);
        // One row at a time through the same row functions, no threads.
        let input: Vec<f32> = src.iter().map(|&v| v.to_f32()).collect();
        let mut mid = vec![0.0f32; input.len()];
        for y in 0..h {
            vertical_row(&input, 0, &mut mid[y * w * 4..(y + 1) * w * 4], y, w, h, &k);
        }
        let mut single = vec![0u8; src.len()];
        let mut padded = Vec::new();
        for y in 0..h {
            horizontal_row(&mid[y * w * 4..(y + 1) * w * 4], &mut single[y * w * 4..(y + 1) * w * 4], w, &k, &mut padded);
        }
        assert_eq!(threaded, single);
    }

    #[test]
    fn progress_reaches_one() {
        let p = Progress::new();
        let src = vec![50u8; 30 * 20 * 4];
        blur_amount(&src, 30, 20, 0.5, Quality::Full, Some(&p));
        assert!((p.fraction() - 1.0).abs() < 1e-6);
    }

    /// The downscaled preview stays close to the full-resolution result on smooth content.
    #[test]
    fn proxy_preview_is_close_to_the_commit() {
        let (w, h) = (800, 600);
        let mut src = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                // A soft gradient with a hard-edged square on top.
                let sq = (300..500).contains(&x) && (200..400).contains(&y);
                let v = if sq { 230 } else { (x * 255 / w) as u8 / 2 };
                src[(y * w + x) * 4..][..4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let a = 0.6;
        assert!(preview_scale(w, h, &kernel(a)) > 1, "this size previews on a proxy");
        let full = blur_amount(&src, w, h, a, Quality::Full, None);
        let prev = blur_amount(&src, w, h, a, Quality::Preview, None);
        let diff: f64 = full.iter().zip(&prev).map(|(a, b)| (f64::from(*a) - f64::from(*b)).abs()).sum::<f64>() / full.len() as f64;
        assert!(diff < 2.5, "mean abs difference {diff}");
    }
}
