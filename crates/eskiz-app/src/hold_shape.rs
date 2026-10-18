//! Hold to Shape: snap a freshly drawn stroke to a clean shape.
//!
//! Everything here is pure geometry on canvas-pixel points, so it is unit-tested without
//! a window. The app uses it like this:
//! 1. [`StillTimer`] notices the pointer has been still for 0.65 s at the end of a stroke.
//! 2. [`classify`] picks the best fit: line, arc, ellipse or circle, or a polyline,
//!    rectangle or triangle when there are corners.
//! 3. [`outline`] resamples the shape into dense points, which the stroke engine paints
//!    with the current brush.
//! 4. While the pen is still down, [`transform`] (scale and rotate about [`anchor`]) and
//!    [`constrain`] (15-degree lines, circles, squares) update the shape.
//! 5. After the pen lifts, "Edit Shape" shows [`handles`] (control points). [`drag_handle`]
//!    reshapes the shape as one is dragged, and [`hit_handle`] finds the one under a point.

use std::f64::consts::{PI, TAU};

/// A canvas-pixel point.
pub type P = (f64, f64);

/// The kind of shape a stroke was snapped to. Used in the "Shape created" banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Line,
    Arc,
    Ellipse,
    Circle,
    Rectangle,
    Triangle,
    /// A closed shape with four or more corners that is not a rectangle.
    Polygon,
    /// An open path with straight segments.
    Polyline,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Line => "Line",
            Kind::Arc => "Arc",
            Kind::Ellipse => "Ellipse",
            Kind::Circle => "Circle",
            Kind::Rectangle => "Rectangle",
            Kind::Triangle => "Triangle",
            Kind::Polygon => "Polygon",
            Kind::Polyline => "Polyline",
        }
    }
}

/// A geometric shape. Angles are radians.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Line { a: P, b: P },
    /// Circular arc from angle `a0` sweeping to `a1`.
    Arc { c: P, r: f64, a0: f64, a1: f64 },
    Ellipse { c: P, rx: f64, ry: f64, rot: f64 },
    Rect { c: P, w: f64, h: f64, rot: f64 },
    /// Triangle or polyline. `closed` joins the last point back to the first.
    Polygon { pts: Vec<P>, closed: bool },
}

/// A classification result.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub kind: Kind,
    pub shape: Shape,
}

/// Strokes shorter than this (canvas px) are never shapes.
const MIN_LENGTH: f64 = 12.0;
/// The most corners a polyline or polygon may have.
const MAX_CORNERS: usize = 10;

fn dist(a: P, b: P) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_dist(p: P, a: P, b: P) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= f64::EPSILON {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    dist(p, (a.0 + dx * t, a.1 + dy * t))
}

/// Drop consecutive points closer than `min_gap`.
fn dedupe(points: &[P], min_gap: f64) -> Vec<P> {
    let mut out: Vec<P> = Vec::with_capacity(points.len());
    for &p in points {
        if out.last().is_none_or(|&q| dist(p, q) >= min_gap) {
            out.push(p);
        }
    }
    out
}

fn path_length(pts: &[P]) -> f64 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Diagonal of the bounding box.
fn bbox_diag(pts: &[P]) -> f64 {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    (x1 - x0).hypot(y1 - y0)
}

/// Ramer-Douglas-Peucker simplification. The first and last points are kept.
fn simplify(pts: &[P], eps: f64) -> Vec<P> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((i, j)) = stack.pop() {
        let mut worst = (0.0f64, i);
        for (k, &p) in pts.iter().enumerate().take(j).skip(i + 1) {
            let d = segment_dist(p, pts[i], pts[j]);
            if d > worst.0 {
                worst = (d, k);
            }
        }
        if worst.0 > eps {
            keep[worst.1] = true;
            stack.push((i, worst.1));
            stack.push((worst.1, j));
        }
    }
    pts.iter()
        .zip(keep)
        .filter(|(_, k)| *k)
        .map(|(p, _)| *p)
        .collect()
}

/// Least-squares ellipse from the point covariance. Accurate for points spread evenly
/// around the outline, as a hand-drawn closed loop is. Returns `(centre, a, b, rotation)`
/// with `a >= b`.
fn fit_ellipse(pts: &[P]) -> Option<(P, f64, f64, f64)> {
    let n = pts.len() as f64;
    let (mx, my) = pts.iter().fold((0.0, 0.0), |(x, y), p| (x + p.0 / n, y + p.1 / n));
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &(x, y) in pts {
        let (dx, dy) = (x - mx, y - my);
        sxx += dx * dx / n;
        syy += dy * dy / n;
        sxy += dx * dy / n;
    }
    let tr = sxx + syy;
    let det = sxx * syy - sxy * sxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, (tr / 2.0 - disc).max(0.0));
    // For points spread evenly over an ellipse, variance along an axis is a^2 / 2.
    let a = (2.0 * l1).sqrt();
    let b = (2.0 * l2).sqrt();
    if b < 2.0 || a < 2.0 {
        return None;
    }
    let rot = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    Some(((mx, my), a, b, rot))
}

/// Mean and maximum relative error of the points against the ellipse, plus the largest
/// angular gap (radians) between them as seen from the centre.
fn ellipse_error(pts: &[P], c: P, a: f64, b: f64, rot: f64) -> (f64, f64, f64) {
    let (s, co) = rot.sin_cos();
    let mut angles: Vec<f64> = Vec::with_capacity(pts.len());
    let (mut sum, mut worst) = (0.0, 0.0f64);
    for &(x, y) in pts {
        let (dx, dy) = (x - c.0, y - c.1);
        let u = dx * co + dy * s;
        let v = -dx * s + dy * co;
        let q = ((u / a).powi(2) + (v / b).powi(2)).sqrt();
        let e = (q - 1.0).abs();
        sum += e;
        worst = worst.max(e);
        angles.push((v / b).atan2(u / a));
    }
    angles.sort_by(f64::total_cmp);
    let mut gap = angles.first().map_or(TAU, |first| first + TAU - angles.last().copied().unwrap_or(0.0));
    for w in angles.windows(2) {
        gap = gap.max(w[1] - w[0]);
    }
    (sum / pts.len() as f64, worst, gap)
}

