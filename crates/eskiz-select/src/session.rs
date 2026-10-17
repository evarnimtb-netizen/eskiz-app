//! Transform sessions: one source snapshot, an initial mapping and an accumulated
//! mapping. Preview and commit both resample the original source once.
//!
//! A session can also enter warp mode. Then the mesh is the mapping: its control
//! points are in canvas coordinates, gestures move them, and render, marquee and commit all go
//! through the mesh.

use crate::error::{Result, SelectError};
use crate::geom::{rect_to_quad, Mat3, Point, Quad, Rect};
use crate::geom::IRect;
use crate::image::{over, premul8_from_straight, straight_from_premul8, Image, Rgba};
use crate::mask::Mask;
use crate::resample::{dest_region, map_point, par_rows, resample_bounded, resample_mask, Interp, Sampler};
use crate::warp::{embed, Warp};

/// A transform in progress on one target's pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformSession {
    /// The target's pixels at session start, canvas-sized.
    source: Source,
    /// Area cleared from the layer on commit: the selection value, or 1 where the layer has pixels.
    footprint: Footprint,
    /// Bounding box of the source pixels, in source coordinates.
    source_rect: Rect,
    /// Mapping in force when the session started (identity for a fresh layer).
    initial: Mat3,
    /// Composition of the affine gestures made in this session.
    accumulated: Mat3,
    interp: Interp,
    /// Warp mesh when the session is in warp mode. Its control points are in canvas coordinates.
    mesh: Option<Warp>,
    /// The `bounds` of the source as integer pixels (`source_rect` as a rectangle).
    bounds: IRect,
    /// A byte-backed session's source box-averaged by a factor, for [`Self::preview_premul`].
    small: Option<Small>,
}

/// What a session transforms.
#[derive(Debug, Clone, PartialEq)]
enum Source {
    Float(Image),
    /// Premultiplied RGBA8, canvas-sized.
    Bytes { width: usize, height: usize, pixels: Vec<u8> },
}

/// The area a commit clears from the layer.
#[derive(Debug, Clone, PartialEq)]
enum Footprint {
    /// Every pixel of the layer.
    Full,
    Mask(Mask),
}

impl Footprint {
    fn at(&self, i: usize) -> f32 {
        match self {
            Footprint::Full => 1.0,
            Footprint::Mask(m) => m.data[i],
        }
    }
}

/// A proxy of a byte-backed source: premultiplied RGBA8 averaged over `k x k` blocks, with the
/// selection already applied to its alpha.
#[derive(Debug, Clone, PartialEq)]
struct Small {
    k: usize,
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

/// Bounding box of the pixels that show: alpha above zero after the selection scales it.
fn byte_bounds(width: usize, height: usize, pixels: &[u8], selection: Option<&Mask>) -> Option<IRect> {
    let mut found: Option<(usize, usize, usize, usize)> = None;
    for y in 0..height {
        let row = &pixels[y * width * 4..(y + 1) * width * 4];
        for (x, p) in row.as_chunks::<4>().0.iter().enumerate() {
            if p[3] == 0 {
                continue;
            }
            if selection.is_some_and(|m| f32::from(p[3]) / 255.0 * m.data[y * width + x] <= 0.0) {
                continue;
            }
            found = Some(match found {
                None => (x, y, x + 1, y + 1),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
            });
        }
    }
    found.map(|(x0, y0, x1, y1)| IRect::from_bounds(x0, y0, x1, y1))
}

impl TransformSession {
    /// Starts a session on `source`. `footprint` must be the same size as `source`.
    /// Fails with `NoContent` when the source has no pixels.
    pub fn new(source: Image, footprint: Mask, initial: Mat3, interp: Interp) -> Result<Self> {
        if footprint.width != source.width || footprint.height != source.height {
            return Err(SelectError::SizeMismatch);
        }
        let bounds = source.alpha_bounds().ok_or(SelectError::NoContent)?;
        Ok(Self {
            source: Source::Float(source),
            footprint: Footprint::Mask(footprint),
            source_rect: Rect::from_irect(bounds),
            bounds,
            initial,
            accumulated: Mat3::IDENTITY,
            interp,
            mesh: None,
            small: None,
        })
    }

