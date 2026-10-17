//! Adjustments panel and the adjustment banner.
//!
//! The panel lists the sixteen adjustments of the design notes, with a Layer or Pencil switch that
//! chooses the mode the next one opens in. Choosing one opens the banner (see
//! [`EskizApp::adjust_banner`]) and closes the panel. The logic of the mode is in `adjust.rs`;
//! the chips, choice rows, curve and gradient editors are in `adjust_extras.rs`.

use eframe::egui::{self, vec2, Color32, CornerRadius, Id, Margin, Order, Pos2, Rect, RichText, Sense, Stroke};
use eskiz_core::filters::{AdjustKind, Adjustment};

use crate::adjust::{format_value, Pick};
use crate::app::{EskizApp, BAR_HEIGHT};
use crate::chrome::{small_label, title_row};
use crate::icons::{self, IconFn};
use crate::icons_actions as ia;
use crate::panels::adjust_extras::{chip, curves_ui, gradient_ui, segment};
use crate::panels::rows::{hint, list_row, Trailing};
use crate::theme::Palette;

/// Icon for a list row.
fn icon_for(kind: AdjustKind) -> IconFn {
    match kind {
        AdjustKind::Opacity => ia::icon_droplet,
        AdjustKind::GaussianBlur | AdjustKind::Bloom => ia::icon_blur,
        AdjustKind::MotionBlur => ia::icon_motion_blur,
        AdjustKind::PerspectiveBlur => ia::icon_perspective_blur,
        AdjustKind::Hsb | AdjustKind::ColourBalance | AdjustKind::Recolor => ia::icon_hsb,
        AdjustKind::Curves | AdjustKind::GradientMap => ia::icon_curve,
        AdjustKind::Sharpen => ia::icon_sharpen,
        AdjustKind::Noise => ia::icon_noise,
        AdjustKind::ChromaticAberration => ia::icon_chromatic,
        AdjustKind::Glitch => ia::icon_glitch,
        AdjustKind::Halftone => ia::icon_halftone,
        _ => icons::icon_adjustments,
    }
}

/// Id of the Layer or Pencil choice the panel offers for the next adjustment.
fn pencil_pref_id() -> Id {
    Id::new("adjust-pencil-pref")
}

