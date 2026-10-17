//! Premultiplied RGBA f32 buffer shared by every filter, with sampling and mask helpers.
//!
//! The layout matches eskiz-core's `Layer` (premultiplied, row-major, four channels per
//! pixel). RGBA8 import and export use the same `/255` and round-to-nearest rules as
//! eskiz-core's `premul_from_u8` and `to_u8`.

use rayon::prelude::*;

/// One premultiplied RGBA pixel.
pub type Px = [f32; 4];

/// Interaction mode of an adjustment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InteractionMode {
    /// Applies to the whole layer, limited by the selection mask.
    #[default]
    Layer,
    /// Applies only where the user paints.
    Pencil,
}

/// Premultiplied RGBA f32 image: `width * height * 4` values, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Image {
    /// Fully transparent image.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height * 4],
        }
    }

    /// Image where every pixel equals `px`.
    pub fn filled(width: usize, height: usize, px: Px) -> Self {
        Self {
            width,
            height,
            data: px
                .iter()
                .copied()
                .cycle()
                .take(width * height * 4)
                .collect(),
        }
    }

    /// Import premultiplied RGBA8 (the eskiz-core layer layout).
    pub fn from_rgba8_premul(width: usize, height: usize, bytes: &[u8]) -> Self {
        assert_eq!(
            bytes.len(),
            width * height * 4,
            "RGBA8 buffer has the wrong size"
        );
        Self {
            width,
            height,
            data: bytes.iter().map(|&b| f32::from(b) / 255.0).collect(),
        }
    }

    /// Export premultiplied RGBA8, rounding and clamping like eskiz-core's `to_u8`.
    pub fn to_rgba8_premul(&self) -> Vec<u8> {
        self.data
            .iter()
            .map(|&v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }

    /// Pixel at integer coordinates. Panics when out of bounds.
    #[inline]
    pub fn px(&self, x: usize, y: usize) -> Px {
        let i = (y * self.width + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    fn px_at(&self, x: isize, y: isize, zero_outside: bool) -> Px {
        let inside = x >= 0 && y >= 0 && x < self.width as isize && y < self.height as isize;
        if inside {
            return self.px(x as usize, y as usize);
        }
        if zero_outside {
            return [0.0; 4];
        }
        let cx = x.clamp(0, self.width as isize - 1) as usize;
        let cy = y.clamp(0, self.height as isize - 1) as usize;
        self.px(cx, cy)
    }

    /// Pixel with clamp-to-edge addressing.
    pub fn px_clamped(&self, x: isize, y: isize) -> Px {
        self.px_at(x, y, false)
    }

    /// Pixel, or transparent black outside the image.
    pub fn px_or_zero(&self, x: isize, y: isize) -> Px {
        self.px_at(x, y, true)
    }

    /// Bilinear sample at texel coordinates (texel `i` centred on `i`), clamp-to-edge.
    pub fn sample_texel(&self, fx: f32, fy: f32) -> Px {
        self.bilinear(fx, fy, false)
    }

    /// Bilinear sample at texel coordinates, transparent outside the image.
    pub fn sample_texel_or_zero(&self, fx: f32, fy: f32) -> Px {
        self.bilinear(fx, fy, true)
    }

    /// Bilinear sample at normalised coordinates in [0, 1]^2, clamp-to-edge.
    /// Pixel `i` covers `[i/W, (i+1)/W)` and is centred at `(i + 0.5)/W`.
    pub fn sample_uv(&self, u: f32, v: f32) -> Px {
        self.sample_texel(u * self.width as f32 - 0.5, v * self.height as f32 - 0.5)
    }

    fn bilinear(&self, fx: f32, fy: f32, zero_outside: bool) -> Px {
        let x0 = fx.floor();
        let y0 = fy.floor();
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.px_at(x0, y0, zero_outside);
        let b = self.px_at(x0 + 1, y0, zero_outside);
        let c = self.px_at(x0, y0 + 1, zero_outside);
        let d = self.px_at(x0 + 1, y0 + 1, zero_outside);
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }

    /// Build an image pixel by pixel, in parallel over rows.
    pub fn generate<F>(width: usize, height: usize, f: F) -> Self
    where
        F: Fn(usize, usize) -> Px + Sync,
    {
        let mut out = Self::new(width, height);
        if width == 0 || height == 0 {
            return out;
        }
        out.data
            .par_chunks_mut(width * 4)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, dst) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    *dst = f(x, y);
                }
            });
        out
    }

    /// Map every pixel (with its coordinates) into a new image, in parallel.
    pub fn map<F>(&self, f: F) -> Self
    where
        F: Fn(usize, usize, Px) -> Px + Sync,
    {
        Self::generate(self.width, self.height, |x, y| f(x, y, self.px(x, y)))
    }

    /// Multiply every channel (both colour and alpha) by `k`.
    pub fn scale(&self, k: f32) -> Self {
        self.map(|_, _, p| p.map(|v| v * k))
    }

    /// Box-average downscale by an integer factor (used by the Gaussian preview).
    pub fn downscale_box(&self, k: usize) -> Self {
        let k = k.max(1);
        let nw = self.width.div_ceil(k);
        let nh = self.height.div_ceil(k);
        Self::generate(nw, nh, |x, y| {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0f32;
            for sy in (y * k)..((y + 1) * k).min(self.height) {
                for sx in (x * k)..((x + 1) * k).min(self.width) {
                    for (a, v) in acc.iter_mut().zip(self.px(sx, sy)) {
                        *a += v;
                    }
                    n += 1.0;
                }
            }
            acc.map(|a| a / n)
        })
    }

    /// Bilinear resample to a new size.
    pub fn resize_bilinear(&self, nw: usize, nh: usize) -> Self {
        Self::generate(nw, nh, |x, y| {
            let u = (x as f32 + 0.5) / nw as f32;
            let v = (y as f32 + 0.5) / nh as f32;
            self.sample_uv(u, v)
        })
    }
}

