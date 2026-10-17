//! CPU dab rasteriser, matching the GPU dab and grain shader maths.
//!
//! A dab is a rotated quad of `sx x sy` px centred on `(x, y)`. Each covered pixel
//! samples the shape texture and, for moving grain, the grain texture, and
//! accumulates coverage into a [`CoverageBuffer`]. Colour is constant per stroke,
//! so the stroke buffer holds coverage (the A channel) only.
//!
//! Textures are procedural and match `shapeTextureData` / `grainTextureData` in
//! the JS. Values are quantised to 8 bits like the JS textures, and sampled
//! bilinearly with GL-style texel centres.

use crate::brushes::{Accumulation, Brush, Grain, GrainMode, Shape};
use crate::geometry::{clamp01, Dab, Rng};

/// Shape texture resolution (renderer.js `_shapeTexture`).
pub const SHAPE_TEX_SIZE: usize = 128;
/// Grain texture resolution (renderer.js `_grainTexture`).
pub const GRAIN_TEX_SIZE: usize = 256;
/// Seed used for the procedural grain (renderer.js `grainTextureData`).
pub const GRAIN_SEED: u32 = 7;

/// Quantise to 8 bits and back, as the JS RGBA8 textures do.
fn quantise8(v: f64) -> f32 {
    ((clamp01(v) * 255.0).round() / 255.0) as f32
}

/// `floor(x)` as an integer, for texture coordinates. One saturating cast, no libm call (without
/// SSE4.1 `f64::floor` is one, and every extra `as i64` costs a handful of instructions).
#[inline(always)]
fn floor_i64(x: f64) -> i64 {
    let t = x as i64;
    if (t as f64) > x {
        t - 1
    } else {
        t
    }
}

/// Bilinear lookup with GL texel centres (`uv * n - 0.5`). `WRAP` selects REPEAT,
/// otherwise CLAMP_TO_EDGE.
#[inline(always)]
fn bilinear<const WRAP: bool>(data: &[f32], n: usize, u: f64, v: f64) -> f64 {
    let ni = n as i64;
    let fx = u * n as f64 - 0.5;
    let fy = v * n as f64 - 0.5;
    let (xi, yi) = (floor_i64(fx), floor_i64(fy));
    let tx = fx - xi as f64;
    let ty = fy - yi as f64;
    let (v00, v10, v01, v11);
    if xi >= 0 && yi >= 0 && xi + 1 < ni && yi + 1 < ni {
        // Inside the texture: all four texels are neighbours, no clamping or wrapping.
        let base = yi as usize * n + xi as usize;
        let (r0, r1) = (&data[base..base + 2], &data[base + n..base + n + 2]);
        (v00, v10, v01, v11) = (f64::from(r0[0]), f64::from(r0[1]), f64::from(r1[0]), f64::from(r1[1]));
    } else {
        let idx = |i: i64| -> usize {
            if WRAP {
                i.rem_euclid(ni) as usize
            } else {
                i.clamp(0, ni - 1) as usize
            }
        };
        let (ix0, ix1) = (idx(xi), idx(xi + 1));
        let (iy0, iy1) = (idx(yi), idx(yi + 1));
        v00 = f64::from(data[iy0 * n + ix0]);
        v10 = f64::from(data[iy0 * n + ix1]);
        v01 = f64::from(data[iy1 * n + ix0]);
        v11 = f64::from(data[iy1 * n + ix1]);
    }
    let top = v00 + (v10 - v00) * tx;
    let bottom = v01 + (v11 - v01) * tx;
    top + (bottom - top) * ty
}

/// Bilinear taps along one axis: texel indices and the weight of the second.
struct Tap {
    i0: usize,
    i1: usize,
    t: f64,
}

/// Brush tip texture: alpha is the tip shape (white = paint).
#[derive(Clone, Debug)]
pub struct ShapeTexture {
    n: usize,
    alpha: Vec<f32>,
}

impl ShapeTexture {
    /// `softRound` uses a smoothstep falloff. `hardRound` is a one-texel anti-aliased disc.
    pub fn new(shape: Shape, n: usize) -> Self {
        let mut alpha = vec![0.0f32; n * n];
        let nf = n as f64;
        for y in 0..n {
            for x in 0..n {
                let dx = ((x as f64 + 0.5) / nf) * 2.0 - 1.0;
                let dy = ((y as f64 + 0.5) / nf) * 2.0 - 1.0;
                let r = dx.hypot(dy);
                let a = match shape {
                    Shape::HardRound => clamp01((1.0 - r) * (nf / 2.0) + 0.5),
                    Shape::SoftRound => {
                        let t = clamp01(1.0 - r);
                        t * t * (3.0 - 2.0 * t)
                    }
                };
                alpha[y * n + x] = quantise8(a);
            }
        }
        Self { n, alpha }
    }

