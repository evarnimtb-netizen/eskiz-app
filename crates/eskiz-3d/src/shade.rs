//! CPU reference of the preview shader: Cook-Torrance with a GGX
//! distribution and Schlick Fresnel, up to four direct lights, a constant
//! ambient term standing in for image-based lighting, then exposure and gamma.

use std::f32::consts::PI;

use glam::{Vec2, Vec3};

/// Lights beyond this count are ignored.
pub const MAX_LIGHTS: usize = 4;
/// Roughness is clamped to at least this for the specular lobe.
pub const ROUGHNESS_FLOOR: f32 = 0.045;
/// Reflectance of dielectrics at normal incidence.
pub const DIELECTRIC_F0: f32 = 0.04;
/// Stored light intensity is multiplied by this.
pub const LIGHT_INTENSITY_SCALE: f32 = 50.0;
/// Output gamma exponent.
pub const OUTPUT_GAMMA: f32 = 0.45459;

/// A direct light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// World position (point light) or direction *towards* the light (directional).
    pub vector: Vec3,
    pub directional: bool,
    /// Linear RGB colour.
    pub colour: Vec3,
    /// Stored intensity; the shader multiplies it by [`LIGHT_INTENSITY_SCALE`].
    pub intensity: f32,
}

impl Light {
    /// Directional light shining from direction `to_light`.
    pub fn directional(to_light: Vec3, colour: Vec3, intensity: f32) -> Self {
        Self {
            vector: to_light,
            directional: true,
            colour,
            intensity,
        }
    }

    /// Point light with inverse-square falloff.
    pub fn point(position: Vec3, colour: Vec3, intensity: f32) -> Self {
        Self {
            vector: position,
            directional: false,
            colour,
            intensity,
        }
    }
}

/// Inputs for shading one pixel. Colours are linear RGB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceInput {
    pub position: Vec3,
    /// Unit surface normal (already normal-mapped).
    pub normal: Vec3,
    /// World-space eye position.
    pub eye: Vec3,
    pub base_colour: Vec3,
    pub roughness: f32,
    pub metallic: f32,
    /// Ambient occlusion, 1 = unoccluded.
    pub ao: f32,
}

/// Linear HDR radiance for one pixel (before exposure and gamma).
///
/// `ambient` is a constant sky colour used for the indirect term; `lights` past
/// [`MAX_LIGHTS`] are ignored.
pub fn shade(s: &SurfaceInput, lights: &[Light], ambient: Vec3) -> Vec3 {
    let n = s.normal.normalize_or_zero();
    let v = (s.eye - s.position).normalize_or_zero();
    let metallic = s.metallic.clamp(0.0, 1.0);
    let rough = s.roughness.clamp(0.0, 1.0).max(ROUGHNESS_FLOOR);
    let f0 = Vec3::splat(DIELECTRIC_F0).lerp(s.base_colour, metallic);
    let diffuse_colour = s.base_colour * (1.0 - metallic);
    let n_dot_v = n.dot(v).max(1e-4);

    let mut out = ambient * diffuse_colour * s.ao;
    for l in lights.iter().take(MAX_LIGHTS) {
        let (dir, atten) = if l.directional {
            (l.vector.normalize_or_zero(), 1.0)
        } else {
            let d = l.vector - s.position;
            (d.normalize_or_zero(), 1.0 / d.length_squared().max(1e-6))
        };
        let n_dot_l = n.dot(dir);
        if n_dot_l <= 0.0 {
            continue;
        }
        let h = (v + dir).normalize_or_zero();
        let n_dot_h = n.dot(h).max(0.0);
        let v_dot_h = v.dot(h).max(0.0);
        let d_term = ggx_distribution(n_dot_h, rough);
        let g_term = smith_schlick_ggx(n_dot_v, rough) * smith_schlick_ggx(n_dot_l, rough);
        let f = fresnel_schlick(v_dot_h, f0);
        let spec = f * (d_term * g_term / (4.0 * n_dot_v * n_dot_l).max(1e-4));
        let kd = (Vec3::ONE - f) * diffuse_colour / PI;
        let radiance = l.colour * (l.intensity * LIGHT_INTENSITY_SCALE * atten);
        out += (kd + spec) * radiance * n_dot_l;
    }
    out
}

/// Exposure then gamma, per channel, clamped to 0..1 for display.
pub fn encode_output(linear: Vec3, exposure: f32) -> Vec3 {
    (linear * exposure)
        .max(Vec3::ZERO)
        .powf(OUTPUT_GAMMA)
        .min(Vec3::ONE)
}

/// GGX / Trowbridge-Reitz normal distribution with `alpha = roughness^2`.
pub fn ggx_distribution(n_dot_h: f32, roughness: f32) -> f32 {
    let a = roughness * roughness;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    a2 / (PI * d * d).max(1e-12)
}

/// Schlick-GGX geometry term for one direction (direct-light remap of roughness).
pub fn smith_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    n_dot_x / (n_dot_x * (1.0 - k) + k)
}

/// Schlick Fresnel.
pub fn fresnel_schlick(cos_theta: f32, f0: Vec3) -> Vec3 {
    f0 + (Vec3::ONE - f0) * (1.0 - cos_theta).clamp(0.0, 1.0).powi(5)
}

/// Tangent frame of a triangle from positions and UVs, orthogonalised against `normal`.
/// Returns `(tangent, bitangent)`, where the tangent follows +u and the bitangent points
/// up the image (decreasing v, since UV v grows downward).
pub fn tangent_frame(p: [Vec3; 3], uv: [Vec2; 3], normal: Vec3) -> (Vec3, Vec3) {
    let (e1, e2) = (p[1] - p[0], p[2] - p[0]);
    let (d1, d2) = (uv[1] - uv[0], uv[2] - uv[0]);
    let det = d1.x * d2.y - d2.x * d1.y;
    if det.abs() < 1e-12 {
        let t = normal.any_orthonormal_vector();
        return (t, normal.cross(t));
    }
    // dP/du and -dP/dv, solved from the edge and UV deltas.
    let dp_du = (e1 * d2.y - e2 * d1.y) / det;
    let minus_dp_dv = (e1 * d2.x - e2 * d1.x) / det;
    let t = (dp_du - normal * normal.dot(dp_du)).normalize_or_zero();
    let t = if t == Vec3::ZERO {
        normal.any_orthonormal_vector()
    } else {
        t
    };
    // Choose the sign of cross(normal, t) that agrees with -dP/dv. This keeps the
    // bitangent pointing up the image for mirrored UVs too.
    let n_cross_t = normal.cross(t);
    let b = if minus_dp_dv.dot(n_cross_t) < 0.0 {
        -n_cross_t
    } else {
        n_cross_t
    };
    (t, b)
}

/// Decode a tangent-space normal-map sample (`rgb` in 0..1) into world space.
pub fn apply_normal_map(rgb: [f32; 3], tangent: Vec3, bitangent: Vec3, normal: Vec3) -> Vec3 {
    let t = Vec3::new(rgb[0] * 2.0 - 1.0, rgb[1] * 2.0 - 1.0, rgb[2] * 2.0 - 1.0);
    (tangent * t.x + bitangent * t.y + normal * t.z).normalize_or_zero()
}
