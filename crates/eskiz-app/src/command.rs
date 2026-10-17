//! Reversible edits. Every undoable action is a [`Command`]: a user-facing name and a list of
//! [`Change`]s. A change records both the state before and after, so applying it forwards or
//! backwards is symmetric. Changes refer to layers and groups by id, never by position, so a
//! command stays valid whatever happened to the stack around it.
//!
//! Layer operations are built from a few primitives (insert, delete, move, properties, mask,
//! pixels), so "merge down" or "group" are just several changes in one command.
//!
//! Pixel changes hold **tiles**, not pixel copies: layers are stored in copy-on-write tiles
//! (see `eskiz_core::canvas`), so a snapshot of a tile is a reference count. A stroke records
//! only the tiles it touched, before and after; a whole-layer change keeps the layer's tile
//! table before and after, and tiles the change left alone are shared between the two, so
//! they cost nothing. [`Command::bytes`] and [`crate::history::History::bytes`] count each
//! distinct tile once.

use std::collections::HashSet;
use std::sync::Arc;

use eskiz_core::canvas::{Layer, Tile, TileSlot};
use eskiz_core::document::{Document, Location, Mask, Node, Props};
use eskiz_select::Selection;
use eskiz_text::TextLayer;

use crate::history::PixelRect;

/// What the app must redraw after a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    /// The composite did not change.
    None,
    /// Only this canvas rectangle changed.
    Rect(PixelRect),
    /// Re-composite the whole canvas.
    Everything,
}

impl Refresh {
    /// The refresh that covers both.
    pub fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Everything, _) | (_, Self::Everything) => Self::Everything,
            (Self::None, x) | (x, Self::None) => x,
            (Self::Rect(a), Self::Rect(b)) => Self::Rect(a.union(b)),
        }
    }

    /// The rectangle to redraw on a `width x height` canvas, or `None` when nothing changed.
    pub fn rect(self, width: usize, height: usize) -> Option<PixelRect> {
        match self {
            Self::None => None,
            Self::Rect(r) => Some(r),
            Self::Everything => Some(PixelRect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            }),
        }
    }
}

/// A document-wide operation that is its own bookkeeping (flip canvas now; crop and resize
/// later). `apply` and `revert` must be exact inverses.
pub trait DocOp: Send {
    fn apply(&self, doc: &mut Document);
    fn revert(&self, doc: &mut Document);
    /// Bytes of data the operation keeps for undo (for the history budget).
    fn bytes(&self) -> usize {
        0
    }
}

/// Flip every layer and mask across the canvas centre. Its own inverse.
pub struct FlipCanvas {
    pub horizontal: bool,
}

impl DocOp for FlipCanvas {
    fn apply(&self, doc: &mut Document) {
        doc.flip(self.horizontal);
    }
    fn revert(&self, doc: &mut Document) {
        doc.flip(self.horizontal);
    }
}

/// A subtree and the pixels of its layers, as stored in a command. The layers share their tiles
/// with the document, so capturing one copies no pixels, and an empty layer holds none.
pub struct Placement {
    pub parent: Option<u64>,
    pub index: usize,
    pub node: Node,
    pub pixels: Vec<(u64, Layer)>,
    /// The text sources of the text layers in the subtree, keyed by layer id.
    pub texts: Vec<(u64, TextLayer)>,
}

impl Placement {
    /// Capture the subtree `id` as it is now.
    pub fn capture(doc: &Document, id: u64) -> Option<Self> {
        let loc = doc.locate(id)?;
        let node = doc.node(id)?.clone();
        let mut ids = Vec::new();
        node.layer_ids(&mut ids);
        let texts = ids.iter().filter_map(|&l| Some((l, doc.texts.get(&l)?.clone()))).collect();
        let pixels = ids.into_iter().filter_map(|l| Some((l, doc.layer(l)?.clone()))).collect();
        Some(Self {
            parent: loc.parent,
            index: loc.index,
            node,
            pixels,
            texts,
        })
    }

    /// A subtree that is not in the document yet (a new or duplicated layer).
    pub fn new(parent: Option<u64>, index: usize, node: Node, pixels: Vec<(u64, Layer)>) -> Self {
        Self {
            parent,
            index,
            node,
            pixels,
            texts: Vec::new(),
        }
    }

