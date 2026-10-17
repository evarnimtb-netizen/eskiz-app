//! Layer tree description handed to the compositor.
//!
//! The tree borrows the GPU layers, so the app builds a `Scene` per composite from its own
//! document model. Order is bottom to top, as in the design notes.

use crate::layer::{GpuLayer, GpuMask};
pub use eskiz_core::BlendMode;

/// A raster layer in the stack.
pub struct SceneLayer<'a> {
    pub layer: &'a GpuLayer,
    /// Layer mask; `None` behaves as 1 everywhere.
    pub mask: Option<&'a GpuMask>,
    pub visible: bool,
    /// 0..1.
    pub opacity: f32,
    pub blend: BlendMode,
    /// Clip to the raster item directly below in the same parent.
    pub clipped: bool,
}

impl<'a> SceneLayer<'a> {
    /// Visible, opaque, Normal, unclipped, unmasked.
    pub fn new(layer: &'a GpuLayer) -> Self {
        Self {
            layer,
            mask: None,
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            clipped: false,
        }
    }

    pub fn with_mask(mut self, mask: &'a GpuMask) -> Self {
        self.mask = Some(mask);
        self
    }

    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    pub fn clipped(mut self) -> Self {
        self.clipped = true;
        self
    }

    pub fn hidden(mut self) -> Self {
        self.visible = false;
        self
    }
}

/// A group. Children composite **in place** onto what lies below: the
/// group's own opacity and blend mode are stored but not applied (open question O2), and a
/// hidden group hides its whole subtree. A clipped child clips only to a base inside the
/// same group.
pub struct SceneGroup<'a> {
    pub visible: bool,
    /// Stored for the document; not applied while compositing in place.
    pub opacity: f32,
    /// Stored for the document; not applied while compositing in place.
    pub blend: BlendMode,
    pub children: Vec<SceneItem<'a>>,
}

impl<'a> SceneGroup<'a> {
    pub fn new(children: Vec<SceneItem<'a>>) -> Self {
        Self {
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            children,
        }
    }
}

/// Item of a stack.
pub enum SceneItem<'a> {
    Layer(SceneLayer<'a>),
    Group(SceneGroup<'a>),
}

/// A document to composite.
pub struct Scene<'a> {
    /// Opaque background colour (straight RGB, 0..1). `None` means the background is
    /// hidden, so the backdrop starts transparent.
    pub background: Option<[f32; 3]>,
    /// Bottom to top.
    pub items: Vec<SceneItem<'a>>,
    /// Display transform for the output pass (colour management): a row-major 3x3
    /// matrix on **linear** light, from the document's colour space to the display's. Each output
    /// pixel's straight colour is decoded with the sRGB transfer curve, multiplied by the matrix,
    /// clamped to 0..1 (gamut clipping), encoded again and premultiplied. Compositing itself
    /// happens in the document's space. `None` leaves the output as composited.
    pub display: Option<[[f32; 3]; 3]>,
}

impl<'a> Scene<'a> {
    /// White background, no items, no display transform.
    pub fn new() -> Self {
        Self {
            background: Some([1.0; 3]),
            items: Vec::new(),
            display: None,
        }
    }

    pub fn push_layer(&mut self, l: SceneLayer<'a>) {
        self.items.push(SceneItem::Layer(l));
    }
}

impl Default for Scene<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Shader mode number for a blend mode: the position in [`BlendMode::ALL`].
pub fn mode_index(mode: BlendMode) -> u32 {
    BlendMode::ALL
        .iter()
        .position(|&m| m == mode)
        .expect("every mode is in ALL") as u32
}
