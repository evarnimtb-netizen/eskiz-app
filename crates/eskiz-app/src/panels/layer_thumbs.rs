//! Thumbnails for the layers panel: a small texture per raster layer (pixels over a
//! checkerboard) and one per mask, cached by item id.
//!
//! A thumbnail is only redrawn when [`Session::layer_epoch`] says the item's pixels or mask
//! changed since it was drawn, and at most a few per frame, so a stroke on a 2048x1536
//! canvas costs one thumbnail and nothing else. Drawing one samples a 4x4 grid of source
//! pixels per thumbnail pixel (about 160 000 reads), so it stays well under a millisecond
//! however large the layer is.

use std::collections::HashMap;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions, Vec2};

use crate::session::Session;
use eskiz_core::canvas::Layer;
use eskiz_core::document::Mask;

/// Box a layer thumbnail is fitted into, points.
pub const THUMB_BOX: Vec2 = Vec2::new(58.0, 44.0);
/// Box a mask thumbnail is fitted into, points.
pub const MASK_BOX: Vec2 = Vec2::new(38.0, 30.0);
/// Texels per point: thumbnails are drawn at 2x so they stay sharp on high-density screens.
const SCALE: f32 = 2.0;
/// Source samples per thumbnail pixel, per axis.
const GRID: usize = 4;
/// Checkerboard cell, texels, and its two greys.
const CELL: usize = 8;
const CHECK_LIGHT: u8 = 0xF4;
const CHECK_DARK: u8 = 0xD2;
/// Thumbnails redrawn per frame at most.
pub const BUDGET: usize = 4;

/// The size (points) of a `w x h` canvas fitted inside `bx`, keeping its aspect ratio.
pub fn fit(w: usize, h: usize, bx: Vec2) -> Vec2 {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let k = (bx.x / w).min(bx.y / h);
    Vec2::new((w * k).round().max(1.0), (h * k).round().max(1.0))
}

fn texels(size: Vec2) -> (usize, usize) {
    ((size.x * SCALE).round().max(1.0) as usize, (size.y * SCALE).round().max(1.0) as usize)
}

/// Source coordinate of sample `i` of `GRID` inside the span `[a, b)`.
fn sample_at(a: usize, b: usize, i: usize) -> usize {
    let span = b.saturating_sub(a).max(1);
    (a + (2 * i + 1) * span / (2 * GRID)).min(b.max(a + 1) - 1)
}

/// Draw layer pixels (premultiplied RGBA8, `w x h`) over a checkerboard into `tw x th` RGB texels.
pub fn render_layer(pixels: &[u8], w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
    render_with(|x, y| {
        let o = (y * w + x) * 4;
        [pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]]
    }, w, h, tw, th)
}

/// [`render_layer`] reading a tiled layer directly, so only the sampled pixels are touched.
pub fn render_tiled(layer: &Layer, w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
    render_with(|x, y| layer.pixel(x, y), w, h, tw, th)
}

fn render_with(get: impl Fn(usize, usize) -> [u8; 4], w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(tw * th * 3);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut acc = [0u32; 4];
            for j in 0..GRID {
                let y = sample_at(y0, y1, j).min(h - 1);
                for i in 0..GRID {
                    let p = get(sample_at(x0, x1, i).min(w - 1), y);
                    for (a, p) in acc.iter_mut().zip(p) {
                        *a += u32::from(p);
                    }
                }
            }
            let n = (GRID * GRID) as u32;
            let [r, g, b, a] = acc.map(|v| v / n);
            let check = if (tx / CELL + ty / CELL).is_multiple_of(2) { CHECK_LIGHT } else { CHECK_DARK };
            let under = u32::from(check) * (255 - a.min(255)) / 255;
            for c in [r, g, b] {
                out.push((c + under).min(255) as u8);
            }
        }
    }
    out
}

/// Draw a mask (255 shows) as greys into `tw x th` texels.
pub fn render_mask(mask: &Mask, w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(tw * th);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut acc = 0u32;
            for j in 0..GRID {
                let row = sample_at(y0, y1, j).min(h - 1) * w;
                for i in 0..GRID {
                    acc += u32::from(mask.data.get(row + sample_at(x0, x1, i).min(w - 1)).copied().unwrap_or(0));
                }
            }
            out.push((acc / (GRID * GRID) as u32) as u8);
        }
    }
    out
}

/// One cached thumbnail.
pub struct Thumb {
    pub tex: TextureHandle,
    /// Size on screen, points.
    pub size: Vec2,
    epoch: u64,
}

/// What the cache must hold for one visible item.
#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub id: u64,
    /// A raster layer (groups have no pixel thumbnail).
    pub layer: bool,
    pub mask: bool,
}

/// All thumbnails of the layers panel.
#[derive(Default)]
pub struct ThumbCache {
    layers: HashMap<u64, Thumb>,
    masks: HashMap<u64, Thumb>,
    /// How many thumbnails have been drawn so far (tests check that idle frames draw none).
    pub renders: usize,
}

impl ThumbCache {
    pub fn layer(&self, id: u64) -> Option<&Thumb> {
        self.layers.get(&id)
    }