    /// Starts a session on premultiplied RGBA8 `pixels` (`width * height * 4` bytes) that the
    /// session keeps as they are (a quarter of the size of the float image, converted per pixel
    /// as it is sampled). `selection`, when given, is the footprint and also scales the alpha of
    /// the source; without it the whole layer moves. Renders the same pixels as [`Self::new`]
    /// would on the equivalent float image, and commits with [`Self::commit_premul`].
    /// Fails with `NoContent` when nothing in the source is visible.
    pub fn new_bytes(width: usize, height: usize, pixels: Vec<u8>, selection: Option<Mask>, initial: Mat3, interp: Interp) -> Result<Self> {
        if width.checked_mul(height).and_then(|n| n.checked_mul(4)) != Some(pixels.len()) {
            return Err(SelectError::SizeMismatch);
        }
        if selection.as_ref().is_some_and(|m| m.width != width || m.height != height) {
            return Err(SelectError::SizeMismatch);
        }
        let bounds = byte_bounds(width, height, &pixels, selection.as_ref()).ok_or(SelectError::NoContent)?;
        Ok(Self {
            source: Source::Bytes { width, height, pixels },
            footprint: selection.map_or(Footprint::Full, Footprint::Mask),
            source_rect: Rect::from_irect(bounds),
            bounds,
            initial,
            accumulated: Mat3::IDENTITY,
            interp,
            mesh: None,
            small: None,
        })
    }

    pub fn source_rect(&self) -> Rect {
        self.source_rect
    }

    pub fn interp(&self) -> Interp {
        self.interp
    }

    /// Changes the resampling kernel. The mapping and any warp mesh are kept.
    pub fn set_interp(&mut self, interp: Interp) {
        self.interp = interp;
    }

    /// True once the session is in warp mode.
    pub fn is_warp(&self) -> bool {
        self.mesh.is_some()
    }

    /// The warp mesh, in warp mode.
    pub fn warp(&self) -> Option<&Warp> {
        self.mesh.as_ref()
    }

    /// The warp mesh for editing control points, in warp mode.
    pub fn warp_mut(&mut self) -> Option<&mut Warp> {
        self.mesh.as_mut()
    }

    /// Switches the session to warp mode. The control grid is the source rectangle's even grid
    /// carried through the current mapping, so the picture does not jump. Entering warp mode
    /// twice has no further effect. Fails with `Singular` if the mapping sends a point to
    /// infinity.
    pub fn enter_warp(&mut self) -> Result<()> {
        if self.mesh.is_some() {
            return Ok(());
        }
        let mapping = self.mapping();
        let mut mesh = Warp::identity(self.source_rect);
        for column in mesh.ctrl.iter_mut() {
            for point in column.iter_mut() {
                *point = mapping.apply(*point).ok_or(SelectError::Singular)?;
            }
        }
        self.mesh = Some(mesh);
        Ok(())
    }

    /// Final affine mapping: `accumulated * initial`. In warp mode the mesh replaces
    /// it, and this returns the affine part only.
    pub fn mapping(&self) -> Mat3 {
        self.accumulated * self.initial
    }

    /// Applies a gesture. In affine mode it composes onto the accumulated matrix:
    /// `accumulated = gesture * accumulated`. In warp mode it moves every control point.
    /// Pixels are never resampled between gestures.
    pub fn apply_gesture(&mut self, gesture: Mat3) {
        match self.mesh.as_mut() {
            Some(mesh) => {
                for column in mesh.ctrl.iter_mut() {
                    for point in column.iter_mut() {
                        *point = gesture.apply(*point).unwrap_or(*point);
                    }
                }
            }
            None => self.accumulated = gesture * self.accumulated,
        }
    }

    /// Sets the affine mapping directly. Fails with `Singular` when the initial mapping cannot
    /// be inverted, and with `WrongMode` in warp mode (edit control points instead).
    pub fn set_mapping(&mut self, mapping: Mat3) -> Result<()> {
        if self.mesh.is_some() {
            return Err(SelectError::WrongMode);
        }
        let inverse = self.initial.inverse().ok_or(SelectError::Singular)?;
        self.accumulated = mapping * inverse;
        Ok(())
    }

    /// Sets the quad that the source rectangle maps to. This is how free, uniform and distort
    /// edits feed the session. Fails with `WrongMode` in warp mode.
    pub fn set_quad(&mut self, quad: &Quad) -> Result<()> {
        if self.mesh.is_some() {
            return Err(SelectError::WrongMode);
        }
        let mapping = rect_to_quad(self.source_rect, quad).ok_or(SelectError::Singular)?;
        self.set_mapping(mapping)
    }

    /// The current quad. In affine mode, the source rectangle's corners through the mapping. In
    /// warp mode, the mesh's four corner control points.
    pub fn quad(&self) -> Option<Quad> {
        if let Some(mesh) = &self.mesh {
            let c = &mesh.ctrl;
            return Some([c[0][0], c[3][0], c[3][3], c[0][3]]);
        }
        let m = self.mapping();
        let mut out = [Point::default(); 4];
        for (o, corner) in out.iter_mut().zip(self.source_rect.corners()) {
            *o = m.apply(corner)?;
        }
        Some(out)
    }

