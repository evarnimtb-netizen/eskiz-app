//! Canvas view transform: zoom, rotation and offset, with screen <-> canvas mapping.
//!
//! The canvas's top-left corner sits at `viewport.min + offset`. Canvas pixels are scaled
//! by `zoom` (screen points per pixel) and then rotated by `rotation` (radians, clockwise
//! on screen) around that corner:
//!
//! `screen = viewport.min + offset + R(rotation) * (canvas_px * zoom)`

use eframe::egui::{Pos2, Rect, Vec2};

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 32.0;

/// Zoom snaps to 100% when it enters this band around 1.0.
pub const ZOOM_SNAP: f32 = 0.05;
/// Rotation snaps to 0 when it enters this band (5 degrees).
pub const ROTATION_SNAP: f32 = 5.0 * std::f32::consts::PI / 180.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f32,
    /// Position of the canvas's top-left corner, relative to the top-left of the viewport.
    pub offset: Vec2,
    /// Rotation in radians.
    pub rotation: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset: Vec2::ZERO,
            rotation: 0.0,
        }
    }
}

/// `(cos, sin)` of an angle.
fn cos_sin(a: f32) -> (f32, f32) {
    (a.cos(), a.sin())
}

impl View {
    /// Zoom so the whole canvas fits in `viewport` with a small margin, and centre it.
    /// Rotation is reset to zero.
    pub fn fit(viewport: Rect, canvas_w: f32, canvas_h: f32) -> Self {
        let zoom = ((viewport.width() / canvas_w).min(viewport.height() / canvas_h) * 0.95)
            .clamp(MIN_ZOOM, MAX_ZOOM);
        let size = Vec2::new(canvas_w * zoom, canvas_h * zoom);
        Self {
            zoom,
            offset: (viewport.size() - size) / 2.0,
            rotation: 0.0,
        }
    }

    /// Screen position of a canvas point (in canvas pixels).
    pub fn canvas_to_screen(&self, viewport: Rect, x: f64, y: f64) -> Pos2 {
        let (c, s) = cos_sin(self.rotation);
        let (x, y) = (x as f32 * self.zoom, y as f32 * self.zoom);
        viewport.min + self.offset + Vec2::new(x * c - y * s, x * s + y * c)
    }

    /// Screen position to canvas pixels.
    pub fn screen_to_canvas(&self, viewport: Rect, p: Pos2) -> (f64, f64) {
        let (c, s) = cos_sin(self.rotation);
        let d = p - (viewport.min + self.offset);
        // Inverse rotation, then divide by the zoom.
        let x = d.x * c + d.y * s;
        let y = -d.x * s + d.y * c;
        (f64::from(x / self.zoom), f64::from(y / self.zoom))
    }

    /// The four screen corners of the canvas: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(&self, viewport: Rect, canvas_w: f32, canvas_h: f32) -> [Pos2; 4] {
        [
            self.canvas_to_screen(viewport, 0.0, 0.0),
            self.canvas_to_screen(viewport, f64::from(canvas_w), 0.0),
            self.canvas_to_screen(viewport, f64::from(canvas_w), f64::from(canvas_h)),
            self.canvas_to_screen(viewport, 0.0, f64::from(canvas_h)),
        ]
    }

    /// The four screen corners of a canvas-space rectangle `[left, top, right, bottom]`, in order:
    /// top-left, top-right, bottom-right, bottom-left (of the canvas axes, whatever the rotation).
    pub fn rect_corners(&self, viewport: Rect, r: [f64; 4]) -> [Pos2; 4] {
        [
            self.canvas_to_screen(viewport, r[0], r[1]),
            self.canvas_to_screen(viewport, r[2], r[1]),
            self.canvas_to_screen(viewport, r[2], r[3]),
            self.canvas_to_screen(viewport, r[0], r[3]),
        ]
    }

    /// Whether the view is upright (no rotation to speak of).
    pub fn is_upright(&self) -> bool {
        self.rotation.abs() < 1e-6
    }

    /// Multiply the zoom by `factor`, keeping the canvas point under `anchor` fixed.
    pub fn zoom_at(&mut self, viewport: Rect, anchor: Pos2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let (px, py) = self.screen_to_canvas(viewport, anchor);
        self.zoom = new_zoom;
        self.pin(viewport, anchor, px, py);
    }

    /// Rotate by `delta` radians around the screen point `anchor`.
    pub fn rotate_at(&mut self, viewport: Rect, anchor: Pos2, delta: f32) {
        if !delta.is_finite() {
            return;
        }
        let (px, py) = self.screen_to_canvas(viewport, anchor);
        self.rotation += delta;
        self.pin(viewport, anchor, px, py);
    }

    /// Put canvas point `(px, py)` back under screen point `anchor`, by moving the offset.
    fn pin(&mut self, viewport: Rect, anchor: Pos2, px: f64, py: f64) {
        let (c, s) = cos_sin(self.rotation);
        let (x, y) = (px as f32 * self.zoom, py as f32 * self.zoom);
        let rotated = Vec2::new(x * c - y * s, x * s + y * c);
        self.offset = anchor - viewport.min - rotated;
    }

    pub fn pan(&mut self, delta: Vec2) {
        self.offset += delta;
    }

    /// Whether the zoom is at 100%.
    pub fn at_100(&self) -> bool {
        (self.zoom - 1.0).abs() < 1e-4
    }
}

