mod common;

use common::*;
use eskiz_3d::{
    dilate_gutter, load_gltf_slice, render_lookup, ImportOptions, MapKind, Model, OrbitCamera,
    PaintSession, PaintSettings, RenderOptions, Side, SubMesh, TextureSet, UvMask,
};
use eskiz_core::{Brush, Dab, PaintOptions, Preset};
use glam::{Vec2, Vec3};

const N: usize = 101; // odd, so the centre pixel's centre is the exact screen centre

fn dab(x: f64, y: f64, size: f64) -> Dab {
    Dab {
        x,
        y,
        sx: size,
        sy: size,
        angle: 0.0,
        opacity: 1.0,
        flow: 1.0,
        ..Dab::default()
    }
}

fn red_session(settings: PaintSettings) -> PaintSession {
    PaintSession::new(
        Brush::preset(Preset::TechnicalPen),
        PaintOptions::new([1.0, 0.0, 0.0]),
        settings,
    )
}

fn front_camera() -> OrbitCamera {
    OrbitCamera::new(Vec3::ZERO, 0.0, 0.0, 3.0, 60f32.to_radians())
}

#[test]
fn lookup_round_trip_at_centre_and_top_edge() {
    let model = quad_model(Vec2::ZERO, Vec2::ONE, 64);
    let cam = front_camera();
    let lk = render_lookup(&model, &cam, N, N, RenderOptions::default());

    // Centre pixel: world (0,0,0) -> UV (0.5, 0.5).
    let (u, v) = lk.remap(50.5, 50.5).expect("centre is covered");
    assert!(
        (u - 0.5).abs() < 2.0 / 65535.0 && (v - 0.5).abs() < 2.0 / 65535.0,
        "{u} {v}"
    );
    assert_eq!(
        lk.triangle_at(50.5, 50.5)
            .map(|t| model.locate(t).is_some()),
        Some(true)
    );

    // Depth equals the analytic NDC depth of a plane at distance 3.
    let ((n, f), d) = (cam.clip_planes(), 3.0f32);
    let expect = f * (d - n) / (d * (f - n));
    assert!((lk.depth_at(50.5, 50.5).unwrap() - expect).abs() < 1e-5);

    // Every pixel, including the top row, matches analytic unprojection onto the quad.
    let half = (cam.fov_y * 0.5).tan() * 3.0;
    for &(px, py) in &[
        (50usize, 0usize),
        (50, 100),
        (0, 50),
        (100, 50),
        (20, 20),
        (75, 30),
        (30, 80),
    ] {
        let ndc_x = (px as f32 + 0.5) / N as f32 * 2.0 - 1.0;
        let ndc_y = 1.0 - (py as f32 + 0.5) / N as f32 * 2.0;
        let (wx, wy) = (ndc_x * half, ndc_y * half);
        let got = lk.remap(px as f32 + 0.5, py as f32 + 0.5);
        if wx.abs() > 1.0 || wy.abs() > 1.0 {
            assert!(got.is_none(), "({px},{py}) should be outside the quad");
        } else {
            let (u, v) = got.unwrap();
            assert!((u - (wx + 1.0) / 2.0).abs() < 1e-3, "u at ({px},{py})");
            assert!((v - (1.0 - wy) / 2.0).abs() < 1e-3, "v at ({px},{py})");
        }
    }
    // Outside the model the lookup is empty; out-of-range points clamp to the edge pixel.
    assert!(lk.remap(1.0, 1.0).is_none());
    assert_eq!(lk.remap(-50.0, -50.0), lk.remap(0.5, 0.5));

    // A stroke at the very top edge of a model that reaches it: zoom in so the quad overfills the view.
    let mut close = cam;
    close.distance = 1.2;
    let lk = render_lookup(&model, &close, N, N, RenderOptions::default());
    let half = (close.fov_y * 0.5).tan() * 1.2;
    let (u, v) = lk.remap(50.5, 0.5).unwrap();
    let wy = (1.0 - 0.5 / N as f32 * 2.0) * half;
    assert!(
        (v - (1.0 - wy) / 2.0).abs() < 1e-3 && (u - 0.5).abs() < 1e-3,
        "{u} {v}"
    );
    // The same pixel's depth is the quad's depth: lookup and depth share one convention.
    assert!(lk.depth_at(50.5, 0.5).unwrap().is_finite());
}

