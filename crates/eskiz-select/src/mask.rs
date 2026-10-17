//! Selection masks: single-channel coverage at canvas size.

use crate::error::{Result, SelectError};
use crate::geom::IRect;
use crate::image::bounds_where;

const MAGIC: &[u8; 4] = b"EZSM";
const FORMAT_VERSION: u32 = 1;
/// Magic (4) + version (4) + width (8) + height (8).
const HEADER_LEN: usize = 24;

/// Coverage values in `[0, 1]`, row-major, index `y * width + x`.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

/// How a new mask combines with the existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombineMode {
    /// Result is the new mask (clear, then draw).
    Replace,
    /// Result is the per-pixel maximum (union).
    Add,
    /// Result is `old * (1 - new)`.
    Subtract,
}

impl Mask {
    /// A mask with every pixel at 0.
    pub fn empty(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    /// A mask with every pixel at 1.
    pub fn full(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![1.0; width * height],
        }
    }

    /// Value at `(x, y)`; 0 outside the mask.
    pub fn get(&self, x: usize, y: usize) -> f32 {
        if x < self.width && y < self.height {
            self.data[y * self.width + x]
        } else {
            0.0
        }
    }

    /// Writes a value clamped to `[0, 1]` (non-finite becomes 0); ignored outside the mask.
    pub fn set(&mut self, x: usize, y: usize, value: f32) {
        if x < self.width && y < self.height {
            let v = if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            };
            self.data[y * self.width + x] = v;
        }
    }

    /// `1 - value` at every pixel.
    pub fn invert(&self) -> Mask {
        Mask {
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|v| 1.0 - v).collect(),
        }
    }

    /// Combines `self` (the old mask) with `new` using `mode`.
    pub fn combine(&self, new: &Mask, mode: CombineMode) -> Result<Mask> {
        if self.width != new.width || self.height != new.height {
            return Err(SelectError::SizeMismatch);
        }
        let data = match mode {
            CombineMode::Replace => new.data.clone(),
            CombineMode::Add => self
                .data
                .iter()
                .zip(&new.data)
                .map(|(old, n)| old.max(*n))
                .collect(),
            CombineMode::Subtract => self
                .data
                .iter()
                .zip(&new.data)
                .map(|(old, n)| old * (1.0 - n))
                .collect(),
        };
        Ok(Mask {
            width: self.width,
            height: self.height,
            data,
        })
    }

    /// Bounding box of the pixels with a value above zero.
    pub fn bounds(&self) -> Option<IRect> {
        bounds_where(self.width, self.height, |i| self.data[i] > 0.0)
    }

    /// The mask as one byte per pixel (`round(value * 255)`), the layout of an `eskiz-gpu`
    /// `GpuMask` or a layer mask in a `.procreate` file.
    pub fn to_u8(&self) -> Vec<u8> {
        self.data
            .iter()
            .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect()
    }

    /// Builds a mask from one byte per pixel. Returns `None` on a length mismatch.
    pub fn from_u8(width: usize, height: usize, bytes: &[u8]) -> Option<Self> {
        if width.checked_mul(height)? != bytes.len() {
            return None;
        }
        Some(Self {
            width,
            height,
            data: bytes.iter().map(|&b| f32::from(b) / 255.0).collect(),
        })
    }

    /// Serialises the mask. Values are stored as exact little-endian f32, so a
    /// round trip through [`Mask::from_bytes`] is exact.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.data.len() * 4);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.width as u64).to_le_bytes());
        out.extend_from_slice(&(self.height as u64).to_le_bytes());
        for v in &self.data {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Parses a mask written by [`Mask::to_bytes`]. Rejects bad magic, versions,
    /// lengths and values outside `[0, 1]`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Mask> {
        if bytes.len() < HEADER_LEN || &bytes[..4] != MAGIC {
            return Err(SelectError::BadData);
        }
        let version = read_u32(bytes, 4).ok_or(SelectError::BadData)?;
        if version != FORMAT_VERSION {
            return Err(SelectError::BadData);
        }
        let width = read_u64(bytes, 8).ok_or(SelectError::BadData)? as usize;
        let height = read_u64(bytes, 16).ok_or(SelectError::BadData)? as usize;
        let count = width.checked_mul(height).ok_or(SelectError::BadData)?;
        let body = &bytes[HEADER_LEN..];
        if body.len() != count.checked_mul(4).ok_or(SelectError::BadData)? {
            return Err(SelectError::BadData);
        }
        let data: Vec<f32> = (0..count)
            .map(|i| {
                let b = &body[4 * i..4 * i + 4];
                f32::from_le_bytes([b[0], b[1], b[2], b[3]])
            })
            .collect();
        if data.iter().any(|v| !(0.0..=1.0).contains(v)) {
            return Err(SelectError::BadData);
        }
        Ok(Mask {
            width,
            height,
            data,
        })
    }
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let chunk: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(chunk))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let chunk: [u8; 8] = bytes.get(at..at + 8)?.try_into().ok()?;
    Some(u64::from_le_bytes(chunk))
}