impl EskizApp {
    pub(crate) fn adjustments_body(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        title_row(ui, "Adjustments");
        let pencil = ui.data(|d| d.get_temp::<bool>(pencil_pref_id())).unwrap_or(false);
        let mut pencil_next = pencil;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = (ui.available_width() - 6.0) / 2.0;
            if segment(ui, &pal, w, "Layer", !pencil) {
                pencil_next = false;
            }
            if segment(ui, &pal, w, "Pencil", pencil) {
                pencil_next = true;
            }
        });
        if pencil_next != pencil {
            ui.data_mut(|d| d.insert_temp(pencil_pref_id(), pencil_next));
        }
        ui.add_space(8.0);
        let busy = self.session.is_stroking();
        let mut chosen = None;
        let mut liquify_chosen = false;
        egui::ScrollArea::vertical()
            .id_salt("adjustments-list")
            .auto_shrink([false, true])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                // Liquify is a mode of its own, not an AdjustKind: it opens the bottom toolbar.
                if list_row(ui, &pal, icons::icon_adjustments, "Liquify", None, !busy, Trailing::Chevron).clicked() {
                    liquify_chosen = true;
                }
                for kind in AdjustKind::ALL {
                    if list_row(ui, &pal, icon_for(kind), kind.name(), None, !busy, Trailing::Chevron).clicked() {
                        chosen = Some(kind);
                    }
                }
                ui.add_space(2.0);
                hint(ui, &pal, "Layer changes the whole layer. Pencil applies the effect only where you paint. Tap the Adjustments button again to apply.");
            });
        if liquify_chosen {
            self.enter_liquify();
        }
        if let Some(kind) = chosen {
            self.enter_adjustment_mode(kind, pencil_next);
        }
    }

    /// The banner under the top bar while an adjustment is open: its name and Apply, Cancel and
    /// Reset; the Layer or Pencil switch (or the sample buttons of Clone and Recolor); the curve or
    /// gradient editor; the choice rows; the parameter chips with a slider for the selected one.
    /// While the final render runs it shows a progress bar instead.
    pub(crate) fn adjust_banner(&mut self, ctx: &egui::Context) {
        let Some(st) = self.adjust.as_ref() else {
            return;
        };
        let pal = self.pal;
        let adj: Adjustment = st.adj.clone();
        let kind = adj.kind;
        let visible = adj.visible_params();
        let param = if visible.contains(&st.param) { st.param } else { visible.first().copied().unwrap_or(0) };
        let value = adj.value(param);
        let (applying, progress) = (st.applying(), st.progress());
        let pick = st.pick;
        let colour = self.colour;
        let screen = ctx.content_rect();
        let width = (screen.width() - 32.0).clamp(280.0, 560.0);
        let params = kind.params();
        let choices: Vec<usize> = visible.iter().copied().filter(|&i| !params[i].choices.is_empty()).collect();
        let sliders: Vec<usize> = visible.iter().copied().filter(|&i| params[i].choices.is_empty()).collect();
        let mut set: Option<(usize, f64)> = None;
        let mut select = None;
        let mut store: Option<Adjustment> = None;
        let mut pencil: Option<bool> = None;
        let mut toggle_pick: Option<Pick> = None;
        let mut use_colour = false;
        let (mut apply, mut cancel, mut reset) = (false, false, false);
        egui::Area::new(Id::new("adjust-banner"))
            .order(Order::Foreground)
            .fixed_pos(Pos2::new(screen.center().x - width / 2.0, screen.top() + BAR_HEIGHT + 10.0))
            .show(ctx, |ui| {
                crate::theme::panel_frame(&pal).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
                    ui.set_width(width - 28.0);
                    ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(kind.name()).size(15.0).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let apply_btn = egui::Button::new(RichText::new("Apply").color(Color32::WHITE).strong())
                                .fill(pal.accent)
                                .min_size(vec2(68.0, 32.0));
                            apply = ui.add_enabled(!applying, apply_btn).clicked();
                            cancel = ui.add_enabled(!applying, egui::Button::new("Cancel").min_size(vec2(68.0, 32.0))).clicked();
                            reset = ui.add_enabled(!applying, egui::Button::new("Reset").min_size(vec2(60.0, 32.0))).clicked();
                        });
                    });
                    if applying {
                        ui.add(egui::ProgressBar::new(progress).desired_width(ui.available_width()).text(format!("Applying {}", kind.name())));
                        return;
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if kind == AdjustKind::Clone {
                            if chip(ui, &pal, "Set source", pick == Pick::CloneSource) {
                                toggle_pick = Some(Pick::CloneSource);
                            }
                            ui.label(RichText::new("then paint").size(12.5).color(pal.text_dim));
                        } else {
                            let w = 92.0;
                            if segment(ui, &pal, w, "Layer", !adj.pencil) {
                                pencil = Some(false);
                            }
                            if segment(ui, &pal, w, "Pencil", adj.pencil) {
                                pencil = Some(true);
                            }
                            if kind == AdjustKind::Recolor {
                                if chip(ui, &pal, "Sample color", pick == Pick::RecolorSource) {
                                    toggle_pick = Some(Pick::RecolorSource);
                                }
                                if chip(ui, &pal, "Use current color", false) {
                                    use_colour = true;
                                }
                            }
                        }
                    });
                    match kind {
                        AdjustKind::Curves => {
                            if let Some(next) = curves_ui(ui, &pal, &adj) {
                                store = Some(next);
                            }
                        }
                        AdjustKind::GradientMap => {
                            if let Some(next) = gradient_ui(ui, &pal, &adj) {
                                store = Some(next);
                            }
                        }
                        _ => {}
                    }
                    for &i in &choices {
                        let spec = params[i];
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.label(RichText::new(spec.name).size(12.5).color(pal.text_dim));
                            for (k, name) in spec.choices.iter().enumerate() {
                                if chip(ui, &pal, name, adj.value(i).round() as usize == k) {
                                    set = Some((i, k as f64));
                                }
                            }
                        });
                    }
                    if !sliders.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for &i in &sliders {
                                if chip(ui, &pal, params[i].name, i == param) {
                                    select = Some(i);
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            let label_w = 64.0;
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width() - label_w - 8.0, 34.0), Sense::hover());
                            if let Some(v) = banner_slider(ui, &pal, rect, params[param].min, params[param].max, value, kind, param) {
                                set = Some((param, v));
                            }
                            ui.label(RichText::new(format_value(kind, param, value)).size(13.0).monospace());
                        });
                    }
                    let note = match kind {
                        AdjustKind::Curves => "Click to add a point, drag to move it, right-click to remove it.",
                        AdjustKind::Clone => "Tap Set source, then drag on the canvas to paint. Drag the source handle to move it.",
                        _ if adj.pencil => "Drag on the canvas to paint where the effect applies.",
                        _ if adj.handle_indices().is_some() => "Drag left or right to change the value, or drag the origin handle.",
                        _ => "Drag left or right on the canvas to change the value.",
                    };
                    small_label(ui, &pal, note);
                });
            });
        if let Some(i) = select {
            self.adjust_select_param(i);
        }
        if let Some((i, v)) = set {
            self.adjust_set(i, v);
        }
        if let Some(next) = store {
            self.adjust_store(next);
        }
        if let Some(p) = pencil {
            self.adjust_set_pencil(p);
        }
        if let Some(p) = toggle_pick {
            self.adjust_toggle_pick(p);
        }
        if use_colour {
            for (k, v) in colour.iter().enumerate() {
                self.adjust_set(3 + k, f64::from(*v) / 255.0);
            }
        }
        if reset {
            self.adjust_reset();
        }
        if cancel {
            self.adjust_cancel();
        }
        if apply {
            self.adjust_apply(ctx);
        }
        if self.adjusting() {
            self.adjust_draw_handle(ctx);
        }
    }
}

