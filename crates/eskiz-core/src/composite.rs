//! CPU compositor for the layer tree.
//!
//! The canvas is processed in small blocks. For each block the background is laid down and
//! the tree is walked bottom to top: hidden items are skipped, groups are composited into
//! their own transparent buffer and then onto the backdrop (2.5), a base layer and the
//! clipped layers above it form a clip group (2.4), and every other layer uses the
//! single-layer formula (2.3). Because the work is per block, a small dirty rectangle only
//! touches the pixels inside it.
//!
//! # How it is organised (performance)
//!
//! * The layer tree is resolved once per call into a flat [`Item`] plan: hidden items,
//!   layers without pixels and items that cannot change a pixel (opacity 0, empty groups) are
//!   dropped, opacities are clamped, and masks are checked for length, so none of that runs
//!   per block or per pixel.
//! * Blocks are independent, so they are composited in parallel with `std::thread::scope`.
//!   Every pixel is computed by exactly the same `f64` arithmetic whatever the thread count
//!   or the block grid, so the output is byte-identical to a single-threaded run. A call
//!   that turns out to be cheap (a few small blocks over empty tiles) never spawns a thread.
//! * Each thread owns one block buffer and a small pool of group buffers, reused for every
//!   block it handles.
//! * The inner span loops are specialised on "Normal blend" and "has a mask", and the two
//!   per-pixel `u8` divisions (`c / a` and `v / 255`) are table look-ups of the very same
//!   `f64` values, so the arithmetic is unchanged.

use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use crate::blend::{blend_colour, BlendMode};
use crate::canvas::Layer;
use crate::compose::round_to_u8;
use crate::document::{Document, Mask, Node, NodeKind};
use crate::pixel::{Chan, Depth};

/// Edge of one processing block in pixels. Blocks are aligned to this grid on the canvas, so
/// a block never straddles a tile edge.
const BLOCK: usize = 128;
const _: () = assert!(crate::canvas::TILE.is_multiple_of(BLOCK));

/// How much parallelism a call may use.
#[derive(Clone, Copy)]
pub(crate) struct Par {
    /// Most threads, the calling one included.
    pub threads: usize,
    /// Threads are only started once the remaining work is estimated to take at least this
    /// long (seconds); starting one costs tens of microseconds.
    pub min_secs: f64,
}

impl Par {
    /// The default: all cores, but only for calls that are worth it.
    fn auto() -> Self {
        Self {
            threads: thread_count(),
            min_secs: 1.5e-3,
        }
    }
}

/// Options for one composite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositeOpts {
    /// Leave private items out (flattened exports.
    pub skip_private: bool,
    /// Start from the background colour (when it is visible). Off gives a transparent start.
    pub background: bool,
}

impl Default for CompositeOpts {
    /// The canvas view: private items included, background included.
    fn default() -> Self {
        Self {
            skip_private: false,
            background: true,
        }
    }
}

/// Premultiplied RGBA.
type Px = [f32; 4];

#[derive(Clone, Copy)]
struct Block {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

fn fin01(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------------------

/// Composite a rectangle of the document. Returns straight RGBA8 for the rectangle clipped to
/// the canvas (`w * h` pixels, row-major). With a visible background the alpha is 255.
pub fn composite_rect(doc: &Document, x: usize, y: usize, w: usize, h: usize, opts: CompositeOpts) -> Vec<u8> {
    composite_rect_par(doc, x, y, w, h, opts, Par::auto())
}

/// [`composite_rect`] as straight RGBA16 (`w * h * 4` samples). The blending runs in `f32` and
/// is rounded once, so a 16-bit document keeps its precision; an 8-bit document is widened.
pub fn composite_rect16(doc: &Document, x: usize, y: usize, w: usize, h: usize, opts: CompositeOpts) -> Vec<u16> {
    composite_rect16_par(doc, x, y, w, h, opts, Par::auto())
}

/// Composite the whole canvas. See [`composite_rect`].
pub fn composite(doc: &Document, opts: CompositeOpts) -> Vec<u8> {
    composite_rect(doc, 0, 0, doc.width, doc.height, opts)
}

/// Composite the whole canvas as straight RGBA16. See [`composite_rect16`].
pub fn composite16(doc: &Document, opts: CompositeOpts) -> Vec<u16> {
    composite_rect16(doc, 0, 0, doc.width, doc.height, opts)
}

/// Composite the whole canvas without the background, as premultiplied RGBA8 (the pixel
/// format of an 8-bit [`Layer`]; a 16-bit document rounds). Used by merge down and flatten.
/// See [`composite_to_tiled`] for the tiled form, which keeps the document's depth and does
/// not build a full-size buffer.
pub fn composite_to_layer(doc: &Document, skip_private: bool) -> Vec<u8> {
    composite_to_tiled(doc, skip_private).to_rgba()
}

/// [`composite_to_layer`] as a tiled [`Layer`]: tiles where nothing is drawn are not allocated.
pub fn composite_to_tiled(doc: &Document, skip_private: bool) -> Layer {
    composite_to_tiled_par(doc, skip_private, Par::auto())
}

/// The composite of a rectangle converted for egui: premultiplied RGBA8 pixels, each a `u32`
/// whose four bytes in memory are `[r, g, b, a]` (so [`cast_pixels`] can hand the `Vec` to
/// `egui::Color32`, which is four bytes aligned to four, without copying it), exactly what `egui::ColorImage::from_rgba_unmultiplied` followed by
/// reading back the `Color32` bytes gives for the output of [`composite_rect`] (`w * h`
/// pixels, row-major, clipped to the canvas like [`composite_rect`]).
///
/// `map` runs on every block's straight RGBA8 pixels before the premultiplication (the app
/// uses it for the colour-space-to-display conversion; pass `|_| {}` for none). It is called
/// from several threads, once per block, with a whole number of pixels.
pub fn composite_rect_premul(
    doc: &Document,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    opts: CompositeOpts,
    map: &(dyn Fn(&mut [u8]) + Sync),
) -> Vec<u32> {
    let reg = Region::new(doc, x, y, w, h);
    let mut out = vec![0u32; (reg.x1 - reg.x0) * (reg.y1 - reg.y0)];
    let items = plan(doc, &doc.root, opts);
    let bands = Bands::new(&mut out, &reg, 1);
    for_each_block(doc, &items, &reg, bg_px(doc, opts), Par::auto(), &|blk, buf, bufs| {
        bufs.words32.resize(blk.w * blk.h, 0);
        let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut bufs.words32);
        let opaque = convert_rows_u8(buf, blk.w, bytes);
        map(bytes);
        if !opaque {
            for p in bytes.as_chunks_mut::<4>().0 {
                *p = premultiply_u8(*p);
            }
        }
        bands.write(&reg, blk, &bufs.words32);
    });
    drop(bands);
    out
}

/// Reinterpret the pixels of [`composite_rect_premul`] as another four-byte pixel type
/// (`egui::Color32` in the app) without copying them. Panics if `T` is not four bytes aligned
/// to four.
pub fn cast_pixels<T: bytemuck::Pod>(px: Vec<u32>) -> Vec<T> {
    bytemuck::allocation::cast_vec(px)
}

/// `egui::Color32::from_rgba_unmultiplied`, as bytes: straight RGBA8 to premultiplied RGBA8.
/// Kept in this crate (which does not depend on egui); the app's tests compare it with egui's
/// own function for every colour and alpha value, so an egui upgrade that changes the rounding
/// is caught.
#[inline(always)]
pub fn premultiply_u8(p: [u8; 4]) -> [u8; 4] {
    match p[3] {
        255 => p,
        0 => [0; 4],
        a => {
            // egui's `mul_frac_round`: `val * a / 255` rounded, in fixed point.
            let f = |v: u8| {
                let t = u16::from(v) * u16::from(a) + 128;
                ((t + (t >> 8)) >> 8) as u8
            };
            [f(p[0]), f(p[1]), f(p[2]), a]
        }
    }
}

// ---------------------------------------------------------------------------------------
// Thread-count aware variants (the tests drive these)
// ---------------------------------------------------------------------------------------

fn thread_count() -> usize {
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| {
        if let Some(n) = std::env::var("ESKIZ_COMPOSITE_THREADS").or_else(|_| std::env::var("EASEL_COMPOSITE_THREADS" /* legacy-easel */)).ok().and_then(|s| s.parse::<usize>().ok()) {
            return n.clamp(1, 64);
        }
        crate::parallelism::cpus().min(16)
    })
}

pub(crate) fn composite_rect_par(
    doc: &Document,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    opts: CompositeOpts,
    par: Par,
) -> Vec<u8> {
    let reg = Region::new(doc, x, y, w, h);
    let mut out = vec![0u8; (reg.x1 - reg.x0) * (reg.y1 - reg.y0) * 4];
    let items = plan(doc, &doc.root, opts);
    let bands = Bands::new(&mut out, &reg, 4);
    for_each_block(doc, &items, &reg, bg_px(doc, opts), par, &|blk, buf, bufs| {
        bufs.bytes.resize(blk.w * blk.h * 4, 0);
        convert_rows_u8(buf, blk.w, &mut bufs.bytes);
        bands.write(&reg, blk, &bufs.bytes);
    });
    drop(bands);
    out
}

pub(crate) fn composite_rect16_par(
    doc: &Document,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    opts: CompositeOpts,
    par: Par,
) -> Vec<u16> {
    let reg = Region::new(doc, x, y, w, h);
    let mut out = vec![0u16; (reg.x1 - reg.x0) * (reg.y1 - reg.y0) * 4];
    let items = plan(doc, &doc.root, opts);
    let bands = Bands::new(&mut out, &reg, 4);
    for_each_block(doc, &items, &reg, bg_px(doc, opts), par, &|blk, buf, bufs| {
        bufs.words.resize(blk.w * blk.h * 4, 0);
        for (p, o) in buf.iter().zip(bufs.words.as_chunks_mut::<4>().0) {
            *o = to_straight_u16(*p);
        }
        bands.write(&reg, blk, &bufs.words);
    });
    drop(bands);
    out
}

