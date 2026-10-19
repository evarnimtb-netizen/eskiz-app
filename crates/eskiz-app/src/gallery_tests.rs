//! Headless UI tests for the Gallery: snapshots (written to `target/snapshots`) and flows.

use super::*;
use crate::app::Prefs;
use crate::snapshot::{snapshot_dir, Headless};
use crate::storage;
use eskiz_core::BlendMode;
use std::fs;

const WINDOW: Vec2 = Vec2::new(1280.0, 800.0);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-gallery-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

use crate::onboarding_sample::scene as art;

/// A library of `n` demo documents. Art is drawn small; the metadata reports full sizes.
fn library_with(name: &str, n: usize) -> (Library, PathBuf) {
    let dir = temp(name);
    let lib = Library::at(&dir);
    let sizes = [(2048, 1536), (1600, 1600), (3840, 2160), (1200, 1800), (2480, 3508)];
    let titles = ["Harbour at dusk", "Study in green", "Night sky", "Untitled 3", "A very long title that does not fit on one line"];
    for i in 0..n {
        let (w, h) = sizes[i % 5];
        let id = storage::new_id();
        lib.save(&id, titles[i % 5], &art(w / 8, h / 8, i)).unwrap();
        // Stagger modification times so the newest-first order is stable.
        std::thread::sleep(Duration::from_millis(20));
    }
    for (i, e) in lib.list().iter().rev().enumerate() {
        let (w, h) = sizes[i % 5];
        fs::write(dir.join(format!("{}.meta", e.id)), format!("{w}\n{h}\n{}\n", e.title)).unwrap();
    }
    (lib, dir)
}

fn gallery_app(lib: &Library, light: bool) -> Headless {
    let lib = lib.clone();
    let mut h = Headless::from_app(WINDOW, move |ctx| {
        EskizApp::with_library(ctx, Prefs { light, ..Prefs::default() }, lib)
    });
    h.settle_storage();
    h.advance(0.3);
    h
}

fn save_png(h: &mut Headless, name: &str) {
    h.render().save_png(&snapshot_dir().join(name)).unwrap();
}

fn swipe_left(h: &mut Headless, from: Pos2, dx: f32) {
    h.move_to(from);
    h.step();
    h.press(from, true);
    h.step();
    for i in 1..=6 {
        h.move_to(from - Vec2::new(dx * i as f32 / 6.0, 0.0));
        h.step();
    }
    h.press(from - Vec2::new(dx, 0.0), false);
    h.step();
    h.step();
}

