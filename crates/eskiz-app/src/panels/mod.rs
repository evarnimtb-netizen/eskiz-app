//! Popover panel bodies. Each file adds one `*_body` method to `EskizApp`.

pub mod actions;
pub mod animation;
pub mod adjust_extras;
pub mod adjustments;
pub mod brush;
pub mod colour;
pub mod harmony;
pub mod layer_card;
pub mod layer_selftest;
pub mod layer_picker;
pub mod layer_thumbs;
pub mod layers;
pub mod liquify;
pub mod pressure;
pub mod rows;
pub mod studio;
pub mod selection;
pub mod text_style;
pub mod transform;
#[cfg(test)]
mod layers_tests;
#[cfg(test)]
mod layers_interaction_tests;
pub mod three_d;
