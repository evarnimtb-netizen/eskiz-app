//! Glue between an [`Adjustment`] and the CPU reference filters of `eskiz-filters`.
//!
//! The parameter indices used here are the ones `AdjustKind::params` lists, so a change to the
//! table there must change this file too. Every function takes an optional mask (the Pencil
//! paint, the selection, or both multiplied) and passes it on unchanged: `eskiz-filters` applies
//! the shared mix rule itself, and the pyramid blur and gradient map use the mask in their own
//! ways.

use eskiz_filters::{
    balance, bloom, chromatic, clone, curves, glitch, gradient_map, halftone, motion, noise, perspective, pyramid, recolor,
    sharpen, Image,
};

use super::{AdjustKind, Adjustment, Quality};
use crate::pixel::Chan;

/// Render `adj` over premultiplied RGBA `src` (8 or 16 bits) through `eskiz-filters`.
///
/// Preview quality uses the reduced-scale variants where `eskiz-filters` has them (motion and
/// perspective blur). Types that this module does not handle return the source unchanged.
pub fn render<T: Chan>(adj: &Adjustment, src: &[T], width: usize, height: usize, quality: Quality, mask: Option<&[f32]>) -> Vec<T> {
    // A clone with nothing painted, or a pencil with nothing painted, changes nothing.
    if matches!(adj.kind, AdjustKind::Clone) && mask.is_none() {
        return src.to_vec();
    }
    if adj.kind == AdjustKind::Curves && T::MAX == 255.0 {
        return render_curves_lut(adj, src, mask);
    }
    if pointwise(adj.kind) && width > 0 {
        return render_strips(adj, src, width, quality, mask);
    }
    render_whole(adj, src, width, height, quality, mask)
}

/// Does each output pixel depend only on the same input pixel (and the same mask value)? Those
/// types can be rendered a band of rows at a time with identical results.
pub fn pointwise(kind: AdjustKind) -> bool {
    matches!(kind, AdjustKind::Curves | AdjustKind::GradientMap | AdjustKind::Recolor | AdjustKind::ColourBalance)
}

/// Curves on 8-bit data through lookup tables, with exactly the arithmetic of the `f32` path.
///
/// Each channel's output depends only on the pixel's alpha and that channel's value (the
/// unpremultiplied colour is `value / alpha`), so a table over the 256 x 256 pairs holds
/// `CurveSet::channel(k, value / alpha)` for each of r, g and b. A pixel is then three lookups,
/// a multiply by alpha, the mask mix and the rounding the other path does, in the same order, so
/// the bytes are the same. (The tables are built from the same expressions: 196 608 curve
/// lookups, a few milliseconds.)
fn render_curves_lut<T: Chan>(adj: &Adjustment, src: &[T], mask: Option<&[f32]>) -> Vec<T> {
    let pts = adj.curve_points();
    let curve = |k: usize| curves::Curve::from_table(pts.table(k));
    let set = curves::CurveSet { master: curve(0), red: curve(1), green: curve(2), blue: curve(3) };
    let mut lut = vec![[0.0f32; 3]; 256 * 256];
    for a in 0..256usize {
        let af = a as f32 / 255.0;
        for v in 0..256usize {
            let c = if af <= 0.0 { 0.0 } else { (v as f32 / 255.0) / af };
            lut[(a << 8) | v] = [set.channel(0, c), set.channel(1, c), set.channel(2, c)];
        }
    }
    let mut out = vec![T::default(); src.len()];
    // Bands of whole pixels (4096 per row of the band), so the mask index follows the band.
    const BAND: usize = 4096;
    super::par_rows(src, &mut out, BAND * 4, |first, s, o| {
        for (j, (px, dst)) in s.as_chunks::<4>().0.iter().zip(o.as_chunks_mut::<4>().0.iter_mut()).enumerate() {
            let t = mask.map_or(1.0, |m| m[first * BAND + j].clamp(0.0, 1.0));
            let a = px[3].raw() as usize;
            let af = px[3].to_f32() / T::MAXF;
            for k in 0..3 {
                let sv = px[k].to_f32() / T::MAXF;
                let adjusted = lut[(a << 8) | px[k].raw() as usize][k] * af;
                let d = sv + (adjusted - sv) * t;
                dst[k] = T::from_f32_round(d.clamp(0.0, 1.0) * T::MAXF);
            }
            let d = af; // alpha is unchanged: af + (af - af) * t
            dst[3] = T::from_f32_round(d.clamp(0.0, 1.0) * T::MAXF);
        }
    });
    out
}

/// Rows per band of [`render_strips`]: 64 rows of an 8000-pixel canvas are 8 MiB of `f32`.
const STRIP_ROWS: usize = 64;

/// Render a pointwise type one band of rows at a time, bands in parallel: only a band is ever
/// converted to `f32`, so the working set is the output plus a few MiB per thread.
pub fn render_strips<T: Chan>(adj: &Adjustment, src: &[T], width: usize, quality: Quality, mask: Option<&[f32]>) -> Vec<T> {
    let mut out = vec![T::default(); src.len()];
    let row = width * 4;
    super::par_rows(src, &mut out, row, |first, s, o| {
        for (b, (sb, ob)) in s.chunks(STRIP_ROWS * row).zip(o.chunks_mut(STRIP_ROWS * row)).enumerate() {
            let y0 = first + b * STRIP_ROWS;
            let rows = sb.len() / row;
            let m = mask.map(|m| &m[y0 * width..(y0 + rows) * width]);
            ob.copy_from_slice(&render_whole(adj, sb, width, rows, quality, m));
        }
    });
    out
}

