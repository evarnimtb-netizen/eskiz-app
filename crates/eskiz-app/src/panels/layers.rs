//! Layers panel: cards with thumbnails, groups, drag to reorder, swipe
//! actions, the thumbnail menu, inline opacity and blend controls, and the background row.
//!
//! The panel never edits the document itself. While it draws, it collects [`Action`]s; once
//! the frame's widgets are done they are applied through [`crate::session::Session`], so each
//! one is a named, undoable step and the row list the frame was drawn from is never changed
//! under it. Card looks and the drop-target maths are in `layer_card.rs`, thumbnails in
//! `layer_thumbs.rs`, the background colour picker in `layer_picker.rs`.

use eframe::egui::{
    self, text::{CCursor, CCursorRange}, Align, Align2, Color32, CornerRadius, FontId, Id, LayerId, Margin, Order, Pos2, Rect,
    Sense, Stroke, StrokeKind, UiBuilder, Vec2,
};

use crate::app::EskizApp;
use crate::colour::{hsv_to_rgb, rgb_to_hsv, Hsv};
use crate::icons;
use crate::panels::layer_card::{self as card, CardGeo, CardPaint, DropTarget, Marker, Row, ACTIONS_W, CARD_H, CLIP_INDENT, GAP, INDENT};
use crate::panels::layer_picker::{self, PICKER_H};
use crate::panels::layer_thumbs::{self, ThumbCache, Want};
use crate::session::{Edit, MaskFill};
use crate::theme::Palette;
use eskiz_core::blend::BlendMode;

/// Distance a horizontal drag must pass to reveal the swipe actions, points.
pub const SWIPE_PX: f32 = 40.0;
/// Movement before a press on a card becomes a swipe or a reorder.
const START_PX: f32 = 6.0;
/// A touch must rest this long before a vertical drag lifts the card; sooner it scrolls.
const TOUCH_HOLD: f64 = 0.25;
/// Height of the inline opacity and blend controls.
const INLINE_H: f32 = 224.0;
const BLEND_LIST_H: f32 = 134.0;
const BLEND_ROW_H: f32 = 26.0;
const BG_ROW_H: f32 = 52.0;
const MENU_ROW_H: f32 = 34.0;
const MENU_W: f32 = 284.0;

/// An item of the thumbnail menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Rename,
    Select,
    Copy,
    Fill,
    Clear,
    Rasterize,
    AlphaLock,
    Mask,
    Clipping,
    Assist,
    Invert,
    Reference,
    MergeDown,
    CombineDown,
    Group,
    Ungroup,
    Duplicate,
    Delete,
}

/// Something the panel asks the session to do.
#[derive(Clone, Debug)]
enum Action {
    Add,
    Select(u64),
    ToggleVisible(u64, bool),
    ToggleCollapse(u64),
    ToggleControls(u64),
    ToggleMenu(u64),
    Reveal(Option<u64>),
    Lock(u64),
    Duplicate(u64),
    Delete(u64),
    Move { id: u64, parent: Option<u64>, index: usize },
    Menu(Act, u64),
    Opacity(u64, f64),
    EndDrag,
    Blend(u64, BlendMode),
    RenameCommit(u64, String),
    RenameCancel,
    BgVisible(bool),
    BgDrag([u8; 3]),
    BgSet([u8; 3]),
    TogglePicker,
}

/// The name being edited in place.
pub struct Rename {
    pub id: u64,
    pub text: String,
    fresh: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Undecided,
    Swipe,
    Reorder,
    Scroll,
}

/// A press on a card that may turn into a swipe or a reorder.
struct Press {
    id: u64,
    acc: Vec2,
    mode: Mode,
    start: f64,
    revealed: bool,
}

/// A card being dragged.
pub struct DragState {
    pub id: u64,
    grab: Vec2,
    pub pointer: Pos2,
    /// Where it would land if released now.
    pub target: Option<DropTarget>,
}

/// State of the Layers panel that is not the document.
#[derive(Default)]
pub struct LayersUi {
    pub thumbs: ThumbCache,
    /// Item whose opacity and blend controls are open under its card.
    pub controls: Option<u64>,
    /// Item whose thumbnail menu is open.
    pub menu: Option<u64>,
    pub rename: Option<Rename>,
    /// Item whose swipe actions are showing.
    pub revealed: Option<u64>,
    press: Option<Press>,
    pub drag: Option<DragState>,
    /// The background colour picker is open.
    pub picker: bool,
    bg_hsv: Option<Hsv>,
    /// Scroll the blend list to this item's mode when it is next drawn.
    blend_scroll: Option<u64>,
    last_seen: f64,
    scroll_to_active: bool,
    /// The menu was just opened: scroll its card up so the menu fits below, and wait a few
    /// frames for the scroll before drawing it.
    menu_scroll: bool,
    menu_wait: u8,
    menu_anchor_y: f32,
    /// Where each visible card was drawn last frame, for tests and hit checks.
    pub geometry: Vec<(u64, CardGeo)>,
    pub add_rect: Option<Rect>,
    pub bg_swatch: Option<Rect>,
    pub bg_check: Option<Rect>,
    pub menu_cells: Vec<(Act, Rect)>,
    pub slider: Option<Rect>,
    pub blend_cells: Vec<(BlendMode, Rect)>,
    pub swipe_buttons: Vec<(&'static str, Rect)>,
    pub picker_rect: Option<Rect>,
}

impl LayersUi {
    /// Escape belongs to the panel (cancel a rename or a drag, close the menu or the picker)
    /// before it closes the panel itself.
    pub fn wants_escape(&self) -> bool {
        self.rename.is_some() || self.drag.is_some() || self.menu.is_some() || self.picker
    }

