//! Wet mixing: pickup, carry, mixing and soft edges for wet brushes and the smudge tool.
//!
//! The engine already computes a per-dab `mixture`: dilution, fall-off, charge and
//! attack folded into one number). This module is what acts on it. The documented own text for the
//! mixer is terse, so the model below is Eskiz's, kept inside the documented definitions.
//!
//! # Model
//!
//! All mixing is done on **premultiplied** RGBA in 0..1, in the layer's stored colour space,
//! the same space [`crate::compose`] works in. Averaging and lerping premultiplied values is
//! the alpha-correct way to mix, so a clear area dilutes paint instead of darkening it.
//!
//! A wet stroke keeps, per path, a [`Carry`]: the canvas paint the brush has picked up.
//! For every dab:
//!
//! 1. **Pickup.** The layer under the dab, as it was at pen-down, is sampled ([`sample_under`]): a weighted average of
//!    premultiplied pixels, weighted by the dab's own tip (shape and moving grain), on a grid of
//!    at most [`SAMPLE_GRID`]² points so the cost does not grow with the brush size.
//! 2. **Carry.** First the carry loses paint with distance, `exp(-d / L)` with
//!    `L = diameter · (2 + 10 · charge)`: charge is how long the brush keeps what it holds.
//!    Then it picks up: `carry' = carry · (1 - pull · α_u) + pull · under`. Where the canvas is
//!    clear (`α_u = 0`) the carry is left alone, so colour picked up from a red area is carried
//!    onto bare canvas; where it is opaque this is a plain lerp toward the canvas colour.
//! 3. **Mix.** `mixture` is the share of fresh brush paint, shaped by **grade**:
//!    `s = mixture ^ 4^(1 - 2·grade)` (0.5 is linear, lower favours the carried paint, higher
//!    favours the brush). The dab paint is `P = s · brush + (1 - s) · carry`, premultiplied.
//!    (Canvas paint being pulled in also displaces some of the brush paint: the share becomes
//!    `s · (1 - ½ · pull · α_u · (1 - s))`, which leaves `s = 0` and `s = 1` alone.)
//!    Its alpha `P.a` scales the dab coverage, and its straight colour `P.rgb / P.a` is
//!    blended into the stroke's colour buffer by the share of coverage this dab adds.
//!    With `mixture = 1` (dilution 0, no fall-off) `P` is exactly the brush colour and the
//!    result is bit-identical to a normal brush.
//! 4. **Soft edges.** `wetness` (plus the dab's dynamics blur) softens the stroke's coverage
//!    around the dab with a box blur; `bleed` also lets it spread beyond the dab's edge.
//!
//! The stroke buffer therefore holds, per pixel, the coverage `A` and a colour delta from the
//! brush colour; the layer is `composite(before, base + delta, A)`, and
//! stays recomputable from the snapshot taken at pen-down.
//!
//! The smudge tool uses [`Carry`] and [`sample_under`] too, with no paint of its own
//! (`mixture = 0`) and a carry that never runs out.

use crate::brushes::{Brush, Wet};
use crate::canvas::{Layer, PaintOptions};
use crate::compose::{self, PixelOpts};
use crate::geometry::{clamp01, stroke_colour, Dab, Stroke};
use crate::raster::{dab_bounds, dab_pixel_rect, CoverageBuffer, DabRaster, CHUNK};

/// The pickup samples at most this many points along each axis of the dab.
pub const SAMPLE_GRID: usize = 24;
/// Largest blur radius, in pixels, of the wetness and bleed softening.
pub const MAX_SOFT_RADIUS: usize = 24;
/// How far beyond a dab's quad a wet dab can change pixels, in pixels.
pub const WET_MARGIN: usize = MAX_SOFT_RADIUS + 4;

/// Paint a brush holds that it picked up from the canvas: premultiplied RGBA in 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Carry {
    pub rgba: [f32; 4],
}

impl Carry {
    /// A carry holding exactly `rgba` (premultiplied).
    pub fn holding(rgba: [f32; 4]) -> Self {
        Self { rgba }
    }

    /// Pick up from `under` (premultiplied) at rate `pull` in 0..1:
    /// `carry · (1 - pull · α_under) + pull · under`.
    pub fn pick_up(&mut self, under: [f32; 4], pull: f32) {
        let pull = pull.clamp(0.0, 1.0);
        let keep = 1.0 - pull * under[3];
        for (c, u) in self.rgba.iter_mut().zip(under) {
            *c = *c * keep + pull * u;
        }
    }

    /// Lose paint: multiply by `keep` in 0..1.
    pub fn retain(&mut self, keep: f32) {
        for c in &mut self.rgba {
            *c *= keep;
        }
    }
}

