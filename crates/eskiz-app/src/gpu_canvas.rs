//! The GPU mirror of the document and its compositor.
//!
//! [`GpuCanvas`] keeps one [`GpuLayer`] (and, for masked layers, one [`GpuMask`]) per raster
//! layer of the document, as sparse 256 x 256 tiles. The CPU document stays the editing buffer
//! and the ground truth: the brush engine paints into `eskiz_core::Layer`, and this module
//! copies what changed to the GPU and composites there. Nothing in here touches egui, so the
//! tests can run it headlessly against the CPU compositor ([`crate::gpu_display`] is the egui
//! half).
//!
//! # What is uploaded, and when
//!
//! The caller says what may have changed ([`GpuCanvas::mark_rect`] for a stroke's dirty
//! rectangle, [`GpuCanvas::mark_all`] for anything that needs the whole canvas recomposited,
//! such as a property change or a reorder) and [`GpuCanvas::flush`] does the work once per
//! frame:
//!
//! * a layer is uploaded when its `Session::layer_epoch` differs from the value at its last
//!   upload (a committed edit, undo, a restored layer, ...). If a rectangle is pending only that
//!   rectangle is uploaded; after [`GpuCanvas::mark_all`] the whole layer is;
//! * the layer a stroke is painting into has no epoch change until the stroke is committed, so
//!   [`GpuCanvas::mark_rect`] also names it and its rectangle is uploaded regardless;
//! * a new layer is uploaded in full, and layers that left the document are dropped;
//! * a layer preview (adjustments, transform) is mirrored as a separate [`GpuLayer`] that takes
//!   the place of its layer in the scene.
//!
//! Empty tiles are never allocated on either side. After the uploads the compositor redoes only
//! the tiles under the dirty rectangle (or all tiles).
//!
//! # What the GPU path cannot show
//!
//! `eskiz-gpu` composites groups in place: a group's own opacity, blend mode and mask are not
//! applied. [`supports`] reports whether a document can be shown
//! exactly; the app falls back to the CPU display for documents with such groups.

use std::collections::{HashMap, HashSet};

use eskiz_core::document::{Document, Node, NodeKind};
use eskiz_core::canvas::Tile;
use eskiz_core::Layer;
use eskiz_gpu::{Adjuster, Adjustment, Compositor, GpuContext, GpuError, GpuLayer, GpuMask, Rect, Region, Scene, SceneGroup, SceneItem, SceneLayer};

use crate::history::PixelRect;
use crate::session::Session;

/// One document layer on the GPU.
struct Mirror {
    layer: GpuLayer,
    mask: Option<GpuMask>,
    /// `Session::layer_epoch` at the last upload.
    epoch: u64,
}

/// The layer preview on the GPU.
struct PreviewMirror {
    id: u64,
    layer: GpuLayer,
}

/// What the next flush has to do.
#[derive(Default)]
struct Pending {
    /// Recomposite the whole canvas.
    all: bool,
    /// Union of the dirty rectangles.
    rect: Option<PixelRect>,
    /// Layers whose pixels changed inside `rect` without a committed edit (a stroke in progress).
    layers: HashSet<u64>,
}

/// Work done by one [`GpuCanvas::flush`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlushStats {
    /// Whether anything was composited.
    pub composited: bool,
    /// Tiles uploaded to layer or mask textures are not counted; this is output tiles redone.
    pub tiles: usize,
    /// Layers uploaded (whole or in part).
    pub layers_uploaded: usize,
    /// True when the whole canvas was recomposited.
    pub full: bool,
}

/// GPU mirror of one document, plus its compositor.
pub struct GpuCanvas {
    ctx: GpuContext,
    comp: Option<Compositor>,
    size: (usize, usize),
    mirrors: HashMap<u64, Mirror>,
    preview: Option<PreviewMirror>,
    /// `Session::preview_gen` at the last sync.
    preview_gen: u64,
    pending: Pending,
    last: FlushStats,
    /// Pipelines of the GPU adjustments, created on first use.
    adjuster: Option<Adjuster>,
    /// The display matrix the last composite used (`None` for sRGB documents). A change of the
    /// document's colour space recomposites everything.
    display: Option<[[f32; 3]; 3]>,
}

