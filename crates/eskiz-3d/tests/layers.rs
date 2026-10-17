mod common;

use common::*;
use eskiz_3d::{
    load_gltf_slice, model_layer_groups, texture_set_layers, ImportOptions, LayerNode, MapKind,
    Model,
};

fn one_set_model() -> Model {
    load_gltf_slice(
        cube_gltf(UvMode::Atlas, 4).as_bytes(),
        &ImportOptions {
            default_texture_size: 64,
        },
    )
    .unwrap()
    .model
}

#[test]
fn texture_set_has_the_spec_layer_structure() {
    let m = one_set_model();
    let g = texture_set_layers(&m.texture_sets[0]);
    assert_eq!(g.name, "red");
    assert_eq!(g.children.len(), 5);
    let LayerNode::Group(base) = &g.children[0] else {
        panic!("the base group comes first")
    };
    assert_eq!(base.name, "Base");
    assert_eq!(
        base.children,
        vec![
            LayerNode::Map(MapKind::Colour),
            LayerNode::Map(MapKind::Roughness),
            LayerNode::Map(MapKind::Metallic)
        ]
    );
    assert_eq!(g.children[1], LayerNode::Composite);
    assert_eq!(
        g.children[2],
        LayerNode::Selection {
            width: 64,
            height: 64
        }
    );
    assert_eq!(g.children[3], LayerNode::Map(MapKind::Normal));
    assert_eq!(g.children[4], LayerNode::Map(MapKind::AmbientOcclusion));
    assert_eq!(g.maps(), MapKind::ALL.to_vec());
    assert!(g.visible && !g.locked && (g.opacity - 1.0).abs() < 1e-6);
}

#[test]
fn model_groups_follow_reverse_material_order_and_carry_set_state() {
    let mut m = one_set_model();
    let mut second = m.texture_sets[0].clone();
    second.name = "second".into();
    second.locked = true;
    second.opacity = 0.5;
    m.texture_sets.push(second);
    let groups = model_layer_groups(&m);
    assert_eq!(
        groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        ["second", "red"]
    );
    assert!(groups[0].locked && (groups[0].opacity - 0.5).abs() < 1e-6);
    assert!(!groups[1].locked);
}
