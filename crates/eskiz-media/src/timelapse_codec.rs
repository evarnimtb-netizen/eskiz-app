//! Timelapse export paths that need the H.264 muxer and decoder (feature `h264`, enabled by `source`
//! and `libloading`).

use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use std::time::Duration;

use mp4::{Mp4Reader, TrackType};
use openh264::formats::YUVSource;

use crate::error::{MediaError, Result};
use crate::frame::{duration_for_fps, Frame};
use crate::fsutil::write_atomic;
use crate::h264::new_decoder;
use crate::mp4_export::{Mp4Encoder, Mp4Options};
use crate::mux::{avcc_to_annex_b, push_annex_b_nal, Muxer};
use crate::spec::{self, SUMMARY_FPS, SUMMARY_MAX_SECONDS};
use super::{Segment, TimelapseRecorder};

impl TimelapseRecorder {
    pub(crate) fn export_full_encoded(&mut self, path: &Path) -> Result<u64> {
        let first = self.segments.first().ok_or(MediaError::NoFrames)?;
        let (width, height) = (first.width, first.height);
        if self
            .segments
            .iter()
            .any(|s| (s.width, s.height) != (width, height))
        {
            return Err(MediaError::Unsupported(
                "segments differ in size; re-encoding them is not implemented".into(),
            ));
        }
        let segments = &self.segments;
        let mut written = 0u64;
        write_atomic(path, |file: &mut File| {
            let mut muxer = Muxer::begin(file, width, height, &segments[0].sps, &segments[0].pps)?;
            for segment in segments {
                let bytes = segment.read_bytes()?;
                let mut reader =
                    Mp4Reader::read_header(Cursor::new(&bytes[..]), bytes.len() as u64)?;
                let track = video_track(&reader)?;
                for sample_id in 1..=track.samples {
                    let sample = reader
                        .read_sample(track.id, sample_id)?
                        .ok_or_else(|| MediaError::InvalidFrame("segment ends early".into()))?;
                    muxer.push(&sample.bytes, sample.is_sync, sample.duration)?;
                    written += 1;
                }
            }
            muxer.finish()
        })?;
        Ok(written)
    }

    pub(crate) fn decode_preview(&self, max_frames: usize, max_side: u32) -> Result<Vec<Frame>> {
        let total: usize = self.segments.iter().map(|s| s.frames as usize).sum();
        if total == 0 {
            return Err(MediaError::NoFrames);
        }
        let step = total.div_ceil(max_frames.max(1)).max(1);
        let duration = duration_for_fps(self.settings.fps);
        let mut decoder = new_decoder()?;
        let mut out = Vec::new();
        let mut source = 0usize;
        let mut annex_b = Vec::new();
        for segment in &self.segments {
            let bytes = segment.read_bytes()?;
            let mut reader = Mp4Reader::read_header(Cursor::new(&bytes[..]), bytes.len() as u64)?;
            let track = video_track(&reader)?;
            annex_b.clear();
            push_annex_b_nal(&mut annex_b, &segment.sps);
            push_annex_b_nal(&mut annex_b, &segment.pps);
            decoder.decode(&annex_b)?;
            for sample_id in 1..=track.samples {
                let sample = reader
                    .read_sample(track.id, sample_id)?
                    .ok_or_else(|| MediaError::InvalidFrame("segment ends early".into()))?;
                annex_b.clear();
                avcc_to_annex_b(&sample.bytes, &mut annex_b)?;
                if let Some(yuv) = decoder.decode(&annex_b)? {
                    if source.is_multiple_of(step) {
                        out.push(preview_frame(&yuv, max_side, duration)?);
                    }
                    source += 1;
                }
            }
        }
        for yuv in decoder.flush_remaining()? {
            if source.is_multiple_of(step) {
                out.push(preview_frame(&yuv, max_side, duration)?);
            }
            source += 1;
        }
        Ok(out)
    }