/// Least-squares circle (Kasa fit). Returns `(centre, radius)`.
fn fit_circle(pts: &[P]) -> Option<(P, f64)> {
    let n = pts.len() as f64;
    let (mx, my) = pts.iter().fold((0.0, 0.0), |(x, y), p| (x + p.0 / n, y + p.1 / n));
    let (mut suu, mut svv, mut suv, mut suuu, mut svvv, mut suvv, mut svuu) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &(x, y) in pts {
        let (u, v) = (x - mx, y - my);
        suu += u * u;
        svv += v * v;
        suv += u * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-9 {
        return None;
    }
    let b1 = 0.5 * (suuu + suvv);
    let b2 = 0.5 * (svvv + svuu);
    let uc = (b1 * svv - b2 * suv) / det;
    let vc = (suu * b2 - suv * b1) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    if !r.is_finite() || r <= 0.0 {
        return None;
    }
    Some(((mx + uc, my + vc), r))
}

/// Are these four corners a rectangle (every corner close to 90 degrees)?
fn is_rectangle(v: &[P]) -> bool {
    (0..4).all(|i| {
        let a = (v[(i + 1) % 4].0 - v[i].0, v[(i + 1) % 4].1 - v[i].1);
        let b = (v[(i + 2) % 4].0 - v[(i + 1) % 4].0, v[(i + 2) % 4].1 - v[(i + 1) % 4].1);
        let (la, lb) = (a.0.hypot(a.1), b.0.hypot(b.1));
        la > 1e-9 && lb > 1e-9 && ((a.0 * b.0 + a.1 * b.1) / (la * lb)).abs() < 0.25
    })
}

/// Rectangle parameters from four corners in order.
fn rect_from_corners(v: &[P]) -> Shape {
    let c = (
        v.iter().map(|p| p.0).sum::<f64>() / 4.0,
        v.iter().map(|p| p.1).sum::<f64>() / 4.0,
    );
    let e0 = (v[1].0 - v[0].0, v[1].1 - v[0].1);
    let e1 = (v[2].0 - v[1].0, v[2].1 - v[1].1);
    let e2 = (v[3].0 - v[2].0, v[3].1 - v[2].1);
    let e3 = (v[0].0 - v[3].0, v[0].1 - v[3].1);
    // A rectangle looks the same after a half turn, so keep the angle in (-90, 90] degrees.
    let mut rot = e0.1.atan2(e0.0);
    if rot > PI / 2.0 {
        rot -= PI;
    } else if rot <= -PI / 2.0 {
        rot += PI;
    }
    Shape::Rect {
        c,
        w: (e0.0.hypot(e0.1) + e2.0.hypot(e2.1)) / 2.0,
        h: (e1.0.hypot(e1.1) + e3.0.hypot(e3.1)) / 2.0,
        rot,
    }
}

/// Wrap an angle difference into (-PI, PI].
fn wrap(a: f64) -> f64 {
    let mut a = a % TAU;
    if a > PI {
        a -= TAU;
    } else if a <= -PI {
        a += TAU;
    }
    a
}

/// Drop the points the pen wandered through while it was being held: the run at the end of the
/// stroke that stays within `3 * tol` of where the pen came to rest (`tol` is the base drift
/// tolerance the [`StillTimer`] used). One point of the cluster stays, and at least three
/// points are kept.
pub fn trim_hold_tail(points: &[P], tol: f64) -> Vec<P> {
    if points.is_empty() {
        return Vec::new();
    }
    let n = points.len().min(10);
    let tail = &points[points.len() - n..];
    let rest = (
        tail.iter().map(|p| p.0).sum::<f64>() / n as f64,
        tail.iter().map(|p| p.1).sum::<f64>() / n as f64,
    );
    let mut keep = points.len();
    while keep > 3 && dist(points[keep - 1], rest) <= 3.0 * tol {
        keep -= 1;
    }
    let end = (keep + 1).min(points.len());
    points[..end].to_vec()
}

/// Resample the path at an even spacing of `step` (a polyline through the points).
fn resample(pts: &[P], step: f64) -> Vec<P> {
    let mut out = vec![pts[0]];
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = dist(a, b);
        let mut at = step - carry;
        while at <= seg {
            let t = at / seg;
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            at += step;
        }
        carry = seg - (at - step);
    }
    let last = pts[pts.len() - 1];
    if dist(out[out.len() - 1], last) > step * 0.25 {
        out.push(last);
    }
    out
}

/// Smooth a hand-drawn (or jittery) stroke before its corners are looked for: resample it evenly,
/// then average each point with its neighbours. The ends stay where they are, and a real corner
/// (a sharp turn over several samples) survives well enough for the corner finder.
fn smooth(pts: &[P], diag: f64) -> Vec<P> {
    if pts.len() < 5 {
        return pts.to_vec();
    }
    let step = (diag / 100.0).max(1.0);
    let mut cur = resample(pts, step);
    if cur.len() < 5 {
        return cur;
    }
    for _ in 0..1 {
        let n = cur.len();
        let mut next = cur.clone();
        for (i, slot) in next.iter_mut().enumerate().take(n - 1).skip(1) {
            let k = 2.min(i).min(n - 1 - i);
            let (a, b) = (i - k, i + k);
            let cnt = (b - a + 1) as f64;
            *slot = cur[a..=b].iter().fold((0.0, 0.0), |(x, y), p| (x + p.0 / cnt, y + p.1 / cnt));
        }
        cur = next;
    }
    cur
}

/// How jittery the stroke is: the 90th percentile of the distance of its points from the
/// smoothed path (the few corners the smoothing rounds off do not count).
fn jitter_level(pts: &[P], smooth_pts: &[P]) -> f64 {
    if smooth_pts.len() < 2 {
        return 0.0;
    }
    let stride = (pts.len() / 300).max(1);
    let mut devs: Vec<f64> = pts
        .iter()
        .step_by(stride)
        .map(|&p| smooth_pts.windows(2).map(|w| segment_dist(p, w[0], w[1])).fold(f64::MAX, f64::min))
        .collect();
    devs.sort_by(f64::total_cmp);
    devs[(devs.len() * 9 / 10).min(devs.len() - 1)]
}

/// Snap a stroke to the best shape, or `None` when it is not clean enough (a scribble,
/// too short, or too irregular).
pub fn classify(raw: &[P]) -> Option<Fit> {
    let pts = dedupe(raw, 0.5);
    if pts.len() < 3 {
        return None;
    }
    let len = path_length(&pts);
    let diag = bbox_diag(&pts);
    if len < MIN_LENGTH || diag < 8.0 {
        return None;
    }
    // Jitter adds corners. The stroke is smoothed to measure that jitter and to judge how round a
    // loop is; corner positions come from the raw points, found with a tolerance that grows with it.
    let sm = smooth(&pts, diag);
    let noise = jitter_level(&pts, &sm);
    let first = pts[0];
    let last = pts[pts.len() - 1];
    let gap = dist(first, last);
    let closed = gap < 0.15 * diag;
    let eps = (0.035 * diag).max(3.5).max(2.5 * noise);
    // The corners of a closed loop. A loop with only a few corners is a polygon even when it
    // is round enough to pass for an ellipse (a pentagon).
    let ring_verts = closed.then(|| ring_vertices(&pts, eps));

    if !closed {
        // Straight line: the ends are nearly the whole path length apart.
        // The smoothed length: jitter makes the raw path longer than the line it wanders along.
        if gap >= 0.9 * path_length(&sm) {
            let tol = (0.03 * gap).max(3.0).max(2.5 * noise);
            let dev = pts.iter().map(|&p| segment_dist(p, first, last)).fold(0.0, f64::max);
            if dev <= tol {
                return Some(Fit {
                    kind: Kind::Line,
                    shape: Shape::Line { a: first, b: last },
                });
            }
        }
    } else if let Some((c, a, b, rot)) = fit_ellipse(&pts) {
        // Smooth closed loop: an ellipse, or a circle when it is nearly round.
        let (mean, worst, gap_angle) = ellipse_error(&sm, c, a, b, rot);
        let few_corners = ring_verts.as_ref().is_some_and(|v| (3..=6).contains(&v.len()));
        if mean <= 0.06 && worst <= 0.25 && gap_angle <= 1.3 && !(few_corners && mean > 0.03) {
            let (kind, shape) = if b / a >= 0.85 {
                let r = (a + b) / 2.0;
                (Kind::Circle, Shape::Ellipse { c, rx: r, ry: r, rot: 0.0 })
            } else {
                (Kind::Ellipse, Shape::Ellipse { c, rx: a, ry: b, rot })
            };
            return Some(Fit { kind, shape });
        }
    }

    if let Some(verts) = ring_verts {
        match verts.len() {
            3 => Some(Fit {
                kind: Kind::Triangle,
                shape: Shape::Polygon {
                    pts: verts,
                    closed: true,
                },
            }),
            4 if is_rectangle(&verts) => Some(Fit {
                kind: Kind::Rectangle,
                shape: rect_from_corners(&verts),
            }),
            n if (4..=MAX_CORNERS).contains(&n) => Some(Fit {
                kind: Kind::Polygon,
                shape: Shape::Polygon {
                    pts: verts,
                    closed: true,
                },
            }),
            _ => None,
        }
    } else {
        // Open curve: an arc if it is circular, otherwise a polyline with few corners.
        if let Some((c, r)) = fit_circle(&pts) {
            let worst = pts.iter().map(|&p| (dist(p, c) - r).abs()).fold(0.0, f64::max);
            let a0 = (first.1 - c.1).atan2(first.0 - c.0);
            let sweep: f64 = pts
                .windows(2)
                .map(|w| {
                    let t0 = (w[0].1 - c.1).atan2(w[0].0 - c.0);
                    let t1 = (w[1].1 - c.1).atan2(w[1].0 - c.0);
                    wrap(t1 - t0)
                })
                .sum();
            if worst <= (0.04 * r).max(3.0).max(2.5 * noise) && r <= 3.0 * diag && sweep.abs() >= 25f64.to_radians() {
                return Some(Fit {
                    kind: Kind::Arc,
                    shape: Shape::Arc {
                        c,
                        r,
                        a0,
                        a1: a0 + sweep,
                    },
                });
            }
        }
        let verts = simplify(&pts, eps);
        if (3..=MAX_CORNERS).contains(&verts.len()) {
            return Some(Fit {
                kind: Kind::Polyline,
                shape: Shape::Polygon {
                    pts: verts,
                    closed: false,
                },
            });
        }
        None
    }
}

