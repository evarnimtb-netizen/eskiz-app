//! The tile side of the `.procreate` writer: turns each layer's pixels into the compressed
//! `<col>~<row>.lz4` members of the package without ever building a canvas-sized buffer.
//!
//! * A layer's pixels come from dense buffers (`LayerSpec::pixels` / `pixels16`) or from a
//!   [`TileSource`], whose allocated tiles are read in place.
//! * Tiles are cut, checked, measured and compressed in parallel (scoped threads); the members
//!   come out in a fixed order (layers in tree order, tiles row by row), so the archive does
//!   not depend on the number of threads.
//! * The flattened composite that the package carries (and its thumbnail) is built one tile at
//!   a time. Compositing is per pixel, so a tile-wise pass gives the same bytes as the
//!   canvas-wise one; the thumbnail's box filter is accumulated from the composite tiles as
//!   they are made.
//!
//! The output is byte-identical to the dense implementation that preceded it (kept under
//! `cfg(test)` as the baseline the tests compare against).

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::codecs::apple_lz4_compress;
use crate::document::{LayerSpec, Node};
use crate::{Error, Result};

/// Where a layer's pixels live when they are held as tiles rather than one dense buffer.
///
/// A tile is `edge x edge` pixels of premultiplied RGBA with rows `edge` pixels apart, top row
/// first. The tile at `(col, row)` covers canvas pixels `col * edge ..` and `row * edge ..`;
/// the part of an edge tile beyond the canvas is ignored. A missing tile is fully transparent.
/// `edge` must equal the builder's tile size.
pub trait TileSource: Send + Sync {
    /// Edge of one tile in pixels.
    fn edge(&self) -> usize;
    /// Bits per channel of the stored tiles: 8 or 16.
    fn bits(&self) -> u32;
    /// The 8-bit tile (`edge * edge * 4` samples) at `(col, row)`, if the source is 8-bit and the
    /// tile is allocated.
    fn tile8(&self, col: usize, row: usize) -> Option<&[u8]>;
    /// The 16-bit tile (`edge * edge * 4` samples) at `(col, row)`, if the source is 16-bit and
    /// the tile is allocated.
    fn tile16(&self, col: usize, row: usize) -> Option<&[u16]>;
}

/// A shared [`TileSource`] (see `LayerSpec::tiles`). Cloning is cheap; two are equal when they
/// are the same source.
#[derive(Clone)]
pub struct LayerTiles(pub Arc<dyn TileSource>);

impl LayerTiles {
    pub fn new(source: impl TileSource + 'static) -> Self {
        Self(Arc::new(source))
    }
}

impl fmt::Debug for LayerTiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LayerTiles({}-bit, edge {})", self.0.bits(), self.0.edge())
    }
}

impl PartialEq for LayerTiles {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

// ------------------------------------------------------------------------------ threading

/// CPUs for worker pools; Android caps at 4 (phones throttle on many threads).
pub(crate) fn worker_cpus() -> usize {
    let n = std::thread::available_parallelism().map_or(1, usize::from);
    #[cfg(target_os = "android")]
    let n = n.min(4);
    n
}

pub(crate) fn worker_count(jobs: usize) -> usize {
    let cpus = worker_cpus();
    cpus.min(jobs).max(1)
}

/// Runs `f(state, job)` for every job in `0..jobs` on a few scoped threads (each with its own
/// `state` from `init`) and returns the results in job order.
pub(crate) fn par_map_with<S, T: Send>(
    jobs: usize,
    init: impl Fn() -> S + Sync,
    f: impl Fn(&mut S, usize) -> T + Sync,
) -> Vec<T> {
    par_map_with_threads(usize::MAX, jobs, init, f)
}

/// [`par_map_with`] on at most `max_threads` threads.
pub(crate) fn par_map_with_threads<S, T: Send>(
    max_threads: usize,
    jobs: usize,
    init: impl Fn() -> S + Sync,
    f: impl Fn(&mut S, usize) -> T + Sync,
) -> Vec<T> {
    let threads = worker_count(jobs).min(max_threads).max(1);
    if threads <= 1 {
        let mut state = init();
        return (0..jobs).map(|j| f(&mut state, j)).collect();
    }
    let next = AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, T)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut state = init();
                    let mut mine = Vec::new();
                    loop {
                        let j = next.fetch_add(1, Ordering::Relaxed);
                        if j >= jobs {
                            break;
                        }
                        mine.push((j, f(&mut state, j)));
                    }
                    mine
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
            .collect()
    });
    let mut slots: Vec<Option<T>> = (0..jobs).map(|_| None).collect();
    for (j, value) in parts.into_iter().flatten() {
        slots[j] = Some(value);
    }
    slots.into_iter().map(|s| s.expect("every job ran")).collect()
}

