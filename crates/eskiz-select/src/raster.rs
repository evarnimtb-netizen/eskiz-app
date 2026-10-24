//! Shape rasterisation for selections: rectangles, ellipses, polygons and
//! freehand strokes with the drawn-path smoothing pass.

use crate::geom::{Point, Rect};
use crate::mask::Mask;

/// Minimum spacing between captured freehand samples, in pixels.
pub const FREEHAND_SPACING_PX: f32 = 2.0;

/// Supersampling per axis for polygon edges (4 x 4 sub-samples per pixel).
const SUB: usize = 4;

/// Rectangle with exact area coverage on pixel edges: pixels `[x0, x1) x [y0, y1)` are
/// fully covered and fractional edges get partial coverage.
pub fn rectangle(width: usize, height: usize, rect: Rect) -> Mask {
    let mut mask = Mask::empty(width, height);
    for y in 0..height {
        let cover_y = overlap(y as f32, rect.y0, rect.y1);
        for x in 0..width {
            let cover_x = overlap(x as f32, rect.x0, rect.x1);
            mask.data[y * width + x] = cover_x * cover_y;
        }
    }
    mask
}

fn overlap(p: f32, lo: f32, hi: f32) -> f32 {
    ((p + 1.0).min(hi) - p.max(lo)).clamp(0.0, 1.0)
}

/// Ellipse inscribed in `bbox`, with a one-pixel anti-aliased edge.
/// A pixel is inside when its centre satisfies the ellipse equation.
pub fn ellipse(width: usize, height: usize, bbox: Rect) -> Mask {
    let mut mask = Mask::empty(width, height);
    let (rx, ry) = (bbox.width() * 0.5, bbox.height() * 0.5);
    if rx <= 0.0 || ry <= 0.0 {
        return mask;
    }
    let (cx, cy) = (bbox.x0 + rx, bbox.y0 + ry);
    for y in 0..height {
        for x in 0..width {
            let dx = (x as f32 + 0.5 - cx) / rx;
            let dy = (y as f32 + 0.5 - cy) / ry;
            let radial = (dx * dx + dy * dy).sqrt();
            // Signed distance to the edge in pixels: (1 - radial) over how fast `radial` changes
            // per pixel here. Along the long axis it changes more slowly, so dividing by the
            // short radius alone gave a ramp several pixels wide at the tips.
            let grad = ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt() / radial.max(1e-6);
            let signed_px = (1.0 - radial) / grad.max(1e-6);
            mask.data[y * width + x] = (signed_px + 0.5).clamp(0.0, 1.0);
        }
    }
    mask
}

/// Closed polygon rasterised with even-odd fill and 4 x 4 supersampling.
/// The polygon closes from the last point back to the first. Fewer than three points give
/// an empty mask.
pub fn polygon(width: usize, height: usize, points: &[Point]) -> Mask {
    let mut mask = Mask::empty(width, height);
    if points.len() < 3 {
        return mask;
    }
    let weight = 1.0 / (SUB * SUB) as f32;
    let mut xs: Vec<f32> = Vec::new();
    for y in 0..height {
        for s in 0..SUB {
            let sample_y = y as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            crossings(points, sample_y, &mut xs);
            xs.sort_by(f32::total_cmp);
            for pair in 0..xs.len() / 2 {
                let (xa, xb) = (xs[2 * pair], xs[2 * pair + 1]);
                let x_lo = (xa.floor().max(0.0) as usize).min(width);
                let x_hi = (xb.ceil().max(0.0) as usize).min(width);
                for x in x_lo..x_hi {
                    let hits = (0..SUB)
                        .filter(|&i| {
                            let sample_x = x as f32 + (i as f32 + 0.5) / SUB as f32;
                            sample_x >= xa && sample_x < xb
                        })
                        .count();
                    mask.data[y * width + x] += hits as f32 * weight;
                }
            }
        }
    }
    for v in &mut mask.data {
        *v = v.clamp(0.0, 1.0);
    }
    mask
}

/// X positions where the closed polygon crosses the horizontal line at `sample_y`.
fn crossings(points: &[Point], sample_y: f32, out: &mut Vec<f32>) {
    for (p, q) in points.iter().zip(points.iter().cycle().skip(1)) {
        if (p.y <= sample_y && q.y > sample_y) || (q.y <= sample_y && p.y > sample_y) {
            out.push(p.x + (sample_y - p.y) * (q.x - p.x) / (q.y - p.y));
        }
    }
}

/// One smoothing pass over a drawn path: each interior vertex becomes
/// `0.25 v + 0.375 prev + 0.375 next`, reading the original neighbours. Endpoints are kept.
pub fn smooth_path(points: &[Point]) -> Vec<Point> {
    let n = points.len();
    let mut out = points.to_vec();
    if n < 3 {
        return out;
    }
    for (i, o) in out.iter_mut().enumerate().skip(1).take(n - 2) {
        *o = points[i] * 0.25 + points[i - 1] * 0.375 + points[i + 1] * 0.375;
    }
    out
}

/// A freehand stroke being drawn. Samples closer than the spacing to the last kept sample
/// are dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct FreehandStroke {
    points: Vec<Point>,
    min_spacing: f32,
}

impl Default for FreehandStroke {
    fn default() -> Self {
        Self::with_spacing(FREEHAND_SPACING_PX)
    }
}

impl FreehandStroke {
    pub fn with_spacing(min_spacing: f32) -> Self {
        Self {
            points: Vec::new(),
            min_spacing,
        }
    }

    /// Adds a pointer sample if it is far enough from the previous kept sample.
    pub fn push(&mut self, p: Point) {
        let keep = match self.points.last() {
            Some(last) => last.distance(p) >= self.min_spacing,
            None => true,
        };
        if keep {
            self.points.push(p);
        }
    }

    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Ends the stroke: smooths the captured path once, closes it and rasterises it.
    pub fn finish(&self, width: usize, height: usize) -> Mask {
        polygon(width, height, &smooth_path(&self.points))
    }
}
