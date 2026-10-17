//! Timelapse recording.
//!
//! [`TimelapseRecorder::commit`] takes one document composite per committed change. Each frame is
//! downsampled into the capture size and encoded at `1 / fps` seconds, so frame n sits at n / fps regardless
//! of wall-clock time. When a segment reaches its frame count it is finalised and a new one starts. Segments
//! are kept in memory or as files in a directory.
//!
//! Exports:
//! - [`TimelapseRecorder::export_full`] copies every segment's encoded samples into one MP4. No re-encoding,
//!   and no dissolve between segments (a gap, see the crate README).
//! - [`TimelapseRecorder::export_summary`] writes a 30 s summary. Recordings of 30 s or less export as full.
//!   Longer ones are decoded and re-encoded, picking frames by the documented time warp (900 frames at 30 fps).

use std::borrow::Cow;
use std::fs::{self, File};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::error::{MediaError, Result};
use crate::frame::{duration_for_fps, Frame};
use crate::fsutil::write_atomic;
use crate::mp4_export::{Mp4Encoder, Mp4Options};
use crate::spec::{self, Quality, Resolution};

/// Timelapse recording settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelapseSettings {
    /// Frames per second, 1 to 60. Changes apply to new segments only.
    pub fps: u32,
    /// Quality level. Sets the segment length and bitrate.
    pub quality: Quality,
    /// Resolution box for the capture size.
    pub resolution: Resolution,
    /// Colour that fills transparency and padding.
    pub background: [u8; 3],
}

impl Default for TimelapseSettings {
    fn default() -> Self {
        Self {
            fps: 30,
            quality: Quality::Standard,
            resolution: Resolution::P1080,
            background: [255, 255, 255],
        }
    }
}

/// Where completed segments are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentStore {
    /// Keep MP4 bytes in memory. Suitable for tests and short sessions.
    Memory,
    /// Write `segment_NNNN.mp4` files into this directory, which is created if it is missing.
    Directory(PathBuf),
}

/// Where a completed segment's MP4 bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentData {
    /// In memory.
    Memory(Vec<u8>),
    /// In a file.
    File(PathBuf),
}

/// A completed segment and its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 1-based index in recording order.
    pub index: u32,
    /// Capture width in pixels.
    pub width: u32,
    /// Capture height in pixels.
    pub height: u32,
    /// Frames in the segment.
    pub frames: u32,
    /// Frame rate the segment was recorded at.
    pub fps: u32,
    /// Target bitrate in bits per second.
    pub bitrate_bps: u32,
    /// Quality level.
    pub quality: Quality,
    /// Storage location.
    pub data: SegmentData,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

impl Segment {
    /// Playing time of the segment, in seconds.
    pub fn duration_seconds(&self) -> f64 {
        f64::from(self.frames) / f64::from(self.fps)
    }

    /// The segment's MP4 bytes. Borrowed for memory segments, read from disk for file segments.
    pub fn read_bytes(&self) -> Result<Cow<'_, [u8]>> {
        match &self.data {
            SegmentData::Memory(bytes) => Ok(Cow::Borrowed(bytes.as_slice())),
            SegmentData::File(path) => Ok(Cow::Owned(fs::read(path)?)),
        }
    }
}

/// An open segment: the encoder plus how many frames it has taken.
struct ActiveSegment {
    encoder: Mp4Encoder,
    frames: u32,
    frames_limit: u32,
    fps: u32,
    quality: Quality,
    bitrate_bps: u32,
}

/// Records committed canvas frames as timelapse segments.
pub struct TimelapseRecorder {
    doc: (u32, u32),
    settings: TimelapseSettings,
    capture: (u32, u32),
    store: SegmentStore,
    active: Option<ActiveSegment>,
    segments: Vec<Segment>,
    /// Small frames kept when no H.264 encoder is available (see [`RAW_SIDE`]).
    raw: Vec<Frame>,
    /// Source frames skipped between two kept raw frames; doubles each time the buffer is thinned.
    raw_step: u32,
    raw_seen: u32,
}

/// Longest side of the frames kept when the H.264 encoder is unavailable, and the most kept.
const RAW_SIDE: u32 = 360;
const RAW_MAX_FRAMES: usize = 240;

const NO_ENCODER: &str = crate::h264::MP4_UNAVAILABLE;

// The export and replay paths that mux or decode H.264.
#[cfg(feature = "h264")]
#[path = "timelapse_codec.rs"]
mod codec;

