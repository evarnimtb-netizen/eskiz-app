//! Resampling with nearest, bilinear and cubic B-spline kernels.

use crate::geom::{IRect, Mat3, Point, Rect};
use crate::image::{Image, Rgba};
use crate::mask::Mask;

/// Interpolation used when resampling. Bicubic is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interp {
    Nearest,
    Bilinear,
    #[default]
    Bicubic,
}

/// Resamples `src` into a `dst_width` by `dst_height` image.
///
/// `mapping` takes source coordinates to destination coordinates in continuous pixel space.
/// Each destination pixel centre is pulled back through the inverse, then sampled with the
/// pixel-centre convention: source index `i` has its centre at `i + 0.5`. Colour is
/// premultiplied before sampling and un-premultiplied afterwards. Samples outside the source
/// are transparent. A singular mapping gives a transparent result.
///
/// Only the destination rows and columns that the transformed content can reach are sampled
/// (the rest is transparent either way), and rows run on several threads.
pub fn resample(
    src: &Image,
    mapping: &Mat3,
    dst_width: usize,
    dst_height: usize,
    interp: Interp,
) -> Image {
    let bounds = src.alpha_bounds();
    resample_bounded(&Sampler::new(src), bounds, mapping, dst_width, dst_height, interp)
}

/// [`resample`] for a prepared sampler whose content lies inside `bounds` (`None`: it has none).
pub(crate) fn resample_bounded(
    sampler: &Sampler<'_>,
    bounds: Option<IRect>,
    mapping: &Mat3,
    dst_width: usize,
    dst_height: usize,
    interp: Interp,
) -> Image {
    let mut out = Image::transparent(dst_width, dst_height);
    let (Some(bounds), Some(inverse)) = (bounds, mapping.inverse()) else {
        return out;
    };
    let region = dest_region(mapping, bounds, dst_width, dst_height);
    if region.is_empty() {
        return out;
    }
    let rows = &mut out.pixels[region.y * dst_width..region.y1() * dst_width];
    par_rows(rows, dst_width, |first, band| {
        for (r, row) in band.chunks_mut(dst_width).enumerate() {
            let y = region.y + first + r;
            for (x, dst) in row.iter_mut().enumerate().take(region.x1()).skip(region.x) {
                let centre = Point::new(x as f32 + 0.5, y as f32 + 0.5);
                if let Some(p) = map_point(&inverse, centre) {
                    *dst = sampler.sample(p, interp);
                }
            }
        }
    });
    out
}

/// `m.apply(p)` without the divide when the matrix is affine with a unit bottom row, which
/// leaves `x / 1.0` and `y / 1.0` (bit for bit the same).
#[inline]
pub(crate) fn map_point(m: &Mat3, p: Point) -> Option<Point> {
    let r = &m.m;
    if r[2][0] == 0.0 && r[2][1] == 0.0 && r[2][2] == 1.0 && p.x.is_finite() && p.y.is_finite() {
        return Some(Point::new(r[0][0] * p.x + r[0][1] * p.y + r[0][2], r[1][0] * p.x + r[1][1] * p.y + r[1][2]));
    }
    m.apply(p)
}

/// Source pixels past the content that a kernel can still reach (bicubic: 2).
const KERNEL_REACH: f32 = 3.0;

/// The destination rectangle that content inside `bounds` can reach through `mapping`: the
/// whole destination for a projective mapping, else the bounding box of the mapped (and grown)
/// content rectangle.
pub(crate) fn dest_region(mapping: &Mat3, bounds: IRect, dst_width: usize, dst_height: usize) -> IRect {
    let full = IRect { x: 0, y: 0, w: dst_width, h: dst_height };
    let m = &mapping.m;
    if m[2][0] != 0.0 || m[2][1] != 0.0 || !(m[2][2].is_finite() && m[2][2] != 0.0) {
        return full;
    }
    let rect = Rect::from_irect(bounds);
    let (x0, y0, x1, y1) = (rect.x0 - KERNEL_REACH, rect.y0 - KERNEL_REACH, rect.x1 + KERNEL_REACH, rect.y1 + KERNEL_REACH);
    let mut lo = [f32::INFINITY; 2];
    let mut hi = [f32::NEG_INFINITY; 2];
    for c in [Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)] {
        let Some(p) = mapping.apply(c) else {
            return full;
        };
        if !(p.x.is_finite() && p.y.is_finite()) {
            return full;
        }
        lo = [lo[0].min(p.x), lo[1].min(p.y)];
        hi = [hi[0].max(p.x), hi[1].max(p.y)];
    }
    // Pixel centres sit at +0.5; one more pixel of slack for rounding.
    let clamp = |v: f32, max: usize| (v.max(0.0) as usize).min(max);
    let (cx0, cy0) = (clamp((lo[0] - 1.5).floor(), dst_width), clamp((lo[1] - 1.5).floor(), dst_height));
    let (cx1, cy1) = (clamp((hi[0] + 1.5).ceil(), dst_width), clamp((hi[1] + 1.5).ceil(), dst_height));
    IRect::from_bounds(cx0, cy0, cx1, cy1)
}