    /// A tip from an `n x n` 8-bit grey mask, row-major, white (255) is paint. Returns `None`
    /// when `bytes.len() != n * n` or `n == 0`. This is how imported and library brushes
    /// supply their own tip (see `DabRaster::with_textures`).
    pub fn from_gray(n: usize, bytes: &[u8]) -> Option<Self> {
        if n == 0 || bytes.len() != n * n {
            return None;
        }
        Some(Self {
            n,
            alpha: bytes.iter().map(|&b| f32::from(b) / 255.0).collect(),
        })
    }

    /// Edge length of the square texture.
    pub fn size(&self) -> usize {
        self.n
    }

    /// The two texels and the weight of the second, along one axis, for coordinate `u` in [0, 1]
    /// (the per-axis half of [`sample`](Self::sample), so an axis-aligned dab computes it once
    /// per column and per row instead of once per pixel).
    #[inline]
    fn taps(&self, u: f64) -> Tap {
        let ni = self.n as i64;
        let f = u * self.n as f64 - 0.5;
        let i = floor_i64(f);
        Tap { i0: i.clamp(0, ni - 1) as usize, i1: (i + 1).clamp(0, ni - 1) as usize, t: f - i as f64 }
    }

    /// Bilinear value from the taps of a column and a row: the same arithmetic as `sample`.
    #[inline(always)]
    fn sample_taps(&self, col: &Tap, row: &Tap) -> f64 {
        let n = self.n;
        let v00 = f64::from(self.alpha[row.i0 * n + col.i0]);
        let v10 = f64::from(self.alpha[row.i0 * n + col.i1]);
        let v01 = f64::from(self.alpha[row.i1 * n + col.i0]);
        let v11 = f64::from(self.alpha[row.i1 * n + col.i1]);
        let top = v00 + (v10 - v00) * col.t;
        let bottom = v01 + (v11 - v01) * col.t;
        top + (bottom - top) * row.t
    }

    /// Sample at `(u, v)` in [0, 1]^2 (clamped at the edges).
    #[inline]
    pub fn sample(&self, u: f64, v: f64) -> f64 {
        bilinear::<false>(&self.alpha, self.n, u, v)
    }
}

/// Tileable grain texture (value in 0..1).
#[derive(Clone, Debug)]
pub struct GrainTexture {
    n: usize,
    value: Vec<f32>,
}

impl GrainTexture {
    /// `Noise` is white noise. `Paper` is three passes of a tileable 3x3 box blur,
    /// stretched to the full range. `Grain::None` is treated as noise.
    pub fn new(kind: Grain, n: usize, seed: u32) -> Self {
        let mut rng = Rng::new(seed);
        let mut f: Vec<f32> = (0..n * n).map(|_| rng.next_f64() as f32).collect();
        if kind == Grain::Paper {
            for _ in 0..3 {
                let mut g = vec![0.0f32; n * n];
                for y in 0..n {
                    for x in 0..n {
                        let mut s = 0.0f64;
                        for dy in [n - 1, 0, 1] {
                            for dx in [n - 1, 0, 1] {
                                let xx = (x + dx) % n;
                                let yy = (y + dy) % n;
                                s += f64::from(f[yy * n + xx]);
                            }
                        }
                        g[y * n + x] = (s / 9.0) as f32;
                    }
                }
                f = g;
            }
            let lo = f.iter().copied().fold(f32::INFINITY, f32::min);
            let hi = f.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let span = f64::from(hi - lo);
            let span = if span == 0.0 { 1.0 } else { span };
            for v in f.iter_mut() {
                *v = ((f64::from(*v) - f64::from(lo)) / span) as f32;
            }
        }
        let value = f.into_iter().map(|v| quantise8(f64::from(v))).collect();
        Self { n, value }
    }

    /// A grain from an `n x n` 8-bit grey tile, row-major. Returns `None` when
    /// `bytes.len() != n * n` or `n == 0`.
    pub fn from_gray(n: usize, bytes: &[u8]) -> Option<Self> {
        if n == 0 || bytes.len() != n * n {
            return None;
        }
        Some(Self {
            n,
            value: bytes.iter().map(|&b| f32::from(b) / 255.0).collect(),
        })
    }

    /// Edge length of the square tile.
    pub fn size(&self) -> usize {
        self.n
    }

    /// Sample at `(u, v)` with REPEAT wrapping.
    #[inline]
    pub fn sample(&self, u: f64, v: f64) -> f64 {
        bilinear::<true>(&self.value, self.n, u, v)
    }
}

/// Everything the dab and composite stages need from a brush.
#[derive(Clone, Debug)]
pub struct DabRaster {
    shape: ShapeTexture,
    grain: Option<GrainTexture>,
    /// Moving grain: sampled per stamp inside the dab.
    grain_per_stamp: bool,
    /// Texturized grain: sampled per pixel at composite time.
    grain_texturized: bool,
    invert_shape: bool,
    invert_grain: bool,
    grain_scale: f64,
    grain_depth: f64,
    grain_contrast: f64,
    grain_brightness: f64,
    accumulation: Accumulation,
}

fn nonzero_or(x: f64, default: f64) -> f64 {
    if x == 0.0 {
        default
    } else {
        x
    }
}