/// Without an encoder no segment is ever stored, so these are not reachable; they report the
/// same message as the guards before them.
#[cfg(not(feature = "h264"))]
impl TimelapseRecorder {
    fn export_full_encoded(&mut self, _path: &Path) -> Result<u64> {
        Err(MediaError::EncoderUnavailable(NO_ENCODER.into()))
    }

    fn decode_preview(&self, _max_frames: usize, _max_side: u32) -> Result<Vec<Frame>> {
        Err(MediaError::EncoderUnavailable(NO_ENCODER.into()))
    }

    fn export_summary_encoded(&mut self, _path: &Path) -> Result<()> {
        Err(MediaError::EncoderUnavailable(NO_ENCODER.into()))
    }
}

impl TimelapseRecorder {
    /// Starts a recorder for a document of `doc_width` x `doc_height` (already oriented).
    ///
    /// Fails with [`MediaError::DocumentTooSmall`] when the capture would be under 128 px a side.
    pub fn new(
        doc_width: u32,
        doc_height: u32,
        mut settings: TimelapseSettings,
        store: SegmentStore,
    ) -> Result<Self> {
        settings.fps = settings.fps.clamp(spec::MIN_FPS, spec::MAX_FPS);
        let (box_w, box_h) = spec::oriented_box(settings.resolution, doc_width, doc_height);
        let capture = spec::capture_size(doc_width, doc_height, box_w, box_h)?;
        if let SegmentStore::Directory(dir) = &store {
            fs::create_dir_all(dir)?;
        }
        Ok(Self {
            doc: (doc_width, doc_height),
            settings,
            capture,
            store,
            active: None,
            segments: Vec::new(),
            raw: Vec::new(),
            raw_step: 1,
            raw_seen: 0,
        })
    }

    /// Current settings.
    pub fn settings(&self) -> TimelapseSettings {
        self.settings
    }

    /// Capture size: the frames are downsampled to this.
    pub fn capture_size(&self) -> (u32, u32) {
        self.capture
    }

    /// Bitrate used for new segments.
    pub fn bitrate_bps(&self) -> u32 {
        spec::bitrate_bps(self.capture.0, self.capture.1, self.settings.quality)
    }

    /// Changes the frame rate. The change applies to new segments only.
    pub fn set_fps(&mut self, fps: u32) {
        self.settings.fps = fps.clamp(spec::MIN_FPS, spec::MAX_FPS);
    }

    /// Completed segments in recording order.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Frames recorded so far, including the open segment.
    pub fn recorded_frames(&self) -> u64 {
        let completed: u64 = self.segments.iter().map(|s| u64::from(s.frames)).sum();
        completed + self.active.as_ref().map_or(0, |a| u64::from(a.frames)) + self.raw_seen as u64
    }

    /// Whether the frames are kept as small pictures because the H.264 encoder is unavailable.
    pub fn is_unencoded(&self) -> bool {
        self.raw_seen > 0 && self.segments.is_empty() && self.active.is_none()
    }

    /// Keeps a small copy of `frame` without the encoder; thins the buffer when it is full.
    fn commit_raw(&mut self, frame: &Frame) -> Result<()> {
        if self.raw_seen.is_multiple_of(self.raw_step) {
            let (w, h) = (frame.width(), frame.height());
            let scale = (RAW_SIDE as f32 / w.max(h) as f32).min(1.0);
            let (sw, sh) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
            self.raw.push(frame.resize_box(sw, sh, duration_for_fps(self.settings.fps))?);
            if self.raw.len() > RAW_MAX_FRAMES {
                let mut keep = false;
                self.raw.retain(|_| {
                    keep = !keep;
                    keep
                });
                self.raw_step *= 2;
            }
        }
        self.raw_seen += 1;
        Ok(())
    }

    /// Records one commit. `frame` is the document composite at document size.
    ///
    /// When the open segment reaches its frame count, it is finalised here (roll, no frame dropped).
    pub fn commit(&mut self, frame: &Frame) -> Result<()> {
        if (frame.width(), frame.height()) != self.doc {
            return Err(MediaError::InvalidFrame(format!(
                "frame is {} x {}, the document is {} x {}",
                frame.width(),
                frame.height(),
                self.doc.0,
                self.doc.1
            )));
        }
        if self.raw_seen > 0 {
            return self.commit_raw(frame);
        }
        let mut active = match self.active.take() {
            Some(active) => active,
            None => match self.start_segment() {
                Ok(active) => active,
                // No encoder: keep recording, and report it when the user exports an MP4.
                Err(MediaError::EncoderUnavailable(_)) if self.segments.is_empty() => {
                    return self.commit_raw(frame);
                }
                Err(e) => return Err(e),
            },
        };
        let small =
            frame.resize_box(self.capture.0, self.capture.1, duration_for_fps(active.fps))?;
        active.encoder.push_frame(&small)?;
        active.frames += 1;
        if active.frames >= active.frames_limit {
            self.store_segment(active)?;
        } else {
            self.active = Some(active);
        }
        Ok(())
    }

