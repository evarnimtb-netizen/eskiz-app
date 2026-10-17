//! Mesh-warped rendering: identity mesh, translated control
//! points, affine warp against the affine path, and commit in warp mode.

use eskiz_select::geom::{Mat3, Point, Rect};
use eskiz_select::image::Image;
use eskiz_select::mask::Mask;
use eskiz_select::raster;
use eskiz_select::resample::Interp;
use eskiz_select::session::TransformSession;
use eskiz_select::warp::Warp;
use eskiz_select::SelectError;

fn source(width: usize, height: usize) -> Image {
    Image::from_fn(width, height, |x, y| {
        [
            x as f32 / width as f32,
            y as f32 / height as f32,
            ((x + y) % 5) as f32 / 4.0,
            1.0,
        ]
    })
}

/// Largest per-channel difference between two images of the same size.
fn max_diff(a: &Image, b: &Image) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.pixels
        .iter()
        .zip(&b.pixels)
        .map(|(p, q)| {
            p.iter()
                .zip(q.iter())
                .map(|(x, y)| (x - y).abs())
                .fold(0.0, f32::max)
        })
        .fold(0.0, f32::max)
}

fn session(src: Image, interp: Interp) -> TransformSession {
    let (w, h) = (src.width, src.height);
    TransformSession::new(src, Mask::full(w, h), Mat3::IDENTITY, interp).unwrap()
}

#[test]
fn warp_identity_mesh_render_equals_source_within_one_255() {
    let src = source(12, 10);
    let warp = Warp::identity(Rect::new(0.0, 0.0, 12.0, 10.0));
    let tolerance = 1.0 / 255.0 + 1e-6;
    for interp in [Interp::Nearest, Interp::Bilinear] {
        let out = warp.render(&src, Rect::new(0.0, 0.0, 12.0, 10.0), 12, 10, interp);
        assert!(max_diff(&out, &src) <= tolerance, "{interp:?} differs");
    }
}

#[test]
fn warp_translated_control_points_equal_plain_translate() {
    let src = source(12, 10);
    let cases = [(3.0, 2.0, Interp::Nearest), (0.4, -0.7, Interp::Bilinear)];
    for (dx, dy, interp) in cases {
        let mut warped = session(src.clone(), interp);
        warped.enter_warp().unwrap();
        for column in warped.warp_mut().unwrap().ctrl.iter_mut() {
            for p in column.iter_mut() {
                p.x += dx;
                p.y += dy;
            }
        }
        let mut plain = session(src.clone(), interp);
        plain.apply_gesture(Mat3::translate(dx, dy));
        let diff = max_diff(&warped.render(), &plain.render());
        assert!(diff <= 1e-3, "translate ({dx}, {dy}) differs by {diff}");
    }
}

#[test]
fn warp_affine_mesh_equals_affine_path() {
    let src = source(16, 12);
    let m = Mat3::translate(0.37, 0.61)
        * Mat3::rotate_deg(10.0).about(Point::new(6.0, 5.0))
        * Mat3::scale(0.9, 1.1);

    let mut affine = session(src.clone(), Interp::Bilinear);
    affine.apply_gesture(m);

    let mut warped = session(src.clone(), Interp::Bilinear);
    warped.enter_warp().unwrap();
    warped.apply_gesture(m);

    let diff = max_diff(&warped.render(), &affine.render());
    assert!(
        diff <= 1e-3,
        "affine mesh differs from affine path by {diff}"
    );

    // The marquee moves through the mesh the same way it moves through the affine path.
    let selection = raster::rectangle(16, 12, Rect::new(3.0, 2.0, 9.0, 8.0));
    let warped_marquee = warped.marquee(&selection);
    let affine_marquee = affine.marquee(&selection);
    let marquee_diff = warped_marquee
        .data
        .iter()
        .zip(&affine_marquee.data)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(marquee_diff <= 1e-3, "marquee differs by {marquee_diff}");
}

#[test]
fn warp_commit_leaves_input_untouched_and_uses_mesh() {
    let src = source(12, 10);
    let layer = Image::from_fn(12, 10, |_, _| [0.9, 0.9, 0.9, 1.0]);
    let before = layer.clone();

    let mut s = session(src.clone(), Interp::Nearest);
    s.enter_warp().unwrap();
    s.apply_gesture(Mat3::translate(2.0, 0.0));
    let out = s.commit(&layer).unwrap();

    assert_eq!(layer, before);
    assert_eq!((out.width, out.height), (12, 10));
    // Committing the mesh equals the mesh preview over a cleared footprint.
    assert!(max_diff(&out, &s.render()) <= 1e-6);
    // The content moved right by two pixels: column 0 is now empty, column 2 holds source column 0.
    assert_eq!(out.get(0, 0)[3], 0.0);
    assert!((out.get(2, 0)[0] - src.get(0, 0)[0]).abs() <= 1e-6);
}

#[test]
fn warp_mode_refuses_quad_edits() {
    let mut s = session(source(8, 8), Interp::Nearest);
    s.enter_warp().unwrap();
    let quad = [
        Point::new(0.0, 0.0),
        Point::new(8.0, 0.0),
        Point::new(8.0, 8.0),
        Point::new(0.0, 8.0),
    ];
    assert_eq!(s.set_quad(&quad), Err(SelectError::WrongMode));
    assert_eq!(s.set_mapping(Mat3::IDENTITY), Err(SelectError::WrongMode));
    assert!(s.quad().is_some());
}
