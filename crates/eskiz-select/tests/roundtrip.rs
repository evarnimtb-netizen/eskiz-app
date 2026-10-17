//! Round trips, identities and supporting behaviour: projective map and inverse, warp identity
//! mesh, feather zero, invert twice, and the smaller rules around snapping, fit, outlines,
//! freehand capture and resampling.

use eskiz_select::feather::feather;
use eskiz_select::geom::{self, Mat3, Point, Rect};
use eskiz_select::image::Image;
use eskiz_select::mask::{CombineMode, Mask};
use eskiz_select::outline::{outline, OUTLINE_LEVEL};
use eskiz_select::raster::{self, FreehandStroke};
use eskiz_select::resample::{resample, Interp};
use eskiz_select::selection::{thumbnail_size, SavedSelections, Selection};
use eskiz_select::session::TransformSession;
use eskiz_select::transform::{self, Handle, SnapMode};
use eskiz_select::wand::{flood_select, threshold_after_drag};
use eskiz_select::warp::{DepthOp, Warp};
use eskiz_select::{canvas, SelectError};

fn near_tol(a: Point, b: Point, tol: f32) -> bool {
    (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol
}

#[test]
fn projective_map_and_inverse_round_trip() {
    let rect = Rect::new(0.0, 0.0, 120.0, 80.0);
    let quad = [
        Point::new(12.0, 8.0),
        Point::new(140.0, 30.0),
        Point::new(118.0, 110.0),
        Point::new(-5.0, 70.0),
    ];
    let m = geom::rect_to_quad(rect, &quad).unwrap();
    let inv = m.inverse().unwrap();
    for (corner, target) in rect.corners().iter().zip(quad) {
        assert!(near_tol(m.apply(*corner).unwrap(), target, 1e-3));
        assert!(near_tol(inv.apply(target).unwrap(), *corner, 1e-3));
    }
    for p in [
        Point::new(10.5, 20.25),
        Point::new(60.0, 40.0),
        Point::new(100.75, 5.5),
    ] {
        let back = inv.apply(m.apply(p).unwrap()).unwrap();
        assert!(near_tol(back, p, 1e-3));
    }
}

#[test]
fn mat3_inverse_composes_to_identity() {
    let m = Mat3::translate(3.0, -2.0) * Mat3::rotate_deg(33.0) * Mat3::scale(1.5, 0.75);
    let id = m * m.inverse().unwrap();
    for (i, row) in id.m.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            let expected = if i == j { 1.0 } else { 0.0 };
            assert!((v - expected).abs() <= 1e-4, "entry {i},{j} = {v}");
        }
    }
}

#[test]
fn warp_identity_mesh_is_identity() {
    let bounds = Rect::new(10.0, 20.0, 110.0, 70.0);
    let warp = Warp::identity(bounds);
    for (u, v) in [(0.0, 0.0), (0.25, 0.5), (1.0, 1.0), (0.3, 0.9)] {
        let p = warp.evaluate(u, v);
        let expected = Point::new(10.0 + 100.0 * u, 20.0 + 50.0 * v);
        assert!(near_tol(p, expected, 1e-3), "({u}, {v}) gave {p:?}");
    }
}

#[test]
fn warp_pick_and_drag_control() {
    let mut warp = Warp::identity(Rect::new(0.0, 0.0, 90.0, 90.0));
    let target = warp.ctrl[1][2];
    assert_eq!(warp.pick(target, 24.0), Some((1, 2)));
    assert_eq!(warp.pick(Point::new(500.0, 500.0), 24.0), None);
    warp.drag_control(1, 2, Point::new(5.0, -3.0));
    assert_eq!(warp.ctrl[1][2], Point::new(target.x + 5.0, target.y - 3.0));
    warp.set_depth(0, 0, DepthOp::Back);
    warp.set_depth(0, 0, DepthOp::Flip);
    assert_eq!(warp.depth[0][0], 1.0);
}

