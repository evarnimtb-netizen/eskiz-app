//! Breadcrumbs inside egui-wgpu's frame.
//!
//! eframe has no hook between `ui` and the swap-chain present, so a freeze there used to be
//! reported as "egui paint + present" and nothing finer. A one-pixel paint callback lets the
//! frame leave two more marks: `prepare` runs just before `Surface::get_current_texture`,
//! `paint` runs inside the render pass once the texture was acquired. The last mark before a
//! stall therefore tells acquire (never reached `paint`) from submit and present (reached it).

use eframe::egui::{self, Rect};
use eframe::egui_wgpu::{self, CallbackTrait};
use eframe::wgpu;

use crate::stall::{self, Phase};

struct Marks;

impl CallbackTrait for Marks {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        stall::crumb(Phase::SurfaceAcquire);
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, _pass: &mut wgpu::RenderPass<'static>, _resources: &egui_wgpu::CallbackResources) {
        // Only reached when the swap chain gave a texture.
        stall::crumb(Phase::SubmitPresent);
        stall::surface_ok();
    }
}

/// Add the marks to this frame. Call once per frame, on a wgpu renderer only.
pub fn add(ctx: &egui::Context) {
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("paint-marks"));
    let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0));
    ctx.layer_painter(layer).add(egui_wgpu::Callback::new_paint_callback(rect, Marks));
}
