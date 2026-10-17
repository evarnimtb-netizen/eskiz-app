//! Straight-alpha RGBA buffers at canvas size.

use crate::geom::IRect;

/// One straight-alpha pixel: red, green, blue, alpha, each in `[0, 1]`.
pub type Rgba = [f32; 4];

/// Row-major RGBA raster. Pixel `(x, y)` is at index `y * width + x`.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgba>,
}

impl Image {
    /// A fully transparent image.
    pub fn transparent(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![[0.0; 4]; width * height],
        }
    }

    pub fn from_fn(width: usize, height: usize, mut f: impl FnMut(usize, usize) -> Rgba) -> Self {
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                pixels.push(f(x, y));
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Converts premultiplied RGBA8 (the layout of `eskiz-core` layers, `eskiz-text` rasters and
    /// `eskiz-media` frames) to straight-alpha float. Returns `None` when `bytes` is not
    /// `width * height * 4` long.
    pub fn from_premul_rgba8(width: usize, height: usize, bytes: &[u8]) -> Option<Self> {
        if width.checked_mul(height)?.checked_mul(4)? != bytes.len() {
            return None;
        }
        let pixels = bytes.as_chunks::<4>().0.iter().map(|&p| straight_from_premul8(p)).collect();
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Converts back to premultiplied RGBA8, rounding to nearest like `eskiz-core`'s `to_u8`.
    pub fn to_premul_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for &p in &self.pixels {
            out.extend_from_slice(&premul8_from_straight(p));
        }
        out
    }

    /// Pixel at `(x, y)`, transparent outside the image.
    pub fn get(&self, x: usize, y: usize) -> Rgba {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x]
        } else {
            [0.0; 4]
        }
    }

    /// Writes a pixel; ignored outside the image.
    pub fn set(&mut self, x: usize, y: usize, px: Rgba) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = px;
        }
    }

    /// Bounding box of the pixels with alpha above zero.
    pub fn alpha_bounds(&self) -> Option<IRect> {
        bounds_where(self.width, self.height, |i| self.pixels[i][3] > 0.0)
    }
}

/// One premultiplied RGBA8 pixel as straight-alpha float (what [`Image::from_premul_rgba8`] does
/// to every pixel).
pub fn straight_from_premul8(p: [u8; 4]) -> Rgba {
    let a = f32::from(p[3]) / 255.0;
    if a <= 0.0 {
        [0.0; 4]
    } else {
        [
            (f32::from(p[0]) / 255.0 / a).clamp(0.0, 1.0),
            (f32::from(p[1]) / 255.0 / a).clamp(0.0, 1.0),
            (f32::from(p[2]) / 255.0 / a).clamp(0.0, 1.0),
            a,
        ]
    }
}

/// One straight-alpha float pixel as premultiplied RGBA8, rounding to nearest (what
/// [`Image::to_premul_rgba8`] does to every pixel).
pub fn premul8_from_straight(p: Rgba) -> [u8; 4] {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let a = p[3].clamp(0.0, 1.0);
    [q(p[0] * a), q(p[1] * a), q(p[2] * a), q(a)]
}

/// Bounding box of the row-major indices for which `keep` holds.
pub(crate) fn bounds_where(
    width: usize,
    height: usize,
    keep: impl Fn(usize) -> bool,
) -> Option<IRect> {
    if width == 0 || height == 0 {
        return None;
    }
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for y in 0..height {
        for x in 0..width {
            if !keep(y * width + x) {
                continue;
            }
            bounds = Some(match bounds {
                None => (x, y, x + 1, y + 1),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
            });
        }
    }
    bounds.map(|(x0, y0, x1, y1)| IRect::from_bounds(x0, y0, x1, y1))
}

/// Straight-alpha source-over of `src` onto `dst` (Porter-Duff over, un-premultiplied result).
pub fn over(src: Rgba, dst: Rgba) -> Rgba {
    let a_out = src[3] + (1.0 - src[3]) * dst[3];
    if a_out <= 0.0 {
        return [0.0; 4];
    }
    let mix = |s: f32, d: f32| ((s * src[3] + d * dst[3] * (1.0 - src[3])) / a_out).clamp(0.0, 1.0);
    [
        mix(src[0], dst[0]),
        mix(src[1], dst[1]),
        mix(src[2], dst[2]),
        a_out.clamp(0.0, 1.0),
    ]
}

#[cfg(test)]
mod convert_tests {
    use super::*;

    /// A pixel the transform does not touch goes straight to float, through `over` with a
    /// transparent source, and back. For every valid premultiplied pixel that is the identity,
    /// which is what lets the byte-backed commit copy untouched pixels instead of converting them.
    #[test]
    fn untouched_pixels_survive_the_float_round_trip() {
        for a in 0..=255u8 {
            for v in 0..=a {
                let px = [v, a - v / 2, v / 3, a];
                let mut dst = straight_from_premul8(px);
                dst[3] *= (1.0f32 - 0.0).clamp(0.0, 1.0);
                if dst[3] <= 0.0 {
                    dst = [0.0; 4];
                }
                let back = premul8_from_straight(over([0.0; 4], dst));
                let want = if a == 0 { [0; 4] } else { px };
                assert_eq!(back, want, "pixel {px:?}");
            }
        }
    }

    #[test]
    fn premultiplied_rgba8_round_trips() {
        let bytes: Vec<u8> = vec![0, 0, 0, 0, 255, 0, 0, 255, 64, 32, 0, 128, 10, 10, 10, 20];
        let img = Image::from_premul_rgba8(4, 1, &bytes).unwrap();
        assert_eq!(img.get(1, 0), [1.0, 0.0, 0.0, 1.0]);
        let back = img.to_premul_rgba8();
        for (a, b) in back.iter().zip(&bytes) {
            assert!(a.abs_diff(*b) <= 1, "{back:?} vs {bytes:?}");
        }
        assert!(Image::from_premul_rgba8(3, 2, &bytes).is_none());
    }
}
