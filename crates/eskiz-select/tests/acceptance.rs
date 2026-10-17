//! Acceptance tests for the
//! selection (43 to 53), transform (54 to 62), clipboard (63 to 66) and canvas (67 to 71) items.
//! Test 72 (DPI metadata) is not part of this crate. Each test is named after its spec number.

use eskiz_select::canvas::{self, Anchor};
use eskiz_select::clipboard::{self, Clip};
use eskiz_select::feather::feather;
use eskiz_select::geom::{self, IRect, Mat3, Point, Rect};
use eskiz_select::mask::{CombineMode, Mask};
use eskiz_select::outline::{outline, OUTLINE_LEVEL};
use eskiz_select::raster;
use eskiz_select::resample::{resample, Interp};
use eskiz_select::selection::{SavedSelections, Selection};
use eskiz_select::session::TransformSession;
use eskiz_select::transform::{self, Handle, SnapMode};
use eskiz_select::warp::tile_counts;
use eskiz_select::{Image, SelectError};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const SAMPLE: [f32; 4] = [0.2, 0.4, 0.6, 1.0];

fn count_selected(m: &Mask) -> usize {
    m.data.iter().filter(|&&v| v > 0.5).count()
}

fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() <= 1e-3 && (a.y - b.y).abs() <= 1e-3
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1, y1)
}

#[test]
fn acceptance_43_wand_threshold_zero_selects_exact_square() {
    let img = Image::from_fn(20, 20, |x, y| {
        if (5..15).contains(&x) && (5..15).contains(&y) {
            RED
        } else {
            WHITE
        }
    });
    let mask = eskiz_select::wand::flood_select(&img, (10, 10), 0.0);
    for y in 0..20 {
        for x in 0..20 {
            let inside = (5..15).contains(&x) && (5..15).contains(&y);
            let expected = if inside { 1.0 } else { 0.0 };
            assert_eq!(mask.get(x, y), expected, "pixel {x},{y}");
        }
    }
}

#[test]
fn acceptance_44_wand_full_threshold_selects_whole_connected_area() {
    let img = Image::from_fn(12, 9, |x, _| if x == 3 { RED } else { WHITE });
    let mask = eskiz_select::wand::flood_select(&img, (0, 0), 1.0);
    assert!(mask.data.iter().all(|&v| v == 1.0));
}

#[test]
fn acceptance_45_add_is_union_and_subtract_removes_second() {
    let a = raster::rectangle(20, 20, rect(0.0, 0.0, 10.0, 10.0));
    let b = raster::rectangle(20, 20, rect(5.0, 5.0, 15.0, 15.0));
    let union = a.combine(&b, CombineMode::Add).unwrap();
    assert_eq!(count_selected(&union), 175);
    assert_eq!(union.get(12, 12), 1.0);
    assert_eq!(union.get(2, 2), 1.0);

    let diff = a.combine(&b, CombineMode::Subtract).unwrap();
    assert_eq!(count_selected(&diff), 75);
    assert_eq!(diff.get(7, 7), 0.0);
    assert_eq!(diff.get(2, 2), 1.0);
}

#[test]
fn acceptance_46_invert_twice_returns_original() {
    let m = raster::ellipse(40, 30, rect(3.0, 4.0, 33.0, 26.0));
    let back = m.invert().invert();
    for (a, b) in m.data.iter().zip(&back.data) {
        assert!((a - b).abs() <= 1e-6);
    }
}

#[test]
fn acceptance_47_feather_zero_is_hard_and_feather_ten_is_soft() {
    let m = raster::rectangle(40, 40, rect(10.0, 10.0, 30.0, 30.0));
    let hard = feather(&m, 0.0);
    assert!(hard.data.iter().all(|&v| v == 0.0 || v == 1.0));

    let soft = feather(&m, 10.0);
    assert!(soft.data.iter().all(|&v| (0.0..=1.0).contains(&v)));
    assert!(soft.data.iter().any(|&v| v > 0.05 && v < 0.95));
}

#[test]
fn acceptance_48_save_then_load_is_exact() {
    let m = raster::ellipse(30, 30, rect(2.0, 3.0, 25.0, 20.0));
    let mut store = SavedSelections::default();
    let id = store.save("shape", &m);
    assert_eq!(store.load(id).as_ref(), Some(&m));
    assert_eq!(Mask::from_bytes(&m.to_bytes()).unwrap(), m);
}

