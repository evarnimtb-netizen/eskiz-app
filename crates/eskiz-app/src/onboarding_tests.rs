//! Tests of the welcome tour on the running (headless) app: flow, upgrade migration, the
//! recovery sheet, accessibility names, the sample document and the snapshots of every card and
//! coach mark in both looks. Snapshots go to `target/snapshots/tour_*.png`.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, accesskit, Event, Key, Modifiers, Pos2, Rect, Vec2};

use crate::app::{EskizApp, Prefs, LEGACY_PREF_KEY, PREF_KEY};
use crate::document::Document;
use crate::gallery::Screen;
use crate::onboarding::{self, copy, InputKind, Step, Tour, KEY, SAMPLE_TITLE, TOUR_VERSION};
use crate::snapshot::{snapshot_dir, Headless};
use crate::storage::Library;
use crate::theme::UiIdentity;

const WINDOW: Vec2 = Vec2::new(1280.0, 800.0);

#[derive(Default)]
struct Mem(HashMap<String, String>);
impl eframe::Storage for Mem {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
    fn flush(&mut self) {}
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-tour-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A fresh install: no storage entries at all.
fn fresh() -> Tour {
    Tour::from_storage(Some(&Mem::default()), false)
}

/// The Gallery over an empty library in `dir`, with `tour`.
fn gallery(dir: &PathBuf, tour: Tour, identity: UiIdentity, light: bool) -> Headless {
    let lib = Library::at(dir);
    let mut h = Headless::from_app(WINDOW, move |ctx| {
        let mut app = EskizApp::with_library(ctx, Prefs { identity, light, ..Prefs::default() }, lib);
        app.tour = tour;
        app
    });
    h.ctx.enable_accesskit();
    h.settle_storage();
    h.advance(0.2);
    h
}

fn names(h: &Headless) -> Vec<String> {
    h.access
        .as_ref()
        .map(|u| u.nodes.iter().filter_map(|(_, n)| n.label().or_else(|| n.value()).map(str::to_owned)).collect())
        .unwrap_or_default()
}

fn node_rect(h: &Headless, name: &str) -> Rect {
    let update = h.access.as_ref().expect("accesskit is on");
    let r: accesskit::Rect = update
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some(name) || n.value() == Some(name))
        .and_then(|(_, n)| n.bounds())
        .unwrap_or_else(|| panic!("no node {name:?} in {:?}", names(h)));
    Rect::from_min_max(Pos2::new(r.x0 as f32, r.y0 as f32), Pos2::new(r.x1 as f32, r.y1 as f32))
}

fn press_button(h: &mut Headless, name: &str) {
    let c = node_rect(h, name).center();
    h.click(c);
    h.advance(0.1);
}