    pub fn mask(&self, id: u64) -> Option<&Thumb> {
        self.masks.get(&id)
    }

    /// Bring the thumbnails of `wanted` up to date, drawing at most `budget` of them, and drop
    /// those of items that no longer exist. Returns true while some are still stale.
    pub fn refresh(&mut self, ctx: &egui::Context, session: &Session, wanted: &[Want], budget: usize) -> bool {
        let doc = &session.doc;
        self.layers.retain(|id, _| doc.layer(*id).is_some());
        self.masks.retain(|id, _| doc.node(*id).is_some_and(|n| n.mask.is_some()));
        let mut left = budget;
        let mut pending = false;
        let opts = TextureOptions::LINEAR;
        for w in wanted {
            let epoch = session.layer_epoch(w.id);
            if w.layer {
                let size = fit(doc.width, doc.height, THUMB_BOX);
                let stale = self.layers.get(&w.id).is_none_or(|t| t.epoch != epoch || t.size != size);
                if stale {
                    if left == 0 {
                        pending = true;
                    } else if let Some(layer) = doc.layer(w.id) {
                        left -= 1;
                        self.renders += 1;
                        let img = layer_image(layer, doc.width, doc.height, size);
                        match self.layers.get_mut(&w.id) {
                            Some(t) if t.size == size => {
                                t.tex.set(img, opts);
                                t.epoch = epoch;
                            }
                            _ => {
                                let tex = ctx.load_texture(format!("layer-thumb-{}", w.id), img, opts);
                                self.layers.insert(w.id, Thumb { tex, size, epoch });
                            }
                        }
                    }
                }
            }
            if w.mask {
                let size = fit(doc.width, doc.height, MASK_BOX);
                let stale = self.masks.get(&w.id).is_none_or(|t| t.epoch != epoch || t.size != size);
                if stale {
                    if left == 0 {
                        pending = true;
                    } else if let Some(mask) = doc.node(w.id).and_then(|n| n.mask.as_ref()) {
                        left -= 1;
                        self.renders += 1;
                        let (tw, th) = texels(size);
                        let gray = render_mask(mask, doc.width, doc.height, tw, th);
                        let img = ColorImage::from_gray([tw, th], &gray);
                        match self.masks.get_mut(&w.id) {
                            Some(t) if t.size == size => {
                                t.tex.set(img, opts);
                                t.epoch = epoch;
                            }
                            _ => {
                                let tex = ctx.load_texture(format!("mask-thumb-{}", w.id), img, opts);
                                self.masks.insert(w.id, Thumb { tex, size, epoch });
                            }
                        }
                    }
                }
            }
        }
        pending
    }
}

fn layer_image(layer: &Layer, w: usize, h: usize, size: Vec2) -> ColorImage {
    let (tw, th) = texels(size);
    let rgb = render_tiled(layer, w, h, tw, th);
    ColorImage::from_rgb([tw, th], &rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_aspect_ratio_inside_the_box() {
        let s = fit(2048, 1536, THUMB_BOX);
        assert!(s.x <= THUMB_BOX.x && s.y <= THUMB_BOX.y);
        assert!(((s.x / s.y) - 4.0 / 3.0).abs() < 0.05);
        let tall = fit(100, 400, THUMB_BOX);
        assert_eq!(tall.y, THUMB_BOX.y);
        assert!(tall.x < 20.0);
    }

    #[test]
    fn an_empty_layer_shows_the_checkerboard_and_paint_covers_it() {
        let (w, h) = (64usize, 48usize);
        let mut px = vec![0u8; w * h * 4];
        let empty = render_layer(&px, w, h, 16, 12);
        assert_eq!(&empty[0..3], &[CHECK_LIGHT; 3]);
        assert_eq!(&empty[CELL * 3..CELL * 3 + 3], &[CHECK_DARK; 3]);
        for p in px.as_chunks_mut::<4>().0 {
            p.copy_from_slice(&[200, 20, 20, 255]);
        }
        let solid = render_layer(&px, w, h, 16, 12);
        assert!(solid.as_chunks::<3>().0.iter().all(|p| *p == [200, 20, 20]));
    }

    #[test]
    fn mask_thumbnail_follows_the_mask() {
        let mut m = Mask::filled(40, 40, 255);
        for y in 0..40 {
            for x in 0..20 {
                m.data[y * 40 + x] = 0;
            }
        }
        let g = render_mask(&m, 40, 40, 10, 10);
        assert_eq!(g[0], 0);
        assert_eq!(g[9], 255);
    }

    #[test]
    fn rendering_a_big_layer_is_cheap() {
        let (w, h) = (2048usize, 1536usize);
        let px = vec![90u8; w * h * 4];
        let t = std::time::Instant::now();
        let img = render_layer(&px, w, h, 116, 88);
        assert_eq!(img.len(), 116 * 88 * 3);
        // Generous bound for slow CI machines and debug builds; release is a few hundred µs.
        assert!(t.elapsed().as_millis() < 250, "{:?}", t.elapsed());
    }
}
