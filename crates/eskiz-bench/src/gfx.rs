//! The display half, driven headlessly: an `egui_wgpu::Renderer` on Eskiz's own `GpuContext`
//! device, so the benchmark can time what the window does to show the canvas:
//!
//! * the CPU path: a `ColorImage` of the flattened canvas (or one rectangle of it) uploaded as
//!   an egui texture (`TextureHandle::set` / `set_partial` in `app.rs`);
//! * the GPU path: the compositor's output texture registered as a native egui texture
//!   (`gpu_display.rs`);
//! * the draw: the one textured quad `draw_canvas` emits, rendered at a zoom into a
//!   viewport-sized target.

use std::sync::Arc;

use eframe::egui::epaint::{self, ClippedPrimitive, Color32, ImageDelta, Mesh, Pos2, Primitive, Rect, TextureId, Vertex};
use eframe::egui::TextureOptions;
use eframe::egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use eframe::wgpu;
use eskiz_gpu::GpuContext;

use crate::report::ms_since;

/// The viewport the quad is drawn into: a 2560 x 1440 window at 1 pixel per point (a 13"
/// laptop at 2x is a similar pixel count).
pub const VIEW_W: u32 = 2560;
pub const VIEW_H: u32 = 1440;

pub struct Display {
    pub renderer: Renderer,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
}

impl Display {
    pub fn new(ctx: &GpuContext) -> Self {
        let renderer = Renderer::new(&ctx.device, wgpu::TextureFormat::Rgba8Unorm, RendererOptions::default());
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench-viewport"),
            size: wgpu::Extent3d { width: VIEW_W, height: VIEW_H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        Self { renderer, target, target_view }
    }

    /// Upload a whole straight-RGBA8 image as the canvas texture, as `sync_full` does.
    /// Returns the texture id and `(convert_ms, upload_ms)`.
    pub fn upload_full(&mut self, ctx: &GpuContext, w: usize, h: usize, rgba: &[u8]) -> (TextureId, f64, f64) {
        let t = std::time::Instant::now();
        let image = epaint::ColorImage::from_rgba_unmultiplied([w, h], rgba);
        let convert = ms_since(t);
        let id = TextureId::User(1);
        let delta = ImageDelta::full(epaint::ImageData::Color(Arc::new(image)), TextureOptions::LINEAR);
        let t = std::time::Instant::now();
        self.renderer.update_texture(&ctx.device, &ctx.queue, id, &delta);
        ctx.wait_idle();
        (id, convert, ms_since(t))
    }

    /// Upload one rectangle into the canvas texture, as `refresh` does. Returns `(convert_ms, upload_ms)`.
    pub fn upload_partial(&mut self, ctx: &GpuContext, id: TextureId, pos: [usize; 2], w: usize, h: usize, rgba: &[u8]) -> (f64, f64) {
        let t = std::time::Instant::now();
        let image = epaint::ColorImage::from_rgba_unmultiplied([w, h], rgba);
        let convert = ms_since(t);
        let delta = ImageDelta::partial(pos, epaint::ImageData::Color(Arc::new(image)), TextureOptions::LINEAR);
        let t = std::time::Instant::now();
        self.renderer.update_texture(&ctx.device, &ctx.queue, id, &delta);
        ctx.wait_idle();
        (convert, ms_since(t))
    }

    /// [`Display::upload_full`] for the compositor's premultiplied output (`Session::display_pixels`),
    /// as `sync_full` does since the fused display path. Returns the texture id and
    /// `(convert_ms, upload_ms)`; the convert step is only the wrap into a `ColorImage`.
    pub fn upload_full_premul(&mut self, ctx: &GpuContext, w: usize, h: usize, px: Vec<u32>) -> (TextureId, f64, f64) {
        let t = std::time::Instant::now();
        let image = premul_image([w, h], px);
        let convert = ms_since(t);
        let id = TextureId::User(1);
        let delta = ImageDelta::full(epaint::ImageData::Color(Arc::new(image)), TextureOptions::LINEAR);
        let t = std::time::Instant::now();
        self.renderer.update_texture(&ctx.device, &ctx.queue, id, &delta);
        ctx.wait_idle();
        (id, convert, ms_since(t))
    }

    /// [`Display::upload_partial`] for premultiplied pixels (`Session::display_pixels_rect`).
    pub fn upload_partial_premul(&mut self, ctx: &GpuContext, id: TextureId, pos: [usize; 2], w: usize, h: usize, px: Vec<u32>) -> (f64, f64) {
        let t = std::time::Instant::now();
        let image = premul_image([w, h], px);
        let convert = ms_since(t);
        let delta = ImageDelta::partial(pos, epaint::ImageData::Color(Arc::new(image)), TextureOptions::LINEAR);
        let t = std::time::Instant::now();
        self.renderer.update_texture(&ctx.device, &ctx.queue, id, &delta);
        ctx.wait_idle();
        (convert, ms_since(t))
    }

    /// Register the compositor's output as the canvas texture (the GPU path).
    pub fn register(&mut self, ctx: &GpuContext, view: &wgpu::TextureView) -> TextureId {
        self.renderer.register_native_texture(&ctx.device, view, wgpu::FilterMode::Linear)
    }

    /// Draw the canvas quad once at `zoom` (screen pixels per canvas pixel), centred, and wait
    /// for the GPU. Returns milliseconds for encode + submit + GPU completion.
    pub fn draw(&mut self, ctx: &GpuContext, tex: TextureId, canvas_w: usize, canvas_h: usize, zoom: f32) -> f64 {
        let t = std::time::Instant::now();
        let (vw, vh) = (VIEW_W as f32, VIEW_H as f32);
        let (cw, ch) = (canvas_w as f32 * zoom, canvas_h as f32 * zoom);
        let min = Pos2::new((vw - cw) / 2.0, (vh - ch) / 2.0);
        let corners = [min, Pos2::new(min.x + cw, min.y), Pos2::new(min.x + cw, min.y + ch), Pos2::new(min.x, min.y + ch)];
        let mut mesh = Mesh::with_texture(tex);
        let uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        for (p, (u, v)) in corners.iter().zip(uvs) {
            mesh.vertices.push(Vertex { pos: *p, uv: Pos2::new(u, v), color: Color32::WHITE });
        }
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        let jobs = vec![ClippedPrimitive {
            clip_rect: Rect::from_min_max(Pos2::ZERO, Pos2::new(vw, vh)),
            primitive: Primitive::Mesh(mesh),
        }];
        let screen = ScreenDescriptor { size_in_pixels: [VIEW_W, VIEW_H], pixels_per_point: 1.0 };
        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bench-draw") });
        let extra = self.renderer.update_buffers(&ctx.device, &ctx.queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench-canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        ctx.queue.submit(extra.into_iter().chain(std::iter::once(encoder.finish())));
        ctx.wait_idle();
        let _ = &self.target;
        ms_since(t)
    }
}

/// The same wrap `app.rs::color_image` does: the pixel `Vec` is reinterpreted, not copied.
fn premul_image(size: [usize; 2], px: Vec<u32>) -> epaint::ColorImage {
    epaint::ColorImage::new(size, eskiz_core::composite::cast_pixels::<Color32>(px))
}
