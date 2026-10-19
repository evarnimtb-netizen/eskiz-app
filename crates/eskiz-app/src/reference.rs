//! The reference window: a floating,
//! movable, resizable window that shows the whole canvas (live) or an imported picture, with
//! its own zoom and pan. It is never part of the document and never exported.
//!
//! The face camera of the design notes are not implemented (it needs camera access).
//!
//! The live view reads the canvas texture, so it follows every stroke as it is painted.

use std::path::Path;

use eframe::egui::{self, pos2, vec2, Color32, ColorImage, CornerRadius, Id, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Vec2};

use crate::app::{EskizApp, BAR_HEIGHT};
use crate::export::expand_home;
use crate::import::decode_image;

/// What the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefSource {
    /// The document, live.
    Canvas,
    /// An imported picture.
    Image,
}

/// A picture imported into the reference window.
pub struct RefImage {
    pub tex: TextureHandle,
    pub w: usize,
    pub h: usize,
}

/// Zoom limits, as a multiple of "fit the window".
pub const ZOOM_RANGE: (f32, f32) = (1.0, 16.0);

/// State of the reference window.
pub struct Reference {
    pub open: bool,
    pub source: RefSource,
    pub image: Option<RefImage>,
    /// 1.0 fits the whole picture in the window.
    pub zoom: f32,
    /// Offset of the picture's centre from the window's centre, points.
    pub pan: Vec2,
    /// The path field of the Image source.
    pub path: String,
    /// The last error from opening a picture.
    pub error: String,
    /// Where the window sits on screen, from the last frame (for tests).
    pub rect: Rect,
    /// Where the picture area is, from the last frame (for tests).
    pub view_rect: Rect,
}

impl Default for Reference {
    fn default() -> Self {
        Self {
            open: false,
            source: RefSource::Canvas,
            image: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            path: String::new(),
            error: String::new(),
            rect: Rect::NOTHING,
            view_rect: Rect::NOTHING,
        }
    }
}

