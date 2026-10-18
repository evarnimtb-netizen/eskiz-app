//! The Gallery's header, grid, cards, stacks and drag-and-drop.
//! A child of `gallery`, so it works on the gallery's state.
//!
//! Stacks: a stack is a card of its own that shows a 2 x 2 grid of its first four artworks.
//! Tapping it opens the stack, with a back button. Creating and changing stacks is by drag:
//!
//! * a long press (0.35 s, under 8 px of movement) lifts a card, which then follows the pointer;
//! * released over another card's middle half: a card goes into a stack (first), or two
//!   artworks make a new stack, or a card is dropped into a stack;
//! * released over a card's left or right quarter: the card moves before or after it (manual
//!   order only);
//! * inside an open stack, released outside the stack's grid (over the header or below it): the
//!   artwork leaves the stack and returns to the root list, after the stack.
//!
//! Shorter drags still scroll, and a leftward swipe still reveals Share, Duplicate and Delete.

use super::*;
use crate::stacks::{self, Item, SortKey, SortMode};

/// How long a press must rest before the card lifts, seconds.
const LIFT_SECS: f64 = 0.35;
/// Movement that cancels a press, points.
const LIFT_TRAVEL: f32 = 8.0;

/// One card on screen: an artwork, or a stack of artworks.
#[derive(Clone, Debug)]
pub(super) struct Tile {
    pub id: String,
    pub stack: bool,
    pub title: String,
    pub size: (usize, usize),
    /// The artwork itself, or a stack's artworks in order.
    pub members: Vec<DocEntry>,
}

/// Which part of a card the pointer is over when something is dropped on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Before,
    Middle,
    After,
}

fn zone(rect: Rect, p: Pos2) -> Zone {
    let f = (p.x - rect.left()) / rect.width().max(1.0);
    if f < 0.25 {
        Zone::Before
    } else if f > 0.75 {
        Zone::After
    } else {
        Zone::Middle
    }
}

/// The 2 x 2 mini grid on a stack's card.
fn stack_art(painter: &Painter, gp: &Palette, thumb: Rect, texs: &[Option<(TextureId, Vec2)>]) {
    let radius = CornerRadius::same(8);
    painter.rect_filled(thumb, radius, gp.panel);
    let inner = thumb.shrink(8.0);
    let gap = 4.0;
    let cell = Vec2::new((inner.width() - gap) / 2.0, (inner.height() - gap) / 2.0);
    for k in 0..4 {
        let (col, row) = ((k % 2) as f32, (k / 2) as f32);
        let r = Rect::from_min_size(inner.min + Vec2::new(col * (cell.x + gap), row * (cell.y + gap)), cell);
        match texs.get(k).copied().flatten() {
            Some((tid, size)) => {
                let uv = crop_uv_4x3(size.x, size.y);
                painter.add(RectShape::filled(r, CornerRadius::same(4), Color32::WHITE).with_texture(tid, uv));
            }
            None => {
                painter.rect_filled(r, CornerRadius::same(4), Color32::from_white_alpha(14));
            }
        }
    }
    painter.rect_stroke(thumb, radius, Stroke::new(1.0, Color32::from_white_alpha(24)), StrokeKind::Inside);
}

impl EskizApp {
    /// The cards on screen, in display order: the root list, or the artworks of the open stack.
    pub(super) fn tiles(&self) -> Vec<Tile> {
        let g = &self.gal;
        let find = |id: &str| g.entries.iter().find(|e| e.id == id).cloned();
        let art = |e: DocEntry| Tile { id: e.id.clone(), stack: false, title: e.title.clone(), size: (e.width, e.height), members: vec![e] };
        let tiles: Vec<Tile> = match &g.open_stack {
            Some(sid) => g.index.stack(sid).map(|s| s.docs.iter().filter_map(|d| find(d)).map(art).collect()).unwrap_or_default(),
            None => g
                .index
                .order
                .iter()
                .filter_map(|item| match item {
                    Item::Doc(id) => find(id).map(art),
                    Item::Stack(sid) => g.index.stack(sid).map(|s| Tile {
                        id: s.id.clone(),
                        stack: true,
                        title: s.name.clone(),
                        size: (0, 0),
                        members: s.docs.iter().filter_map(|d| find(d)).collect(),
                    }),
                })
                .collect(),
        };
        let keys: Vec<SortKey> = tiles
            .iter()
            .map(|t| SortKey {
                name: t.title.clone(),
                modified: t.members.iter().map(|m| m.modified).max().unwrap_or(SystemTime::UNIX_EPOCH),
                area: t.members.iter().map(|m| (m.width * m.height) as u64).max().unwrap_or(0),
            })
            .collect();
        let mut slots: Vec<Option<Tile>> = tiles.into_iter().map(Some).collect();
        stacks::display_order(&keys, g.index.sort).into_iter().filter_map(|i| slots[i].take()).collect()
    }

