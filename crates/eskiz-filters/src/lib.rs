//! eskiz-filters: CPU implementations of Eskiz's adjustments and Liquify.
//!
//! These are the ground truth that GPU shaders are tested against. Pixels are
//! premultiplied RGBA in an [`Image`] (f32, row-major, nominally 0..1). Every
//! `apply` takes an optional per-pixel mask (`None` means 1 everywhere) and,
//! unless the design notes say otherwise, mixes its output with the source by that
//! mask. Items whose values are not confirmed are
//! flagged in their doc comments.

pub mod balance;
pub mod bloom;
pub mod chromatic;
pub mod clone;
pub mod colour;
pub mod curves;
pub mod gaussian;
pub mod glitch;
pub mod gradient_map;
pub mod halftone;
pub mod hsb;
pub mod image;
pub mod liquify;
pub mod motion;
pub mod noise;
pub mod opacity;
pub mod perspective;
pub mod pyramid;
pub mod random;
pub mod recolor;
pub mod sharpen;

pub use image::{mask_at, mix_masked, mix_weighted, Image, InteractionMode, Px};

/// Clamp an amount or any [0, 1] control.
pub fn clamp_unit(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// Drag-on-canvas value mapping:
/// change = dx / canvas width x range span, clamped to the range.
pub fn drag_to_value(current: f32, dx_points: f32, canvas_width: f32, range: (f32, f32)) -> f32 {
    let span = range.1 - range.0;
    (current + dx_points / canvas_width * span).clamp(range.0, range.1)
}

/// The Gaussian blur entry of the adjustment dispatch.
/// Layer mode runs the Gaussian blur; Pencil mode replaces it with the pyramid blur.
pub fn blur_for_mode(
    mode: InteractionMode,
    src: &Image,
    mask: Option<&[f32]>,
    amount: f32,
) -> Image {
    match mode {
        InteractionMode::Layer => gaussian::apply(src, mask, &gaussian::GaussianParams { amount }),
        InteractionMode::Pencil => pyramid::apply(src, mask, &pyramid::PyramidParams { amount }),
    }
}
