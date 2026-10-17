//! Automatic (magic wand) selection by colour distance.

use crate::image::{Image, Rgba};
use crate::mask::Mask;

/// Default wand threshold.
pub const DEFAULT_THRESHOLD: f32 = 0.40;

/// Threshold change per point of drag at speed factor 1.
pub const THRESHOLD_PER_DRAG_POINT: f32 = 1.0 / 200.0;

/// Colour distance: the largest absolute channel difference over R, G, B and A.
pub fn colour_distance(a: Rgba, b: Rgba) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

/// Selects the 4-connected region around `seed` whose pixels lie within `threshold`
/// of the seed colour. Threshold is clamped to `[0, 1]`. A seed outside the image gives an
/// empty mask. Selected pixels get coverage 1.
pub fn flood_select(source: &Image, seed: (i64, i64), threshold: f32) -> Mask {
    let (w, h) = (source.width, source.height);
    let mut mask = Mask::empty(w, h);
    let (sx, sy) = seed;
    if w == 0 || h == 0 || sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
        return mask;
    }
    let t = if threshold.is_nan() {
        0.0
    } else {
        threshold.clamp(0.0, 1.0)
    };
    let seed_idx = sy as usize * w + sx as usize;
    let seed_px = source.pixels[seed_idx];
    let mut visited = vec![false; w * h];
    visited[seed_idx] = true;
    let mut stack = vec![seed_idx];
    while let Some(idx) = stack.pop() {
        mask.data[idx] = 1.0;
        for nb in neighbours(idx, w, h).into_iter().flatten() {
            if visited[nb] {
                continue;
            }
            visited[nb] = true;
            if colour_distance(seed_px, source.pixels[nb]) <= t {
                stack.push(nb);
            }
        }
    }
    mask
}

fn neighbours(idx: usize, w: usize, h: usize) -> [Option<usize>; 4] {
    let (x, y) = (idx % w, idx / w);
    [
        (x > 0).then(|| idx - 1),
        (x + 1 < w).then(|| idx + 1),
        (y > 0).then(|| idx - w),
        (y + 1 < h).then(|| idx + w),
    ]
}

/// Threshold after a drag of `drag_points` (horizontal, in points) at `speed_factor`.
/// Each point changes the threshold by 1/200 times the speed factor; the result is clamped to
/// `[0, 1]`. The speed factor curve is not specified, so the caller supplies it.
pub fn threshold_after_drag(threshold: f32, drag_points: f32, speed_factor: f32) -> f32 {
    (threshold + drag_points * speed_factor * THRESHOLD_PER_DRAG_POINT).clamp(0.0, 1.0)
}
