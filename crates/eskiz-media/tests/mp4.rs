//! MP4 export: encode a moving square, then parse the result with the `mp4` crate.
//! Skips with a message when OpenH264 cannot initialise on the platform. Needs the `source` or
//! `libloading` feature; `mp4_reports_unavailable_without_an_encoder` covers the default build.

use std::time::Duration;

use eskiz_media::{encode_mp4, Frame, MediaError, Mp4Options};
#[cfg(feature = "h264")]
use std::io::Cursor;

/// A `width` x `height` frame with an opaque square at column `x`.
#[cfg(feature = "h264")]
fn moving_square(width: u32, height: u32, x: u32, duration: Duration) -> Frame {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for px in pixels.as_chunks_mut::<4>().0 {
        *px = [230, 230, 230, 255];
    }
    for y in 10..20.min(height) {
        for col in x..(x + 12).min(width) {
            let i = ((y * width + col) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[20, 90, 200, 255]);
        }
    }
    Frame::new(width, height, pixels, duration).unwrap()
}

/// Encodes, or returns `None` after printing a skip message if OpenH264 is unavailable.
#[cfg(feature = "h264")]
fn encode_or_skip(frames: &[Frame]) -> Option<Vec<u8>> {
    match encode_mp4(frames, &Mp4Options::default()) {
        Ok(bytes) => Some(bytes),
        Err(MediaError::EncoderUnavailable(reason)) => {
            eprintln!("SKIPPED mp4 test: OpenH264 cannot initialize on this platform: {reason}");
            None
        }
        Err(other) => panic!("encode failed: {other}"),
    }
}

#[cfg(feature = "h264")]
#[test]
fn mp4_ten_frames_moving_square() {
    let frame_duration = Duration::from_secs_f64(1.0 / 30.0);
    let frames: Vec<Frame> = (0..10)
        .map(|i| moving_square(64, 48, 4 + i * 4, frame_duration))
        .collect();
    let Some(bytes) = encode_or_skip(&frames) else {
        return;
    };

    assert_eq!(&bytes[4..8], b"ftyp", "file starts with an ftyp box");
    let size = bytes.len() as u64;
    let reader = mp4::Mp4Reader::read_header(Cursor::new(bytes), size).unwrap();

    assert_eq!(reader.tracks().len(), 1, "one track");
    let track = reader.tracks().values().next().unwrap();
    assert!(
        matches!(track.track_type(), Ok(mp4::TrackType::Video)),
        "the track is video"
    );
    assert_eq!((track.width(), track.height()), (64, 48));
    assert_eq!(track.sample_count(), 10, "one sample per frame");

    let seconds = reader.duration().as_secs_f64();
    assert!(
        (seconds - 10.0 / 30.0).abs() < 0.02,
        "duration {seconds} s, expected 0.333 s"
    );
}

#[cfg(feature = "h264")]
#[test]
fn mp4_pads_odd_sizes_to_even() {
    let frames: Vec<Frame> = (0..3)
        .map(|i| moving_square(33, 17, i, Duration::from_millis(33)))
        .collect();
    let Some(bytes) = encode_or_skip(&frames) else {
        return;
    };

    let size = bytes.len() as u64;
    let reader = mp4::Mp4Reader::read_header(Cursor::new(bytes), size).unwrap();
    let track = reader.tracks().values().next().unwrap();
    assert_eq!(
        (track.width(), track.height()),
        (34, 18),
        "odd sides are padded up to even"
    );
}

#[cfg(not(feature = "h264"))]
#[test]
fn mp4_reports_unavailable_without_an_encoder() {
    assert!(!eskiz_media::encoder_available());
    let frame = Frame::new(16, 16, vec![255; 16 * 16 * 4], Duration::from_millis(40)).unwrap();
    let err = encode_mp4(&[frame], &Mp4Options::default()).unwrap_err();
    assert!(
        matches!(&err, MediaError::EncoderUnavailable(m) if m == eskiz_media::MP4_UNAVAILABLE),
        "{err}"
    );
}
