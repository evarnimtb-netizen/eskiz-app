//! Fractal noise: 2D simplex base noise, fBm octaves, domain warp and the
//! reference's normalisation tables.

use crate::colour::{premultiply, sat, unpremultiply};
use crate::image::{mix_masked, Image};
use crate::random::Rng;

const F2: f32 = 0.3660254;
const G2: f32 = 0.2113249;
/// Scale that brings the simplex output to about [-1, 1].
const SIMPLEX_SCALE: f32 = 45.4545;
/// The eight gradient vectors `(+-1, +-2)` and `(+-2, +-1)`.
const GRADIENTS: [[f32; 2]; 8] = [
    [1.0, 2.0],
    [-1.0, 2.0],
    [1.0, -2.0],
    [-1.0, -2.0],
    [2.0, 1.0],
    [-2.0, 1.0],
    [2.0, -1.0],
    [-2.0, -1.0],
];
/// Per-channel origins for multichannel noise.
const ORIGIN_R: [f32; 2] = [0.1254, 1421.2134];
const ORIGIN_G: [f32; 2] = [410.312, 35.132];
#[allow(clippy::excessive_precision)]
const ORIGIN_B: [f32; 2] = [8.4512, 4104.3218];
/// Domain-warp sample offsets.
const WARP_X_OFFSET: [f32; 2] = [142.421, 2134.32];
const WARP_Y_OFFSET: [f32; 2] = [-7.213, 142.47];
/// Sample-scale constants.
const SCALE_BASE: f32 = 0.45639828;
const SCALE_FLOOR: f32 = 0.0000517;

/// Signed normalisation ranges `(lo, hi)` for octave counts 1 to 10.
#[allow(clippy::excessive_precision)]
const SIGNED_RANGE: [(f32, f32); 10] = [
    (-1.00455, 1.004689),
    (-1.468506, 1.468356),
    (-1.666975, 1.623065),
    (-1.655811, 1.647271),
    (-1.707711, 1.678567),
    (-1.680347, 1.665980),
    (-1.690773, 1.663760),
    (-1.696420, 1.662005),
    (-1.698025, 1.665462),
    (-1.697638, 1.665179),
];

/// Absolute normalisation ranges for octave counts 1 to 10.
#[allow(clippy::excessive_precision)]
const ABSOLUTE_RANGE: [(f32, f32); 10] = [
    (0.0, 1.004689),
    (0.0, 1.468506),
    (0.0, 1.666975),
    (0.0, 1.708870),
    (0.0, 1.759366),
    (0.0, 1.773251),
    (0.0, 1.778356),
    (0.0, 1.779392),
    (0.0, 1.782100),
    (0.0, 1.783211),
];

/// Fractal kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoiseKind {
    /// `sum += n` (default).
    #[default]
    Signed,
    /// `sum += |n|`.
    Absolute,
    /// `sum += (1 - |n|)^2`. Its range is not recovered (unverified). Uses the absolute table.
    Billowy,
}

/// How the noise combines with the pixel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CombineMode {
    /// Additive (default).
    #[default]
    Additive,
    /// Mix towards a shaped noise value.
    Mix,
}

/// Parameters of the noise filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseParams {
    /// Scale control in [0, 1]. Default 0.
    pub scale: f32,
    /// Amount in [0, 1]. Default 0.23.
    pub amount: f32,
    /// Octave control in [0, 1]. Octave count is `int(4 o) + 1`. Default 0 (one octave).
    pub octaves: f32,
    /// Fractal kind. Default signed.
    pub kind: NoiseKind,
    /// Separate noise field per channel. Default off.
    pub multichannel: bool,
    /// Domain-warp (turbulence) control in [0, 1]. Default 0.7.
    pub turbulence: f32,
    /// Combine mode. Default additive.
    pub combine: CombineMode,
    /// Seed for the permutation table (implementer-chosen;.
    pub seed: u32,
}

impl Default for NoiseParams {
    fn default() -> Self {
        Self {
            scale: 0.0,
            amount: 0.23,
            octaves: 0.0,
            kind: NoiseKind::Signed,
            multichannel: false,
            turbulence: 0.7,
            combine: CombineMode::Additive,
            seed: 0,
        }
    }
}

/// A 512-entry permutation table, as used by the simplex sampler.
pub type Permutation = [u8; 512];

/// Seeded permutation table: a Fisher-Yates shuffle of 0..256, repeated twice.
pub fn permutation(seed: u32) -> Permutation {
    let mut rng = Rng::new(u64::from(seed));
    let mut table: [u8; 256] = std::array::from_fn(|i| i as u8);
    for i in (1..256).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        table.swap(i, j);
    }
    std::array::from_fn(|i| table[i & 255])
}

/// 2D simplex noise: standard skew factors, `(0.5 - r^2)^4` falloff and a
/// scale of 45.4545.
pub fn simplex2d(perm: &Permutation, x: f32, y: f32) -> f32 {
    let s = (x + y) * F2;
    let i = (x + s).floor();
    let j = (y + s).floor();
    let t = (i + j) * G2;
    let x0 = x - (i - t);
    let y0 = y - (j - t);
    let (i1, j1): (usize, usize) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let x1 = x0 - i1 as f32 + G2;
    let y1 = y0 - j1 as f32 + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;
    let ii = (i as i64).rem_euclid(256) as usize;
    let jj = (j as i64).rem_euclid(256) as usize;
    let gi0 = perm[ii + perm[jj] as usize] as usize % 8;
    let gi1 = perm[ii + i1 + perm[jj + j1] as usize] as usize % 8;
    let gi2 = perm[ii + 1 + perm[jj + 1] as usize] as usize % 8;
    let corner = |gi: usize, px: f32, py: f32| {
        let t = 0.5 - px * px - py * py;
        if t < 0.0 {
            0.0
        } else {
            let g = GRADIENTS[gi];
            t.powi(4) * (g[0] * px + g[1] * py)
        }
    };
    SIMPLEX_SCALE * (corner(gi0, x0, y0) + corner(gi1, x1, y1) + corner(gi2, x2, y2))
}

