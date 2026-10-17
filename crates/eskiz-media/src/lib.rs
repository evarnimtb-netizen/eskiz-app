//! Eskiz media export: animated GIF, APNG, H.264 in MP4 (OpenH264) and timelapse recording.
//!
//! Every exporter takes frames as premultiplied RGBA8 ([`Frame`]) with a per-frame duration. Files are
//! written atomically, through a temporary file that is renamed into place.
//!
//! Cargo features choose how H.264 is provided. With neither, no OpenH264 or MP4 muxer code is linked and
//! MP4 paths report [`MP4_UNAVAILABLE`]; GIF, APNG and the raw timelapse still work.
//! - `source`: OpenH264 is compiled from source (BSD-2-Clause).
//! - `libloading`: OpenH264 is loaded at runtime from the path in `ESKIZ_OPENH264_LIB`, for example Cisco's
//!   prebuilt binary. See the crate README.

#![forbid(unsafe_code)]

mod apng;
mod error;
mod frame;
mod fsutil;
mod gif_export;
#[cfg(feature = "h264")]
mod h264;
#[cfg(not(feature = "h264"))]
#[path = "h264_off.rs"]
mod h264;
mod mp4_export;
#[cfg(feature = "h264")]
mod mux;
mod spec;
mod timelapse;

pub use apng::{apng_delay, encode_apng, write_apng_file, ApngOptions};
pub use error::{MediaError, Result};
pub use frame::{duration_for_fps, Frame};
pub use gif_export::{encode_gif, gif_delay_centiseconds, write_gif_file, GifOptions, PaletteMode};
pub use h264::{encoder_available, force_encoder_unavailable, LIBRARY_ENV, MP4_UNAVAILABLE};
pub use mp4_export::{encode_mp4, write_mp4_file, Mp4Options};
pub use spec::{
    bitrate_bps, capture_size, frames_per_segment, oriented_box, summary_source_indices, Quality,
    Resolution, MAX_FPS, MIN_FPS, SUMMARY_FPS, SUMMARY_FRAMES,
};
pub use timelapse::{Segment, SegmentData, SegmentStore, TimelapseRecorder, TimelapseSettings};