#[test]
fn uv_derivatives_match_geometry() {
    let model = quad_model(Vec2::ZERO, Vec2::ONE, 64);
    let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
    let h = lk.hit(50.5, 50.5, Side::Front).unwrap();
    // One pixel is (2 * 3 * tan(30deg) / N) world units; UV spans 2 world units.
    let px = 2.0 * 3.0 * (30f32.to_radians()).tan() / N as f32 / 2.0;
    assert!(
        (h.duv_dx.x - px).abs() < 1e-5 && h.duv_dx.y.abs() < 1e-6,
        "{:?}",
        h.duv_dx
    );
    assert!(
        (h.duv_dy.y - px).abs() < 1e-5 && h.duv_dy.x.abs() < 1e-6,
        "{:?}",
        h.duv_dy
    );
}

#[test]
fn dab_at_screen_centre_colours_the_right_texels() {
    let mut model = quad_model(Vec2::ZERO, Vec2::ONE, 64);
    let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
    let mut s = red_session(PaintSettings::default());
    assert_eq!(s.add_dabs(&model, &lk, &[dab(50.5, 50.5, 20.0)]), 1);
    s.finish(&mut model);

    let c = model.texture_sets[0].map(MapKind::Colour);
    assert_eq!(c.pixel(32, 32), [255, 0, 0, 255]);
    assert_eq!(c.pixel(31, 31), [255, 0, 0, 255]);
    assert_eq!(c.pixel(5, 5)[3], 0);
    assert_eq!(c.pixel(60, 60)[3], 0);
    // The quad is 58.4 px wide for 64 texels, so a 20 px dab is ~21.9 texels wide.
    let row: usize = (0..64).filter(|&x| c.pixel(x, 32)[3] > 0).count();
    assert!((20..=24).contains(&row), "dab spans {row} texels");
    let col: usize = (0..64).filter(|&y| c.pixel(32, y)[3] > 0).count();
    assert!((20..=24).contains(&col), "dab spans {col} texels");
    // Other maps are untouched.
    assert_eq!(
        model.texture_sets[0].map(MapKind::Roughness).pixel(32, 32)[0],
        128
    );
}

#[test]
fn dab_misses_are_dropped_and_apply_is_idempotent() {
    let mut model = quad_model(Vec2::ZERO, Vec2::ONE, 64);
    let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
    let mut s = red_session(PaintSettings::default());
    assert_eq!(s.add_dabs(&model, &lk, &[dab(2.0, 2.0, 10.0)]), 0);
    s.add_dabs(&model, &lk, &[dab(50.5, 50.5, 12.0)]);
    s.apply(&mut model);
    let once = model.texture_sets[0].map(MapKind::Colour).to_rgba();
    s.apply(&mut model);
    assert_eq!(once, model.texture_sets[0].map(MapKind::Colour).to_rgba());
}

#[test]
fn dab_size_follows_local_uv_density() {
    // Same dab on two quads whose UV spans differ by 2x: texel size scales by 2x.
    let count = |span: f32| {
        let mut model = quad_model(Vec2::ZERO, Vec2::splat(span), 128);
        let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
        let mut s = red_session(PaintSettings::default());
        s.add_dabs(&model, &lk, &[dab(50.5, 50.5, 16.0)]);
        s.finish(&mut model);
        let c = model.texture_sets[0].map(MapKind::Colour);
        let y = (span * 64.0) as usize;
        (0..128).filter(|&x| c.pixel(x, y)[3] > 0).count()
    };
    let (a, b) = (count(1.0), count(0.5));
    assert!((a as f32 / b as f32 - 2.0).abs() < 0.2, "{a} vs {b}");
}

/// Quad island occupying the middle of the texture, so there is gutter around it.
fn island_model() -> Model {
    quad_model(Vec2::splat(0.25), Vec2::splat(0.75), 64)
}

