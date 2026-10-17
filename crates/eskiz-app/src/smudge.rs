//! Smudge tool: sample the colour under each dab and carry it along.
//!
//! This is a CPU raster pass on the layer's pixels, separate from the brush stroke engine, but
//! it shares the wet-mix pickup and carry model with wet brushes (`eskiz_core::wet`): each dab
//! samples the layer under it (weighted by the brush tip, strided so cost does not grow with
//! the radius), deposits the colour it is carrying into those pixels at strength = opacity,
//! then picks up part of what it read. A smudge is a wet brush with no paint of its own and a
//! carry that does not run out. Mixing is in premultiplied RGBA, as everywhere else. Undo is
//! the same as for strokes: the engine keeps a copy-on-write snapshot of the layer from before
//! the stroke and records the changed tiles, before and after, on commit.

use std::sync::Arc;

use eskiz_core::canvas::Layer;
use eskiz_core::Depth;
use eskiz_core::raster::ShapeTexture;
use eskiz_core::wet::{sample_under, Carry, SAMPLE_GRID};

use eskiz_select::Selection;

use crate::command::{Change, Command};
use crate::history::PixelRect;

/// Share of the sampled colour that the dab picks up each step (the wet model's `pull`).
const PICKUP: f64 = 0.35;

struct Active {
    layer_id: u64,
    width: usize,
    height: usize,
    /// The layer as it was at pen-down (shares its tiles until they are smudged).
    before: Layer,
    /// The paint the stroke is carrying (premultiplied, 0..1); `None` before the first dab.
    carry: Option<Carry>,
    /// Pickup rate per dab, 0..1.
    pull: f32,
    radius: f64,
    strength: f64,
    /// Keep each pixel's alpha: only colour moves.
    alpha_lock: bool,
    /// The brush tip, when a library brush is selected: its shape weights each pixel of the disc.
    tip: Option<Arc<ShapeTexture>>,
    /// The selection the smudge is clipped to. Pixels outside it are neither read nor changed.
    clip: Option<Arc<Selection>>,
    last: Option<(f64, f64)>,
    rect: Option<PixelRect>,
}

/// Runs at most one smudge stroke at a time.
#[derive(Default)]
pub struct SmudgeEngine {
    active: Option<Active>,
    clip: Option<Arc<Selection>>,
}

impl SmudgeEngine {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// The selection the next smudge is clipped to (no selection: no clipping).
    pub fn set_clip(&mut self, selection: Arc<Selection>) {
        self.clip = Some(selection);
    }

    /// Start a smudge on `layer` (id `layer_id`). `radius` is in canvas pixels and
    /// `strength` (0..1) is the tool's opacity. Returns false if a stroke is running.
    pub fn begin(&mut self, layer: &Layer, layer_id: u64, radius: f64, strength: f64, alpha_lock: bool) -> bool {
        self.begin_with_tip(layer, layer_id, radius, strength, alpha_lock, None)
    }

    /// [`SmudgeEngine::begin`] with a brush tip. The tip is sampled across the disc, so a
    /// textured brush smudges in its own shape.
    pub fn begin_with_tip(
        &mut self,
        layer: &Layer,
        layer_id: u64,
        radius: f64,
        strength: f64,
        alpha_lock: bool,
        tip: Option<Arc<ShapeTexture>>,
    ) -> bool {
        if self.active.is_some() {
            return false;
        }
        self.active = Some(Active {
            layer_id,
            width: layer.width,
            height: layer.height,
            before: layer.clone(),
            carry: None,
            pull: PICKUP as f32,
            radius: radius.clamp(1.0, 2000.0),
            strength: strength.clamp(0.0, 1.0),
            alpha_lock,
            tip,
            clip: self.clip.clone(),
            last: None,
            rect: None,
        });
        true
    }

