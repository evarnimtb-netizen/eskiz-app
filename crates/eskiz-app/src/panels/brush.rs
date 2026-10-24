//! Brush Library panel: sets on the left, brush rows with cached preview
//! strokes on the right, and an import field for `.brush`, `.brushset` and `.abr` files. The
//! data lives in `brush_library`, and the file handling in `brush_import`.

use std::path::Path;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, UiBuilder};

use crate::app::EskizApp;
use crate::brush_import::{self, MY_BRUSHES_SET};
use crate::brush_library::{decode_choice, encode_choice, set_of, BrushPreviews, PREVIEW_H, PREVIEW_W, RENDERS_PER_FRAME};
use crate::chrome::small_label;
use crate::export::expand_home;
use crate::icons;

const SET_COL_W: f32 = 128.0;
const COL_GAP: f32 = 16.0;
const SET_ROW_H: f32 = 40.0;
const BRUSH_ROW_H: f32 = 64.0;

impl EskizApp {
    pub(crate) fn brush_body(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let tool = self.library_tool;
        let scale = BrushPreviews::scale_for(ui.ctx().pixels_per_point());
        let sets = self.bank.sets.clone();
        let current = self.tool_brushes[tool.index()].clone();

        // Title, which tool the library is for, the Edit button and the "+" that opens the import
        // field.
        let mut open_studio = false;
        self.library_layout.edit = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Brush Library").size(17.0).strong());
            crate::user_guide::help_link(ui, &pal, crate::user_guide::Link::BrushLibrary);
            match tool {
                crate::app::Tool::Brush => {}
                crate::app::Tool::Smudge => small_label(ui, &pal, "for Smudge"),
                crate::app::Tool::Eraser => small_label(ui, &pal, "for Eraser"),
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                crate::a11y::named(&resp, egui::WidgetType::Button, true, "Import brushes");
                let glyph = if self.brush_ui.adding { pal.accent } else { pal.text_dim.gamma_multiply(0.6) };
                icons::icon_plus(ui.painter(), Rect::from_center_size(rect.center(), vec2(22.0, 22.0)), glyph);
                if resp.on_hover_text("Import brushes").clicked() {
                    self.brush_ui.adding = !self.brush_ui.adding;
                }
                // Edit opens the Brush Bench on the tool's brush.
                let (rect, resp) = ui.allocate_exact_size(vec2(60.0, 28.0), Sense::click());
                crate::a11y::named(&resp, egui::WidgetType::Button, true, "Edit brush");
                let fill = if resp.hovered() { pal.separator } else { pal.row };
                ui.painter().rect_filled(rect, CornerRadius::same(14), fill);
                crate::icons_brush::icon_edit(ui.painter(), Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(15.0, 15.0)), pal.text_dim);
                ui.painter().text(pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, "Edit", FontId::proportional(13.0), pal.text);
                self.library_layout.edit = Some(rect);
                if resp.on_hover_text("Edit this brush in the Brush Bench").clicked() {
                    open_studio = true;
                }
            });
        });
        if self.brush_ui.adding {
            ui.add_space(4.0);
            let mut path = std::mem::take(&mut self.brush_ui.path);
            let enabled = !path.trim().is_empty();
            let (go, browse) = Self::path_field(ui, &pal, &mut path, "Path to a .brush, .brushset or .abr file", "Import", enabled, true);
            self.brush_ui.path = path;
            if browse {
                let typed = self.brush_ui.path.clone();
                self.request_pick_near(crate::file_access::PickPurpose::ImportBrush, &typed, None);
            }
            if go {
                let file = expand_home(&self.brush_ui.path);
                self.import_brush_file(&file);
            }
        }
        ui.add_space(4.0);

        let set_idx = self.library_set[tool.index()].unwrap_or_else(|| set_of(&sets, &current)).min(sets.len() - 1);
        let height = ui.available_height().clamp(200.0, 480.0);
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let left = Rect::from_min_size(area.min, vec2(SET_COL_W, height));
        let right = Rect::from_min_max(pos2(area.min.x + SET_COL_W + COL_GAP, area.min.y), area.max);
        ui.painter().line_segment(
            [pos2(area.min.x + SET_COL_W + COL_GAP / 2.0, area.min.y + 2.0), pos2(area.min.x + SET_COL_W + COL_GAP / 2.0, area.max.y - 2.0)],
            egui::Stroke::new(1.0, pal.separator),
        );
        self.library_layout.rows.clear();
        self.library_layout.sets.clear();

        // Left column: the sets.
        ui.scope_builder(UiBuilder::new().max_rect(left), |ui| {
            small_label(ui, &pal, "SETS");
            egui::ScrollArea::vertical().id_salt("brush-sets").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, set) in sets.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(SET_COL_W, SET_ROW_H), Sense::click());
                    crate::a11y::toggle(&resp, egui::WidgetType::SelectableLabel, true, i == set_idx, set.name.to_string());
                    self.library_layout.sets.push(rect);
                    let selected = i == set_idx;
                    let fill = if selected {
                        pal.row
                    } else if resp.hovered() {
                        pal.row.gamma_multiply(0.55)
                    } else {
                        Color32::TRANSPARENT
                    };
                    let p = ui.painter();
                    if selected && pal.is_eskiz() {
                        pal.paint_selected(p, rect, 10, 0.2);
                    } else {
                        p.rect_filled(rect, CornerRadius::same(10), fill);
                    }
                    let glyph = if selected { pal.accent } else { pal.text_dim };
                    (set.icon)(p, Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(24.0, 24.0)), glyph);
                    p.text(pos2(rect.left() + 40.0, rect.center().y), Align2::LEFT_CENTER, &set.name, FontId::proportional(13.0), pal.text);
                    if resp.clicked() {
                        self.library_set[tool.index()] = Some(i);
                    }
                }
            });
        });

        // Right column: the brushes of the selected set.
        let set = &sets[set_idx];
        let mut budget = RENDERS_PER_FRAME;
        let mut pending = false;
        ui.scope_builder(UiBuilder::new().max_rect(right), |ui| {
            small_label(ui, &pal, &format!("{} \u{b7} {} BRUSHES", set.name.to_uppercase(), set.brushes.len()));
            if set.brushes.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(if set.name == MY_BRUSHES_SET {
                        "No brushes of your own yet. Open any brush in the Studio (tap it again, or Edit) and press Save."
                    } else {
                        "No imported brushes yet. Drop a .brush, .brushset or .abr file on the window, or use + to add one."
                    })
                        .size(13.0)
                        .color(pal.text_dim),
                );
            }
            let scroll_out = egui::ScrollArea::vertical()
                .id_salt(("brush-rows", set_idx))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    for brush in &set.brushes {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), BRUSH_ROW_H), Sense::click());
                        crate::a11y::toggle(&resp, egui::WidgetType::SelectableLabel, true, brush.id == current, brush.name.to_string());
                        self.library_layout.rows.push((brush.id.clone(), rect));
                        let selected = brush.id == current;
                        if ui.is_rect_visible(rect) {
                            let fill = if selected {
                                pal.accent_fill
                            } else if resp.hovered() {
                                pal.separator
                            } else {
                                pal.row
                            };
                            let text = if selected { pal.on_accent } else { pal.text };
                            ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
                            ui.painter().text(
                                pos2(rect.left() + 14.0, rect.center().y),
                                Align2::LEFT_CENTER,
                                &brush.name,
                                FontId::proportional(13.0),
                                text,
                            );
                            let img = Rect::from_min_size(
                                pos2((rect.right() - 8.0 - PREVIEW_W).round(), (rect.center().y - PREVIEW_H / 2.0).round()),
                                vec2(PREVIEW_W, PREVIEW_H),
                            );
                            match self.previews.get(ui.ctx(), &brush.id, &brush.lb, scale, &mut budget) {
                                Some(tex) => {
                                    ui.painter().image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                                }
                                None => pending = true,
                            }
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            if selected {
                                // Tapping the chosen brush again opens the Brush Bench on it.
                                open_studio = true;
                            } else {
                                self.tool_brushes[tool.index()] = brush.id.clone();
                            }
                        }
                    }
                });
        crate::panels::rows::scroll_cue(ui, &scroll_out, self.pal.panel);
        });
        if pending {
            // Draw the rest of the previews over the next frames.
            ui.ctx().request_repaint();
        }
        if open_studio && !self.open_studio_for_library_tool() {
            self.show_toast("This brush could not be loaded, so it cannot be edited", 3.0);
        }
    }

    /// Imports every brush in `path`, stores them in the Imported set, and selects the first one
    /// for the tool the library is open for. Shows a toast with the outcome.
    pub(crate) fn import_brush_file(&mut self, path: &Path) -> bool {
        let (list, notes) = match brush_import::read_file(path) {
            Ok(read) => read,
            Err(e) => {
                self.show_toast(format!("Could not import: {e}"), 3.0);
                return false;
            }
        };
        let (mut first, mut added, mut failed) = (None, 0usize, notes.len());
        // The first reason is shown: "2 skipped" alone does not say what to fix.
        let mut reason = notes.first().cloned();
        for ib in &list {
            match self.bank.add_imported(ib) {
                Ok(id) => {
                    first = first.or(Some(id));
                    added += 1;
                }
                Err(e) => {
                    failed += 1;
                    reason = reason.or(Some(format!("{}: {e}", ib.name)));
                }
            }
        }
        let Some(id) = first else {
            let why = reason.map_or_else(String::new, |r| format!(" ({r})"));
            self.show_toast(format!("No brush could be imported from that file{why}."), 4.0);
            return false;
        };
        self.tool_brushes[self.library_tool.index()] = id;
        self.brush_ui.adding = false;
        self.brush_ui.path.clear();
        let text = if failed > 0 {
            let why = reason.map_or_else(String::new, |r| format!(" (for example {r})"));
            format!("Imported {added} brush(es), {failed} skipped{why}")
        } else {
            format!("Imported {added} brush(es)")
        };
        self.show_toast(text, 2.5);
        true
    }

    /// Restores each tool's brush from prefs text (see [`encode_choice`]).
    pub(crate) fn set_brush_choice(&mut self, text: &str) {
        let mut missing = 0;
        for (slot, id) in self.tool_brushes.iter_mut().zip(decode_choice(text)) {
            match id {
                // A brush whose file is gone or cannot be read: the tool keeps its default brush.
                Some(id) if self.bank.find(&id).is_none() => missing += 1,
                Some(id) => *slot = id,
                None => {}
            }
        }
        let skipped = self.bank.skipped;
        if missing > 0 || skipped > 0 {
            let what = if skipped > 0 {
                format!("{skipped} saved brush file(s) could not be read and are not listed")
            } else {
                "A chosen brush could not be found".to_owned()
            };
            let tail = if missing > 0 { "; tools that used one are back on their default brush" } else { "" };
            self.show_toast(format!("{what}{tail}."), 4.0);
        }
    }

    /// The text saved in prefs for each tool's brush.
    pub(crate) fn brush_choice_text(&self) -> String {
        encode_choice(&self.tool_brushes)
    }
}
