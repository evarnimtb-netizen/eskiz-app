//! The `--selftest` check for the Layers panel: the whole app runs headless, the panel is
//! driven with pointer events (add, drag to reorder, swipe to delete, visibility), and every
//! edit has to be one named undo step that undo takes back exactly.

use eframe::egui::{Pos2, Rect, Vec2};

use crate::app::Prefs;
use crate::document::Document;
use crate::popover::Panel;
use crate::snapshot::Headless;
use eskiz_core::document::Props;

fn drag(h: &mut Headless, from: Pos2, to: Pos2) {
    h.move_to(from);
    h.step();
    h.press(from, true);
    h.step();
    for i in 1..=10 {
        h.move_to(from + (to - from) * (i as f32 / 10.0));
        h.step();
    }
    h.step();
    h.press(to, false);
    h.step();
    h.step();
}

fn card(h: &Headless, id: u64) -> Result<Rect, String> {
    h.app.layers_ui.geo(id).map(|g| g.card).ok_or_else(|| format!("layers panel: card {id} not drawn"))
}

/// Drive the Layers panel and return the report line.
pub fn check() -> Result<String, String> {
    let mut doc = Document::empty(640, 480);
    let mut ids = Vec::new();
    for k in 0..3u8 {
        let mut px = vec![0u8; 640 * 480 * 4];
        for p in px.as_chunks_mut::<4>().0.iter_mut().take(640 * 100) {
            p.copy_from_slice(&[40 * (k + 1), 90, 200, 255]);
        }
        ids.push(doc.push_layer(None, Props::new(format!("L{}", k + 1)), None, px));
    }
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), doc, Prefs::default());
    h.open_panel(Panel::Layers);
    h.advance(0.3);
    let order = |h: &Headless| h.app.session.doc.layer_ids();
    let start = order(&h);

    // Thumbnails: one per layer, none redrawn while idle.
    let drawn = h.app.layers_ui.thumbs.renders;
    h.advance(0.5);
    if drawn != 3 || h.app.layers_ui.thumbs.renders != drawn {
        return Err(format!("layers panel: {drawn} thumbnails drawn, expected 3 and none while idle"));
    }

    // Add.
    let add = h.app.layers_ui.add_rect.ok_or("layers panel: no add button")?;
    h.click(add.center());
    h.advance(0.1);
    if order(&h).len() != 4 || h.app.session.undo_name() != Some("Add Layer") {
        return Err("layers panel: the + button did not add one layer as one step".into());
    }
    h.app.undo_action();
    h.advance(0.1);
    if order(&h) != start {
        return Err("layers panel: undo of Add Layer did not restore the stack".into());
    }

    // Reorder by dragging the top card below the bottom one.
    let from = card(&h, ids[2])?.center();
    let to = Pos2::new(from.x, card(&h, ids[0])?.bottom() - 4.0);
    let steps = h.app.session.undo_steps();
    drag(&mut h, from, to);
    h.advance(0.1);
    if order(&h) != vec![ids[2], ids[0], ids[1]] || h.app.session.undo_steps() != steps + 1 {
        return Err(format!("layers panel: drag gave {:?} in {} steps", order(&h), h.app.session.undo_steps() - steps));
    }
    h.app.undo_action();
    h.advance(0.1);
    if order(&h) != start {
        return Err("layers panel: undo of the reorder did not restore the stack".into());
    }

    // Swipe the middle card left and delete it.
    let from = card(&h, ids[1])?.center();
    drag(&mut h, from, from - Vec2::new(90.0, 0.0));
    h.advance(0.3);
    let delete = h
        .app
        .layers_ui
        .swipe_buttons
        .iter()
        .find(|(n, _)| *n == "Delete")
        .map(|(_, r)| r.center())
        .ok_or("layers panel: swiping did not reveal the actions")?;
    h.click(delete);
    h.advance(0.2);
    if order(&h) != vec![ids[0], ids[2]] || h.app.session.undo_name() != Some("Delete Layer") {
        return Err("layers panel: swipe delete did not remove the layer as one step".into());
    }
    h.app.undo_action();
    h.advance(0.1);
    if order(&h) != start {
        return Err("layers panel: undo of the delete did not restore the layer".into());
    }

    // Visibility checkbox.
    let check = h.app.layers_ui.geo(ids[2]).map(|g| g.check.center()).ok_or("layers panel: no checkbox")?;
    h.click(check);
    h.advance(0.1);
    let hidden = !h.app.session.doc.node(ids[2]).is_some_and(|n| n.props.visible);
    h.app.undo_action();
    if !hidden || !h.app.session.doc.node(ids[2]).is_some_and(|n| n.props.visible) {
        return Err("layers panel: the checkbox did not hide the layer (and undo show it)".into());
    }
    Ok("  layers panel: add, drag reorder, swipe delete and visibility are one named undo step each; thumbnails cached".to_owned())
}