/// Can the GPU compositor show this document exactly? Groups with their own opacity, blend mode
/// or mask are not supported yet (see the module docs). Neither are groups holding an item with a
/// blend mode other than Normal: the GPU blends such an item against the canvas below the group,
/// while the CPU (export, thumbnails, the fallback) blends it inside the group's own buffer.
pub fn supports(doc: &Document) -> bool {
    fn ok(nodes: &[Node], in_group: bool) -> bool {
        nodes.iter().all(|n| {
            let normal = n.props.blend == eskiz_core::BlendMode::Normal;
            match &n.kind {
                NodeKind::Layer => normal || !in_group,
                NodeKind::Group { children, .. } => {
                    (n.props.opacity - 1.0).abs() < 1e-9
                        && normal
                        && n.mask.is_none()
                        && ok(children, true)
                }
            }
        })
    }
    ok(&doc.root, false)
}

fn unit(v: f64) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0) as f32
    } else {
        0.0
    }
}

fn gpu_rect(r: PixelRect, w: usize, h: usize) -> Option<Rect> {
    let x0 = r.x.min(w);
    let y0 = r.y.min(h);
    let x1 = r.x.saturating_add(r.w).min(w);
    let y1 = r.y.saturating_add(r.h).min(h);
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Copy `rect` of a CPU layer into its GPU mirror. Tiles that are absent on the CPU and on the
/// GPU are skipped; a tile that went blank on the CPU is freed on the GPU when `rect` covers it.
pub fn sync_layer(ctx: &GpuContext, gpu: &mut GpuLayer, cpu: &Layer, rect: PixelRect) -> Result<(), GpuError> {
    let Some((tx0, ty0, tx1, ty1)) = cpu.tile_range(rect.x, rect.y, rect.w, rect.h) else {
        return Ok(());
    };
    // Tiles uploaded since the staging copies were last handed to the GPU.
    let mut queued = 0usize;
    let mut flushed = false;
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let (x, y, w, h) = cpu.tile_rect(tx, ty);
            let ix0 = x.max(rect.x);
            let iy0 = y.max(rect.y);
            let ix1 = (x + w).min(rect.x + rect.w);
            let iy1 = (y + h).min(rect.y + rect.h);
            if ix1 <= ix0 || iy1 <= iy0 {
                continue;
            }
            let coord = (tx as u32, ty as u32);
            if cpu.tile(tx, ty).is_none() {
                if !gpu.has_tile(coord) {
                    continue;
                }
                if (ix0, iy0, ix1, iy1) == (x, y, x + w, y + h) {
                    gpu.free_tile(coord);
                    continue;
                }
            }
            let r = Rect::new(ix0 as u32, iy0 as u32, (ix1 - ix0) as u32, (iy1 - iy0) as u32);
            let part = Rect::new((ix0 - x) as u32, (iy0 - y) as u32, r.w, r.h);
            match (cpu.tile(tx, ty).map(|t| &**t), gpu.depth()) {
                // The common case: upload from the tile's own samples, no copy of the part.
                (Some(Tile::U8(data)), eskiz_core::Depth::U8) => gpu.upload_tile(ctx, coord, part, data)?,
                (Some(Tile::U16(data)), eskiz_core::Depth::U16) => gpu.upload_tile16(ctx, coord, part, data)?,
                // A tile that went blank on the CPU but is only partly covered: upload zeros.
                _ if gpu.depth() == eskiz_core::Depth::U16 => {
                    gpu.upload_rect16(ctx, r, &cpu.read_rect16(ix0, iy0, ix1 - ix0, iy1 - iy0))?;
                }
                _ => {
                    gpu.upload_rect(ctx, r, &cpu.read_rect(ix0, iy0, ix1 - ix0, iy1 - iy0))?;
                }
            }
            // wgpu keeps a staging copy of every queued write until a submission completes;
            // hand them over every few hundred tiles so the first upload of a huge document
            // does not hold a copy of all of it (no effect on small uploads).
            queued += 1;
            if queued >= STAGING_TILES {
                ctx.flush_uploads();
                queued = 0;
                flushed = true;
            }
        }
    }
    if flushed && queued > 0 {
        ctx.flush_uploads();
    }
    Ok(())
}

/// Tile uploads between two hand-overs of the queued writes to the GPU (see `sync_layer`):
/// 128 MiB of 8-bit tiles. Handing them over more often only adds submission cost (measured
/// on the software adapter: 24 tiles made a full-layer upload 1.1 to 1.6 times slower).
const STAGING_TILES: usize = 512;

