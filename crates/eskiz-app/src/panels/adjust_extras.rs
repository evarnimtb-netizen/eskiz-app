//! Controls and editors of the adjustment banner that go beyond a slider: the chip rows, the
//! Layer and Pencil switch, the curve editor, the gradient editor, the sample
//! buttons of Recolor and Clone, and the origin handle drawn on the canvas.

use eframe::egui::{self, vec2, Align2, Color32, CornerRadius, FontId, Id, LayerId, Order, Pos2, RichText, Stroke};
use eskiz_core::filters::{AdjustKind, Adjustment, GradientStops, GRADIENT_PRESETS};

use crate::app::EskizApp;
use crate::curves_widget::curves_editor;
use crate::theme::Palette;

/// A pill-shaped button that shows whether it is the active choice.
pub(crate) fn chip(ui: &mut egui::Ui, pal: &Palette, text: &str, active: bool) -> bool {
    let label = RichText::new(text).size(12.5).color(if active { pal.on_accent } else { pal.text });
    let button = egui::Button::new(label)
        .fill(if active { pal.accent_fill } else { pal.row })
        .corner_radius(CornerRadius::same(14))
        .min_size(vec2(0.0, 28.0));
    ui.add(button).clicked()
}

/// One half of the Layer | Pencil switch: a full-width button with the active one filled.
pub(crate) fn segment(ui: &mut egui::Ui, pal: &Palette, width: f32, text: &str, active: bool) -> bool {
    let label = RichText::new(text).size(13.0).color(if active { pal.on_accent } else { pal.text });
    let button = egui::Button::new(label)
        .fill(if active { pal.accent_fill } else { pal.row })
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(width, 32.0));
    ui.add(button).clicked()
}

const CURVE_NAMES: [&str; 4] = ["Master", "Red", "Green", "Blue"];
const CURVE_COLOURS: [Color32; 4] = [
    Color32::from_rgb(235, 235, 235),
    Color32::from_rgb(235, 90, 80),
    Color32::from_rgb(90, 200, 110),
    Color32::from_rgb(90, 140, 240),
];

/// The curve editor: channel chips (Master, Red, Green, Blue), the curve, and a Reset for the
/// channel shown. Returns the adjustment with the new points when the user changed them.
pub(crate) fn curves_ui(ui: &mut egui::Ui, pal: &Palette, adj: &Adjustment) -> Option<Adjustment> {
    let channel_id = Id::new("adjust-curve-channel");
    let mut channel = ui.data(|d| d.get_temp::<usize>(channel_id)).unwrap_or(0).min(3);
    let mut reset = false;
    ui.horizontal_wrapped(|ui| {
        for (k, name) in CURVE_NAMES.iter().enumerate() {
            if chip(ui, pal, name, k == channel) {
                channel = k;
            }
        }
        if chip(ui, pal, "Reset curve", false) {
            reset = true;
        }
    });
    ui.data_mut(|d| d.insert_temp(channel_id, channel));
    let mut points = adj.curve_points();
    if reset {
        points.channels[channel] = eskiz_core::filters::CurvePoints::default().channels[channel].clone();
    }
    let changed = curves_editor(
        ui,
        Id::new(("adjust-curve", channel)),
        &mut points.channels[channel],
        CURVE_COLOURS[channel],
        pal.row,
        pal.text_dim.gamma_multiply(0.25),
    );
    if changed || reset {
        let mut next = adj.clone();
        next.set_curve_points(points);
        return Some(next);
    }
    None
}