/// The corners of a closed loop: it is simplified starting from the point farthest from its
/// start, so the simplification does not split a corner.
fn ring_vertices(pts: &[P], eps: f64) -> Vec<P> {
    let first = pts[0];
    let far = (0..pts.len())
        .max_by(|&i, &j| dist(pts[i], first).total_cmp(&dist(pts[j], first)))
        .unwrap_or(0);
    let mut ring: Vec<P> = pts[far..].iter().chain(pts[..far].iter()).copied().collect();
    ring.push(pts[far]);
    let mut verts = simplify(&ring, eps);
    verts.pop();
    // Two corners closer than this are one corner seen through jitter.
    while verts.len() > 3 {
        let n = verts.len();
        match (0..n).min_by(|&i, &j| dist(verts[i], verts[(i + 1) % n]).total_cmp(&dist(verts[j], verts[(j + 1) % n]))) {
            Some(i) if dist(verts[i], verts[(i + 1) % n]) < 1.5 * eps => {
                let (a, b) = (verts[i], verts[(i + 1) % n]);
                verts[i] = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                verts.remove((i + 1) % n);
            }
            _ => break,
        }
    }
    // The loop's start may sit in the middle of an edge: drop corners that barely turn.
    let barely = 25f64.to_radians();
    while verts.len() > 3 {
        let n = verts.len();
        let turn = |i: usize| {
            let (a, b, c) = (verts[(i + n - 1) % n], verts[i], verts[(i + 1) % n]);
            wrap((c.1 - b.1).atan2(c.0 - b.0) - (b.1 - a.1).atan2(b.0 - a.0)).abs()
        };
        match (0..n).min_by(|&i, &j| turn(i).total_cmp(&turn(j))) {
            Some(i) if turn(i) < barely => {
                verts.remove(i);
            }
            _ => break,
        }
    }
    verts
}

/// The point a transform pivots about: the first point of a line, the centre of
/// everything else, and the centroid of a polygon.
pub fn anchor(shape: &Shape) -> P {
    match shape {
        Shape::Line { a, .. } => *a,
        Shape::Arc { c, .. } | Shape::Ellipse { c, .. } | Shape::Rect { c, .. } => *c,
        Shape::Polygon { pts, .. } => {
            let n = pts.len().max(1) as f64;
            (
                pts.iter().map(|p| p.0).sum::<f64>() / n,
                pts.iter().map(|p| p.1).sum::<f64>() / n,
            )
        }
    }
}

/// Scale by `scale` and rotate by `rot` radians, both about `pivot`.
pub fn transform(shape: &Shape, pivot: P, scale: f64, rot: f64) -> Shape {
    let (s, c) = rot.sin_cos();
    let map = |p: P| {
        let (dx, dy) = ((p.0 - pivot.0) * scale, (p.1 - pivot.1) * scale);
        (pivot.0 + dx * c - dy * s, pivot.1 + dx * s + dy * c)
    };
    match shape {
        Shape::Line { a, b } => Shape::Line { a: map(*a), b: map(*b) },
        Shape::Arc { c: centre, r, a0, a1 } => Shape::Arc {
            c: map(*centre),
            r: r * scale,
            a0: a0 + rot,
            a1: a1 + rot,
        },
        Shape::Ellipse { c: centre, rx, ry, rot: r0 } => Shape::Ellipse {
            c: map(*centre),
            rx: rx * scale,
            ry: ry * scale,
            rot: r0 + rot,
        },
        Shape::Rect { c: centre, w, h, rot: r0 } => Shape::Rect {
            c: map(*centre),
            w: w * scale,
            h: h * scale,
            rot: r0 + rot,
        },
        Shape::Polygon { pts, closed } => Shape::Polygon {
            pts: pts.iter().map(|&p| map(p)).collect(),
            closed: *closed,
        },
    }
}

/// The second-finger / Shift constraint. Lines snap to 15-degree steps about their first
/// point, ellipses become circles, and rectangles become squares. Others are unchanged.
pub fn constrain(shape: &Shape) -> Shape {
    match shape {
        Shape::Line { a, b } => {
            let len = dist(*a, *b);
            let step = 15f64.to_radians();
            let ang = (b.1 - a.1).atan2(b.0 - a.0);
            let snapped = (ang / step).round() * step;
            Shape::Line {
                a: *a,
                b: (a.0 + len * snapped.cos(), a.1 + len * snapped.sin()),
            }
        }
        Shape::Ellipse { c, rx, ry, rot } => {
            let r = (rx + ry) / 2.0;
            Shape::Ellipse {
                c: *c,
                rx: r,
                ry: r,
                rot: *rot,
            }
        }
        Shape::Rect { c, w, h, rot } => {
            let s = (w + h) / 2.0;
            Shape::Rect {
                c: *c,
                w: s,
                h: s,
                rot: *rot,
            }
        }
        other => other.clone(),
    }
}

/// Corner points or sampled outline, as a polyline, before resampling.
fn outline_points(shape: &Shape) -> (Vec<P>, bool) {
    match shape {
        Shape::Line { a, b } => (vec![*a, *b], false),
        Shape::Arc { c, r, a0, a1 } => {
            let n = 360;
            let pts = (0..=n)
                .map(|i| {
                    let t = a0 + (a1 - a0) * i as f64 / n as f64;
                    (c.0 + r * t.cos(), c.1 + r * t.sin())
                })
                .collect();
            (pts, false)
        }
        Shape::Ellipse { c, rx, ry, rot } => {
            let (s, co) = rot.sin_cos();
            let n = 720;
            let pts = (0..n)
                .map(|i| {
                    let t = TAU * i as f64 / n as f64;
                    let (x, y) = (rx * t.cos(), ry * t.sin());
                    (c.0 + x * co - y * s, c.1 + x * s + y * co)
                })
                .collect();
            (pts, true)
        }
        Shape::Rect { c, w, h, rot } => {
            let (s, co) = rot.sin_cos();
            let (hw, hh) = (w / 2.0, h / 2.0);
            let pts = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)]
                .iter()
                .map(|&(x, y)| (c.0 + x * co - y * s, c.1 + x * s + y * co))
                .collect();
            (pts, true)
        }
        Shape::Polygon { pts, closed } => (pts.clone(), *closed),
    }
}

/// Dense outline points, spaced about `step` canvas pixels apart. Closed shapes end on
/// their first point again.
pub fn outline(shape: &Shape, step: f64) -> Vec<P> {
    let step = step.max(0.25);
    let (pts, closed) = outline_points(shape);
    if pts.len() < 2 {
        return pts;
    }
    let segments = if closed { pts.len() } else { pts.len() - 1 };
    let mut out = vec![pts[0]];
    for i in 0..segments {
        let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
        let n = (dist(a, b) / step).ceil().max(1.0) as usize;
        for k in 1..=n {
            let t = k as f64 / n as f64;
            out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
        }
    }
    out
}

