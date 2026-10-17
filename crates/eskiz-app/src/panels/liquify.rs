//! The Liquify toolbar along the bottom edge and the brush circle over the canvas.
//! The logic is in [`crate::liquify_tool`].
//!
//! Row one picks the stroke mode. Row two has the sliders (Size, Pressure, Distortion,
//! Momentum), the Adjust magnitude slider (section 19.8), Reset (section 19.9), and Cancel and
//! Done on the right.

use eframe::egui::{self, vec2, Color32, CornerRadius, Id, LayerId, Order, RichText, Stroke};
use eskiz_filters::liquify::{radius, Mode};

use crate::app::{over_ui, EskizApp};
use crate::liquify_tool::MODES;

/// What the toolbar asked for during one frame. The actions run after the panel is drawn.
#[derive(Default)]
struct Asks {
    mode: Option<Mode>,
    reset: bool,
    done: bool,
    cancel: bool,
    bake: bool,
}

impl EskizApp {
    /// The bottom toolbar while Liquify is open, and the brush circle over the canvas.
    pub(crate) fn liquify_bar(&mut self, ui: &mut egui::Ui) {
        let Some(st) = self.liquify.as_ref() else {
            return;
        };
        let ctx = ui.ctx().clone();
        let pal = self.pal;
        let busy = st.is_stroking();
        let mut s = st.settings();
        let mut magnitude = st.magnitude();
        let mut magnitude_changed = false;
        let mut asks = Asks::default();
        egui::Panel::bottom("liquify_bar")
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.panel).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                crate::user_guide::help_link(ui, &pal, crate::user_guide::Link::Liquify);
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                ui.horizontal_wrapped(|ui| {
                    for (mode, label) in MODES {
                        let active = s.mode == mode;
                        let text = RichText::new(label).size(12.5).color(if active { pal.on_accent } else { pal.text });
                        let chip = egui::Button::new(text)
                            .fill(if active { pal.accent_fill } else { pal.row })
                            .corner_radius(CornerRadius::same(14))
                            .min_size(vec2(0.0, 28.0));
                        if ui.add_enabled(!busy, chip).clicked() {
                            asks.mode = Some(mode);
                        }
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        mini_slider(ui, "Size", &mut s.size);
                        mini_slider(ui, "Pressure", &mut s.pressure);
                        mini_slider(ui, "Distortion", &mut s.distortion);
                        mini_slider(ui, "Momentum", &mut s.momentum);
                    });
                    let resp = mini_slider(ui, "Adjust", &mut magnitude);
                    magnitude_changed = resp.changed();
                    if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                        asks.bake = true;
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Reset").min_size(vec2(60.0, 28.0))).clicked() {
                        asks.reset = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let done = egui::Button::new(RichText::new("Done").color(pal.on_primary).strong())
                            .fill(pal.primary)
                            .min_size(vec2(68.0, 28.0));
                        asks.done = ui.add_enabled(!busy, done).clicked();
                        asks.cancel = ui.add_enabled(!busy, egui::Button::new("Cancel").min_size(vec2(68.0, 28.0))).clicked();
                    });
                });
            });
        if let Some(mode) = asks.mode {
            s.mode = mode;
        }
        if let Some(st) = self.liquify.as_mut() {
            if s != st.settings() {
                st.set_settings(s);
            }
            if magnitude_changed {
                st.set_magnitude(magnitude);
            }
            if asks.bake {
                st.bake();
            }
        }
        if asks.reset {
            self.liquify_reset();
        }
        if asks.done {
            self.liquify_done();
        }
        if asks.cancel {
            self.liquify_cancel();
        }
        self.liquify_cursor(&ctx);
    }

    /// A circle the size of the brush (`R` in layer pixels, scaled to the view) at the pointer.
    fn liquify_cursor(&self, ctx: &egui::Context) {
        let Some(st) = self.liquify.as_ref() else {
            return;
        };
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        if !self.viewport.contains(pos) || over_ui(ctx, pos) {
            return;
        }
        let r = radius(st.settings().size) * self.view.zoom;
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("liquify-cursor")));
        painter.circle_stroke(pos, r, Stroke::new(2.0, Color32::from_black_alpha(140)));
        painter.circle_stroke(pos, r, Stroke::new(1.0, Color32::WHITE));
    }
}

/// A slider with its name in front of it, so each name sits beside its own slider.
fn mini_slider(ui: &mut egui::Ui, name: &str, value: &mut f32) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.add(egui::Slider::new(value, 0.0..=1.0).show_value(false))
    })
    .inner
}
