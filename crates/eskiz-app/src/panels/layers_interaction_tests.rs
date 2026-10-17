//! Interaction tests for the Layers panel: every gesture goes through the headless app, and
//! every edit is checked for its undo step, its name and its undo.

use eframe::egui::{self, Pos2, Vec2};

use super::layers_tests::{blank, centre, drag, ellipse, geo, line, menu_item, open, open_menu, order, toast};
use crate::document::Document;
use crate::panels::layers::Act;
use crate::popover::Panel;
use crate::snapshot::Headless;
use eskiz_core::blend::BlendMode;
use eskiz_core::document::{Node, Props};

/// `n` layers named L1.. with a coloured blob each (bottom to top), and their ids.
fn small(n: usize, w: usize, h: usize) -> (Document, Vec<u64>) {
    let mut d = Document::empty(w, h);
    let mut ids = Vec::new();
    for k in 0..n {
        let mut px = blank(w, h);
        let c = [[220, 60, 60, 255], [60, 160, 90, 255], [70, 90, 210, 255], [230, 190, 50, 255]][k % 4];
        ellipse(&mut px, w, h, w as f64 * (0.25 + 0.1 * (k % 5) as f64), h as f64 * 0.5, h as f64 * 0.25, h as f64 * 0.25, c);
        ids.push(d.push_layer(None, Props::new(format!("L{}", k + 1)), None, px));
    }
    (d, ids)
}

