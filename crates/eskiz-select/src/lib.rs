//! Selection, transform, clipboard and canvas algorithms for Eskiz.
//!
//! Everything here works on plain buffers: no UI, no GPU, no file dialogs.
//! Coordinates are canvas pixels with the origin at the top left; pixel (x, y)
//! covers `[x, x + 1) x [y, y + 1)` and its centre is `(x + 0.5, y + 0.5)`.
//! Colour is straight RGBA with channels in `[0, 1]`.
//!

pub mod canvas;
pub mod clipboard;
pub mod error;
pub mod feather;
pub mod geom;
pub mod image;
pub mod mask;
pub mod outline;
pub mod raster;
pub mod resample;
pub mod selection;
pub mod session;
pub mod transform;
pub mod wand;
pub mod warp;

pub use error::{Result, SelectError};
pub use geom::{IRect, Mat3, Point, Quad, Rect};
pub use image::Image;
pub use mask::{CombineMode, Mask};
pub use resample::Interp;
pub use selection::Selection;
pub use session::TransformSession;
