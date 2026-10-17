//! Transform geometry: quad edits, uniform scale, snapping, numeric input, flip and fit.
//! Pure functions over quads and matrices; sessions live in `session`.

use crate::geom::{quad_bounds, Mat3, Point, Quad, Rect};

/// Angle snap step in degrees.
pub const ANGLE_SNAP_STEP_DEG: f32 = 15.0;
/// Angle snap threshold in degrees.
pub const ANGLE_SNAP_THRESHOLD_DEG: f32 = 5.0;

/// How a dragged angle is mapped to a 15-degree multiple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapMode {
    /// Snap to the nearest multiple (Eskiz's rule, proposed).
    #[default]
    Nearest,
    /// Use the multiple below the angle (the first step of the angle snap).
    /// The multiple above is not tested here.
    Truncate,
}

/// A handle on a quad: a corner, or the side between corner `i` and corner `i + 1` (mod 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Corner(usize),
    Edge(usize),
}

/// Normalises an angle to `(-180, 180]` degrees.
pub fn normalise_angle(deg: f32) -> f32 {
    let r = deg.rem_euclid(360.0);
    if r > 180.0 {
        r - 360.0
    } else {
        r
    }
}

/// Snaps an angle to a 15-degree multiple when it is within 5 degrees of one.
pub fn snap_angle(deg: f32, mode: SnapMode) -> f32 {
    let step = ANGLE_SNAP_STEP_DEG;
    let candidate = match mode {
        SnapMode::Nearest => (deg / step).round() * step,
        SnapMode::Truncate => (deg / step).floor() * step,
    };
    if (deg - candidate).abs() <= ANGLE_SNAP_THRESHOLD_DEG {
        candidate
    } else {
        deg
    }
}

/// Scale factor from a typed size: `typed / current`. None for a non-positive size.
pub fn numeric_scale(current: f32, typed: f32) -> Option<f32> {
    (current > 0.0).then(|| typed / current)
}

/// Per-axis scale from typed sizes. With the aspect locked, both axes use the smaller factor.
pub fn numeric_scale_pair(
    current: (f32, f32),
    typed: (f32, f32),
    aspect_locked: bool,
) -> Option<(f32, f32)> {
    let sx = numeric_scale(current.0, typed.0)?;
    let sy = numeric_scale(current.1, typed.1)?;
    if aspect_locked {
        let s = sx.min(sy);
        Some((s, s))
    } else {
        Some((sx, sy))
    }
}

/// Rotation to apply for a typed absolute angle: `typed - current`, normalised.
pub fn numeric_rotation(current_deg: f32, typed_deg: f32) -> f32 {
    normalise_angle(typed_deg - current_deg)
}

/// Free drag: a corner moves alone; an edge moves its two corners together.
pub fn free_drag(quad: Quad, handle: Handle, delta: Point) -> Quad {
    let mut out = quad;
    for i in handle_corners(handle) {
        out[i] = out[i] + delta;
    }
    out
}

/// Body drag: translates the whole quad.
pub fn translate_quad(quad: Quad, delta: Point) -> Quad {
    quad.map(|p| p + delta)
}

/// Distort: moves one corner pin; the others stay.
pub fn distort_move_corner(quad: Quad, index: usize, to: Point) -> Quad {
    let mut out = quad;
    if let Some(corner) = out.get_mut(index) {
        *corner = to;
    }
    out
}

/// Uniform drag: scales the whole quad about the opposite anchor. The factor is
/// `1 + projection(drag, direction) / length`, where the direction and length run from the
/// anchor to the grabbed corner or side midpoint. The aspect ratio is kept.
pub fn uniform_drag(quad: Quad, handle: Handle, drag: Point) -> Quad {
    let (anchor, grabbed) = match handle {
        Handle::Corner(i) => (quad[(i + 2) % 4], quad[i % 4]),
        Handle::Edge(i) => (
            midpoint(quad[(i + 2) % 4], quad[(i + 3) % 4]),
            midpoint(quad[i % 4], quad[(i + 1) % 4]),
        ),
    };
    let d = grabbed - anchor;
    let len = d.length();
    if len <= f32::EPSILON {
        return quad;
    }
    let s = 1.0 + drag.dot(d * (1.0 / len)) / len;
    quad.map(|p| anchor + (p - anchor) * s)
}

/// Rotation by `deg` about `centre`: `T(centre) * R * T(-centre)`.
pub fn rotate_about(centre: Point, deg: f32) -> Mat3 {
    Mat3::rotate_deg(deg).about(centre)
}

/// Rotation by 45 degrees about the centre of the bounds.
pub fn rotate_45_about_bounds(bounds: Rect) -> Mat3 {
    rotate_about(bounds.centre(), 45.0)
}

/// Mirror about the centre of `bounds`. Horizontal mirrors left and right.
pub fn flip_about(bounds: Rect, horizontal: bool) -> Mat3 {
    let mirror = if horizontal {
        Mat3::scale(-1.0, 1.0)
    } else {
        Mat3::scale(1.0, -1.0)
    };
    mirror.about(bounds.centre())
}

/// Gesture that scales `content` uniformly to fit inside a canvas of the given size, centred.
/// Content that already fits keeps its scale and is only centred. None when the
/// content has no area.
pub fn fit_gesture(content: Rect, canvas_w: f32, canvas_h: f32) -> Option<Mat3> {
    let (cw, ch) = (content.width(), content.height());
    if cw <= 0.0 || ch <= 0.0 {
        return None;
    }
    let s = if cw <= canvas_w && ch <= canvas_h {
        1.0
    } else {
        (canvas_w / cw).min(canvas_h / ch)
    };
    let target = Point::new(canvas_w * 0.5, canvas_h * 0.5);
    let from = content.centre();
    Some(
        Mat3::translate(target.x, target.y) * Mat3::scale(s, s) * Mat3::translate(-from.x, -from.y),
    )
}

/// Axis-aligned bounds of a quad, as a rectangle.
pub fn bounds_of(quad: &Quad) -> Rect {
    quad_bounds(quad)
}

fn midpoint(a: Point, b: Point) -> Point {
    (a + b) * 0.5
}

fn handle_corners(handle: Handle) -> Vec<usize> {
    match handle {
        Handle::Corner(i) => vec![i % 4],
        Handle::Edge(i) => vec![i % 4, (i + 1) % 4],
    }
}