fn key(h: &mut Headless, k: egui::Key) {
    h.event(egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.step();
}

/// What undo must restore: the tree, every layer's pixels and the background.
fn state(h: &Headless) -> (Vec<Node>, Vec<Vec<u8>>, [f64; 3], bool) {
    let d = &h.app.session.doc;
    (
        d.root.clone(),
        d.layer_ids().iter().map(|&i| d.layer(i).unwrap().to_rgba()).collect(),
        d.background,
        d.background_visible,
    )
}

fn undo(h: &mut Headless) {
    h.app.undo_action();
    h.advance(0.1);
}

#[test]
fn add_button_adds_a_layer_above_the_selected_one() {
    let (doc, ids) = small(3, 1024, 768);
    let mut h = open(false, doc);
    h.app.active = ids[1];
    h.advance(0.1);
    let add = h.app.layers_ui.add_rect.unwrap();
    h.click(centre(add));
    h.advance(0.1);
    let now = order(&h);
    assert_eq!(now.len(), 4);
    let new = h.app.active;
    assert_eq!(now, vec![ids[0], ids[1], new, ids[2]]);
    assert_eq!(h.app.session.undo_name(), Some("Add Layer"));
    undo(&mut h);
    assert_eq!(order(&h), ids);
    assert_eq!(toast(&h), "Undo Add Layer");
    assert!(h.app.session.doc.node(h.app.active).is_some());
}

#[test]
fn dragging_a_card_reorders_with_one_undo_step() {
    let (doc, ids) = small(3, 1024, 768);
    let mut h = open(false, doc);
    let (a, c) = (ids[0], ids[2]);
    let before = state(&h);
    let steps = h.app.session.undo_steps();
    // Top card (c) down below the bottom card (a): the stack a, b, c becomes c, a, b.
    let from = centre(geo(&h, c).card);
    let ga = geo(&h, a).card;
    drag(&mut h, &line(from, Pos2::new(from.x, ga.bottom() - 4.0), 14));
    h.advance(0.1);
    assert_eq!(order(&h), vec![c, a, ids[1]]);
    assert_eq!(h.app.session.undo_steps(), steps + 1, "one undo step per reorder");
    assert_eq!(h.app.session.undo_name(), Some("Move Layer"));
    assert_eq!(h.app.active, c, "the dragged card is selected");
    undo(&mut h);
    assert_eq!(state(&h), before);
    assert_eq!(toast(&h), "Undo Move Layer");
}

#[test]
fn a_short_drag_or_a_drop_in_place_changes_nothing() {
    let (doc, ids) = small(3, 1024, 768);
    let mut h = open(false, doc);
    let steps = h.app.session.undo_steps();
    let from = centre(geo(&h, ids[1]).card);
    drag(&mut h, &line(from, from + Vec2::new(0.0, 3.0), 2));
    drag(&mut h, &line(from, from + Vec2::new(0.0, 14.0), 6));
    assert_eq!(order(&h), ids);
    assert_eq!(h.app.session.undo_steps(), steps);
}

#[test]
fn dropping_on_a_group_moves_into_it_and_dragging_out_leaves_it() {
    let mut d = Document::empty(256, 192);
    let a = d.push_layer(None, Props::new("A"), None, Vec::new());
    let g = d.push_group(None, Props::new("G"), None);
    let x = d.push_layer(Some(g), Props::new("X"), None, Vec::new());
    let b = d.push_layer(None, Props::new("B"), None, Vec::new());
    let mut h = open(false, d);
    // A onto the middle of the group card.
    let from = centre(geo(&h, a).card);
    let to = centre(geo(&h, g).card);
    drag(&mut h, &line(from, to, 12));
    h.advance(0.1);
    assert_eq!(h.app.session.doc.locate(a).unwrap().parent, Some(g), "A is in the group");
    assert_eq!(h.app.session.undo_name(), Some("Move Layer"));
    // X out of the group: above B.
    let from = centre(geo(&h, x).card);
    let to = Pos2::new(from.x, geo(&h, b).card.top() + 3.0);
    drag(&mut h, &line(from, to, 14));
    h.advance(0.1);
    assert_eq!(h.app.session.doc.locate(x).unwrap().parent, None);
    assert_eq!(h.app.session.doc.root.last().unwrap().id, x, "X is on top of the stack");
    undo(&mut h);
    assert_eq!(h.app.session.doc.locate(x).unwrap().parent, Some(g));
    undo(&mut h);
    assert_eq!(h.app.session.doc.locate(a).unwrap().parent, None);
}

fn button(h: &Headless, name: &str) -> Pos2 {
    centre(h.app.layers_ui.swipe_buttons.iter().find(|(l, _)| *l == name).map(|(_, r)| *r).expect("swipe button"))
}

fn swipe(h: &mut Headless, id: u64, dx: f32) {
    let from = centre(geo(h, id).card);
    drag(h, &line(from, from - Vec2::new(dx, 0.0), 8));
    h.advance(0.3);
}

#[test]
fn swiping_left_reveals_lock_duplicate_and_delete() {
    let (doc, ids) = small(3, 1024, 768);
    let mut h = open(false, doc);
    // Under 40 points: nothing.
    swipe(&mut h, ids[1], 30.0);
    assert_eq!(h.app.layers_ui.revealed, None);
    swipe(&mut h, ids[1], 90.0);
    assert_eq!(h.app.layers_ui.revealed, Some(ids[1]));
    let names: Vec<_> = h.app.layers_ui.swipe_buttons.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, vec!["Lock", "Duplicate", "Delete"]);

    // Lock.
    let p = button(&h, "Lock");
    h.click(p);
    h.advance(0.3);
    assert!(h.app.session.doc.node(ids[1]).unwrap().props.locked);
    assert_eq!(h.app.session.undo_name(), Some("Lock Layer"));
    assert_eq!(h.app.layers_ui.revealed, None);
    undo(&mut h);
    assert!(!h.app.session.doc.node(ids[1]).unwrap().props.locked);

    // Delete.
    let before = state(&h);
    swipe(&mut h, ids[1], 90.0);
    let p = button(&h, "Delete");
    h.click(p);
    h.advance(0.2);
    assert_eq!(order(&h), vec![ids[0], ids[2]]);
    assert_eq!(h.app.session.undo_name(), Some("Delete Layer"));
    assert!(h.app.session.doc.node(h.app.active).is_some(), "the selection moved to a neighbor");
    undo(&mut h);
    assert_eq!(state(&h), before);

    // Duplicate.
    swipe(&mut h, ids[1], 90.0);
    let p = button(&h, "Duplicate");
    h.click(p);
    h.advance(0.2);
    assert_eq!(order(&h).len(), 4);
    assert_eq!(h.app.session.undo_name(), Some("Duplicate Layer"));
    assert_eq!(h.app.session.doc.node(h.app.active).unwrap().props.name, "L2 copy");
    undo(&mut h);
    assert_eq!(state(&h), before);

    // Tapping another card closes the actions.
    swipe(&mut h, ids[1], 90.0);
    let g = geo(&h, ids[2]);
    h.click(Pos2::new(g.name.left() + 4.0, g.card.center().y));
    h.advance(0.3);
    assert_eq!(h.app.layers_ui.revealed, None);
}