impl Reference {
    /// Back to "fit the window".
    pub fn reset_view(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    /// Zoom by `factor` keeping the picture point under `cursor` fixed. `centre` is the middle
    /// of the picture area.
    pub fn zoom_at(&mut self, centre: Pos2, cursor: Pos2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let next = (self.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let k = next / self.zoom;
        let to_cursor = cursor - centre;
        self.pan = to_cursor - (to_cursor - self.pan) * k;
        self.zoom = next;
        self.clamp_pan_free();
    }

    /// At fit zoom the picture stays centred.
    fn clamp_pan_free(&mut self) {
        if self.zoom <= 1.0 + 1e-4 {
            self.pan = Vec2::ZERO;
        }
    }
}

/// Where the picture of size `(w, h)` is drawn inside `view`, for the given zoom and pan: it
/// fits the area at zoom 1 and keeps its aspect ratio.
pub fn picture_rect(view: Rect, w: f32, h: f32, zoom: f32, pan: Vec2) -> Rect {
    if w <= 0.0 || h <= 0.0 {
        return Rect::from_center_size(view.center(), Vec2::ZERO);
    }
    let fit = (view.width() / w).min(view.height() / h);
    Rect::from_center_size(view.center() + pan, vec2(w * fit * zoom, h * fit * zoom))
}

impl EskizApp {
    /// Show or hide the reference window.
    pub(crate) fn toggle_reference(&mut self) {
        self.reference.open = !self.reference.open;
    }

    /// Load a picture for the reference window and show it.
    pub(crate) fn load_reference_image(&mut self, ctx: &egui::Context, path: &Path) {
        match decode_image(path) {
            Ok((w, h, rgba)) => {
                // A picture wider than the GPU's texture limit would make the renderer fail.
                let side = ctx.input(|i| i.max_texture_side).min(8192);
                let (w, h, rgba) = crate::import::limit_side(w, h, rgba, side);
                let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                let tex = ctx.load_texture("reference-image", image, TextureOptions::LINEAR);
                self.reference.image = Some(RefImage { tex, w, h });
                self.reference.source = RefSource::Image;
                self.reference.open = true;
                self.reference.error.clear();
                self.reference.reset_view();
            }
            Err(e) => {
                self.reference.error = e.clone();
                self.show_toast(format!("Could not open: {e}"), 3.0);
            }
        }
    }

    /// The window itself. Call once per frame, over the canvas.
    pub(crate) fn reference_window(&mut self, ctx: &egui::Context) {
        if !self.reference.open || self.fullscreen {
            return;
        }
        let pal = self.pal;
        let screen = ctx.content_rect();
        let mut open = true;
        let mut load: Option<String> = None;
        let mut browse = false;
        // Clear of the sidebar, in the lower corner on its side.
        let left = if self.prefs.sidebar_right { screen.left() + 16.0 } else { screen.left() + 64.0 };
        let default_pos = pos2(left, (screen.bottom() - 310.0).max(screen.top() + BAR_HEIGHT + 8.0));
        let response = egui::Window::new("Reference")
            .id(Id::new("reference-window"))
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(default_pos)
            .default_size(vec2(280.0, 236.0))
            .min_size(vec2(160.0, 130.0))
            .constrain_to(screen)
            .frame(crate::theme::panel_frame(&pal).inner_margin(egui::Margin::same(8)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                ui.horizontal(|ui| {
                    for (source, label) in [(RefSource::Canvas, "Canvas"), (RefSource::Image, "Image")] {
                        let active = self.reference.source == source;
                        let text = RichText::new(label).size(12.0).color(if active { pal.on_accent } else { pal.text });
                        let chip = egui::Button::new(text)
                            .fill(if active { pal.accent_fill } else { pal.row })
                            .corner_radius(CornerRadius::same(12))
                            .min_size(vec2(0.0, 24.0));
                        if ui.add(chip).clicked() {
                            self.reference.source = source;
                            self.reference.reset_view();
                        }
                    }
                    if ui.add(egui::Button::new(RichText::new("Fit").size(12.0)).min_size(vec2(0.0, 24.0))).clicked() {
                        self.reference.reset_view();
                    }
                    ui.label(RichText::new(format!("{:.0}%", self.reference.zoom * 100.0)).size(11.5).color(pal.text_dim));
                });
                if self.reference.source == RefSource::Image && self.reference.image.is_none() {
                    ui.label(RichText::new("Type the path of a PNG or JPEG picture.").size(12.0).color(pal.text_dim));
                }
                let path_row = self.reference.source == RefSource::Image;
                let footer = if path_row { 34.0 } else { 0.0 };
                let area = vec2(ui.available_width(), (ui.available_height() - footer).max(40.0));
                let (rect, resp) = ui.allocate_exact_size(area, Sense::click_and_drag());
                self.draw_reference_view(ui, rect, &resp);
                if path_row {
                    ui.horizontal(|ui| {
                        let edit = egui::TextEdit::singleline(&mut self.reference.path)
                            .hint_text("~/Pictures/photo.jpg")
                            .desired_width((ui.available_width() - 144.0).max(40.0));
                        let r = ui.add(edit);
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let click = ui.add(egui::Button::new("Open").min_size(vec2(56.0, 26.0))).clicked();
                        if ui.add(egui::Button::new("Browse...").min_size(vec2(72.0, 26.0))).clicked() {
                            browse = true;
                        }
                        if (enter || click) && !self.reference.path.trim().is_empty() {
                            load = Some(self.reference.path.clone());
                        }
                    });
                }
            });
        if let Some(r) = response {
            self.reference.rect = r.response.rect;
        }
        if browse {
            let typed = self.reference.path.clone();
            self.request_pick_near(crate::file_access::PickPurpose::Reference, &typed, None);
        }
        if let Some(path) = load {
            let file = expand_home(&path);
            self.load_reference_image(ctx, &file);
        }
        if !open {
            self.reference.open = false;
        }
    }

    /// The picture area: the canvas or the picture, with pan and zoom.
    fn draw_reference_view(&mut self, ui: &mut egui::Ui, rect: Rect, resp: &egui::Response) {
        let pal = self.pal;
        self.reference.view_rect = rect;
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(6), pal.row);
        let (tex, size) = match self.reference.source {
            RefSource::Canvas => (self.canvas_texture_id(), self.canvas_size()),
            RefSource::Image => match &self.reference.image {
                Some(i) => (Some(i.tex.id()), (i.w as f32, i.h as f32)),
                None => (None, (0.0, 0.0)),
            },
        };
        // Wheel and pinch zoom about the cursor; a drag pans; a double click fits.
        if resp.hovered() {
            let (zoom, scroll, pos) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.y, i.pointer.hover_pos()));
            let factor = zoom * (1.0 + scroll * 0.004).max(0.1);
            if let (true, Some(p)) = ((factor - 1.0).abs() > 1e-4, pos) {
                self.reference.zoom_at(rect.center(), p, factor);
            }
        }
        if resp.dragged() {
            self.reference.pan += resp.drag_delta();
            self.reference.clamp_pan_free();
        }
        if resp.double_clicked() {
            self.reference.reset_view();
        }
        // The canvas texture is the GPU composite when that is on, else the CPU texture.
        let Some(tex) = tex else {
            return;
        };
        let img = picture_rect(rect, size.0, size.1, self.reference.zoom, self.reference.pan);
        // Keep some of the picture in view whatever the pan.
        let slack = rect.size() * 0.5;
        let pan = self.reference.pan;
        let limit = (img.size() / 2.0 + slack - vec2(24.0, 24.0)).max(Vec2::ZERO);
        let clamped = vec2(pan.x.clamp(-limit.x, limit.x), pan.y.clamp(-limit.y, limit.y));
        if clamped != pan {
            self.reference.pan = clamped;
        }
        let img = picture_rect(rect, size.0, size.1, self.reference.zoom, self.reference.pan);
        // Paper behind a canvas with transparent parts.
        if self.reference.source == RefSource::Canvas {
            painter.rect_filled(img, CornerRadius::ZERO, Color32::from_rgb(
                (self.session.doc.background[0] * 255.0) as u8,
                (self.session.doc.background[1] * 255.0) as u8,
                (self.session.doc.background[2] * 255.0) as u8,
            ));
        }
        painter.image(tex, img, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        painter.rect_stroke(img, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_black_alpha(70)), StrokeKind::Outside);
        if !self.reference.error.is_empty() && self.reference.source == RefSource::Image {
            painter.text(rect.left_bottom() + vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, &self.reference.error, egui::FontId::proportional(11.0), pal.danger);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), vec2(200.0, 100.0))
    }

    #[test]
    fn the_picture_fits_the_area_and_keeps_its_aspect() {
        // A square picture in a wide area is limited by the height.
        let r = picture_rect(view(), 50.0, 50.0, 1.0, Vec2::ZERO);
        assert!((r.height() - 100.0).abs() < 1e-4 && (r.width() - 100.0).abs() < 1e-4);
        assert!((r.center() - view().center()).length() < 1e-4);
        // A wide picture is limited by the width.
        let r = picture_rect(view(), 400.0, 100.0, 1.0, Vec2::ZERO);
        assert!((r.width() - 200.0).abs() < 1e-4 && (r.height() - 50.0).abs() < 1e-4);
        // Zoom and pan scale and move it.
        let r = picture_rect(view(), 400.0, 100.0, 2.0, vec2(10.0, -5.0));
        assert!((r.width() - 400.0).abs() < 1e-4);
        assert!((r.center() - (view().center() + vec2(10.0, -5.0))).length() < 1e-4);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor_fixed() {
        let mut r = Reference::default();
        let centre = view().center();
        let cursor = centre + vec2(40.0, 10.0);
        // Where the cursor is on the picture (0..1) before and after.
        let uv = |r: &Reference| {
            let img = picture_rect(view(), 400.0, 200.0, r.zoom, r.pan);
            ((cursor.x - img.left()) / img.width(), (cursor.y - img.top()) / img.height())
        };
        let before = uv(&r);
        r.zoom_at(centre, cursor, 3.0);
        assert!((r.zoom - 3.0).abs() < 1e-5);
        let after = uv(&r);
        assert!((before.0 - after.0).abs() < 1e-4 && (before.1 - after.1).abs() < 1e-4);
        // Zoom is limited, and zooming out to fit recentres the picture.
        r.zoom_at(centre, cursor, 100.0);
        assert!((r.zoom - ZOOM_RANGE.1).abs() < 1e-5);
        r.zoom_at(centre, cursor, 0.0001);
        assert!((r.zoom - 1.0).abs() < 1e-5 && r.pan == Vec2::ZERO);
    }
}
