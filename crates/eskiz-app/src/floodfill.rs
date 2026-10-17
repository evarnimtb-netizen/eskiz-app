//! Pour Fill flood fill, as a pure function over a tiled layer.
//!
//! * **Source.** The fill reads the pixels of the *active layer* only (not the composite),
//!   (the source is the current layer by default). A closed shape therefore has to be on the layer being filled. On an empty
//!   layer the fill covers the whole layer, as it would with any flood fill.
//! * **Distance.** `d(p, q) = max over R, G, B, A of |p - q| / 255` on premultiplied pixels.
//!   Premultiplied values make transparent pixels all equal whatever their hidden colour.
//! * **Region.** Every pixel 4-connected to the seed with `d <= threshold`. Threshold 0 takes
//!   only pixels identical to the seed; 1 takes the whole connected area.
//! * **Antialiased edges.** A pixel at distance `d` is treated as `d` of "something else"
//!   mixed with `1 - d` of the seed, so the fill replaces only the seed part:
//!   `out = p + (1 - d) * (fill - seed)`. Fully-seed pixels become the fill colour, partly
//!   covered edge pixels keep their line and gain fill underneath, and pixels at `d = 1`
//!   are untouched. The same formula is applied to the one-pixel ring just outside the
//!   region, so edge pixels beyond the threshold get no transparent halo.

use eskiz_core::canvas::Layer;
use eskiz_core::pixel::Chan;

use crate::history::PixelRect;

/// The default Pour Fill threshold (50%).
pub const DEFAULT_THRESHOLD: f32 = 0.5;

/// The pixels a fill would write.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    /// Bounding box of every pixel the fill may change.
    pub rect: PixelRect,
    /// New premultiplied RGBA8 pixels for `rect` (same layout as `PixelRect::crop`).
    pub pixels: Vec<u8>,
    /// Pixels inside the region (not counting the antialiasing ring).
    pub region_pixels: usize,
}

/// Distance between two premultiplied RGBA pixels, 0..=1.
pub fn distance(a: [u8; 4], b: [u8; 4]) -> f32 {
    distance_t(a, b)
}

fn distance_t<T: Chan>(a: [T; 4], b: [T; 4]) -> f32 {
    let m = (0..4).map(|i| (a[i].to_f32() - b[i].to_f32()).abs()).fold(0.0, f32::max);
    m / T::MAXF
}

/// [`Fill`] for a 16-bit layer: the same region and rectangle, with RGBA16 pixels.
#[derive(Clone, Debug)]
pub struct Fill16 {
    pub rect: PixelRect,
    /// New premultiplied RGBA16 samples for `rect`.
    pub pixels: Vec<u16>,
    pub region_pixels: usize,
}

/// Flood-fill from `seed` (canvas pixel) over a premultiplied RGBA8 `layer`.
/// `None` when the seed is outside the layer. The layer is not modified: write `Fill::pixels`
/// into `Fill::rect` (`Layer::write_rect`) to apply it.
pub fn flood_fill(layer: &Layer, seed: (i64, i64), threshold: f32, colour: [u8; 3]) -> Option<Fill> {
    let (rect, pixels, region_pixels) = fill_core::<u8>(
        layer,
        |x, y| layer.pixel(x, y),
        |r| layer.read_rect(r.x, r.y, r.w, r.h),
        seed,
        threshold,
        colour,
    )?;
    Some(Fill { rect, pixels, region_pixels })
}

/// [`flood_fill`] on a 16-bit layer. Regions are found with 16-bit distances and the fill colour
/// is widened (`v * 257`), so nothing goes through 8 bits.
pub fn flood_fill16(layer: &Layer, seed: (i64, i64), threshold: f32, colour: [u8; 3]) -> Option<Fill16> {
    let (rect, pixels, region_pixels) = fill_core::<u16>(
        layer,
        |x, y| layer.pixel16(x, y),
        |r| layer.read_rect16(r.x, r.y, r.w, r.h),
        seed,
        threshold,
        colour.map(eskiz_core::pixel::up8),
    )?;
    Some(Fill16 { rect, pixels, region_pixels })
}

/// A maximal run of pixels on one row that are within the threshold of the seed (`x0..=x1`).
#[derive(Clone, Copy, Debug)]
struct Run {
    x0: u32,
    x1: u32,
    /// Reached from the seed: part of the region.
    member: bool,
}

impl Run {
    fn new(x0: usize, x1: usize) -> Self {
        Self { x0: x0 as u32, x1: x1 as u32, member: false }
    }

    fn span(&self) -> (usize, usize) {
        (self.x0 as usize, self.x1 as usize)
    }
}

