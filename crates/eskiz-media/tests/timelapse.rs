//! Timelapse: capture sizing, bitrate and segment arithmetic, the summary frame plan, and an end-to-end
//! record / roll / export run.

use std::time::Duration;

use eskiz_media::{
    bitrate_bps, capture_size, frames_per_segment, summary_source_indices, Frame, MediaError,
    Quality, SegmentStore, TimelapseRecorder, TimelapseSettings,
};
#[cfg(feature = "h264")]
use eskiz_media::Resolution;

#[test]
fn capture_size_fits_the_box_and_keeps_aspect() {
    // Landscape 4000 x 3000 into 1920 x 1080: scale 0.36.
    assert_eq!(capture_size(4000, 3000, 1920, 1080).unwrap(), (1440, 1080));
    // Small documents are multiplied by 4 first: 200 x 100 becomes 800 x 400.
    assert_eq!(capture_size(200, 100, 1920, 1080).unwrap(), (800, 400));
    // Never enlarged beyond the box, and sides are even.
    assert_eq!(capture_size(2001, 1001, 1920, 1080).unwrap(), (1920, 960));
}

#[test]
fn capture_size_rejects_tiny_documents() {
    let err = capture_size(30, 30, 1920, 1080).unwrap_err();
    assert!(
        matches!(err, MediaError::DocumentTooSmall { .. }),
        "got {err}"
    );
}

#[test]
fn bitrate_matches_the_spec_examples() {
    // 1080p Standard is about 7.9 Mbit/s; 4K Studio is about 40 Mbit/s.
    let standard = bitrate_bps(1920, 1080, Quality::Standard);
    assert!((7_880_000..=7_960_000).contains(&standard), "{standard}");
    let studio_4k = bitrate_bps(3840, 2160, Quality::Studio);
    assert!(
        (40_200_000..=40_400_000).contains(&studio_4k),
        "{studio_4k}"
    );
}

#[test]
fn segment_lengths_follow_quality() {
    // Frames = fps x segment seconds (Low 60 s, Standard 8 s, High and Studio 2 s).
    assert_eq!(frames_per_segment(30, Quality::Low), 1800);
    assert_eq!(frames_per_segment(30, Quality::Standard), 240);
    assert_eq!(frames_per_segment(30, Quality::High), 60);
    assert_eq!(frames_per_segment(30, Quality::Studio), 60);
    assert_eq!(frames_per_segment(1, Quality::Low), 60);
}

#[test]
fn summary_plan_has_900_monotonic_frames_with_the_spec_landmarks() {
    let n = 5000;
    let plan = summary_source_indices(n);
    assert_eq!(plan.len(), 900);
    assert!(
        plan.windows(2).all(|w| w[0] <= w[1]),
        "source order never goes back"
    );
    assert!(plan.iter().all(|&s| s < n));
    // Outputs 0 to 49 read the source in order.
    assert_eq!(&plan[..50], &(0..50).collect::<Vec<_>>()[..]);
    // Output 50 is source 50. Output 880 is source N - 21.
    assert_eq!(plan[50], 50);
    assert_eq!(plan[880], n - 21);
    // Outputs 881 to 899 continue in order from there: N - 20 to N - 2.
    assert_eq!(plan[881], n - 20);
    assert_eq!(plan[899], n - 2);
}

#[test]
fn summary_plan_clamps_short_recordings() {
    let plan = summary_source_indices(31);
    assert_eq!(plan.len(), 900);
    assert!(
        plan.iter().all(|&s| s < 31),
        "no index past the last source frame"
    );
    assert!(
        plan.windows(2).all(|w| w[0] <= w[1]),
        "still in source order"
    );
    assert!(summary_source_indices(0).is_empty());
}

#[cfg(feature = "h264")]
fn commit_frame(width: u32, height: u32, step: u32) -> Frame {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for (i, px) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let v = ((i as u32 + step * 7) % 251) as u8;
        *px = [v, 255 - v, 128, 255];
    }
    Frame::new(width, height, pixels, Duration::from_millis(16)).unwrap()
}

#[cfg(feature = "h264")]
fn low_fps_settings(fps: u32) -> TimelapseSettings {
    TimelapseSettings {
        fps,
        quality: Quality::Low,
        resolution: Resolution::P1080,
        background: [255, 255, 255],
    }
}

