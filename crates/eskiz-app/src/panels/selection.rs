//! The Selection toolbar along the bottom edge, the Automatic threshold banner and the
//! animated marching ants.

use eframe::egui::{self, Align2, Color32, CornerRadius, Id, Margin, Order, Pos2, Rect, RichText, Stroke, Vec2};
use eskiz_select::outline::{Polyline, DASH_LENGTH_PX, DASH_SPEED_PX_PER_S};
use eskiz_select::{CombineMode, Point, Selection};

use crate::app::{EskizApp, BAR_HEIGHT};
use crate::icons::{self, IconFn};
use crate::icons_brush;
use crate::icons_selection;
use crate::icons_tools;
use crate::selection_tool::{self, ClipAction, Pending, Shape};
use crate::view::View;

/// One toolbar action, collected while the toolbar is drawn and run after it.
#[derive(Clone, Copy)]
enum Action {
    Shape(Shape),
    Combine(CombineMode),
    Invert,
    Save,
    Load(u64),
    Clear,
    Clip(ClipAction),
    Done,
}

impl EskizApp {
    /// Bottom toolbar and banner for Selection.
    pub(crate) fn selection_toolbar(&mut self, ctx: &egui::Context, alpha: f32) {
        let pal = self.pal;
        if let Some(text) = self.sel.banner.clone() {
            egui::Area::new(Id::new("sel-banner"))
                .order(Order::Foreground)
                .anchor(Align2::CENTER_TOP, Vec2::new(0.0, BAR_HEIGHT + 12.0))
                .interactable(false)
                .show(ctx, |ui| {
                    ui.set_opacity(alpha);
                    egui::Frame::new()
                        .fill(pal.panel)
                        .corner_radius(CornerRadius::same(10))
                        .stroke(Stroke::new(1.0, pal.hairline))
                        .inner_margin(Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.label(RichText::new(text).size(13.0).color(pal.text));
                        });
                });
        }

        let shape = self.sel.shape;
        let combine = self.sel.combine;
        let has_selection = self.session.selection().is_active();
        let mut feather = self.sel.feather_px;
        let saved: Vec<(u64, String)> = self.sel.saved.list().iter().map(|s| (s.id, s.name.clone())).collect();
        let has_clip = self.sel.clip.is_some();
        let mut actions: Vec<Action> = Vec::new();

        egui::Area::new(Id::new("sel-toolbar"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -12.0))
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::new()
                    .fill(pal.panel)
                    .corner_radius(CornerRadius::same(16))
                    .stroke(Stroke::new(1.0, pal.hairline))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            crate::user_guide::help_link(ui, &pal, crate::user_guide::Link::Selection);
                            let shapes: [(Shape, IconFn, &str); 5] = [
                                (Shape::Automatic, icons_selection::icon_wand, "Automatic"),
                                (Shape::Freehand, icons_selection::icon_lasso, "Freehand"),
                                (Shape::Polygon, icons_tools::icon_polygon, "Polygon"),
                                (Shape::Rectangle, icons_selection::icon_select_rect, "Rectangle"),
                                (Shape::Ellipse, icons_selection::icon_select_ellipse, "Ellipse"),
                            ];
                            for (s, icon, label) in shapes {
                                if icons::icon_button(ui, icon, shape == s, true, label, &pal).clicked() {
                                    actions.push(Action::Shape(s));
                                }
                            }
                            ui.separator();
                            let modes: [(CombineMode, IconFn, &str); 2] = [
                                (CombineMode::Add, icons::icon_plus, "Add"),
                                (CombineMode::Subtract, icons_tools::icon_minus, "Subtract"),
                            ];
                            for (m, icon, label) in modes {
                                if icons::icon_button(ui, icon, combine == m, true, label, &pal).clicked() {
                                    // Tapping the active mode turns it off again (Replace).
                                    let next = if combine == m { CombineMode::Replace } else { m };
                                    actions.push(Action::Combine(next));
                                }
                            }
                            if icons::icon_button(ui, icons_tools::icon_invert, false, has_selection, "Invert", &pal).clicked() {
                                actions.push(Action::Invert);
                            }
                            ui.scope(|ui| {
                                ui.spacing_mut().slider_width = 140.0;
                                icons::icon_badge(ui, icons_tools::icon_feather, &pal, "Feather");
                                ui.add(egui::Slider::new(&mut feather, 0.0..=100.0).suffix(" px").max_decimals(0))
                                    .on_hover_text("Feather");
                            });
                            ui.separator();
                            if icons::icon_button(ui, icons_tools::icon_save, false, has_selection, "Save", &pal).clicked() {
                                actions.push(Action::Save);
                            }
                            let load = icons::icon_button(ui, icons_tools::icon_load, false, true, "Load", &pal);
                            egui::Popup::menu(&load).show(|ui| {
                                if saved.is_empty() {
                                    ui.label("Nothing saved yet.");
                                }
                                for (id, name) in &saved {
                                    if ui.button(name).clicked() {
                                        actions.push(Action::Load(*id));
                                        ui.close();
                                    }
                                }
                            });
                            if icons::icon_button(ui, icons_brush::icon_clear, false, has_selection, "Clear", &pal).clicked() {
                                actions.push(Action::Clear);
                            }
                            ui.separator();
                            ui.menu_button("Edit", |ui| {
                                for (action, label) in [
                                    (ClipAction::Copy, "Copy"),
                                    (ClipAction::Cut, "Cut"),
                                    (ClipAction::CopyAll, "Copy All"),
                                    (ClipAction::Paste, "Paste"),
                                    (ClipAction::Duplicate, "Duplicate"),
                                ] {
                                    let enabled = action != ClipAction::Paste || has_clip;
                                    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                                        actions.push(Action::Clip(action));
                                        ui.close();
                                    }
                                }
                            });
                            if ui.button("Done").clicked() {
                                actions.push(Action::Done);
                            }
                        });
                    });
            });

        self.sel.feather_px = feather;
        for action in actions {
            self.run_selection_action(action);
        }
    }

    fn run_selection_action(&mut self, action: Action) {
        match action {
            Action::Shape(s) => {
                self.sel.shape = s;
                self.sel.pending = Pending::Idle;
            }
            Action::Combine(m) => self.sel.combine = m,
            Action::Invert => {
                let (w, h) = (self.session.doc.width, self.session.doc.height);
                let inverted = self.session.selection().invert(w, h);
                self.session.set_selection("Selection", inverted);
            }
            Action::Save => {
                if let Some(mask) = self.session.selection().mask().cloned() {
                    let n = self.sel.saved.list().len() + 1;
                    self.sel.saved.save(format!("Selection {n}"), &mask);
                    self.show_toast(format!("Saved selection {n}"), 1.2);
                }
            }
            Action::Load(id) => {
                if let Some(mask) = self.sel.saved.load(id) {
                    self.session.set_selection("Selection", Selection::active(mask));
                }
            }
            Action::Clear => {
                self.session.set_selection("Selection", Selection::none());
            }
            Action::Clip(clip) => self.clip_action(clip),
            Action::Done => self.exit_edit_mode(),
        }
    }

    /// Marching ants for the committed selection, and the shape being drawn.
    pub(crate) fn selection_overlay(&mut self, painter: &egui::Painter) {
        self.selection_outlines();
        let vp = self.viewport;
        let view = &self.view;
        let now = self.now;
        if let Some((_, lines)) = &self.sel.ants {
            for line in lines {
                draw_marching(painter, view, vp, line, now);
            }
        }
        if let Some(preview) = &self.sel.preview {
            for line in preview {
                draw_marching(painter, view, vp, line, now);
            }
        }
        // The shape being drawn, as an open or closed outline.
        let shape_points: Option<(Vec<Point>, bool)> = match &self.sel.pending {
            Pending::Freehand(stroke) => Some((stroke.points().to_vec(), false)),
            Pending::Polygon(pts) => Some((pts.clone(), false)),
            Pending::Drag { shape, a, b } => Some((selection_tool::preview_polyline(*shape, *a, *b), true)),
            _ => None,
        };
        if let Some((pts, closed)) = shape_points {
            let line = Polyline { points: pts, closed };
            draw_marching(painter, view, vp, &line, now);
        }
    }
}

