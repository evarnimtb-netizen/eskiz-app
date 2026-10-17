//! eskiz-core: pure-CPU brush engine.
//!
//! Pipeline:
//! `Sample` -> [`geometry::Stroke`] (smoothing, dynamics, spacing) -> [`geometry::Dab`]s
//! -> [`raster`] (coverage) -> [`canvas::Layer::paint_stroke`] (composite) -> [`compose::flatten`].

pub mod animation;
pub mod blend;
pub mod brushes;
pub mod canvas;
pub mod compose;
pub mod composite;
pub mod document;
pub mod filters;
pub mod geometry;
pub mod parallelism;
pub mod pixel;
pub mod raster;
pub mod wet;

pub use blend::BlendMode;
pub use eskiz_guides::{ColourSpace, PixelConverter};
pub use brushes::{
    Accumulation, Brush, CurveSet, Grain, GrainMode, Preset, Shape, Taper, TaperPair, Wet,
};
pub use canvas::{Canvas, Layer, PaintOptions, Tile, TileSlot, TILE, TILE_BYTES, TILE_SAMPLES};
pub use pixel::Depth;
pub use composite::{composite, composite16, composite_rect, composite_rect16, composite_to_layer, composite_to_tiled, CompositeOpts};
pub use document::{Document, Location, Mask, Node, NodeKind, Props};
pub use geometry::{
    Dab, PointerType, Sample, Stroke, StrokeEnd, StrokeOptions, StrokeUpdate, TailRedraw,
};
