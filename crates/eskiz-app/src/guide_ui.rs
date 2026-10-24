//! Drawing guides in the window: the lines over the canvas, the guide editor, and the
//! Guided Layer hook for strokes. The settings and geometry live in [`crate::guides`].

use eskiz_guides::{DrawingAssist, Rect as GuideRect, Vec2};
use eframe::egui::{self, Color32, Painter, Stroke};

use crate::app::EskizApp;
use crate::guides::{GuideKind, SymMode};
use crate::icons::{self, IconFn};
use crate::icons_guides;
use crate::icons_tools;

/// The glyph for a guide type in the editor.
fn kind_icon(kind: GuideKind) -> IconFn {
    match kind {
        GuideKind::Grid => icons_tools::icon_grid,
        GuideKind::Isometric => icons_tools::icon_isometric,
        GuideKind::Perspective => icons_guides::icon_guide,
        GuideKind::Symmetry => icons_guides::icon_symmetry,
    }
}

impl EskizApp {
    /// Once a frame: follow the open document (its guides load and save with it), and show
    /// the guide editor when it is open.
    pub(crate) fn guides_frame(&mut self, ctx: &egui::Context) {
        let key = self.gal.current.as_ref().and_then(|c| c.id.clone());
        let dir = self.gal.library.as_ref().map(|l| l.root().to_path_buf());
        let assigned = self.gal.assigned_id.clone();
        self.guides.sync(key.as_deref(), dir.as_deref(), assigned.as_deref());
        self.guide_editor(ctx);
        self.palette_files_window(ctx);
    }

    /// The guide lines, drawn over the canvas. They are computed in canvas space and mapped
    /// through the view, so they follow pan, zoom and rotation.
    pub(crate) fn draw_guides(&self, painter: &Painter) {
        let settings = &self.guides.settings;
        if !settings.visible {
            return;
        }
        let vp = self.viewport;
        let mut min = Vec2::new(f64::MAX, f64::MAX);
        let mut max = Vec2::new(f64::MIN, f64::MIN);
        for corner in [
            vp.left_top(),
            vp.right_top(),
            vp.right_bottom(),
            vp.left_bottom(),
        ] {
            let (x, y) = self.view.screen_to_canvas(vp, corner);
            min = Vec2::new(min.x.min(x), min.y.min(y));
            max = Vec2::new(max.x.max(x), max.y.max(y));
        }
        let visible = GuideRect::new(min.x, min.y, max.x - min.x, max.y - min.y);
        let (w, h) = (
            self.session.doc.width as f64,
            self.session.doc.height as f64,
        );
        let [r, g, b] = settings.colour;
        let zoom = f64::from(self.view.zoom);
        for line in settings.lines(w, h, visible) {
            let a = self
                .view
                .canvas_to_screen(vp, line.segment.a.x, line.segment.a.y);
            let z = self
                .view
                .canvas_to_screen(vp, line.segment.b.x, line.segment.b.y);
            let alpha = (line.opacity * 255.0).round() as u8;
            // Thickness is in canvas pixels; keep at least one screen pixel so lines stay visible.
            let width = (line.thickness * zoom).max(1.0) as f32;
            painter.line_segment(
                [a, z],
                Stroke::new(width, Color32::from_rgba_unmultiplied(r, g, b, alpha)),
            );
        }
    }

    /// The Guided Layer for a stroke on the active layer, if that layer has it on.
    pub(crate) fn assist_for_stroke(&self) -> Option<DrawingAssist> {
        self.guides
            .stroke_assist(self.active, self.session.doc.width, self.session.doc.height)
    }