impl DabRaster {
    /// Textures and grain settings for `brush`. Zero `grain_scale` means 1.
    pub fn new(brush: &Brush) -> Self {
        let grain_on = brush.grain != Grain::None;
        Self::with_textures(
            brush,
            ShapeTexture::new(brush.shape, SHAPE_TEX_SIZE),
            grain_on.then(|| GrainTexture::new(brush.grain, GRAIN_TEX_SIZE, GRAIN_SEED)),
        )
    }

    /// Like [`DabRaster::new`] but with caller-supplied tip and grain textures, e.g. from
    /// `eskiz-brushlib` or an imported `.brush`. `grain` of `None` turns grain off whatever
    /// `brush.grain` says; a `Some` grain is used when `brush.grain` is not `Grain::None`.
    pub fn with_textures(brush: &Brush, shape: ShapeTexture, grain: Option<GrainTexture>) -> Self {
        let grain = if brush.grain == Grain::None { None } else { grain };
        let grain_on = grain.is_some();
        Self {
            shape,
            grain,
            grain_per_stamp: grain_on && brush.grain_mode == GrainMode::Moving,
            grain_texturized: grain_on && brush.grain_mode != GrainMode::Moving,
            invert_shape: brush.invert_shape,
            invert_grain: brush.invert_grain,
            grain_scale: nonzero_or(brush.grain_scale, 1.0),
            grain_depth: brush.grain_depth,
            grain_contrast: brush.grain_contrast,
            grain_brightness: brush.grain_brightness,
            accumulation: brush.accumulation,
        }
    }

    /// Grain tone: invert, then contrast and brightness, clamped to [0, 1].
    fn tone(&self, g: f64) -> f64 {
        let g = if self.invert_grain { 1.0 - g } else { g };
        clamp01((self.grain_contrast + 1.0) * (g - 0.5) + self.grain_brightness + 0.5)
    }

    /// Whether [`texturized_tone`](Self::texturized_tone) can return a tone.
    #[inline]
    pub fn has_texturized_grain(&self) -> bool {
        self.grain_texturized && self.grain.is_some()
    }

    /// Texturized grain at pixel `(px, py)` as `(tone, depth)`, used by the
    /// composite stage. `None` when grain is off or moving.
    #[inline]
    pub fn texturized_tone(&self, px: i64, py: i64) -> Option<(f64, f64)> {
        if !self.grain_texturized {
            return None;
        }
        let grain = self.grain.as_ref()?;
        let size = grain.size() as f64 * self.grain_scale;
        let g = grain.sample((px as f64 + 0.5) / size, (py as f64 + 0.5) / size);
        Some((self.tone(g), self.grain_depth))
    }
}

/// Axis-aligned bounds `(min_x, min_y, max_x, max_y)` of a dab quad, in px.
pub fn dab_bounds(d: &Dab) -> (f64, f64, f64, f64) {
    let hx = d.sx / 2.0;
    let hy = d.sy / 2.0;
    let (c, s) = (d.angle.cos(), d.angle.sin());
    let ex = (hx * c).abs() + (hy * s).abs();
    let ey = (hx * s).abs() + (hy * c).abs();
    (d.x - ex, d.y - ey, d.x + ex, d.y + ey)
}

/// Inclusive pixel rectangle `(x0, y0, x1, y1)` covering `dabs`, clipped to a
/// `width x height` canvas. `None` when nothing is visible.
pub fn dab_pixel_rect(dabs: &[Dab], width: usize, height: usize) -> Option<(i64, i64, i64, i64)> {
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for d in dabs {
        if !(d.sx > 0.0 && d.sy > 0.0) {
            continue;
        }
        let (a, b, c, e) = dab_bounds(d);
        bounds = Some(match bounds {
            None => (a, b, c, e),
            Some((x0, y0, x1, y1)) => (x0.min(a), y0.min(b), x1.max(c), y1.max(e)),
        });
    }
    let (min_x, min_y, max_x, max_y) = bounds?;
    let x0 = (min_x.floor() as i64 - 1).max(0);
    let y0 = (min_y.floor() as i64 - 1).max(0);
    let x1 = (max_x.ceil() as i64 + 1).min(width as i64 - 1);
    let y1 = (max_y.ceil() as i64 + 1).min(height as i64 - 1);
    if x0 > x1 || y0 > y1 {
        None
    } else {
        Some((x0, y0, x1, y1))
    }
}

/// Edge, in pixels, of the square chunks the stroke buffers are allocated in.
pub(crate) const CHUNK: usize = 64;

/// Stroke-wide coverage (the A channel of the stroke buffer), limited to a pixel rectangle.
/// Coordinates passed to `get` and `splat` are canvas pixels.
///
/// The buffer is stored in 64 x 64 chunks that are allocated the first time a dab touches them
/// (a missing chunk is all zero), so a stroke costs memory in proportion to the area it
/// covers, not to the canvas or to its bounding box, and nothing is ever copied as it grows.
#[derive(Clone, Debug)]
pub struct CoverageBuffer {
    x0: i64,
    y0: i64,
    width: usize,
    height: usize,
    /// Chunk columns.
    cols: usize,
    /// `rows * cols` chunks, row-major; `None` is all zero.
    chunks: Vec<Option<Box<[f32]>>>,
}

