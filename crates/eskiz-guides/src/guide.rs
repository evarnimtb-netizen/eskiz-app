//! Settings shared by every guide, and the trait that turns a guide into
//! styled line segments for a viewport.

use crate::geom::{Rect, Segment};

/// Display settings for a guide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideStyle {
    /// Opacity in `0..=1`.
    pub opacity: f64,
    /// Line thickness in canvas pixels (non-negative).
    pub thickness: f64,
}

impl GuideStyle {
    /// Builds a style, clamping opacity to `0..=1` and thickness to be non-negative.
    pub fn new(opacity: f64, thickness: f64) -> Self {
        Self {
            opacity: opacity.clamp(0.0, 1.0),
            thickness: thickness.max(0.0),
        }
    }
}

impl Default for GuideStyle {
    /// Half opacity and a 1 px line.
    fn default() -> Self {
        Self::new(0.5, 1.0)
    }
}

/// A guide line with the style it should be drawn with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyledSegment {
    pub segment: Segment,
    pub opacity: f64,
    pub thickness: f64,
}

/// Anything that renders to canvas-space line segments for a viewport.
pub trait GuideRender {
    /// Display settings applied to every line of this guide.
    fn style(&self) -> GuideStyle;

    /// Lines in canvas space, clipped to `viewport`.
    fn lines(&self, viewport: Rect) -> Vec<Segment>;

    /// [`lines`](Self::lines) with the guide's style attached to each segment.
    fn render(&self, viewport: Rect) -> Vec<StyledSegment> {
        let style = self.style();
        self.lines(viewport)
            .into_iter()
            .map(|segment| StyledSegment {
                segment,
                opacity: style.opacity,
                thickness: style.thickness,
            })
            .collect()
    }
}
