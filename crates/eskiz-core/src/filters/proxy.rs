//! Proxy previews: a slider drag renders the adjustment on a box-downscaled copy of the layer
//! (about the size of the screen) and scales the result back up, instead of rendering every pixel
//! of a large canvas for every change. The commit never uses a proxy.
//!
//! Only types whose parameters do not depend on the pixel size take part (see [`proxy_safe`]):
//! the per-pixel types, and the ones whose parameters are fractions of the image. Gaussian blur
//! scales its kernel by the proxy factor. The rest (sharpen, noise, halftone, bloom, clone and the
//! Pencil-mode pyramid blur work in whole pixels) render at full size as before.

use std::sync::Arc;

use super::{downscale, gaussian, thread_count, AdjustKind, Adjustment, Quality};
use crate::canvas::{Layer, Tile, TILE, TILE_SAMPLES};

/// Longest side, in pixels, that a proxy aims for.
pub const PROXY_EDGE: usize = 1600;

/// Canvases up to this longest side are previewed at full size (a threaded render is quick enough).
pub const PROXY_MIN_EDGE: usize = 2048;

/// Do the parameters of this type mean the same on a downscaled copy of the layer?
pub fn proxy_safe(kind: AdjustKind, pencil: bool) -> bool {
    use AdjustKind::*;
    match kind {
        Opacity | Hsb | Curves | GradientMap | Recolor | ColourBalance | MotionBlur | PerspectiveBlur | ChromaticAberration | Glitch => true,
        // Only the Layer-mode Gaussian blur is the real blur; Pencil mode is the pyramid blur.
        GaussianBlur => !pencil,
        Sharpen | Noise | Halftone | Bloom | Clone => false,
    }
}

/// The downscale factor for a preview of `kind` on a `w x h` canvas shown at `zoom` (screen pixels
/// per canvas pixel). 1 means "render at full size". The proxy is about [`PROXY_EDGE`] pixels
/// across, and never coarser than the screen shows the canvas.
pub fn proxy_factor(kind: AdjustKind, pencil: bool, w: usize, h: usize, zoom: f32) -> usize {
    let edge = w.max(h);
    if !proxy_safe(kind, pencil) || edge <= PROXY_MIN_EDGE {
        return 1;
    }
    let by_size = edge.div_ceil(PROXY_EDGE);
    let by_zoom = if zoom.is_finite() && zoom > 0.0 { (1.0 / zoom).floor().max(1.0) as usize } else { 1 };
    by_size.min(by_zoom).max(1)
}

/// A layer (and selection) box-downscaled by `k`.
#[derive(Clone, Debug)]
pub struct Proxy {
    pub k: usize,
    /// Proxy size.
    pub width: usize,
    pub height: usize,
    /// Premultiplied RGBA8 at the proxy size.
    pub pixels: Vec<u8>,
    /// The selection mask at the proxy size, when there is one.
    pub selection: Option<Vec<f32>>,
}

impl Proxy {
    /// Downscale `src` (and `selection`) of a `w x h` canvas by `k`.
    pub fn build(src: &[u8], selection: Option<&[f32]>, w: usize, h: usize, k: usize) -> Self {
        let (pixels, width, height) = downscale(src, w, h, k);
        let selection = selection.map(|m| downscale_mask(m, w, h, k).0);
        Self { k, width, height, pixels, selection }
    }
}

/// Box-average a one-channel mask by `k`. Returns the values and the size.
pub fn downscale_mask(mask: &[f32], w: usize, h: usize, k: usize) -> (Vec<f32>, usize, usize) {
    let k = k.max(1);
    let (pw, ph) = (w.div_ceil(k), h.div_ceil(k));
    let mut out = vec![0.0f32; pw * ph];
    super::par_out(&mut out, pw, |first, o| {
        for (r, orow) in o.chunks_mut(pw).enumerate() {
            let py = first + r;
            let (y0, y1) = (py * k, ((py + 1) * k).min(h));
            for (px, v) in orow.iter_mut().enumerate() {
                let (x0, x1) = (px * k, ((px + 1) * k).min(w));
                let mut acc = 0.0f32;
                for y in y0..y1 {
                    acc += mask[y * w + x0..y * w + x1].iter().sum::<f32>();
                }
                *v = acc / ((y1 - y0) * (x1 - x0)) as f32;
            }
        }
    });
    (out, pw, ph)
}

/// Render the preview of `adj` from `proxy` as a canvas-sized layer: the proxy result scaled up
/// straight into tiles (no canvas-sized buffer in between). `paint` is the full-size Pencil paint,
/// if any; it is downscaled here and multiplied into the proxy's selection.
pub fn render_preview(adj: &Adjustment, proxy: &Proxy, paint: Option<&[f32]>, w: usize, h: usize) -> Layer {
    let paint = paint.map(|p| downscale_mask(p, w, h, proxy.k).0);
    let mask: Option<Vec<f32>> = match (paint, proxy.selection.as_deref()) {
        (None, None) => None,
        (Some(p), None) => Some(p),
        (None, Some(s)) => Some(s.to_vec()),
        (Some(p), Some(s)) => Some(p.iter().zip(s).map(|(a, b)| a * b).collect()),
    };
    let small = if adj.kind == AdjustKind::GaussianBlur && !adj.pencil {
        gaussian::blur_scaled(&proxy.pixels, proxy.width, proxy.height, adj.value(0), proxy.k, mask.as_deref())
    } else {
        adj.render(&proxy.pixels, proxy.width, proxy.height, Quality::Preview, None, mask.as_deref())
    };
    upscale_layer(&small, proxy.width, proxy.height, w, h, proxy.k)
}

