//! Warp mesh: a 4 x 4 grid of control points, 3 spans per axis, evaluated as a
//! bicubic Bernstein surface, plus the tile-count formula used for tessellation.

use crate::geom::{unit_square_to_quad, IRect, Mat3, Point, Quad, Rect};
use crate::image::Image;
use crate::mask::Mask;
use crate::resample::{Interp, Sampler};

/// Slack on tile-local coordinates, so pixel centres on shared tile edges are never dropped.
const TILE_EPS: f32 = 1e-3;

/// Source pixels a sample may reach past the mesh edge. The bicubic kernel reaches two pixels,
/// so boundary tiles are extended by this much (Eskiz's choice; the design notes do not give it).
const EDGE_REACH_PX: f32 = 2.0;

/// Control points per axis.
pub const CONTROL_COUNT: usize = 4;
/// Spans per axis (control points minus one).
pub const SPANS: usize = 3;
/// Smallest tile count per axis.
pub const MIN_TILES_PER_AXIS: usize = 3;
/// Largest tile count per axis.
pub const MAX_TILES_PER_AXIS: usize = 64_000;
/// Tile budget used when none is given.
pub const DEFAULT_MAX_TILES: f32 = 40_000.0;
/// Control point pick radius in screen pixels.
pub const PICK_RADIUS_PX: f32 = 24.0;

/// Depth operation on a control point. Depth only decides which overlapping parts
/// of the warped surface sit in front.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DepthOp {
    Front,
    Back,
    Keep,
    Flip,
}

/// Applies a depth operation. Front is +1, Back is -1, Keep leaves the value, and Flip negates
/// it (the flip uses a fixed magnitude).
pub fn apply_depth(current: f32, op: DepthOp) -> f32 {
    match op {
        DepthOp::Front => 1.0,
        DepthOp::Back => -1.0,
        DepthOp::Keep => current,
        DepthOp::Flip => -current,
    }
}

/// Warp control mesh.
#[derive(Debug, Clone, PartialEq)]
pub struct Warp {
    /// Control points indexed `ctrl[i][j]`: `i` runs along u (x), `j` along v (y).
    pub ctrl: [[Point; CONTROL_COUNT]; CONTROL_COUNT],
    /// Depth per control point, same indexing as `ctrl`.
    pub depth: [[f32; CONTROL_COUNT]; CONTROL_COUNT],
}

impl Warp {
    /// The evenly spread grid over `bounds`. Evaluating it returns the original rectangle.
    pub fn identity(bounds: Rect) -> Self {
        let mut ctrl = [[Point::default(); CONTROL_COUNT]; CONTROL_COUNT];
        for (i, column) in ctrl.iter_mut().enumerate() {
            for (j, point) in column.iter_mut().enumerate() {
                let fu = i as f32 / SPANS as f32;
                let fv = j as f32 / SPANS as f32;
                *point = Point::new(
                    bounds.x0 + bounds.width() * fu,
                    bounds.y0 + bounds.height() * fv,
                );
            }
        }
        Self {
            ctrl,
            depth: [[1.0; CONTROL_COUNT]; CONTROL_COUNT],
        }
    }

    /// Surface point at parameters `(u, v)` in `[0, 1]^2`:
    /// `S(u, v) = sum_ij B_i(u) B_j(v) P_ij` with cubic Bernstein weights.
    pub fn evaluate(&self, u: f32, v: f32) -> Point {
        let bu = bernstein(u);
        let bv = bernstein(v);
        let mut p = Point::default();
        for (i, column) in self.ctrl.iter().enumerate() {
            for (j, cp) in column.iter().enumerate() {
                p = p + *cp * (bu[i] * bv[j]);
            }
        }
        p
    }

    /// Surface samples at tile centres, `t = (k + 0.5) / N` on each axis. Row-major, `nx` per row.
    pub fn tile_centres(&self, nx: usize, ny: usize) -> Vec<Point> {
        let mut out = Vec::with_capacity(nx * ny);
        for j in 0..ny {
            for i in 0..nx {
                let u = (i as f32 + 0.5) / nx as f32;
                let v = (j as f32 + 0.5) / ny as f32;
                out.push(self.evaluate(u, v));
            }
        }
        out
    }

