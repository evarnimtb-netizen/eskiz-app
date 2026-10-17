//! Per-pixel stroke compositing and flattening.
//!
//! Layer pixels are premultiplied RGBA8 in memory. The maths runs in `f64` on
//! 0..1 values and is converted back to 8 bits when written.

use crate::canvas::Canvas;
use crate::geometry::clamp01;

/// Inputs for one pixel of the stroke composite.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelOpts {
    /// Layer before the stroke, premultiplied RGBA in 0..1.
    pub layer: [f64; 4],
    /// Stroke coverage A in 0..1.
    pub coverage: f64,
    /// Stroke colour, RGB in 0..1.
    pub colour: [f64; 3],
    pub stroke_opacity: f64,
    /// Wet-edge amount in [0, 1].
    pub wet_edges: f64,
    /// Alpha threshold; coverage below it is dropped when > 0.
    pub threshold: f64,
    /// Burnt-edge amount in [0, 1].
    pub burnt: f64,
    /// Texturized grain as `(tone, depth)` at this pixel, applied after wet edges.
    pub grain: Option<(f64, f64)>,
    pub erase: bool,
    pub alpha_lock: bool,
    /// Paper colour used by [`display_pixel`]. White when `None`.
    pub paper: Option<[f64; 3]>,
}

impl Default for PixelOpts {
    fn default() -> Self {
        Self {
            layer: [0.0; 4],
            coverage: 0.0,
            colour: [0.0; 3],
            stroke_opacity: 1.0,
            wet_edges: 0.0,
            threshold: 0.0,
            burnt: 0.0,
            grain: None,
            erase: false,
            alpha_lock: false,
            paper: None,
        }
    }
}

/// GLSL `smoothstep(e0, e1, x)`.
fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

/// Stroke composite for one pixel. Output is premultiplied.
///
/// Order follows: wet edges, texturized grain, opacity, alpha threshold,
/// burnt edges, then erase, lock transparency or normal source-over.
#[inline]
pub fn stroke_composite_pixel(o: &PixelOpts) -> [f64; 4] {
    let l = o.layer;
    let mut a = clamp01(o.coverage);

    // Wet edges: A_w = mix(A, 1 - |sin(4.5A)/(4.5A)|, wE) for A > 0. With wE = 0 the mix leaves
    // A exactly as it is, so the `sin` is skipped.
    if a > 0.0 && o.wet_edges != 0.0 {
        let x: f64 = 4.5 * a;
        let wet = 1.0 - (x.sin() / x).abs();
        a += (wet - a) * o.wet_edges;
    }
    // Texturized grain applies per stroke, after wet edges.
    if let Some((tone, depth)) = o.grain {
        a *= (1.0 - depth) + tone * depth;
    }

    let mut f = clamp01(o.stroke_opacity * clamp01(a));
    if o.threshold > 0.0 && a < o.threshold {
        f = 0.0;
    }

    let mut col = o.colour;
    if o.burnt > 0.0 && f > 0.0 {
        let band = 1.0 - smoothstep(0.0, 0.5, a);
        let k = 1.0 - o.burnt * band;
        col = [col[0] * k, col[1] * k, col[2] * k];
    }

    if o.erase {
        // Erase: layerBefore * (1 - f).
        return [
            l[0] * (1.0 - f),
            l[1] * (1.0 - f),
            l[2] * (1.0 - f),
            l[3] * (1.0 - f),
        ];
    }
    if o.alpha_lock {
        // Lock transparency: keep the layer alpha and paint only over existing pixels.
        return [
            l[0] + (col[0] * l[3] - l[0]) * f,
            l[1] + (col[1] * l[3] - l[1]) * f,
            l[2] + (col[2] * l[3] - l[2]) * f,
            l[3],
        ];
    }
    // Normal: source-over with alpha f (premultiplied).
    [
        col[0] * f + l[0] * (1.0 - f),
        col[1] * f + l[1] * (1.0 - f),
        col[2] * f + l[2] * (1.0 - f),
        f + l[3] * (1.0 - f),
    ]
}

/// Put an opaque paper colour under a premultiplied pixel.
pub fn over_paper(premul: [f64; 4], paper: [f64; 3]) -> [f64; 4] {
    [
        premul[0] + paper[0] * (1.0 - premul[3]),
        premul[1] + paper[1] * (1.0 - premul[3]),
        premul[2] + paper[2] * (1.0 - premul[3]),
        1.0,
    ]
}

/// Display colour for one pixel: stroke composite, then paper underneath.
pub fn display_pixel(o: &PixelOpts) -> [f64; 4] {
    over_paper(stroke_composite_pixel(o), o.paper.unwrap_or([1.0; 3]))
}

