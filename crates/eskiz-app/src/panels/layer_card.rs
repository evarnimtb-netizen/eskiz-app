//! The look of one layer card and the drop-target maths of
//! drag-to-reorder. Everything here is pure drawing or pure arithmetic over the document, so
//! the interaction code in `layers.rs` stays small.

use eframe::egui::{
    epaint::RectShape, text::LayoutJob, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke,
    StrokeKind, TextureId, Vec2,
};

use crate::icons;
use crate::panels::layer_thumbs::{MASK_BOX, THUMB_BOX};
use crate::theme::Palette;
use eskiz_core::blend::BlendMode;
use eskiz_core::document::{Document, Node, NodeKind};

/// Height of a card, points.
pub const CARD_H: f32 = 56.0;
/// Vertical gap between cards.
pub const GAP: f32 = 6.0;
/// Extra left indent per level of group nesting.
pub const INDENT: f32 = 18.0;
/// Extra left indent of a clipped layer; the hook arrow sits in it.
pub const CLIP_INDENT: f32 = 20.0;
/// Width of the three swipe actions together.
pub const ACTIONS_W: f32 = 176.0;
const RADIUS: u8 = 10;

/// One row of the flattened layer tree, top of the stack first.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: u64,
    pub depth: usize,
    pub name: String,
    pub group: bool,
    pub collapsed: bool,
    /// Direct children (groups only).
    pub children: usize,
    pub visible: bool,
    /// Visible and in no hidden group.
    pub shown: bool,
    pub opacity: f64,
    pub blend: BlendMode,
    pub clipped: bool,
    pub alpha_lock: bool,
    pub locked: bool,
    pub has_mask: bool,
    pub parent: Option<u64>,
    /// Index among the siblings (bottom is 0).
    pub index: usize,
}