fn new_chunk() -> Box<[f32]> {
    vec![0.0; CHUNK * CHUNK].into_boxed_slice()
}

impl CoverageBuffer {
    /// Empty buffer covering `width x height` pixels from `(x0, y0)`. No pixel memory is
    /// allocated until something is painted.
    pub fn new(x0: i64, y0: i64, width: usize, height: usize) -> Self {
        let (cols, rows) = (width.div_ceil(CHUNK), height.div_ceil(CHUNK));
        Self { x0, y0, width, height, cols, chunks: vec![None; cols * rows] }
    }

    /// Coverage at canvas pixel `(x, y)`; zero outside the buffer.
    #[inline]
    pub fn get(&self, x: i64, y: i64) -> f64 {
        let lx = x - self.x0;
        let ly = y - self.y0;
        if lx < 0 || ly < 0 || lx >= self.width as i64 || ly >= self.height as i64 {
            return 0.0;
        }
        let (lx, ly) = (lx as usize, ly as usize);
        match &self.chunks[ly / CHUNK * self.cols + lx / CHUNK] {
            Some(c) => f64::from(c[ly % CHUNK * CHUNK + lx % CHUNK]),
            None => 0.0,
        }
    }

    /// Write the coverage of canvas pixels `x0..x1` on row `y` to `out` (`x1 - x0` values; zero
    /// outside the buffer).
    pub fn read_row(&self, y: i64, x0: i64, x1: i64, out: &mut [f32]) {
        out.fill(0.0);
        let ly = y - self.y0;
        if ly < 0 || ly >= self.height as i64 {
            return;
        }
        let lo = (x0 - self.x0).max(0);
        let hi = (x1 - self.x0).min(self.width as i64);
        let ly = ly as usize;
        let mut lx = lo;
        while lx < hi {
            let cx = lx as usize / CHUNK;
            let seg_end = (((cx + 1) * CHUNK) as i64).min(hi);
            if let Some(c) = &self.chunks[ly / CHUNK * self.cols + cx] {
                let from = ly % CHUNK * CHUNK + lx as usize % CHUNK;
                let n = (seg_end - lx) as usize;
                let dst = (lx + self.x0 - x0) as usize;
                out[dst..dst + n].copy_from_slice(&c[from..from + n]);
            }
            lx = seg_end;
        }
    }

    /// Reset the coverage to zero inside the inclusive canvas rectangle `(x0, y0, x1, y1)`.
    pub fn clear_rect(&mut self, (x0, y0, x1, y1): (i64, i64, i64, i64)) {
        let lo_x = (x0 - self.x0).max(0);
        let hi_x = (x1 - self.x0).min(self.width as i64 - 1);
        let lo_y = (y0 - self.y0).max(0);
        let hi_y = (y1 - self.y0).min(self.height as i64 - 1);
        if lo_x > hi_x || lo_y > hi_y {
            return;
        }
        let (lo_x, hi_x, lo_y, hi_y) = (lo_x as usize, hi_x as usize, lo_y as usize, hi_y as usize);
        for cy in lo_y / CHUNK..=hi_y / CHUNK {
            for cx in lo_x / CHUNK..=hi_x / CHUNK {
                let Some(c) = &mut self.chunks[cy * self.cols + cx] else {
                    continue;
                };
                let (cx0, cy0) = (cx * CHUNK, cy * CHUNK);
                let (a, b) = (lo_x.max(cx0) - cx0, hi_x.min(cx0 + CHUNK - 1) - cx0);
                for ly in lo_y.max(cy0) - cy0..=hi_y.min(cy0 + CHUNK - 1) - cy0 {
                    c[ly * CHUNK + a..=ly * CHUNK + b].fill(0.0);
                }
            }
        }
    }

    /// Reset all coverage to zero and give the memory back.
    pub fn clear_all(&mut self) {
        self.chunks.iter_mut().for_each(|c| *c = None);
    }

    /// Pixels of memory allocated so far (whole chunks), for tests and diagnostics.
    pub fn allocated_pixels(&self) -> usize {
        self.chunks.iter().flatten().count() * CHUNK * CHUNK
    }

    /// Rasterise `dabs` into the buffer with the stroke's accumulation mode.
    pub fn splat(&mut self, dabs: &[Dab], raster: &DabRaster) {
        for d in dabs {
            self.splat_dab(d, raster, None);
        }
    }

    /// Like [`splat`](Self::splat), but only the pixels inside `clip`, an inclusive canvas
    /// rectangle `(x0, y0, x1, y1)`, are touched. Each pixel gets the same value as an unclipped
    /// splat would give it, so re-splatting the dabs that reach a cleared rectangle, in their
    /// original order, rebuilds exactly that part of the buffer.
    pub fn splat_clipped(&mut self, dabs: &[Dab], raster: &DabRaster, clip: (i64, i64, i64, i64)) {
        for d in dabs {
            self.splat_dab(d, raster, Some(clip));
        }
    }