// ------------------------------------------------------------------------------ geometry

/// The canvas and its tile grid.
#[derive(Clone, Copy)]
pub(crate) struct Grid {
    pub w: usize,
    pub h: usize,
    pub t: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Grid {
    pub fn new(w: usize, h: usize, t: usize) -> Self {
        Self { w, h, t, cols: w.div_ceil(t), rows: h.div_ceil(t) }
    }

    /// Origin and in-bounds size `(x0, y0, tw, th)` of tile `(col, row)`.
    pub fn rect(&self, col: usize, row: usize) -> (usize, usize, usize, usize) {
        let (x0, y0) = (col * self.t, row * self.t);
        (x0, y0, self.t.min(self.w - x0), self.t.min(self.h - y0))
    }
}

/// Bounding box `(x0, y0, x1, y1)` (exclusive end) in canvas pixels.
pub(crate) type Bounds = (usize, usize, usize, usize);

fn union(a: Option<Bounds>, b: Bounds) -> Option<Bounds> {
    Some(match a {
        None => b,
        Some((x0, y0, x1, y1)) => (x0.min(b.0), y0.min(b.1), x1.max(b.2), y1.max(b.3)),
    })
}

/// One layer's tile members and the bounds of its non-transparent pixels.
#[derive(Default)]
pub(crate) struct LayerOut {
    /// `(member name, payload)`, in archive order.
    pub files: Vec<(String, Vec<u8>)>,
    pub content: Option<Bounds>,
}

// ------------------------------------------------------------------------------ sources

/// The pixel source of a layer at the depth being written.
#[derive(Clone, Copy)]
pub(crate) enum Src<'a> {
    Empty,
    D8(&'a [u8]),
    D16(&'a [u16]),
    Tiled(&'a dyn TileSource),
}

/// The source a layer writes at `bits` per channel. Tiles win over dense buffers. For dense
/// buffers a 16-bit archive prefers `pixels16` and an 8-bit one `pixels` (the other is
/// converted).
pub(crate) fn resolve(spec: &LayerSpec, bits: u32) -> Src<'_> {
    if let Some(t) = &spec.tiles {
        return Src::Tiled(&*t.0);
    }
    if bits == 16 {
        match (&spec.pixels16, &spec.pixels) {
            (Some(p), _) => Src::D16(p),
            (None, Some(p)) => Src::D8(p),
            (None, None) => Src::Empty,
        }
    } else {
        match (&spec.pixels, &spec.pixels16) {
            (Some(p), _) => Src::D8(p),
            (None, Some(p)) => Src::D16(p),
            (None, None) => Src::Empty,
        }
    }
}

fn down16(v: u16) -> u8 {
    ((u32::from(v) + 128) / 257) as u8
}

/// Whether the source holds data for tile `(col, row)` (a dense buffer always does).
fn has_data(src: &Src<'_>, col: usize, row: usize) -> bool {
    match src {
        Src::Empty => false,
        Src::D8(_) | Src::D16(_) => true,
        Src::Tiled(s) => {
            if s.bits() == 16 {
                s.tile16(col, row).is_some()
            } else {
                s.tile8(col, row).is_some()
            }
        }
    }
}

/// The in-bounds pixels of tile `(col, row)` as `tw * th` pixels, rows `tw` pixels apart, at
/// `bits_out` per channel (little-endian samples when 16): `None` when the source holds nothing
/// for the tile. 8 bits widens by `v * 257` into 16, and 16 rounds down into 8 (`down16`).
/// The slice borrows the source when no conversion or cropping is needed, `scratch` otherwise.
fn fetch<'a>(
    src: &Src<'a>,
    bits_out: u32,
    g: &Grid,
    col: usize,
    row: usize,
    scratch: &'a mut Vec<u8>,
) -> Option<&'a [u8]> {
    let (x0, y0, tw, th) = g.rect(col, row);
    scratch.clear();
    match *src {
        Src::Empty => None,
        Src::D8(p) => {
            if bits_out == 8 {
                for y in 0..th {
                    let o = ((y0 + y) * g.w + x0) * 4;
                    scratch.extend_from_slice(&p[o..o + tw * 4]);
                }
            } else {
                for y in 0..th {
                    let o = ((y0 + y) * g.w + x0) * 4;
                    scratch.extend(p[o..o + tw * 4].iter().flat_map(|&v| (u16::from(v) * 257).to_le_bytes()));
                }
            }
            Some(&scratch[..])
        }
        Src::D16(p) => {
            if bits_out == 16 {
                for y in 0..th {
                    let o = ((y0 + y) * g.w + x0) * 4;
                    scratch.extend(p[o..o + tw * 4].iter().flat_map(|v| v.to_le_bytes()));
                }
            } else {
                for y in 0..th {
                    let o = ((y0 + y) * g.w + x0) * 4;
                    scratch.extend(p[o..o + tw * 4].iter().map(|&v| down16(v)));
                }
            }
            Some(&scratch[..])
        }
        Src::Tiled(s) => {
            let edge = s.edge();
            if s.bits() == 16 {
                let tile = s.tile16(col, row)?;
                for y in 0..th {
                    let r = &tile[y * edge * 4..y * edge * 4 + tw * 4];
                    if bits_out == 16 {
                        scratch.extend(r.iter().flat_map(|v| v.to_le_bytes()));
                    } else {
                        scratch.extend(r.iter().map(|&v| down16(v)));
                    }
                }
                Some(&scratch[..])
            } else {
                let tile = s.tile8(col, row)?;
                if bits_out == 8 {
                    if tw == edge && th == edge {
                        return Some(&tile[..tw * th * 4]);
                    }
                    for y in 0..th {
                        scratch.extend_from_slice(&tile[y * edge * 4..y * edge * 4 + tw * 4]);
                    }
                } else {
                    for y in 0..th {
                        let r = &tile[y * edge * 4..y * edge * 4 + tw * 4];
                        scratch.extend(r.iter().flat_map(|&v| (u16::from(v) * 257).to_le_bytes()));
                    }
                }
                Some(&scratch[..])
            }
        }
    }
}