/// The visible rows of the tree: top of the stack first, children of collapsed groups left out.
pub fn rows(doc: &Document) -> Vec<Row> {
    fn walk(items: &[Node], parent: Option<u64>, depth: usize, parent_shown: bool, out: &mut Vec<Row>) {
        for (index, n) in items.iter().enumerate().rev() {
            let (group, collapsed) = match &n.kind {
                NodeKind::Group { collapsed, .. } => (true, *collapsed),
                NodeKind::Layer => (false, false),
            };
            out.push(Row {
                id: n.id,
                depth,
                name: n.props.name.clone(),
                group,
                collapsed,
                children: n.children().len(),
                visible: n.props.visible,
                shown: n.props.visible && parent_shown,
                opacity: n.props.opacity,
                blend: n.props.blend,
                clipped: n.props.clipped && !group,
                alpha_lock: n.props.alpha_lock && !group,
                locked: n.props.locked,
                has_mask: n.mask.is_some(),
                parent,
                index,
            });
            if group && !collapsed {
                walk(n.children(), Some(n.id), depth + 1, n.props.visible && parent_shown, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(&doc.root, None, 0, true, &mut out);
    out
}

/// The letter shown on the card for a blend mode: one letter where that is clear, two
/// otherwise. All 26 are different.
pub fn blend_letter(mode: BlendMode) -> &'static str {
    use BlendMode::*;
    match mode {
        Normal => "N",
        Multiply => "M",
        Screen => "S",
        Add => "A",
        Lighten => "L",
        Exclusion => "E",
        Difference => "D",
        Subtract => "Su",
        LinearBurn => "LB",
        ColorDodge => "CD",
        ColorBurn => "CB",
        Overlay => "O",
        HardLight => "HL",
        Color => "C",
        Luminosity => "Lu",
        Hue => "H",
        Saturation => "Sa",
        SoftLight => "SL",
        Darken => "Dk",
        HardMix => "HM",
        VividLight => "VL",
        LinearLight => "LL",
        PinLight => "PL",
        LighterColor => "LC",
        DarkerColor => "DC",
        Divide => "Dv",
    }
}

/// The small second line of a card: blend mode and opacity when they are not the defaults.
pub fn subtitle(row: &Row) -> String {
    let mut parts = Vec::new();
    if row.group {
        parts.push(match row.children {
            1 => "1 item".to_owned(),
            n => format!("{n} items"),
        });
    }
    if row.blend != BlendMode::Normal {
        parts.push(row.blend.name().to_owned());
    }
    if row.opacity < 0.995 {
        parts.push(format!("{:.0}%", row.opacity * 100.0));
    }
    parts.join(" \u{b7} ")
}

/// Where the parts of a card are, for drawing and for hit tests.
#[derive(Clone, Copy, Debug)]
pub struct CardGeo {
    pub card: Rect,
    pub chevron: Option<Rect>,
    pub thumb: Rect,
    pub mask: Option<Rect>,
    /// Room for the name and the second line.
    pub name: Rect,
    pub lock: Option<Rect>,
    pub letter: Rect,
    pub check: Rect,
}

pub fn geo(card: Rect, row: &Row) -> CardGeo {
    let cy = card.center().y;
    let mut x = card.left() + 6.0;
    let chevron = row.group.then(|| {
        let r = Rect::from_min_size(Pos2::new(x - 2.0, cy - 12.0), Vec2::new(20.0, 24.0));
        x += 20.0;
        r
    });
    let thumb = Rect::from_min_size(Pos2::new(x, cy - THUMB_BOX.y / 2.0), THUMB_BOX);
    x = thumb.right() + 8.0;
    let mask = row.has_mask.then(|| {
        let r = Rect::from_min_size(Pos2::new(x, cy - MASK_BOX.y / 2.0), MASK_BOX);
        x = r.right() + 8.0;
        r
    });
    let check = Rect::from_center_size(Pos2::new(card.right() - 8.0 - 13.0, cy), Vec2::splat(26.0));
    let letter = Rect::from_center_size(Pos2::new(check.left() - 6.0 - 16.0, cy), Vec2::new(32.0, 26.0));
    let mut right = letter.left() - 6.0;
    let lock = row.locked.then(|| {
        let r = Rect::from_center_size(Pos2::new(right - 8.0, cy), Vec2::splat(16.0));
        right = r.left() - 4.0;
        r
    });
    let name = Rect::from_min_max(Pos2::new(x, card.top()), Pos2::new(right.max(x + 20.0), card.bottom()));
    CardGeo {
        card,
        chevron,
        thumb,
        mask,
        name,
        lock,
        letter,
        check,
    }
}

/// A single-row text layout that ends in an ellipsis instead of wrapping.
pub fn one_line(text: &str, size: f32, colour: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), colour);
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('\u{2026}');
    job
}

/// What the card shows besides the row's own data.
pub struct CardPaint {
    pub selected: bool,
    /// The blend/opacity controls are open under this card.
    pub open: bool,
    /// Fades the content (the card being dragged leaves a faint copy behind).
    pub alpha: f32,
    pub thumb: Option<(TextureId, Vec2)>,
    pub mask: Option<(TextureId, Vec2)>,
    /// Draw the name; off while the rename field covers it.
    pub show_name: bool,
}

/// `c` at straight alpha `a` (for white this equals `Color32::from_white_alpha(a)`).
fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// A filled rounded square with a tick: the visibility checkbox.
pub fn draw_checkbox(p: &Painter, pal: &Palette, rect: Rect, on: bool, on_accent: bool) {
    let b = Rect::from_center_size(rect.center(), Vec2::splat(22.0));
    let r = CornerRadius::same(7);
    if on {
        let (fill, tick) = if on_accent { (pal.on_accent, pal.accent_fill) } else { (pal.accent_fill, pal.on_accent) };
        p.rect_filled(b, r, fill);
        icons::icon_check(p, b.shrink(2.0), tick);
    } else {
        let line = if on_accent { with_alpha(pal.on_accent, 190) } else { pal.text_dim };
        p.rect_stroke(b, r, Stroke::new(1.5, line), StrokeKind::Inside);
    }
}

/// The alpha-lock badge: a tiny checkerboard in the corner of the thumbnail.
fn alpha_badge(p: &Painter, corner: Pos2) {
    let b = Rect::from_min_size(corner - Vec2::splat(15.0), Vec2::splat(13.0));
    p.rect_filled(b.expand(1.5), CornerRadius::same(4), Color32::from_black_alpha(170));
    let cell = b.width() / 3.0;
    for j in 0..3 {
        for i in 0..3 {
            let c = if (i + j) % 2 == 0 { Color32::WHITE } else { Color32::from_gray(120) };
            p.rect_filled(
                Rect::from_min_size(b.min + Vec2::new(i as f32 * cell, j as f32 * cell), Vec2::splat(cell)),
                CornerRadius::ZERO,
                c,
            );
        }
    }
}

/// Draw one card into `rect` and return where its parts are.
pub fn paint_card(p: &Painter, pal: &Palette, rect: Rect, row: &Row, st: &CardPaint) -> CardGeo {
    let g = geo(rect, row);
    let a = st.alpha;
    let content = if row.shown { a } else { a * 0.45 };
    let (fill, fg, fg2) = if st.selected {
        (pal.accent_fill, pal.on_accent, with_alpha(pal.on_accent, 200))
    } else {
        (pal.row, pal.text, pal.text_dim)
    };
    let (fg, fg2) = (fg.gamma_multiply(content), fg2.gamma_multiply(content));
    p.rect_filled(rect, CornerRadius::same(RADIUS), fill.gamma_multiply(a));
    if !st.selected {
        p.rect_stroke(rect, CornerRadius::same(RADIUS), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
    }

    if row.clipped {
        let hook = Rect::from_center_size(Pos2::new(rect.left() - CLIP_INDENT / 2.0 - 1.0, rect.center().y - 2.0), Vec2::splat(18.0));
        icons::icon_clip_hook(p, hook, pal.text_dim.gamma_multiply(a));
    }
    if let Some(c) = g.chevron {
        let icon = Rect::from_center_size(c.center(), Vec2::splat(16.0));
        if row.collapsed {
            icons::icon_chevron_right(p, icon, fg2);
        } else {
            icons::icon_chevron_down(p, icon, fg2);
        }
    }

    // Thumbnail.
    let tr = CornerRadius::same(5);
    if row.group {
        p.rect_filled(g.thumb, tr, Color32::from_black_alpha(if st.selected { 50 } else { 38 }).gamma_multiply(a));
        icons::icon_folder(p, Rect::from_center_size(g.thumb.center(), Vec2::splat(28.0)), fg);
        p.rect_stroke(g.thumb, tr, Stroke::new(1.0, Color32::from_white_alpha(18)), StrokeKind::Inside);
    } else {
        p.rect_filled(g.thumb, tr, Color32::from_black_alpha(if st.selected { 50 } else { 38 }).gamma_multiply(a));
        if let Some((tex, size)) = st.thumb {
            let img = Rect::from_center_size(g.thumb.center(), size);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            p.add(RectShape::filled(img, tr, Color32::WHITE.gamma_multiply(content)).with_texture(tex, uv));
            let edge = if st.selected { Color32::from_white_alpha(230) } else { Color32::from_black_alpha(90) };
            p.rect_stroke(img, tr, Stroke::new(1.0, edge.gamma_multiply(a)), StrokeKind::Inside);
            if row.alpha_lock {
                alpha_badge(p, img.right_bottom());
            }
        }
    }
    if let (Some(m), Some((tex, size))) = (g.mask, st.mask) {
        let img = Rect::from_center_size(m.center(), size);
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        p.add(RectShape::filled(img, CornerRadius::same(4), Color32::WHITE.gamma_multiply(content)).with_texture(tex, uv));
        let edge = if st.selected { Color32::from_white_alpha(200) } else { Color32::from_white_alpha(70) };
        p.rect_stroke(img, CornerRadius::same(4), Stroke::new(1.0, edge.gamma_multiply(a)), StrokeKind::Inside);
    } else if let Some(m) = g.mask {
        p.rect_stroke(m, CornerRadius::same(4), Stroke::new(1.0, fg2), StrokeKind::Inside);
    }

    // Name and second line.
    if st.show_name {
        let sub = subtitle(row);
        let w = g.name.width();
        let name_y = if sub.is_empty() { g.name.center().y - 8.0 } else { g.name.center().y - 15.0 };
        let galley = p.layout_job(one_line(&row.name, 13.0, fg, w));
        p.galley(Pos2::new(g.name.left(), name_y), galley, fg);
        if !sub.is_empty() {
            let galley = p.layout_job(one_line(&sub, 11.0, fg2, w));
            p.galley(Pos2::new(g.name.left(), g.name.center().y + 2.0), galley, fg2);
        }
    }

    if let Some(l) = g.lock {
        icons::icon_lock(p, l, fg2);
    }

    // Blend letter chip.
    let chip = Rect::from_center_size(g.letter.center(), Vec2::new(g.letter.width(), 24.0));
    let cr = CornerRadius::same(7);
    if st.open {
        let (f, t) = if st.selected { (pal.on_accent, pal.accent_fill) } else { (pal.accent_fill, pal.on_accent) };
        p.rect_filled(chip, cr, f);
        p.text(chip.center(), Align2::CENTER_CENTER, blend_letter(row.blend), FontId::proportional(12.0), t);
    } else {
        p.rect_stroke(chip, cr, Stroke::new(1.2, fg2), StrokeKind::Inside);
        p.text(chip.center(), Align2::CENTER_CENTER, blend_letter(row.blend), FontId::proportional(12.0), fg);
    }

    draw_checkbox(p, pal, g.check, row.visible, st.selected);
    g
}

/// How a dragged card would land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropTarget {
    pub parent: Option<u64>,
    /// Index among the destination's items with the dragged one taken out (what
    /// `Session::move_item` expects).
    pub index: usize,
    /// Dropping here changes nothing.
    pub noop: bool,
    pub marker: Marker,
}

/// What to draw for a drop target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Marker {
    /// A line across the gap at `y`, indented to `depth`.
    Line { y: f32, depth: usize },
    /// Highlight the group card at this row.
    Into { row: usize },
}

