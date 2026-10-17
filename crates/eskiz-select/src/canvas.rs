//! Canvas operations: crop, resize with and without resampling, and flip.
//! Masks use the same operations as images so layers, masks and selections stay in step.

use crate::error::{Result, SelectError};
use crate::geom::{IRect, Mat3};
use crate::image::Image;
use crate::mask::Mask;
use crate::resample::{resample, resample_mask, Interp};

/// Smallest canvas or image side in pixels.
pub const MIN_SIDE: usize = 1;
/// Largest canvas or image side in pixels.
pub const MAX_SIDE: usize = 16_384;

/// Where content stays when the canvas size changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopCentre,
    TopRight,
    MiddleLeft,
    Centre,
    MiddleRight,
    BottomLeft,
    BottomCentre,
    BottomRight,
}

/// Crop rectangle in whole pixels: the origin and size are
/// truncated, the rectangle is clamped to the canvas, and an empty result is refused.
pub fn crop_rect(
    canvas_width: usize,
    canvas_height: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> Result<IRect> {
    let x0 = x.trunc() as i64;
    let y0 = y.trunc() as i64;
    let x1 = x0 + w.trunc() as i64;
    let y1 = y0 + h.trunc() as i64;
    let (cw, ch) = (canvas_width as i64, canvas_height as i64);
    let (x0, x1) = (x0.clamp(0, cw), x1.clamp(0, cw));
    let (y0, y1) = (y0.clamp(0, ch), y1.clamp(0, ch));
    if x1 <= x0 || y1 <= y0 {
        return Err(SelectError::EmptyRect);
    }
    Ok(IRect::from_bounds(
        x0 as usize,
        y0 as usize,
        x1 as usize,
        y1 as usize,
    ))
}

/// Crops an image to `rect`.
pub fn crop(img: &Image, rect: IRect) -> Result<Image> {
    check_inside(img.width, img.height, rect)?;
    Ok(Image {
        width: rect.w,
        height: rect.h,
        pixels: crop_pixels(&img.pixels, img.width, rect),
    })
}

/// Crops a mask to `rect`.
pub fn crop_mask(mask: &Mask, rect: IRect) -> Result<Mask> {
    check_inside(mask.width, mask.height, rect)?;
    Ok(Mask {
        width: rect.w,
        height: rect.h,
        data: crop_pixels(&mask.data, mask.width, rect),
    })
}

/// Crop with a transform: the
/// content is resampled into the `rect` frame. `transform` maps canvas coordinates to canvas
/// coordinates before the crop offset is applied.
pub fn crop_resampled(img: &Image, rect: IRect, transform: Mat3, interp: Interp) -> Result<Image> {
    if rect.is_empty() {
        return Err(SelectError::EmptyRect);
    }
    let offset = Mat3::translate(-(rect.x as f32), -(rect.y as f32));
    Ok(resample(img, &(offset * transform), rect.w, rect.h, interp))
}

/// Sets a new canvas size without resampling. Content keeps its pixel size and is placed by
/// `anchor`. The new area is transparent.
pub fn resize_canvas(img: &Image, new_w: usize, new_h: usize, anchor: Anchor) -> Result<Image> {
    check_side(new_w)?;
    check_side(new_h)?;
    let (ox, oy) = anchor_offset(anchor, img.width, img.height, new_w, new_h);
    Ok(Image {
        width: new_w,
        height: new_h,
        pixels: place_pixels(&img.pixels, img.width, img.height, new_w, new_h, ox, oy),
    })
}

/// Canvas resize for a mask, placed the same way as [`resize_canvas`].
pub fn resize_canvas_mask(mask: &Mask, new_w: usize, new_h: usize, anchor: Anchor) -> Result<Mask> {
    check_side(new_w)?;
    check_side(new_h)?;
    let (ox, oy) = anchor_offset(anchor, mask.width, mask.height, new_w, new_h);
    Ok(Mask {
        width: new_w,
        height: new_h,
        data: place_pixels(&mask.data, mask.width, mask.height, new_w, new_h, ox, oy),
    })
}

/// Offset of the old content inside a new canvas for the given anchor. Odd differences round
/// down, so a centred placement sits one pixel up or left when the difference is odd.
pub fn anchor_offset(
    anchor: Anchor,
    old_w: usize,
    old_h: usize,
    new_w: usize,
    new_h: usize,
) -> (i64, i64) {
    let (kx, ky) = match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::TopCentre => (1, 0),
        Anchor::TopRight => (2, 0),
        Anchor::MiddleLeft => (0, 1),
        Anchor::Centre => (1, 1),
        Anchor::MiddleRight => (2, 1),
        Anchor::BottomLeft => (0, 2),
        Anchor::BottomCentre => (1, 2),
        Anchor::BottomRight => (2, 2),
    };
    let dw = new_w as i64 - old_w as i64;
    let dh = new_h as i64 - old_h as i64;
    ((dw * kx).div_euclid(2), (dh * ky).div_euclid(2))
}