    /// Nearest control point within `radius` of `p`, as `(i, j)`.
    pub fn pick(&self, p: Point, radius: f32) -> Option<(usize, usize)> {
        let mut best: Option<(f32, (usize, usize))> = None;
        for (i, column) in self.ctrl.iter().enumerate() {
            for (j, cp) in column.iter().enumerate() {
                let d = cp.distance(p);
                if d > radius {
                    continue;
                }
                let better = match best {
                    Some((bd, _)) => d < bd,
                    None => true,
                };
                if better {
                    best = Some((d, (i, j)));
                }
            }
        }
        best.map(|(_, idx)| idx)
    }

    /// Moves one control point by `delta`.
    pub fn drag_control(&mut self, i: usize, j: usize, delta: Point) {
        if let Some(p) = self.ctrl.get_mut(i).and_then(|c| c.get_mut(j)) {
            *p = *p + delta;
        }
    }

    /// Moves the four control points of the span containing `(u, v)` by `delta`.
    /// The per-corner weights are not specified; all four move fully here.
    pub fn drag_span(&mut self, u: f32, v: f32, delta: Point) {
        let (i, j) = span_of(u, v);
        for (ci, cj) in [(i, j), (i + 1, j), (i, j + 1), (i + 1, j + 1)] {
            self.drag_control(ci, cj, delta);
        }
    }

    /// Sets the depth operation on one control point.
    pub fn set_depth(&mut self, i: usize, j: usize, op: DepthOp) {
        if let Some(d) = self.depth.get_mut(i).and_then(|r| r.get_mut(j)) {
            *d = apply_depth(*d, op);
        }
    }

    /// Depth of the surface at `(u, v)`, interpolated with the same Bernstein weights as the
    /// positions.
    fn evaluate_depth(&self, u: f32, v: f32) -> f32 {
        let bu = bernstein(u);
        let bv = bernstein(v);
        let mut d = 0.0;
        for (i, row) in self.depth.iter().enumerate() {
            for (j, &value) in row.iter().enumerate() {
                d += value * bu[i] * bv[j];
            }
        }
        d
    }

    /// Renders `src` through the mesh into a `dst_width` by `dst_height` image.
    ///
    /// Eskiz's choice where the design notes are open: the parameter square of `source_rect` is
    /// split into the tile grid from [`tile_counts`]. Each tile's four corners are evaluated on
    /// the surface, giving a destination quad. A destination pixel centre inside that quad is
    /// mapped back to the tile's source rectangle through the tile's projective map, then sampled
    /// with `interp`. Tiles are drawn in ascending order of surface depth at their centre, so
    /// higher depth overwrites lower and folded meshes resolve. Pixels outside every tile stay
    /// transparent.
    pub fn render(
        &self,
        src: &Image,
        source_rect: Rect,
        dst_width: usize,
        dst_height: usize,
        interp: Interp,
    ) -> Image {
        let (region, part) = self.render_region(&Sampler::new(src), source_rect, dst_width, dst_height, interp);
        embed(region, part, dst_width, dst_height)
    }