    /// The pixel range `(lo_x, lo_y, hi_x, hi_y)` (inclusive) a dab can touch, inside the buffer
    /// and `clip`. `None` when the dab is degenerate or the range is empty.
    fn dab_range(&self, d: &Dab, clip: Option<(i64, i64, i64, i64)>) -> Option<(i64, i64, i64, i64)> {
        if !(d.sx > 0.0 && d.sy > 0.0) {
            return None;
        }
        let (bx0, by0, bx1, by1) = dab_bounds(d);
        let mut lo_x = (bx0.floor() as i64 - 1).max(self.x0);
        let mut lo_y = (by0.floor() as i64 - 1).max(self.y0);
        let mut hi_x = (bx1.ceil() as i64 + 1).min(self.x0 + self.width as i64 - 1);
        let mut hi_y = (by1.ceil() as i64 + 1).min(self.y0 + self.height as i64 - 1);
        if let Some((cx0, cy0, cx1, cy1)) = clip {
            (lo_x, lo_y, hi_x, hi_y) = (lo_x.max(cx0), lo_y.max(cy0), hi_x.min(cx1), hi_y.min(cy1));
        }
        (lo_x <= hi_x && lo_y <= hi_y).then_some((lo_x, lo_y, hi_x, hi_y))
    }

    /// Splat one dab without a per-pixel callback. A dab covering at least [`PAR_DAB_AREA`]
    /// pixels is split into bands of chunk rows, one thread each (chunk rows are disjoint).
    fn splat_dab(&mut self, d: &Dab, r: &DabRaster, clip: Option<(i64, i64, i64, i64)>) {
        let Some((lo_x, lo_y, hi_x, hi_y)) = self.dab_range(d, clip) else {
            return;
        };
        let splat = DabSplat::new(d, r, lo_x, hi_x, 1.0);
        let (x0, y0, cols) = (self.x0, self.y0, self.cols);
        let first = ((lo_y - y0) as usize) / CHUNK;
        let last = ((hi_y - y0) as usize) / CHUNK;
        let nrows = last - first + 1;
        let area = (hi_y - lo_y + 1) as usize * (hi_x - lo_x + 1) as usize;
        let threads = if area >= PAR_DAB_AREA && nrows >= 2 {
            crate::parallelism::cpus().min(8).min(nrows)
        } else {
            1
        };
        let grid = &mut self.chunks[first * cols..(last + 1) * cols];
        if threads <= 1 {
            splat.rows(grid, cols, (x0, y0), first, (lo_y, hi_y), |_, _, _, _, _| {});
            return;
        }
        std::thread::scope(|scope| {
            let mut rest = grid;
            let mut cy = first;
            for t in 0..threads {
                let n = nrows / threads + usize::from(t < nrows % threads);
                let (band, tail) = rest.split_at_mut(n * cols);
                rest = tail;
                let band_first = cy;
                cy += n;
                let splat = &splat;
                scope.spawn(move || {
                    let band_lo = lo_y.max(y0 + (band_first * CHUNK) as i64);
                    let band_hi = hi_y.min(y0 + ((band_first + n) * CHUNK) as i64 - 1);
                    splat.rows(band, cols, (x0, y0), band_first, (band_lo, band_hi), |_, _, _, _, _| {});
                });
            }
        });
    }

    /// Rasterise one dab with its alpha scaled by `alpha_scale`. After each covered pixel is
    /// accumulated, `on_pixel(px, py, old, new, cov)` is called with the canvas pixel, the
    /// accumulated coverage before and after, and the dab's own coverage there.
    /// With a scale of 1 and a no-op callback this is exactly [`CoverageBuffer::splat`].
    pub(crate) fn splat_one_with(
        &mut self,
        d: &Dab,
        r: &DabRaster,
        alpha_scale: f64,
        on_pixel: impl FnMut(i64, i64, f32, f32, f64),
    ) {
        let Some((lo_x, lo_y, hi_x, hi_y)) = self.dab_range(d, None) else {
            return;
        };
        let splat = DabSplat::new(d, r, lo_x, hi_x, alpha_scale);
        let (x0, y0, cols) = (self.x0, self.y0, self.cols);
        let first = ((lo_y - y0) as usize) / CHUNK;
        let last = ((hi_y - y0) as usize) / CHUNK;
        splat.rows(&mut self.chunks[first * cols..(last + 1) * cols], cols, (x0, y0), first, (lo_y, hi_y), on_pixel);
    }

    /// Pixel rectangle `(x0, y0, width, height)` this buffer covers.
    pub fn bounds(&self) -> (i64, i64, usize, usize) {
        (self.x0, self.y0, self.width, self.height)
    }

    /// Set the coverage at canvas pixel `(x, y)`. Outside the buffer this does nothing.
    pub(crate) fn set(&mut self, x: i64, y: i64, v: f32) {
        let (lx, ly) = (x - self.x0, y - self.y0);
        if lx >= 0 && ly >= 0 && lx < self.width as i64 && ly < self.height as i64 {
            let (lx, ly) = (lx as usize, ly as usize);
            let chunk = self.chunks[ly / CHUNK * self.cols + lx / CHUNK].get_or_insert_with(new_chunk);
            chunk[ly % CHUNK * CHUNK + lx % CHUNK] = v;
        }
    }
}

