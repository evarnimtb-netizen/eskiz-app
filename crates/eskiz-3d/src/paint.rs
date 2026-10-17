//! Painting on the model: screen-space dabs are mapped through the lookup into
//! texture space, stamped with eskiz-core's raster, then dilated into the gutter.

use std::collections::BTreeMap;

use eskiz_core::compose::{self, PixelOpts};
use eskiz_core::raster::{dab_pixel_rect, CoverageBuffer, DabRaster};
use eskiz_core::{Brush, Dab, Layer, PaintOptions};

use crate::lookup::{Hit, LookupBuffer, Side};
use crate::model::{MapKind, Model, UvMask};

/// Acceptance default: dilate painted texels by 3 texels.
pub const DEFAULT_DILATION: usize = 3;

/// Per-document painting settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaintSettings {
    /// Also paint the farthest surface under each dab (needs a lookup rendered
    /// with [`crate::RenderOptions::back_layer`]).
    pub paint_through: bool,
    /// Gutter dilation radius in texels; [`PaintSession::finish`] clamps it to 2..=4.
    pub dilation: usize,
    /// The map the stroke paints into.
    pub map: MapKind,
}

impl Default for PaintSettings {
    fn default() -> Self {
        Self {
            paint_through: false,
            dilation: DEFAULT_DILATION,
            map: MapKind::Colour,
        }
    }
}

/// A screen dab converted to a texture-space dab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MappedDab {
    /// Texture set (material) index.
    pub set: usize,
    /// UV island label at the dab centre ([`UvMask::SHARED`] when ambiguous).
    pub island: u32,
    /// Which lookup surface produced it.
    pub side: Side,
    /// Dab in texel coordinates of the texture set.
    pub dab: Dab,
}

/// Island a dab centred on texel `(x, y)` paints: the centre texel's island, or the
/// nearest single island within `reach` texels when the centre is a gutter or shared
/// texel. Shared texels never become the dab's island, so a dab cannot spill onto
/// unrelated islands. Returns `None` when no island is that close.
fn nearest_island(mask: &UvMask, x: usize, y: usize, reach: usize) -> Option<u32> {
    let (w, h) = (mask.width as i64, mask.height as i64);
    let (cx, cy) = (x as i64, y as i64);
    for r in 0..=reach as i64 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let (nx, ny) = (cx + dx, cy + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let l = mask.label[(ny * w + nx) as usize];
                if l != 0 && l != UvMask::SHARED {
                    return Some(l);
                }
            }
        }
    }
    None
}

/// Convert one screen-space dab using a lookup hit. The dab's size, rotation and
/// centre are carried into texel space through the hit's UV derivatives, so the
/// brush keeps its on-screen size at the stroke point.
fn map_one(model: &Model, hit: &Hit, side: Side, d: &Dab) -> Option<MappedDab> {
    let set = model.material_of(hit.tri)?;
    let ts = model.texture_sets.get(set)?;
    let (tw, th) = (ts.width as f32, ts.height as f32);
    // Texels moved per screen pixel along +x and +y.
    let ex = glam::Vec2::new(hit.duv_dx.x * tw, hit.duv_dx.y * th);
    let ey = glam::Vec2::new(hit.duv_dy.x * tw, hit.duv_dy.y * th);
    let (s, c) = (d.angle.sin() as f32, d.angle.cos() as f32);
    // Dab axes in screen pixels (matches eskiz-core's local frame).
    let a = glam::Vec2::new(c, s) * d.sx as f32;
    let b = glam::Vec2::new(-s, c) * d.sy as f32;
    let ta = ex * a.x + ey * a.y;
    let tb = ex * b.x + ey * b.y;
    let (sx, sy) = (ta.length(), tb.length());
    let cap = 2.0 * tw.max(th);
    if !(sx.is_finite() && sy.is_finite()) || sx < 1e-3 || sy < 1e-3 {
        return None;
    }
    let mask = model.masks.get(set)?;
    let x = ((hit.u * tw) as usize).min(ts.width - 1);
    let y = ((hit.v * th) as usize).min(ts.height - 1);
    // Reach covers the dab's radius so a dab centred on a gutter or shared texel still finds its island.
    let reach = (sx.max(sy) * 0.5)
        .ceil()
        .clamp(DEFAULT_DILATION as f32, 64.0) as usize;
    let island = nearest_island(mask, x, y, reach)?;
    let mut dab = *d;
    dab.x = f64::from(hit.u * tw);
    dab.y = f64::from(hit.v * th);
    dab.sx = f64::from(sx.min(cap));
    dab.sy = f64::from(sy.min(cap));
    dab.angle = f64::from(ta.y.atan2(ta.x));
    Some(MappedDab {
        set,
        island,
        side,
        dab,
    })
}