/// Index of the run that contains `x`, if any. Runs are sorted and disjoint.
fn run_at(runs: &[Run], x: usize) -> Option<usize> {
    let i = runs.partition_point(|r| (r.x1 as usize) < x);
    runs.get(i).filter(|r| r.x0 as usize <= x).map(|_| i)
}

/// Collects the maximal runs of inside pixels along one row. Pixels must come in increasing x.
#[derive(Default)]
struct RunBuilder {
    runs: Vec<Run>,
    open: Option<usize>,
}

impl RunBuilder {
    fn at(&mut self, x: usize, inside: bool) {
        match (inside, self.open) {
            (true, None) => self.open = Some(x),
            (false, Some(x0)) => {
                self.runs.push(Run::new(x0, x - 1));
                self.open = None;
            }
            _ => {}
        }
    }

    fn finish(mut self, width: usize) -> Vec<Run> {
        if let Some(x0) = self.open {
            self.runs.push(Run::new(x0, width - 1));
        }
        self.runs
    }
}

/// The threshold test against the seed, and the runs of each row, read from the layer the first
/// time the fill reaches the row.
struct Scan<'a, T: Chan> {
    layer: &'a Layer,
    seed_px: [T; 4],
    threshold: f32,
    rows: Vec<Option<Vec<Run>>>,
}

impl<T: Chan> Scan<'_, T> {
    fn inside(&self, p: [T; 4]) -> bool {
        distance_t(p, self.seed_px) <= self.threshold
    }

    /// The runs of row `y`, read on first use.
    fn row(&mut self, y: usize) -> &mut Vec<Run> {
        if self.rows[y].is_none() {
            let runs = self.read_row(y);
            self.rows[y] = Some(runs);
        }
        self.rows[y].as_mut().expect("row was read above")
    }

    /// The runs of row `y` if the fill has read it, else none.
    fn cached(&self, y: usize) -> &[Run] {
        self.rows[y].as_deref().unwrap_or(&[])
    }

    /// Reads row `y` span by span. An unallocated tile is one uniform span, so it costs one test.
    fn read_row(&self, y: usize) -> Vec<Run> {
        let mut b = RunBuilder::default();
        for (x, span) in self.layer.row_spans(y, 0, self.layer.width) {
            if let Some(s) = span.data {
                for (i, p) in s.as_chunks::<4>().0.iter().enumerate() {
                    b.at(x + i, self.inside(convert::<u8, T>(p)));
                }
            } else if let Some(s) = span.data16 {
                for (i, p) in s.as_chunks::<4>().0.iter().enumerate() {
                    b.at(x + i, self.inside(convert::<u16, T>(p)));
                }
            } else {
                b.at(x, self.inside([T::default(); 4]));
            }
        }
        b.finish(self.layer.width)
    }
}

/// One pixel's four samples at another depth, exactly as `Layer::pixel` (8-bit, rounding) and
/// `Layer::pixel16` (16-bit, widening) convert them.
fn convert<S: Chan, T: Chan>(p: &[S]) -> [T; 4] {
    [0, 1, 2, 3].map(|k| T::from_u16(p[k].to_u16()))
}

/// Marks the columns of the region's runs in `runs`, each widened by `pad` columns either side,
/// in `touched` (which covers the columns of `rect`).
fn mark_touched(touched: &mut [bool], rect: PixelRect, runs: &[Run], pad: usize) {
    for r in runs.iter().filter(|r| r.member) {
        let (a, b) = r.span();
        let lo = a.saturating_sub(pad).max(rect.x);
        let hi = (b + pad).min(rect.x + rect.w - 1);
        touched[lo - rect.x..=hi - rect.x].fill(true);
    }
}

/// One pixel of the fill: `p` moves towards `fill` by the share that is not the seed (`1 - d`),
/// then is made a valid premultiplied colour.
fn blend<T: Chan>(p: [T; 4], seed_px: [T; 4], fill: [T; 4]) -> [T; 4] {
    let k = 1.0 - distance_t(p, seed_px);
    let mut out = [T::default(); 4];
    for c in 0..4 {
        let v = p[c].to_f32() + k * (fill[c].to_f32() - seed_px[c].to_f32());
        out[c] = T::from_f32_round(v);
    }
    // Keep the pixel a valid premultiplied colour.
    let a = out[3];
    for c in out.iter_mut().take(3) {
        if c.raw() > a.raw() {
            *c = a;
        }
    }
    out
}

