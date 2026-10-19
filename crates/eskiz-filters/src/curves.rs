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
    /// Identity curve. Entry `i` is `i / 256`; `lookup` maps every input to itself.
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

    /// Lookup at `c` in [0, 1] with linear interpolation. Entry `i` sits at `i / 256`; the last
    /// interval (255/256 to 1) continues the slope of the one before it, so the identity maps
    /// every input to itself, white included.
    pub fn lookup(&self, c: f32) -> f32 {
        let pos = (c.clamp(0.0, 1.0) * 256.0).clamp(0.0, 256.0);
        let i0 = (pos.floor() as usize).min(LUT_LEN - 1);
        let i1 = (i0 + 1).min(LUT_LEN - 1);
        let (lo, hi) = if i0 == LUT_LEN - 1 { (LUT_LEN - 2, LUT_LEN - 1) } else { (i0, i1) };
        let t = pos - i0 as f32;
        let slope = self.table[hi] - self.table[lo];
        (self.table[i0] + slope * t).clamp(0.0, 1.0)
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

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn the_identity_maps_every_8_bit_value_to_itself() {
        let c = Curve::identity();
        for v in 0..=255u32 {
            let x = v as f32 / 255.0;
            assert_eq!((c.lookup(x) * 255.0).round() as u32, v);
        }
        let out = curves_rgb([1.0, 254.0 / 255.0, 0.5], &CurvesParams::default());
        assert_eq!((out[0] * 255.0).round() as u32, 255);
    }
}
