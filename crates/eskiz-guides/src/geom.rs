//! Canvas-space primitives shared by the guides and the assist: vectors,
//! rectangles, segments with rectangle clipping, and stroke samples.
//!
//! Coordinates are canvas pixels with y growing downwards. Angles are radians
//! unless a function says otherwise. A positive angle turns from +x towards +y,
//! which is clockwise on screen.

use std::ops::{Add, Mul, Sub};

/// Tolerance below which a vector counts as zero.
pub(crate) const EPS: f64 = 1e-12;

/// Upper bound on the number of lines one guide family may produce.
pub(crate) const MAX_LINES: i64 = 100_000;

/// A point or vector in canvas space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    /// The origin.
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Unit vector at `radians`.
    pub fn from_angle(radians: f64) -> Self {
        let (s, c) = radians.sin_cos();
        Self::new(c, s)
    }

    pub fn dot(self, other: Vec2) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// Z component of the 3D cross product (signed area term).
    pub fn cross(self, other: Vec2) -> f64 {
        self.x * other.y - self.y * other.x
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn distance(self, other: Vec2) -> f64 {
        (self - other).length()
    }

    /// Unit vector in the same direction, or `None` when the vector is (nearly) zero.
    pub fn normalised(self) -> Option<Vec2> {
        let len = self.length();
        (len > EPS).then(|| self * (1.0 / len))
    }

    /// Rotates about the origin by `radians`.
    pub fn rotate(self, radians: f64) -> Vec2 {
        let (s, c) = radians.sin_cos();
        Vec2::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }

    /// Linear interpolation between `self` (t = 0) and `other` (t = 1).
    pub fn lerp(self, other: Vec2, t: f64) -> Vec2 {
        self + (other - self) * t
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, rhs: f64) -> Vec2 {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

/// Axis-aligned rectangle in canvas space, used as the guide viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            min: Vec2::new(x, y),
            max: Vec2::new(x + width, y + height),
        }
    }

    pub fn centre(self) -> Vec2 {
        self.min.lerp(self.max, 0.5)
    }

    pub fn diagonal(self) -> f64 {
        self.min.distance(self.max)
    }

    pub fn corners(self) -> [Vec2; 4] {
        [
            self.min,
            Vec2::new(self.max.x, self.min.y),
            self.max,
            Vec2::new(self.min.x, self.max.y),
        ]
    }

    pub fn contains(self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }
}

/// A finite line segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub a: Vec2,
    pub b: Vec2,
}

impl Segment {
    pub fn new(a: Vec2, b: Vec2) -> Self {
        Self { a, b }
    }

    pub fn length(self) -> f64 {
        self.a.distance(self.b)
    }

    /// Clips to `rect` (Liang-Barsky). `None` when nothing of the segment is inside.
    /// A line that only touches a corner or edge clips to a point, which is dropped
    /// because it has no direction.
    pub fn clip(self, rect: Rect) -> Option<Segment> {
        let clipped = self.clip_inner(rect)?;
        (clipped.length() > EPS).then_some(clipped)
    }

    fn clip_inner(self, rect: Rect) -> Option<Segment> {
        let d = self.b - self.a;
        let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
        let edges = [
            (-d.x, self.a.x - rect.min.x),
            (d.x, rect.max.x - self.a.x),
            (-d.y, self.a.y - rect.min.y),
            (d.y, rect.max.y - self.a.y),
        ];
        for (p, q) in edges {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    if t > t1 {
                        return None;
                    }
                    t0 = t0.max(t);
                } else {
                    if t < t0 {
                        return None;
                    }
                    t1 = t1.min(t);
                }
            }
        }
        Some(Segment::new(self.a + d * t0, self.a + d * t1))
    }
}

/// One input sample of a stroke: a canvas position and a pressure in `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub pos: Vec2,
    pub pressure: f64,
}

impl Sample {
    pub fn new(pos: Vec2, pressure: f64) -> Self {
        Self { pos, pressure }
    }

    /// The same sample at another position (pressure is kept).
    pub fn with_pos(self, pos: Vec2) -> Self {
        Self { pos, ..self }
    }
}

/// True for a finite, strictly positive grid spacing.
pub(crate) fn valid_spacing(spacing: f64) -> bool {
    spacing.is_finite() && spacing > 0.0
}

/// The full line through `point` with unit direction `dir`, clipped to `rect`.
pub(crate) fn line_through(point: Vec2, dir: Vec2, rect: Rect) -> Option<Segment> {
    let reach = rect.diagonal() + point.distance(rect.centre());
    Segment::new(point - dir * reach, point + dir * reach).clip(rect)
}

/// The half-line from `origin` along unit direction `dir`, clipped to `rect`.
pub(crate) fn ray_from(origin: Vec2, dir: Vec2, rect: Rect) -> Option<Segment> {
    let reach = rect.diagonal() + origin.distance(rect.centre());
    Segment::new(origin, origin + dir * reach).clip(rect)
}

/// Parallel lines with unit direction `dir`, spaced `spacing` apart along the normal.
/// Line `k` passes through `origin + normal * (k * spacing)`. Returns nothing for an
/// invalid spacing or when the family would exceed [`MAX_LINES`].
pub(crate) fn line_family(origin: Vec2, dir: Vec2, spacing: f64, rect: Rect) -> Vec<Segment> {
    if !valid_spacing(spacing) {
        return Vec::new();
    }
    let normal = Vec2::new(-dir.y, dir.x);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for corner in rect.corners() {
        let t = normal.dot(corner - origin) / spacing;
        lo = lo.min(t);
        hi = hi.max(t);
    }
    // Truncating casts are fine: the bounds are finite and clamped by MAX_LINES below.
    let (k0, k1) = (lo.floor() as i64, hi.ceil() as i64);
    if k1 - k0 > MAX_LINES {
        return Vec::new();
    }
    (k0..=k1)
        .filter_map(|k| {
            let base = origin + normal * (k as f64 * spacing);
            line_through(base, dir, rect)
        })
        .collect()
}
