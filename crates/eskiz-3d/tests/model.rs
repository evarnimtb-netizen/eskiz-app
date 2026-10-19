mod common;

use common::*;
use eskiz_3d::check::overlapping_texels;
use eskiz_3d::{
    export_map_pngs, load_gltf_slice, load_obj_str, write_glb_bytes, FatalIssue, ImportError,
    ImportOptions, MapKind, Warning,
};

fn opts() -> ImportOptions {
    ImportOptions {
        default_texture_size: 64,
    }
}

#[test]
fn loads_generated_gltf_cube() {
    let m = load_gltf_slice(cube_gltf(UvMode::Atlas, 4).as_bytes(), &opts()).unwrap();
    assert!(m.report.warnings.is_empty(), "{:?}", m.report);
    assert!(!m.normals_generated);
    let model = m.model;
    assert_eq!(model.submeshes.len(), 1);
    assert_eq!(model.triangle_count(), 12);
    assert_eq!(model.submeshes[0].positions.len(), 24);
    assert_eq!(model.submeshes[0].uvs.len(), 24);
    assert_eq!(model.texture_sets.len(), 1);
    let ts = &model.texture_sets[0];
    assert_eq!(ts.name, "red");
    assert_eq!((ts.width, ts.height), (64, 64));
    // Colour factor (1,0,0) becomes a constant red map; roughness 0.5, metallic 0.25.
    assert_eq!(ts.map(MapKind::Colour).pixel(3, 3), [255, 0, 0, 255]);
    assert_eq!(ts.map(MapKind::Roughness).pixel(3, 3)[0], 128);
    assert_eq!(ts.map(MapKind::Metallic).pixel(3, 3)[0], 64);
    assert_eq!(ts.map(MapKind::Normal).pixel(0, 0), [128, 128, 255, 255]);
    assert_eq!(
        ts.map(MapKind::AmbientOcclusion).pixel(0, 0),
        [255, 255, 255, 255]
    );
    let (lo, hi) = model.bounds().unwrap();
    assert!(
        (lo + glam::Vec3::splat(0.5)).length() < 1e-5
            && (hi - glam::Vec3::splat(0.5)).length() < 1e-5
    );
}

#[test]
fn loads_generated_obj_cube_with_material() {
    let m = load_obj_str(&cube_obj(UvMode::Atlas, true), Some(CUBE_MTL), &opts()).unwrap();
    let model = &m.model;
    assert_eq!(model.triangle_count(), 12);
    // OBJ has no normals: they are generated (flat), and vertices are unwelded.
    assert!(m.normals_generated);
    assert_eq!(
        model.submeshes[0].normals.len(),
        model.submeshes[0].positions.len()
    );
    assert_eq!(model.texture_sets[0].name, "blue");
    assert_eq!(
        model.texture_sets[0].map(MapKind::Colour).pixel(0, 0),
        [0, 0, 255, 255]
    );
    // v is flipped to the top-left convention: the first face's UVs round trip.
    let expect = cube(UvMode::Atlas).uvs;
    let got = &model.submeshes[0].uvs;
    assert!(got
        .iter()
        .any(|g| (g.x - expect[0][0]).abs() < 1e-5 && (g.y - expect[0][1]).abs() < 1e-5));
    // Without an MTL a default set is made.
    let m = load_obj_str(&cube_obj(UvMode::Atlas, false), None, &opts()).unwrap();
    assert_eq!(m.model.texture_sets[0].name, "default");
}

#[test]
fn texture_sets_list_in_reverse_material_order() {
    let mut m = load_gltf_slice(cube_gltf(UvMode::Atlas, 4).as_bytes(), &opts())
        .unwrap()
        .model;
    let extra = m.texture_sets[0].clone();
    m.texture_sets.push(extra);
    assert_eq!(m.texture_set_display_order(), vec![1, 0]);
}

#[test]
fn missing_uvs_are_fatal() {
    for r in [
        load_gltf_slice(cube_gltf(UvMode::None, 4).as_bytes(), &opts()).unwrap_err(),
        load_obj_str(&cube_obj(UvMode::None, false), None, &opts()).unwrap_err(),
    ] {
        match r {
            ImportError::Rejected(rep) => {
                assert!(rep.is_fatal());
                assert!(
                    matches!(rep.fatal[0], FatalIssue::MissingUvs { .. }),
                    "{rep:?}"
                );
                assert!(!rep.messages().is_empty());
            }
            e => panic!("unexpected {e}"),
        }
    }
}

#[test]
fn point_cloud_and_empty_files_are_fatal() {
    let rep = |r: Result<_, ImportError>| match r {
        Err(ImportError::Rejected(rep)) => rep,
        Ok(_) => panic!("should have been rejected"),
        Err(e) => panic!("{e}"),
    };
    let r = rep(load_gltf_slice(
        cube_gltf(UvMode::Atlas, 0).as_bytes(),
        &opts(),
    ));
    assert_eq!(r.fatal, vec![FatalIssue::PointCloud]);
    let r = rep(load_gltf_slice(empty_gltf().as_bytes(), &opts()));
    assert_eq!(r.fatal, vec![FatalIssue::NoMesh]);
    let r = rep(load_obj_str("v 0 0 0\nv 1 0 0\nv 0 1 0\n", None, &opts()));
    assert_eq!(r.fatal, vec![FatalIssue::PointCloud]);
    let r = rep(load_obj_str("# nothing\n", None, &opts()));
    assert_eq!(r.fatal, vec![FatalIssue::NoMesh]);
}

