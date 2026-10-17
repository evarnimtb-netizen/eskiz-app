//! Chromatic aberration: red and blue shift in opposite directions while
//! green stays put. Axial is radial from an origin; transverse is a fixed offset from a
//! blurred copy.

use crate::gaussian;
use crate::image::{mix_masked, Image, Px};

/// Alpha factor `0.333252`: the average of three samples.
pub const ALPHA_SCALE: f32 = 0.333252;

/// Axial parameters. Defaults other than origin and amount are
/// unverified.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxialParams {
    /// Origin in normalised coordinates. Default (0.5, 0.5).
    pub origin: [f32; 2],
    /// Amount vector. Default (0.01, 0.01) (unverified).
    pub amount: [f32; 2],
    /// Inverse range `R^-1 > 0`. Default 1 (unverified).
    pub inverse_range: f32,
    /// Falloff. Default 0 (unverified).
    pub falloff: f32,
    /// Transition strength. Default 1 (unverified).
    pub transition: f32,
}

impl Default for AxialParams {
    fn default() -> Self {
        Self {
            origin: [0.5, 0.5],
            amount: [0.01, 0.01],
            inverse_range: 1.0,
            falloff: 0.0,
            transition: 1.0,
        }
    }
}

/// Transverse parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransverseParams {
    /// Offset in normalised units. Use [`default_offset`] for the design notes default.
    pub offset: [f32; 2],
    /// Transparency in [0, 1]. Default 0 (unverified).
    pub transparency: f32,
    /// Transverse strength `S`. The blur amount is `0.1 S^0.75`. Default 0 (identity copy).
    pub strength: f32,
}

impl Default for TransverseParams {
    fn default() -> Self {
        Self {
            offset: [0.01, 0.01],
            transparency: 0.0,
            strength: 0.0,
        }
    }
}

/// Default transverse offset: 1% of the larger side in pixels, normalised per axis.
pub fn default_offset(width: usize, height: usize) -> [f32; 2] {
    let m = width.max(height) as f32;
    [0.01 * m / width as f32, 0.01 * m / height as f32]
}

/// Axial offset for a normalised position.
pub fn axial_offset(params: &AxialParams, uv: [f32; 2], width: usize, height: usize) -> [f32; 2] {
    let d = [uv[0] - params.origin[0], uv[1] - params.origin[1]];
    let r = d[0].hypot(d[1]);
    if r <= 0.01 {
        return [0.0, 0.0];
    }
    let sx = width as f32 * d[0];
    let sy = height as f32 * d[1];
    let x = params.inverse_range * sx.hypot(sy);
    let t = (0.03 - x + params.falloff) * params.transition;
    let k = x / ((1.0 + t.exp()) * r);
    [params.amount[0] * d[0] * k, params.amount[1] * d[1] * k]
}

fn inside(u: [f32; 2]) -> bool {
    (0.0..=1.0).contains(&u[0]) && (0.0..=1.0).contains(&u[1])
}

fn clamp_channels(rgb: [f32; 3], alpha: f32) -> [f32; 3] {
    rgb.map(|v| v.min(alpha).max(0.0))
}

/// Axial chromatic aberration, mixed by the mask.
pub fn apply_axial(src: &Image, mask: Option<&[f32]>, params: &AxialParams) -> Image {
    let (w, h) = (src.width, src.height);
    let out = src.map(|x, y, centre: Px| {
        let uv = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
        let off = axial_offset(params, uv, w, h);
        let ru = [uv[0] - off[0], uv[1] - off[1]];
        let bu = [uv[0] + off[0], uv[1] + off[1]];
        let r_s = if inside(ru) {
            src.sample_uv(ru[0], ru[1])
        } else {
            centre
        };
        let b_s = if inside(bu) {
            src.sample_uv(bu[0], bu[1])
        } else {
            centre
        };
        let alpha = (r_s[3] + centre[3] + b_s[3]) * ALPHA_SCALE;
        let rgb = clamp_channels([r_s[0], centre[1], b_s[2]], alpha);
        [rgb[0], rgb[1], rgb[2], alpha]
    });
    mix_masked(src, &out, mask)
}

/// Transverse chromatic aberration. Red is read from the blurred copy
/// shifted by `-offset` and blue from it shifted by `+offset`.
pub fn apply_transverse(src: &Image, mask: Option<&[f32]>, params: &TransverseParams) -> Image {
    let blurred = gaussian::blur(src, 0.1 * params.strength.clamp(0.0, 1.0).powf(0.75));
    let t = params.transparency.clamp(0.0, 1.0);
    let (w, h) = (src.width, src.height);
    let out = src.map(|x, y, centre: Px| {
        let uv = [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32];
        let ru = [uv[0] - params.offset[0], uv[1] - params.offset[1]];
        let bu = [uv[0] + params.offset[0], uv[1] + params.offset[1]];
        let r_s = if inside(ru) {
            blurred.sample_uv(ru[0], ru[1])
        } else {
            centre
        };
        let b_s = if inside(bu) {
            blurred.sample_uv(bu[0], bu[1])
        } else {
            centre
        };
        let r = r_s[0] * (1.0 - t) + centre[0] * t;
        let b = b_s[2] * (1.0 - t) + centre[2] * t;
        let alpha = (r_s[3] + centre[3] + b_s[3]) * ALPHA_SCALE;
        let rgb = clamp_channels([r, centre[1], b], alpha);
        [rgb[0], rgb[1], rgb[2], alpha]
    });
    mix_masked(src, &out, mask)
}
