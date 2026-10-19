//! Rasterisation. Glyph outlines come from swash. The output is grey-scale antialiased,
//! with each glyph origin on a whole pixel and no hinting. Outline strokes are centred on the
//! outline, and the fill is drawn over them. Output is premultiplied RGBA8.

use swash::scale::image::Image;
use swash::scale::{Render, ScaleContext, Source};
use swash::zeno::{Format, Stroke, Transform};

use crate::fonts::FontLibrary;
use crate::layout::{Layout, Rect};
use crate::model::{Affine, Color};

/// Premultiplied RGBA8 pixels, row-major, with the document position of the top-left pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub origin: (i32, i32),
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Raster {
    /// A raster with no pixels.
    pub fn empty() -> Self {
        Self {
            origin: (0, 0),
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }
    }

    /// The premultiplied RGBA of the pixel at `(x, y)` relative to the raster's top-left corner.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// Bounds of the pixels with non-zero alpha, in document pixels (`x0, y0, x1, y1`, with the
    /// maximum edges exclusive).
    pub fn ink_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut found: Option<(u32, u32, u32, u32)> = None;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.pixel(x, y)[3] == 0 {
                    continue;
                }
                found = Some(match found {
                    None => (x, y, x + 1, y + 1),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
                });
            }
        }
        found.map(|(x0, y0, x1, y1)| {
            let (ox, oy) = self.origin;
            (
                ox + x0 as i32,
                oy + y0 as i32,
                ox + x1 as i32,
                oy + y1 as i32,
            )
        })
    }

    /// Sum of the alpha channel. A rough measure of ink coverage.
    pub fn alpha_sum(&self) -> u64 {
        self.pixels
            .iter()
            .skip(3)
            .step_by(4)
            .map(|&a| u64::from(a))
            .sum()
    }
}

/// An 8-bit coverage mask in document pixels, painted in one colour.
struct Mask {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    data: Vec<u8>,
    color: Color,
}

