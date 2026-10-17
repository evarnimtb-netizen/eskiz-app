use eskiz_3d::{apply_normal_map, tangent_frame};
use glam::{Vec2, Vec3};

#[test]
fn tangent_frame_points_up_the_image_for_normal_and_mirrored_uvs() {
    let p = [
        Vec3::new(-1., -1., 0.),
        Vec3::new(1., -1., 0.),
        Vec3::new(1., 1., 0.),
    ];
    let n = Vec3::Z;
    // Standard layout: u grows with +x and v grows downward, so up the image is +y.
    let std_uv = [Vec2::new(0., 1.), Vec2::new(1., 1.), Vec2::new(1., 0.)];
    let (t, b) = tangent_frame(p, std_uv, n);
    assert!(t.abs_diff_eq(Vec3::X, 1e-5), "{t}");
    assert!(b.abs_diff_eq(Vec3::Y, 1e-5), "{b}");
    // Mirrored layout: u grows with -x, so the tangent flips but up the image is still +y.
    let mir_uv = [Vec2::new(1., 1.), Vec2::new(0., 1.), Vec2::new(0., 0.)];
    let (t, b) = tangent_frame(p, mir_uv, n);
    assert!(t.abs_diff_eq(-Vec3::X, 1e-5), "{t}");
    assert!(b.abs_diff_eq(Vec3::Y, 1e-5), "{b}");
    // A flat-green normal sample tilts the normal up the image.
    let world = apply_normal_map([0.5, 1.0, 0.5], t, b, n);
    assert!(world.abs_diff_eq(Vec3::Y, 1e-4), "{world}");
}
