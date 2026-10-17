//! H.264 in MP4 export. Frames are composited over a background, padded to even sizes, converted to
//! BT.709 I420, encoded with OpenH264, and muxed with the `mp4` crate.

use crate::spec::{bitrate_bps, Quality};

#[cfg(feature = "h264")]
pub(crate) use encoder::Mp4Encoder;
#[cfg(feature = "h264")]
pub use encoder::{encode_mp4, write_mp4_file};
#[cfg(not(feature = "h264"))]
pub(crate) use off::Mp4Encoder;
#[cfg(not(feature = "h264"))]
pub use off::{encode_mp4, write_mp4_file};

/// MP4 export options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4Options {
    /// Target bitrate in bits per second. [`crate::bitrate_bps`] gives the documented value.
    pub bitrate_bps: u32,
    /// Colour that fills transparent pixels and the even-size padding. White by default.
    pub background: [u8; 3],
}

impl Default for Mp4Options {
    fn default() -> Self {
        Self {
            bitrate_bps: bitrate_bps(1920, 1080, Quality::Standard),
            background: [255, 255, 255],
        }
    }
}

#[cfg(feature = "h264")]
mod encoder {
    use std::fs::File;
    use std::io::{Cursor, Seek, Write};
    use std::path::Path;
    use std::time::Duration;

    use openh264::formats::YUVSource;

    use super::Mp4Options;
    use crate::error::{MediaError, Result};
    use crate::frame::{common_size, Frame};
    use crate::fsutil::write_atomic;
    use crate::h264::{rgb_to_i420, H264Encoder};
    use crate::mux::{access_unit_from_annex_b, Muxer, TRACK_TIMESCALE};

    /// One encoded picture in the sample table.
    struct Sample {
        avcc: Vec<u8>,
        is_sync: bool,
        duration: u64,
    }

    /// Streams frames into an H.264 sample table. [`Mp4Encoder::finish`] writes the MP4.
    pub(crate) struct Mp4Encoder {
        source: (u32, u32),
        padded: (u32, u32),
        background: [u8; 3],
        encoder: H264Encoder,
        sps: Option<Vec<u8>>,
        pps: Option<Vec<u8>>,
        samples: Vec<Sample>,
        /// Presentation time of the next picture, in seconds.
        elapsed_seconds: f64,
        /// Ticks of pictures the encoder has not emitted yet. They go to the next sample.
        carry_ticks: u64,
    }

    impl Mp4Encoder {
        /// Encoder for pictures of `width` x `height`. Odd sides are padded by one row or column.
        pub(crate) fn new(width: u32, height: u32, options: &Mp4Options, max_fps: f32) -> Result<Self> {
            if width == 0 || height == 0 {
                return Err(MediaError::InvalidSettings(format!(
                    "zero size {width} x {height}"
                )));
            }
            Ok(Self {
                source: (width, height),
                padded: (width.next_multiple_of(2), height.next_multiple_of(2)),
                background: options.background,
                encoder: H264Encoder::new(options.bitrate_bps, max_fps)?,
                sps: None,
                pps: None,
                samples: Vec::new(),
                elapsed_seconds: 0.0,
                carry_ticks: 0,
            })
        }

        /// Adds one frame, shown for its own duration.
        pub(crate) fn push_frame(&mut self, frame: &Frame) -> Result<()> {
            if (frame.width(), frame.height()) != self.source {
                return Err(MediaError::InvalidFrame(format!(
                    "frame is {} x {}, encoder expects {} x {}",
                    frame.width(),
                    frame.height(),
                    self.source.0,
                    self.source.1
                )));
            }
            let rgb = frame.composite_rgb(self.background);
            let src = (self.source.0 as usize, self.source.1 as usize);
            let dst = (self.padded.0 as usize, self.padded.1 as usize);
            let yuv = rgb_to_i420(&rgb, src, dst, self.background);
            self.push_yuv(&yuv, frame.duration())
        }

        /// Adds one already-converted picture (of the coded size) shown for `duration`.
        pub(crate) fn push_yuv<Y: YUVSource>(&mut self, yuv: &Y, duration: Duration) -> Result<()> {
            let coded = (self.padded.0 as usize, self.padded.1 as usize);
            if yuv.dimensions() != coded {
                return Err(MediaError::InvalidFrame(
                    "picture does not match the coded size".into(),
                ));
            }
            // Timestamps are rounded from exact seconds, so long runs do not drift.
            let ticks = |seconds: f64| (seconds * f64::from(TRACK_TIMESCALE)).round() as u64;
            let start = ticks(self.elapsed_seconds);
            self.elapsed_seconds += duration.as_secs_f64();
            self.carry_ticks += ticks(self.elapsed_seconds) - start;

            let stream = self.encoder.encode(yuv)?;
            if stream.is_empty() {
                return Ok(());
            }
            let unit = access_unit_from_annex_b(&stream);
            if self.sps.is_none() {
                self.sps = unit.sps;
            }
            if self.pps.is_none() {
                self.pps = unit.pps;
            }
            if unit.avcc.is_empty() {
                return Ok(());
            }
            self.samples.push(Sample {
                avcc: unit.avcc,
                is_sync: unit.is_sync,
                duration: std::mem::take(&mut self.carry_ticks),
            });
            Ok(())
        }

