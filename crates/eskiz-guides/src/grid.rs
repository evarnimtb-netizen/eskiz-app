//! Rectilinear and isometric guides.
//!
//! [`Grid`] is a square lattice that can be rotated and offset. [`Isometric`]
//! has three line families at 30, 90 and 150 degrees, the usual isometric axes.

use crate::geom::{line_family, valid_spacing, Rect, Segment, Vec2};
use crate::guide::{GuideRender, GuideStyle};

/// Isometric axis directions in degrees (canvas space, y down).
pub const ISOMETRIC_ANGLES_DEG: [f64; 3] = [30.0, 90.0, 150.0];

/// A 2D grid with spacing, origin and rotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    /// Distance between neighbouring lines, in canvas pixels.
    pub spacing: f64,
    /// A grid intersection. Lines pass through `origin + i * spacing * u + j * spacing * v`.
    pub origin: Vec2,
    /// Rotation of both axes, in radians.
    pub rotation: f64,
    pub style: GuideStyle,
}

impl Grid {
    /// An unrotated grid at the canvas origin.
    pub fn new(spacing: f64) -> Self {
        Self {
            spacing,
            origin: Vec2::ZERO,
            rotation: 0.0,
            style: GuideStyle::default(),
        }
    }

    /// Unit vectors `(u, v)` along the two grid axes.
    pub fn axes(&self) -> (Vec2, Vec2) {
        let u = Vec2::from_angle(self.rotation);
        (u, Vec2::new(-u.y, u.x))
    }

    /// The nearest grid intersection to `p`. Returns `p` unchanged when the spacing is
    /// not a positive finite number.
    pub fn snap(&self, p: Vec2) -> Vec2 {
        if !valid_spacing(self.spacing) {
            return p;
        }
        let (u, v) = self.axes();
        let local = p - self.origin;
        let i = (local.dot(u) / self.spacing).round();
        let j = (local.dot(v) / self.spacing).round();
        self.origin + u * (i * self.spacing) + v * (j * self.spacing)
    }
}

impl GuideRender for Grid {
    fn style(&self) -> GuideStyle {
        self.style
    }

    fn lines(&self, viewport: Rect) -> Vec<Segment> {
        let (u, v) = self.axes();
        let mut out = line_family(self.origin, u, self.spacing, viewport);
        out.extend(line_family(self.origin, v, self.spacing, viewport));
        out
    }
}

/// An isometric lattice: three families of lines at 30, 90 and 150 degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct Isometric {
    /// Perpendicular distance between neighbouring lines of each family, in pixels.
    pub spacing: f64,
    /// A point that every family passes through.
    pub origin: Vec2,
    /// Rotation applied to all three axes, in radians.
    pub rotation: f64,
    pub style: GuideStyle,
}

impl Isometric {
    /// An unrotated isometric grid at the canvas origin.
    pub fn new(spacing: f64) -> Self {
        Self {
            spacing,
            origin: Vec2::ZERO,
            rotation: 0.0,
            style: GuideStyle::default(),
        }
    }

    /// The three unit axis vectors.
    pub fn axes(&self) -> [Vec2; 3] {
        ISOMETRIC_ANGLES_DEG.map(|deg| Vec2::from_angle(deg.to_radians() + self.rotation))
    }

    /// The axis closest in angle to `dir`. The sign of `dir` is ignored, so the result
    /// may point either way along the axis.
    pub fn nearest_axis(&self, dir: Vec2) -> Vec2 {
        let mut best = Vec2::ZERO;
        let mut best_score = f64::NEG_INFINITY;
        for axis in self.axes() {
            let score = dir.dot(axis).abs();
            if score > best_score {
                best_score = score;
                best = axis;
            }
        }
        best
    }

    /// Projects `p` onto the nearest isometric axis through `anchor`.
    pub fn snap_to_axis(&self, anchor: Vec2, p: Vec2) -> Vec2 {
        let d = p - anchor;
        let axis = self.nearest_axis(d);
        anchor + axis * d.dot(axis)
    }
}

impl GuideRender for Isometric {
    fn style(&self) -> GuideStyle {
        self.style
    }

    fn lines(&self, viewport: Rect) -> Vec<Segment> {
        self.axes()
            .into_iter()
            .flat_map(|axis| line_family(self.origin, axis, self.spacing, viewport))
            .collect()
    }
}