    /// The drawn geometry of a card.
    pub fn geo(&self, id: u64) -> Option<CardGeo> {
        self.geometry.iter().find(|(i, _)| *i == id).map(|(_, g)| *g)
    }
}

fn menu_items(row: &Row, can_delete: bool, can_merge: bool, can_combine: bool, has_base: bool, is_text: bool) -> Vec<(Act, String, bool, bool, bool)> {
    // (action, label, enabled, checked, danger)
    let it = |a, l: &str, en, ch, dg| (a, l.to_owned(), en, ch, dg);
    if row.group {
        return vec![
            it(Act::Rename, "Rename", true, false, false),
            it(Act::Select, "Select", false, false, false),
            it(Act::Copy, "Copy", false, false, false),
            it(Act::Mask, if row.has_mask { "Remove Mask" } else { "Add Mask" }, true, false, false),
            it(Act::Invert, "Invert", false, false, false),
            it(Act::Ungroup, "Ungroup", true, false, false),
            it(Act::Duplicate, "Duplicate", true, false, false),
            it(Act::Delete, "Delete", can_delete, false, true),
        ];
    }
    // A text layer is edited through its source: pixel fills and clears wait until it is rasterised.
    let mut items = vec![
        it(Act::Rename, "Rename", true, false, false),
        it(Act::Select, "Select", false, false, false),
        it(Act::Copy, "Copy", false, false, false),
        it(Act::Fill, "Fill Layer", !row.locked && !is_text, false, false),
        it(Act::Clear, "Clear", !row.locked && !is_text, false, false),
        it(Act::AlphaLock, "Lock Transparency", true, row.alpha_lock, false),
        it(Act::Mask, if row.has_mask { "Remove Mask" } else { "Add Mask" }, true, false, false),
        it(Act::Clipping, "Clip to Layer Below", has_base, row.clipped, false),
        it(Act::Assist, "Guided Layer", true, false, false),
        it(Act::Invert, "Invert", false, false, false),
        it(Act::Reference, "Reference", false, false, false),
        it(Act::MergeDown, "Merge Down", can_merge, false, false),
        it(Act::CombineDown, "Combine Down", can_combine, false, false),
        it(Act::Group, "Group", true, false, false),
        it(Act::Duplicate, "Duplicate", true, false, false),
        it(Act::Delete, "Delete", can_delete, false, true),
    ];
    if is_text {
        items.insert(3, it(Act::Rasterize, "Rasterize Text", !row.locked, false, false));
    }
    items
}

fn menu_height(items: usize) -> f32 {
    items.div_ceil(2) as f32 * MENU_ROW_H + 16.0
}

/// A mouse pointer over `rect` gets a soft highlight.
fn hover_fill(p: &egui::Painter, pal: &Palette, rect: Rect, radius: u8) {
    p.rect_filled(rect, CornerRadius::same(radius), if pal.light { Color32::from_black_alpha(14) } else { Color32::from_white_alpha(16) });
}

impl EskizApp {
    /// Show an action result: redraw, and keep the selection where it is.
    fn layer_keep(&mut self, edit: Option<Edit>) -> bool {
        match edit {
            Some(e) => {
                self.apply_refresh(e.refresh);
                true
            }
            None => false,
        }
    }

    /// Show an action result and select the item it produced.
    fn layer_select(&mut self, edit: Option<Edit>) -> bool {
        match edit {
            Some(e) => {
                if let Some(id) = e.id {
                    self.active = id;
                }
                self.apply_refresh(e.refresh);
                true
            }
            None => false,
        }
    }

    /// Forget panel state that points at things that are gone, and reset it when the panel was closed.
    fn layers_sanitize(&mut self, ui: &egui::Ui) {
        let now = self.now;
        let down = ui.input(|i| i.pointer.any_down() || i.pointer.any_released());
        let doc = &self.session.doc;
        let u = &mut self.layers_ui;
        if now - u.last_seen > 0.3 {
            u.menu = None;
            u.rename = None;
            u.revealed = None;
            u.picker = false;
            u.press = None;
            u.drag = None;
            u.scroll_to_active = true;
        }
        u.last_seen = now;
        for slot in [&mut u.controls, &mut u.menu, &mut u.revealed] {
            if slot.is_some_and(|id| doc.node(id).is_none()) {
                *slot = None;
            }
        }
        if u.rename.as_ref().is_some_and(|r| doc.node(r.id).is_none()) {
            u.rename = None;
        }
        if u.drag.as_ref().is_some_and(|d| doc.node(d.id).is_none()) || !down {
            u.drag = None;
        }
        if !down {
            u.press = None;
        }
    }