    pub(crate) fn export_summary_encoded(&mut self, path: &Path) -> Result<()> {
        let total: u64 = self.segments.iter().map(|s| u64::from(s.frames)).sum();
        if total == 0 {
            return Err(MediaError::NoFrames);
        }
        let seconds: f64 = self.segments.iter().map(Segment::duration_seconds).sum();
        if seconds <= f64::from(SUMMARY_MAX_SECONDS) {
            return self.export_full(path).map(|_| ());
        }

        let plan = spec::summary_source_indices(total as usize);
        let (w, h) = self.capture;
        let options = Mp4Options {
            bitrate_bps: self.bitrate_bps(),
            background: self.settings.background,
        };
        let mut encoder = Mp4Encoder::new(w, h, &options, SUMMARY_FPS as f32)?;
        let output_duration = duration_for_fps(SUMMARY_FPS);
        let mut decoder = new_decoder()?;

        let mut next = 0usize;
        let mut source = 0usize;
        let mut annex_b = Vec::new();
        for segment in &self.segments {
            let bytes = segment.read_bytes()?;
            let mut reader = Mp4Reader::read_header(Cursor::new(&bytes[..]), bytes.len() as u64)?;
            let track = video_track(&reader)?;
            annex_b.clear();
            push_annex_b_nal(&mut annex_b, &segment.sps);
            push_annex_b_nal(&mut annex_b, &segment.pps);
            decoder.decode(&annex_b)?;
            for sample_id in 1..=track.samples {
                let sample = reader
                    .read_sample(track.id, sample_id)?
                    .ok_or_else(|| MediaError::InvalidFrame("segment ends early".into()))?;
                annex_b.clear();
                avcc_to_annex_b(&sample.bytes, &mut annex_b)?;
                if let Some(yuv) = decoder.decode(&annex_b)? {
                    emit_planned(
                        &mut encoder,
                        &plan,
                        &mut next,
                        source,
                        &yuv,
                        output_duration,
                    )?;
                    source += 1;
                }
            }
        }
        for yuv in decoder.flush_remaining()? {
            emit_planned(
                &mut encoder,
                &plan,
                &mut next,
                source,
                &yuv,
                output_duration,
            )?;
            source += 1;
        }
        if next != plan.len() {
            return Err(MediaError::InvalidFrame(format!(
                "decoder produced {source} frames, the summary needs the rest of the plan"
            )));
        }
        write_atomic(path, |file: &mut File| encoder.finish(file))
    }
}


/// One decoded picture as an opaque RGBA frame, scaled down so its longer side is at most `max_side`.
fn preview_frame(yuv: &openh264::decoder::DecodedYUV<'_>, max_side: u32, duration: Duration) -> Result<Frame> {
    let (w, h) = yuv.dimensions();
    let mut rgb = vec![0u8; w * h * 3];
    yuv.write_rgb8(&mut rgb);
    let rgba: Vec<u8> = rgb.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect();
    let frame = Frame::new(w as u32, h as u32, rgba, duration)?;
    let longest = w.max(h) as u32;
    if longest <= max_side.max(2) {
        return Ok(frame);
    }
    let scale = f64::from(max_side.max(2)) / f64::from(longest);
    let pw = ((w as f64 * scale) as u32).max(2) & !1;
    let ph = ((h as f64 * scale) as u32).max(2) & !1;
    frame.resize_box(pw, ph, duration)
}

/// Encodes every summary output that shows decoded source frame `source`.
fn emit_planned<Y: YUVSource>(
    encoder: &mut Mp4Encoder,
    plan: &[usize],
    next: &mut usize,
    source: usize,
    picture: &Y,
    duration: Duration,
) -> Result<()> {
    while *next < plan.len() && plan[*next] == source {
        encoder.push_yuv(picture, duration)?;
        *next += 1;
    }
    Ok(())
}

struct VideoTrack {
    id: u32,
    samples: u32,
}

fn video_track<R: Read + Seek>(reader: &Mp4Reader<R>) -> Result<VideoTrack> {
    reader
        .tracks()
        .values()
        .find(|t| matches!(t.track_type(), Ok(TrackType::Video)))
        .map(|t| VideoTrack {
            id: t.track_id(),
            samples: t.sample_count(),
        })
        .ok_or_else(|| MediaError::InvalidFrame("segment has no video track".into()))
}
