//! Numbers and formulas for timelapse and export. Pure functions, no I/O.

use crate::error::{MediaError, Result};

/// Frame rate limits for timelapse and animation export.
pub const MIN_FPS: u32 = 1;
/// Frame rate limits for timelapse and animation export.
pub const MAX_FPS: u32 = 60;
/// Smallest side allowed for a timelapse capture.
pub const MIN_CAPTURE_SIDE: u32 = 128;
/// Number of output frames in a timelapse summary.
pub const SUMMARY_FRAMES: usize = 900;
/// Frame rate of the summary and of its output.
pub const SUMMARY_FPS: u32 = 30;
/// Summaries of recordings this long or shorter are the full-length recording.
#[cfg_attr(not(feature = "h264"), allow(dead_code))]
pub const SUMMARY_MAX_SECONDS: u32 = 30;

/// Timelapse quality level. Sets the segment length and the bitrate constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// Level 0. Minutes-long segments, lowest bitrate.
    Low,
    /// Level 1, the default.
    #[default]
    Standard,
    /// Level 2.
    High,
    /// Level 3. Highest bitrate.
    Studio,
}

impl Quality {
    /// Bitrate constant k, in kilobits per second per square root of pixels.
    pub fn bitrate_k(self) -> f64 {
        match self {
            Quality::Low => 3.0,
            Quality::Standard => 5.5,
            Quality::High => 9.0,
            Quality::Studio => 14.0,
        }
    }

    /// Segment length in seconds. The frame count is the frame rate times this.
    pub fn segment_seconds(self) -> u32 {
        match self {
            Quality::Low => 60,
            Quality::Standard => 8,
            Quality::High | Quality::Studio => 2,
        }
    }
}

/// Timelapse resolution box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    /// 1920 x 1080, the default.
    #[default]
    P1080,
    /// 2048 x 2048.
    K2,
    /// 3840 x 2160. Eskiz uses this box without a 4096 x 4096 fallback.
    K4,
}

impl Resolution {
    /// The box in landscape orientation.
    pub fn landscape_box(self) -> (u32, u32) {
        match self {
            Resolution::P1080 => (1920, 1080),
            Resolution::K2 => (2048, 2048),
            Resolution::K4 => (3840, 2160),
        }
    }
}

/// H.264 bitrate in bits per second: sqrt(width x height) x k.
///
/// For 1920 x 1080 at Standard this is about 7.9 Mbit/s. For 3840 x 2160 at Studio it is about 40 Mbit/s.
pub fn bitrate_bps(width: u32, height: u32, quality: Quality) -> u32 {
    let pixels = f64::from(width) * f64::from(height);
    (pixels.sqrt() * quality.bitrate_k() * 1000.0).round() as u32
}

/// Frames in one timelapse segment: the frame rate times the segment length.
pub fn frames_per_segment(fps: u32, quality: Quality) -> u32 {
    fps * quality.segment_seconds()
}

/// The resolution box for a document, rotated to the document's orientation (Eskiz choice.
pub fn oriented_box(resolution: Resolution, doc_width: u32, doc_height: u32) -> (u32, u32) {
    let (w, h) = resolution.landscape_box();
    if doc_height > doc_width {
        (h, w)
    } else {
        (w, h)
    }
}

/// Capture size for a document of `doc_width` x `doc_height`, inside the box `box_w` x `box_h`.
///
/// 1. A document whose longer side is 256 px or less is multiplied by 4 on both sides.
/// 2. The result is fitted inside the box, keeping aspect. It is never enlarged beyond the box.
/// 3. Each side is rounded to an even number and clamped to 128 px up to the box side.
///
/// A document whose scaled sides are under 128 px is rejected. That check uses the sides after
/// step 1, before the fit, because step 3 clamps the result up to 128 px.
pub fn capture_size(doc_width: u32, doc_height: u32, box_w: u32, box_h: u32) -> Result<(u32, u32)> {
    if doc_width == 0 || doc_height == 0 {
        return Err(MediaError::InvalidSettings(format!(
            "zero document size {doc_width} x {doc_height}"
        )));
    }
    let (mut w, mut h) = (f64::from(doc_width), f64::from(doc_height));
    if doc_width.max(doc_height) <= 256 {
        w *= 4.0;
        h *= 4.0;
    }
    if w < f64::from(MIN_CAPTURE_SIDE) || h < f64::from(MIN_CAPTURE_SIDE) {
        return Err(MediaError::DocumentTooSmall {
            width: doc_width,
            height: doc_height,
        });
    }
    let scale = (f64::from(box_w) / w).min(f64::from(box_h) / h).min(1.0);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    let cw = even(w * scale).clamp(MIN_CAPTURE_SIDE, box_w.max(MIN_CAPTURE_SIDE));
    let ch = even(h * scale).clamp(MIN_CAPTURE_SIDE, box_h.max(MIN_CAPTURE_SIDE));
    Ok((cw, ch))
}

/// For each of the 900 summary output frames, the index of the source frame it shows.
///
/// `total` is the number of source frames N. The caller decides whether a summary is needed at all; this
/// function always returns 900 entries for `total > 0`. Indices never exceed `total - 1`, so short
/// recordings (where the formula would step past the end) clamp to the last frame.
///
/// Output 0 to 49 read the source in order. Outputs 50 to 880 follow the time warp
/// `f = (1 - t^1.5)^(2/3)` with `t = (i - 50) / 830`, showing source `50 + floor((1 - f) * (N - 71))`.
/// Outputs 881 to 899 continue in order from source frame N - 21, so they show N - 20 to N - 2.
pub fn summary_source_indices(total: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    let last = total - 1;
    let mut out: Vec<usize> = Vec::with_capacity(SUMMARY_FRAMES);
    for i in 0..SUMMARY_FRAMES {
        let source = if i < 50 {
            i.min(last)
        } else if i <= 880 {
            let t = (i - 50) as f64 / 830.0;
            let f = (1.0 - t.powf(1.5)).powf(2.0 / 3.0);
            // For N under 72 the span would be negative and the warp would run backwards. Eskiz holds
            // the warp at source frame 50 instead (a choice; the design notes do not cover this case).
            let offset = ((1.0 - f) * (total as f64 - 71.0).max(0.0)).floor();
            (50.0 + offset).clamp(0.0, last as f64) as usize
        } else {
            (out[880] + (i - 880)).min(last)
        };
        out.push(source);
    }
    out
}
