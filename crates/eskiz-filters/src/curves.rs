//! Curves: a master curve composed with per-channel curves, with an optional
//! blend towards a second set of curves.

use crate::colour::{premultiply, unpremultiply};
use crate::image::{mix_masked, Image};

/// Table length.
pub const LUT_LEN: usize = 256;

/// One 256-entry tone curve with outputs in [0, 1].
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub table: [f32; LUT_LEN],
}

impl Curve {
    /// Identity curve. Entry `i` is `i / 256`, which matches the half-texel lookup of
    /// so that the lookup is exact (the identity is within 1/255 at every
    /// input).
    pub fn identity() -> Self {
        Self {
            table: std::array::from_fn(|i| i as f32 / 256.0),
        }
    }

    /// Curve from a table. Outputs are clamped to [0, 1].
    pub fn from_table(table: [f32; LUT_LEN]) -> Self {
        Self {
            table: table.map(|v| v.clamp(0.0, 1.0)),
        }
    }

    /// Lookup with the half-texel correction `u = c + 0.5/256` and linear interpolation.
    pub fn lookup(&self, c: f32) -> f32 {
        let u = c.clamp(0.0, 1.0) + 0.5 / 256.0;
        let pos = (u * 256.0 - 0.5).clamp(0.0, 255.0);
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(LUT_LEN - 1);
        let t = pos - i0 as f32;
        self.table[i0] + (self.table[i1] - self.table[i0]) * t
    }
}

/// Master plus red, green and blue curves for one set.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveSet {
    pub master: Curve,
    pub red: Curve,
    pub green: Curve,
    pub blue: Curve,
}

impl CurveSet {
    /// All-identity set.
    pub fn identity() -> Self {
        Self {
            master: Curve::identity(),
            red: Curve::identity(),
            green: Curve::identity(),
            blue: Curve::identity(),
        }
    }

    /// Output for channel `k` (0 = r, 1 = g, 2 = b): the master is applied first, then the
    /// channel curve.
    pub fn channel(&self, k: usize, c: f32) -> f32 {
        let m = self.master.lookup(c);
        let curve = match k {
            0 => &self.red,
            1 => &self.green,
            _ => &self.blue,
        };
        curve.lookup(m)
    }
}

/// Parameters of the curves adjustment.
#[derive(Clone, Debug, PartialEq)]
pub struct CurvesParams {
    /// Target curves.
    pub target: CurveSet,
    /// Blend in [0, 1]. Default 1 (no blend source).
    pub blend: f32,
    /// Optional blend source. Without it, blend is taken as 1.
    pub blend_source: Option<CurveSet>,
}

impl Default for CurvesParams {
    fn default() -> Self {
        Self {
            target: CurveSet::identity(),
            blend: 1.0,
            blend_source: None,
        }
    }
}

/// Map one unpremultiplied colour through the curves.
pub fn curves_rgb(c: [f32; 3], params: &CurvesParams) -> [f32; 3] {
    let base: [f32; 3] = std::array::from_fn(|k| params.target.channel(k, c[k]));
    let blend = params.blend.clamp(0.0, 1.0);
    match &params.blend_source {
        Some(src) if blend != 1.0 => std::array::from_fn(|k| {
            let alt = src.channel(k, c[k]);
            (alt * alt * (1.0 - blend) + base[k] * base[k] * blend)
                .max(0.0)
                .sqrt()
        }),
        _ => base,
    }
}

/// Apply the curves and mix by the mask with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &CurvesParams) -> Image {
    let out = src.map(|_, _, px| premultiply(curves_rgb(unpremultiply(px), params), px[3]));
    mix_masked(src, &out, mask)
}