#[test]
fn acceptance_49_paint_clip_scales_by_selection_value() {
    let mut m = Mask::empty(4, 1);
    m.set(0, 0, 1.0);
    m.set(1, 0, 0.5);
    let sel = Selection::active(m);
    assert_eq!(sel.clip_coverage(0, 0, 1.0), 1.0);
    assert_eq!(sel.clip_coverage(1, 0, 1.0), 0.5);
    assert_eq!(sel.clip_coverage(2, 0, 1.0), 0.0);
    assert_eq!(Selection::none().clip_coverage(2, 0, 0.7), 0.7);
}

#[test]
fn acceptance_50_rectangle_covers_half_open_pixel_range() {
    let m = raster::rectangle(64, 64, rect(10.0, 10.0, 50.0, 30.0));
    assert_eq!(m.get(10, 10), 1.0);
    assert_eq!(m.get(49, 29), 1.0);
    assert_eq!(m.get(9, 10), 0.0);
    assert_eq!(m.get(50, 10), 0.0);
    assert_eq!(m.get(10, 30), 0.0);
    assert_eq!(m.get(10, 9), 0.0);
    assert_eq!(count_selected(&m), 40 * 20);
}

#[test]
fn acceptance_51_ellipse_centre_selected_corner_not() {
    let m = raster::ellipse(100, 50, rect(0.0, 0.0, 100.0, 50.0));
    assert_eq!(m.get(50, 25), 1.0);
    assert_eq!(m.get(0, 0), 0.0);
}

#[test]
fn acceptance_52_smoothing_weight_is_quarter_on_self() {
    let pts = [
        Point::new(0.0, 0.0),
        Point::new(1.0, 0.0),
        Point::new(0.0, 0.0),
    ];
    let s = raster::smooth_path(&pts);
    assert!((s[1].x - 0.25).abs() <= 1e-6);
    assert_eq!(s[0], pts[0]);
    assert_eq!(s[2], pts[2]);
}

#[test]
fn acceptance_53_marching_ants_are_display_only() {
    let m = raster::rectangle(16, 16, rect(4.0, 4.0, 12.0, 12.0));
    let before = m.clone();
    let lines = outline(&m, OUTLINE_LEVEL);
    assert_eq!(m, before);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].closed);
    let min_x = lines[0]
        .points
        .iter()
        .map(|p| p.x)
        .fold(f32::INFINITY, f32::min);
    let max_x = lines[0]
        .points
        .iter()
        .map(|p| p.x)
        .fold(f32::NEG_INFINITY, f32::max);
    assert!((min_x - 4.0).abs() <= 1e-4);
    assert!((max_x - 12.0).abs() <= 1e-4);
}

#[test]
fn acceptance_54_uniform_scale_keeps_aspect_ratio() {
    let q = rect(0.0, 0.0, 100.0, 50.0).corners();
    let q2 = transform::uniform_drag(q, Handle::Corner(2), Point::new(10.0, 0.0));
    let width = q2[1].x - q2[0].x;
    let height = q2[2].y - q2[1].y;
    assert!((width - 2.0 * height).abs() <= 0.1);
}

#[test]
fn acceptance_55_rotate_90_then_back_nearest_is_exact() {
    let img = Image::from_fn(8, 8, |x, y| {
        [
            x as f32 / 8.0,
            y as f32 / 8.0,
            ((x * y) % 7) as f32 / 7.0,
            1.0,
        ]
    });
    let centre = Point::new(4.0, 4.0);
    let there = resample(
        &img,
        &Mat3::rotate_deg(90.0).about(centre),
        8,
        8,
        Interp::Nearest,
    );
    let back = resample(
        &there,
        &Mat3::rotate_deg(-90.0).about(centre),
        8,
        8,
        Interp::Nearest,
    );
    assert_eq!(back, img);
}

#[test]
fn acceptance_56_angle_snap_nearest_rule() {
    assert_eq!(transform::snap_angle(27.0, SnapMode::Nearest), 30.0);
    assert_eq!(transform::snap_angle(22.0, SnapMode::Nearest), 22.0);
}

#[test]
fn acceptance_57_numeric_input_scale_and_rotation() {
    assert_eq!(transform::numeric_scale(100.0, 200.0), Some(2.0));
    let (sx, sy) = transform::numeric_scale_pair((100.0, 50.0), (200.0, 50.0), false).unwrap();
    assert_eq!((sx, sy), (2.0, 1.0));
    assert_eq!(transform::numeric_rotation(0.0, 90.0), 90.0);
}

