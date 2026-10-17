//! Glitch. The sampling skeleton follows §18.2. The preset values and the
//! selection thresholds are not recovered, so every preset uses the §18.4 placeholder, which is
//! easy to change in [`placeholder`].

use crate::colour::unpremultiply;
use crate::image::{mix_masked, Image};

/// Placeholder cell counts across and down.
pub const PLACEHOLDER_CELLS: [f32; 2] = [8.0, 8.0];
/// Placeholder selection thresholds `threshold_A` and `threshold_B`.
pub const PLACEHOLDER_THRESHOLD: f32 = 0.5;
/// Placeholder horizontal offset, as a fraction of the layer width, at amount 1.
pub const PLACEHOLDER_OFFSET: f32 = 0.01;

/// The placeholder preset values. Used by every preset index until final values
/// are recovered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlitchPlaceholder {
    pub cells: [f32; 2],
    pub threshold_block: f32,
    pub threshold_line: f32,
    /// Per-channel offsets at amount 1, in normalised units, `[R, G, B]`.
    pub offsets: [[f32; 2]; 3],
}

/// The §18.4 placeholder. It is a placeholder, not reference behaviour.
pub fn placeholder() -> GlitchPlaceholder {
    GlitchPlaceholder {
        cells: PLACEHOLDER_CELLS,
        threshold_block: PLACEHOLDER_THRESHOLD,
        threshold_line: PLACEHOLDER_THRESHOLD,
        offsets: [
            [PLACEHOLDER_OFFSET, 0.0],
            [0.0, 0.0],
            [-PLACEHOLDER_OFFSET, 0.0],
        ],
    }
}

/// Parameters of the glitch filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlitchParams {
    /// Preset index 0 to 3. All four use the placeholder. Default 0.
    pub preset: u8,
    /// Amount in [0, 1], which multiplies the RGB offsets. Default 0.5.
    pub amount: f32,
    /// Noise seed (a time-based seed would also work). Default 0.
    pub seed: f32,
    /// Cell counts across and down. `None` uses the placeholder. Setting the counts to the layer
    /// size gives a per-texel grid, under which amount 0 is the identity.
    pub cells: Option<[f32; 2]>,
}

impl Default for GlitchParams {
    fn default() -> Self {
        Self {
            preset: 0,
            amount: 0.5,
            seed: 0.0,
            cells: None,
        }
    }
}

/// `hash(v) = fract(sin(dot(floor(v), (12.9898, 78.233))) * 43758.5453)`.
pub fn hash(v: [f64; 2]) -> f64 {
    let d = v[0].floor() * 12.9898 + v[1].floor() * 78.233;
    let s = (d.sin() * 43758.5453).fract();
    if s < 0.0 {
        s + 1.0
    } else {
        s
    }
}

/// Three decorrelated values:
/// `r0 = hash(v + 10 scale)`, `r1 = hash(k1 (v + 100 r0 scale))` with `k1 = 1 + seed (r0 - 0.5)`,
/// and `r2 = hash(k2 (v + 10000 r1 scale))` with `k2 = 1 + seed (r1 - 0.5)`.
pub fn noise3(v: [f64; 2], seed: f64, scale: [f64; 2]) -> [f64; 3] {
    let r0 = hash([v[0] + 10.0 * scale[0], v[1] + 10.0 * scale[1]]);
    let k1 = 1.0 + seed * (r0 - 0.5);
    let r1 = hash([
        k1 * (v[0] + 100.0 * r0 * scale[0]),
        k1 * (v[1] + 100.0 * r0 * scale[1]),
    ]);
    let k2 = 1.0 + seed * (r1 - 0.5);
    let r2 = hash([
        k2 * (v[0] + 10000.0 * r1 * scale[0]),
        k2 * (v[1] + 10000.0 * r1 * scale[1]),
    ]);
    [r0, r1, r2]
}

/// Apply the glitch. Each channel is sampled at `base + sum amount offset_c`,
/// nearest-texel, where `base` snaps to the cell centre. Each sample is unpremultiplied by its
/// own alpha. The output alpha is the maximum of the three sample alphas, and the result is
/// premultiplied. The mix uses the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &GlitchParams) -> Image {
    let gp = placeholder();
    let cells = params.cells.unwrap_or(gp.cells);
    let (w, h) = (src.width, src.height);
    let (wf, hf) = (w as f64, h as f64);
    let seed = f64::from(params.seed);
    let amount = f64::from(params.amount.clamp(0.0, 1.0));
    let cells64 = [f64::from(cells[0]), f64::from(cells[1])];
    let thr_a = f64::from(gp.threshold_block);
    let thr_b = f64::from(gp.threshold_line);
    let out = src.map(|x, y, _| {
        let uv = [(x as f64 + 0.5) / wf, (y as f64 + 0.5) / hf];
        let p = [uv[0] - 0.5, uv[1] - 0.5];
        // Block pass: v = cells p, scale (1, 1). Line pass: v = cells (p.y, p.x), scale = cells.
        let block = noise3([cells64[0] * p[0], cells64[1] * p[1]], seed, [1.0, 1.0]);
        let line = noise3([cells64[0] * p[1], cells64[1] * p[0]], seed, cells64);
        let sel_a = if block[2] + 0.004 <= thr_a { 1.0 } else { 0.0 };
        let sel_b = if line[1] + 0.004 <= thr_b { 1.0 } else { 0.0 };
        let sum = sel_a + sel_b;
        // Snapped base: the cell centre, (floor(uv cells) + 0.5) / cells. Degenerate cell counts
        // keep uv. The snap scales in with amount: base = uv + amount (snapped - uv), so amount 0
        // samples the source pixel exactly.
        let snapped = [
            if cells64[0] > 1e-6 {
                ((uv[0] * cells64[0]).floor() + 0.5) / cells64[0]
            } else {
                uv[0]
            },
            if cells64[1] > 1e-6 {
                ((uv[1] * cells64[1]).floor() + 0.5) / cells64[1]
            } else {
                uv[1]
            },
        ];
        let base = [
            uv[0] + amount * (snapped[0] - uv[0]),
            uv[1] + amount * (snapped[1] - uv[1]),
        ];
        let samples: [[f32; 4]; 3] = std::array::from_fn(|c| {
            let off = gp.offsets[c];
            let cu = base[0] + sum * amount * f64::from(off[0]);
            let cv = base[1] + sum * amount * f64::from(off[1]);
            let ix = ((cu * wf).floor() as isize).clamp(0, w as isize - 1) as usize;
            let iy = ((cv * hf).floor() as isize).clamp(0, h as isize - 1) as usize;
            src.px(ix, iy)
        });
        let alpha = samples[0][3].max(samples[1][3]).max(samples[2][3]);
        let r = unpremultiply(samples[0])[0];
        let g = unpremultiply(samples[1])[1];
        let b = unpremultiply(samples[2])[2];
        [r * alpha, g * alpha, b * alpha, alpha]
    });
    mix_masked(src, &out, mask)
}
