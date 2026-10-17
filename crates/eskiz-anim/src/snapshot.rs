//! Builds the animation stack from a document.

use eskiz_core::composite::composite_to_tiled;
use eskiz_core::{Document, Layer};

use crate::frames::{StackItem, StackSnapshot};

/// The top-level items of `doc` as a [`StackSnapshot`], bottom first, with the node id of each item.
///
/// A raster layer is taken with its visibility and opacity. A group is composited into one layer, so
/// its children blend inside it (the group's own opacity and blend mode go on the item). Private items
/// are kept in the snapshot but are in no frame. Layer masks are not applied: a masked layer needs its
/// mask baked first.
pub fn snapshot_from_document(doc: &Document) -> (StackSnapshot, Vec<u64>) {
    let (w, h) = (doc.width, doc.height);
    let mut items = Vec::with_capacity(doc.root.len());
    let mut ids = Vec::with_capacity(doc.root.len());
    for node in &doc.root {
        let mut layer = if node.is_group() {
            group_layer(doc, node.children())
        } else {
            doc.layer(node.id)
                .cloned()
                .unwrap_or_else(|| Layer::new(w, h))
        };
        layer.visible = node.props.visible;
        layer.opacity = node.props.opacity;
        items.push(StackItem {
            layer,
            blend: node.props.blend,
            private: node.props.private,
            clipped: node.props.clipped,
            animation_background: node.props.anim_background,
            animation_foreground: node.props.anim_foreground,
            hold: node.props.hold,
        });
        ids.push(node.id);
    }
    (StackSnapshot { width: w, height: h, items }, ids)
}

/// The children of a group, composited over nothing into one layer. The group's own properties are
/// left to the caller.
fn group_layer(doc: &Document, children: &[eskiz_core::Node]) -> Layer {
    let mut tmp = Document::empty(doc.width, doc.height);
    let mut ids = Vec::new();
    for node in children {
        node.layer_ids(&mut ids);
    }
    for id in ids {
        if let Some(layer) = doc.layer(id) {
            tmp.put_layer(id, layer.clone());
        }
    }
    tmp.root = children.to_vec();
    composite_to_tiled(&tmp, true)
}