#[test]
fn acceptance_58_quad_corners_and_identity_square() {
    let p = [
        Point::new(10.0, 5.0),
        Point::new(90.0, 20.0),
        Point::new(80.0, 70.0),
        Point::new(0.0, 60.0),
    ];
    let m = geom::unit_square_to_quad(&p).unwrap();
    let at = |u: f32, v: f32| m.apply(Point::new(u, v)).unwrap();
    assert!(near(at(0.0, 0.0), p[0]));
    assert!(near(at(1.0, 0.0), p[1]));
    assert!(near(at(1.0, 1.0), p[2]));
    assert!(near(at(0.0, 1.0), p[3]));

    let sq = rect(0.0, 0.0, 10.0, 10.0);
    let id = geom::rect_to_quad(sq, &sq.corners()).unwrap();
    assert!(near(
        id.apply(Point::new(3.25, 7.5)).unwrap(),
        Point::new(3.25, 7.5)
    ));
}

#[test]
fn acceptance_59_nearest_enlarge_gives_blocks_bilinear_blends() {
    let img = Image::from_fn(2, 2, |x, y| if (x + y) % 2 == 0 { BLACK } else { WHITE });
    let up = Mat3::scale(2.0, 2.0);

    let nearest = resample(&img, &up, 4, 4, Interp::Nearest);
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(nearest.get(x, y), img.get(x / 2, y / 2), "pixel {x},{y}");
        }
    }

    let bilinear = resample(&img, &up, 4, 4, Interp::Bilinear);
    let blended = bilinear.get(1, 0)[0];
    assert!(blended > 0.05 && blended < 0.95, "blended value {blended}");
}

#[test]
fn acceptance_60_warp_tile_counts_are_clamped() {
    // Small content still gets at least three tiles per axis.
    let (nx, ny) = tile_counts(1.0, 1_000_000.0, 40_000.0);
    assert_eq!(nx, 3);
    assert_eq!(ny, 64_000);
    // Counts never exceed 64000.
    let (nx, ny) = tile_counts(1_000_000.0, 1.0, 40_000.0);
    assert_eq!(nx, 64_000);
    assert_eq!(ny, 3);
}

#[test]
fn acceptance_61_commit_resamples_once_from_original() {
    let src = Image::from_fn(12, 12, |x, y| [x as f32 / 12.0, y as f32 / 12.0, 0.5, 1.0]);
    let footprint = Mask::empty(12, 12);
    let mut two = TransformSession::new(
        src.clone(),
        footprint.clone(),
        Mat3::IDENTITY,
        Interp::Bicubic,
    )
    .unwrap();
    two.apply_gesture(Mat3::translate(0.5, 0.0));
    two.apply_gesture(Mat3::translate(0.25, 0.0));

    let mut one = TransformSession::new(src, footprint, Mat3::IDENTITY, Interp::Bicubic).unwrap();
    one.apply_gesture(Mat3::translate(0.75, 0.0));

    let layer = Image::transparent(12, 12);
    assert_eq!(two.commit(&layer).unwrap(), one.commit(&layer).unwrap());
}

#[test]
fn acceptance_62_commit_leaves_input_layer_untouched() {
    let src = Image::from_fn(6, 6, |x, y| [x as f32 / 6.0, y as f32 / 6.0, 0.25, 1.0]);
    let layer = Image::from_fn(6, 6, |_, _| [0.9, 0.9, 0.9, 1.0]);
    let before = layer.clone();
    let session = TransformSession::new(
        src.clone(),
        Mask::full(6, 6),
        Mat3::IDENTITY,
        Interp::Nearest,
    )
    .unwrap();
    let out = session.commit(&layer).unwrap();
    assert_eq!(layer, before);
    // Identity mapping over a fully cleared footprint reproduces the source.
    assert_eq!(out, src);
    // A cancelled session is simply dropped; the layer was never written.
    drop(session);
    assert_eq!(layer, before);
}

#[test]
fn acceptance_63_copy_takes_only_selected_pixels() {
    let layer = Image::from_fn(16, 16, |_, _| SAMPLE);
    let sel = Selection::active(raster::ellipse(16, 16, rect(0.0, 0.0, 16.0, 16.0)));
    let clip = clipboard::copy_selection(&layer, &sel).unwrap();
    // The top left of the bounding box lies outside the ellipse.
    assert_eq!(clip.image.get(0, 0)[3], 0.0);
    let cx = clip.image.width / 2;
    let cy = clip.image.height / 2;
    assert_eq!(clip.image.get(cx, cy)[3], 1.0);
    assert_eq!(&clip.image.get(cx, cy)[..3], &SAMPLE[..3]);
}

