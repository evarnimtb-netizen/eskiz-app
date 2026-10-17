//! Pour Fill: drag the top-bar colour circle onto the canvas to
//! flood-fill the region under it on the active layer.
//!
//! The gesture has two phases.
//! * **Carry.** The colour circle follows the pointer. Releasing over the canvas fills the
//!   region under the pointer at the default threshold (50%). Releasing anywhere else
//!   cancels.
//! * **Dropped.** When the pointer rests over the canvas for [`DWELL_SECS`], the drop point
//!   is fixed there and the fill appears as a live preview. Dragging on horizontally changes
//!   the threshold (1% per 2 px, right is looser) and a small bubble shows it. Releasing
//!   commits one undo step named "Pour Fill".
//!
//! The fill itself is [`crate::floodfill`]; the preview and undo glue is in
//! [`crate::session::Session::pour_fill_preview`] and friends.

use eframe::egui::{self, Align2, Color32, CornerRadius, Id, LayerId, Order, Pos2, Rect, Stroke, StrokeKind, Vec2};

use crate::app::{over_ui, EskizApp};
use crate::floodfill::DEFAULT_THRESHOLD;

/// How long the pointer must rest over the canvas to fix the drop point, seconds.
pub const DWELL_SECS: f64 = 0.3;
/// Movement below this still counts as resting, screen points.
const REST_SLOP: f32 = 6.0;
/// Horizontal points per 1% of threshold.
pub const POINTS_PER_PERCENT: f32 = 2.0;

/// Threshold for a horizontal drag of `dx` points from the drop point.
pub fn threshold_for_drag(dx: f32) -> f32 {
    (DEFAULT_THRESHOLD + dx / POINTS_PER_PERCENT / 100.0).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DropPhase {
    /// Carrying the colour; `anchor` and `since` track how long the pointer has rested.
    Carry { anchor: Pos2, since: f64 },
    /// The fill is previewed. `seed` is the canvas pixel, `percent` the threshold shown.
    Dropped { seed_screen: Pos2, seed: (i64, i64), percent: i32 },
}

/// A Pour Fill in progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DropState {
    pub colour: [u8; 3],
    pub pos: Pos2,
    pub phase: DropPhase,
}

impl EskizApp {
    /// The colour circle started to be dragged.
    pub(crate) fn begin_pour_fill(&mut self, pos: Pos2) {
        if self.adjust.is_some() {
            return;
        }
        if !self.session.can_paint(self.active) {
            let doc = &self.session.doc;
            let msg = if doc.node(self.active).is_none_or(|n| n.is_group()) {
                "Select a layer to fill."
            } else if doc.effective_locked(self.active) {
                "The active layer is locked."
            } else {
                "The active layer is hidden. Show it to fill."
            };
            self.show_toast(msg, 2.0);
            return;
        }
        if self.session.is_stroking() {
            return;
        }
        self.popovers.close();
        self.pour_fill = Some(DropState {
            colour: self.colour,
            pos,
            phase: DropPhase::Carry { anchor: pos, since: self.now },
        });
    }

    /// Canvas pixel under a screen point, when the point is over the canvas itself.
    fn drop_seed(&self, ctx: &egui::Context, pos: Pos2) -> Option<(i64, i64)> {
        if !self.viewport.contains(pos) || over_ui(ctx, pos) {
            return None;
        }
        let (fx, fy) = self.view.screen_to_canvas(self.viewport, pos);
        let (x, y) = (fx.floor() as i64, fy.floor() as i64);
        let d = &self.session.doc;
        (x >= 0 && y >= 0 && x < d.width as i64 && y < d.height as i64).then_some((x, y))
    }

    fn drop_preview(&mut self, seed: (i64, i64), percent: i32, colour: [u8; 3]) {
        let threshold = percent as f32 / 100.0;
        if let Some(rect) = self.session.pour_fill_preview(self.active, seed, threshold, colour) {
            self.refresh(rect);
        }
    }

