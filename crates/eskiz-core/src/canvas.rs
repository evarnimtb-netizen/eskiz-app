//! Canvas and layers.
//!
//! A [`Layer`] stores premultiplied RGBA in **sparse 256x256 tiles**, at 8 or 16 bits per
//! channel ([`Depth`]). Each tile slot is an `Option<Arc<Tile>>`:
//!
//! * `None` is a fully transparent tile and costs nothing, so an empty layer allocates no
//!   pixel memory.
//! * Cloning a layer clones only the slot table and bumps reference counts. Undo, duplicate,
//!   the gallery's document snapshot and the stroke engines' "before" copy all rely on this.
//! * Writes go through [`Arc::make_mut`], so a write copies only the tile it touches, and
//!   only when another layer or history entry still shares it.
//!
//! Tiles are always a full 256x256 (edge tiles carry padding that stays zero). A layer
//! never holds a pixel outside `width x height`.
//!
//! # Depth
//!
//! A [`Tile`] is `U8` (256 KiB) or `U16` (512 KiB, native-endian `u16`). All tiles of a layer
//! have the layer's depth, and all layers of a document share the document's. The 8-bit loops
//! are the original ones: the tile code is generic over [`Chan`] and monomorphised per depth.
//!
//! # Pixel access
//!
//! Every accessor comes in an 8-bit form (`pixel`, `read_rect`, `to_rgba`, ...) and a 16-bit
//! form with a `16` suffix. On a layer of the *other* depth the call converts: 8-bit reads of a
//! 16-bit layer round to nearest, 16-bit reads of an 8-bit layer widen exactly. Hot paths and
//! anything that must keep the precision of a 16-bit layer use the `16` forms (or
//! [`Layer::update_rect_unit`], which hands the closure `f64` values in 0..1 at either depth).
//!
//! * One pixel: [`Layer::pixel`], [`Layer::set_pixel`].
//! * Rectangles: [`Layer::read_rect`], [`Layer::write_rect`].
//! * The whole layer as a flat buffer, for import, export and filters: [`Layer::to_rgba`],
//!   [`Layer::from_rgba`].
//! * Hot loops: [`Layer::row_spans`] / [`Layer::row_span`] hand out one tile-row slice at a
//!   time, and [`Layer::update_rect`] runs a per-pixel function over a rectangle one tile at
//!   a time. Neither does a tile lookup per pixel.
//! * Tiles themselves: [`Layer::tile`], [`Layer::set_tile`], [`Layer::is_empty_tile`],
//!   [`Layer::tile_count`], [`Layer::compact`], and [`TileSlot`] snapshots for undo.
//!
//! [`Layer::paint_stroke`] rasterises a stroke's dabs into a coverage buffer and composites
//! it onto the layer with the composite.

use std::sync::Arc;

use crate::compose::{self, PixelOpts};
use crate::geometry::Stroke;
use crate::pixel::{down_dithered, px_down, px_up, dither_threshold, Chan, Depth};
use crate::raster::{dab_pixel_rect, CoverageBuffer, DabRaster};

/// Edge of one tile in pixels.
pub const TILE: usize = 256;
/// Samples (channel values) in one tile: 256 x 256 pixels x 4 channels.
pub const TILE_SAMPLES: usize = TILE * TILE * 4;
/// Bytes in one 8-bit (RGBA8) tile. A 16-bit tile is twice this: [`Depth::tile_bytes`].
pub const TILE_BYTES: usize = TILE_SAMPLES;

pub(crate) static ZERO_TILE: [u8; TILE_SAMPLES] = [0; TILE_SAMPLES];
pub(crate) static ZERO_TILE16: [u16; TILE_SAMPLES] = [0; TILE_SAMPLES];

/// 256x256 premultiplied RGBA pixels, row-major, at 8 or 16 bits per channel.
#[derive(Clone, PartialEq, Eq)]
pub enum Tile {
    U8(Box<[u8]>),
    U16(Box<[u16]>),
}

impl std::fmt::Debug for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tile({}x{}, {})", TILE, TILE, self.depth().label())
    }
}

impl Tile {
    /// A transparent 8-bit tile.
    pub fn blank() -> Self {
        Tile::U8(vec![0u8; TILE_SAMPLES].into_boxed_slice())
    }

    /// A transparent 16-bit tile.
    pub fn blank16() -> Self {
        Tile::U16(vec![0u16; TILE_SAMPLES].into_boxed_slice())
    }

    /// A transparent tile of `depth`.
    pub fn blank_depth(depth: Depth) -> Self {
        match depth {
            Depth::U8 => Self::blank(),
            Depth::U16 => Self::blank16(),
        }
    }

    /// A tile from `TILE_SAMPLES` 8-bit samples. `None` when the length is wrong.
    pub fn from_u8(data: Vec<u8>) -> Option<Self> {
        (data.len() == TILE_SAMPLES).then(|| Tile::U8(data.into_boxed_slice()))
    }

    /// A tile from `TILE_SAMPLES` 16-bit samples. `None` when the length is wrong.
    pub fn from_u16(data: Vec<u16>) -> Option<Self> {
        (data.len() == TILE_SAMPLES).then(|| Tile::U16(data.into_boxed_slice()))
    }

    /// The depth of this tile.
    pub fn depth(&self) -> Depth {
        match self {
            Tile::U8(_) => Depth::U8,
            Tile::U16(_) => Depth::U16,
        }
    }

    /// The raw bytes of the tile, row-major RGBA: `TILE_BYTES` for 8-bit, twice that for
    /// 16-bit (native-endian `u16`, which is little-endian on every supported target and is what
    /// the GPU upload wants).
    pub fn data(&self) -> &[u8] {
        match self {
            Tile::U8(d) => d,
            Tile::U16(d) => bytemuck::cast_slice(d),
        }
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        match self {
            Tile::U8(d) => d,
            Tile::U16(d) => bytemuck::cast_slice_mut(d),
        }
    }

    /// The samples of a 16-bit tile; `None` for an 8-bit one.
    pub fn data16(&self) -> Option<&[u16]> {
        match self {
            Tile::U16(d) => Some(d),
            Tile::U8(_) => None,
        }
    }

    pub fn data16_mut(&mut self) -> Option<&mut [u16]> {
        match self {
            Tile::U16(d) => Some(d),
            Tile::U8(_) => None,
        }
    }

    /// Bytes this tile occupies.
    pub fn byte_len(&self) -> usize {
        self.depth().tile_bytes()
    }

    /// Is every sample zero?
    pub fn is_blank(&self) -> bool {
        match self {
            Tile::U8(d) => d.iter().all(|&b| b == 0),
            Tile::U16(d) => d.iter().all(|&b| b == 0),
        }
    }