    /// [`Warp::render`] for a prepared sampler, returning only the destination rectangle that
    /// the mesh covers (and that rectangle), so a mesh over a large canvas does not need a
    /// canvas-sized float image.
    pub(crate) fn render_region(
        &self,
        sampler: &Sampler<'_>,
        source_rect: Rect,
        dst_width: usize,
        dst_height: usize,
        interp: Interp,
    ) -> (IRect, Image) {
        let (nx, ny) = tile_counts(source_rect.width(), source_rect.height(), DEFAULT_MAX_TILES);
        let (fnx, fny) = (nx as f32, ny as f32);
        let step = Point::new(source_rect.width() / fnx, source_rect.height() / fny);
        let mut tiles = Vec::with_capacity(nx * ny);
        for j in 0..ny {
            for i in 0..nx {
                let (u0, u1) = (i as f32 / fnx, (i + 1) as f32 / fnx);
                let (v0, v1) = (j as f32 / fny, (j + 1) as f32 / fny);
                let quad: Quad = [
                    self.evaluate(u0, v0),
                    self.evaluate(u1, v0),
                    self.evaluate(u1, v1),
                    self.evaluate(u0, v1),
                ];
                // Relative to the first corner, so the projective solve keeps precision.
                let origin = quad[0];
                let relative = quad.map(|p| p - origin);
                let Some(to_quad) = unit_square_to_quad(&relative) else {
                    continue;
                };
                let Some(to_local) = to_quad.inverse() else {
                    continue;
                };
                // Boundary tiles reach EDGE_REACH_PX source pixels past the mesh edge, so the
                // pixels that the affine path also draws next to the edge are drawn here too.
                // Interior tiles keep only a tolerance for shared edges.
                let margin_x = if step.x > 0.0 {
                    EDGE_REACH_PX / step.x
                } else {
                    0.0
                };
                let margin_y = if step.y > 0.0 {
                    EDGE_REACH_PX / step.y
                } else {
                    0.0
                };
                let range = [
                    if i == 0 { -margin_x } else { -TILE_EPS },
                    if i + 1 == nx {
                        1.0 + margin_x
                    } else {
                        1.0 + TILE_EPS
                    },
                    if j == 0 { -margin_y } else { -TILE_EPS },
                    if j + 1 == ny {
                        1.0 + margin_y
                    } else {
                        1.0 + TILE_EPS
                    },
                ];
                tiles.push(Tile {
                    origin,
                    to_quad,
                    to_local,
                    range,
                    src_origin: Point::new(
                        source_rect.x0 + u0 * source_rect.width(),
                        source_rect.y0 + v0 * source_rect.height(),
                    ),
                    src_step: step,
                    depth: self.evaluate_depth((u0 + u1) * 0.5, (v0 + v1) * 0.5),
                });
            }
        }
        // Stable sort: equal depths keep grid order.
        tiles.sort_by(|a, b| a.depth.total_cmp(&b.depth));

        // The destination rectangle that any tile reaches, then the tiles drawn into it.
        let reach = |tile: &Tile| -> Option<(usize, usize, usize, usize)> {
            let bounds = tile.destination_bounds()?;
            let x0 = bounds.x0.floor().max(0.0) as usize;
            let y0 = bounds.y0.floor().max(0.0) as usize;
            let x1 = (bounds.x1.ceil().max(0.0) as usize).min(dst_width);
            let y1 = (bounds.y1.ceil().max(0.0) as usize).min(dst_height);
            (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
        };
        let mut region: Option<(usize, usize, usize, usize)> = None;
        for tile in &tiles {
            if let Some((x0, y0, x1, y1)) = reach(tile) {
                region = Some(match region {
                    None => (x0, y0, x1, y1),
                    Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
                });
            }
        }
        let Some((rx0, ry0, rx1, ry1)) = region else {
            return (IRect { x: 0, y: 0, w: 0, h: 0 }, Image::transparent(0, 0));
        };
        let rect = IRect::from_bounds(rx0, ry0, rx1, ry1);
        let mut out = Image::transparent(rect.w, rect.h);
        for tile in &tiles {
            let Some(bounds) = tile.destination_bounds() else {
                continue;
            };
            let x0 = bounds.x0.floor().max(0.0) as usize;
            let y0 = bounds.y0.floor().max(0.0) as usize;
            let x1 = (bounds.x1.ceil().max(0.0) as usize).min(dst_width);
            let y1 = (bounds.y1.ceil().max(0.0) as usize).min(dst_height);
            for y in y0..y1 {
                for x in x0..x1 {
                    let centre = Point::new(x as f32 + 0.5, y as f32 + 0.5) - tile.origin;
                    let Some(local) = tile.to_local.apply(centre) else {
                        continue;
                    };
                    let [x_lo, x_hi, y_lo, y_hi] = tile.range;
                    if local.x < x_lo || local.x > x_hi || local.y < y_lo || local.y > y_hi {
                        continue;
                    }
                    // Unclamped: the source position is the tile's linear map, which may sit
                    // up to EDGE_REACH_PX outside the source rectangle on boundary tiles.
                    let p = Point::new(
                        tile.src_origin.x + local.x * tile.src_step.x,
                        tile.src_origin.y + local.y * tile.src_step.y,
                    );
                    out.pixels[(y - rect.y) * rect.w + (x - rect.x)] = sampler.sample(p, interp);
                }
            }
        }
        (rect, out)
    }

    /// [`Warp::render`] for a mask: the mask's values pass through the same mesh and kernels.
    pub fn render_mask(
        &self,
        mask: &Mask,
        source_rect: Rect,
        dst_width: usize,
        dst_height: usize,
        interp: Interp,
    ) -> Mask {
        let (region, part) = self.render_region(&Sampler::from_mask(mask), source_rect, dst_width, dst_height, interp);
        let mut out = Mask { width: dst_width, height: dst_height, data: vec![0.0; dst_width * dst_height] };
        for (r, row) in part.pixels.chunks(region.w.max(1)).enumerate() {
            let at = (region.y + r) * dst_width + region.x;
            for (dst, p) in out.data[at..at + region.w].iter_mut().zip(row) {
                *dst = p[0].clamp(0.0, 1.0);
            }
        }
        out
    }

    /// The mesh with every control point multiplied by `s` (the mesh as seen on a copy of the
    /// canvas scaled by `s`).
    pub fn scaled(&self, s: f32) -> Warp {
        let mut out = self.clone();
        for column in out.ctrl.iter_mut() {
            for p in column.iter_mut() {
                *p = Point::new(p.x * s, p.y * s);
            }
        }
        out
    }
}

/// One tile of the warp mesh, ready to rasterise.
struct Tile {
    /// First corner of the destination quad; the tile's coordinates are taken relative to it.
    origin: Point,
    /// Maps the tile's local unit square to the destination (relative to `origin`).
    to_quad: Mat3,
    /// Maps destination (relative to `origin`) to the tile's local unit square.
    to_local: Mat3,
    /// Accepted local range: `[x_lo, x_hi, y_lo, y_hi]`.
    range: [f32; 4],
    /// Source position of the tile's local (0, 0) corner.
    src_origin: Point,
    /// Source extent of one tile along x and y.
    src_step: Point,
    /// Surface depth at the tile centre, used for draw order.
    depth: f32,
}

impl Tile {
    /// Destination bounds of the accepted local range, from its projected corners.
    fn destination_bounds(&self) -> Option<Rect> {
        let [x_lo, x_hi, y_lo, y_hi] = self.range;
        let corners = [(x_lo, y_lo), (x_hi, y_lo), (x_hi, y_hi), (x_lo, y_hi)];
        let mut points = corners.iter().filter_map(|&(u, v)| {
            self.to_quad
                .apply(Point::new(u, v))
                .map(|p| p + self.origin)
        });
        let first = points.next()?;
        let mut bounds = Rect::new(first.x, first.y, first.x, first.y);
        for p in points {
            bounds.x0 = bounds.x0.min(p.x);
            bounds.y0 = bounds.y0.min(p.y);
            bounds.x1 = bounds.x1.max(p.x);
            bounds.y1 = bounds.y1.max(p.y);
        }
        Some(bounds)
    }
}

/// Span `(i, j)` containing parameters `(u, v)`, clamped to `0..SPANS`.
pub fn span_of(u: f32, v: f32) -> (usize, usize) {
    (span_index(u), span_index(v))
}

fn span_index(t: f32) -> usize {
    (t * SPANS as f32).floor().clamp(0.0, (SPANS - 1) as f32) as usize
}

/// Cubic Bernstein basis at `t`: `(1-t)^3, 3t(1-t)^2, 3t^2(1-t), t^3`.
fn bernstein(t: f32) -> [f32; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

/// Tile counts for content of `width` by `height` pixels:
/// `s = max(0.01, sqrt(W * H / max_tiles))` and per axis `N = clamp(floor(size / s), 3, 64000)`.
pub fn tile_counts(width: f32, height: f32, max_tiles: f32) -> (usize, usize) {
    let s = (width * height / max_tiles).sqrt().max(0.01);
    (axis_tiles(width, s), axis_tiles(height, s))
}

fn axis_tiles(size: f32, s: f32) -> usize {
    let n = (size / s).floor().max(1.0);
    n.clamp(MIN_TILES_PER_AXIS as f32, MAX_TILES_PER_AXIS as f32) as usize
}

/// Place the `region` of a canvas-sized transparent image: `part` is the pixels of that region.
pub(crate) fn embed(region: IRect, part: Image, width: usize, height: usize) -> Image {
    if region.x == 0 && region.y == 0 && region.w == width && region.h == height {
        return part;
    }
    let mut out = Image::transparent(width, height);
    for (r, row) in part.pixels.chunks(region.w.max(1)).enumerate().take(region.h) {
        let at = (region.y + r) * width + region.x;
        out.pixels[at..at + region.w].copy_from_slice(row);
    }
    out
}