#[test]
fn feather_zero_is_identity() {
    let m = raster::ellipse(40, 30, Rect::new(3.0, 4.0, 33.0, 26.0));
    assert_eq!(feather(&m, 0.0), m);
    let empty = Mask::empty(10, 10);
    assert_eq!(feather(&empty, 8.0), empty);
}

#[test]
fn invert_twice_is_identity_on_masks_and_selection() {
    let m = raster::polygon(
        24,
        24,
        &[
            Point::new(2.0, 3.0),
            Point::new(20.0, 5.0),
            Point::new(12.0, 21.0),
        ],
    );
    let back = m.invert().invert();
    for (a, b) in m.data.iter().zip(&back.data) {
        assert!((a - b).abs() <= 1e-6);
    }
    // No selection inverts to the whole canvas; inverting that gives no selection data.
    let all = Selection::none().invert(4, 4);
    assert_eq!(all.value_at(3, 3), 1.0);
    assert!(all.invert(4, 4).invert(4, 4).mask().is_some());
}

#[test]
fn replace_clears_then_draws() {
    let m = raster::rectangle(8, 8, Rect::new(1.0, 1.0, 4.0, 4.0));
    let sel = Selection::active(Mask::full(8, 8))
        .combine(&m, CombineMode::Replace)
        .unwrap();
    assert_eq!(sel.mask(), Some(&m));
}

#[test]
fn polygon_matches_rectangle_for_integer_corners() {
    let rect = Rect::new(2.0, 3.0, 9.0, 11.0);
    let by_rect = raster::rectangle(16, 16, rect);
    let by_polygon = raster::polygon(16, 16, &rect.corners());
    for (a, b) in by_rect.data.iter().zip(&by_polygon.data) {
        assert!((a - b).abs() <= 1e-6);
    }
}

#[test]
fn ellipse_values_stay_in_unit_range() {
    let m = raster::ellipse(33, 17, Rect::new(1.5, 0.25, 30.0, 15.0));
    assert!(m.data.iter().all(|&v| (0.0..=1.0).contains(&v)));
    assert!(m.data.iter().any(|&v| v > 0.0 && v < 1.0));
}

#[test]
fn freehand_thins_samples_and_closes_shape() {
    let mut stroke = FreehandStroke::default();
    for i in 0..20 {
        stroke.push(Point::new(i as f32, 0.0));
    }
    assert_eq!(stroke.points().len(), 10);
    stroke.push(Point::new(10.0, 10.0));
    stroke.push(Point::new(0.0, 10.0));
    let mask = stroke.finish(24, 24);
    assert!(mask.data.iter().any(|&v| v > 0.9));
}

#[test]
fn smoothing_keeps_endpoints() {
    let pts = [
        Point::new(0.0, 0.0),
        Point::new(4.0, 0.0),
        Point::new(8.0, 4.0),
        Point::new(3.0, 9.0),
    ];
    let s = raster::smooth_path(&pts);
    assert_eq!(s[0], pts[0]);
    assert_eq!(s[3], pts[3]);
}

#[test]
fn wand_outside_seed_gives_empty_selection() {
    let img = Image::from_fn(5, 5, |_, _| WHITE_PX);
    assert!(flood_select(&img, (-1, 2), 0.5)
        .data
        .iter()
        .all(|&v| v == 0.0));
    assert!(flood_select(&img, (5, 2), 0.5)
        .data
        .iter()
        .all(|&v| v == 0.0));
}

const WHITE_PX: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

#[test]
fn wand_threshold_drag_maps_and_clamps() {
    assert!((threshold_after_drag(0.4, 10.0, 1.0) - 0.45).abs() <= 1e-6);
    assert_eq!(threshold_after_drag(0.99, 100.0, 1.0), 1.0);
    assert_eq!(threshold_after_drag(0.01, -100.0, 1.0), 0.0);
}

