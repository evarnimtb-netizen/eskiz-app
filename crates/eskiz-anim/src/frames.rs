//! Frames built from a layer-tree snapshot, and their holds (2.3).

use eskiz_core::{BlendMode, Layer};

use crate::onion::render_with_onion;
use crate::settings::MAX_HOLD;

/// One top-level stack item: a layer, or a group flattened to one composite by the caller.
#[derive(Clone, Debug, PartialEq)]
pub struct StackItem {
    /// Pixels, opacity and visibility. Its size must match the snapshot canvas.
    pub layer: Layer,
    /// Blend mode used when the item is composited into its frame.
    pub blend: BlendMode,
    /// Private items are in no frame, export or onion skin.
    pub private: bool,
    /// A clipped item joins the frame of the item directly below it.
    pub clipped: bool,
    /// Marks the bottom-most item as the animation background (drawn beneath every frame).
    pub animation_background: bool,
    /// Marks the top-most item as the animation foreground (drawn above every frame).
    pub animation_foreground: bool,
    /// Extra frames this item is held for. Values above 30 are clamped.
    pub hold: u32,
}

impl StackItem {
    /// A visible, unflagged item with Normal blend and no hold.
    pub fn new(layer: Layer) -> Self {
        Self {
            layer,
            blend: BlendMode::Normal,
            private: false,
            clipped: false,
            animation_background: false,
            animation_foreground: false,
            hold: 0,
        }
    }
}

/// The layer stack at one moment, bottom item first.
#[derive(Clone, Debug, PartialEq)]
pub struct StackSnapshot {
    pub width: usize,
    pub height: usize,
    /// Top-level items, bottom to top. A group is one item.
    pub items: Vec<StackItem>,
}

/// One animation frame: the stack items it composites, bottom first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimFrame {
    /// Indices into [`FrameList::items`], in stack order.
    pub items: Vec<usize>,
    /// Hold of the bottom-most item. The frame shows for `hold + 1` ticks.
    pub hold: u32,
}

/// Errors from building a [`FrameList`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AnimError {
    #[error("canvas size {width} x {height} is empty")]
    EmptyCanvas { width: usize, height: usize },
    #[error("stack item {index} does not match the {width} x {height} canvas")]
    ItemSize {
        index: usize,
        width: usize,
        height: usize,
    },
}

/// The animation frames of a stack, with the flagged background and foreground items.
///
/// Frame 0 is the bottom-most frame. The background and foreground items are not frames; they are
/// drawn beneath and above every frame.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameList {
    width: usize,
    height: usize,
    items: Vec<StackItem>,
    frames: Vec<AnimFrame>,
    background: Option<usize>,
    foreground: Option<usize>,
}

impl FrameList {
    /// Builds the frames from a stack snapshot.
    ///
    /// Items that are hidden or private are in no frame. A clipped item joins the frame of the item
    /// directly below it. If that item is not in a frame, the clipped item starts its own frame
    /// (a `[choice]`). Any other visible item starts a new frame.
    pub fn build(snapshot: StackSnapshot) -> Result<Self, AnimError> {
        let StackSnapshot {
            width,
            height,
            items,
        } = snapshot;
        if width == 0 || height == 0 {
            return Err(AnimError::EmptyCanvas { width, height });
        }
        for (index, item) in items.iter().enumerate() {
            if item.layer.width != width || item.layer.height != height {
                return Err(AnimError::ItemSize {
                    index,
                    width,
                    height,
                });
            }
        }

        let background = items
            .first()
            .filter(|item| item.animation_background)
            .map(|_| 0);
        let foreground = items
            .last()
            .filter(|item| item.animation_foreground)
            .map(|_| items.len() - 1)
            .filter(|&top| Some(top) != background);

        let mut frames: Vec<AnimFrame> = Vec::new();
        // The frame each item landed in, or None when it is in no frame.
        let mut frame_of: Vec<Option<usize>> = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let excluded = Some(i) == background
                || Some(i) == foreground
                || !item.layer.visible
                || item.private;
            if excluded {
                frame_of.push(None);
                continue;
            }
            let below = if item.clipped {
                i.checked_sub(1).and_then(|p| frame_of[p])
            } else {
                None
            };
            let frame = match below {
                Some(f) => {
                    frames[f].items.push(i);
                    f
                }
                None => {
                    frames.push(AnimFrame {
                        items: vec![i],
                        hold: 0,
                    });
                    frames.len() - 1
                }
            };
            frame_of.push(Some(frame));
        }
        for frame in &mut frames {
            frame.hold = items[frame.items[0]].hold.min(MAX_HOLD);
        }

        Ok(Self {
            width,
            height,
            items,
            frames,
            background,
            foreground,
        })
    }

    /// Canvas size as `(width, height)`.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Number of frames, not counting the background and foreground items.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// True when the stack gives no frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Frame `index`, if it exists.
    pub fn frame(&self, index: usize) -> Option<&AnimFrame> {
        self.frames.get(index)
    }

    /// Hold of frame `index` in extra ticks, or 0 when it does not exist.
    pub fn hold(&self, index: usize) -> u32 {
        self.frames.get(index).map_or(0, |f| f.hold)
    }

    /// Every stack item, bottom first, including the background and foreground items.
    pub fn items(&self) -> &[StackItem] {
        &self.items
    }

    /// Stack item `index`.
    pub fn item(&self, index: usize) -> Option<&StackItem> {
        self.items.get(index)
    }

    /// Index of the item flagged as the animation background (the bottom-most item), if any.
    pub fn background_item(&self) -> Option<usize> {
        self.background
    }

    /// Index of the item flagged as the animation foreground (the top-most item), if any.
    pub fn foreground_item(&self) -> Option<usize> {
        self.foreground
    }

    /// True when a stack item is drawn at all: visible and not private.
    pub fn is_drawn(&self, index: usize) -> bool {
        self.items
            .get(index)
            .is_some_and(|item| item.layer.visible && !item.private)
    }

    /// The stack items composited into frame `index`, bottom first. Empty when the frame does not exist.
    pub fn frame_items(&self, index: usize) -> Vec<&StackItem> {
        self.frames.get(index).map_or_else(Vec::new, |frame| {
            frame.items.iter().map(|&i| &self.items[i]).collect()
        })
    }

    /// Flattened RGBA8 of frame `index` over white paper, with the background and foreground and no
    /// onion skins. Row-major, alpha 255.
    pub fn render(&self, index: usize) -> Vec<u8> {
        render_with_onion(self, index, &[], BlendMode::Normal)
    }
}

impl FrameList {
    /// Premultiplied RGBA8 of frame `index` with the background and foreground drawn, and no paper:
    /// transparent where nothing is painted. The transparent GIF and APNG exports use it.
    pub fn render_transparent(&self, index: usize) -> Vec<u8> {
        let (width, height) = self.size();
        let mut doc = eskiz_core::Document::empty(width, height);
        let mut order: Vec<&StackItem> = Vec::new();
        if let Some(i) = self.background_item().filter(|&i| self.is_drawn(i)) {
            order.push(&self.items[i]);
        }
        order.extend(self.frame_items(index));
        if let Some(i) = self.foreground_item().filter(|&i| self.is_drawn(i)) {
            order.push(&self.items[i]);
        }
        for item in order {
            let props = eskiz_core::Props {
                visible: item.layer.visible,
                opacity: item.layer.opacity,
                blend: item.blend,
                ..eskiz_core::Props::new("")
            };
            doc.push_raster(None, props, None, item.layer.clone());
        }
        eskiz_core::composite::composite_to_layer(&doc, true)
    }
}
