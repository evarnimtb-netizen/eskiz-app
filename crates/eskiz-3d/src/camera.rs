//! Orbit camera.

use glam::{Mat4, Vec3};

/// Camera near clip distance.
pub const NEAR: f32 = 0.01;
/// Allowed zoom range.
pub const ZOOM_RANGE: (f32, f32) = (0.05, 30.0);
/// Pan speed factor.
pub const PAN_SPEED: f32 = 0.66;

/// Turntable camera looking at `target` from `distance`, Y up.
///
/// `yaw` rotates around +Y (0 looks down -Z from +Z), `pitch` is elevation in
/// radians (positive = camera above the target).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
    /// Distance at zoom 1, set by [`OrbitCamera::frame`].
    pub base_distance: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            distance: 3.0,
            fov_y: 60f32.to_radians(),
            near: NEAR,
            far: 1000.0,
            base_distance: 3.0,
        }
    }
}

impl OrbitCamera {
    /// Camera with explicit orbit parameters.
    pub fn new(target: Vec3, yaw: f32, pitch: f32, distance: f32, fov_y: f32) -> Self {
        Self {
            target,
            yaw,
            pitch,
            distance,
            fov_y,
            base_distance: distance,
            ..Self::default()
        }
    }

    /// World-space eye position.
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + self.distance * Vec3::new(cp * sy, sp, cp * cy)
    }

    /// Right-handed view matrix.
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    /// Right-handed perspective projection with depth in 0..1, between [`Self::clip_planes`].
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let (near, far) = self.clip_planes();
        Mat4::perspective_rh(self.fov_y, aspect, near, far)
    }

    /// Near and far planes for the framed model at the current zoom. The far plane follows the
    /// distance, so zooming out never puts the model behind it; the near plane scales with the
    /// model, so a tiny model is not clipped away and a huge one keeps its depth precision.
    /// `near` and `far` are lower and upper bounds on top of that.
    pub fn clip_planes(&self) -> (f32, f32) {
        let radius = (self.base_distance * (self.fov_y * 0.5).sin() / 1.1).max(1e-6);
        let near = (radius * 1e-3).min(self.near);
        let far = self.far.max((self.distance + radius) * 4.0);
        (near, far)
    }

    /// `projection * view`.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// Orbit by yaw and pitch deltas (radians). Pitch is kept below the poles.
    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        let lim = 89f32.to_radians();
        self.yaw += d_yaw;
        self.pitch = (self.pitch + d_pitch).clamp(-lim, lim);
    }

    /// Current zoom (1 at the framed distance; larger is closer).
    pub fn zoom(&self) -> f32 {
        self.base_distance / self.distance
    }

    /// Multiply the zoom by `factor`, clamped to 0.05..30.
    pub fn zoom_by(&mut self, factor: f32) {
        let z = (self.zoom() * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        self.distance = self.base_distance / z;
    }

    /// Pan in the view plane by screen-fraction deltas (1.0 = viewport height).
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let v = self.view().inverse();
        let right = v.x_axis.truncate();
        let up = v.y_axis.truncate();
        let world_per_screen = 2.0 * self.distance * (self.fov_y * 0.5).tan() * PAN_SPEED;
        self.target += (-right * dx + up * dy) * world_per_screen;
    }

    /// Frame a bounding box: centre on it and back off so it fits the view.
    pub fn frame(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let radius = ((max - min) * 0.5).length().max(1e-4);
        self.distance = radius / (self.fov_y * 0.5).sin() * 1.1;
        self.base_distance = self.distance;
        self.far = (self.distance + radius) * 4.0;
    }
}