/// Weighted average of the layer's premultiplied pixels over `x0..x1, y0..y1` (exclusive ends,
/// clipped to the layer), as 0..1 RGBA. `weight(x, y)` gives each pixel's weight; zero skips
/// the pixel. At most `max_axis` samples are taken along each axis, rows read as tile-contiguous
/// spans. `None` when the total weight is zero.
pub fn sample_under(
    layer: &Layer,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    max_axis: usize,
    mut weight: impl FnMut(usize, usize) -> f64,
) -> Option<[f32; 4]> {
    let (x1, y1) = (x1.min(layer.width), y1.min(layer.height));
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let step = (x1 - x0).max(y1 - y0).div_ceil(max_axis.max(1)).max(1);
    let off = step / 2;
    let mut sum = [0.0f64; 4];
    let mut total = 0.0f64;
    let scale = if layer.depth() == crate::pixel::Depth::U16 { 65535.0 } else { 255.0 };
    let mut y = y0 + off;
    while y < y1 {
        // First grid column at or after the span start.
        for (sx, span) in layer.row_spans(y, x0, x1) {
            let rel = sx.saturating_sub(x0 + off);
            let mut x = x0 + off + rel.div_ceil(step) * step;
            while x < sx + span.len {
                let w = weight(x, y);
                if w > 0.0 {
                    total += w;
                    let i = (x - sx) * 4;
                    if let Some(data) = span.data {
                        for c in 0..4 {
                            sum[c] += w * f64::from(data[i + c]);
                        }
                    } else if let Some(data) = span.data16 {
                        for c in 0..4 {
                            sum[c] += w * f64::from(data[i + c]);
                        }
                    }
                }
                x += step;
            }
        }
        y += step;
    }
    if total <= 0.0 {
        return None;
    }
    Some(sum.map(|s| (s / (total * scale)) as f32))
}

/// The exponent that turns `mixture` into the brush-paint share (see the module docs).
pub fn grade_exponent(grade: f64) -> f64 {
    4f64.powf(1.0 - 2.0 * clamp01(grade))
}

/// Brush-paint share for a dab with `mixture` at the given grade.
pub fn paint_share(mixture: f64, grade: f64) -> f64 {
    clamp01(mixture).powf(grade_exponent(grade))
}

/// The colour deltas of a stroke: three `f32` per pixel of a rectangle, stored in 64 x 64 chunks
/// that exist only where the stroke has been (a missing chunk is all zero; see
/// [`CoverageBuffer`]).
#[derive(Clone, Debug)]
struct DeltaBuffer {
    ox: i64,
    oy: i64,
    w: usize,
    h: usize,
    cols: usize,
    chunks: Vec<Option<Box<[f32]>>>,
}

impl DeltaBuffer {
    fn new(ox: i64, oy: i64, w: usize, h: usize) -> Self {
        let (cols, rows) = (w.div_ceil(CHUNK), h.div_ceil(CHUNK));
        Self { ox, oy, w, h, cols, chunks: vec![None; cols * rows] }
    }

    /// Chunk number and offset of the pixel's first channel, or `None` outside the rectangle.
    fn locate(&self, x: i64, y: i64) -> Option<(usize, usize)> {
        let (lx, ly) = (x - self.ox, y - self.oy);
        if lx < 0 || ly < 0 || lx >= self.w as i64 || ly >= self.h as i64 {
            return None;
        }
        let (lx, ly) = (lx as usize, ly as usize);
        Some((ly / CHUNK * self.cols + lx / CHUNK, (ly % CHUNK * CHUNK + lx % CHUNK) * 3))
    }

    /// The pixel's three channels for writing (its chunk is allocated); `None` outside.
    fn at_mut(&mut self, x: i64, y: i64) -> Option<&mut [f32]> {
        let (chunk, off) = self.locate(x, y)?;
        let data = self.chunks[chunk].get_or_insert_with(|| vec![0.0; CHUNK * CHUNK * 3].into_boxed_slice());
        Some(&mut data[off..off + 3])
    }

    /// The pixel's three channels; `None` outside the rectangle, zeros where nothing was laid.
    fn at(&self, x: i64, y: i64) -> Option<[f32; 3]> {
        let (chunk, off) = self.locate(x, y)?;
        Some(self.chunks[chunk].as_ref().map_or([0.0; 3], |c| [c[off], c[off + 1], c[off + 2]]))
    }
}

/// Per-path state of a wet stroke.
#[derive(Clone, Copy, Debug, Default)]
struct PathState {
    carry: Carry,
    last: Option<(f64, f64)>,
}