#[test]
fn overlapping_uvs_warn() {
    let clean = load_gltf_slice(cube_gltf(UvMode::Atlas, 4).as_bytes(), &opts()).unwrap();
    assert_eq!(overlapping_texels(&clean.model, 0, 512), 0);
    let m = load_gltf_slice(cube_gltf(UvMode::Overlap, 4).as_bytes(), &opts()).unwrap();
    assert!(
        m.report.warnings.iter().any(
            |w| matches!(w, Warning::OverlappingUvs { texture_set: 0, texels } if *texels > 1000)
        ),
        "{:?}",
        m.report
    );
}

#[test]
fn uvs_outside_unit_square_warn_as_repeating() {
    let m = load_gltf_slice(cube_gltf(UvMode::Repeating, 4).as_bytes(), &opts()).unwrap();
    assert!(
        m.report
            .warnings
            .iter()
            .any(|w| matches!(w, Warning::TextureRepeating { .. })),
        "{:?}",
        m.report
    );
}

#[test]
fn glb_export_reimports_with_same_texture_size() {
    let mut model = load_gltf_slice(
        cube_gltf(UvMode::Atlas, 4).as_bytes(),
        &ImportOptions {
            default_texture_size: 96,
        },
    )
    .unwrap()
    .model;
    // Distinguishable painted content.
    let ts = &mut model.texture_sets[0];
    let c = ts.map_mut(MapKind::Colour);
    c.set_pixel(20, 10, [10, 200, 30, 255]);
    let r = ts.map_mut(MapKind::Roughness);
    r.set_pixel(20, 10, [77, 77, 77, 255]);

    let glb = write_glb_bytes(&model).unwrap();
    assert_eq!(&glb[..4], b"glTF");
    let back = load_gltf_slice(
        &glb,
        &ImportOptions {
            default_texture_size: 8,
        },
    )
    .unwrap();
    let b = &back.model;
    assert_eq!(
        (b.texture_sets[0].width, b.texture_sets[0].height),
        (96, 96)
    );
    assert_eq!(b.triangle_count(), 12);
    assert_eq!(b.texture_sets[0].name, "red");
    assert_eq!(
        b.texture_sets[0].map(MapKind::Colour).pixel(20, 10),
        [10, 200, 30, 255]
    );
    assert_eq!(
        b.texture_sets[0].map(MapKind::Roughness).pixel(20, 10)[0],
        77
    );
    assert_eq!(b.texture_sets[0].map(MapKind::Metallic).pixel(0, 0)[0], 64);
    assert!(back.report.warnings.is_empty());
}

#[test]
fn per_map_pngs_are_written() {
    let model = load_gltf_slice(cube_gltf(UvMode::Atlas, 4).as_bytes(), &opts())
        .unwrap()
        .model;
    let dir = std::env::temp_dir().join(format!("eskiz3d_pngs_{}", std::process::id()));
    let paths = export_map_pngs(&model, &dir, Some(300)).unwrap();
    assert_eq!(paths.len(), 5);
    assert!(paths[0]
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .ends_with("red_color.png"));
    let img = image::open(&paths[0]).unwrap().to_rgba8();
    assert_eq!((img.width(), img.height()), (64, 64));
    assert_eq!(img.get_pixel(1, 1).0, [255, 0, 0, 255]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn gltf_node_cycle_is_rejected() {
    let json = br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"children":[0]}]}"#;
    let r = eskiz_3d::load_gltf_slice(json, &eskiz_3d::ImportOptions::default());
    assert!(r.is_err());
}

#[test]
fn gltf_shared_child_is_rejected() {
    let json = br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"children":[1,2]},{"children":[3]},{"children":[3]},{}]}"#;
    let r = eskiz_3d::load_gltf_slice(json, &eskiz_3d::ImportOptions::default());
    assert!(r.is_err());
}

#[test]
fn uv_mask_wraps_tiled_uvs() {
    use glam::Vec2;
    // A quad whose UVs sit entirely in tile (2, 1): the mask must still cover the 0..1 tile.
    let m = quad_model(Vec2::new(2.0, 1.0), Vec2::new(3.0, 2.0), 16);
    let mask = eskiz_3d::check::build_uv_mask(&m, 0);
    assert!(mask.label.iter().filter(|&&l| l != 0).count() > 16 * 16 / 2);
    // Tiled 0..4 covers everything.
    let m = quad_model(Vec2::ZERO, Vec2::splat(4.0), 16);
    let mask = eskiz_3d::check::build_uv_mask(&m, 0);
    assert!(mask.label.iter().all(|&l| l != 0));
}

#[test]
fn uv_mask_ignores_non_finite_uvs() {
    use glam::Vec2;
    let m = quad_model(Vec2::new(f32::NAN, 0.0), Vec2::new(1.0, f32::INFINITY), 8);
    let mask = eskiz_3d::check::build_uv_mask(&m, 0);
    assert_eq!(mask.label.len(), 64);
}

#[test]
fn huge_default_texture_size_is_capped() {
    let o = ImportOptions {
        default_texture_size: 1 << 30,
    };
    let r = load_obj_str(&cube_obj(UvMode::Atlas, false), None, &o).unwrap();
    assert!(r.model.texture_sets[0].width <= 4096);
}