/// Copy `rect` of a canvas-sized mask (`width` bytes per row) to the GPU.
fn sync_mask(ctx: &GpuContext, gpu: &mut GpuMask, data: &[u8], width: usize, rect: PixelRect) -> Result<(), GpuError> {
    let mut crop = Vec::with_capacity(rect.w * rect.h);
    for row in rect.y..rect.y + rect.h {
        let start = row * width + rect.x;
        match data.get(start..start + rect.w) {
            Some(s) => crop.extend_from_slice(s),
            None => crop.resize(crop.len() + rect.w, 255),
        }
    }
    gpu.upload_rect(ctx, Rect::new(rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32), &crop)?;
    Ok(())
}

impl GpuCanvas {
    /// A canvas on this device. Nothing is allocated until [`GpuCanvas::resize`].
    pub fn new(ctx: GpuContext) -> Self {
        Self {
            ctx,
            comp: None,
            size: (0, 0),
            mirrors: HashMap::new(),
            preview: None,
            preview_gen: 0,
            pending: Pending::default(),
            last: FlushStats::default(),
            adjuster: None,
            display: None,
        }
    }

    /// What the last flush that composited something did.
    pub fn last_flush(&self) -> FlushStats {
        self.last
    }

    pub fn context(&self) -> &GpuContext {
        &self.ctx
    }

    /// Use another GPU wait bound (tests on slow software adapters).
    #[cfg(test)]
    pub(crate) fn set_wait_bound(&mut self, bound: std::time::Duration) {
        self.ctx.set_wait_bound(bound);
    }

    /// Can a `w x h` canvas be composited on this device?
    pub fn fits(&self, w: usize, h: usize) -> bool {
        let max = self.ctx.max_canvas_edge() as usize;
        w > 0 && h > 0 && w <= max && h <= max
    }

    /// Size of the canvas the compositor was made for.
    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// Make the output `w x h`. Returns true when the output texture is new, so that whoever
    /// registered it (egui) must register it again. Drops the mirrors and marks everything dirty
    /// on a change.
    pub fn resize(&mut self, w: usize, h: usize) -> bool {
        if self.comp.is_some() && self.size == (w, h) {
            return false;
        }
        self.mirrors.clear();
        self.preview = None;
        self.comp = Some(Compositor::new(&self.ctx, w as u32, h as u32));
        self.size = (w, h);
        self.pending = Pending {
            all: true,
            ..Pending::default()
        };
        true
    }

    /// The composited canvas, premultiplied RGBA8. `None` before [`GpuCanvas::resize`].
    pub fn output_view(&self) -> Option<&eframe::wgpu::TextureView> {
        self.comp.as_ref().map(Compositor::output_view)
    }

    /// Forget every GPU layer: the next flush creates them again and uploads all pixels
    /// (after the app returns from the background, or on a new device).
    pub fn invalidate_mirrors(&mut self) {
        self.mirrors.clear();
        self.preview = None;
        self.preview_gen = 0;
        self.pending.all = true;
    }

    /// Everything must be recomposited at the next flush.
    pub fn mark_all(&mut self) {
        self.pending.all = true;
    }

    /// Pixels of layer `layer` changed inside `rect` (a stroke in progress).
    pub fn mark_rect(&mut self, rect: PixelRect, layer: u64) {
        self.pending.rect = Some(self.pending.rect.map_or(rect, |r| r.union(rect)));
        self.pending.layers.insert(layer);
    }

    /// Is there anything to do at the next flush?
    pub fn is_dirty(&self) -> bool {
        self.pending.all || self.pending.rect.is_some()
    }