/// The scanline fill. The region is the set of pixels 4-connected to the seed within the
/// threshold, found one run of matching pixels at a time: a run on a row is either entirely in
/// the region or not, so each run is visited once, and its neighbours on the rows above and
/// below that match are queued. The ring (the non-region pixels next to the region) gets the
/// same blend as the region, so the bounding box is the region's grown by one pixel.
fn fill_core<T: Chan>(
    layer: &Layer,
    px: impl Fn(usize, usize) -> [T; 4],
    read: impl FnOnce(PixelRect) -> Vec<T>,
    seed: (i64, i64),
    threshold: f32,
    colour: [T; 3],
) -> Option<(PixelRect, Vec<T>, usize)> {
    let (w, h) = (layer.width, layer.height);
    if w == 0 || h == 0 || seed.0 < 0 || seed.1 < 0 || seed.0 >= w as i64 || seed.1 >= h as i64 {
        return None;
    }
    let threshold = threshold.clamp(0.0, 1.0);
    let (sx, sy) = (seed.0 as usize, seed.1 as usize);
    let seed_px = px(sx, sy);
    let mut scan = Scan { layer, seed_px, threshold, rows: vec![None; h] };

    // The seed is always a member, as in the plain reference fill. It is within the threshold of itself
    // unless the threshold is NaN, when its run is the seed alone.
    let runs = scan.row(sy);
    if run_at(runs, sx).is_none() {
        let at = runs.partition_point(|r| (r.x1 as usize) < sx);
        runs.insert(at, Run::new(sx, sx));
    }

    let mut stack = vec![(sx, sy)];
    let mut region_pixels = 0;
    let (mut bx0, mut bx1, mut by0, mut by1) = (sx, sx, sy, sy);
    while let Some((x, y)) = stack.pop() {
        let runs = scan.row(y);
        let Some(i) = run_at(runs, x) else { continue };
        if runs[i].member {
            continue;
        }
        runs[i].member = true;
        let (x0, x1) = runs[i].span();
        region_pixels += x1 - x0 + 1;
        bx0 = bx0.min(x0);
        bx1 = bx1.max(x1);
        by0 = by0.min(y);
        by1 = by1.max(y);
        for ny in [y.checked_sub(1), Some(y + 1).filter(|&n| n < h)].into_iter().flatten() {
            let above = scan.row(ny);
            let first = above.partition_point(|r| (r.x1 as usize) < x0);
            for r in above[first..].iter().take_while(|r| r.x0 as usize <= x1) {
                if !r.member {
                    stack.push(((r.x0 as usize).max(x0), ny));
                }
            }
        }
    }

    // The region's box grown by one pixel, clipped: the ring is exactly that margin, because
    // the region's extreme pixels each have a non-region neighbour outside the box.
    let (x0, y0) = (bx0.saturating_sub(1), by0.saturating_sub(1));
    let (x1, y1) = ((bx1 + 1).min(w - 1), (by1 + 1).min(h - 1));
    let rect = PixelRect { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 };

    let mut pixels = read(rect);
    let fill = [colour[0], colour[1], colour[2], T::from_f32_round(T::MAXF)];
    let mut touched = vec![false; rect.w];
    for y in rect.y..rect.y + rect.h {
        // Region and ring: the region's runs on this row (widened by one column), and the
        // region's runs on the rows above and below.
        touched.fill(false);
        mark_touched(&mut touched, rect, scan.cached(y), 1);
        if y > 0 {
            mark_touched(&mut touched, rect, scan.cached(y - 1), 0);
        }
        if y + 1 < h {
            mark_touched(&mut touched, rect, scan.cached(y + 1), 0);
        }
        // The blend depends only on the pixel's value, so a repeat of the last value reuses it.
        let mut last: Option<([T; 4], [T; 4])> = None;
        for (i, _) in touched.iter().enumerate().filter(|(_, t)| **t) {
            let o = ((y - rect.y) * rect.w + i) * 4;
            let p = [pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]];
            let out = match last {
                Some((q, out)) if q == p => out,
                _ => {
                    let out = blend(p, seed_px, fill);
                    last = Some((p, out));
                    out
                }
            };
            pixels[o..o + 4].copy_from_slice(&out);
        }
    }
    Some((rect, pixels, region_pixels))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 40;
    const H: usize = 30;

    fn blank() -> Vec<u8> {
        vec![0; W * H * 4]
    }

    fn set(buf: &mut [u8], x: usize, y: usize, p: [u8; 4]) {
        buf[(y * W + x) * 4..(y * W + x) * 4 + 4].copy_from_slice(&p);
    }

    fn get(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * W + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// `flood_fill` over a flat `W x H` buffer.
    fn ff(buf: &[u8], seed: (i64, i64), threshold: f32, colour: [u8; 3]) -> Option<Fill> {
        flood_fill(&Layer::from_rgba(W, H, buf), seed, threshold, colour)
    }

    /// Black box outline from (10,8) to (29,21), `t` pixels thick, hard edges.
    fn boxed(t: usize) -> Vec<u8> {
        let mut b = blank();
        for y in 8..=21 {
            for x in 10..=29 {
                let edge = x < 10 + t || x > 29 - t || y < 8 + t || y > 21 - t;
                if edge {
                    set(&mut b, x, y, [0, 0, 0, 255]);
                }
            }
        }
        b
    }

    fn apply(layer: &mut [u8], f: &Fill) {
        f.rect.write(layer, W, &f.pixels);
    }

    const RED: [u8; 3] = [255, 0, 0];

    #[test]
    fn fills_a_closed_region_and_nothing_outside() {
        let mut layer = boxed(2);
        let f = ff(&layer, (20, 15), 0.5, RED).unwrap();
        // Interior is 16 x 10 pixels.
        assert_eq!(f.region_pixels, 16 * 10);
        apply(&mut layer, &f);
        assert_eq!(get(&layer, 20, 15), [255, 0, 0, 255]);
        assert_eq!(get(&layer, 12, 10), [255, 0, 0, 255]);
        assert_eq!(get(&layer, 27, 19), [255, 0, 0, 255]);
        // The outline is untouched and the outside is still transparent.
        assert_eq!(get(&layer, 10, 8), [0, 0, 0, 255]);
        assert_eq!(get(&layer, 11, 15), [0, 0, 0, 255]);
        assert_eq!(get(&layer, 2, 2), [0, 0, 0, 0]);
        assert_eq!(get(&layer, 35, 25), [0, 0, 0, 0]);
    }

    #[test]
    fn a_gap_in_the_line_leaks_but_a_closed_line_does_not() {
        let mut open = boxed(1);
        set(&mut open, 10, 15, [0, 0, 0, 0]);
        let leaked = ff(&open, (20, 15), 0.5, RED).unwrap();
        assert!(leaked.region_pixels > 18 * 12, "fill should escape through the gap");
        let closed = ff(&boxed(1), (20, 15), 0.5, RED).unwrap();
        assert_eq!(closed.region_pixels, 18 * 12);
        // From outside the box the fill never enters it.
        let out = ff(&boxed(1), (2, 2), 0.5, RED).unwrap();
        assert_eq!(out.region_pixels, W * H - 20 * 14);
    }

    #[test]
    fn diagonal_line_pixels_do_not_leak_with_four_connectivity() {
        let mut b = blank();
        for i in 0..30 {
            set(&mut b, i, i, [0, 0, 0, 255]);
        }
        // A one-pixel diagonal wall from corner to corner splits the canvas under
        // 4-connectivity (8-connectivity would leak through it).
        let f = ff(&b, (30, 5), 0.5, RED).unwrap();
        let mut b2 = b.clone();
        apply(&mut b2, &f);
        assert_eq!(get(&b2, 30, 5), [255, 0, 0, 255]);
        assert_eq!(get(&b2, 5, 12), [0, 0, 0, 0]);
        assert_eq!(get(&b2, 12, 5), [255, 0, 0, 255]);
    }

    #[test]
    fn threshold_controls_what_counts_as_the_same_colour() {
        // Left half transparent, right half a faint 30% grey wash, then a black wall.
        let mut b = blank();
        for y in 0..H {
            for x in 20..W {
                set(&mut b, x, y, [77, 77, 77, 77]); // 30% black, premultiplied
            }
        }
        let strict = ff(&b, (5, 5), 0.1, RED).unwrap();
        assert_eq!(strict.region_pixels, 20 * H, "10% stops at a 30% difference");
        let loose = ff(&b, (5, 5), 0.5, RED).unwrap();
        assert_eq!(loose.region_pixels, W * H, "50% crosses it");
        // Zero takes only identical pixels, 1 takes everything.
        assert_eq!(ff(&b, (5, 5), 0.0, RED).unwrap().region_pixels, 20 * H);
        assert_eq!(ff(&b, (5, 5), 1.0, RED).unwrap().region_pixels, W * H);
        // Exactly at the threshold is included: 77/255 = 0.302.
        assert_eq!(ff(&b, (5, 5), 77.0 / 255.0, RED).unwrap().region_pixels, W * H);
    }

    #[test]
    fn antialiased_edges_are_included_without_a_halo() {
        // Closed ring of antialiased black: a 50% grey wall around the opaque core.
        let mut layer = blank();
        for y in 8..=21 {
            for x in 10..=29 {
                let core = x == 10 || x == 29 || y == 8 || y == 21;
                let soft = (x == 11 || x == 28 || y == 9 || y == 20) && x > 10 && x < 29 && y > 8 && y < 21;
                if core {
                    set(&mut layer, x, y, [0, 0, 0, 255]);
                } else if soft {
                    set(&mut layer, x, y, [0, 0, 0, 153]); // 60% coverage, beyond a 50% threshold
                }
            }
        }
        let f = ff(&layer, (20, 15), 0.5, RED).unwrap();
        // The 60% pixels are outside the 50% region, so only the 16 x 10... interior inside
        // them is in the region.
        assert_eq!(f.region_pixels, 16 * 10);
        apply(&mut layer, &f);
        // A soft edge pixel now has full alpha: its line plus fill underneath, no gap.
        let edge = get(&layer, 11, 15);
        assert_eq!(edge[3], 255, "edge pixel must be opaque, got {edge:?}");
        // 60% black over red: red channel is 40% of 255.
        assert!((i32::from(edge[0]) - 102).abs() <= 1, "{edge:?}");
        // Pixels outside the wall are untouched.
        assert_eq!(get(&layer, 5, 15), [0, 0, 0, 0]);
        assert_eq!(get(&layer, 10, 15), [0, 0, 0, 255]);
    }

    #[test]
    fn partly_covered_pixels_inside_the_threshold_are_blended() {
        let mut layer = blank();
        for y in 0..H {
            set(&mut layer, 20, y, [0, 0, 0, 51]); // 20% black wall across the canvas
        }
        let f = ff(&layer, (5, 5), 0.5, RED).unwrap();
        apply(&mut layer, &f);
        let wall = get(&layer, 20, 10);
        assert_eq!(wall[3], 255);
        // 20% black over red keeps 80% of the red.
        assert!((i32::from(wall[0]) - 204).abs() <= 1, "{wall:?}");
        assert_eq!(get(&layer, 5, 5), [255, 0, 0, 255]);
    }

    #[test]
    fn opaque_regions_are_recoloured() {
        // An opaque white shape with a black outline: filling the white with red.
        let mut layer = vec![255u8; W * H * 4];
        for x in 0..W {
            set(&mut layer, x, 15, [0, 0, 0, 255]);
        }
        let f = ff(&layer, (5, 5), 0.5, [0, 0, 255]).unwrap();
        apply(&mut layer, &f);
        assert_eq!(get(&layer, 5, 5), [0, 0, 255, 255]);
        assert_eq!(get(&layer, 5, 15), [0, 0, 0, 255]);
        assert_eq!(get(&layer, 5, 25), [255, 255, 255, 255], "the other side is a separate region");
    }

    #[test]
    fn seed_outside_the_layer_gives_none() {
        let b = blank();
        assert!(ff(&b, (-1, 0), 0.5, RED).is_none());
        assert!(ff(&b, (0, H as i64), 0.5, RED).is_none());
    }

    #[test]
    fn rect_covers_exactly_what_changed() {
        let layer = boxed(2);
        let f = ff(&layer, (20, 15), 0.5, RED).unwrap();
        assert!(f.rect.x >= 10 && f.rect.y >= 8 && f.rect.x + f.rect.w <= 30 && f.rect.y + f.rect.h <= 22);
        assert_eq!(f.pixels.len(), f.rect.bytes());
        // Applying onto a copy changes nothing outside the rect.
        let mut after = layer.clone();
        apply(&mut after, &f);
        for y in 0..H {
            for x in 0..W {
                let inside = x >= f.rect.x && x < f.rect.x + f.rect.w && y >= f.rect.y && y < f.rect.y + f.rect.h;
                if !inside {
                    assert_eq!(get(&after, x, y), get(&layer, x, y));
                }
            }
        }
    }

    #[test]
    fn distance_is_the_largest_channel_difference() {
        assert_eq!(distance([0, 0, 0, 0], [0, 0, 0, 0]), 0.0);
        assert_eq!(distance([0, 0, 0, 0], [0, 0, 0, 255]), 1.0);
        assert!((distance([10, 20, 30, 40], [15, 20, 30, 40]) - 5.0 / 255.0).abs() < 1e-6);
    }
}

