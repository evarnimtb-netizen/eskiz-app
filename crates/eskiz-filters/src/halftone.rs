//! Halftone: CMYK plate separation, dot screens and recombination.
//!
//! The dot-radius law and coverage are verified. The pre-filter (uniform box of radius half
//! the pitch), the overlay blend details and the white fill follow the documented wording but
//! are partly unverified (see the doc comments).

use crate::colour::{smoothstep, unpremultiply};
use crate::image::{mix_masked, Image, Px};
use rayon::prelude::*;

/// Plate screen angles for screen kind 0, in degrees: cyan 15, magenta 75, yellow 0,
/// black 45.
pub const PLATE_ANGLES_DEG: [f32; 4] = [15.0, 75.0, 0.0, 45.0];

/// Dot-diameter table for `rho > 1`, indexed by `t = 100 d - 78`.
#[allow(clippy::excessive_precision)]
pub const RHO_TABLE: [f32; 23] = [
    0.996557,
    1.003136,
    1.010545,
    1.018540,
    1.027054,
    1.036071,
    1.045592,
    1.055636,
    1.066228,
    1.077407,
    1.089219,
    1.101729,
    1.115014,
    1.129177,
    1.144352,
    1.160716,
    1.178516,
    1.198109,
    1.220043,
    1.245244,
    1.275516,
    1.315516,
    std::f32::consts::SQRT_2,
];

/// Parameters of the halftone filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HalftoneParams {
    /// Control in [0, 1] that sets the cell pitch `2 + 96 amount`. Default 0.12.
    pub amount: f32,
    /// Dot softness in pixels, [0, 2]. Default 0.5 (unverified).
    pub softness: f32,
    /// Anti-alias width in pixels. Default 1 (unverified).
    pub anti_alias: f32,
    /// Overlay mode. Default off.
    pub overlay: bool,
    /// White fill. Default off.
    pub white_fill: bool,
}

impl Default for HalftoneParams {
    fn default() -> Self {
        Self {
            amount: 0.12,
            softness: 0.5,
            anti_alias: 1.0,
            overlay: false,
            white_fill: false,
        }
    }
}

/// Cell pitch in pixels: `2 + 96 amount`.
pub fn pitch(amount: f32) -> f32 {
    2.0 + 96.0 * amount.clamp(0.0, 1.0)
}

/// Dot diameter in units of the cell pitch for an ink density `d`.
/// The documented `rho` is a diameter: the disc area is `d` of the cell, and the coverage formula
/// compares `2 dist` against it.
pub fn dot_diameter(density: f32) -> f32 {
    let d = density.clamp(0.0, 1.0);
    if d <= 0.0 {
        return 0.0;
    }
    let rho = 1.1287 * d.sqrt();
    if rho <= 1.0 {
        return rho;
    }
    let t = (100.0 * d - 78.0).clamp(0.0, 22.0);
    let i = (t.floor() as usize).min(21);
    let f = t - i as f32;
    RHO_TABLE[i] + (RHO_TABLE[i + 1] - RHO_TABLE[i]) * f
}

/// Plate densities `[C, M, Y, K]` of a pixel. They
/// are scaled by source alpha so transparent pixels carry no ink (implementation choice).
pub fn plate_densities(px: Px) -> [f32; 4] {
    let c = unpremultiply(px);
    let k = 1.0 - c[0].max(c[1]).max(c[2]);
    let denom = 1.0 - k;
    let (cc, mm, yy) = if denom > 1e-6 {
        (
            (1.0 - c[0] - k) / denom,
            (1.0 - c[1] - k) / denom,
            (1.0 - c[2] - k) / denom,
        )
    } else {
        (0.0, 0.0, 0.0)
    };
    [cc, mm, yy, k].map(|v| v.clamp(0.0, 1.0) * px[3])
}

/// Box pre-filter of every plate's density, uniform weights, radius `r`,
/// unverified weights and radius). Uses a summed-area table.
fn prefilter(dens: &[[f32; 4]], w: usize, h: usize, radius: f32) -> Vec<[f32; 4]> {
    let stride = w + 1;
    let mut sat = vec![[0.0f64; 4]; stride * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            let d = dens[y * w + x];
            let up = sat[y * stride + x + 1];
            let left = sat[(y + 1) * stride + x];
            let diag = sat[y * stride + x];
            let cell = &mut sat[(y + 1) * stride + x + 1];
            for (k, c) in cell.iter_mut().enumerate() {
                *c = f64::from(d[k]) + up[k] + left[k] - diag[k];
            }
        }
    }
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let x0 = ((px - radius).floor().max(0.0) as usize).min(w - 1);
            let x1 = ((px + radius).ceil().max(0.0) as usize).min(w - 1);
            let y0 = ((py - radius).floor().max(0.0) as usize).min(h - 1);
            let y1 = ((py + radius).ceil().max(0.0) as usize).min(h - 1);
            let at = |xx: usize, yy: usize| sat[yy * stride + xx];
            let (a, b, c, d) = (
                at(x1 + 1, y1 + 1),
                at(x0, y1 + 1),
                at(x1 + 1, y0),
                at(x0, y0),
            );
            let count = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f64;
            std::array::from_fn(|k| ((a[k] - b[k] - c[k] + d[k]) / count) as f32)
        })
        .collect()
}

