//! Document model v2.
//!
//! A [`Document`] holds a tree of [`Node`]s (raster layers and groups, ordered bottom to
//! top) plus a background colour. Pixel buffers are **not** stored in the tree: a raster
//! layer node only carries a stable `id`, and the pixels live in a side store keyed by that
//! id ([`Document::layer`]). Each [`Layer`] is stored in sparse copy-on-write tiles, so cloning
//! a document (for a gallery save, say) shares all pixel memory.
//!
//! Everything that refers to an item (undo commands, the active layer, the layers panel)
//! uses ids, never positions, so reordering and undo cannot leave stale references.

use std::collections::{BTreeMap, HashMap};

use crate::blend::BlendMode;
use crate::animation::AnimationSettings;
use crate::canvas::Layer;
use crate::pixel::Depth;
use eskiz_guides::ColourSpace;
use eskiz_text::TextLayer;

/// Properties shared by layers and groups.
///
/// `clipped` and `alpha_lock` have no effect on a group.
#[derive(Clone, Debug, PartialEq)]
pub struct Props {
    pub name: String,
    pub visible: bool,
    /// 0..1.
    pub opacity: f64,
    pub blend: BlendMode,
    /// Clipped to the base layer below.
    pub clipped: bool,
    /// Painting keeps each pixel's alpha.
    pub alpha_lock: bool,
    /// Blocks pixel edits. Does not change the composite.
    pub locked: bool,
    /// Left out of flattened outputs that ask for it.
    pub private: bool,
    /// Extra Frames frames this item is held for. Stored as `animationHeldLength`.
    pub hold: u32,
    /// The bottom-most item is the animation background. Only the bottom item can be one.
    pub anim_background: bool,
    /// The top-most item is the animation foreground. Only the top item can be one.
    pub anim_foreground: bool,
}

impl Props {
    /// Default properties with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            clipped: false,
            alpha_lock: false,
            locked: false,
            private: false,
            hold: 0,
            anim_background: false,
            anim_foreground: false,
        }
    }
}

/// A canvas-sized 8-bit mask: 255 shows the item, 0 hides it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    /// `width * height` bytes, row-major.
    pub data: Vec<u8>,
}

impl Mask {
    /// A mask filled with `value` (255 = reveal all, 0 = hide all).
    pub fn filled(width: usize, height: usize, value: u8) -> Self {
        Self {
            data: vec![value; width * height],
        }
    }
}

/// What a node is.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// A raster layer. Its pixels are in [`Document::layer`] under the node's id.
    Layer,
    /// A group. `children` are ordered bottom to top. `collapsed` is a UI flag only.
    Group { children: Vec<Node>, collapsed: bool },
}

/// One item of the layer tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// Stable identity, unique within the document (layers and groups share one counter).
    pub id: u64,
    pub props: Props,
    pub mask: Option<Mask>,
    pub kind: NodeKind,
}

impl Node {
    pub fn is_group(&self) -> bool {
        matches!(self.kind, NodeKind::Group { .. })
    }

    /// Children of a group; empty for a layer.
    pub fn children(&self) -> &[Node] {
        match &self.kind {
            NodeKind::Group { children, .. } => children,
            NodeKind::Layer => &[],
        }
    }

    /// Mutable children of a group; `None` for a layer.
    pub fn children_mut(&mut self) -> Option<&mut Vec<Node>> {
        match &mut self.kind {
            NodeKind::Group { children, .. } => Some(children),
            NodeKind::Layer => None,
        }
    }

    /// Ids of every raster layer in this subtree, depth first, bottom to top.
    pub fn layer_ids(&self, out: &mut Vec<u64>) {
        match &self.kind {
            NodeKind::Layer => out.push(self.id),
            NodeKind::Group { children, .. } => children.iter().for_each(|c| c.layer_ids(out)),
        }
    }

    /// The node with `id` in this subtree (including itself).
    pub fn find(&self, id: u64) -> Option<&Node> {
        if self.id == id {
            return Some(self);
        }
        self.children().iter().find_map(|c| c.find(id))
    }

    fn find_mut(&mut self, id: u64) -> Option<&mut Node> {
        if self.id == id {
            return Some(self);
        }
        self.children_mut()?.iter_mut().find_map(|c| c.find_mut(id))
    }
}

