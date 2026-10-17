//! Frames settings. The types live in `eskiz-core` with the document, and are
//! re-exported here so animation code can use one path.

pub use eskiz_core::animation::{
    clamp_frame_rate, AnimationSettings, OnionBlend, PlaybackDirection, PlaybackMode, MAX_HOLD,
    MAX_ONION_COUNT, MIN_ONION_COUNT, NEXT_COLOUR, ONION_FALLOFF, PREVIOUS_COLOUR,
};
pub use eskiz_core::animation::{MAX_FPS, MIN_FPS};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_limits_match_the_media_exporters() {
        assert_eq!(MIN_FPS, eskiz_media::MIN_FPS);
        assert_eq!(MAX_FPS, eskiz_media::MAX_FPS);
    }
}
