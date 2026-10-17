//! Copy, cut and paste of pixels. Colour is copied exactly, with no conversion.

use crate::geom::IRect;
use crate::image::{bounds_where, Image};
use crate::selection::Selection;

/// Pixels on the clipboard, with the canvas position they came from when known.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub image: Image,
    pub origin: Option<(usize, usize)>,
}

/// Copies the layer inside the selection. The clip covers the selection's bounding box, and
/// each alpha is multiplied by the selection value (zero outside the selection). With no
/// selection it copies the layer's content bounds. None when there is nothing to copy.
pub fn copy_selection(layer: &Image, sel: &Selection) -> Option<Clip> {
    let bounds = match sel.mask() {
        Some(mask) => mask.bounds()?,
        None => layer.alpha_bounds()?,
    };
    let mut image = Image::transparent(bounds.w, bounds.h);
    for y in 0..bounds.h {
        for x in 0..bounds.w {
            let (sx, sy) = (bounds.x + x, bounds.y + y);
            let mut px = layer.get(sx, sy);
            px[3] *= sel.value_at(sx, sy);
            image.pixels[y * bounds.w + x] = px;
        }
    }
    Some(Clip {
        image,
        origin: Some((bounds.x, bounds.y)),
    })
}

/// Copies the whole composite. The origin is the canvas origin.
pub fn copy_all(composite: &Image) -> Clip {
    Clip {
        image: composite.clone(),
        origin: Some((0, 0)),
    }
}

/// Cut: the clip from [`copy_selection`], and the layer with the selected area cleared.
/// Alpha is multiplied by `1 - s`, so a fully selected pixel becomes transparent and an
/// unselected one is kept. The input layer is not modified, so undo is the caller's copy.
pub fn cut_selection(layer: &Image, sel: &Selection) -> Option<(Clip, Image)> {
    let clip = copy_selection(layer, sel)?;
    let mut out = layer.clone();
    for y in 0..layer.height {
        for x in 0..layer.width {
            let idx = y * layer.width + x;
            let s = sel.value_at(x, y);
            out.pixels[idx][3] *= (1.0 - s).clamp(0.0, 1.0);
            if out.pixels[idx][3] <= 0.0 {
                out.pixels[idx] = [0.0; 4];
            }
        }
    }
    Some((clip, out))
}

/// Paste into a new canvas-sized layer. The clip goes at `at`, else at its original origin,
/// else centred. Pixels outside the canvas are dropped (see the report for the gap).
pub fn paste(clip: &Clip, canvas_w: usize, canvas_h: usize, at: Option<(usize, usize)>) -> Image {
    let (ox, oy): (i64, i64) = match at.or(clip.origin) {
        Some((x, y)) => (x as i64, y as i64),
        None => (
            (canvas_w as i64 - clip.image.width as i64) / 2,
            (canvas_h as i64 - clip.image.height as i64) / 2,
        ),
    };
    let mut out = Image::transparent(canvas_w, canvas_h);
    for y in 0..clip.image.height {
        for x in 0..clip.image.width {
            let dx = ox + x as i64;
            let dy = oy + y as i64;
            if let (Ok(dx), Ok(dy)) = (usize::try_from(dx), usize::try_from(dy)) {
                if dx < canvas_w && dy < canvas_h {
                    out.pixels[dy * canvas_w + dx] = clip.image.pixels[y * clip.image.width + x];
                }
            }
        }
    }
    out
}

/// Bounding box of the non-transparent pixels of an image, for callers that need the content box.
pub fn content_bounds(image: &Image) -> Option<IRect> {
    bounds_where(image.width, image.height, |i| image.pixels[i][3] > 0.0)
}