/// Where a node sits: its parent group (`None` = top level) and index among the siblings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub parent: Option<u64>,
    pub index: usize,
}

/// Resolution of a new document, dots per inch (the `.procreate` default).
pub const DEFAULT_DPI: f64 = 132.0;

/// A copied subtree: the node, its layers' pixels and its text sources, keyed by the new ids.
pub type SubtreeCopy = (Node, Vec<(u64, Layer)>, Vec<(u64, TextLayer)>);

/// Every layer's pixels and every node's mask, as a whole-canvas operation keeps them for undo.
/// Layers share their tiles with the document, so a snapshot copies no pixels.
#[derive(Clone, Debug)]
pub struct CanvasPixels {
    /// The document's bits per channel when the snapshot was taken.
    pub depth: Depth,
    /// Layer id and its pixels, sorted by id.
    pub layers: Vec<(u64, Layer)>,
    /// Node id and its mask (`None` when the node has none), in tree order.
    pub masks: Vec<(u64, Option<Mask>)>,
}

/// A canvas-sized document: layer tree, pixel store and background.
#[derive(Clone, Debug)]
pub struct Document {
    pub width: usize,
    pub height: usize,
    /// Top-level items, bottom to top.
    pub root: Vec<Node>,
    /// Background colour, RGB 0..1. Not a layer.
    pub background: [f64; 3],
    pub background_visible: bool,
    /// Resolution in dots per inch. Stored with the document, read and written by the `.procreate`
    /// format, and shown in Canvas information.
    pub dpi: f64,
    /// The colour space the pixel values are in. New documents are sRGB. Stored in the
    /// `.procreate` colour profile, shown in Canvas information, converted by Actions > Canvas >
    /// Convert, mapped to the display by the compositors and embedded as an ICC profile in exports.
    pub colour_space: ColourSpace,
    /// Bits per channel of every layer (8 or 16). Layers added or imported are converted to it.
    /// Masks stay 8-bit. Changed with [`Document::convert_depth`].
    pub depth: Depth,
    /// Frames settings, stored with the document.
    pub animation: AnimationSettings,
    layers: HashMap<u64, Layer>,
    next_id: u64,
    /// Text layers: the attributed source of each text layer, by node id. The node's raster
    /// layer holds the last rendering of it. A node with no entry is an ordinary raster layer.
    pub texts: BTreeMap<u64, TextLayer>,
}

impl Document {
    /// Blank white document with one empty layer called "Layer 1".
    pub fn new(width: usize, height: usize) -> Self {
        let mut d = Self::empty(width, height);
        d.add_layer_above(None);
        d
    }

