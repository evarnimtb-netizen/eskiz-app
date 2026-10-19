//! eskiz-types: value types that more than one layer of Eskiz must agree on.
//!
//! This is a leaf crate with no dependencies, so every other crate, `eskiz-core` included, can
//! use it. It holds data and the small amount of maths that belongs to the data, nothing else.
//! See `docs/adr/0001-split-eskiz-app.md`.
//!
//! - [`colour`]: [`Rgb`] (gamma-encoded floats) and [`Rgb8`] (8-bit, hex strings).
//! - [`colour_space`]: [`ColourSpace`] and the sRGB / Display P3 converters.
//! - [`text`]: the text-layer data ([`TextLayer`], [`Run`], [`Attrs`], [`Affine`], [`Color`]).
//!
//! `eskiz-guides` re-exports `colour` and `colour_space` under its own paths, and `eskiz-text`
//! re-exports `text` as `eskiz_text::model`.

pub mod colour;
pub mod colour_space;
pub mod text;

pub use colour::{ParseHexError, Rgb, Rgb8};
pub use colour_space::{convert, ColourConverter, ColourSpace, PixelConverter};
pub use text::{Affine, Alignment, Attrs, Color, FontFace, Run, TextLayer, WritingDirection};