    fn insert(&self, doc: &mut Document) {
        for (id, layer) in &self.pixels {
            doc.put_layer(*id, layer.clone());
        }
        for (id, text) in &self.texts {
            doc.texts.insert(*id, text.clone());
        }
        doc.insert_node(self.parent, self.index, self.node.clone());
    }

    fn remove(&self, doc: &mut Document) {
        if doc.remove_node(self.node.id).is_some() {
            for (id, _) in &self.pixels {
                doc.take_layer(*id);
            }
            for (id, _) in &self.texts {
                doc.texts.remove(id);
            }
        }
    }

    fn account(&self, seen: &mut HashSet<*const Tile>) -> usize {
        let tiles: usize = self.pixels.iter().map(|(_, l)| account_layer(l, seen)).sum();
        tiles + self.node.mask.as_ref().map_or(0, |m| m.data.len())
    }
}

/// Bytes of the tiles of `layer` that are not in `seen` yet; marks them seen.
fn account_layer(layer: &Layer, seen: &mut HashSet<*const Tile>) -> usize {
    let mut bytes = 0;
    for ty in 0..layer.tiles_y() {
        for tx in 0..layer.tiles_x() {
            if let Some(t) = layer.tile(tx, ty) {
                if seen.insert(Arc::as_ptr(t)) {
                    bytes += t.byte_len();
                }
            }
        }
    }
    bytes
}

/// The distinct tile allocations a layer references, with their size in bytes.
fn layer_tiles(layer: &Layer) -> std::collections::HashMap<*const Tile, usize> {
    let mut out = std::collections::HashMap::new();
    for ty in 0..layer.tiles_y() {
        for tx in 0..layer.tiles_x() {
            if let Some(t) = layer.tile(tx, ty) {
                out.insert(Arc::as_ptr(t), t.byte_len());
            }
        }
    }
    out
}

/// One reversible primitive.
pub enum Change {
    /// The tiles of a layer that a stroke, smudge or Pour Fill changed, before and after.
    /// `rect` is the pixel rectangle to redraw. Only tiles whose pixels differ are listed.
    Tiles {
        layer: u64,
        rect: PixelRect,
        before: Vec<TileSlot>,
        after: Vec<TileSlot>,
    },
    /// A whole layer's pixels (clear, filters, fills, merge). Both layers share every tile the
    /// change did not touch, so only the difference costs memory.
    LayerPixels { layer: u64, before: Layer, after: Layer },
    /// Add a subtree (backwards: remove it).
    Insert(Placement),
    /// Remove a subtree (backwards: put it back with all its pixels).
    Delete(Placement),
    /// Move a node. `to.index` is the index after the node has been taken out of its old list.
    Move { id: u64, from: Location, to: Location },
    /// Name, visibility, opacity, blend mode, clipping and the flags.
    Props { id: u64, before: Props, after: Props },
    /// Add, remove or replace a layer or group mask.
    Mask {
        id: u64,
        before: Option<Mask>,
        after: Option<Mask>,
    },
    /// The text source of a text layer (`None` once it is rasterised). Pixels are a separate
    /// `LayerPixels` or `Tiles` change in the same command.
    Text {
        id: u64,
        before: Option<TextLayer>,
        after: Option<TextLayer>,
    },
    /// Background colour and visibility.
    Background {
        before: ([f64; 3], bool),
        after: ([f64; 3], bool),
    },
    /// A document-wide operation.
    Doc(Box<dyn DocOp>),
}

impl Change {
    /// Apply forwards (`forward`) or backwards.
    pub fn apply(&self, doc: &mut Document, forward: bool) {
        match self {
            Self::Tiles {
                layer, before, after, ..
            } => {
                if let Some(l) = doc.layer_mut(*layer) {
                    l.restore_tiles(if forward { after } else { before });
                }
            }
            Self::LayerPixels { layer, before, after } => {
                if let Some(l) = doc.layer_mut(*layer) {
                    l.copy_tiles_from(if forward { after } else { before });
                }
            }
            Self::Insert(p) => {
                if forward {
                    p.insert(doc);
                } else {
                    p.remove(doc);
                }
            }
            Self::Delete(p) => {
                if forward {
                    p.remove(doc);
                } else {
                    p.insert(doc);
                }
            }
            Self::Move { id, from, to } => {
                let dst = if forward { to } else { from };
                if let Some((_, node)) = doc.remove_node(*id) {
                    doc.insert_node(dst.parent, dst.index, node);
                }
            }
            Self::Props { id, before, after } => {
                if let Some(n) = doc.node_mut(*id) {
                    n.props = if forward { after } else { before }.clone();
                }
            }
            Self::Mask { id, before, after } => {
                if let Some(n) = doc.node_mut(*id) {
                    n.mask = if forward { after } else { before }.clone();
                }
            }
            Self::Text { id, before, after } => {
                match if forward { after } else { before } {
                    Some(text) => doc.texts.insert(*id, text.clone()),
                    None => doc.texts.remove(id),
                };
            }
            Self::Background { before, after } => {
                let (c, v) = if forward { after } else { before };
                doc.background = *c;
                doc.background_visible = *v;
            }
            Self::Doc(op) => {
                if forward {
                    op.apply(doc);
                } else {
                    op.revert(doc);
                }
            }
        }
    }