/// The pixel-at-a-time fill this file shipped before the scanline rewrite, copied verbatim. It
/// is the oracle for the equivalence tests and is compiled only for them.
#[cfg(test)]
mod reference {
    use super::*;

    pub fn flood_fill(layer: &Layer, seed: (i64, i64), threshold: f32, colour: [u8; 3]) -> Option<Fill> {
        let (w, h) = (layer.width, layer.height);
        let (rect, pixels, region_pixels) = fill_core::<u8>(
            w,
            h,
            |i| layer.pixel(i % w, i / w),
            |r| layer.read_rect(r.x, r.y, r.w, r.h),
            seed,
            threshold,
            colour,
        )?;
        Some(Fill { rect, pixels, region_pixels })
    }

    pub fn flood_fill16(layer: &Layer, seed: (i64, i64), threshold: f32, colour: [u8; 3]) -> Option<Fill16> {
        let (w, h) = (layer.width, layer.height);
        let (rect, pixels, region_pixels) = fill_core::<u16>(
            w,
            h,
            |i| layer.pixel16(i % w, i / w),
            |r| layer.read_rect16(r.x, r.y, r.w, r.h),
            seed,
            threshold,
            colour.map(eskiz_core::pixel::up8),
        )?;
        Some(Fill16 { rect, pixels, region_pixels })
    }