#[test]
fn the_last_layer_cannot_be_deleted_from_the_swipe() {
    let (doc, ids) = small(1, 256, 192);
    let mut h = open(false, doc);
    swipe(&mut h, ids[0], 90.0);
    let p = button(&h, "Delete");
    h.click(p);
    h.advance(0.1);
    assert_eq!(order(&h), ids);
}

#[test]
fn the_checkbox_toggles_visibility() {
    let (doc, ids) = small(3, 1024, 768);
    let mut h = open(false, doc);
    let before = state(&h);
    let flat = h.app.session.flatten();
    h.click(centre(geo(&h, ids[2]).check));
    h.advance(0.1);
    assert!(!h.app.session.doc.node(ids[2]).unwrap().props.visible);
    assert_ne!(h.app.session.flatten(), flat, "hiding a layer changes the picture");
    assert_eq!(h.app.session.undo_name(), Some("Hide Layer"));
    h.click(centre(geo(&h, ids[2]).check));
    assert!(h.app.session.doc.node(ids[2]).unwrap().props.visible);
    undo(&mut h);
    undo(&mut h);
    assert_eq!(state(&h), before);
}

#[test]
fn the_blend_letter_opens_opacity_and_blend_controls() {
    let (doc, ids) = small(3, 1024, 768);
    let mut h = open(false, doc);
    let id = ids[1];
    let before = state(&h);
    let steps = h.app.session.undo_steps();
    h.click(centre(geo(&h, id).letter));
    h.advance(0.2);
    assert_eq!(h.app.layers_ui.controls, Some(id));
    assert_eq!(h.app.active, id, "opening the controls selects the layer");

    // Blend: pick Multiply from the list.
    let cell = h.app.layers_ui.blend_cells.iter().find(|(m, _)| *m == BlendMode::Multiply).map(|(_, r)| *r).unwrap();
    h.click(centre(cell));
    h.advance(0.1);
    assert_eq!(h.app.session.doc.node(id).unwrap().props.blend, BlendMode::Multiply);
    assert_eq!(h.app.session.undo_name(), Some("Layer Blend Mode"));

    // Opacity: one drag across the slider is one undo step.
    let s = h.app.layers_ui.slider.unwrap();
    let (y, x0, x1) = (s.center().y, s.left() + 10.0, s.right() - 10.0);
    let mid = Pos2::new((x0 + x1) / 2.0, y);
    drag(&mut h, &line(Pos2::new(x1, y), mid, 10));
    h.advance(0.1);
    let o = h.app.session.doc.node(id).unwrap().props.opacity;
    assert!((o - 0.5).abs() < 0.06, "opacity {o}");
    assert_eq!(h.app.session.undo_steps(), steps + 2, "blend and one opacity drag");
    assert_eq!(h.app.session.undo_name(), Some("Layer Opacity"));
    undo(&mut h);
    undo(&mut h);
    assert_eq!(state(&h), before);
    // The letter closes the controls again.
    h.click(centre(geo(&h, id).letter));
    assert_eq!(h.app.layers_ui.controls, None);
}