/// Map screen-space dabs through `lookup`.
///
/// Dabs whose centre misses the model are dropped. With `paint_through` the
/// back surface is stamped too when it is a different triangle.
pub fn map_dabs(
    model: &Model,
    lookup: &LookupBuffer,
    dabs: &[Dab],
    paint_through: bool,
) -> Vec<MappedDab> {
    let mut out = Vec::new();
    for d in dabs {
        let (x, y) = (d.x as f32, d.y as f32);
        let front = lookup.hit(x, y, Side::Front);
        if let Some(h) = &front {
            out.extend(map_one(model, h, Side::Front, d));
        }
        if paint_through {
            if let Some(b) = lookup.hit(x, y, Side::Back) {
                if front.as_ref().map(|f| f.tri) != Some(b.tri) {
                    out.extend(map_one(model, &b, Side::Back, d));
                }
            }
        }
    }
    out
}

type Rect = (i64, i64, i64, i64);

/// One 3D stroke: accumulates mapped dabs and composites them into texture layers.
///
/// Typical use per input batch: [`PaintSession::add_dabs`] then [`PaintSession::apply`]
/// (idempotent, so it can run every frame), and [`PaintSession::finish`] on pen-up.
pub struct PaintSession {
    brush: Brush,
    raster: DabRaster,
    opts: PaintOptions,
    settings: PaintSettings,
    groups: BTreeMap<(usize, u32), Vec<Dab>>,
    base: BTreeMap<usize, Layer>,
    dirty: BTreeMap<usize, Rect>,
}

impl PaintSession {
    /// Start a stroke with `brush` and colour/erase options.
    pub fn new(brush: Brush, opts: PaintOptions, settings: PaintSettings) -> Self {
        Self {
            raster: DabRaster::new(&brush),
            brush,
            opts,
            settings,
            groups: BTreeMap::new(),
            base: BTreeMap::new(),
            dirty: BTreeMap::new(),
        }
    }

    /// Settings in effect.
    pub fn settings(&self) -> PaintSettings {
        self.settings
    }

    /// Map new screen-space dabs and queue them. Returns how many texture-space dabs were queued.
    pub fn add_dabs(&mut self, model: &Model, lookup: &LookupBuffer, dabs: &[Dab]) -> usize {
        let mapped = map_dabs(model, lookup, dabs, self.settings.paint_through);
        let n = mapped.len();
        for m in mapped {
            self.groups
                .entry((m.set, m.island))
                .or_default()
                .push(m.dab);
        }
        n
    }

    /// Composite all queued dabs onto the model's texture layers.
    ///
    /// Always starts from the layer as it was before the stroke, so calling it
    /// repeatedly does not double-apply dabs. Locked texture sets are skipped.
    pub fn apply(&mut self, model: &mut Model) {
        let Model {
            texture_sets,
            masks,
            ..
        } = model;
        let kind = self.settings.map;
        // Restore touched layers to their pre-stroke state.
        let sets: Vec<usize> = self.groups.keys().map(|k| k.0).collect();
        for &s in &sets {
            let Some(ts) = texture_sets.get_mut(s) else {
                continue;
            };
            let layer = ts.map_mut(kind);
            match self.base.get(&s) {
                Some(b) => layer.copy_tiles_from(b),
                None => {
                    self.base.insert(s, layer.clone());
                }
            }
        }
        for (&(s, island), dabs) in &self.groups {
            let Some(ts) = texture_sets.get_mut(s) else {
                continue;
            };
            // Locked and hidden texture sets take no paint.
            if ts.locked || !ts.visible {
                continue;
            }
            let Some(mask) = masks.get(s) else { continue };
            let layer = ts.map_mut(kind);
            if let Some(r) = self.composite(layer, mask, island, dabs) {
                let e = self.dirty.entry(s).or_insert(r);
                *e = (e.0.min(r.0), e.1.min(r.1), e.2.max(r.2), e.3.max(r.3));
            }
        }
    }