/// Scale the `pw x ph` proxy `src` (made with factor `k`) up to a `w x h` layer by repeating each
/// proxy pixel as a `k x k` block, writing each 256-pixel tile directly; tiles that come out
/// transparent are not allocated. Tiles are filled on several threads.
///
/// Blocks, not a smooth filter: the proxy factor never exceeds `1 / zoom`, so a block is at most
/// one screen pixel wide and the display shows no difference, and a block copy is several times
/// cheaper than interpolating 64 million pixels.
pub fn upscale_layer(src: &[u8], pw: usize, ph: usize, w: usize, h: usize, k: usize) -> Layer {
    upscale_layer_over(src, pw, ph, w, h, k, None)
}

/// [`upscale_layer`] composited over `base` (an 8-bit layer of the same size): a tile the proxy
/// leaves empty is the base's tile itself (shared, not copied), and elsewhere the proxy pixel
/// goes over the base pixel (premultiplied source-over, rounded).
pub fn upscale_layer_over(src: &[u8], pw: usize, ph: usize, w: usize, h: usize, k: usize, base: Option<&Layer>) -> Layer {
    let mut layer = Layer::new(w, h);
    let (tiles_x, tiles_y) = (layer.tiles_x(), layer.tiles_y());
    let coords: Vec<(usize, usize)> = (0..tiles_y).flat_map(|ty| (0..tiles_x).map(move |tx| (tx, ty))).collect();
    let n = thread_count(w * h * 4, coords.len());
    let per = coords.len().div_ceil(n.max(1)).max(1);
    let fill = |chunk: &[(usize, usize)]| -> Vec<(usize, usize, Arc<Tile>)> {
        let mut made = Vec::new();
        for &(tx, ty) in chunk {
            let (x0, y0) = (tx * TILE, ty * TILE);
            let (tw, th) = (TILE.min(w - x0), TILE.min(h - y0));
            let under = base.and_then(|b| b.tile(tx, ty));
            let mut data = vec![0u8; TILE_SAMPLES];
            repeat_block_rect(src, pw, ph, k, (x0, y0, tw, th), &mut data, TILE);
            if data.iter().all(|&v| v == 0) {
                // Nothing of the moved content here: the base tile stays as it is.
                if let Some(u) = under {
                    made.push((tx, ty, Arc::clone(u)));
                }
                continue;
            }
            if let Some(u) = under {
                for (d, b) in data.as_chunks_mut::<4>().0.iter_mut().zip(u.data().as_chunks::<4>().0) {
                    let keep = 255 - u32::from(d[3]);
                    if keep > 0 {
                        for c in 0..4 {
                            d[c] = (u32::from(d[c]) + (u32::from(b[c]) * keep + 127) / 255).min(255) as u8;
                        }
                    }
                }
            }
            if let Some(t) = Tile::from_u8(data) {
                made.push((tx, ty, Arc::new(t)));
            }
        }
        made
    };
    let made: Vec<Vec<(usize, usize, Arc<Tile>)>> = if n <= 1 {
        vec![fill(&coords)]
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = coords.chunks(per).map(|c| scope.spawn(|| fill(c))).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
        })
    };
    for (tx, ty, t) in made.into_iter().flatten() {
        layer.set_tile(tx, ty, Some(t));
    }
    layer
}

/// Fill the canvas rectangle `(x0, y0, w, h)` of `out` (RGBA8, `stride` pixels per row) with the
/// proxy `src`'s pixels repeated `k x k`: output pixel `(x, y)` is proxy pixel `(x / k, y / k)`.
pub fn repeat_block_rect(src: &[u8], pw: usize, ph: usize, k: usize, (x0, y0, w, h): (usize, usize, usize, usize), out: &mut [u8], stride: usize) {
    let k = k.max(1);
    for r in 0..h {
        let sy = ((y0 + r) / k).min(ph - 1);
        let srow = &src[sy * pw * 4..(sy + 1) * pw * 4];
        // Rows of one block row are identical: copy the first one.
        if r > 0 && (y0 + r) % k != 0 {
            let (done, rest) = out.split_at_mut(r * stride * 4);
            rest[..w * 4].copy_from_slice(&done[(r - 1) * stride * 4..][..w * 4]);
            continue;
        }
        let orow = &mut out[r * stride * 4..][..w * 4];
        for (i, px) in orow.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let sx = ((x0 + i) / k).min(pw - 1);
            px.copy_from_slice(&srow[sx * 4..sx * 4 + 4]);
        }
    }
}