    fn dims(&self) -> (usize, usize) {
        match &self.source {
            Source::Float(img) => (img.width, img.height),
            Source::Bytes { width, height, .. } => (*width, *height),
        }
    }

    /// The selection outline (marquee) moved with the content: the mask resampled through the
    /// current mapping (or the mesh in warp mode), at canvas size.
    pub fn marquee(&self, selection: &Mask) -> Mask {
        let (w, h) = self.dims();
        match &self.mesh {
            Some(mesh) => mesh.render_mask(selection, self.source_rect, w, h, self.interp),
            None => resample_mask(selection, &self.mapping(), w, h, self.interp),
        }
    }

    /// The scale factor of the selection on a byte-backed source's alpha, when there is one.
    fn alpha_scale(&self) -> Option<&[f32]> {
        match &self.footprint {
            Footprint::Full => None,
            Footprint::Mask(m) => Some(&m.data),
        }
    }

    fn sampler(&self) -> Sampler<'_> {
        match &self.source {
            Source::Float(img) => Sampler::new(img),
            Source::Bytes { width, height, pixels } => Sampler::from_premul_u8(*width, *height, pixels, self.alpha_scale()),
        }
    }

    /// Preview: the source resampled through the current mapping, or through the mesh in warp
    /// mode, at canvas size. The layer itself is not touched. (A byte-backed session builds a
    /// canvas-sized float image here; large canvases preview with [`Self::preview_premul`].)
    pub fn render(&self) -> Image {
        let (w, h) = self.dims();
        let sampler = self.sampler();
        match &self.mesh {
            Some(mesh) => {
                let (region, part) = mesh.render_region(&sampler, self.source_rect, w, h, self.interp);
                embed(region, part, w, h)
            }
            None => resample_bounded(&sampler, Some(self.bounds), &self.mapping(), w, h, self.interp),
        }
    }

    /// Commit: returns the layer with the footprint cleared and the single resample of the
    /// original source written over it. The input layer is not modified. Dropping the session
    /// without calling this is a cancel.
    pub fn commit(&self, layer: &Image) -> Result<Image> {
        let (w, h) = self.dims();
        if layer.width != w || layer.height != h {
            return Err(SelectError::SizeMismatch);
        }
        let rendered = self.render();
        let mut out = layer.clone();
        for (i, px) in out.pixels.iter_mut().enumerate() {
            px[3] *= (1.0 - self.footprint.at(i)).clamp(0.0, 1.0);
            if px[3] <= 0.0 {
                *px = [0.0; 4];
            }
        }
        for (dst, src) in out.pixels.iter_mut().zip(rendered.pixels.iter()) {
            *dst = over(*src, *dst);
        }
        Ok(out)
    }

