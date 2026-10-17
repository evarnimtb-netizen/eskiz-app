//! Animated PNG (APNG) export. Full-canvas 8-bit RGBA frames, so partial alpha is kept.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use crate::error::Result;
use crate::frame::{common_size, Frame};
use crate::fsutil::write_atomic;

/// APNG export options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ApngOptions {
    /// Number of plays. 0 loops forever (the default).
    pub loop_count: u32,
}

/// Frame delay as a fraction in seconds for the APNG `fcTL` chunk: milliseconds over 1000, or whole seconds
/// when the duration is longer than the 16-bit millisecond range.
pub fn apng_delay(duration: Duration) -> (u16, u16) {
    let ms = (duration.as_secs_f64() * 1000.0).round();
    if ms <= f64::from(u16::MAX) {
        (ms as u16, 1000)
    } else {
        (u16::MAX, 1)
    }
}

/// Encodes frames as an animated PNG.
///
/// Frames are full canvas with `Source` blending and no disposal. Pixels are written as straight
/// (non-premultiplied) RGBA8, so partial alpha survives.
pub fn encode_apng<W: Write>(writer: W, frames: &[Frame], options: &ApngOptions) -> Result<()> {
    let (width, height) = common_size(frames)?;
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_animated(frames.len() as u32, options.loop_count)?;
    let mut writer = encoder.write_header()?;
    for frame in frames {
        let (numerator, denominator) = apng_delay(frame.duration());
        writer.set_frame_delay(numerator, denominator)?;
        writer.set_blend_op(png::BlendOp::Source)?;
        writer.set_dispose_op(png::DisposeOp::None)?;
        writer.write_image_data(&frame.to_straight_rgba())?;
    }
    writer.finish()?;
    Ok(())
}

/// Encodes frames to an APNG file. The file is written atomically.
pub fn write_apng_file(path: &Path, frames: &[Frame], options: &ApngOptions) -> Result<()> {
    write_atomic(path, |file: &mut File| encode_apng(file, frames, options))
}