/// Dabs covering at least this many pixels are splatted by several threads.
const PAR_DAB_AREA: usize = 256 * 256;

/// One dab, ready to be accumulated into chunk rows of a coverage buffer.
struct DabSplat<'a> {
    d: &'a Dab,
    r: &'a DabRaster,
    /// Cosine and sine of the dab angle.
    c: f64,
    s: f64,
    /// Half the quad size.
    hx: f64,
    hy: f64,
    /// Opacity times flow, and the caller's extra factor.
    alpha: f64,
    scale: f64,
    lo_x: i64,
    hi_x: i64,
    /// For a dab with angle 0 and a tip that is the shape texture alone: then the quad's `u`
    /// depends only on the column and `v` only on the row (with `c = 1`, `s = 0` the rotation
    /// `lx = dx * c + dy * s`, `ly = -dx * s + dy * c` is `lx = dx`, `ly = dy` exactly), so the
    /// texture taps are computed once per column (here) and per row. The arithmetic per pixel is
    /// the same as without, so the coverage is bit-for-bit the same.
    columns: Option<Vec<Option<Tap>>>,
}

impl<'a> DabSplat<'a> {
    fn new(d: &'a Dab, r: &'a DabRaster, lo_x: i64, hi_x: i64, scale: f64) -> Self {
        let (c, s) = (d.angle.cos(), d.angle.sin());
        let (hx, hy) = (d.sx / 2.0, d.sy / 2.0);
        let columns = (c == 1.0 && s == 0.0 && r.separable_tip()).then(|| {
            (lo_x..=hi_x)
                .map(|px| {
                    let dx = px as f64 + 0.5 - d.x;
                    (dx.abs() <= hx).then(|| r.shape.taps(dx / d.sx + 0.5))
                })
                .collect()
        });
        Self { d, r, c, s, hx, hy, alpha: clamp01(d.opacity * d.flow), scale, lo_x, hi_x, columns }
    }

    /// Accumulate the dab into pixel rows `lo_y..=hi_y` (canvas rows). `grid` holds whole chunk
    /// rows of a buffer with origin `(x0, y0)` and `cols` chunk columns, the first being chunk
    /// row `first_cy`. Chunks the dab reaches are allocated.
    fn rows(
        &self,
        grid: &mut [Option<Box<[f32]>>],
        cols: usize,
        (x0, y0): (i64, i64),
        first_cy: usize,
        (lo_y, hi_y): (i64, i64),
        mut on_pixel: impl FnMut(i64, i64, f32, f32, f64),
    ) {
        let (d, r) = (self.d, self.r);
        let chunk = CHUNK as i64;
        for py in lo_y..=hi_y {
            // Everything that depends only on the row, hoisted out of the pixel loop.
            let dy = py as f64 + 0.5 - d.y;
            let (dys, dyc) = (dy * self.s, dy * self.c);
            let row_tap = match &self.columns {
                Some(_) => {
                    if dy.abs() > self.hy {
                        continue;
                    }
                    Some(r.shape.taps(dy / d.sy + 0.5))
                }
                None => None,
            };
            let (ly, cy) = ((py - y0) as usize % CHUNK, (py - y0) as usize / CHUNK - first_cy);
            for cx in ((self.lo_x - x0) / chunk)..=((self.hi_x - x0) / chunk) {
                let seg_lo = self.lo_x.max(x0 + cx * chunk);
                let seg_hi = self.hi_x.min(x0 + cx * chunk + chunk - 1);
                let chunk_data = grid[cy * cols + cx as usize].get_or_insert_with(new_chunk);
                let start = ly * CHUNK + (seg_lo - x0 - cx * chunk) as usize;
                let seg = &mut chunk_data[start..start + (seg_hi - seg_lo + 1) as usize];
                // Build-up or glaze, into one cell: returns (old, new, the dab's own coverage).
                let accumulate = |cell: &mut f32, value: f64| {
                    let cov = clamp01(value) * self.alpha * self.scale;
                    let a = f64::from(*cell);
                    let next = match r.accumulation {
                        // Glaze: overlapping dabs never exceed the stroke opacity.
                        Accumulation::Glaze => a.max(cov),
                        // Build-up: alpha-over.
                        Accumulation::Buildup => a + (1.0 - a) * cov,
                    };
                    *cell = next as f32;
                    (a as f32, next as f32, cov)
                };
                if let (Some(columns), Some(row_tap)) = (&self.columns, &row_tap) {
                    for (cell, px) in seg.iter_mut().zip(seg_lo..=seg_hi) {
                        let Some(col) = &columns[(px - self.lo_x) as usize] else {
                            continue;
                        };
                        let v = r.shape.sample_taps(col, row_tap);
                        let (old, new, cov) = accumulate(cell, if r.invert_shape { 1.0 - v } else { v });
                        on_pixel(px, py, old, new, cov);
                    }
                } else {
                    for (cell, px) in seg.iter_mut().zip(seg_lo..=seg_hi) {
                        let dx = px as f64 + 0.5 - d.x;
                        let lx = dx * self.c + dys;
                        let ly = -dx * self.s + dyc;
                        if lx.abs() > self.hx || ly.abs() > self.hy {
                            continue;
                        }
                        let (old, new, cov) = accumulate(cell, r.tip_at(d, lx / d.sx + 0.5, ly / d.sy + 0.5));
                        on_pixel(px, py, old, new, cov);
                    }
                }
            }
        }
    }
}

