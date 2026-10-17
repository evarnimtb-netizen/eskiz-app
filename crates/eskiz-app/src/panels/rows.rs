//! Shared widgets for list-style panels (Actions, Adjustments): the tab strip, the icon row and
//! the toggle switch. Rows are rounded cards with a 24-point icon, a label and an optional
//! second line, like the cards in the Layers panel.

use eframe::egui::{self, vec2, Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Response, Sense, Stroke, Ui};

use crate::icons::IconFn;
use crate::theme::Palette;

/// Height of a row with one line of text.
pub const ROW_H: f32 = 46.0;
/// Height of a row with a second line.
pub const ROW_H_TALL: f32 = 54.0;

/// What sits at the right end of a row.
pub enum Trailing<'a> {
    None,
    Chevron,
    /// A switch showing this state. The caller flips its setting when the row is clicked.
    Switch(bool),
    /// A short dim value ("Soon", "2048 x 1536").
    Text(&'a str),
}

/// Colour of a label or icon on a row.
fn ink(pal: &Palette, enabled: bool) -> egui::Color32 {
    if enabled {
        pal.text
    } else {
        pal.disabled()
    }
}

/// A row: icon, label, optional dim second line, and a trailing element. Disabled rows are
/// greyed and do not react. Returns the row's response (`clicked()` is the action).
pub fn list_row(
    ui: &mut Ui,
    pal: &Palette,
    icon: IconFn,
    label: &str,
    sub: Option<&str>,
    enabled: bool,
    trailing: Trailing,
) -> Response {
    let height = if sub.is_some() { ROW_H_TALL } else { ROW_H };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), sense);
    let name = match sub {
        Some(s) => format!("{label}, {s}"),
        None => label.to_owned(),
    };
    match &trailing {
        Trailing::Switch(on) => crate::a11y::toggle(&resp, egui::WidgetType::Checkbox, enabled, *on, name),
        _ => crate::a11y::named(&resp, egui::WidgetType::Button, enabled, name),
    }
    let painter = ui.painter_at(rect);
    let fill = if enabled && resp.hovered() { pal.separator } else { pal.row };
    painter.rect_filled(rect, CornerRadius::same(10), fill);
    let colour = ink(pal, enabled);
    icon(&painter, Rect::from_center_size(Pos2::new(rect.left() + 26.0, rect.center().y), vec2(24.0, 24.0)), colour);

    // Right end first, so the text knows how much room it has.
    let mut right = rect.right() - 14.0;
    match trailing {
        Trailing::None => {}
        Trailing::Chevron => {
            let c = Pos2::new(right - 4.0, rect.center().y);
            let line = Stroke::new(1.75, pal.text_dim);
            painter.line_segment([c + vec2(-3.0, -6.0), c + vec2(3.0, 0.0)], line);
            painter.line_segment([c + vec2(3.0, 0.0), c + vec2(-3.0, 6.0)], line);
            right -= 14.0;
        }
        Trailing::Switch(on) => {
            switch(&painter, pal, Rect::from_min_size(Pos2::new(right - 40.0, rect.center().y - 12.0), vec2(40.0, 24.0)), on, enabled);
            right -= 48.0;
        }
        Trailing::Text(t) => {
            let galley = painter.layout_no_wrap(t.to_owned(), FontId::proportional(12.0), pal.text_dim);
            let w = galley.size().x;
            painter.galley(Pos2::new(right - w, rect.center().y - galley.size().y / 2.0), galley, pal.text_dim);
            right -= w + 10.0;
        }
    }
    let text_x = rect.left() + 50.0;
    let max_w = (right - text_x).max(20.0);
    match sub {
        Some(s) => {
            painter.text(Pos2::new(text_x, rect.center().y - 9.0), Align2::LEFT_CENTER, label, FontId::proportional(14.0), colour);
            let line = fit_text(&painter, s, &FontId::proportional(11.0), max_w);
            painter.text(Pos2::new(text_x, rect.center().y + 10.0), Align2::LEFT_CENTER, line, FontId::proportional(11.0), pal.text_dim);
        }
        None => {
            let line = fit_text(&painter, label, &FontId::proportional(14.0), max_w);
            painter.text(Pos2::new(text_x, rect.center().y), Align2::LEFT_CENTER, line, FontId::proportional(14.0), colour);
        }
    }
    crate::a11y::focus_ring(ui.painter(), rect, &resp, pal, 10);
    resp
}

/// Shorten `text` with an ellipsis until it measures no wider than `max_w`: from the left for
/// file paths (the end is what matters), from the right for anything else.
fn fit_text(painter: &egui::Painter, text: &str, font: &FontId, max_w: f32) -> String {
    let width = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_owned();
    }
    let from_left = text.contains('/') || text.contains('\\');
    let chars: Vec<char> = text.chars().collect();
    let build = |keep: usize| -> String {
        if from_left {
            format!("\u{2026}{}", chars[chars.len() - keep..].iter().collect::<String>())
        } else {
            format!("{}\u{2026}", chars[..keep].iter().collect::<String>())
        }
    };
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width(&build(mid)) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    build(lo)
}

/// A pill switch: accent track and a right-hand knob when on.
pub fn switch(painter: &egui::Painter, pal: &Palette, rect: Rect, on: bool, enabled: bool) {
    let track = if on { pal.accent } else { pal.separator };
    let track = if enabled { track } else { track.gamma_multiply(0.5) };
    painter.rect_filled(rect, CornerRadius::same(12), track);
    let x = if on { rect.right() - 12.0 } else { rect.left() + 12.0 };
    painter.circle_filled(Pos2::new(x, rect.center().y), 9.0, egui::Color32::WHITE);
}

/// A strip of icon tabs with a label under each icon. Returns the tab that was clicked.
pub fn tab_bar(ui: &mut Ui, pal: &Palette, tabs: &[(IconFn, &str)], selected: usize) -> Option<usize> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 52.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(10), pal.row);
    let cell = width / tabs.len() as f32;
    let mut clicked = None;
    for (i, (icon, label)) in tabs.iter().enumerate() {
        let cell_rect = Rect::from_min_size(Pos2::new(rect.left() + cell * i as f32, rect.top()), vec2(cell, rect.height()));
        let resp = ui.interact(cell_rect, Id::new(("tab", label)), Sense::click());
        crate::a11y::toggle(&resp, egui::WidgetType::SelectableLabel, true, i == selected, *label);
        if resp.clicked() {
            clicked = Some(i);
        }
        let active = i == selected;
        let colour = if active { pal.accent } else if resp.hovered() { pal.text } else { pal.text_dim };
        if active {
            painter.rect_filled(cell_rect.shrink2(vec2(4.0, 4.0)), CornerRadius::same(8), pal.accent.gamma_multiply(0.16));
        }
        icon(&painter, Rect::from_center_size(Pos2::new(cell_rect.center().x, cell_rect.top() + 18.0), vec2(22.0, 22.0)), colour);
        painter.text(
            Pos2::new(cell_rect.center().x, cell_rect.top() + 39.0),
            Align2::CENTER_CENTER,
            *label,
            FontId::proportional(10.5),
            colour,
        );
        crate::a11y::focus_ring(ui.painter(), cell_rect, &resp, pal, 8);
    }
    clicked
}

/// A dim, wrapped hint under a group of rows.
pub fn hint(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(11.5).color(pal.text_dim)).wrap());
}
