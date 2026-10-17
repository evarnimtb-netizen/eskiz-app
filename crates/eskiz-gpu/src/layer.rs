//! Sparse tiled layers and layer masks.
//!
//! # Storage format
//!
//! Layer tiles of an 8-bit document are **RGBA8Unorm, premultiplied**. This is the same layout
//! `eskiz-core`'s `Layer` uses, so uploads and downloads are bit-exact, and the 8-bit rounding
//! of the CPU reference is shared.
//!
//! Layer tiles of a 16-bit document are **RGBA16Float, premultiplied**: twice the memory (a full
//! 4096x4096 layer is 128 MiB instead of 64 MiB). Half floats keep at least 11 significant bits
//! for every value above 1/2048, which is well beyond 8 bits at any brightness, and, unlike
//! `Rgba16Unorm`, need no optional device feature. The CPU `u16` samples are converted on
//! upload (`v / 65535`, round to nearest even) and back on download. The composite shader reads
//! both formats through `textureLoad` as `f32`, so it is unchanged; its output stays RGBA8
//! (the display), which is also what makes display conversion identical at both depths.
//! Masks are R8Unorm tiles.

use crate::context::{GpuContext, GpuError};
use crate::tile::{Rect, TileCoord, TileStore};
use eskiz_core::Depth;

/// One raster layer: sparse 256x256 premultiplied RGBA tiles, 8-bit (`Rgba8Unorm`) or 16-bit
/// (`Rgba16Float`).
pub struct GpuLayer {
    pub(crate) store: TileStore,
    depth: Depth,
}

impl GpuLayer {
    /// Empty (fully transparent) 8-bit layer. No GPU memory is allocated yet.
    pub fn new(width: u32, height: u32) -> Self {
        Self::new_depth(width, height, Depth::U8)
    }

    /// Empty layer of `depth`. No GPU memory is allocated yet.
    pub fn new_depth(width: u32, height: u32, depth: Depth) -> Self {
        let (format, bpp) = match depth {
            Depth::U8 => (wgpu::TextureFormat::Rgba8Unorm, 4),
            Depth::U16 => (wgpu::TextureFormat::Rgba16Float, 8),
        };
        Self {
            store: TileStore::new(format, bpp, true, width, height, "eskiz layer tiles"),
            depth,
        }
    }

    /// Bits per channel of the tile storage.
    pub fn depth(&self) -> Depth {
        self.depth
    }

    pub fn width(&self) -> u32 {
        self.store.width
    }

    pub fn height(&self) -> u32 {
        self.store.height
    }

    /// Upload premultiplied RGBA8 pixels (`w * h * 4` bytes, tightly packed) into `rect`.
    /// Tiles that are empty and receive only zeros stay unallocated. Returns the rectangle
    /// to mark dirty in the compositor. A 16-bit layer widens the values (`v * 257`).
    pub fn upload_rect(&mut self, ctx: &GpuContext, rect: Rect, rgba: &[u8]) -> Result<Rect, GpuError> {
        match self.depth {
            Depth::U8 => self.store.upload_rect(ctx, rect, rgba)?,
            Depth::U16 => {
                let wide: Vec<u16> = rgba.iter().map(|&v| eskiz_core::pixel::up8(v)).collect();
                return self.upload_rect16(ctx, rect, &wide);
            }
        }
        Ok(rect)
    }

    /// Upload premultiplied RGBA16 pixels (`w * h * 4` samples, tightly packed) into `rect`. An
    /// 8-bit layer rounds them.
    pub fn upload_rect16(&mut self, ctx: &GpuContext, rect: Rect, rgba: &[u16]) -> Result<Rect, GpuError> {
        match self.depth {
            Depth::U8 => {
                let narrow: Vec<u8> = rgba.iter().map(|&v| eskiz_core::pixel::down16(v)).collect();
                self.store.upload_rect(ctx, rect, &narrow)?;
            }
            Depth::U16 => {
                let halves: Vec<u16> = rgba.iter().map(|&v| unorm16_to_f16(v)).collect();
                self.store.upload_rect(ctx, rect, bytemuck::cast_slice(&halves))?;
            }
        }
        Ok(rect)
    }

    /// Upload the part `part` (in pixels inside the tile) of tile `tile` from the tile's own
    /// samples: `data` is one whole 256 x 256 tile of premultiplied RGBA8 (`eskiz_core`'s
    /// `Tile::data`), so no copy of the part is made on the way. The result is exactly that
    /// of [`GpuLayer::upload_rect`] with the same pixels. A 16-bit layer widens (`v * 257`).
    pub fn upload_tile(&mut self, ctx: &GpuContext, tile: TileCoord, part: Rect, data: &[u8]) -> Result<(), GpuError> {
        match self.depth {
            Depth::U8 => self.store.upload_tile_part(ctx, tile, part.x, part.y, part.w, part.h, data),
            Depth::U16 => {
                let wide: Vec<u16> = data.iter().map(|&v| eskiz_core::pixel::up8(v)).collect();
                self.upload_tile16(ctx, tile, part, &wide)
            }
        }
    }