/// Coverage of one dot at distance `dist`. `diameter` is in
/// pixels.
fn coverage(dist: f32, diameter: f32, softness: f32, anti_alias: f32) -> f32 {
    if diameter <= 0.0 {
        return 0.0;
    }
    let a = anti_alias.max(1e-6);
    let t = ((2.0 * dist - (diameter + softness) + a) / a).clamp(0.0, 1.0);
    1.0 - smoothstep(t)
}

/// One screen: a rotated grid of dots for one plate.
struct Screen<'a> {
    pitch: f32,
    sin: f32,
    cos: f32,
    plate: usize,
    pre: &'a [[f32; 4]],
    width: usize,
    height: usize,
    softness: f32,
    anti_alias: f32,
}

impl Screen<'_> {
    fn to_image(&self, u: f32, v: f32) -> [f32; 2] {
        [self.cos * u - self.sin * v, self.sin * u + self.cos * v]
    }

    fn cover(&self, p: [f32; 2], gi: f32, gj: f32) -> f32 {
        let c = self.to_image((gi + 0.5) * self.pitch, (gj + 0.5) * self.pitch);
        let ix = (c[0].floor().max(0.0) as usize).min(self.width - 1);
        let iy = (c[1].floor().max(0.0) as usize).min(self.height - 1);
        let density = self.pre[iy * self.width + ix][self.plate];
        let diameter = dot_diameter(density) * self.pitch;
        let dist = (p[0] - c[0]).hypot(p[1] - c[1]);
        coverage(dist, diameter, self.softness, self.anti_alias)
    }

    /// Plate value at `p`: own-cell dot plus its neighbours on the side of `p`.
    fn value(&self, p: [f32; 2]) -> f32 {
        let u = self.cos * p[0] + self.sin * p[1];
        let v = -self.sin * p[0] + self.cos * p[1];
        let fu = u / self.pitch;
        let fv = v / self.pitch;
        let (ci, cj) = (fu.floor(), fv.floor());
        let dx = if fu - ci < 0.5 { -1.0 } else { 1.0 };
        let dy = if fv - cj < 0.5 { -1.0 } else { 1.0 };
        (self.cover(p, ci, cj) + self.cover(p, ci + dx, cj) + self.cover(p, ci, cj + dy))
            .clamp(0.0, 1.0)
    }
}

/// Overlay blend.
fn overlay_px(src: Px, ht: Px) -> Px {
    let sum: Px = std::array::from_fn(|k| src[k] + (1.0 - src[k]) * src[k]);
    let sum_u = unpremultiply(sum);
    let ht_u = unpremultiply(ht);
    let ht_a = ht[3];
    let sum_a = sum[3];
    let blend: [f32; 3] = std::array::from_fn(|k| sum_u[k] * ht_u[k]);
    let hm: Px = std::array::from_fn(|k| {
        if k < 3 {
            let mixed = ht[k] * (1.0 - sum_a) + ht_a * blend[k] * sum_a;
            mixed + (1.0 - ht_a) * sum[k]
        } else {
            ht_a + (1.0 - ht_a) * sum_a
        }
    });
    std::array::from_fn(|k| sum[k] * (1.0 - 0.6665) + hm[k] * 0.6665)
}

/// Apply the halftone, then mix with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &HalftoneParams) -> Image {
    let (w, h) = (src.width, src.height);
    let p = pitch(params.amount);
    let dens: Vec<[f32; 4]> = src
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| plate_densities([c[0], c[1], c[2], c[3]]))
        .collect();
    let pre = prefilter(&dens, w, h, p / 2.0);
    let screens: [Screen; 4] = std::array::from_fn(|k| {
        let a = PLATE_ANGLES_DEG[k].to_radians();
        Screen {
            pitch: p,
            sin: a.sin(),
            cos: a.cos(),
            plate: k,
            pre: &pre,
            width: w,
            height: h,
            softness: params.softness,
            anti_alias: params.anti_alias,
        }
    });
    let mut result = Image::generate(w, h, |x, y| {
        let pt = [x as f32 + 0.5, y as f32 + 0.5];
        let [c, m, yv, k]: [f32; 4] = std::array::from_fn(|i| screens[i].value(pt));
        let max_cmy = c.max(m).max(yv);
        let rgb: [f32; 3] = [c, m, yv].map(|plate| {
            let presence = if plate > 0.0 { 0.0 } else { 1.0 };
            (presence * (1.0 - max_cmy) + (1.0 - plate) * max_cmy) * max_cmy * (1.0 - k)
        });
        let alpha = max_cmy * (1.0 - k) + k;
        [rgb[0], rgb[1], rgb[2], alpha]
    });
    if params.overlay {
        result = Image::generate(w, h, |x, y| overlay_px(src.px(x, y), result.px(x, y)));
    }
    if params.white_fill {
        result = result.map(|_, _, px| {
            let add = 1.0 - px[3];
            px.map(|v| v + add)
        });
    }
    mix_masked(src, &result, mask)
}
