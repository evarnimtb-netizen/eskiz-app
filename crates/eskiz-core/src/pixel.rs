//! Pixel depth: 8-bit and 16-bit channels, and the conversions between them.
//!
//! Every [`crate::Layer`] stores premultiplied RGBA in one of two depths ([`Depth`]). The tile
//! code is generic over the channel type through [`Chan`], so the 8-bit path compiles to the
//! same loops it always had and the 16-bit path shares every line of tile logic with it.
//!
//! # Conversions
//!
//! * Up: `v * 257`, so 0 stays 0 and 255 becomes 65535 exactly.
//! * Down: `(v + 128) / 257`, round to nearest. `down(up(v)) == v` for every 8-bit value, so a
//!   document converted to 16 bits and back is unchanged.
//! * Down with dithering ([`down_dithered`]): `(v + t) / 257` with a per-pixel threshold `t` in
//!   `0..=256` from [`dither_threshold`]. The same threshold is used for all four channels, so
//!   the dither adds luminance grain but no colour noise and keeps `colour <= alpha`.
//!   A value that is an exact multiple of 257 (anything that began as 8-bit) is never changed.

use crate::canvas::Tile;

/// Bits per channel of a layer or document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Depth {
    /// 8 bits per channel (premultiplied RGBA8). The default.
    #[default]
    U8,
    /// 16 bits per channel (premultiplied RGBA16, native-endian `u16`).
    U16,
}

impl Depth {
    /// 8 or 16.
    pub fn bits(self) -> u32 {
        match self {
            Depth::U8 => 8,
            Depth::U16 => 16,
        }
    }

    /// 1 or 2.
    pub fn bytes_per_channel(self) -> usize {
        match self {
            Depth::U8 => 1,
            Depth::U16 => 2,
        }
    }

    /// Bytes in one 256x256 RGBA tile.
    pub fn tile_bytes(self) -> usize {
        crate::canvas::TILE_BYTES * self.bytes_per_channel()
    }

    /// "8-bit" or "16-bit".
    pub fn label(self) -> &'static str {
        match self {
            Depth::U8 => "8-bit",
            Depth::U16 => "16-bit",
        }
    }

    /// Depth from a bit count (8 or 16). Anything else is `None`.
    pub fn from_bits(bits: u32) -> Option<Self> {
        match bits {
            8 => Some(Depth::U8),
            16 => Some(Depth::U16),
            _ => None,
        }
    }
}

/// 8-bit to 16-bit: `v * 257`.
#[inline(always)]
pub fn up8(v: u8) -> u16 {
    u16::from(v) * 257
}

/// 16-bit to 8-bit, round to nearest.
#[inline(always)]
pub fn down16(v: u16) -> u8 {
    ((u32::from(v) + 128) / 257) as u8
}

/// 16-bit to 8-bit with a dither threshold `t` (see [`dither_threshold`]).
#[inline(always)]
pub fn down_dithered(v: u16, t: u32) -> u8 {
    ((u32::from(v) + t) / 257).min(255) as u8
}

/// Per-pixel dither threshold in `0..=256`: interleaved gradient noise, which has no visible
/// repeating pattern and a nearly uniform value histogram.
#[inline]
pub fn dither_threshold(x: usize, y: usize) -> u32 {
    let (fx, fy) = (x as f32, y as f32);
    let n = (0.067_110_56 * fx + 0.005_837_15 * fy).fract();
    let ign = (52.982_918 * n).fract();
    ((ign * 257.0) as u32).min(256)
}

/// One premultiplied RGBA8 pixel to 16 bits.
#[inline(always)]
pub fn px_up(p: [u8; 4]) -> [u16; 4] {
    p.map(up8)
}

/// One premultiplied RGBA16 pixel to 8 bits, rounding.
#[inline(always)]
pub fn px_down(p: [u16; 4]) -> [u8; 4] {
    p.map(down16)
}

/// A channel type of a tile: `u8` or `u16`.
pub trait Chan: Copy + Default + PartialEq + Send + Sync + 'static {
    /// The depth that stores this channel type.
    const DEPTH: Depth;
    /// The largest value (255 or 65535).
    const MAX: f64;
    /// A value in 0..1 to this channel, rounding and clamping (NaN gives 0).
    fn from_unit(v: f64) -> Self;
    /// This channel as a value in 0..1.
    fn to_unit(self) -> f64;
    /// The raw integer value as `f64` (0..=255 or 0..=65535).
    fn raw(self) -> f64;
    /// The largest value as `f32`.
    const MAXF: f32;
    /// The raw integer value as `f32` (exact at both depths).
    fn to_f32(self) -> f32;
    /// Round a raw-scale `f32` (0..=MAX) to this channel, clamping.
    fn from_f32_round(v: f32) -> Self;
    /// Round a raw-scale `f64` (0..=MAX) to this channel, clamping.
    fn from_f64_round(v: f64) -> Self;
    /// Widen to 16 bits (`u8` values are multiplied by 257).
    fn to_u16(self) -> u16;
    /// Narrow from 16 bits (`u8` rounds to nearest).
    fn from_u16(v: u16) -> Self;
    /// The tile's samples if it has this depth. Callers have matched the layer's depth.
    fn slice(t: &Tile) -> &[Self];
    /// Mutable samples of a tile of this depth.
    fn slice_mut(t: &mut Tile) -> &mut [Self];
    /// A transparent tile of this depth.
    fn blank_tile() -> Tile;
    /// Is the value zero?
    fn is_zero(self) -> bool;
    /// A shared tile of zeros.
    fn zeros() -> &'static [Self];
}