#[test]
fn acceptance_64_cut_copies_then_clears_source() {
    let layer = Image::from_fn(8, 8, |_, _| SAMPLE);
    let sel = Selection::active(raster::rectangle(8, 8, rect(2.0, 2.0, 6.0, 6.0)));
    let (clip, after) = clipboard::cut_selection(&layer, &sel).unwrap();
    assert_eq!((clip.image.width, clip.image.height), (4, 4));
    assert_eq!(clip.image.get(0, 0)[3], 1.0);
    assert_eq!(after.get(3, 3)[3], 0.0);
    assert_eq!(after.get(0, 0)[3], 1.0);
    // The input layer is unchanged, which is what undo restores.
    assert_eq!(layer.get(3, 3)[3], 1.0);
}

#[test]
fn acceptance_65_paste_creates_layer_at_origin_or_centre() {
    let layer = Image::from_fn(16, 16, |_, _| SAMPLE);
    let sel = Selection::active(raster::rectangle(16, 16, rect(4.0, 4.0, 8.0, 8.0)));
    let clip = clipboard::copy_selection(&layer, &sel).unwrap();

    let pasted = clipboard::paste(&clip, 16, 16, None);
    assert_eq!(pasted.get(4, 4)[3], 1.0);
    assert_eq!(pasted.get(3, 3)[3], 0.0);
    // Pasting enters a transform session over the new layer.
    assert!(
        TransformSession::new(pasted, Mask::empty(16, 16), Mat3::IDENTITY, Interp::Bicubic).is_ok()
    );

    let floating = Clip {
        image: clip.image.clone(),
        origin: None,
    };
    let centred = clipboard::paste(&floating, 16, 16, None);
    assert_eq!(centred.get(6, 6)[3], 1.0);
    assert_eq!(centred.get(5, 5)[3], 0.0);
}

#[test]
fn acceptance_66_copy_paste_preserves_values_exactly() {
    let composite = Image::from_fn(10, 7, |x, y| {
        [0.123_456, x as f32 / 3.0, y as f32 / 7.0, 0.876_5]
    });
    let clip = clipboard::copy_all(&composite);
    let pasted = clipboard::paste(&clip, 10, 7, None);
    assert_eq!(pasted, composite);
}

#[test]
fn acceptance_67_crop_inside_canvas() {
    let img = Image::from_fn(32, 32, |x, y| [x as f32 / 32.0, y as f32 / 32.0, 0.0, 1.0]);
    let r = canvas::crop_rect(32, 32, 4.0, 6.0, 10.0, 12.0).unwrap();
    assert_eq!(
        r,
        IRect {
            x: 4,
            y: 6,
            w: 10,
            h: 12
        }
    );
    let out = canvas::crop(&img, r).unwrap();
    assert_eq!((out.width, out.height), (10, 12));
    assert_eq!(out.get(0, 0), img.get(4, 6));
}

#[test]
fn acceptance_68_zero_size_crop_is_refused() {
    assert_eq!(
        canvas::crop_rect(32, 32, 4.0, 4.0, 0.0, 5.0),
        Err(SelectError::EmptyRect)
    );
    assert_eq!(
        canvas::crop_rect(32, 32, 4.0, 4.0, 5.0, 0.0),
        Err(SelectError::EmptyRect)
    );
}

#[test]
fn acceptance_69_resize_canvas_top_left_keeps_content_in_place() {
    let img = Image::from_fn(4, 4, |x, _| [x as f32 / 4.0, 0.0, 0.0, 1.0]);
    let out = canvas::resize_canvas(&img, 8, 8, Anchor::TopLeft).unwrap();
    assert_eq!((out.width, out.height), (8, 8));
    assert_eq!(out.get(3, 3), img.get(3, 3));
    assert_eq!(out.get(4, 4)[3], 0.0);
}

#[test]
fn acceptance_70_resize_image_bicubic_matches_new_size() {
    let img = Image::from_fn(4, 4, |x, y| [x as f32 / 4.0, y as f32 / 4.0, 0.5, 1.0]);
    let out = canvas::resize_image(&img, 10, 6, Interp::Bicubic).unwrap();
    assert_eq!((out.width, out.height), (10, 6));
}

#[test]
fn acceptance_71_flip_canvas_twice_is_identity() {
    let img = Image::from_fn(5, 3, |x, y| [x as f32 / 5.0, y as f32 / 3.0, 0.25, 1.0]);
    assert_eq!(
        canvas::flip_image(&canvas::flip_image(&img, true), true),
        img
    );
    assert_eq!(
        canvas::flip_image(&canvas::flip_image(&img, false), false),
        img
    );
    assert_eq!(canvas::flip_image(&img, true).get(0, 0), img.get(4, 0));
}