/// The gradient editor: preset chips, the stop chips with add and remove, and the selected
/// stop's position and colour. Returns the adjustment with the new stops when they changed.
pub(crate) fn gradient_ui(ui: &mut egui::Ui, pal: &Palette, adj: &Adjustment) -> Option<Adjustment> {
    let sel_id = Id::new("adjust-gradient-stop");
    let mut stops: GradientStops = adj.gradient_stops();
    let mut sel = ui.data(|d| d.get_temp::<usize>(sel_id)).unwrap_or(0).min(stops.stops.len().saturating_sub(1));
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for (k, (name, _)) in GRADIENT_PRESETS.iter().enumerate() {
            if chip(ui, pal, name, false) {
                stops = GradientStops::preset(k);
                sel = 0;
                changed = true;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        for i in 0..stops.stops.len() {
            if chip(ui, pal, &format!("Stop {}", i + 1), i == sel) {
                sel = i;
            }
        }
        if chip(ui, pal, "+ Stop", false) && stops.stops.len() < 8 {
            let i = sel.min(stops.stops.len() - 1);
            let a = stops.stops[i];
            let b = stops.stops.get(i + 1).copied().unwrap_or((1.0, [1.0; 3]));
            let mid = (a.0 + b.0) / 2.0;
            let rgb: [f32; 3] = std::array::from_fn(|k| (a.1[k] + b.1[k]) / 2.0);
            stops.stops.insert(i + 1, (mid, rgb));
            sel = i + 1;
            changed = true;
        }
        if chip(ui, pal, "Remove", false) && stops.stops.len() > 2 {
            stops.stops.remove(sel.min(stops.stops.len() - 1));
            sel = sel.min(stops.stops.len() - 1);
            changed = true;
        }
    });
    if let Some(stop) = stops.stops.get_mut(sel) {
        let (mut pos, mut rgb) = (stop.0, stop.1);
        let mut edited = false;
        ui.horizontal(|ui| {
            ui.label("Position");
            edited |= ui.add(egui::Slider::new(&mut pos, 0.0..=1.0).show_value(true)).changed();
        });
        ui.horizontal(|ui| {
            for (k, name) in ["R", "G", "B"].iter().enumerate() {
                ui.label(*name);
                edited |= ui.add(egui::Slider::new(&mut rgb[k], 0.0..=1.0).show_value(false)).changed();
            }
        });
        if edited {
            stop.0 = pos;
            stop.1 = rgb;
            changed = true;
        }
    }
    if changed {
        // Sorting can move the selected stop past a neighbour: keep it selected, so the slider
        // being dragged goes on editing the same stop.
        let selected = stops.stops.get(sel).copied();
        stops.stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        if let Some(s) = selected {
            sel = stops.stops.iter().position(|t| *t == s).unwrap_or(sel);
        }
    }
    ui.data_mut(|d| d.insert_temp(sel_id, sel));
    if changed {
        let mut next = adj.clone();
        next.set_gradient_stops(stops);
        return Some(next);
    }
    None
}

impl EskizApp {
    /// Draw the origin (Perspective blur, axial Chromatic aberration) or the clone source as a
    /// handle on the canvas. Drag it on the canvas to move it.
    pub(crate) fn adjust_draw_handle(&self, ctx: &egui::Context) {
        let Some(p) = self.adjust_handle_screen() else {
            return;
        };
        let Some(kind) = self.adjustment().map(|a| a.kind) else {
            return;
        };
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("adjust-handle")));
        let dark = Stroke::new(3.0, Color32::from_black_alpha(170));
        let light = Stroke::new(1.5, Color32::WHITE);
        for stroke in [dark, light] {
            painter.circle_stroke(p, 9.0, stroke);
            painter.line_segment([p - vec2(14.0, 0.0), p - vec2(5.0, 0.0)], stroke);
            painter.line_segment([p + vec2(5.0, 0.0), p + vec2(14.0, 0.0)], stroke);
            painter.line_segment([p - vec2(0.0, 14.0), p - vec2(0.0, 5.0)], stroke);
            painter.line_segment([p + vec2(0.0, 5.0), p + vec2(0.0, 14.0)], stroke);
        }
        let label = if kind == AdjustKind::Clone { "Source" } else { "Origin" };
        let pos: Pos2 = p + vec2(0.0, 22.0);
        painter.text(pos, Align2::CENTER_TOP, label, FontId::proportional(12.0), Color32::WHITE);
    }
}