// ----- Edit Shape handles -----

/// What dragging a handle does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleKind {
    /// A control point: an end, corner, vertex or radius.
    Point,
    /// Moves the whole shape.
    Move,
}

/// A control point of a shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handle {
    pub pos: P,
    pub kind: HandleKind,
}

fn pt(pos: P) -> Handle {
    Handle { pos, kind: HandleKind::Point }
}

fn mv(pos: P) -> Handle {
    Handle { pos, kind: HandleKind::Move }
}

fn rotate(v: P, rot: f64) -> P {
    let (s, c) = rot.sin_cos();
    (v.0 * c - v.1 * s, v.0 * s + v.1 * c)
}

fn add(a: P, b: P) -> P {
    (a.0 + b.0, a.1 + b.1)
}

fn arc_point(c: P, r: f64, a: f64) -> P {
    (c.0 + r * a.cos(), c.1 + r * a.sin())
}

/// The handles of a shape, in a fixed order per type:
/// * line: start, end, then a move handle at the middle;
/// * arc: start, end, and a point on the arc (its bulge);
/// * ellipse and circle: a move handle at the centre, then right, bottom, left and top;
/// * rectangle: a move handle at the centre, then the four corners clockwise from top left;
/// * polygon and polyline: every vertex, then a move handle at the centroid.
pub fn handles(shape: &Shape) -> Vec<Handle> {
    match shape {
        Shape::Line { a, b } => vec![pt(*a), pt(*b), mv(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0))],
        Shape::Arc { c, r, a0, a1 } => vec![
            pt(arc_point(*c, *r, *a0)),
            pt(arc_point(*c, *r, *a1)),
            pt(arc_point(*c, *r, (a0 + a1) / 2.0)),
        ],
        Shape::Ellipse { c, rx, ry, rot } => {
            let at = |x: f64, y: f64| add(*c, rotate((x, y), *rot));
            vec![mv(*c), pt(at(*rx, 0.0)), pt(at(0.0, *ry)), pt(at(-rx, 0.0)), pt(at(0.0, -ry))]
        }
        Shape::Rect { c, w, h, rot } => {
            let (hw, hh) = (w / 2.0, h / 2.0);
            let mut v = vec![mv(*c)];
            v.extend([(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].map(|k| pt(add(*c, rotate(k, *rot)))));
            v
        }
        Shape::Polygon { pts, .. } => {
            let mut v: Vec<Handle> = pts.iter().map(|&p| pt(p)).collect();
            v.push(mv(anchor(shape)));
            v
        }
    }
}

/// The circular arc that starts at `p0`, passes through `pm` and ends at `p1`. `None` when
/// the three points are in a line (or two coincide).
pub fn arc_from_three(p0: P, pm: P, p1: P) -> Option<Shape> {
    let d = 2.0 * (p0.0 * (pm.1 - p1.1) + pm.0 * (p1.1 - p0.1) + p1.0 * (p0.1 - pm.1));
    if d.abs() < 1e-6 {
        return None;
    }
    let sq = |p: P| p.0 * p.0 + p.1 * p.1;
    let ux = (sq(p0) * (pm.1 - p1.1) + sq(pm) * (p1.1 - p0.1) + sq(p1) * (p0.1 - pm.1)) / d;
    let uy = (sq(p0) * (p1.0 - pm.0) + sq(pm) * (p0.0 - p1.0) + sq(p1) * (pm.0 - p0.0)) / d;
    let c = (ux, uy);
    let r = dist(c, p0);
    if !r.is_finite() || !(1.0..=1.0e6).contains(&r) {
        return None;
    }
    let ang = |p: P| (p.1 - c.1).atan2(p.0 - c.0);
    let (a0, am, a1) = (ang(p0), ang(pm), ang(p1));
    // Sweep the short way round if that passes the middle point, else the long way.
    let short = wrap(a1 - a0);
    let t = wrap(am - a0);
    let passes = if short >= 0.0 { (0.0..=short).contains(&t) } else { (short..=0.0).contains(&t) };
    let sweep = if passes {
        short
    } else if short >= 0.0 {
        short - TAU
    } else {
        short + TAU
    };
    Some(Shape::Arc { c, r, a0, a1: a0 + sweep })
}

/// The shape after handle `index` (see [`handles`]) is dragged to `to`. An out-of-range index,
/// or a drag that would make the shape degenerate, returns the shape unchanged.
pub fn drag_handle(shape: &Shape, index: usize, to: P) -> Shape {
    const MIN: f64 = 2.0;
    let unchanged = || shape.clone();
    match shape {
        Shape::Line { a, b } => match index {
            0 => Shape::Line { a: to, b: *b },
            1 => Shape::Line { a: *a, b: to },
            2 => {
                let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
                let d = (to.0 - mid.0, to.1 - mid.1);
                Shape::Line { a: add(*a, d), b: add(*b, d) }
            }
            _ => unchanged(),
        },
        Shape::Arc { .. } => {
            if index >= 3 {
                return unchanged();
            }
            let h = handles(shape);
            // start, middle, end
            let mut p = [h[0].pos, h[2].pos, h[1].pos];
            p[[0, 2, 1][index]] = to;
            arc_from_three(p[0], p[1], p[2]).unwrap_or_else(unchanged)
        }
        Shape::Ellipse { c, rx, ry, rot } => {
            if index == 0 {
                return Shape::Ellipse { c: to, rx: *rx, ry: *ry, rot: *rot };
            }
            if !(1..=4).contains(&index) {
                return unchanged();
            }
            let local = rotate((to.0 - c.0, to.1 - c.1), -rot);
            let circle = (rx - ry).abs() < 1e-9;
            let (mut nx, mut ny) = (*rx, *ry);
            if index % 2 == 1 {
                nx = local.0.abs().max(MIN);
                if circle {
                    ny = nx;
                }
            } else {
                ny = local.1.abs().max(MIN);
                if circle {
                    nx = ny;
                }
            }
            Shape::Ellipse { c: *c, rx: nx, ry: ny, rot: *rot }
        }
        Shape::Rect { c: _, w: _, h: _, rot } => {
            if index == 0 {
                if let Shape::Rect { w, h, rot, .. } = shape {
                    return Shape::Rect { c: to, w: *w, h: *h, rot: *rot };
                }
            }
            if !(1..=4).contains(&index) {
                return unchanged();
            }
            let corners = handles(shape);
            // The opposite corner stays put.
            let fixed = corners[1 + (index - 1 + 2) % 4].pos;
            let d = rotate((to.0 - fixed.0, to.1 - fixed.1), -rot);
            let (nw, nh) = (d.0.abs().max(MIN), d.1.abs().max(MIN));
            let sx = if d.0 < 0.0 { -1.0 } else { 1.0 };
            let sy = if d.1 < 0.0 { -1.0 } else { 1.0 };
            let centre = add(fixed, rotate((sx * nw / 2.0, sy * nh / 2.0), *rot));
            Shape::Rect { c: centre, w: nw, h: nh, rot: *rot }
        }
        Shape::Polygon { pts, closed } => {
            let n = pts.len();
            if index < n {
                let mut next = pts.clone();
                next[index] = to;
                Shape::Polygon { pts: next, closed: *closed }
            } else if index == n {
                let at = anchor(shape);
                let d = (to.0 - at.0, to.1 - at.1);
                Shape::Polygon { pts: pts.iter().map(|&p| add(p, d)).collect(), closed: *closed }
            } else {
                unchanged()
            }
        }
    }
}

