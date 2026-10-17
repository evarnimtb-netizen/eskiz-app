//! Perspective guides: one, two or three vanishing points with an optional horizon.
//!
//! Each vanishing point draws a fan of lines through it. The fan covers the full
//! 180 degrees of undirected lines, so a viewport always sees every line the fan
//! contains. With two or three points the horizon runs through the first two
//! points. With one point it is horizontal through that point.

use std::f64::consts::PI;

use crate::geom::{line_through, Rect, Segment, Vec2};
use crate::guide::{GuideRender, GuideStyle};

/// Undirected lines through each vanishing point by default (one every 15 degrees).
pub const DEFAULT_DENSITY: u32 = 12;

/// Number of vanishing points in a perspective guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerspectiveKind {
    OnePoint,
    TwoPoint,
    ThreePoint,
}

impl PerspectiveKind {
    pub fn point_count(self) -> usize {
        match self {
            PerspectiveKind::OnePoint => 1,
            PerspectiveKind::TwoPoint => 2,
            PerspectiveKind::ThreePoint => 3,
        }
    }
}

/// A perspective guide. Vanishing points can sit off the canvas and can be moved.
#[derive(Debug, Clone, PartialEq)]
pub struct Perspective {
    kind: PerspectiveKind,
    points: Vec<Vec2>,
    /// Undirected lines drawn through each vanishing point across 180 degrees.
    pub density: u32,
    /// Whether the horizon is drawn.
    pub show_horizon: bool,
    pub style: GuideStyle,
}

impl Perspective {
    /// A one-point guide with its vanishing point at `vp`.
    pub fn one_point(vp: Vec2) -> Self {
        Self::from_points(PerspectiveKind::OnePoint, vec![vp])
    }

    /// A two-point guide. The horizon runs through `a` and `b`.
    pub fn two_point(a: Vec2, b: Vec2) -> Self {
        Self::from_points(PerspectiveKind::TwoPoint, vec![a, b])
    }

    /// A three-point guide. The third point is conventionally the vertical (zenith or
    /// nadir) vanishing point, but it can be placed anywhere.
    pub fn three_point(a: Vec2, b: Vec2, c: Vec2) -> Self {
        Self::from_points(PerspectiveKind::ThreePoint, vec![a, b, c])
    }

    fn from_points(kind: PerspectiveKind, points: Vec<Vec2>) -> Self {
        debug_assert_eq!(points.len(), kind.point_count());
        Self {
            kind,
            points,
            density: DEFAULT_DENSITY,
            show_horizon: true,
            style: GuideStyle::default(),
        }
    }

    pub fn kind(&self) -> PerspectiveKind {
        self.kind
    }

    /// The vanishing points, in construction order.
    pub fn points(&self) -> &[Vec2] {
        &self.points
    }

    /// Moves vanishing point `index`. Returns `false` when `index` is out of range.
    pub fn set_point(&mut self, index: usize, p: Vec2) -> bool {
        match self.points.get_mut(index) {
            Some(slot) => {
                *slot = p;
                true
            }
            None => false,
        }
    }

    /// The horizon as `(point, unit direction)`. With one point it is horizontal
    /// through that point. Otherwise it runs through the first two points, and is
    /// horizontal through the first point if those two coincide.
    pub fn horizon(&self) -> (Vec2, Vec2) {
        let first = self.points[0];
        let dir = self
            .points
            .get(1)
            .and_then(|second| (*second - first).normalised())
            .unwrap_or(Vec2::new(1.0, 0.0));
        (first, dir)
    }

    /// Chooses the vanishing point whose line from `anchor` best matches `drag`.
    /// Lines are undirected, so a drag matches a line in either direction. Returns the
    /// unit direction from `anchor` to the chosen vanishing point, or `None` when the
    /// drag or every candidate is degenerate. Ties go to the earlier point.
    pub fn best_direction(&self, anchor: Vec2, drag: Vec2) -> Option<Vec2> {
        let drag = drag.normalised()?;
        let mut best: Option<(f64, Vec2)> = None;
        for vp in &self.points {
            let Some(dir) = (*vp - anchor).normalised() else {
                continue;
            };
            let score = dir.dot(drag).abs();
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, dir));
            }
        }
        best.map(|(_, dir)| dir)
    }
}

impl GuideRender for Perspective {
    fn style(&self) -> GuideStyle {
        self.style
    }

    fn lines(&self, viewport: Rect) -> Vec<Segment> {
        let density = self.density.max(1);
        let mut out = Vec::new();
        for vp in &self.points {
            for k in 0..density {
                let dir = Vec2::from_angle(PI * f64::from(k) / f64::from(density));
                out.extend(line_through(*vp, dir, viewport));
            }
        }
        if self.show_horizon {
            let (point, dir) = self.horizon();
            out.extend(line_through(point, dir, viewport));
        }
        out
    }
}
