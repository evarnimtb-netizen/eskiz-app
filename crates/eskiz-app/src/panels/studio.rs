//! The Brush Bench sheet: a modal sheet with the setting sections on the
//! left, the settings of the chosen section in the middle and a live drawing pad on the right.
//! State and editing rules live in `studio`, the settings table in `studio_settings`.

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, ColorImage, CornerRadius, FontId, Frame, Id, Margin, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, TextureOptions, Ui, UiBuilder,
};
use eskiz_brushlib::grains::GrainKind;
use eskiz_brushlib::tips::TipKind;
use eskiz_brushlib::{generated_grain, generated_tip, preview_stroke};
use eskiz_core::brushes::{Accumulation, Brush, GrainMode, Points};
use eskiz_core::geometry::{BrushCurve, Sample};

use crate::app::EskizApp;
use crate::chrome::small_label;
use crate::icons_brush as ib;
use crate::input::{apply_pressure_curve, sample_from, sample_from_pen, PointerKind, RawPointer};
use crate::panels::rows::switch;
use crate::brush_library::BrushId;
use crate::studio::StudioState;
use crate::studio_settings::{controls, Control, CurveId, Section, SliderDef, ToggleDef};
use crate::theme::{self, Palette};

const HEADER_H: f32 = 64.0;
const SECTION_COL_W: f32 = 188.0;
const PAD_PAPER: Color32 = Color32::from_rgb(0xFB, 0xFA, 0xF7);

type Edit = Box<dyn FnOnce(&mut Brush)>;

/// What the sheet asks the app to do after the frame has been drawn.
#[derive(Default)]
struct Actions {
    edits: Vec<Edit>,
    tip: Option<TipKind>,
    /// `Some(None)` is "no grain".
    grain: Option<Option<GrainKind>>,
    rename: Option<String>,
    reset: bool,
    done: bool,
    save: bool,
    duplicate: bool,
    delete: bool,
}

/// A button drawn at an exact rectangle.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
}

fn pill(ui: &mut Ui, pal: &Palette, rect: Rect, label: &str, style: ButtonStyle, enabled: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("studio-button", label)), if enabled { Sense::click() } else { Sense::hover() });
    crate::a11y::named(&resp, egui::WidgetType::Button, enabled, label);
    crate::a11y::focus_ring(ui.painter(), rect, &resp, pal, 10);
    let (fill, text) = match style {
        ButtonStyle::Primary => (pal.primary, pal.on_primary),
        ButtonStyle::Secondary => (if resp.hovered() { pal.separator } else { pal.row }, pal.text),
        ButtonStyle::Danger => (if resp.hovered() { pal.danger.gamma_multiply(0.25) } else { pal.row }, pal.danger),
    };
    let fill = if style == ButtonStyle::Primary && resp.hovered() { pal.primary.gamma_multiply(0.88) } else { fill };
    let text = if enabled { text } else { pal.text_dim.gamma_multiply(0.5) };
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.5), text);
    resp
}

impl EskizApp {
    /// Draw the Brush Bench when it is open, and carry out what the person did in it.
    pub(crate) fn studio_ui(&mut self, ctx: &egui::Context) {
        let Some(mut st) = self.studio.take() else {
            return;
        };
        let pal = self.pal;
        let screen = ctx.content_rect();
        let w = (screen.width() - 32.0).clamp(560.0, 980.0);
        let h = (screen.height() - 56.0).clamp(380.0, 680.0);
        let backdrop = Color32::from_black_alpha(if pal.light { 96 } else { 150 });
        let frame = Frame::new()
            .fill(pal.panel)
            .corner_radius(CornerRadius::same(16))
            .stroke(Stroke::new(1.0, pal.hairline))
            .shadow(theme::shadow(&pal))
            .inner_margin(Margin::ZERO);
        let mut act = Actions::default();
        egui::Modal::new(Id::new("brush-studio"))
            .backdrop_color(backdrop)
            .frame(frame)
            .show(ctx, |ui| self.studio_sheet(ui, &mut st, &mut act, w, h));

        self.studio = Some(st);
        for e in act.edits {
            self.studio_edit_brush(e);
        }
        if let Some(kind) = act.tip {
            self.studio_edit(|w| w.set_tip(kind));
        }
        if let Some(kind) = act.grain {
            self.studio_edit(|w| w.set_grain(kind));
        }
        if let Some(name) = act.rename {
            self.studio_rename(name);
        }
        if act.reset {
            self.studio_reset();
        }
        if act.save {
            self.studio_save();
        }
        if act.duplicate {
            self.studio_duplicate();
        }
        if act.delete {
            self.studio_delete();
        }
        if act.done {
            self.close_studio();
        }
    }

    fn studio_sheet(&self, ui: &mut Ui, st: &mut StudioState, act: &mut Actions, w: f32, h: f32) {
        let pal = self.pal;
        let (sheet, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        st.layout = Default::default();
        st.layout.sheet = Some(sheet);
        let header = Rect::from_min_size(sheet.min, vec2(w, HEADER_H));
        let body = Rect::from_min_max(pos2(sheet.left(), header.bottom()), sheet.max);
        let settings_w = ((w - SECTION_COL_W) * 0.47).clamp(320.0, 400.0);
        let col1 = Rect::from_min_size(body.min, vec2(SECTION_COL_W, body.height()));
        let col2 = Rect::from_min_size(pos2(col1.right(), body.top()), vec2(settings_w, body.height()));
        let col3 = Rect::from_min_max(pos2(col2.right(), body.top()), body.max);
        let p = ui.painter();
        p.line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, pal.separator));
        for x in [col1.right(), col2.right()] {
            p.line_segment([pos2(x, body.top()), pos2(x, body.bottom())], Stroke::new(1.0, pal.separator));
        }
        // Slightly darker well behind the section list.
        p.rect_filled(
            col1.shrink2(vec2(0.5, 0.0)),
            CornerRadius { nw: 0, ne: 0, sw: 16, se: 0 },
            if pal.light { Color32::from_black_alpha(6) } else { Color32::from_black_alpha(36) },
        );