/// Snap a zoom change: when a pinch carries the zoom into the 100% band from outside,
/// it lands exactly on 1.0. Returns the new zoom and whether it snapped.
pub fn snap_zoom(prev: f32, next: f32) -> (f32, bool) {
    let inside = |z: f32| (z - 1.0).abs() <= ZOOM_SNAP;
    if inside(next) && !inside(prev) {
        (1.0, true)
    } else {
        (next, false)
    }
}

/// Rotation `r` wrapped into [-pi, pi).
pub fn wrap_angle(r: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if (-PI..PI).contains(&r) {
        return r;
    }
    (r + PI).rem_euclid(TAU) - PI
}

/// Distance in radians from `r` to the nearest quarter turn (0, 90, 180, 270 degrees), and that quarter.
fn nearest_quarter(r: f32) -> (f32, f32) {
    let q = std::f32::consts::FRAC_PI_2;
    let k = (r / q).round();
    ((r - k * q).abs(), k * q)
}

/// Snap a rotation change: entering the band around 0, 90, 180 or 270 degrees lands exactly on it.
/// Returns the new rotation and whether it snapped.
pub fn snap_rotation(prev: f32, next: f32) -> (f32, bool) {
    let (d_prev, _) = nearest_quarter(prev);
    let (d_next, quarter) = nearest_quarter(next);
    if d_next <= ROTATION_SNAP && d_prev > ROTATION_SNAP {
        (wrap_angle(quarter), true)
    } else {
        (next, false)
    }
}

/// A rotation being dragged: `raw` is the unsnapped angle the hand has turned to. Within the snap
/// band of a quarter turn the view sits exactly on it (so the canvas "sticks" until the hand
/// leaves the band); outside it follows `raw`. Returns the view rotation and whether it is snapped.
pub fn sticky_rotation(raw: f32) -> (f32, bool) {
    let (d, quarter) = nearest_quarter(raw);
    if d <= ROTATION_SNAP {
        (wrap_angle(quarter), true)
    } else {
        (wrap_angle(raw), false)
    }
}