#[test]
fn outline_of_empty_mask_is_empty_and_full_canvas_closes() {
    assert!(outline(&Mask::empty(6, 6), OUTLINE_LEVEL).is_empty());
    let full = outline(&Mask::full(6, 4), OUTLINE_LEVEL);
    assert_eq!(full.len(), 1);
    assert!(full[0].closed);
}

#[test]
fn angle_snap_truncate_and_normalise() {
    assert_eq!(transform::snap_angle(17.0, SnapMode::Truncate), 15.0);
    assert_eq!(transform::snap_angle(22.0, SnapMode::Truncate), 22.0);
    assert_eq!(transform::snap_angle(-10.0, SnapMode::Nearest), -15.0);
    assert_eq!(transform::normalise_angle(270.0), -90.0);
    assert_eq!(transform::normalise_angle(180.0), 180.0);
}

#[test]
fn fit_scales_and_centres_content() {
    let content = Rect::new(0.0, 0.0, 200.0, 100.0);
    let m = transform::fit_gesture(content, 100.0, 100.0).unwrap();
    assert!(near_tol(
        m.apply(Point::new(0.0, 0.0)).unwrap(),
        Point::new(0.0, 25.0),
        1e-3
    ));
    assert!(near_tol(
        m.apply(Point::new(200.0, 100.0)).unwrap(),
        Point::new(100.0, 75.0),
        1e-3
    ));
    // Content that already fits keeps its scale.
    let small = Rect::new(0.0, 0.0, 20.0, 10.0);
    let m = transform::fit_gesture(small, 100.0, 100.0).unwrap();
    assert!(near_tol(
        m.apply(Point::new(0.0, 0.0)).unwrap(),
        Point::new(40.0, 45.0),
        1e-3
    ));
}

#[test]
fn flip_twice_is_identity_and_flip_mirrors_bounds() {
    let bounds = Rect::new(10.0, 10.0, 30.0, 20.0);
    let f = transform::flip_about(bounds, true);
    let twice = f * f;
    for (i, row) in twice.m.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            let expected = if i == j { 1.0 } else { 0.0 };
            assert!((v - expected).abs() <= 1e-5);
        }
    }
    assert!(near_tol(
        f.apply(Point::new(10.0, 10.0)).unwrap(),
        Point::new(30.0, 10.0),
        1e-4
    ));
}