/// The handle nearest to `p` within `radius`, preferring control points over the move handle
/// when they overlap.
pub fn hit_handle(shape: &Shape, p: P, radius: f64) -> Option<usize> {
    let rank = |h: &Handle| u8::from(h.kind == HandleKind::Move);
    handles(shape)
        .iter()
        .enumerate()
        .filter(|(_, h)| dist(h.pos, p) <= radius)
        .min_by(|(_, a), (_, b)| (rank(a), dist(a.pos, p)).partial_cmp(&(rank(b), dist(b.pos, p))).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

/// The Hold to Shape trigger: the pen stays put for `secs`, give or take drift.
///
/// A glass tablet lets the pen creep and jitter a little during the hold, so a strict standstill
/// (the position never leaves a small circle around where it stopped) restarts again and again.
/// This keeps the samples of the last `secs` and fires when they all lie within a tolerance of
/// their centre. The tolerance is the caller's base (a few screen points, see
/// [`StillTimer::update_with_tolerance`]) and grows with the measured jitter, up to three times
/// the base.
#[derive(Clone, Debug)]
pub struct StillTimer {
    samples: std::collections::VecDeque<(f64, P)>,
    since: f64,
    fired: bool,
    radius: f64,
    secs: f64,
}

impl StillTimer {
    pub fn new(p: P, t: f64) -> Self {
        // Hold still for 0.65 s.
        Self::with_secs(p, t, 0.65)
    }

    /// A timer that fires after `secs` of stillness (the Preferences slider).
    pub fn with_secs(p: P, t: f64, secs: f64) -> Self {
        let mut samples = std::collections::VecDeque::new();
        samples.push_back((t, p));
        Self { samples, since: t, fired: false, radius: 6.0, secs }
    }

    /// Feed the pointer position at time `t`, with the default tolerance (6 canvas px). Returns
    /// true once, when the pointer has been still long enough.
    pub fn update(&mut self, p: P, t: f64) -> bool {
        self.update_with_tolerance(p, t, self.radius)
    }

    /// As [`StillTimer::update`], with the base tolerance `tol` in canvas px (the caller converts
    /// from screen points with the zoom). Jitter in the window widens it, up to `3 * tol`.
    pub fn update_with_tolerance(&mut self, p: P, t: f64, tol: f64) -> bool {
        self.samples.push_back((t, p));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|&(t0, _)| t0 < t - self.secs) {
            self.samples.pop_front();
        }
        let n = self.samples.len() as f64;
        let (cx, cy) = self.samples.iter().fold((0.0, 0.0), |(x, y), &(_, q)| (x + q.0 / n, y + q.1 / n));
        let spread = self.samples.iter().map(|&(_, q)| dist(q, (cx, cy))).fold(0.0, f64::max);
        // Measured jitter: the mean step between consecutive samples in the window.
        let steps: Vec<f64> = self.samples.iter().zip(self.samples.iter().skip(1)).map(|(a, b)| dist(a.1, b.1)).collect();
        let jitter = if steps.is_empty() { 0.0 } else { steps.iter().sum::<f64>() / steps.len() as f64 };
        let allowed = (4.0 * jitter).clamp(tol, 3.0 * tol);
        if spread > allowed {
            self.samples.clear();
            self.samples.push_back((t, p));
            self.since = t;
            self.fired = false;
            return false;
        }
        if !self.fired && t - self.since >= self.secs {
            self.fired = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic jitter in [-amp, amp].
    fn jitter(i: usize, amp: f64) -> f64 {
        let x = ((i as u64).wrapping_mul(2_654_435_761) % 1000) as f64 / 1000.0;
        (x * 2.0 - 1.0) * amp
    }

    fn ellipse_pts(c: P, rx: f64, ry: f64, rot: f64, n: usize) -> Vec<P> {
        let (s, co) = rot.sin_cos();
        (0..n)
            .map(|i| {
                let t = TAU * i as f64 / n as f64;
                let (x, y) = (rx * t.cos(), ry * t.sin());
                (c.0 + x * co - y * s, c.1 + x * s + y * co)
            })
            .collect()
    }

    fn with_ring(pts: Vec<P>) -> Vec<P> {
        let mut v = pts.clone();
        v.push(pts[0]);
        v
    }

    fn rect_pts(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<P> {
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        let mut out = Vec::new();
        for i in 0..4 {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            let n = (dist(a, b) / 4.0).round() as usize;
            for k in 0..n {
                let t = k as f64 / n as f64;
                out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            }
        }
        out.push(corners[0]);
        out
    }

    #[test]
    fn noisy_straight_stroke_is_a_line() {
        let pts: Vec<P> = (0..60)
            .map(|i| {
                let t = i as f64 / 59.0;
                (20.0 + 300.0 * t + jitter(i, 1.0), 40.0 + 120.0 * t + jitter(i + 7, 1.0))
            })
            .collect();
        let fit = classify(&pts).expect("line");
        assert_eq!(fit.kind, Kind::Line);
        match fit.shape {
            Shape::Line { a, b } => {
                assert!(dist(a, (20.0, 40.0)) < 3.0);
                assert!(dist(b, (320.0, 160.0)) < 3.0);
            }
            other => panic!("expected a line, got {other:?}"),
        }
    }

    #[test]
    fn closed_ellipse_is_an_ellipse_with_its_axes() {
        let pts = with_ring(ellipse_pts((500.0, 400.0), 200.0, 100.0, 0.0, 160));
        let fit = classify(&pts).expect("ellipse");
        assert_eq!(fit.kind, Kind::Ellipse);
        match fit.shape {
            Shape::Ellipse { c, rx, ry, .. } => {
                assert!(dist(c, (500.0, 400.0)) < 3.0);
                assert!((rx - 200.0).abs() < 8.0, "rx {rx}");
                assert!((ry - 100.0).abs() < 8.0, "ry {ry}");
            }
            other => panic!("expected an ellipse, got {other:?}"),
        }
    }

    #[test]
    fn rotated_ellipse_keeps_its_rotation() {
        let pts = with_ring(ellipse_pts((300.0, 300.0), 220.0, 90.0, 0.5, 200));
        let fit = classify(&pts).expect("rotated ellipse");
        assert_eq!(fit.kind, Kind::Ellipse);
        if let Shape::Ellipse { rx, ry, rot, .. } = fit.shape {
            assert!((rx - 220.0).abs() < 10.0 && (ry - 90.0).abs() < 10.0, "{rx} {ry}");
            // The major axis is the same line as 0.5 rad, modulo PI.
            let d = (rot - 0.5).rem_euclid(PI);
            assert!(d < 0.05 || PI - d < 0.05, "rot {rot}");
        } else {
            panic!("not an ellipse");
        }
    }

    #[test]
    fn closed_round_stroke_is_a_circle() {
        let pts = with_ring(ellipse_pts((250.0, 250.0), 80.0, 78.0, 0.0, 140));
        let fit = classify(&pts).expect("circle");
        assert_eq!(fit.kind, Kind::Circle);
        if let Shape::Ellipse { rx, ry, .. } = fit.shape {
            assert!((rx - 79.0).abs() < 4.0 && (rx - ry).abs() < 1e-9);
        } else {
            panic!("circle should be an ellipse with equal radii");
        }
    }

    #[test]
    fn closed_rectangle_is_a_rectangle() {
        let pts = rect_pts(100.0, 100.0, 300.0, 200.0);
        let fit = classify(&pts).expect("rectangle");
        assert_eq!(fit.kind, Kind::Rectangle);
        if let Shape::Rect { c, w, h, rot } = fit.shape {
            assert!(dist(c, (200.0, 150.0)) < 3.0);
            assert!((w - 200.0).abs() < 5.0 && (h - 100.0).abs() < 5.0, "{w} x {h}");
            assert!(rot.abs() < 0.05);
        } else {
            panic!("not a rectangle");
        }
    }

    #[test]
    fn closed_three_corner_stroke_is_a_triangle() {
        let mut pts = Vec::new();
        let corners = [(100.0, 300.0), (300.0, 300.0), (200.0, 120.0), (100.0, 300.0)];
        for w in corners.windows(2) {
            let n = (dist(w[0], w[1]) / 4.0).round() as usize;
            for k in 0..n {
                let t = k as f64 / n as f64;
                pts.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
            }
        }
        pts.push((100.0, 300.0));
        let fit = classify(&pts).expect("triangle");
        assert_eq!(fit.kind, Kind::Triangle);
        if let Shape::Polygon { pts, closed } = fit.shape {
            assert!(closed && pts.len() == 3);
        } else {
            panic!("triangle should be a polygon");
        }
    }

    #[test]
    fn open_half_circle_is_an_arc() {
        let pts: Vec<P> = (0..=60)
            .map(|i| {
                let t = PI * i as f64 / 60.0;
                (400.0 + 150.0 * t.cos(), 300.0 + 150.0 * t.sin())
            })
            .collect();
        let fit = classify(&pts).expect("arc");
        assert_eq!(fit.kind, Kind::Arc);
        if let Shape::Arc { c, r, .. } = fit.shape {
            assert!(dist(c, (400.0, 300.0)) < 3.0 && (r - 150.0).abs() < 3.0);
        } else {
            panic!("not an arc");
        }
    }

    #[test]
    fn scribble_and_tiny_strokes_are_not_shapes() {
        let scribble: Vec<P> = (0..120)
            .map(|i| (100.0 + i as f64 * 3.0, 200.0 + jitter(i * 13, 90.0)))
            .collect();
        assert!(classify(&scribble).is_none());
        let tiny = [(10.0, 10.0), (11.0, 10.5), (12.0, 11.0)];
        assert!(classify(&tiny).is_none());
    }

    #[test]
    fn transform_scales_and_rotates_about_the_pivot() {
        let e = Shape::Ellipse {
            c: (100.0, 100.0),
            rx: 50.0,
            ry: 20.0,
            rot: 0.0,
        };
        let pivot = anchor(&e);
        assert_eq!(pivot, (100.0, 100.0));
        match transform(&e, pivot, 2.0, PI / 2.0) {
            Shape::Ellipse { c, rx, ry, rot } => {
                assert!(dist(c, pivot) < 1e-9);
                assert!((rx - 100.0).abs() < 1e-9 && (ry - 40.0).abs() < 1e-9);
                assert!((rot - PI / 2.0).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
        let line = Shape::Line {
            a: (0.0, 0.0),
            b: (10.0, 0.0),
        };
        match transform(&line, (0.0, 0.0), 1.0, PI / 2.0) {
            Shape::Line { b, .. } => assert!(dist(b, (0.0, 10.0)) < 1e-9),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn constraints_snap_lines_ellipses_and_rectangles() {
        let line = Shape::Line {
            a: (0.0, 0.0),
            b: (100.0, 100.0 * 20f64.to_radians().tan()),
        };
        match constrain(&line) {
            Shape::Line { a, b } => {
                let ang = (b.1 - a.1).atan2(b.0 - a.0);
                assert!((ang - 15f64.to_radians()).abs() < 1e-9);
                assert!((dist(a, b) - dist((0.0, 0.0), (100.0, 100.0 * 20f64.to_radians().tan()))).abs() < 1e-9);
            }
            other => panic!("{other:?}"),
        }
        match constrain(&Shape::Ellipse {
            c: (0.0, 0.0),
            rx: 30.0,
            ry: 10.0,
            rot: 0.3,
        }) {
            Shape::Ellipse { rx, ry, .. } => assert!((rx - ry).abs() < 1e-9 && (rx - 20.0).abs() < 1e-9),
            other => panic!("{other:?}"),
        }
        match constrain(&Shape::Rect {
            c: (0.0, 0.0),
            w: 40.0,
            h: 10.0,
            rot: 0.0,
        }) {
            Shape::Rect { w, h, .. } => assert!((w - h).abs() < 1e-9),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn outline_is_dense_and_closed_shapes_return_to_start() {
        let rect = Shape::Rect {
            c: (50.0, 50.0),
            w: 40.0,
            h: 20.0,
            rot: 0.0,
        };
        let pts = outline(&rect, 2.0);
        assert!(pts.len() >= 60, "{}", pts.len());
        assert!(dist(pts[0], pts[pts.len() - 1]) < 1e-9);
        let line = Shape::Line {
            a: (0.0, 0.0),
            b: (20.0, 0.0),
        };
        let pts = outline(&line, 2.0);
        assert_eq!(pts.len(), 11);
        assert!(dist(pts[10], (20.0, 0.0)) < 1e-9);
    }

    fn polyline_pts(corners: &[P]) -> Vec<P> {
        let mut pts = Vec::new();
        for w in corners.windows(2) {
            let n = (dist(w[0], w[1]) / 4.0).round().max(1.0) as usize;
            for k in 0..n {
                let t = k as f64 / n as f64;
                pts.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
            }
        }
        pts.push(corners[corners.len() - 1]);
        pts
    }

    #[test]
    fn open_zigzag_is_a_polyline_with_its_corners() {
        let corners = [(50.0, 50.0), (150.0, 220.0), (260.0, 60.0), (370.0, 230.0), (480.0, 70.0)];
        let fit = classify(&polyline_pts(&corners)).expect("polyline");
        assert_eq!(fit.kind, Kind::Polyline);
        match fit.shape {
            Shape::Polygon { pts, closed } => {
                assert!(!closed && pts.len() == 5, "{pts:?}");
                for (p, c) in pts.iter().zip(corners) {
                    assert!(dist(*p, c) < 6.0, "{p:?} vs {c:?}");
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn closed_pentagon_is_a_polygon() {
        let mut corners: Vec<P> = (0..5)
            .map(|k| {
                let a = TAU * k as f64 / 5.0 - PI / 2.0;
                (300.0 + 120.0 * a.cos(), 300.0 + 120.0 * a.sin())
            })
            .collect();
        corners.push(corners[0]);
        let fit = classify(&polyline_pts(&corners)).expect("polygon");
        assert_eq!(fit.kind, Kind::Polygon);
        assert_eq!(fit.kind.name(), "Polygon");
        match fit.shape {
            Shape::Polygon { pts, closed } => assert!(closed && pts.len() == 5, "{}", pts.len()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn closed_skewed_quadrilateral_is_a_polygon_not_a_rectangle() {
        let corners = [(100.0, 100.0), (320.0, 130.0), (260.0, 260.0), (60.0, 230.0), (100.0, 100.0)];
        let fit = classify(&polyline_pts(&corners)).expect("quad");
        assert_eq!(fit.kind, Kind::Polygon);
    }

    #[test]
    fn every_shape_has_handles_and_they_sit_on_the_shape() {
        let line = Shape::Line { a: (0.0, 0.0), b: (100.0, 0.0) };
        let h = handles(&line);
        assert_eq!(h.len(), 3);
        assert_eq!(h[2].kind, HandleKind::Move);
        assert!(dist(h[2].pos, (50.0, 0.0)) < 1e-9);
        let arc = Shape::Arc { c: (0.0, 0.0), r: 100.0, a0: 0.0, a1: PI };
        let h = handles(&arc);
        assert!(dist(h[0].pos, (100.0, 0.0)) < 1e-9 && dist(h[1].pos, (-100.0, 0.0)) < 1e-9 && dist(h[2].pos, (0.0, 100.0)) < 1e-9);
        let ell = Shape::Ellipse { c: (10.0, 10.0), rx: 40.0, ry: 20.0, rot: 0.0 };
        let h = handles(&ell);
        assert_eq!(h.len(), 5);
        assert!(dist(h[1].pos, (50.0, 10.0)) < 1e-9 && dist(h[2].pos, (10.0, 30.0)) < 1e-9);
        let rect = Shape::Rect { c: (0.0, 0.0), w: 40.0, h: 20.0, rot: 0.0 };
        let h = handles(&rect);
        assert_eq!(h.len(), 5);
        assert!(dist(h[1].pos, (-20.0, -10.0)) < 1e-9 && dist(h[3].pos, (20.0, 10.0)) < 1e-9);
        let tri = Shape::Polygon { pts: vec![(0.0, 0.0), (30.0, 0.0), (0.0, 30.0)], closed: true };
        let h = handles(&tri);
        assert_eq!(h.len(), 4);
        assert!(dist(h[3].pos, (10.0, 10.0)) < 1e-9);
    }

    #[test]
    fn dragging_line_and_polygon_handles_moves_points_or_the_whole_shape() {
        let line = Shape::Line { a: (0.0, 0.0), b: (100.0, 0.0) };
        assert_eq!(drag_handle(&line, 1, (100.0, 50.0)), Shape::Line { a: (0.0, 0.0), b: (100.0, 50.0) });
        assert_eq!(drag_handle(&line, 2, (60.0, 10.0)), Shape::Line { a: (10.0, 10.0), b: (110.0, 10.0) });
        assert_eq!(drag_handle(&line, 7, (1.0, 1.0)), line);
        let tri = Shape::Polygon { pts: vec![(0.0, 0.0), (30.0, 0.0), (0.0, 30.0)], closed: true };
        match drag_handle(&tri, 1, (60.0, 5.0)) {
            Shape::Polygon { pts, closed } => assert!(closed && pts == vec![(0.0, 0.0), (60.0, 5.0), (0.0, 30.0)]),
            other => panic!("{other:?}"),
        }
        match drag_handle(&tri, 3, (20.0, 10.0)) {
            Shape::Polygon { pts, .. } => {
                assert!(dist(pts[0], (10.0, 0.0)) < 1e-9 && dist(pts[1], (40.0, 0.0)) < 1e-9 && dist(pts[2], (10.0, 30.0)) < 1e-9);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn dragging_ellipse_and_rectangle_handles_resizes_about_the_fixed_side() {
        let ell = Shape::Ellipse { c: (100.0, 100.0), rx: 40.0, ry: 20.0, rot: 0.0 };
        match drag_handle(&ell, 1, (170.0, 100.0)) {
            Shape::Ellipse { c, rx, ry, .. } => assert!(dist(c, (100.0, 100.0)) < 1e-9 && (rx - 70.0).abs() < 1e-9 && (ry - 20.0).abs() < 1e-9),
            other => panic!("{other:?}"),
        }
        match drag_handle(&ell, 0, (10.0, 20.0)) {
            Shape::Ellipse { c, .. } => assert_eq!(c, (10.0, 20.0)),
            other => panic!("{other:?}"),
        }
        // A circle stays a circle.
        let circle = Shape::Ellipse { c: (0.0, 0.0), rx: 30.0, ry: 30.0, rot: 0.0 };
        match drag_handle(&circle, 2, (0.0, 55.0)) {
            Shape::Ellipse { rx, ry, .. } => assert!((rx - 55.0).abs() < 1e-9 && (ry - 55.0).abs() < 1e-9),
            other => panic!("{other:?}"),
        }
        // The rectangle's opposite corner stays where it was.
        let rect = Shape::Rect { c: (100.0, 50.0), w: 80.0, h: 40.0, rot: 0.0 };
        let before = handles(&rect)[1].pos;
        let next = drag_handle(&rect, 3, (200.0, 100.0));
        let after = handles(&next);
        assert!(dist(after[1].pos, before) < 1e-9, "{:?} vs {before:?}", after[1].pos);
        assert!(dist(after[3].pos, (200.0, 100.0)) < 1e-9);
        // The same holds for a rotated rectangle.
        let rect = Shape::Rect { c: (100.0, 50.0), w: 80.0, h: 40.0, rot: 0.4 };
        let h0 = handles(&rect);
        let target = (h0[2].pos.0 + 25.0, h0[2].pos.1 + 10.0);
        let next = drag_handle(&rect, 2, target);
        let h1 = handles(&next);
        assert!(dist(h1[4].pos, h0[4].pos) < 1e-9 && dist(h1[2].pos, target) < 1e-9);
    }

    #[test]
    fn arc_from_three_points_passes_through_all_three() {
        let (p0, pm, p1) = ((0.0, 0.0), (50.0, -40.0), (100.0, 0.0));
        let Some(Shape::Arc { c, r, .. }) = arc_from_three(p0, pm, p1) else {
            panic!("no arc")
        };
        for p in [p0, pm, p1] {
            assert!((dist(c, p) - r).abs() < 1e-6);
        }
        // The sweep passes the middle point: it is the arc's middle handle.
        let arc = arc_from_three(p0, pm, p1).unwrap();
        let h = handles(&arc);
        assert!(dist(h[0].pos, p0) < 1e-6 && dist(h[1].pos, p1) < 1e-6);
        assert!(dist(h[2].pos, pm) < 1e-6, "{:?}", h[2].pos);
        // Collinear points have no arc.
        assert!(arc_from_three((0.0, 0.0), (10.0, 0.0), (20.0, 0.0)).is_none());
    }

    #[test]
    fn dragging_an_arc_handle_keeps_the_other_two_points() {
        let arc = Shape::Arc { c: (0.0, 0.0), r: 100.0, a0: PI, a1: TAU };
        let h = handles(&arc);
        let (start, end) = (h[0].pos, h[1].pos);
        // Pull the bulge handle out: the ends stay.
        let next = drag_handle(&arc, 2, (0.0, -160.0));
        let nh = handles(&next);
        assert!(dist(nh[0].pos, start) < 1e-6 && dist(nh[1].pos, end) < 1e-6);
        assert!(dist(nh[2].pos, (0.0, -160.0)) < 1e-6);
        // Move the end: the start and the bulge stay.
        let next = drag_handle(&arc, 1, (120.0, 40.0));
        let nh = handles(&next);
        assert!(dist(nh[0].pos, start) < 1e-6 && dist(nh[1].pos, (120.0, 40.0)) < 1e-6);
        // The circle still passes through the old bulge point.
        let Shape::Arc { c, r, .. } = next else { panic!("not an arc") };
        assert!((dist(c, h[2].pos) - r).abs() < 1e-6);
        // A drag that makes the three points collinear is refused.
        assert_eq!(drag_handle(&arc, 2, (0.0, 0.0)), arc);
    }

    #[test]
    fn hit_testing_prefers_control_points_over_the_move_handle() {
        let line = Shape::Line { a: (0.0, 0.0), b: (10.0, 0.0) };
        // Only the middle handle is within 2 of (5, 1).
        assert_eq!(hit_handle(&line, (5.0, 1.0), 2.0), Some(2));
        assert_eq!(hit_handle(&line, (1.0, 1.0), 3.0), Some(0));
        assert_eq!(hit_handle(&line, (50.0, 50.0), 6.0), None);
        // Overlapping move and point handles: the point wins.
        let tiny = Shape::Line { a: (0.0, 0.0), b: (4.0, 0.0) };
        assert_ne!(hit_handle(&tiny, (2.0, 0.0), 9.0), Some(2));
        assert_ne!(hit_handle(&tiny, (1.9, 0.0), 9.0), Some(2));
    }

    #[test]
    fn still_timer_fires_once_after_the_hold_time() {
        let mut t = StillTimer::new((10.0, 10.0), 0.0);
        assert!(!t.update((10.5, 10.0), 0.3));
        assert!(!t.update((10.5, 10.0), 0.6));
        assert!(t.update((10.5, 10.0), 0.66));
        assert!(!t.update((10.5, 10.0), 0.9));
        // Moving again re-arms it.
        assert!(!t.update((40.0, 10.0), 1.0));
        assert!(t.update((40.0, 10.0), 1.7));
    }

    /// Seeded noise (xorshift), uniform in [-1, 1].
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
        }
    }

    /// Add pen jitter: white noise of `amp` px plus a slow random-walk drift of `amp` px in total.
    fn noisy(pts: &[P], amp: f64, seed: u64) -> Vec<P> {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (mut dx, mut dy) = (0.0, 0.0);
        pts.iter()
            .map(|&(x, y)| {
                dx += rng.next() * amp * 0.15;
                dy += rng.next() * amp * 0.15;
                (x + rng.next() * amp + dx, y + rng.next() * amp + dy)
            })
            .collect()
    }

    /// Points along a closed polygon at about `gap` px spacing.
    fn polygon_pts(corners: &[P], gap: f64, closed: bool) -> Vec<P> {
        let n = corners.len();
        let edges = if closed { n } else { n - 1 };
        let mut out = Vec::new();
        for i in 0..edges {
            let (a, b) = (corners[i], corners[(i + 1) % n]);
            let k = (dist(a, b) / gap).round().max(1.0) as usize;
            for j in 0..k {
                let t = j as f64 / k as f64;
                out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
            }
        }
        out.push(if closed { corners[0] } else { corners[n - 1] });
        out
    }

    /// Jitter amplitudes as a fraction of the shape's size: 0.5 %, 1 %, 2 % and 3 %.
    const NOISE: [f64; 4] = [0.005, 0.01, 0.02, 0.03];

    #[test]
    fn noisy_circles_come_out_as_circles() {
        for (k, frac) in NOISE.iter().enumerate() {
            for seed in 1..=20u64 {
                let r = 90.0;
                let pts = noisy(&with_ring(ellipse_pts((300.0, 300.0), r, r, 0.0, 120)), frac * 2.0 * r, seed + 100 * k as u64);
                let fit = classify(&pts).unwrap_or_else(|| panic!("circle amp {frac} seed {seed}: no shape"));
                assert_eq!(fit.kind, Kind::Circle, "circle amp {frac} seed {seed}");
            }
        }
    }

    #[test]
    fn noisy_ellipses_come_out_as_ellipses() {
        for (k, frac) in NOISE.iter().enumerate() {
            for seed in 1..=20u64 {
                let rot = seed as f64 * 0.31;
                let pts = noisy(&with_ring(ellipse_pts((300.0, 300.0), 140.0, 60.0, rot, 140)), frac * 120.0, seed + 100 * k as u64);
                let fit = classify(&pts).unwrap_or_else(|| panic!("ellipse amp {frac} seed {seed}: no shape"));
                assert_eq!(fit.kind, Kind::Ellipse, "ellipse amp {frac} seed {seed}");
            }
        }
    }

    #[test]
    fn noisy_rectangles_and_triangles_stay_polygons() {
        let rect = polygon_pts(&[(100.0, 100.0), (320.0, 100.0), (320.0, 240.0), (100.0, 240.0)], 4.0, true);
        let tri = polygon_pts(&[(100.0, 300.0), (320.0, 300.0), (210.0, 110.0)], 4.0, true);
        for (k, frac) in NOISE.iter().take(3).enumerate() {
            for seed in 1..=20u64 {
                let sd = seed + 100 * k as u64;
                let fit = classify(&noisy(&rect, frac * 140.0, sd)).unwrap_or_else(|| panic!("rect amp {frac} seed {seed}: no shape"));
                assert_eq!(fit.kind, Kind::Rectangle, "rectangle amp {frac} seed {seed}");
                let fit = classify(&noisy(&tri, frac * 190.0, sd)).unwrap_or_else(|| panic!("triangle amp {frac} seed {seed}: no shape"));
                assert_eq!(fit.kind, Kind::Triangle, "triangle amp {frac} seed {seed}");
            }
        }
    }

    #[test]
    fn noisy_zigzags_stay_polylines_and_noisy_lines_stay_lines() {
        let zig = polygon_pts(&[(100.0, 100.0), (200.0, 260.0), (300.0, 100.0), (400.0, 260.0)], 4.0, false);
        let line = polygon_pts(&[(100.0, 100.0), (420.0, 180.0)], 4.0, false);
        for (k, frac) in NOISE.iter().take(3).enumerate() {
            for seed in 1..=20u64 {
                let sd = seed + 100 * k as u64;
                let fit = classify(&noisy(&zig, frac * 160.0, sd)).unwrap_or_else(|| panic!("zigzag amp {frac} seed {seed}: no shape"));
                assert_eq!(fit.kind, Kind::Polyline, "zigzag amp {frac} seed {seed}");
                let fit = classify(&noisy(&line, frac * 100.0, sd)).unwrap_or_else(|| panic!("line amp {frac} seed {seed}: no shape"));
                assert_eq!(fit.kind, Kind::Line, "line amp {frac} seed {seed}");
            }
        }
    }

    #[test]
    fn a_pentagon_is_still_a_polygon_after_smoothing() {
        let c: Vec<P> = (0..5).map(|i| (300.0 + 100.0 * (TAU * i as f64 / 5.0).cos(), 300.0 + 100.0 * (TAU * i as f64 / 5.0).sin())).collect();
        for seed in 1..=10u64 {
            let fit = classify(&noisy(&polygon_pts(&c, 4.0, true), 3.0, seed)).expect("pentagon");
            assert_eq!(fit.kind, Kind::Polygon, "pentagon seed {seed}");
        }
    }

    #[test]
    fn a_hold_tail_does_not_add_corners() {
        // A circle drawn by hand, then the pen drifts around the end point for the hold.
        let mut pts = noisy(&with_ring(ellipse_pts((300.0, 300.0), 90.0, 90.0, 0.0, 120)), 2.0, 7);
        let end = *pts.last().unwrap();
        let mut rng = Rng(99);
        for _ in 0..40 {
            pts.push((end.0 + rng.next() * 6.0, end.1 + rng.next() * 6.0));
        }
        let trimmed = trim_hold_tail(&pts, 6.0);
        assert!(trimmed.len() < pts.len() - 30, "the cluster is cut off");
        assert_eq!(classify(&trimmed).map(|f| f.kind), Some(Kind::Circle));
    }

    #[test]
    fn a_drifting_pen_still_triggers_the_hold() {
        // 1 px of white noise plus a creep of 4 px over the hold: a strict standstill never fires.
        let mut rng = Rng(5);
        let mut t = StillTimer::with_secs((100.0, 100.0), 0.0, 0.65);
        let mut fired_at = None;
        for i in 1..120 {
            let time = i as f64 / 120.0;
            let p = (100.0 + time * 6.0 + rng.next(), 100.0 + rng.next());
            if t.update_with_tolerance(p, time, 6.0) && fired_at.is_none() {
                fired_at = Some(time);
            }
        }
        let at = fired_at.expect("the hold fires despite the drift");
        assert!((0.64..0.8).contains(&at), "fires after the hold time, not much later: {at}");
        // A steady creep of 15 px/s (about 10 px in the hold): the old strict standstill (6 px around the
        // point where the pen stopped) restarted every 0.4 s and never fired.
        let mut t = StillTimer::with_secs((0.0, 0.0), 0.0, 0.65);
        assert!((1..240).any(|i| t.update_with_tolerance((i as f64 / 120.0 * 15.0, 0.0), i as f64 / 120.0, 6.0)), "slow creep still holds");
        // A pen that is really moving never fires.
        let mut t = StillTimer::with_secs((0.0, 0.0), 0.0, 0.65);
        assert!(!(1..240).any(|i| t.update_with_tolerance((i as f64 * 0.8, 0.0), i as f64 / 120.0, 6.0)));
    }

    #[test]
    fn a_noisy_hexagon_is_still_a_polygon() {
        let c: Vec<P> = (0..6).map(|i| (300.0 + 100.0 * (TAU * i as f64 / 6.0).cos(), 300.0 + 100.0 * (TAU * i as f64 / 6.0).sin())).collect();
        for seed in 1..=10u64 {
            let fit = classify(&noisy(&polygon_pts(&c, 4.0, true), 2.0, seed)).expect("hexagon");
            assert_eq!(fit.kind, Kind::Polygon, "hexagon seed {seed}");
        }
    }
}