    /// Finalises the open segment, if any. The next commit starts a new one.
    pub fn finish_segment(&mut self) -> Result<()> {
        if let Some(active) = self.active.take() {
            if active.frames > 0 {
                self.store_segment(active)?;
            }
        }
        Ok(())
    }

    fn start_segment(&self) -> Result<ActiveSegment> {
        let (w, h) = self.capture;
        let bitrate_bps = self.bitrate_bps();
        let options = Mp4Options {
            bitrate_bps,
            background: self.settings.background,
        };
        let fps = self.settings.fps;
        let quality = self.settings.quality;
        Ok(ActiveSegment {
            encoder: Mp4Encoder::new(w, h, &options, fps as f32)?,
            frames: 0,
            frames_limit: spec::frames_per_segment(fps, quality),
            fps,
            quality,
            bitrate_bps,
        })
    }

    fn store_segment(&mut self, active: ActiveSegment) -> Result<()> {
        let (sps, pps) = active.encoder.parameter_sets().ok_or_else(|| {
            MediaError::InvalidFrame("the encoder produced no parameter sets".into())
        })?;
        let index = self.segments.len() as u32 + 1;
        let data = match &self.store {
            SegmentStore::Memory => {
                let mut out = Cursor::new(Vec::new());
                active.encoder.finish(&mut out)?;
                SegmentData::Memory(out.into_inner())
            }
            SegmentStore::Directory(dir) => {
                let path = dir.join(format!("segment_{index:04}.mp4"));
                write_atomic(&path, |file: &mut File| active.encoder.finish(file))?;
                SegmentData::File(path)
            }
        };
        let (width, height) = self.capture;
        self.segments.push(Segment {
            index,
            width,
            height,
            frames: active.frames,
            fps: active.fps,
            bitrate_bps: active.bitrate_bps,
            quality: active.quality,
            data,
            sps,
            pps,
        });
        Ok(())
    }

    /// Writes the full recording to one MP4 and returns the frame count. Finalises the open segment first.
    ///
    /// Segments are copied sample for sample. All segments must share one size, because a single MP4 track
    /// has one size. Re-encoding a mixed-size recording is not implemented.
    pub fn export_full(&mut self, path: &Path) -> Result<u64> {
        self.finish_segment()?;
        if self.is_unencoded() {
            return Err(MediaError::EncoderUnavailable(NO_ENCODER.into()));
        }
        self.export_full_encoded(path)
    }

    /// Decodes the recording for an in-app replay: at most `max_frames` opaque frames, taken evenly from
    /// the recording in order, each scaled to fit a `max_side` box. Decodes every segment, so it is slow on
    /// long recordings and should run off the UI thread.
    pub fn preview_frames(&self, max_frames: usize, max_side: u32) -> Result<Vec<Frame>> {
        if self.is_unencoded() {
            let step = self.raw.len().div_ceil(max_frames.max(1)).max(1);
            return self
                .raw
                .iter()
                .step_by(step)
                .map(|f| {
                    let scale = (max_side as f32 / f.width().max(f.height()) as f32).min(1.0);
                    let (w, h) = (((f.width() as f32 * scale) as u32).max(1), ((f.height() as f32 * scale) as u32).max(1));
                    f.resize_box(w, h, f.duration())
                })
                .collect();
        }
        self.decode_preview(max_frames, max_side)
    }

    /// Writes a summary MP4. Recordings of 30 s or less export as full-length.
    ///
    /// Longer recordings decode every segment and re-encode 900 frames at 30 fps, choosing sources with
    /// [`spec::summary_source_indices`]. Finalises the open segment first.
    pub fn export_summary(&mut self, path: &Path) -> Result<()> {
        self.finish_segment()?;
        if self.is_unencoded() {
            return Err(MediaError::EncoderUnavailable(NO_ENCODER.into()));
        }
        self.export_summary_encoded(path)
    }
}