    /// This tile at another depth. 8 to 16 is exact; 16 to 8 rounds, or dithers when
    /// `dither` is given the tile's canvas origin `(x, y)`.
    pub fn converted(&self, to: Depth, dither: Option<(usize, usize)>) -> Tile {
        match (self, to) {
            (Tile::U8(d), Depth::U16) => Tile::U16(d.iter().map(|&v| crate::pixel::up8(v)).collect()),
            (Tile::U16(d), Depth::U8) => match dither {
                None => Tile::U8(d.iter().map(|&v| crate::pixel::down16(v)).collect()),
                Some((ox, oy)) => {
                    let mut out = vec![0u8; TILE_SAMPLES];
                    for (i, (s, o)) in d.as_chunks::<4>().0.iter().zip(out.as_chunks_mut::<4>().0.iter_mut()).enumerate() {
                        let t = dither_threshold(ox + i % TILE, oy + i / TILE);
                        for k in 0..4 {
                            o[k] = down_dithered(s[k], t);
                        }
                    }
                    Tile::U8(out.into_boxed_slice())
                }
            },
            _ => self.clone(),
        }
    }
}

/// One tile slot of a layer, as stored by undo steps: its tile coordinates and the tile
/// (`None` = transparent). Holding one keeps the tile alive and shared, not copied.
#[derive(Clone, Debug)]
pub struct TileSlot {
    pub tx: usize,
    pub ty: usize,
    pub tile: Option<Arc<Tile>>,
}

/// One slice of a row inside a single tile: see [`Layer::row_span`].
///
/// A span of an 8-bit layer fills `data`, a span of a 16-bit layer fills `data16`, and a span
/// over a transparent tile has both `None`.
#[derive(Clone, Copy, Debug)]
pub struct RowSpan<'a> {
    /// Pixels in the span (always at least 1).
    pub len: usize,
    /// `len * 4` bytes of premultiplied RGBA, for an 8-bit layer with an allocated tile.
    pub data: Option<&'a [u8]>,
    /// `len * 4` premultiplied RGBA16 samples, for a 16-bit layer with an allocated tile.
    pub data16: Option<&'a [u16]>,
}

impl RowSpan<'_> {
    /// Is the whole span transparent (the tile is not allocated)?
    pub fn is_transparent(&self) -> bool {
        self.data.is_none() && self.data16.is_none()
    }

    /// The span as `[f64; 4]` premultiplied values in 0..1 at either depth.
    pub fn pixel_unit(&self, i: usize) -> [f64; 4] {
        if let Some(d) = self.data {
            return [0, 1, 2, 3].map(|k| d[i * 4 + k].to_unit());
        }
        if let Some(d) = self.data16 {
            return [0, 1, 2, 3].map(|k| d[i * 4 + k].to_unit());
        }
        [0.0; 4]
    }
}

/// Iterator of `(x, span)` over one row, split at tile edges: see [`Layer::row_spans`].
pub struct RowSpans<'a> {
    layer: &'a Layer,
    y: usize,
    x: usize,
    x_end: usize,
}

impl<'a> Iterator for RowSpans<'a> {
    type Item = (usize, RowSpan<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.x >= self.x_end {
            return None;
        }
        let span = self.layer.row_span(self.x, self.y, self.x_end);
        let x = self.x;
        self.x += span.len;
        Some((x, span))
    }
}

/// How a stroke is applied to a layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintOptions {
    /// Stroke colour, RGB in 0..1.
    pub colour: [f64; 3],
    /// Erase instead of paint.
    pub erase: bool,
    /// Paint only over existing pixels and keep the layer alpha.
    pub alpha_lock: bool,
}

impl PaintOptions {
    /// Normal painting with `colour`.
    pub fn new(colour: [f64; 3]) -> Self {
        Self {
            colour,
            erase: false,
            alpha_lock: false,
        }
    }
}

/// One raster layer of premultiplied RGBA pixels (8 or 16 bits per channel) in copy-on-write
/// tiles.
///
/// Cloning is cheap (see the module docs). Equality compares pixels: a missing tile equals
/// a tile of zeros.
#[derive(Clone, Debug)]
pub struct Layer {
    pub width: usize,
    pub height: usize,
    depth: Depth,
    tiles_x: usize,
    tiles_y: usize,
    /// `tiles_x * tiles_y` slots, row-major.
    tiles: Vec<Option<Arc<Tile>>>,
    pub visible: bool,
    /// Layer opacity in 0..1, applied by [`compose::flatten`].
    pub opacity: f64,
}

impl PartialEq for Layer {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.depth == other.depth
            && self.visible == other.visible
            && self.opacity == other.opacity
            && (0..self.tiles_y).all(|ty| (0..self.tiles_x).all(|tx| self.tile_eq(other, tx, ty)))
    }
}

impl Layer {
    /// Fully transparent 8-bit layer. Allocates no pixel memory.
    pub fn new(width: usize, height: usize) -> Self {
        Self::new_depth(width, height, Depth::U8)
    }

    /// Fully transparent layer of `depth`. Allocates no pixel memory.
    pub fn new_depth(width: usize, height: usize, depth: Depth) -> Self {
        let tiles_x = width.div_ceil(TILE);
        let tiles_y = height.div_ceil(TILE);
        Self {
            width,
            height,
            depth,
            tiles_x,
            tiles_y,
            tiles: vec![None; tiles_x * tiles_y],
            visible: true,
            opacity: 1.0,
        }
    }

    /// Bits per channel of this layer.
    pub fn depth(&self) -> Depth {
        self.depth
    }

    /// A layer from a flat `width * height * 4` premultiplied RGBA8 buffer (row-major).
    /// Tiles that are entirely transparent are not allocated. A buffer of the wrong length
    /// gives a blank layer.
    pub fn from_rgba(width: usize, height: usize, rgba: impl AsRef<[u8]>) -> Self {
        let mut layer = Self::new(width, height);
        let rgba = rgba.as_ref();
        if rgba.len() == width * height * 4 {
            layer.write_rect(0, 0, width, height, rgba);
        }
        layer
    }

    /// A 16-bit layer from a flat `width * height * 4` premultiplied RGBA16 buffer (row-major).
    /// A buffer of the wrong length gives a blank 16-bit layer.
    pub fn from_rgba16(width: usize, height: usize, rgba: impl AsRef<[u16]>) -> Self {
        let mut layer = Self::new_depth(width, height, Depth::U16);
        let rgba = rgba.as_ref();
        if rgba.len() == width * height * 4 {
            layer.write_rect16(0, 0, width, height, rgba);
        }
        layer
    }

    /// The whole layer as a flat `width * height * 4` buffer (row-major), rounded to 8 bits.
    pub fn to_rgba(&self) -> Vec<u8> {
        self.read_rect(0, 0, self.width, self.height)
    }

    /// The whole layer as a flat `width * height * 4` RGBA16 buffer (exact for a 16-bit layer,
    /// widened for an 8-bit one).
    pub fn to_rgba16(&self) -> Vec<u16> {
        self.read_rect16(0, 0, self.width, self.height)
    }

