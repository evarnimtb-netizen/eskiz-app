//! Export sequencing for the `eskiz-media` exporters.

use eskiz_media::{duration_for_fps, Frame};

use crate::frames::FrameList;
use crate::settings::{AnimationSettings, PlaybackDirection};

/// What the frames are flattened over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    /// Over white paper, so every frame is opaque. Used by MP4 and by GIF without transparency.
    #[default]
    White,
    /// Keep transparency where nothing is painted. Used by GIF and APNG with a transparent background.
    Transparent,
}

/// Sequences the animation frames into `eskiz-media` frames.
///
/// The export is one pass through the frames: in reverse order for [`PlaybackDirection::Reverse`],
/// otherwise forward. Each frame is written `hold + 1` times, each for `1 / export fps` seconds.
/// The playback mode does not change the export, so a
/// three-frame animation with no holds lasts `3 / fps` seconds.
///
/// Hidden and private items are never in a frame, and onion skins are never drawn. An empty
/// [`FrameList`] gives no frames.
pub fn export_frames(settings: &AnimationSettings, frames: &FrameList) -> Vec<Frame> {
    export_frames_with(settings, frames, Paper::White, &mut |_, _| true).unwrap_or_default()
}

/// [`export_frames`] with the paper chosen, and progress.
///
/// `progress(done, total)` is called before each frame is rendered, and once more at the end with
/// `done == total`. Returning `false` cancels the export, and the result is `None`.
pub fn export_frames_with(
    settings: &AnimationSettings,
    frames: &FrameList,
    paper: Paper,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Option<Vec<Frame>> {
    let n = frames.len();
    if n == 0 {
        return Some(Vec::new());
    }
    let (width, height) = frames.size();
    let duration = duration_for_fps(settings.export_fps());
    let order: Vec<usize> = match settings.direction {
        PlaybackDirection::Forward => (0..n).collect(),
        PlaybackDirection::Reverse => (0..n).rev().collect(),
    };

    let mut out = Vec::new();
    for (done, index) in order.into_iter().enumerate() {
        if !progress(done, n) {
            return None;
        }
        let pixels = match paper {
            Paper::White => frames.render(index),
            Paper::Transparent => frames.render_transparent(index),
        };
        for _ in 0..=frames.hold(index) {
            // The pixel count always matches the size, so this only fails on a zero-size canvas,
            // which FrameList::build rejects.
            if let Ok(frame) = Frame::new(width as u32, height as u32, pixels.clone(), duration) {
                out.push(frame);
            }
        }
    }
    progress(n, n);
    Some(out)
}
