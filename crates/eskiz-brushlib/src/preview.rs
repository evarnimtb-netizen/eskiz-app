//! Stroke previews for the Brush Library panel.
//!
//! A sample S-curve is fed through eskiz-core's [`Stroke`] (smoothing, dynamics,
//! spacing and end taper). The dabs are then rasterised with the library brush's
//! own tip and grain textures and composited with eskiz-core's
//! [`compose::stroke_composite_pixel`].
//!
//! eskiz-core's built-in dab rasteriser only knows two tip shapes, so the
//! rasterisation step is reimplemented here to use the generated textures. It
//! follows the same maths as eskiz-core.

use crate::grains::GRAIN_SIZE;
use crate::LibraryBrush;
use eskiz_core::brushes::{Accumulation, Brush, GrainMode};
use eskiz_core::compose::{self, PixelOpts};
use eskiz_core::geometry::{Dab, PointerType, Sample, Stroke, StrokeOptions};
use image::{GrayImage, Rgba, RgbaImage};
use std::f64::consts::PI;

/// Paper colour behind previews (linear 0..1 RGB).
pub const PAPER: [f64; 3] = [0.965, 0.955, 0.925];
/// Ink colour for preview strokes.
pub const INK: [f64; 3] = [0.10, 0.10, 0.14];

/// Input samples in the sample stroke.
const SAMPLES: usize = 160;
/// Sample rate of the sample stroke, in Hz.
const SAMPLE_RATE: f64 = 120.0;

/// Render `brush` as an opaque RGBA preview of `w x h` pixels: a sample S-curve
/// drawn in [`INK`] over [`PAPER`].
pub fn preview_stroke(brush: &LibraryBrush, w: u32, h: u32) -> RgbaImage {
    let (wu, hu) = dims(w, h);
    let alpha = render_alpha(brush, wu, hu);
    RgbaImage::from_fn(wu as u32, hu as u32, |x, y| {
        let a = alpha[y as usize * wu + x as usize];
        let premul = [INK[0] * a, INK[1] * a, INK[2] * a, a];
        let px = compose::over_paper(premul, PAPER);
        Rgba([
            compose::to_u8(px[0]),
            compose::to_u8(px[1]),
            compose::to_u8(px[2]),
            255,
        ])
    })
}

/// The stroke's alpha coverage as an 8-bit mask of `w x h` pixels (white is
/// full coverage). Useful for measuring edges without the paper colour.
pub fn stroke_mask(brush: &LibraryBrush, w: u32, h: u32) -> GrayImage {
    let (wu, hu) = dims(w, h);
    let alpha = render_alpha(brush, wu, hu);
    GrayImage::from_fn(wu as u32, hu as u32, |x, y| {
        image::Luma([compose::to_u8(alpha[y as usize * wu + x as usize])])
    })
}

fn dims(w: u32, h: u32) -> (usize, usize) {
    (w.max(1) as usize, h.max(1) as usize)
}

/// Layer alpha (0..1) for the sample stroke, row-major.
fn render_alpha(lb: &LibraryBrush, w: usize, h: usize) -> Vec<f64> {
    let brush = fitted(&lb.brush, h);
    let dabs = sample_dabs(&brush, w, h);
    let coverage = splat(&dabs, &lb.tip, lb.grain.as_ref(), &brush, w, h);
    composite(&brush, lb.grain.as_ref(), &coverage, w, h)
}

/// Clamp the brush diameter so the largest dab fits comfortably in the preview.
fn fitted(brush: &Brush, h: usize) -> Brush {
    let mut b = brush.clone();
    let limit = 0.42 * h as f64;
    if b.size > limit {
        b.size = limit;
    }
    b
}

/// Run the S-curve sample stroke through eskiz-core and return the final dabs.
fn sample_dabs(brush: &Brush, w: usize, h: usize) -> Vec<Dab> {
    let (wf, hf) = (w as f64, h as f64);
    let mut stroke = Stroke::new(brush.clone(), StrokeOptions::default());
    for i in 0..SAMPLES {
        let t = i as f64 / (SAMPLES - 1) as f64;
        let ease = 0.5 - 0.5 * (PI * t).cos();
        let x = wf * (0.1 + 0.8 * t);
        let y = hf * (0.28 + 0.44 * ease + 0.03 * (3.0 * PI * t).sin());
        let pressure = 0.2 + 0.8 * (PI * t).sin().powf(0.8);
        stroke.add_sample(Sample {
            x,
            y,
            t: i as f64 / SAMPLE_RATE,
            pressure: Some(pressure),
            pointer: PointerType::Pen,
            ..Sample::default()
        });
    }
    stroke.finish().dabs
}