    /// The guide editor window (Actions, Canvas, "Edit drawing guide").
    fn guide_editor(&mut self, ctx: &egui::Context) {
        if !self.guides.editor_open {
            return;
        }
        let (w, h) = (
            self.session.doc.width as f64,
            self.session.doc.height as f64,
        );
        let active = self.active;
        let pal = self.pal;
        let assist_before = self.guides.assist_on(active);
        let mut assist = assist_before;
        let mut s = self.guides.settings.clone();
        let mut open = true;
        egui::Window::new("Drawing guide")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(300.0)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 60.0))
            .show(ctx, |ui| {
                crate::theme::checkbox(ui, &mut s.visible, "Show guide");
                ui.horizontal(|ui| {
                    ui.label("Type");
                    for kind in GuideKind::ALL {
                        if icons::icon_button(ui, kind_icon(kind), s.kind == kind, true, kind.label(), &pal).clicked() {
                            s.kind = kind;
                        }
                    }
                });
                match s.kind {
                    GuideKind::Grid | GuideKind::Isometric => {
                        ui.add(egui::Slider::new(&mut s.grid_size, 8.0..=512.0).text("Grid size"));
                    }
                    GuideKind::Perspective => {
                        ui.horizontal(|ui| {
                            ui.label("Vanishing points");
                            for n in 1..=3 {
                                ui.selectable_value(&mut s.perspective_points, n, n.to_string());
                            }
                        });
                        for i in 0..s.perspective_points {
                            ui.horizontal(|ui| {
                                ui.label(format!("Point {}", i + 1));
                                let mut x = s.vanishing[i].x * w;
                                let mut y = s.vanishing[i].y * h;
                                let cx = ui.add(egui::DragValue::new(&mut x).speed(4.0).suffix(" px")).changed();
                                let cy = ui.add(egui::DragValue::new(&mut y).speed(4.0).suffix(" px")).changed();
                                if cx {
                                    s.vanishing[i].x = x / w;
                                }
                                if cy {
                                    s.vanishing[i].y = y / h;
                                }
                            });
                        }
                    }
                    GuideKind::Symmetry => {
                        ui.horizontal(|ui| {
                            ui.label("Mirror");
                            egui::ComboBox::from_id_salt("guide-sym").selected_text(s.sym.label()).show_ui(ui, |ui| {
                                for mode in SymMode::ALL {
                                    ui.selectable_value(&mut s.sym, mode, mode.label());
                                }
                            });
                        });
                        if s.sym == SymMode::Radial {
                            ui.add(egui::Slider::new(&mut s.radial_segments, 2..=32).text("Copies"));
                        }
                        crate::theme::checkbox(ui, &mut s.rotational, "Rotational (turn copies, don't mirror)");
                        ui.horizontal(|ui| {
                            ui.label("Center");
                            let mut x = s.centre.x * w;
                            let mut y = s.centre.y * h;
                            let cx = ui.add(egui::DragValue::new(&mut x).speed(4.0).suffix(" px")).changed();
                            let cy = ui.add(egui::DragValue::new(&mut y).speed(4.0).suffix(" px")).changed();
                            if cx {
                                s.centre.x = x / w;
                            }
                            if cy {
                                s.centre.y = y / h;
                            }
                        });
                    }
                }
                ui.add(egui::Slider::new(&mut s.opacity, 0.0..=1.0).text("Opacity"));
                ui.add(egui::Slider::new(&mut s.thickness, 0.5..=8.0).text("Thickness"));
                ui.horizontal(|ui| {
                    ui.label("Color");
                    ui.color_edit_button_srgb(&mut s.colour);
                });
                if matches!(s.kind, GuideKind::Isometric | GuideKind::Perspective) {
                    ui.add(egui::Slider::new(&mut s.lock_distance, 0.0..=64.0).text("Lock after (px)"));
                }
                ui.separator();
                crate::theme::checkbox(ui, &mut assist, "Guided Layer on the active layer");
                ui.label(
                    egui::RichText::new(
                        "Strokes snap to the grid, isometric or perspective guide, or mirror through the \
                         symmetry guide. Copies paint as one undo step.",
                    )
                    .small(),
                );
            });
        self.guides.settings = s;
        if assist != assist_before {
            self.guides.set_assist(active, assist);
        }
        self.guides.editor_open = open;
    }
}