// ------------------------------------------------------------------------------ measuring

/// Bounds `(x0, y0, x1, y1)` (relative to the tile, exclusive end) of the pixels that have any
/// non-zero byte, in `tw * th` pixels of `bpp` bytes. `None` when every byte is zero.
fn nonzero_bounds(buf: &[u8], tw: usize, th: usize, bpp: usize) -> Option<Bounds> {
    let mut bounds: Option<Bounds> = None;
    let stride = tw * bpp;
    for y in 0..th {
        let row = &buf[y * stride..(y + 1) * stride];
        if row.iter().all(|&b| b == 0) {
            continue;
        }
        let nonzero = |p: &[u8]| p.iter().any(|&b| b != 0);
        let first = row.chunks_exact(bpp).position(nonzero).unwrap_or(0);
        let last = row.chunks_exact(bpp).rposition(nonzero).unwrap_or(0);
        bounds = union(bounds, (first, y, last + 1, y + 1));
    }
    bounds
}

fn member(uuid: &str, col: usize, row: usize) -> String {
    format!("{uuid}/{col}~{row}.lz4")
}

// ------------------------------------------------------------------------------ layers

/// Compresses the tiles of every layer. `layers` are `` in archive order. Returns
/// one [`LayerOut`] per layer, in the same order.
pub(crate) fn encode_layers(layers: &[(&LayerSpec, &str)], g: &Grid, bits: u32) -> Vec<LayerOut> {
    let srcs: Vec<Src<'_>> = layers.iter().map(|(spec, _)| resolve(spec, bits)).collect();
    let per_layer = g.cols * g.rows;
    let bpp = (bits / 2) as usize;
    let jobs = layers.len() * per_layer;
    type Tile = Option<(Bounds, Vec<u8>)>;
    let results: Vec<Tile> = par_map_with(jobs, Vec::<u8>::new, |scratch, j| {
        let (layer, pos) = (j / per_layer.max(1), j % per_layer.max(1));
        let (row, col) = (pos / g.cols, pos % g.cols);
        let (x0, y0, tw, th) = g.rect(col, row);
        let buf = fetch(&srcs[layer], bits, g, col, row, scratch)?;
        let (bx0, by0, bx1, by1) = nonzero_bounds(buf, tw, th, bpp)?;
        Some(((x0 + bx0, y0 + by0, x0 + bx1, y0 + by1), apple_lz4_compress(buf)))
    });
    let mut outs: Vec<LayerOut> = layers.iter().map(|_| LayerOut::default()).collect();
    for (j, tile) in results.into_iter().enumerate() {
        let Some((bounds, data)) = tile else { continue };
        let layer = j / per_layer;
        let pos = j % per_layer;
        let (row, col) = (pos / g.cols, pos % g.cols);
        let out = &mut outs[layer];
        out.content = union(out.content, bounds);
        out.files.push((member(layers[layer].1, col, row), data));
    }
    outs
}