/// Wet-stroke state: the colour delta buffer, plus a carry for each path of the stroke.
///
/// Create one per stroke over the same rectangle as the stroke's [`CoverageBuffer`].
#[derive(Clone, Debug)]
pub struct WetStroke {
    wet: Wet,
    /// Straight colour minus the brush colour, per pixel.
    delta: DeltaBuffer,
    paths: Vec<PathState>,
}

/// An exclusive pixel rectangle `(x0, y0, x1, y1)`.
pub type Rect = (usize, usize, usize, usize);

/// What [`WetStroke::composite`] needs besides the stroke buffers.
pub struct ComposeCtx<'a> {
    /// The stroke colour the deltas are relative to.
    pub base: [f64; 3],
    pub brush: &'a Brush,
    pub raster: &'a DabRaster,
    pub alpha_lock: bool,
    /// Optional selection: `(x, y, coverage) -> coverage`.
    pub clip: Option<&'a dyn Fn(usize, usize, f64) -> f64>,
}

impl WetStroke {
    /// State for a stroke with `paths` paths over the canvas rectangle starting at `(ox, oy)`.
    /// The colour buffer is allocated in chunks, as the stroke reaches them.
    pub fn new(ox: i64, oy: i64, w: usize, h: usize, wet: &Wet, paths: usize) -> Self {
        Self {
            wet: *wet,
            delta: DeltaBuffer::new(ox, oy, w, h),
            paths: vec![PathState::default(); paths.max(1)],
        }
    }

    /// Bytes of colour-delta memory allocated so far (whole chunks), for tests and diagnostics.
    pub fn allocated_bytes(&self) -> usize {
        self.delta.chunks.iter().flatten().count() * CHUNK * CHUNK * 3 * 4
    }

    /// Carry of path `i`, for tests and inspection.
    pub fn carry(&self, path: usize) -> Carry {
        self.paths[path.min(self.paths.len() - 1)].carry
    }

    /// Lay one dab of path `path`: pick up from `under`, mix, and accumulate into `cov` and the
    /// colour delta. `under` is the layer as it was at pen-down, so a stroke never picks up its
    /// own paint (one stroke is one glaze) and the result does not depend on how
    /// often the caller recomposites. `base` is the stroke colour. Returns the rectangle whose
    /// pixels changed (the dab plus any softening), or `None` when nothing did. Recomposite it
    /// with [`WetStroke::composite`].
    pub fn apply_dab(
        &mut self,
        path: usize,
        d: &Dab,
        base: [f64; 3],
        raster: &DabRaster,
        cov: &mut CoverageBuffer,
        under: &Layer,
    ) -> Option<Rect> {
        if !(d.sx > 0.0 && d.sy > 0.0) {
            return None;
        }
        let path = path.min(self.paths.len() - 1);
        let (bx0, by0, bx1, by1) = dab_bounds(d);
        let (cw, ch) = (under.width as i64, under.height as i64);
        let x0 = (bx0.floor() as i64 - 1).max(0);
        let y0 = (by0.floor() as i64 - 1).max(0);
        let x1 = (bx1.ceil() as i64 + 2).min(cw);
        let y1 = (by1.ceil() as i64 + 2).min(ch);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let (c, s) = (d.angle.cos(), d.angle.sin());

        // 1. Pickup: the layer under the dab, weighted by the dab's tip.
        let sampled = sample_under(under, x0 as usize, y0 as usize, x1 as usize, y1 as usize, SAMPLE_GRID, |x, y| {
            raster.tip_value(d, c, s, x as i64, y as i64).map_or(0.0, clamp01)
        });

        // 2. Carry: lose paint with distance, then pick up.
        let wet = self.wet;
        let st = &mut self.paths[path];
        if let Some((lx, ly)) = st.last {
            let dist = (d.x - lx).hypot(d.y - ly);
            let diameter = ((d.sx + d.sy) * 0.5).max(1.0);
            let length = diameter * (2.0 + 10.0 * clamp01(wet.load_amount));
            st.carry.retain((-dist / length).exp() as f32);
        }
        st.last = Some((d.x, d.y));
        if let Some(u) = sampled {
            st.carry.pick_up(u, clamp01(wet.pull) as f32);
        }

        // 3. Mix the brush paint with the carry. Canvas paint that is being pulled in displaces
        // some brush paint: up to half of the diluted part at full pull over opaque canvas.
        let under_alpha = sampled.map_or(0.0, |u| f64::from(u[3]));
        let share = paint_share(d.mixture, wet.grade);
        let share = share * (1.0 - 0.5 * clamp01(wet.pull) * under_alpha * (1.0 - share));
        let cr = st.carry.rgba.map(f64::from);
        let paint = [
            share * base[0] + (1.0 - share) * cr[0],
            share * base[1] + (1.0 - share) * cr[1],
            share * base[2] + (1.0 - share) * cr[2],
            share + (1.0 - share) * cr[3],
        ];
        if paint[3] <= 1e-6 {
            return None;
        }
        let dab_delta = [
            (paint[0] / paint[3] - base[0]) as f32,
            (paint[1] / paint[3] - base[1]) as f32,
            (paint[2] / paint[3] - base[2]) as f32,
        ];

        // Accumulate coverage (alpha scaled by the paint's alpha) and blend the colour in by
        // the share of coverage this dab contributes.
        let delta = &mut self.delta;
        cov.splat_one_with(d, raster, paint[3], |px, py, _old, new, cov_here| {
            if new <= 0.0 {
                return;
            }
            let Some(px_delta) = delta.at_mut(px, py) else {
                return;
            };
            let k = (cov_here as f32 / new).clamp(0.0, 1.0);
            for ch in 0..3 {
                px_delta[ch] += (dab_delta[ch] - px_delta[ch]) * k;
            }
        });
        let mut rect = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);