    /// Feed one canvas-pixel position. Returns the rectangle that changed.
    pub fn push(&mut self, layer: &mut Layer, x: f64, y: f64) -> Option<PixelRect> {
        let a = self.active.as_mut()?;
        let dirty = match a.last {
            // The first dab has no path behind it: place it at the pen-down point only.
            None => dab(layer, a, x, y),
            Some(from) => {
                // Dabs are placed every fifth of the radius along the path, so the smudge is continuous.
                let spacing = (a.radius * 0.2).max(0.75);
                let len = (x - from.0).hypot(y - from.1);
                let steps = (len / spacing).ceil().max(1.0) as usize;
                let mut dirty: Option<PixelRect> = None;
                for k in 1..=steps {
                    let t = k as f64 / steps as f64;
                    let (dx, dy) = (from.0 + (x - from.0) * t, from.1 + (y - from.1) * t);
                    if let Some(r) = dab(layer, a, dx, dy) {
                        dirty = Some(dirty.map_or(r, |d| d.union(r)));
                    }
                }
                dirty
            }
        };
        a.last = Some((x, y));
        if let Some(r) = dirty {
            a.rect = Some(a.rect.map_or(r, |old| old.union(r)));
        }
        dirty
    }

    /// Pen-up: return the undo entry, or `None` when nothing changed.
    pub fn end(&mut self, layer: &mut Layer) -> Option<Command> {
        let a = self.active.take()?;
        let rect = a.rect?;
        layer.share_unchanged_in_rect(&a.before, rect.x, rect.y, rect.w, rect.h);
        Some(Command::single(
            "Smudge",
            Change::tiles_between(a.layer_id, &a.before, layer, rect),
        ))
    }

    /// Abandon the smudge and restore the pixels it changed.
    pub fn cancel(&mut self, layer: &mut Layer) -> Option<PixelRect> {
        let a = self.active.take()?;
        let rect = a.rect?;
        layer.copy_tiles_in_rect_from(&a.before, rect.x, rect.y, rect.w, rect.h);
        Some(rect)
    }
}

