//! Popover framework.
//!
//! A popover is a rounded panel anchored under its top-bar button, with a small arrow nub
//! pointing at the button. Only one is open at a time. It opens with a 150 ms eased
//! animation. Tapping outside it, or painting on the canvas, closes it. The caller decides
//! what goes in the body (`draw` takes a closure); this module only handles state, the
//! animation and the placement.

use eframe::egui::{self, epaint::PathShape, Area, Color32, Id, Order, Rect, Shape, Stroke};

use crate::theme::{self, Palette};

/// Open and close duration, seconds.
pub const OPEN_SECS: f32 = 0.15;

/// What a notice popover says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Gallery,
}

/// The popovers Eskiz can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Actions,
    Adjustments,
    /// Brush Library. Smudge and Eraser open the same library for their own brush.
    Brush,
    Layers,
    Colour,
    Notice(Notice),
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Panel::Actions => "Actions",
            Panel::Adjustments => "Adjustments",
            Panel::Brush => "Brush Library",
            Panel::Layers => "Layers",
            Panel::Colour => "Color",
            Panel::Notice(Notice::Gallery) => "Gallery",
        }
    }

    /// Width in points.
    pub fn width(self) -> f32 {
        match self {
            Panel::Brush => 520.0,
            Panel::Layers => 320.0,
            Panel::Notice(_) => 300.0,
            _ => 340.0,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Panel::Actions => "actions",
            Panel::Adjustments => "adjustments",
            Panel::Brush => "brush",
            Panel::Layers => "layers",
            Panel::Colour => "colour",
            Panel::Notice(Notice::Gallery) => "notice-gallery",
        }
    }
}

/// An open popover and the top-bar button it hangs from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub panel: Panel,
    pub rect: Rect,
}

/// What `frame` returns for this frame: which panel to draw and how far open it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visible {
    pub panel: Panel,
    pub anchor: Rect,
    /// 0 (closed) to 1 (fully open), eased.
    pub ease: f32,
}

/// Popover state. Keep one in the app.
#[derive(Clone, Debug, Default)]
pub struct Popovers {
    open: Option<Anchor>,
    /// The last panel that was open. Kept during the close animation.
    shown: Option<Anchor>,
    /// Screen rectangle of the body drawn last frame, for outside-tap tests.
    body: Option<Rect>,
}