enum Slot {
    Above(usize),
    Below(usize),
    Into(usize),
}

/// Where dragging `dragged` with the pointer at `p` would drop. `cards` and `extents` are the
/// rectangles of each row's card and of the card plus anything open under it; `left` is the
/// list's left edge (for the nesting depth of the pointer). `None` when nothing is a target.
pub fn drop_target(doc: &Document, rows: &[Row], cards: &[Rect], extents: &[Rect], dragged: u64, p: Pos2, left: f32) -> Option<DropTarget> {
    let dloc = doc.locate(dragged)?;
    let cand: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].id != dragged && !doc.is_ancestor(dragged, rows[i].id))
        .collect();
    let (&first, &last) = (cand.first()?, cand.last()?);

    let mut slot = None;
    for &i in &cand {
        let (card, ext) = (cards[i], extents[i]);
        if p.y >= ext.top() - GAP / 2.0 && p.y <= ext.bottom() + GAP / 2.0 {
            let f = ((p.y - card.top()) / card.height()).clamp(0.0, 1.0);
            let r = &rows[i];
            slot = Some(if r.group && !r.collapsed && r.children > 0 {
                if f < 0.3 { Slot::Above(i) } else { Slot::Into(i) }
            } else if r.group {
                if f < 0.25 {
                    Slot::Above(i)
                } else if f > 0.75 {
                    Slot::Below(i)
                } else {
                    Slot::Into(i)
                }
            } else if f < 0.5 {
                Slot::Above(i)
            } else {
                Slot::Below(i)
            });
            break;
        }
    }
    let slot = match slot {
        Some(s) => s,
        None => {
            // Over the dragged card itself (or its children): it stays where it is.
            let own = (0..rows.len())
                .filter(|i| !cand.contains(i))
                .any(|i| p.y >= extents[i].top() - GAP / 2.0 && p.y <= extents[i].bottom() + GAP / 2.0);
            if own {
                return Some(DropTarget {
                    parent: dloc.parent,
                    index: dloc.index,
                    noop: true,
                    marker: Marker::Line { y: p.y, depth: 0 },
                });
            }
            if p.y < cards[first].top() {
                Slot::Above(first)
            } else {
                Slot::Below(last)
            }
        }
    };

    let (parent, mut index, marker) = match slot {
        Slot::Above(i) => {
            let r = &rows[i];
            (r.parent, r.index + 1, Marker::Line { y: cards[i].top() - GAP / 2.0, depth: r.depth })
        }
        Slot::Below(i) => {
            // Left of the nesting level: leave the group the row is the last child of.
            let mut cur = i;
            while rows[cur].index == 0 && rows[cur].parent.is_some() && p.x < left + rows[cur].depth as f32 * INDENT - 8.0 {
                match rows.iter().position(|r| Some(r.id) == rows[cur].parent) {
                    Some(up) => cur = up,
                    None => break,
                }
            }
            let r = &rows[cur];
            (r.parent, r.index, Marker::Line { y: extents[i].bottom() + GAP / 2.0, depth: r.depth })
        }
        Slot::Into(i) => {
            let len = doc.children(Some(rows[i].id))?.len();
            (Some(rows[i].id), len, Marker::Into { row: i })
        }
    };
    if dloc.parent == parent && dloc.index < index {
        index -= 1;
    }
    let noop = dloc.parent == parent && dloc.index == index;
    Some(DropTarget { parent, index, noop, marker })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eskiz_core::document::Props;

    #[test]
    fn every_blend_mode_has_its_own_letter() {
        let mut seen = std::collections::HashSet::new();
        for m in BlendMode::ALL {
            let l = blend_letter(m);
            assert!(!l.is_empty() && l.len() <= 2, "{m:?}");
            assert!(seen.insert(l), "{l} is used twice");
        }
        assert_eq!(blend_letter(BlendMode::Normal), "N");
        assert_eq!(blend_letter(BlendMode::Multiply), "M");
        assert_eq!(blend_letter(BlendMode::Screen), "S");
    }

    /// root: a, G(b, c), d  (bottom to top)
    fn doc() -> (Document, [u64; 5]) {
        let mut d = Document::empty(8, 8);
        let a = d.push_layer(None, Props::new("a"), None, Vec::new());
        let g = d.push_group(None, Props::new("G"), None);
        let b = d.push_layer(Some(g), Props::new("b"), None, Vec::new());
        let c = d.push_layer(Some(g), Props::new("c"), None, Vec::new());
        let dd = d.push_layer(None, Props::new("d"), None, Vec::new());
        (d, [a, g, b, c, dd])
    }

    /// Cards stacked from y = 0, one per row, 62 apart.
    fn layout(rows: &[Row]) -> Vec<Rect> {
        (0..rows.len())
            .map(|i| Rect::from_min_size(Pos2::new(rows[i].depth as f32 * INDENT, i as f32 * (CARD_H + GAP)), Vec2::new(296.0, CARD_H)))
            .collect()
    }

    #[test]
    fn rows_are_top_first_and_skip_collapsed_children() {
        let (mut d, [a, g, b, c, dd]) = doc();
        let r = rows(&d);
        assert_eq!(r.iter().map(|r| r.id).collect::<Vec<_>>(), vec![dd, g, c, b, a]);
        assert_eq!(r[2].depth, 1);
        if let Some(Node { kind: NodeKind::Group { collapsed, .. }, .. }) = d.node_mut(g) {
            *collapsed = true;
        }
        assert_eq!(rows(&d).iter().map(|r| r.id).collect::<Vec<_>>(), vec![dd, g, a]);
    }

    #[test]
    fn drop_above_below_and_into() {
        let (d, [a, g, b, c, dd]) = doc();
        let r = rows(&d); // d, G, c, b, a
        let cards = layout(&r);
        let p = |row: usize, f: f32| Pos2::new(100.0, cards[row].top() + f * CARD_H);

        // Drag `a` (bottom) above the top card: top of the root.
        let t = drop_target(&d, &r, &cards, &cards, a, p(0, 0.2), 0.0).unwrap();
        assert_eq!((t.parent, t.index, t.noop), (None, 2, false));
        // Middle of the expanded group: into it, at the top.
        let t = drop_target(&d, &r, &cards, &cards, a, p(1, 0.6), 0.0).unwrap();
        assert_eq!((t.parent, t.index), (Some(g), 2));
        assert!(matches!(t.marker, Marker::Into { row: 1 }));
        // Between c and b inside the group.
        let t = drop_target(&d, &r, &cards, &cards, a, p(2, 0.8), 0.0).unwrap();
        assert_eq!((t.parent, t.index), (Some(g), 1));
        // Dropping `d` where it already is changes nothing.
        let t = drop_target(&d, &r, &cards, &cards, dd, p(0, 0.3), 0.0).unwrap();
        assert!(t.noop);
        // Below `b` (the group's last child) with the pointer left of its indent: out of the group.
        let t = drop_target(&d, &r, &cards, &cards, c, Pos2::new(2.0, cards[3].bottom() - 4.0), 0.0).unwrap();
        assert_eq!((t.parent, t.index), (None, 1));
        // A group cannot go into itself.
        let t = drop_target(&d, &r, &cards, &cards, g, p(2, 0.5), 0.0).unwrap();
        assert_ne!(t.parent, Some(g));
        let _ = (b, c);
    }

    #[test]
    fn same_parent_indices_count_the_list_without_the_dragged_item() {
        let (d, [a, _g, _b, _c, dd]) = doc();
        let r = rows(&d);
        let cards = layout(&r);
        // `d` dropped below `a` (bottom): index 0.
        let t = drop_target(&d, &r, &cards, &cards, dd, Pos2::new(100.0, cards[4].bottom() - 2.0), 0.0).unwrap();
        assert_eq!((t.parent, t.index), (None, 0));
        // `a` dropped above `d`: it ends on top. Root holds a, G, d: without `a` there are 2.
        let t = drop_target(&d, &r, &cards, &cards, a, Pos2::new(100.0, cards[0].top() + 2.0), 0.0).unwrap();
        assert_eq!((t.parent, t.index), (None, 2));
    }
}