/// One smudge dab at `(cx, cy)`. Returns the rectangle it touched.
fn dab(layer: &mut Layer, a: &mut Active, cx: f64, cy: f64) -> Option<PixelRect> {
    let r = a.radius;
    let x0 = (cx - r).floor().max(0.0) as i64;
    let y0 = (cy - r).floor().max(0.0) as i64;
    let x1 = (cx + r).ceil().min(a.width as f64 - 1.0) as i64;
    let y1 = (cy + r).ceil().min(a.height as f64 - 1.0) as i64;
    let rect = PixelRect::from_inclusive(x0, y0, x1, y1, a.width, a.height)?;

    // Weight of a pixel: a soft disc, shaped by the brush tip when there is one.
    // Outside the selection a pixel is neither read nor changed; at its soft edge it counts less.
    let tip = a.tip.clone();
    let clip = a.clip.clone();
    let weight = |x: usize, y: usize| -> f64 {
        let (dx, dy) = (((x as f64 + 0.5) - cx) / r, ((y as f64 + 0.5) - cy) / r);
        let d = dx.hypot(dy);
        if d >= 1.0 {
            return 0.0;
        }
        let clip_w = clip.as_ref().map_or(1.0, |c| f64::from(c.clip_coverage(x, y, 1.0)));
        if clip_w <= 0.0 {
            return 0.0;
        }
        let w = (1.0 - d * d).powi(2) * clip_w;
        match &tip {
            Some(t) => w * t.sample(dx * 0.5 + 0.5, dy * 0.5 + 0.5),
            None => w,
        }
    };

    // What the disc holds now (pickup).
    let under = sample_under(layer, rect.x, rect.y, rect.x + rect.w, rect.y + rect.h, SAMPLE_GRID, weight)?;
    let carry = *a.carry.get_or_insert(Carry::holding(under));

    // Deposit the carried paint.
    let (strength, alpha_lock) = (a.strength as f32, a.alpha_lock);
    // The mix runs in f32 on 0..1 values (`old` already divided by the depth's maximum) and is
    // rounded once, to the layer's depth: 255 for 8 bits, 65535 for 16.
    let mix = |w: f64, old: [f32; 4]| -> Option<[f32; 4]> {
        if w <= 0.0 {
            return None;
        }
        let old_alpha = old[3];
        if alpha_lock && old_alpha == 0.0 {
            return None; // nothing to move on a transparent pixel
        }
        let k = (strength * w as f32).clamp(0.0, 1.0);
        let mut mixed: [f32; 4] = std::array::from_fn(|c| old[c] * (1.0 - k) + carry.rgba[c] * k);
        if alpha_lock {
            // Keep the pixel's alpha and move only its colour: rescale the mixed colour
            // (premultiplied by the mixed alpha) to the old alpha.
            let scale = if mixed[3] > 0.0 { old_alpha / mixed[3] } else { 0.0 };
            for m in mixed.iter_mut().take(3) {
                *m = (*m * scale).min(old_alpha);
            }
            mixed[3] = old_alpha;
        }
        Some(mixed)
    };
    let (x1, y1) = (rect.x + rect.w, rect.y + rect.h);
    match layer.depth() {
        Depth::U8 => layer.update_rect(None, rect.x, rect.y, x1, y1, |x, y, old| {
            match mix(weight(x, y), old.map(|v| f32::from(v) / 255.0)) {
                Some(m) => std::array::from_fn(|c| (m[c] * 255.0).round().clamp(0.0, 255.0) as u8),
                None => old,
            }
        }),
        Depth::U16 => layer.update_rect16(None, rect.x, rect.y, x1, y1, |x, y, old| {
            match mix(weight(x, y), old.map(|v| f32::from(v) / 65535.0)) {
                Some(m) => std::array::from_fn(|c| (m[c] * 65535.0).round().clamp(0.0, 65535.0) as u16),
                None => old,
            }
        }),
    }

    // Pick up part of what the disc held, so the carried colour moves along the stroke.
    if let Some(c) = a.carry.as_mut() {
        c.pick_up(under, a.pull);
    }
    Some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 80;
    const H: usize = 40;

    /// Left half red, right half blue, fully opaque.
    fn two_tone() -> Layer {
        let mut c = Layer::new(W, H);
        for y in 0..H {
            for x in 0..W {
                let px = if x < W / 2 { [220, 30, 30, 255] } else { [30, 30, 220, 255] };
                c.set_pixel(x, y, px);
            }
        }
        c
    }

    fn pixel(c: &Layer, x: usize, y: usize) -> [u8; 4] {
        c.pixel(x, y)
    }

    #[test]
    fn smudging_across_an_edge_drags_colour_forward() {
        let mut canvas = two_tone();
        let mut engine = SmudgeEngine::default();
        assert!(engine.begin(&canvas, 9, 8.0, 0.9, false));
        // Start in the red, then drag right through the blue half.
        engine.push(&mut canvas, 30.0, 20.0);
        for step in 1..=30 {
            engine.push(&mut canvas, 30.0 + f64::from(step), 20.0);
        }
        engine.end(&mut canvas).unwrap();
        // A pixel the stroke crossed is now redder than the untouched blue.
        let untouched = pixel(&two_tone(), 50, 20);
        let smudged = pixel(&canvas, 50, 20);
        assert!(smudged[0] > untouched[0] + 20, "{smudged:?} vs {untouched:?}");
        assert!(smudged[2] < untouched[2], "{smudged:?}");
    }

    #[test]
    fn a_brush_tip_shapes_the_smudge() {
        use eskiz_core::raster::ShapeTexture;
        // An empty tip weights every pixel to zero: the smudge moves nothing.
        let empty = ShapeTexture::from_gray(4, &[0u8; 16]).unwrap();
        let mut canvas = two_tone();
        let before = canvas.to_rgba();
        let mut engine = SmudgeEngine::default();
        assert!(engine.begin_with_tip(&canvas, 9, 8.0, 0.9, false, Some(Arc::new(empty))));
        engine.push(&mut canvas, 30.0, 20.0);
        engine.push(&mut canvas, 50.0, 20.0);
        assert_eq!(canvas.to_rgba(), before, "a zero tip leaves the layer alone");

        // The same stroke with no tip does move the colour across the edge.
        let mut plain = two_tone();
        let mut engine = SmudgeEngine::default();
        assert!(engine.begin(&plain, 9, 8.0, 0.9, false));
        engine.push(&mut plain, 30.0, 20.0);
        engine.push(&mut plain, 50.0, 20.0);
        assert_ne!(plain.to_rgba(), before);
    }

    #[test]
    fn smudge_carries_colour_onto_bare_canvas_and_into_a_textured_tip() {
        use eskiz_core::brushes::Shape;
        // Red on the left, nothing on the right.
        let make = || {
            let mut c = Layer::new(W, H);
            for y in 0..H {
                for x in 0..W / 2 {
                    c.set_pixel(x, y, [220, 30, 30, 255]);
                }
            }
            c
        };
        let drag = |tip: Option<Arc<ShapeTexture>>| {
            let mut canvas = make();
            let mut engine = SmudgeEngine::default();
            assert!(engine.begin_with_tip(&canvas, 9, 8.0, 0.9, false, tip));
            for step in 0..=30 {
                engine.push(&mut canvas, 30.0 + f64::from(step), 20.0);
            }
            engine.end(&mut canvas).unwrap();
            canvas
        };
        for tip in [None, Some(Arc::new(ShapeTexture::new(Shape::SoftRound, 32)))] {
            let canvas = drag(tip.clone());
            // Red paint now sits on the formerly bare canvas to the right of the edge...
            let p = pixel(&canvas, 46, 20);
            assert!(p[3] > 40 && p[0] > p[2] + 20, "{p:?}");
            // ...and the canvas beyond the end of the stroke stays clear.
            assert_eq!(pixel(&canvas, 75, 20), [0; 4]);
        }
    }

    #[test]
    fn smudge_undo_restores_the_layer() {
        let original = two_tone();
        let mut canvas = two_tone();
        let mut engine = SmudgeEngine::default();
        engine.begin(&canvas, 1, 6.0, 1.0, false);
        for i in 0..20 {
            engine.push(&mut canvas, 20.0 + f64::from(i) * 2.0, 15.0);
        }
        let cmd = engine.end(&mut canvas).unwrap();
        assert_ne!(canvas, original);
        let mut doc = eskiz_core::document::Document::empty(W, H);
        let id = doc.push_raster(None, eskiz_core::document::Props::new("l"), None, canvas.clone());
        let mut cmd = cmd;
        for c in &mut cmd.changes {
            if let Change::Tiles { layer, .. } = c {
                *layer = id;
            }
        }
        cmd.undo(&mut doc);
        assert_eq!(*doc.layer(id).unwrap(), original);
        cmd.redo(&mut doc);
        assert_eq!(*doc.layer(id).unwrap(), canvas);
    }

    #[test]
    fn zero_strength_changes_nothing_and_cancel_restores() {
        let original = two_tone();
        let mut canvas = two_tone();
        let mut engine = SmudgeEngine::default();
        engine.begin(&canvas, 1, 6.0, 0.0, false);
        engine.push(&mut canvas, 30.0, 20.0);
        engine.push(&mut canvas, 40.0, 20.0);
        assert_eq!(canvas, original);
        engine.end(&mut canvas);

        let mut engine = SmudgeEngine::default();
        engine.begin(&canvas, 1, 6.0, 1.0, false);
        engine.push(&mut canvas, 30.0, 20.0);
        engine.push(&mut canvas, 45.0, 20.0);
        assert!(engine.cancel(&mut canvas).is_some());
        assert_eq!(canvas, original);
    }
}
