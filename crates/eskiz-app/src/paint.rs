//! Stroke engine: live preview while the pointer moves, exact commit on pen-up.
//!
//! A stroke keeps a snapshot of its layer from before pen-down (a cheap copy-on-write clone:
//! tiles are shared until the stroke paints on them) and a stroke-wide coverage buffer. Each
//! committed dab is splatted into the coverage, and only the pixels it touches are
//! recomposited from the snapshot. Recompositing from the
//! saved copy (and not from the already-painted pixels) avoids double compositing.
//! The result matches `Layer::paint_stroke_with` on the finished stroke exactly.
//! The pixel maths is the same as `Layer::paint_stroke_with`.

use std::sync::Arc;

use eskiz_core::brushes::Brush;
use eskiz_core::compose::{self, PixelOpts};
use eskiz_core::geometry::{stroke_colour, Dab, Sample, Stroke, StrokeOptions};
use eskiz_core::raster::{dab_pixel_rect, CoverageBuffer, DabRaster};
use eskiz_core::canvas::Layer;
use eskiz_core::wet::{self, ComposeCtx, WetStroke};

use eskiz_select::Selection;

use crate::command::{Change, Command};
use crate::history::PixelRect;

/// Settings for one stroke, as chosen in the UI.
#[derive(Clone, Debug)]
pub struct StrokeSettings {
    pub brush: Brush,
    /// RGB in 0..1.
    pub colour: [f64; 3],
    /// Erase instead of paint.
    pub erase: bool,
    /// Tip and grain textures for the brush (see `DabRaster::with_textures`). `None` uses the
    /// brush's built-in tip.
    pub raster: Option<Arc<DabRaster>>,
}

struct Active {
    layer_id: u64,
    width: usize,
    height: usize,
    /// One stroke per path. A Guided Layer symmetry has several; they share the coverage
    /// buffer and become one undo entry.
    strokes: Vec<Stroke>,
    /// The tip and grain rasters (textured library brushes, or the built-in tip).
    raster: Arc<DabRaster>,
    coverage: CoverageBuffer,
    /// The layer as it was at pen-down. Shares its tiles with the layer until they are painted.
    before: Layer,
    colour: [f64; 3],
    erase: bool,
    /// Keep each pixel's alpha.
    alpha_lock: bool,
    /// Union of every rectangle recomposited so far.
    rect: Option<PixelRect>,
    /// The active selection. Each pixel's stroke coverage is multiplied by it.
    clip: Option<Arc<Selection>>,
    /// Wet-mix state when the brush mixes (see `eskiz_core::wet`): its colour buffer and carries.
    wet: Option<WetStroke>,
    /// The committed dabs of each path, in order, so pen-up can tell whether the end taper
    /// changed any of them (a wet stroke depends on the order, so it is replayed if so).
    live_dabs: Vec<Vec<Dab>>,
}

/// Runs at most one stroke at a time.
#[derive(Default)]
pub struct PaintEngine {
    active: Option<Active>,
    /// Rotation to add to pen azimuth so tilt is measured in canvas space:
    /// minus the view rotation, in radians. Read when a stroke begins.
    pub canvas_rotation: f64,
}

impl PaintEngine {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Start a stroke on `layer` (whose id is `layer_id`). `seed` makes the stroke's
    /// randomness reproducible. With `alpha_lock` the layer keeps each pixel's alpha.
    /// Returns false if a stroke is already running.
    pub fn begin(
        &mut self,
        layer: &Layer,
        layer_id: u64,
        settings: &StrokeSettings,
        seed: u32,
        alpha_lock: bool,
    ) -> bool {
        self.begin_paths(layer, layer_id, settings, seed, alpha_lock, 1)
    }

    /// Like [`begin`](Self::begin), but the stroke has `paths` paths that paint together:
    /// [`push_paths`](Self::push_paths) feeds one sample to each. They share one undo entry.
    pub fn begin_paths(
        &mut self,
        layer: &Layer,
        layer_id: u64,
        settings: &StrokeSettings,
        seed: u32,
        alpha_lock: bool,
        paths: usize,
    ) -> bool {
        if self.active.is_some() {
            return false;
        }
        let (width, height) = (layer.width, layer.height);
        let strokes: Vec<Stroke> = (0..paths.max(1))
            .map(|_| {
                Stroke::new(
                    settings.brush.clone(),
                    StrokeOptions {
                        seed,
                        canvas_rotation: self.canvas_rotation,
                        ..StrokeOptions::default()
                    },
                )
            })
            .collect();
        // Colour dynamics: one jittered colour for the whole stroke. Every path shares
        // the seed, so they share the offsets too.
        let colour = stroke_colour(&settings.brush, settings.colour, strokes[0].meta());
        let wet = wet::is_wet(&settings.brush, settings.erase)
            .then(|| WetStroke::new(0, 0, width, height, &settings.brush.wet, strokes.len()));
        let live_dabs = vec![Vec::new(); strokes.len()];
        self.active = Some(Active {
            layer_id,
            width,
            height,
            strokes,
            raster: settings
                .raster
                .clone()
                .unwrap_or_else(|| Arc::new(DabRaster::new(&settings.brush))),
            coverage: CoverageBuffer::new(0, 0, width, height),
            before: layer.clone(),
            colour,
            erase: settings.erase,
            alpha_lock,
            rect: None,
            clip: None,
            wet,
            live_dabs,
        });
        true
    }