#[test]
fn rename_through_the_thumbnail_menu() {
    let (doc, ids) = small(3, 1024, 768);
    let mut h = open(false, doc);
    let id = ids[1];
    // The menu only opens from the selected layer's thumbnail; on another layer it selects.
    h.click(centre(geo(&h, id).thumb));
    assert_eq!(h.app.active, id);
    assert_eq!(h.app.layers_ui.menu, None);
    h.click(centre(geo(&h, id).thumb));
    assert_eq!(h.app.layers_ui.menu, Some(id));
    h.advance(0.1);
    menu_item(&mut h, Act::Rename);
    h.advance(0.1);
    assert!(h.app.layers_ui.rename.is_some());
    h.event(egui::Event::Text("Sky".to_owned()));
    h.step();
    key(&mut h, egui::Key::Enter);
    h.advance(0.1);
    assert_eq!(h.app.session.doc.node(id).unwrap().props.name, "Sky");
    assert_eq!(h.app.session.undo_name(), Some("Rename Layer"));
    assert!(h.app.layers_ui.rename.is_none());
    undo(&mut h);
    assert_eq!(h.app.session.doc.node(id).unwrap().props.name, "L2");

    // Escape cancels, and does not close the panel.
    open_menu(&mut h, id);
    menu_item(&mut h, Act::Rename);
    h.advance(0.1);
    h.event(egui::Event::Text("Nope".to_owned()));
    h.step();
    key(&mut h, egui::Key::Escape);
    h.advance(0.1);
    assert_eq!(h.app.session.doc.node(id).unwrap().props.name, "L2");
    assert_eq!(h.app.popovers.open_panel(), Some(Panel::Layers));
}

#[test]
fn collapsing_a_group_hides_its_children() {
    let mut d = Document::empty(256, 192);
    let g = d.push_group(None, Props::new("G"), None);
    let x = d.push_layer(Some(g), Props::new("X"), None, Vec::new());
    let y = d.push_layer(Some(g), Props::new("Y"), None, Vec::new());
    let mut h = open(false, d);
    assert!(h.app.layers_ui.geo(x).is_some() && h.app.layers_ui.geo(y).is_some());
    let steps = h.app.session.undo_steps();
    let chev = geo(&h, g).chevron.unwrap();
    h.click(centre(chev));
    h.advance(0.1);
    assert!(h.app.layers_ui.geo(x).is_none() && h.app.layers_ui.geo(g).is_some());
    h.click(centre(geo(&h, g).chevron.unwrap()));
    h.advance(0.1);
    assert!(h.app.layers_ui.geo(x).is_some());
    assert_eq!(h.app.session.undo_steps(), steps, "collapsing is panel state, not an edit");
}

#[test]
fn background_colour_swatch_picker_and_checkbox() {
    let (doc, _) = small(2, 512, 384);
    let mut h = open(false, doc);
    let before = state(&h);
    let steps = h.app.session.undo_steps();
    // Visibility.
    h.click(centre(h.app.layers_ui.bg_check.unwrap()));
    h.advance(0.1);
    assert!(!h.app.session.doc.background_visible);
    assert_eq!(h.app.session.undo_name(), Some("Hide Background"));
    h.click(centre(h.app.layers_ui.bg_check.unwrap()));
    assert!(h.app.session.doc.background_visible);
    // The picker: a preset is one step.
    h.click(centre(h.app.layers_ui.bg_swatch.unwrap()));
    h.advance(0.2);
    assert!(h.app.layers_ui.picker);
    let pr = h.app.layers_ui.picker_rect.unwrap();
    let (sv, _, presets) = crate::panels::layer_picker::parts(pr);
    let cream = Pos2::new(presets.left() + presets.width() / 8.0 * 1.5, presets.center().y);
    h.click(cream);
    h.advance(0.1);
    assert_eq!(h.app.session.doc.background.map(|v| (v * 255.0).round() as u8), [245, 239, 224]);
    assert_eq!(h.app.session.undo_steps(), steps + 3);
    assert_eq!(h.app.session.undo_name(), Some("Background Color"));
    // Dragging in the square is one step however long it takes.
    let s2 = h.app.session.undo_steps();
    drag(&mut h, &line(sv.center(), sv.center() + Vec2::new(40.0, 30.0), 10));
    h.advance(0.1);
    assert_eq!(h.app.session.undo_steps(), s2 + 1);
    assert_ne!(h.app.session.doc.background.map(|v| (v * 255.0).round() as u8), [245, 239, 224]);
    for _ in 0..4 {
        undo(&mut h);
    }
    assert_eq!(state(&h), before);
    assert_eq!(toast(&h), "Undo Hide Background");
    // A press outside the picker (but inside the panel) closes it.
    h.click(Pos2::new(960.0, 92.0));
    assert!(!h.app.layers_ui.picker);
}