/// WCAG contrast ratio between two RGB colours in 0..1.
pub fn contrast_ratio(a: [f64; 3], b: [f64; 3]) -> f64 {
    let lum = |c: [f64; 3]| {
        let lin = |v: f64| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    };
    let la = lum(a);
    let lb = lum(b);
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `v / 255.0` for every byte value: the loop reads this instead of dividing four times a pixel.
static UNIT_OF_U8: [f64; 256] = {
    let mut t = [0.0; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as f64 / 255.0;
        i += 1;
    }
    t
};

/// A byte as a value in 0..1 (`v / 255`).
#[inline(always)]
pub fn unit_from_u8(v: u8) -> f64 {
    UNIT_OF_U8[usize::from(v)]
}

/// Read a premultiplied RGBA8 pixel as f64 values in 0..1.
#[inline]
pub fn premul_from_u8(px: &[u8]) -> [f64; 4] {
    [unit_from_u8(px[0]), unit_from_u8(px[1]), unit_from_u8(px[2]), unit_from_u8(px[3])]
}

/// `x.round() as u8` for `x` in `0..=255` (NaN gives 0), without the libm `round` call that
/// the compositor's inner loops would otherwise make for every channel. Rounds halves up,
/// exactly like `f64::round` does for non-negative values.
#[inline(always)]
pub fn round_to_u8(x: f64) -> u8 {
    let t = x as u8; // saturating truncation
    t.saturating_add(u8::from(x - f64::from(t) >= 0.5))
}

/// Convert a 0..1 value to 8 bits, rounding and clamping.
#[inline]
pub fn to_u8(v: f64) -> u8 {
    round_to_u8(clamp01(v) * 255.0)
}

/// Flatten all visible layers over opaque white paper.
///
/// Returns tightly packed RGBA8 of `canvas.width * canvas.height` pixels with
/// alpha 255. Each layer is scaled by its `opacity` and composited source-over.
pub fn flatten(canvas: &Canvas) -> Vec<u8> {
    let n = canvas.width * canvas.height;
    let mut out = Vec::with_capacity(n * 4);
    for i in 0..n {
        let mut acc = [1.0f64; 3];
        for layer in canvas.layers.iter().filter(|l| l.visible) {
            let px = premul_from_u8(&layer.pixel(i % canvas.width, i / canvas.width));
            let op = clamp01(layer.opacity);
            let src = [px[0] * op, px[1] * op, px[2] * op, px[3] * op];
            for c in 0..3 {
                acc[c] = src[c] + acc[c] * (1.0 - src[3]);
            }
        }
        out.extend(acc.iter().map(|&v| to_u8(v)));
        out.push(255);
    }
    out
}

#[cfg(test)]
mod round_tests {
    use super::*;

    /// Round-half-up of `x` in `0..=255`, computed exactly: `2 * x` and `floor` are exact in
    /// binary floating point, so this never suffers the `x + 0.5` rounding error (`0.5 - 2⁻⁵⁴ + 0.5`
    /// is `1.0`) and it does not depend on the platform's libm. `floor(2x)` is odd exactly when
    /// the fraction of `x` is at least a half, and then `ceil(floor(2x) / 2)` is the next integer.
    fn exact_half_up(x: f64) -> u8 {
        let twice = (2.0 * x).floor() as u32;
        twice.div_ceil(2) as u8
    }

    /// Inputs that pin down the halves: the smallest double below 0.5 must round down.
    const EDGES: [f64; 7] = [0.0, 0.5, 0.499_999_999_999_999_94, 1.5, 254.5, 255.0, 127.5];

    #[test]
    fn round_to_u8_rounds_halves_up() {
        let mut x = 0.0f64;
        while x <= 255.0 {
            assert_eq!(round_to_u8(x), exact_half_up(x), "{x}");
            x += 0.013_7;
        }
        for x in EDGES {
            assert_eq!(round_to_u8(x), exact_half_up(x), "{x}");
        }
        assert_eq!(round_to_u8(f64::NAN), 0);
    }

    /// Cross-check against `f64::round`, which `round_to_u8` is documented to match.
    ///
    /// Skipped on `x86_64-pc-windows-gnu`: there `f64::round` is a call into the statically
    /// linked mingw-w64 `round` (libmingwex), which computes `ceil(x) - x > 0.5` and so returns
    /// `1.0` for `0.499_999_999_999_999_94` (`1.0 - x` rounds to exactly `0.5`). That is a bug in
    /// the MinGW C runtime, present on real Windows as much as under Wine; `round_to_u8` is right
    /// and is checked against the exact oracle above on every platform. Release builds use the
    /// MSVC target, whose UCRT `round` is correct.
    #[test]
    #[cfg(not(all(target_os = "windows", target_env = "gnu")))]
    fn f64_round_agrees_with_the_exact_oracle() {
        let mut x = 0.0f64;
        while x <= 255.0 {
            assert_eq!(x.round() as u8, exact_half_up(x), "{x}");
            x += 0.013_7;
        }
        for x in EDGES {
            assert_eq!(x.round() as u8, exact_half_up(x), "{x}");
        }
    }
}

#[cfg(test)]
mod fast_path_tests {
    use super::*;

    /// With no wet edges the `sin` stage is skipped; the result must be what the stage gave,
    /// which was `a + (wet - a) * 0`: the same bits.
    #[test]
    fn skipping_the_wet_edge_stage_changes_nothing() {
        for i in 0..=1000 {
            let coverage = f64::from(i) / 1000.0;
            let o = PixelOpts { layer: [0.2, 0.3, 0.4, 0.8], coverage, colour: [0.9, 0.1, 0.5], ..PixelOpts::default() };
            let got = stroke_composite_pixel(&o);
            let mut a = clamp01(coverage);
            if a > 0.0 {
                let x: f64 = 4.5 * a;
                a += ((1.0 - (x.sin() / x).abs()) - a) * 0.0;
            }
            let f = clamp01(a);
            let want = [
                0.9 * f + 0.2 * (1.0 - f),
                0.1 * f + 0.3 * (1.0 - f),
                0.5 * f + 0.4 * (1.0 - f),
                f + 0.8 * (1.0 - f),
            ];
            assert!(got.iter().zip(want).all(|(g, w)| g.to_bits() == w.to_bits()), "coverage {coverage}");
        }
    }
}