/// Compresses a layer mask (`g.w * g.h` grey bytes) as an inverted-fill layer under `uuid`:
/// grey opaque tiles at `bits` per channel, and no member for a tile that shows everything (a
/// missing mask tile reads as 255). The first tile is always written: a mask with no tiles at
/// all reads back as no mask.
pub(crate) fn encode_mask(mask: &[u8], g: &Grid, bits: u32, uuid: &str) -> LayerOut {
    let mut out = LayerOut::default();
    let mut buf = Vec::new();
    for row in 0..g.rows {
        for col in 0..g.cols {
            let (x0, y0, tw, th) = g.rect(col, row);
            let line = |y: usize| &mask[(y0 + y) * g.w + x0..(y0 + y) * g.w + x0 + tw];
            if (row, col) != (0, 0) && (0..th).all(|y| line(y).iter().all(|&v| v == 255)) {
                continue;
            }
            buf.clear();
            for y in 0..th {
                for &v in line(y) {
                    if bits == 16 {
                        let wide = (u16::from(v) * 257).to_le_bytes();
                        buf.extend_from_slice(&[wide[0], wide[1], wide[0], wide[1], wide[0], wide[1], 0xFF, 0xFF]);
                    } else {
                        buf.extend_from_slice(&[v, v, v, 0xFF]);
                    }
                }
            }
            out.files.push((member(uuid, col, row), apple_lz4_compress(&buf)));
        }
    }
    out
}

// ------------------------------------------------------------------------------ composite