#[test]
fn menu_actions_are_named_undoable_steps() {
    let (doc, ids) = small(3, 512, 384);
    let mut h = open(false, doc);
    h.app.colour = [10, 200, 30];
    let top = ids[2];
    let mid = ids[1];

    type Case = (&'static str, u64, Act, &'static str);
    let cases: [Case; 9] = [
        ("fill", top, Act::Fill, "Fill Layer"),
        ("clear", top, Act::Clear, "Clear Layer"),
        ("lock transparency", top, Act::AlphaLock, "Lock Transparency"),
        ("add mask", top, Act::Mask, "Add Mask"),
        ("clipping", top, Act::Clipping, "Clip to Layer Below"),
        ("merge down", top, Act::MergeDown, "Merge Down"),
        ("combine down", top, Act::CombineDown, "Combine Down"),
        ("group", mid, Act::Group, "Group"),
        ("duplicate", mid, Act::Duplicate, "Duplicate Layer"),
    ];
    for (what, id, act, name) in cases {
        let before = state(&h);
        let steps = h.app.session.undo_steps();
        open_menu(&mut h, id);
        menu_item(&mut h, act);
        assert_eq!(h.app.session.undo_steps(), steps + 1, "{what}: one step");
        assert_eq!(h.app.session.undo_name(), Some(name), "{what}");
        assert_ne!(state(&h), before, "{what} changed something");
        assert!(h.app.layers_ui.menu.is_none(), "{what}: the menu closes");
        undo(&mut h);
        assert_eq!(state(&h), before, "{what}: undo");
        assert_eq!(toast(&h), format!("Undo {name}"));
    }
    // Delete.
    let before = state(&h);
    open_menu(&mut h, top);
    menu_item(&mut h, Act::Delete);
    assert_eq!(order(&h).len(), 2);
    undo(&mut h);
    assert_eq!(state(&h), before);
    // The fill used the current colour and covers the layer.
    open_menu(&mut h, top);
    menu_item(&mut h, Act::Fill);
    let px = h.app.session.doc.layer(top).unwrap().pixel(3, 3);
    assert_eq!(px, [10, 200, 30, 255]);
    // Placeholders are listed but do nothing.
    let steps = h.app.session.undo_steps();
    open_menu(&mut h, top);
    for act in [Act::Select, Act::Copy, Act::Invert, Act::Reference] {
        let cell = h.app.layers_ui.menu_cells.iter().find(|(a, _)| *a == act).map(|(_, r)| *r).unwrap();
        h.click(centre(cell));
    }
    assert_eq!(h.app.session.undo_steps(), steps);
    assert!(h.app.layers_ui.menu.is_some(), "disabled items leave the menu open");
}

#[test]
fn group_menu_offers_ungroup_and_mask() {
    let mut d = Document::empty(128, 96);
    let g = d.push_group(None, Props::new("G"), None);
    let x = d.push_layer(Some(g), Props::new("X"), None, vec![255; 128 * 96 * 4]);
    d.push_layer(None, Props::new("Y"), None, Vec::new());
    let mut h = open(false, d);
    open_menu(&mut h, g);
    assert!(h.app.layers_ui.menu_cells.iter().any(|(a, _)| *a == Act::Ungroup));
    assert!(h.app.layers_ui.menu_cells.iter().all(|(a, _)| *a != Act::Fill && *a != Act::MergeDown));
    menu_item(&mut h, Act::Ungroup);
    assert_eq!(h.app.session.undo_name(), Some("Ungroup"));
    assert_eq!(h.app.session.doc.locate(x).unwrap().parent, None);
    assert_eq!(h.app.active, x);
    undo(&mut h);
    assert!(h.app.session.doc.node(g).is_some());
}