    /// Clip the stroke in progress to `selection`. Call right after [`PaintEngine::begin`].
    pub fn set_clip(&mut self, selection: Arc<Selection>) {
        if let Some(a) = self.active.as_mut() {
            a.clip = Some(selection);
        }
    }

    /// Feed one sample. Returns the canvas rectangle that changed, for the caller to
    /// refresh on screen.
    pub fn push(&mut self, layer: &mut Layer, sample: Sample) -> Option<PixelRect> {
        self.push_paths(layer, std::slice::from_ref(&sample))
    }

    /// Feed one sample to each path of the stroke: `samples[i]` goes to path `i`. Returns the
    /// canvas rectangle that changed.
    pub fn push_paths(&mut self, layer: &mut Layer, samples: &[Sample]) -> Option<PixelRect> {
        let a = self.active.as_mut()?;
        let mut dabs = Vec::new();
        let mut per_path = Vec::new();
        for (i, (stroke, sample)) in a.strokes.iter_mut().zip(samples).enumerate() {
            let committed = stroke.add_sample(*sample).committed_dabs;
            dabs.extend(committed.iter().copied());
            per_path.push((i, committed));
        }
        let rect = if a.wet.is_some() {
            let rect = a.lay_wet_dabs(&per_path)?;
            for (i, d) in per_path {
                a.live_dabs[i].extend(d);
            }
            rect
        } else {
            let rect = dab_rect(&dabs, a.width, a.height)?;
            a.coverage.splat(&dabs, &a.raster);
            for (i, d) in per_path {
                a.live_dabs[i].extend(d);
            }
            rect
        };
        a.composite(layer, rect);
        a.rect = Some(a.rect.map_or(rect, |r| r.union(rect)));
        Some(rect)
    }

    /// Pen-up. Applies the end taper, recomposites what it changed, and returns the undo entry.
    /// `None` when the stroke painted nothing.
    ///
    /// The taper only changes the last dabs, so a one-path dry stroke re-splats and recomposites
    /// just the rectangle those dabs touch (nothing at all when none changed). Wet strokes and
    /// strokes with several paths replay everything: each wet dab depends on the ones before it,
    /// and paths are laid in a different order than the one the coverage was built in.
    pub fn end(&mut self, layer: &mut Layer) -> Option<Command> {
        self.end_with(layer, true)
    }

    fn end_with(&mut self, layer: &mut Layer, tail_only: bool) -> Option<Command> {
        let mut a = self.active.take()?;
        let mut finals: Vec<Vec<Dab>> = Vec::new();
        for stroke in a.strokes.iter_mut() {
            finals.push(stroke.finish().dabs);
        }
        let rect = if a.wet.is_some() {
            if finals == a.live_dabs {
                // No dab changed: what is on the layer is already the final stroke.
                a.rect?
            } else {
                // The end taper changed dabs already laid, and each wet dab depends on the ones
                // before it: put the layer back and lay the whole stroke again.
                let old = a.rect;
                a.coverage = CoverageBuffer::new(0, 0, a.width, a.height);
                let brush = a.strokes[0].brush();
                a.wet = Some(WetStroke::new(0, 0, a.width, a.height, &brush.wet, finals.len()));
                let paths: Vec<(usize, Vec<Dab>)> = finals.into_iter().enumerate().collect();
                let new = a.lay_wet_dabs(&paths);
                let rect = [old, new].into_iter().flatten().reduce(PixelRect::union)?;
                layer.copy_tiles_in_rect_from(&a.before, rect.x, rect.y, rect.w, rect.h);
                a.composite(layer, rect);
                rect
            }
        } else if tail_only && finals.len() == 1 && a.rect.is_some() {
            a.replay_tail(layer, &finals[0])?
        } else {
            let dabs: Vec<Dab> = finals.into_iter().flatten().collect();
            // The end taper can change dabs already shown, so rebuild the coverage from the
            // final list before the last composite.
            a.coverage.clear_all();
            a.coverage.splat(&dabs, &a.raster);
            let final_rect = dab_rect(&dabs, a.width, a.height);
            let rect = [a.rect, final_rect].into_iter().flatten().reduce(PixelRect::union)?;
            a.composite(layer, rect);
            rect
        };
        // An erase can leave tiles empty: free them so they are not stored as undo data.
        layer.compact_rect(rect.x, rect.y, rect.w, rect.h);
        // Tiles the stroke's bounding box reached but never changed were copied on first
        // touch: hand the copies back so only really painted tiles are separate memory.
        layer.share_unchanged_in_rect(&a.before, rect.x, rect.y, rect.w, rect.h);

        Some(Command::single(
            "Stroke",
            Change::tiles_between(a.layer_id, &a.before, layer, rect),
        ))
    }

    /// Abandon the stroke and restore the pixels it changed. Returns the restored rectangle.
    pub fn cancel(&mut self, layer: &mut Layer) -> Option<PixelRect> {
        let a = self.active.take()?;
        let rect = a.rect?;
        layer.copy_tiles_in_rect_from(&a.before, rect.x, rect.y, rect.w, rect.h);
        Some(rect)
    }
}

