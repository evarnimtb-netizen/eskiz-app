//! Frames logic.
//!
//! Pure CPU code with no UI. The app passes in a [`StackSnapshot`] of its layer stack, and this crate
//! provides:
//! - [`AnimationSettings`] with the design notes defaults (2.1);
//! - [`FrameList`], the frames built from the stack (2.2), with holds (2.3);
//! - [`Playback`], the playhead and its clock (2.4);
//! - [`onion_skins`] and [`render_with_onion`] (2.5);
//! - [`export_frames`], which sequences frames for the `eskiz-media` exporters (2.6, 2.7).

#![forbid(unsafe_code)]

pub mod export;
pub mod frames;
pub mod onion;
pub mod playback;
pub mod settings;
pub mod snapshot;

pub use export::{export_frames, export_frames_with, Paper};
pub use frames::{AnimError, AnimFrame, FrameList, StackItem, StackSnapshot};
pub use onion::{onion_skins, render_with_onion, tint_premul, OnionSide, OnionSkin};
pub use playback::{Playback, Tick};
pub use snapshot::snapshot_from_document;
pub use settings::{
    clamp_frame_rate, AnimationSettings, OnionBlend, PlaybackDirection, PlaybackMode, MAX_FPS,
    MIN_FPS,
};