#[test]
fn thumbnails_are_cached_and_refreshed_only_for_changed_layers() {
    let (doc, ids) = small(3, 2048, 1536);
    let mut h = open(false, doc);
    let n = h.app.layers_ui.thumbs.renders;
    assert_eq!(n, 3, "one thumbnail per layer");
    h.advance(1.0);
    assert_eq!(h.app.layers_ui.thumbs.renders, n, "idle frames draw nothing");

    // Property edits do not touch pixels.
    h.app.session.set_opacity(ids[0], 0.5);
    h.app.session.set_name(ids[1], "x");
    h.advance(0.3);
    assert_eq!(h.app.layers_ui.thumbs.renders, n);

    // A stroke on one layer redraws that thumbnail only.
    let brush = eskiz_core::brushes::Brush::preset(eskiz_core::brushes::Preset::Pencil);
    let st = crate::paint::StrokeSettings {
        brush,
        colour: [0.0, 0.0, 0.0],
        erase: false,
        raster: None,
    };
    assert!(h.app.session.begin_stroke(ids[2], &st));
    for i in 0..30 {
        h.app.session.push(crate::input::sample_from(crate::input::RawPointer {
            x: 100.0 + f64::from(i) * 10.0,
            y: 700.0,
            time: f64::from(i) / 120.0,
            kind: crate::input::PointerKind::Mouse,
            force: None,
        }));
    }
    h.app.session.end_stroke();
    h.advance(0.3);
    assert_eq!(h.app.layers_ui.thumbs.renders, n + 1);
    // Undo redraws it again, and so does restoring a deleted layer.
    h.app.undo_action();
    h.advance(0.3);
    assert_eq!(h.app.layers_ui.thumbs.renders, n + 2);
    h.app.session.delete_layer(ids[0]);
    h.advance(0.2);
    h.app.undo_action();
    h.advance(0.3);
    assert_eq!(h.app.layers_ui.thumbs.renders, n + 3);
}

#[test]
fn many_layers_scroll_and_the_panel_height_is_capped() {
    let (doc, ids) = small(40, 256, 192);
    let mut h = open(false, doc);
    let top = ids[39];
    let g0 = geo(&h, top).card;
    // The panel never grows past 600 points.
    let panel_bottom = h.app.layers_ui.bg_swatch.unwrap().bottom();
    assert!(panel_bottom < 60.0 + 600.0 + 24.0, "bottom {panel_bottom}");
    assert!(geo(&h, ids[0]).card.top() > panel_bottom, "the lowest card is out of view");
    h.move_to(g0.center());
    h.step();
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, -300.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.advance(0.3);
    assert!(geo(&h, top).card.top() < g0.top() - 100.0, "the list scrolled");
    // Few layers: the panel follows its content.
    let (doc, _) = small(2, 256, 192);
    let h2 = open(false, doc);
    assert!(h2.app.layers_ui.bg_swatch.unwrap().bottom() < panel_bottom - 150.0);
}

#[test]
fn writes_short_list_snapshots() {
    for light in [false, true] {
        let (doc, ids) = small(3, 2048, 1536);
        let mut h = open(light, doc);
        super::layers_tests::snap(&mut h, "short", light);
        // The menu on a short list makes room for itself.
        open_menu(&mut h, ids[2]);
        h.advance(0.3);
        super::layers_tests::snap(&mut h, "short_menu", light);
        let cell = h.app.layers_ui.menu_cells.last().map(|(_, r)| *r).unwrap();
        assert!(h.app.popovers.contains(centre(cell)), "the menu stays inside the panel");
    }
}
