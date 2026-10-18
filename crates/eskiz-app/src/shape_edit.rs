//! Edit Shape: after Hold to Shape snaps a stroke and the pen lifts, the shape
//! shows control points. Dragging one reshapes it live; a tap anywhere else commits it.
//!
//! The geometry (which handles a shape has, what dragging one does) is in
//! [`crate::hold_shape`]. A reshape undoes the shape's stroke and paints the new one, so the
//! history only ever holds the final shape as a single "Stroke" step.

use eframe::egui::{self, vec2, Color32, Painter, Stroke};

use crate::app::{EskizApp, FrameInput};
use crate::hold_shape::{self, HandleKind};

/// Radius of a handle on screen, points.
const HANDLE_R: f32 = 9.0;

impl EskizApp {
    /// Is Edit Shape open (the shape has been snapped and its handles are showing)?
    pub fn editing_shape(&self) -> bool {
        self.shape_edit.as_ref().is_some_and(|e| !e.live)
    }

    /// One frame of pointer input while Edit Shape is open.
    pub(crate) fn shape_handles_frame(&mut self, input: &FrameInput, press_on_ui: bool) {
        let Some(edit) = self.shape_edit.as_ref() else {
            return;
        };
        // Something else changed the document: the shape is final.
        if edit.edits != self.session.edits() {
            self.shape_edit = None;
            return;
        }
        let (vp, view) = (self.viewport, self.view);
        if input.primary_pressed && !press_on_ui {
            let Some(origin) = input.press_origin else {
                return;
            };
            let at = view.screen_to_canvas(vp, origin);
            let reach = f64::from(HANDLE_R * 1.6 / view.zoom);
            let hit = hold_shape::hit_handle(&edit.current, at, reach);
            match hit {
                Some(i) => {
                    if let Some(e) = self.shape_edit.as_mut() {
                        e.drag = Some(i);
                    }
                }
                None => {
                    // A tap elsewhere commits the shape.
                    self.shape_edit = None;
                    self.show_toast("Shape committed", 1.0);
                    return;
                }
            }
        }
        let Some(edit) = self.shape_edit.as_ref() else {
            return;
        };
        let Some(index) = edit.drag else {
            return;
        };
        if !input.primary_down {
            // Released: rasterize the dragged shape once.
            self.settle_shape_preview();
            if let Some(e) = self.shape_edit.as_mut() {
                e.drag = None;
            }
            return;
        }
        let Some(pointer) = input.pointer else {
            return;
        };
        let to = view.screen_to_canvas(vp, pointer);
        let mut next = hold_shape::drag_handle(&edit.current, index, to);
        // Shift keeps circles round, squares square and lines on 15-degree steps.
        let endpoint = matches!(edit.current, hold_shape::Shape::Line { .. }) && index == 1;
        if input.shift && (endpoint || !matches!(edit.current, hold_shape::Shape::Line { .. })) {
            next = hold_shape::constrain(&next);
        }
        if next == edit.current {
            return;
        }
        // Dragging only moves the vector preview; the shape is rasterized on release.
        self.begin_shape_preview();
        if let Some(e) = self.shape_edit.as_mut() {
            e.current = next;
        }
    }

    /// The shape as a vector line in the brush colour and size, drawn while it is dragged.
    fn draw_shape_preview(&self, painter: &Painter, shape: &hold_shape::Shape) {
        let params = self.tool_params[self.tool.index()];
        let width = (f64::from(params.size) * f64::from(self.view.zoom)).clamp(1.5, 400.0) as f32;
        let [r, g, b] = self.colour;
        let alpha = (params.opacity.clamp(0.1, 1.0) * 255.0) as u8;
        let colour = Color32::from_rgba_unmultiplied(r, g, b, alpha);
        let points: Vec<_> = hold_shape::outline(shape, 2.0)
            .into_iter()
            .map(|(x, y)| self.view.canvas_to_screen(self.viewport, x, y))
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(width, colour)));
    }

    /// The handles of the shape being edited. Called after the canvas is drawn.
    pub(crate) fn draw_shape_handles(&self, painter: &Painter) {
        let Some(edit) = self.shape_edit.as_ref() else {
            return;
        };
        let pal = self.pal;
        if edit.previewing {
            self.draw_shape_preview(painter, &edit.current);
        }
        if edit.live {
            return;
        }
        let handles = hold_shape::handles(&edit.current);
        // Thin guide lines from the centre-like move handle are left out: the handles alone
        // read clearly on any artwork.
        for (i, h) in handles.iter().enumerate() {
            let at = self.view.canvas_to_screen(self.viewport, h.pos.0, h.pos.1);
            let active = edit.drag == Some(i);
            painter.circle_filled(at, HANDLE_R + 1.5, Color32::from_black_alpha(70));
            match h.kind {
                HandleKind::Point => {
                    painter.circle_filled(at, HANDLE_R, if active { pal.accent } else { Color32::WHITE });
                    painter.circle_stroke(at, HANDLE_R, Stroke::new(2.0, pal.accent));
                }
                HandleKind::Move => {
                    // The move handle is filled, with a plus.
                    painter.circle_filled(at, HANDLE_R, pal.accent);
                    painter.circle_stroke(at, HANDLE_R, Stroke::new(2.0, if active { Color32::WHITE } else { pal.accent }));
                    let k = HANDLE_R * 0.5;
                    painter.line_segment([at - vec2(k, 0.0), at + vec2(k, 0.0)], Stroke::new(1.75, Color32::WHITE));
                    painter.line_segment([at - vec2(0.0, k), at + vec2(0.0, k)], Stroke::new(1.75, Color32::WHITE));
                }
            }
        }
    }
}