        // Take the brush by value so the controls can read it while `st` changes.
        let brush = st.working.brush.clone();
        ui.scope_builder(UiBuilder::new().max_rect(header), |ui| self.studio_header(ui, st, act, header));
        ui.scope_builder(UiBuilder::new().max_rect(col1), |ui| self.studio_sections(ui, st, col1));
        ui.scope_builder(UiBuilder::new().max_rect(col2), |ui| self.studio_settings_col(ui, st, &brush, act, col2));
        ui.scope_builder(UiBuilder::new().max_rect(col3), |ui| self.studio_pad_col(ui, st, &brush, col3));
    }

    fn studio_header(&self, ui: &mut Ui, st: &mut StudioState, act: &mut Actions, header: Rect) {
        let pal = self.pal;
        let mid = header.center().y;
        let h = 38.0;
        // Right end: Reset, Duplicate, Save and Done.
        let bw = [64.0, 92.0, 66.0, 66.0];
        let mut x = header.right() - 20.0;
        let mut rects = [Rect::NOTHING; 4];
        for i in (0..4).rev() {
            rects[i] = Rect::from_min_size(pos2(x - bw[i], mid - h / 2.0), vec2(bw[i], h));
            x -= bw[i] + 8.0;
        }
        let [reset, duplicate, save, done] = rects;
        // The name field, on a soft well.
        let field_w = (x - 8.0 - (header.left() + 20.0)).clamp(120.0, 300.0);
        let field = Rect::from_min_size(pos2(header.left() + 20.0, mid - h / 2.0), vec2(field_w, h));
        ui.painter().rect_filled(field, CornerRadius::same(10), pal.row);
        ib::icon_edit(ui.painter(), Rect::from_center_size(pos2(field.left() + 20.0, mid), vec2(18.0, 18.0)), pal.text_dim);
        // A small badge at the right of the field says whether the brush is saved.
        let badge = if st.dirty() {
            Some(("Unsaved", pal.accent))
        } else if st.is_user_brush() {
            Some(("Saved", pal.text_dim))
        } else {
            None
        };
        let badge_w = if badge.is_some() && field_w > 200.0 { 58.0 } else { 0.0 };
        if let (Some((text, colour)), true) = (badge, badge_w > 0.0) {
            ui.painter().text(pos2(field.right() - 10.0, mid), Align2::RIGHT_CENTER, text, FontId::proportional(11.5), colour);
        }
        let mut name = st.name.clone();
        let edit = egui::TextEdit::singleline(&mut name)
            .font(FontId::proportional(16.0))
            .frame(Frame::NONE)
            .vertical_align(egui::Align::Center)
            .desired_width(field.width() - 44.0 - badge_w)
            .min_size(vec2(field.width() - 44.0 - badge_w, field.height()))
            .margin(Margin::ZERO);
        let resp = ui.put(Rect::from_min_max(pos2(field.left() + 38.0, field.top()), field.max - vec2(badge_w, 0.0)), edit);
        st.layout.name = Some(field);
        if resp.changed() {
            act.rename = Some(name);
        }

        let can_reset = st.dirty();
        if pill(ui, &pal, reset, "Reset", ButtonStyle::Secondary, can_reset).on_hover_text("Put the brush back as it was when you opened or last saved it").clicked() {
            act.reset = true;
        }
        if pill(ui, &pal, duplicate, "Duplicate", ButtonStyle::Secondary, true).on_hover_text("Save a copy in My Brushes and edit the copy").clicked() {
            act.duplicate = true;
        }
        let save_tip = if st.is_user_brush() { "Save the changes to this brush" } else { "Save a copy of this brush in My Brushes" };
        if pill(ui, &pal, save, "Save", if st.dirty() || !st.is_user_brush() { ButtonStyle::Primary } else { ButtonStyle::Secondary }, true).on_hover_text(save_tip).clicked() {
            act.save = true;
        }
        if pill(ui, &pal, done, "Done", ButtonStyle::Secondary, true).on_hover_text("Close without saving").clicked() {
            act.done = true;
        }
        st.layout.reset = Some(reset);
        st.layout.duplicate = Some(duplicate);
        st.layout.save = Some(save);
        st.layout.done = Some(done);
    }

    fn studio_sections(&self, ui: &mut Ui, st: &mut StudioState, col: Rect) {
        let pal = self.pal;
        egui::ScrollArea::vertical().id_salt("studio-sections").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.add_space(10.0);
            for s in Section::ALL {
                let (rect, resp) = ui.allocate_exact_size(vec2(col.width() - 16.0, 44.0), Sense::click());
                crate::a11y::toggle(&resp, egui::WidgetType::SelectableLabel, true, st.section == s, s.label());
                let rect = rect.translate(vec2(8.0, 0.0));
                st.layout.sections.push((s, rect));
                let selected = st.section == s;
                let p = ui.painter();
                if selected {
                    pal.paint_selected(p, rect, 10, 0.2);
                } else {
                    p.rect_filled(rect, CornerRadius::same(10), if resp.hovered() { pal.row.gamma_multiply(0.7) } else { Color32::TRANSPARENT });
                }
                let ink = if selected { pal.accent } else { pal.text };
                (s.icon())(p, Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(22.0, 22.0)), if selected { pal.accent } else { pal.text_dim });
                p.text(pos2(rect.left() + 42.0, rect.center().y), Align2::LEFT_CENTER, s.label(), FontId::proportional(13.5), ink);
                if resp.clicked() {
                    st.section = s;
                    st.confirm_delete = false;
                }
            }
            ui.add_space(10.0);
        });
    }

    fn studio_settings_col(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, act: &mut Actions, col: Rect) {
        let pal = self.pal;
        let inner = col.shrink2(vec2(16.0, 0.0));
        ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
            ui.add_space(14.0);
            ui.label(RichText::new(st.section.label()).size(17.0).strong());
            ui.add(egui::Label::new(RichText::new(st.section.blurb()).size(12.0).color(pal.text_dim)).wrap());
            ui.add_space(8.0);
            let scroll_out = egui::ScrollArea::vertical()
                .id_salt(("studio-settings", st.section as u8))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    self.studio_controls(ui, st, brush, act);
                    ui.add_space(24.0);
                });
        crate::panels::rows::scroll_cue(ui, &scroll_out, self.pal.panel);
        });
    }

    /// Draw the rows of the open section. Consecutive sliders and toggles share one card.
    fn studio_controls(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, act: &mut Actions) {
        let pal = self.pal;
        let rows = controls(st.section, st.finger_taper);
        let grain_off = st.section == Section::Grain && st.working.grain.is_none();
        let wet_off = st.section == Section::WetMix && !brush.wet.enabled;
        let mut i = 0;
        while i < rows.len() {
            match rows[i] {
                Control::Group(name) => {
                    if i > 0 {
                        ui.add_space(6.0);
                    }
                    small_label(ui, &pal, &name.to_uppercase());
                    i += 1;
                    let start = i;
                    while i < rows.len() && matches!(rows[i], Control::Slider(_) | Control::Toggle(_)) {
                        i += 1;
                    }
                    if i > start {
                        let enabled = if st.section == Section::Grain {
                            !grain_off
                        } else {
                            !(wet_off && !matches!(rows[start], Control::Toggle(_)))
                        };
                        self.studio_card(ui, st, brush, act, &rows[start..i], enabled, wet_off);
                    }
                }
                Control::Slider(_) | Control::Toggle(_) => {
                    let start = i;
                    while i < rows.len() && matches!(rows[i], Control::Slider(_) | Control::Toggle(_)) {
                        i += 1;
                    }
                    self.studio_card(ui, st, brush, act, &rows[start..i], !grain_off, wet_off);
                }
                Control::Note(text) => {
                    ui.add(egui::Label::new(RichText::new(text).size(11.5).color(pal.text_dim)).wrap());
                    i += 1;
                }
                Control::TipPicker => {
                    self.tip_picker(ui, st, brush, act);
                    i += 1;
                }
                Control::GrainPicker => {
                    self.grain_picker(ui, st, brush, act);
                    i += 1;
                }
                Control::GrainMode => {
                    ui.add_enabled_ui(!grain_off, |ui| {
                        let sel = usize::from(brush.grain_mode == GrainMode::Moving);
                        if let Some(n) = segmented(ui, &pal, &["Texturized", "Moving"], sel, &mut st.layout.segments) {
                            act.edits.push(Box::new(move |b| b.grain_mode = if n == 1 { GrainMode::Moving } else { GrainMode::Texturized }));
                        }
                    });
                    ui.add(egui::Label::new(RichText::new(if brush.grain_mode == GrainMode::Moving {
                        "Moving: every stamp carries its own patch of grain."
                    } else {
                        "Texturized: the grain is fixed to the canvas, like paper."
                    }).size(11.5).color(pal.text_dim)).wrap());
                    i += 1;
                }
                Control::Accumulation => {
                    let sel = usize::from(brush.accumulation == Accumulation::Buildup);
                    if let Some(n) = segmented(ui, &pal, &["Glaze", "Build-up"], sel, &mut st.layout.segments) {
                        act.edits.push(Box::new(move |b| b.accumulation = if n == 1 { Accumulation::Buildup } else { Accumulation::Glaze }));
                    }
                    ui.add(egui::Label::new(RichText::new(if brush.accumulation == Accumulation::Buildup {
                        "Build-up: stamps in one stroke add up, like an airbrush."
                    } else {
                        "Glaze: one stroke is one layer. Crossing yourself does not darken it."
                    }).size(11.5).color(pal.text_dim)).wrap());
                    i += 1;
                }
                Control::TaperTabs => {
                    let sel = usize::from(st.finger_taper);
                    if let Some(n) = segmented(ui, &pal, &["Pencil", "Touch"], sel, &mut st.layout.segments) {
                        st.finger_taper = n == 1;
                    }
                    i += 1;
                }
                Control::Curves(set) => {
                    let slot = usize::from(st.section == Section::WetMix);
                    self.curve_block(ui, st, brush, act, set, slot, !wet_off);
                    i += 1;
                }
                Control::AboutCard => {
                    self.about_card(ui, st, brush, act);
                    i += 1;
                }
            }
        }
    }

    /// A rounded card holding sliders and toggles separated by hairlines.
    #[allow(clippy::too_many_arguments)]
    fn studio_card(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, act: &mut Actions, rows: &[Control], enabled: bool, wet_off: bool) {
        let pal = self.pal;
        Frame::new().fill(pal.row).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(14, 2)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (n, c) in rows.iter().enumerate() {
                if n > 0 {
                    let y = ui.cursor().top();
                    ui.painter().line_segment([pos2(ui.max_rect().left(), y), pos2(ui.max_rect().right(), y)], Stroke::new(1.0, pal.hairline));
                }
                match c {
                    Control::Slider(def) => {
                        let value = (def.get)(brush);
                        // The first toggle of Wet mix stays live while the rest are off.
                        let on = enabled && !(wet_off && st.section == Section::WetMix);
                        if let Some(v) = slider_row(ui, &pal, def, value, on, &mut st.layout.sliders) {
                            let set = def.set;
                            act.edits.push(Box::new(move |b| set(b, v)));
                        }
                    }
                    Control::Toggle(def) => {
                        let value = (def.get)(brush);
                        let on = enabled || st.section == Section::WetMix;
                        if toggle_row(ui, &pal, def, value, on) {
                            let set = def.set;
                            act.edits.push(Box::new(move |b| set(b, !value)));
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    fn tip_picker(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, act: &mut Actions) {
        let pal = self.pal;
        small_label(ui, &pal, "TIP");
        let cols = 4;
        let gap = 8.0;
        let cell_w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let cell_h = cell_w + 18.0;
        let current = st.working.tip_kind;
        for (row, chunk) in TipKind::ALL.chunks(cols).enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (k, kind) in chunk.iter().enumerate() {
                    let idx = row * cols + k;
                    let (rect, resp) = ui.allocate_exact_size(vec2(cell_w, cell_h), Sense::click());
                    st.layout.cells.push((true, idx, rect));
                    let tex = thumb(ui.ctx(), st, true, idx);
                    picker_cell(ui, &pal, rect, &resp, current == Some(*kind), kind.label(), Some(tex), brush.invert_shape);
                    if resp.clicked() {
                        act.tip = Some(*kind);
                    }
                }
            });
        }
        if current.is_none() {
            ui.add(egui::Label::new(RichText::new("This brush uses a tip image from a file. Pick one above to replace it.").size(11.5).color(pal.text_dim)).wrap());
        }
    }

    fn grain_picker(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, act: &mut Actions) {
        let pal = self.pal;
        small_label(ui, &pal, "GRAIN");
        let cols = 4;
        let gap = 8.0;
        let cell_w = (ui.available_width() - gap * (cols as f32 - 1.0)) / cols as f32;
        let cell_h = cell_w + 18.0;
        let all: Vec<Option<GrainKind>> = std::iter::once(None).chain(GrainKind::ALL.into_iter().map(Some)).collect();
        let has_grain = st.working.grain.is_some();
        let current = st.working.grain_kind;
        for (row, chunk) in all.chunks(cols).enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (k, kind) in chunk.iter().enumerate() {
                    let idx = row * cols + k;
                    let (rect, resp) = ui.allocate_exact_size(vec2(cell_w, cell_h), Sense::click());
                    st.layout.cells.push((false, idx, rect));
                    let tex = kind.map(|_| thumb(ui.ctx(), st, false, idx - 1));
                    let selected = match kind {
                        None => !has_grain,
                        Some(g) => has_grain && current == Some(*g),
                    };
                    picker_cell(ui, &pal, rect, &resp, selected, kind.map_or("None", GrainKind::label), tex, false);
                    if resp.clicked() {
                        act.grain = Some(*kind);
                    }
                }
            });
        }
        if has_grain && current.is_none() {
            ui.add(egui::Label::new(RichText::new("This brush uses a grain image from a file. Pick one above to replace it.").size(11.5).color(pal.text_dim)).wrap());
        }
        let _ = brush;
    }

    /// Chips to pick a curve, and the editor for the picked one.
    #[allow(clippy::too_many_arguments)]
    fn curve_block(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, act: &mut Actions, set: &'static [CurveId], slot: usize, enabled: bool) {
        let pal = self.pal;
        let pick = st.curve_pick[slot].min(set.len() - 1);
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if !enabled {
                ui.disable();
            }
            for (n, id) in set.iter().enumerate() {
                let active = n == pick;
                let text = RichText::new(id.label()).size(12.5).color(if active { pal.on_accent } else { pal.text });
                let chip = egui::Button::new(text).fill(if active { pal.accent_fill } else { pal.row }).corner_radius(CornerRadius::same(14)).min_size(vec2(0.0, 28.0));
                if ui.add(chip).clicked() {
                    st.curve_pick[slot] = n;
                }
            }
        });
        let id = set[pick];
        let points = id.points(&brush.curves).clone();
        ui.add_enabled_ui(enabled, |ui| {
            if let Some(new) = curve_editor(ui, &pal, &points, Id::new(("studio-curve", id as u8))) {
                act.edits.push(Box::new(move |b| *id.points_mut(&mut b.curves) = new));
            }
            let identity = id.points(&Brush::default().curves).clone();
            let changed = points != identity;
            ui.horizontal(|ui| {
                if ui.add_enabled(changed, egui::Button::new("Reset curve").min_size(vec2(0.0, 30.0))).clicked() {
                    act.edits.push(Box::new(move |b| *id.points_mut(&mut b.curves) = identity));
                }
            });
            ui.add(egui::Label::new(RichText::new("Drag a point to shape the curve. Tap the plot to add one, double-tap a point to remove it.").size(11.5).color(pal.text_dim)).wrap());
        });
    }

    fn about_card(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, act: &mut Actions) {
        let pal = self.pal;
        Frame::new().fill(pal.row).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(14, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(if st.name.trim().is_empty() { "My brush" } else { st.name.trim() }).size(16.0).strong());
            let kind = match &st.target {
                BrushId::User(_) => "One of My Brushes",
                BrushId::Imported(_) => "An imported brush",
                BrushId::Library(_) => "An Eskiz brush",
            };
            small_label(ui, &pal, kind);
            ui.add_space(6.0);
            let tip = st.working.tip_kind.map_or("Image from a file", TipKind::label);
            let grain = match (&st.working.grain, st.working.grain_kind) {
                (None, _) => "None".to_owned(),
                (Some(_), kind) => format!(
                    "{} ({})",
                    kind.map_or("Image from a file", GrainKind::label),
                    if brush.grain_mode == GrainMode::Moving { "moving" } else { "texturized" }
                ),
            };
            for (k, v) in [
                ("Tip", tip.to_owned()),
                ("Grain", grain),
                ("Painting", if brush.accumulation == Accumulation::Buildup { "Build-up".to_owned() } else { "Glaze".to_owned() }),
                ("Size", format!("{:.0} px", brush.size)),
            ] {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(k).size(12.5).color(pal.text_dim));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(v).size(12.5));
                    });
                });
            }
        });
        ui.horizontal(|ui| {
            let bw = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(vec2(bw, 38.0), Sense::hover());
            let label = if st.confirm_delete { "Tap again to delete" } else { "Delete" };
            if pill(ui, &pal, rect, label, ButtonStyle::Danger, st.is_user_brush()).clicked() {
                if st.confirm_delete {
                    act.delete = true;
                } else {
                    st.confirm_delete = true;
                }
            }
        });
        let note = if st.is_user_brush() {
            "Save writes the changes to this brush's file in My Brushes."
        } else {
            "Built-in and imported brushes are never changed. Save makes your own copy in My Brushes."
        };
        ui.add(egui::Label::new(RichText::new(note).size(11.5).color(pal.text_dim)).wrap());
        if let Some(status) = &st.status {
            ui.add(egui::Label::new(RichText::new(status).size(11.5).color(pal.accent)).wrap());
        }
    }

    /// The right column: a stroke preview and the drawing pad.
    fn studio_pad_col(&self, ui: &mut Ui, st: &mut StudioState, brush: &Brush, col: Rect) {
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let inner = col.shrink2(vec2(16.0, 0.0));
        let top = col.top() + 14.0;
        // Stroke preview strip.
        let strip = Rect::from_min_size(pos2(inner.left(), top), vec2(inner.width(), 66.0));
                st.layout.strip = Some(strip);
        let scale = crate::brush_library::BrushPreviews::scale_for(ctx.pixels_per_point());
        let key = (st.rev, scale, strip.width() as i32);
        if st.strip.as_ref().is_none_or(|(k, _)| *k != key) {
            let (w, h) = ((strip.width() * scale as f32) as u32, (strip.height() * scale as f32) as u32);
            let img = preview_stroke(&st.working.library_brush(), w.max(1), h.max(1));
            let image = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            let tex = ctx.load_texture("brush-studio-strip", image, TextureOptions::LINEAR);
            st.strip = Some((key, tex));
        }
        if let Some((_, tex)) = &st.strip {
            ui.painter().image(tex.id(), strip, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }

        // Pad header.
        let head_y = strip.bottom() + 14.0;
        small_label_at(ui, &pal, pos2(inner.left() + 2.0, head_y + 14.0), "DRAWING PAD");
        let clear = Rect::from_min_size(pos2(inner.right() - 78.0, head_y), vec2(78.0, 30.0));
        if pill(ui, &pal, clear, "Clear", ButtonStyle::Secondary, st.pad.strokes > 0 || st.pad.drawing).clicked() {
            st.pad.clear();
        }
        st.layout.clear = Some(clear);

        // The pad.
        let pad = Rect::from_min_max(pos2(inner.left(), head_y + 40.0), pos2(inner.right(), col.bottom() - 16.0));
        st.layout.pad = Some(pad);
        let pad_scale = (ctx.pixels_per_point().ceil() as u32).clamp(1, 2);
        st.pad.ensure((pad.width() * pad_scale as f32) as usize, (pad.height() * pad_scale as f32) as usize, pad_scale);
        ui.painter().rect_filled(pad, CornerRadius::same(12), PAD_PAPER);
        if let Some(tex) = st.pad.texture(&ctx) {
            ui.painter().image(tex.id(), pad, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        if st.pad.strokes == 0 && !st.pad.drawing {
            ui.painter().text(pad.center(), Align2::CENTER_CENTER, "Draw here to try the brush", FontId::proportional(13.0), Color32::from_black_alpha(90));
        }
        ui.painter().rect_stroke(pad, CornerRadius::same(12), Stroke::new(1.0, pal.separator), StrokeKind::Inside);
        let resp = ui.interact(pad, Id::new("studio-pad"), Sense::click_and_drag());
        crate::a11y::named(&resp, egui::WidgetType::Other, true, "Brush test pad, draw here to try the brush");
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        self.pad_input(ui, st, brush, pad);
    }

    /// Turn this frame's pointer, touch and pen input into pad strokes.
    fn pad_input(&self, ui: &Ui, st: &mut StudioState, brush: &Brush, pad: Rect) {
        let input = self.read_input(ui);
        let scale = f64::from(st.pad.scale());
        let to_px = |p: Pos2| (f64::from(p.x - pad.left()) * scale, f64::from(p.y - pad.top()) * scale);
        let started = input.primary_pressed && input.press_origin.is_some_and(|p| pad.contains(p));
        if started && !st.pad.drawing {
            let mut b = brush.clone();
            b.size *= scale;
            let raster = st.raster();
            st.pad.begin(&b, raster);
            st.pad.last = None;
        }
        if !st.pad.drawing {
            return;
        }
        if !input.primary_down && !input.primary_released {
            st.pad.end();
            return;
        }
        let mut samples: Vec<Sample> = Vec::new();
        if input.pen_samples.is_empty() {
            if let Some(p) = input.pointer {
                if st.pad.last != Some(p) || started {
                    st.pad.last = Some(p);
                    let (x, y) = to_px(p);
                    let touch = input.touch.is_some();
                    let raw = RawPointer {
                        x,
                        y,
                        time: input.time,
                        kind: if touch { PointerKind::Touch } else { PointerKind::Mouse },
                        force: input.touch.flatten().map(f64::from),
                    };
                    samples.push(sample_from(raw));
                }
            }
        } else {
            for s in &input.pen_samples {
                let (x, y) = to_px(s.pos);
                samples.push(sample_from_pen(x, y, s.time, s.reading));
            }
        }
        for s in samples {
            let s = apply_pressure_curve(s, &self.prefs.pressure_curve);
            st.pad.push(s);
        }
        if input.primary_released {
            st.pad.end();
        }
    }
}

fn small_label_at(ui: &Ui, pal: &Palette, at: Pos2, text: &str) {
    ui.painter().text(at, Align2::LEFT_CENTER, text, FontId::proportional(11.0), pal.text_dim);
}

/// The thumbnail of a tip (`tip` true) or a grain, made on first use.
fn thumb(ctx: &egui::Context, st: &mut StudioState, tip: bool, idx: usize) -> egui::TextureId {
    if let Some(t) = st.thumbs.get(&(tip, idx)) {
        return t.id();
    }
    const N: usize = 64;
    let values: Vec<f32> = if tip {
        // The generated tip, at half size.
        let full = generated_tip(TipKind::ALL[idx]);
        let n = full.width() as usize;
        let at = |x: usize, y: usize| f32::from(full.as_raw()[y * n + x]) / 255.0;
        (0..N * N)
            .map(|i| {
                let (x, y) = ((i % N) * n / N, (i / N) * n / N);
                let step = (n / N).max(1);
                let mut s = 0.0;
                for dy in 0..step {
                    for dx in 0..step {
                        s += at((x + dx).min(n - 1), (y + dy).min(n - 1));
                    }
                }
                s / (step * step) as f32
            })
            .collect()
    } else {
        // Half of the 256 px tile at half size: real grain, at about the scale it paints.
        let full = generated_grain(GrainKind::ALL[idx]);
        let n = full.width() as usize;
        let at = |x: usize, y: usize| f32::from(full.as_raw()[y * n + x]) / 255.0;
        (0..N * N)
            .map(|i| {
                let (x, y) = (i % N * 2, i / N * 2);
                let mut s = 0.0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    s += at(x + dx, y + dy);
                }
                s / 4.0
            })
            .collect()
    };
    let pixels: Vec<Color32> = values.iter().map(|v| Color32::from_white_alpha((v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
    let image = ColorImage { size: [N, N], source_size: vec2(N as f32, N as f32), pixels };
    let name = if tip { format!("studio-tip-{idx}") } else { format!("studio-grain-{idx}") };
    let tex = ctx.load_texture(name, image, TextureOptions::LINEAR);
    let id = tex.id();
    st.thumbs.insert((tip, idx), tex);
    id
}

#[allow(clippy::too_many_arguments)]
fn picker_cell(ui: &Ui, pal: &Palette, rect: Rect, resp: &egui::Response, selected: bool, label: &str, tex: Option<egui::TextureId>, invert: bool) {
    crate::a11y::toggle(resp, egui::WidgetType::SelectableLabel, true, selected, label);
    crate::a11y::focus_ring(ui.painter(), rect, resp, pal, 10);
    let p = ui.painter();
    if selected {
        pal.paint_selected(p, rect, 10, 0.18);
    } else {
        p.rect_filled(rect, CornerRadius::same(10), if resp.hovered() { pal.separator } else { pal.row });
    }
    let img = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, rect.width() - 12.0));
    // Paint sits on a darker well so the tip reads like ink on paper.
    p.rect_filled(img, CornerRadius::same(7), if pal.light { Color32::from_black_alpha(10) } else { Color32::from_black_alpha(60) });
    match tex {
        Some(id) => {
            let ink = if selected { pal.accent } else { pal.text };
            if invert {
                p.rect_filled(img, CornerRadius::same(7), ink.gamma_multiply(0.85));
            }
            p.image(id, img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), if invert { pal.panel } else { ink });
        }
        None => {
            // "None": a slashed circle.
            let c = img.center();
            let r = img.width() * 0.22;
            let col = pal.text_dim;
            p.circle_stroke(c, r, Stroke::new(1.75, col));
            p.line_segment([c + vec2(-r * 0.7, r * 0.7), c + vec2(r * 0.7, -r * 0.7)], Stroke::new(1.75, col));
        }
    }
    p.text(
        pos2(rect.center().x, rect.bottom() - 10.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if selected { pal.accent } else { pal.text_dim },
    );
    if selected {
        p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(2.0, pal.accent), StrokeKind::Inside);
    }
}

/// A two or three way switch. Returns the segment that was tapped.
fn segmented(ui: &mut Ui, pal: &Palette, labels: &[&str], selected: usize, rects: &mut Vec<(String, Rect)>) -> Option<usize> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), pal.row);
    let cell = rect.width() / labels.len() as f32;
    let mut out = None;
    for (i, label) in labels.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + cell * i as f32, rect.top()), vec2(cell, rect.height())).shrink(3.0);
        rects.push(((*label).to_owned(), r));
        let resp = ui.interact(r, ui.id().with(("segment", *label)), Sense::click());
        crate::a11y::toggle(&resp, egui::WidgetType::SelectableLabel, true, i == selected, *label);
        crate::a11y::focus_ring(ui.painter(), r, &resp, pal, 8);
        if i == selected {
            ui.painter().rect_filled(r, CornerRadius::same(8), pal.accent_fill);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(8), pal.separator);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, FontId::proportional(13.0), if i == selected { pal.on_accent } else { pal.text });
        if resp.clicked() && i != selected {
            out = Some(i);
        }
    }
    out
}

