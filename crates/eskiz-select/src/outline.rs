//! Marching-ants outlines: marching squares over a mask, stitched into polylines.
//! Display only; nothing here feeds back into pixels.

use std::collections::{HashMap, VecDeque};

use crate::geom::Point;
use crate::mask::Mask;

/// Dash length in screen pixels.
pub const DASH_LENGTH_PX: f32 = 4.0;
/// Dash travel speed in screen pixels per second.
pub const DASH_SPEED_PX_PER_S: f32 = 8.0;
/// Level at which the selection boundary is drawn.
pub const OUTLINE_LEVEL: f32 = 0.5;

/// A stitched contour. `closed` is true when the last point connects back to the first.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub closed: bool,
}

/// A grid edge between two neighbouring samples. Horizontal edges join `(x, y)` to
/// `(x + 1, y)`; vertical edges join `(x, y)` to `(x, y + 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct EdgeKey {
    x: usize,
    y: usize,
    horizontal: bool,
}

/// Contours of `mask` at `level`. The sample grid is padded with zeros, so contours that
/// touch the canvas edge still close. Crossings are linearly interpolated between samples.
pub fn outline(mask: &Mask, level: f32) -> Vec<Polyline> {
    if mask.width == 0 || mask.height == 0 {
        return Vec::new();
    }
    let sw = mask.width + 2;
    let sh = mask.height + 2;
    let mut samples = vec![0.0f32; sw * sh];
    for y in 0..mask.height {
        for x in 0..mask.width {
            samples[(y + 1) * sw + x + 1] = mask.data[y * mask.width + x];
        }
    }
    let value = |x: usize, y: usize| samples[y * sw + x];
    let inside = |x: usize, y: usize| value(x, y) >= level;
    let edge_point = |e: EdgeKey| -> Point {
        let (bx, by) = if e.horizontal {
            (e.x + 1, e.y)
        } else {
            (e.x, e.y + 1)
        };
        let (va, vb) = (value(e.x, e.y), value(bx, by));
        let t = if (vb - va).abs() < 1e-12 {
            0.5
        } else {
            ((level - va) / (vb - va)).clamp(0.0, 1.0)
        };
        let pa = sample_position(e.x, e.y);
        let pb = sample_position(bx, by);
        pa + (pb - pa) * t
    };

    let mut segments: Vec<(EdgeKey, EdgeKey)> = Vec::new();
    for cy in 0..sh - 1 {
        for cx in 0..sw - 1 {
            let a = inside(cx, cy);
            let b = inside(cx + 1, cy);
            let c = inside(cx + 1, cy + 1);
            let d = inside(cx, cy + 1);
            let case = u8::from(a) | (u8::from(b) << 1) | (u8::from(c) << 2) | (u8::from(d) << 3);
            let top = EdgeKey {
                x: cx,
                y: cy,
                horizontal: true,
            };
            let bottom = EdgeKey {
                x: cx,
                y: cy + 1,
                horizontal: true,
            };
            let left = EdgeKey {
                x: cx,
                y: cy,
                horizontal: false,
            };
            let right = EdgeKey {
                x: cx + 1,
                y: cy,
                horizontal: false,
            };
            let centre_value =
                (value(cx, cy) + value(cx + 1, cy) + value(cx + 1, cy + 1) + value(cx, cy + 1))
                    * 0.25;
            let centre_inside = centre_value >= level;
            let mut add = |p: EdgeKey, q: EdgeKey| segments.push((p, q));
            match case {
                1 | 14 => add(left, top),
                2 | 13 => add(top, right),
                3 | 12 => add(left, right),
                4 | 11 => add(right, bottom),
                6 | 9 => add(top, bottom),
                7 | 8 => add(left, bottom),
                5 => {
                    if centre_inside {
                        add(top, right);
                        add(left, bottom);
                    } else {
                        add(left, top);
                        add(right, bottom);
                    }
                }
                10 => {
                    if centre_inside {
                        add(left, top);
                        add(right, bottom);
                    } else {
                        add(top, right);
                        add(left, bottom);
                    }
                }
                _ => {}
            }
        }
    }

    let mut adjacency: HashMap<EdgeKey, Vec<usize>> = HashMap::new();
    for (i, &(p, q)) in segments.iter().enumerate() {
        adjacency.entry(p).or_default().push(i);
        adjacency.entry(q).or_default().push(i);
    }
    let mut used = vec![false; segments.len()];
    let mut lines = Vec::new();
    for (start, &(k0, k1)) in segments.iter().enumerate() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut keys: VecDeque<EdgeKey> = VecDeque::from([k0, k1]);
        let mut closed = false;
        while let Some(&tail) = keys.back() {
            let Some(next) = take_next(&adjacency, &segments, &mut used, tail) else {
                break;
            };
            keys.push_back(next);
            if keys.front() == keys.back() {
                closed = true;
                break;
            }
        }
        if closed {
            keys.pop_back();
        } else {
            while let Some(&head) = keys.front() {
                let Some(prev) = take_next(&adjacency, &segments, &mut used, head) else {
                    break;
                };
                keys.push_front(prev);
            }
        }
        let points: Vec<Point> = keys.iter().map(|&k| edge_point(k)).collect();
        lines.push(Polyline { points, closed });
    }
    lines
}

/// Canvas position of sample `(x, y)`. Sample index `x` is pixel `x - 1`, centred at `x - 0.5`.
fn sample_position(x: usize, y: usize) -> Point {
    Point::new(x as f32 - 0.5, y as f32 - 0.5)
}

/// Marks an unused segment at `key` as used and returns its other endpoint.
fn take_next(
    adjacency: &HashMap<EdgeKey, Vec<usize>>,
    segments: &[(EdgeKey, EdgeKey)],
    used: &mut [bool],
    key: EdgeKey,
) -> Option<EdgeKey> {
    let seg = *adjacency.get(&key)?.iter().find(|&&s| !used[s])?;
    used[seg] = true;
    let (p, q) = segments[seg];
    Some(if p == key { q } else { p })
}