    /// Bring the GPU mirror up to date with the session and recomposite what changed. Submits
    /// GPU work and returns without waiting for it.
    pub fn flush(&mut self, session: &Session) -> Result<FlushStats, GpuError> {
        let doc = &session.doc;
        let (w, h) = (doc.width, doc.height);
        if self.comp.is_none() || self.size != (w, h) {
            if !self.fits(w, h) {
                return Err(GpuError::Invalid(format!("a {w} x {h} canvas does not fit the GPU limits")));
            }
            self.resize(w, h);
        }
        let preview_changed = session.preview_gen() != self.preview_gen;
        let display = crate::colour_management::display_matrix(doc.colour_space);
        let display_changed = display != self.display;
        if !self.is_dirty() && !preview_changed && !display_changed {
            return Ok(FlushStats::default());
        }
        let mut all = self.pending.all || preview_changed || display_changed;
        let rect = self.pending.rect.and_then(|r| {
            let r = PixelRect {
                x: r.x.min(w),
                y: r.y.min(h),
                w: r.w.min(w.saturating_sub(r.x)),
                h: r.h.min(h.saturating_sub(r.y)),
            };
            (r.w > 0 && r.h > 0).then_some(r)
        });
        let whole = PixelRect { x: 0, y: 0, w, h };

        // Layers: drop the ones that left, create the new ones, upload what changed.
        let ids = doc.layer_ids();
        let live: HashSet<u64> = ids.iter().copied().collect();
        self.mirrors.retain(|id, _| live.contains(id));
        let mut uploaded = 0;
        crate::stall::crumb(crate::stall::Phase::GpuUpload);
        for &id in &ids {
            let epoch = session.layer_epoch(id);
            // A mirror of the wrong depth (the document was converted) is rebuilt.
            let fresh = self.mirrors.get(&id).is_none_or(|m| m.layer.depth() != doc.depth);
            if fresh {
                self.mirrors.insert(
                    id,
                    Mirror {
                        layer: GpuLayer::new_depth(w as u32, h as u32, doc.depth),
                        mask: None,
                        epoch,
                    },
                );
            }
            let m = self.mirrors.get_mut(&id).expect("just inserted");
            let changed = fresh || m.epoch != epoch;
            let streaming = self.pending.layers.contains(&id);
            let node = doc.node(id);
            let has_mask = node.is_some_and(|n| n.mask.is_some());
            let mask_toggled = has_mask != m.mask.is_some();
            if mask_toggled {
                m.mask = has_mask.then(|| GpuMask::new(w as u32, h as u32, 1.0));
            }
            // Where to copy from. A committed edit with no rectangle to go on (or after
            // `mark_all`, when the edit may be anywhere) copies the whole layer.
            let region = if fresh || mask_toggled || (changed && (self.pending.all || rect.is_none())) {
                Some(whole)
            } else if changed || streaming {
                rect
            } else {
                None
            };
            let Some(region) = region else {
                continue;
            };
            all |= region == whole;
            if let Some(cpu) = doc.layer(id) {
                sync_layer(&self.ctx, &mut m.layer, cpu, region)?;
            }
            if let (Some(gm), Some(mask)) = (m.mask.as_mut(), node.and_then(|n| n.mask.as_ref())) {
                sync_mask(&self.ctx, gm, &mask.data, w, region)?;
            }
            m.epoch = epoch;
            uploaded += 1;
        }

        // The layer preview replaces its layer in the scene.
        if preview_changed {
            self.preview = None;
            if let Some((id, pixels)) = session.preview_layer() {
                if live.contains(&id) {
                    let mut layer = GpuLayer::new_depth(w as u32, h as u32, pixels.depth());
                    sync_layer(&self.ctx, &mut layer, pixels, whole)?;
                    self.preview = Some(PreviewMirror { id, layer });
                }
            }
            self.preview_gen = session.preview_gen();
        }

        let region = if all {
            Region::All
        } else {
            match rect.and_then(|r| gpu_rect(r, w, h)) {
                Some(r) => Region::Rect(r),
                None => Region::All,
            }
        };
        let full = matches!(region, Region::All);

        let background = doc
            .background_visible
            .then(|| [unit(doc.background[0]), unit(doc.background[1]), unit(doc.background[2])]);
        let preview = self.preview.as_ref();
        let mut scene = Scene::new();
        scene.background = background;
        scene.display = display;
        scene.items = scene_items(&doc.root, &self.mirrors, preview);
        let comp = self.comp.as_mut().expect("resized above");
        crate::stall::crumb(crate::stall::Phase::GpuComposite);
        let stats = comp.composite(&self.ctx, &scene, &region);
        crate::stall::crumb(crate::stall::Phase::CanvasComposite);
        let stats = stats?;
        self.pending = Pending::default();
        self.display = display;
        self.last = FlushStats {
            composited: true,
            tiles: stats.tiles,
            layers_uploaded: uploaded,
            full,
        };
        Ok(self.last)
    }