impl Active {
    /// Pen-up for a one-path dry stroke: bring the coverage and the layer from the live dabs to
    /// `finals`. Only the dabs from the first one that differs on (compared exactly, so the
    /// result is bit-for-bit the same as replaying everything) can have changed any pixel, and
    /// they all lie in `tail`, the rectangle of the old and the new tail. Inside it the coverage
    /// is cleared and rebuilt from the earlier dabs that reach it and then the new tail, in
    /// their original order, so every pixel sees the same dabs in the same order as in a full
    /// replay. Returns the stroke's whole rectangle (for the undo entry).
    fn replay_tail(&mut self, layer: &mut Layer, finals: &[Dab]) -> Option<PixelRect> {
        let live = &self.live_dabs[0];
        let from = live.iter().zip(finals).take_while(|(l, f)| l == f).count();
        let stroke_rect = self.rect?;
        let tail = dab_pixel_rect(&live[from..], self.width, self.height)
            .into_iter()
            .chain(dab_pixel_rect(&finals[from..], self.width, self.height))
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
        let Some(tail) = tail else {
            // The old and the new tail are both invisible: nothing on the layer changes.
            return Some(stroke_rect);
        };
        self.coverage.clear_rect(tail);
        let reaches = |d: &Dab| {
            dab_pixel_rect(std::slice::from_ref(d), self.width, self.height)
                .is_some_and(|r| r.0 <= tail.2 && r.2 >= tail.0 && r.1 <= tail.3 && r.3 >= tail.1)
        };
        for d in finals[..from].iter().filter(|d| reaches(d)) {
            self.coverage.splat_clipped(std::slice::from_ref(d), &self.raster, tail);
        }
        self.coverage.splat_clipped(&finals[from..], &self.raster, tail);
        let rect = PixelRect::from_inclusive(tail.0, tail.1, tail.2, tail.3, self.width, self.height)?;
        self.composite(layer, rect);
        Some(stroke_rect.union(rect))
    }