    /// Advance a Pour Fill by one frame: track the pointer, preview, commit on release,
    /// and draw the carried colour and the threshold bubble.
    pub(crate) fn pour_fill_frame(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.pour_fill.take() else {
            return;
        };
        let (pos, down) = ctx.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
        let Some(pos) = pos else {
            self.finish_pour_fill(ctx, st, None, false);
            return;
        };
        st.pos = pos;
        if !down {
            self.finish_pour_fill(ctx, st, Some(pos), true);
            return;
        }
        match st.phase {
            DropPhase::Carry { anchor, since } => {
                if pos.distance(anchor) > REST_SLOP {
                    st.phase = DropPhase::Carry { anchor: pos, since: self.now };
                } else if self.now - since >= DWELL_SECS {
                    if let Some(seed) = self.drop_seed(ctx, pos) {
                        let percent = (DEFAULT_THRESHOLD * 100.0).round() as i32;
                        self.drop_preview(seed, percent, st.colour);
                        st.phase = DropPhase::Dropped { seed_screen: pos, seed, percent };
                    }
                }
            }
            DropPhase::Dropped { seed_screen, seed, percent } => {
                let now_percent = (threshold_for_drag(pos.x - seed_screen.x) * 100.0).round() as i32;
                if now_percent != percent {
                    self.drop_preview(seed, now_percent, st.colour);
                    st.phase = DropPhase::Dropped { seed_screen, seed, percent: now_percent };
                }
            }
        }
        self.draw_pour_fill(ctx, &st);
        ctx.request_repaint();
        self.pour_fill = Some(st);
    }

    /// The button was released (or the pointer vanished).
    fn finish_pour_fill(&mut self, ctx: &egui::Context, st: DropState, pos: Option<Pos2>, released: bool) {
        match st.phase {
            DropPhase::Dropped { .. } if released => {
                if let Some(rect) = self.session.pour_fill_commit() {
                    self.refresh(rect);
                    self.note_colour_used(st.colour);
                    self.show_toast("Pour Fill", 1.0);
                }
            }
            DropPhase::Carry { .. } if released => {
                // Dropped straight away: fill at the default threshold.
                let seed = pos.and_then(|p| self.drop_seed(ctx, p));
                if let Some(seed) = seed {
                    let t = DEFAULT_THRESHOLD;
                    if let Some(rect) = self.session.pour_fill(self.active, seed, t, st.colour) {
                        self.refresh(rect);
                        self.note_colour_used(st.colour);
                        self.show_toast("Pour Fill", 1.0);
                    }
                }
            }
            _ => {
                if let Some(rect) = self.session.pour_fill_cancel() {
                    self.refresh(rect);
                }
            }
        }
    }

    fn draw_pour_fill(&self, ctx: &egui::Context, st: &DropState) {
        let pal = self.pal;
        let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("colour-drop")));
        let c = Color32::from_rgb(st.colour[0], st.colour[1], st.colour[2]);
        match st.phase {
            DropPhase::Carry { .. } => {
                painter.circle_filled(st.pos + Vec2::new(0.0, 2.0), 17.0, Color32::from_black_alpha(60));
                painter.circle_filled(st.pos, 15.0, c);
                painter.circle_stroke(st.pos, 15.0, Stroke::new(2.0, pal.ring));
            }
            DropPhase::Dropped { seed_screen, percent, .. } => {
                // Drop point marker and a guide line along the drag.
                painter.line_segment([seed_screen, Pos2::new(st.pos.x, seed_screen.y)], Stroke::new(1.5, Color32::from_white_alpha(150)));
                painter.circle_filled(seed_screen, 5.0, c);
                painter.circle_stroke(seed_screen, 5.0, Stroke::new(2.0, Color32::WHITE));
                painter.circle_stroke(seed_screen, 6.5, Stroke::new(1.0, Color32::from_black_alpha(140)));
                let size = Vec2::new(58.0, 26.0);
                let bubble = Rect::from_center_size(Pos2::new(st.pos.x, seed_screen.y - 40.0), size);
                painter.rect_filled(bubble, CornerRadius::same(9), pal.panel);
                painter.rect_stroke(bubble, CornerRadius::same(9), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
                painter.text(
                    bubble.center(),
                    Align2::CENTER_CENTER,
                    format!("{percent}%"),
                    egui::FontId::proportional(13.0),
                    pal.text,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_starts_at_half_and_moves_one_percent_per_two_points() {
        assert_eq!(threshold_for_drag(0.0), 0.5);
        assert!((threshold_for_drag(20.0) - 0.6).abs() < 1e-6);
        assert!((threshold_for_drag(-20.0) - 0.4).abs() < 1e-6);
        assert_eq!(threshold_for_drag(10_000.0), 1.0);
        assert_eq!(threshold_for_drag(-10_000.0), 0.0);
    }
}
