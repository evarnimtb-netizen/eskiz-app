//! Sparse tile storage shared by layers and masks.
//!
//! Tiles are 256x256 texels. A [`TileStore`] keeps its tiles in a few `texture_2d_array`
//! "pages". A page is created only when the previous pages are full, and page capacity
//! grows 4, 8, 16, 32, 64, 64, ... slices, so memory follows the number of non-empty tiles
//! (one tile of RGBA8 costs 1 MiB for the first page). Tiles that were never written are
//! not allocated and read back as zeros.

use crate::context::{GpuContext, GpuError};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Tile edge in pixels.
pub const TILE: u32 = 256;

/// Largest page, in slices.
const PAGE_MAX: u32 = 64;

/// Tile coordinate (column, row).
pub type TileCoord = (u32, u32);

/// A pixel rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Smallest rectangle containing both. An empty rectangle is ignored.
    pub fn union(self, o: Rect) -> Rect {
        if self.is_empty() {
            return o;
        }
        if o.is_empty() {
            return self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Tiles (inclusive range) touched by this rectangle, or `None` when empty.
    pub fn tile_range(&self) -> Option<(TileCoord, TileCoord)> {
        if self.is_empty() {
            return None;
        }
        Some((
            (self.x / TILE, self.y / TILE),
            ((self.x + self.w - 1) / TILE, (self.y + self.h - 1) / TILE),
        ))
    }
}

static NEXT_PAGE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileSlot {
    pub page: u32,
    pub slice: u32,
}

pub(crate) struct Page {
    #[allow(dead_code)]
    pub id: u64,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub capacity: u32,
    /// Slices handed out so far (high-water mark).
    pub used: u32,
}

pub(crate) struct TileStore {
    pub format: wgpu::TextureFormat,
    pub bpp: u32,
    pub storage: bool,
    /// Byte value of an unallocated tile (0 for layers, the default value for masks).
    pub empty_byte: u8,
    pub width: u32,
    pub height: u32,
    pub pages: Vec<Page>,
    pub map: HashMap<TileCoord, TileSlot>,
    free: Vec<TileSlot>,
    label: &'static str,
}

impl TileStore {
    pub fn new(
        format: wgpu::TextureFormat,
        bpp: u32,
        storage: bool,
        width: u32,
        height: u32,
        label: &'static str,
    ) -> Self {
        Self {
            format,
            bpp,
            storage,
            empty_byte: 0,
            width,
            height,
            pages: Vec::new(),
            map: HashMap::new(),
            free: Vec::new(),
            label,
        }
    }

    pub fn tile_bytes(&self) -> u64 {
        u64::from(TILE) * u64::from(TILE) * u64::from(self.bpp)
    }

    pub fn tile_count(&self) -> usize {
        self.map.len()
    }

    /// Bytes held by page textures (allocated, including unused slices of a page).
    pub fn allocated_bytes(&self) -> u64 {
        self.pages
            .iter()
            .map(|p| u64::from(p.capacity) * self.tile_bytes())
            .sum()
    }

    pub fn get(&self, c: TileCoord) -> Option<TileSlot> {
        self.map.get(&c).copied()
    }

    fn new_page(&mut self, ctx: &GpuContext) {
        let capacity = (4u32 << self.pages.len().min(5)).min(PAGE_MAX);
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC;
        if self.storage {
            usage |= wgpu::TextureUsages::STORAGE_BINDING;
        }
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(self.label),
            size: wgpu::Extent3d {
                width: TILE,
                height: TILE,
                depth_or_array_layers: capacity,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(self.label),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        self.pages.push(Page {
            id: NEXT_PAGE_ID.fetch_add(1, Ordering::Relaxed),
            texture,
            view,
            capacity,
            used: 0,
        });
    }

    /// Get or allocate the tile at `c`. A recycled slot is zeroed first.
    pub fn ensure(&mut self, ctx: &GpuContext, c: TileCoord) -> TileSlot {
        self.ensure_with(ctx, c, true)
    }

    /// [`TileStore::ensure`], optionally leaving a recycled slot as it was (`fill == false`)
    /// for a caller that overwrites the whole tile right away.
    fn ensure_with(&mut self, ctx: &GpuContext, c: TileCoord, fill: bool) -> TileSlot {
        if let Some(s) = self.map.get(&c) {
            return *s;
        }
        let slot = if let Some(s) = self.free.pop() {
            if fill {
                self.fill_slot(ctx, s);
            }
            s
        } else {
            if self.pages.last().is_none_or(|p| p.used == p.capacity) {
                self.new_page(ctx);
            }
            let page_idx = self.pages.len() - 1;
            let page = self.pages.last_mut().expect("page exists");
            let slice = page.used;
            page.used += 1;
            let s = TileSlot {
                page: page_idx as u32,
                slice,
            };
            if self.empty_byte != 0 && fill {
                self.fill_slot(ctx, s);
            }
            s
        };
        self.map.insert(c, slot);
        slot
    }

    fn fill_slot(&self, ctx: &GpuContext, s: TileSlot) {
        let fill = vec![self.empty_byte; self.tile_bytes() as usize];
        self.write_slot(ctx, s, 0, 0, TILE, TILE, &fill, TILE * self.bpp);
    }

    /// Drop a tile; its slot is recycled.
    pub fn free_tile(&mut self, c: TileCoord) {
        if let Some(s) = self.map.remove(&c) {
            self.free.push(s);
        }
    }

    /// Drop every tile and every page texture.
    pub fn clear(&mut self) {
        self.map.clear();
        self.free.clear();
        self.pages.clear();
    }

    #[allow(clippy::too_many_arguments)]
    fn write_slot(
        &self,
        ctx: &GpuContext,
        s: TileSlot,
        ox: u32,
        oy: u32,
        w: u32,
        h: u32,
        data: &[u8],
        bytes_per_row: u32,
    ) {
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pages[s.page as usize].texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: ox,
                    y: oy,
                    z: s.slice,
                },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    fn check_rect(&self, r: Rect) -> Result<(), GpuError> {
        if r.x.checked_add(r.w).is_none_or(|v| v > self.width)
            || r.y.checked_add(r.h).is_none_or(|v| v > self.height)
        {
            return Err(GpuError::Invalid(format!(
                "rect {r:?} outside {}x{}",
                self.width, self.height
            )));
        }
        Ok(())
    }

    /// Upload tightly packed pixels into `r`. Tiles that are not allocated and whose part of
    /// the data equals the empty byte stay unallocated.
    pub fn upload_rect(&mut self, ctx: &GpuContext, r: Rect, data: &[u8]) -> Result<(), GpuError> {
        self.check_rect(r)?;
        let bpp = self.bpp as usize;
        if data.len() != r.w as usize * r.h as usize * bpp {
            return Err(GpuError::Invalid(format!(
                "expected {} bytes for {}x{}, got {}",
                r.w as usize * r.h as usize * bpp,
                r.w,
                r.h,
                data.len()
            )));
        }
        let Some(((tx0, ty0), (tx1, ty1))) = r.tile_range() else {
            return Ok(());
        };
        let stride = r.w as usize * bpp;
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                // Sub-rectangle of `r` inside this tile, in canvas pixels.
                let x0 = r.x.max(tx * TILE);
                let y0 = r.y.max(ty * TILE);
                let x1 = (r.x + r.w).min((tx + 1) * TILE);
                let y1 = (r.y + r.h).min((ty + 1) * TILE);
                let (sw, sh) = ((x1 - x0) as usize, (y1 - y0) as usize);
                let start = (y0 - r.y) as usize * stride + (x0 - r.x) as usize * bpp;
                let exists = self.map.contains_key(&(tx, ty));
                if !exists {
                    let all_zero = (0..sh).all(|row| {
                        let o = start + row * stride;
                        data[o..o + sw * bpp].iter().all(|&b| b == self.empty_byte)
                    });
                    if all_zero {
                        continue;
                    }
                }
                let whole_tile = (x1 - x0, y1 - y0) == (TILE, TILE);
                let slot = self.ensure_with(ctx, (tx, ty), !whole_tile);
                self.write_slot(
                    ctx,
                    slot,
                    x0 - tx * TILE,
                    y0 - ty * TILE,
                    sw as u32,
                    sh as u32,
                    &data[start..],
                    stride as u32,
                );
            }
        }
        Ok(())
    }

    /// Upload the part `(ox, oy, w, h)` of tile `c` straight from a whole tile's data
    /// (`TILE * TILE * bpp` bytes, row-major, as `eskiz_core` stores it), without packing the
    /// part into a buffer of its own first: the rows keep the tile's stride (a multiple of 256
    /// bytes, so the driver can copy them as they are). Same result as [`TileStore::upload_rect`]
    /// of that part, including that an unallocated tile receiving only the empty byte stays
    /// unallocated.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_tile_part(
        &mut self,
        ctx: &GpuContext,
        c: TileCoord,
        ox: u32,
        oy: u32,
        w: u32,
        h: u32,
        tile: &[u8],
    ) -> Result<(), GpuError> {
        let bpp = self.bpp as usize;
        let stride = TILE as usize * bpp;
        if tile.len() != stride * TILE as usize || ox.checked_add(w).is_none_or(|v| v > TILE) || oy.checked_add(h).is_none_or(|v| v > TILE) {
            return Err(GpuError::Invalid(format!(
                "tile part {ox},{oy} {w}x{h} of {} bytes does not fit a {TILE}x{TILE} tile",
                tile.len()
            )));
        }
        if w == 0 || h == 0 {
            return Ok(());
        }
        self.check_rect(Rect::new(c.0 * TILE + ox, c.1 * TILE + oy, w, h))?;
        let start = oy as usize * stride + ox as usize * bpp;
        let row_bytes = w as usize * bpp;
        if !self.map.contains_key(&c) {
            let empty = (0..h as usize).all(|row| {
                let o = start + row * stride;
                tile[o..o + row_bytes].iter().all(|&b| b == self.empty_byte)
            });
            if empty {
                return Ok(());
            }
        }
        let slot = self.ensure_with(ctx, c, (w, h) != (TILE, TILE));
        self.write_slot(ctx, slot, ox, oy, w, h, &tile[start..], stride as u32);
        Ok(())
    }

    /// Read back `r` as tightly packed pixels. Missing tiles read as zeros.
    pub fn download(&self, ctx: &GpuContext, r: Rect) -> Result<Vec<u8>, GpuError> {
        self.check_rect(r)?;
        let bpp = self.bpp as usize;
        let mut out = vec![self.empty_byte; r.w as usize * r.h as usize * bpp];
        let Some(((tx0, ty0), (tx1, ty1))) = r.tile_range() else {
            return Ok(out);
        };
        let mut found: Vec<(TileCoord, TileSlot)> = Vec::new();
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                if let Some(s) = self.get((tx, ty)) {
                    found.push(((tx, ty), s));
                }
            }
        }
        if found.is_empty() {
            return Ok(out);
        }
        let tb = self.tile_bytes();
        let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("eskiz-gpu download"),
            size: tb * found.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for (i, (_, s)) in found.iter().enumerate() {
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.pages[s.page as usize].texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: s.slice,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: i as u64 * tb,
                        bytes_per_row: Some(TILE * self.bpp),
                        rows_per_image: Some(TILE),
                    },
                },
                wgpu::Extent3d {
                    width: TILE,
                    height: TILE,
                    depth_or_array_layers: 1,
                },
            );
        }
        ctx.queue.submit([enc.finish()]);
        let raw = ctx.read_buffer(&staging, tb * found.len() as u64)?;
        for (i, ((tx, ty), _)) in found.iter().enumerate() {
            let tile = &raw[i * tb as usize..(i + 1) * tb as usize];
            let x0 = r.x.max(tx * TILE);
            let y0 = r.y.max(ty * TILE);
            let x1 = (r.x + r.w).min((tx + 1) * TILE);
            let y1 = (r.y + r.h).min((ty + 1) * TILE);
            let row_bytes = (x1 - x0) as usize * bpp;
            for y in y0..y1 {
                let src = ((y - ty * TILE) as usize * TILE as usize + (x0 - tx * TILE) as usize) * bpp;
                let dst = ((y - r.y) as usize * r.w as usize + (x0 - r.x) as usize) * bpp;
                out[dst..dst + row_bytes].copy_from_slice(&tile[src..src + row_bytes]);
            }
        }
        Ok(out)
    }
}