pub(crate) fn composite_to_tiled_par(doc: &Document, skip_private: bool, par: Par) -> Layer {
    let opts = CompositeOpts {
        skip_private,
        background: false,
    };
    let reg = Region::new(doc, 0, 0, doc.width, doc.height);
    let out = Mutex::new(Layer::new_depth(doc.width, doc.height, doc.depth));
    let items = plan(doc, &doc.root, opts);
    for_each_block(doc, &items, &reg, [0.0; 4], par, &|blk, buf, bufs| match doc.depth {
        Depth::U8 => {
            bufs.bytes.resize(blk.w * blk.h * 4, 0);
            for (v, o) in buf.as_flattened().iter().zip(bufs.bytes.iter_mut()) {
                *o = unit_to_u8(*v);
            }
            lock(&out).write_rect(blk.x, blk.y, blk.w, blk.h, &bufs.bytes);
        }
        Depth::U16 => {
            bufs.words.clear();
            for p in buf {
                bufs.words.extend(p.iter().map(|&v| <u16 as Chan>::from_unit(f64::from(v))));
            }
            lock(&out).write_rect16(blk.x, blk.y, blk.w, blk.h, &bufs.words);
        }
    });
    out.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn bg_px(doc: &Document, opts: CompositeOpts) -> Px {
    if opts.background && doc.background_visible {
        let c = doc.background.map(|v| fin01(v) as f32);
        [c[0], c[1], c[2], 1.0]
    } else {
        [0.0; 4]
    }
}

/// A compositor value in `0..=1` as a byte: `round_to_u8(fin01(v) * 255.0)` without the
/// clamp and the branches. `v * 255` is exact in `f64` (24 + 8 bits) and so is `+ 0.5`, so
/// truncating it is exactly round-half-up; the stored values are always finite and inside
/// `0..=1` because every store goes through `fin01(..) as f32`.
#[inline(always)]
fn unit_to_u8(v: f32) -> u8 {
    (f64::from(v) * 255.0 + 0.5) as u8
}

/// Straight RGBA8 of `buf` (rows of `w` pixels) into `out`. Rows whose alpha is all 1.0, which
/// is every row over a visible background, take a branch-free path that the compiler
/// vectorises. Returns true when every row was like that (so the output alpha is all 255).
fn convert_rows_u8(buf: &[Px], w: usize, out: &mut [u8]) -> bool {
    let mut all = true;
    for (row, orow) in buf.chunks_exact(w).zip(out.chunks_exact_mut(w * 4)) {
        if row.iter().all(|p| p[3] == 1.0) {
            for (v, o) in row.as_flattened().iter().zip(orow.iter_mut()) {
                *o = unit_to_u8(*v);
            }
        } else {
            all = false;
            for (p, o) in row.iter().zip(orow.as_chunks_mut::<4>().0) {
                *o = to_straight_u8(*p);
            }
        }
    }
    all
}

#[inline]
fn to_straight_u8(p: Px) -> [u8; 4] {
    let a = fin01(f64::from(p[3]));
    if a <= 0.0 {
        return [0; 4];
    }
    if a == 1.0 {
        // `v / 1.0 == v` exactly, so the division is skipped.
        let f = |v: f32| round_to_u8(fin01(f64::from(v)) * 255.0);
        return [f(p[0]), f(p[1]), f(p[2]), 255];
    }
    let f = |v: f32| round_to_u8(fin01(f64::from(v) / a) * 255.0);
    [f(p[0]), f(p[1]), f(p[2]), round_to_u8(a * 255.0)]
}

#[inline]
fn to_straight_u16(p: Px) -> [u16; 4] {
    let a = fin01(f64::from(p[3]));
    if a <= 0.0 {
        return [0; 4];
    }
    let f = |v: f32| <u16 as Chan>::from_unit(fin01(f64::from(v) / a));
    [f(p[0]), f(p[1]), f(p[2]), <u16 as Chan>::from_unit(a)]
}

// ---------------------------------------------------------------------------------------
// Region, block grid, output bands
// ---------------------------------------------------------------------------------------

/// The pixel rectangle `x0..x1, y0..y1` of a call, already clipped to the canvas.
struct Region {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Region {
    fn new(doc: &Document, x: usize, y: usize, w: usize, h: usize) -> Self {
        let x1 = x.saturating_add(w).min(doc.width);
        let y1 = y.saturating_add(h).min(doc.height);
        Self {
            x0: x.min(x1),
            y0: y.min(y1),
            x1,
            y1,
        }
    }

    fn is_empty(&self) -> bool {
        self.x0 >= self.x1 || self.y0 >= self.y1
    }

    /// First block column and row on the absolute grid, and how many there are.
    fn grid(&self) -> (usize, usize, usize, usize) {
        let (c0, r0) = (self.x0 / BLOCK, self.y0 / BLOCK);
        let cols = (self.x1 - 1) / BLOCK - c0 + 1;
        let rows = (self.y1 - 1) / BLOCK - r0 + 1;
        (c0, r0, cols, rows)
    }
}

/// An output buffer for a [`Region`] cut into one lock-protected slice per block row, so
/// threads that finish blocks of different block rows never wait for each other and no
/// `unsafe` is needed.
struct Bands<'a, T> {
    bands: Vec<Mutex<&'a mut [T]>>,
    row0: usize,
    /// Elements of `T` per pixel (4 for flat samples, 1 for `[u8; 4]` pixels).
    per_px: usize,
}

impl<'a, T: Copy> Bands<'a, T> {
    /// `out` is `rw * rh * per_px` elements.
    fn new(out: &'a mut [T], reg: &Region, per_px: usize) -> Self {
        let mut bands = Vec::new();
        let row0 = reg.y0 / BLOCK;
        if !reg.is_empty() {
            let rw4 = (reg.x1 - reg.x0) * per_px;
            let mut rest = out;
            let mut y = reg.y0;
            while y < reg.y1 {
                let end = ((y / BLOCK + 1) * BLOCK).min(reg.y1);
                let (band, tail) = rest.split_at_mut((end - y) * rw4);
                bands.push(Mutex::new(band));
                rest = tail;
                y = end;
            }
        }
        Self { bands, row0, per_px }
    }

    /// Copy one block's elements (`blk.w * blk.h * per_px`, row-major) to their place.
    fn write(&self, reg: &Region, blk: Block, data: &[T]) {
        let k = blk.y / BLOCK - self.row0;
        let band_y0 = reg.y0.max((self.row0 + k) * BLOCK);
        let n = self.per_px;
        let row_len = (reg.x1 - reg.x0) * n;
        let mut band = lock(&self.bands[k]);
        for r in 0..blk.h {
            let dst = (blk.y - band_y0 + r) * row_len + (blk.x - reg.x0) * n;
            band[dst..dst + blk.w * n].copy_from_slice(&data[r * blk.w * n..(r + 1) * blk.w * n]);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Block driver: parallel loop with per-thread scratch
// ---------------------------------------------------------------------------------------

/// Per-thread reusable buffers.
#[derive(Default)]
struct Scratch {
    /// The block's own pixels.
    main: Vec<Px>,
    /// Group buffers not in use.
    pool: Vec<Vec<Px>>,
    /// Conversion buffers handed to the output closure.
    bufs: Bufs,
}

#[derive(Default)]
struct Bufs {
    bytes: Vec<u8>,
    words: Vec<u16>,
    /// Whole pixels, for the output that is handed to egui.
    words32: Vec<u32>,
}

/// Shared read-only data of one call.
struct Cx {
    t: &'static Tables,
    /// Document width, for mask indexing.
    width: usize,
}

type Emit<'e> = &'e (dyn Fn(Block, &[Px], &mut Bufs) + Sync);

/// Composite every block of `reg` and hand each finished block (premultiplied pixels, the
/// block's rectangle) to `emit`, from any thread.
///
/// The calling thread works through the blocks and times them; worker threads are only
/// started once the measured block time says the remaining blocks are worth it.
fn for_each_block(doc: &Document, items: &[Item], reg: &Region, bg: Px, par: Par, emit: Emit) {
    if reg.is_empty() {
        return;
    }
    let cx = Cx {
        t: tables(),
        width: doc.width,
    };
    let (c0, r0, cols, rows) = reg.grid();
    let total = cols * rows;
    let next = AtomicUsize::new(0);
    // Composite and emit the next block; false when none is left.
    let process = |sc: &mut Scratch| -> bool {
        let i = next.fetch_add(1, Ordering::Relaxed);
        if i >= total {
            return false;
        }
        let (col, row) = (c0 + i % cols, r0 + i / cols);
        let x = reg.x0.max(col * BLOCK);
        let y = reg.y0.max(row * BLOCK);
        let blk = Block {
            x,
            y,
            w: reg.x1.min((col + 1) * BLOCK) - x,
            h: reg.y1.min((row + 1) * BLOCK) - y,
        };
        let mut buf = std::mem::take(&mut sc.main);
        buf.clear();
        buf.resize(blk.w * blk.h, bg);
        render_items(&cx, items, blk, &mut buf, sc);
        emit(blk, &buf, &mut sc.bufs);
        sc.main = buf;
        true
    };
    let mut sc = Scratch::default();
    if par.threads <= 1 || total <= 1 {
        while process(&mut sc) {}
        return;
    }
    let t0 = Instant::now();
    std::thread::scope(|s| {
        let mut done = 0usize;
        let mut spawned = false;
        while process(&mut sc) {
            done += 1;
            if !spawned {
                let remaining = total.saturating_sub(next.load(Ordering::Relaxed));
                let per_block = t0.elapsed().as_secs_f64() / done as f64;
                if remaining > 0 && per_block * remaining as f64 >= par.min_secs {
                    spawned = true;
                    for _ in 1..par.threads.min(remaining + 1) {
                        s.spawn(|| {
                            let mut sc = Scratch::default();
                            while process(&mut sc) {}
                        });
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------------------
// Lookup tables (the same f64 values the division would give)
// ---------------------------------------------------------------------------------------

struct Tables {
    /// `v / 255.0` for every byte: a byte alpha or mask value as a unit value.
    unit: [f64; 256],
    /// `(v / 255.0) as f32` for every byte: an opaque 8-bit sample as the compositor stores it.
    unit32: [f32; 256],
    /// `fin01(c / a)` for `a` in `1..=255` and `c` in `0..=255`, at `a << 8 | c`: the straight
    /// colour of an 8-bit premultiplied sample.
    straight: Box<[f64]>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut unit = [0.0; 256];
        let mut unit32 = [0.0; 256];
        for v in 0..256 {
            unit[v] = v as f64 / 255.0;
            unit32[v] = unit[v] as f32;
        }
        let mut straight = vec![0.0; 256 * 256];
        for a in 1..256usize {
            for c in 0..256usize {
                straight[a << 8 | c] = fin01(c as f64 / a as f64);
            }
        }
        Tables {
            unit,
            unit32,
            straight: straight.into_boxed_slice(),
        }
    })
}

/// A channel type that can be turned into a straight colour.
trait Src: Chan {
    /// Is [`Src::opaque`] available for this depth?
    const FAST_OPAQUE: bool;
    /// A non-transparent premultiplied pixel (`px[3]` is not zero) to (straight colour, alpha).
    fn straight(t: &Tables, px: &[Self]) -> ([f64; 3], f64);
    /// Is the pixel fully opaque (alpha at its maximum, so the unit alpha is exactly 1.0)?
    fn is_opaque(px: &[Self]) -> bool;
    /// The straight colour of an opaque pixel rounded to `f32`, which is what a Normal,
    /// full-opacity layer leaves in a backdrop whose alpha is 0 or 1 (see `draw_span`).
    fn opaque(t: &Tables, px: &[Self]) -> [f32; 3];
}

impl Src for u8 {
    const FAST_OPAQUE: bool = true;
    #[inline(always)]
    fn is_opaque(px: &[u8]) -> bool {
        px[3] == 255
    }
    #[inline(always)]
    fn opaque(t: &Tables, px: &[u8]) -> [f32; 3] {
        [t.unit32[usize::from(px[0])], t.unit32[usize::from(px[1])], t.unit32[usize::from(px[2])]]
    }
    #[inline(always)]
    fn straight(t: &Tables, px: &[u8]) -> ([f64; 3], f64) {
        let a = usize::from(px[3]) << 8;
        (
            [
                t.straight[a | usize::from(px[0])],
                t.straight[a | usize::from(px[1])],
                t.straight[a | usize::from(px[2])],
            ],
            t.unit[usize::from(px[3])],
        )
    }
}

impl Src for u16 {
    // The rounding argument behind the shortcut needs the alpha denominator to be at most 255.
    const FAST_OPAQUE: bool = false;
    #[inline(always)]
    fn is_opaque(px: &[u16]) -> bool {
        px[3] == u16::MAX
    }
    #[inline(always)]
    fn opaque(_: &Tables, px: &[u16]) -> [f32; 3] {
        let f = |v: u16| fin01(f64::from(v) / 65535.0) as f32;
        [f(px[0]), f(px[1]), f(px[2])]
    }
    #[inline(always)]
    fn straight(_: &Tables, px: &[u16]) -> ([f64; 3], f64) {
        let a = px[3].to_unit();
        let f = |v: u16| fin01(v.raw() / px[3].raw());
        ([f(px[0]), f(px[1]), f(px[2])], a)
    }
}

// ---------------------------------------------------------------------------------------
// The plan: the layer tree resolved once per call
// ---------------------------------------------------------------------------------------

/// A raster layer with everything a block needs to draw it.
struct LayerPlan<'a> {
    pixels: &'a Layer,
    /// `fin01` of the layer opacity.
    opacity: f64,
    mode: BlendMode,
    /// The mask, at least `width * height` bytes (a short mask is padded with 255, which is
    /// what a missing byte means).
    mask: Option<Cow<'a, [u8]>>,
}

enum Item<'a> {
    /// A layer on its own.
    Layer(LayerPlan<'a>),
    /// A base layer and the clipped layers above it.
    Clip(LayerPlan<'a>, Vec<LayerPlan<'a>>),
    /// A group, composited into its own buffer first.
    Group {
        opacity: f64,
        mode: BlendMode,
        mask: Option<Cow<'a, [u8]>>,
        children: Vec<Item<'a>>,
    },
}

/// Is this item left out of the composite?
fn skipped(n: &Node, opts: CompositeOpts) -> bool {
    !n.props.visible || (opts.skip_private && n.props.private)
}

fn mask_of<'a>(doc: &Document, m: Option<&'a Mask>) -> Option<Cow<'a, [u8]>> {
    m.map(|m| {
        let need = doc.width * doc.height;
        if m.data.len() >= need {
            Cow::Borrowed(&m.data[..])
        } else {
            let mut v = m.data.clone();
            v.resize(need, 255);
            Cow::Owned(v)
        }
    })
}

fn layer_plan<'a>(doc: &'a Document, n: &'a Node) -> Option<LayerPlan<'a>> {
    let pixels = doc.layer(n.id)?;
    Some(LayerPlan {
        pixels,
        opacity: fin01(n.props.opacity),
        mode: n.props.blend,
        mask: mask_of(doc, n.mask.as_ref()),
    })
}

/// Resolve `items` (bottom to top) into a plan. The structure mirrors the tree walk of the
/// original block compositor exactly; only items that cannot change a pixel are dropped.
fn plan<'a>(doc: &'a Document, items: &'a [Node], opts: CompositeOpts) -> Vec<Item<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < items.len() {
        let item = &items[i];
        i += 1;
        // The clipped layers directly above a base layer. A clipped layer that
        // is itself reached here has no valid base and draws unclipped.
        let mut run_end = i;
        if !item.is_group() && !item.props.clipped {
            while run_end < items.len() && !items[run_end].is_group() && items[run_end].props.clipped {
                run_end += 1;
            }
        }
        let run = &items[i..run_end];
        i = run_end;
        if skipped(item, opts) {
            continue; // a hidden base takes its clipped layers with it
        }
        match &item.kind {
            NodeKind::Group { children, .. } => {
                let opacity = fin01(item.props.opacity);
                if children.is_empty() || opacity <= 0.0 {
                    continue;
                }
                let children = plan(doc, children, opts);
                if children.is_empty() {
                    continue; // nothing drawn into the group buffer: nothing to composite
                }
                out.push(Item::Group {
                    opacity,
                    mode: item.props.blend,
                    mask: mask_of(doc, item.mask.as_ref()),
                    children,
                });
            }
            NodeKind::Layer => {
                let Some(base) = layer_plan(doc, item) else {
                    continue;
                };
                let clipped: Vec<LayerPlan> = run
                    .iter()
                    .filter(|n| !skipped(n, opts))
                    .filter_map(|n| layer_plan(doc, n))
                    .collect();
                if base.opacity <= 0.0 {
                    continue;
                }
                if clipped.is_empty() {
                    out.push(Item::Layer(base));
                } else {
                    out.push(Item::Clip(base, clipped));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Drawing one block
// ---------------------------------------------------------------------------------------

/// Composite `items` (bottom to top) onto `dst`, which covers `blk`.
fn render_items(cx: &Cx, items: &[Item], blk: Block, dst: &mut [Px], sc: &mut Scratch) {
    for item in items {
        match item {
            Item::Layer(l) => draw_layer(cx, l, blk, dst),
            Item::Clip(base, clipped) => match base.pixels.depth() {
                Depth::U8 => draw_clip_group::<u8>(cx, base, clipped, blk, dst),
                Depth::U16 => draw_clip_group::<u16>(cx, base, clipped, blk, dst),
            },
            Item::Group {
                opacity,
                mode,
                mask,
                children,
            } => {
                let mut buf = sc.pool.pop().unwrap_or_default();
                buf.clear();
                buf.resize(blk.w * blk.h, [0.0; 4]);
                render_items(cx, children, blk, &mut buf, sc);
                if *mode == BlendMode::Normal {
                    draw_group::<true>(cx, &buf, *opacity, *mode, mask.as_deref(), blk, dst);
                } else {
                    draw_group::<false>(cx, &buf, *opacity, *mode, mask.as_deref(), blk, dst);
                }
                sc.pool.push(buf);
            }
        }
    }
}

#[inline(always)]
fn mask_at(t: &Tables, m: Option<&[u8]>, i: usize) -> f64 {
    m.map_or(1.0, |m| t.unit[usize::from(m[i])])
}

/// A finished group buffer onto the backdrop.
fn draw_group<const NORMAL: bool>(
    cx: &Cx,
    buf: &[Px],
    opacity: f64,
    mode: BlendMode,
    mask: Option<&[u8]>,
    blk: Block,
    dst: &mut [Px],
) {
    for r in 0..blk.h {
        let row = &buf[r * blk.w..(r + 1) * blk.w];
        let out = &mut dst[r * blk.w..(r + 1) * blk.w];
        let m0 = (blk.y + r) * cx.width + blk.x;
        for (c, (p, d)) in row.iter().zip(out).enumerate() {
            let a = f64::from(p[3]);
            if a <= 0.0 {
                continue;
            }
            let o = opacity * mask_at(cx.t, mask, m0 + c);
            if o <= 0.0 {
                continue;
            }
            let cs = [f64::from(p[0]) / a, f64::from(p[1]) / a, f64::from(p[2]) / a];
            over::<NORMAL>(d, cs, a, o, mode);
        }
    }
}

/// Single-layer composite of one block. Each row is walked one tile span at a
/// time, so an empty tile is skipped without looking at its pixels.
fn draw_layer(cx: &Cx, l: &LayerPlan, blk: Block, dst: &mut [Px]) {
    let masked = l.mask.is_some();
    match (l.mode == BlendMode::Normal, masked) {
        (true, false) => draw_layer_t::<true, false>(cx, l, blk, dst),
        (true, true) => draw_layer_t::<true, true>(cx, l, blk, dst),
        (false, false) => draw_layer_t::<false, false>(cx, l, blk, dst),
        (false, true) => draw_layer_t::<false, true>(cx, l, blk, dst),
    }
}

fn draw_layer_t<const NORMAL: bool, const MASKED: bool>(cx: &Cx, l: &LayerPlan, blk: Block, dst: &mut [Px]) {
    let mask = l.mask.as_deref();
    for r in 0..blk.h {
        let y = blk.y + r;
        let row = &mut dst[r * blk.w..(r + 1) * blk.w];
        for (x, span) in l.pixels.row_spans(y, blk.x, blk.x + blk.w) {
            let out = &mut row[x - blk.x..x - blk.x + span.len];
            let m: &[u8] = match mask {
                Some(m) if MASKED => &m[y * cx.width + x..y * cx.width + x + span.len],
                _ => &[],
            };
            if let Some(data) = span.data {
                draw_span::<u8, NORMAL, MASKED>(cx.t, data, m, l.opacity, l.mode, out);
            } else if let Some(data) = span.data16 {
                draw_span::<u16, NORMAL, MASKED>(cx.t, data, m, l.opacity, l.mode, out);
            }
        }
    }
}

/// One tile span of a layer over the backdrop pixels `out`. `mask` holds the span's mask
/// bytes when `MASKED`.
#[inline(always)]
fn draw_span<T: Src, const NORMAL: bool, const MASKED: bool>(
    t: &Tables,
    data: &[T],
    mask: &[u8],
    opacity: f64,
    mode: BlendMode,
    out: &mut [Px],
) {
    for (c, (px, d)) in data.as_chunks::<4>().0.iter().zip(out).enumerate() {
        if px[3].is_zero() {
            continue;
        }
        let o = if MASKED { opacity * t.unit[usize::from(mask[c])] } else { opacity };
        if o <= 0.0 {
            continue;
        }
        // A Normal, fully opaque, full-opacity source over a backdrop of alpha 0 or 1 just
        // replaces the backdrop. The general formula gives `d + 1.0 * (cs - d)` per channel
        // there, which is `cs` up to two roundings of at most 2^-52; the nearest `f32`
        // rounding boundary is at least ~9e-13 away from any `c / 255`, so the stored `f32`
        // is `cs as f32` exactly (an exhaustive test checks this for all 256 colours). The
        // alpha is `1.0` in both cases.
        if NORMAL && T::FAST_OPAQUE && o == 1.0 && T::is_opaque(px) && (d[3] == 1.0 || d[3] == 0.0) {
            let c = T::opaque(t, px);
            *d = [c[0], c[1], c[2], 1.0];
            continue;
        }
        let (cs, a) = T::straight(t, px);
        over::<NORMAL>(d, cs, a, o, mode);
    }
}

fn draw_clip_group<T: Src>(cx: &Cx, base: &LayerPlan, clipped: &[LayerPlan], blk: Block, dst: &mut [Px]) {
    if base.mode == BlendMode::Normal {
        draw_clip_group_t::<T, true>(cx, base, clipped, blk, dst);
    } else {
        draw_clip_group_t::<T, false>(cx, base, clipped, blk, dst);
    }
}

/// A base layer and the clipped layers above it.
fn draw_clip_group_t<T: Src, const NORMAL: bool>(
    cx: &Cx,
    base: &LayerPlan,
    clipped: &[LayerPlan],
    blk: Block,
    dst: &mut [Px],
) {
    let t = cx.t;
    let base_mask = base.mask.as_deref();
    let mut spans: Vec<Option<&[T]>> = Vec::with_capacity(clipped.len());
    for r in 0..blk.h {
        let y = blk.y + r;
        let x_end = blk.x + blk.w;
        let mut x = blk.x;
        while x < x_end {
            // Every layer has the same tile grid, so all spans at (x, y) have the same length.
            let (len, base_span) = base.pixels.row_span_t::<T>(x, y, x_end);
            let Some(base_data) = base_span else {
                x += len; // A = 0 for the whole span: nothing from the clip group shows here
                continue;
            };
            spans.clear();
            spans.extend(clipped.iter().map(|cl| cl.pixels.row_span_t::<T>(x, y, x_end).1));
            for c in 0..len {
                let bp = &base_data[c * 4..c * 4 + 4];
                if bp[3].is_zero() {
                    continue;
                }
                let m = y * cx.width + x + c;
                let (mut cg, a_base) = T::straight(t, bp);
                let group_alpha = a_base * mask_at(t, base_mask, m);
                if group_alpha <= 0.0 {
                    continue;
                }
                for (cl, span) in clipped.iter().zip(&spans) {
                    let Some(data) = span else {
                        continue;
                    };
                    let cp = &data[c * 4..c * 4 + 4];
                    if cp[3].is_zero() {
                        continue;
                    }
                    let (cs, a) = T::straight(t, cp);
                    let w = cl.opacity * mask_at(t, cl.mask.as_deref(), m) * a;
                    if w <= 0.0 {
                        continue;
                    }
                    // `blend_colour(Normal, ..)` only clamps `cs`, which is already in 0..1.
                    let b = if cl.mode == BlendMode::Normal { cs } else { blend_colour(cl.mode, cg, cs) };
                    for k in 0..3 {
                        cg[k] = (1.0 - w) * cg[k] + w * b[k];
                    }
                }
                over::<NORMAL>(&mut dst[r * blk.w + (x - blk.x) + c], cg, group_alpha, base.opacity, base.mode);
            }
            x += len;
        }
    }
}

/// The single-layer formula for one pixel: source straight colour `cs`, alpha
/// `a_s`, effective opacity `o`, blend `mode`, over the premultiplied backdrop `dst`.
/// `NORMAL` is `mode == Normal`, known to the caller.
#[inline(always)]
fn over<const NORMAL: bool>(dst: &mut Px, cs: [f64; 3], a_s: f64, o: f64, mode: BlendMode) {
    let ab = f64::from(dst[3]);
    let d = [f64::from(dst[0]), f64::from(dst[1]), f64::from(dst[2])];
    if ab == 0.0 || ab == 1.0 {
        // Transparent or opaque backdrop. `(1 - ab) * cs + ab * blended` is exactly `blended`
        // when `ab` is 1 (`0 * cs + 1 * blended`) and exactly `cs` when it is 0 (`1 * cs + 0 * x`),
        // and `blended` is `cs` for Normal and over a transparent backdrop. Over an opaque
        // backdrop `cb = d / 1.0` is `d`.
        let src = if NORMAL || ab == 0.0 { cs } else { blend_colour(mode, d, cs) };
        for k in 0..3 {
            let co = a_s * src[k] + (1.0 - a_s) * d[k];
            let v = d[k] + o * (co - d[k]);
            // For Normal every operand is finite and in range, so `fin01` is the clamp.
            dst[k] = if NORMAL { v.clamp(0.0, 1.0) } else { fin01(v) } as f32;
        }
        if ab == 0.0 {
            // `alpha_o` is `a_s + (1 - a_s) * 0 = a_s`, and `0 + o * (a_s - 0)` is `o * a_s`.
            dst[3] = fin01(o * a_s) as f32;
        }
        // Over an opaque backdrop the alpha stays 1.0: `alpha_o` is 1 up to 2^-52, which
        // rounds to the same `f32`.
        return;
    }
    let blended = if !NORMAL {
        let cb = [d[0] / ab, d[1] / ab, d[2] / ab];
        blend_colour(mode, cb, cs)
    } else {
        cs
    };
    let alpha_o = a_s + (1.0 - a_s) * ab;
    for k in 0..3 {
        let co = a_s * ((1.0 - ab) * cs[k] + ab * blended[k]) + (1.0 - a_s) * d[k];
        dst[k] = fin01(d[k] + o * (co - d[k])) as f32;
    }
    dst[3] = fin01(ab + o * (alpha_o - ab)) as f32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Props;

    const W: usize = 4;
    const H: usize = 4;

    fn solid(rgb: [u8; 3], a: u8) -> Vec<u8> {
        // Input is straight; the layer store is premultiplied.
        let p = |v: u8| ((u32::from(v) * u32::from(a) + 127) / 255) as u8;
        let px = [p(rgb[0]), p(rgb[1]), p(rgb[2]), a];
        px.iter().copied().cycle().take(W * H * 4).collect()
    }

    fn doc() -> Document {
        Document::empty(W, H)
    }

    fn add(d: &mut Document, parent: Option<u64>, name: &str, px: Vec<u8>, f: impl FnOnce(&mut Props)) -> u64 {
        let mut p = Props::new(name);
        f(&mut p);
        d.push_layer(parent, p, None, px)
    }

    fn at(d: &Document, x: usize, y: usize) -> [u8; 4] {
        let buf = composite(d, CompositeOpts::default());
        let i = (y * W + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn near(a: [u8; 4], b: [u8; 4]) {
        for k in 0..4 {
            assert!((i32::from(a[k]) - i32::from(b[k])).abs() <= 1, "{a:?} vs {b:?}");
        }
    }

    const RED: [u8; 3] = [255, 0, 0];
    const BLUE: [u8; 3] = [0, 0, 255];

    #[test]
    fn empty_document_shows_background() {
        let mut d = doc();
        assert_eq!(at(&d, 1, 1), [255, 255, 255, 255]);
        d.background = [1.0, 0.0, 0.0];
        assert_eq!(at(&d, 1, 1), [255, 0, 0, 255]);
        d.background_visible = false;
        assert_eq!(at(&d, 1, 1), [0, 0, 0, 0]);
    }

    #[test]
    fn multiply_two_opaque_layers() {
        // Acceptance 1.
        let mut d = doc();
        add(&mut d, None, "b", solid([255, 128, 0], 255), |_| {});
        add(&mut d, None, "t", solid([128, 128, 128], 255), |p| p.blend = BlendMode::Multiply);
        near(at(&d, 0, 0), [128, 64, 0, 255]);
    }

    #[test]
    fn opacity_half_blue_over_red() {
        // Acceptance 2 and 19.
        let mut d = doc();
        add(&mut d, None, "b", solid(RED, 255), |_| {});
        let t = add(&mut d, None, "t", solid(BLUE, 255), |p| p.opacity = 0.5);
        near(at(&d, 0, 0), [128, 0, 128, 255]);
        d.node_mut(t).unwrap().props.opacity = 0.0;
        near(at(&d, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn half_alpha_white_over_black() {
        // Acceptance 3.
        let mut d = doc();
        add(&mut d, None, "b", solid([0, 0, 0], 255), |_| {});
        add(&mut d, None, "t", solid([255, 255, 255], 128), |_| {});
        near(at(&d, 0, 0), [128, 128, 128, 255]);
    }

    #[test]
    fn hidden_layer_contributes_nothing() {
        // Acceptance 20.
        let mut d = doc();
        add(&mut d, None, "b", solid(RED, 255), |_| {});
        add(&mut d, None, "t", solid(BLUE, 255), |p| p.visible = false);
        near(at(&d, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn mask_half_and_zero() {
        // Acceptance 21.
        let mut d = doc();
        add(&mut d, None, "b", solid(RED, 255), |_| {});
        let t = add(&mut d, None, "t", solid(BLUE, 255), |_| {});
        d.node_mut(t).unwrap().mask = Some(Mask::filled(W, H, 128));
        near(at(&d, 0, 0), [127, 0, 128, 255]);
        d.node_mut(t).unwrap().mask = Some(Mask::filled(W, H, 0));
        near(at(&d, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn outputs_are_finite_for_all_modes() {
        // Acceptance 22: every mode over a grid of inputs stays in range (u8 output cannot be
        // NaN, so check that no panic occurs and alpha stays 255 over an opaque backdrop).
        let vals = [0u8, 128, 255];
        for mode in BlendMode::ALL {
            for &b in &vals {
                for &s in &vals {
                    for &a in &[0u8, 128, 255] {
                        let mut d = doc();
                        add(&mut d, None, "b", solid([b, s, b], 255), |_| {});
                        add(&mut d, None, "t", solid([s, b, s], a), |p| {
                            p.blend = mode;
                            p.opacity = 0.7;
                        });
                        assert_eq!(at(&d, 2, 2)[3], 255);
                    }
                }
            }
        }
    }

    #[test]
    fn group_at_half_opacity_equals_flattened_then_faded() {
        // Acceptance 23.
        let mut d = doc();
        add(&mut d, None, "bg", solid([0, 255, 0], 255), |_| {});
        let g = d.push_group(None, Props::new("g"), None);
        add(&mut d, Some(g), "1", solid(RED, 255), |_| {});
        add(&mut d, Some(g), "2", solid(BLUE, 128), |_| {});
        d.node_mut(g).unwrap().props.opacity = 0.5;
        let grouped = at(&d, 0, 0);

        let mut f = doc();
        add(&mut f, None, "bg", solid([0, 255, 0], 255), |_| {});
        add(&mut f, None, "1", solid(RED, 255), |_| {});
        add(&mut f, None, "2", solid(BLUE, 128), |_| {});
        let flat = composite_to_layer(&{
            let mut only = doc();
            add(&mut only, None, "1", solid(RED, 255), |_| {});
            add(&mut only, None, "2", solid(BLUE, 128), |_| {});
            only
        }, false);
        let mut faded = doc();
        add(&mut faded, None, "bg", solid([0, 255, 0], 255), |_| {});
        let top = faded.push_layer(None, Props::new("flat"), None, flat);
        faded.node_mut(top).unwrap().props.opacity = 0.5;
        near(grouped, at(&faded, 0, 0));
        // And it differs from the ungrouped stack at half opacity applied per layer.
        assert_ne!(grouped, at(&f, 0, 0));
    }

    #[test]
    fn group_isolation() {
        // Acceptance 24.
        let mut d = doc();
        d.background = [1.0, 0.0, 0.0];
        let g = d.push_group(None, Props::new("g"), None);
        add(&mut d, Some(g), "w", solid([255, 255, 255], 255), |p| p.blend = BlendMode::Multiply);
        near(at(&d, 0, 0), [255, 255, 255, 255]);
        // Without the group the Multiply keeps the red.
        let mut e = doc();
        e.background = [1.0, 0.0, 0.0];
        add(&mut e, None, "w", solid([255, 255, 255], 255), |p| p.blend = BlendMode::Multiply);
        near(at(&e, 0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn hidden_and_empty_groups() {
        // Acceptance 25 and 26.
        let mut d = doc();
        add(&mut d, None, "b", solid(RED, 255), |_| {});
        let empty = d.push_group(None, Props::new("empty"), None);
        near(at(&d, 0, 0), [255, 0, 0, 255]);
        let g = d.push_group(None, Props::new("g"), None);
        add(&mut d, Some(g), "blue", solid(BLUE, 255), |_| {});
        near(at(&d, 0, 0), [0, 0, 255, 255]);
        d.node_mut(g).unwrap().props.visible = false;
        near(at(&d, 0, 0), [255, 0, 0, 255]);
        let _ = empty;
    }

    #[test]
    fn group_mask_and_blend() {
        let mut d = doc();
        add(&mut d, None, "b", solid(RED, 255), |_| {});
        let g = d.push_group(None, Props::new("g"), Some(Mask::filled(W, H, 0)));
        add(&mut d, Some(g), "blue", solid(BLUE, 255), |_| {});
        near(at(&d, 0, 0), [255, 0, 0, 255]);
        d.node_mut(g).unwrap().mask = None;
        d.node_mut(g).unwrap().props.blend = BlendMode::Multiply;
        near(at(&d, 0, 0), [0, 0, 0, 255]);
    }

    /// Left half of a base layer opaque red, right half transparent.
    fn half_base() -> Vec<u8> {
        let mut px = vec![0u8; W * H * 4];
        for y in 0..H {
            for x in 0..W / 2 {
                px[(y * W + x) * 4..(y * W + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        px
    }

    #[test]
    fn clipping_stays_inside_the_base() {
        // Acceptance 27 and 28.
        let mut d = doc();
        add(&mut d, None, "under", solid([0, 255, 0], 255), |_| {});
        add(&mut d, None, "base", half_base(), |_| {});
        add(&mut d, None, "clip", solid(BLUE, 255), |p| p.clipped = true);
        near(at(&d, 0, 0), [0, 0, 255, 255]);
        near(at(&d, 3, 0), [0, 255, 0, 255]);
    }

    #[test]
    fn clip_group_fades_as_a_whole() {
        // Acceptance 29.
        let mut d = doc();
        add(&mut d, None, "base", solid(RED, 255), |p| p.opacity = 0.5);
        add(&mut d, None, "clip", solid(BLUE, 255), |p| p.clipped = true);
        // Blue replaces red in the clip group, then the group is shown at 50% over white.
        near(at(&d, 0, 0), [128, 128, 255, 255]);
    }

    #[test]
    fn clipped_layer_without_base_draws_normally() {
        // Acceptance 30.
        let mut d = doc();
        add(&mut d, None, "clip", solid(BLUE, 255), |p| p.clipped = true);
        near(at(&d, 0, 0), [0, 0, 255, 255]);
        let mut e = doc();
        let g = e.push_group(None, Props::new("g"), None);
        add(&mut e, Some(g), "x", solid(RED, 255), |_| {});
        add(&mut e, None, "clip", solid(BLUE, 255), |p| p.clipped = true);
        near(at(&e, 0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn hidden_base_hides_clipped_layers() {
        // Acceptance 31.
        let mut d = doc();
        add(&mut d, None, "base", solid(RED, 255), |p| p.visible = false);
        add(&mut d, None, "clip", solid(BLUE, 255), |p| p.clipped = true);
        near(at(&d, 0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn clipped_layer_uses_its_blend_mode() {
        let mut d = doc();
        add(&mut d, None, "base", solid([255, 128, 0], 255), |_| {});
        add(&mut d, None, "clip", solid([128, 128, 128], 255), |p| {
            p.clipped = true;
            p.blend = BlendMode::Multiply;
        });
        near(at(&d, 0, 0), [128, 64, 0, 255]);
    }

    #[test]
    fn private_items_can_be_left_out() {
        let mut d = doc();
        add(&mut d, None, "p", solid(RED, 255), |p| p.private = true);
        let opts = CompositeOpts { skip_private: true, background: true };
        assert_eq!(composite(&d, opts)[0..4], [255, 255, 255, 255]);
        assert_eq!(composite(&d, CompositeOpts::default())[0..4], [255, 0, 0, 255]);
    }

    #[test]
    fn rect_matches_the_same_area_of_a_full_composite() {
        let mut d = Document::empty(300, 200);
        let mut px = vec![0u8; 300 * 200 * 4];
        for (i, v) in px.iter_mut().enumerate() {
            *v = if i % 4 == 3 { 200 } else { (i % 97) as u8 };
        }
        for (i, v) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let a = v[3];
            for c in v.iter_mut().take(3) {
                *c = (*c).min(a);
            }
            let _ = i;
        }
        let g = d.push_group(None, Props::new("g"), None);
        d.push_layer(Some(g), Props::new("a"), None, px.clone());
        let mut p = Props::new("b");
        p.blend = BlendMode::Overlay;
        p.opacity = 0.6;
        d.push_layer(Some(g), p, Some(Mask::filled(300, 200, 200)), px);
        let full = composite(&d, CompositeOpts::default());
        let (rx, ry, rw, rh) = (120, 60, 140, 90);
        let part = composite_rect(&d, rx, ry, rw, rh, CompositeOpts::default());
        for y in 0..rh {
            for x in 0..rw {
                let a = &part[(y * rw + x) * 4..][..4];
                let b = &full[((ry + y) * 300 + rx + x) * 4..][..4];
                assert_eq!(a, b);
            }
        }
        // Out-of-range rectangles are clipped.
        assert_eq!(composite_rect(&d, 290, 190, 50, 50, CompositeOpts::default()).len(), 10 * 10 * 4);
        assert!(composite_rect(&d, 400, 0, 5, 5, CompositeOpts::default()).is_empty());
    }
}


/// The block compositor as it was before it was parallelised and specialised, kept as the
/// reference the new one is compared with byte for byte.
#[cfg(test)]
#[allow(dead_code, clippy::too_many_arguments, clippy::needless_range_loop)]
mod reference {
    use super::CompositeOpts;
    use crate::blend::{blend_colour, BlendMode};
    use crate::canvas::Layer;
    use crate::compose::round_to_u8;
    use crate::document::{Document, Mask, Node, NodeKind};
    use crate::pixel::{Chan, Depth};

    /// Edge of one processing block in pixels.
    const BLOCK: usize = 128;

    /// Premultiplied RGBA.
    type Px = [f32; 4];

    #[derive(Clone, Copy)]
    struct Block {
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    }

    fn fin01(v: f64) -> f64 {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Composite a rectangle of the document. Returns straight RGBA8 for the rectangle clipped to
    /// the canvas (`w * h` pixels, row-major). With a visible background the alpha is 255.
    pub fn composite_rect(doc: &Document, x: usize, y: usize, w: usize, h: usize, opts: CompositeOpts) -> Vec<u8> {
        let x1 = x.saturating_add(w).min(doc.width);
        let y1 = y.saturating_add(h).min(doc.height);
        let x0 = x.min(x1);
        let y0 = y.min(y1);
        let (rw, rh) = (x1 - x0, y1 - y0);
        let mut out = vec![0u8; rw * rh * 4];
        let bg: Px = if opts.background && doc.background_visible {
            let c = doc.background.map(|v| fin01(v) as f32);
            [c[0], c[1], c[2], 1.0]
        } else {
            [0.0; 4]
        };
        let mut by = y0;
        while by < y1 {
            let bh = BLOCK.min(y1 - by);
            let mut bx = x0;
            while bx < x1 {
                let bw = BLOCK.min(x1 - bx);
                let blk = Block { x: bx, y: by, w: bw, h: bh };
                let mut buf = vec![bg; bw * bh];
                composite_items(doc, &doc.root, blk, &mut buf, opts);
                for r in 0..bh {
                    let dst = ((by - y0 + r) * rw + (bx - x0)) * 4;
                    for c in 0..bw {
                        let p = buf[r * bw + c];
                        out[dst + c * 4..dst + c * 4 + 4].copy_from_slice(&to_straight_u8(p));
                    }
                }
                bx += bw;
            }
            by += bh;
        }
        out
    }

    /// [`composite_rect`] as straight RGBA16 (`w * h * 4` samples). The blending runs in `f32` and
    /// is rounded once, so a 16-bit document keeps its precision; an 8-bit document is widened.
    pub fn composite_rect16(doc: &Document, x: usize, y: usize, w: usize, h: usize, opts: CompositeOpts) -> Vec<u16> {
        let x1 = x.saturating_add(w).min(doc.width);
        let y1 = y.saturating_add(h).min(doc.height);
        let x0 = x.min(x1);
        let y0 = y.min(y1);
        let (rw, rh) = (x1 - x0, y1 - y0);
        let mut out = vec![0u16; rw * rh * 4];
        let bg: Px = if opts.background && doc.background_visible {
            let c = doc.background.map(|v| fin01(v) as f32);
            [c[0], c[1], c[2], 1.0]
        } else {
            [0.0; 4]
        };
        let mut by = y0;
        while by < y1 {
            let bh = BLOCK.min(y1 - by);
            let mut bx = x0;
            while bx < x1 {
                let bw = BLOCK.min(x1 - bx);
                let blk = Block { x: bx, y: by, w: bw, h: bh };
                let mut buf = vec![bg; bw * bh];
                composite_items(doc, &doc.root, blk, &mut buf, opts);
                for r in 0..bh {
                    let dst = ((by - y0 + r) * rw + (bx - x0)) * 4;
                    for c in 0..bw {
                        let p = buf[r * bw + c];
                        out[dst + c * 4..dst + c * 4 + 4].copy_from_slice(&to_straight_u16(p));
                    }
                }
                bx += bw;
            }
            by += bh;
        }
        out
    }

    /// Composite the whole canvas. See [`composite_rect`].
    pub fn composite(doc: &Document, opts: CompositeOpts) -> Vec<u8> {
        composite_rect(doc, 0, 0, doc.width, doc.height, opts)
    }

    /// Composite the whole canvas as straight RGBA16. See [`composite_rect16`].
    pub fn composite16(doc: &Document, opts: CompositeOpts) -> Vec<u16> {
        composite_rect16(doc, 0, 0, doc.width, doc.height, opts)
    }

    /// Composite the whole canvas without the background, as premultiplied RGBA8 (the pixel
    /// format of an 8-bit [`Layer`]; a 16-bit document rounds). Used by merge down and flatten.
    /// See [`composite_to_tiled`] for the tiled form, which keeps the document's depth and does
    /// not build a full-size buffer.
    pub fn composite_to_layer(doc: &Document, skip_private: bool) -> Vec<u8> {
        composite_to_tiled(doc, skip_private).to_rgba()
    }

    /// [`composite_to_layer`] as a tiled [`Layer`]: tiles where nothing is drawn are not allocated.
    pub fn composite_to_tiled(doc: &Document, skip_private: bool) -> Layer {
        let opts = CompositeOpts {
            skip_private,
            background: false,
        };
        let (w, h) = (doc.width, doc.height);
        let mut out = Layer::new_depth(w, h, doc.depth);
        let mut bytes = Vec::new();
        let mut words: Vec<u16> = Vec::new();
        let mut by = 0;
        while by < h {
            let bh = BLOCK.min(h - by);
            let mut bx = 0;
            while bx < w {
                let bw = BLOCK.min(w - bx);
                let blk = Block { x: bx, y: by, w: bw, h: bh };
                let mut buf = vec![[0.0f32; 4]; bw * bh];
                composite_items(doc, &doc.root, blk, &mut buf, opts);
                match doc.depth {
                    Depth::U8 => {
                        bytes.clear();
                        for p in &buf {
                            bytes.extend(p.iter().map(|&v| round_to_u8(fin01(f64::from(v)) * 255.0)));
                        }
                        out.write_rect(bx, by, bw, bh, &bytes);
                    }
                    Depth::U16 => {
                        words.clear();
                        for p in &buf {
                            words.extend(p.iter().map(|&v| <u16 as Chan>::from_unit(f64::from(v))));
                        }
                        out.write_rect16(bx, by, bw, bh, &words);
                    }
                }
                bx += bw;
            }
            by += bh;
        }
        out
    }

    fn to_straight_u8(p: Px) -> [u8; 4] {
        let a = fin01(f64::from(p[3]));
        if a <= 0.0 {
            return [0; 4];
        }
        let f = |v: f32| round_to_u8(fin01(f64::from(v) / a) * 255.0);
        [f(p[0]), f(p[1]), f(p[2]), round_to_u8(a * 255.0)]
    }

    fn to_straight_u16(p: Px) -> [u16; 4] {
        let a = fin01(f64::from(p[3]));
        if a <= 0.0 {
            return [0; 4];
        }
        let f = |v: f32| <u16 as Chan>::from_unit(fin01(f64::from(v) / a));
        [f(p[0]), f(p[1]), f(p[2]), <u16 as Chan>::from_unit(a)]
    }

    /// Is this item left out of the composite?
    fn skipped(n: &Node, opts: CompositeOpts) -> bool {
        !n.props.visible || (opts.skip_private && n.props.private)
    }

    /// What a layer needs while compositing one block.
    struct LayerRef<'a> {
        node: &'a Node,
        pixels: &'a Layer,
    }

    fn resolve<'a>(doc: &'a Document, n: &'a Node) -> Option<LayerRef<'a>> {
        let pixels = doc.layer(n.id)?;
        Some(LayerRef { node: n, pixels })
    }

    fn mask_at(m: Option<&Mask>, i: usize) -> f64 {
        m.map_or(1.0, |m| f64::from(m.data.get(i).copied().unwrap_or(255)) / 255.0)
    }

    /// Composite `items` (bottom to top) onto `dst`, which covers `blk`.
    fn composite_items(doc: &Document, items: &[Node], blk: Block, dst: &mut [Px], opts: CompositeOpts) {
        let mut i = 0;
        while i < items.len() {
            let item = &items[i];
            i += 1;
            // The clipped layers directly above a base layer. A clipped layer that
            // is itself reached here has no valid base and draws unclipped.
            let mut run_end = i;
            if !item.is_group() && !item.props.clipped {
                while run_end < items.len() && !items[run_end].is_group() && items[run_end].props.clipped {
                    run_end += 1;
                }
            }
            let run = &items[i..run_end];
            i = run_end;
            if skipped(item, opts) {
                continue; // a hidden base takes its clipped layers with it
            }
            match &item.kind {
                NodeKind::Group { children, .. } => composite_group(doc, item, children, blk, dst, opts),
                NodeKind::Layer => {
                    let Some(base) = resolve(doc, item) else {
                        continue;
                    };
                    let clipped: Vec<LayerRef> = run
                        .iter()
                        .filter(|n| !skipped(n, opts))
                        .filter_map(|n| resolve(doc, n))
                        .collect();
                    if clipped.is_empty() {
                        draw_layer(doc, &base, blk, dst);
                    } else {
                        draw_clip_group(doc, &base, &clipped, blk, dst);
                    }
                }
            }
        }
    }

    fn composite_group(doc: &Document, node: &Node, children: &[Node], blk: Block, dst: &mut [Px], opts: CompositeOpts) {
        if children.is_empty() || node.props.opacity <= 0.0 {
            return;
        }
        let mut buf = vec![[0.0f32; 4]; blk.w * blk.h];
        composite_items(doc, children, blk, &mut buf, opts);
        let mode = node.props.blend;
        let opacity = fin01(node.props.opacity);
        for r in 0..blk.h {
            for c in 0..blk.w {
                let p = buf[r * blk.w + c];
                let a = f64::from(p[3]);
                if a <= 0.0 {
                    continue;
                }
                let o = opacity * mask_at(node.mask.as_ref(), (blk.y + r) * doc.width + blk.x + c);
                if o <= 0.0 {
                    continue;
                }
                let cs = [f64::from(p[0]) / a, f64::from(p[1]) / a, f64::from(p[2]) / a];
                over(&mut dst[r * blk.w + c], cs, a, o, mode);
            }
        }
    }

    /// Premultiplied pixel to (straight colour, alpha).
    #[inline(always)]
    fn straight<T: Chan>(px: &[T]) -> ([f64; 3], f64) {
        let a = px[3].to_unit();
        if px[3].is_zero() {
            return ([0.0; 3], 0.0);
        }
        let f = |v: T| fin01(v.raw() / px[3].raw());
        ([f(px[0]), f(px[1]), f(px[2])], a)
    }

    /// Single-layer composite of one block. Each row is walked one tile span at a
    /// time, so an empty tile is skipped without looking at its pixels.
    fn draw_layer(doc: &Document, l: &LayerRef, blk: Block, dst: &mut [Px]) {
        let opacity = fin01(l.node.props.opacity);
        if opacity <= 0.0 {
            return;
        }
        let mode = l.node.props.blend;
        let mask = l.node.mask.as_ref();
        for r in 0..blk.h {
            let y = blk.y + r;
            for (x, span) in l.pixels.row_spans(y, blk.x, blk.x + blk.w) {
                if let Some(data) = span.data {
                    draw_span(doc, data, y, x, blk, r, opacity, mode, mask, dst);
                } else if let Some(data) = span.data16 {
                    draw_span(doc, data, y, x, blk, r, opacity, mode, mask, dst);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn draw_span<T: Chan>(
        doc: &Document,
        data: &[T],
        y: usize,
        x: usize,
        blk: Block,
        r: usize,
        opacity: f64,
        mode: BlendMode,
        mask: Option<&Mask>,
        dst: &mut [Px],
    ) {
        for (c, px) in data.as_chunks::<4>().0.iter().enumerate() {
            if px[3].is_zero() {
                continue;
            }
            let o = opacity * mask_at(mask, y * doc.width + x + c);
            if o <= 0.0 {
                continue;
            }
            let (cs, a) = straight(px);
            over(&mut dst[r * blk.w + (x - blk.x) + c], cs, a, o, mode);
        }
    }

    /// A base layer and the clipped layers above it.
    fn draw_clip_group(doc: &Document, base: &LayerRef, clipped: &[LayerRef], blk: Block, dst: &mut [Px]) {
        match base.pixels.depth() {
            Depth::U8 => draw_clip_group_t::<u8>(doc, base, clipped, blk, dst),
            Depth::U16 => draw_clip_group_t::<u16>(doc, base, clipped, blk, dst),
        }
    }

    fn draw_clip_group_t<T: Chan>(doc: &Document, base: &LayerRef, clipped: &[LayerRef], blk: Block, dst: &mut [Px]) {
        let opacity = fin01(base.node.props.opacity);
        if opacity <= 0.0 {
            return;
        }
        let mode = base.node.props.blend;
        let mut spans: Vec<Option<&[T]>> = Vec::with_capacity(clipped.len());
        for r in 0..blk.h {
            let y = blk.y + r;
            let x_end = blk.x + blk.w;
            let mut x = blk.x;
            while x < x_end {
                // Every layer has the same tile grid, so all spans at (x, y) have the same length.
                let (len, base_span) = base.pixels.row_span_t::<T>(x, y, x_end);
                let Some(base_data) = base_span else {
                    x += len; // A = 0 for the whole span: nothing from the clip group shows here
                    continue;
                };
                spans.clear();
                spans.extend(clipped.iter().map(|cl| cl.pixels.row_span_t::<T>(x, y, x_end).1));
                for c in 0..len {
                    let bp = &base_data[c * 4..c * 4 + 4];
                    if bp[3].is_zero() {
                        continue;
                    }
                    let m = y * doc.width + x + c;
                    let (mut cg, a_base) = straight(bp);
                    let group_alpha = a_base * mask_at(base.node.mask.as_ref(), m);
                    if group_alpha <= 0.0 {
                        continue;
                    }
                    for (cl, span) in clipped.iter().zip(&spans) {
                        let Some(data) = span else {
                            continue;
                        };
                        let cp = &data[c * 4..c * 4 + 4];
                        if cp[3].is_zero() {
                            continue;
                        }
                        let (cs, a) = straight(cp);
                        let w = fin01(cl.node.props.opacity) * mask_at(cl.node.mask.as_ref(), m) * a;
                        if w <= 0.0 {
                            continue;
                        }
                        let b = blend_colour(cl.node.props.blend, cg, cs);
                        for k in 0..3 {
                            cg[k] = (1.0 - w) * cg[k] + w * b[k];
                        }
                    }
                    over(&mut dst[r * blk.w + (x - blk.x) + c], cg, group_alpha, opacity, mode);
                }
                x += len;
            }
        }
    }

    /// The single-layer formula for one pixel: source straight colour `cs`, alpha
    /// `a_s`, effective opacity `o`, blend `mode`, over the premultiplied backdrop `dst`.
    fn over(dst: &mut Px, cs: [f64; 3], a_s: f64, o: f64, mode: BlendMode) {
        let ab = f64::from(dst[3]);
        let d = [f64::from(dst[0]), f64::from(dst[1]), f64::from(dst[2])];
        let blended = if ab > 0.0 && mode != BlendMode::Normal {
            let cb = [d[0] / ab, d[1] / ab, d[2] / ab];
            blend_colour(mode, cb, cs)
        } else {
            cs
        };
        let alpha_o = a_s + (1.0 - a_s) * ab;
        for k in 0..3 {
            let co = a_s * ((1.0 - ab) * cs[k] + ab * blended[k]) + (1.0 - a_s) * d[k];
            dst[k] = fin01(d[k] + o * (co - d[k])) as f32;
        }
        dst[3] = fin01(ab + o * (alpha_o - ab)) as f32;
    }
}

/// Old against new, on randomised documents: every blend mode, opacity, masks (including short
/// ones), clipping, nested groups with opacity / blend / mask, hidden and private items, empty
/// and partly painted tiles, odd canvas sizes, partial edge tiles, dirty rectangles, 8 and
/// 16 bits, and 1 to 8 threads (with the "is it worth it" estimate switched off, so the threaded
/// path really runs).
#[cfg(test)]
mod equivalence {
    use super::*;
    use crate::document::{Mask, Props};

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64*
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() >> 11) as usize % n.max(1)
        }
        fn chance(&mut self, percent: usize) -> bool {
            self.below(100) < percent
        }
        fn unit(&mut self) -> f64 {
            (self.next() >> 11) as f64 / (1u64 << 53) as f64
        }
        fn byte(&mut self) -> u8 {
            (self.next() >> 24) as u8
        }
    }

    const SIZES: [(usize, usize); 10] = [
        (1, 1),
        (2, 3),
        (37, 19),
        (130, 129),
        (257, 131),
        (300, 200),
        (129, 257),
        (515, 260),
        (64, 513),
        (255, 255),
    ];

    /// One layer's premultiplied RGBA8: a random painted region, with all the alpha cases.
    fn pixels8(rng: &mut Rng, w: usize, h: usize) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 4];
        let kind = rng.below(6);
        if kind == 0 {
            return px; // an empty layer: every tile transparent
        }
        // The painted region: everything, or a random rectangle (so some tiles stay empty).
        let (x0, y0, x1, y1) = if kind == 1 {
            (0, 0, w, h)
        } else {
            let x0 = rng.below(w);
            let y0 = rng.below(h);
            (x0, y0, x0 + 1 + rng.below(w - x0), y0 + 1 + rng.below(h - y0))
        };
        let style = rng.below(5);
        for y in y0..y1 {
            for x in x0..x1 {
                let a = match style {
                    0 => 255,
                    1 => rng.byte(),
                    2 => {
                        if rng.chance(50) {
                            0
                        } else {
                            255
                        }
                    }
                    3 => 1 + rng.byte() % 4,
                    _ => ((x * 255) / w.max(1)) as u8,
                };
                let straight = [rng.byte(), rng.byte(), rng.byte()];
                let o = (y * w + x) * 4;
                for k in 0..3 {
                    let p = ((u32::from(straight[k]) * u32::from(a) + 127) / 255) as u8;
                    // Now and then colour above alpha, which a layer can hold after filters.
                    px[o + k] = if rng.chance(2) { straight[k] } else { p };
                }
                px[o + 3] = a;
            }
        }
        px
    }

    fn pixels16(rng: &mut Rng, w: usize, h: usize) -> Vec<u16> {
        let base = pixels8(rng, w, h);
        base.iter()
            .map(|&v| {
                let wide = crate::pixel::up8(v);
                // Jitter so the values are not all multiples of 257.
                if rng.chance(60) {
                    wide.saturating_add(rng.below(257) as u16)
                } else {
                    wide
                }
            })
            .collect()
    }

    fn opacity(rng: &mut Rng) -> f64 {
        match rng.below(10) {
            0 => 0.0,
            1..=3 => 1.0,
            4 => 0.5,
            5 => 1.5,
            6 => -0.2,
            7 => f64::NAN,
            _ => rng.unit(),
        }
    }

    fn mask(rng: &mut Rng, w: usize, h: usize) -> Option<Mask> {
        if !rng.chance(30) {
            return None;
        }
        let mut m = match rng.below(5) {
            0 => Mask::filled(w, h, 0),
            1 => Mask::filled(w, h, 255),
            2 => Mask::filled(w, h, 128),
            _ => Mask {
                data: (0..w * h).map(|_| rng.byte()).collect(),
            },
        };
        if rng.chance(15) {
            // A short mask: the missing bytes mean "reveal".
            m.data.truncate(rng.below(w * h + 1));
        }
        Some(m)
    }

    fn props(rng: &mut Rng, name: &str, allow_clip: bool) -> Props {
        let mut p = Props::new(name);
        p.visible = !rng.chance(12);
        p.opacity = opacity(rng);
        p.blend = if rng.chance(40) {
            BlendMode::Normal
        } else {
            BlendMode::ALL[rng.below(BlendMode::ALL.len())]
        };
        p.clipped = allow_clip && rng.chance(30);
        p.private = rng.chance(10);
        p
    }

    fn fill(rng: &mut Rng, d: &mut Document, parent: Option<u64>, depth: usize, count: usize) {
        let (w, h) = (d.width, d.height);
        for i in 0..count {
            if depth < 2 && rng.chance(25) {
                let p = props(rng, "g", false);
                let m = mask(rng, w, h);
                let g = d.push_group(parent, p, m);
                let n = rng.below(4);
                fill(rng, d, Some(g), depth + 1, n);
            } else {
                let p = props(rng, &format!("l{i}"), true);
                let m = mask(rng, w, h);
                if d.depth == Depth::U16 {
                    let px = pixels16(rng, w, h);
                    d.push_layer16(parent, p, m, px);
                } else {
                    let px = pixels8(rng, w, h);
                    d.push_layer(parent, p, m, px);
                }
            }
        }
    }

    fn random_doc(rng: &mut Rng, depth: Depth) -> Document {
        let (w, h) = SIZES[rng.below(SIZES.len())];
        let mut d = Document::empty_depth(w, h, depth);
        d.background = [rng.unit(), rng.unit(), rng.unit()];
        if rng.chance(8) {
            d.background = [1.5, -1.0, f64::NAN];
        }
        d.background_visible = rng.chance(75);
        let n = 1 + rng.below(6);
        fill(rng, &mut d, None, 0, n);
        d
    }

    fn random_rect(rng: &mut Rng, d: &Document) -> (usize, usize, usize, usize) {
        match rng.below(5) {
            0 => (0, 0, d.width, d.height),
            1 => (rng.below(d.width + 3), rng.below(d.height + 3), rng.below(300), rng.below(300)),
            2 => (rng.below(d.width), rng.below(d.height), usize::MAX, usize::MAX),
            3 => (d.width.saturating_sub(1), d.height.saturating_sub(1), 5, 5),
            _ => (
                rng.below(d.width),
                rng.below(d.height),
                1 + rng.below(d.width),
                1 + rng.below(d.height),
            ),
        }
    }

    const PARS: [Par; 4] = [
        Par { threads: 1, min_secs: 0.0 },
        Par { threads: 2, min_secs: 0.0 },
        Par { threads: 3, min_secs: 0.0 },
        Par { threads: 8, min_secs: 0.0 },
    ];

    fn check8(doc: &Document, seed: u64, rng: &mut Rng) {
        for opts in [
            CompositeOpts::default(),
            CompositeOpts { skip_private: true, background: true },
            CompositeOpts { skip_private: false, background: false },
        ] {
            let (x, y, w, h) = random_rect(rng, doc);
            let want = reference::composite_rect(doc, x, y, w, h, opts);
            for par in PARS {
                let got = composite_rect_par(doc, x, y, w, h, opts, par);
                assert!(got == want, "seed {seed}: rect ({x},{y},{w},{h}) {opts:?} threads {}", par.threads);
            }
            // The default entry point too.
            assert!(composite_rect(doc, x, y, w, h, opts) == want, "seed {seed}: default entry point");
            // And the premultiplied form for egui.
            let want_pm: Vec<u8> = want
                .as_chunks::<4>().0.iter()
                .flat_map(|p| {
                    let a = p[3];
                    let f = |v: u8| ((f32::from(v) * (f32::from(a) / 255.0)) + 0.5) as u8;
                    match a {
                        0 => [0; 4],
                        255 => [p[0], p[1], p[2], 255],
                        _ => [f(p[0]), f(p[1]), f(p[2]), a],
                    }
                })
                .collect();
            let got_pm: Vec<u8> = bytemuck::cast_slice(&composite_rect_premul(doc, x, y, w, h, opts, &|_| {})).to_vec();
            assert!(got_pm == want_pm, "seed {seed}: premultiplied rect ({x},{y},{w},{h}) {opts:?}");
        }
    }

    #[test]
    fn eight_bit_matches_the_reference() {
        for seed in 1..=70u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let doc = random_doc(&mut rng, Depth::U8);
            check8(&doc, seed, &mut rng);
        }
    }

    #[test]
    fn sixteen_bit_matches_the_reference() {
        for seed in 1..=30u64 {
            let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
            let doc = random_doc(&mut rng, Depth::U16);
            let opts = CompositeOpts::default();
            let (x, y, w, h) = random_rect(&mut rng, &doc);
            let want = reference::composite_rect16(&doc, x, y, w, h, opts);
            let want8 = reference::composite_rect(&doc, x, y, w, h, opts);
            for par in PARS {
                assert!(composite_rect16_par(&doc, x, y, w, h, opts, par) == want, "seed {seed}: 16-bit rect, threads {}", par.threads);
                assert!(composite_rect_par(&doc, x, y, w, h, opts, par) == want8, "seed {seed}: 8-bit rect of a 16-bit document");
            }
        }
    }

    #[test]
    fn tiled_output_matches_the_reference() {
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407) | 1);
            let depth = if seed % 3 == 0 { Depth::U16 } else { Depth::U8 };
            let doc = random_doc(&mut rng, depth);
            for skip_private in [false, true] {
                let want = reference::composite_to_tiled(&doc, skip_private);
                for par in PARS {
                    let got = composite_to_tiled_par(&doc, skip_private, par);
                    assert_eq!(got.depth(), want.depth());
                    assert_eq!(got.tile_count(), want.tile_count(), "seed {seed}: allocated tiles, threads {}", par.threads);
                    assert!(got.to_rgba16() == want.to_rgba16(), "seed {seed}: tiled pixels, threads {}", par.threads);
                }
            }
        }
    }

    #[test]
    fn premultiply_matches_the_float_form_for_every_value() {
        // egui documents `mul_frac_round` as equal to the float multiply it replaced.
        for a in 0..=255u8 {
            for v in 0..=255u8 {
                let want = match a {
                    0 => [0; 4],
                    255 => [v, v, v, 255],
                    _ => {
                        let f = ((f32::from(v) * (f32::from(a) / 255.0)) + 0.5) as u8;
                        [f, f, f, a]
                    }
                };
                assert_eq!(premultiply_u8([v, v, v, a]), want, "v {v} a {a}");
            }
        }
    }

    #[test]
    fn display_map_runs_before_premultiplication() {
        let mut rng = Rng(77);
        let doc = random_doc(&mut rng, Depth::U8);
        let invert = |px: &mut [u8]| {
            for p in px.as_chunks_mut::<4>().0 {
                p[0] = 255 - p[0];
            }
        };
        let (w, h) = (doc.width, doc.height);
        let mut want = reference::composite_rect(&doc, 0, 0, w, h, CompositeOpts::default());
        invert(&mut want);
        for p in want.as_chunks_mut::<4>().0 {
            *p = premultiply_u8(*p);
        }
        assert!(bytemuck::cast_slice::<u32, u8>(&composite_rect_premul(&doc, 0, 0, w, h, CompositeOpts::default(), &invert)) == &want[..]);
    }

    #[test]
    fn unit_to_u8_is_round_to_u8() {
        let mut rng = Rng(5);
        let mut vals: Vec<f32> = (0..=255u32).flat_map(|k| [k as f32 / 255.0, (k as f32 + 0.5) / 255.0, (k as f64 / 255.0) as f32]).collect();
        vals.extend((0..4_000_000).map(|_| rng.unit() as f32));
        vals.extend((0..400_000).map(|_| (rng.unit() * rng.unit() * 1e-3) as f32));
        vals.extend([0.0, 1.0, f32::MIN_POSITIVE, 1e-40, 0.5 / 255.0, 1.0 - f32::EPSILON / 2.0]);
        // Every f32 next to a rounding boundary k + 0.5.
        for k in 0..255u32 {
            let b = ((f64::from(k) + 0.5) / 255.0) as f32;
            for step in -3i32..=3 {
                vals.push(f32::from_bits((b.to_bits() as i64 + i64::from(step)) as u32));
            }
        }
        for v in vals {
            if !(0.0..=1.0).contains(&v) {
                continue;
            }
            assert_eq!(unit_to_u8(v), round_to_u8(fin01(f64::from(v)) * 255.0), "{v}");
        }
    }

    #[test]
    fn opaque_shortcut_rounds_like_the_general_formula() {
        // Opaque source over alpha 0 or 1, Normal, opacity 1: the general formula per channel
        // against `cs as f32`, for every colour and many backdrop values.
        let t = tables();
        let mut rng = Rng(99);
        let mut backdrops: Vec<f32> = (0..=255u32).map(|v| (v as f64 / 255.0) as f32).collect();
        backdrops.extend((0..2_000_000).map(|_| rng.unit() as f32));
        backdrops.extend((0..200_000).map(|_| (rng.unit() * rng.unit() * rng.unit() * 1e-3) as f32));
        backdrops.extend([0.0, 1.0, f32::MIN_POSITIVE, 1e-30]);
        for c in 0..=255usize {
            let cs = t.straight[255 << 8 | c];
            for &d in &backdrops {
                for ab in [0.0f64, 1.0] {
                    let d = f64::from(d);
                    let co = 1.0 * ((1.0 - ab) * cs + ab * cs) + (1.0 - 1.0) * d;
                    let general = fin01(d + 1.0 * (co - d)) as f32;
                    assert_eq!(general, cs as f32, "c {c} d {d} ab {ab}");
                }
            }
        }
    }

    /// A 1024 x 700 document large enough for several block rows and tile edges, full-size
    /// layers with every blend mode, clipping and groups: the shape of the benchmark's
    /// "mixed" document.
    #[test]
    fn large_mixed_document_matches_the_reference() {
        let (w, h) = (1030, 703);
        let mut rng = Rng(0xC0FFEE);
        let mut d = Document::empty(w, h);
        let g = d.push_group(None, Props::new("g"), None);
        for (i, mode) in BlendMode::ALL.iter().enumerate() {
            let mut p = Props::new(format!("l{i}"));
            p.blend = *mode;
            p.opacity = 0.6 + 0.01 * i as f64;
            p.clipped = i % 5 == 4;
            let px: Vec<u8> = {
                let mut v = pixels8(&mut rng, w, h);
                // Make most of it opaque so the layers really stack up.
                for (k, b) in v.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    if k % 7 != 0 {
                        b[3] = 255;
                    }
                }
                v
            };
            let parent = if i % 2 == 0 { Some(g) } else { None };
            d.push_layer(parent, p, mask(&mut rng, w, h), px);
        }
        d.node_mut(g).unwrap().props.opacity = 0.8;
        d.node_mut(g).unwrap().props.blend = BlendMode::Overlay;
        let want = reference::composite(&d, CompositeOpts::default());
        for par in PARS {
            assert!(composite_rect_par(&d, 0, 0, w, h, CompositeOpts::default(), par) == want, "threads {}", par.threads);
        }
    }

    fn bench_doc(side: usize, layers: usize, mixed: bool) -> Document {
        let mut rng = Rng(12345);
        let mut d = Document::empty(side, side);
        let mut px = vec![0u8; side * side * 4];
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let a = if i % 5 == 0 { 200 } else { 255 };
            let c = |v: u8| ((u32::from(v) * a + 127) / 255) as u8;
            *p = [c((i % 251) as u8), c((i % 241) as u8), c((i % 239) as u8), a as u8];
        }
        let g = d.push_group(None, Props::new("g"), None);
        for i in 0..layers {
            let mut p = Props::new(format!("l{i}"));
            if mixed {
                p.blend = BlendMode::ALL[[0, 1, 2, 11, 17, 13, 14, 3][i % 8]];
                p.opacity = 0.6 + 0.02 * (i % 10) as f64;
                p.clipped = i % 5 == 4;
            }
            let parent = if mixed && i % 3 == 0 { Some(g) } else { None };
            let _ = rng.next();
            d.push_layer(parent, p, None, px.clone());
        }
        d
    }

    /// `cargo test --release -p eskiz-core --lib timing_vs_reference -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn timing_vs_reference() {
        let time = |f: &mut dyn FnMut()| {
            f();
            let t = std::time::Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e3
        };
        for (side, layers, mixed) in [(2048, 1, false), (2048, 20, false), (2048, 20, true), (4096, 1, false)] {
            let d = bench_doc(side, layers, mixed);
            let o = CompositeOpts::default();
            let old = time(&mut || drop(reference::composite(&d, o)));
            let one = time(&mut || drop(composite_rect_par(&d, 0, 0, side, side, o, Par { threads: 1, min_secs: 0.0 })));
            let auto = time(&mut || drop(composite(&d, o)));
            let r1 = time(&mut || drop(reference::composite_rect(&d, 100, 100, 1024, 1024, o)));
            let r2 = time(&mut || drop(composite_rect(&d, 100, 100, 1024, 1024, o)));
            println!("{side}^2 x{layers} mixed={mixed}: full old {old:.1} ms | new 1 thread {one:.1} | new auto ({} threads) {auto:.1} || 1024^2 rect old {r1:.1} new {r2:.1}", thread_count());
        }
    }

    /// Per blend mode: one layer of that mode over an opaque layer, 2048^2, one thread.
    #[test]
    #[ignore]
    fn timing_per_mode() {
        let side = 2048;
        for mode in BlendMode::ALL {
            let mut d = bench_doc(side, 2, false);
            let top = d.root.last().unwrap().id;
            d.node_mut(top).unwrap().props.blend = mode;
            d.node_mut(top).unwrap().props.opacity = 0.8;
            let o = CompositeOpts::default();
            let mut best_old = f64::MAX;
            let mut best_new = f64::MAX;
            for _ in 0..3 {
                let t = std::time::Instant::now();
                drop(reference::composite(&d, o));
                best_old = best_old.min(t.elapsed().as_secs_f64() * 1e3);
                let t = std::time::Instant::now();
                drop(composite_rect_par(&d, 0, 0, side, side, o, Par { threads: 1, min_secs: 0.0 }));
                best_new = best_new.min(t.elapsed().as_secs_f64() * 1e3);
            }
            println!("{:>14}: old {best_old:7.1} ms  new {best_new:7.1} ms", mode.name());
        }
    }

    #[test]
    #[ignore]
    fn timing_layers() {
        let side = 2048;
        for n in [0usize, 1, 2, 4] {
            for op in [1.0, 0.8] {
                let mut d = bench_doc(side, n, false);
                for id in d.layer_ids() {
                    d.node_mut(id).unwrap().props.opacity = op;
                }
                let o = CompositeOpts::default();
                let mut best_new = f64::MAX;
                let mut best_old = f64::MAX;
                for _ in 0..4 {
                    let t = std::time::Instant::now();
                    drop(composite_rect_par(&d, 0, 0, side, side, o, Par { threads: 1, min_secs: 0.0 }));
                    best_new = best_new.min(t.elapsed().as_secs_f64() * 1e3);
                    let t = std::time::Instant::now();
                    drop(reference::composite(&d, o));
                    best_old = best_old.min(t.elapsed().as_secs_f64() * 1e3);
                }
                println!("{n} layers opacity {op}: old {best_old:7.1} ms  new {best_new:7.1} ms");
            }
        }
    }
}
