//! Points, rectangles, quads and 3x3 projective matrices.

use std::ops::{Add, Mul, Sub};

/// A point in canvas coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// Four corners in the order top left, top right, bottom right, bottom left.
/// These are the images of the unit square corners (0, 0), (1, 0), (1, 1), (0, 1).
pub type Quad = [Point; 4];

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn dot(self, other: Point) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// The 2D cross product `x * other.y - y * other.x`.
    pub fn det(self, other: Point) -> f32 {
        self.x * other.y - self.y * other.x
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    pub fn distance(self, other: Point) -> f32 {
        (self - other).length()
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, rhs: Point) -> Point {
        Point::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, rhs: Point) -> Point {
        Point::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f32> for Point {
    type Output = Point;
    fn mul(self, rhs: f32) -> Point {
        Point::new(self.x * rhs, self.y * rhs)
    }
}

/// Integer pixel rectangle: top-left corner plus size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IRect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl IRect {
    /// Rectangle covering `[x0, x1) x [y0, y1)`. Reversed bounds give an empty rectangle.
    pub fn from_bounds(x0: usize, y0: usize, x1: usize, y1: usize) -> Self {
        Self {
            x: x0,
            y: y0,
            w: x1.saturating_sub(x0),
            h: y1.saturating_sub(y0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    pub fn x1(&self) -> usize {
        self.x + self.w
    }

    pub fn y1(&self) -> usize {
        self.y + self.h
    }
}

/// Continuous rectangle `[x0, x1] x [y0, y1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub const fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    /// The rectangle spanned by two corner points, in either order.
    pub fn from_corners(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }

    pub fn from_irect(r: IRect) -> Self {
        Self::new(r.x as f32, r.y as f32, r.x1() as f32, r.y1() as f32)
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn centre(&self) -> Point {
        Point::new((self.x0 + self.x1) * 0.5, (self.y0 + self.y1) * 0.5)
    }

    /// Corners in quad order: top left, top right, bottom right, bottom left.
    pub fn corners(&self) -> Quad {
        [
            Point::new(self.x0, self.y0),
            Point::new(self.x1, self.y0),
            Point::new(self.x1, self.y1),
            Point::new(self.x0, self.y1),
        ]
    }
}

/// Axis-aligned bounds of a quad.
pub fn quad_bounds(quad: &Quad) -> Rect {
    let min_x = quad.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
    let min_y = quad.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
    let max_x = quad.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
    let max_y = quad.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
    Rect::new(min_x, min_y, max_x, max_y)
}

/// A 3x3 projective matrix acting on column vectors `(x, y, 1)`.
/// Affine maps have bottom row `(0, 0, 1)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3 {
    /// Row-major entries `m[row][column]`.
    pub m: [[f32; 3]; 3],
}

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3 {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    pub fn translate(tx: f32, ty: f32) -> Self {
        Self {
            m: [[1.0, 0.0, tx], [0.0, 1.0, ty], [0.0, 0.0, 1.0]],
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Self {
            m: [[sx, 0.0, 0.0], [0.0, sy, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    /// Rotation by `deg` degrees, clockwise on screen (y points down).
    /// Multiples of 90 degrees use exact entries so that quarter turns are exact.
    pub fn rotate_deg(deg: f32) -> Self {
        let (sin, cos) = if deg.rem_euclid(90.0) == 0.0 {
            let quarter = (deg / 90.0).round() as i64;
            match quarter.rem_euclid(4) {
                0 => (0.0, 1.0),
                1 => (1.0, 0.0),
                2 => (0.0, -1.0),
                _ => (-1.0, 0.0),
            }
        } else {
            let r = deg.to_radians();
            (r.sin(), r.cos())
        };
        Self {
            m: [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    /// Applies `self` about `anchor`: `T(anchor) * self * T(-anchor)`.
    pub fn about(self, anchor: Point) -> Self {
        Self::translate(anchor.x, anchor.y) * self * Self::translate(-anchor.x, -anchor.y)
    }

    /// Maps a point, dividing by the projective w. None when w is zero.
    pub fn apply(&self, p: Point) -> Option<Point> {
        let m = &self.m;
        let x = m[0][0] * p.x + m[0][1] * p.y + m[0][2];
        let y = m[1][0] * p.x + m[1][1] * p.y + m[1][2];
        let w = m[2][0] * p.x + m[2][1] * p.y + m[2][2];
        if !w.is_finite() || w.abs() < 1e-12 {
            None
        } else {
            Some(Point::new(x / w, y / w))
        }
    }

    pub fn det(&self) -> f32 {
        let m = &self.m;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// The inverse, or None when the matrix is singular.
    pub fn inverse(&self) -> Option<Mat3> {
        let d = self.det();
        if !d.is_finite() || d.abs() < 1e-30 {
            return None;
        }
        let m = &self.m;
        let k = 1.0 / d;
        Some(Mat3 {
            m: [
                [
                    (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * k,
                    -(m[0][1] * m[2][2] - m[0][2] * m[2][1]) * k,
                    (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * k,
                ],
                [
                    -(m[1][0] * m[2][2] - m[1][2] * m[2][0]) * k,
                    (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * k,
                    -(m[0][0] * m[1][2] - m[0][2] * m[1][0]) * k,
                ],
                [
                    (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * k,
                    -(m[0][0] * m[2][1] - m[0][1] * m[2][0]) * k,
                    (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * k,
                ],
            ],
        })
    }
}

impl Mul for Mat3 {
    type Output = Mat3;

    /// Composition: `(a * b).apply(p) == a.apply(b.apply(p))`.
    fn mul(self, rhs: Mat3) -> Mat3 {
        let mut out = [[0.0f32; 3]; 3];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| self.m[i][k] * rhs.m[k][j]).sum();
            }
        }
        Mat3 { m: out }
    }
}

/// Heckbert's unit-square-to-quad map: `(0,0) -> p0`, `(1,0) -> p1`,
/// `(1,1) -> p2`, `(0,1) -> p3`. None when the quad is degenerate.
pub fn unit_square_to_quad(p: &Quad) -> Option<Mat3> {
    let d1 = p[1] - p[2];
    let d2 = p[3] - p[2];
    let sum = p[0] - p[1] + p[2] - p[3];
    let den = d1.det(d2);
    if !den.is_finite() || den.abs() < 1e-12 {
        return None;
    }
    let g = sum.det(d2) / den;
    let h = d1.det(sum) / den;
    let a = p[1] - p[0] + p[1] * g;
    let b = p[3] - p[0] + p[3] * h;
    let c = p[0];
    Some(Mat3 {
        m: [[a.x, b.x, c.x], [a.y, b.y, c.y], [g, h, 1.0]],
    })
}

/// Projective map from a source rectangle to a destination quad. The rectangle's corners
/// (top left, top right, bottom right, bottom left) map to the quad's corners in order.
pub fn rect_to_quad(rect: Rect, quad: &Quad) -> Option<Mat3> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let to_unit =
        Mat3::scale(1.0 / rect.width(), 1.0 / rect.height()) * Mat3::translate(-rect.x0, -rect.y0);
    Some(unit_square_to_quad(quad)? * to_unit)
}
