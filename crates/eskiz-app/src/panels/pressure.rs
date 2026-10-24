//! Pressure curve editor: the global pressure curve in Prefs, an
//! editable spline through four control points. The Actions panel's Prefs tab calls
//! [`EskizApp::pressure_curve_editor`].

use eframe::egui::{self, Align2, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};

use crate::app::EskizApp;
use crate::chrome::small_label;
use crate::pressure::PressureCurve;

/// Side of the square plot, points.
const PLOT: f32 = 200.0;
/// Radius of a draggable control point.
const HANDLE: f32 = 6.0;

/// Curve space (0..1, y up) to screen.
fn to_screen(rect: Rect, p: [f32; 2]) -> Pos2 {
    Pos2::new(rect.left() + p[0] * rect.width(), rect.bottom() - p[1] * rect.height())
}

/// Screen to curve space.
fn from_screen(rect: Rect, p: Pos2) -> [f32; 2] {
    [(p.x - rect.left()) / rect.width(), (rect.bottom() - p.y) / rect.height()]
}

impl EskizApp {
    /// The pressure curve editor: a plot with two draggable inner control points, the live
    /// pen pressure as a marker on the curve, a Reset button and the tablet status.
    pub(crate) fn pressure_curve_editor(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        small_label(ui, &pal, "PRESSURE CURVE");
        let side = PLOT.min(ui.available_width());
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
        self.pen.editor_rect = Some(rect);
        let mut curve = self.prefs.pressure_curve;

        let painter = ui.painter_at(rect.expand(HANDLE + 2.0));
        painter.rect_filled(rect, 8.0, pal.row);
        // Grid at quarters, and the identity diagonal for reference.
        for k in 1..4 {
            let f = k as f32 / 4.0;
            let line = Stroke::new(1.0, pal.separator);
            painter.line_segment([to_screen(rect, [f, 0.0]), to_screen(rect, [f, 1.0])], line);
            painter.line_segment([to_screen(rect, [0.0, f]), to_screen(rect, [1.0, f])], line);
        }
        painter.line_segment(
            [to_screen(rect, [0.0, 0.0]), to_screen(rect, [1.0, 1.0])],
            Stroke::new(1.0, pal.text_dim.gamma_multiply(0.5)),
        );
        painter.rect_stroke(rect, 8.0, Stroke::new(1.0, pal.hairline), StrokeKind::Inside);

        // Drag handles first, so the curve below is drawn with this frame's points.
        let points = curve.points();
        // When the hit boxes overlap, the handle nearer the pointer is registered last, so it
        // wins: either point can be grabbed however close together they are.
        let hover = ui.input(|i| i.pointer.hover_pos());
        let nearer_first = hover.is_some_and(|h| {
            h.distance(to_screen(rect, points[1])) < h.distance(to_screen(rect, points[2]))
        });
        let order = if nearer_first { [1, 0] } else { [0, 1] };
        for i in order {
            let centre = to_screen(rect, points[i + 1]);
            let hit = Rect::from_center_size(centre, Vec2::splat(HANDLE * 4.0));
            let id = ui.id().with(("pressure-point", i));
            let resp = ui.interact(hit, id, Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Grab);
            // The point keeps its offset from the pointer, so grabbing a handle by its edge
            // does not make it jump.
            if resp.drag_started() {
                // A drag starts once the pointer has moved: measure from where it was pressed.
                if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                    ui.data_mut(|d| d.insert_temp(id, centre - p));
                }
            }
            if resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let grab = ui.data(|d| d.get_temp::<Vec2>(id)).unwrap_or_default();
                    let c = from_screen(rect, p + grab);
                    curve.set_point(i, c[0], c[1]);
                }
            }
            if resp.double_clicked() {
                let f = (i + 1) as f32 / 3.0;
                curve.set_point(i, f, f);
            }
        }
        self.prefs.pressure_curve = curve;

        let pts: Vec<Pos2> = (0..=64).map(|k| {
            let x = k as f32 / 64.0;
            to_screen(rect, [x, curve.eval(x)])
        }).collect();
        painter.add(Shape::line(pts, Stroke::new(2.0, pal.accent)));

        // Live pen pressure: a dot on the curve at the raw reading.
        if let Some(raw) = self.pen.tracker.last_pressure().filter(|p| *p > 0.0) {
            let at = to_screen(rect, [raw, curve.eval(raw)]);
            painter.line_segment(
                [to_screen(rect, [raw, 0.0]), at],
                Stroke::new(1.0, pal.accent.gamma_multiply(0.5)),
            );
            painter.circle_filled(at, 4.0, pal.text);
        }

        for (i, p) in curve.points().iter().enumerate() {
            let c = to_screen(rect, *p);
            if i == 0 || i == 3 {
                painter.circle_filled(c, 3.5, pal.panel);
                painter.circle_stroke(c, 3.5, Stroke::new(1.5, pal.text_dim));
            } else {
                painter.circle_filled(c, HANDLE, pal.accent);
                painter.circle_stroke(c, HANDLE, Stroke::new(2.0, pal.panel));
            }
        }
        painter.text(
            rect.left_bottom() + Vec2::new(8.0, -6.0),
            Align2::LEFT_BOTTOM,
            "Pressure in, effect up",
            FontId::proportional(11.0),
            pal.text_dim,
        );

        // Numbers for the live pen, so a tester can see what the tablet sends without drawing.
        let readout = match self.pen.tracker.last_pose() {
            Some(pose) => {
                let end = if self.pen.frame.eraser { "eraser end" } else { "pen end" };
                let touch = if self.pen.frame.down { "touching" } else { "hovering" };
                format!("Live: {end}, {touch}, {}", crate::app::pen_reading_text(&crate::tablet::reading_from_pose(pose)))
            }
            None => "Live: no pen in range (hover the pen over the window)".to_owned(),
        };
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(readout).size(12.0).color(pal.text_dim)).wrap());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let changed = !self.prefs.pressure_curve.is_identity();
            if ui.add_enabled(changed, egui::Button::new("Reset")).clicked() {
                self.prefs.pressure_curve = PressureCurve::default();
            }
            let status = self.pen.status_line();
            // A long backend message wraps inside the panel instead of widening it.
            ui.add(egui::Label::new(egui::RichText::new(status).size(12.0).color(pal.text_dim)).wrap());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Prefs;
    use crate::document::Document;
    use crate::popover::Panel;
    use crate::snapshot::{snapshot_dir, Headless};

    fn open_prefs(light: bool, curve: PressureCurve) -> Headless {
        let prefs = Prefs { light, pressure_curve: curve, ..Prefs::default() };
        let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(512, 512), prefs);
        h.app.pen.status = "Pen: Wayland tablet protocol".to_owned();
        h.app.actions_ui.tab = crate::panels::actions::ActionsTab::Prefs;
        h.open_panel(Panel::Actions);
        // The editor is at the bottom of the Prefs list: scroll it into view.
        h.scroll_at(Pos2::new(180.0, 400.0), -1000.0);
        h
    }

    #[test]
    fn editor_snapshots_dark_and_light() {
        let mut curve = PressureCurve::default();
        curve.set_point(0, 0.1230, 0.2803);
        curve.set_point(1, 0.5752, 0.8797);
        for (light, name) in [(false, "pressure_editor_dark.png"), (true, "pressure_editor_light.png")] {
            let mut h = open_prefs(light, curve);
            let rect = h.app.pen.editor_rect.expect("editor was drawn");
            assert!(rect.width() > 100.0);
            let img = h.render();
            img.save_png(&snapshot_dir().join(name)).unwrap();
            // The accent curve crosses the plot: some pixel in the plot is the accent colour.
            let accent = h.app.pal.accent;
            let mut found = false;
            for y in rect.top() as usize..rect.bottom() as usize {
                for x in rect.left() as usize..rect.right() as usize {
                    let p = img.pixel(x, y);
                    found |= p[0] == accent.r() && p[1] == accent.g() && p[2] == accent.b();
                }
            }
            assert!(found, "no accent pixels in the plot");
        }
    }

    #[test]
    fn dragging_a_handle_edits_the_curve() {
        let mut h = open_prefs(false, PressureCurve::default());
        let rect = h.app.pen.editor_rect.expect("editor was drawn");
        assert!(h.app.prefs.pressure_curve.is_identity());
        let start = to_screen(rect, h.app.prefs.pressure_curve.points()[1]);
        let target = to_screen(rect, [0.2, 0.6]);
        h.move_to(start);
        h.press(start, true);
        h.step();
        h.move_to(target);
        h.step();
        h.press(target, false);
        h.step();
        let c = h.app.prefs.pressure_curve;
        assert!(!c.is_identity());
        assert!((c.points()[1][0] - 0.2).abs() < 0.03 && (c.points()[1][1] - 0.6).abs() < 0.03, "{c:?}");
        h.render().save_png(&snapshot_dir().join("pressure_editor_dragged.png")).unwrap();
    }

    #[test]
    fn a_handle_grabbed_by_its_edge_does_not_jump() {
        let mut h = open_prefs(false, PressureCurve::default());
        let rect = h.app.pen.editor_rect.expect("editor was drawn");
        let before = h.app.prefs.pressure_curve.points()[1];
        let start = to_screen(rect, before) + Vec2::new(8.0, 8.0);
        h.move_to(start);
        h.press(start, true);
        h.step();
        let target = start + Vec2::new(0.0, -20.0);
        h.move_to(target);
        h.step();
        h.press(target, false);
        h.step();
        let after = h.app.prefs.pressure_curve.points()[1];
        // Moved straight up by 20 pt: x is unchanged, not pulled 8 pt towards the pointer.
        assert!((after[0] - before[0]).abs() < 1e-3, "{before:?} -> {after:?}");
        assert!((after[1] - before[1] - 20.0 / rect.height()).abs() < 0.01, "{before:?} -> {after:?}");
    }
}