        /// SPS and PPS, once the encoder has produced them.
        pub(crate) fn parameter_sets(&self) -> Option<(Vec<u8>, Vec<u8>)> {
            match (&self.sps, &self.pps) {
                (Some(sps), Some(pps)) => Some((sps.clone(), pps.clone())),
                _ => None,
            }
        }

        /// Writes the MP4 to `writer`.
        pub(crate) fn finish<W: Write + Seek>(mut self, writer: W) -> Result<()> {
            let (Some(sps), Some(pps)) = (self.sps.take(), self.pps.take()) else {
                return Err(MediaError::InvalidFrame(
                    "the encoder produced no parameter sets".into(),
                ));
            };
            let Some(last) = self.samples.last_mut() else {
                return Err(MediaError::NoFrames);
            };
            last.duration += std::mem::take(&mut self.carry_ticks);
            let mut muxer = Muxer::begin(writer, self.padded.0, self.padded.1, &sps, &pps)?;
            for sample in &self.samples {
                muxer.push(&sample.avcc, sample.is_sync, sample.duration as u32)?;
            }
            muxer.finish()
        }
    }

    /// Frame duration to a frame rate for the encoder's rate control, clamped to the timelapse range.
    fn max_fps(duration: Duration) -> f32 {
        (1.0 / duration.as_secs_f64()).clamp(1.0, 60.0) as f32
    }

    /// Encodes frames to MP4 bytes in memory.
    ///
    /// Every frame must share one size. Odd sides are padded to even with `options.background`.
    /// Each frame is shown for its own duration.
    pub fn encode_mp4(frames: &[Frame], options: &Mp4Options) -> Result<Vec<u8>> {
        let (width, height) = common_size(frames)?;
        let mut encoder = Mp4Encoder::new(width, height, options, max_fps(frames[0].duration()))?;
        for frame in frames {
            encoder.push_frame(frame)?;
        }
        let mut out = Cursor::new(Vec::new());
        encoder.finish(&mut out)?;
        Ok(out.into_inner())
    }

    /// Encodes frames to an MP4 file. The file is written atomically.
    pub fn write_mp4_file(path: &Path, frames: &[Frame], options: &Mp4Options) -> Result<()> {
        let (width, height) = common_size(frames)?;
        let mut encoder = Mp4Encoder::new(width, height, options, max_fps(frames[0].duration()))?;
        for frame in frames {
            encoder.push_frame(frame)?;
        }
        write_atomic(path, |file: &mut File| encoder.finish(file))
    }
}

/// Builds without an H.264 encoder (no `source` or `libloading` feature): every MP4 path reports
/// [`crate::MP4_UNAVAILABLE`].
#[cfg(not(feature = "h264"))]
mod off {
    use std::path::Path;

    use super::Mp4Options;
    use crate::error::{MediaError, Result};
    use crate::frame::Frame;

    fn unavailable<T>() -> Result<T> {
        Err(MediaError::EncoderUnavailable(crate::h264::MP4_UNAVAILABLE.to_owned()))
    }

    /// Uninhabited: no encoder can be created in this build.
    pub(crate) struct Mp4Encoder(std::convert::Infallible);

    impl Mp4Encoder {
        pub(crate) fn new(_: u32, _: u32, _: &Mp4Options, _: f32) -> Result<Self> {
            unavailable()
        }
        pub(crate) fn push_frame(&mut self, _: &Frame) -> Result<()> {
            match self.0 {}
        }
        pub(crate) fn parameter_sets(&self) -> Option<(Vec<u8>, Vec<u8>)> {
            match self.0 {}
        }
        pub(crate) fn finish<W>(self, _: W) -> Result<()> {
            match self.0 {}
        }
    }

    /// MP4 export is not compiled in.
    pub fn encode_mp4(_: &[Frame], _: &Mp4Options) -> Result<Vec<u8>> {
        unavailable()
    }

    /// MP4 export is not compiled in.
    pub fn write_mp4_file(_: &Path, _: &[Frame], _: &Mp4Options) -> Result<()> {
        unavailable()
    }
}