fn esc(h: &mut Headless) {
    for pressed in [true, false] {
        h.event(Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.advance(0.1);
}

fn paint_stroke(h: &mut Headless) {
    let vp = h.app.viewport;
    let a = vp.center() - Vec2::new(120.0, 40.0);
    h.move_to(a);
    h.step();
    h.press(a, true);
    h.step();
    for i in 1..=12 {
        h.move_to(a + Vec2::new(20.0 * i as f32, 6.0 * i as f32));
        h.step();
    }
    h.press(a + Vec2::new(240.0, 72.0), false);
    h.advance(0.2);
}

/// Open the sample from card 2 and wait for the canvas.
fn open_sample(h: &mut Headless) {
    if h.app.tour.step() == Some(Step::Welcome) {
        press_button(h, copy::SHOW_AROUND);
    }
    assert_eq!(h.app.tour.step(), Some(Step::Sample));
    press_button(h, copy::OPEN_SAMPLE);
    for _ in 0..120 {
        h.step();
        if matches!(h.app.gal.screen, Screen::Canvas) {
            break;
        }
    }
    h.advance(0.3);
    assert!(matches!(h.app.gal.screen, Screen::Canvas), "the sample opened");
}

// ------------------------------------------------------------------ startup

#[test]
fn fresh_install_shows_card_one_over_the_empty_gallery() {
    let dir = temp("fresh");
    let h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    assert_eq!(h.app.tour.step(), Some(Step::Welcome));
    assert_eq!(h.app.tour.stored_value(), Some(TOUR_VERSION));
    assert!(h.app.tour.card_rect.is_positive());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn upgrade_with_prefs_and_no_tour_key_is_migrated_and_sees_nothing() {
    let dir = temp("upgrade");
    let mut old = Mem::default();
    eframe::Storage::set_string(&mut old, PREF_KEY, Prefs::default().encode());
    let mut h = gallery(&dir, Tour::from_storage(Some(&old), false), UiIdentity::Classic, false);
    assert_eq!(h.app.tour.step(), None);
    assert!(!h.app.tour.is_waiting());
    let mut after = Mem::default();
    h.app.persist_prefs(&mut after);
    assert_eq!(after.get_string_for(KEY).as_deref(), Some("tour=1"), "migrated silently");
    // Even with an empty library, the old-name prefs key counts too.
    let mut older = Mem::default();
    eframe::Storage::set_string(&mut older, LEGACY_PREF_KEY, Prefs::default().encode());
    let h = gallery(&dir, Tour::from_storage(Some(&older), false), UiIdentity::Classic, false);
    assert_eq!(h.app.tour.step(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

trait GetFor {
    fn get_string_for(&self, key: &str) -> Option<String>;
}
impl GetFor for Mem {
    fn get_string_for(&self, key: &str) -> Option<String> {
        eframe::Storage::get_string(self, key)
    }
}

#[test]
fn a_library_with_artwork_never_shows_it() {
    let dir = temp("artwork");
    let lib = Library::at(&dir);
    lib.save("a1", "Old drawing", &Document::new(64, 64)).unwrap();
    let h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    assert_eq!(h.app.tour.step(), None);
    assert_eq!(h.app.tour.stored_value(), Some(TOUR_VERSION), "marked as seen");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_crash_mid_tour_does_not_replay_it() {
    let dir = temp("crash");
    let mut mem = Mem::default();
    let mut h = gallery(&dir, Tour::from_storage(Some(&mem), false), UiIdentity::Classic, false);
    assert_eq!(h.app.tour.step(), Some(Step::Welcome));
    // eframe's periodic save runs while the tour is on screen; then the process dies.
    h.app.persist_prefs(&mut mem);
    drop(h);
    let t = Tour::from_storage(Some(&mem), false);
    assert!(!t.is_waiting() && t.step().is_none(), "next launch shows nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_recovery_sheet_goes_first_and_the_tour_waits_for_it() {
    let dir = temp("recovery");
    let lib = Library::at(&dir);
    let tour = fresh();
    let mut h = Headless::from_app(WINDOW, move |ctx| {
        let mut app = EskizApp::with_library(ctx, Prefs::default(), lib);
        app.gal.offers.push(crate::journal::Pending { id: "x".into(), title: "Harbour".into(), saved_at: std::time::SystemTime::now() });
        app.tour = tour;
        app
    });
    h.settle_storage();
    h.advance(0.3);
    assert_eq!(h.app.tour.step(), None, "no tour under the recovery sheet");
    assert!(h.app.tour.is_waiting());
    // Closing the sheet (here: the offer is dealt with) lets the tour start.
    h.app.gal.offers.clear();
    h.advance(0.2);
    assert_eq!(h.app.tour.step(), Some(Step::Welcome));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ skipping

#[test]
fn skip_button_and_escape_end_the_tour_on_each_card() {
    for (i, how) in ["button", "escape"].iter().enumerate() {
        for step in [Step::Welcome, Step::Sample] {
            let dir = temp(&format!("skip-{i}-{step:?}"));
            let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
            if step == Step::Sample {
                press_button(&mut h, copy::SHOW_AROUND);
            }
            assert_eq!(h.app.tour.step(), Some(step));
            if *how == "button" {
                press_button(&mut h, copy::SKIP);
            } else {
                esc(&mut h);
            }
            assert_eq!(h.app.tour.step(), None, "{how} on {step:?}");
            assert_eq!(h.app.tour.stored_value(), Some(TOUR_VERSION));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[test]
fn start_a_new_canvas_ends_the_tour_and_opens_the_new_canvas_menu() {
    let dir = temp("newcanvas");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    press_button(&mut h, copy::SHOW_AROUND);
    press_button(&mut h, copy::NEW_CANVAS);
    assert_eq!(h.app.tour.step(), None);
    assert!(h.app.gal.library.as_ref().unwrap().list().is_empty(), "no sample was made");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_gallery_underneath_is_inert_while_a_card_is_up() {
    let dir = temp("modal");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    h.click(h.app.gal.header.plus.center());
    h.advance(0.4);
    assert!(h.app.gal.menu.is_none(), "the + button is behind the card");
    assert_eq!(h.app.tour.step(), Some(Step::Welcome));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ the whole tour

#[test]
fn the_full_tour_on_the_sample_document() {
    let dir = temp("full");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    open_sample(&mut h);
    assert_eq!(h.app.tour.step(), Some(Step::Paint));
    assert_eq!(h.app.session.doc.width, 1600);
    assert_eq!(h.app.session.doc.height, 1000);

    // Nothing moves on by itself: no timer.
    h.advance(5.0);
    assert_eq!(h.app.tour.step(), Some(Step::Paint));

    paint_stroke(&mut h);
    assert_eq!(h.app.tour.step(), Some(Step::Undo), "a committed stroke finishes mark 1");
    h.advance(1.0);
    assert_eq!(h.app.tour.step(), Some(Step::Undo));

    press_button(&mut h, "Undo");
    assert_eq!(h.app.tour.step(), Some(Step::Size), "the first undo finishes mark 2");

    for pressed in [true, false] {
        h.event(Event::Key { key: Key::CloseBracket, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.advance(0.2);
    assert_eq!(h.app.tour.step(), Some(Step::Done), "a size change finishes mark 3");

    press_button(&mut h, copy::DONE);
    assert_eq!(h.app.tour.step(), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_size_mark_also_finishes_on_the_slider() {
    let dir = temp("slider");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    open_sample(&mut h);
    paint_stroke(&mut h);
    press_button(&mut h, "Undo");
    assert_eq!(h.app.tour.step(), Some(Step::Size));
    let r = h.app.tour_rects.size;
    h.click(r.center() + Vec2::new(0.0, 30.0));
    h.advance(0.2);
    assert_eq!(h.app.tour.step(), Some(Step::Done));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skip_tour_and_escape_work_on_every_coach_mark() {
    for (i, how) in ["button", "escape"].iter().enumerate() {
        for target in [Step::Paint, Step::Undo, Step::Size, Step::Done] {
            let dir = temp(&format!("markskip-{i}-{target:?}"));
            let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
            open_sample(&mut h);
            if target != Step::Paint {
                paint_stroke(&mut h);
            }
            if matches!(target, Step::Size | Step::Done) {
                press_button(&mut h, "Undo");
            }
            if target == Step::Done {
                h.app.tool_params[h.app.tool.index()].size += 10.0;
                h.advance(0.2);
            }
            assert_eq!(h.app.tour.step(), Some(target));
            if *how == "button" && target != Step::Done {
                press_button(&mut h, copy::SKIP_TOUR);
            } else {
                esc(&mut h);
            }
            assert_eq!(h.app.tour.step(), None, "{how} on {target:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[test]
fn replay_from_help_starts_at_card_one_without_changing_the_stored_version() {
    let dir = temp("replay");
    let seen = Tour::at_startup(Some(TOUR_VERSION), true, false);
    let lib = Library::at(&dir);
    let mut h = Headless::from_app(WINDOW, move |ctx| {
        let mut app = EskizApp::new(ctx, Document::new(800, 600), String::new(), Prefs::default());
        app.gal.attach(ctx, lib);
        app.tour = seen;
        app
    });
    h.ctx.enable_accesskit();
    h.settle_storage();
    assert_eq!(h.app.tour.step(), None);
    // Actions > Help > Welcome tour, by the buttons.
    h.open_panel(crate::popover::Panel::Actions);
    press_button(&mut h, "Help");
    h.advance(0.3);
    press_button(&mut h, &format!("{}, {}", copy::HELP_ROW, copy::HELP_ROW_SUB));
    h.advance(0.3);
    assert!(matches!(h.app.gal.screen, Screen::Gallery));
    assert_eq!(h.app.tour.step(), Some(Step::Welcome));
    assert_eq!(h.app.tour.stored_value(), Some(TOUR_VERSION));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_settings_switch_writes_tour_zero_and_it_replays_once() {
    let dir = temp("again");
    let mut h = gallery(&dir, Tour::at_startup(Some(TOUR_VERSION), true, false), UiIdentity::Classic, false);
    h.app.tour.set_show_again(true);
    let mut mem = Mem::default();
    eframe::Storage::set_string(&mut mem, PREF_KEY, Prefs::default().encode());
    h.app.persist_prefs(&mut mem);
    assert_eq!(mem.get_string_for(KEY).as_deref(), Some("tour=0"));
    let _ = std::fs::remove_dir_all(&dir);
    // Next launch: prefs, artwork and all, the tour opens once.
    let dir2 = temp("again2");
    Library::at(&dir2).save("a1", "Old drawing", &Document::new(64, 64)).unwrap();
    let mut h = gallery(&dir2, Tour::from_storage(Some(&mem), false), UiIdentity::Classic, false);
    assert_eq!(h.app.tour.step(), Some(Step::Welcome));
    h.app.persist_prefs(&mut mem);
    assert_eq!(mem.get_string_for(KEY).as_deref(), Some("tour=1"), "the switch turned itself off");
    let _ = std::fs::remove_dir_all(&dir2);
}

// ------------------------------------------------------------------ input kinds

#[test]
fn touch_input_shows_the_touch_wording_and_the_mouse_the_key_hints() {
    let dir = temp("wording");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    open_sample(&mut h);
    let find = |h: &Headless, s: &str| names(h).iter().any(|n| n.contains(s));
    // The pointer moved in open_sample: that is a mouse.
    assert_eq!(h.app.tour.input, InputKind::Mouse);
    assert!(find(&h, "Press B"), "{:?}", names(&h));
    h.event(Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::Start,
        pos: Pos2::new(600.0, 400.0),
        force: None,
    });
    h.step();
    h.event(Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(1),
        phase: egui::TouchPhase::Cancel,
        pos: Pos2::new(600.0, 400.0),
        force: None,
    });
    h.advance(0.2);
    assert_eq!(h.app.tour.input, InputKind::Touch);
    assert!(find(&h, "Drag one finger to paint"), "{:?}", names(&h));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ accessibility

#[test]
fn cards_and_marks_are_findable_by_accessible_name() {
    let dir = temp("a11y");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    let n = names(&h);
    for want in [copy::welcome_title().as_str(), copy::SHOW_AROUND, copy::SKIP] {
        assert!(n.iter().any(|x| x == want), "{want:?} not in {n:?}");
    }
    press_button(&mut h, copy::SHOW_AROUND);
    let n = names(&h);
    for want in [copy::SAMPLE_TITLE, copy::OPEN_SAMPLE, copy::NEW_CANVAS, copy::SKIP] {
        assert!(n.iter().any(|x| x == want), "{want:?} not in {n:?}");
    }
    open_sample(&mut h);
    let n = names(&h);
    for want in [copy::PAINT_TITLE, copy::SKIP_TOUR] {
        assert!(n.iter().any(|x| x == want), "{want:?} not in {n:?}");
    }
    paint_stroke(&mut h);
    press_button(&mut h, "Undo");
    h.app.tool_params[h.app.tool.index()].size += 10.0;
    h.advance(0.2);
    let n = names(&h);
    for want in [copy::DONE_TITLE, copy::DONE] {
        assert!(n.iter().any(|x| x == want), "{want:?} not in {n:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn focus_lands_on_the_primary_button_of_each_card() {
    let dir = temp("focus");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    let focused = |h: &Headless| {
        let u = h.access.as_ref().unwrap();
        u.nodes.iter().find(|(id, _)| *id == u.focus).and_then(|(_, n)| n.label().map(str::to_owned))
    };
    assert_eq!(focused(&h).as_deref(), Some(copy::SHOW_AROUND));
    press_button(&mut h, copy::SHOW_AROUND);
    h.advance(0.1);
    assert_eq!(focused(&h).as_deref(), Some(copy::OPEN_SAMPLE));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tab_stays_inside_the_card() {
    let dir = temp("tab");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..8 {
        for pressed in [true, false] {
            h.event(Event::Key { key: Key::Tab, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
            h.step();
        }
        let u = h.access.as_ref().unwrap();
        if let Some(l) = u.nodes.iter().find(|(id, _)| *id == u.focus).and_then(|(_, n)| n.label().map(str::to_owned)) {
            seen.insert(l);
        }
    }
    let allowed = [copy::SHOW_AROUND, copy::SKIP];
    assert!(seen.iter().all(|s| allowed.contains(&s.as_str())), "focus left the card: {seen:?}");
    assert!(seen.len() == 2, "{seen:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_empty_gallery_is_announced_to_screen_readers() {
    let dir = temp("empty-label");
    let h = gallery(&dir, Tour::idle(), UiIdentity::Classic, false);
    assert!(
        names(&h).iter().any(|n| n.starts_with("No artwork yet.") && n.contains("new canvas")),
        "empty state has no accessible name: {:?}",
        names(&h)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tour_text_meets_contrast_in_every_look() {
    use crate::a11y::contrast_ratio;
    use crate::theme::palette_for;
    for identity in [UiIdentity::Classic, UiIdentity::Eskiz] {
        for light in [false, true] {
            // Marks use the app palette, the Gallery cards the dark one.
            let mut pals = vec![palette_for(identity, light)];
            if light {
                pals.push(palette_for(identity, false));
            }
            for pal in pals {
                for (what, fg) in [("text", pal.text), ("text_dim", pal.text_dim)] {
                    let r = contrast_ratio(fg, pal.panel);
                    assert!(r >= 4.5, "{what} on panel {r:.2}:1 ({identity:?} light={light})");
                }
                let fill = if identity == UiIdentity::Eskiz { pal.primary } else { pal.accent };
                let r = contrast_ratio(onboarding::ink_on(fill), fill);
                assert!(r >= 4.5, "button text on the accent {r:.2}:1 ({identity:?})");
            }
        }
    }
    // The Gallery cards' primary button: white on the accent.
    for identity in [UiIdentity::Classic, UiIdentity::Eskiz] {
        let gp = crate::theme::palette_for(identity, false);
        let fill = if identity == UiIdentity::Eskiz { gp.primary } else { gp.accent };
        let r = contrast_ratio(onboarding::ink_on(fill), fill);
        assert!(r >= 4.5, "card button text on the accent {r:.2}:1 ({identity:?})");
    }
}

// ------------------------------------------------------------------ the sample

#[test]
fn the_sample_is_a_normal_saved_document_that_reopens() {
    let dir = temp("sample");
    let mut h = gallery(&dir, fresh(), UiIdentity::Classic, false);
    open_sample(&mut h);
    let lib = Library::at(&dir);
    let entries = lib.list();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, SAMPLE_TITLE);
    assert_eq!((entries[0].width, entries[0].height), (1600, 1000));
    let doc = lib.load(&entries[0].id).unwrap();
    assert_eq!((doc.width, doc.height), (1600, 1000));
    assert_eq!(doc.layer_ids().len(), 1);
    let want = crate::onboarding_sample::sample_document();
    assert_eq!(doc.layer_at(0).unwrap().pixel(800, 100), want.layer_at(0).unwrap().pixel(800, 100));
    let mut doc = doc;
    assert_eq!(doc.props_at_mut(0).unwrap().name, "Sample");
    assert!(!h.app.is_dirty(), "opening the sample is not an edit");
    // It is an ordinary artwork: a second replay opens the same one, not a copy.
    h.app.replay_tour(&h.ctx.clone());
    h.advance(0.4);
    open_sample(&mut h);
    assert_eq!(lib.list().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ snapshots

fn shot(h: &mut Headless, name: &str) {
    h.render().save_png(&snapshot_dir().join(name)).unwrap();
}

fn look(identity: UiIdentity) -> &'static str {
    match identity {
        UiIdentity::Classic => "classic",
        UiIdentity::Eskiz => "eskiz",
    }
}

fn press_key(h: &mut Headless, key: Key) {
    for pressed in [true, false] {
        h.event(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.advance(0.2);
}

fn touch_once(h: &mut Headless) {
    for phase in [egui::TouchPhase::Start, egui::TouchPhase::End] {
        h.event(Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(1), phase, pos: Pos2::new(640.0, 700.0), force: None });
        h.step();
    }
    h.advance(0.2);
}

fn check_mark_layout(h: &Headless, what: &str) {
    let (card, target) = (h.app.tour.card_rect, h.app.tour.target_rect);
    assert!(card.is_positive(), "{what}: no card drawn");
    assert!(Rect::from_min_size(Pos2::ZERO, WINDOW).contains_rect(card), "{what}: card {card:?} leaves the window");
    if target.is_positive() {
        assert!(!card.intersects(target), "{what}: card {card:?} covers its target {target:?}");
    }
}

#[test]
fn writes_tour_snapshots_in_both_looks() {
    for identity in [UiIdentity::Classic, UiIdentity::Eskiz] {
        let l = look(identity);
        let dir = temp(&format!("shots-{l}"));
        let mut h = gallery(&dir, fresh(), identity, false);
        shot(&mut h, &format!("tour_{l}_1_welcome.png"));
        press_button(&mut h, copy::SHOW_AROUND);
        shot(&mut h, &format!("tour_{l}_2_sample.png"));
        open_sample(&mut h);

        // Mark 1, before any input kind is known the two-line form; here the mouse moved.
        check_mark_layout(&h, "paint (mouse)");
        shot(&mut h, &format!("tour_{l}_3_paint_mouse.png"));
        touch_once(&mut h);
        check_mark_layout(&h, "paint (touch)");
        shot(&mut h, &format!("tour_{l}_3_paint_touch.png"));

        paint_stroke(&mut h);
        h.app.tour.input = InputKind::Mouse;
        h.advance(0.1);
        check_mark_layout(&h, "undo (mouse)");
        shot(&mut h, &format!("tour_{l}_4_undo_mouse.png"));
        h.app.tour.input = InputKind::Touch;
        h.advance(0.1);
        check_mark_layout(&h, "undo (touch)");
        shot(&mut h, &format!("tour_{l}_4_undo_touch.png"));

        press_button(&mut h, "Undo");
        check_mark_layout(&h, "size");
        shot(&mut h, &format!("tour_{l}_5_size.png"));
        press_key(&mut h, Key::CloseBracket);
        assert_eq!(h.app.tour.step(), Some(Step::Done));
        check_mark_layout(&h, "done");
        shot(&mut h, &format!("tour_{l}_6_done.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }
    // One light-theme shot of the canvas marks, and the unknown-input two-line form.
    let dir = temp("shots-light");
    let mut h = gallery(&dir, fresh(), UiIdentity::Eskiz, true);
    shot(&mut h, "tour_eskiz_light_1_welcome.png");
    open_sample(&mut h);
    h.app.tour.input = InputKind::Unknown;
    h.advance(0.1);
    check_mark_layout(&h, "paint (unknown input, light)");
    shot(&mut h, "tour_eskiz_light_3_paint_unknown.png");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn marks_clear_their_targets_with_the_sidebar_on_the_right_and_on_a_small_window() {
    let dir = temp("layouts");
    let lib = Library::at(&dir);
    let mut h = Headless::from_app(Vec2::new(900.0, 520.0), move |ctx| {
        let mut app = EskizApp::with_library(ctx, Prefs { sidebar_right: true, ..Prefs::default() }, lib);
        app.tour = fresh();
        app
    });
    h.ctx.enable_accesskit();
    h.settle_storage();
    h.advance(0.2);
    open_sample(&mut h);
    let window = Rect::from_min_size(Pos2::ZERO, h.size);
    paint_stroke(&mut h);
    for what in ["undo", "size"] {
        let (card, target) = (h.app.tour.card_rect, h.app.tour.target_rect);
        assert!(window.contains_rect(card), "{what}: {card:?}");
        assert!(!card.intersects(target), "{what}: {card:?} covers {target:?}");
        if what == "undo" {
            press_button(&mut h, "Undo");
        }
    }
    shot(&mut h, "tour_classic_small_right.png");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = onboarding::MARK_W;
}