/// A labelled slider row. Returns the new value while it is dragged (or reset by a double tap).
fn slider_row(ui: &mut Ui, pal: &Palette, def: &SliderDef, value: f64, enabled: bool, rects: &mut Vec<(&'static str, Rect)>) -> Option<f64> {
    let sense = if enabled { Sense::click_and_drag() } else { Sense::hover() };
    let resp = ui.allocate_response(vec2(ui.available_width(), 56.0), sense);
    let rect = resp.rect;
    rects.push((def.label, rect));
    let (x0, x1) = (rect.left() + 10.0, rect.right() - 10.0);
    let mut out = None;
    if enabled && resp.has_focus() {
        // With keyboard focus the arrow keys move the value one step (a hundredth of the track
        // when the setting has no step), and Home and End go to the ends.
        crate::a11y::keep_arrows(ui, resp.id);
        let dir = ui.input_mut(|i| {
            let up = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight);
            let down = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft);
            f64::from(i32::from(up) - i32::from(down))
        });
        if dir != 0.0 {
            let next = if def.step > 0.0 {
                (value + dir * def.step).clamp(def.min, def.max)
            } else {
                def.from_unit(def.to_unit(value) + dir * 0.01)
            };
            if (next - value).abs() > 1e-9 {
                out = Some(next);
            }
        }
    }
    if enabled {
        // A quick run of taps counts as a triple tap in egui; both reset the slider.
        if resp.double_clicked() || resp.triple_clicked() {
            out = Some(def.default_value());
        } else if resp.dragged() || resp.is_pointer_button_down_on() {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = f64::from(((p.x - x0) / (x1 - x0)).clamp(0.0, 1.0));
                let mut v = def.from_unit(t);
                // Bipolar sliders stick to zero.
                if def.min < 0.0 && def.max > 0.0 && v.abs() < 0.025 * (def.max - def.min) {
                    v = 0.0;
                }
                if (v - value).abs() > 1e-9 {
                    out = Some(v);
                }
            }
        }
    }
    let shown = out.unwrap_or(value);
    crate::a11y::slider(&resp, enabled, format!("{} {}", def.label, def.unit.format(shown)));
    crate::a11y::focus_ring(ui.painter(), rect, &resp, pal, 8);
    let ink = if enabled { pal.text } else { pal.disabled() };
    let p = ui.painter();
    p.text(pos2(rect.left() + 2.0, rect.top() + 18.0), Align2::LEFT_CENTER, def.label, FontId::proportional(13.5), ink);
    p.text(pos2(rect.right() - 2.0, rect.top() + 18.0), Align2::RIGHT_CENTER, def.unit.format(shown), FontId::monospace(12.0), if enabled { pal.text_dim } else { pal.disabled() });
    let y = rect.top() + 41.0;
    let track_col = pal.text_dim.gamma_multiply(0.28);
    p.rect_filled(Rect::from_min_max(pos2(x0, y - 2.5), pos2(x1, y + 2.5)), CornerRadius::same(3), track_col);
    let at = |v: f64| x0 + (x1 - x0) * def.to_unit(v) as f32;
    let cx = at(shown);
    let origin = if def.min < 0.0 && def.max > 0.0 { at(0.0) } else { x0 };
    let fill = if enabled { pal.accent_fill } else { pal.accent_fill.gamma_multiply(0.35) };
    p.rect_filled(Rect::from_min_max(pos2(origin.min(cx), y - 2.5), pos2(origin.max(cx), y + 2.5)), CornerRadius::same(3), fill);
    // A tick where the default sits, so a double tap has a visible target.
    let dx = at(def.default_value());
    if (dx - cx).abs() > 6.0 {
        p.line_segment([pos2(dx, y - 6.0), pos2(dx, y + 6.0)], Stroke::new(1.0, pal.text_dim.gamma_multiply(0.6)));
    }
    if def.min < 0.0 && def.max > 0.0 && (origin - dx).abs() > 1.0 {
        p.line_segment([pos2(origin, y - 6.0), pos2(origin, y + 6.0)], Stroke::new(1.0, pal.text_dim.gamma_multiply(0.6)));
    }
    let knob = if enabled { Color32::WHITE } else { Color32::from_gray(150) };
    p.circle_filled(pos2(cx, y + 1.0), 11.0, Color32::from_black_alpha(36));
    p.circle_filled(pos2(cx, y), 10.0, knob);
    p.circle_stroke(pos2(cx, y), 10.0, Stroke::new(1.0, Color32::from_black_alpha(60)));
    if !def.help.is_empty() && resp.hovered() {
        resp.on_hover_text(def.help);
    }
    out
}

