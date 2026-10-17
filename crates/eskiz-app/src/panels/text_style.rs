//! Edit Style window: font family with search, face, size,
//! kerning, tracking, leading, baseline shift, alignment, outline, underline, vertical, all caps,
//! colour and box width, for the text layer being typed into or the active text layer.
//!
//! Every control is a live change through [`EskizApp::text_restyle`]. A drag is one undo step,
//! recorded when the pointer is released (see `text_tick`).

use eframe::egui::{self, Color32, DragValue};
use eskiz_text::model::{LEADING_RANGE, SIZE_RANGE, SPACING_RANGE};
use eskiz_text::{Alignment, Attrs, Color, FontFace};

use crate::app::EskizApp;
use crate::text_tool::STYLE_STEP;

/// The face for a bold and italic pair.
fn face(bold: bool, italic: bool) -> FontFace {
    match (bold, italic) {
        (false, false) => FontFace::Regular,
        (true, false) => FontFace::Bold,
        (false, true) => FontFace::Italic,
        (true, true) => FontFace::BoldItalic,
    }
}

impl EskizApp {
    /// The Edit Style window. Shown while a text layer is the target (see `text_target`).
    pub(crate) fn text_style_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.text_target() else { return };
        let Some(layer) = self.session.doc.texts.get(&id).cloned() else { return };
        let caret = if self.text.editing == Some(id) { self.text.selection().start } else { 0 };
        let attrs: Attrs = layer.attrs_at(caret).cloned().unwrap_or_default();
        let families: Vec<String> = self.text.fonts().families().into_iter().filter(|f| !f.starts_with('.')).collect();
        let mut search = std::mem::take(&mut self.text.style_search);
        let mut box_width = layer.box_width;

        // The rows scroll inside a height that fits under the window's anchor (130 px covers the
        // title bar, the frame and a gutter at the bottom). When they overflow, a solid bar stays
        // visible so the cut-off row has a cue.
        let body_max = (ctx.content_rect().height() - 130.0).max(160.0);
        egui::Window::new("Edit Style")
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 60.0))
            .resizable(false)
            .collapsible(true)
            .vscroll(false)
            .default_width(260.0)
            .default_height(body_max)
            .show(ctx, |ui| {
                ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                egui::ScrollArea::vertical()
                    .id_salt("text-style-body")
                    .max_height(body_max)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                    ui.label("Font");
                    ui.add(egui::TextEdit::singleline(&mut search).hint_text("Search fonts").desired_width(f32::INFINITY));
                    let current = if attrs.font_family.is_empty() { "System default" } else { attrs.font_family.as_str() };
                    ui.small(format!("Current: {current}"));
                    egui::ScrollArea::vertical().max_height(120.0).id_salt("text-style-fonts").show(ui, |ui| {
                        let needle = search.to_lowercase();
                        for family in families.iter().filter(|f| needle.is_empty() || f.to_lowercase().contains(&needle)).take(300) {
                            if ui.selectable_label(*family == attrs.font_family, family).clicked() {
                                let name = family.clone();
                                self.text_restyle(STYLE_STEP, move |a| a.font_family = name.clone());
                            }
                        }
                    });

                    let (mut bold, mut italic) = (attrs.font_face.is_bold(), attrs.font_face.is_italic());
                    ui.horizontal(|ui| {
                        if crate::theme::checkbox(ui, &mut bold, "Bold").changed() || crate::theme::checkbox(ui, &mut italic, "Italic").changed() {
                            let f = face(bold, italic);
                            self.text_restyle(STYLE_STEP, move |a| a.font_face = f);
                        }
                    });
                    ui.separator();

                    let mut size = attrs.size_pt;
                    ui.horizontal(|ui| {
                        ui.label("Size");
                        if ui.add(DragValue::new(&mut size).range(SIZE_RANGE.0..=SIZE_RANGE.1).speed(0.5).suffix(" pt")).changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.size_pt = size);
                        }
                    });
                    let mut kerning = attrs.kerning;
                    ui.horizontal(|ui| {
                        ui.label("Kerning");
                        if ui.add(DragValue::new(&mut kerning).range(SPACING_RANGE.0..=SPACING_RANGE.1).speed(0.005)).changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.kerning = kerning);
                        }
                    });
                    let mut tracking = attrs.tracking;
                    ui.horizontal(|ui| {
                        ui.label("Tracking");
                        if ui.add(DragValue::new(&mut tracking).range(SPACING_RANGE.0..=SPACING_RANGE.1).speed(0.005)).changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.tracking = tracking);
                        }
                    });
                    let mut leading = attrs.leading;
                    ui.horizontal(|ui| {
                        ui.label("Leading");
                        if ui.add(DragValue::new(&mut leading).range(LEADING_RANGE.0..=LEADING_RANGE.1).speed(0.01)).changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.leading = leading);
                        }
                    });
                    let mut baseline = attrs.baseline_shift_pt;
                    ui.horizontal(|ui| {
                        ui.label("Baseline");
                        if ui.add(DragValue::new(&mut baseline).range(-500.0..=500.0).speed(0.5).suffix(" pt")).changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.baseline_shift_pt = baseline);
                        }
                    });
                    let mut outline = attrs.outline_pt;
                    ui.horizontal(|ui| {
                        ui.label("Outline");
                        if ui.add(DragValue::new(&mut outline).range(0.0..=200.0).speed(0.2).suffix(" pt")).changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.outline_pt = outline);
                        }
                    });

                    ui.horizontal(|ui| {
                        for (label, value) in [("Left", Alignment::Left), ("Center", Alignment::Centre), ("Right", Alignment::Right), ("Justify", Alignment::Justified)] {
                            if ui.selectable_label(attrs.alignment == value, label).clicked() {
                                self.text_restyle(STYLE_STEP, move |a| a.alignment = value);
                            }
                        }
                    });
                    let (mut underline, mut vertical, mut caps) = (attrs.underline, attrs.vertical, attrs.all_caps);
                    ui.horizontal_wrapped(|ui| {
                        if crate::theme::checkbox(ui, &mut underline, "Underline").changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.underline = underline);
                        }
                        if crate::theme::checkbox(ui, &mut vertical, "Vertical").changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.vertical = vertical);
                        }
                        if crate::theme::checkbox(ui, &mut caps, "All caps").changed() {
                            self.text_restyle(STYLE_STEP, move |a| a.all_caps = caps);
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.label("Color");
                        let c = attrs.color;
                        let swatch = Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a);
                        ui.colored_label(swatch, "\u{25A0}");
                        if ui.button("Use current color").clicked() {
                            let [r, g, b] = self.colour;
                            let colour = Color::rgba(r, g, b, 255);
                            self.text_restyle(STYLE_STEP, move |a| a.color = colour);
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Box width");
                        let changed = ui
                            .add(DragValue::new(&mut box_width).range(0.0..=5000.0).speed(1.0).suffix(" pt"))
                            .on_hover_text("Wrap width. Zero means no wrapping.")
                            .changed();
                        if changed {
                            self.text_set_box_width(box_width);
                        }
                    });
                    ui.separator();
                    if ui.button("Rasterize").on_hover_text("Keep the pixels, drop the text source").clicked() {
                        self.rasterize_text_action(id);
                    }
                    });
            });
        self.text.style_search = search;
    }
}
