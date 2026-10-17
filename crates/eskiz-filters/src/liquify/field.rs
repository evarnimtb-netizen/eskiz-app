//! Displacement field storage, particle rasterisation, the per-stroke cap and the resolve step.
//! The field runs at layer resolution (scale 1), in pixels.

use super::falloff;
use super::particles::{displacement, Particle, ParticleKind};
use crate::image::mask_at;
use rayon::prelude::*;

/// A two-component displacement value per texel, in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub width: usize,
    pub height: usize,
    pub data: Vec<[f32; 2]>,
}

impl Field {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![[0.0; 2]; width * height],
        }
    }

    /// Texel value at integer coordinates.
    pub fn get(&self, x: usize, y: usize) -> [f32; 2] {
        self.data[y * self.width + x]
    }

    /// Bilinear read at texel coordinates, clamped to the field's edges.
    pub fn sample_texel(&self, fx: f32, fy: f32) -> [f32; 2] {
        let x0 = fx.floor();
        let y0 = fy.floor();
        let (tx, ty) = (fx - x0, fy - y0);
        let cl = |v: f32, n: usize| (v as isize).clamp(0, n as isize - 1) as usize;
        let (x0, x1) = (cl(x0, self.width), cl(x0 + 1.0, self.width));
        let (y0, y1) = (cl(y0, self.height), cl(y0 + 1.0, self.height));
        let a = self.get(x0, y0);
        let b = self.get(x1, y0);
        let c = self.get(x0, y1);
        let d = self.get(x1, y1);
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }

    /// Multiply every texel by `k` (used when baking the adjust magnitude).
    pub fn scale(&mut self, k: f32) {
        for v in &mut self.data {
            *v = v.map(|c| c * k);
        }
    }

    /// Set every texel to zero.
    pub fn zero(&mut self) {
        self.data.fill([0.0; 2]);
    }
}

/// Accumulation buffers for one tick: the summed offset per texel and the summed reconstruct
/// weight per texel.
#[derive(Clone, Debug, PartialEq)]
pub struct Accumulator {
    pub width: usize,
    pub height: usize,
    pub offset: Vec<[f32; 2]>,
    pub weight: Vec<f32>,
}

impl Accumulator {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            offset: vec![[0.0; 2]; width * height],
            weight: vec![0.0; width * height],
        }
    }

    /// Rasterise one particle as a square quad of half-width `radius`. The
    /// falloff uses the minimum-distance clamp `d_min = 0.3` (scale 1). The particle's signed
    /// amount is applied by [`displacement`].
    pub fn splat(&mut self, p: &Particle) {
        let r = p.radius;
        if r <= 0.0 {
            return;
        }
        let (w, h) = (self.width as isize, self.height as isize);
        let x0 = ((p.pos[0] - r).floor() as isize).max(0);
        let x1 = ((p.pos[0] + r).ceil() as isize).min(w - 1);
        let y0 = ((p.pos[1] - r).floor() as isize).max(0);
        let y1 = ((p.pos[1] + r).ceil() as isize).min(h - 1);
        if x0 > x1 || y0 > y1 {
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                let q = [x as f32 + 0.5, y as f32 + 0.5];
                let dist = (q[0] - p.pos[0]).hypot(q[1] - p.pos[1]);
                let shape = falloff(dist, r, 1.0, 0.3);
                if shape == 0.0 {
                    continue;
                }
                let i = y as usize * self.width + x as usize;
                if p.kind == ParticleKind::Reconstruct {
                    self.weight[i] += shape * p.amount;
                } else {
                    let centre = match p.kind {
                        ParticleKind::Pinch | ParticleKind::Expand | ParticleKind::TwirlRight | ParticleKind::TwirlLeft => p.centre,
                        _ => p.pos,
                    };
                    let d = displacement(p.kind, q, centre, p.dir, shape, p.amount);
                    self.offset[i][0] += d[0];
                    self.offset[i][1] += d[1];
                }
            }
        }
    }
}

/// Per-stroke cap (§19A): the total offset deposited at each texel during one stroke is limited
/// to `limit` in length. `total` holds the deposits so far in this stroke and is updated. The
/// tick's offset is shortened to fit.
pub fn cap_stroke_deposit(acc: &mut Accumulator, total: &mut [[f32; 2]], limit: f32) {
    for (o, t) in acc.offset.iter_mut().zip(total.iter_mut()) {
        let sum = [t[0] + o[0], t[1] + o[1]];
        let len = sum[0].hypot(sum[1]);
        let clamped = if len > limit && len > 0.0 {
            let k = limit / len;
            [sum[0] * k, sum[1] * k]
        } else {
            sum
        };
        *o = [clamped[0] - t[0], clamped[1] - t[1]];
        *t = clamped;
    }
}

/// Reconstruct attenuation and zeroing: `strength = r 25 m`,
/// `atten = 0.75^strength`, `v = result x atten`, and zero when `|v| < 0.25`.
pub fn reconstruct(result: [f32; 2], weight: f32, mask: f32) -> [f32; 2] {
    if weight <= 0.0 {
        return result;
    }
    let atten = super::reconstruct_attenuation(weight, mask);
    let v = [result[0] * atten, result[1] * atten];
    if v[0].hypot(v[1]) < 0.25 {
        [0.0, 0.0]
    } else {
        v
    }
}

/// Resolve step, with scale 1 and pixel units:
/// `scaled = offset x m`, the previous field is read at `q + scaled`, and `result = scaled + field`.
/// The reconstruct attenuation follows, and the result is clamped to `max_len` (§19A: the
/// accumulated field is at most R).
pub fn resolve(prev: &Field, acc: &Accumulator, mask: Option<&[f32]>, max_len: f32) -> Field {
    let (w, h) = (prev.width, prev.height);
    let mut data = vec![[0.0f32; 2]; w * h];
    data.par_iter_mut().enumerate().for_each(|(i, out)| {
        let (x, y) = (i % w, i / w);
        let m = mask_at(mask, i);
        let scaled = [acc.offset[i][0] * m, acc.offset[i][1] * m];
        let prev_v = prev.sample_texel(x as f32 + scaled[0], y as f32 + scaled[1]);
        let result = [scaled[0] + prev_v[0], scaled[1] + prev_v[1]];
        let mut v = reconstruct(result, acc.weight[i], m);
        let len = v[0].hypot(v[1]);
        if len > max_len {
            let k = max_len / len;
            v = [v[0] * k, v[1] * k];
        }
        *out = v;
    });
    Field {
        width: w,
        height: h,
        data,
    }
}
