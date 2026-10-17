//! Input frames: premultiplied RGBA8 pixels plus the time the frame is shown for.

use std::time::Duration;

use crate::error::{MediaError, Result};

/// One picture: premultiplied RGBA8, row-major, top row first, and how long it is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    duration: Duration,
}

impl Frame {
    /// Creates a frame. `pixels` must hold `width * height * 4` bytes of premultiplied RGBA8.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>, duration: Duration) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(MediaError::InvalidFrame(format!(
                "zero size {width} x {height}"
            )));
        }
        let expected = width as usize * height as usize * 4;
        if pixels.len() != expected {
            return Err(MediaError::InvalidFrame(format!(
                "expected {expected} bytes for {width} x {height} RGBA, got {}",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            pixels,
            duration,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied RGBA8 pixels.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// How long this frame is shown.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Straight (non-premultiplied) RGBA8, which is what PNG stores.
    pub fn to_straight_rgba(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len());
        for px in self.pixels.as_chunks::<4>().0 {
            let a = u32::from(px[3]);
            for &c in &px[..3] {
                out.push(unpremultiply(c, a));
            }
            out.push(px[3]);
        }
        out
    }

    /// Composites the frame over an opaque background colour and returns RGB8.
    pub fn composite_rgb(&self, background: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() / 4 * 3);
        for px in self.pixels.as_chunks::<4>().0 {
            let a = u32::from(px[3]);
            for (&c, &bg) in px[..3].iter().zip(&background) {
                out.push(over_background(c, a, bg));
            }
        }
        out
    }

    /// Box-filter (area average) resample to `width` x `height`. The result carries `duration`.
    ///
    /// Works in premultiplied space, so transparent edges do not darken.
    pub fn resize_box(&self, width: u32, height: u32, duration: Duration) -> Result<Frame> {
        if width == 0 || height == 0 {
            return Err(MediaError::InvalidSettings(format!(
                "zero target size {width} x {height}"
            )));
        }
        if (width, height) == (self.width, self.height) {
            return Ok(Frame {
                duration,
                ..self.clone()
            });
        }
        let (sw, sh) = (self.width as usize, self.height as usize);
        let (dw, dh) = (width as usize, height as usize);
        let mut out = vec![0u8; dw * dh * 4];
        for dy in 0..dh {
            let y0 = dy * sh / dh;
            let y1 = ((dy + 1) * sh / dh).max(y0 + 1);
            for dx in 0..dw {
                let x0 = dx * sw / dw;
                let x1 = ((dx + 1) * sw / dw).max(x0 + 1);
                let mut sum = [0u64; 4];
                let mut count = 0u64;
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        let i = (sy * sw + sx) * 4;
                        for (total, &p) in sum.iter_mut().zip(&self.pixels[i..i + 4]) {
                            *total += u64::from(p);
                        }
                        count += 1;
                    }
                }
                let o = (dy * dw + dx) * 4;
                for (dst, total) in out[o..o + 4].iter_mut().zip(sum) {
                    *dst = ((total + count / 2) / count) as u8;
                }
            }
        }
        Frame::new(width, height, out, duration)
    }
}

/// Duration of one frame at `fps` frames per second.
pub fn duration_for_fps(fps: u32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))
}

/// Checks that all frames share one size and returns it.
pub(crate) fn common_size(frames: &[Frame]) -> Result<(u32, u32)> {
    let first = frames.first().ok_or(MediaError::NoFrames)?;
    let size = (first.width, first.height);
    if frames.iter().any(|f| (f.width, f.height) != size) {
        return Err(MediaError::InvalidFrame("frames differ in size".into()));
    }
    Ok(size)
}

/// Composites one premultiplied channel over an opaque background channel.
pub(crate) fn over_background(premultiplied: u8, alpha: u32, background: u8) -> u8 {
    let v = u32::from(premultiplied) + (u32::from(background) * (255 - alpha) + 127) / 255;
    v.min(255) as u8
}

fn unpremultiply(c: u8, alpha: u32) -> u8 {
    (u32::from(c) * 255 + alpha / 2)
        .checked_div(alpha)
        .unwrap_or(0)
        .min(255) as u8
}