/// Run `f(first_row, band)` over bands of whole rows (`row_len` items each) on scoped threads.
pub(crate) fn par_rows<T: Send>(out: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = out.len().checked_div(row_len).unwrap_or(0);
    let threads = if out.len() < 1 << 16 {
        1
    } else {
        worker_cpus().clamp(1, 8).min(rows.max(1))
    };
    if threads <= 1 {
        f(0, out);
        return;
    }
    let per = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        for (i, band) in out.chunks_mut(per * row_len).enumerate() {
            let f = &f;
            scope.spawn(move || f(i * per, band));
        }
    });
}

/// Resamples a mask through `mapping` with the same conventions as [`resample`]. Samples that
/// fall outside the mask are left out of the average rather than counted as 0, so the result
/// is normalised over the samples that exist.
pub fn resample_mask(
    mask: &Mask,
    mapping: &Mat3,
    dst_width: usize,
    dst_height: usize,
    interp: Interp,
) -> Mask {
    let sampler = Sampler::from_mask(mask);
    let bounds = (mask.width > 0 && mask.height > 0).then_some(IRect { x: 0, y: 0, w: mask.width, h: mask.height });
    // Only the red channel matters: the mask's values are carried as grey with alpha 1.
    let resampled = resample_bounded(&sampler, bounds, mapping, dst_width, dst_height, interp);
    mask_from_image(&resampled, dst_width, dst_height)
}

/// Carries each mask value in the red channel of an opaque pixel, so the image kernels apply.
#[cfg(test)]
pub(crate) fn mask_as_image(mask: &Mask) -> Image {
    Image::from_fn(mask.width, mask.height, |x, y| {
        let v = mask.data[y * mask.width + x];
        [v, v, v, 1.0]
    })
}

/// Reads the red channel of a resampled image back into a mask.
pub(crate) fn mask_from_image(image: &Image, width: usize, height: usize) -> Mask {
    Mask {
        width,
        height,
        data: image.pixels.iter().map(|p| p[0].clamp(0.0, 1.0)).collect(),
    }
}

/// A source prepared for sampling, with out-of-range taps transparent. Shared by [`resample`]
/// and the mesh warp so both use identical kernels.
pub(crate) struct Sampler<'a> {
    width: usize,
    height: usize,
    taps: Taps<'a>,
}

#[allow(clippy::large_enum_variant)] // one short-lived value per resample; boxing would add a hop per tap
enum Taps<'a> {
    /// Premultiplied once, up front.
    Float(Vec<Rgba>),
    /// Premultiplied RGBA8 read in place. Each tap is converted to straight float and back to
    /// premultiplied exactly as `Image::from_premul_rgba8` + `Sampler::new` would, through a
    /// table over (alpha, value); where `scale` is below one the pixel's alpha is multiplied by
    /// it first (a selection that fades the source), computed per tap.
    Bytes { pixels: &'a [u8], scale: Option<&'a [f32]>, lut: Box<[f32]>, alpha: [f32; 256] },
    /// A mask's values as grey with alpha 1.
    Mask(&'a [f32]),
}