    /// Does the composite change? Name, lock, alpha-lock and private flags do not.
    pub fn refresh(&self) -> Refresh {
        match self {
            Self::Tiles { rect, .. } => Refresh::Rect(*rect),
            Self::Text { .. } => Refresh::None,
            Self::Props { before, after, .. } => {
                if before.visible != after.visible
                    || before.opacity != after.opacity
                    || before.blend != after.blend
                    || before.clipped != after.clipped
                {
                    Refresh::Everything
                } else {
                    Refresh::None
                }
            }
            _ => Refresh::Everything,
        }
    }

    /// A change of the tiles that differ between `before` (the layer as it was) and `after`
    /// (as it is now) inside `rect`. Tiles the two layers share are left out, and tiles that
    /// only look different by allocation are left out when their pixels are equal.
    pub fn tiles_between(layer: u64, before: &Layer, after: &Layer, rect: PixelRect) -> Self {
        let changed = after.changed_tiles(before, rect.x, rect.y, rect.w, rect.h);
        Self::Tiles {
            layer,
            rect,
            before: before.snapshot_tiles(&changed),
            after: after.snapshot_tiles(&changed),
        }
    }

    /// Bytes of data held, for the history budget. A tile counts once however many times it
    /// is referenced; `seen` carries the tiles already counted (by other changes).
    pub fn account(&self, seen: &mut HashSet<*const Tile>) -> usize {
        match self {
            Self::Tiles { before, after, .. } => before
                .iter()
                .chain(after)
                .filter_map(|s| s.tile.as_ref())
                .filter(|t| seen.insert(Arc::as_ptr(t)))
                .map(|t| t.byte_len())
                .sum(),
            Self::LayerPixels { before, after, .. } => {
                // Tiles both sides hold are the ones the change left alone: the document keeps
                // them either way, so only the tiles that differ are undo data.
                let (b, a) = (layer_tiles(before), layer_tiles(after));
                b.keys().filter(|k| !a.contains_key(*k)).chain(a.keys().filter(|k| !b.contains_key(*k))).filter(|&&t| seen.insert(t)).map(|t| b.get(t).or(a.get(t)).copied().unwrap_or(0)).sum()
            }
            Self::Insert(p) | Self::Delete(p) => p.account(seen),
            Self::Mask { before, after, .. } => {
                before.as_ref().map_or(0, |m| m.data.len()) + after.as_ref().map_or(0, |m| m.data.len())
            }
            Self::Doc(op) => op.bytes(),
            Self::Move { .. } | Self::Props { .. } | Self::Background { .. } | Self::Text { .. } => 0,
        }
    }
}

/// The selection before and after one step. Undo restores `before`, redo restores `after`.
#[derive(Clone, Debug)]
pub struct SelectionStep {
    pub before: Arc<Selection>,
    pub after: Arc<Selection>,
}

/// One undo step: a name for the toast and the changes made, in order.
pub struct Command {
    /// "Stroke", "Add Layer", ... The undo toast shows "Undo <name>".
    pub name: &'static str,
    pub changes: Vec<Change>,
    /// The selection change made by this step, if any (see [`SelectionStep`]).
    pub selection: Option<SelectionStep>,
}