#[cfg(feature = "h264")]
#[test]
fn recorder_counts_segments_and_rolls_at_the_frame_limit() {
    // 300 x 128 is used as is (no x4 for sides over 256), so frames stay small and fast to encode.
    let mut recorder =
        TimelapseRecorder::new(300, 128, low_fps_settings(1), SegmentStore::Memory).unwrap();
    assert_eq!(recorder.capture_size(), (300, 128));

    for step in 0..130 {
        recorder.commit(&commit_frame(300, 128, step)).unwrap();
    }
    // Low at 1 fps is 60 frames per segment: two rolled segments, 10 frames still open.
    assert_eq!(recorder.segments().len(), 2);
    assert_eq!(recorder.recorded_frames(), 130);

    recorder.finish_segment().unwrap();
    let frames: Vec<u32> = recorder.segments().iter().map(|s| s.frames).collect();
    assert_eq!(frames, vec![60, 60, 10]);
    assert_eq!(
        recorder
            .segments()
            .iter()
            .map(|s| s.index)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[cfg(feature = "h264")]
#[test]
fn export_full_writes_every_segment_into_one_mp4() {
    let mut recorder =
        TimelapseRecorder::new(300, 128, low_fps_settings(1), SegmentStore::Memory).unwrap();
    for step in 0..70 {
        recorder.commit(&commit_frame(300, 128, step)).unwrap();
    }
    let dir = std::env::temp_dir().join(format!("eskiz-media-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("full.mp4");

    match recorder.export_full(&path) {
        Ok(written) => assert_eq!(written, 70),
        Err(MediaError::EncoderUnavailable(reason)) => {
            eprintln!("SKIPPED timelapse export: OpenH264 cannot initialize: {reason}");
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        Err(other) => panic!("export failed: {other}"),
    }

    let bytes = std::fs::read(&path).unwrap();
    let size = bytes.len() as u64;
    let reader = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).unwrap();
    let track = reader.tracks().values().next().unwrap();
    assert_eq!(track.sample_count(), 70);
    // 70 frames at 1 fps lasts 70 seconds.
    assert!((reader.duration().as_secs_f64() - 70.0).abs() < 0.05);
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(feature = "h264")]
#[test]
fn summary_of_a_long_recording_is_thirty_seconds() {
    // 31 frames at 1 fps is 31 seconds, which is over the 30 s limit, so the summary re-encodes 900 frames.
    let mut recorder =
        TimelapseRecorder::new(300, 128, low_fps_settings(1), SegmentStore::Memory).unwrap();
    for step in 0..31 {
        recorder.commit(&commit_frame(300, 128, step)).unwrap();
    }
    let dir = std::env::temp_dir().join(format!("eskiz-media-summary-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("summary.mp4");

    match recorder.export_summary(&path) {
        Ok(()) => {}
        Err(MediaError::EncoderUnavailable(reason)) => {
            eprintln!("SKIPPED summary test: OpenH264 cannot initialize: {reason}");
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        Err(other) => panic!("summary failed: {other}"),
    }

    let bytes = std::fs::read(&path).unwrap();
    let size = bytes.len() as u64;
    let reader = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), size).unwrap();
    let track = reader.tracks().values().next().unwrap();
    assert_eq!(track.sample_count(), 900);
    assert!((reader.duration().as_secs_f64() - 30.0).abs() < 0.05);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn recording_without_an_encoder_keeps_frames_and_export_reports_it() {
    eskiz_media::force_encoder_unavailable(true);
    let settings = TimelapseSettings::default();
    let mut rec = TimelapseRecorder::new(400, 300, settings, SegmentStore::Memory).unwrap();
    for i in 0..300u32 {
        let px = vec![(i % 250) as u8; 400 * 300 * 4];
        let frame = Frame::new(400, 300, px, Duration::from_millis(33)).unwrap();
        rec.commit(&frame).expect("committing must not need the encoder");
    }
    assert_eq!(rec.recorded_frames(), 300);
    assert!(rec.is_unencoded());
    let preview = rec.preview_frames(50, 200).unwrap();
    assert!(!preview.is_empty() && preview.len() <= 50);
    let dir = std::env::temp_dir().join(format!("eskiz-noenc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let err = rec.export_full(&dir.join("a.mp4")).unwrap_err();
    assert!(matches!(err, MediaError::EncoderUnavailable(_)), "{err}");
    let err = rec.export_summary(&dir.join("b.mp4")).unwrap_err();
    assert!(matches!(err, MediaError::EncoderUnavailable(_)), "{err}");
    eskiz_media::force_encoder_unavailable(false);
}