impl DabRaster {
    /// Whether the tip value is the shape texture alone (no moving grain).
    #[inline]
    fn separable_tip(&self) -> bool {
        !(self.grain_per_stamp && self.grain.is_some())
    }

    /// How overlapping dabs accumulate in a stroke.
    pub fn accumulation(&self) -> Accumulation {
        self.accumulation
    }

    /// Tip value (shape with moving grain, before opacity and flow) of dab `d` at the centre of
    /// canvas pixel `(px, py)`, where `c` and `s` are the cosine and sine of the dab angle.
    /// `None` outside the dab's quad.
    pub(crate) fn tip_value(&self, d: &Dab, c: f64, s: f64, px: i64, py: i64) -> Option<f64> {
        let (hx, hy) = (d.sx / 2.0, d.sy / 2.0);
        // Map the pixel centre into the quad's local frame.
        let dx = px as f64 + 0.5 - d.x;
        let dy = py as f64 + 0.5 - d.y;
        let lx = dx * c + dy * s;
        let ly = -dx * s + dy * c;
        if lx.abs() > hx || ly.abs() > hy {
            return None;
        }
        Some(self.tip_at(d, lx / d.sx + 0.5, ly / d.sy + 0.5))
    }

    /// Tip value at the point `(u, v)` of the dab's quad (both in 0..1).
    #[inline(always)]
    fn tip_at(&self, d: &Dab, u: f64, v: f64) -> f64 {
        // Shape, then (moving) grain multiplied in.
        let mut shape = self.shape.sample(u, v);
        if self.invert_shape {
            shape = 1.0 - shape;
        }
        let mut value = shape;
        if self.grain_per_stamp {
            if let Some(grain) = &self.grain {
                let gu = u / self.grain_scale + d.grain_x;
                let gv = v / self.grain_scale + d.grain_y;
                let tone = self.tone(grain.sample(gu, gv));
                value = shape * ((1.0 - self.grain_depth) + tone * self.grain_depth);
            }
        }
        value
    }
}

#[cfg(test)]
mod texture_tests {
    use super::*;

    #[test]
    fn custom_textures_replace_the_built_in_ones() {
        assert!(ShapeTexture::from_gray(4, &[0; 15]).is_none());
        assert!(GrainTexture::from_gray(0, &[]).is_none());
        // A tip that is paint on the left half only.
        let mut bytes = vec![0u8; 16 * 16];
        for y in 0..16 {
            for x in 0..8 {
                bytes[y * 16 + x] = 255;
            }
        }
        let tip = ShapeTexture::from_gray(16, &bytes).unwrap();
        assert_eq!(tip.size(), 16);
        assert!(tip.sample(0.1, 0.5) > 0.99);
        assert!(tip.sample(0.9, 0.5) < 0.01);

        let brush = Brush::default();
        let raster = DabRaster::with_textures(&brush, tip, None);
        assert!(raster.texturized_tone(3, 3).is_none());
        // A grain given for a brush with grain off is ignored.
        let grain = GrainTexture::from_gray(2, &[0, 255, 255, 0]).unwrap();
        let tip2 = ShapeTexture::new(Shape::HardRound, 8);
        assert!(DabRaster::with_textures(&brush, tip2.clone(), Some(grain.clone()))
            .texturized_tone(0, 0)
            .is_none());
        let grainy = Brush {
            grain: Grain::Noise,
            grain_depth: 1.0,
            ..Brush::default()
        };
        assert!(DabRaster::with_textures(&grainy, tip2, Some(grain))
            .texturized_tone(0, 0)
            .is_some());
    }
}