    /// [`GpuLayer::upload_tile`] with a whole tile of premultiplied RGBA16 samples (`eskiz_core`'s
    /// `Tile::data16`). A 16-bit layer converts just the part to half floats, in one pass; an
    /// 8-bit layer rounds the samples.
    pub fn upload_tile16(&mut self, ctx: &GpuContext, tile: TileCoord, part: Rect, data: &[u16]) -> Result<(), GpuError> {
        let t = crate::tile::TILE as usize;
        if data.len() != t * t * 4 || part.x as usize + part.w as usize > t || part.y as usize + part.h as usize > t {
            return Err(GpuError::Invalid(format!("tile part {part:?} of {} samples does not fit a tile", data.len())));
        }
        let row = |y: usize| {
            let o = ((part.y as usize + y) * t + part.x as usize) * 4;
            &data[o..o + part.w as usize * 4]
        };
        let abs = Rect::new(tile.0 * crate::tile::TILE + part.x, tile.1 * crate::tile::TILE + part.y, part.w, part.h);
        match self.depth {
            Depth::U8 => {
                let mut narrow = Vec::with_capacity(part.w as usize * part.h as usize * 4);
                for y in 0..part.h as usize {
                    narrow.extend(row(y).iter().map(|&v| eskiz_core::pixel::down16(v)));
                }
                self.store.upload_rect(ctx, abs, &narrow)
            }
            Depth::U16 => {
                let mut halves = Vec::with_capacity(part.w as usize * part.h as usize * 4);
                for y in 0..part.h as usize {
                    halves.extend(row(y).iter().map(|&v| unorm16_to_f16(v)));
                }
                self.store.upload_rect(ctx, abs, bytemuck::cast_slice(&halves))
            }
        }
    }

    /// Upload a whole canvas-sized premultiplied RGBA8 buffer.
    pub fn upload_all(&mut self, ctx: &GpuContext, rgba: &[u8]) -> Result<Rect, GpuError> {
        let r = Rect::new(0, 0, self.width(), self.height());
        self.upload_rect(ctx, r, rgba)
    }

    /// Read `rect` back as premultiplied RGBA8. Missing tiles read as transparent. A 16-bit
    /// layer rounds.
    pub fn download(&self, ctx: &GpuContext, rect: Rect) -> Result<Vec<u8>, GpuError> {
        match self.depth {
            Depth::U8 => self.store.download(ctx, rect),
            Depth::U16 => Ok(self.download16(ctx, rect)?.into_iter().map(eskiz_core::pixel::down16).collect()),
        }
    }

    /// Read `rect` back as premultiplied RGBA16 (an 8-bit layer is widened). Half-float tiles
    /// give back the nearest of the 65536 levels.
    pub fn download16(&self, ctx: &GpuContext, rect: Rect) -> Result<Vec<u16>, GpuError> {
        match self.depth {
            Depth::U8 => Ok(self.store.download(ctx, rect)?.into_iter().map(eskiz_core::pixel::up8).collect()),
            Depth::U16 => {
                let raw = self.store.download(ctx, rect)?;
                let halves: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|b| u16::from_ne_bytes(*b)).collect();
                Ok(halves.into_iter().map(f16_to_unorm16).collect())
            }
        }
    }

    /// Read the whole layer back.
    pub fn download_all(&self, ctx: &GpuContext) -> Result<Vec<u8>, GpuError> {
        self.download(ctx, Rect::new(0, 0, self.width(), self.height()))
    }

    /// Number of allocated tiles.
    pub fn tile_count(&self) -> usize {
        self.store.tile_count()
    }

    /// True if the tile exists (is non-empty as far as the GPU knows).
    pub fn has_tile(&self, tile: TileCoord) -> bool {
        self.store.get(tile).is_some()
    }

    /// Bytes of GPU texture memory held by this layer.
    pub fn allocated_bytes(&self) -> u64 {
        self.store.allocated_bytes()
    }

    /// Release one tile (it becomes transparent).
    pub fn free_tile(&mut self, tile: TileCoord) {
        self.store.free_tile(tile);
    }

    /// Release all tiles and textures.
    pub fn clear(&mut self) {
        self.store.clear();
    }
}

