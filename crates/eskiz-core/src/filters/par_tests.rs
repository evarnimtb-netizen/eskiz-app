//! Equivalence tests for the threaded and banded filter paths: each new path is compared with
//! the serial whole-image code it replaced (copied here as the baseline) on seeded random input.

use super::gaussian::{self, Kernel};
use super::*;
use crate::pixel::Chan;

/// xorshift64*: a seeded stream of values, so a failure reproduces.
pub(crate) struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Random premultiplied RGBA: colour never above alpha, a share of transparent and opaque pixels.
pub(crate) fn random_pixels<T: Chan>(seed: u64, w: usize, h: usize) -> Vec<T> {
    let mut rng = Rng(seed | 1);
    let mut out = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        let a = match rng.next() % 8 {
            0 => 0.0,
            1 | 2 => 1.0,
            _ => rng.unit(),
        };
        out.extend([rng.unit() * a, rng.unit() * a, rng.unit() * a, a].map(|v| T::from_f32_round(v * T::MAXF)));
    }
    out
}

/// A mask with exact 0 and 1 entries as well as fractions.
pub(crate) fn random_mask(seed: u64, n: usize) -> Vec<f32> {
    let mut rng = Rng(seed | 1);
    (0..n)
        .map(|_| match rng.next() % 5 {
            0 => 0.0,
            1 => 1.0,
            _ => rng.unit(),
        })
        .collect()
}

fn ref_vertical_row(input: &[f32], dst: &mut [f32], y: usize, width: usize, height: usize, k: &Kernel) {
    let row = width * 4;
    let w0 = k.weights[0];
    for (d, s) in dst.iter_mut().zip(&input[y * row..(y + 1) * row]) {
        *d = w0 * s;
    }
    for (i, &w) in k.weights.iter().enumerate().skip(1) {
        let up = &input[y.saturating_sub(i) * row..][..row];
        let down = &input[(y + i).min(height - 1) * row..][..row];
        for ((d, a), b) in dst.iter_mut().zip(up).zip(down) {
            *d += w * (a + b);
        }
    }
}


/// The blur as it was before it ran in bands: whole-image `f32` input and middle pass.
fn ref_blur<T: Chan>(src: &[T], width: usize, height: usize, k: &Kernel, progress: Option<&Progress>) -> Vec<T> {
    if k.taps == 1 || width == 0 || height == 0 {
        return src.to_vec();
    }
    if let Some(p) = progress {
        p.add_total(2 * height);
    }
    let row = width * 4;
    let input: Vec<f32> = src.iter().map(|&v| v.to_f32()).collect();
    let n_workers = gaussian::workers(height, width, k.taps);
    let rows_per = height.div_ceil(n_workers);

    // Pass 1, vertical: each output row mixes the rows `i` above and below (clamped to the edge).
    let mut mid = vec![0.0f32; input.len()];
    std::thread::scope(|scope| {
        for (chunk_index, out) in mid.chunks_mut(rows_per * row).enumerate() {
            let input = &input;
            scope.spawn(move || {
                let first = chunk_index * rows_per;
                for (r, dst) in out.chunks_mut(row).enumerate() {
                    ref_vertical_row(input, dst, first + r, width, height, k);
                    if let Some(p) = progress {
                        p.add_done(1);
                    }
                }
            });
        }
    });
    drop(input);

    // Pass 2, horizontal, on each row of the first pass's output.
    let mut done = vec![T::default(); src.len()];
    std::thread::scope(|scope| {
        for (chunk_index, out) in done.chunks_mut(rows_per * row).enumerate() {
            let mid = &mid;
            scope.spawn(move || {
                let first = chunk_index * rows_per;
                let mut padded = Vec::new();
                for (r, dst) in out.chunks_mut(row).enumerate() {
                    let y = first + r;
                    gaussian::horizontal_row(&mid[y * row..(y + 1) * row], dst, width, k, &mut padded);
                    if let Some(p) = progress {
                        p.add_done(1);
                    }
                }
            });
        }
    });
    done
}


