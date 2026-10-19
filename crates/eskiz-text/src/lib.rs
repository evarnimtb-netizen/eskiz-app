//! eskiz-text: text layers for Eskiz, with attributed runs, fonts and layout.
//!
//! * [`model`]: attributed runs ([`Attrs`], [`Run`]), colour, the affine placement and [`TextLayer`]
//!   (data defined in `eskiz-types`), plus [`TextLayerExt`] for layout and rasterising.
//! * [`fonts`]: [`FontLibrary`], with system fonts, imported files and folders, and the
//!   missing-font fallback order.
//! * [`layout`]: cosmic-text shaping, then the documented line, alignment, leading, justification
//!   and vertical column rules.
//! * [`raster`]: premultiplied RGBA8 output with grey-scale antialiasing and outline strokes.
//! * [`storage`]: conversion between attributes and the saved attribute keys.

pub mod fonts;
pub mod layout;
pub mod model;
pub mod raster;
pub mod storage;

pub use fonts::{DirReport, FontError, FontLibrary, Replacement, Resolution};
pub use layout::{Layout, LayoutMetrics, LineBox, PlacedGlyph, Rect, Underline};
pub use model::{
    Affine, Alignment, Attrs, Color, FontFace, Run, TextLayer, TextLayerExt, WritingDirection,
};
pub use raster::Raster;
pub use storage::{SavedValue, StorageError};