    /// Show `adj` applied to layer `id` in place of that layer, computed on the GPU from the
    /// layer's mirror (no CPU round trip). The document is not touched; the final apply stays on
    /// the CPU. Replaced by the next call, by [`GpuCanvas::clear_gpu_preview`], or when the
    /// session sets or clears its own CPU preview.
    pub fn set_gpu_preview(&mut self, session: &Session, id: u64, adj: &Adjustment) -> Result<(), GpuError> {
        // Brings the layer's mirror up to date (a no-op when nothing is pending).
        self.flush(session)?;
        let ctx = &self.ctx;
        let m = self
            .mirrors
            .get(&id)
            .ok_or_else(|| GpuError::Invalid(format!("layer {id} is not mirrored")))?;
        let adjuster = self.adjuster.get_or_insert_with(|| Adjuster::new(ctx));
        let layer = adjuster.apply(ctx, &m.layer, None, adj)?;
        self.preview = Some(PreviewMirror { id, layer });
        self.pending.all = true;
        Ok(())
    }

    /// Drop a preview made by [`GpuCanvas::set_gpu_preview`].
    pub fn clear_gpu_preview(&mut self) {
        if self.preview.take().is_some() {
            self.pending.all = true;
        }
    }

    /// The composite as straight RGBA8, read back from the GPU (blocks). For tests and export
    /// checks; the live display never reads back.
    pub fn read_straight(&self) -> Result<Vec<u8>, GpuError> {
        let comp = self.comp.as_ref().ok_or_else(|| GpuError::Invalid("no canvas".into()))?;
        let mut px = comp.download_output_all(&self.ctx)?;
        for p in px.as_chunks_mut::<4>().0 {
            let a = u32::from(p[3]);
            if a == 0 || a == 255 {
                continue;
            }
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
        Ok(px)
    }

    /// Wait for the GPU to finish what was submitted, for at most the context's wait bound.
    /// False on a timeout: the context is then marked stalled and the app moves to the CPU path.
    pub fn wait_idle(&self) -> bool {
        crate::stall::crumb(crate::stall::Phase::GpuReadback);
        let ok = self.ctx.wait_idle();
        crate::stall::crumb(crate::stall::Phase::CanvasComposite);
        ok
    }

    /// Number of layers mirrored.
    pub fn layer_count(&self) -> usize {
        self.mirrors.len()
    }
}

fn scene_items<'a>(
    nodes: &'a [Node],
    mirrors: &'a HashMap<u64, Mirror>,
    preview: Option<&'a PreviewMirror>,
) -> Vec<SceneItem<'a>> {
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        match &n.kind {
            NodeKind::Group { children, .. } => {
                let mut g = SceneGroup::new(scene_items(children, mirrors, preview));
                g.visible = n.props.visible;
                g.opacity = unit(n.props.opacity);
                g.blend = n.props.blend;
                out.push(SceneItem::Group(g));
            }
            NodeKind::Layer => {
                let Some(m) = mirrors.get(&n.id) else {
                    continue;
                };
                let layer = match preview {
                    Some(p) if p.id == n.id => &p.layer,
                    _ => &m.layer,
                };
                out.push(SceneItem::Layer(SceneLayer {
                    layer,
                    mask: m.mask.as_ref(),
                    visible: n.props.visible,
                    opacity: unit(n.props.opacity),
                    blend: n.props.blend,
                    clipped: n.props.clipped,
                }));
            }
        }
    }
    out
}

#[cfg(test)]
mod group_support_tests {
    use super::*;
    use eskiz_core::document::Props;
    use eskiz_core::BlendMode;

    fn node(id: u64, kind: NodeKind, blend: BlendMode) -> Node {
        let mut props = Props::new(format!("n{id}"));
        props.blend = blend;
        Node { id, props, mask: None, kind }
    }

    #[test]
    fn a_group_with_a_non_normal_child_goes_to_the_cpu_display() {
        let group = |blend| {
            let kid = node(2, NodeKind::Layer, blend);
            node(1, NodeKind::Group { children: vec![kid], collapsed: false }, BlendMode::Normal)
        };
        let mut doc = Document::new(8, 8);
        doc.root = vec![group(BlendMode::Normal)];
        assert!(supports(&doc));
        doc.root = vec![group(BlendMode::Multiply)];
        assert!(!supports(&doc));
        // A non-Normal layer at the top level has the same backdrop on both paths.
        doc.root = vec![node(3, NodeKind::Layer, BlendMode::Multiply)];
        assert!(supports(&doc));
    }
}