/// A labelled switch row. Returns true when it was tapped.
fn toggle_row(ui: &mut Ui, pal: &Palette, def: &ToggleDef, value: bool, enabled: bool) -> bool {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.allocate_response(vec2(ui.available_width(), 48.0), sense);
    let rect = resp.rect;
    crate::a11y::toggle(&resp, egui::WidgetType::Checkbox, enabled, value, def.label);
    crate::a11y::focus_ring(ui.painter(), rect, &resp, pal, 8);
    let ink = if enabled { pal.text } else { pal.disabled() };
    ui.painter().text(pos2(rect.left() + 2.0, rect.center().y), Align2::LEFT_CENTER, def.label, FontId::proportional(13.5), ink);
    switch(ui.painter(), pal, Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), vec2(40.0, 24.0)), value, enabled);
    let clicked = resp.clicked();
    if !def.help.is_empty() && resp.hovered() {
        resp.on_hover_text(def.help);
    }
    clicked
}

/// An editable response curve on a square plot. Returns new control points after a change.
fn curve_editor(ui: &mut Ui, pal: &Palette, points: &Points, id: Id) -> Option<Points> {
    let side = ui.available_width().min(260.0);
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    let to_screen = |p: [f64; 2]| pos2(rect.left() + p[0] as f32 * rect.width(), rect.bottom() - p[1] as f32 * rect.height());
    let from_screen = |p: Pos2| [f64::from((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0), f64::from((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0)];
    let mut pts = points.clone();
    pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    let mut changed = false;

    // Tap on empty plot: add a point.
    let bg = ui.interact(rect, id.with("plot"), Sense::click());
    crate::a11y::named(&bg, egui::WidgetType::Other, true, "Response curve plot, tap to add a point");
    // Handles are registered after the plot, so they win the click when over one.
    let n = pts.len();
    let mut remove = None;
    for i in 0..n {
        let hit = Rect::from_center_size(to_screen(pts[i]), vec2(30.0, 30.0));
        let resp = ui.interact(hit, id.with(("pt", i)), Sense::click_and_drag());
        crate::a11y::slider(&resp, true, format!("Curve point {} of {}, input {:.0}%, output {:.0}%", i + 1, n, pts[i][0] * 100.0, pts[i][1] * 100.0));
        crate::a11y::focus_ring(ui.painter(), hit.shrink(6.0), &resp, pal, 15);
        if resp.has_focus() {
            // Arrow keys nudge the point by 1 %: up and down change the output, left and right
            // the input (the end points keep their input).
            crate::a11y::keep_arrows(ui, resp.id);
            let (dx, dy) = ui.input_mut(|k| {
                let mut d = |key| f64::from(i32::from(k.consume_key(egui::Modifiers::NONE, key)));
                let dy = d(egui::Key::ArrowUp) - d(egui::Key::ArrowDown);
                let dx = d(egui::Key::ArrowRight) - d(egui::Key::ArrowLeft);
                (dx, dy)
            });
            if dx != 0.0 || dy != 0.0 {
                let (lo, hi) = if i == 0 || i == n - 1 { (pts[i][0], pts[i][0]) } else { (pts[i - 1][0] + 0.01, pts[i + 1][0] - 0.01) };
                pts[i] = [(pts[i][0] + dx * 0.01).clamp(lo.min(hi), hi.max(lo)), (pts[i][1] + dy * 0.01).clamp(0.0, 1.0)];
                changed = true;
            }
        }
        if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let c = from_screen(p);
                let (lo, hi) = if i == 0 || i == n - 1 {
                    (pts[i][0], pts[i][0])
                } else {
                    (pts[i - 1][0] + 0.01, pts[i + 1][0] - 0.01)
                };
                pts[i] = [c[0].clamp(lo.min(hi), hi.max(lo)), c[1]];
                changed = true;
            }
        }
        if resp.double_clicked() && i != 0 && i != n - 1 && n > 2 {
            remove = Some(i);
        }
    }
    if let Some(i) = remove {
        pts.remove(i);
        changed = true;
    } else if bg.clicked() && !changed {
        if let Some(p) = bg.interact_pointer_pos() {
            let near = pts.iter().any(|q| to_screen(*q).distance(p) < 16.0);
            if !near && pts.len() < 8 {
                pts.push(from_screen(p));
                pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
                changed = true;
            }
        }
    }

    let p = ui.painter_at(rect.expand(10.0));
    p.rect_filled(rect, CornerRadius::same(10), pal.row);
    for k in 1..4 {
        let f = k as f32 / 4.0;
        let line = Stroke::new(1.0, pal.separator);
        p.line_segment([pos2(rect.left() + f * rect.width(), rect.top()), pos2(rect.left() + f * rect.width(), rect.bottom())], line);
        p.line_segment([pos2(rect.left(), rect.top() + f * rect.height()), pos2(rect.right(), rect.top() + f * rect.height())], line);
    }
    p.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, pal.disabled()));
    p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
    let curve = BrushCurve::new(&pts);
    let line: Vec<Pos2> = (0..=64)
        .map(|k| {
            let x = f64::from(k) / 64.0;
            to_screen([x, curve.eval(x).clamp(-0.05, 1.05)])
        })
        .collect();
    p.add(egui::Shape::line(line, Stroke::new(2.25, pal.accent)));
    for (i, q) in pts.iter().enumerate() {
        let c = to_screen(*q);
        if i == 0 || i == pts.len() - 1 {
            p.circle_filled(c, 5.0, pal.panel);
            p.circle_stroke(c, 5.0, Stroke::new(1.75, pal.accent));
        } else {
            p.circle_filled(c, 6.5, pal.accent);
            p.circle_stroke(c, 6.5, Stroke::new(2.0, pal.panel));
        }
    }
    p.text(rect.left_bottom() + vec2(8.0, -6.0), Align2::LEFT_BOTTOM, "Input", FontId::proportional(10.5), pal.text_dim);
    p.text(rect.left_top() + vec2(8.0, 6.0), Align2::LEFT_TOP, "Effect", FontId::proportional(10.5), pal.text_dim);
    changed.then_some(pts)
}
