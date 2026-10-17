//! MP4 muxing of H.264 access units (through the pure-Rust `mp4` crate), and Annex B / AVCC conversion.
//!
//! Encoded pictures arrive as Annex B byte streams with start codes. MP4 stores length-prefixed (AVCC)
//! NAL units, and keeps the SPS and PPS in the sample entry rather than in the samples.

use std::io::{Seek, Write};

use mp4::{
    AvcConfig, Bytes, FourCC, MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType,
};

use crate::error::{MediaError, Result};

/// Sample clock of the video track, in ticks per second.
pub(crate) const TRACK_TIMESCALE: u32 = 90_000;

/// Writes one H.264 video track. Samples are appended in order.
pub(crate) struct Muxer<W: Write + Seek> {
    writer: Mp4Writer<W>,
    next_start: u64,
}

impl<W: Write + Seek> Muxer<W> {
    /// Starts an MP4 file with one H.264 track of `width` x `height` (`sps` and `pps` are raw NAL units).
    pub(crate) fn begin(
        writer: W,
        width: u32,
        height: u32,
        sps: &[u8],
        pps: &[u8],
    ) -> Result<Self> {
        let width = u16::try_from(width)
            .map_err(|_| MediaError::InvalidSettings("width exceeds 65535".into()))?;
        let height = u16::try_from(height)
            .map_err(|_| MediaError::InvalidSettings("height exceeds 65535".into()))?;
        let config = Mp4Config {
            major_brand: FourCC::from(*b"isom"),
            minor_version: 512,
            compatible_brands: [*b"isom", *b"iso2", *b"avc1", *b"mp41"]
                .into_iter()
                .map(FourCC::from)
                .collect(),
            // Same clock as the track. The mp4 crate truncates each sample's duration into the movie
            // timescale, so a different movie clock drifts over long recordings (1/30 s samples
            // would total 29.7 s over 900 frames).
            timescale: TRACK_TIMESCALE,
        };
        let mut writer = Mp4Writer::write_start(writer, &config)?;
        writer.add_track(&TrackConfig {
            track_type: TrackType::Video,
            timescale: TRACK_TIMESCALE,
            language: "und".to_string(),
            media_conf: MediaConfig::AvcConfig(AvcConfig {
                width,
                height,
                seq_param_set: sps.to_vec(),
                pic_param_set: pps.to_vec(),
            }),
        })?;
        Ok(Self {
            writer,
            next_start: 0,
        })
    }

    /// Appends one access unit in AVCC form, lasting `duration` ticks.
    pub(crate) fn push(&mut self, avcc: &[u8], is_sync: bool, duration: u32) -> Result<()> {
        let sample = Mp4Sample {
            start_time: self.next_start,
            duration,
            rendering_offset: 0,
            is_sync,
            bytes: Bytes::copy_from_slice(avcc),
        };
        self.writer.write_sample(1, &sample)?;
        self.next_start += u64::from(duration);
        Ok(())
    }

    /// Finishes the file by writing the movie header.
    pub(crate) fn finish(mut self) -> Result<()> {
        self.writer.write_end()?;
        Ok(())
    }
}

/// Splits an Annex B byte stream into NAL units, without their start codes.
pub(crate) fn split_annex_b(stream: &[u8]) -> Vec<&[u8]> {
    let mut payload_starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            payload_starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(payload_starts.len());
    for (k, &begin) in payload_starts.iter().enumerate() {
        let mut end = payload_starts
            .get(k + 1)
            .map_or(stream.len(), |next| next - 3);
        // A four-byte start code leaves a zero byte at the end of the previous unit.
        while end > begin && stream[end - 1] == 0 {
            end -= 1;
        }
        if end > begin {
            nals.push(&stream[begin..end]);
        }
    }
    nals
}

/// One encoded picture ready for the MP4 sample table.
pub(crate) struct AccessUnit {
    /// Length-prefixed slices and IDR pictures. Empty when the picture only carried parameter sets.
    pub avcc: Vec<u8>,
    /// True when the picture contains an IDR slice.
    pub is_sync: bool,
    /// Sequence parameter set, when present in this picture.
    pub sps: Option<Vec<u8>>,
    /// Picture parameter set, when present in this picture.
    pub pps: Option<Vec<u8>>,
}

/// Converts one Annex B access unit to AVCC. SPS and PPS go to `sps` and `pps`. SEI and access unit
/// delimiters are dropped.
pub(crate) fn access_unit_from_annex_b(stream: &[u8]) -> AccessUnit {
    let mut unit = AccessUnit {
        avcc: Vec::new(),
        is_sync: false,
        sps: None,
        pps: None,
    };
    for nal in split_annex_b(stream) {
        match nal[0] & 0x1f {
            7 => {
                if unit.sps.is_none() {
                    unit.sps = Some(nal.to_vec());
                }
            }
            8 => {
                if unit.pps.is_none() {
                    unit.pps = Some(nal.to_vec());
                }
            }
            6 | 9 => {}
            kind => {
                if kind == 5 {
                    unit.is_sync = true;
                }
                unit.avcc
                    .extend_from_slice(&(nal.len() as u32).to_be_bytes());
                unit.avcc.extend_from_slice(nal);
            }
        }
    }
    unit
}

/// Appends an AVCC sample to `out` as Annex B, for a decoder.
pub(crate) fn avcc_to_annex_b(avcc: &[u8], out: &mut Vec<u8>) -> Result<()> {
    let mut rest = avcc;
    while rest.len() >= 4 {
        let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let body = rest
            .get(4..4 + len)
            .ok_or_else(|| MediaError::InvalidFrame("corrupt H.264 sample".into()))?;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(body);
        rest = &rest[4 + len..];
    }
    Ok(())
}

/// Appends one NAL unit to `out` with a four-byte start code.
pub(crate) fn push_annex_b_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}