/// Renders a laid-out text with the device transform `t` (layer transform times any scale).
pub(crate) fn render(layout: &Layout, fonts: &mut FontLibrary, t: Affine) -> Raster {
    if !layout.bounds.is_empty() {
        // Refuse before any glyph is drawn when the text alone needs an absurd canvas.
        let (a, b, c, d) = device_rect(t, layout.bounds, false);
        let (w, h) = (i64::from(c) - i64::from(a), i64::from(d) - i64::from(b));
        if w.saturating_mul(h) > MAX_RASTER_PIXELS {
            return Raster::empty();
        }
    }
    let sources = [Source::Outline];
    let mut scale_ctx = ScaleContext::new();
    let mut masks: Vec<Mask> = Vec::new();

    for glyph in &layout.glyphs {
        let Some(font) = fonts.font(glyph.font_id, glyph.font_weight) else {
            continue;
        };
        let font_ref = font.as_swash();
        let mut scaler = scale_ctx
            .builder(font_ref)
            .size(glyph.font_size)
            .hint(false)
            .build();

        // The origin is snapped to a whole device pixel (no subpixel positioning).
        let (px, py) = t.apply(glyph.origin.0, glyph.origin.1);
        let (ox, oy) = (px.round() as i32, py.round() as i32);

        // Font outlines are y-up, and swash masks use a bottom-left origin, so `top` is the
        // highest point above the glyph origin. The device is y-down, so the outline is mirrored
        // into y-up device space: x' = a*u - c*v, y' = -b*u + d*v.
        let outline_transform = Transform::new(t.a, -t.b, -t.c, t.d, 0.0, 0.0);

        if glyph.outline_pt > 0.0 {
            let mut stroke = Render::new(&sources);
            stroke
                .format(Format::Alpha)
                .transform(Some(outline_transform))
                .style(Stroke::new(glyph.outline_pt * t.linear_scale()));
            if let Some(image) = stroke.render(&mut scaler, glyph.glyph_id) {
                push_image(&mut masks, image, ox, oy, glyph.color);
            }
        }

        let mut fill = Render::new(&sources);
        fill.format(Format::Alpha)
            .transform(Some(outline_transform));
        if let Some(image) = fill.render(&mut scaler, glyph.glyph_id) {
            push_image(&mut masks, image, ox, oy, glyph.color);
        }
    }

    for underline in &layout.underlines {
        let (x0, y0, x1, y1) = device_rect(t, underline.rect, true);
        let (x1, y1) = (x1.max(x0.saturating_add(1)), y1.max(y0.saturating_add(1)));
        let (uw, uh) = (i64::from(x1) - i64::from(x0), i64::from(y1) - i64::from(y0));
        if uw * uh > MAX_RASTER_PIXELS {
            return Raster::empty();
        }
        masks.push(Mask {
            x: x0,
            y: y0,
            width: uw as u32,
            height: uh as u32,
            data: vec![255; (uw * uh) as usize],
            color: underline.color,
        });
    }

    // Canvas: the text bounds plus the outline gutter, grown to hold every mask.
    let mut rects: Vec<(i64, i64, i64, i64)> = masks
        .iter()
        .map(|m| {
            (
                i64::from(m.x),
                i64::from(m.y),
                i64::from(m.x) + i64::from(m.width),
                i64::from(m.y) + i64::from(m.height),
            )
        })
        .collect();
    if !layout.bounds.is_empty() {
        let (a, b, c, d) = device_rect(t, layout.bounds, false);
        rects.push((i64::from(a), i64::from(b), i64::from(c), i64::from(d)));
    }
    if rects.is_empty() {
        return Raster::empty();
    }
    let x0 = rects.iter().map(|r| r.0).min().unwrap_or(0);
    let y0 = rects.iter().map(|r| r.1).min().unwrap_or(0);
    let x1 = rects.iter().map(|r| r.2).max().unwrap_or(0);
    let y1 = rects.iter().map(|r| r.3).max().unwrap_or(0);
    let (w64, h64) = ((x1 - x0).max(0), (y1 - y0).max(0));
    if w64 == 0 || h64 == 0 || w64 > i64::from(i32::MAX) || h64 > i64::from(i32::MAX) {
        return Raster::empty();
    }
    // Text that would need an absurd canvas (huge size, outline or transform) is not drawn.
    if w64.saturating_mul(h64) > MAX_RASTER_PIXELS {
        return Raster::empty();
    }
    let (width, height) = (w64 as u32, h64 as u32);
    let (x0, y0) = (x0 as i32, y0 as i32);

    // Source-over composite in premultiplied float, then quantised to RGBA8.
    let stride = width as usize;
    let mut acc = vec![0.0f32; stride * height as usize * 4];
    for mask in &masks {
        let mx = (mask.x - x0) as usize;
        let my = (mask.y - y0) as usize;
        let mw = mask.width as usize;
        for j in 0..mask.height as usize {
            for i in 0..mw {
                let raw = mask.data.get(j * mw + i).copied().unwrap_or(0);
                let coverage = f32::from(raw) / 255.0 * (f32::from(mask.color.a) / 255.0);
                if coverage <= 0.0 {
                    continue;
                }
                let idx = ((my + j) * stride + (mx + i)) * 4;
                let inv = 1.0 - coverage;
                acc[idx] = f32::from(mask.color.r) / 255.0 * coverage + acc[idx] * inv;
                acc[idx + 1] = f32::from(mask.color.g) / 255.0 * coverage + acc[idx + 1] * inv;
                acc[idx + 2] = f32::from(mask.color.b) / 255.0 * coverage + acc[idx + 2] * inv;
                acc[idx + 3] = coverage + acc[idx + 3] * inv;
            }
        }
    }
    let pixels = acc
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    Raster {
        origin: (x0, y0),
        width,
        height,
        pixels,
    }
}

/// Largest text raster, in pixels (64 MP).
pub const MAX_RASTER_PIXELS: i64 = 64 * 1024 * 1024;

fn push_image(masks: &mut Vec<Mask>, image: Image, ox: i32, oy: i32, color: Color) {
    let placement = image.placement;
    if placement.width == 0 || placement.height == 0 {
        return;
    }
    // `top` is the highest point of the mask above the origin, so the first row is `top` above it.
    masks.push(Mask {
        x: ox + placement.left,
        y: oy - placement.top,
        width: placement.width,
        height: placement.height,
        data: image.data,
        color,
    });
}

/// Device-space bounds of a layer-space rectangle. `round` snaps to the nearest pixel edges,
/// otherwise the bounds are expanded outwards.
fn device_rect(t: Affine, rect: Rect, round: bool) -> (i32, i32, i32, i32) {
    let corners = [
        t.apply(rect.x0, rect.y0),
        t.apply(rect.x1, rect.y0),
        t.apply(rect.x0, rect.y1),
        t.apply(rect.x1, rect.y1),
    ];
    let mut bounds = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        bounds = (
            bounds.0.min(x),
            bounds.1.min(y),
            bounds.2.max(x),
            bounds.3.max(y),
        );
    }
    let (x0, y0, x1, y1) = bounds;
    if round {
        (
            x0.round() as i32,
            y0.round() as i32,
            x1.round() as i32,
            y1.round() as i32,
        )
    } else {
        (
            x0.floor() as i32,
            y0.floor() as i32,
            x1.ceil() as i32,
            y1.ceil() as i32,
        )
    }
}