    /// Lay wet dabs, `(path, dabs)` pairs in order, into the coverage and colour buffers. Each
    /// dab picks up from the layer snapshot taken at pen-down. Returns the rectangle changed.
    fn lay_wet_dabs(&mut self, paths: &[(usize, Vec<Dab>)]) -> Option<PixelRect> {
        let wet = self.wet.as_mut()?;
        let mut dirty: Option<wet::Rect> = None;
        for (path, dabs) in paths {
            for d in dabs {
                if let Some(r) = wet.apply_dab(*path, d, self.colour, &self.raster, &mut self.coverage, &self.before) {
                    dirty = Some(wet::union(dirty, r));
                }
            }
        }
        let (x0, y0, x1, y1) = dirty?;
        Some(PixelRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// Write `rect` of the layer from `before` plus the current coverage. The pixel loop runs
    /// one tile at a time (see `Layer::update_rect`), reading the snapshot and writing the layer.
    fn composite(&self, layer: &mut Layer, rect: PixelRect) {
        let brush = self.strokes[0].brush();
        if let Some(wet) = &self.wet {
            let clip = self.clip.as_ref().map(|c| {
                let c = Arc::clone(c);
                move |x: usize, y: usize, cov: f64| f64::from(c.clip_coverage(x, y, cov as f32))
            });
            let ctx = ComposeCtx {
                base: self.colour,
                brush,
                raster: &self.raster,
                alpha_lock: self.alpha_lock,
                clip: clip.as_ref().map(|f| f as &dyn Fn(usize, usize, f64) -> f64),
            };
            wet.composite(layer, &self.before, &self.coverage, (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h), &ctx);
            return;
        }
        // The maths runs in f64 on 0..1 values and is rounded once, to the layer's depth. The
        // 8-bit loop keeps its own closure, so it is as tight as before 16-bit layers existed.
        let grain_on = self.raster.has_texturized_grain();
        let f = |x: usize, y: usize, old: [f64; 4]| -> [f64; 4] {
            let (xi, yi) = (x as i64, y as i64);
            let mut coverage = self.coverage.get(xi, yi);
            if let Some(clip) = &self.clip {
                coverage = f64::from(clip.clip_coverage(x, y, coverage as f32));
            }
            if coverage <= 0.0 {
                // No coverage: every mode (paint, erase, lock transparency) leaves the pixel as it was.
                return old;
            }
            let o = PixelOpts {
                layer: old,
                coverage,
                colour: self.colour,
                wet_edges: brush.wet_edges,
                threshold: brush.alpha_threshold,
                burnt: brush.burnt_edges,
                grain: if grain_on { self.raster.texturized_tone(xi, yi) } else { None },
                erase: self.erase,
                alpha_lock: self.alpha_lock,
                ..PixelOpts::default()
            };
            compose::stroke_composite_pixel(&o)
        };
        let (x1, y1) = (rect.x + rect.w, rect.y + rect.h);
        if layer.depth() == eskiz_core::Depth::U16 {
            layer.update_rect_unit(Some(&self.before), rect.x, rect.y, x1, y1, f);
            return;
        }
        let px = |x: usize, y: usize, old: [u8; 4]| -> [u8; 4] {
            let out = f(x, y, compose::premul_from_u8(&old));
            [compose::to_u8(out[0]), compose::to_u8(out[1]), compose::to_u8(out[2]), compose::to_u8(out[3])]
        };
        let threads = eskiz_core::parallelism::cpus().min(8).min(rect.h / 16);
        if rect.w * rect.h < PAR_COMPOSITE_AREA || threads < 2 {
            layer.update_rect(Some(&self.before), rect.x, rect.y, x1, y1, px);
            return;
        }
        // A large rectangle: compute the new pixels in bands of rows on several threads (the
        // maths reads only the snapshot and the coverage), then write them tile by tile.
        let (rx, ry) = (rect.x, rect.y);
        let mut new = self.before.read_rect(rx, ry, rect.w, rect.h);
        let band_rows = rect.h.div_ceil(threads);
        // The common brush (no selection, grain, wet edges, threshold or burnt edges) gets a
        // row loop that reads the coverage as a slice.
        let plain = self.clip.is_none()
            && !grain_on
            && brush.wet_edges == 0.0
            && brush.alpha_threshold <= 0.0
            && brush.burnt_edges <= 0.0;
        std::thread::scope(|scope| {
            for (b, band) in new.chunks_mut(band_rows * rect.w * 4).enumerate() {
                let (px, w) = (&px, rect.w);
                scope.spawn(move || {
                    let mut cov = vec![0.0f32; if plain { w } else { 0 }];
                    for (i, row) in band.chunks_exact_mut(w * 4).enumerate() {
                        let y = ry + b * band_rows + i;
                        if plain {
                            self.coverage.read_row(y as i64, rx as i64, (rx + w) as i64, &mut cov);
                            self.plain_row(row, &cov);
                        } else {
                            for (k, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                                *p = px(rx + k, y, *p);
                            }
                        }
                    }
                });
            }
        });
        let w = rect.w;
        layer.update_rect(Some(&self.before), rx, ry, x1, y1, |x, y, _| {
            let i = ((y - ry) * w + (x - rx)) * 4;
            [new[i], new[i + 1], new[i + 2], new[i + 3]]
        });
    }
}

impl Active {
    /// [`Active::composite`]'s pixel maths for a row of 8-bit pixels (`row`, holding the layer
    /// before the stroke) when nothing but the stroke colour, the erase and the alpha-lock modes
    /// matters: the operations of `compose::stroke_composite_pixel` with the unused stages
    /// removed, in the same order, so the result is bit-for-bit the same.
    fn plain_row(&self, row: &mut [u8], coverage: &[f32]) {
        let col = self.colour;
        for (p, &c) in row.as_chunks_mut::<4>().0.iter_mut().zip(coverage) {
            let f = f64::from(c);
            if f <= 0.0 {
                continue;
            }
            let f = f.min(1.0);
            let l = compose::premul_from_u8(p);
            let out = if self.erase {
                [l[0] * (1.0 - f), l[1] * (1.0 - f), l[2] * (1.0 - f), l[3] * (1.0 - f)]
            } else if self.alpha_lock {
                [
                    l[0] + (col[0] * l[3] - l[0]) * f,
                    l[1] + (col[1] * l[3] - l[1]) * f,
                    l[2] + (col[2] * l[3] - l[2]) * f,
                    l[3],
                ]
            } else {
                [col[0] * f + l[0] * (1.0 - f), col[1] * f + l[1] * (1.0 - f), col[2] * f + l[2] * (1.0 - f), f + l[3] * (1.0 - f)]
            };
            p.copy_from_slice(&out.map(compose::to_u8));
        }
    }
}

/// Rectangles of at least this many pixels are composited on several threads.
const PAR_COMPOSITE_AREA: usize = 128 * 128;

/// The coverage a brush stroke would lay down, without painting it: the same dabs, tip,
/// grain, size and opacity as [`PaintEngine`], accumulated into a stroke-wide coverage buffer.
/// Adjustment Pencil mode uses it to paint its mask with the current brush.
pub struct CoverageStroke {
    stroke: Stroke,
    raster: Arc<DabRaster>,
    coverage: CoverageBuffer,
    width: usize,
    height: usize,
    /// Dabs have been committed (until then a lone tap is previewed from a copy of the stroke).
    committed_any: bool,
    rect: Option<PixelRect>,
}

impl CoverageStroke {
    pub fn new(settings: &StrokeSettings, width: usize, height: usize, seed: u32) -> Self {
        Self {
            stroke: Stroke::new(settings.brush.clone(), StrokeOptions { seed, ..StrokeOptions::default() }),
            raster: settings.raster.clone().unwrap_or_else(|| Arc::new(DabRaster::new(&settings.brush))),
            coverage: CoverageBuffer::new(0, 0, width, height),
            width,
            height,
            committed_any: false,
            rect: None,
        }
    }

    /// Feed one sample. Returns the rectangle whose coverage changed.
    pub fn push(&mut self, sample: Sample) -> Option<PixelRect> {
        let dabs = self.stroke.add_sample(sample).committed_dabs;
        let dabs = if dabs.is_empty() && !self.committed_any {
            // The stroke has no dabs until it has a length: show a tap from a copy of it that
            // gets a second sample on the same spot.
            let mut tap = self.stroke.clone();
            tap.add_sample(Sample { t: sample.t + 1.0 / 120.0, ..sample });
            tap.finish().dabs
        } else {
            self.committed_any = true;
            dabs
        };
        let rect = dab_rect(&dabs, self.width, self.height)?;
        self.coverage.splat(&dabs, &self.raster);
        self.rect = Some(self.rect.map_or(rect, |r| r.union(rect)));
        Some(rect)
    }

    /// Pen-up: apply the end taper. Returns the rectangle the stroke covers.
    pub fn finish(&mut self) -> Option<PixelRect> {
        let dabs = self.stroke.finish().dabs;
        self.coverage.clear_all();
        self.coverage.splat(&dabs, &self.raster);
        let rect = dab_rect(&dabs, self.width, self.height)?;
        self.rect = Some(self.rect.map_or(rect, |r| r.union(rect)));
        self.rect
    }

    /// Raise `mask` (`width * height` values in 0..1) to this stroke's coverage inside `rect`.
    pub fn merge_into(&self, mask: &mut [f32], rect: PixelRect) {
        for y in rect.y..rect.y + rect.h {
            for x in rect.x..rect.x + rect.w {
                let c = self.coverage.get(x as i64, y as i64) as f32;
                let m = &mut mask[y * self.width + x];
                if c > *m {
                    *m = c;
                }
            }
        }
    }

    /// The accumulated coverage.
    pub fn coverage(&self) -> &CoverageBuffer {
        &self.coverage
    }
}

/// Pixel rectangle covering `dabs`, clipped to the canvas.
fn dab_rect(dabs: &[Dab], width: usize, height: usize) -> Option<PixelRect> {
    let (x0, y0, x1, y1) = dab_pixel_rect(dabs, width, height)?;
    PixelRect::from_inclusive(x0, y0, x1, y1, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eskiz_core::brushes::Preset;
    use eskiz_core::geometry::{PointerType, Sample, Stroke, StrokeOptions};
    use eskiz_core::canvas::TileSlot;
    use eskiz_core::PaintOptions;

    /// The layer id, rect and tile snapshots of a stroke command.
    fn tiles_of(cmd: &Command) -> (u64, PixelRect, Vec<TileSlot>, Vec<TileSlot>) {
        match &cmd.changes[0] {
            Change::Tiles { layer, rect, before, after } => (*layer, *rect, before.clone(), after.clone()),
            _ => panic!("not a tile change"),
        }
    }

    // Wider and taller than one 256-pixel tile, and not a multiple of it, so strokes cross
    // tile edges in both directions.
    const W: usize = 600;
    const H: usize = 300;

    fn settings(erase: bool) -> StrokeSettings {
        let mut brush = Brush::preset(Preset::InkWithTaper);
        brush.size = 26.0;
        StrokeSettings {
            brush,
            colour: [0.1, 0.4, 0.8],
            erase,
            raster: None,
        }
    }

    fn samples(n: usize) -> Vec<Sample> {
        (0..n)
            .map(|i| {
                let t = i as f64 / 120.0;
                let p = i as f64 / n as f64;
                Sample {
                    x: 20.0 + 560.0 * p,
                    y: 150.0 + 120.0 * (p * 6.0).sin(),
                    t,
                    pressure: Some(0.2 + 0.8 * (p * std::f64::consts::PI).sin()),
                    has_pressure: Some(true),
                    pointer: PointerType::Pen,
                    ..Sample::default()
                }
            })
            .collect()
    }

    fn canvas_with_base() -> Layer {
        let mut buf = vec![0u8; W * H * 4];
        // Non-blank base so compositing has something to mix with.
        for (i, px) in buf.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = ((i * 7) % 200) as u8;
            px.copy_from_slice(&[v, v / 2, 255 - v, 255]);
        }
        Layer::from_rgba(W, H, buf)
    }

    #[test]
    fn engine_matches_layer_paint_stroke_with() {
        let s = settings(false);
        let pts = samples(90);

        // Reference: the core stroke, finished, painted in one go.
        let mut reference = canvas_with_base();
        let mut core_stroke = Stroke::new(
            s.brush.clone(),
            StrokeOptions {
                seed: 7,
                ..StrokeOptions::default()
            },
        );
        for p in &pts {
            core_stroke.add_sample(*p);
        }
        core_stroke.finish();
        let mut opts = PaintOptions::new(s.colour);
        opts.erase = false;
        reference.paint_stroke_with(&core_stroke, opts);

        // Engine: incremental pushes, then finish.
        let mut canvas = canvas_with_base();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 42, &s, 7, false));
        for p in &pts {
            let before = canvas.to_rgba();
            let rect = engine.push(&mut canvas, *p);
            // Every changed pixel must lie inside the rectangle the push reports.
            if let Some(r) = rect {
                for (i, (a, b)) in before.as_chunks::<4>().0.iter().zip(canvas.to_rgba().as_chunks::<4>().0.iter()).enumerate() {
                    if a != b {
                        let (x, y) = (i % W, i / W);
                        assert!(x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h, "pixel {x},{y} outside {r:?}");
                    }
                }
            } else {
                assert_eq!(before, canvas.to_rgba());
            }
        }
        let (id, ..) = tiles_of(&engine.end(&mut canvas).expect("stroke painted something"));
        assert!(!engine.is_active());
        assert_eq!(id, 42);
        assert_eq!(canvas.to_rgba(), reference.to_rgba());
    }