    /// This layer at `to` bits per channel, tile by tile (shared nothing, blank tiles stay
    /// unallocated). 8 to 16 is exact; 16 to 8 rounds, or dithers when `dither` is set.
    pub fn converted(&self, to: Depth, dither: bool) -> Layer {
        if to == self.depth {
            return self.clone();
        }
        let mut out = Layer::new_depth(self.width, self.height, to);
        out.visible = self.visible;
        out.opacity = self.opacity;
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                if let Some(t) = self.tile(tx, ty) {
                    let conv = t.converted(to, dither.then_some((tx * TILE, ty * TILE)));
                    if !conv.is_blank() {
                        out.set_tile(tx, ty, Some(Arc::new(conv)));
                    }
                }
            }
        }
        out
    }

    // ----- geometry of the tile grid -----

    /// Tile columns.
    pub fn tiles_x(&self) -> usize {
        self.tiles_x
    }

    /// Tile rows.
    pub fn tiles_y(&self) -> usize {
        self.tiles_y
    }

    /// Canvas rectangle `(x, y, w, h)` of tile `(tx, ty)`, clipped to the layer.
    pub fn tile_rect(&self, tx: usize, ty: usize) -> (usize, usize, usize, usize) {
        let (x, y) = (tx * TILE, ty * TILE);
        (
            x,
            y,
            TILE.min(self.width.saturating_sub(x)),
            TILE.min(self.height.saturating_sub(y)),
        )
    }

    /// Inclusive tile range `(tx0, ty0, tx1, ty1)` touched by the pixel rectangle
    /// `x..x + w, y..y + h` after clipping it to the layer. `None` when nothing is left.
    pub fn tile_range(&self, x: usize, y: usize, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        if x >= x1 || y >= y1 {
            return None;
        }
        Some((x / TILE, y / TILE, (x1 - 1) / TILE, (y1 - 1) / TILE))
    }

    fn slot(&self, tx: usize, ty: usize) -> usize {
        ty * self.tiles_x + tx
    }

    // ----- tiles -----

    /// The tile at `(tx, ty)`; `None` when it is transparent (or outside the grid).
    pub fn tile(&self, tx: usize, ty: usize) -> Option<&Arc<Tile>> {
        if tx >= self.tiles_x || ty >= self.tiles_y {
            return None;
        }
        self.tiles[self.slot(tx, ty)].as_ref()
    }

    /// Is tile `(tx, ty)` unallocated (all transparent)?
    pub fn is_empty_tile(&self, tx: usize, ty: usize) -> bool {
        self.tile(tx, ty).is_none()
    }

    /// Replace the tile at `(tx, ty)`. A tile of the wrong size or depth is ignored.
    pub fn set_tile(&mut self, tx: usize, ty: usize, tile: Option<Arc<Tile>>) {
        if tx < self.tiles_x && ty < self.tiles_y && tile.as_ref().is_none_or(|t| t.depth() == self.depth) {
            let i = self.slot(tx, ty);
            self.tiles[i] = tile;
        }
    }

    /// Mutable access to a tile, allocating it if it was transparent and copying it first if
    /// it is shared (copy on write). Panics if `(tx, ty)` is outside the grid.
    pub fn tile_mut(&mut self, tx: usize, ty: usize) -> &mut Tile {
        let i = self.slot(tx, ty);
        let depth = self.depth;
        Arc::make_mut(self.tiles[i].get_or_insert_with(|| Arc::new(Tile::blank_depth(depth))))
    }

    /// Number of allocated tiles.
    pub fn tile_count(&self) -> usize {
        self.tiles.iter().flatten().count()
    }

    /// Bytes of pixel data this layer references (allocated tiles, shared or not).
    pub fn tile_bytes(&self) -> usize {
        self.tile_count() * self.depth.tile_bytes()
    }

    /// Is the whole layer transparent? True without scanning when no tile is allocated.
    pub fn is_blank(&self) -> bool {
        self.tiles.iter().flatten().all(|t| t.is_blank())
    }

    /// Drop allocated tiles that hold only zeros. Returns how many were dropped.
    pub fn compact(&mut self) -> usize {
        self.compact_range(0, 0, self.tiles_x, self.tiles_y)
    }

    /// [`Layer::compact`] limited to the tiles touched by a pixel rectangle.
    pub fn compact_rect(&mut self, x: usize, y: usize, w: usize, h: usize) -> usize {
        match self.tile_range(x, y, w, h) {
            Some((tx0, ty0, tx1, ty1)) => self.compact_range(tx0, ty0, tx1 + 1, ty1 + 1),
            None => 0,
        }
    }

    fn compact_range(&mut self, tx0: usize, ty0: usize, tx1: usize, ty1: usize) -> usize {
        let mut dropped = 0;
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let i = self.slot(tx, ty);
                if self.tiles[i].as_ref().is_some_and(|t| t.is_blank()) {
                    self.tiles[i] = None;
                    dropped += 1;
                }
            }
        }
        dropped
    }

    /// Make the layer transparent again, freeing every tile.
    pub fn clear(&mut self) {
        self.tiles.iter_mut().for_each(|t| *t = None);
    }

    /// Do both layers hold the same pixels in tile `(tx, ty)`? Shared tiles compare by
    /// pointer, so this is cheap for layers that descend from one another.
    pub fn tile_eq(&self, other: &Layer, tx: usize, ty: usize) -> bool {
        match (self.tile(tx, ty), other.tile(tx, ty)) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (Some(t), None) | (None, Some(t)) => t.is_blank(),
        }
    }

    /// Do both layers share the very same allocation in tile `(tx, ty)` (or both lack it)?
    pub fn tile_shared(&self, other: &Layer, tx: usize, ty: usize) -> bool {
        match (self.tile(tx, ty), other.tile(tx, ty)) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Tiles inside the pixel rectangle whose pixels differ from `other`'s (same size).
    /// Tiles that are shared are skipped without comparing pixels.
    pub fn changed_tiles(&self, other: &Layer, x: usize, y: usize, w: usize, h: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        if self.tiles_x != other.tiles_x || self.tiles_y != other.tiles_y {
            return out;
        }
        if let Some((tx0, ty0, tx1, ty1)) = self.tile_range(x, y, w, h) {
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    if !self.tile_eq(other, tx, ty) {
                        out.push((tx, ty));
                    }
                }
            }
        }
        out
    }

    /// Snapshot the given tile slots (cheap: reference counts only).
    pub fn snapshot_tiles(&self, coords: &[(usize, usize)]) -> Vec<TileSlot> {
        coords
            .iter()
            .map(|&(tx, ty)| TileSlot {
                tx,
                ty,
                tile: self.tile(tx, ty).cloned(),
            })
            .collect()
    }

    /// Put snapshotted tile slots back.
    pub fn restore_tiles(&mut self, slots: &[TileSlot]) {
        for s in slots {
            self.set_tile(s.tx, s.ty, s.tile.clone());
        }
    }

    /// Take every tile slot of `other` (same size and depth), sharing its tiles. Mismatches
    /// are ignored.
    pub fn copy_tiles_from(&mut self, other: &Layer) {
        if self.tiles_x == other.tiles_x && self.tiles_y == other.tiles_y && self.depth == other.depth {
            self.tiles.clone_from(&other.tiles);
        }
    }

    /// Take the tile slots touched by a pixel rectangle from `other` (same size and depth).
    pub fn copy_tiles_in_rect_from(&mut self, other: &Layer, x: usize, y: usize, w: usize, h: usize) {
        if self.tiles_x != other.tiles_x || self.tiles_y != other.tiles_y || self.depth != other.depth {
            return;
        }
        if let Some((tx0, ty0, tx1, ty1)) = self.tile_range(x, y, w, h) {
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    let i = self.slot(tx, ty);
                    self.tiles[i].clone_from(&other.tiles[i]);
                }
            }
        }
    }

    /// Share tiles with `other` wherever they hold the same pixels, so a derived layer (a
    /// filter result, say) only costs memory for what actually changed. Frees the duplicates.
    pub fn share_unchanged_with(&mut self, other: &Layer) {
        self.share_unchanged_in(other, 0, 0, self.tiles_x, self.tiles_y);
    }

    /// [`Layer::share_unchanged_with`] limited to the tiles touched by a pixel rectangle. A
    /// stroke uses it to give back the tiles it copied but then left as they were.
    pub fn share_unchanged_in_rect(&mut self, other: &Layer, x: usize, y: usize, w: usize, h: usize) {
        if let Some((tx0, ty0, tx1, ty1)) = self.tile_range(x, y, w, h) {
            self.share_unchanged_in(other, tx0, ty0, tx1 + 1, ty1 + 1);
        }
    }

    fn share_unchanged_in(&mut self, other: &Layer, tx0: usize, ty0: usize, tx1: usize, ty1: usize) {
        if self.tiles_x != other.tiles_x || self.tiles_y != other.tiles_y || self.depth != other.depth {
            return;
        }
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                let i = self.slot(tx, ty);
                match (&self.tiles[i], &other.tiles[i]) {
                    (Some(a), Some(b)) if !Arc::ptr_eq(a, b) && a == b => self.tiles[i].clone_from(&other.tiles[i]),
                    (Some(a), None) if a.is_blank() => self.tiles[i] = None,
                    _ => {}
                }
            }
        }
    }

    /// Apply `f` to every pixel of every allocated tile. Transparent tiles stay transparent,
    /// so `f` should map a transparent pixel to a transparent pixel. Padding beyond the layer
    /// is skipped. On a 16-bit layer the pixel goes through 8 bits; use
    /// [`Layer::map_allocated16`] to keep the precision.
    pub fn map_allocated(&mut self, mut f: impl FnMut([u8; 4]) -> [u8; 4]) {
        match self.depth {
            Depth::U8 => self.map_allocated_impl::<u8>(&mut f),
            Depth::U16 => self.map_allocated_impl::<u16>(&mut |p| px_up(f(px_down(p)))),
        }
    }

    /// [`Layer::map_allocated`] with 16-bit pixels (an 8-bit layer is widened and narrowed).
    pub fn map_allocated16(&mut self, mut f: impl FnMut([u16; 4]) -> [u16; 4]) {
        match self.depth {
            Depth::U16 => self.map_allocated_impl::<u16>(&mut f),
            Depth::U8 => self.map_allocated_impl::<u8>(&mut |p| px_down(f(px_up(p)))),
        }
    }

    fn map_allocated_impl<T: Chan>(&mut self, f: &mut dyn FnMut([T; 4]) -> [T; 4]) {
        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                if self.tile(tx, ty).is_none() {
                    continue;
                }
                let (_, _, tw, th) = self.tile_rect(tx, ty);
                let tile = T::slice_mut(self.tile_mut(tx, ty));
                for row in 0..th {
                    let o = row * TILE * 4;
                    for px in tile[o..o + tw * 4].as_chunks_mut::<4>().0.iter_mut() {
                        *px = f(*px);
                    }
                }
            }
        }
    }

    // ----- single pixels -----

    /// Premultiplied RGBA8 at `(x, y)`; transparent outside the layer. A 16-bit layer rounds.
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        match self.depth {
            Depth::U8 => self.pixel_t::<u8>(x, y),
            Depth::U16 => px_down(self.pixel_t::<u16>(x, y)),
        }
    }

    /// Premultiplied RGBA16 at `(x, y)` (exact for a 16-bit layer, widened for an 8-bit one).
    pub fn pixel16(&self, x: usize, y: usize) -> [u16; 4] {
        match self.depth {
            Depth::U16 => self.pixel_t::<u16>(x, y),
            Depth::U8 => px_up(self.pixel_t::<u8>(x, y)),
        }
    }

    /// The pixel as 0..1 premultiplied values at either depth.
    pub fn pixel_unit(&self, x: usize, y: usize) -> [f64; 4] {
        match self.depth {
            Depth::U8 => self.pixel_t::<u8>(x, y).map(Chan::to_unit),
            Depth::U16 => self.pixel_t::<u16>(x, y).map(Chan::to_unit),
        }
    }

    fn pixel_t<T: Chan>(&self, x: usize, y: usize) -> [T; 4] {
        if x >= self.width || y >= self.height {
            return [T::default(); 4];
        }
        match self.tiles[self.slot(x / TILE, y / TILE)].as_ref() {
            Some(t) => {
                let i = ((y % TILE) * TILE + x % TILE) * 4;
                let d = T::slice(t);
                [d[i], d[i + 1], d[i + 2], d[i + 3]]
            }
            None => [T::default(); 4],
        }
    }

    /// Set the pixel at `(x, y)`. Outside the layer, and transparent writes onto a transparent
    /// tile, do nothing. A 16-bit layer widens the value.
    pub fn set_pixel(&mut self, x: usize, y: usize, px: [u8; 4]) {
        match self.depth {
            Depth::U8 => self.set_pixel_t::<u8>(x, y, px),
            Depth::U16 => self.set_pixel_t::<u16>(x, y, px_up(px)),
        }
    }

    /// [`Layer::set_pixel`] with a 16-bit pixel (an 8-bit layer rounds it).
    pub fn set_pixel16(&mut self, x: usize, y: usize, px: [u16; 4]) {
        match self.depth {
            Depth::U16 => self.set_pixel_t::<u16>(x, y, px),
            Depth::U8 => self.set_pixel_t::<u8>(x, y, px_down(px)),
        }
    }

    fn set_pixel_t<T: Chan>(&mut self, x: usize, y: usize, px: [T; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let (tx, ty) = (x / TILE, y / TILE);
        if px == [T::default(); 4] && self.is_empty_tile(tx, ty) {
            return;
        }
        let i = ((y % TILE) * TILE + x % TILE) * 4;
        T::slice_mut(self.tile_mut(tx, ty))[i..i + 4].copy_from_slice(&px);
    }

    // ----- rows and spans (hot loops) -----

    /// [`Layer::row_span`] for channel type `T`: the span length and its samples. The samples
    /// are `None` when the tile is transparent or the layer has the other depth.
    pub fn row_span_t<T: Chan>(&self, x: usize, y: usize, x_end: usize) -> (usize, Option<&[T]>) {
        let len = (TILE - x % TILE).min(x_end - x);
        if T::DEPTH != self.depth {
            return (len, None);
        }
        let i = ((y % TILE) * TILE + x % TILE) * 4;
        let data = self.tiles[self.slot(x / TILE, y / TILE)].as_ref().map(|t| &T::slice(t)[i..i + len * 4]);
        (len, data)
    }

    /// The slice of row `y` starting at `x` and ending at the next tile edge or at `x_end`
    /// (exclusive), whichever comes first. `x < x_end <= width` and `y < height` are required.
    pub fn row_span(&self, x: usize, y: usize, x_end: usize) -> RowSpan<'_> {
        let len = (TILE - x % TILE).min(x_end - x);
        let i = ((y % TILE) * TILE + x % TILE) * 4;
        let mut span = RowSpan { len, data: None, data16: None };
        if let Some(t) = self.tiles[self.slot(x / TILE, y / TILE)].as_ref() {
            match &**t {
                Tile::U8(d) => span.data = Some(&d[i..i + len * 4]),
                Tile::U16(d) => span.data16 = Some(&d[i..i + len * 4]),
            }
        }
        span
    }

    /// Row `y` from `x0` to `x1` (exclusive) as `(x, span)` pieces, split at tile edges.
    /// Empty when the row or range is outside the layer.
    pub fn row_spans(&self, y: usize, x0: usize, x1: usize) -> RowSpans<'_> {
        let x_end = if y < self.height { x1.min(self.width) } else { 0 };
        RowSpans {
            layer: self,
            y,
            x: x0,
            x_end,
        }
    }

    // ----- rectangles -----

    /// Premultiplied RGBA8 of the rectangle `x..x + w, y..y + h` as `w * h * 4` bytes
    /// (row-major). Parts outside the layer read as transparent. A 16-bit layer rounds.
    pub fn read_rect(&self, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
        match self.depth {
            Depth::U8 => {
                let mut out = vec![0u8; w * h * 4];
                self.read_rect_t::<u8>(x, y, w, h, &mut out);
                out
            }
            Depth::U16 => crate::pixel::rgba16_to_8(&self.read_rect16(x, y, w, h)),
        }
    }

    /// [`Layer::read_rect`] into an existing `w * h * 4` buffer, which must be zeroed where
    /// the layer is transparent (a fresh `vec![0; ..]` is).
    pub fn read_rect_into(&self, x: usize, y: usize, w: usize, h: usize, out: &mut [u8]) {
        match self.depth {
            Depth::U8 => self.read_rect_t::<u8>(x, y, w, h, out),
            Depth::U16 => {
                let wide = self.read_rect16(x, y, w, h);
                if out.len() >= w * h * 4 {
                    for (o, v) in out.iter_mut().zip(&wide) {
                        *o = crate::pixel::down16(*v);
                    }
                }
            }
        }
    }

    /// Premultiplied RGBA16 of a rectangle (see [`Layer::read_rect`]).
    pub fn read_rect16(&self, x: usize, y: usize, w: usize, h: usize) -> Vec<u16> {
        let mut out = vec![0u16; w * h * 4];
        self.read_rect16_into(x, y, w, h, &mut out);
        out
    }

    /// [`Layer::read_rect16`] into a zeroed `w * h * 4` buffer.
    pub fn read_rect16_into(&self, x: usize, y: usize, w: usize, h: usize, out: &mut [u16]) {
        match self.depth {
            Depth::U16 => self.read_rect_t::<u16>(x, y, w, h, out),
            Depth::U8 => {
                let mut narrow = vec![0u8; w * h * 4];
                self.read_rect_t::<u8>(x, y, w, h, &mut narrow);
                if out.len() >= w * h * 4 {
                    for (o, v) in out.iter_mut().zip(&narrow) {
                        *o = crate::pixel::up8(*v);
                    }
                }
            }
        }
    }

    fn read_rect_t<T: Chan>(&self, x: usize, y: usize, w: usize, h: usize, out: &mut [T]) {
        debug_assert_eq!(T::DEPTH, self.depth);
        let x1 = x.saturating_add(w).min(self.width);
        let y1 = y.saturating_add(h).min(self.height);
        if x >= x1 || y >= y1 || out.len() < w * h * 4 {
            return;
        }
        for row in y..y1 {
            let mut sx = x;
            while sx < x1 {
                let len = (TILE - sx % TILE).min(x1 - sx);
                if let Some(t) = self.tiles[self.slot(sx / TILE, row / TILE)].as_ref() {
                    let i = ((row % TILE) * TILE + sx % TILE) * 4;
                    let o = ((row - y) * w + (sx - x)) * 4;
                    out[o..o + len * 4].copy_from_slice(&T::slice(t)[i..i + len * 4]);
                }
                sx += len;
            }
        }
    }

    /// Write `data` (`w * h * 4` bytes, as returned by [`Layer::read_rect`]) into the
    /// rectangle at `(x, y)`. Parts outside the layer are dropped. A tile that is transparent
    /// and would only receive zeros stays unallocated. A 16-bit layer widens the data.
    pub fn write_rect(&mut self, x: usize, y: usize, w: usize, h: usize, data: &[u8]) {
        match self.depth {
            Depth::U8 => self.write_rect_t::<u8>(x, y, w, h, data),
            Depth::U16 => {
                if data.len() >= w * h * 4 {
                    self.write_rect_t::<u16>(x, y, w, h, &crate::pixel::rgba8_to_16(&data[..w * h * 4]));
                }
            }
        }
    }

    /// [`Layer::write_rect`] with RGBA16 data (an 8-bit layer rounds it).
    pub fn write_rect16(&mut self, x: usize, y: usize, w: usize, h: usize, data: &[u16]) {
        match self.depth {
            Depth::U16 => self.write_rect_t::<u16>(x, y, w, h, data),
            Depth::U8 => {
                if data.len() >= w * h * 4 {
                    self.write_rect_t::<u8>(x, y, w, h, &crate::pixel::rgba16_to_8(&data[..w * h * 4]));
                }
            }
        }
    }

    fn write_rect_t<T: Chan>(&mut self, x: usize, y: usize, w: usize, h: usize, data: &[T]) {
        debug_assert_eq!(T::DEPTH, self.depth);
        if data.len() < w * h * 4 {
            return;
        }
        let Some((tx0, ty0, tx1, ty1)) = self.tile_range(x, y, w, h) else {
            return;
        };
        let (x1, y1) = (x.saturating_add(w).min(self.width), y.saturating_add(h).min(self.height));
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let (px0, py0) = (x.max(tx * TILE), y.max(ty * TILE));
                let (px1, py1) = (x1.min((tx + 1) * TILE), y1.min((ty + 1) * TILE));
                let src = |row: usize| {
                    let o = ((row - y) * w + (px0 - x)) * 4;
                    &data[o..o + (px1 - px0) * 4]
                };
                if self.is_empty_tile(tx, ty) && (py0..py1).all(|r| src(r).iter().all(|b| b.is_zero())) {
                    continue;
                }
                let tile = T::slice_mut(self.tile_mut(tx, ty));
                for row in py0..py1 {
                    let o = ((row % TILE) * TILE + px0 % TILE) * 4;
                    let s = src(row);
                    tile[o..o + s.len()].copy_from_slice(s);
                }
            }
        }
    }

    /// Run `f(x, y, old) -> new` over every pixel of the rectangle `x0..x1, y0..y1`
    /// (exclusive ends, clipped to the layer), one tile at a time.
    ///
    /// `old` is read from `source` when given (a snapshot of this layer from before a stroke,
    /// say) and from this layer otherwise; `new` is always written to this layer. A tile that
    /// was transparent and stays transparent is not allocated. Tiles shared with another
    /// layer or an undo step are copied before the first write. On a 16-bit layer the pixel
    /// goes through 8 bits; use [`Layer::update_rect16`] or [`Layer::update_rect_unit`].
    #[inline(always)]
    pub fn update_rect(
        &mut self,
        source: Option<&Layer>,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        mut f: impl FnMut(usize, usize, [u8; 4]) -> [u8; 4],
    ) {
        match self.depth {
            Depth::U8 => self.update_rect_impl::<u8>(source, x0, y0, x1, y1, f),
            Depth::U16 => self.update_rect_impl::<u16>(source, x0, y0, x1, y1, |x, y, o| px_up(f(x, y, px_down(o)))),
        }
    }

    /// [`Layer::update_rect`] with 16-bit pixels (an 8-bit layer is widened and narrowed).
    #[inline(always)]
    pub fn update_rect16(
        &mut self,
        source: Option<&Layer>,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        mut f: impl FnMut(usize, usize, [u16; 4]) -> [u16; 4],
    ) {
        match self.depth {
            Depth::U16 => self.update_rect_impl::<u16>(source, x0, y0, x1, y1, f),
            Depth::U8 => self.update_rect_impl::<u8>(source, x0, y0, x1, y1, |x, y, o| px_down(f(x, y, px_up(o)))),
        }
    }

    /// [`Layer::update_rect`] where the closure sees premultiplied `f64` values in 0..1 and
    /// returns the same. Values are rounded and clamped to the layer's depth on the way back, so
    /// the maths runs at full precision at either depth. This is how painting, wet mixing and
    /// smudging write pixels.
    #[inline(always)]
    pub fn update_rect_unit(
        &mut self,
        source: Option<&Layer>,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        mut f: impl FnMut(usize, usize, [f64; 4]) -> [f64; 4],
    ) {
        match self.depth {
            Depth::U8 => self.update_rect_impl::<u8>(source, x0, y0, x1, y1, |x, y, o| f(x, y, o.map(Chan::to_unit)).map(<u8 as Chan>::from_unit)),
            Depth::U16 => self.update_rect_impl::<u16>(source, x0, y0, x1, y1, |x, y, o| f(x, y, o.map(Chan::to_unit)).map(<u16 as Chan>::from_unit)),
        }
    }

    #[inline(always)]
    fn update_rect_impl<T: Chan>(
        &mut self,
        source: Option<&Layer>,
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        mut f: impl FnMut(usize, usize, [T; 4]) -> [T; 4],
    ) {
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let Some((tx0, ty0, tx1, ty1)) = self.tile_range(x0, y0, w, h) else {
            return;
        };
        let (x1, y1) = (x1.min(self.width), y1.min(self.height));
        let source = source.filter(|s| s.depth == self.depth);
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                let (px0, py0) = (x0.max(tx * TILE), y0.max(ty * TILE));
                let (px1, py1) = (x1.min((tx + 1) * TILE), y1.min((ty + 1) * TILE));
                let slot = self.slot(tx, ty);
                let existed = self.tiles[slot].is_some();
                let mut arc = self.tiles[slot].take().unwrap_or_else(|| Arc::new(T::blank_tile()));
                let tile = T::slice_mut(Arc::make_mut(&mut arc));
                let mut any = false;
                // The source is hoisted out of the pixel loop: one slice for the whole tile.
                let src_data: Option<&[T]> = source.map(|s| s.tile(tx, ty).map_or(zeros::<T>(), |t| T::slice(t)));
                macro_rules! run {
                    ($read:expr) => {
                        for y in py0..py1 {
                            let row = ((y % TILE) * TILE + px0 % TILE) * 4;
                            for (k, x) in (px0..px1).enumerate() {
                                let i = row + k * 4;
                                let old: [T; 4] = $read(i, &*tile);
                                let new = f(x, y, old);
                                any |= !(new[0].is_zero() & new[1].is_zero() & new[2].is_zero() & new[3].is_zero());
                                tile[i..i + 4].copy_from_slice(&new);
                            }
                        }
                    };
                }
                match src_data {
                    Some(s) => run!(|i: usize, _d: &[T]| [s[i], s[i + 1], s[i + 2], s[i + 3]]),
                    None => run!(|i: usize, d: &[T]| [d[i], d[i + 1], d[i + 2], d[i + 3]]),
                }
                if existed || any {
                    self.tiles[slot] = Some(arc);
                }
            }
        }
    }

    /// Mirror the layer across its centre.
    pub fn flip(&mut self, horizontal: bool) {
        if self.tile_count() == 0 {
            return;
        }
        let (visible, opacity) = (self.visible, self.opacity);
        match self.depth {
            Depth::U8 => {
                let mut flat = self.to_rgba();
                crate::document::flip_buffer(&mut flat, self.width, self.height, 4, horizontal);
                *self = Layer::from_rgba(self.width, self.height, flat);
            }
            Depth::U16 => {
                let mut flat = self.to_rgba16();
                crate::document::flip_buffer(&mut flat, self.width, self.height, 4, horizontal);
                *self = Layer::from_rgba16(self.width, self.height, flat);
            }
        }
        self.visible = visible;
        self.opacity = opacity;
    }

    // ----- painting -----

    /// Paint a stroke with `colour`. Call [`Stroke::finish`] first so the end taper is included.
    pub fn paint_stroke(&mut self, stroke: &Stroke, colour: [f64; 3]) {
        self.paint_stroke_with(stroke, PaintOptions::new(colour));
    }

    /// Paint, erase or alpha-lock a stroke onto this layer.
    ///
    /// Only pixels inside the dab bounds are composited. Pixels with zero
    /// coverage would be unchanged anyway. Only the touched tiles are copied or allocated,
    /// and tiles an erase leaves empty are freed.
    pub fn paint_stroke_with(&mut self, stroke: &Stroke, opts: PaintOptions) {
        let raster = DabRaster::new(stroke.brush());
        self.paint_stroke_with_raster(stroke, opts, &raster);
    }

    /// [`Layer::paint_stroke_with`] using a prepared [`DabRaster`], so a brush with its own
    /// tip and grain textures (`DabRaster::with_textures`) paints through the same pipeline.
    /// The compositing maths runs in `f64` and is rounded once, to the layer's depth.
    pub fn paint_stroke_with_raster(&mut self, stroke: &Stroke, opts: PaintOptions, raster: &DabRaster) {
        if crate::wet::is_wet(stroke.brush(), opts.erase) {
            crate::wet::paint_stroke(self, stroke, opts, raster);
            return;
        }
        let dabs = stroke.dabs();
        let Some((x0, y0, x1, y1)) = dab_pixel_rect(dabs, self.width, self.height) else {
            return;
        };
        let brush = stroke.brush();
        let mut cov = CoverageBuffer::new(x0, y0, (x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);
        cov.splat(dabs, raster);

        let (ux0, uy0, ux1, uy1) = (x0 as usize, y0 as usize, x1 as usize + 1, y1 as usize + 1);
        let colour = crate::geometry::stroke_colour(brush, opts.colour, stroke.meta());
        let f = |px: usize, py: usize, old: [f64; 4]| -> [f64; 4] {
            let (pxi, pyi) = (px as i64, py as i64);
            let o = PixelOpts {
                layer: old,
                coverage: cov.get(pxi, pyi),
                colour,
                wet_edges: brush.wet_edges,
                threshold: brush.alpha_threshold,
                burnt: brush.burnt_edges,
                grain: raster.texturized_tone(pxi, pyi),
                erase: opts.erase,
                alpha_lock: opts.alpha_lock,
                ..PixelOpts::default()
            };
            compose::stroke_composite_pixel(&o)
        };
        // The 8-bit loop keeps its own closure, so it is as tight as before 16-bit layers existed.
        if self.depth == Depth::U16 {
            self.update_rect_unit(None, ux0, uy0, ux1, uy1, f);
        } else {
            self.update_rect(None, ux0, uy0, ux1, uy1, |px, py, old| {
                let out = f(px, py, compose::premul_from_u8(&old));
                [compose::to_u8(out[0]), compose::to_u8(out[1]), compose::to_u8(out[2]), compose::to_u8(out[3])]
            });
        }
        if opts.erase {
            self.compact_rect(ux0, uy0, ux1 - ux0, uy1 - uy0);
        }
    }
}

