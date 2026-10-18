//! The Transform toolbar, the bounding box with its handles, and the 4x4 warp mesh.
//! Geometry comes from `eskiz_select`; the session and the
//! commit are in [`crate::selection_tool`].

use eframe::egui::{self, Align2, Color32, CornerRadius, Id, Margin, Order, Pos2, Rect, Stroke, Vec2};
use eskiz_select::geom::{Point, Quad};
use eskiz_select::transform::{bounds_of, flip_about, fit_gesture, rotate_45_about_bounds, rotate_about};
use eskiz_select::Interp;

use crate::app::{EskizApp, FrameInput};
use crate::icons::{self, IconFn};
use crate::icons_actions;
use crate::icons_tools;
use crate::selection_tool::{self, Drag, Hit, TransformAction, TransformMode, TransformTool};
use crate::view::View;

/// Radius of a grab point, screen points.
const GRAB_PT: f32 = 14.0;

impl EskizApp {
    /// The Transform toolbar along the bottom edge, with a live preview when it changed.
    pub(crate) fn transform_toolbar(&mut self, ctx: &egui::Context, alpha: f32) {
        let pal = self.pal;
        let Some(t) = self.transform.as_mut() else {
            return;
        };
        if t.dirty {
            self.refresh_transform_preview_now();
            ctx.request_repaint();
        }
        let Some(t) = self.transform.as_ref() else {
            return;
        };
        let (mode, snap, interp) = (t.mode, t.snap, t.interp);
        let mut action = None;
        egui::Area::new(Id::new("tr-toolbar"))
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
                            crate::user_guide::help_link(ui, &pal, crate::user_guide::Link::Transform);
                            let modes: [(TransformMode, IconFn, &str); 4] = [
                                (TransformMode::Free, icons_tools::icon_freeform, "Freeform"),
                                (TransformMode::Uniform, icons_tools::icon_uniform, "Uniform"),
                                (TransformMode::Distort, icons_tools::icon_distort, "Distort"),
                                (TransformMode::Warp, icons_tools::icon_warp, "Warp"),
                            ];
                            for (m, icon, label) in modes {
                                if icons::icon_button(ui, icon, mode == m, true, label, &pal).clicked() {
                                    action = Some(TransformAction::Mode(m));
                                }
                            }
                            if icons::icon_button(ui, icons_tools::icon_snap, snap, true, "Snap 15°", &pal).clicked() {
                                action = Some(TransformAction::Snap);
                            }
                            let actions: [(TransformAction, IconFn, &str); 4] = [
                                (TransformAction::FlipH, icons_actions::icon_flip_h, "Flip H"),
                                (TransformAction::FlipV, icons_actions::icon_flip_v, "Flip V"),
                                (TransformAction::Rotate45, icons_tools::icon_rotate, "Rotate 45°"),
                                (TransformAction::Fit, icons_tools::icon_fit, "Fit"),
                            ];
                            for (a, icon, label) in actions {
                                if icons::icon_button(ui, icon, false, true, label, &pal).clicked() {
                                    action = Some(a);
                                }
                            }
                            for (i, label) in [(Interp::Nearest, "Nearest"), (Interp::Bilinear, "Bilinear"), (Interp::Bicubic, "Bicubic")] {
                                if ui.add(egui::Button::new(label).selected(interp == i)).clicked() {
                                    action = Some(TransformAction::Interp(i));
                                }
                            }
                            if ui.button("Apply").clicked() {
                                action = Some(TransformAction::Apply);
                            }
                            if ui.button("Cancel").clicked() {
                                action = Some(TransformAction::Cancel);
                            }
                        });
                    });
            });
        if let Some(a) = action {
            self.transform_action(ctx, a);
        }
    }

    fn transform_action(&mut self, ctx: &egui::Context, action: TransformAction) {
        let (cw, ch) = self.canvas_size();
        match action {
            TransformAction::Apply => return self.apply_transform(),
            TransformAction::Cancel => return self.cancel_transform(),
            _ => {}
        }
        let Some(t) = self.transform.as_mut() else {
            return;
        };
        let quad = t.session.quad();
        match action {
            TransformAction::Mode(m) => {
                t.mode = m;
                if m == TransformMode::Warp && !t.session.is_warp() {
                    t.session.enter_warp().ok();
                }
            }
            TransformAction::Snap => t.snap = !t.snap,
            TransformAction::FlipH | TransformAction::FlipV => {
                if let (Some(q), false) = (quad, t.session.is_warp()) {
                    let g = flip_about(bounds_of(&q), action == TransformAction::FlipH);
                    t.session.apply_gesture(g);
                }
            }
            TransformAction::Rotate45 => {
                if let (Some(q), false) = (quad, t.session.is_warp()) {
                    t.session.apply_gesture(rotate_45_about_bounds(bounds_of(&q)));
                }
            }
            TransformAction::Fit => {
                if let (Some(q), false) = (quad, t.session.is_warp()) {
                    if let Some(g) = fit_gesture(bounds_of(&q), cw, ch) {
                        t.session.apply_gesture(g);
                    }
                }
            }
            TransformAction::Interp(i) => {
                t.interp = i;
                t.session.set_interp(i);
            }
            TransformAction::Apply | TransformAction::Cancel => {}
        }
        t.dirty = true;
        self.refresh_transform_preview_now();
        ctx.request_repaint();
    }

    /// Presses and drags on the bounding box, or on warp control points.
    pub(crate) fn transform_pointer(&mut self, input: &FrameInput, pressed_on_ui: bool) {
        let vp = self.viewport;
        let view = &self.view;
        let Some(t) = self.transform.as_mut() else {
            return;
        };
        let Some(quad) = t.session.quad() else {
            return;
        };
        if input.primary_pressed && !pressed_on_ui {
            if let Some(p) = input.press_origin {
                let c = selection_tool::canvas_pt(view, vp, p);
                if let Some(hit) = hit_test(view, vp, p, c, quad, t) {
                    let centre = centre_of(&quad);
                    t.drag = Some(Drag {
                        hit,
                        start: quad,
                        origin: c,
                        last: c,
                        centre,
                        start_angle: selection_tool::angle_about(centre, c),
                        applied_deg: 0.0,
                    });
                }
            }
        }
        if !input.primary_down {
            // Releasing redraws the preview at rest (full size, the chosen kernel).
            if t.drag.take().is_some() {
                t.dirty = true;
            }
            return;
        }
        let (Some(p), true) = (input.pointer, t.drag.is_some()) else {
            return;
        };
        let cur = selection_tool::canvas_pt(view, vp, p);
        let snap = t.snap;
        let mode = t.mode;
        let Some(drag) = t.drag.as_mut() else {
            return;
        };
        let delta = Point::new(cur.x - drag.origin.x, cur.y - drag.origin.y);
        match drag.hit {
            Hit::Control(i, j) => {
                let step = Point::new(cur.x - drag.last.x, cur.y - drag.last.y);
                if let Some(w) = t.session.warp_mut() {
                    w.drag_control(i, j, step);
                }
            }
            Hit::Rotate => {
                let total = selection_tool::angle_about(drag.centre, cur) - drag.start_angle;
                let target = selection_tool::snapped(total, snap);
                let step = target - drag.applied_deg;
                if step.abs() > 1e-6 {
                    t.session.apply_gesture(rotate_about(drag.centre, step));
                }
                drag.applied_deg = target;
            }
            Hit::Inside => {
                t.session.set_quad(&selection_tool::moved_quad(drag.start, delta)).ok();
            }
            hit @ (Hit::Corner(_) | Hit::Edge(_)) => {
                if let Some(q) = selection_tool::quad_for_drag(mode, hit, drag.start, delta) {
                    t.session.set_quad(&q).ok();
                }
            }
        }
        drag.last = cur;
        t.dirty = true;
    }

    /// The bounding box, its handles, and the warp mesh, drawn over the canvas.
    pub(crate) fn transform_overlay(&self, painter: &egui::Painter) {
        let Some(t) = self.transform.as_ref() else {
            return;
        };
        let Some(quad) = t.session.quad() else {
            return;
        };
        let vp = self.viewport;
        let view = &self.view;
        // Two-tone marks (a dark underlay, a light overlay, handles with a dark ring inside a light one) read on a
        // white canvas as well as on a dark one.
        let (dark, white) = (Color32::from_rgb(0x14, 0x15, 0x17), Color32::WHITE);
        let line = |painter: &egui::Painter, pts: [Pos2; 2], w: f32| {
            painter.line_segment(pts, Stroke::new(w + 2.5, dark.gamma_multiply(0.85)));
            painter.line_segment(pts, Stroke::new(w, white));
        };
        let dot = |painter: &egui::Painter, c: Pos2, r: f32, fill: Color32| {
            painter.circle_filled(c, r + 2.0, white);
            painter.circle(c, r, fill, Stroke::new(1.5, dark));
        };
        let screen = |c: Point| view.canvas_to_screen(vp, f64::from(c.x), f64::from(c.y));
        if let Some(mesh) = t.session.warp() {
            for i in 0..4 {
                for j in 0..4 {
                    let p = screen(mesh.ctrl[i][j]);
                    if i + 1 < 4 {
                        line(painter, [p, screen(mesh.ctrl[i + 1][j])], 1.0);
                    }
                    if j + 1 < 4 {
                        line(painter, [p, screen(mesh.ctrl[i][j + 1])], 1.0);
                    }
                    dot(painter, p, 5.0, white);
                }
            }
            return;
        }
        let corners: Vec<Pos2> = quad.iter().map(|&c| screen(c)).collect();
        for i in 0..4 {
            line(painter, [corners[i], corners[(i + 1) % 4]], 1.5);
        }
        for &c in &corners {
            dot(painter, c, 6.0, white);
        }
        for i in 0..4 {
            let mid = Pos2::new(
                (corners[i].x + corners[(i + 1) % 4].x) / 2.0,
                (corners[i].y + corners[(i + 1) % 4].y) / 2.0,
            );
            dot(painter, mid, 4.5, white);
        }
        let top = rotate_handle(&corners);
        line(painter, [top_mid(&corners), top], 1.5);
        dot(painter, top, 6.0, dark);
    }
}