/// Octave count `int(4 o) + 1`, clamped to [1, 10].
pub fn octave_count(octaves: f32) -> usize {
    ((4.0 * octaves.clamp(0.0, 1.0)).floor() as usize + 1).clamp(1, 10)
}

/// Normalisation range `(lo, hi)` for a fractal kind and octave count.
pub fn normalisation_range(kind: NoiseKind, count: usize) -> (f32, f32) {
    let idx = count.clamp(1, 10) - 1;
    match kind {
        NoiseKind::Signed => SIGNED_RANGE[idx],
        NoiseKind::Absolute | NoiseKind::Billowy => ABSOLUTE_RANGE[idx],
    }
}

/// Fractal sum. Frequency doubles and amplitude halves per octave.
pub fn fbm(perm: &Permutation, x: f32, y: f32, octaves: usize, kind: NoiseKind) -> f32 {
    let mut sum = 0.0;
    let mut freq = 1.0;
    let mut amp = 1.0;
    for _ in 0..octaves {
        let n = simplex2d(perm, x * freq, y * freq);
        let c = match kind {
            NoiseKind::Signed => n,
            NoiseKind::Absolute => n.abs(),
            NoiseKind::Billowy => (1.0 - n.abs()).powi(2),
        };
        sum += amp * c;
        freq *= 2.0;
        amp *= 0.5;
    }
    sum
}

/// Domain warp: offsets `p` by `0.32 T (w_x, w_y)`, where the two warp
/// samples are simplex at `2p` plus the given offsets.
pub fn warp(perm: &Permutation, p: [f32; 2], turbulence: f32) -> [f32; 2] {
    let t = turbulence.clamp(0.0, 1.0);
    if t <= 0.0 {
        return p;
    }
    let q = [2.0 * p[0], 2.0 * p[1]];
    let wx = simplex2d(perm, q[0] + WARP_X_OFFSET[0], q[1] + WARP_X_OFFSET[1]);
    let wy = simplex2d(perm, q[0] + WARP_Y_OFFSET[0], q[1] + WARP_Y_OFFSET[1]);
    let s = 0.32 * t;
    [p[0] + s * wx, p[1] + s * wy]
}

/// Sample scale per axis factor `0.45639828 (1 - s)^4 + 0.0000517`.
pub fn sample_scale(scale: f32) -> f32 {
    let s = scale.clamp(0.0, 1.0);
    SCALE_BASE * (1.0 - s).powi(4) + SCALE_FLOOR
}

/// Noise value at a point in the normalised output range `n`:
/// `(raw - lo) / (hi - lo) - 0.5`.
pub fn normalised_noise(
    perm: &Permutation,
    p: [f32; 2],
    params: &NoiseParams,
    origin: [f32; 2],
    count: usize,
) -> f32 {
    let q = warp(
        perm,
        [p[0] + origin[0], p[1] + origin[1]],
        params.turbulence,
    );
    let raw = fbm(perm, q[0], q[1], count, params.kind);
    let (lo, hi) = normalisation_range(params.kind, count);
    (raw - lo) / (hi - lo) - 0.5
}

/// Apply noise to an image, then mix by the mask with the default rule.
pub fn apply(src: &Image, mask: Option<&[f32]>, params: &NoiseParams) -> Image {
    let perm = permutation(params.seed);
    let count = octave_count(params.octaves);
    let amount = params.amount.clamp(0.0, 1.0);
    let fscale = sample_scale(params.scale);
    let (w, h) = (src.width as f32, src.height as f32);
    let origins = if params.multichannel {
        [ORIGIN_R, ORIGIN_G, ORIGIN_B]
    } else {
        [[0.0; 2]; 3]
    };
    let out = src.map(|x, y, px| {
        let uv = [(x as f32 + 0.5) / w, (y as f32 + 0.5) / h];
        let p = [(uv[0] - 0.5) * w * fscale, (uv[1] - 0.5) * h * fscale];
        let n: [f32; 3] =
            std::array::from_fn(|c| normalised_noise(&perm, p, params, origins[c], count));
        let alpha = px[3];
        let rgb = unpremultiply(px);
        match params.combine {
            CombineMode::Additive => {
                let out_rgb: [f32; 3] =
                    std::array::from_fn(|c| sat(3.0 * amount * (2.0 * n[c]) + rgb[c]));
                premultiply(out_rgb, alpha)
            }
            CombineMode::Mix => {
                let k = (3.0 * amount - 1.0).max(0.0) * 5.0 + 1.0;
                let t = sat(3.0 * amount);
                let out_rgb: [f32; 3] = std::array::from_fn(|c| {
                    let shaped = k * n[c] + 0.5;
                    rgb[c] * (1.0 - t) + shaped * t
                });
                let out_alpha = alpha * (1.0 - t) + t;
                premultiply(out_rgb, out_alpha)
            }
        }
    });
    mix_masked(src, &out, mask)
}