    /// A document with no items at all. Callers must add a layer before painting.
    pub fn empty(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            root: Vec::new(),
            background: [1.0; 3],
            background_visible: true,
            dpi: DEFAULT_DPI,
            colour_space: ColourSpace::default(),
            depth: Depth::U8,
            animation: AnimationSettings::default(),
            layers: HashMap::new(),
            next_id: 1,
            texts: BTreeMap::new(),
        }
    }

    /// [`Document::empty`] at the given depth.
    pub fn empty_depth(width: usize, height: usize, depth: Depth) -> Self {
        let mut d = Self::empty(width, height);
        d.depth = depth;
        d
    }

    /// [`Document::new`] at the given depth.
    pub fn new_depth(width: usize, height: usize, depth: Depth) -> Self {
        let mut d = Self::empty_depth(width, height, depth);
        d.add_layer_above(None);
        d
    }

    /// A fresh id. Ids are never reused within a document.
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ----- pixel store -----

    /// Pixels of raster layer `id`.
    pub fn layer(&self, id: u64) -> Option<&Layer> {
        self.layers.get(&id)
    }

    pub fn layer_mut(&mut self, id: u64) -> Option<&mut Layer> {
        self.layers.get_mut(&id)
    }

    /// Store pixels for layer `id` (used when restoring a deleted layer).
    pub fn put_layer(&mut self, id: u64, layer: Layer) {
        self.layers.insert(id, layer);
        self.next_id = self.next_id.max(id + 1);
    }

    /// Remove and return the pixels of layer `id`.
    pub fn take_layer(&mut self, id: u64) -> Option<Layer> {
        self.layers.remove(&id)
    }

    /// A transparent layer of the document's size and depth. Allocates no pixel memory.
    pub fn blank_layer(&self) -> Layer {
        Layer::new_depth(self.width, self.height, self.depth)
    }

    /// Convert every layer to `depth` and make it the document's depth. 8 to 16 is exact; 16 to
    /// 8 rounds, or dithers when `dither` is set. Layers keep their ids, tiles are rebuilt
    /// (nothing is shared with the old layers). Masks are 8-bit at both depths.
    pub fn convert_depth(&mut self, depth: Depth, dither: bool) {
        if depth == self.depth {
            return;
        }
        for layer in self.layers.values_mut() {
            *layer = layer.converted(depth, dither);
        }
        self.depth = depth;
    }

    /// Bytes of pixel data held by all layers, counting a tile shared between layers once.
    pub fn tile_bytes(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        let mut bytes = 0;
        for l in self.layers.values() {
            for ty in 0..l.tiles_y() {
                for tx in 0..l.tiles_x() {
                    if let Some(t) = l.tile(tx, ty) {
                        if seen.insert(std::sync::Arc::as_ptr(t)) {
                            bytes += t.byte_len();
                        }
                    }
                }
            }
        }
        bytes
    }

    // ----- tree queries -----

    /// The node with `id`.
    pub fn node(&self, id: u64) -> Option<&Node> {
        self.root.iter().find_map(|n| n.find(id))
    }

    pub fn node_mut(&mut self, id: u64) -> Option<&mut Node> {
        self.root.iter_mut().find_map(|n| n.find_mut(id))
    }

    /// Siblings under `parent` (`None` = top level). `None` if `parent` is not a group.
    pub fn children(&self, parent: Option<u64>) -> Option<&Vec<Node>> {
        match parent {
            None => Some(&self.root),
            Some(p) => match &self.node(p)?.kind {
                NodeKind::Group { children, .. } => Some(children),
                NodeKind::Layer => None,
            },
        }
    }

    pub fn children_mut(&mut self, parent: Option<u64>) -> Option<&mut Vec<Node>> {
        match parent {
            None => Some(&mut self.root),
            Some(p) => self.node_mut(p)?.children_mut(),
        }
    }

    /// Where `id` sits in the tree.
    pub fn locate(&self, id: u64) -> Option<Location> {
        fn walk(items: &[Node], parent: Option<u64>, id: u64) -> Option<Location> {
            for (index, n) in items.iter().enumerate() {
                if n.id == id {
                    return Some(Location { parent, index });
                }
                if let Some(l) = walk(n.children(), Some(n.id), id) {
                    return Some(l);
                }
            }
            None
        }
        walk(&self.root, None, id)
    }

    /// Ids of every raster layer, depth first, bottom to top.
    pub fn layer_ids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        self.root.iter().for_each(|n| n.layer_ids(&mut out));
        out
    }

    /// Number of raster layers.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Id of the `index`-th raster layer in [`Document::layer_ids`] order.
    pub fn layer_id(&self, index: usize) -> Option<u64> {
        self.layer_ids().get(index).copied()
    }

    /// Pixels of the `index`-th raster layer (bottom to top, depth first).
    pub fn layer_at(&self, index: usize) -> Option<&Layer> {
        self.layers.get(&self.layer_id(index)?)
    }

    pub fn layer_at_mut(&mut self, index: usize) -> Option<&mut Layer> {
        let id = self.layer_id(index)?;
        self.layers.get_mut(&id)
    }

    /// Properties of the `index`-th raster layer.
    pub fn props_at_mut(&mut self, index: usize) -> Option<&mut Props> {
        let id = self.layer_id(index)?;
        self.node_mut(id).map(|n| &mut n.props)
    }

    /// Position of layer `id` in [`Document::layer_ids`] order.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.layer_ids().iter().position(|&l| l == id)
    }

    /// Is `ancestor` a strict ancestor of `id`?
    pub fn is_ancestor(&self, ancestor: u64, id: u64) -> bool {
        self.node(ancestor)
            .is_some_and(|a| a.children().iter().any(|c| c.find(id).is_some()))
    }

    /// Ids of the groups containing `id`, nearest first.
    pub fn ancestors(&self, id: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut cur = id;
        while let Some(Location { parent: Some(p), .. }) = self.locate(cur) {
            out.push(p);
            cur = p;
        }
        out
    }

    /// Visible itself and in no hidden group.
    pub fn effective_visible(&self, id: u64) -> bool {
        self.node(id).is_some_and(|n| n.props.visible)
            && self.ancestors(id).iter().all(|&a| self.node(a).is_some_and(|n| n.props.visible))
    }

    /// Locked itself or inside a locked group.
    pub fn effective_locked(&self, id: u64) -> bool {
        self.node(id).is_some_and(|n| n.props.locked)
            || self.ancestors(id).iter().any(|&a| self.node(a).is_some_and(|n| n.props.locked))
    }

    // ----- tree edits (no history; the app's Session wraps these in commands) -----

    /// Insert `node` under `parent` at `index` (clamped). Returns false if `parent` is not a group.
    pub fn insert_node(&mut self, parent: Option<u64>, index: usize, node: Node) -> bool {
        let Some(list) = self.children_mut(parent) else {
            return false;
        };
        let at = index.min(list.len());
        list.insert(at, node);
        true
    }

    /// Detach a node (and its subtree) from the tree. Pixel buffers stay in the store.
    pub fn remove_node(&mut self, id: u64) -> Option<(Location, Node)> {
        let loc = self.locate(id)?;
        let list = self.children_mut(loc.parent)?;
        Some((loc, list.remove(loc.index)))
    }

    /// A new empty layer directly above `above` (in its parent), or on top of the root when
    /// `None` or unknown. Returns the new id.
    pub fn add_layer_above(&mut self, above: Option<u64>) -> u64 {
        let id = self.alloc_id();
        let node = Node {
            id,
            props: Props::new(format!("Layer {id}")),
            mask: None,
            kind: NodeKind::Layer,
        };
        let loc = above
            .and_then(|a| self.locate(a))
            .map(|l| Location {
                parent: l.parent,
                index: l.index + 1,
            })
            .unwrap_or(Location {
                parent: None,
                index: self.root.len(),
            });
        self.layers.insert(id, self.blank_layer());
        self.insert_node(loc.parent, loc.index, node);
        id
    }

    /// Append an imported raster layer on top of `parent` (or the root). `pixels` must be
    /// `width * height * 4` premultiplied RGBA8 bytes; any other length (an empty vector, say)
    /// gives a transparent layer.
    pub fn push_layer(&mut self, parent: Option<u64>, props: Props, mask: Option<Mask>, pixels: Vec<u8>) -> u64 {
        let layer = Layer::from_rgba(self.width, self.height, pixels);
        self.push_raster(parent, props, mask, layer)
    }

    /// [`Document::push_layer`] with premultiplied RGBA16 samples (`width * height * 4`). The
    /// layer is stored at the document's depth.
    pub fn push_layer16(&mut self, parent: Option<u64>, props: Props, mask: Option<Mask>, pixels: Vec<u16>) -> u64 {
        let layer = Layer::from_rgba16(self.width, self.height, pixels);
        self.push_raster(parent, props, mask, layer)
    }

    /// [`Document::push_layer`] with ready-made tiles. A layer of another size is replaced by a
    /// transparent one, and one of another depth is converted to the document's.
    pub fn push_raster(&mut self, parent: Option<u64>, props: Props, mask: Option<Mask>, layer: Layer) -> u64 {
        let id = self.alloc_id();
        let layer = if (layer.width, layer.height) == (self.width, self.height) {
            layer.converted(self.depth, false)
        } else {
            self.blank_layer()
        };
        self.layers.insert(id, layer);
        let node = Node {
            id,
            props,
            mask,
            kind: NodeKind::Layer,
        };
        let at = self.children(parent).map_or(0, Vec::len);
        self.insert_node(parent, at, node);
        id
    }

    /// Append an empty group on top of `parent` (or the root). Returns its id.
    pub fn push_group(&mut self, parent: Option<u64>, props: Props, mask: Option<Mask>) -> u64 {
        let id = self.alloc_id();
        let node = Node {
            id,
            props,
            mask,
            kind: NodeKind::Group {
                children: Vec::new(),
                collapsed: false,
            },
        };
        let at = self.children(parent).map_or(0, Vec::len);
        self.insert_node(parent, at, node);
        id
    }

    /// A deep copy of the subtree `id` with fresh ids, and the copied pixel buffers keyed by
    /// the new layer ids. Nothing is inserted.
    pub fn duplicate_subtree(&mut self, id: u64) -> Option<(Node, Vec<(u64, Layer)>)> {
        let (node, pixels, _) = self.duplicate_subtree_with_texts(id)?;
        Some((node, pixels))
    }

    /// [`Document::duplicate_subtree`] that also copies the text source of each text layer,
    /// keyed by the new layer ids, so the copy stays editable.
    pub fn duplicate_subtree_with_texts(&mut self, id: u64) -> Option<SubtreeCopy> {
        let mut node = self.node(id)?.clone();
        let mut pixels = Vec::new();
        let mut texts = Vec::new();
        self.renumber(&mut node, &mut pixels, &mut texts);
        Some((node, pixels, texts))
    }

    fn renumber(&mut self, node: &mut Node, pixels: &mut Vec<(u64, Layer)>, texts: &mut Vec<(u64, TextLayer)>) {
        let old = node.id;
        node.id = self.alloc_id();
        if let Some(t) = self.texts.get(&old) {
            texts.push((node.id, t.clone()));
        }
        match &mut node.kind {
            NodeKind::Layer => {
                if let Some(l) = self.layers.get(&old) {
                    pixels.push((node.id, l.clone()));
                }
            }
            NodeKind::Group { children, .. } => {
                for c in children {
                    self.renumber(c, pixels, texts);
                }
            }
        }
    }

    /// Every layer's pixels and every node's mask. [`Document::restore_canvas`] puts them back.
    pub fn canvas_pixels(&self) -> CanvasPixels {
        fn walk(items: &[Node], out: &mut Vec<(u64, Option<Mask>)>) {
            for n in items {
                out.push((n.id, n.mask.clone()));
                walk(n.children(), out);
            }
        }
        let mut layers: Vec<(u64, Layer)> = self.layers.iter().map(|(id, l)| (*id, l.clone())).collect();
        layers.sort_by_key(|(id, _)| *id);
        let mut masks = Vec::new();
        walk(&self.root, &mut masks);
        CanvasPixels { depth: self.depth, layers, masks }
    }

    /// Set the canvas size and resolution, and replace the pixels and masks in `px`. Layers and
    /// nodes keep their ids, order and properties; only the pixels and masks change.
    pub fn restore_canvas(&mut self, width: usize, height: usize, dpi: f64, px: &CanvasPixels) {
        self.width = width;
        self.height = height;
        self.dpi = dpi;
        self.depth = px.depth;
        for (id, layer) in &px.layers {
            // Operations that build layers from 8-bit buffers (crop, say) hand back 8-bit layers;
            // the document keeps the depth of the snapshot.
            self.put_layer(*id, layer.converted(px.depth, false));
        }
        for (id, mask) in &px.masks {
            if let Some(n) = self.node_mut(*id) {
                n.mask = mask.clone();
            }
        }
    }

    /// Mirror every layer and mask across the canvas centre.
    pub fn flip(&mut self, horizontal: bool) {
        let (w, h) = (self.width, self.height);
        for layer in self.layers.values_mut() {
            layer.flip(horizontal);
        }
        fn masks(items: &mut [Node], w: usize, h: usize, horizontal: bool) {
            for n in items {
                if let Some(m) = n.mask.as_mut() {
                    flip_buffer(&mut m.data, w, h, 1, horizontal);
                }
                if let Some(c) = n.children_mut() {
                    masks(c, w, h, horizontal);
                }
            }
        }
        masks(&mut self.root, w, h, horizontal);
    }
}