/// A shared all-zero tile of the channel type.
#[inline(always)]
fn zeros<T: Chan>() -> &'static [T] {
    T::zeros()
}

/// A document: a fixed-size stack of layers.
#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Bottom to top.
    pub layers: Vec<Layer>,
}

impl Canvas {
    /// Empty canvas with no layers.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            layers: Vec::new(),
        }
    }

    /// Add a transparent layer on top and return its index.
    pub fn add_layer(&mut self) -> usize {
        self.layers.push(Layer::new(self.width, self.height));
        self.layers.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic, never-all-zero pixel for `(x, y)`.
    fn px(x: usize, y: usize) -> [u8; 4] {
        [(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 1 + ((x * 3 + y * 7) % 250) as u8]
    }

    fn filled(w: usize, h: usize) -> Layer {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                buf[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&px(x, y));
            }
        }
        Layer::from_rgba(w, h, buf)
    }

    #[test]
    fn empty_layer_allocates_no_tiles() {
        let l = Layer::new(2048, 1536);
        assert_eq!((l.tiles_x(), l.tiles_y()), (8, 6));
        assert_eq!(l.tile_count(), 0);
        assert_eq!(l.tile_bytes(), 0);
        assert!(l.is_blank());
        assert!(l.is_empty_tile(3, 3));
        assert_eq!(l.pixel(2047, 1535), [0; 4]);
        // Reading never allocates.
        assert!(l.read_rect(100, 100, 700, 700).iter().all(|&b| b == 0));
        assert_eq!(l.tile_count(), 0);
        // Writing zeros onto an empty layer does not allocate either.
        let mut m = l.clone();
        m.write_rect(10, 10, 600, 600, &vec![0; 600 * 600 * 4]);
        m.set_pixel(5, 5, [0; 4]);
        assert_eq!(m.tile_count(), 0);
    }

    #[test]
    fn odd_canvas_sizes_round_trip_through_rgba() {
        for (w, h) in [(1, 1), (255, 257), (256, 256), (257, 1), (513, 300), (700, 530)] {
            let l = filled(w, h);
            assert_eq!(l.tiles_x(), w.div_ceil(TILE));
            assert_eq!(l.tile_count(), l.tiles_x() * l.tiles_y(), "{w}x{h}");
            let flat = l.to_rgba();
            assert_eq!(flat.len(), w * h * 4);
            assert_eq!(Layer::from_rgba(w, h, &flat), l);
            for &(x, y) in &[(0, 0), (w - 1, h - 1), (w / 2, h / 2), ((w - 1).min(255), (h - 1).min(256))] {
                assert_eq!(l.pixel(x, y), px(x, y), "{w}x{h} at {x},{y}");
                assert_eq!(&flat[(y * w + x) * 4..(y * w + x) * 4 + 4], &px(x, y));
            }
        }
        // A buffer of the wrong length gives a blank layer.
        assert_eq!(Layer::from_rgba(4, 4, vec![1; 10]).tile_count(), 0);
    }

    #[test]
    fn sparse_content_only_allocates_touched_tiles() {
        let mut buf = vec![0u8; 700 * 530 * 4];
        let i = (300 * 700 + 600) * 4; // tile (2, 1)
        buf[i + 3] = 9;
        let l = Layer::from_rgba(700, 530, buf);
        assert_eq!(l.tile_count(), 1);
        assert!(!l.is_empty_tile(2, 1));
        assert!(l.is_empty_tile(0, 0));
        assert!(!l.is_blank());
        assert_eq!(l.pixel(600, 300), [0, 0, 0, 9]);
    }

    #[test]
    fn read_and_write_rect_across_tile_boundaries() {
        let (w, h) = (700, 530);
        let mut l = Layer::new(w, h);
        // Spans 3x3 tiles: x 250..520, y 250..520.
        let (rx, ry, rw, rh) = (250, 250, 270, 270);
        let mut data = Vec::new();
        for y in ry..ry + rh {
            for x in rx..rx + rw {
                data.extend_from_slice(&px(x, y));
            }
        }
        l.write_rect(rx, ry, rw, rh, &data);
        assert_eq!(l.read_rect(rx, ry, rw, rh), data);
        assert_eq!(l.tile_count(), 9);
        assert_eq!(l.pixel(rx, ry), px(rx, ry));
        assert_eq!(l.pixel(255, 256), px(255, 256));
        assert_eq!(l.pixel(256, 255), px(256, 255));
        assert_eq!(l.pixel(rx - 1, ry), [0; 4]);
        assert_eq!(l.pixel(rx + rw, ry), [0; 4]);
        // A sub-rectangle read matches the flat buffer.
        let sub = l.read_rect(255, 255, 3, 3);
        for (k, (x, y)) in (255..258).flat_map(|y| (255..258).map(move |x| (x, y))).enumerate() {
            assert_eq!(&sub[k * 4..k * 4 + 4], &px(x, y));
        }
        // Reads and writes hanging off the canvas are clipped, not panics.
        let edge = l.read_rect(w - 2, h - 2, 5, 5);
        assert_eq!(edge.len(), 5 * 5 * 4);
        l.write_rect(w - 1, h - 1, 4, 4, &[7; 64]);
        assert_eq!(l.pixel(w - 1, h - 1), [7; 4]);
    }

    #[test]
    fn clones_share_tiles_and_writes_copy_only_the_touched_one() {
        let a = filled(600, 300);
        assert_eq!(a.tile_count(), 3 * 2);
        let mut b = a.clone();
        for ty in 0..2 {
            for tx in 0..3 {
                assert!(b.tile_shared(&a, tx, ty), "a clone shares every tile");
                assert_eq!(Arc::strong_count(a.tile(tx, ty).unwrap()), 2);
            }
        }
        b.set_pixel(300, 10, [1, 2, 3, 4]); // tile (1, 0)
        assert_eq!(a.pixel(300, 10), px(300, 10), "the original is untouched");
        assert_eq!(b.pixel(300, 10), [1, 2, 3, 4]);
        for ty in 0..2 {
            for tx in 0..3 {
                assert_eq!(b.tile_shared(&a, tx, ty), (tx, ty) != (1, 0));
            }
        }
        assert_eq!(Arc::strong_count(a.tile(1, 0).unwrap()), 1);
        assert_eq!(Arc::strong_count(a.tile(0, 0).unwrap()), 2);
        // Changing it back makes the layers equal by pixels, though not by allocation.
        b.set_pixel(300, 10, px(300, 10));
        assert_eq!(a, b);
        assert!(!b.tile_shared(&a, 1, 0));
        assert_eq!(b.changed_tiles(&a, 0, 0, 600, 300), Vec::<(usize, usize)>::new());
        b.set_pixel(0, 299, [9, 9, 9, 9]);
        assert_eq!(b.changed_tiles(&a, 0, 0, 600, 300), vec![(0, 1)]);
    }

    #[test]
    fn missing_tile_equals_a_zero_tile() {
        let mut a = Layer::new(300, 300);
        a.tile_mut(1, 1);
        assert_eq!(a.tile_count(), 1);
        assert_eq!(a, Layer::new(300, 300));
        assert!(a.is_blank());
        assert_eq!(a.compact(), 1);
        assert_eq!(a.tile_count(), 0);
        let mut b = filled(300, 300);
        b.set_pixel(1, 1, [0; 4]);
        assert_ne!(b, filled(300, 300));
    }

    #[test]
    fn row_spans_split_at_tile_edges_and_report_empty_tiles() {
        let mut l = Layer::new(600, 300);
        l.set_pixel(260, 5, [1, 1, 1, 1]); // tile (1, 0) only
        let spans: Vec<_> = l.row_spans(5, 200, 530).map(|(x, s)| (x, s.len, s.data.is_some())).collect();
        assert_eq!(spans, vec![(200, 56, false), (256, 256, true), (512, 18, false)]);
        let (x, s) = l.row_spans(5, 250, 300).nth(1).unwrap();
        assert_eq!((x, s.len), (256, 44));
        assert_eq!(&s.data.unwrap()[4 * 4..4 * 4 + 4], &[1, 1, 1, 1]);
        // Rows and ranges outside the layer give no spans.
        assert_eq!(l.row_spans(300, 0, 10).count(), 0);
        assert_eq!(l.row_spans(0, 590, 900).map(|(_, s)| s.len).sum::<usize>(), 10);
        assert_eq!(l.row_spans(0, 10, 10).count(), 0);
    }

    #[test]
    fn update_rect_matches_per_pixel_writes_and_is_copy_on_write() {
        let src = filled(600, 300);
        let mut a = src.clone();
        a.update_rect(None, 200, 100, 520, 280, |x, y, old| [old[0].wrapping_add(x as u8), old[1], old[2] ^ y as u8, old[3]]);
        for &(x, y) in &[(199, 100), (200, 100), (519, 279), (520, 279), (300, 150), (256, 256)] {
            let inside = (200..520).contains(&x) && (100..280).contains(&y);
            let o = px(x, y);
            let want = if inside { [o[0].wrapping_add(x as u8), o[1], o[2] ^ y as u8, o[3]] } else { o };
            assert_eq!(a.pixel(x, y), want, "{x},{y}");
        }
        assert_eq!(src, filled(600, 300), "the source layer is untouched");
        let mut b = src.clone();
        b.update_rect(None, 0, 0, 10, 10, |_, _, old| old);
        assert!(!b.tile_shared(&src, 0, 0), "an identity write still unshares the tile");
        assert!(b.tile_shared(&src, 1, 0) && b.tile_shared(&src, 0, 1));

        // Reading from a source layer.
        let mut c = Layer::new(600, 300);
        c.update_rect(Some(&src), 250, 250, 262, 252, |_, _, old| old);
        assert_eq!(c.pixel(256, 251), px(256, 251));
        assert_eq!(c.tile_count(), 2, "only the two touched tiles exist");
    }

    #[test]
    fn update_rect_does_not_allocate_when_nothing_is_painted() {
        let mut l = Layer::new(600, 300);
        l.update_rect(None, 0, 0, 600, 300, |_, _, old| old);
        assert_eq!(l.tile_count(), 0);
        l.update_rect(None, 100, 100, 300, 200, |x, y, _| if x == 270 && y == 150 { [0, 0, 0, 1] } else { [0; 4] });
        assert_eq!(l.tile_count(), 1);
        assert!(!l.is_empty_tile(1, 0));
    }

    #[test]
    fn snapshots_restore_and_share_unchanged_tiles() {
        let mut a = filled(600, 300);
        let before = a.clone();
        let snap = a.snapshot_tiles(&[(0, 0), (2, 1)]);
        a.set_pixel(0, 0, [0; 4]);
        a.set_pixel(599, 299, [0; 4]);
        assert_ne!(a, before);
        a.restore_tiles(&snap);
        assert_eq!(a, before);

        // A rebuilt copy shares what did not change, and frees empty tiles.
        let mut rebuilt = Layer::from_rgba(600, 300, before.to_rgba());
        assert!(!rebuilt.tile_shared(&before, 0, 0));
        rebuilt.set_pixel(5, 5, [1, 1, 1, 1]);
        rebuilt.share_unchanged_with(&before);
        assert!(rebuilt.tile_shared(&before, 1, 1));
        assert!(!rebuilt.tile_shared(&before, 0, 0));
        let mut z = Layer::from_rgba(600, 300, vec![0u8; 600 * 300 * 4]);
        z.tile_mut(0, 0);
        z.share_unchanged_with(&Layer::new(600, 300));
        assert_eq!(z.tile_count(), 0);
    }

    #[test]
    fn copy_tiles_helpers_share_without_copying() {
        let a = filled(600, 300);
        let mut b = Layer::new(600, 300);
        b.copy_tiles_in_rect_from(&a, 260, 10, 5, 5);
        assert_eq!(b.tile_count(), 1);
        assert!(b.tile_shared(&a, 1, 0));
        b.copy_tiles_from(&a);
        assert_eq!(b, a);
        assert!(b.tile_shared(&a, 2, 1));
        // A different size is ignored.
        let mut c = Layer::new(10, 10);
        c.copy_tiles_from(&a);
        assert_eq!(c.tile_count(), 0);
    }

    #[test]
    fn map_allocated_skips_empty_tiles_and_padding() {
        let mut l = Layer::new(300, 300);
        l.set_pixel(299, 299, [10, 20, 30, 40]);
        l.map_allocated(|p| [p[0], p[1], p[2], p[3] / 2]);
        assert_eq!(l.pixel(299, 299), [10, 20, 30, 20]);
        assert_eq!(l.tile_count(), 1);
    }

    #[test]
    fn flip_mirrors_and_is_an_involution() {
        let l = filled(300, 70);
        let mut m = l.clone();
        m.flip(true);
        assert_eq!(m.pixel(0, 3), px(299, 3));
        assert_eq!(m.pixel(299, 3), px(0, 3));
        m.flip(true);
        assert_eq!(m, l);
        m.flip(false);
        assert_eq!(m.pixel(5, 0), px(5, 69));
        let mut blank = Layer::new(300, 70);
        blank.flip(false);
        assert_eq!(blank.tile_count(), 0);
    }
}
