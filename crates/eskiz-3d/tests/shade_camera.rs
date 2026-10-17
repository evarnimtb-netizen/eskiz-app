use eskiz_3d::{
    encode_output, shade, Light, OrbitCamera, SurfaceInput, MAX_LIGHTS, ROUGHNESS_FLOOR,
};
use glam::{Vec3, Vec4};

fn surface(rough: f32, metal: f32) -> SurfaceInput {
    SurfaceInput {
        position: Vec3::ZERO,
        normal: Vec3::Z,
        eye: Vec3::new(0.0, 0.0, 3.0),
        base_colour: Vec3::new(0.8, 0.2, 0.1),
        roughness: rough,
        metallic: metal,
        ao: 1.0,
    }
}

fn light() -> Light {
    Light::directional(Vec3::new(0.3, 0.4, 1.0), Vec3::ONE, 0.02)
}

const _: () = assert!(0.01 < ROUGHNESS_FLOOR);

#[test]
fn roughness_floor_makes_zero_and_small_values_identical() {
    let a = shade(&surface(0.0, 0.0), &[light()], Vec3::ZERO);
    let b = shade(&surface(0.01, 0.0), &[light()], Vec3::ZERO);
    assert_eq!(a, b);
    let c = shade(&surface(0.3, 0.0), &[light()], Vec3::ZERO);
    assert_ne!(a, c);
}

#[test]
fn only_four_lights_count() {
    let four = [light(); MAX_LIGHTS];
    let mut five = four.to_vec();
    five.push(Light::directional(
        Vec3::new(-0.5, 0.2, 1.0),
        Vec3::ONE,
        5.0,
    ));
    assert_eq!(
        shade(&surface(0.5, 0.0), &four, Vec3::ZERO),
        shade(&surface(0.5, 0.0), &five, Vec3::ZERO)
    );
}

#[test]
fn output_encoding_matches_spec() {
    let e = encode_output(Vec3::splat(0.5), 1.0);
    assert!((e.x - 0.73).abs() < 0.005, "{e}");
    assert_eq!(encode_output(Vec3::splat(4.0), 1.0), Vec3::ONE);
}

#[test]
fn metals_tint_highlights_and_have_no_diffuse() {
    let l = [Light::directional(Vec3::Z, Vec3::ONE, 0.02)];
    let dielectric = shade(&surface(0.4, 0.0), &l, Vec3::ZERO);
    let metal = shade(&surface(0.4, 1.0), &l, Vec3::ZERO);
    // Head-on highlight on a red-ish metal keeps the albedo ratio (F0 = albedo).
    assert!(metal.x / metal.y > dielectric.x / dielectric.y);
    // Back-facing light adds nothing.
    let behind = [Light::directional(-Vec3::Z, Vec3::ONE, 1.0)];
    assert_eq!(shade(&surface(0.5, 0.0), &behind, Vec3::ZERO), Vec3::ZERO);
}

#[test]
fn orbit_camera_matrices() {
    let mut cam = OrbitCamera::new(Vec3::new(1.0, 2.0, 3.0), 0.7, 0.3, 4.0, 50f32.to_radians());
    // The target sits on the view axis, at -distance.
    let v = cam.view() * Vec4::new(1.0, 2.0, 3.0, 1.0);
    assert!(
        v.x.abs() < 1e-5 && v.y.abs() < 1e-5 && (v.z + 4.0).abs() < 1e-5,
        "{v}"
    );
    let clip = cam.view_projection(1.5) * Vec4::new(1.0, 2.0, 3.0, 1.0);
    assert!((clip.x / clip.w).abs() < 1e-5 && (clip.y / clip.w).abs() < 1e-5);
    assert!((cam.eye() - cam.target).length() - 4.0 < 1e-5);
    // Yaw 0 / pitch 0 looks down -Z from +Z.
    cam.yaw = 0.0;
    cam.pitch = 0.0;
    assert!((cam.eye() - (cam.target + Vec3::Z * 4.0)).length() < 1e-5);
    // Zoom is clamped to 0.05..30; pitch to just under the poles.
    cam.zoom_by(1e6);
    assert!((cam.zoom() - 30.0).abs() < 1e-3);
    cam.zoom_by(1e-9);
    assert!((cam.zoom() - 0.05).abs() < 1e-5);
    cam.orbit(0.0, 10.0);
    assert!(cam.pitch < std::f32::consts::FRAC_PI_2);
    // Framing a box centres on it.
    cam.frame(Vec3::splat(-1.0), Vec3::splat(3.0));
    assert_eq!(cam.target, Vec3::ONE);
    assert!((cam.zoom() - 1.0).abs() < 1e-6);
}