/// A 0..65535 sample as half-float bits holding `v / 65535`, rounded to nearest even.
pub fn unorm16_to_f16(v: u16) -> u16 {
    f32_to_f16(f32::from(v) / 65535.0)
}

/// Half-float bits to a 0..65535 sample (`value * 65535`, rounded, clamped).
pub fn f16_to_unorm16(h: u16) -> u16 {
    (f16_to_f32(h) * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16
}

/// IEEE binary32 to binary16 bits, round to nearest even. Handles the whole finite range,
/// subnormals, infinities and NaN.
pub fn f32_to_f16(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let man = b & 0x007F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if man != 0 { 0x0200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1F {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x0080_0000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let round = (m >> (shift - 1)) & 1;
        let sticky = m & ((1 << (shift - 1)) - 1);
        let r = half + (round & (u32::from(sticky != 0) | (half & 1)));
        return sign | r as u16;
    }
    let half = ((e as u32) << 10) | (man >> 13);
    let round = (man >> 12) & 1;
    let sticky = man & 0x0FFF;
    let r = half + (round & (u32::from(sticky != 0) | (half & 1)));
    sign | r as u16
}

/// Binary16 bits to `f32`.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = u32::from((h >> 10) & 0x1F);
    let man = u32::from(h & 0x03FF);
    let bits = match exp {
        0 if man == 0 => sign,
        0 => {
            // Subnormal: normalise.
            let shift = man.leading_zeros() - 21;
            let m = (man << shift) & 0x03FF;
            sign | ((113 - shift) << 23) | (m << 13)
        }
        0x1F => sign | 0x7F80_0000 | (man << 13),
        _ => sign | ((exp + 112) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}

/// A layer mask: sparse R8Unorm tiles where 1 shows and 0 hides.
///
/// Tiles that are not allocated read as `default_value`: 1.0 for "reveal all", 0.0 for
/// "hide all". So a fresh reveal-all mask costs no memory.
pub struct GpuMask {
    pub(crate) store: TileStore,
    default_value: f32,
}

impl GpuMask {
    /// Mask where every unallocated tile has value `default_value` (clamped to 0..1).
    pub fn new(width: u32, height: u32, default_value: f32) -> Self {
        let default_value = default_value.clamp(0.0, 1.0);
        let mut store = TileStore::new(
            wgpu::TextureFormat::R8Unorm,
            1,
            false,
            width,
            height,
            "eskiz mask tiles",
        );
        store.empty_byte = (default_value * 255.0).round() as u8;
        Self { store, default_value }
    }

    pub fn default_value(&self) -> f32 {
        self.default_value
    }

    /// Upload `w * h` mask bytes (value = byte / 255). Tiles stay unallocated while their
    /// data equals the default byte.
    pub fn upload_rect(&mut self, ctx: &GpuContext, rect: Rect, bytes: &[u8]) -> Result<Rect, GpuError> {
        self.store.upload_rect(ctx, rect, bytes)?;
        Ok(rect)
    }

    /// Read `rect` back as mask bytes. Unallocated tiles read as the default value.
    pub fn download(&self, ctx: &GpuContext, rect: Rect) -> Result<Vec<u8>, GpuError> {
        self.store.download(ctx, rect)
    }

    pub fn tile_count(&self) -> usize {
        self.store.tile_count()
    }

    pub fn allocated_bytes(&self) -> u64 {
        self.store.allocated_bytes()
    }
}

#[cfg(test)]
mod half_tests {
    use super::*;

    #[test]
    fn half_conversion_round_trips_every_half() {
        for h in 0..=u16::MAX {
            if (h >> 10) & 0x1F == 0x1F {
                continue; // inf and NaN
            }
            assert_eq!(f32_to_f16(f16_to_f32(h)), h, "{h:#06x}");
        }
        assert_eq!(f32_to_f16(1.0), 0x3C00);
        assert_eq!(f32_to_f16(0.0), 0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
    }

    #[test]
    fn unorm16_survives_the_half_round_trip_within_its_precision() {
        for v in (0..=u16::MAX).step_by(7).chain([0, 1, 65534, 65535]) {
            let back = f16_to_unorm16(unorm16_to_f16(v));
            // 11 significant bits: the error is at most one half step of the value's binade.
            let tol = (f64::from(v) / 1024.0).max(33.0);
            assert!((f64::from(back) - f64::from(v)).abs() <= tol, "{v} -> {back}");
        }
        assert_eq!(f16_to_unorm16(unorm16_to_f16(65535)), 65535);
        assert_eq!(f16_to_unorm16(unorm16_to_f16(0)), 0);
    }
}