impl<'a> Sampler<'a> {
    pub(crate) fn new(src: &Image) -> Self {
        let premultiplied = src
            .pixels
            .iter()
            .map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]])
            .collect();
        Self {
            width: src.width,
            height: src.height,
            taps: Taps::Float(premultiplied),
        }
    }

    /// Samples premultiplied RGBA8 `pixels` (`width * height * 4` bytes) without copying them.
    /// `scale`, when given, multiplies each pixel's alpha (`width * height` values).
    pub(crate) fn from_premul_u8(width: usize, height: usize, pixels: &'a [u8], scale: Option<&'a [f32]>) -> Self {
        let mut lut = vec![0.0f32; 256 * 256].into_boxed_slice();
        for a in 1..256usize {
            let af = a as f32 / 255.0;
            for v in 0..256usize {
                let c = (v as f32 / 255.0 / af).clamp(0.0, 1.0);
                lut[(a << 8) | v] = c * af;
            }
        }
        let alpha = std::array::from_fn(|a| a as f32 / 255.0);
        Self { width, height, taps: Taps::Bytes { pixels, scale, lut, alpha } }
    }

    pub(crate) fn from_mask(mask: &'a Mask) -> Self {
        Self { width: mask.width, height: mask.height, taps: Taps::Mask(&mask.data) }
    }

    #[inline]
    fn tap(&self, x: i64, y: i64) -> Rgba {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return [0.0; 4];
        }
        self.tap_inside(x as usize, y as usize)
    }

    /// [`Self::tap`] for a pixel known to be inside the image.
    #[inline]
    fn tap_inside(&self, x: usize, y: usize) -> Rgba {
        let i = y * self.width + x;
        match &self.taps {
            Taps::Float(px) => px[i],
            Taps::Mask(m) => {
                let v = m[i];
                [v, v, v, 1.0]
            }
            Taps::Bytes { pixels, scale, lut, .. } => {
                let p = &pixels[i * 4..i * 4 + 4];
                let a = usize::from(p[3]);
                if a == 0 {
                    return [0.0; 4];
                }
                match scale.map(|s| s[i]).filter(|&s| s != 1.0) {
                    None => {
                        let row = &lut[a << 8..(a << 8) + 256];
                        [row[usize::from(p[0])], row[usize::from(p[1])], row[usize::from(p[2])], a as f32 / 255.0]
                    }
                    Some(s) => {
                        let af = a as f32 / 255.0;
                        let scaled = af * s;
                        let c = |v: u8| (f32::from(v) / 255.0 / af).clamp(0.0, 1.0) * scaled;
                        [c(p[0]), c(p[1]), c(p[2]), scaled]
                    }
                }
            }
        }
    }

    /// Straight-colour sample at source continuous coordinates `p`.
    pub(crate) fn sample(&self, p: Point, interp: Interp) -> Rgba {
        let px = match interp {
            Interp::Nearest => self.tap(p.x.floor() as i64, p.y.floor() as i64),
            Interp::Bilinear | Interp::Bicubic => {
                // Taps reach one pixel before and two after the base texel; inside that margin
                // there is no bounds check per tap.
                let (x0, y0) = ((p.x - 0.5).floor(), (p.y - 0.5).floor());
                let interior = x0 >= 1.0 && y0 >= 1.0 && x0 + 2.0 < self.width as f32 && y0 + 2.0 < self.height as f32;
                if let (true, Taps::Bytes { pixels, scale: None, lut, alpha }) = (interior, &self.taps) {
                    return unpremultiply(bytes_kernel(pixels, lut, alpha, self.width, p, interp == Interp::Bicubic));
                }
                match (interp, interior) {
                    (Interp::Bilinear, true) => bilinear(&|x, y| self.tap_inside(x as usize, y as usize), p),
                    (Interp::Bilinear, false) => bilinear(&|x, y| self.tap(x, y), p),
                    (_, true) => bicubic(&|x, y| self.tap_inside(x as usize, y as usize), p),
                    (_, false) => bicubic(&|x, y| self.tap(x, y), p),
                }
            }
        };
        unpremultiply(px)
    }
}

/// Bilinear or bicubic taps over premultiplied RGBA8 read in place, for a point whose whole
/// neighbourhood (one pixel before, two after) is inside the image and a source with no alpha
/// scale. The same weights, taps order and sums as [`bilinear`] and [`bicubic`] over
/// [`Sampler::tap_inside`]; taps that cannot add anything (transparent, or weight zero) are
/// skipped, which leaves every sum unchanged.
#[inline]
/// CPUs for worker pools; Android caps at 4 (phones throttle on many threads).
fn worker_cpus() -> usize {
    let n = std::thread::available_parallelism().map_or(1, usize::from);
    #[cfg(target_os = "android")]
    let n = n.min(4);
    n
}

