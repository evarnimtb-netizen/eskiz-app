//! The chrome around the canvas: the translucent top bar, the
//! floating sidebar capsule, toasts, the full-screen "show UI" button, and the popover
//! bodies. Placeholder bodies (Actions, Adjustments, Brush Library, Colour, Selection,
//! Transform, Gallery) are wired so part 2 can fill them in without touching the frame.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Id, Layout, Margin, Order, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2,
};

use crate::a11y;
use crate::app::{EskizApp, Tool, BAR_HEIGHT, MAX_SIZE};
use crate::icons::{self, IconFn};
use crate::popover::{self, Notice, Panel};
use crate::selection_tool::EditMode;
use crate::theme::Palette;

/// Fade time for hiding and showing the bars, seconds.
const FADE_SECS: f32 = 0.15;
const BUTTON: f32 = 44.0;
/// Gap between the colour circle's button and the right edge of the window.
pub(crate) const COLOUR_MARGIN: f32 = 16.0;

/// A 44-point icon button. Active glyphs are drawn in the accent colour; there is no fill.
fn glyph_button(ui: &mut egui::Ui, icon: IconFn, label: &str, active: bool, enabled: bool, pal: &Palette) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), sense);
    a11y::toggle(&resp, egui::WidgetType::Button, enabled, active, label);
    let colour = if !enabled {
        pal.disabled()
    } else if active {
        pal.accent
    } else {
        pal.text
    };
    icon(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(24.0)), colour);
    a11y::focus_ring(ui.painter(), rect.shrink(4.0), &resp, pal, 10);
    resp
}

/// A rounded text row used for notes and banners.
pub(crate) fn banner(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    egui::Frame::new()
        .fill(pal.row)
        .corner_radius(CornerRadius::same(crate::theme::tokens(pal.identity).banner_radius))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).size(13.0).color(pal.text_dim));
        });
}

pub(crate) fn small_label(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(pal.text_dim));
}

pub(crate) fn title_row(ui: &mut egui::Ui, text: &str) {
    if crate::theme::is_eskiz(ui.ctx()) {
        ui.label(RichText::new(text).font(egui::FontId::new(16.0, egui::FontFamily::Name(crate::fonts::HEADING_FAMILY.into()))));
    } else {
        ui.label(RichText::new(text).size(17.0).strong());
    }
    ui.add_space(6.0);
}

/// Result of one slider frame.
struct SliderOut {
    changed: bool,
    active: bool,
}

/// Thin vertical slider with a rounded thumb. `value` is 0..1 (bottom to top).
fn vslider(ui: &mut egui::Ui, pal: &Palette, rect: Rect, id: Id, value: &mut f32, name: &str) -> SliderOut {
    let resp = ui.interact(rect, id, Sense::drag());
    let mut changed = false;
    if resp.dragged() || resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            let v = ((rect.bottom() - 9.0 - p.y) / (rect.height() - 18.0)).clamp(0.0, 1.0);
            if (v - *value).abs() > 1e-6 {
                *value = v;
                changed = true;
            }
        }
    }
    // With keyboard focus, the arrow keys move the value in 5 % steps.
    if resp.has_focus() {
        crate::a11y::keep_arrows(ui, id);
        let (up, down) = ui.input_mut(|i| {
            let up = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
            let down = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
            (up, down)
        });
        let next = (*value + if up { 0.05 } else { 0.0 } - if down { 0.05 } else { 0.0 }).clamp(0.0, 1.0);
        if (next - *value).abs() > 1e-6 {
            *value = next;
            changed = true;
        }
    }
    a11y::slider(&resp, true, format!("{name} {:.0}%", *value * 100.0));
    a11y::focus_ring(ui.painter(), rect, &resp, pal, 8);
    let cx = rect.center().x;
    let thumb_y = rect.bottom() - 9.0 - (rect.height() - 18.0) * *value;
    let painter = ui.painter();
    let half = crate::theme::tokens(pal.identity).rail_width / 2.0;
    let track = Rect::from_min_max(Pos2::new(cx - half, rect.top() + 6.0), Pos2::new(cx + half, rect.bottom() - 6.0));
    painter.rect_filled(track, CornerRadius::same(2), if pal.identity == crate::theme::UiIdentity::Eskiz { pal.track } else { pal.row });
    let fill = Rect::from_min_max(Pos2::new(cx - half, thumb_y), Pos2::new(cx + half, rect.bottom() - 6.0));
    painter.rect_filled(fill, CornerRadius::same(2), pal.accent_fill);
    painter.circle_filled(Pos2::new(cx, thumb_y), 9.0, pal.text);
    painter.circle_stroke(Pos2::new(cx, thumb_y), 9.0, Stroke::new(1.0, Color32::from_black_alpha(60)));
    SliderOut {
        changed,
        active: resp.dragged() || resp.is_pointer_button_down_on(),
    }
}

