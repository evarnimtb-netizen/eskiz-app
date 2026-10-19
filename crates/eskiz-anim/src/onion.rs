//! Onion skins: which neighbouring frames show, their opacity and tint, and compositing.

use eskiz_core::compose::to_u8;
use eskiz_core::{blend, BlendMode, Canvas, Layer};

use crate::frames::{FrameList, StackItem};
use crate::settings::{AnimationSettings, ONION_FALLOFF};

/// Which side of the current frame an onion frame sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnionSide {
    Before,
    After,
}

/// One onion frame to draw under the current frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnionSkin {
    /// Index of the neighbouring frame in the [`FrameList`].
    pub frame: usize,
    /// Distance from the current frame: 1 for the nearest.
    pub distance: u32,
    /// Opacity after the falloff: the setting for distance 1, times 0.75 for each further frame.
    pub opacity: f64,
    /// Tint colour: the previous-frame colour before the current frame, the next-frame colour after.
    pub tint: [u8; 3],
    pub side: OnionSide,
}

/// The onion frames for the current frame.
///
/// Returns nothing while playing, when onion skins are off, or when the index is out of range.
/// Otherwise it returns up to `onion_count` frames on each side, nearest first within each side.
pub fn onion_skins(
    settings: &AnimationSettings,
    list: &FrameList,
    current: usize,
    playing: bool,
) -> Vec<OnionSkin> {
    let n = list.len();
    if !settings.onion_enabled || playing || current >= n {
        return Vec::new();
    }
    let count = settings.onion_count() as usize;
    let before = count.min(current);
    let after = count.min(n - 1 - current);
    let mut skins = Vec::with_capacity(before + after);
    for d in 1..=before {
        skins.push(skin(settings, current - d, d, OnionSide::Before));
    }
    for d in 1..=after {
        skins.push(skin(settings, current + d, d, OnionSide::After));
    }
    skins
}

fn skin(settings: &AnimationSettings, frame: usize, distance: usize, side: OnionSide) -> OnionSkin {
    let exponent = i32::try_from(distance - 1).unwrap_or(i32::MAX);
    let tint = match side {
        OnionSide::Before => settings.previous_colour,
        OnionSide::After => settings.next_colour,
    };
    OnionSkin {
        frame,
        distance: distance as u32,
        opacity: settings.onion_opacity() * ONION_FALLOFF.powi(exponent),
        tint,
        side,
    }
}

/// Tints a coverage value with a colour (silhouette tint).
///
/// The result is premultiplied RGBA with the colour `tint` and alpha `alpha`, so the frame's shape
/// shows in the tint colour and the frame's own colours are dropped. The tint colour is given
/// but no formula, so this is a `[choice]`. Alpha is clamped to 0 to 1.
pub fn tint_premul(alpha: f64, tint: [u8; 3]) -> [f64; 4] {
    let a = alpha.clamp(0.0, 1.0);
    [
        f64::from(tint[0]) / 255.0 * a,
        f64::from(tint[1]) / 255.0 * a,
        f64::from(tint[2]) / 255.0 * a,
        a,
    ]
}

/// Composites the current frame over its onion skins, the background and the foreground.
///
/// Layering, bottom to top: background, onion frames (farthest first), current frame items in stack
/// order, foreground. Onion frames use `onion_blend` and the current frame uses each item's own blend.
/// The result is flattened over white paper, as RGBA8 with alpha 255, row-major.
pub fn render_with_onion(
    list: &FrameList,
    current: usize,
    skins: &[OnionSkin],
    onion_blend: BlendMode,
) -> Vec<u8> {
    let (width, height) = list.size();
    let mut stack: Vec<(Layer, BlendMode)> = Vec::new();
    let mut clips: Vec<bool> = Vec::new();

    if let Some(item) = list
        .background_item()
        .filter(|&i| list.is_drawn(i))
        .and_then(|i| list.item(i))
    {
        stack.push(entry(item));
        clips.push(false);
    }
    let mut ordered: Vec<&OnionSkin> = skins.iter().collect();
    ordered.sort_by_key(|skin| std::cmp::Reverse(skin.distance));
    for skin in ordered {
        stack.push((tinted_layer(list, skin), onion_blend));
        clips.push(false);
    }
    for item in list.frame_items(current) {
        stack.push(entry(item));
        clips.push(item.clipped);
    }
    if let Some(item) = list
        .foreground_item()
        .filter(|&i| list.is_drawn(i))
        .and_then(|i| list.item(i))
    {
        stack.push(entry(item));
        clips.push(false);
    }

    if clips.iter().any(|&c| c) {
        return composite_clipped(width, height, stack, &clips);
    }
    let (layers, modes): (Vec<Layer>, Vec<BlendMode>) = stack.into_iter().unzip();
    let canvas = Canvas {
        width,
        height,
        layers,
    };
    blend::flatten(&canvas, &modes)
}

/// Composite a stack that has clipped layers through the document compositor, over white paper.
fn composite_clipped(
    width: usize,
    height: usize,
    stack: Vec<(Layer, BlendMode)>,
    clips: &[bool],
) -> Vec<u8> {
    let mut doc = eskiz_core::Document::empty(width, height);
    for ((layer, blend), &clipped) in stack.into_iter().zip(clips) {
        let props = eskiz_core::Props {
            visible: layer.visible,
            opacity: layer.opacity,
            blend,
            clipped,
            ..eskiz_core::Props::new("")
        };
        doc.push_raster(None, props, None, layer);
    }
    let straight = eskiz_core::composite::composite(
        &doc,
        eskiz_core::composite::CompositeOpts {
            skip_private: true,
            background: false,
        },
    );
    straight
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
        // Straight RGBA over white paper.
        let a = u32::from(p[3]);
        let over = |c: u8| ((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8;
        [over(p[0]), over(p[1]), over(p[2]), 255]
        })
        .collect()
}

/// Layer and blend of a stack item.
fn entry(item: &StackItem) -> (Layer, BlendMode) {
    (item.layer.clone(), item.blend)
}

/// The onion frame as a tinted layer: the frame's union alpha, times the skin opacity, in the tint.
fn tinted_layer(list: &FrameList, skin: &OnionSkin) -> Layer {
    let (width, height) = list.size();
    let mut buf = Vec::with_capacity(width * height * 4);
    for alpha in frame_alpha(list, skin.frame) {
        buf.extend(tint_premul(alpha * skin.opacity, skin.tint).map(to_u8));
    }
    Layer::from_rgba(width, height, buf)
}

/// Union alpha of the items in frame `index`, per pixel, in 0 to 1.
///
/// The union `a = as + ab (1 - as)` does not depend on the blend mode, so it is exact for every mode.
fn frame_alpha(list: &FrameList, index: usize) -> Vec<f64> {
    let (width, height) = list.size();
    let mut acc = vec![0.0_f64; width * height];
    for item in list.frame_items(index) {
        let opacity = item.layer.opacity.clamp(0.0, 1.0);
        let rgba = item.layer.to_rgba();
        for (p, a) in acc.iter_mut().enumerate() {
            let src = f64::from(rgba[p * 4 + 3]) / 255.0 * opacity;
            *a = src + *a * (1.0 - src);
        }
    }
    acc
}