/// A node of the layer tree as the composite sees it: always at 8 bits.
pub(crate) enum CNode<'a> {
    Layer { hidden: bool, opacity: f64, src: Src<'a> },
    Group { hidden: bool, opacity: f64, children: Vec<CNode<'a>> },
}

pub(crate) fn composite_nodes(nodes: &[Node]) -> Vec<CNode<'_>> {
    nodes
        .iter()
        .map(|n| match n {
            Node::Layer(l) => CNode::Layer { hidden: l.hidden, opacity: l.opacity, src: resolve_composite(l) },
            Node::Group(g) => CNode::Group { hidden: g.hidden, opacity: g.opacity, children: composite_nodes(&g.children) },
        })
        .collect()
}

/// The composite reads `pixels` before `pixels16` whatever the archive's depth.
fn resolve_composite(spec: &LayerSpec) -> Src<'_> {
    resolve(spec, 8)
}

/// One pixel of premultiplied `over`: `dst = src * k + dst * (1 - src_alpha * k)`.
#[inline(always)]
pub(crate) fn over_px(d: &mut [u8], s: &[u8], k: f32) {
    // A transparent source leaves the destination exactly as it is.
    if s[0] | s[1] | s[2] | s[3] == 0 {
        return;
    }
    // An opaque source at full strength replaces it (`inv` is exactly 0).
    if s[3] == 255 && k == 1.0 {
        d.copy_from_slice(s);
        return;
    }
    let inv = 1.0 - f32::from(s[3]) / 255.0 * k;
    for (dc, sc) in d.iter_mut().zip(s.iter()) {
        let v = f32::from(*sc) * k + f32::from(*dc) * inv;
        *dc = v.round().clamp(0.0, 255.0) as u8;
    }
}

pub(crate) fn over_tile(dst: &mut [u8], src: &[u8], opacity: f64) {
    let k = opacity.clamp(0.0, 1.0) as f32;
    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0.iter()) {
        over_px(d, s, k);
    }
}