impl Command {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            changes: Vec::new(),
            selection: None,
        }
    }

    pub fn single(name: &'static str, change: Change) -> Self {
        Self {
            name,
            changes: vec![change],
            selection: None,
        }
    }

    /// Apply one change to `doc` now and record it.
    pub fn apply(&mut self, doc: &mut Document, change: Change) {
        change.apply(doc, true);
        self.changes.push(change);
    }

    /// Re-apply every change (redo).
    pub fn redo(&self, doc: &mut Document) {
        for c in &self.changes {
            c.apply(doc, true);
        }
    }

    /// Revert every change, newest first (undo).
    pub fn undo(&self, doc: &mut Document) {
        for c in self.changes.iter().rev() {
            c.apply(doc, false);
        }
    }

    pub fn refresh(&self) -> Refresh {
        self.changes.iter().fold(Refresh::None, |r, c| r.union(c.refresh()))
    }

    /// Bytes of data held, counting each distinct tile once.
    pub fn bytes(&self) -> usize {
        self.account(&mut HashSet::new())
    }

    /// Like [`Command::bytes`], but skipping tiles already in `seen` (held by other commands).
    pub fn account(&self, seen: &mut HashSet<*const Tile>) -> usize {
        let pixels: usize = self.changes.iter().map(|c| c.account(seen)).sum();
        let mask_bytes = |s: &Selection| s.mask().map_or(0, |m| m.data.len() * 4);
        let selection: usize = self
            .selection
            .as_ref()
            .map_or(0, |step| mask_bytes(&step.before) + mask_bytes(&step.after));
        pixels + selection
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.selection.is_none()
    }

    /// Ids of the layers and groups whose pixels or mask this command changes, pushed onto
    /// `out`. Returns false when it may change every layer (a document-wide operation).
    /// The layers panel uses this to refresh only the thumbnails that went stale.
    pub fn touched(&self, out: &mut Vec<u64>) -> bool {
        for c in &self.changes {
            match c {
                Change::Tiles { layer, .. } | Change::LayerPixels { layer, .. } => out.push(*layer),
                Change::Mask { id, .. } => out.push(*id),
                Change::Insert(p) | Change::Delete(p) => {
                    out.push(p.node.id);
                    p.node.layer_ids(out);
                }
                Change::Doc(_) => return false,
                Change::Move { .. } | Change::Props { .. } | Change::Background { .. } | Change::Text { .. } => {}
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eskiz_core::canvas::TILE_BYTES;
    use eskiz_core::document::NodeKind;


    fn doc_with_two() -> (Document, u64, u64) {
        let mut d = Document::empty(2, 2);
        let a = d.push_layer(None, Props::new("a"), None, vec![5; 16]);
        let b = d.push_layer(None, Props::new("b"), None, vec![7; 16]);
        (d, a, b)
    }

    #[test]
    fn delete_keeps_pixels_and_position() {
        let (mut d, a, b) = doc_with_two();
        let mut cmd = Command::new("Delete Layer");
        let p = Placement::capture(&d, a).unwrap();
        cmd.apply(&mut d, Change::Delete(p));
        assert_eq!(d.layer_ids(), vec![b]);
        assert!(d.layer(a).is_none());
        cmd.undo(&mut d);
        assert_eq!(d.layer_ids(), vec![a, b]);
        assert_eq!(d.layer(a).unwrap().to_rgba(), vec![5; 16]);
        cmd.redo(&mut d);
        assert_eq!(d.layer_ids(), vec![b]);
    }

    #[test]
    fn move_into_group_and_back() {
        let (mut d, a, b) = doc_with_two();
        let g = d.push_group(None, Props::new("g"), None);
        let from = d.locate(a).unwrap();
        let mut cmd = Command::new("Move Layer");
        cmd.apply(
            &mut d,
            Change::Move {
                id: a,
                from,
                to: Location { parent: Some(g), index: 0 },
            },
        );
        assert_eq!(d.locate(a).unwrap().parent, Some(g));
        assert_eq!(d.root.iter().map(|n| n.id).collect::<Vec<_>>(), vec![b, g]);
        cmd.undo(&mut d);
        assert_eq!(d.locate(a), Some(Location { parent: None, index: 0 }));
        assert!(matches!(d.node(g).unwrap().kind, NodeKind::Group { .. }));
    }

    #[test]
    fn empty_pixel_snapshot_means_transparent() {
        let (mut d, a, _) = doc_with_two();
        let before = d.layer(a).unwrap().clone();
        let mut cmd = Command::new("Clear Layer");
        let after = d.blank_layer();
        cmd.apply(&mut d, Change::LayerPixels { layer: a, before, after });
        assert!(d.layer(a).unwrap().to_rgba().iter().all(|&v| v == 0));
        assert_eq!(d.layer(a).unwrap().tile_count(), 0);
        cmd.undo(&mut d);
        assert_eq!(d.layer(a).unwrap().to_rgba(), vec![5; 16]);
    }

    #[test]
    fn bytes_count_each_distinct_tile_once() {
        let mut d = Document::empty(600, 300);
        let a = d.push_layer(None, Props::new("a"), None, vec![5; 600 * 300 * 4]);
        assert_eq!(d.layer(a).unwrap().tile_count(), 6);
        // Replacing one tile of six: only the old and the new tile are undo data.
        let before = d.layer(a).unwrap().clone();
        let mut after = before.clone();
        after.set_pixel(300, 10, [1, 2, 3, 4]);
        let c = Change::LayerPixels { layer: a, before: before.clone(), after };
        assert_eq!(c.account(&mut HashSet::new()), 2 * TILE_BYTES);
        // A stroke-style change lists the same two tiles.
        let mut after = before.clone();
        after.set_pixel(300, 10, [1, 2, 3, 4]);
        let t = Change::tiles_between(a, &before, &after, PixelRect { x: 0, y: 0, w: 600, h: 300 });
        let mut seen = HashSet::new();
        assert_eq!(t.account(&mut seen), 2 * TILE_BYTES);
        // Counting again with the same `seen` finds nothing new.
        assert_eq!(t.account(&mut seen), 0);
        // Deleting a layer keeps its tiles, once, however many commands hold them.
        let p = Placement::capture(&d, a).unwrap();
        assert_eq!(Change::Delete(p).account(&mut HashSet::new()), 6 * TILE_BYTES);
        let clone = Placement::capture(&d, a).unwrap();
        let mut seen = HashSet::new();
        assert_eq!(Change::Insert(clone).account(&mut seen), 6 * TILE_BYTES);
        assert_eq!(Change::Delete(Placement::capture(&d, a).unwrap()).account(&mut seen), 0);
    }

    #[test]
    fn tiles_change_applies_both_ways_and_skips_unchanged_tiles() {
        let mut d = Document::empty(600, 300);
        let a = d.push_layer(None, Props::new("a"), None, vec![5; 600 * 300 * 4]);
        let before = d.layer(a).unwrap().clone();
        let mut cmd = Command::new("Stroke");
        {
            let l = d.layer_mut(a).unwrap();
            l.set_pixel(10, 10, [9, 9, 9, 9]);
            l.set_pixel(590, 290, [8, 8, 8, 8]);
        }
        let rect = PixelRect { x: 0, y: 0, w: 600, h: 300 };
        let change = Change::tiles_between(a, &before, d.layer(a).unwrap(), rect);
        let Change::Tiles { before: b, after: af, .. } = &change else { panic!() };
        assert_eq!((b.len(), af.len()), (2, 2), "only the two touched tiles are stored");
        cmd.changes.push(change);
        let painted = d.layer(a).unwrap().clone();
        cmd.undo(&mut d);
        assert_eq!(*d.layer(a).unwrap(), before);
        cmd.redo(&mut d);
        assert_eq!(*d.layer(a).unwrap(), painted);
    }

    #[test]
    fn props_refresh_ignores_name_only_changes() {
        let a = Props::new("x");
        let mut b = a.clone();
        b.name = "y".into();
        let c = Change::Props { id: 1, before: a.clone(), after: b };
        assert_eq!(c.refresh(), Refresh::None);
        let mut v = a.clone();
        v.opacity = 0.5;
        assert_eq!(Change::Props { id: 1, before: a, after: v }.refresh(), Refresh::Everything);
    }

    #[test]
    fn refresh_union_prefers_everything_then_rects() {
        let r = PixelRect { x: 0, y: 0, w: 2, h: 2 };
        let s = PixelRect { x: 4, y: 4, w: 2, h: 2 };
        assert_eq!(Refresh::None.union(Refresh::Rect(r)), Refresh::Rect(r));
        assert_eq!(
            Refresh::Rect(r).union(Refresh::Rect(s)),
            Refresh::Rect(PixelRect { x: 0, y: 0, w: 6, h: 6 })
        );
        assert_eq!(Refresh::Rect(r).union(Refresh::Everything), Refresh::Everything);
        assert_eq!(Refresh::Everything.rect(8, 6), Some(PixelRect { x: 0, y: 0, w: 8, h: 6 }));
    }
}