#[cfg(test)]
mod splat_tests {
    use super::*;
    use crate::brushes::GrainMode;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as f64 / (1u64 << 31) as f64
        }
    }

    /// Whether two buffers of the same rectangle hold the same bits.
    fn same(a: &CoverageBuffer, b: &CoverageBuffer) -> bool {
        let (x0, y0, w, h) = a.bounds();
        (y0..y0 + h as i64).all(|y| (x0..x0 + w as i64).all(|x| a.get(x, y).to_bits() == b.get(x, y).to_bits()))
    }

    /// The textbook version: every pixel of the dab's range through `tip_value`, one at a time.
    fn reference_splat(buf: &mut CoverageBuffer, dabs: &[Dab], r: &DabRaster, clip: Option<(i64, i64, i64, i64)>) {
        for d in dabs {
            let Some((lo_x, lo_y, hi_x, hi_y)) = buf.dab_range(d, clip) else {
                continue;
            };
            let (c, s) = (d.angle.cos(), d.angle.sin());
            let alpha = clamp01(d.opacity * d.flow);
            for py in lo_y..=hi_y {
                for px in lo_x..=hi_x {
                    let Some(value) = r.tip_value(d, c, s, px, py) else {
                        continue;
                    };
                    let cov = clamp01(value) * alpha;
                    let a = buf.get(px, py);
                    let next = match r.accumulation {
                        Accumulation::Glaze => a.max(cov),
                        Accumulation::Buildup => a + (1.0 - a) * cov,
                    };
                    buf.set(px, py, next as f32);
                }
            }
        }
    }

    #[test]
    fn splat_matches_the_per_pixel_reference() {
        let mut rng = Lcg(99);
        for case in 0..24u32 {
            let brush = Brush {
                shape: if case % 2 == 0 { Shape::SoftRound } else { Shape::HardRound },
                accumulation: if case % 3 == 0 { Accumulation::Glaze } else { Accumulation::Buildup },
                invert_shape: case % 7 == 5,
                grain: if case % 4 == 3 { Grain::Paper } else { Grain::None },
                grain_mode: GrainMode::Moving,
                grain_depth: 0.6,
                ..Brush::default()
            };
            let r = DabRaster::new(&brush);
            // 700 x 520 buffer at an offset; the dabs run over its edges. Some are wide enough
            // (over 256 x 256 px) for the threaded splat; some have angle 0 (the axis path).
            let mut buf = CoverageBuffer::new(-30, 20, 700, 520);
            let mut want = buf.clone();
            let dabs: Vec<Dab> = (0..6)
                .map(|_| {
                    let size = if case % 5 == 0 { 300.0 + 150.0 * rng.next() } else { 10.0 + 120.0 * rng.next() };
                    Dab {
                        x: -50.0 + 750.0 * rng.next(),
                        y: 540.0 * rng.next(),
                        sx: size,
                        sy: if case % 6 == 1 { size * 0.4 } else { size },
                        angle: if case % 3 == 2 { std::f64::consts::TAU * rng.next() } else { 0.0 },
                        opacity: 0.2 + 0.8 * rng.next(),
                        flow: 0.2 + 0.8 * rng.next(),
                        grain_x: rng.next(),
                        grain_y: rng.next(),
                        ..Dab::default()
                    }
                })
                .collect();
            let clip = (case % 4 == 1).then_some((100, 100, 400, 300));
            match clip {
                Some(c) => buf.splat_clipped(&dabs, &r, c),
                None => buf.splat(&dabs, &r),
            }
            reference_splat(&mut want, &dabs, &r, clip);
            assert!(same(&buf, &want), "case {case}");
            assert!(clip.is_some() || buf.allocated_pixels() > 0, "case {case} painted nothing");
        }
    }

    #[test]
    fn clear_rect_and_clipped_splat_rebuild_a_region() {
        let r = DabRaster::new(&Brush { accumulation: Accumulation::Buildup, ..Brush::default() });
        let dab = |x: f64| Dab { x, y: 60.0, sx: 50.0, sy: 50.0, opacity: 0.8, flow: 0.5, ..Dab::default() };
        let dabs: Vec<Dab> = (0..8).map(|i| dab(20.0 + 12.0 * i as f64)).collect();
        let mut whole = CoverageBuffer::new(0, 0, 200, 120);
        whole.splat(&dabs, &r);
        let mut part = whole.clone();
        let region = (60, 20, 110, 100);
        part.clear_rect(region);
        assert_eq!(part.get(80, 60), 0.0);
        part.splat_clipped(&dabs, &r, region);
        assert!(same(&part, &whole));
    }

    #[test]
    fn chunks_are_allocated_only_where_dabs_reach_and_read_back_like_a_flat_buffer() {
        let r = DabRaster::new(&Brush::default());
        let mut b = CoverageBuffer::new(0, 0, 8000, 8000);
        assert_eq!(b.allocated_pixels(), 0);
        b.splat(&[Dab { x: 4000.0, y: 4000.0, sx: 20.0, sy: 20.0, opacity: 1.0, flow: 1.0, ..Dab::default() }], &r);
        assert!(b.allocated_pixels() > 0 && b.allocated_pixels() < 100 * 100, "{}", b.allocated_pixels());
        assert!(b.get(4000, 4000) > 0.9);
        assert_eq!(b.get(10, 10), 0.0);
        // read_row agrees with get, across chunk edges and off the buffer.
        let mut row = vec![1.0f32; 120];
        b.read_row(4000, 3940, 4060, &mut row);
        for (k, v) in row.iter().enumerate() {
            assert_eq!(f64::from(*v), b.get(3940 + k as i64, 4000));
        }
        let mut edge = vec![1.0f32; 10];
        b.read_row(-1, 0, 10, &mut edge);
        assert!(edge.iter().all(|&v| v == 0.0));
        b.clear_all();
        assert_eq!(b.allocated_pixels(), 0);
        assert_eq!(b.get(4000, 4000), 0.0);
    }
}
