//! CPU rasteriser producing the screen-to-UV lookup buffer.
//!
//! Screen convention: pixel `(0, 0)` is the top-left; `y` grows downward; pixel
//! `(x, y)` has its centre at `(x + 0.5, y + 0.5)`. The UV, depth and triangle
//! buffers share one index, so there is no Y flip between lookup and depth.

use glam::{Vec2, Vec3, Vec4};

use crate::camera::OrbitCamera;
use crate::model::Model;

/// Marker for "no triangle here".
pub const NO_TRI: u32 = u32::MAX;

/// Which surface layer to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Nearest surface under the pixel.
    Front,
    /// Farthest surface under the pixel (only with [`RenderOptions::back_layer`]).
    Back,
}

/// Rasteriser options.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RenderOptions {
    /// Also keep the farthest surface per pixel, for paint-through.
    pub back_layer: bool,
    /// Drop triangles that face away from the camera (counter-clockwise = front).
    pub cull_backfaces: bool,
}

/// One rasterised (possibly near-clipped) triangle, kept for UV derivatives.
#[derive(Clone, Copy, Debug)]
pub struct ScreenTri {
    /// Global model triangle id.
    pub tri: u32,
    p: [Vec2; 3],
    iw: [f32; 3],
    uv: [Vec2; 3],
}

/// Per-pixel buffers of one surface layer.
#[derive(Clone, Debug)]
pub struct SurfaceBuffer {
    /// `(u, v)` as 16-bit normalised pairs, 65535 = 1.0. Zero where uncovered.
    pub uv: Vec<[u16; 2]>,
    /// NDC depth in 0..1 (0 near). Infinite where uncovered.
    pub depth: Vec<f32>,
    /// Index into [`LookupBuffer::tris`], or [`NO_TRI`].
    pub st: Vec<u32>,
}

impl SurfaceBuffer {
    fn new(n: usize, back: bool) -> Self {
        Self {
            uv: vec![[0, 0]; n],
            depth: vec![
                if back {
                    f32::NEG_INFINITY
                } else {
                    f32::INFINITY
                };
                n
            ],
            st: vec![NO_TRI; n],
        }
    }
}

/// What a lookup read returns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// Decoded UV in 0..1 (from the 16-bit pair).
    pub u: f32,
    pub v: f32,
    /// NDC depth (0 near, 1 far).
    pub depth: f32,
    /// Global triangle id.
    pub tri: u32,
    /// UV change per screen pixel along +x: `(du/dx, dv/dx)`.
    pub duv_dx: Vec2,
    /// UV change per screen pixel along +y: `(du/dy, dv/dy)`.
    pub duv_dy: Vec2,
}

/// The lookup buffer: a screen-sized map from pixels to UVs, depth and triangle ids.
#[derive(Clone, Debug)]
pub struct LookupBuffer {
    pub width: usize,
    pub height: usize,
    pub front: SurfaceBuffer,
    pub back: Option<SurfaceBuffer>,
    pub tris: Vec<ScreenTri>,
}