/// Dashes walking along the outline, black and white, moving at `DASH_SPEED_PX_PER_S`.
fn draw_marching(painter: &egui::Painter, view: &View, vp: Rect, line: &Polyline, now: f64) {
    let mut pts: Vec<Pos2> = line
        .points
        .iter()
        .map(|p| view.canvas_to_screen(vp, f64::from(p.x), f64::from(p.y)))
        .collect();
    if line.closed {
        if let Some(&first) = pts.first() {
            pts.push(first);
        }
    }
    let period = 2.0 * DASH_LENGTH_PX;
    let offset = (now as f32 * DASH_SPEED_PX_PER_S).rem_euclid(period);
    let mut travelled = 0.0f32;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = (b - a).length();
        if len <= 0.0 {
            continue;
        }
        let mut t = 0.0f32;
        while t < len {
            let phase = (travelled + t + offset).rem_euclid(period);
            let black = phase < DASH_LENGTH_PX;
            let to_edge = if black { DASH_LENGTH_PX - phase } else { period - phase };
            let step = to_edge.min(len - t).max(0.5);
            let p0 = a + (b - a) * (t / len);
            let p1 = a + (b - a) * ((t + step).min(len) / len);
            let colour = if black { Color32::BLACK } else { Color32::WHITE };
            // One screen pixel wide, whatever the zoom.
            painter.line_segment([p0, p1], Stroke::new(1.0, colour));
            t += step;
        }
        travelled += len;
    }
}

#[cfg(test)]
mod tests {
    use crate::app::Prefs;
    use crate::document::Document;
    use crate::selection_tool::EditMode;
    use crate::snapshot::{snapshot_dir, Headless};
    use eframe::egui::Vec2;

    #[test]
    fn writes_selection_and_transform_snapshots() {
        let dir = snapshot_dir();
        let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(2048, 1536), Prefs::default());
        // A block of colour to select and transform.
        let (w, hgt) = (2048usize, 1536usize);
        let mut px = vec![0u8; w * hgt * 4];
        for y in 500..1000 {
            for x in 700..1300 {
                px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[40, 110, 200, 255]);
            }
        }
        let id = h.app.session.doc.layer_id(0).unwrap();
        h.app.session.replace_layer_pixels(id, px, "Fill");
        h.app.needs_full = true;
        h.app.edit_mode = EditMode::Select;
        // Anchored areas size themselves on the frame after they first appear.
        h.render();
        h.render().save_png(&dir.join("selection_toolbar.png")).unwrap();
        h.app.edit_mode = EditMode::Off;
        h.app.begin_transform();
        h.render();
        h.render().save_png(&dir.join("transform_toolbar.png")).unwrap();
        h.app.cancel_transform();
    }
}