/// Whether any visible layer of `nodes` has data at the tile.
fn any_data(nodes: &[CNode<'_>], col: usize, row: usize) -> bool {
    nodes.iter().any(|n| match n {
        CNode::Layer { hidden, src, .. } => !hidden && has_data(src, col, row),
        CNode::Group { hidden, children, .. } => !hidden && any_data(children, col, row),
    })
}

/// Composites the tile of `nodes` (top-most first, painted from the end) into `acc`, which
/// holds `tw * th` zeroed pixels.
fn composite_tile(nodes: &[CNode<'_>], g: &Grid, col: usize, row: usize, acc: &mut [u8], scratch: &mut Vec<u8>) {
    for node in nodes.iter().rev() {
        match node {
            CNode::Layer { hidden, opacity, src } => {
                if *hidden {
                    continue;
                }
                if let Some(px) = fetch(src, 8, g, col, row, scratch) {
                    over_tile(acc, px, *opacity);
                }
            }
            CNode::Group { hidden, opacity, children } => {
                if *hidden || !any_data(children, col, row) {
                    continue;
                }
                let mut inner = vec![0u8; acc.len()];
                composite_tile(children, g, col, row, &mut inner, scratch);
                over_tile(acc, &inner, *opacity);
            }
        }
    }
}

// ------------------------------------------------------------------------------ thumbnail

/// The thumbnail's box filter: the output size and the integer sums of premultiplied values
/// under each output pixel, accumulated tile by tile.
pub(crate) struct Thumb {
    pub ow: usize,
    pub oh: usize,
    xmap: Vec<u32>,
    ymap: Vec<u32>,
    sums: Mutex<Vec<i64>>,
}

/// Longest side of the package thumbnail.
pub(crate) const THUMBNAIL_MAX: usize = 1024;

/// Output size of the thumbnail of a `w x h` canvas.
pub(crate) fn thumbnail_size(w: usize, h: usize) -> (usize, usize) {
    let longest = w.max(h);
    if longest <= THUMBNAIL_MAX {
        (w, h)
    } else {
        let scale = THUMBNAIL_MAX as f64 / longest as f64;
        (((w as f64 * scale).round() as usize).max(1), ((h as f64 * scale).round() as usize).max(1))
    }
}

/// Source range `[start, end)` of output index `o` of `n` along an axis of `len` pixels.
fn span(o: usize, n: usize, len: usize) -> (usize, usize) {
    let start = o * len / n;
    (start, ((o + 1) * len / n).max(start + 1))
}

impl Thumb {
    /// A filter for a `w x h` canvas whose every pixel starts as `background` (premultiplied;
    /// zeros when there is none).
    pub fn new(w: usize, h: usize, background: [u8; 4]) -> Self {
        let (ow, oh) = thumbnail_size(w, h);
        // The boxes partition the canvas (ow <= w, oh <= h), so output index = the box whose
        // start is the last one at or before the pixel.
        let map = |len: usize, n: usize| -> Vec<u32> { (0..len).map(|x| (((x + 1) * n - 1) / len) as u32).collect() };
        let (xmap, ymap) = (map(w, ow), map(h, oh));
        let mut sums = vec![0i64; ow * oh * 4];
        if background != [0; 4] {
            for oy in 0..oh {
                let (y0, y1) = span(oy, oh, h);
                for ox in 0..ow {
                    let (x0, x1) = span(ox, ow, w);
                    let count = ((y1 - y0) * (x1 - x0)) as i64;
                    let o = (oy * ow + ox) * 4;
                    for c in 0..4 {
                        sums[o + c] = count * i64::from(background[c]);
                    }
                }
            }
        }
        Self { ow, oh, xmap, ymap, sums: Mutex::new(sums) }
    }

    /// Adds the difference `now - background` of the `tw x th` pixels `base` (the canvas pixels
    /// that differ from the background, premultiplied) whose top-left is canvas pixel `(x0, y0)`.
    fn add(&self, x0: usize, y0: usize, tw: usize, th: usize, base: &[u8], background: [u8; 4]) {
        let (ox_lo, ox_hi) = (self.xmap[x0] as usize, self.xmap[x0 + tw - 1] as usize);
        let (oy_lo, oy_hi) = (self.ymap[y0] as usize, self.ymap[y0 + th - 1] as usize);
        let win_w = ox_hi - ox_lo + 1;
        let mut local = vec![0i64; win_w * (oy_hi - oy_lo + 1) * 4];
        for y in 0..th {
            let oy = self.ymap[y0 + y] as usize - oy_lo;
            for x in 0..tw {
                let p = &base[(y * tw + x) * 4..(y * tw + x) * 4 + 4];
                let o = (oy * win_w + (self.xmap[x0 + x] as usize - ox_lo)) * 4;
                for c in 0..4 {
                    local[o + c] += i64::from(p[c]) - i64::from(background[c]);
                }
            }
        }
        let mut sums = self.sums.lock().unwrap_or_else(|e| e.into_inner());
        for (wy, chunk) in local.chunks_exact(win_w * 4).enumerate() {
            let o = ((oy_lo + wy) * self.ow + ox_lo) * 4;
            for (s, v) in sums[o..o + win_w * 4].iter_mut().zip(chunk) {
                *s += v;
            }
        }
    }

    /// The box-filtered pixels (straight RGBA8), as the dense thumbnail made them.
    fn finish(self, w: usize, h: usize) -> Vec<u8> {
        let sums = self.sums.into_inner().unwrap_or_else(|e| e.into_inner());
        let (ow, oh) = (self.ow, self.oh);
        let mut out = vec![0u8; ow * oh * 4];
        for oy in 0..oh {
            let (y0, y1) = span(oy, oh, h);
            for ox in 0..ow {
                let (x0, x1) = span(ox, ow, w);
                let count = ((y1 - y0) * (x1 - x0)) as u64;
                let o = (oy * ow + ox) * 4;
                let avg = [0, 1, 2, 3].map(|c| sums[o + c] as u64 as f64 / count as f64);
                let alpha = avg[3] / 255.0;
                for c in 0..3 {
                    out[o + c] = if alpha > 0.0 { (avg[c] / alpha).round().clamp(0.0, 255.0) as u8 } else { 0 };
                }
                out[o + 3] = avg[3].round() as u8;
            }
        }
        out
    }
}

/// PNG bytes of straight RGBA8 pixels.
pub(crate) fn encode_png(rgba: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, w as u32, h as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| Error::Encode(e.to_string()))?;
        writer.write_image_data(rgba).map_err(|e| Error::Encode(e.to_string()))?;
    }
    Ok(png_bytes)
}