    #[test]
    fn wet_engine_matches_layer_paint_stroke_with() {
        // The ink preset has an end taper, so pen-up replays the stroke.
        let mut s = settings(false);
        s.brush.wet = eskiz_core::Wet {
            enabled: true,
            dilution: 0.5,
            falloff_amount: 0.3,
            pull: 0.8,
            wetness: 0.3,
            bleed: 0.2,
            ..eskiz_core::Wet::default()
        };
        let pts = samples(90);

        let mut reference = canvas_with_base();
        let mut core_stroke = Stroke::new(s.brush.clone(), StrokeOptions { seed: 7, ..StrokeOptions::default() });
        for p in &pts {
            core_stroke.add_sample(*p);
        }
        core_stroke.finish();
        reference.paint_stroke_with(&core_stroke, PaintOptions::new(s.colour));

        let mut canvas = canvas_with_base();
        let original = canvas.clone();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 42, &s, 7, false));
        for p in &pts {
            let before = canvas.to_rgba();
            let rect = engine.push(&mut canvas, *p);
            // Every changed pixel lies inside the reported rectangle, softening included.
            let now = canvas.to_rgba();
            for (i, (a, b)) in before.as_chunks::<4>().0.iter().zip(now.as_chunks::<4>().0.iter()).enumerate() {
                if a != b {
                    let r = rect.expect("pixels changed without a rectangle");
                    let (x, y) = (i % W, i / W);
                    assert!(x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h, "pixel {x},{y} outside {r:?}");
                }
            }
        }
        engine.end(&mut canvas).expect("stroke painted something");
        assert_ne!(canvas, original);
        assert_eq!(canvas.to_rgba(), reference.to_rgba());

        // A wet stroke can be cancelled too.
        let mut canvas = canvas_with_base();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 1, &s, 7, false));
        for p in pts.iter().take(30) {
            engine.push(&mut canvas, *p);
        }
        assert!(engine.cancel(&mut canvas).is_some());
        assert_eq!(canvas, original);
    }

    #[test]
    fn a_wet_stroke_across_a_selection_edge_leaves_the_outside_unchanged() {
        use eskiz_select::{Mask, Selection};
        let mut s = settings(false);
        s.brush.wet = eskiz_core::Wet {
            enabled: true,
            dilution: 0.4,
            pull: 1.0,
            wetness: 0.5,
            bleed: 0.6,
            ..eskiz_core::Wet::default()
        };
        // Select the left half (x < 300); the stroke runs the whole width.
        let mut data = vec![0f32; W * H];
        for row in data.chunks_mut(W) {
            row[..300].fill(1.0);
        }
        let selection = Arc::new(Selection::active(Mask { width: W, height: H, data }));
        let original = canvas_with_base();
        let mut canvas = original.clone();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 5, &s, 7, false));
        engine.set_clip(selection);
        for p in samples(90) {
            engine.push(&mut canvas, p);
        }
        engine.end(&mut canvas).expect("the inside was painted");
        let (before, after) = (original.to_rgba(), canvas.to_rgba());
        let (mut inside_changed, mut outside_changed) = (0, 0);
        for (i, (a, b)) in before.as_chunks::<4>().0.iter().zip(after.as_chunks::<4>().0.iter()).enumerate() {
            if a != b {
                if i % W < 300 { inside_changed += 1 } else { outside_changed += 1 }
            }
        }
        assert!(inside_changed > 500, "inside {inside_changed}");
        assert_eq!(outside_changed, 0, "pixels outside the selection changed");
    }

    #[test]
    fn undo_entry_stores_only_the_touched_tiles_and_restores_them() {
        let s = settings(false);
        let mut canvas = canvas_with_base();
        let original = canvas.clone();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 1, &s, 3, false));
        for p in samples(60) {
            engine.push(&mut canvas, p);
        }
        let cmd = engine.end(&mut canvas).unwrap();
        let (_, _, before, after) = tiles_of(&cmd);
        let painted = canvas.clone();
        assert_ne!(painted, original);

        // The stroke crosses tile columns 0..=2 and rows 0..=1: only tiles that changed are kept.
        assert!(!before.is_empty() && before.len() <= 6, "{} tiles", before.len());
        assert_eq!(before.len(), after.len());
        for slot in &before {
            let (x, y, _, _) = canvas.tile_rect(slot.tx, slot.ty);
            assert!(!painted.tile_eq(&original, slot.tx, slot.ty), "tile {x},{y} did not change");
        }
        // Tiles the stroke did not change are still the very same allocation.
        let stored: Vec<_> = before.iter().map(|s| (s.tx, s.ty)).collect();
        for ty in 0..canvas.tiles_y() {
            for tx in 0..canvas.tiles_x() {
                if !stored.contains(&(tx, ty)) {
                    assert!(canvas.tile_shared(&original, tx, ty), "tile {tx},{ty} was copied needlessly");
                }
            }
        }
        // Applying the change through a document undoes and redoes it.
        let mut doc = eskiz_core::document::Document::empty(W, H);
        let id = doc.push_raster(None, eskiz_core::document::Props::new("l"), None, painted.clone());
        let mut cmd = cmd;
        for c in &mut cmd.changes {
            if let Change::Tiles { layer, .. } = c {
                *layer = id;
            }
        }
        cmd.undo(&mut doc);
        assert_eq!(*doc.layer(id).unwrap(), original);
        cmd.redo(&mut doc);
        assert_eq!(*doc.layer(id).unwrap(), painted);
    }

    #[test]
    fn cancel_restores_the_layer() {
        let s = settings(false);
        let mut canvas = canvas_with_base();
        let original = canvas.clone();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 1, &s, 3, false));
        for p in samples(40) {
            engine.push(&mut canvas, p);
        }
        assert_ne!(canvas, original);
        assert!(engine.cancel(&mut canvas).is_some());
        assert!(!engine.is_active());
        assert_eq!(canvas, original);
        assert!(canvas.tile_shared(&original, 0, 0), "cancel puts the original tiles back");
    }

    #[test]
    fn erase_reduces_alpha_and_frees_empty_tiles() {
        let mut canvas = Layer::from_rgba(W, H, vec![255; W * H * 4]);
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 1, &settings(true), 1, false));
        for p in samples(50) {
            engine.push(&mut canvas, p);
        }
        engine.end(&mut canvas).unwrap();
        let min_alpha = canvas.to_rgba().as_chunks::<4>().0.iter().map(|p| p[3]).min().unwrap();
        assert!(min_alpha < 255, "erasing should make some pixels transparent");

        // Erasing everything on a small layer leaves no tile behind.
        let mut small = Layer::new(100, 100);
        small.set_pixel(50, 50, [255, 255, 255, 255]);
        let mut brush = Brush::preset(Preset::InkWithTaper);
        brush.size = 400.0;
        let st = StrokeSettings { brush, colour: [0.0; 3], erase: true, raster: None };
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&small, 1, &st, 1, false));
        for i in 0..30 {
            engine.push(&mut small, Sample { x: 20.0 + i as f64 * 2.0, y: 50.0, t: i as f64 / 60.0, pressure: Some(1.0), has_pressure: Some(true), pointer: PointerType::Pen, ..Sample::default() });
        }
        engine.end(&mut small).unwrap();
        if small.is_blank() {
            assert_eq!(small.tile_count(), 0, "a blank layer holds no tiles");
        }
    }

    /// Tiny deterministic generator for the randomised comparisons (fixed seeds).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as f64 / (1u64 << 31) as f64
        }
    }

    fn random_path(rng: &mut Lcg, n: usize) -> Vec<Sample> {
        let (mut x, mut y) = (40.0 + 500.0 * rng.next(), 40.0 + 220.0 * rng.next());
        let mut ang = rng.next() * std::f64::consts::TAU;
        (0..n)
            .map(|i| {
                ang += (rng.next() - 0.5) * 0.8;
                x = (x + 7.0 * ang.cos()).clamp(2.0, W as f64 - 2.0);
                y = (y + 7.0 * ang.sin()).clamp(2.0, H as f64 - 2.0);
                Sample {
                    x,
                    y,
                    t: i as f64 / 120.0,
                    pressure: Some(0.1 + 0.9 * rng.next()),
                    has_pressure: Some(true),
                    pointer: PointerType::Pen,
                    ..Sample::default()
                }
            })
            .collect()
    }

    /// Run a whole stroke, pen-up either by the tail replay or by the full replay.
    fn run_stroke(base: &Layer, s: &StrokeSettings, pts: &[Sample], seed: u32, lock: bool, clip: Option<&Arc<Selection>>, tail: bool) -> (Layer, PixelRect) {
        let mut canvas = base.clone();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 1, s, seed, lock));
        if let Some(c) = clip {
            engine.set_clip(Arc::clone(c));
        }
        for p in pts {
            engine.push(&mut canvas, *p);
        }
        let rect = match engine.end_with(&mut canvas, tail) {
            Some(cmd) => tiles_of(&cmd).1,
            None => PixelRect { x: 0, y: 0, w: 0, h: 0 },
        };
        (canvas, rect)
    }

    #[test]
    fn tail_replay_is_pixel_identical_to_the_full_replay() {
        use eskiz_select::{Mask, Selection};
        let mut rng = Lcg(12345);
        let half = {
            let mut data = vec![0f32; W * H];
            for (i, v) in data.iter_mut().enumerate() {
                *v = ((i % W) as f32 / W as f32).clamp(0.0, 1.0);
            }
            Arc::new(Selection::active(Mask { width: W, height: H, data }))
        };
        let mut tapered = 0;
        for case in 0..36u32 {
            let preset = Preset::ALL[case as usize % Preset::ALL.len()];
            let mut brush = Brush::preset(preset);
            brush.wet.enabled = false;
            brush.size = 8.0 + 60.0 * rng.next();
            if case % 5 == 0 {
                brush.wet_edges = 0.7;
            }
            if case % 7 == 3 {
                brush.alpha_threshold = 0.3;
            }
            let erase = case % 6 == 1;
            let lock = case % 6 == 2;
            let s = StrokeSettings { brush, colour: [rng.next(), rng.next(), rng.next()], erase, raster: None };
            let pts = random_path(&mut rng, 10 + (case as usize * 7) % 70);
            let mut base = canvas_with_base();
            if case % 4 == 3 {
                base = base.converted(eskiz_core::Depth::U16, false);
            }
            let clip = (case % 3 == 1).then_some(&half);
            let fast = run_stroke(&base, &s, &pts, case, lock, clip, true);
            let full = run_stroke(&base, &s, &pts, case, lock, clip, false);
            assert_eq!(fast.1, full.1, "case {case} ({preset:?}): undo rectangle");
            assert!(fast.0 == full.0, "case {case} ({preset:?}): pixels differ");
            tapered += usize::from(fast.0 != base);
        }
        assert!(tapered > 20, "most cases must actually paint ({tapered})");
    }

    #[test]
    fn pen_up_without_a_changed_dab_leaves_the_layer_alone() {
        // A hard round brush with no taper: the finish changes no dab, so nothing is redrawn.
        let mut brush = Brush::preset(Preset::TechnicalPen);
        brush.size = 20.0;
        let s = StrokeSettings { brush, colour: [0.2, 0.3, 0.9], erase: false, raster: None };
        let pts = samples(40);
        let base = canvas_with_base();
        let fast = run_stroke(&base, &s, &pts, 3, false, None, true);
        let full = run_stroke(&base, &s, &pts, 3, false, None, false);
        assert!(fast.0 == full.0 && fast.1 == full.1);
        assert_ne!(fast.0, base);
    }

    #[test]
    fn big_dabs_match_the_core_reference() {
        // Dabs over 128 x 128 px take the threaded composite and the row fast path.
        let mut rng = Lcg(777);
        let mut checked = 0;
        for case in 0..14u32 {
            let mut brush = Brush::preset(Preset::ALL[case as usize % Preset::ALL.len()]);
            brush.wet.enabled = false;
            brush.size = 140.0 + 120.0 * rng.next();
            if case % 4 == 2 {
                brush.wet_edges = 0.6;
            }
            let erase = case % 5 == 1;
            let lock = case % 5 == 2;
            let s = StrokeSettings { brush, colour: [rng.next(), rng.next(), rng.next()], erase, raster: None };
            let pts = random_path(&mut rng, 24);
            let base = canvas_with_base();

            let mut reference = base.clone();
            let mut core_stroke = Stroke::new(s.brush.clone(), StrokeOptions { seed: case, ..StrokeOptions::default() });
            for p in &pts {
                core_stroke.add_sample(*p);
            }
            core_stroke.finish();
            let mut opts = PaintOptions::new(s.colour);
            opts.erase = erase;
            opts.alpha_lock = lock;
            reference.paint_stroke_with(&core_stroke, opts);

            let (canvas, _) = run_stroke(&base, &s, &pts, case, lock, None, true);
            assert!(canvas == reference, "case {case}: engine and reference differ");
            checked += usize::from(canvas != base);
        }
        assert!(checked >= 10);
    }

    #[test]
    fn growing_wet_buffers_match_the_fixed_reference() {
        // Random walks head left, right, up and down, so the stroke buffers grow in every
        // direction; `Layer::paint_stroke_with` uses fixed buffers.
        let mut rng = Lcg(4242);
        for case in 0..10u32 {
            let mut brush = Brush::preset(Preset::ALL[case as usize % Preset::ALL.len()]);
            brush.size = 12.0 + 50.0 * rng.next();
            brush.wet = eskiz_core::Wet {
                enabled: true,
                dilution: rng.next(),
                pull: rng.next(),
                wetness: if case % 3 == 0 { 0.0 } else { rng.next() * 0.6 },
                bleed: if case % 3 == 1 { 0.0 } else { rng.next() * 0.5 },
                ..eskiz_core::Wet::default()
            };
            let erase = false;
            let s = StrokeSettings { brush, colour: [rng.next(), rng.next(), rng.next()], erase, raster: None };
            let pts = random_path(&mut rng, 60);
            let base = if case % 4 == 3 { canvas_with_base().converted(eskiz_core::Depth::U16, false) } else { canvas_with_base() };
            let mut reference = base.clone();
            let mut core_stroke = Stroke::new(s.brush.clone(), StrokeOptions { seed: case, ..StrokeOptions::default() });
            for p in &pts {
                core_stroke.add_sample(*p);
            }
            core_stroke.finish();
            reference.paint_stroke_with(&core_stroke, PaintOptions::new(s.colour));
            let (canvas, _) = run_stroke(&base, &s, &pts, case, false, None, true);
            assert!(canvas == reference, "case {case}: engine and reference differ");
            assert_ne!(canvas, base, "case {case} painted nothing");
        }
    }

    #[test]
    fn every_byte_survives_the_unit_round_trip() {
        for v in 0..=255u8 {
            assert_eq!(compose::to_u8(compose::unit_from_u8(v)), v);
        }
    }

    #[test]
    fn only_one_stroke_at_a_time() {
        let s = settings(false);
        let canvas = canvas_with_base();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 1, &s, 1, false));
        assert!(!engine.begin(&canvas, 1, &s, 2, false));
    }

    #[test]
    fn empty_stroke_produces_no_undo_entry() {
        let mut canvas = canvas_with_base();
        let mut engine = PaintEngine::default();
        assert!(engine.begin(&canvas, 1, &settings(false), 1, false));
        assert!(engine.end(&mut canvas).is_none());
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use eskiz_core::geometry::PointerType;

    fn sample(x: f64, t: f64) -> Sample {
        Sample { x, y: 50.0, t, pressure: Some(1.0), has_pressure: Some(true), pointer: PointerType::Pen, ..Sample::default() }
    }

    #[test]
    fn a_lone_tap_still_leaves_coverage() {
        let s = StrokeSettings { brush: Brush { size: 20.0, ..Brush::default() }, colour: [0.0; 3], erase: false, raster: None };
        let mut c = CoverageStroke::new(&s, 100, 100, 1);
        assert!(c.push(sample(50.0, 0.01)).is_some());
        assert!(c.coverage().get(50, 50) > 0.0);
        assert_eq!(c.coverage().get(5, 5), 0.0);
    }
}