fn bytes_kernel(pixels: &[u8], lut: &[f32], alpha: &[f32; 256], width: usize, p: Point, cubic: bool) -> Rgba {
    let (fx, fy) = (p.x - 0.5, p.y - 0.5);
    let (x0f, y0f) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0f, fy - y0f);
    let (x0, y0) = (x0f as usize, y0f as usize);
    let mut acc = [0.0f32; 4];
    let mut add = |i: usize, w: f32| {
        let q = &pixels[i * 4..i * 4 + 4];
        let a = usize::from(q[3]);
        if a == 0 || w == 0.0 {
            return;
        }
        let row = &lut[a << 8..(a << 8) + 256];
        acc[0] += row[usize::from(q[0])] * w;
        acc[1] += row[usize::from(q[1])] * w;
        acc[2] += row[usize::from(q[2])] * w;
        acc[3] += alpha[a] * w;
    };
    if cubic {
        let wxs = [-1.0f32, 0.0, 1.0, 2.0].map(|i| bspline(i - tx));
        for (j, dj) in (-1..=2i64).enumerate() {
            let wy = bspline(dj as f32 - ty);
            let base = (y0 + j - 1) * width + x0 - 1;
            for (i, wx) in wxs.iter().enumerate() {
                add(base + i, wx * wy);
            }
        }
    } else {
        let base = y0 * width + x0;
        add(base, (1.0 - tx) * (1.0 - ty));
        add(base + 1, tx * (1.0 - ty));
        add(base + width, (1.0 - tx) * ty);
        add(base + width + 1, tx * ty);
    }
    acc
}

/// Bilinear taps: the four source pixels around the point.
fn bilinear(sample: &impl Fn(i64, i64) -> Rgba, p: Point) -> Rgba {
    let (fx, fy) = (p.x - 0.5, p.y - 0.5);
    let (x0f, y0f) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0f, fy - y0f);
    let (x0, y0) = (x0f as i64, y0f as i64);
    let mut acc = [0.0f32; 4];
    for (px, w) in [
        (sample(x0, y0), (1.0 - tx) * (1.0 - ty)),
        (sample(x0 + 1, y0), tx * (1.0 - ty)),
        (sample(x0, y0 + 1), (1.0 - tx) * ty),
        (sample(x0 + 1, y0 + 1), tx * ty),
    ] {
        for (a, v) in acc.iter_mut().zip(px) {
            *a += v * w;
        }
    }
    acc
}

/// Bicubic taps: a 4 x 4 neighbourhood weighted by the cubic B-spline kernel.
fn bicubic(sample: &impl Fn(i64, i64) -> Rgba, p: Point) -> Rgba {
    let (fx, fy) = (p.x - 0.5, p.y - 0.5);
    let (x0f, y0f) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0f, fy - y0f);
    let (x0, y0) = (x0f as i64, y0f as i64);
    let wxs = [-1.0f32, 0.0, 1.0, 2.0].map(|i| bspline(i - tx));
    let mut acc = [0.0f32; 4];
    for j in -1..=2i64 {
        let wy = bspline(j as f32 - ty);
        for (i, wx) in (-1..=2i64).zip(wxs) {
            let w = wx * wy;
            let px = sample(x0 + i, y0 + j);
            for (a, v) in acc.iter_mut().zip(px) {
                *a += v * w;
            }
        }
    }
    acc
}

/// Cubic B-spline kernel. Weights are non-negative and sum to 1 over the four taps.
fn bspline(d: f32) -> f32 {
    let d = d.abs();
    if d < 1.0 {
        (2.0 / 3.0) - d * d + d * d * d * 0.5
    } else if d < 2.0 {
        let t = 2.0 - d;
        t * t * t / 6.0
    } else {
        0.0
    }
}

/// Converts a premultiplied sample back to straight colour, clamping every channel to `[0, 1]`.
fn unpremultiply(p: Rgba) -> Rgba {
    let a = p[3].clamp(0.0, 1.0);
    if a <= 0.0 || !a.is_finite() {
        return [0.0; 4];
    }
    let c = |v: f32| (v / a).clamp(0.0, 1.0);
    [c(p[0]), c(p[1]), c(p[2]), a]
}

#[cfg(test)]
mod tests {
    //! The resampler against the single-threaded, whole-canvas, allocating version it replaced.
    use super::*;

    mod reference {
        use super::super::{Interp, Point, Rgba, Image, Mat3};
        /// A source prepared for sampling: premultiplied once, with out-of-range taps transparent.
        /// Shared by [`resample`] and the mesh warp so both use identical kernels.
        struct RefSampler {
            width: usize,
            height: usize,
            premultiplied: Vec<Rgba>,
        }