/// The rotation one quarter turn from `r`: `dir` is +1 for clockwise (Rotate 90 right) or -1 for
/// counter-clockwise. From a rotation that is not on a quarter, it goes to the next quarter in
/// that direction. The result is wrapped into [-pi, pi).
pub fn quarter_turn(r: f32, dir: i32) -> f32 {
    let q = std::f32::consts::FRAC_PI_2;
    let steps = r / q;
    let k = if dir >= 0 { (steps + 1e-3).floor() + 1.0 } else { (steps - 1e-3).ceil() - 1.0 };
    wrap_angle(k * q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn viewport() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), Vec2::new(800.0, 600.0))
    }

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn fit_centres_and_fits() {
        let vp = viewport();
        let v = View::fit(vp, 2048.0, 1536.0);
        let c = v.corners(vp, 2048.0, 1536.0);
        let centre = (c[0] + c[2].to_vec2()) / 2.0;
        assert!(c[1].x - c[0].x <= vp.width() + 1e-3);
        assert!((centre - vp.center()).length() < 1e-2);
    }

    #[test]
    fn screen_to_canvas_inverts_canvas_to_screen() {
        let vp = viewport();
        let v = View {
            zoom: 0.5,
            offset: Vec2::new(30.0, 10.0),
            rotation: 0.0,
        };
        let p = v.canvas_to_screen(vp, 100.0, 50.0);
        assert!(close(v.screen_to_canvas(vp, p), (100.0, 50.0)));
    }

    #[test]
    fn rotated_view_round_trips() {
        let vp = viewport();
        for deg in [-170.0f32, -45.0, 0.0, 17.0, 90.0, 123.0] {
            let v = View {
                zoom: 1.7,
                offset: Vec2::new(-40.0, 25.0),
                rotation: deg.to_radians(),
            };
            for (x, y) in [(0.0, 0.0), (13.5, 77.25), (640.0, 480.0), (2047.0, 1535.0)] {
                let p = v.canvas_to_screen(vp, x, y);
                let back = v.screen_to_canvas(vp, p);
                assert!(close(back, (x, y)), "rotation {deg}: {back:?} vs {x},{y}");
            }
        }
    }

    #[test]
    fn rotated_painting_lands_on_the_right_pixel() {
        // A 90 degree view: canvas x axis points down the screen.
        let vp = viewport();
        let v = View {
            zoom: 2.0,
            offset: Vec2::ZERO,
            rotation: std::f32::consts::FRAC_PI_2,
        };
        let origin = v.canvas_to_screen(vp, 0.0, 0.0);
        let down = v.canvas_to_screen(vp, 10.0, 0.0);
        assert!((down.x - origin.x).abs() < 1e-3);
        assert!((down.y - origin.y - 20.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_keeps_anchor_fixed_under_rotation() {
        let vp = viewport();
        let mut v = View {
            zoom: 0.8,
            offset: Vec2::new(10.0, 10.0),
            rotation: 0.6,
        };
        let anchor = pos2(400.0, 300.0);
        let before = v.screen_to_canvas(vp, anchor);
        v.zoom_at(vp, anchor, 2.5);
        assert!(close(v.screen_to_canvas(vp, anchor), before));
        assert!((v.zoom - 2.0).abs() < 1e-5);
    }

    #[test]
    fn rotate_keeps_anchor_fixed() {
        let vp = viewport();
        let mut v = View {
            zoom: 1.3,
            offset: Vec2::new(-20.0, 5.0),
            rotation: 0.1,
        };
        let anchor = pos2(250.0, 420.0);
        let before = v.screen_to_canvas(vp, anchor);
        v.rotate_at(vp, anchor, 1.0);
        assert!(close(v.screen_to_canvas(vp, anchor), before));
        assert!((v.rotation - 1.1).abs() < 1e-5);
    }

    #[test]
    fn zoom_is_clamped() {
        let vp = viewport();
        let mut v = View::default();
        for _ in 0..100 {
            v.zoom_at(vp, vp.center(), 2.0);
        }
        assert_eq!(v.zoom, MAX_ZOOM);
        for _ in 0..200 {
            v.zoom_at(vp, vp.center(), 0.5);
        }
        assert_eq!(v.zoom, MIN_ZOOM);
        let before = v;
        v.zoom_at(vp, vp.center(), f32::NAN);
        assert_eq!(v, before);
    }

    #[test]
    fn zoom_snaps_to_100_when_entering_the_band() {
        assert_eq!(snap_zoom(0.8, 1.03), (1.0, true));
        assert_eq!(snap_zoom(0.8, 1.2), (1.2, false));
        // Already inside the band: free movement, no repeated snapping.
        assert_eq!(snap_zoom(1.02, 1.04), (1.04, false));
        assert!(View { zoom: 1.0, ..View::default() }.at_100());
    }

    #[test]
    fn rotation_snaps_to_zero_within_five_degrees() {
        let (r, snapped) = snap_rotation(0.2, 4.0f32.to_radians());
        assert!(snapped && r == 0.0);
        let (r, snapped) = snap_rotation(0.2, 6.0f32.to_radians());
        assert!(!snapped && (r - 6.0f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn pan_moves_canvas() {
        let mut v = View::default();
        v.pan(Vec2::new(5.0, -3.0));
        assert_eq!(v.offset, Vec2::new(5.0, -3.0));
    }

    #[test]
    fn rotation_snaps_to_every_quarter_turn() {
        let q = std::f32::consts::FRAC_PI_2;
        let (r, snapped) = snap_rotation(q - 0.3, q - 0.05);
        assert!(snapped);
        assert!((r - q).abs() < 1e-6);
        let (r, snapped) = snap_rotation(3.0, std::f32::consts::PI + 0.04);
        assert!(snapped && (r.abs() - std::f32::consts::PI).abs() < 1e-5, "180 degrees wraps to -pi: {r}");
        assert_eq!(snap_rotation(0.7, 0.8), (0.8, false));
    }

    #[test]
    fn sticky_rotation_holds_a_quarter_until_the_hand_leaves_the_band() {
        let q = std::f32::consts::FRAC_PI_2;
        assert_eq!(sticky_rotation(0.05), (0.0, true));
        let (r, snapped) = sticky_rotation(q + 0.07);
        assert!(snapped && (r - q).abs() < 1e-6);
        let (r, snapped) = sticky_rotation(q + 0.2);
        assert!(!snapped && (r - (q + 0.2)).abs() < 1e-6);
        // Unbounded raw angles wrap.
        let (r, _) = sticky_rotation(2.0 * std::f32::consts::TAU + 0.5);
        assert!((r - 0.5).abs() < 1e-4);
    }

    #[test]
    fn quarter_turns_go_to_the_next_quarter_in_the_direction() {
        let q = std::f32::consts::FRAC_PI_2;
        assert!((quarter_turn(0.0, 1) - q).abs() < 1e-5);
        assert!((quarter_turn(0.0, -1) + q).abs() < 1e-5);
        assert!((quarter_turn(0.3, 1) - q).abs() < 1e-5);
        assert!(quarter_turn(0.3, -1).abs() < 1e-5);
        assert!((quarter_turn(q, 1).abs() - std::f32::consts::PI).abs() < 1e-5);
        assert!(quarter_turn(-q, 1).abs() < 1e-5);
        // Four turns come back to upright, whichever way.
        let mut r = 0.0;
        for _ in 0..4 {
            r = quarter_turn(r, -1);
        }
        assert!(r.abs() < 1e-4);
    }
}