fn encode(u: f32) -> u16 {
    (u.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// Wrap UVs outside 0..1 (texture repeating), keeping exactly 1.0 as 1.0.
fn wrap(u: f32) -> f32 {
    if (0.0..=1.0).contains(&u) {
        u
    } else {
        u - u.floor()
    }
}

impl LookupBuffer {
    fn pixel_index(&self, x: f32, y: f32) -> usize {
        let xi = (x.floor().max(0.0) as usize).min(self.width.saturating_sub(1));
        let yi = (y.floor().max(0.0) as usize).min(self.height.saturating_sub(1));
        yi * self.width + xi
    }

    fn layer(&self, side: Side) -> Option<&SurfaceBuffer> {
        match side {
            Side::Front => Some(&self.front),
            Side::Back => self.back.as_ref(),
        }
    }

    /// Read the lookup at screen point `(x, y)` (clamped to the viewport, nearest texel).
    pub fn hit(&self, x: f32, y: f32, side: Side) -> Option<Hit> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let i = self.pixel_index(x, y);
        let buf = self.layer(side)?;
        let st = buf.st[i];
        if st == NO_TRI {
            return None;
        }
        let px = (i % self.width) as f32 + 0.5;
        let py = (i / self.width) as f32 + 0.5;
        let t = &self.tris[st as usize];
        let (duv_dx, duv_dy) = t.derivatives(Vec2::new(px, py));
        Some(Hit {
            u: f32::from(buf.uv[i][0]) / 65535.0,
            v: f32::from(buf.uv[i][1]) / 65535.0,
            depth: buf.depth[i],
            tri: t.tri,
            duv_dx,
            duv_dy,
        })
    }

    /// `(u, v)` of the front surface under `(x, y)`, or `None` if the model does not cover it.
    pub fn remap(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let i = self.pixel_index(x, y);
        if self.width == 0 || self.front.st[i] == NO_TRI {
            return None;
        }
        Some((
            f32::from(self.front.uv[i][0]) / 65535.0,
            f32::from(self.front.uv[i][1]) / 65535.0,
        ))
    }

    /// NDC depth of the front surface under `(x, y)`.
    pub fn depth_at(&self, x: f32, y: f32) -> Option<f32> {
        let i = self.pixel_index(x, y);
        if self.width == 0 || self.front.st[i] == NO_TRI {
            return None;
        }
        Some(self.front.depth[i])
    }

    /// Global triangle id of the front surface under `(x, y)`.
    pub fn triangle_at(&self, x: f32, y: f32) -> Option<u32> {
        let i = self.pixel_index(x, y);
        if self.width == 0 || self.front.st[i] == NO_TRI {
            return None;
        }
        Some(self.tris[self.front.st[i] as usize].tri)
    }
}

impl ScreenTri {
    fn bary_area(&self) -> f32 {
        edge(self.p[0], self.p[1], self.p[2])
    }

    /// Analytic perspective-correct UV derivatives at screen point `q`.
    fn derivatives(&self, q: Vec2) -> (Vec2, Vec2) {
        let area = self.bary_area();
        if area.abs() < 1e-12 {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        let [p0, p1, p2] = self.p;
        let l = [
            edge(p1, p2, q) / area,
            edge(p2, p0, q) / area,
            edge(p0, p1, q) / area,
        ];
        // d(lambda_i)/dx and /dy.
        let dlx = [
            -(p2.y - p1.y) / area,
            -(p0.y - p2.y) / area,
            -(p1.y - p0.y) / area,
        ];
        let dly = [
            (p2.x - p1.x) / area,
            (p0.x - p2.x) / area,
            (p1.x - p0.x) / area,
        ];
        let mut d = 0.0;
        let (mut dx, mut dy) = (0.0, 0.0);
        let mut n = Vec2::ZERO;
        let (mut nx, mut ny) = (Vec2::ZERO, Vec2::ZERO);
        for i in 0..3 {
            d += l[i] * self.iw[i];
            dx += dlx[i] * self.iw[i];
            dy += dly[i] * self.iw[i];
            n += l[i] * self.iw[i] * self.uv[i];
            nx += dlx[i] * self.iw[i] * self.uv[i];
            ny += dly[i] * self.iw[i] * self.uv[i];
        }
        if d.abs() < 1e-20 {
            return (Vec2::ZERO, Vec2::ZERO);
        }
        let uv = n / d;
        ((nx - uv * dx) / d, (ny - uv * dy) / d)
    }
}

fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

#[derive(Clone, Copy)]
struct ClipVert {
    pos: Vec4,
    uv: Vec2,
}

/// Clip a triangle against the near plane (clip-space z >= 0).
fn clip_near(tri: [ClipVert; 3], out: &mut Vec<ClipVert>) {
    out.clear();
    for i in 0..3 {
        let a = tri[i];
        let b = tri[(i + 1) % 3];
        let (da, db) = (a.pos.z, b.pos.z);
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(ClipVert {
                pos: a.pos.lerp(b.pos, t),
                uv: a.uv.lerp(b.uv, t),
            });
        }
    }
}

/// Rasterise `model` from `camera` into a `width x height` lookup buffer.
pub fn render_lookup(
    model: &Model,
    camera: &OrbitCamera,
    width: usize,
    height: usize,
    opts: RenderOptions,
) -> LookupBuffer {
    let n = width * height;
    let mut buf = LookupBuffer {
        width,
        height,
        front: SurfaceBuffer::new(n, false),
        back: opts.back_layer.then(|| SurfaceBuffer::new(n, true)),
        tris: Vec::new(),
    };
    if n == 0 {
        return buf;
    }
    let vp = camera.view_projection(width as f32 / height as f32);
    let mut poly = Vec::with_capacity(6);
    for (si, sm) in model.submeshes.iter().enumerate() {
        if sm.uvs.is_empty() {
            continue;
        }
        let base = model.tri_offset(si);
        for t in 0..sm.triangle_count() {
            let p = sm.tri_positions(t);
            let uv = sm.tri_uvs(t);
            let cv = |k: usize| ClipVert {
                pos: vp * p[k].extend(1.0),
                uv: uv[k],
            };
            clip_near([cv(0), cv(1), cv(2)], &mut poly);
            for k in 1..poly.len().saturating_sub(1) {
                raster_triangle(
                    &mut buf,
                    [poly[0], poly[k], poly[k + 1]],
                    base + t as u32,
                    opts,
                );
            }
        }
    }
    buf
}

fn to_screen(v: Vec4, w: f32, h: f32) -> (Vec2, f32, f32) {
    let iw = 1.0 / v.w;
    let ndc = Vec3::new(v.x * iw, v.y * iw, v.z * iw);
    (
        Vec2::new((ndc.x * 0.5 + 0.5) * w, (0.5 - ndc.y * 0.5) * h),
        ndc.z,
        iw,
    )
}

fn raster_triangle(buf: &mut LookupBuffer, v: [ClipVert; 3], tri: u32, opts: RenderOptions) {
    let (w, h) = (buf.width as f32, buf.height as f32);
    let s = [
        to_screen(v[0].pos, w, h),
        to_screen(v[1].pos, w, h),
        to_screen(v[2].pos, w, h),
    ];
    let p = [s[0].0, s[1].0, s[2].0];
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-9 || !area.is_finite() {
        return;
    }
    // Screen y points down, so a counter-clockwise triangle has negative area here.
    if opts.cull_backfaces && area > 0.0 {
        return;
    }
    let min = p[0].min(p[1]).min(p[2]);
    let max = p[0].max(p[1]).max(p[2]);
    let x0 = (min.x - 0.5).ceil().max(0.0) as usize;
    let y0 = (min.y - 0.5).ceil().max(0.0) as usize;
    let x1 = ((max.x - 0.5).floor() + 1.0).clamp(0.0, w) as usize;
    let y1 = ((max.y - 0.5).floor() + 1.0).clamp(0.0, h) as usize;
    let z = [s[0].1, s[1].1, s[2].1];
    let iw = [s[0].2, s[1].2, s[2].2];
    let uv = [v[0].uv, v[1].uv, v[2].uv];
    let mut id: Option<u32> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let l = [
                edge(p[1], p[2], q) / area,
                edge(p[2], p[0], q) / area,
                edge(p[0], p[1], q) / area,
            ];
            if l[0] < 0.0 || l[1] < 0.0 || l[2] < 0.0 {
                continue;
            }
            let depth = l[0] * z[0] + l[1] * z[1] + l[2] * z[2];
            if !(0.0..=1.0).contains(&depth) {
                continue;
            }
            let i = y * buf.width + x;
            let is_front = depth < buf.front.depth[i];
            let is_back = buf.back.as_ref().is_some_and(|b| depth > b.depth[i]);
            if !is_front && !is_back {
                continue;
            }
            let sid = *id.get_or_insert_with(|| {
                buf.tris.push(ScreenTri { tri, p, iw, uv });
                (buf.tris.len() - 1) as u32
            });
            let d = l[0] * iw[0] + l[1] * iw[1] + l[2] * iw[2];
            let puv = (l[0] * iw[0] * uv[0] + l[1] * iw[1] * uv[1] + l[2] * iw[2] * uv[2]) / d;
            let enc = [encode(wrap(puv.x)), encode(wrap(puv.y))];
            if is_front {
                buf.front.depth[i] = depth;
                buf.front.uv[i] = enc;
                buf.front.st[i] = sid;
            }
            if is_back {
                if let Some(b) = buf.back.as_mut() {
                    b.depth[i] = depth;
                    b.uv[i] = enc;
                    b.st[i] = sid;
                }
            }
        }
    }
}
