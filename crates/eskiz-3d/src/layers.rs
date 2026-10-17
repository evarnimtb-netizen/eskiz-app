//! Layer structure of texture sets.
//!
//! eskiz-core has no layer tree, so this module describes the nodes a document
//! builds for each texture set: a base group (colour, roughness, metallic), a derived
//! composite, a selection layer, and the normal and ambient occlusion maps as their
//! own layers. Visibility, lock and opacity belong to the set and sit on the top group.

use crate::model::{MapKind, Model, TextureSet};

/// Name of the base group inside each texture set.
pub const BASE_GROUP_NAME: &str = "Base";

/// A node of a texture set's layer structure.
#[derive(Clone, Debug, PartialEq)]
pub enum LayerNode {
    /// A painted map layer.
    Map(MapKind),
    /// The blended result of the set. Derived, never painted directly.
    Composite,
    /// Selection layer, sized to the texture bounds (`width` x `height` texels).
    Selection { width: usize, height: usize },
    /// A nested group.
    Group(LayerGroup),
}

/// A group of layer nodes. The top-level group of a texture set carries the set's state.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerGroup {
    pub name: String,
    pub children: Vec<LayerNode>,
    pub visible: bool,
    pub locked: bool,
    /// 0..1.
    pub opacity: f32,
}

impl LayerGroup {
    /// Map kinds in tree order, depth first.
    pub fn maps(&self) -> Vec<MapKind> {
        let mut out = Vec::new();
        collect_maps(&self.children, &mut out);
        out
    }
}

fn collect_maps(nodes: &[LayerNode], out: &mut Vec<MapKind>) {
    for node in nodes {
        match node {
            LayerNode::Map(kind) => out.push(*kind),
            LayerNode::Group(group) => collect_maps(&group.children, out),
            LayerNode::Composite | LayerNode::Selection { .. } => {}
        }
    }
}

/// Layer structure of one texture set: base group, composite, selection, normal, AO.
pub fn texture_set_layers(ts: &TextureSet) -> LayerGroup {
    let base = LayerNode::Group(LayerGroup {
        name: BASE_GROUP_NAME.to_string(),
        children: MapKind::BASE_GROUP
            .iter()
            .map(|&k| LayerNode::Map(k))
            .collect(),
        visible: true,
        locked: false,
        opacity: 1.0,
    });
    LayerGroup {
        name: ts.name.clone(),
        children: vec![
            base,
            LayerNode::Composite,
            LayerNode::Selection {
                width: ts.width,
                height: ts.height,
            },
            LayerNode::Map(MapKind::Normal),
            LayerNode::Map(MapKind::AmbientOcclusion),
        ],
        visible: ts.visible,
        locked: ts.locked,
        opacity: ts.opacity,
    }
}

/// Layer groups for every texture set of `model`, in the usual list order (reverse material order).
pub fn model_layer_groups(model: &Model) -> Vec<LayerGroup> {
    model
        .texture_set_display_order()
        .into_iter()
        .map(|i| texture_set_layers(&model.texture_sets[i]))
        .collect()
}