        // 4. Soft edges.
        let amount = clamp01(wet.wetness + d.blur);
        let bleed = clamp01(wet.bleed);
        if amount > 0.0 || bleed > 0.0 {
            if let Some(r) = self.soften(d, raster, cov, amount, bleed, dab_delta) {
                rect = (rect.0.min(r.0), rect.1.min(r.1), rect.2.max(r.2), rect.3.max(r.3));
            }
        }
        Some(rect)
    }

    /// Blur the stroke's coverage around dab `d` (wetness) and let it spread outwards (bleed).
    fn soften(
        &mut self,
        d: &Dab,
        raster: &DabRaster,
        cov: &mut CoverageBuffer,
        amount: f64,
        bleed: f64,
        dab_delta: [f32; 3],
    ) -> Option<Rect> {
        let diameter = ((d.sx + d.sy) * 0.5).max(1.0);
        let radius = ((amount * 0.12 + bleed * 0.3) * diameter).round().clamp(1.0, MAX_SOFT_RADIUS as f64) as i64;
        let (bx0, by0, bx1, by1) = dab_bounds(d);
        let (cx0, cy0, cw, ch) = cov.bounds();
        let rx0 = (bx0.floor() as i64 - 1 - radius).max(cx0);
        let ry0 = (by0.floor() as i64 - 1 - radius).max(cy0);
        let rx1 = (bx1.ceil() as i64 + 2 + radius).min(cx0 + cw as i64);
        let ry1 = (by1.ceil() as i64 + 2 + radius).min(cy0 + ch as i64);
        if rx0 >= rx1 || ry0 >= ry1 {
            return None;
        }
        let (w, h) = ((rx1 - rx0) as usize, (ry1 - ry0) as usize);
        let mut a = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                a[y * w + x] = cov.get(rx0 + x as i64, ry0 + y as i64) as f32;
            }
        }
        let blurred = box_blur(&a, w, h, radius as usize);

        // The influence mask is the dab's own tip, grown by the blur radius.
        let grown = Dab {
            sx: d.sx + 2.0 * radius as f64,
            sy: d.sy + 2.0 * radius as f64,
            ..*d
        };
        let (c, s) = (grown.angle.cos(), grown.angle.sin());
        let amount = amount as f32;
        let bleed = bleed as f32;
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (rx0 + x as i64, ry0 + y as i64);
                let mask = raster.tip_value(&grown, c, s, px, py).map_or(0.0, clamp01) as f32;
                if mask <= 0.0 {
                    continue;
                }
                let old = a[y * w + x];
                let ab = blurred[y * w + x];
                let soft = old + (ab - old) * amount * mask;
                let target = (ab * (1.0 + 1.5 * bleed)).min(1.0);
                let new = soft + (soft.max(target) - soft) * bleed * mask;
                if (new - old).abs() > 1e-6 {
                    cov.set(px, py, new);
                    if new > old {
                        if let Some(dst) = self.delta.at_mut(px, py) {
                            let k = (new - old) / new;
                            for (dst, target) in dst.iter_mut().zip(dab_delta) {
                                *dst += (target - *dst) * k;
                            }
                        }
                    }
                }
            }
        }
        Some((rx0 as usize, ry0 as usize, rx1 as usize, ry1 as usize))
    }

    /// Write `rect` of `layer` from `before` plus the stroke buffers (with the
    /// per-pixel stroke colour). Reads `before`, writes `layer`.
    pub fn composite(&self, layer: &mut Layer, before: &Layer, cov: &CoverageBuffer, rect: Rect, cx: &ComposeCtx<'_>) {
        let brush = cx.brush;
        let f = |x: usize, y: usize, old: [f64; 4]| -> [f64; 4] {
            let (xi, yi) = (x as i64, y as i64);
            let mut coverage = cov.get(xi, yi);
            if let Some(clip) = cx.clip {
                coverage = clip(x, y, coverage);
            }
            let mut colour = cx.base;
            if let Some(px_delta) = self.delta.at(xi, yi) {
                for (c, d) in colour.iter_mut().zip(px_delta) {
                    *c += f64::from(d);
                }
            }
            let o = PixelOpts {
                layer: old,
                coverage,
                colour,
                wet_edges: brush.wet_edges,
                threshold: brush.alpha_threshold,
                burnt: brush.burnt_edges,
                grain: cx.raster.texturized_tone(xi, yi),
                alpha_lock: cx.alpha_lock,
                ..PixelOpts::default()
            };
            compose::stroke_composite_pixel(&o)
        };
        // The 8-bit loop is the original one (its own closure, so it stays as tight as it was);
        // the 16-bit loop runs the same maths and rounds once, to 16 bits.
        if layer.depth() == crate::pixel::Depth::U16 {
            layer.update_rect_unit(Some(before), rect.0, rect.1, rect.2, rect.3, f);
        } else {
            layer.update_rect(Some(before), rect.0, rect.1, rect.2, rect.3, |x, y, old| {
                let out = f(x, y, compose::premul_from_u8(&old));
                [compose::to_u8(out[0]), compose::to_u8(out[1]), compose::to_u8(out[2]), compose::to_u8(out[3])]
            });
        }
    }
}