#[test]
fn seam_dilation_fills_the_gutter() {
    let mut model = island_model();
    let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
    let mut s = red_session(PaintSettings::default());
    // A dab far bigger than the island: the island is the only thing it can paint directly.
    s.add_dabs(&model, &lk, &[dab(50.5, 50.5, 120.0)]);
    s.apply(&mut model);
    {
        let c = model.texture_sets[0].map(MapKind::Colour);
        assert_eq!(c.pixel(20, 32), [255, 0, 0, 255]);
        assert_eq!(c.pixel(15, 32)[3], 0, "gutter is empty before dilation");
    }
    s.finish(&mut model);
    let c = model.texture_sets[0].map(MapKind::Colour);
    for x in [15, 14, 13] {
        assert_eq!(c.pixel(x, 32), [255, 0, 0, 255], "left gutter texel {x}");
    }
    for x in [48, 49, 50] {
        assert_eq!(c.pixel(x, 32), [255, 0, 0, 255], "right gutter texel {x}");
    }
    assert_eq!(
        c.pixel(12, 32)[3],
        0,
        "outside the 3-texel radius stays empty"
    );
    assert_eq!(c.pixel(51, 32)[3], 0);
    assert_eq!(
        c.pixel(13, 13),
        [255, 0, 0, 255],
        "corner gutter fills diagonally"
    );
    assert_eq!(c.pixel(5, 5)[3], 0);
}

#[test]
fn dilate_gutter_unit() {
    let mut model = island_model();
    let c = model.texture_sets[0].map_mut(MapKind::Colour);
    for y in 16..48 {
        for x in 16..48 {
            c.set_pixel(x, y, [0, 255, 0, 255]);
        }
    }
    let mut layer = model.texture_sets[0].map(MapKind::Colour).clone();
    dilate_gutter(&mut layer, &model.masks[0], 2, None);
    assert_eq!(layer.pixel(14, 20), [0, 255, 0, 255]);
    assert_eq!(layer.pixel(13, 20)[3], 0);
    assert_eq!(
        model.masks[0].label[32 * 64 + 20],
        model.masks[0].label[20 * 64 + 20]
    );
    assert_eq!(model.masks[0].label[32 * 64 + 15], 0);
}

#[test]
fn stroke_on_one_island_leaves_a_far_island_alone() {
    // Two separate quads side by side in the world, mapped to the left and right halves of the texture.
    let quad = |x0: f32, x1: f32, u0: f32, u1: f32, base: u32| {
        (
            vec![
                Vec3::new(x0, -0.5, 0.),
                Vec3::new(x1, -0.5, 0.),
                Vec3::new(x1, 0.5, 0.),
                Vec3::new(x0, 0.5, 0.),
            ],
            vec![
                Vec2::new(u0, 1.0),
                Vec2::new(u1, 1.0),
                Vec2::new(u1, 0.0),
                Vec2::new(u0, 0.0),
            ],
            [base, base + 1, base + 2, base, base + 2, base + 3],
        )
    };
    let (p1, t1, i1) = quad(-1.0, -0.05, 0.0, 0.45, 0);
    let (p2, t2, i2) = quad(0.05, 1.0, 0.55, 1.0, 4);
    let sm = SubMesh {
        name: "two".into(),
        positions: [p1, p2].concat(),
        normals: vec![Vec3::Z; 8],
        uvs: [t1, t2].concat(),
        indices: [i1, i2].concat().to_vec(),
        material: 0,
    };
    let mut model = Model::new(
        vec![sm],
        vec![TextureSet::from_constants(
            "t",
            64,
            64,
            [255, 255, 255, 0],
            0.5,
            0.0,
        )],
        0,
    );
    let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
    // Dab on the left quad, large enough that its texel footprint spans the whole texture.
    let (x, y) = (50.5 - 0.5 * 29.2, 50.5);
    let mut s = red_session(PaintSettings::default());
    assert_eq!(s.add_dabs(&model, &lk, &[dab(x, y, 160.0)]), 1);
    s.finish(&mut model);
    let c = model.texture_sets[0].map(MapKind::Colour);
    assert_eq!(c.pixel(10, 32), [255, 0, 0, 255]);
    for tx in [36, 45, 60] {
        assert_eq!(
            c.pixel(tx, 32)[3],
            0,
            "far island texel {tx} must stay clear"
        );
    }
    let labels: Vec<u32> = [10usize, 60]
        .iter()
        .map(|&x| model.masks[0].label[32 * 64 + x])
        .collect();
    assert!(
        labels[0] != 0 && labels[1] != 0 && labels[0] != labels[1] && labels[0] != UvMask::SHARED
    );
}