        impl RefSampler {
            fn new(src: &Image) -> Self {
                let premultiplied = src
                    .pixels
                    .iter()
                    .map(|p| [p[0] * p[3], p[1] * p[3], p[2] * p[3], p[3]])
                    .collect();
                Self {
                    width: src.width,
                    height: src.height,
                    premultiplied,
                }
            }

            fn tap(&self, x: i64, y: i64) -> Rgba {
                if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
                    return [0.0; 4];
                }
                self.premultiplied[y as usize * self.width + x as usize]
            }

            /// Straight-colour sample at source continuous coordinates `p`.
            fn sample(&self, p: Point, interp: Interp) -> Rgba {
                let tap = |x: i64, y: i64| self.tap(x, y);
                let px = match interp {
                    Interp::Nearest => self.tap(p.x.floor() as i64, p.y.floor() as i64),
                    Interp::Bilinear => ref_bilinear(&tap, p),
                    Interp::Bicubic => ref_bicubic(&tap, p),
                };
                ref_unpremultiply(px)
            }
        }

        /// Bilinear taps: the four source pixels around the point.
        fn ref_bilinear(sample: &dyn Fn(i64, i64) -> Rgba, p: Point) -> Rgba {
            let (fx, fy) = (p.x - 0.5, p.y - 0.5);
            let (x0f, y0f) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0f, fy - y0f);
            let (x0, y0) = (x0f as i64, y0f as i64);
            ref_weighted(&[
                (sample(x0, y0), (1.0 - tx) * (1.0 - ty)),
                (sample(x0 + 1, y0), tx * (1.0 - ty)),
                (sample(x0, y0 + 1), (1.0 - tx) * ty),
                (sample(x0 + 1, y0 + 1), tx * ty),
            ])
        }

        /// Bicubic taps: a 4 x 4 neighbourhood weighted by the cubic B-spline kernel.
        fn ref_bicubic(sample: &dyn Fn(i64, i64) -> Rgba, p: Point) -> Rgba {
            let (fx, fy) = (p.x - 0.5, p.y - 0.5);
            let (x0f, y0f) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0f, fy - y0f);
            let (x0, y0) = (x0f as i64, y0f as i64);
            let mut taps = Vec::with_capacity(16);
            for j in -1..=2i64 {
                let wy = ref_bspline(j as f32 - ty);
                for i in -1..=2i64 {
                    let wx = ref_bspline(i as f32 - tx);
                    taps.push((sample(x0 + i, y0 + j), wx * wy));
                }
            }
            ref_weighted(&taps)
        }

        /// Cubic B-spline kernel. Weights are non-negative and sum to 1 over the four taps.
        fn ref_bspline(d: f32) -> f32 {
            let d = d.abs();
            if d < 1.0 {
                (2.0 / 3.0) - d * d + d * d * d * 0.5
            } else if d < 2.0 {
                let t = 2.0 - d;
                t * t * t / 6.0
            } else {
                0.0
            }
        }

        fn ref_weighted(taps: &[(Rgba, f32)]) -> Rgba {
            let mut acc = [0.0f32; 4];
            for (px, w) in taps {
                for (a, v) in acc.iter_mut().zip(px) {
                    *a += v * w;
                }
            }
            acc
        }

        /// Converts a premultiplied sample back to straight colour, clamping every channel to `[0, 1]`.
        fn ref_unpremultiply(p: Rgba) -> Rgba {
            let a = p[3].clamp(0.0, 1.0);
            if a <= 0.0 || !a.is_finite() {
                return [0.0; 4];
            }
            let c = |v: f32| (v / a).clamp(0.0, 1.0);
            [c(p[0]), c(p[1]), c(p[2]), a]
        }

        pub fn resample(src: &Image, mapping: &Mat3, dw: usize, dh: usize, interp: Interp) -> Image {
            let mut out = Image::transparent(dw, dh);
            let Some(inverse) = mapping.inverse() else {
                return out;
            };
            let sampler = RefSampler::new(src);
            for y in 0..dh {
                for x in 0..dw {
                    let centre = Point::new(x as f32 + 0.5, y as f32 + 0.5);
                    let Some(p) = inverse.apply(centre) else {
                        continue;
                    };
                    out.pixels[y * dw + x] = sampler.sample(p, interp);
                }
            }
            out
        }
    }

    /// xorshift stream so a failure reproduces.
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

    /// A straight-alpha image with transparent borders, partial and full alpha.
    pub(crate) fn random_image(seed: u64, w: usize, h: usize) -> Image {
        let mut rng = Rng(seed | 1);
        Image::from_fn(w, h, |x, y| {
            let edge = x < 2 || y < 2 || x + 3 > w || y + 3 > h;
            let a = if edge { 0.0 } else { [1.0, 1.0, rng.unit(), 0.0][(rng.next() % 4) as usize] };
            if a == 0.0 { [0.0; 4] } else { [rng.unit(), rng.unit(), rng.unit(), a] }
        })
    }

    pub(crate) fn mappings() -> Vec<Mat3> {
        let rot = |deg: f32| Mat3::translate(40.0, 30.0) * Mat3::rotate_deg(deg) * Mat3::translate(-20.0, -15.0);
        vec![
            Mat3::IDENTITY,
            Mat3::translate(7.3, -4.9),
            rot(15.0),
            rot(-77.0) * Mat3::scale(1.3, 0.8),
            Mat3::scale(0.37, 0.41),
            Mat3::scale(-1.0, 1.0) * Mat3::translate(-60.0, 0.0),
            Mat3 { m: [[1.0, 0.1, 3.0], [0.05, 0.9, 1.0], [0.0004, 0.0002, 1.0]] },
            Mat3 { m: [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 0.0]] },
        ]
    }

    #[test]
    fn resample_equals_the_reference() {
        let img = random_image(7, 61, 47);
        for (i, m) in mappings().iter().enumerate() {
            for interp in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
                let want = reference::resample(&img, m, 83, 71, interp);
                assert!(resample(&img, m, 83, 71, interp) == want, "mapping {i} {interp:?}");
            }
        }
    }

    /// Large enough that the rows run on several threads.
    #[test]
    fn threaded_resample_equals_the_reference() {
        let img = random_image(9, 300, 260);
        let m = Mat3::translate(150.0, 130.0) * Mat3::rotate_deg(28.6) * Mat3::scale(1.1, 1.1) * Mat3::translate(-150.0, -130.0);
        for interp in [Interp::Bilinear, Interp::Bicubic] {
            assert!(resample(&img, &m, 320, 300, interp) == reference::resample(&img, &m, 320, 300, interp));
        }
    }

    #[test]
    fn resample_mask_equals_the_reference() {
        let mut rng = Rng(5);
        let mask = Mask { width: 53, height: 41, data: (0..53 * 41).map(|_| [0.0, 1.0, rng.unit()][(rng.next() % 3) as usize]).collect() };
        for (i, m) in mappings().iter().enumerate() {
            for interp in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
                let want = mask_from_image(&reference::resample(&mask_as_image(&mask), m, 70, 66, interp), 70, 66);
                assert!(resample_mask(&mask, m, 70, 66, interp) == want, "mapping {i} {interp:?}");
            }
        }
    }

    /// The byte sampler hands the kernels exactly the floats that converting the layer to a
    /// straight float image (and the selection to an alpha factor) would have.
    #[test]
    fn byte_sampler_equals_the_float_sampler() {
        let (w, h) = (45, 39);
        let mut rng = Rng(11);
        let mut bytes = vec![0u8; w * h * 4];
        for p in bytes.as_chunks_mut::<4>().0 {
            let a = [0u8, 255, 255, (rng.next() % 256) as u8][(rng.next() % 4) as usize];
            *p = [0, 1, 2].map(|_| (rng.next() % 256) as u8 % (a.max(1))).into_iter().chain([a]).collect::<Vec<_>>().try_into().unwrap();
            if a == 0 { *p = [0; 4]; }
        }
        let scale: Vec<f32> = (0..w * h).map(|_| [0.0, 1.0, 1.0, rng.unit()][(rng.next() % 4) as usize]).collect();
        for use_scale in [false, true] {
            let mut img = Image::from_premul_rgba8(w, h, &bytes).unwrap();
            if use_scale {
                for (px, &s) in img.pixels.iter_mut().zip(&scale) {
                    px[3] *= s;
                }
            }
            let float = Sampler::new(&img);
            let byte = Sampler::from_premul_u8(w, h, &bytes, use_scale.then_some(scale.as_slice()));
            for i in 0..400 {
                let p = Point::new(rng.unit() * (w as f32 + 6.0) - 3.0, rng.unit() * (h as f32 + 6.0) - 3.0);
                for interp in [Interp::Nearest, Interp::Bilinear, Interp::Bicubic] {
                    assert_eq!(byte.sample(p, interp), float.sample(p, interp), "scale {use_scale} sample {i} {interp:?}");
                }
            }
        }
    }

}