fn centre_of(quad: &Quad) -> Point {
    let sum = quad.iter().fold((0.0f32, 0.0f32), |acc, p| (acc.0 + p.x, acc.1 + p.y));
    Point::new(sum.0 / 4.0, sum.1 / 4.0)
}

fn top_mid(corners: &[Pos2]) -> Pos2 {
    Pos2::new((corners[0].x + corners[1].x) / 2.0, (corners[0].y + corners[1].y) / 2.0)
}

/// The rotate handle sits outside the top edge, away from the centre.
fn rotate_handle(corners: &[Pos2]) -> Pos2 {
    let mid = top_mid(corners);
    let centre = Pos2::new(
        corners.iter().map(|p| p.x).sum::<f32>() / 4.0,
        corners.iter().map(|p| p.y).sum::<f32>() / 4.0,
    );
    let dir = (mid - centre).normalized();
    mid + dir * selection_tool::ROTATE_HANDLE_PT
}

/// The handle under the pointer, if any. Warp mode only grabs control points.
fn hit_test(view: &View, vp: Rect, p: Pos2, c: Point, quad: Quad, t: &TransformTool) -> Option<Hit> {
    let zoom = view.zoom;
    if t.session.is_warp() {
        let mesh = t.session.warp()?;
        return mesh.pick(c, selection_tool::WARP_PICK_PT / zoom).map(|(i, j)| Hit::Control(i, j));
    }
    let screen = |c: Point| view.canvas_to_screen(vp, f64::from(c.x), f64::from(c.y));
    let corners: Vec<Pos2> = quad.iter().map(|&q| screen(q)).collect();
    if rotate_handle(&corners).distance(p) <= GRAB_PT {
        return Some(Hit::Rotate);
    }
    if let Some(i) = (0..4).find(|&i| corners[i].distance(p) <= GRAB_PT) {
        return Some(Hit::Corner(i));
    }
    if let Some(i) = (0..4).find(|&i| {
        let mid = Pos2::new((corners[i].x + corners[(i + 1) % 4].x) / 2.0, (corners[i].y + corners[(i + 1) % 4].y) / 2.0);
        mid.distance(p) <= GRAB_PT
    }) {
        return Some(Hit::Edge(i));
    }
    inside_quad(&corners, p).then_some(Hit::Inside)
}

/// Is `p` inside the convex quad, given its corners in order?
fn inside_quad(corners: &[Pos2], p: Pos2) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if cross != 0.0 {
            if sign != 0.0 && cross.signum() != sign.signum() {
                return false;
            }
            sign = cross;
        }
    }
    true
}