    pub(crate) fn layers_body(&mut self, ui: &mut egui::Ui) {
        if self.three_d.is_some() {
            self.three_d_layers_body(ui);
            return;
        }
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        self.layers_sanitize(ui);
        let rows = card::rows(&self.session.doc);
        let wants: Vec<Want> = rows
            .iter()
            .map(|r| Want {
                id: r.id,
                layer: !r.group,
                mask: r.has_mask,
            })
            .collect();
        if self.layers_ui.thumbs.refresh(&ctx, &self.session, &wants, layer_thumbs::BUDGET) {
            ctx.request_repaint();
        }
        let mut acts: Vec<Action> = Vec::new();
        let busy = self.session.is_stroking();

        // Title and the add button.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Layers").size(17.0).strong());
            crate::user_guide::help_link(ui, &pal, crate::user_guide::Link::Layers);
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let (rect, resp) = ui.allocate_exact_size(Vec2::splat(36.0), if busy { Sense::hover() } else { Sense::click() });
                crate::a11y::named(&resp, egui::WidgetType::Button, !busy, "Add layer");
                if resp.hovered() && !busy {
                    ui.painter().circle_filled(rect.center(), 18.0, pal.row);
                }
                icons::icon_plus(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(22.0)), if busy { pal.text_dim } else { pal.text });
                if resp.clicked() {
                    acts.push(Action::Add);
                }
                self.layers_ui.add_rect = Some(rect);
            });
        });
        ui.add_space(4.0);

        // The cards.
        let menu_open = self.layers_ui.menu.is_some();
        let picker_open = self.layers_ui.picker;
        let mut min_h = 0.0f32;
        let list_top = ui.cursor().top();
        if menu_open {
            let id = self.layers_ui.menu.unwrap_or(0);
            let n = rows.iter().find(|r| r.id == id).map_or(15, |r| if r.group { 8 } else { 15 });
            // Room for the menu below its card, so it does not cover the card it belongs to.
            let below = self.layers_ui.geo(id).map_or(CARD_H, |g| (g.card.bottom() - list_top).max(CARD_H));
            min_h = min_h.max(below + 6.0 + menu_height(n) + 4.0);
        }
        if picker_open {
            min_h = min_h.max(PICKER_H + 12.0);
        }
        let max_list = (ui.available_height() - BG_ROW_H - 12.0).max(120.0);
        let mut anchor: Option<Rect> = None;
        self.layers_ui.geometry.clear();
        self.layers_ui.swipe_buttons.clear();
        self.layers_ui.slider = None;
        self.layers_ui.blend_cells.clear();
        let out = egui::ScrollArea::vertical()
            .id_salt("layers-scroll")
            .max_height(max_list)
            .auto_shrink([false, true])
            .animated(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = GAP;
                self.layer_list(ui, &rows, &mut acts, &mut anchor);
            });
        // A short list still leaves room for the menu or the picker floating over it.
        let used = ui.min_rect().bottom() - list_top;
        let mut list = Rect::from_min_size(out.inner_rect.min, Vec2::new(out.inner_rect.width(), used.max(0.0)));
        if list.height() < min_h {
            ui.add_space(min_h - list.height());
            list = Rect::from_min_size(list.min, Vec2::new(list.width(), min_h));
        }

        // Background colour row.
        ui.add_space(8.0);
        let (bg_row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), BG_ROW_H), Sense::hover());
        self.background_row(ui, bg_row, &mut acts);

        // Overlays drawn over the list: the thumbnail menu and the background picker.
        let settled = anchor.is_some_and(|a| {
            let u = &mut self.layers_ui;
            let same = (a.top() - u.menu_anchor_y).abs() < 0.5;
            u.menu_anchor_y = a.top();
            same
        });
        if self.layers_ui.menu_wait > 0 && !settled {
            self.layers_ui.menu_wait -= 1;
            self.layers_ui.menu_cells.clear();
            ctx.request_repaint();
        } else if let (Some(id), Some(anchor)) = (self.layers_ui.menu, anchor) {
            self.layers_ui.menu_wait = 0;
            if let Some(row) = rows.iter().find(|r| r.id == id) {
                self.layer_menu(ui, list, anchor, row, &mut acts);
            }
        } else {
            self.layers_ui.menu_cells.clear();
        }
        if self.layers_ui.picker {
            self.background_picker(ui, list, bg_row, &mut acts);
        } else {
            self.layers_ui.picker_rect = None;
        }

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            let u = &mut self.layers_ui;
            if u.rename.is_some() {
                acts.push(Action::RenameCancel);
            } else if u.menu.is_some() || u.picker {
                u.menu = None;
                u.picker = false;
            }
        }
        if !acts.is_empty() || self.layers_ui.drag.is_some() {
            ctx.request_repaint();
        }
        self.layers_apply(&rows, acts);
    }

    /// All cards, the open inline controls, and the drag in progress.
    fn layer_list(&mut self, ui: &mut egui::Ui, rows: &[Row], acts: &mut Vec<Action>, anchor: &mut Option<Rect>) {
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let now = self.now;
        let width = ui.available_width();
        let left = ui.cursor().left();
        let viewport = ui.clip_rect();
        // Escape puts a lifted card back (egui also ends the drag on Escape, so check first).
        if self.layers_ui.drag.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.layers_ui.drag = None;
        }
        let reordering = self.layers_ui.drag.is_some();
        let mut cards: Vec<Rect> = Vec::with_capacity(rows.len());
        let mut extents: Vec<Rect> = Vec::with_capacity(rows.len());
        let mut ghost: Option<(usize, Rect)> = None;

        for (i, row) in rows.iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, CARD_H), Sense::hover());
            let base = rect.left() + row.depth as f32 * INDENT + if row.clipped { CLIP_INDENT } else { 0.0 };
            let full = Rect::from_min_max(Pos2::new(base, rect.top()), rect.right_bottom());
            let press = self.layers_ui.press.as_ref().filter(|p| p.id == row.id && p.mode == Mode::Swipe);
            let target_off = if self.layers_ui.revealed == Some(row.id) { ACTIONS_W } else { 0.0 };
            let off = match press {
                Some(p) => ((if p.revealed { ACTIONS_W } else { 0.0 }) - p.acc.x).clamp(0.0, ACTIONS_W + 24.0),
                None => ctx.animate_value_with_time(Id::new(("lyr-off", row.id)), target_off, 0.15),
            };
            let card = full.translate(Vec2::new(-off, 0.0));
            cards.push(full);
            let painter = ui.painter().with_clip_rect(rect.intersect(viewport));
            let selected = self.active == row.id;
            let dragging_this = self.layers_ui.drag.as_ref().is_some_and(|d| d.id == row.id);
            let renaming = self.layers_ui.rename.as_ref().is_some_and(|r| r.id == row.id);

            if off > 0.5 {
                self.swipe_buttons(ui, &painter, rect, row, off, acts);
            }

            let g = card::geo(card, row);
            self.layers_ui.geometry.push((row.id, g));
            // Registered first so the parts below sit on top of it.
            let resp = ui.interact(card, Id::new(("lyr-card", row.id)), Sense::click_and_drag());
            crate::a11y::toggle(&resp, egui::WidgetType::Button, true, selected, card_name(row));
            let (mut r_chev, mut r_thumb, mut r_letter, mut r_check) = (None, None, None, None);
            if !reordering && off < 1.0 {
                r_chev = g.chevron.map(|c| ui.interact(c.expand2(Vec2::new(2.0, 8.0)), Id::new(("lyr-chev", row.id)), Sense::click()));
                r_thumb = Some(ui.interact(g.thumb, Id::new(("lyr-thumb", row.id)), Sense::click()));
                r_letter = Some(ui.interact(g.letter.expand2(Vec2::new(2.0, 6.0)), Id::new(("lyr-letter", row.id)), Sense::click()));
                r_check = Some(ui.interact(g.check.expand(4.0), Id::new(("lyr-check", row.id)), Sense::click()));
            }
            if let Some(r) = &r_check {
                crate::a11y::toggle(r, egui::WidgetType::Checkbox, true, row.visible, format!("Visible, {}", row.name));
            }
            if let Some(r) = &r_letter {
                crate::a11y::named(r, egui::WidgetType::Button, true, format!("Blend mode {}, {}", row.blend.name(), row.name));
            }
            if let Some(r) = &r_thumb {
                crate::a11y::named(r, egui::WidgetType::Button, true, format!("Layer menu, {}", row.name));
            }
            if let Some(r) = &r_chev {
                let verb = if row.collapsed { "Expand" } else { "Collapse" };
                crate::a11y::named(r, egui::WidgetType::Button, true, format!("{verb} {}", row.name));
            }

            let thumb = self.layers_ui.thumbs.layer(row.id).map(|t| (t.tex.id(), t.size));
            let mask = self.layers_ui.thumbs.mask(row.id).map(|t| (t.tex.id(), t.size));
            let st = CardPaint {
                selected,
                open: self.layers_ui.controls == Some(row.id),
                alpha: if dragging_this { 0.3 } else { 1.0 },
                thumb,
                mask,
                show_name: !renaming,
            };
            if resp.hovered() && !selected && !reordering && off < 1.0 {
                hover_fill(&painter, &pal, card, 10);
            }
            card::paint_card(&painter, &pal, card, row, &st);
            if self.layers_ui.menu == Some(row.id) {
                *anchor = Some(g.thumb);
                if self.layers_ui.menu_scroll {
                    self.layers_ui.menu_scroll = false;
                    let need = menu_height(if row.group { 8 } else { 15 }) + 8.0;
                    if card.bottom() + need > viewport.bottom() {
                        resp.scroll_to_me(Some(Align::Min));
                    } else {
                        self.layers_ui.menu_wait = 0;
                    }
                }
            }
            if self.layers_ui.scroll_to_active && selected && !viewport.contains_rect(card) {
                resp.scroll_to_me(Some(Align::Center));
            }
            if selected {
                self.layers_ui.scroll_to_active = false;
            }

            // Taps on the parts.
            if let Some(r) = r_chev {
                if r.clicked() {
                    acts.push(Action::ToggleCollapse(row.id));
                }
            }
            if let Some(r) = r_thumb {
                if r.clicked() {
                    acts.push(if selected { Action::ToggleMenu(row.id) } else { Action::Select(row.id) });
                }
            }
            if let Some(r) = r_letter {
                if r.clicked() {
                    acts.push(Action::Select(row.id));
                    acts.push(Action::ToggleControls(row.id));
                }
            }
            if let Some(r) = r_check {
                if r.clicked() {
                    acts.push(Action::ToggleVisible(row.id, !row.visible));
                }
            }
            if resp.clicked() {
                if self.layers_ui.revealed.is_some() {
                    acts.push(Action::Reveal(None));
                }
                acts.push(Action::Select(row.id));
            }

            // Swipe and reorder.
            let pointer = ui.input(|i| i.pointer.latest_pos());
            let touch = ui.input(|i| i.any_touches());
            if resp.drag_started() {
                self.layers_ui.press = Some(Press {
                    id: row.id,
                    acc: Vec2::ZERO,
                    mode: Mode::Undecided,
                    start: now,
                    revealed: self.layers_ui.revealed == Some(row.id),
                });
            }
            if resp.dragged() {
                let delta = resp.drag_delta();
                let mut lift = None;
                if let Some(p) = self.layers_ui.press.as_mut().filter(|p| p.id == row.id) {
                    p.acc += delta;
                    if p.mode == Mode::Undecided && p.acc.length() > START_PX {
                        p.mode = if p.acc.x.abs() > p.acc.y.abs() {
                            Mode::Swipe
                        } else if touch && now - p.start < TOUCH_HOLD {
                            Mode::Scroll
                        } else {
                            Mode::Reorder
                        };
                        if p.mode == Mode::Reorder {
                            lift = Some(p.id);
                        }
                    }
                    if p.mode == Mode::Scroll {
                        ui.scroll_with_delta(Vec2::new(0.0, delta.y));
                    }
                }
                if let (Some(id), Some(pos)) = (lift, pointer) {
                    self.layers_ui.drag = Some(DragState {
                        id,
                        grab: pos - card.min,
                        pointer: pos,
                        target: None,
                    });
                    acts.push(Action::Select(id));
                    self.layers_ui.controls = None;
                    self.layers_ui.menu = None;
                }
            }
            if resp.drag_stopped() {
                if let Some(p) = self.layers_ui.press.take().filter(|p| p.id == row.id) {
                    match p.mode {
                        Mode::Swipe => {
                            let open = if p.revealed { p.acc.x < SWIPE_PX } else { p.acc.x < -SWIPE_PX };
                            acts.push(Action::Reveal(open.then_some(row.id)));
                        }
                        Mode::Reorder => {
                            if let Some(t) = self.layers_ui.drag.as_ref().and_then(|d| d.target).filter(|t| !t.noop) {
                                acts.push(Action::Move {
                                    id: row.id,
                                    parent: t.parent,
                                    index: t.index,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                self.layers_ui.drag = None;
            }
            if dragging_this {
                if let Some(d) = &self.layers_ui.drag {
                    ghost = Some((i, Rect::from_min_size(d.pointer - d.grab, card.size())));
                }
            }

            if renaming {
                self.rename_field(ui, row, g.name, acts);
            }

            // Inline opacity and blend controls.
            let mut extent = rect;
            if self.layers_ui.controls == Some(row.id) {
                let (r, _) = ui.allocate_exact_size(Vec2::new(width, INLINE_H), Sense::hover());
                let block = Rect::from_min_max(Pos2::new(base, r.top()), r.right_bottom());
                self.inline_controls(ui, block, row, acts);
                extent = rect.union(r);
            }
            extents.push(extent);
        }

        // The drag in progress: pointer, target, marker and the lifted card.
        if let Some(d) = self.layers_ui.drag.as_mut() {
            if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                d.pointer = pos;
                // Scroll when held near the top or bottom edge.
                if pos.y < viewport.top() + 26.0 {
                    ui.scroll_with_delta(Vec2::new(0.0, 9.0));
                } else if pos.y > viewport.bottom() - 26.0 {
                    ui.scroll_with_delta(Vec2::new(0.0, -9.0));
                }
                d.target = card::drop_target(&self.session.doc, rows, &cards, &extents, d.id, pos, left);
            }
        }
        if let Some((i, r)) = ghost {
            let layer = LayerId::new(Order::Tooltip, Id::new("layers-ghost"));
            let p = ctx.layer_painter(layer);
            // Follow the pointer sideways only a little, so the card still reads as a row.
            let x = cards[i].left() + (r.left() - cards[i].left()).clamp(-24.0, 24.0);
            let r = Rect::from_min_size(Pos2::new(x, r.top()), r.size());
            p.add(crate::theme::shadow(&pal).as_shape(r, CornerRadius::same(10)));
            let row = &rows[i];
            let thumb = self.layers_ui.thumbs.layer(row.id).map(|t| (t.tex.id(), t.size));
            let mask = self.layers_ui.thumbs.mask(row.id).map(|t| (t.tex.id(), t.size));
            let st = CardPaint {
                selected: true,
                open: false,
                alpha: 0.92,
                thumb,
                mask,
                show_name: true,
            };
            card::paint_card(&p, &pal, r, row, &st);
        }
        // The marker goes over the lifted card, so it stays visible under the pointer.
        let target = self.layers_ui.drag.as_ref().and_then(|d| d.target).filter(|t| !t.noop);
        if let Some(t) = target {
            let layer = LayerId::new(Order::Tooltip, Id::new("layers-ghost"));
            let p = ctx.layer_painter(layer).with_clip_rect(viewport);
            match t.marker {
                Marker::Line { y, depth } => {
                    let x0 = left + depth as f32 * INDENT;
                    // Light on dark (dark on light) with a thin outline, so it shows over both
                    // the accent card being dragged and the panel.
                    let bar = Rect::from_min_max(Pos2::new(x0, y - 1.5), Pos2::new(left + width, y + 1.5));
                    p.rect_filled(bar.expand(1.0), CornerRadius::same(3), Color32::from_black_alpha(90));
                    p.rect_filled(bar, CornerRadius::same(2), pal.text);
                    p.circle_filled(Pos2::new(x0 + 4.0, y), 6.0, pal.text);
                    p.circle_stroke(Pos2::new(x0 + 4.0, y), 6.0, Stroke::new(2.0, pal.accent));
                }
                Marker::Into { row } => {
                    let r = cards[row];
                    p.rect_filled(r, CornerRadius::same(10), pal.selected_bg(0.22));
                    p.rect_stroke(r, CornerRadius::same(10), Stroke::new(2.5, pal.accent), StrokeKind::Inside);
                }
            }
        }
    }

    /// Lock, Duplicate and Delete behind a swiped card.
    fn swipe_buttons(&mut self, ui: &mut egui::Ui, painter: &egui::Painter, row_rect: Rect, row: &Row, off: f32, acts: &mut Vec<Action>) {
        let pal = self.pal;
        let t = (off / ACTIONS_W).clamp(0.0, 1.0);
        let w = ACTIONS_W / 3.0;
        let x0 = row_rect.right() - ACTIONS_W;
        let lock_label = if row.locked { "Unlock" } else { "Lock" };
        let items: [(&'static str, icons::IconFn, Color32); 3] = [
            (lock_label, icons::icon_lock, if pal.is_eskiz() { pal.track } else { Color32::from_rgb(0x6E, 0x76, 0x86) }),
            ("Duplicate", icons::icon_layers, pal.accent_fill),
            ("Delete", icons::icon_trash, pal.danger),
        ];
        let can_delete = self.can_delete_layer(row.id);
        for (k, (label, icon, fill)) in items.into_iter().enumerate() {
            let cell = Rect::from_min_size(Pos2::new(x0 + w * k as f32, row_rect.top()), Vec2::new(w, CARD_H));
            let radius = if k == 2 { CornerRadius { nw: 0, sw: 0, ne: 10, se: 10 } } else { CornerRadius::ZERO };
            let enabled = k != 2 || can_delete;
            let fill = if enabled { fill } else { fill.gamma_multiply(0.4) };
            painter.rect_filled(cell, radius, fill.gamma_multiply(t));
            icon(painter, Rect::from_center_size(Pos2::new(cell.center().x, cell.top() + 21.0), Vec2::splat(22.0)), Color32::WHITE.gamma_multiply(t));
            painter.text(
                Pos2::new(cell.center().x, cell.bottom() - 11.0),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(11.0),
                Color32::WHITE.gamma_multiply(t),
            );
            self.layers_ui.swipe_buttons.push((label, cell));
            if self.layers_ui.revealed == Some(row.id) && off > ACTIONS_W * 0.8 {
                let resp = ui.interact(cell, Id::new(("lyr-act", row.id, k)), if enabled { Sense::click() } else { Sense::hover() });
                crate::a11y::named(&resp, egui::WidgetType::Button, enabled, format!("{label}, {}", row.name));
                if resp.clicked() {
                    acts.push(match k {
                        0 => Action::Lock(row.id),
                        1 => Action::Duplicate(row.id),
                        _ => Action::Delete(row.id),
                    });
                }
            }
        }
    }

    /// The rename text field over the card's name.
    fn rename_field(&mut self, ui: &mut egui::Ui, row: &Row, name: Rect, acts: &mut Vec<Action>) {
        let ctx = ui.ctx().clone();
        let Some(rn) = self.layers_ui.rename.as_mut() else {
            return;
        };
        let field = Rect::from_center_size(name.center(), Vec2::new(name.width(), 30.0));
        // A white field on the accent card reads as "editing" in both themes.
        ui.painter().rect_filled(field, CornerRadius::same(7), Color32::from_gray(0xF6));
        ui.painter().rect_stroke(field, CornerRadius::same(7), Stroke::new(1.5, Color32::from_black_alpha(60)), StrokeKind::Inside);
        let id = Id::new(("lyr-rename", row.id));
        let edit = egui::TextEdit::singleline(&mut rn.text)
            .id(id)
            .font(FontId::proportional(13.0))
            .frame(egui::Frame::NONE)
            .margin(Margin::symmetric(8, 6))
            .text_color(Color32::from_gray(0x1C))
            .vertical_align(Align::Center)
            .desired_width(field.width());
        let resp = ui.put(field, edit);
        if rn.fresh {
            rn.fresh = false;
            resp.request_focus();
            let len = rn.text.chars().count();
            if let Some(mut state) = egui::TextEdit::load_state(&ctx, id) {
                state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(len))));
                state.store(&ctx, id);
            }
        } else if resp.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                acts.push(Action::RenameCancel);
            } else {
                acts.push(Action::RenameCommit(row.id, rn.text.clone()));
            }
        }
    }

    /// Opacity slider and blend-mode list, open under a card.
    fn inline_controls(&mut self, ui: &mut egui::Ui, block: Rect, row: &Row, acts: &mut Vec<Action>) {
        let pal = self.pal;
        let p = ui.painter().clone();
        let radius = CornerRadius::same(10);
        p.rect_filled(block, radius, pal.panel);
        p.rect_stroke(block, radius, Stroke::new(1.0, pal.separator), StrokeKind::Inside);
        let pad = 12.0;
        p.text(Pos2::new(block.left() + pad, block.top() + 16.0), Align2::LEFT_CENTER, "Opacity", FontId::proportional(12.0), pal.text_dim);

        // Slider.
        let track = Rect::from_min_max(Pos2::new(block.left() + pad + 10.0, block.top() + 38.0), Pos2::new(block.right() - pad - 10.0, block.top() + 38.0));
        let hit = Rect::from_min_max(Pos2::new(block.left() + pad, block.top() + 26.0), Pos2::new(block.right() - pad, block.top() + 52.0));
        let resp = ui.interact(hit, Id::new(("lyr-opacity", row.id)), Sense::click_and_drag());
        self.layers_ui.slider = Some(hit);
        let mut value = row.opacity as f32;
        if resp.is_pointer_button_down_on() {
            if let Some(pos) = resp.interact_pointer_pos() {
                value = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
                if (f64::from(value) - row.opacity).abs() > 1e-4 {
                    acts.push(Action::Opacity(row.id, f64::from(value)));
                }
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            acts.push(Action::EndDrag);
        }
        p.text(
            Pos2::new(block.right() - pad, block.top() + 16.0),
            Align2::RIGHT_CENTER,
            format!("{:.0}%", value * 100.0),
            FontId::proportional(12.0),
            pal.text,
        );
        let bar = Rect::from_center_size(Pos2::new(hit.center().x, hit.center().y), Vec2::new(track.width() + 20.0, 6.0));
        p.rect_filled(bar, CornerRadius::same(3), pal.row);
        let tx = track.left() + track.width() * value;
        p.rect_filled(Rect::from_min_max(bar.min, Pos2::new(tx, bar.max.y)), CornerRadius::same(3), pal.accent_fill);
        p.circle_filled(Pos2::new(tx, bar.center().y), 10.0, Color32::WHITE);
        p.circle_stroke(Pos2::new(tx, bar.center().y), 10.0, Stroke::new(1.0, Color32::from_black_alpha(70)));

        // Blend modes.
        p.text(Pos2::new(block.left() + pad, block.top() + 70.0), Align2::LEFT_CENTER, "Blend mode", FontId::proportional(12.0), pal.text_dim);
        let list = Rect::from_min_size(Pos2::new(block.left() + 8.0, block.top() + 82.0), Vec2::new(block.width() - 16.0, BLEND_LIST_H));
        let scroll_to = self.layers_ui.blend_scroll == Some(row.id);
        let cells = &mut self.layers_ui.blend_cells;
        let mut scrolled = false;
        ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("blend-scroll", row.id))
                .max_height(BLEND_LIST_H)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for mode in BlendMode::ALL {
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), BLEND_ROW_H), Sense::click());
                        crate::a11y::toggle(&resp, egui::WidgetType::SelectableLabel, true, mode == row.blend, mode.name());
                        let on = mode == row.blend;
                        if on {
                            ui.painter().rect_filled(r, CornerRadius::same(7), pal.accent_fill);
                        } else if resp.hovered() {
                            hover_fill(ui.painter(), &pal, r, 7);
                        }
                        let fg = if on { pal.on_accent } else { pal.text };
                        ui.painter().text(Pos2::new(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, mode.name(), FontId::proportional(13.0), fg);
                        ui.painter().text(
                            Pos2::new(r.right() - 12.0, r.center().y),
                            Align2::RIGHT_CENTER,
                            card::blend_letter(mode),
                            FontId::proportional(12.0),
                            if on { Color32::from_white_alpha(210) } else { pal.text_dim },
                        );
                        if on && scroll_to {
                            resp.scroll_to_me(Some(Align::Center));
                            scrolled = true;
                        }
                        cells.push((mode, r));
                        if resp.clicked() {
                            acts.push(Action::Blend(row.id, mode));
                        }
                    }
                });
        });
        if scrolled {
            self.layers_ui.blend_scroll = None;
        }
    }

    /// The menu that opens from the selected card's thumbnail.
    fn layer_menu(&mut self, ui: &mut egui::Ui, list: Rect, anchor: Rect, row: &Row, acts: &mut Vec<Action>) {
        let pal = self.pal;
        let can_delete = self.can_delete_layer(row.id);
        let can_merge = self.session.can_merge_down(row.id);
        let below = self.session.item_below(row.id);
        let can_combine = below.is_some();
        let is_text = self.session.doc.texts.contains_key(&row.id);
        let mut items = menu_items(row, can_delete, can_merge, can_combine, below.is_some_and(|b| self.session.doc.node(b).is_some_and(|n| !n.is_group())), is_text);
        for item in items.iter_mut().filter(|i| matches!(i.0, Act::Assist)) {
            item.3 = self.guides.assist_on(row.id);
        }
        let h = menu_height(items.len());
        let x = (anchor.left() - 6.0).clamp(list.left(), (list.right() - MENU_W).max(list.left()));
        let below_top = anchor.bottom() + 6.0;
        let y = if below_top + h <= list.bottom() { below_top } else { (anchor.top() - 6.0 - h).max(list.top()) };
        let y = y.min((list.bottom() - h).max(list.top()));
        let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(MENU_W, h));
        let p = ui.painter().clone();
        let frame = CornerRadius::same(12);
        p.add(crate::theme::shadow(&pal).as_shape(rect, frame));
        p.rect_filled(rect, frame, pal.panel);
        p.rect_stroke(rect, frame, Stroke::new(1.0, pal.separator), StrokeKind::Inside);
        // The nub toward the thumbnail.
        let nx = anchor.center().x.clamp(rect.left() + 18.0, rect.right() - 18.0);
        if y >= anchor.bottom() {
            p.add(egui::Shape::convex_polygon(
                vec![Pos2::new(nx - 6.0, rect.top() + 0.5), Pos2::new(nx, rect.top() - 6.0), Pos2::new(nx + 6.0, rect.top() + 0.5)],
                pal.panel,
                Stroke::NONE,
            ));
        }
        let rows_n = items.len().div_ceil(2);
        let cell_w = (MENU_W - 16.0) / 2.0;
        self.layers_ui.menu_cells.clear();
        for (i, (act, label, enabled, checked, danger)) in items.iter().enumerate() {
            let (col, r) = (i / rows_n, i % rows_n);
            let cell = Rect::from_min_size(
                Pos2::new(rect.left() + 8.0 + col as f32 * cell_w, rect.top() + 8.0 + r as f32 * MENU_ROW_H),
                Vec2::new(cell_w, MENU_ROW_H),
            );
            // Disabled items still take the click, so it does not fall through to a card below.
            let resp = ui.interact(cell, Id::new(("lyr-menu", *act as u8)), Sense::click());
            crate::a11y::toggle(&resp, egui::WidgetType::Button, *enabled, *checked, label.to_string());
            if *enabled && resp.hovered() {
                hover_fill(&p, &pal, cell.shrink2(Vec2::new(1.0, 1.0)), 8);
            }
            let colour = if !enabled {
                pal.disabled()
            } else if *danger {
                pal.danger
            } else {
                pal.text
            };
            p.text(Pos2::new(cell.left() + 10.0, cell.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), colour);
            if *checked {
                icons::icon_check(&p, Rect::from_center_size(Pos2::new(cell.right() - 16.0, cell.center().y), Vec2::splat(16.0)), pal.accent);
            }
            self.layers_ui.menu_cells.push((*act, cell));
            if *enabled && resp.clicked() {
                acts.push(Action::Menu(*act, row.id));
            }
        }
        // A press outside the menu (and its thumbnail) closes it.
        let press = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None });
        if press.is_some_and(|o| !rect.contains(o) && !anchor.contains(o)) {
            self.layers_ui.menu = None;
        }
    }

    /// "Background colour": swatch (tap for the picker) and visibility checkbox.
    fn background_row(&mut self, ui: &mut egui::Ui, rect: Rect, acts: &mut Vec<Action>) {
        let pal = self.pal;
        let p = ui.painter().clone();
        p.rect_filled(rect, CornerRadius::same(10), pal.row);
        p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
        let (colour, visible) = (self.session.doc.background, self.session.doc.background_visible);
        let rgb = colour.map(|v| (v * 255.0).round() as u8);
        let dim = if visible { 1.0 } else { 0.45 };
        let swatch = Rect::from_min_size(Pos2::new(rect.left() + 8.0, rect.center().y - 20.0), Vec2::new(58.0, 40.0));
        let r = CornerRadius::same(6);
        p.rect_filled(swatch, r, { let s = crate::colour_management::show(rgb); Color32::from_rgb(s[0], s[1], s[2]) }.gamma_multiply(dim));
        p.rect_stroke(swatch, r, Stroke::new(1.0, Color32::from_gray(128).gamma_multiply(0.6)), StrokeKind::Inside);
        if !visible {
            // A slash marks a hidden background.
            p.line_segment([swatch.left_bottom() + Vec2::new(8.0, -6.0), swatch.right_top() + Vec2::new(-8.0, 6.0)], Stroke::new(2.0, pal.text_dim));
        }
        if self.layers_ui.picker {
            p.rect_stroke(swatch.expand(2.0), CornerRadius::same(8), Stroke::new(2.0, pal.accent), StrokeKind::Outside);
        }
        let resp = ui.interact(swatch, Id::new("bg-swatch"), Sense::click());
        crate::a11y::named(&resp, egui::WidgetType::Button, true, "Background color");
        if resp.clicked() {
            acts.push(Action::TogglePicker);
        }
        p.text(Pos2::new(swatch.right() + 12.0, rect.center().y), Align2::LEFT_CENTER, "Background color", FontId::proportional(13.0), pal.text.gamma_multiply(dim.max(0.7)));
        let check = Rect::from_center_size(Pos2::new(rect.right() - 8.0 - 13.0, rect.center().y), Vec2::splat(26.0));
        card::draw_checkbox(&p, &pal, check, visible, false);
        let resp = ui.interact(check.expand(6.0), Id::new("bg-check"), Sense::click());
        crate::a11y::toggle(&resp, egui::WidgetType::Checkbox, true, visible, "Background visible");
        if resp.clicked() {
            acts.push(Action::BgVisible(!visible));
        }
        self.layers_ui.bg_swatch = Some(swatch);
        self.layers_ui.bg_check = Some(check);
    }

    /// The compact colour picker, floating over the bottom of the list.
    fn background_picker(&mut self, ui: &mut egui::Ui, list: Rect, bg_row: Rect, acts: &mut Vec<Action>) {
        let pal = self.pal;
        let rgb = self.session.doc.background.map(|v| (v * 255.0).round() as u8);
        let mut hsv = match self.layers_ui.bg_hsv {
            Some(h) if hsv_to_rgb(h) == rgb => h,
            _ => rgb_to_hsv(rgb),
        };
        let rect = Rect::from_min_size(Pos2::new(bg_row.left(), (bg_row.top() - 8.0 - PICKER_H).max(list.top())), Vec2::new(bg_row.width(), PICKER_H));
        let out = layer_picker::compact_picker(ui, &pal, rect, &mut hsv);
        self.layers_ui.bg_hsv = Some(hsv);
        self.layers_ui.picker_rect = Some(rect);
        if let Some(c) = out.dragged {
            acts.push(Action::BgDrag(c));
        }
        if let Some(c) = out.preset {
            acts.push(Action::BgSet(c));
        }
        if out.ended {
            acts.push(Action::EndDrag);
        }
        let swatch = self.layers_ui.bg_swatch.unwrap_or(Rect::NOTHING);
        let press = ui.input(|i| if i.pointer.any_pressed() { i.pointer.press_origin() } else { None });
        if press.is_some_and(|o| !rect.contains(o) && !swatch.expand(4.0).contains(o)) {
            self.layers_ui.picker = false;
        }
    }

    /// Can item `id` be deleted? A document keeps at least one raster layer.
    fn can_delete_layer(&self, id: u64) -> bool {
        let Some(n) = self.session.doc.node(id) else {
            return false;
        };
        let mut ids = Vec::new();
        n.layer_ids(&mut ids);
        ids.len() < self.session.doc.layer_count()
    }

    fn layer_refused(&mut self, id: u64) {
        let locked = self.session.doc.effective_locked(id);
        self.show_toast(if locked { "The layer is locked." } else { "That is not possible right now." }, 2.0);
    }

    /// Carry out what the frame collected, each as one named, undoable session step.
    fn layers_apply(&mut self, rows: &[Row], acts: Vec<Action>) {
        for act in acts {
            match act {
                Action::Add => {
                    let e = self.session.add_layer(Some(self.active));
                    self.layer_select(e);
                }
                Action::Select(id) => {
                    self.active = id;
                    let u = &mut self.layers_ui;
                    if u.revealed.is_some_and(|r| r != id) {
                        u.revealed = None;
                    }
                    if u.menu.is_some_and(|m| m != id) {
                        u.menu = None;
                    }
                }
                Action::ToggleVisible(id, v) => {
                    let e = self.session.set_visible(id, v);
                    self.layer_keep(e);
                }
                Action::ToggleCollapse(id) => {
                    let collapsed = rows.iter().find(|r| r.id == id).is_some_and(|r| r.collapsed);
                    self.session.set_collapsed(id, !collapsed);
                }
                Action::ToggleControls(id) => {
                    let u = &mut self.layers_ui;
                    u.controls = if u.controls == Some(id) { None } else { Some(id) };
                    u.blend_scroll = u.controls;
                    u.menu = None;
                }
                Action::ToggleMenu(id) => {
                    let u = &mut self.layers_ui;
                    u.menu = if u.menu == Some(id) { None } else { Some(id) };
                    u.menu_scroll = u.menu.is_some();
                    u.menu_wait = 20;
                    u.menu_anchor_y = f32::NAN;
                    u.picker = false;
                }
                Action::Reveal(id) => {
                    self.layers_ui.revealed = id;
                    self.layers_ui.menu = None;
                }
                Action::Lock(id) => {
                    let locked = self.session.doc.node(id).is_some_and(|n| n.props.locked);
                    let e = self.session.set_locked(id, !locked);
                    self.layer_keep(e);
                    self.layers_ui.revealed = None;
                }
                Action::Duplicate(id) => {
                    let e = self.session.duplicate_layer(id);
                    self.layer_select(e);
                    self.layers_ui.revealed = None;
                }
                Action::Delete(id) => self.delete_item(rows, id),
                Action::Move { id, parent, index } => {
                    if let Some(g) = parent {
                        self.session.set_collapsed(g, false);
                    }
                    let e = self.session.move_item(id, parent, index);
                    self.layer_keep(e);
                }
                Action::Menu(act, id) => {
                    self.layers_ui.menu = None;
                    self.run_menu_act(rows, act, id);
                }
                Action::Opacity(id, v) => {
                    let e = self.session.drag_opacity(id, v);
                    self.layer_keep(e);
                }
                Action::EndDrag => self.session.end_drag(),
                Action::Blend(id, mode) => {
                    let e = self.session.set_blend(id, mode);
                    self.layer_keep(e);
                }
                Action::RenameCommit(id, text) => {
                    self.layers_ui.rename = None;
                    let text = text.trim().to_owned();
                    if !text.is_empty() {
                        let e = self.session.set_name(id, &text);
                        self.layer_keep(e);
                    }
                }
                Action::RenameCancel => self.layers_ui.rename = None,
                Action::BgVisible(v) => {
                    let e = self.session.set_background_visible(v);
                    self.layer_keep(e);
                }
                Action::BgDrag(c) => {
                    let e = self.session.drag_background(c.map(|v| f64::from(v) / 255.0));
                    self.layer_keep(e);
                }
                Action::BgSet(c) => {
                    let e = self.session.set_background_colour(c.map(|v| f64::from(v) / 255.0));
                    self.layer_keep(e);
                }
                Action::TogglePicker => {
                    let u = &mut self.layers_ui;
                    u.picker = !u.picker;
                    u.menu = None;
                }
            }
        }
    }

    /// Delete an item and select its neighbour (below it, else above).
    fn delete_item(&mut self, rows: &[Row], id: u64) {
        let pos = rows.iter().position(|r| r.id == id);
        let neighbour = pos.and_then(|i| {
            let skip = |r: &&Row| r.id != id && !self.session.doc.is_ancestor(id, r.id);
            rows[i + 1..].iter().find(skip).or_else(|| rows[..i].iter().rev().find(skip)).map(|r| r.id)
        });
        self.layers_ui.revealed = None;
        let e = self.session.delete_layer(id);
        if self.layer_keep(e) {
            if self.active == id || self.session.doc.node(self.active).is_none() {
                self.active = self.session.valid_active(neighbour);
            }
        } else {
            self.show_toast("A document needs at least one layer.", 2.0);
        }
    }

    fn run_menu_act(&mut self, rows: &[Row], act: Act, id: u64) {
        let Some(row) = rows.iter().find(|r| r.id == id) else {
            return;
        };
        match act {
            Act::Rename => {
                self.layers_ui.rename = Some(Rename {
                    id,
                    text: row.name.clone(),
                    fresh: true,
                });
            }
            Act::Fill => {
                let colour = self.colour;
                match self.session.fill_layer(id, colour) {
                    Some(e) => {
                        self.layer_keep(Some(e));
                        self.note_colour_used(colour);
                    }
                    None => self.layer_refused(id),
                }
            }
            Act::Clear => {
                let e = self.session.clear_layer(id);
                if !self.layer_keep(e) {
                    self.layer_refused(id);
                }
            }
            Act::AlphaLock => {
                let e = self.session.set_alpha_lock(id, !row.alpha_lock);
                self.layer_keep(e);
            }
            Act::Mask => {
                let e = if row.has_mask { self.session.remove_mask(id) } else { self.session.add_mask(id, MaskFill::Reveal) };
                self.layer_keep(e);
            }
            Act::Clipping => {
                let e = self.session.set_clipped(id, !row.clipped);
                self.layer_keep(e);
            }
            Act::MergeDown => {
                let e = self.session.merge_down(id);
                if !self.layer_select(e) {
                    self.layer_refused(id);
                }
            }
            Act::CombineDown => {
                let e = self.session.combine_down(id);
                self.layer_select(e);
            }
            Act::Group => {
                let e = self.session.group_items(&[id]);
                self.layer_select(e);
            }
            Act::Ungroup => {
                let e = self.session.ungroup(id);
                self.layer_select(e);
            }
            Act::Duplicate => {
                let e = self.session.duplicate_layer(id);
                self.layer_select(e);
            }
            Act::Rasterize => self.rasterize_text_action(id),
            Act::Delete => self.delete_item(rows, id),
            // Selection, clipboard, mask inversion and the reference window come with the
            // Selection phase; the menu lists them greyed out.
            Act::Assist => {
                let on = self.guides.assist_on(id);
                self.guides.set_assist(id, !on);
            }
            Act::Select | Act::Copy | Act::Invert | Act::Reference => {}
        }
    }
}

/// Accessible name of a layer card: what it is, whether it shows, its blend mode and opacity.
fn card_name(row: &Row) -> String {
    let kind = if row.group { "Group" } else { "Layer" };
    let state = if row.visible { "visible" } else { "hidden" };
    format!("{kind} {}, {state}, {}, opacity {:.0}%", row.name, row.blend.name(), row.opacity * 100.0)
}