    fn composite(
        &self,
        layer: &mut Layer,
        mask: &UvMask,
        island: u32,
        dabs: &[Dab],
    ) -> Option<Rect> {
        let (x0, y0, x1, y1) = dab_pixel_rect(dabs, layer.width, layer.height)?;
        let mut cov = CoverageBuffer::new(x0, y0, (x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        cov.splat(dabs, &self.raster);
        let brush = &self.brush;
        // Tile-aware pass over the dab rectangle: each pixel reads its old value and the result is
        // written back in place, copying only the tiles that are touched and shared.
        layer.update_rect(
            None,
            x0 as usize,
            y0 as usize,
            x1 as usize + 1,
            y1 as usize + 1,
            |x, y, old| {
                let (px, py) = (x as i64, y as i64);
                let label = mask.label[y * mask.width + x];
                // Gutter texels (label 0) only receive paint through dilation. A different
                // island never receives paint; texels where islands touch accept either.
                if label == 0 || (label != island && label != UvMask::SHARED) {
                    return old;
                }
                let coverage = cov.get(px, py);
                if coverage <= 0.0 {
                    return old;
                }
                let o = PixelOpts {
                    layer: compose::premul_from_u8(&old),
                    coverage,
                    colour: self.opts.colour,
                    wet_edges: brush.wet_edges,
                    threshold: brush.alpha_threshold,
                    burnt: brush.burnt_edges,
                    grain: self.raster.texturized_tone(px, py),
                    erase: self.opts.erase,
                    alpha_lock: self.opts.alpha_lock,
                    ..PixelOpts::default()
                };
                let out = compose::stroke_composite_pixel(&o);
                [
                    compose::to_u8(out[0]),
                    compose::to_u8(out[1]),
                    compose::to_u8(out[2]),
                    compose::to_u8(out[3]),
                ]
            },
        );
        Some((x0, y0, x1, y1))
    }

    /// End the stroke: apply the final state and dilate painted texels into the gutter.
    ///
    /// Returns what the stroke changed: each texture map it touched, before and after, for undo
    /// (see [`TexelChange`]). Maps the stroke left as they were are not listed.
    pub fn finish(mut self, model: &mut Model) -> Vec<TexelChange> {
        self.apply(model);
        let r = self.settings.dilation.clamp(2, 4);
        let kind = self.settings.map;
        let Model {
            texture_sets,
            masks,
            ..
        } = model;
        for (&s, &rect) in &self.dirty {
            if let (Some(ts), Some(mask)) = (texture_sets.get_mut(s), masks.get(s)) {
                dilate_gutter(ts.map_mut(kind), mask, r, Some(rect));
            }
        }
        let mut changes = Vec::new();
        for (s, before) in std::mem::take(&mut self.base) {
            if let Some(ts) = texture_sets.get(s) {
                let after = ts.map(kind).clone();
                if after != before {
                    changes.push(TexelChange { set: s, kind, before, after });
                }
            }
        }
        changes
    }
}

/// One texture map that a 3D stroke changed: its state before the stroke and after it. Layers
/// are copy-on-write, so a change costs only the tiles the stroke actually touched.
#[derive(Clone)]
pub struct TexelChange {
    /// Index of the texture set in `Model::texture_sets`.
    pub set: usize,
    pub kind: MapKind,
    pub before: Layer,
    pub after: Layer,
}

/// Put the maps of `changes` back to their state before the stroke (`forward` false), or to
/// their state after it (`forward` true). Sets that no longer exist are skipped.
pub fn restore_texels(model: &mut Model, changes: &[TexelChange], forward: bool) {
    for c in changes {
        if let Some(ts) = model.texture_sets.get_mut(c.set) {
            let src = if forward { &c.after } else { &c.before };
            ts.map_mut(c.kind).copy_tiles_from(src);
        }
    }
}

/// Fill empty-UV texels within `radius` texels of an island with the nearest
/// island colour, so mipmapping and filtering do not show seams.
///
/// `rect` limits the work to the given inclusive texel rectangle (expanded by
/// `radius`); `None` processes the whole layer. Gutter texels are overwritten
/// every time, so earlier dilation never goes stale.
pub fn dilate_gutter(
    layer: &mut Layer,
    mask: &UvMask,
    radius: usize,
    rect: Option<(i64, i64, i64, i64)>,
) {
    let (w, h) = (layer.width as i64, layer.height as i64);
    if radius == 0 || mask.width != layer.width || mask.height != layer.height {
        return;
    }
    let r = radius as i64;
    let (x0, y0, x1, y1) = match rect {
        Some((a, b, c, d)) => (
            (a - r).max(0),
            (b - r).max(0),
            (c + r).min(w - 1),
            (d + r).min(h - 1),
        ),
        None => (0, 0, w - 1, h - 1),
    };
    if x0 > x1 || y0 > y1 {
        return;
    }
    let (rw, rh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
    // valid[i]: texel is an island texel or already filled by an earlier pass.
    let mut valid = vec![false; rw * rh];
    for y in y0..=y1 {
        for x in x0..=x1 {
            valid[(y - y0) as usize * rw + (x - x0) as usize] =
                mask.label[(y * w + x) as usize] != 0;
        }
    }
    for _ in 0..radius {
        let mut fills: Vec<(usize, [u8; 4], i64, i64)> = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                let li = (y - y0) as usize * rw + (x - x0) as usize;
                if valid[li] {
                    continue;
                }
                let mut best: Option<[u8; 4]> = None;
                for dy in -1..=1i64 {
                    for dx in -1..=1i64 {
                        let (nx, ny) = (x + dx, y + dy);
                        if (dx == 0 && dy == 0) || nx < x0 || ny < y0 || nx > x1 || ny > y1 {
                            continue;
                        }
                        if !valid[(ny - y0) as usize * rw + (nx - x0) as usize] {
                            continue;
                        }
                        let px = layer.pixel(nx as usize, ny as usize);
                        // Prefer the most opaque neighbour; ties keep the first (stable).
                        if best.is_none_or(|b| px[3] > b[3]) {
                            best = Some(px);
                        }
                    }
                }
                if let Some(px) = best {
                    fills.push((li, px, x, y));
                }
            }
        }
        if fills.is_empty() {
            break;
        }
        for (li, px, x, y) in fills {
            layer.set_pixel(x as usize, y as usize, px);
            valid[li] = true;
        }
    }
}