    fn fill_core<T: Chan>(
        w: usize,
        h: usize,
        px: impl Fn(usize) -> [T; 4],
        read: impl FnOnce(PixelRect) -> Vec<T>,
        seed: (i64, i64),
        threshold: f32,
        colour: [T; 3],
    ) -> Option<(PixelRect, Vec<T>, usize)> {
        if w == 0 || h == 0 || seed.0 < 0 || seed.1 < 0 || seed.0 >= w as i64 || seed.1 >= h as i64 {
            return None;
        }
        let threshold = threshold.clamp(0.0, 1.0);
        let seed_i = seed.1 as usize * w + seed.0 as usize;
        let seed_px = px(seed_i);

        // 0 = unseen, 1 = in the region, 2 = outside it but touching it (the antialiasing ring).
        let mut state = vec![0u8; w * h];
        let mut members: Vec<u32> = Vec::new();
        let mut ring: Vec<u32> = Vec::new();
        let mut stack: Vec<u32> = vec![seed_i as u32];
        state[seed_i] = 1;
        members.push(seed_i as u32);
        while let Some(i) = stack.pop() {
            let i = i as usize;
            let (x, y) = (i % w, i / w);
            let mut visit = |n: usize| {
                if state[n] != 0 {
                    return;
                }
                if distance_t(px(n), seed_px) <= threshold {
                    state[n] = 1;
                    members.push(n as u32);
                    stack.push(n as u32);
                } else {
                    state[n] = 2;
                    ring.push(n as u32);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }

        // Bounding box of region and ring.
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for &i in members.iter().chain(ring.iter()) {
            let (x, y) = (i as usize % w, i as usize / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let rect = PixelRect {
            x: x0,
            y: y0,
            w: x1 - x0 + 1,
            h: y1 - y0 + 1,
        };
        let mut pixels = read(rect);
        let max = T::MAXF;
        let fill = [colour[0], colour[1], colour[2], T::from_f32_round(max)];
        for &i in members.iter().chain(ring.iter()) {
            let i = i as usize;
            let p = px(i);
            let d = distance_t(p, seed_px);
            let k = 1.0 - d;
            let o = ((i / w - rect.y) * rect.w + (i % w - rect.x)) * 4;
            let mut out = [T::default(); 4];
            for c in 0..4 {
                let v = p[c].to_f32() + k * (fill[c].to_f32() - seed_px[c].to_f32());
                out[c] = T::from_f32_round(v);
            }
            // Keep the pixel a valid premultiplied colour.
            let a = out[3];
            for c in out.iter_mut().take(3) {
                if c.raw() > a.raw() {
                    *c = a;
                }
            }
            pixels[o..o + 4].copy_from_slice(&out);
        }
        Some((rect, pixels, members.len()))
    }
}

/// Seeded random layers: the scanline fill must match the reference byte for byte.
#[cfg(test)]
mod equivalence {
    use super::*;
    use super::reference;
    use eskiz_core::pixel::Depth;

    /// splitmix64: a small deterministic RNG, so no dependency is needed.
    struct Rng(u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Uniform in `0..n` (`n > 0`).
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % n as u64) as usize
        }

        /// Uniform in `lo..=hi`.
        fn range(&mut self, lo: usize, hi: usize) -> usize {
            lo + self.below(hi - lo + 1)
        }

        /// True with probability `p`.
        fn chance(&mut self, p: f64) -> bool {
            ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) < p
        }
    }

    /// A premultiplied pixel with alpha `a` and colour channels at most `a`.
    fn premul(rng: &mut Rng, a: u32) -> [u16; 4] {
        let c = |rng: &mut Rng| rng.range(0, a as usize) as u16;
        [c(rng), c(rng), c(rng), a as u16]
    }

    fn put(buf: &mut [u16], w: usize, x: usize, y: usize, p: [u16; 4]) {
        buf[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&p);
    }

    /// Row-major premultiplied RGBA with channel maximum `max` (255 or 65535), in one of seven
    /// shapes: empty, painted, blobs, noise, palette blocks, outlines, and a gradient whose
    /// values sit on the threshold boundaries.
    fn generate(rng: &mut Rng, w: usize, h: usize, max: u32, kind: usize) -> Vec<u16> {
        let mut buf = vec![0u16; w * h * 4];
        match kind {
            0 => {}
            1 => {
                // Painted: an opaque base colour, with a share of pixels replaced by other colours.
                let base = premul(rng, max);
                let noise = [0.0, 0.02, 0.3][rng.below(3)];
                for y in 0..h {
                    for x in 0..w {
                        let p = if rng.chance(noise) { premul(rng, max) } else { base };
                        put(&mut buf, w, x, y, p);
                    }
                }
            }
            2 => {
                // Blobs: discs of random colour, sometimes with a soft edge.
                for _ in 0..rng.range(1, 8) {
                    let (cx, cy) = (rng.below(w) as f32, rng.below(h) as f32);
                    let r = rng.range(1, w.max(h) / 3 + 1) as f32;
                    let a = rng.range(max as usize / 2, max as usize) as u32;
                    let p = premul(rng, a);
                    let soft = rng.chance(0.5);
                    for y in 0..h {
                        for x in 0..w {
                            let d2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
                            if d2 > r * r {
                                continue;
                            }
                            let t = if soft { 1.0 - d2 / (r * r) } else { 1.0 };
                            let q = p.map(|c| (f32::from(c) * t) as u16);
                            put(&mut buf, w, x, y, q);
                        }
                    }
                }
            }
            3 => {
                // Noise: scattered random pixels over a random density.
                let density = rng.below(100) as f64 / 100.0;
                for y in 0..h {
                    for x in 0..w {
                        if rng.chance(density) {
                            let a = rng.range(0, max as usize) as u32;
                            put(&mut buf, w, x, y, premul(rng, a));
                        }
                    }
                }
            }
            4 => {
                // Palette blocks: a few colours in a chequer of blocks, some holes.
                let palette: Vec<[u16; 4]> = (0..rng.range(2, 4)).map(|_| premul(rng, max)).collect();
                let (bw, bh) = (rng.range(1, 40), rng.range(1, 40));
                for y in 0..h {
                    for x in 0..w {
                        if rng.chance(0.1) {
                            continue;
                        }
                        let i = (x / bw + (y / bh) * 3) % palette.len();
                        put(&mut buf, w, x, y, palette[i]);
                    }
                }
            }
            5 => {
                // Outlines: opaque black rectangle borders with occasional gaps.
                for _ in 0..rng.range(1, 5) {
                    let (x0, y0) = (rng.below(w), rng.below(h));
                    let (x1, y1) = (rng.range(x0, w - 1), rng.range(y0, h - 1));
                    let t = rng.range(1, 2);
                    let gap = rng.chance(0.5);
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let edge = x < x0 + t || x + t > x1 || y < y0 + t || y + t > y1;
                            if edge && !(gap && rng.chance(0.02)) {
                                put(&mut buf, w, x, y, [0, 0, 0, max as u16]);
                            }
                        }
                    }
                }
            }
            _ => {
                // Gradient with noise: alpha climbs across the layer, so neighbours sit on either
                // side of many thresholds.
                for y in 0..h {
                    for x in 0..w {
                        let a = ((x * max as usize) / w.max(1) + rng.below(3)).min(max as usize) as u32;
                        put(&mut buf, w, x, y, premul(rng, a));
                    }
                }
            }
        }
        buf
    }

    /// Layer sizes that hit tile edges (255, 256, 257, 511, 512, 513) and one-pixel strips.
    fn pick_size(rng: &mut Rng) -> (usize, usize) {
        const EDGE: [usize; 9] = [1, 2, 3, 255, 256, 257, 511, 512, 513];
        let dim = |rng: &mut Rng| if rng.chance(0.3) { EDGE[rng.below(EDGE.len())] } else { rng.range(1, 300) };
        (dim(rng), dim(rng))
    }

    /// Seeds on the edges and corners, on tile corners, inside, and just outside the layer.
    fn pick_seed(rng: &mut Rng, w: usize, h: usize) -> (i64, i64) {
        let (wi, hi) = (w as i64, h as i64);
        match rng.below(10) {
            0 => (0, 0),
            1 => (wi - 1, 0),
            2 => (0, hi - 1),
            3 => (wi - 1, hi - 1),
            4 => (rng.below(w) as i64, 0),
            5 => (0, rng.below(h) as i64),
            6 => (255.min(wi - 1), 255.min(hi - 1)),
            7 => (256.min(wi - 1), 256.min(hi - 1)),
            8 => [(-1, 0), (0, -1), (wi, 0), (0, hi)][rng.below(4)],
            _ => (rng.below(w) as i64, rng.below(h) as i64),
        }
    }

    /// Thresholds at the clamp edges, at exact distances of common pixel values, and random.
    fn pick_threshold(rng: &mut Rng) -> f32 {
        const FIXED: [f32; 10] = [0.0, 1.0e-3, 0.1, 77.0 / 255.0, 0.5, 0.9, 1.0, -0.5, 1.5, f32::NAN];
        if rng.chance(0.6) {
            FIXED[rng.below(FIXED.len())]
        } else {
            rng.below(10_001) as f32 / 10_000.0
        }
    }

    /// The 8-bit fill (`flood_fill`) on `layer` must match the reference, rectangle, pixels,
    /// count and the bytes the fill writes into the layer.
    fn check8(layer: &Layer, seed: (i64, i64), t: f32, colour: [u8; 3]) {
        let new = flood_fill(layer, seed, t, colour);
        let old = reference::flood_fill(layer, seed, t, colour);
        let what = format!("flood_fill {}x{} depth {:?} seed {seed:?} threshold {t}", layer.width, layer.height, layer.depth());
        assert_eq!(new, old, "{what}");
        if let (Some(n), Some(o)) = (new, old) {
            let (mut a, mut b) = (layer.clone(), layer.clone());
            a.write_rect(n.rect.x, n.rect.y, n.rect.w, n.rect.h, &n.pixels);
            b.write_rect(o.rect.x, o.rect.y, o.rect.w, o.rect.h, &o.pixels);
            assert_eq!(a.to_rgba16(), b.to_rgba16(), "{what}: layer bytes");
        }
    }

    /// As [`check8`] for the 16-bit fill (`flood_fill16`).
    fn check16(layer: &Layer, seed: (i64, i64), t: f32, colour: [u8; 3]) {
        let new = flood_fill16(layer, seed, t, colour);
        let old = reference::flood_fill16(layer, seed, t, colour);
        let what = format!("flood_fill16 {}x{} depth {:?} seed {seed:?} threshold {t}", layer.width, layer.height, layer.depth());
        match (new, old) {
            (None, None) => {}
            (Some(n), Some(o)) => {
                assert_eq!((n.rect, &n.pixels, n.region_pixels), (o.rect, &o.pixels, o.region_pixels), "{what}");
                let (mut a, mut b) = (layer.clone(), layer.clone());
                a.write_rect16(n.rect.x, n.rect.y, n.rect.w, n.rect.h, &n.pixels);
                b.write_rect16(o.rect.x, o.rect.y, o.rect.w, o.rect.h, &o.pixels);
                assert_eq!(a.to_rgba16(), b.to_rgba16(), "{what}: layer bytes");
            }
            (n, o) => panic!("{what}: new {:?} vs reference {:?}", n.is_some(), o.is_some()),
        }
    }

    /// Both fills on both layer depths: 8-bit and 16-bit layers each go through each entry point.
    fn check_all(layer8: &Layer, layer16: &Layer, seed: (i64, i64), t: f32, colour: [u8; 3]) {
        check8(layer8, seed, t, colour);
        check8(layer16, seed, t, colour);
        check16(layer8, seed, t, colour);
        check16(layer16, seed, t, colour);
    }

    #[test]
    fn scanline_fill_matches_reference_on_random_layers() {
        const CASES: usize = 220;
        let mut rng = Rng(0x5EED_F100D);
        for case in 0..CASES {
            let (w, h) = pick_size(&mut rng);
            let kind = rng.below(7);
            let buf16 = generate(&mut rng, w, h, 65535, kind);
            let buf8: Vec<u8> = generate(&mut rng, w, h, 255, kind).iter().map(|&v| v as u8).collect();
            let layer8 = Layer::from_rgba(w, h, &buf8);
            let layer16 = Layer::from_rgba16(w, h, &buf16);
            assert_eq!(layer8.depth(), Depth::U8);
            assert_eq!(layer16.depth(), Depth::U16);
            for _ in 0..3 {
                let seed = pick_seed(&mut rng, w, h);
                let t = pick_threshold(&mut rng);
                let colour = [rng.below(256) as u8, rng.below(256) as u8, rng.below(256) as u8];
                eprintln!("case {case}: {w}x{h} kind {kind} seed {seed:?} threshold {t} color {colour:?}");
                check_all(&layer8, &layer16, seed, t, colour);
            }
        }
    }

    #[test]
    fn empty_and_fully_painted_layers_match_reference() {
        let sizes = [(1, 1), (1, 300), (300, 1), (255, 256), (257, 257), (513, 300)];
        for (w, h) in sizes {
            let empty8 = Layer::new(w, h);
            let empty16 = Layer::new_depth(w, h, Depth::U16);
            let paint8 = Layer::from_rgba(w, h, vec![200u8; w * h * 4]);
            let paint16 = Layer::from_rgba16(w, h, vec![51_000u16; w * h * 4]);
            let corners = [(0, 0), (w as i64 - 1, 0), (0, h as i64 - 1), (w as i64 - 1, h as i64 - 1)];
            for seed in corners {
                for t in [0.0, 0.5, 1.0] {
                    check_all(&empty8, &empty16, seed, t, [9, 80, 200]);
                    check_all(&paint8, &paint16, seed, t, [9, 80, 200]);
                }
            }
        }
    }
}