impl Popovers {
    /// The open panel, if any (not counting one that is closing).
    pub fn open_panel(&self) -> Option<Panel> {
        self.open.map(|a| a.panel)
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Open `panel` under `button`. Opening another panel replaces the current one.
    pub fn open(&mut self, panel: Panel, button: Rect) {
        self.open = Some(Anchor { panel, rect: button });
    }

    /// Open `panel`, or close it when it is already the open one.
    pub fn toggle(&mut self, panel: Panel, button: Rect) {
        if self.open_panel() == Some(panel) {
            self.close();
        } else {
            self.open(panel, button);
        }
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// Whether a screen point lies inside the open popover or its button. A press outside
    /// both closes the popover.
    pub fn contains(&self, pos: egui::Pos2) -> bool {
        self.body.is_some_and(|r| r.contains(pos))
            || self.open.is_some_and(|a| a.rect.expand(4.0).contains(pos))
    }

    /// Advance the open/close animation and return what to draw this frame. `None` when
    /// nothing is visible. With `reduce_motion` the popover appears and disappears at once.
    pub fn frame(&mut self, ctx: &egui::Context, reduce_motion: bool) -> Option<Visible> {
        let secs = popover_secs(reduce_motion);
        let t = ctx.animate_bool_with_time(Id::new("popover-open"), self.open.is_some(), secs);
        if self.open.is_some() {
            self.shown = self.open;
        }
        if self.open.is_none() && t <= 0.001 {
            self.shown = None;
            self.body = None;
            return None;
        }
        let anchor = self.shown?;
        Some(Visible {
            panel: anchor.panel,
            anchor: anchor.rect,
            ease: ease_out(t),
        })
    }

    /// Where the body was drawn last frame (for placement checks and outside-tap tests).
    pub fn body_rect(&self) -> Option<Rect> {
        self.body
    }

    /// Remember where the body was drawn.
    pub fn set_body(&mut self, rect: Rect) {
        self.body = Some(rect);
    }
}

/// Open and close time for a popover: `OPEN_SECS`, or no animation under reduced motion.
pub fn popover_secs(reduce_motion: bool) -> f32 {
    if reduce_motion {
        0.0
    } else {
        OPEN_SECS
    }
}

/// Cubic ease-out.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Draw a visible popover. `body` fills the panel below the title (the caller draws the
/// title itself, so Brush Library can add its "+" beside it). Returns the body's rectangle.
pub fn draw(ctx: &egui::Context, pal: &Palette, vis: Visible, body: impl FnOnce(&mut egui::Ui)) -> Rect {
    let screen = ctx.content_rect();
    let width = vis.panel.width();
    // Room below the button: the 12 px gap, the frame's 24 px of margin and an 8 px edge.
    let max_h = (screen.height() - (vis.anchor.bottom() - screen.top()) - 46.0).clamp(220.0, 600.0);
    let left = (vis.anchor.center().x - width / 2.0).clamp(screen.left() + 8.0, screen.right() - width - 8.0);
    // Slide down 8 points while opening.
    let top = vis.anchor.bottom() + 12.0 - (1.0 - vis.ease) * 8.0;
    let out = Area::new(Id::new(("popover", vis.panel.id())))
        .order(Order::Foreground)
        .fixed_pos(egui::pos2(left, top))
        .show(ctx, |ui| {
            ui.set_opacity(vis.ease);
            ui.set_max_width(width);
            theme::panel_frame(pal).show(ui, |ui| {
                ui.set_width(width - 24.0);
                ui.set_max_height(max_h);
                body(ui);
            });
        });
    let rect = out.response.rect;
    // The nub: a small triangle pointing up at the button, drawn outside the body.
    let nub_x = vis.anchor.center().x.clamp(rect.left() + 18.0, rect.right() - 18.0);
    let painter = ctx.layer_painter(out.response.layer_id);
    let tip = egui::pos2(nub_x, rect.top() - 7.0);
    let base_y = rect.top() + 1.0;
    painter.add(Shape::Path(PathShape::convex_polygon(
        vec![
            egui::pos2(nub_x - 7.0, base_y),
            tip,
            egui::pos2(nub_x + 7.0, base_y),
        ],
        pal.panel,
        Stroke::NONE,
    )));
    painter.line_segment(
        [egui::pos2(nub_x - 7.0, base_y), tip],
        Stroke::new(1.0, pal.hairline),
    );
    painter.line_segment(
        [tip, egui::pos2(nub_x + 7.0, base_y)],
        Stroke::new(1.0, pal.hairline),
    );
    rect
}

/// Colour helper for a dimmed label, used by placeholder rows.
pub fn dim(pal: &Palette) -> Color32 {
    pal.text_dim
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{pos2, Vec2};

    fn button() -> Rect {
        Rect::from_min_size(pos2(100.0, 0.0), Vec2::new(44.0, 48.0))
    }

    #[test]
    fn one_popover_at_a_time_and_toggle_closes_it() {
        let mut p = Popovers::default();
        p.toggle(Panel::Brush, button());
        assert_eq!(p.open_panel(), Some(Panel::Brush));
        p.toggle(Panel::Layers, button());
        assert_eq!(p.open_panel(), Some(Panel::Layers));
        p.toggle(Panel::Layers, button());
        assert!(!p.is_open());
    }

    #[test]
    fn ease_out_runs_from_zero_to_one() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.5);
        assert!(ease_out(2.0) == 1.0);
    }

    #[test]
    fn closing_clears_the_open_state() {
        let mut p = Popovers::default();
        p.open(Panel::Colour, button());
        p.close();
        assert!(!p.is_open());
        assert_eq!(p.open_panel(), None);
    }

    #[test]
    fn taps_inside_the_button_or_body_are_not_outside() {
        let mut p = Popovers::default();
        p.open(Panel::Actions, button());
        p.set_body(Rect::from_min_size(pos2(40.0, 60.0), Vec2::new(340.0, 300.0)));
        assert!(p.contains(pos2(120.0, 20.0)));
        assert!(p.contains(pos2(200.0, 200.0)));
        assert!(!p.contains(pos2(600.0, 500.0)));
    }
}