/// The box downscale before it was threaded.
fn ref_downscale<T: Chan>(src: &[T], width: usize, height: usize, k: usize) -> (Vec<T>, usize, usize) {
    let k = k.max(1);
    let (pw, ph) = (width.div_ceil(k), height.div_ceil(k));
    let mut out = vec![T::default(); pw * ph * 4];
    for py in 0..ph {
        let (y0, y1) = (py * k, ((py + 1) * k).min(height));
        for px in 0..pw {
            let (x0, x1) = (px * k, ((px + 1) * k).min(width));
            let mut acc = [0u64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * width + x) * 4;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += src[i + c].raw() as u64;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = (py * pw + px) * 4;
            for (c, a) in acc.iter().enumerate() {
                out[o + c] = T::from_f32_round(((a + n / 2) / n) as f32);
            }
        }
    }
    (out, pw, ph)
}

/// The bilinear upscale before it was threaded.
fn ref_upscale<T: Chan>(src: &[T], pw: usize, ph: usize, width: usize, height: usize, k: usize) -> Vec<T> {
    let kf = k.max(1) as f32;
    let mut out = vec![T::default(); width * height * 4];
    for y in 0..height {
        let fy = ((y as f32 + 0.5) / kf - 0.5).clamp(0.0, (ph - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(ph - 1);
        for x in 0..width {
            let fx = ((x as f32 + 0.5) / kf - 0.5).clamp(0.0, (pw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(pw - 1);
            let o = (y * width + x) * 4;
            for c in 0..4 {
                let at = |xx: usize, yy: usize| src[(yy * pw + xx) * 4 + c].to_f32();
                let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
                let bot = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
                out[o + c] = T::from_f32_round(top + (bot - top) * ty);
            }
        }
    }
    out
}

fn blur_matches<T: Chan + std::fmt::Debug>(seed: u64, w: usize, h: usize, amount: f64) {
    let src = random_pixels::<T>(seed, w, h);
    let k = gaussian::kernel(amount);
    assert_eq!(gaussian::blur(&src, w, h, &k, None), ref_blur(&src, w, h, &k, None), "{w}x{h} amount {amount}");
}

/// The banded blur gives the same bytes as the whole-image blur: 8 and 16 bits, kernels longer
/// than the image is tall, odd sizes, bands that do not divide the height, and the threaded sizes.
#[test]
fn banded_blur_equals_the_whole_image_blur() {
    for (i, &(w, h)) in [(1, 1), (7, 5), (97, 83), (64, 200), (33, 31), (300, 230)].iter().enumerate() {
        for amount in [0.05, 0.3, 1.0] {
            blur_matches::<u8>(10 + i as u64, w, h, amount);
            blur_matches::<u16>(20 + i as u64, w, h, amount);
        }
    }
}

#[test]
fn blur_progress_reaches_one() {
    let src = random_pixels::<u8>(5, 40, 70);
    let p = Progress::new();
    let _ = gaussian::blur(&src, 40, 70, &gaussian::kernel(0.5), Some(&p));
    assert!((p.fraction() - 1.0).abs() < 1e-6);
}

#[test]
fn threaded_downscale_and_upscale_equal_the_serial_ones() {
    for (i, &(w, h, k)) in [(10, 7, 1), (97, 83, 2), (301, 229, 3), (640, 480, 5), (13, 400, 4)].iter().enumerate() {
        let src8 = random_pixels::<u8>(30 + i as u64, w, h);
        let (got, pw, ph) = downscale(&src8, w, h, k);
        let (want, rw, rh) = ref_downscale(&src8, w, h, k);
        assert_eq!((got.as_slice(), pw, ph), (want.as_slice(), rw, rh), "downscale {w}x{h} k{k}");
        assert_eq!(upscale(&got, pw, ph, w, h, k), ref_upscale(&got, pw, ph, w, h, k), "upscale {w}x{h} k{k}");
        let src16 = random_pixels::<u16>(40 + i as u64, w, h);
        let (got, pw, ph) = downscale(&src16, w, h, k);
        assert_eq!(got, ref_downscale(&src16, w, h, k).0);
        assert_eq!(upscale(&got, pw, ph, w, h, k), ref_upscale(&got, pw, ph, w, h, k));
    }
}

/// A pointwise function over a threaded buffer equals the same function over its pieces.
fn piecewise<T: Chan + std::fmt::Debug>(src: &[T], f: impl Fn(&[T]) -> Vec<T>) {
    let whole = f(src);
    let mut pieces = Vec::new();
    for c in src.chunks(4 * 1000) {
        pieces.extend(f(c));
    }
    assert_eq!(whole, pieces);
}

#[test]
fn threaded_hsb_opacity_equal_the_serial_ones() {
    let (w, h) = (300, 230);
    let a8 = random_pixels::<u8>(1, w, h);
    let a16 = random_pixels::<u16>(2, w, h);
    for (hue, sat, bri) in [(0.3, 0.0, 0.0), (0.0, -0.4, 0.2), (-0.7, 0.5, -0.3)] {
        piecewise(&a8, |s| hsb::apply_t(s, hue, sat, bri));
        piecewise(&a16, |s| hsb::apply_t(s, hue, sat, bri));
    }
    piecewise(&a8, |s| opacity::apply_t(s, 0.37));
    piecewise(&a16, |s| opacity::apply_t(s, 0.81));
}

#[test]
fn threaded_mix_and_alpha_lock_equal_the_serial_ones() {
    let (w, h) = (300, 230);
    let src = random_pixels::<u8>(3, w, h);
    let other = random_pixels::<u8>(4, w, h);
    let mask = random_mask(5, w * h);
    let mut got = other.clone();
    mix_bytes(&src, &mut got, Some(&mask));
    // The serial rule, pixel by pixel.
    let mut want = other.clone();
    for (i, m) in mask.iter().enumerate() {
        let t = m.clamp(0.0, 1.0);
        if t >= 1.0 {
            continue;
        }
        for k in i * 4..i * 4 + 4 {
            let (s, o) = (f32::from(src[k]), f32::from(other[k]));
            want[k] = u8::from_f32_round(s + (o - s) * t);
        }
    }
    assert_eq!(got, want);
    let mut locked = other.clone();
    keep_alpha(&src, &mut locked);
    let mut serial = Vec::new();
    for (s, o) in src.chunks(4 * 500).zip(other.chunks(4 * 500)) {
        let mut piece = o.to_vec();
        keep_alpha(s, &mut piece);
        serial.extend(piece);
    }
    assert_eq!(locked, serial);
}

fn curve_adjustment() -> Adjustment {
    let mut adj = Adjustment::new(AdjustKind::Curves);
    let mut pts = CurvePoints::default();
    pts.channels[0] = vec![[0.0, 0.1], [0.4, 0.3], [1.0, 0.95]];
    pts.channels[2] = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
    adj.set_curve_points(pts);
    adj
}

fn pointwise_adjustments() -> Vec<Adjustment> {
    let mut v = vec![curve_adjustment()];
    let mut gm = Adjustment::new(AdjustKind::GradientMap);
    gm.set(0, 0.8);
    v.push(gm);
    let mut rc = Adjustment::new(AdjustKind::Recolor);
    for (i, x) in [0.8, 0.3, 0.2, 0.1, 0.5, 0.9, 0.6].into_iter().enumerate() {
        rc.set(i, x);
    }
    v.push(rc);
    let mut cb = Adjustment::new(AdjustKind::ColourBalance);
    for (i, x) in [0.3, -0.2, 0.1, 0.0, 0.4, -0.5, -0.3, 0.2, 0.6].into_iter().enumerate() {
        cb.set(i, x);
    }
    v.push(cb);
    v
}

/// Strip-wise rendering of the pointwise types equals the whole-image `f32` path: 8 and 16
/// bits, with and without a mask (a selection or Pencil paint), for sizes that are not a
/// multiple of the strip height and large enough to be threaded.
#[test]
fn strip_rendering_equals_the_whole_image_path() {
    for (i, &(w, h)) in [(70, 150), (300, 230), (5, 3)].iter().enumerate() {
        let s8 = random_pixels::<u8>(50 + i as u64, w, h);
        let s16 = random_pixels::<u16>(60 + i as u64, w, h);
        let mask = random_mask(70 + i as u64, w * h);
        for adj in pointwise_adjustments() {
            for m in [None, Some(mask.as_slice())] {
                for q in [Quality::Preview, Quality::Full] {
                    assert_eq!(bridge::render(&adj, &s8, w, h, q, m), bridge::render_whole(&adj, &s8, w, h, q, m), "{:?} u8 {w}x{h} mask {}", adj.kind, m.is_some());
                    assert_eq!(bridge::render(&adj, &s16, w, h, q, m), bridge::render_whole(&adj, &s16, w, h, q, m), "{:?} u16 {w}x{h}", adj.kind);
                }
            }
        }
    }
}

/// With lock transparency the committed result still keeps the layer's coverage (the lock runs after
/// the render, on the threaded `keep_alpha`).
#[test]
fn strip_render_with_alpha_lock_keeps_coverage() {
    let (w, h) = (300, 230);
    let src = random_pixels::<u8>(80, w, h);
    let mut out = bridge::render(&curve_adjustment(), &src, w, h, Quality::Full, None);
    keep_alpha(&src, &mut out);
    assert!(src.as_chunks::<4>().0.iter().zip(out.as_chunks::<4>().0).all(|(a, b)| a[3] == b[3]));
}

// ---- proxies ----

#[test]
fn proxy_factor_follows_size_zoom_and_type() {
    use AdjustKind::*;
    assert_eq!(proxy_factor(Hsb, false, 2048, 2048, 0.2), 1, "small canvases render in full");
    assert_eq!(proxy_factor(Hsb, false, 8000, 8000, 0.15), 5);
    assert_eq!(proxy_factor(Curves, false, 4096, 4096, 0.3), 3);
    assert_eq!(proxy_factor(Curves, false, 8000, 8000, 1.0), 1, "at 100% the preview is full size");
    assert_eq!(proxy_factor(Curves, false, 8000, 8000, 0.5), 2, "never coarser than the screen shows");
    assert_eq!(proxy_factor(Sharpen, false, 8000, 8000, 0.1), 1, "pixel-sized parameters do not use a proxy");
    assert_eq!(proxy_factor(GaussianBlur, true, 8000, 8000, 0.1), 1, "the Pencil pyramid blur does not either");
    assert_eq!(proxy_factor(GaussianBlur, false, 8000, 8000, 0.1), 5);
}

fn flat(w: usize, h: usize, px: [u8; 4]) -> Vec<u8> {
    (0..w * h).flat_map(|_| px).collect()
}

/// On a flat layer a proxy preview is exactly the full-size render of the same colour, whatever
/// the factor (the pointwise types are exact; the blur of a flat colour does not change it).
#[test]
fn proxy_preview_of_a_flat_layer_equals_the_full_render() {
    let (w, h) = (203, 157);
    let src = flat(w, h, [120, 60, 30, 200]);
    let mut hsb_adj = Adjustment::new(AdjustKind::Hsb);
    hsb_adj.set(0, 0.2);
    hsb_adj.set(2, 0.1);
    let mut blur = Adjustment::new(AdjustKind::GaussianBlur);
    blur.set(0, 0.6);
    for adj in [hsb_adj, curve_adjustment(), blur] {
        for k in [2, 3, 5] {
            let proxy = Proxy::build(&src, None, w, h, k);
            let got = render_preview(&adj, &proxy, None, w, h).to_rgba();
            let want = adj.render(&src, w, h, Quality::Full, None, None);
            assert_eq!(got, want, "{:?} k{k}", adj.kind);
        }
    }
}

/// A selection (and paint) mask is downscaled with the layer: outside the mask the preview is the
/// source, and a half mask lands halfway.
#[test]
fn proxy_preview_honours_selection_and_paint() {
    let (w, h) = (200, 120);
    let src = flat(w, h, [100, 100, 100, 255]);
    let mut adj = Adjustment::new(AdjustKind::Hsb);
    adj.set(2, 0.3);
    let full = adj.render(&src, w, h, Quality::Full, None, None);
    assert_ne!(full, src);
    // Left half selected.
    let sel: Vec<f32> = (0..w * h).map(|i| if i % w < w / 2 { 1.0 } else { 0.0 }).collect();
    let proxy = Proxy::build(&src, Some(&sel), w, h, 4);
    let out = render_preview(&adj, &proxy, None, w, h).to_rgba();
    assert_eq!(&out[..4], &full[..4], "inside the selection");
    assert_eq!(&out[(w - 1) * 4..w * 4], &src[(w - 1) * 4..w * 4], "outside the selection");
    // Paint covering the right half too: the product leaves only the left half changed.
    let paint = vec![1.0f32; w * h];
    assert_eq!(render_preview(&adj, &proxy, Some(&paint), w, h).to_rgba(), out);
    // No paint at all in Pencil mode: nothing changes.
    let zero = vec![0.0f32; w * h];
    assert_eq!(render_preview(&adj, &proxy, Some(&zero), w, h).to_rgba(), src);
}

#[test]
fn downscale_mask_averages_blocks() {
    let m: Vec<f32> = (0..10 * 6).map(|i| (i % 10) as f32).collect();
    let (d, pw, ph) = proxy::downscale_mask(&m, 10, 6, 4);
    assert_eq!((pw, ph), (3, 2));
    assert!((d[0] - 1.5).abs() < 1e-6 && (d[1] - 5.5).abs() < 1e-6 && (d[2] - 8.5).abs() < 1e-6);
}

/// The tile-wise scale-up repeats each proxy pixel as a k x k block (so it equals the plain
/// per-pixel lookup), over tile boundaries and ragged edges, and a transparent proxy allocates no
/// tiles.
#[test]
fn upscale_layer_repeats_blocks() {
    for (i, &(w, h, k)) in [(300usize, 270usize, 3usize), (515, 130, 5), (257, 257, 2), (40, 30, 1)].iter().enumerate() {
        let (pw, ph) = (w.div_ceil(k), h.div_ceil(k));
        let small = random_pixels::<u8>(90 + i as u64, pw, ph);
        let got = proxy::upscale_layer(&small, pw, ph, w, h, k).to_rgba();
        for y in 0..h {
            for x in 0..w {
                let s = ((y / k) * pw + x / k) * 4;
                assert_eq!(got[(y * w + x) * 4..][..4], small[s..s + 4], "{w}x{h} k{k} at {x},{y}");
            }
        }
    }
    let empty = proxy::upscale_layer(&vec![0u8; 10 * 10 * 4], 10, 10, 600, 600, 60);
    assert_eq!(empty.tile_count(), 0);
}

/// The Curves table path (8-bit) gives the bytes of the `f32` path for every (alpha, value) pair,
/// including values above alpha, with and without a mask.
#[test]
fn curves_table_path_equals_the_float_path_for_every_pixel_value() {
    let adj = curve_adjustment();
    let mut src = Vec::new();
    for a in 0..=255u8 {
        for v in (0..=255u8).step_by(3) {
            src.extend([v, 255 - v, v / 2, a]);
        }
    }
    let n = src.len() / 4;
    let mask = random_mask(12, n);
    for m in [None, Some(mask.as_slice())] {
        assert_eq!(bridge::render(&adj, &src, n, 1, Quality::Full, m), bridge::render_whole(&adj, &src, n, 1, Quality::Full, m), "mask {}", m.is_some());
    }
}