#[test]
fn session_quad_edit_and_cut_footprint() {
    let src = Image::from_fn(10, 10, |x, _| {
        if x < 5 {
            [0.5, 0.5, 0.5, 1.0]
        } else {
            [0.0; 4]
        }
    });
    let footprint = Mask::full(10, 10);
    let mut session =
        TransformSession::new(src, footprint, Mat3::IDENTITY, Interp::Nearest).unwrap();
    let q = session.quad().unwrap();
    let moved = transform::free_drag(q, Handle::Corner(1), Point::new(0.0, 0.0));
    session.set_quad(&moved).unwrap();
    assert!(near_tol(session.quad().unwrap()[1], moved[1], 1e-3));
    let layer = Image::from_fn(10, 10, |_, _| [0.9, 0.1, 0.1, 1.0]);
    let out = session.commit(&layer).unwrap();
    assert_eq!(out.get(0, 0), [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(out.get(9, 9)[3], 0.0);
}

#[test]
fn session_rejects_empty_source_and_mismatched_footprint() {
    let empty = Image::transparent(4, 4);
    assert_eq!(
        TransformSession::new(empty, Mask::full(4, 4), Mat3::IDENTITY, Interp::Nearest),
        Err(SelectError::NoContent)
    );
    let src = Image::from_fn(4, 4, |_, _| WHITE_PX);
    assert_eq!(
        TransformSession::new(src, Mask::full(3, 3), Mat3::IDENTITY, Interp::Nearest),
        Err(SelectError::SizeMismatch)
    );
}

#[test]
fn session_marquee_moves_with_content() {
    let src = Image::from_fn(10, 10, |_, _| WHITE_PX);
    let mut session =
        TransformSession::new(src, Mask::full(10, 10), Mat3::IDENTITY, Interp::Nearest).unwrap();
    session.apply_gesture(Mat3::translate(5.0, 0.0));
    let selection = raster::rectangle(10, 10, Rect::new(2.0, 2.0, 4.0, 4.0));
    let moved = session.marquee(&selection);
    assert_eq!(moved.get(6, 2), 0.0);
    assert_eq!(moved.get(7, 2), 1.0);
    assert_eq!(moved.get(8, 2), 1.0);
    assert_eq!(moved.get(9, 2), 0.0);
}

#[test]
fn bicubic_preserves_constant_colour() {
    let img = Image::from_fn(16, 16, |_, _| [0.3, 0.6, 0.9, 1.0]);
    let m = Mat3::rotate_deg(30.0).about(Point::new(8.0, 8.0)) * Mat3::scale(1.3, 1.3);
    let out = resample(&img, &m, 24, 24, Interp::Bicubic);
    for px in &out.pixels {
        if px[3] > 0.0 {
            assert!((px[0] - 0.3).abs() <= 1e-5);
            assert!((px[1] - 0.6).abs() <= 1e-5);
            assert!((px[2] - 0.9).abs() <= 1e-5);
        }
        assert!(px.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}

#[test]
fn tile_count_budget_and_thumbnail() {
    assert_eq!(eskiz_select::warp::tile_counts(0.0, 0.0, 40_000.0).0, 3);
    assert_eq!(thumbnail_size(276, 138, 138), (138, 69));
    assert_eq!(thumbnail_size(100, 50, 138), (100, 50));
}

#[test]
fn crop_rect_truncates_and_clamps() {
    assert_eq!(
        canvas::crop_rect(32, 32, 4.9, 6.9, 10.2, 12.8).unwrap(),
        eskiz_select::IRect {
            x: 4,
            y: 6,
            w: 10,
            h: 12
        }
    );
    assert_eq!(
        canvas::crop_rect(32, 32, 30.0, 30.0, 10.0, 10.0).unwrap(),
        eskiz_select::IRect {
            x: 30,
            y: 30,
            w: 2,
            h: 2
        }
    );
}

#[test]
fn saved_selections_ids_are_unique_and_removal_works() {
    let m = raster::rectangle(4, 4, Rect::new(0.0, 0.0, 2.0, 2.0));
    let mut store = SavedSelections::default();
    let a = store.save("a", &m);
    let b = store.save("b", &m);
    assert_ne!(a, b);
    assert!(store.remove(a).is_some());
    assert_eq!(store.load(a), None);
    assert_eq!(store.list().len(), 1);
}

#[test]
fn mask_bytes_reject_malformed_input() {
    assert_eq!(Mask::from_bytes(&[0u8; 3]), Err(SelectError::BadData));
    let mut bytes = Mask::full(2, 2).to_bytes();
    bytes[0] = b'X';
    assert_eq!(Mask::from_bytes(&bytes), Err(SelectError::BadData));
    let mut truncated = Mask::full(2, 2).to_bytes();
    truncated.pop();
    assert_eq!(Mask::from_bytes(&truncated), Err(SelectError::BadData));
}

#[test]
fn mask_combine_rejects_size_mismatch() {
    let a = Mask::empty(3, 3);
    let b = Mask::empty(4, 3);
    assert_eq!(
        a.combine(&b, CombineMode::Add),
        Err(SelectError::SizeMismatch)
    );
}

#[test]
fn wand_selects_connected_region_only() {
    // Two red blocks separated by white; seeding one must not pick up the other.
    let img = Image::from_fn(9, 3, |x, _| {
        if !(3..=5).contains(&x) {
            [1.0, 0.0, 0.0, 1.0]
        } else {
            WHITE_PX
        }
    });
    let mask = flood_select(&img, (0, 0), 0.1);
    assert_eq!(mask.get(0, 0), 1.0);
    assert_eq!(mask.get(7, 0), 0.0);
}