/// A horizontal slider over `rect`. Bipolar ranges fill from the middle and snap to zero.
/// Returns the new value while it is being dragged.
#[allow(clippy::too_many_arguments)]
fn banner_slider(ui: &mut egui::Ui, pal: &Palette, rect: Rect, min: f64, max: f64, value: f64, kind: AdjustKind, param: usize) -> Option<f64> {
    let resp = ui.interact(rect, Id::new(("adjust-slider", kind.name(), param)), Sense::click_and_drag());
    let inset = 12.0;
    let (x0, x1) = (rect.left() + inset, rect.right() - inset);
    let mut out = None;
    if resp.dragged() || resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = f64::from(((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0));
            let mut v = min + t * (max - min);
            if min < 0.0 && v.abs() < 0.02 * (max - min) {
                v = 0.0;
            }
            if (v - value).abs() > 1e-9 {
                out = Some(v);
            }
        }
    }
    let shown = out.unwrap_or(value);
    let t = ((shown - min) / (max - min)) as f32;
    let cx = x0 + (x1 - x0) * t;
    let y = rect.center().y;
    let painter = ui.painter_at(rect.expand(4.0));
    let track = Rect::from_min_max(Pos2::new(x0, y - 2.5), Pos2::new(x1, y + 2.5));
    painter.rect_filled(track, CornerRadius::same(3), pal.row);
    let origin = if min < 0.0 { (x0 + x1) / 2.0 } else { x0 };
    let fill = Rect::from_min_max(Pos2::new(origin.min(cx), y - 2.5), Pos2::new(origin.max(cx), y + 2.5));
    painter.rect_filled(fill, CornerRadius::same(3), pal.accent);
    if min < 0.0 {
        painter.line_segment([Pos2::new(origin, y - 7.0), Pos2::new(origin, y + 7.0)], Stroke::new(1.0, pal.text_dim));
    }
    painter.circle_filled(Pos2::new(cx, y), 10.0, pal.text);
    painter.circle_stroke(Pos2::new(cx, y), 10.0, Stroke::new(1.0, Color32::from_black_alpha(60)));
    out
}
