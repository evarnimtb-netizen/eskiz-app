//! eskiz-guides: drawing guides, guided layer and colour maths for Eskiz.
//!
//! Pure geometry and colour, with no UI and no dependencies. The UI draws the
//! segments that [`GuideRender`] produces, and feeds stroke samples through
//! [`DrawingAssist`]. Design choices are documented in `README.md`.
//!
//! - [`grid`]: [`Grid`] and [`Isometric`] guides.
//! - [`perspective`]: [`Perspective`] guide with one, two or three vanishing points.
//! - [`symmetry`]: [`Symmetry`] guide and its copy transforms.
//! - [`assist`]: [`DrawingAssist`], which snaps and mirrors strokes.
//! - [`colour`], [`disc`], [`harmony`], [`palette`], [`colour_space`]: colour maths.

pub mod assist;
pub mod colour;
pub mod colour_space;
pub mod disc;
pub mod geom;
pub mod grid;
pub mod guide;
pub mod harmony;
pub mod palette;
pub mod perspective;
pub mod symmetry;

pub use assist::{DrawingAssist, SnapGuide, StrokeAssist, DEFAULT_LOCK_DISTANCE};
pub use colour::{
    hsb_to_rgb, rgb_to_hsb, ColourHistory, Hsb, Hsv, ParseHexError, Rgb, Rgb8, HISTORY_CAPACITY,
};
pub use colour_space::{convert, ColourConverter, ColourSpace, PixelConverter};
pub use disc::{ColourDisc, DiscHit};
pub use geom::{Rect, Sample, Segment, Vec2};
pub use grid::{Grid, Isometric};
pub use guide::{GuideRender, GuideStyle, StyledSegment};
pub use harmony::Harmony;
pub use palette::{extract_palette, MAX_PALETTE_COLOURS};
pub use perspective::{Perspective, PerspectiveKind};
pub use symmetry::{Symmetry, SymmetryKind};