/// Render over the whole buffer through one `f32` [`Image`] (the reference path).
pub fn render_whole<T: Chan>(adj: &Adjustment, src: &[T], width: usize, height: usize, quality: Quality, mask: Option<&[f32]>) -> Vec<T> {
    let v = |i: usize| adj.value(i);
    let f = |i: usize| adj.value(i) as f32;
    // The filters run in f32 on 0..1 premultiplied values, so a 16-bit layer goes in and out
    // without an 8-bit round trip.
    let img = Image { width, height, data: src.iter().map(|&v| v.to_f32() / T::MAXF).collect() };
    let out = match adj.kind {
        AdjustKind::GaussianBlur => {
            // Only Pencil mode gets here: the pyramid blur replaces the Gaussian.
            pyramid::apply(&img, mask, &pyramid::PyramidParams { amount: f(0) })
        }
        AdjustKind::MotionBlur => {
            let p = motion::MotionParams { vector: [f(0), f(1)], amount: f(2) };
            match quality {
                Quality::Preview => motion::preview(&img, mask, &p),
                Quality::Full => motion::apply(&img, mask, &p),
            }
        }
        AdjustKind::PerspectiveBlur => {
            let p = perspective::PerspectiveParams { origin: [f(0), f(1)], direction: [f(2), f(3)], amount: f(4) };
            match quality {
                Quality::Preview => perspective::preview(&img, mask, &p),
                Quality::Full => perspective::apply(&img, mask, &p),
            }
        }
        AdjustKind::Sharpen => sharpen::apply(&img, mask, &sharpen::SharpenParams { amount: f(0) }),
        AdjustKind::Noise => {
            let p = noise::NoiseParams {
                scale: f(0),
                amount: f(1),
                octaves: f(2),
                turbulence: f(3),
                kind: match v(4) as usize {
                    1 => noise::NoiseKind::Absolute,
                    2 => noise::NoiseKind::Billowy,
                    _ => noise::NoiseKind::Signed,
                },
                multichannel: v(5) >= 0.5,
                combine: if v(6) >= 0.5 { noise::CombineMode::Mix } else { noise::CombineMode::Additive },
                seed: 0,
            };
            noise::apply(&img, mask, &p)
        }
        AdjustKind::ChromaticAberration => {
            if v(0) < 0.5 {
                let p = chromatic::AxialParams {
                    origin: [f(1), f(2)],
                    amount: [f(3), f(4)],
                    inverse_range: f(5),
                    falloff: f(6),
                    transition: f(7),
                };
                chromatic::apply_axial(&img, mask, &p)
            } else {
                let p = chromatic::TransverseParams { offset: [f(3), f(4)], transparency: f(8), strength: f(9) };
                chromatic::apply_transverse(&img, mask, &p)
            }
        }
        AdjustKind::Glitch => {
            let p = glitch::GlitchParams { preset: v(0) as u8, amount: f(1), seed: 0.0, cells: None };
            glitch::apply(&img, mask, &p)
        }
        AdjustKind::Halftone => {
            let p = halftone::HalftoneParams {
                amount: f(0),
                softness: f(1),
                anti_alias: f(2),
                overlay: v(3) >= 0.5,
                white_fill: v(4) >= 0.5,
            };
            halftone::apply(&img, mask, &p)
        }
        AdjustKind::Bloom => {
            // The burn control n maps to b = 1.3 n - 0.3.
            let p = bloom::BloomParams {
                upper: f(0),
                lower: f(1),
                burn: 1.3 * f(2) - 0.3,
                spread: f(3),
                amount: f(4),
                preserve_alpha: false,
            };
            bloom::apply(&img, mask, &p)
        }
        AdjustKind::ColourBalance => {
            let zone = |z: usize| [f(3 * z), f(3 * z + 1), f(3 * z + 2)];
            let p = balance::BalanceParams { shadows: zone(0), midtones: zone(1), highlights: zone(2) };
            balance::apply(&img, mask, &p)
        }
        AdjustKind::Curves => {
            let c = adj.curve_points();
            let curve = |k: usize| curves::Curve::from_table(c.table(k));
            let target = curves::CurveSet { master: curve(0), red: curve(1), green: curve(2), blue: curve(3) };
            curves::apply(&img, mask, &curves::CurvesParams { target, blend: 1.0, blend_source: None })
        }
        AdjustKind::GradientMap => {
            let table = adj.gradient_stops().table();
            // The starting table is the target itself: commits are exact.
            let p = gradient_map::GradientMapParams { start: table.clone(), target: table, amount: f(0), blend: 1.0 };
            gradient_map::apply(&img, mask, &p)
        }
        AdjustKind::Recolor => {
            let p = recolor::RecolorParams {
                source: [f(0), f(1), f(2)],
                target: [f(3), f(4), f(5)],
                tolerance: f(6),
            };
            recolor::apply(&img, mask, &p)
        }
        AdjustKind::Clone => {
            let p = clone::CloneParams { offset: [f(3), f(4)], opacity: f(2) };
            clone::apply(&img, mask, &p)
        }
        // Opacity and HSB run in `filters/mod.rs` (`render` never sends them here).
        AdjustKind::Opacity | AdjustKind::Hsb => return src.to_vec(),
    };
    out.data.iter().map(|&v| T::from_f32_round(v.clamp(0.0, 1.0) * T::MAXF)).collect()
}