// ------------------------------------------------------------------------------ the pass

/// The composite layer's tiles and the thumbnail PNG.
pub(crate) struct CompositeOut {
    /// Present when the archive carries the composite.
    pub layer: Option<LayerOut>,
    pub thumbnail: Vec<u8>,
}

/// Builds the flattened composite tile by tile: compresses its tiles (when `write` is set, at
/// `bits` per channel) under `uuid`, and accumulates the thumbnail over `background`
/// (straight RGBA, `None` when the background is hidden).
pub(crate) fn composite_and_thumbnail(
    tree: &[Node],
    g: &Grid,
    bits: u32,
    write: Option<&str>,
    background: Option<[f32; 4]>,
) -> Result<CompositeOut> {
    let nodes = composite_nodes(tree);
    let background = background.map(crate::document::premultiply);
    let bg = background.unwrap_or([0; 4]);
    let thumb = Thumb::new(g.w, g.h, bg);
    let per = g.cols * g.rows;
    struct State {
        acc: Vec<u8>,
        scratch: Vec<u8>,
        base: Vec<u8>,
        wide: Vec<u8>,
    }
    let results: Vec<Option<(Bounds, Vec<u8>)>> = par_map_with(
        per,
        || State { acc: Vec::new(), scratch: Vec::new(), base: Vec::new(), wide: Vec::new() },
        |st, j| {
            let (row, col) = (j / g.cols, j % g.cols);
            if !any_data(&nodes, col, row) {
                return None;
            }
            let (x0, y0, tw, th) = g.rect(col, row);
            st.acc.clear();
            st.acc.resize(tw * th * 4, 0);
            composite_tile(&nodes, g, col, row, &mut st.acc, &mut st.scratch);
            let (bx0, by0, bx1, by1) = nonzero_bounds(&st.acc, tw, th, 4)?;
            // the thumbnail sees the composite over the background
            st.base.clear();
            if background.is_some() {
                st.base.extend(std::iter::repeat_n(bg, tw * th).flatten());
                over_tile(&mut st.base, &st.acc, 1.0);
            } else {
                st.base.extend_from_slice(&st.acc);
            }
            thumb.add(x0, y0, tw, th, &st.base, bg);
            let data = match write {
                Some(_) => {
                    let bytes: &[u8] = if bits == 16 {
                        st.wide.clear();
                        st.wide.extend(st.acc.iter().flat_map(|&v| (u16::from(v) * 257).to_le_bytes()));
                        &st.wide
                    } else {
                        &st.acc
                    };
                    apple_lz4_compress(bytes)
                }
                None => Vec::new(),
            };
            Some(((x0 + bx0, y0 + by0, x0 + bx1, y0 + by1), data))
        },
    );
    let layer = write.map(|uuid| {
        let mut out = LayerOut::default();
        for (j, tile) in results.into_iter().enumerate() {
            let Some((bounds, data)) = tile else { continue };
            out.content = union(out.content, bounds);
            out.files.push((member(uuid, j % g.cols, j / g.cols), data));
        }
        out
    });
    let (ow, oh) = (thumb.ow, thumb.oh);
    let thumbnail = encode_png(&thumb.finish(g.w, g.h), ow, oh)?;
    Ok(CompositeOut { layer, thumbnail })
}