/// Separable box blur of radius `r` with clamped edges.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let n = (2 * r + 1) as f32;
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut acc: f32 = (0..=2 * r).map(|i| row[i.saturating_sub(r).min(w - 1)]).sum();
        for x in 0..w {
            tmp[y * w + x] = acc / n;
            let add = row[(x + r + 1).min(w - 1)];
            let sub = row[x.saturating_sub(r)];
            acc += add - sub;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for x in 0..w {
        let mut acc: f32 = (0..=2 * r).map(|i| tmp[i.saturating_sub(r).min(h - 1) * w + x]).sum();
        for y in 0..h {
            out[y * w + x] = acc / n;
            let add = tmp[(y + r + 1).min(h - 1) * w + x];
            let sub = tmp[y.saturating_sub(r) * w + x];
            acc += add - sub;
        }
    }
    out
}

/// Whether `brush` paints with wet mixing (erasing never does).
pub fn is_wet(brush: &Brush, erase: bool) -> bool {
    brush.wet.enabled && !erase
}

/// Paint a finished stroke onto `layer` dab by dab, so each dab picks up what the earlier
/// ones left. Used by [`Layer::paint_stroke_with_raster`] for wet brushes.
pub(crate) fn paint_stroke(layer: &mut Layer, stroke: &Stroke, opts: PaintOptions, raster: &DabRaster) {
    let dabs = stroke.dabs();
    let Some((x0, y0, x1, y1)) = dab_pixel_rect(dabs, layer.width, layer.height) else {
        return;
    };
    let brush = stroke.brush();
    let m = WET_MARGIN as i64;
    let (bx0, by0) = ((x0 - m).max(0), (y0 - m).max(0));
    let (bx1, by1) = ((x1 + 1 + m).min(layer.width as i64), (y1 + 1 + m).min(layer.height as i64));
    let (bw, bh) = ((bx1 - bx0) as usize, (by1 - by0) as usize);
    let mut cov = CoverageBuffer::new(bx0, by0, bw, bh);
    let mut wet = WetStroke::new(bx0, by0, bw, bh, &brush.wet, 1);
    let base = stroke_colour(brush, opts.colour, stroke.meta());
    let cx = ComposeCtx {
        base,
        brush,
        raster,
        alpha_lock: opts.alpha_lock,
        clip: None,
    };
    let before = layer.clone();
    let mut dirty: Option<Rect> = None;
    for d in dabs {
        if let Some(r) = wet.apply_dab(0, d, base, raster, &mut cov, &before) {
            dirty = Some(union(dirty, r));
        }
    }
    if let Some(rect) = dirty {
        wet.composite(layer, &before, &cov, rect, &cx);
    }
}

/// Smallest rectangle holding `a` (when there is one) and `b`.
pub fn union(a: Option<Rect>, b: Rect) -> Rect {
    a.map_or(b, |a| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
}
