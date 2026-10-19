//! Text layer model: attributed runs, colour, the placement transform and the
//! wrap width.
//!
//! The data types are defined in `eskiz-types` (`eskiz_types::text`), so `eskiz-core` can store
//! text layers without depending on fonts, and are re-exported here unchanged. The methods that
//! need fonts are on [`TextLayerExt`].

pub use eskiz_types::text::{
    Affine, Alignment, Attrs, Color, FontFace, Run, TextLayer, WritingDirection, DEFAULT_SIZE_PT,
    LEADING_RANGE, SIZE_RANGE, SPACING_RANGE,
};

use crate::fonts::FontLibrary;
use crate::layout::{self, Layout};
use crate::raster::{self, Raster};

/// The [`TextLayer`] methods that need a [`FontLibrary`]: layout and rasterising.
pub trait TextLayerExt {
    /// Lays the text out with cosmic-text and the documented rules.
    fn layout(&self, fonts: &mut FontLibrary) -> Layout;

    /// Rasterises the layer at its own scale: the result is the layer's pixels.
    fn rasterise(&self, fonts: &mut FontLibrary) -> Raster;

    /// Rasterises the layer with the transform followed by a uniform `scale`.
    ///
    /// Glyphs are drawn from their outlines at this scale, so a layer scaled up three times is
    /// rendered at that detail. It is not enlarged from a smaller bitmap. A non-positive or
    /// non-finite scale returns an empty raster.
    fn rasterise_at_scale(&self, fonts: &mut FontLibrary, scale: f32) -> Raster;
}

impl TextLayerExt for TextLayer {
    fn layout(&self, fonts: &mut FontLibrary) -> Layout {
        layout::layout(self, fonts)
    }

    fn rasterise(&self, fonts: &mut FontLibrary) -> Raster {
        self.rasterise_at_scale(fonts, 1.0)
    }

    fn rasterise_at_scale(&self, fonts: &mut FontLibrary, scale: f32) -> Raster {
        if !(scale.is_finite() && scale > 0.0) {
            return Raster::empty();
        }
        let layout = self.layout(fonts);
        raster::render(&layout, fonts, self.transform.scaled(scale))
    }
}