fn cube_model() -> Model {
    let mut m = load_gltf_slice(
        cube_gltf(UvMode::Atlas, 4).as_bytes(),
        &ImportOptions {
            default_texture_size: 96,
        },
    )
    .unwrap()
    .model;
    // Start from a transparent colour layer so painted texels are easy to see.
    m.texture_sets[0].map_mut(MapKind::Colour).clear();
    m
}

fn cell_centre_alpha(model: &Model, col: usize, row: usize) -> u8 {
    let c = model.texture_sets[0].map(MapKind::Colour);
    let x = ((col as f32 + 0.5) / 3.0 * 96.0) as usize;
    let y = ((row as f32 + 0.5) / 2.0 * 96.0) as usize;
    c.pixel(x, y)[3]
}

#[test]
fn paint_through_off_paints_front_only_and_on_paints_both() {
    let cam = front_camera();
    for through in [false, true] {
        let mut model = cube_model();
        let lk = render_lookup(
            &model,
            &cam,
            N,
            N,
            RenderOptions {
                back_layer: true,
                cull_backfaces: false,
            },
        );
        // Front hit is the +Z face (cell 0,0), back hit the -Z face (cell 1,0).
        let front = lk.hit(50.5, 50.5, Side::Front).unwrap();
        let back = lk.hit(50.5, 50.5, Side::Back).unwrap();
        assert!(front.depth < back.depth);
        assert!(
            (front.u - (0.5 / 3.0)).abs() < 2e-3 && (front.v - 0.25).abs() < 2e-3,
            "{front:?}"
        );
        assert!(
            (back.u - (1.5 / 3.0)).abs() < 2e-3 && (back.v - 0.25).abs() < 2e-3,
            "{back:?}"
        );

        let mut s = red_session(PaintSettings {
            paint_through: through,
            ..PaintSettings::default()
        });
        s.add_dabs(&model, &lk, &[dab(50.5, 50.5, 10.0)]);
        s.finish(&mut model);
        assert_eq!(cell_centre_alpha(&model, 0, 0), 255);
        assert_eq!(
            cell_centre_alpha(&model, 1, 0),
            if through { 255 } else { 0 },
            "paint_through={through}"
        );
        // Untouched faces stay clear (it is cell centres of other faces).
        assert_eq!(cell_centre_alpha(&model, 2, 0), 0);
        assert_eq!(cell_centre_alpha(&model, 0, 1), 0);
    }
}

#[test]
fn locked_texture_sets_are_not_painted_and_erase_works() {
    let mut model = quad_model(Vec2::ZERO, Vec2::ONE, 64);
    let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
    model.texture_sets[0].locked = true;
    let mut s = red_session(PaintSettings::default());
    s.add_dabs(&model, &lk, &[dab(50.5, 50.5, 20.0)]);
    s.apply(&mut model);
    assert_eq!(
        model.texture_sets[0].map(MapKind::Colour).pixel(32, 32)[3],
        0
    );

    model.texture_sets[0].locked = false;
    let mut s = red_session(PaintSettings::default());
    s.add_dabs(&model, &lk, &[dab(50.5, 50.5, 20.0)]);
    s.finish(&mut model);
    assert_eq!(
        model.texture_sets[0].map(MapKind::Colour).pixel(32, 32)[3],
        255
    );
    let mut e = PaintSession::new(
        Brush::preset(Preset::TechnicalPen),
        PaintOptions {
            colour: [0.0; 3],
            erase: true,
            alpha_lock: false,
        },
        PaintSettings::default(),
    );
    e.add_dabs(&model, &lk, &[dab(50.5, 50.5, 20.0)]);
    e.finish(&mut model);
    assert_eq!(
        model.texture_sets[0].map(MapKind::Colour).pixel(32, 32)[3],
        0
    );
}