/// Mirror a `width x height` buffer of `bpp`-byte pixels. Horizontal swaps left and right.
pub fn flip_buffer<T>(buf: &mut [T], width: usize, height: usize, bpp: usize, horizontal: bool) {
    if buf.len() != width * height * bpp {
        return;
    }
    if horizontal {
        for row in buf.chunks_exact_mut(width * bpp) {
            for x in 0..width / 2 {
                for c in 0..bpp {
                    row.swap(x * bpp + c, (width - 1 - x) * bpp + c);
                }
            }
        }
    } else {
        let row_len = width * bpp;
        for y in 0..height / 2 {
            let (top, bottom) = buf.split_at_mut((height - 1 - y) * row_len);
            top[y * row_len..(y + 1) * row_len].swap_with_slice(&mut bottom[..row_len]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_document_has_one_layer() {
        let d = Document::new(4, 3);
        assert_eq!(d.root.len(), 1);
        assert_eq!(d.layer_count(), 1);
        assert_eq!(d.root[0].props.name, "Layer 1");
        assert!(d.layer(d.root[0].id).is_some());
    }

    #[test]
    fn add_inserts_above_and_inside_groups() {
        let mut d = Document::new(4, 3);
        let a = d.root[0].id;
        let g = d.push_group(None, Props::new("G"), None);
        let inner = d.push_layer(Some(g), Props::new("in"), None, Vec::new());
        let above = d.add_layer_above(Some(a));
        assert_eq!(d.locate(above), Some(Location { parent: None, index: 1 }));
        let in2 = d.add_layer_above(Some(inner));
        assert_eq!(d.locate(in2), Some(Location { parent: Some(g), index: 1 }));
        assert_eq!(d.layer_ids(), vec![a, above, inner, in2]);
        assert_eq!(d.ancestors(in2), vec![g]);
        assert!(d.is_ancestor(g, inner));
        assert!(!d.is_ancestor(inner, g));
    }

    #[test]
    fn effective_flags_follow_ancestors() {
        let mut d = Document::empty(2, 2);
        let g = d.push_group(None, Props::new("G"), None);
        let l = d.push_layer(Some(g), Props::new("L"), None, Vec::new());
        assert!(d.effective_visible(l));
        d.node_mut(g).unwrap().props.visible = false;
        assert!(!d.effective_visible(l));
        d.node_mut(g).unwrap().props.locked = true;
        assert!(d.effective_locked(l));
    }

    #[test]
    fn duplicate_gives_fresh_ids_and_copies_pixels() {
        let mut d = Document::empty(2, 2);
        let g = d.push_group(None, Props::new("G"), None);
        let l = d.push_layer(Some(g), Props::new("L"), None, vec![9; 16]);
        let (copy, pixels) = d.duplicate_subtree(g).unwrap();
        assert_ne!(copy.id, g);
        assert_ne!(copy.children()[0].id, l);
        assert_eq!(pixels.len(), 1);
        assert_eq!(pixels[0].1.to_rgba(), vec![9; 16]);
        // The copy shares the original's tiles.
        assert!(pixels[0].1.tile_shared(d.layer(l).unwrap(), 0, 0));
    }

    #[test]
    fn blank_layers_cost_no_pixel_memory() {
        let mut d = Document::new(2048, 1536);
        for _ in 0..19 {
            d.add_layer_above(None);
        }
        assert_eq!(d.layer_count(), 20);
        assert_eq!(d.tile_bytes(), 0);
        let id = d.layer_id(3).unwrap();
        d.layer_mut(id).unwrap().set_pixel(1000, 700, [1, 2, 3, 4]);
        assert_eq!(d.tile_bytes(), crate::canvas::TILE_BYTES);
        // A cloned document shares every tile.
        let c = d.clone();
        assert_eq!(c.tile_bytes(), crate::canvas::TILE_BYTES);
    }

    #[test]
    fn flip_twice_is_identity_and_moves_pixels() {
        let mut d = Document::empty(3, 2);
        let px: Vec<u8> = (0..24).collect();
        let l = d.push_layer(None, Props::new("L"), Some(Mask { data: vec![1, 2, 3, 4, 5, 6] }), px.clone());
        d.flip(true);
        assert_eq!(&d.layer(l).unwrap().to_rgba()[0..4], &px[8..12]);
        assert_eq!(d.node(l).unwrap().mask.as_ref().unwrap().data, vec![3, 2, 1, 6, 5, 4]);
        d.flip(true);
        assert_eq!(d.layer(l).unwrap().to_rgba(), px);
        d.flip(false);
        assert_eq!(&d.layer(l).unwrap().to_rgba()[0..12], &px[12..24]);
        d.flip(false);
        assert_eq!(d.layer(l).unwrap().to_rgba(), px);
    }
}