    /// The header bar: the title, and the buttons for the view on screen.
    pub(super) fn header_bar(&mut self, ui: &mut egui::Ui, gp: &Palette, bar: Rect, interactive: bool) {
        let painter = ui.painter().clone();
        painter.rect_filled(bar, CornerRadius::ZERO, gp.bar);
        painter.line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, gp.separator));
        // The title bar is hidden on macOS and the window is dragged by this bar.
        let drag = ui.interact(bar, Id::new("gallery-header-drag"), Sense::drag());
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        // The same macOS inset as the canvas top bar: clear of the traffic-light buttons.
        let left = crate::chrome::title_inset_for(self.mac_insets, self.fullscreen);
        let title_font = crate::chrome::title_font(ui.ctx(), gp);
        let cy = bar.center().y;
        let sense = if interactive { Sense::click() } else { Sense::hover() };
        let open = self.gal.open_stack.clone();
        let mut x = bar.right() - 24.0;
        // select, import, plus, sort, stack
        let mut clicked = (false, false, false, false, false);
        let mut back = false;
        let mut delete_stack = false;
        if let Some(sid) = open.as_ref() {
            // Back to the gallery, then the stack's name.
            let galley = painter.layout_no_wrap("Gallery".to_owned(), title_font.clone(), gp.text);
            let rect = Rect::from_min_size(Pos2::new(bar.left() + left - 8.0, cy - 18.0), Vec2::new(galley.size().x + 44.0, 36.0));
            let resp = ui.interact(rect, Id::new("gallery-back"), sense);
            crate::a11y::named(&resp, egui::WidgetType::Button, interactive, "Back to gallery");
            if resp.hovered() && interactive {
                painter.rect_filled(rect, CornerRadius::same(8), gp.row);
            }
            let c = Pos2::new(rect.left() + 12.0, cy);
            let chevron = Stroke::new(2.0, gp.text);
            painter.line_segment([c + Vec2::new(4.0, -7.0), c + Vec2::new(-3.0, 0.0)], chevron);
            painter.line_segment([c + Vec2::new(-3.0, 0.0), c + Vec2::new(4.0, 7.0)], chevron);
            painter.galley(Pos2::new(rect.left() + 28.0, cy - galley.size().y / 2.0), galley, gp.text);
            self.gal.header.back = rect;
            back = resp.clicked() && interactive;
            let name = self.gal.index.stack(sid).map_or_else(|| "Stack".to_owned(), |s| s.name.clone());
            let job = one_line(&name, 22.0, gp.text, bar.width() * 0.4);
            let title = painter.layout_job(job);
            painter.galley(Pos2::new(rect.right() + 8.0, cy - title.size().y / 2.0), title, gp.text);
            let (rect, hit) = header_text(ui, gp, &mut x, cy, "Delete stack", gp.danger, "gallery-delete-stack", sense);
            self.gal.header.delete_stack = rect;
            delete_stack = hit && interactive;
            x -= 8.0;
            let (rect, hit) = header_text(ui, gp, &mut x, cy, "Sort", gp.text, "gallery-sort", sense);
            self.gal.header.sort = rect;
            clicked.3 = hit;
        } else {
            self.gal.header.title = painter.text(Pos2::new(bar.left() + left, cy), Align2::LEFT_CENTER, "Gallery", title_font, gp.text);
            if self.gal.select {
                let (rect, hit) = header_text(ui, gp, &mut x, cy, "Done", gp.primary_text, "gallery-done", sense);
                self.gal.header.select = rect;
                clicked.0 = hit;
                x -= 12.0;
                let n = self.gal.selected.len();
                let (label, colour) = if n == 0 {
                    ("Delete".to_owned(), gp.disabled())
                } else {
                    (format!("Delete ({n})"), gp.danger)
                };
                let (_, hit) = header_text(ui, gp, &mut x, cy, &label, colour, "gallery-delete-selected", if n == 0 { Sense::hover() } else { sense });
                if hit && n > 0 {
                    let mut ids: Vec<String> = self.gal.selected.iter().cloned().collect();
                    ids.sort();
                    self.gal.confirm = Some(ids);
                }
                if n >= 2 {
                    x -= 12.0;
                    let (rect, hit) = header_text(ui, gp, &mut x, cy, &format!("Stack ({n})"), gp.accent, "gallery-stack-selected", sense);
                    self.gal.header.stack = rect;
                    clicked.4 = hit;
                }
            } else {
                // "+" is a 36 point accent circle with a drawn plus.
                let plus = Rect::from_center_size(Pos2::new(x - 18.0, cy), Vec2::splat(36.0));
                let resp = ui.interact(plus, Id::new("gallery-plus"), sense);
                crate::a11y::named(&resp, egui::WidgetType::Button, interactive, "New drawing");
                let fill = if resp.hovered() { gp.primary.gamma_multiply(0.85) } else { gp.primary };
                if gp.identity == crate::theme::UiIdentity::Eskiz {
                    // Eskiz: a rounded square tile with dark ink, not the accent circle.
                    painter.rect_filled(plus, CornerRadius::same(10), fill);
                } else {
                    painter.circle_filled(plus.center(), 18.0, fill);
                }
                icons::icon_plus(&painter, Rect::from_center_size(plus.center(), Vec2::splat(22.0)), gp.on_primary);
                self.gal.header.plus = plus;
                clicked.2 = resp.clicked() && interactive;
                x = plus.left() - 20.0;
                let (rect, hit) = header_text(ui, gp, &mut x, cy, "Import", gp.text, "gallery-import", sense);
                self.gal.header.import = rect;
                clicked.1 = hit;
                x -= 8.0;
                let (rect, hit) = header_text(ui, gp, &mut x, cy, "Select", gp.text, "gallery-select", sense);
                self.gal.header.select = rect;
                clicked.0 = hit;
                x -= 8.0;
                let (rect, hit) = header_text(ui, gp, &mut x, cy, "Sort", gp.text, "gallery-sort", sense);
                self.gal.header.sort = rect;
                clicked.3 = hit;
            }
        }
        if back {
            self.gal.open_stack = None;
            self.gal.revealed = None;
            self.gal.menu = None;
        }
        if delete_stack {
            if let Some(sid) = open {
                self.gal.stack_confirm = Some((sid, false));
            }
        }
        if clicked.0 {
            self.gal.select = !self.gal.select;
            self.gal.selected.clear();
            self.gal.revealed = None;
            self.gal.menu = None;
        }
        if clicked.1 {
            let anchor = self.gal.header.import;
            self.toggle_menu(Menu::Import, anchor);
        }
        if clicked.2 {
            let anchor = self.gal.header.plus;
            self.toggle_menu(Menu::NewCanvas, anchor);
        }
        if clicked.3 {
            let anchor = self.gal.header.sort;
            self.toggle_menu(Menu::Sort, anchor);
        }
        if clicked.4 {
            self.stack_selected();
        }
    }

    pub(crate) fn toggle_menu(&mut self, menu: Menu, anchor: Rect) {
        self.gal.revealed = None;
        self.gal.menu = if self.gal.menu.map(|m| m.0) == Some(menu) { None } else { Some((menu, anchor)) };
    }

    /// Draws the empty gallery's blank-canvas tile and returns its rectangle (it is a button).
    pub(super) fn empty_state(&mut self, ui: &mut egui::Ui, gp: &Palette, body: Rect) -> Rect {
        let painter = ui.painter();
        let c = body.center() - Vec2::new(0.0, 24.0);
        // A blank canvas with a plus: original artwork drawn in code.
        let frame = Rect::from_center_size(c - Vec2::new(0.0, 30.0), Vec2::new(132.0, 99.0));
        painter.add(Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(120) }.as_shape(frame, 8));
        painter.rect_filled(frame, CornerRadius::same(8), gp.panel);
        painter.rect_stroke(frame, CornerRadius::same(8), Stroke::new(1.0, gp.hairline), StrokeKind::Inside);
        icons::icon_plus(painter, Rect::from_center_size(frame.center(), Vec2::splat(40.0)), gp.text_dim);
        painter.text(c + Vec2::new(0.0, 50.0), Align2::CENTER_CENTER, "No artwork yet", FontId::proportional(20.0), gp.text);
        painter.text(
            c + Vec2::new(0.0, 78.0),
            Align2::CENTER_CENTER,
            "Click or tap + to start a new canvas, or choose Import to open a .procreate file.",
            FontId::proportional(13.0),
            gp.text_dim,
        );
        // Painter text has no accessibility node, so give the empty-state text one.
        let text_area = Rect::from_center_size(c + Vec2::new(0.0, 64.0), Vec2::new(460.0, 56.0));
        ui.interact(text_area, Id::new("gallery-empty-text"), Sense::hover()).widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                "No artwork yet. Click or tap plus to start a new canvas, or choose Import to open a .procreate file.",
            )
        });
        frame
    }

    /// The grid of cards for the root list or the open stack.
    pub(super) fn grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, gp: &Palette, body: Rect, interactive: bool, actions: &mut Vec<Action>) {
        let tiles = self.tiles();
        // The cards of last frame stay until this grid draws its own, so a lift that starts
        // before the grid draws can still find where the cards are.
        self.gal.card_rects.clear();
        let width = body.width();
        let cols = columns_for(width);
        let card_w = ((width - 2.0 * SIDE_MARGIN - GUTTER * (cols - 1) as f32) / cols as f32).floor();
        let thumb_h = (card_w * 0.75).round();
        let row_h = thumb_h + CARD_TEXT_H + GUTTER;
        let rows = tiles.len().div_ceil(cols);
        let total = Vec2::new(width, 24.0 + rows as f32 * row_h + 16.0);
        let (area, _) = ui.allocate_exact_size(total, Sense::hover());
        let clip = ui.clip_rect();

        let bg = ui.interact(area, Id::new("gallery-bg"), Sense::click());
        if interactive && bg.clicked() {
            actions.push(Action::Background);
        }

        let mut loads = 0;
        for (i, tile) in tiles.iter().enumerate() {
            let (col, row) = (i % cols, i / cols);
            let origin = area.min + Vec2::new(SIDE_MARGIN + col as f32 * (card_w + GUTTER), 24.0 + row as f32 * row_h);
            let thumb = Rect::from_min_size(origin, Vec2::new(card_w, thumb_h));
            let card = Rect::from_min_size(origin, Vec2::new(card_w, thumb_h + CARD_TEXT_H));
            self.gal.card_rects.push((tile.id.clone(), card));
            if !card.intersects(clip) {
                continue;
            }
            let shown = if tile.stack { 4 } else { 1 };
            let texs: Vec<Option<(TextureId, Vec2)>> = tile.members.iter().take(shown).map(|e| self.thumb_texture(ctx, e, &mut loads)).collect();
            self.card(ui, gp, tile, thumb, card, &texs, interactive, actions);
        }
    }

    /// The thumbnail texture for `entry`, loading it from disk if needed (a few per frame).
    fn thumb_texture(&mut self, ctx: &egui::Context, entry: &DocEntry, loads: &mut usize) -> Option<(TextureId, Vec2)> {
        let fresh = self.gal.thumbs.get(&entry.id).is_some_and(|s| s.modified == entry.modified);
        if !fresh && *loads < THUMB_LOADS_PER_FRAME {
            *loads += 1;
            let slot = match self.gal.library.as_ref().and_then(|l| l.thumbnail(&entry.id)) {
                Some(t) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
                    ThumbSlot {
                        modified: entry.modified,
                        tex: Some(ctx.load_texture(format!("thumb-{}", entry.id), image, TextureOptions::LINEAR)),
                        size: Vec2::new(t.width as f32, t.height as f32),
                    }
                }
                None => ThumbSlot { modified: entry.modified, tex: None, size: Vec2::ZERO },
            };
            self.gal.thumbs.insert(entry.id.clone(), slot);
        }
        self.gal.thumbs.get(&entry.id).and_then(|s| s.tex.as_ref().map(|t| (t.id(), s.size)))
    }

    #[allow(clippy::too_many_arguments)]
    fn card(
        &mut self,
        ui: &mut egui::Ui,
        gp: &Palette,
        tile: &Tile,
        thumb: Rect,
        card: Rect,
        texs: &[Option<(TextureId, Vec2)>],
        interactive: bool,
        actions: &mut Vec<Action>,
    ) {
        let painter = ui.painter().clone();
        let radius = CornerRadius::same(8);
        let id = tile.id.as_str();
        let selected = self.gal.selected.contains(id);
        let revealed = self.gal.revealed.as_deref() == Some(id);
        let editing = self.gal.renaming.as_ref().is_some_and(|r| r.0 == id);
        let frame = ui.ctx().cumulative_frame_nr();

        let sense = if interactive { Sense::click_and_drag() } else { Sense::hover() };
        let resp = ui.interact(card, Id::new(("gallery-card", id)), sense);
        crate::a11y::toggle(&resp, egui::WidgetType::Button, interactive, selected, format!("{} {}", if tile.stack { "Stack" } else { "Drawing" }, tile.title));

        // Shadow, art, hairline.
        painter.add(Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(130) }.as_shape(thumb, radius));
        if tile.stack {
            stack_art(&painter, gp, thumb, texs);
        } else {
            match texs.first().copied().flatten() {
                Some((tid, size)) => {
                    let uv = crop_uv_4x3(size.x, size.y);
                    painter.add(RectShape::filled(thumb, radius, Color32::WHITE).with_texture(tid, uv));
                }
                None => {
                    painter.rect_filled(thumb, radius, gp.panel);
                    icons::icon_layers(&painter, Rect::from_center_size(thumb.center(), Vec2::splat(36.0)), gp.text_dim);
                }
            }
            painter.rect_stroke(thumb, radius, Stroke::new(1.0, Color32::from_white_alpha(24)), StrokeKind::Inside);
        }
        if resp.hovered() && interactive && !revealed && self.gal.lift.is_none() {
            painter.rect_stroke(thumb.expand(1.0), CornerRadius::same(9), Stroke::new(2.0, Color32::from_white_alpha(70)), StrokeKind::Outside);
        }
        if selected {
            painter.rect_stroke(thumb.expand(2.0), CornerRadius::same(10), Stroke::new(3.0, gp.accent), StrokeKind::Outside);
        }

        // Title (tap to rename) and the line under it.
        let text_x = card.left() + 2.0;
        let title_rect = Rect::from_min_size(Pos2::new(text_x, thumb.bottom() + 6.0), Vec2::new(card.width() - 4.0, 26.0));
        let mut title_tapped = false;
        if interactive && !self.gal.select && !revealed && !editing && self.gal.lift.is_none() {
            let title = ui.interact(title_rect, Id::new(("gallery-title", id)), Sense::click());
            title_tapped = title.clicked();
            if title_tapped {
                self.gal.renaming = Some((id.to_owned(), tile.title.clone(), true));
            }
        }
        if editing {
            self.title_editor(ui, gp, id, title_rect);
        } else {
            painter.galley(
                Pos2::new(text_x, thumb.bottom() + 10.0),
                painter.layout_job(one_line(&tile.title, 14.0, gp.text, card.width() - 4.0)),
                gp.text,
            );
        }
        let line = if tile.stack {
            let n = tile.members.len();
            format!("{n} artwork{}", if n == 1 { "" } else { "s" })
        } else {
            format!("{} × {} px", tile.size.0, tile.size.1)
        };
        painter.text(Pos2::new(text_x, thumb.bottom() + 31.0), Align2::LEFT_TOP, line, FontId::proportional(11.0), gp.text_dim);

        if self.gal.select && !tile.stack {
            let c = thumb.left_top() + Vec2::new(18.0, 18.0);
            if selected {
                painter.circle_filled(c, 11.0, gp.accent_fill);
                icons::icon_check(&painter, Rect::from_center_size(c, Vec2::splat(18.0)), gp.on_accent);
            } else {
                painter.circle_filled(c, 11.0, Color32::from_black_alpha(90));
                painter.circle_stroke(c, 11.0, Stroke::new(2.0, Color32::WHITE));
            }
        }

        if revealed && !tile.stack {
            if let Some(entry) = tile.members.first() {
                self.card_actions(ui, gp, entry, thumb, actions);
            }
        }

        if !interactive {
            return;
        }
        // A still press that lasts becomes a lift (see `lift_frame`).
        if resp.is_pointer_button_down_on() && self.gal.press.is_none() && self.gal.lift.is_none() && !editing {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(card.center());
            let now = ui.input(|i| i.time);
            self.gal.press = Some((id.to_owned(), origin, now));
        }
        if self.gal.lift.is_some() {
            return;
        }
        if resp.drag_started() {
            self.gal.drag = Vec2::ZERO;
        }
        if resp.dragged() {
            self.gal.drag += resp.drag_delta();
            // A mostly vertical drag over a card scrolls the page, as touch users expect.
            if self.gal.drag.y.abs() > self.gal.drag.x.abs() * 1.5 {
                ui.scroll_with_delta(Vec2::new(0.0, resp.drag_delta().y));
            }
        }
        if resp.drag_stopped() {
            let d = self.gal.drag;
            if !self.gal.select && !tile.stack && d.x < -SWIPE_PX && d.x.abs() > d.y.abs() && frame != self.gal.drop_frame {
                actions.push(Action::Reveal(Some(id.to_owned())));
            }
            self.gal.drag = Vec2::ZERO;
        }
        if resp.secondary_clicked() && !self.gal.select && !tile.stack {
            actions.push(Action::Reveal(Some(id.to_owned())));
        } else if resp.clicked() && !title_tapped && !editing && frame != self.gal.drop_frame {
            actions.push(Action::Tap(id.to_owned(), thumb));
        }
    }

    /// The inline title editor. Enter or a tap elsewhere saves; Escape cancels.
    fn title_editor(&mut self, ui: &mut egui::Ui, gp: &Palette, id: &str, rect: Rect) {
        let Some((rid, mut text, focus)) = self.gal.renaming.take() else {
            return;
        };
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let edit = egui::TextEdit::singleline(&mut text)
            .id(Id::new(("rename-edit", id)))
            .font(FontId::proportional(14.0))
            .text_color(gp.text)
            .desired_width(rect.width())
            .frame(egui::Frame::NONE);
        let out = child.add(edit);
        if focus {
            out.request_focus();
        }
        if out.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            return;
        }
        if out.lost_focus() {
            self.commit_rename(&rid, &text);
            return;
        }
        self.gal.renaming = Some((rid, text, false));
    }

    /// Save a new name for an artwork or a stack.
    pub(super) fn commit_rename(&mut self, id: &str, title: &str) {
        let title = crate::storage::clean_title(title);
        if self.gal.index.is_stack(id) {
            self.mutate_index(|ix| ix.rename_stack(id, &title));
            return;
        }
        let Some(lib) = self.gal.library.clone() else {
            return;
        };
        match lib.rename(id, &title) {
            Ok(()) => self.gal.refresh(),
            Err(e) => self.show_toast(format!("Could not rename: {e}"), 3.0),
        }
    }

    /// Share, Duplicate and Delete over a swiped card.
    fn card_actions(&mut self, ui: &mut egui::Ui, gp: &Palette, entry: &DocEntry, thumb: Rect, actions: &mut Vec<Action>) {
        let reveal_secs = if self.reduce_motion() { 0.0 } else { 0.15 };
        let t = ui.ctx().animate_bool_with_time(Id::new(("reveal", &entry.id)), true, reveal_secs);
        let painter = ui.painter().clone();
        painter.rect_filled(thumb, CornerRadius::same(8), Color32::from_black_alpha((190.0 * t) as u8));
        let items: [(&str, icons::IconFn, Color32, Action); 3] = [
            ("Share", icon_share, gp.text, Action::Share(entry.id.clone())),
            ("Duplicate", icons::icon_layers, gp.text, Action::Duplicate(entry.id.clone())),
            ("Delete", icons::icon_trash, gp.danger, Action::AskDelete(vec![entry.id.clone()])),
        ];
        let slot = (thumb.width() / 3.0).min(84.0);
        let start = thumb.center().x - slot * 1.5;
        for (i, (label, icon, colour, action)) in items.into_iter().enumerate() {
            let cell = Rect::from_min_size(Pos2::new(start + slot * i as f32, thumb.center().y - 34.0), Vec2::new(slot, 68.0));
            let resp = ui.interact(cell, Id::new(("card-action", &entry.id, label)), Sense::click());
            crate::a11y::named(&resp, egui::WidgetType::Button, true, label);
            let disc = Rect::from_center_size(Pos2::new(cell.center().x, cell.top() + 22.0), Vec2::splat(42.0));
            let fill = if resp.hovered() { Color32::from_white_alpha(50) } else { Color32::from_white_alpha(28) };
            painter.circle_filled(disc.center(), 21.0, fill);
            icon(&painter, Rect::from_center_size(disc.center(), Vec2::splat(22.0)), colour.gamma_multiply(t));
            painter.text(
                Pos2::new(cell.center().x, cell.top() + 54.0),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(11.0),
                colour.gamma_multiply(t),
            );
            if resp.clicked() {
                actions.push(action);
            }
        }
    }

    /// Long-press drags: lift a card, follow the pointer, and drop it on release.
    pub(super) fn lift_frame(&mut self, ctx: &egui::Context) {
        let (down, released, pos, now) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.primary_released(), i.pointer.latest_pos(), i.time));
        if let Some((id, origin, since)) = self.gal.press.clone() {
            if !down || pos.is_some_and(|p| p.distance(origin) > LIFT_TRAVEL) {
                self.gal.press = None;
            } else if now - since >= LIFT_SECS {
                self.gal.press = None;
                self.gal.revealed = None;
                self.gal.lift_size = self.gal.card_rects.iter().find(|(c, _)| *c == id).map_or(Vec2::ZERO, |(_, r)| r.size());
                self.gal.lift = Some(id);
            } else {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
        }
        if self.gal.lift.is_none() {
            return;
        }
        if released {
            self.finish_lift(ctx, pos);
            return;
        }
        let Some(p) = pos else {
            return;
        };
        ctx.request_repaint();
        let Some(id) = self.gal.lift.clone() else {
            return;
        };
        let gp = theme::palette_for(self.prefs.identity, false);
        let layer = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("gallery-lift")));
        let size = Vec2::new(self.gal.lift_size.x, (self.gal.lift_size.y - CARD_TEXT_H).max(1.0));
        let r = Rect::from_center_size(p, size);
        layer.add(Shadow { offset: [0, 10], blur: 26, spread: 0, color: Color32::from_black_alpha(150) }.as_shape(r, CornerRadius::same(8)));
        match self.gal.thumbs.get(&id).and_then(|s| s.tex.as_ref().map(|t| (t.id(), s.size))) {
            Some((tid, sz)) => {
                let uv = crop_uv_4x3(sz.x, sz.y);
                layer.add(RectShape::filled(r, CornerRadius::same(8), Color32::WHITE).with_texture(tid, uv));
            }
            None => {
                layer.rect_filled(r, CornerRadius::same(8), gp.panel);
            }
        }
        if let Some((tid, rect)) = self.drop_target(&id, p) {
            let is_stack = self.gal.index.is_stack(&tid);
            match zone(rect, p) {
                Zone::Middle => {
                    layer.rect_stroke(rect.shrink(2.0), CornerRadius::same(10), Stroke::new(3.0, gp.accent), StrokeKind::Inside);
                }
                edge => {
                    let x = if edge == Zone::Before { rect.left() } else { rect.right() };
                    layer.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(4.0, gp.accent));
                }
            }
            let hint = match (zone(rect, p), is_stack) {
                (Zone::Middle, true) => "Add to stack",
                (Zone::Middle, false) => "Make a stack",
                _ => "Move here",
            };
            let text_at = Pos2::new(rect.center().x, rect.top() - 12.0);
            layer.text(text_at, Align2::CENTER_CENTER, hint, FontId::proportional(13.0), gp.text);
        }
    }

    /// The card under `p`, other than `id`, and the card's rectangle.
    fn drop_target(&self, id: &str, p: Pos2) -> Option<(String, Rect)> {
        self.gal.card_rects.iter().find(|(tid, r)| tid != id && r.contains(p)).cloned()
    }

    /// Finish a drag: act on the card under the pointer, or take an artwork out of its stack.
    fn finish_lift(&mut self, ctx: &egui::Context, pos: Option<Pos2>) {
        let Some(id) = self.gal.lift.take() else {
            return;
        };
        self.gal.drop_frame = ctx.cumulative_frame_nr();
        let Some(p) = pos else {
            return;
        };
        let target = self.drop_target(&id, p);
        let item = if self.gal.index.is_stack(&id) { Item::Stack(id.clone()) } else { Item::Doc(id.clone()) };
        if let Some(sid) = self.gal.open_stack.clone() {
            if !self.gal.body.contains(p) {
                let after = self.gal.index.next_root(&Item::Stack(sid));
                self.mutate_index(|ix| ix.unstack(&id, after.as_ref()));
                return;
            }
            if let Some((tid, rect)) = target {
                let z = zone(rect, p);
                if self.gal.index.sort == SortMode::Manual && z != Zone::Middle {
                    let before = if z == Zone::Before { Some(tid.clone()) } else { self.gal.index.next_in_stack(&tid) };
                    let sid = self.gal.open_stack.clone().unwrap_or_default();
                    self.mutate_index(|ix| ix.move_in_stack(&sid, &id, before.as_deref()));
                }
            }
            return;
        }
        let Some((tid, rect)) = target else {
            return;
        };
        let z = zone(rect, p);
        let target_is_stack = self.gal.index.is_stack(&tid);
        let target_item = if target_is_stack { Item::Stack(tid.clone()) } else { Item::Doc(tid.clone()) };
        let dragged_is_stack = matches!(item, Item::Stack(_));
        match z {
            Zone::Middle if target_is_stack => {
                if !dragged_is_stack {
                    self.mutate_index(|ix| ix.add_to_stack(&id, &tid));
                }
            }
            Zone::Middle => {
                if !dragged_is_stack && !target_is_stack {
                    let name = self.gal.index.next_stack_name();
                    let sid = new_id();
                    let docs = vec![id.clone(), tid.clone()];
                    self.mutate_index(|ix| ix.stack_docs(sid, name, &docs, &tid));
                }
            }
            edge => {
                if self.gal.index.sort == SortMode::Manual {
                    let before = if edge == Zone::Before { Some(target_item.clone()) } else { self.gal.index.next_root(&target_item) };
                    self.mutate_index(|ix| ix.move_before(&item, before.as_ref()));
                }
            }
        }
    }

    /// Apply a change to the library index: write it, then show it. Returns nothing when the
    /// change is a no-op.
    pub(super) fn mutate_index(&mut self, f: impl FnOnce(&mut crate::stacks::LibraryIndex) -> bool) {
        let mut next = self.gal.index.clone();
        if !f(&mut next) {
            return;
        }
        let res = self.gal.library.as_ref().map(|lib| lib.write_index(&next));
        if let Some(Err(e)) = res {
            self.show_toast(format!("Could not save the gallery order: {e}"), 3.0);
        }
        self.gal.index = next;
        if self.gal.open_stack.as_ref().is_some_and(|s| !self.gal.index.is_stack(s)) {
            self.gal.open_stack = None;
        }
    }

    /// Select mode's Stack button: the selected artworks become one stack.
    fn stack_selected(&mut self) {
        let ids: Vec<String> = self.tiles().iter().filter(|t| !t.stack && self.gal.selected.contains(&t.id)).map(|t| t.id.clone()).collect();
        if ids.len() < 2 {
            return;
        }
        let sid = new_id();
        let name = self.gal.index.next_stack_name();
        let anchor = ids[0].clone();
        self.mutate_index(|ix| ix.stack_docs(sid, name, &ids, &anchor));
        self.gal.select = false;
        self.gal.selected.clear();
    }

    /// The sort menu's rows. Returns the mode picked.
    pub(super) fn sort_body(&mut self, ui: &mut egui::Ui, gp: &Palette) -> Option<SortMode> {
        let mut choice = None;
        for mode in SortMode::ALL {
            let on = self.gal.index.sort == mode;
            let btn = egui::Button::new(RichText::new(mode.label()).color(gp.text)).selected(on).min_size(Vec2::new(ui.available_width(), 36.0));
            if ui.add(btn).clicked() {
                choice = Some(mode);
            }
        }
        choice
    }

    pub(super) fn set_sort(&mut self, mode: SortMode) {
        self.mutate_index(|ix| {
            if ix.sort == mode {
                false
            } else {
                ix.sort = mode;
                true
            }
        });
    }

    /// Delete a stack: its artworks go too, after a second confirmation that names the count.
    pub(super) fn stack_confirm_dialog(&mut self, ctx: &egui::Context, gp: &Palette) {
        let Some((sid, second)) = self.gal.stack_confirm.clone() else {
            return;
        };
        let Some(stack) = self.gal.index.stack(&sid).cloned() else {
            self.gal.stack_confirm = None;
            return;
        };
        let n = stack.docs.len();
        let screen = ctx.content_rect();
        let mut decision: Option<bool> = None;
        egui::Area::new(Id::new("gallery-stack-scrim"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(150));
                if resp.clicked() {
                    decision = Some(false);
                }
            });
        egui::Area::new(Id::new("gallery-stack-confirm"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                theme::panel_frame(gp).inner_margin(Margin::same(20)).show(ui, |ui| {
                    dark_widgets(ui, gp);
                    ui.set_width(300.0);
                    let (title, body, action) = if second {
                        (
                            format!("Delete {n} artworks?"),
                            "The stack and every artwork in it are removed. This cannot be undone.".to_owned(),
                            "Delete",
                        )
                    } else {
                        (
                            format!("Delete \"{}\"?", stack.name),
                            format!("This stack holds {n} artworks. Next, confirm that they go too."),
                            "Continue",
                        )
                    };
                    ui.label(RichText::new(title).size(17.0).strong().color(gp.text));
                    ui.add_space(4.0);
                    ui.label(RichText::new(body).size(13.0).color(gp.text_dim));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 8.0) / 2.0;
                        if ui.add_sized([w, 36.0], egui::Button::new("Cancel")).clicked() {
                            decision = Some(false);
                        }
                        let btn = egui::Button::new(RichText::new(action).color(Color32::WHITE)).fill(gp.danger);
                        if ui.add_sized([w, 36.0], btn).clicked() {
                            decision = Some(true);
                        }
                    });
                });
            });
        match decision {
            Some(true) if !second => self.gal.stack_confirm = Some((sid, true)),
            Some(true) => {
                self.gal.stack_confirm = None;
                self.gal.open_stack = None;
                self.delete_entries(&stack.docs);
            }
            Some(false) => self.gal.stack_confirm = None,
            None => {}
        }
    }

    pub(super) fn apply_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Background => {
                self.gal.revealed = None;
            }
            Action::Reveal(id) => {
                self.gal.menu = None;
                self.gal.revealed = id;
            }
            Action::Tap(id, thumb) => {
                if debug_input() {
                    eprintln!(
                        "[input f{}] Tap card {id} menu_open={} revealed={:?}",
                        ctx.cumulative_frame_nr(),
                        self.gal.menu.is_some(),
                        self.gal.revealed
                    );
                }
                let is_stack = self.gal.index.is_stack(&id);
                if self.gal.revealed.is_some() {
                    self.gal.revealed = None;
                } else if self.gal.menu.is_some() {
                    self.gal.menu = None;
                } else if self.gal.select {
                    if !is_stack && !self.gal.selected.remove(&id) {
                        self.gal.selected.insert(id);
                    }
                } else if is_stack {
                    self.gal.open_stack = Some(id);
                } else {
                    self.open_entry(&id, thumb);
                }
            }
            Action::Share(id) => {
                self.gal.revealed = None;
                // Ask for a folder; with no dialog available the item goes to the default folder.
                self.gal.share_target = Some(id);
                self.request_pick(crate::file_access::PickPurpose::GalleryShare, None, None);
            }
            Action::Duplicate(id) => {
                self.gal.revealed = None;
                match self.gal.library.as_ref().map(|l| l.duplicate(&id)) {
                    Some(Ok(copy)) => {
                        self.gal.refresh();
                        self.show_toast(format!("Created {}", copy.title), 2.0);
                    }
                    Some(Err(e)) => self.show_toast(format!("Could not duplicate: {e}"), 4.0),
                    None => {}
                }
            }
            Action::AskDelete(ids) => {
                self.gal.revealed = None;
                self.gal.confirm = Some(ids);
            }
        }
    }
}