/// Texture lookup with GL-style texel centres and bilinear filtering. `wrap`
/// selects repeat addressing, otherwise clamp-to-edge. Returns 0..1.
fn sample_gray(img: &GrayImage, u: f64, v: f64, wrap: bool) -> f64 {
    let (w, h) = img.dimensions();
    let (wi, hi) = (i64::from(w), i64::from(h));
    let fx = u * f64::from(w) - 0.5;
    let fy = v * f64::from(h) - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let at = |x: f64, y: f64| -> f64 {
        let (mut xi, mut yi) = (x as i64, y as i64);
        if wrap {
            xi = xi.rem_euclid(wi);
            yi = yi.rem_euclid(hi);
        } else {
            xi = xi.clamp(0, wi - 1);
            yi = yi.clamp(0, hi - 1);
        }
        f64::from(img.get_pixel(xi as u32, yi as u32).0[0]) / 255.0
    };
    let top = at(x0, y0) + (at(x0 + 1.0, y0) - at(x0, y0)) * tx;
    let bottom = at(x0, y0 + 1.0) + (at(x0 + 1.0, y0 + 1.0) - at(x0, y0 + 1.0)) * tx;
    top + (bottom - top) * ty
}

/// Grain tone: optional invert, then contrast and brightness, clamped.
fn tone(g: f64, brush: &Brush) -> f64 {
    let g = if brush.invert_grain { 1.0 - g } else { g };
    ((brush.grain_contrast + 1.0) * (g - 0.5) + brush.grain_brightness + 0.5).clamp(0.0, 1.0)
}

/// Grain scale as a usable divisor (zero means 1).
fn grain_scale(brush: &Brush) -> f64 {
    if brush.grain_scale == 0.0 {
        1.0
    } else {
        brush.grain_scale
    }
}

/// Rasterise dabs into a `w x h` coverage buffer.
fn splat(
    dabs: &[Dab],
    tip: &GrayImage,
    grain: Option<&GrayImage>,
    brush: &Brush,
    w: usize,
    h: usize,
) -> Vec<f64> {
    let mut cov = vec![0.0f64; w * h];
    let moving = grain.filter(|_| brush.grain_mode == GrainMode::Moving);
    let gscale = grain_scale(brush);
    for d in dabs {
        let (hx, hy) = (d.sx / 2.0, d.sy / 2.0);
        if hx <= 0.0 || hy <= 0.0 {
            continue;
        }
        let (c, s) = (d.angle.cos(), d.angle.sin());
        let ex = hx * c.abs() + hy * s.abs();
        let ey = hx * s.abs() + hy * c.abs();
        let x0 = ((d.x - ex).floor() as i64 - 1).max(0);
        let x1 = ((d.x + ex).ceil() as i64 + 1).min(w as i64 - 1);
        let y0 = ((d.y - ey).floor() as i64 - 1).max(0);
        let y1 = ((d.y + ey).ceil() as i64 + 1).min(h as i64 - 1);
        let alpha = (d.opacity * d.flow).clamp(0.0, 1.0);
        for py in y0..=y1 {
            for px in x0..=x1 {
                let dx = px as f64 + 0.5 - d.x;
                let dy = py as f64 + 0.5 - d.y;
                let lx = dx * c + dy * s;
                let ly = -dx * s + dy * c;
                // One-pixel anti-aliased quad edge, so rotated tips do not stair-step.
                let edge =
                    (hx - lx.abs() + 0.5).clamp(0.0, 1.0) * (hy - ly.abs() + 0.5).clamp(0.0, 1.0);
                if edge <= 0.0 {
                    continue;
                }
                let u = lx / d.sx + 0.5;
                let v = ly / d.sy + 0.5;
                let mut value = sample_gray(tip, u, v, false);
                if let Some(g) = moving {
                    let t = tone(
                        sample_gray(g, u / gscale + d.grain_x, v / gscale + d.grain_y, true),
                        brush,
                    );
                    value *= (1.0 - brush.grain_depth) + t * brush.grain_depth;
                }
                let stamp_cov = value.clamp(0.0, 1.0) * alpha * edge;
                let idx = py as usize * w + px as usize;
                let a = cov[idx];
                cov[idx] = match brush.accumulation {
                    // Glaze: overlapping dabs never exceed the stroke opacity.
                    Accumulation::Glaze => a.max(stamp_cov),
                    Accumulation::Buildup => a + (1.0 - a) * stamp_cov,
                };
            }
        }
    }
    cov
}

/// Composite the coverage onto an empty layer with eskiz-core's stroke composite
/// and return the resulting alpha. Texturized grain is applied per pixel here.
fn composite(
    brush: &Brush,
    grain: Option<&GrayImage>,
    coverage: &[f64],
    w: usize,
    h: usize,
) -> Vec<f64> {
    let texturized = grain.filter(|_| brush.grain_mode != GrainMode::Moving);
    let tile = f64::from(GRAIN_SIZE) * grain_scale(brush);
    let mut out = vec![0.0f64; w * h];
    for (idx, &cov) in coverage.iter().enumerate() {
        if cov <= 0.0 {
            continue;
        }
        let (px, py) = (idx % w, idx / w);
        let grain_pair = texturized.map(|g| {
            let t = tone(
                sample_gray(g, (px as f64 + 0.5) / tile, (py as f64 + 0.5) / tile, true),
                brush,
            );
            (t, brush.grain_depth)
        });
        let opts = PixelOpts {
            coverage: cov,
            colour: INK,
            wet_edges: brush.wet_edges,
            threshold: brush.alpha_threshold,
            burnt: brush.burnt_edges,
            grain: grain_pair,
            ..PixelOpts::default()
        };
        out[idx] = compose::stroke_composite_pixel(&opts)[3];
    }
    out
}