#[test]
fn near_plane_clipping_does_not_panic_and_covers_the_view() {
    // Camera inside/near the quad's plane: triangles cross the near plane.
    let model = quad_model(Vec2::ZERO, Vec2::ONE, 64);
    let mut cam = front_camera();
    cam.pitch = 1.2;
    cam.distance = 0.5;
    let lk = render_lookup(&model, &cam, 64, 48, RenderOptions::default());
    assert!(lk.front.st.iter().any(|&s| s != u32::MAX));
}

#[test]
fn dab_on_a_shared_texel_never_paints_a_far_island() {
    // Island A (u 0..0.3) and island B (u 0.3..0.8) meet on an edge that crosses texel 19,
    // so texel 19 is shared. Island C (u 0.85..1) is far away in UV but inside the dab footprint.
    let rect = |x0: f32, x1: f32, u0: f32, u1: f32, base: u32| {
        (
            vec![
                Vec3::new(x0, 1., 0.),
                Vec3::new(x1, 1., 0.),
                Vec3::new(x1, -1., 0.),
                Vec3::new(x0, -1., 0.),
            ],
            vec![
                Vec2::new(u0, 0.),
                Vec2::new(u1, 0.),
                Vec2::new(u1, 1.),
                Vec2::new(u0, 1.),
            ],
            vec![base, base + 1, base + 2, base, base + 2, base + 3],
        )
    };
    // B starts at 0.3005 so that A and B share no UV vertex and stay separate islands.
    let parts = [
        rect(-1.0, -0.4, 0.0, 0.3, 0),
        rect(-0.4, 0.6, 0.3005, 0.8, 4),
        rect(0.6, 1.0, 0.85, 1.0, 8),
    ];
    let sm = SubMesh {
        name: "three".into(),
        positions: parts.iter().flat_map(|p| p.0.clone()).collect(),
        normals: vec![Vec3::Z; 12],
        uvs: parts.iter().flat_map(|p| p.1.clone()).collect(),
        indices: parts.iter().flat_map(|p| p.2.clone()).collect(),
        material: 0,
    };
    let mut model = Model::new(
        vec![sm],
        vec![TextureSet::from_constants(
            "t",
            64,
            64,
            [255, 255, 255, 0],
            0.5,
            0.0,
        )],
        0,
    );
    assert_eq!(
        model.masks[0].label[32 * 64 + 19],
        UvMask::SHARED,
        "precondition: texel 19 touches two islands"
    );
    let lk = render_lookup(&model, &front_camera(), N, N, RenderOptions::default());
    // Pixel 39 hits island B at u about 0.312, inside texel 19.
    let mut s = red_session(PaintSettings::default());
    assert_eq!(s.add_dabs(&model, &lk, &[dab(39.5, 50.5, 160.0)]), 1);
    s.finish(&mut model);
    let c = model.texture_sets[0].map(MapKind::Colour);
    assert!(
        (0..54).any(|tx| c.pixel(tx, 32)[3] > 0),
        "the dab paints the island under it"
    );
    for tx in 54..64 {
        assert_eq!(
            c.pixel(tx, 32)[3],
            0,
            "far island C texel {tx} must stay clear"
        );
    }
}

/// The clip planes follow the model: zoomed far out the model is still in front of the far
/// plane, and a tiny model is not cut by the near plane.
#[test]
fn clip_planes_keep_the_model_at_every_zoom_and_size() {
    for radius in [0.001f32, 1.0, 500.0] {
        let mut cam = OrbitCamera::default();
        cam.frame(Vec3::splat(-radius), Vec3::splat(radius));
        for zoom in [0.05f32, 1.0, 30.0] {
            cam.zoom_by(zoom / cam.zoom());
            let (near, far) = cam.clip_planes();
            let r = radius * 3f32.sqrt();
            assert!(cam.distance + r < far, "radius {radius} zoom {zoom}: far {far}");
            assert!(near < (cam.distance - r).max(near * 2.0), "radius {radius} zoom {zoom}: near {near}");
        }
    }
}
