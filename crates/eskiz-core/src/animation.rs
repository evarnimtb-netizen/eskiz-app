//! Frames settings and their defaults.
//!
//! The settings live on [`crate::Document`], so they travel with the document and the `.procreate`
//! writer and reader can store them. `eskiz-anim` builds frames and playback from them.

use crate::blend::BlendMode;


/// Lowest frame rate, in frames per second. `eskiz-media` uses the same value for its exporters.
pub const MIN_FPS: u32 = 1;
/// Highest frame rate, in frames per second.
pub const MAX_FPS: u32 = 60;
/// Fewest onion frames shown on each side.
pub const MIN_ONION_COUNT: u32 = 1;
/// Most onion frames shown on each side.
pub const MAX_ONION_COUNT: u32 = 20;
/// Most extra frames a stack item can be held for.
pub const MAX_HOLD: u32 = 30;
/// Opacity multiplier for each onion frame beyond the nearest one.
pub const ONION_FALLOFF: f64 = 0.75;
/// Default previous-frame onion colour, `#E5484D`.
pub const PREVIOUS_COLOUR: [u8; 3] = [0xE5, 0x48, 0x4D];
/// Default next-frame onion colour, `#3B82F6`.
pub const NEXT_COLOUR: [u8; 3] = [0x3B, 0x82, 0xF6];

/// Playback mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackMode {
    #[default]
    Loop,
    PingPong,
    OneShot,
}

/// Playback direction. Reverse plays the frames in the opposite order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackDirection {
    #[default]
    Forward,
    Reverse,
}

/// Blend mode for onion frames when primary layers are mixed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnionBlend {
    #[default]
    Normal,
    Multiply,
}

/// Clamps a frame rate to the range 1 to 60 fps.
pub fn clamp_frame_rate(fps: u32) -> u32 {
    fps.clamp(MIN_FPS, MAX_FPS)
}

/// Document-level Frames settings.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSettings {
    /// Frames on or off. Default off.
    pub frames_enabled: bool,
    /// Frame rate as stored. Read it through [`AnimationSettings::fps`], which clamps it.
    pub frame_rate: u32,
    pub playback_mode: PlaybackMode,
    pub direction: PlaybackDirection,
    /// Onion skins on or off. Default on.
    pub onion_enabled: bool,
    /// Neighbouring frames shown on each side. Read it through [`AnimationSettings::onion_count`].
    pub onion_count: u32,
    /// Opacity of the nearest onion frame, 0 to 1. Read it through [`AnimationSettings::onion_opacity`].
    pub onion_opacity: f64,
    pub previous_colour: [u8; 3],
    pub next_colour: [u8; 3],
    /// Primary layers mixed with the onion frames. Default off.
    pub primary_mixed: bool,
    /// Onion blend used when `primary_mixed` is on.
    pub onion_blend: OnionBlend,
    /// The file's `skinBlendMode` value (format 4a, default 2). Kept for interop only. The design notes do
    /// not map this index, so compositing uses [`AnimationSettings::onion_blend_mode`].
    pub skin_blend_mode: i64,
    /// Export frame rate. `None` follows the animation frame rate.
    pub export_frame_rate: Option<u32>,
}

impl Default for AnimationSettings {
    fn default() -> Self {
        Self {
            frames_enabled: false,
            frame_rate: 15,
            playback_mode: PlaybackMode::Loop,
            direction: PlaybackDirection::Forward,
            onion_enabled: true,
            onion_count: 12,
            onion_opacity: 0.6,
            previous_colour: PREVIOUS_COLOUR,
            next_colour: NEXT_COLOUR,
            primary_mixed: false,
            onion_blend: OnionBlend::Normal,
            skin_blend_mode: 2,
            export_frame_rate: None,
        }
    }
}

impl AnimationSettings {
    /// Playback frame rate, clamped to 1 to 60.
    pub fn fps(&self) -> u32 {
        clamp_frame_rate(self.frame_rate)
    }

    /// Sets the frame rate, clamping it to 1 to 60.
    pub fn set_frame_rate(&mut self, fps: u32) {
        self.frame_rate = clamp_frame_rate(fps);
    }

    /// Export frame rate: the override if set, otherwise the playback rate. Clamped to 1 to 60.
    pub fn export_fps(&self) -> u32 {
        clamp_frame_rate(self.export_frame_rate.unwrap_or_else(|| self.fps()))
    }

    /// Onion frames shown on each side, clamped to 1 to 20.
    pub fn onion_count(&self) -> u32 {
        self.onion_count.clamp(MIN_ONION_COUNT, MAX_ONION_COUNT)
    }

    /// Nearest onion frame opacity, clamped to 0 to 1.
    pub fn onion_opacity(&self) -> f64 {
        self.onion_opacity.clamp(0.0, 1.0)
    }

    /// Blend mode for onion frames. Normal while primary layers are not mixed in.
    pub fn onion_blend_mode(&self) -> BlendMode {
        match (self.primary_mixed, self.onion_blend) {
            (true, OnionBlend::Multiply) => BlendMode::Multiply,
            _ => BlendMode::Normal,
        }
    }
}