impl Chan for u8 {
    const DEPTH: Depth = Depth::U8;
    const MAX: f64 = 255.0;
    const MAXF: f32 = 255.0;
    #[inline(always)]
    fn from_f32_round(v: f32) -> Self {
        v.round().clamp(0.0, 255.0) as u8
    }
    #[inline(always)]
    fn from_f64_round(v: f64) -> Self {
        v.round().clamp(0.0, 255.0) as u8
    }
    #[inline(always)]
    fn from_unit(v: f64) -> Self {
        crate::compose::to_u8(v)
    }
    #[inline(always)]
    fn to_unit(self) -> f64 {
        f64::from(self) / 255.0
    }
    #[inline(always)]
    fn raw(self) -> f64 {
        f64::from(self)
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        f32::from(self)
    }
    #[inline(always)]
    fn to_u16(self) -> u16 {
        up8(self)
    }
    #[inline(always)]
    fn from_u16(v: u16) -> Self {
        down16(v)
    }
    #[inline(always)]
    fn slice(t: &Tile) -> &[u8] {
        t.data()
    }
    #[inline(always)]
    fn slice_mut(t: &mut Tile) -> &mut [u8] {
        t.data_mut()
    }
    fn blank_tile() -> Tile {
        Tile::blank()
    }
    #[inline(always)]
    fn is_zero(self) -> bool {
        self == 0
    }
    fn zeros() -> &'static [u8] {
        &crate::canvas::ZERO_TILE
    }
}

impl Chan for u16 {
    const DEPTH: Depth = Depth::U16;
    const MAX: f64 = 65535.0;
    const MAXF: f32 = 65535.0;
    #[inline(always)]
    fn from_f32_round(v: f32) -> Self {
        v.round().clamp(0.0, 65535.0) as u16
    }
    #[inline(always)]
    fn from_f64_round(v: f64) -> Self {
        v.round().clamp(0.0, 65535.0) as u16
    }
    #[inline(always)]
    fn from_unit(v: f64) -> Self {
        // `as` saturates and maps NaN to 0.
        (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16
    }
    #[inline(always)]
    fn to_unit(self) -> f64 {
        f64::from(self) / 65535.0
    }
    #[inline(always)]
    fn raw(self) -> f64 {
        f64::from(self)
    }
    #[inline(always)]
    fn to_f32(self) -> f32 {
        f32::from(self)
    }
    #[inline(always)]
    fn to_u16(self) -> u16 {
        self
    }
    #[inline(always)]
    fn from_u16(v: u16) -> Self {
        v
    }
    #[inline(always)]
    fn slice(t: &Tile) -> &[u16] {
        t.data16().expect("16-bit tile expected")
    }
    #[inline(always)]
    fn slice_mut(t: &mut Tile) -> &mut [u16] {
        t.data16_mut().expect("16-bit tile expected")
    }
    fn blank_tile() -> Tile {
        Tile::blank16()
    }
    #[inline(always)]
    fn is_zero(self) -> bool {
        self == 0
    }
    fn zeros() -> &'static [u16] {
        &crate::canvas::ZERO_TILE16
    }
}

/// Convert a flat RGBA8 buffer to RGBA16.
pub fn rgba8_to_16(src: &[u8]) -> Vec<u16> {
    src.iter().map(|&v| up8(v)).collect()
}

/// Convert a flat RGBA16 buffer to RGBA8, rounding.
pub fn rgba16_to_8(src: &[u16]) -> Vec<u8> {
    src.iter().map(|&v| down16(v)).collect()
}

/// Convert a flat `width x height` RGBA16 buffer to RGBA8 with dithering. `(ox, oy)` is the
/// position of the buffer's first pixel in the canvas, so a rectangle dithers the same as the
/// whole image.
pub fn rgba16_to_8_dithered(src: &[u16], width: usize, ox: usize, oy: usize) -> Vec<u8> {
    let mut out = vec![0u8; src.len()];
    if width == 0 {
        return out;
    }
    for (i, (s, o)) in src.as_chunks::<4>().0.iter().zip(out.as_chunks_mut::<4>().0.iter_mut()).enumerate() {
        let t = dither_threshold(ox + i % width, oy + i / width);
        for k in 0..4 {
            o[k] = down_dithered(s[k], t);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_to_sixteen_to_eight_is_identity() {
        for v in 0..=255u8 {
            assert_eq!(down16(up8(v)), v);
            for t in [0, 1, 128, 256] {
                assert_eq!(down_dithered(up8(v), t), v, "{v} t={t}");
            }
        }
        assert_eq!(up8(255), 65535);
        assert_eq!(down16(65535), 255);
        assert_eq!(down16(0), 0);
    }

    #[test]
    fn dither_averages_to_the_true_value() {
        // A flat 16-bit value between two 8-bit levels dithers to a mix with the right mean.
        let v = 257 * 100 + 64; // a quarter of the way from 100 to 101
        let n = 20_000;
        let mut sum = 0u64;
        for i in 0..n {
            sum += u64::from(down_dithered(v, dither_threshold(i % 211, i / 211)));
        }
        let mean = sum as f64 / n as f64;
        assert!((mean - (100.0 + 64.0 / 257.0)).abs() < 0.03, "{mean}");
    }

    #[test]
    fn from_unit_rounds_and_clamps() {
        assert_eq!(<u16 as Chan>::from_unit(1.0), 65535);
        assert_eq!(<u16 as Chan>::from_unit(-1.0), 0);
        assert_eq!(<u16 as Chan>::from_unit(f64::NAN), 0);
        assert_eq!(<u16 as Chan>::from_unit(0.5), 32768);
        assert_eq!(<u8 as Chan>::from_unit(0.5), 128);
    }
}