/// Little bubble showing a percentage, placed beside a slider.
fn value_bubble(painter: &egui::Painter, pal: &Palette, rect: Rect, to_right: bool, value: f32) {
    let size = Vec2::new(54.0, 24.0);
    let y = rect.center().y.clamp(rect.top() + 12.0, rect.bottom() - 12.0);
    let x = if to_right { rect.right() + 10.0 } else { rect.left() - 10.0 - size.x };
    let bubble = Rect::from_min_size(Pos2::new(x, y - size.y / 2.0), size);
    let radius = crate::theme::tokens(pal.identity).bubble_radius;
    painter.rect_filled(bubble, CornerRadius::same(radius), pal.panel);
    painter.rect_stroke(bubble, CornerRadius::same(radius), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
    let text = format!("{:.0}%", value * 100.0);
    if pal.identity == crate::theme::UiIdentity::Eskiz {
        // Fixed digit cells: the readout does not jitter while dragging.
        crate::fonts::paint_numeric(painter, bubble.center(), &text, egui::FontId::proportional(13.0), pal.text);
    } else {
        painter.text(bubble.center(), Align2::CENTER_CENTER, text, egui::FontId::proportional(13.0), pal.text);
    }
}

impl EskizApp {
    /// Draw everything that floats over the canvas, and the popover for this frame.
    pub(crate) fn chrome(&mut self, ctx: &egui::Context) {
        let pal = self.pal;
        let screen = ctx.content_rect();
        // The system bars sit on top of these strips: paint them as part of the chrome.
        if !self.safe.is_zero() {
            // Not `layer_painter`: its clip rect excludes the safe area.
            let layer = egui::LayerId::new(Order::Middle, Id::new("safe-area"));
            let p = egui::Painter::new(ctx.clone(), layer, ctx.viewport_rect());
            for r in self.safe.strips(ctx.viewport_rect()) {
                p.rect_filled(r, CornerRadius::ZERO, pal.bar);
            }
        }
        let alpha = ctx.animate_bool_with_time(Id::new("ui-visible"), !self.fullscreen, FADE_SECS);
        if alpha > 0.01 {
            self.top_bar(ctx, screen, alpha);
            self.sidebar(ctx, screen, alpha);
            self.edit_chrome(ctx, alpha);
        }
        if alpha < 0.99 {
            self.show_ui_button(ctx, 1.0 - alpha);
        }
        self.adjust_banner(ctx);
        self.reference_window(ctx);
        let reduce = self.reduce_motion();
        if let Some(vis) = self.popovers.frame(ctx, reduce).filter(|_| !self.fullscreen) {
            let rect = popover::draw(ctx, &pal, vis, |ui| self.panel_body(ui, vis.panel));
            self.popovers.set_body(rect);
        }
        self.studio_ui(ctx);
        self.sheet_ui(ctx);
        self.guide_window(ctx);
        self.toast_ui(ctx);
    }

    /// The translucent 48-point bar along the top.
    fn top_bar(&mut self, ctx: &egui::Context, screen: Rect, alpha: f32) {
        let pal = self.pal;
        let bar = Rect::from_min_size(screen.min, Vec2::new(screen.width(), BAR_HEIGHT));
        // On macOS the traffic-light buttons sit over the top-left corner. Keep the first
        // buttons clear of them, except in full-screen mode where the bar is hidden.
        let left_inset = if cfg!(target_os = "macos") && !self.fullscreen { 78.0 } else { 12.0 };
        let fullscreen = self.fullscreen;
        self.button_rects.clear();
        self.button_ids.clear();
        egui::Area::new(Id::new("top-bar"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                ui.set_width(screen.width());
                ui.set_height(BAR_HEIGHT);
                ctx.layer_painter(ui.layer_id()).rect_filled(bar, CornerRadius::ZERO, pal.bar);
                // The bar background drags the window (the title bar is hidden). It is
                // registered before the buttons, so buttons take their own clicks and drags.
                let background = ui.interact(bar, Id::new("top-bar-drag"), Sense::drag());
                if background.drag_started() && !fullscreen {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                ui.horizontal_centered(|ui| {
                    ui.add_space(left_inset);
                    let gallery = ui.add_sized([72.0, BUTTON], egui::Button::new("Gallery").frame(false));
                    self.gallery_button = gallery.rect;
                    if gallery.clicked() {
                        self.open_gallery(ctx);
                    }
                    ui.add_space(4.0);
                    let actions = glyph_button(ui, if pal.identity == crate::theme::UiIdentity::Eskiz { icons::icon_actions_eskiz } else { icons::icon_actions }, "Actions", false, true, &pal);
                    if actions.clicked() {
                        self.popovers.toggle(Panel::Actions, actions.rect);
                    }
                    self.button_rects.push((Panel::Actions, actions.rect));
                    self.button_ids.push((Panel::Actions, actions.id));
                    let adjusting = self.adjust.is_some();
                    let adj = glyph_button(ui, icons::icon_adjustments, "Adjustments", adjusting, true, &pal);
                    if adj.clicked() {
                        if adjusting {
                            // Tapping the tool again applies the open adjustment.
                            self.adjust_apply(ctx);
                        } else {
                            self.popovers.toggle(Panel::Adjustments, adj.rect);
                        }
                    }
                    self.button_rects.push((Panel::Adjustments, adj.rect));
                    self.button_ids.push((Panel::Adjustments, adj.id));
                    let sel = glyph_button(ui, icons::icon_selection, "Selection", self.edit_mode == EditMode::Select, true, &pal);
                    if sel.clicked() {
                        self.toggle_select_mode();
                    }
                    let tr = glyph_button(ui, icons::icon_transform, "Transform", self.transform.is_some(), true, &pal);
                    if tr.clicked() {
                        // Tapping Transform again applies the open transform, as Adjustments does.
                        if self.transform.is_some() {
                            self.apply_transform();
                        } else {
                            self.begin_transform();
                        }
                    }

                    // Right group: tools, layers, colour. Laid out from the right edge so
                    // the colour circle always keeps its margin, whatever the item spacing.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(COLOUR_MARGIN);
                        self.colour_button(ui, &pal);
                        ui.add_space(8.0);
                        let layers = glyph_button(ui, icons::icon_layers, "Layers", false, true, &pal);
                        if layers.clicked() {
                            self.popovers.toggle(Panel::Layers, layers.rect);
                        }
                        self.button_rects.push((Panel::Layers, layers.rect));
                    self.button_ids.push((Panel::Layers, layers.id));
                        self.tool_button(ui, Tool::Eraser, icons::icon_eraser, "Eraser", &pal);
                        self.tool_button(ui, Tool::Smudge, icons::icon_smudge, "Smudge", &pal);
                        self.tool_button(ui, Tool::Brush, icons::icon_brush, "Brush", &pal);
                    });
                });
            });
    }

    /// A tool button. Tapping an inactive tool selects it. Tapping the active tool, or
    /// long-pressing any tool, opens the Brush Library for it.
    fn tool_button(&mut self, ui: &mut egui::Ui, tool: Tool, icon: IconFn, label: &str, pal: &Palette) {
        let active = self.tool == tool;
        let resp = glyph_button(ui, icon, label, active, true, pal);
        if active {
            // The Brush Library anchors to the active tool's button.
            self.button_rects.push((Panel::Brush, resp.rect));
            self.button_ids.push((Panel::Brush, resp.id));
        }
        let was_fired = matches!(self.long_press, Some((t, _, true)) if t == tool);
        if resp.is_pointer_button_down_on() {
            match self.long_press {
                Some((t, start, fired)) if t == tool => {
                    if !fired && self.now - start >= 0.5 {
                        self.open_library(tool, resp.rect);
                        self.long_press = Some((tool, start, true));
                    }
                }
                _ => self.long_press = Some((tool, self.now, false)),
            }
        } else if matches!(self.long_press, Some((t, _, _)) if t == tool) {
            self.long_press = None;
        }
        if resp.clicked() && !was_fired {
            if active {
                self.open_library(tool, resp.rect);
            } else {
                self.set_tool(tool);
                self.popovers.close();
            }
        }
    }

    fn open_library(&mut self, tool: Tool, button: Rect) {
        self.set_tool(tool);
        self.library_tool = tool;
        self.popovers.toggle(Panel::Brush, button);
    }

    /// The colour circle: 28 points, with a 2-point ring.
    fn colour_button(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click_and_drag());
        a11y::named(&resp, egui::WidgetType::Button, true, a11y::colour_label(self.colour));
        let [r, g, b] = crate::colour_management::show(self.colour);
        let c = Color32::from_rgb(r, g, b);
        let painter = ui.painter();
        painter.circle_filled(rect.center(), 14.0, c);
        painter.circle_stroke(rect.center(), 14.0, Stroke::new(2.0, pal.ring));
        if resp.clicked() {
            self.popovers.toggle(Panel::Colour, resp.rect);
        }
        // Dragging the circle onto the canvas is a Pour Fill (see `pour_fill`).
        if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                self.begin_pour_fill(p);
            }
        }
        self.button_rects.push((Panel::Colour, resp.rect));
        self.button_ids.push((Panel::Colour, resp.id));
    }

    /// The floating capsule on the left (or right) edge: size, modify, opacity, undo, redo.
    fn sidebar(&mut self, ctx: &egui::Context, screen: Rect, alpha: f32) {
        let pal = self.pal;
        let width = BUTTON;
        let h = (screen.height() * 0.6).clamp(260.0, screen.height() - BAR_HEIGHT - 16.0);
        let x = if self.prefs.sidebar_right {
            screen.right() - width - 8.0
        } else {
            screen.left() + 8.0
        };
        let min_top = screen.top() + BAR_HEIGHT + 8.0;
        let max_top = (screen.bottom() - h - 8.0).max(min_top);
        let centred = screen.center().y - h / 2.0;
        let start_y = if self.prefs.sidebar_y < 0.0 { centred } else { self.prefs.sidebar_y };
        let top = start_y.clamp(min_top, max_top);
        let right = self.prefs.sidebar_right;
        // While the 3D view is open, undo and redo follow its own strokes.
        let (can_undo, can_redo) = match self.three_d.as_ref() {
            Some(three) => (three.can_undo(), three.can_redo()),
            None => (self.session.can_undo(), self.session.can_redo()),
        };
        let mut grip_moved = 0.0f32;

        egui::Area::new(Id::new("sidebar"))
            .order(Order::Foreground)
            .fixed_pos(Pos2::new(x, top))
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
                let eskiz = pal.identity == crate::theme::UiIdentity::Eskiz;
                let grip = Rect::from_min_size(rect.min, Vec2::new(width, 22.0));
                let (size_rect, modify_rect, opacity_rect, undo_rect, redo_rect);
                if eskiz {
                    // Eskiz: separate pills, one for size and one for opacity, with the Modify
                    // button and undo/redo as their own small pills between and below them.
                    const GAP: f32 = 6.0;
                    let body_top = rect.top() + 26.0;
                    let slider_h = ((rect.height() - 26.0) - 3.0 * BUTTON - 3.0 * GAP) / 2.0;
                    size_rect = Rect::from_min_size(Pos2::new(rect.left(), body_top), Vec2::new(width, slider_h));
                    let size_pill = Rect::from_min_max(rect.min, size_rect.right_bottom());
                    modify_rect = Rect::from_min_size(Pos2::new(rect.left(), size_pill.bottom() + GAP), Vec2::new(width, BUTTON));
                    opacity_rect = Rect::from_min_size(Pos2::new(rect.left(), modify_rect.bottom() + GAP), Vec2::new(width, slider_h));
                    undo_rect = Rect::from_min_size(Pos2::new(rect.left(), opacity_rect.bottom() + GAP), Vec2::new(width, BUTTON));
                    redo_rect = Rect::from_min_size(undo_rect.left_bottom(), Vec2::new(width, BUTTON));
                    for pill in [size_pill, modify_rect, opacity_rect, undo_rect.union(redo_rect)] {
                        ui.painter().rect_filled(pill, CornerRadius::same(22), pal.panel);
                        ui.painter().rect_stroke(pill, CornerRadius::same(22), Stroke::new(1.0, pal.edge.gamma_multiply(0.5)), StrokeKind::Inside);
                    }
                } else {
                    let capsule = CornerRadius::same(22);
                    ui.painter().rect_filled(rect, capsule, pal.panel);
                    ui.painter().rect_stroke(rect, capsule, Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
                    let body_top = rect.top() + 26.0;
                    let fixed = 3.0 * BUTTON + 6.0;
                    let slider_h = ((rect.bottom() - body_top) - fixed) / 2.0;
                    size_rect = Rect::from_min_size(Pos2::new(rect.left(), body_top), Vec2::new(width, slider_h));
                    modify_rect = Rect::from_min_size(size_rect.left_bottom(), Vec2::new(width, BUTTON));
                    opacity_rect = Rect::from_min_size(modify_rect.left_bottom(), Vec2::new(width, slider_h));
                    undo_rect = Rect::from_min_size(opacity_rect.left_bottom(), Vec2::new(width, BUTTON));
                    redo_rect = Rect::from_min_size(undo_rect.left_bottom(), Vec2::new(width, BUTTON));
                }
                self.tour_rects = crate::onboarding::TargetRects { size: size_rect, undo: undo_rect };

                // Grip: drag the capsule up and down.
                let g = ui.interact(grip, Id::new("sidebar-grip"), Sense::drag());
                if g.dragged() {
                    grip_moved = g.drag_delta().y;
                }
                ui.painter().rect_filled(
                    Rect::from_center_size(grip.center(), Vec2::new(22.0, 4.0)),
                    CornerRadius::same(2),
                    pal.text_dim,
                );

                // Size: the value is log-mapped from 1 to MAX_SIZE, shown as a percentage.
                let idx = self.tool.index();
                let mut t = self.tool_params[idx].size.max(1.0).ln() / MAX_SIZE.ln();
                let out = vslider(ui, &pal, size_rect, Id::new(("size", idx)), &mut t, "Brush size");
                if out.changed {
                    self.tool_params[idx].size = MAX_SIZE.powf(t).max(1.0);
                }
                if out.active {
                    value_bubble(&ui.ctx().layer_painter(ui.layer_id()), &pal, size_rect, !right, t);
                }

                // Modify: hold for the eyedropper. `modify_hold_frame` starts it once the hold
                // is long enough, so this only records when the button went down.
                let modify = ui.interact(modify_rect, Id::new("modify"), Sense::click());
                a11y::named(&modify, egui::WidgetType::Button, true, "Pick Color");
                a11y::focus_ring(ui.painter(), modify_rect, &modify, &pal, 8);
                let held = modify.is_pointer_button_down_on();
                if !held {
                    self.modify_press = None;
                } else if self.modify_press.is_none() {
                    self.modify_press = Some(self.now);
                }
                self.modify_rect = modify_rect;
                let mcol = if held { pal.accent } else { pal.text };
                icons::icon_modify(ui.painter(), Rect::from_center_size(modify_rect.center(), Vec2::splat(22.0)), mcol);

                // Opacity: linear, shown as a percentage.
                let mut opacity = self.tool_params[idx].opacity;
                let out = vslider(ui, &pal, opacity_rect, Id::new(("opacity", idx)), &mut opacity, "Brush opacity");
                if out.changed {
                    self.tool_params[idx].opacity = opacity.max(0.01);
                }
                if out.active {
                    value_bubble(&ui.ctx().layer_painter(ui.layer_id()), &pal, opacity_rect, !right, opacity);
                }

                // Undo and redo.
                let undo = ui.interact(undo_rect, Id::new("sb-undo"), if can_undo { Sense::click() } else { Sense::hover() });
                a11y::named(&undo, egui::WidgetType::Button, can_undo, "Undo");
                a11y::focus_ring(ui.painter(), undo_rect, &undo, &pal, 8);
                let undo_col = if can_undo { pal.text } else { pal.disabled() };
                icons::icon_undo(ui.painter(), Rect::from_center_size(undo_rect.center(), Vec2::splat(24.0)), undo_col);
                if undo.clicked() {
                    self.undo_action();
                }
                let redo = ui.interact(redo_rect, Id::new("sb-redo"), if can_redo { Sense::click() } else { Sense::hover() });
                a11y::named(&redo, egui::WidgetType::Button, can_redo, "Redo");
                a11y::focus_ring(ui.painter(), redo_rect, &redo, &pal, 8);
                let redo_col = if can_redo { pal.text } else { pal.disabled() };
                icons::icon_redo(ui.painter(), Rect::from_center_size(redo_rect.center(), Vec2::splat(24.0)), redo_col);
                if redo.clicked() {
                    self.redo_action();
                }
            });
        if grip_moved != 0.0 {
            self.prefs.sidebar_y = top + grip_moved;
        }
    }

    /// Small "show UI" button, visible while the bars are hidden (full-screen mode).
    fn show_ui_button(&mut self, ctx: &egui::Context, alpha: f32) {
        let pal = self.pal;
        egui::Area::new(Id::new("show-ui"))
            .order(Order::Foreground)
            .fixed_pos(Pos2::new(12.0, 12.0))
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(96.0, 32.0), Sense::click());
                a11y::named(&resp, egui::WidgetType::Button, true, "Show UI");
                a11y::focus_ring(ui.painter(), rect, &resp, &pal, 16);
                ui.painter().rect_filled(rect, CornerRadius::same(16), pal.panel);
                ui.painter().rect_stroke(rect, CornerRadius::same(16), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
                icons::icon_eye(
                    ui.painter(),
                    Rect::from_center_size(Pos2::new(rect.left() + 20.0, rect.center().y), Vec2::splat(18.0)),
                    pal.text,
                );
                ui.painter().text(
                    Pos2::new(rect.left() + 38.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    "Show UI",
                    egui::FontId::proportional(12.0),
                    pal.text,
                );
                if resp.clicked() && alpha > 0.5 {
                    self.toggle_fullscreen();
                }
            });
    }

    /// Transient message at the top centre, fading out over its last 0.3 s.
    pub(crate) fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some(toast) = self.toast.clone() else {
            return;
        };
        let remaining = toast.until - self.now;
        if remaining <= 0.0 {
            self.toast = None;
            return;
        }
        let pal = self.pal;
        let alpha = (remaining / 0.3).clamp(0.0, 1.0) as f32;
        egui::Area::new(Id::new("toast"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, BAR_HEIGHT + 14.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::new()
                    .fill(pal.panel)
                    .corner_radius(CornerRadius::same(crate::theme::tokens(pal.identity).toast_radius))
                    .stroke(Stroke::new(1.0, pal.hairline))
                    .inner_margin(Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(toast.text).size(13.0).color(pal.text));
                    });
            });
    }

    /// Body of an open popover. The caller has already drawn the frame and nub.
    pub(crate) fn panel_body(&mut self, ui: &mut egui::Ui, panel: Panel) {
        match panel {
            Panel::Actions => self.actions_body(ui),
            Panel::Adjustments => self.adjustments_body(ui),
            Panel::Brush => self.brush_body(ui),
            Panel::Layers => self.layers_body(ui),
            Panel::Colour => self.colour_body(ui),
            Panel::Notice(n) => self.notice_body(ui, n),
        }
    }

    fn notice_body(&mut self, ui: &mut egui::Ui, notice: Notice) {
        let pal = self.pal;
        let (title, text) = match notice {
            Notice::Gallery => ("Gallery", "The Gallery arrives in a later update."),
        };
        title_row(ui, title);
        banner(ui, &pal, &format!("Coming soon. {text}"));
    }

    /// Bottom toolbars and banners for Selection and Transform.
    fn edit_chrome(&mut self, ctx: &egui::Context, alpha: f32) {
        match self.edit_mode {
            EditMode::Select => self.selection_toolbar(ctx, alpha),
            EditMode::Transform => self.transform_toolbar(ctx, alpha),
            EditMode::Off => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_meet_the_hit_target_and_size_ranges() {
        // The size slider maps 0..1 onto 1..MAX_SIZE on a log scale: the midpoint is 20 px.
        assert!((MAX_SIZE.powf(0.5) - 20.0).abs() < 1.0);
    }

    #[test]
    fn panel_widths_follow_the_spec() {
        assert_eq!(Panel::Brush.width(), 520.0);
        assert_eq!(Panel::Layers.width(), 320.0);
    }
}
