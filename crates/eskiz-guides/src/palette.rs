//! Palette extraction from image pixels, using k-means with k-means++ seeding.
//!
//! The result is deterministic: the same pixels and `max_colours` always give the same
//! palette. Colours are clustered in sRGB (8-bit channels, not perceptual Lab), and the
//! palette is ordered by how many pixels each cluster holds, largest first.

use std::collections::BTreeMap;

use crate::colour::Rgb8;

/// Largest palette this module returns.
pub const MAX_PALETTE_COLOURS: usize = 30;

/// Pixels with alpha below this are ignored.
const MIN_ALPHA: u8 = 128;
/// Upper bound on points fed to k-means. Larger images are subsampled with a fixed stride.
const MAX_SAMPLES: usize = 20_000;
/// Lloyd iterations cap. Convergence normally stops sooner.
const MAX_ITERATIONS: usize = 30;
/// Fixed seed so that extraction is deterministic.
const SEED: u64 = 0x4561_7365_6C00_0001;

/// Extracts up to `max_colours` representative colours from RGBA pixels. The count is
/// capped at [`MAX_PALETTE_COLOURS`]. Transparent pixels (alpha below 128) are skipped.
///
/// If the image has no more distinct colours than the cap, those colours are returned
/// exactly, most frequent first.
pub fn extract_palette(pixels: &[[u8; 4]], max_colours: usize) -> Vec<Rgb8> {
    let k = max_colours.min(MAX_PALETTE_COLOURS);
    if k == 0 {
        return Vec::new();
    }
    let opaque: Vec<[u8; 3]> = pixels
        .iter()
        .filter(|p| p[3] >= MIN_ALPHA)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    if opaque.is_empty() {
        return Vec::new();
    }

    let mut histogram: BTreeMap<[u8; 3], usize> = BTreeMap::new();
    for c in &opaque {
        *histogram.entry(*c).or_insert(0) += 1;
    }
    if histogram.len() <= k {
        let mut exact: Vec<([u8; 3], usize)> = histogram.into_iter().collect();
        exact.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        return exact
            .into_iter()
            .map(|(c, _)| Rgb8::new(c[0], c[1], c[2]))
            .collect();
    }

    let stride = opaque.len().div_ceil(MAX_SAMPLES).max(1);
    let points: Vec<[f64; 3]> = opaque
        .iter()
        .step_by(stride)
        .map(|c| [f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])
        .collect();
    kmeans(&points, k)
}

/// Small deterministic PRNG (SplitMix64).
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform index in `0..n`. `n` must be non-zero.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn dist2(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

fn nearest(p: &[f64; 3], centres: &[[f64; 3]]) -> usize {
    let mut best = 0;
    let mut best_d = f64::INFINITY;
    for (i, c) in centres.iter().enumerate() {
        let d = dist2(p, c);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

fn kmeans(points: &[[f64; 3]], k: usize) -> Vec<Rgb8> {
    let mut rng = SplitMix64(SEED);

    // k-means++ seeding: each new centre is drawn with probability proportional to its
    // squared distance from the nearest existing centre.
    let mut centres = vec![points[rng.below(points.len())]];
    let mut nearest_d2: Vec<f64> = points.iter().map(|p| dist2(p, &centres[0])).collect();
    while centres.len() < k {
        let total: f64 = nearest_d2.iter().sum();
        if total <= 0.0 {
            break;
        }
        let mut target = rng.unit() * total;
        let mut chosen = points.len() - 1;
        for (i, d) in nearest_d2.iter().enumerate() {
            if target < *d {
                chosen = i;
                break;
            }
            target -= *d;
        }
        let new_centre = points[chosen];
        for (d, p) in nearest_d2.iter_mut().zip(points) {
            *d = d.min(dist2(p, &new_centre));
        }
        centres.push(new_centre);
    }

    // Lloyd iterations.
    let mut assign = vec![usize::MAX; points.len()];
    for _ in 0..MAX_ITERATIONS {
        let mut changed = false;
        for (a, p) in assign.iter_mut().zip(points) {
            let best = nearest(p, &centres);
            if *a != best {
                *a = best;
                changed = true;
            }
        }
        let mut sums = vec![[0.0_f64; 3]; centres.len()];
        let mut counts = vec![0_usize; centres.len()];
        for (&a, p) in assign.iter().zip(points) {
            counts[a] += 1;
            for (s, v) in sums[a].iter_mut().zip(p) {
                *s += v;
            }
        }
        for (i, centre) in centres.iter_mut().enumerate() {
            if counts[i] > 0 {
                let n = counts[i] as f64;
                *centre = sums[i].map(|s| s / n);
            }
        }
        if !changed {
            break;
        }
    }

    // Order clusters by population, then drop empty and duplicate colours.
    let mut counts = vec![0_usize; centres.len()];
    for &a in &assign {
        counts[a] += 1;
    }
    let mut order: Vec<usize> = (0..centres.len()).filter(|&i| counts[i] > 0).collect();
    order.sort_by(|&a, &b| counts[b].cmp(&counts[a]).then(a.cmp(&b)));

    let mut out: Vec<Rgb8> = Vec::with_capacity(order.len());
    for i in order {
        let c = centres[i];
        let colour = Rgb8::new(
            c[0].round().clamp(0.0, 255.0) as u8,
            c[1].round().clamp(0.0, 255.0) as u8,
            c[2].round().clamp(0.0, 255.0) as u8,
        );
        if !out.contains(&colour) {
            out.push(colour);
        }
    }
    out
}