    /// [`Self::commit`] on premultiplied RGBA8 for a byte-backed session, without any
    /// canvas-sized float image: `layer` is the layer's pixels at session start (needed only
    /// when a selection leaves part of the layer in place; `None` clears the whole layer).
    /// Each pixel goes through exactly the conversions and blend of [`Self::commit`], so the
    /// result is the same bytes; rows run on several threads, only the area the content reaches
    /// is resampled, and pixels that nothing touches are copied.
    pub fn commit_premul(&self, layer: Option<&[u8]>) -> Result<Vec<u8>> {
        let Source::Bytes { width: w, height: h, .. } = &self.source else {
            return Err(SelectError::WrongMode);
        };
        let (w, h) = (*w, *h);
        match (&self.footprint, layer) {
            (_, Some(l)) if l.len() != w * h * 4 => return Err(SelectError::SizeMismatch),
            (Footprint::Mask(_), None) => return Err(SelectError::SizeMismatch),
            _ => {}
        }
        let sampler = self.sampler();
        // What the content draws: through the affine inverse, or the mesh's rendered region.
        let (region, warped, inverse) = match &self.mesh {
            Some(mesh) => {
                let (region, part) = mesh.render_region(&sampler, self.source_rect, w, h, self.interp);
                (region, Some(part), None)
            }
            None => match self.mapping().inverse() {
                Some(inv) => (dest_region(&self.mapping(), self.bounds, w, h), None, Some(inv)),
                None => (IRect { x: 0, y: 0, w: 0, h: 0 }, None, None),
            },
        };
        let mut out = vec![0u8; w * h * 4];
        par_rows(&mut out, w * 4, |first, band| {
            for (r, orow) in band.chunks_mut(w * 4).enumerate() {
                let y = first + r;
                let in_rows = y >= region.y && y < region.y1();
                for (x, dst) in orow.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let i = y * w + x;
                    let f = self.footprint.at(i);
                    let inside = in_rows && x >= region.x && x < region.x1();
                    if !inside && f == 0.0 {
                        // Untouched: the float round trip is the identity (see the tests).
                        if let Some(l) = layer {
                            dst.copy_from_slice(&l[i * 4..i * 4 + 4]);
                        }
                        continue;
                    }
                    let src: Rgba = if !inside {
                        [0.0; 4]
                    } else if let Some(part) = &warped {
                        part.pixels[(y - region.y) * region.w + (x - region.x)]
                    } else if let Some(inv) = &inverse {
                        map_point(inv, Point::new(x as f32 + 0.5, y as f32 + 0.5)).map_or([0.0; 4], |p| sampler.sample(p, self.interp))
                    } else {
                        [0.0; 4]
                    };
                    let mut base = layer.map_or([0.0; 4], |l| straight_from_premul8([l[i * 4], l[i * 4 + 1], l[i * 4 + 2], l[i * 4 + 3]]));
                    base[3] *= (1.0 - f).clamp(0.0, 1.0);
                    if base[3] <= 0.0 {
                        base = [0.0; 4];
                    }
                    *dst = premul8_from_straight(over(src, base));
                }
            }
        });
        Ok(out)
    }

    /// A low-resolution render of the moved content for the live preview of a large canvas:
    /// the source averaged over `k x k` blocks (built once per `k`), resampled with `interp`
    /// through the mapping (or mesh) scaled to match, as premultiplied RGBA8 of size
    /// `ceil(width / k) x ceil(height / k)`. Returns `(width, height, pixels)`. The layer
    /// outside the footprint is not included. Byte-backed sessions only.
    pub fn preview_premul(&mut self, k: usize, interp: Interp) -> Result<(usize, usize, Vec<u8>)> {
        let k = k.max(1);
        let Source::Bytes { width: w, height: h, pixels } = &self.source else {
            return Err(SelectError::WrongMode);
        };
        let (w, h) = (*w, *h);
        if self.small.as_ref().is_none_or(|s| s.k != k) {
            let (pw, ph) = (w.div_ceil(k), h.div_ceil(k));
            let mut out = vec![0u8; pw * ph * 4];
            let scale = self.alpha_scale();
            par_rows(&mut out, pw * 4, |first, band| {
                for (r, orow) in band.chunks_mut(pw * 4).enumerate() {
                    let py = first + r;
                    let (y0, y1) = (py * k, ((py + 1) * k).min(h));
                    for (px, o) in orow.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                        let (x0, x1) = (px * k, ((px + 1) * k).min(w));
                        let mut acc = [0.0f32; 4];
                        for y in y0..y1 {
                            for x in x0..x1 {
                                let i = y * w + x;
                                let s = scale.map_or(1.0, |m| m[i]);
                                for (c, a) in acc.iter_mut().enumerate() {
                                    *a += f32::from(pixels[i * 4 + c]) * s;
                                }
                            }
                        }
                        let n = ((y1 - y0) * (x1 - x0)) as f32;
                        for (c, a) in acc.iter().enumerate() {
                            o[c] = (a / n + 0.5) as u8;
                        }
                    }
                }
            });
            self.small = Some(Small { k, width: pw, height: ph, pixels: out });
        }
        let Some(small) = self.small.as_ref() else {
            return Err(SelectError::NoContent);
        };
        let (pw, ph) = (small.width, small.height);
        let sampler = Sampler::from_premul_u8(pw, ph, &small.pixels, None);
        let inv_k = 1.0 / k as f32;
        let bounds = IRect::from_bounds(self.bounds.x / k, self.bounds.y / k, self.bounds.x1().div_ceil(k).min(pw), self.bounds.y1().div_ceil(k).min(ph));
        let image = match &self.mesh {
            Some(mesh) => {
                let (region, part) = mesh.scaled(inv_k).render_region(&sampler, Rect::new(self.source_rect.x0 * inv_k, self.source_rect.y0 * inv_k, self.source_rect.x1 * inv_k, self.source_rect.y1 * inv_k), pw, ph, interp);
                embed(region, part, pw, ph)
            }
            None => {
                let mapping = Mat3::scale(inv_k, inv_k) * self.mapping() * Mat3::scale(k as f32, k as f32);
                resample_bounded(&sampler, Some(bounds), &mapping, pw, ph, interp)
            }
        };
        Ok((pw, ph, image.to_premul_rgba8()))
    }
}