/// Resizes an image to `new_w` by `new_h`, resampling with `interp`.
pub fn resize_image(img: &Image, new_w: usize, new_h: usize, interp: Interp) -> Result<Image> {
    check_side(new_w)?;
    check_side(new_h)?;
    if img.width == 0 || img.height == 0 {
        return Err(SelectError::NoContent);
    }
    let mapping = scale_mapping(img.width, img.height, new_w, new_h);
    Ok(resample(img, &mapping, new_w, new_h, interp))
}

/// Resizes a mask by resampling, with the same interpolation as images.
pub fn resize_mask(mask: &Mask, new_w: usize, new_h: usize, interp: Interp) -> Result<Mask> {
    check_side(new_w)?;
    check_side(new_h)?;
    if mask.width == 0 || mask.height == 0 {
        return Err(SelectError::NoContent);
    }
    let mapping = scale_mapping(mask.width, mask.height, new_w, new_h);
    Ok(resample_mask(mask, &mapping, new_w, new_h, interp))
}

/// Mirrors an image across the canvas centre. Two flips of the same axis restore it.
pub fn flip_image(img: &Image, horizontal: bool) -> Image {
    Image {
        width: img.width,
        height: img.height,
        pixels: flip_pixels(&img.pixels, img.width, img.height, horizontal),
    }
}

/// Mirrors a mask across the canvas centre.
pub fn flip_mask(mask: &Mask, horizontal: bool) -> Mask {
    Mask {
        width: mask.width,
        height: mask.height,
        data: flip_pixels(&mask.data, mask.width, mask.height, horizontal),
    }
}

fn scale_mapping(old_w: usize, old_h: usize, new_w: usize, new_h: usize) -> Mat3 {
    Mat3::scale(new_w as f32 / old_w as f32, new_h as f32 / old_h as f32)
}

fn check_side(n: usize) -> Result<()> {
    if (MIN_SIDE..=MAX_SIDE).contains(&n) {
        Ok(())
    } else {
        Err(SelectError::SizeOutOfRange)
    }
}

fn check_inside(width: usize, height: usize, rect: IRect) -> Result<()> {
    if rect.is_empty() {
        Err(SelectError::EmptyRect)
    } else if rect.x1() > width || rect.y1() > height {
        Err(SelectError::SizeOutOfRange)
    } else {
        Ok(())
    }
}

fn crop_pixels<T: Copy>(data: &[T], width: usize, rect: IRect) -> Vec<T> {
    let mut out = Vec::with_capacity(rect.w * rect.h);
    for y in rect.y..rect.y1() {
        let row = y * width;
        out.extend_from_slice(&data[row + rect.x..row + rect.x1()]);
    }
    out
}

fn place_pixels<T: Copy + Default>(
    data: &[T],
    old_w: usize,
    old_h: usize,
    new_w: usize,
    new_h: usize,
    ox: i64,
    oy: i64,
) -> Vec<T> {
    let mut out = vec![T::default(); new_w * new_h];
    for y in 0..old_h {
        for x in 0..old_w {
            let dx = ox + x as i64;
            let dy = oy + y as i64;
            if let (Ok(dx), Ok(dy)) = (usize::try_from(dx), usize::try_from(dy)) {
                if dx < new_w && dy < new_h {
                    out[dy * new_w + dx] = data[y * old_w + x];
                }
            }
        }
    }
    out
}

fn flip_pixels<T: Copy>(data: &[T], width: usize, height: usize, horizontal: bool) -> Vec<T> {
    let mut out = data.to_vec();
    for y in 0..height {
        for x in 0..width {
            let (sx, sy) = if horizontal {
                (width - 1 - x, y)
            } else {
                (x, height - 1 - y)
            };
            out[y * width + x] = data[sy * width + sx];
        }
    }
    out
}
