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
/// Desktop (pointer) row heights.
pub const ROW_H_COMPACT: f32 = 34.0;
pub const ROW_H_TALL_COMPACT: f32 = 42.0;

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
    let compact = is_compact(ui.ctx());
    // Metrics: full-size touch targets on touch devices, tighter rows for a pointer.
    let (row_h, row_h_tall, icon_size, icon_x, text_x_off, label_pt, sub_pt, sw_w, sw_h) = if compact {
        (ROW_H_COMPACT, ROW_H_TALL_COMPACT, 20.0, 20.0, 40.0, 13.0, 10.5, 34.0, 20.0)
    } else {
        (ROW_H, ROW_H_TALL, 24.0, 26.0, 50.0, 14.0, 11.0, 40.0, 24.0)
    };
    // How much of the row's right end the trailing element takes.
    let reserve = match &trailing {
        Trailing::None => 14.0,
        Trailing::Chevron => 14.0 + 14.0,
        Trailing::Switch(_) => 14.0 + sw_w + 8.0,
        Trailing::Text(t) => {
            let w = ui.painter().layout_no_wrap((*t).to_owned(), FontId::proportional(12.0), pal.text_dim).size().x;
            14.0 + w + 10.0
        }
    };
    let width = ui.available_width();
    let max_w = (width - text_x_off - reserve).max(20.0);
    // The second line wraps (at most two lines) instead of being cut off.
    let sub_galley = sub.map(|s| {
        let mut job = egui::text::LayoutJob::simple(s.to_owned(), FontId::proportional(sub_pt), pal.text_dim, max_w);
        job.wrap.max_rows = 2;
        ui.painter().layout_job(job)
    });
    let extra = sub_galley.as_ref().map_or(0.0, |g| if g.rows.len() > 1 { sub_pt + 2.0 } else { 0.0 });
    let height = if sub.is_some() { row_h_tall + extra } else { row_h };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), sense);
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
    icon(&painter, Rect::from_center_size(Pos2::new(rect.left() + icon_x, rect.center().y), vec2(icon_size, icon_size)), colour);

    let mut right = rect.right() - 14.0;
    match trailing {
        Trailing::None => {}
        Trailing::Chevron => {
            let c = Pos2::new(right - 4.0, rect.center().y);
            let line = Stroke::new(1.75, pal.text_dim);
            painter.line_segment([c + vec2(-3.0, -6.0), c + vec2(3.0, 0.0)], line);
            painter.line_segment([c + vec2(3.0, 0.0), c + vec2(-3.0, 6.0)], line);
        }
        Trailing::Switch(on) => {
            switch(&painter, pal, Rect::from_min_size(Pos2::new(right - sw_w, rect.center().y - sw_h / 2.0), vec2(sw_w, sw_h)), on, enabled);
        }
        Trailing::Text(t) => {
            let galley = painter.layout_no_wrap(t.to_owned(), FontId::proportional(12.0), pal.text_dim);
            let w = galley.size().x;
            painter.galley(Pos2::new(right - w, rect.center().y - galley.size().y / 2.0), galley, pal.text_dim);
            right -= w;
        }
    }
    let _ = right;
    let text_x = rect.left() + text_x_off;
    let label_font = FontId::proportional(label_pt);
    let label_line = fit_text(&painter, label, &label_font, max_w);
    match sub_galley {
        Some(g) => {
            let block = label_pt + 4.0 + g.size().y;
            let top = rect.center().y - block / 2.0;
            painter.text(Pos2::new(text_x, top), Align2::LEFT_TOP, label_line, label_font, colour);
            painter.galley(Pos2::new(text_x, top + label_pt + 4.0), g, pal.text_dim);
        }
        None => {
            painter.text(Pos2::new(text_x, rect.center().y), Align2::LEFT_CENTER, label_line, label_font, colour);
        }
    }
    crate::a11y::focus_ring(ui.painter(), rect, &resp, pal, 10);
    resp
}

/// Context flag: a touch event has been seen, so panels keep full-size touch targets.
const TOUCH_SEEN: &str = "eskiz_touch_seen";

/// Record that the user touches the screen (called when a touch event arrives).
pub fn note_touch(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(Id::new(TOUCH_SEEN), true));
}

/// Whether rows are drawn compact: a desktop with pointer input and no touch seen yet.
pub fn is_compact(ctx: &egui::Context) -> bool {
    !cfg!(target_os = "android") && !ctx.data(|d| d.get_temp::<bool>(Id::new(TOUCH_SEEN)).unwrap_or(false))
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
    let track = if on { pal.accent_fill } else { pal.separator };
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
        let colour = if active { pal.primary_text } else if resp.hovered() { pal.text } else { pal.text_dim };
        if active {
            painter.rect_filled(cell_rect.shrink2(vec2(4.0, 4.0)), CornerRadius::same(8), pal.selected_bg(0.16));
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