#[test]
fn writes_gallery_snapshots() {
    for light in [false, true] {
        let theme = if light { "light" } else { "dark" };

        // Empty.
        let (empty, dir0) = library_with(&format!("empty-{theme}"), 0);
        let mut h = gallery_app(&empty, light);
        save_png(&mut h, &format!("gallery_empty_{theme}.png"));

        // Five documents.
        let (lib, dir) = library_with(&format!("five-{theme}"), 5);
        let mut h = gallery_app(&lib, light);
        assert_eq!(h.app.gal.entries.len(), 5);
        save_png(&mut h, &format!("gallery_5_{theme}.png"));

        // Select mode with two cards chosen.
        h.click(h.app.gal.header.select.center());
        h.advance(0.2);
        let cards = h.app.gal.card_rects.clone();
        h.click(cards[0].1.center());
        h.click(cards[2].1.center());
        assert_eq!(h.app.gal.selected.len(), 2);
        save_png(&mut h, &format!("gallery_select_{theme}.png"));
        h.click(h.app.gal.header.select.center());
        h.advance(0.2);
        assert!(h.app.gal.selected.is_empty());

        // New canvas popover.
        h.click(h.app.gal.header.plus.center());
        h.advance(0.4);
        assert!(h.app.gal.menu.is_some());
        save_png(&mut h, &format!("gallery_new_canvas_{theme}.png"));
        h.click(Pos2::new(16.0, 400.0));
        h.advance(0.3);
        assert!(h.app.gal.menu.is_none());

        // Import popover.
        h.click(h.app.gal.header.import.center());
        h.advance(0.4);
        save_png(&mut h, &format!("gallery_import_{theme}.png"));
        h.click(Pos2::new(16.0, 400.0));
        h.advance(0.3);

        // Swipe left on a card: the actions appear.
        let card = h.app.gal.card_rects[1].1;
        swipe_left(&mut h, card.center(), 90.0);
        h.advance(0.3);
        assert_eq!(h.app.gal.revealed.as_deref(), Some(h.app.gal.entries[1].id.as_str()));
        save_png(&mut h, &format!("gallery_swipe_{theme}.png"));

        // Delete asks first.
        let card = h.app.gal.card_rects[1].1;
        let slot = (card.width() / 3.0).min(84.0);
        h.click(Pos2::new(card.center().x + slot, card.top() + card.width() * 0.375));
        h.advance(0.3);
        assert!(h.app.gal.confirm.is_some(), "delete must be confirmed");
        save_png(&mut h, &format!("gallery_confirm_{theme}.png"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir0);
    }
}

#[test]
fn short_drags_do_not_reveal_actions() {
    let (lib, dir) = library_with("short-swipe", 3);
    let mut h = gallery_app(&lib, false);
    let c = h.app.gal.card_rects[0].1.center();
    swipe_left(&mut h, c, 25.0);
    assert!(h.app.gal.revealed.is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn card_opens_with_the_zoom_and_leaving_saves_and_updates_the_thumbnail() {
    let (lib, dir) = library_with("open", 2);
    let mut h = gallery_app(&lib, false);
    let first = h.app.gal.entries[0].clone();
    let old_thumb = lib.thumbnail(&first.id).unwrap();
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    assert!(matches!(h.app.gal.screen, Screen::Opening(_)));
    h.advance(0.1);
    assert!(matches!(h.app.gal.screen, Screen::Opening(_)), "the zoom lasts about 250 ms");
    save_png(&mut h, "gallery_zoom_mid.png");
    h.settle_storage();
    h.advance(0.4);
    assert!(matches!(h.app.gal.screen, Screen::Canvas));
    assert!(!h.app.is_dirty(), "a freshly opened document is clean");

    // Fill the layer with a flat colour as a stand-in for painting.
    {
        let layer = h.app.session.doc.layer_at_mut(0).unwrap();
        let (w, hh) = (layer.width, layer.height);
        layer.write_rect(0, 0, w, hh, &[200, 30, 30, 255].repeat(w * hh));
    }
    h.app.session.mark_edited();
    assert!(h.app.is_dirty());

    // Back to the gallery: saved, thumbnail updated.
    h.click(h.app.gallery_button.center());
    assert!(matches!(h.app.gal.screen, Screen::Gallery), "the Gallery button leaves the canvas");
    h.settle_storage();
    let new_thumb = lib.thumbnail(&first.id).unwrap();
    assert_ne!(old_thumb.rgba, new_thumb.rgba);
    assert_eq!(&new_thumb.rgba[..4], &[200, 30, 30, 255]);
    assert!(!h.app.is_dirty());
    assert_eq!(&lib.load(&first.id).unwrap().layer_at(0).unwrap().pixel(0, 0), &[200, 30, 30, 255]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn new_canvas_then_gallery_then_restart_shows_the_document() {
    let dir = temp("restart");
    let lib = Library::at(&dir);
    {
        let mut h = gallery_app(&lib, false);
        assert!(h.app.gal.entries.is_empty());
        h.click(h.app.gal.header.plus.center());
        h.advance(0.4);
        // "Square 2048" is the second preset row of the popover.
        let plus = h.app.gal.header.plus;
        let rows_top = plus.bottom() + 12.0 + 12.0 + 17.0 + 6.0 + 20.0;
        h.click(Pos2::new(plus.center().x - 100.0, rows_top + 44.0 + 22.0));
        h.advance(0.5);
        h.settle_storage();
        assert!(matches!(h.app.gal.screen, Screen::Canvas), "the canvas opens after the zoom");
        assert_eq!(h.app.session.doc.width, 2048);
        assert_eq!(h.app.session.doc.height, 2048);
        h.click(h.app.gallery_button.center());
        h.settle_storage();
        assert_eq!(h.app.gal.entries.len(), 1);
        assert_eq!(h.app.gal.entries[0].title, "Untitled");
        h.app.finish_storage();
    }
    // A new app on the same folder: the document is there (acceptance 7).
    let mut h = gallery_app(&lib, false);
    assert_eq!(h.app.gal.entries.len(), 1);
    assert_eq!((h.app.gal.entries[0].width, h.app.gal.entries[0].height), (2048, 2048));
    save_png(&mut h, "gallery_after_restart.png");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn autosave_writes_a_dirty_document_after_the_interval_and_ignores_a_clean_one() {
    let (lib, dir) = library_with("autosave", 1);
    let mut h = gallery_app(&lib, false);
    h.app.gal.autosave_secs = 2.0;
    let id = h.app.gal.entries[0].id.clone();
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    h.advance(0.4);
    h.settle_storage();
    assert!(matches!(h.app.gal.screen, Screen::Canvas));
    let stamp = lib.get(&id).unwrap().modified;

    // Clean: nothing is written, however long we wait.
    h.advance(5.0);
    h.settle_storage();
    assert_eq!(lib.get(&id).unwrap().modified, stamp);

    // Dirty: written once the interval has passed, and not before.
    h.app.session.doc.layer_at_mut(0).unwrap().set_pixel(0, 0, [1, 2, 3, 255]);
    h.app.session.mark_edited();
    h.advance(1.0);
    h.settle_storage();
    assert_eq!(lib.get(&id).unwrap().modified, stamp, "not yet due");
    h.advance(2.0);
    h.settle_storage();
    assert_ne!(lib.get(&id).unwrap().modified, stamp, "autosaved");
    assert!(!h.app.is_dirty());
    assert_eq!(&lib.load(&id).unwrap().layer_at(0).unwrap().pixel(0, 0), &[1, 2, 3, 255]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn layer_property_changes_count_as_unsaved() {
    let (lib, dir) = library_with("props", 1);
    let mut h = gallery_app(&lib, false);
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    h.advance(0.4);
    h.settle_storage();
    assert!(!h.app.is_dirty());
    h.app.session.doc.props_at_mut(0).unwrap().blend = BlendMode::Multiply;
    assert!(h.app.is_dirty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn importing_while_on_the_gallery_adds_the_document() {
    let (lib, dir) = library_with("drop", 0);
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(dir.join("elsewhere")).unwrap();
    let src = dir.join("elsewhere").join("outside.procreate");
    eskiz_format::document::write(&src, &storage::to_builder(&art(160, 120, 1), "Dropped")).unwrap();
    let mut h = gallery_app(&lib, false);
    let ctx = h.ctx.clone();
    h.app.import_into_gallery(&ctx, vec![src]);
    h.settle_storage();
    assert_eq!(h.app.gal.entries.len(), 1);
    assert_eq!(h.app.gal.entries[0].title, "Dropped");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn multi_delete_asks_then_removes() {
    let (lib, dir) = library_with("multidelete", 4);
    let mut h = gallery_app(&lib, false);
    h.click(h.app.gal.header.select.center());
    h.advance(0.2);
    let cards = h.app.gal.card_rects.clone();
    h.click(cards[0].1.center());
    h.click(cards[3].1.center());
    // "Delete (2)" sits left of "Done".
    let done = h.app.gal.header.select;
    h.click(Pos2::new(done.left() - 60.0, done.center().y));
    h.advance(0.2);
    assert_eq!(h.app.gal.confirm.as_ref().map(Vec::len), Some(2));
    assert_eq!(lib.list().len(), 4, "nothing is deleted before the confirmation");
    // The dialog's Delete button is on the right half of its button row.
    let centre = Pos2::new(WINDOW.x / 2.0, WINDOW.y / 2.0);
    h.click(centre + Vec2::new(75.0, 52.0));
    h.advance(0.2);
    assert_eq!(lib.list().len(), 2);
    assert_eq!(h.app.gal.entries.len(), 2);
    let _ = fs::remove_dir_all(&dir);
}

/// Each preset row in the "New canvas" popover creates its canvas on the click, and the popover
/// closes with it. Only the custom "Create" button did this before.
#[test]
fn preset_rows_create_their_canvas() {
    let (lib, dir) = library_with("presets", 1);
    let mut h = gallery_app(&lib, false);
    // The window is WINDOW points at scale 1, so "Screen size" is that many pixels.
    let expected = size_presets((WINDOW.x as usize, WINDOW.y as usize));
    for (i, (name, w, hh)) in expected.into_iter().enumerate() {
        h.click(h.app.gal.header.plus.center());
        h.advance(0.4);
        assert!(h.app.gal.menu.is_some(), "the popover opens for {name}");
        let row = h.app.gal.preset_rects.get(i).copied().expect("a row per preset");
        h.click(row.center());
        // The click closes the popover and starts the zoom to a new canvas of the row's size.
        // The zoom is not run to its end: at 4K and A4 the headless renderer's texture limit
        // (2048 px) is reached, so the request is what is checked.
        assert!(h.app.gal.menu.is_none(), "{name} closes the popover");
        match &h.app.gal.screen {
            Screen::Opening(anim) => assert_eq!(anim.size, (w, hh), "{name} asks for a canvas of its size"),
            _ => panic!("{name} did not start a new canvas"),
        }
        h.app.gal.screen = Screen::Gallery;
    }
    let _ = fs::remove_dir_all(&dir);
}

/// the New canvas popover offers sRGB and Display P3, and the new document gets the
/// chosen space. sRGB is the default.
#[test]
fn new_canvas_popover_chooses_the_colour_space() {
    use eskiz_core::ColourSpace;
    let (lib, dir) = library_with("colour-space", 1);
    let mut h = gallery_app(&lib, false);
    for (choice, want) in [(None, ColourSpace::Srgb), (Some(1usize), ColourSpace::DisplayP3), (Some(0), ColourSpace::Srgb)] {
        h.click(h.app.gal.header.plus.center());
        h.advance(0.4);
        assert_eq!(h.app.gal.space_rects.len(), 2, "one button per color space");
        if let Some(i) = choice {
            h.click(h.app.gal.space_rects[i].center());
            h.advance(0.1);
            if want == ColourSpace::DisplayP3 {
                save_png(&mut h, "gallery_new_canvas_p3.png");
            }
        }
        assert_eq!(h.app.gal.new_space, want);
        let row = h.app.gal.preset_rects[0];
        h.click(row.center());
        match &h.app.gal.screen {
            Screen::Opening(anim) => {
                let doc = anim.doc.as_ref().expect("a blank canvas");
                assert_eq!(doc.colour_space, want);
            }
            _ => panic!("the preset did not start a new canvas"),
        }
        h.app.gal.screen = Screen::Gallery;
    }
    let _ = fs::remove_dir_all(&dir);
}

/// The New canvas popover offers 8-bit and 16-bit canvases, and the choice reaches the document.
#[test]
fn new_canvas_popover_chooses_the_colour_depth() {
    use eskiz_core::Depth;
    let (lib, dir) = library_with("colour-depth", 1);
    let mut h = gallery_app(&lib, false);
    for (choice, want) in [(None, Depth::U8), (Some(1usize), Depth::U16), (Some(0), Depth::U8)] {
        h.click(h.app.gal.header.plus.center());
        h.advance(0.4);
        assert_eq!(h.app.gal.depth_rects.len(), 2, "one button per depth");
        if let Some(i) = choice {
            h.click(h.app.gal.depth_rects[i].center());
            h.advance(0.1);
            if want == Depth::U16 {
                save_png(&mut h, "gallery_new_canvas_16bit.png");
            }
        }
        assert_eq!(h.app.gal.new_depth, want);
        let row = h.app.gal.preset_rects[0];
        h.click(row.center());
        match &h.app.gal.screen {
            Screen::Opening(anim) => {
                let doc = anim.doc.as_ref().expect("a blank canvas");
                assert_eq!(doc.depth, want);
                assert_eq!(doc.layer(doc.layer_id(0).unwrap()).unwrap().depth(), want);
            }
            _ => panic!("the preset did not start a new canvas"),
        }
        h.app.gal.screen = Screen::Gallery;
    }
    let _ = fs::remove_dir_all(&dir);
}

/// A real mouse or trackpad click: the press and the release arrive several frames apart, with
/// a 1–2 px drift in between. Each preset row must still create its canvas.
#[test]
fn preset_rows_create_on_a_press_held_across_frames() {
    let (lib, dir) = library_with("presets-held", 5);
    let mut h = gallery_app(&lib, false);
    let expected = size_presets((WINDOW.x as usize, WINDOW.y as usize));
    for (i, (name, w, hh)) in expected.into_iter().enumerate() {
        h.click(h.app.gal.header.plus.center());
        h.advance(0.4);
        assert!(h.app.gal.menu.is_some(), "the popover opens for {name}");
        let row = h.app.gal.preset_rects.get(i).copied().expect("a row per preset");
        let start = row.center();
        h.move_to(start);
        h.step();
        h.press(start, true);
        for _ in 0..12 {
            h.step();
        }
        let drift = start + Vec2::new(1.0, 1.0);
        h.move_to(drift);
        h.step();
        let end = start + Vec2::new(2.0, 1.0);
        h.move_to(end);
        h.step();
        h.press(end, false);
        h.step();
        assert!(h.app.gal.menu.is_none(), "{name} closes the popover");
        match &h.app.gal.screen {
            Screen::Opening(anim) => assert_eq!(anim.size, (w, hh), "{name} asks for a canvas of its size"),
            _ => panic!("{name} did not start a new canvas on a held press"),
        }
        h.app.gal.screen = Screen::Gallery;
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn backdrop_is_dark_in_both_themes() {
    for light in [false, true] {
        let (lib, dir) = library_with(&format!("backdrop-{light}"), 0);
        let mut h = gallery_app(&lib, light);
        let img = h.render();
        assert_eq!(&img.pixel(20, 400)[..3], &[0x12, 0x12, 0x12]);
        let _ = fs::remove_dir_all(&dir);
    }
}

/// The card of `id` as drawn last frame.
fn card_rect(h: &Headless, id: &str) -> Rect {
    h.app.gal.card_rects.iter().find(|(c, _)| c == id).expect("card on screen").1
}

/// Long-press `from`, drag to `to` and release. The press lifts the card before the move.
fn drag_card(h: &mut Headless, from: Pos2, to: Pos2) {
    h.move_to(from);
    h.step();
    h.press(from, true);
    h.step();
    h.advance(0.5);
    for i in 1..=8 {
        h.move_to(from + (to - from) * (i as f32 / 8.0));
        h.step();
    }
    h.press(to, false);
    h.step();
    h.step();
    h.advance(0.1);
}

#[test]
fn stacks_are_made_by_drag_opened_and_unstacked_and_persist() {
    let (lib, dir) = library_with("stack-flow", 4);
    let mut h = gallery_app(&lib, false);
    let ids: Vec<String> = h.app.gal.entries.iter().map(|e| e.id.clone()).collect();

    // Drop card 0 on the middle of card 1: they become one stack, card 0 first.
    let (from, to) = (card_rect(&h, &ids[0]).center(), card_rect(&h, &ids[1]).center());
    drag_card(&mut h, from, to);
    assert_eq!(h.app.gal.index.stacks.len(), 1, "one stack made");
    let sid = h.app.gal.index.stacks[0].id.clone();
    assert_eq!(h.app.gal.index.stacks[0].docs, vec![ids[0].clone(), ids[1].clone()]);
    assert_eq!(Library::at(&dir).read_index().stacks.len(), 1, "the stack is in library.json");
    save_png(&mut h, "gallery_stacked_dark.png");

    // Tap the stack: the stack opens with its back button.
    h.click(card_rect(&h, &sid).center());
    h.advance(0.3);
    assert_eq!(h.app.gal.open_stack.as_deref(), Some(sid.as_str()));
    save_png(&mut h, "gallery_stack_open_dark.png");

    // Drag an artwork out of the stack, to the header: it returns to the root list and the
    // two-artwork stack dissolves.
    let from = card_rect(&h, &ids[0]).center();
    drag_card(&mut h, from, Pos2::new(640.0, 20.0));
    assert!(h.app.gal.open_stack.is_none(), "a stack with one artwork left is dissolved");
    assert!(Library::at(&dir).read_index().stacks.is_empty());
    assert_eq!(h.app.gal.entries.len(), 4, "no artwork was lost");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_stack_survives_a_restart_with_its_order() {
    let (lib, dir) = library_with("stack-restart", 3);
    let mut h = gallery_app(&lib, false);
    let ids: Vec<String> = h.app.gal.entries.iter().map(|e| e.id.clone()).collect();
    let name = {
        let ix = &mut h.app.gal.index;
        ix.stack_docs("s-restart".into(), "Trip".into(), &[ids[2].clone(), ids[0].clone()], &ids[2]);
        lib.write_index(ix).unwrap();
        "Trip".to_owned()
    };
    drop(h);
    let mut again = gallery_app(&Library::at(&dir), false);
    assert_eq!(again.app.gal.index.stack("s-restart").map(|s| s.name.clone()), Some(name));
    assert_eq!(again.app.gal.index.stack("s-restart").unwrap().docs, vec![ids[2].clone(), ids[0].clone()]);
    assert_eq!(again.app.tiles().iter().filter(|t| t.stack).count(), 1, "the stack is one card");
    again.app.gal.screen = Screen::Gallery;
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn renaming_a_card_saves_the_new_title_to_disk() {
    let (lib, dir) = library_with("rename-ui", 2);
    let mut h = gallery_app(&lib, false);
    let id = h.app.gal.entries[0].id.clone();
    let card = card_rect(&h, &id);
    // Tapping the title starts the editor.
    let title = Pos2::new(card.left() + 30.0, card.top() + (card.height() - CARD_TEXT_H) + 18.0);
    h.click(title);
    h.advance(0.1);
    assert!(h.app.gal.renaming.as_ref().is_some_and(|r| r.0 == id), "tapping the title edits it");
    h.app.commit_rename(&id, "  Renamed\tcard  ");
    h.advance(0.1);
    assert_eq!(h.app.gal.entries.iter().find(|e| e.id == id).unwrap().title, "Renamed card");
    assert_eq!(Library::at(&dir).get(&id).unwrap().title, "Renamed card", "the rename is on disk");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn sort_orders_the_cards() {
    let (lib, dir) = library_with("sort-ui", 4);
    let mut h = gallery_app(&lib, false);
    let titles = |h: &Headless| h.app.tiles().into_iter().map(|t| t.title).collect::<Vec<_>>();
    assert_eq!(titles(&h).len(), 4);
    h.app.set_sort(crate::stacks::SortMode::Name);
    assert_eq!(titles(&h), ["Harbour at dusk", "Night sky", "Study in green", "Untitled 3"]);
    h.app.set_sort(crate::stacks::SortMode::Size);
    let sizes: Vec<usize> = h.app.tiles().iter().map(|t| t.size.0 * t.size.1).collect();
    assert!(sizes.windows(2).all(|w| w[0] >= w[1]), "largest first: {sizes:?}");
    h.app.set_sort(crate::stacks::SortMode::Manual);
    let _ = fs::remove_dir_all(&dir);
}

fn pick_mock(h: &mut Headless) -> std::sync::Arc<crate::file_access::MockFileAccess> {
    let mock = std::sync::Arc::new(crate::file_access::MockFileAccess::new());
    let files: std::sync::Arc<dyn crate::file_access::FileAccess> = mock.clone();
    h.app.files = files;
    mock
}

#[test]
fn gallery_import_browse_imports_the_picked_file() {
    use crate::file_access::{PickKind, PickPurpose, Picked};
    let (lib, dir) = library_with("browse-import", 0);
    fs::create_dir_all(dir.join("elsewhere")).unwrap();
    let src = dir.join("elsewhere").join("picked.procreate");
    eskiz_format::document::write(&src, &storage::to_builder(&art(160, 120, 2), "Picked")).unwrap();
    let mut h = gallery_app(&lib, false);
    h.ctx.enable_accesskit();
    let mock = pick_mock(&mut h);
    mock.push(Picked::Path(src));
    h.click(h.app.gal.header.import.center());
    h.advance(0.4);
    let browse = h
        .access
        .as_ref()
        .and_then(|u| u.nodes.iter().find(|(_, n)| n.label() == Some("Browse...")).and_then(|(_, n)| n.bounds()))
        .expect("a Browse... button in the Import popover");
    h.click(Pos2::new(((browse.x0 + browse.x1) / 2.0) as f32, ((browse.y0 + browse.y1) / 2.0) as f32));
    for _ in 0..4 {
        h.step();
    }
    let req = mock.requests().pop().expect("a request");
    assert_eq!((req.purpose, req.kind), (PickPurpose::GalleryImport, PickKind::OpenFile));
    assert_eq!(req.filters[0].extensions, ["procreate"]);
    h.settle_storage();
    assert_eq!(h.app.gal.entries.len(), 1);
    assert_eq!(h.app.gal.entries[0].title, "Picked");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn gallery_share_asks_for_a_folder_and_exports_there() {
    use crate::file_access::{PickKind, PickPurpose, Picked};
    let (lib, dir) = library_with("share-folder", 1);
    let mut h = gallery_app(&lib, false);
    let mock = pick_mock(&mut h);
    let out = dir.join("chosen");
    fs::create_dir_all(&out).unwrap();
    let id = h.app.gal.entries[0].id.clone();
    let ctx = h.ctx.clone();
    mock.push(Picked::Path(out.clone()));
    h.app.apply_action(&ctx, Action::Share(id.clone()));
    for _ in 0..4 {
        h.step();
    }
    let req = mock.requests().pop().expect("a request");
    assert_eq!((req.purpose, req.kind), (PickPurpose::GalleryShare, PickKind::Folder));
    h.settle_storage();
    for _ in 0..100 {
        if fs::read_dir(&out).map(|d| d.count() > 0).unwrap_or(false) {
            break;
        }
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(fs::read_dir(&out).unwrap().next().is_some(), "a PNG was written into the chosen folder");
    // Cancel does nothing and forgets the target.
    mock.push(Picked::None);
    h.app.apply_action(&ctx, Action::Share(id));
    h.step();
    h.step();
    assert!(h.app.gal.share_target.is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn gallery_share_without_a_dialog_falls_back_to_the_default_folder() {
    use crate::file_access::{PickPurpose, Picked};
    let (lib, dir) = library_with("share-fallback", 1);
    let mut h = gallery_app(&lib, false);
    let mock = pick_mock(&mut h);
    mock.push(Picked::Unavailable("No portal.".into()));
    let id = h.app.gal.entries[0].id.clone();
    let ctx = h.ctx.clone();
    h.app.apply_action(&ctx, Action::Share(id));
    h.step();
    h.step();
    assert_eq!(mock.requests()[0].purpose, PickPurpose::GalleryShare);
    assert!(h.app.gal.share_target.is_none(), "the export was started");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn gallery_share_on_android_skips_the_folder_and_opens_the_share_sheet() {
    use crate::file_access::{FileAccess, MockFileAccess, Picked};
    let (lib, dir) = library_with("share-sheet", 1);
    let mut h = gallery_app(&lib, false);
    let mock = std::sync::Arc::new(MockFileAccess::new());
    mock.set_staged(true);
    mock.set_share_root(dir.join("share-cache"));
    // A scripted pick would be a mistake: none may be asked for.
    mock.push(Picked::Path(dir.join("never")));
    let files: std::sync::Arc<dyn FileAccess> = mock.clone();
    h.app.files = files;
    let id = h.app.gal.entries[0].id.clone();
    let ctx = h.ctx.clone();
    h.app.apply_action(&ctx, Action::Share(id));
    h.settle_storage();
    for _ in 0..150 {
        if !mock.shares().is_empty() {
            break;
        }
        h.step();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(mock.requests().is_empty(), "no folder pick on Android");
    let (files, mime) = mock.shares().pop().expect("the share sheet was opened");
    assert_eq!(mime, "image/png");
    assert_eq!(files.len(), 1);
    assert!(files[0].starts_with(dir.join("share-cache")) && files[0].is_file(), "{files:?}");
    let _ = fs::remove_dir_all(&dir);
}