/// Mask value at pixel index `i` (row-major). `None` means 1 everywhere.
#[inline]
pub fn mask_at(mask: Option<&[f32]>, i: usize) -> f32 {
    mask.map_or(1.0, |m| m[i].clamp(0.0, 1.0))
}

/// Linear mix of two pixels: `a (1 - t) + b t`.
#[inline]
pub fn mix_px(a: Px, b: Px, t: f32) -> Px {
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// Mix `src` towards `adjusted` by a per-pixel weight given by pixel index.
pub fn mix_weighted<W>(src: &Image, adjusted: &Image, weight: W) -> Image
where
    W: Fn(usize) -> f32 + Sync,
{
    let mut out = Image::new(src.width, src.height);
    out.data.par_chunks_mut(4).enumerate().for_each(|(i, dst)| {
        let t = weight(i);
        let s = &src.data[i * 4..i * 4 + 4];
        let a = &adjusted.data[i * 4..i * 4 + 4];
        for ((d, &sv), &av) in dst.iter_mut().zip(s).zip(a) {
            *d = sv + (av - sv) * t;
        }
    });
    out
}

/// The shared mask-mix rule: `out = mix(src, adjusted, m)`.
pub fn mix_masked(src: &Image, adjusted: &Image, mask: Option<&[f32]>) -> Image {
    mix_weighted(src, adjusted, |i| mask_at(mask, i))
}

/// Pencil-mode accumulation: each dab adds its alpha to the
/// painted mask, which is clamped to [0, 1].
pub fn accumulate_dab_mask(mask: &mut [f32], dab_alpha: &[f32]) {
    for (m, &a) in mask.iter_mut().zip(dab_alpha) {
        *m = (*m + a).clamp(0.0, 1.0);
    }
}

/// Expand a single-channel layer to RGBA: the value is copied to r, g, b
/// and alpha.
pub fn expand_single_channel(values: &[f32], width: usize, height: usize) -> Image {
    Image::generate(width, height, |x, y| {
        let v = values[y * width + x];
        [v, v, v, v]
    })
}

/// Reduce an RGBA image to a single channel by keeping r.
pub fn reduce_single_channel(img: &Image) -> Vec<f32> {
    img.data.as_chunks::<4>().0.iter().map(|p| p[0]).collect()
}

/// Expand a grey-plus-alpha layer (`[grey, alpha]` pairs) to RGBA.
pub fn expand_grey_alpha(pairs: &[f32], width: usize, height: usize) -> Image {
    Image::generate(width, height, |x, y| {
        let i = (y * width + x) * 2;
        let (g, a) = (pairs[i], pairs[i + 1]);
        [g, g, g, a]
    })
}

/// Reduce RGBA to a grey-plus-alpha layer (`[r, alpha]` pairs).
pub fn reduce_grey_alpha(img: &Image) -> Vec<f32> {
    img.data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[0], p[3]])
        .collect()
}
