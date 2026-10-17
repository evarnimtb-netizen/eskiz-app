//! RC3: strokes that vanish (every path that could cancel, drop or hide one), the minimal Eskiz
//! look and the gallery title. Snapshots go to `target/snapshots/rc3/`.

use crate::app::{EskizApp, Prefs};
use crate::gallery::Screen;
use crate::snapshot::Headless;
use crate::storage::{self, Library};
use crate::stroke_trace;
use eframe::egui::{self, vec2, Event, Modifiers, Pos2, Vec2};
use std::path::PathBuf;

const WINDOW: Vec2 = vec2(1280.0, 800.0);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-rc3-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[derive(Default)]
struct MemStorage(std::collections::HashMap<String, String>);
impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
    fn flush(&mut self) {}
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
}

/// The app on the canvas of a saved document (a library is attached, so saving is live).
fn canvas_app(name: &str, prefs: Prefs) -> Headless {
    let dir = temp(name);
    let lib = Library::at(&dir);
    let id = storage::new_id();
    lib.save(&id, "Harbour", &storage::sample_document(600, 400)).unwrap();
    let mut h = Headless::from_app(WINDOW, move |ctx| EskizApp::with_library(ctx, prefs, lib));
    h.settle_storage();
    h.advance(0.3);
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    h.advance(0.4);
    h.settle_storage();
    assert!(matches!(h.app.gal.screen, Screen::Canvas));
    h.advance(0.5);
    h
}

fn quiet() -> Prefs {
    Prefs { hold_shape: false, eyedrop_hold: false, ..Prefs::default() }
}

/// Total alpha on the active layer: grows with ink.
fn ink(h: &Headless) -> u64 {
    let id = h.app.active;
    h.app.session.doc.layer(id).map_or(0, |l| l.to_rgba().chunks(4).map(|p| u64::from(p[3])).sum())
}

/// Press at `c` and drag to the right; the button stays down.
/// The ink is still there after a commit (the end taper may thin the last dabs a little).
fn kept(h: &Headless, drawn: u64) -> bool {
    drawn > 0 && ink(h) as f64 >= drawn as f64 * 0.9
}

fn open_stroke(h: &mut Headless) -> Pos2 {
    let c = h.app.viewport.center() - vec2(150.0, 0.0);
    h.move_to(c);
    h.step();
    h.press(c, true);
    h.step();
    let mut p = c;
    for i in 1..=30 {
        p = c + vec2(i as f32 * 8.0, (i as f32 * 0.4).sin() * 30.0);
        h.move_to(p);
        h.step();
    }
    assert!(h.app.session.is_stroking());
    p
}

fn release(h: &mut Headless, p: Pos2) {
    h.press(p, false);
    h.advance(0.2);
}

/// A finished stroke makes the document dirty, so a periodic save has something to write.
fn dirty_the_document(h: &mut Headless) {
    let p = open_stroke(h);
    release(h, p);
    assert!(h.app.is_dirty());
}

#[test]
fn the_periodic_save_never_cancels_a_stroke_in_progress() {
    let mut h = canvas_app("periodic", quiet());
    dirty_the_document(&mut h);
    let steps = h.app.session.undo_steps();
    let p = open_stroke(&mut h);
    let drawn = ink(&h);
    // eframe calls `save` every 30 s. With the old code this cancelled the stroke and rolled the layer back.
    let mut st = MemStorage::default();
    eframe::App::save(&mut h.app, &mut st);
    h.step();
    assert!(h.app.session.is_stroking(), "the stroke is still open after the periodic save");
    assert!(kept(&h, drawn), "no ink was rolled back");
    release(&mut h, p);
    assert_eq!(h.app.session.undo_steps(), steps + 1, "the stroke became an undo step");
    assert!(kept(&h, drawn));
    // The save that was deferred happens once the pen is up.
    h.advance(0.3);
    let mut st = MemStorage::default();
    eframe::App::save(&mut h.app, &mut st);
    h.settle_storage();
    assert!(!h.app.is_dirty(), "the deferred save ran after the stroke");
}

#[test]
fn saving_on_exit_commits_an_open_stroke_instead_of_dropping_it() {
    let mut h = canvas_app("exit", quiet());
    dirty_the_document(&mut h);
    let steps = h.app.session.undo_steps();
    let _ = open_stroke(&mut h);
    let drawn = ink(&h);
    h.app.save_on_exit();
    assert!(!h.app.session.is_stroking());
    assert_eq!(h.app.session.undo_steps(), steps + 1);
    assert!(kept(&h, drawn), "the ink is kept");
}

#[test]
fn opening_the_gallery_mid_stroke_keeps_the_ink() {
    let mut h = canvas_app("gallery", quiet());
    let _ = open_stroke(&mut h);
    let drawn = ink(&h);
    let ctx = h.ctx.clone();
    h.app.open_gallery(&ctx);
    assert!(!h.app.session.is_stroking());
    assert!(kept(&h, drawn));
    assert_eq!(h.app.session.undo_steps(), 1);
}

#[test]
fn scroll_and_pinch_mid_stroke_leave_the_view_and_the_ink_alone() {
    let mut h = canvas_app("scrollzoom", quiet());
    let p = open_stroke(&mut h);
    let view = h.app.view;
    let drawn = ink(&h);
    for _ in 0..5 {
        h.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(40.0, -60.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        h.event(Event::Zoom(1.2));
        h.step();
    }
    assert_eq!(h.app.view, view, "a palm on the trackpad does not move the canvas under the pen");
    assert!(h.app.session.is_stroking() && kept(&h, drawn));
    release(&mut h, p);
    assert_eq!(h.app.session.undo_steps(), 1);
    // Control: with no stroke, the same events do move the view.
    h.event(Event::Zoom(1.2));
    h.step();
    assert_ne!(h.app.view, view);
}

#[test]
fn space_and_the_fit_shortcut_mid_stroke_do_not_move_the_view() {
    let mut h = canvas_app("spacefit", quiet());
    let p = open_stroke(&mut h);
    let view = h.app.view;
    h.event(Event::Key { key: egui::Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.move_to(p + vec2(30.0, 0.0));
    h.step();
    h.app.fit();
    h.advance(0.1);
    assert_eq!(h.app.view, view);
    assert!(h.app.session.is_stroking(), "space does not freeze the stroke");
    h.event(Event::Key { key: egui::Key::Space, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
    release(&mut h, p + vec2(30.0, 0.0));
    assert_eq!(h.app.session.undo_steps(), 1);
}

fn touch(h: &mut Headless, id: u64, phase: egui::TouchPhase, pos: Pos2) {
    h.event(Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(id),
        phase,
        pos,
        force: None,
    });
}

#[test]
fn a_two_finger_contact_mid_mouse_stroke_is_ignored() {
    let mut h = canvas_app("palm", quiet());
    let p = open_stroke(&mut h);
    let view = h.app.view;
    let drawn = ink(&h);
    let a = h.app.viewport.center() + vec2(100.0, 150.0);
    let b = a + vec2(60.0, 0.0);
    touch(&mut h, 1, egui::TouchPhase::Start, a);
    touch(&mut h, 2, egui::TouchPhase::Start, b);
    h.step();
    for i in 1..=6 {
        touch(&mut h, 1, egui::TouchPhase::Move, a - vec2(i as f32 * 6.0, 0.0));
        touch(&mut h, 2, egui::TouchPhase::Move, b + vec2(i as f32 * 6.0, 0.0));
        h.step();
    }
    assert!(h.app.session.is_stroking(), "the palm did not cancel the stroke");
    assert!(kept(&h, drawn));
    assert_eq!(h.app.view, view);
    touch(&mut h, 1, egui::TouchPhase::End, a);
    touch(&mut h, 2, egui::TouchPhase::End, b);
    h.step();
    release(&mut h, p);
    assert_eq!(h.app.session.undo_steps(), 1);
    assert!(stroke_trace::recorded().iter().any(|l| l.starts_with("ignored multi-touch")));
}

#[test]
fn losing_window_focus_mid_stroke_commits_it() {
    let mut h = canvas_app("focus", quiet());
    let _ = open_stroke(&mut h);
    let drawn = ink(&h);
    h.event(Event::WindowFocused(false));
    h.step();
    assert!(!h.app.session.is_stroking());
    assert_eq!(h.app.session.undo_steps(), 1, "the stroke is an undo step, not lost");
    assert!(kept(&h, drawn));
    assert!(stroke_trace::recorded().iter().any(|l| l.contains("commit reason=window lost focus")));
    // A fresh stroke afterwards still draws.
    h.event(Event::WindowFocused(true));
    h.step();
    let p = open_stroke(&mut h);
    release(&mut h, p);
    assert_eq!(h.app.session.undo_steps(), 2);
}

#[test]
fn the_pointer_leaving_the_window_does_not_end_or_drop_the_stroke() {
    let mut h = canvas_app("leave", quiet());
    let p = open_stroke(&mut h);
    let drawn = ink(&h);
    h.event(Event::PointerGone);
    h.advance(0.2);
    assert!(h.app.session.is_stroking());
    h.move_to(p + vec2(10.0, 5.0));
    h.step();
    release(&mut h, p + vec2(10.0, 5.0));
    assert_eq!(h.app.session.undo_steps(), 1);
    assert!(kept(&h, drawn));
}

#[test]
fn a_press_with_the_old_stroke_still_open_keeps_it_and_starts_the_next() {
    let mut h = canvas_app("press2", quiet());
    let p = open_stroke(&mut h);
    let drawn = ink(&h);
    // The release never arrived (drag lock, a swallowed button-up): the user releases and
    // presses again within one frame.
    h.press(p, false);
    h.press(p + vec2(0.0, 80.0), true);
    h.step();
    assert!(h.app.session.undo_steps() >= 1, "the first stroke was committed, not dropped");
    assert!(kept(&h, drawn));
    h.move_to(p + vec2(40.0, 120.0));
    h.step();
    h.press(p + vec2(40.0, 120.0), false);
    h.advance(0.2);
    assert_eq!(h.app.session.undo_steps(), 2, "both strokes are undo steps (trace {:?})", crate::stroke_trace::recorded());
}

#[test]
fn undo_key_repeat_mid_stroke_never_removes_the_stroke() {
    let mut h = canvas_app("undo", quiet());
    dirty_the_document(&mut h);
    let steps = h.app.session.undo_steps();
    let p = open_stroke(&mut h);
    let drawn = ink(&h);
    let cmd = Modifiers { command: true, ..Modifiers::NONE };
    for i in 0..10 {
        h.event(Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: i > 0, modifiers: cmd });
        h.step();
    }
    assert!(h.app.session.is_stroking() && ink(&h) >= drawn, "undo repeat mid-stroke does nothing");
    release(&mut h, p);
    assert_eq!(h.app.session.undo_steps(), steps + 1);
}

#[test]
fn the_pick_color_hold_timer_never_cancels_a_stroke() {
    let mut h = canvas_app("pickhold", Prefs { eyedrop_hold: true, hold_shape: false, ..Prefs::default() });
    let p = open_stroke(&mut h);
    let drawn = ink(&h);
    h.app.modify_press = Some(h.app.now - 5.0);
    h.advance(1.0);
    assert!(h.app.eyedrop.is_none() && h.app.session.is_stroking() && ink(&h) >= drawn);
    h.app.modify_press = None;
    release(&mut h, p);
    assert_eq!(h.app.session.undo_steps(), 1);
}

#[test]
fn autosave_and_the_journal_wait_for_the_pen_and_leave_the_stroke_alone() {
    let mut h = canvas_app("autosave", quiet());
    dirty_the_document(&mut h);
    h.app.gal.autosave_secs = 0.2;
    let steps = h.app.session.undo_steps();
    let p = open_stroke(&mut h);
    let drawn = ink(&h);
    // Hold the button for far longer than the autosave and journal delays.
    for _ in 0..20 {
        h.advance(0.5);
        assert!(h.app.session.is_stroking(), "the stroke is open while timers fire");
    }
    assert_eq!(ink(&h), drawn, "nothing touched the layer");
    release(&mut h, p);
    assert_eq!(h.app.session.undo_steps(), steps + 1);
    h.settle_storage();
    h.advance(1.0);
    h.settle_storage();
    assert!(!h.app.is_dirty(), "after the pen lifted the autosave wrote the stroke");
}

#[test]
fn a_layer_switch_mid_stroke_still_refreshes_the_stroke_layer() {
    let mut h = canvas_app("layer", quiet());
    let first = h.app.active;
    let p = open_stroke(&mut h);
    assert_eq!(h.app.session.stroke_layer(), Some(first));
    let _ = h.app.session.add_layer(None);
    h.app.active = h.app.session.doc.layer_ids().into_iter().find(|&i| i != first).unwrap();
    h.step();
    assert_eq!(h.app.session.stroke_layer(), Some(first), "the stroke stays on the layer it began on");
    release(&mut h, p);
    assert!(h.app.session.undo_steps() >= 1);
}

#[test]
fn the_desktop_never_runs_the_android_lifecycle_path() {
    let h = canvas_app("lc", quiet());
    assert_eq!(h.app.lifecycle_on, cfg!(target_os = "android"));
    assert!(!h.app.lifecycle_on);
}

#[test]
fn suspending_commits_an_open_stroke() {
    let mut h = canvas_app("suspend", quiet());
    let _ = open_stroke(&mut h);
    let drawn = ink(&h);
    let mut st = MemStorage::default();
    h.app.lifecycle_suspend(&mut st);
    assert!(!h.app.session.is_stroking());
    assert_eq!(h.app.session.undo_steps(), 1);
    assert!(kept(&h, drawn));
}

#[test]
fn the_stroke_trace_logs_begin_commit_and_cancel_with_reasons() {
    stroke_trace::clear();
    let mut h = canvas_app("trace", quiet());
    let p = open_stroke(&mut h);
    release(&mut h, p);
    let _ = open_stroke(&mut h);
    h.event(Event::WindowFocused(false));
    h.step();
    h.app.cancel_stroke_for("test");
    let lines = stroke_trace::recorded();
    assert!(lines.iter().any(|l| l.starts_with("begin layer=")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("commit reason=pointer released")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("commit reason=window lost focus")), "{lines:?}");
    // A cancel is logged only when a stroke is open.
    let _ = open_stroke(&mut h);
    h.app.cancel_stroke_for("test discard");
    assert!(stroke_trace::recorded().iter().any(|l| l.starts_with("cancel reason=test discard")));
}

// ---------------------------------------------------------------------------------------------
// 3. Gallery title

use crate::chrome::{title_inset_for, TRAFFIC_LIGHTS_END, TRAFFIC_LIGHTS_GAP, TITLE_MARGIN};
use crate::snapshot::Image;
use crate::theme::UiIdentity;

/// Top-left strip of `img` (the header), with a red line where the macOS traffic lights end.
fn header_crop(img: &Image, mac: bool) -> Image {
    let (w, h) = (460usize, 64usize);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let mut p = img.pixel(x, y);
            if mac && x == TRAFFIC_LIGHTS_END as usize {
                p = [255, 0, 0, 255];
            }
            rgba.extend_from_slice(&p);
        }
    }
    Image { width: w, height: h, rgba }
}

#[test]
fn the_gallery_title_has_one_size_baseline_and_inset_in_both_screens() {
    let dir = crate::snapshot::snapshot_dir().join("rc3");
    std::fs::create_dir_all(&dir).unwrap();
    for identity in UiIdentity::ALL {
        for mac in [true, false] {
            let name = format!("{}_{}", identity.key(), if mac { "mac" } else { "other" });
            let lib_dir = temp(&format!("title-{name}"));
            let lib = Library::at(&lib_dir);
            lib.save(&storage::new_id(), "Harbour", &storage::sample_document(600, 400)).unwrap();
            let prefs = Prefs { identity, ..Prefs::default() };
            let mut h = Headless::from_app(WINDOW, move |ctx| EskizApp::with_library(ctx, prefs, lib));
            h.app.mac_insets = mac;
            h.settle_storage();
            h.advance(0.4);
            let g_title = h.app.gal.header.title;
            h.render().save_png(&dir.join(format!("header_gallery_{name}_full.png"))).unwrap();
            header_crop(&h.render(), mac).save_png(&dir.join(format!("header_gallery_{name}.png"))).unwrap();

            let card = h.app.gal.card_rects[0].1;
            h.click(card.center());
            h.advance(0.5);
            h.settle_storage();
            assert!(matches!(h.app.gal.screen, Screen::Canvas));
            h.advance(0.4);
            let c_title = h.app.gallery_title;
            header_crop(&h.render(), mac).save_png(&dir.join(format!("header_canvas_{name}.png"))).unwrap();

            let inset = title_inset_for(mac, false);
            assert_eq!(inset, if mac { TRAFFIC_LIGHTS_END + TRAFFIC_LIGHTS_GAP } else { TITLE_MARGIN });
            // Same start, same size (so same font and size), same vertical centre (same baseline).
            assert!((g_title.left() - inset).abs() < 1.0, "{name}: gallery title at {} want {inset}", g_title.left());
            assert!((c_title.left() - inset).abs() < 1.0, "{name}: canvas title at {} want {inset}", c_title.left());
            assert!((g_title.width() - c_title.width()).abs() < 0.5, "{name}: widths {} vs {}", g_title.width(), c_title.width());
            assert!((g_title.height() - c_title.height()).abs() < 0.5, "{name}: heights differ");
            assert!((g_title.center().y - c_title.center().y).abs() < 0.5, "{name}: baselines {} vs {}", g_title.center().y, c_title.center().y);
            if mac {
                assert!(g_title.left() >= TRAFFIC_LIGHTS_END + 10.0, "a comfortable gap after the traffic lights");
            }
        }
    }
}

#[test]
fn the_title_inset_is_the_normal_margin_off_macos_and_in_full_screen() {
    assert_eq!(title_inset_for(false, false), TITLE_MARGIN);
    assert_eq!(title_inset_for(true, true), TITLE_MARGIN);
    assert!(title_inset_for(true, false) > TRAFFIC_LIGHTS_END);
    assert!((70.0..=78.0).contains(&TRAFFIC_LIGHTS_END));
}

/// A click without movement is a single sample, which the engine turns into no dab: the app
/// repeats the point so a tap leaves a dot, never an invisible stroke.
#[test]
fn a_click_without_movement_leaves_a_dot() {
    let mut h = canvas_app("dot", quiet());
    let c = h.app.viewport.center();
    let before = ink(&h);
    h.move_to(c);
    h.step();
    h.press(c, true);
    h.step();
    h.press(c, false);
    h.advance(0.2);
    assert!(ink(&h) > before, "the click painted a dot");
    assert_eq!(h.app.session.undo_steps(), 1);
}

// ---------------------------------------------------------------------------------------------
// 2. The minimal Eskiz look

use crate::popover::Panel;
use crate::theme::{palette_for, shadow};
use eframe::egui::Color32;

fn chroma(c: Color32) -> u8 {
    let m = [c.r(), c.g(), c.b()];
    m.iter().max().unwrap() - m.iter().min().unwrap()
}

fn is_orange(p: [u8; 4]) -> bool {
    p[0] > p[1] && p[1] >= p[2] && p[0] - p[2] > 30
}

#[test]
fn the_eskiz_chrome_is_neutral_graphite_and_flat() {
    for light in [false, true] {
        let pal = palette_for(UiIdentity::Eskiz, light);
        for (name, c) in [
            ("backdrop", pal.backdrop),
            ("bar", pal.bar),
            ("panel", pal.panel),
            ("row", pal.row),
            ("hover", pal.hover),
            ("separator", pal.separator),
            ("track", pal.track),
            ("edge", pal.edge),
            ("text", pal.text),
            ("text_dim", pal.text_dim),
            ("accent", pal.accent),
            ("accent_fill", pal.accent_fill),
            ("on_accent", pal.on_accent),
            ("focus_ring", pal.focus_ring),
        ] {
            assert!(chroma(c) <= 14, "{name} is not neutral ({c:?}, light={light})");
        }
        // The one loud colour is the primary.
        assert!(is_orange([pal.primary.r(), pal.primary.g(), pal.primary.b(), 255]));
        assert_eq!(shadow(&pal), eframe::egui::Shadow::NONE, "flat: no panel shadow");
    }
    // Classic keeps one accent for everything and its shadow.
    let c = palette_for(UiIdentity::Classic, false);
    assert_eq!((c.primary, c.primary_text, c.on_primary), (c.accent_fill, c.accent, c.on_accent));
    assert_eq!(shadow(&c).blur, 24);
}

/// Colour on the Eskiz chrome: the sidebar (sliders, buttons) has none, the top bar has orange only
/// on the active tool, and the gallery header only on its one primary button.
#[test]
fn eskiz_spends_the_accent_only_on_the_active_tool_and_the_primary_button() {
    let mut h = canvas_app("minimal", Prefs { identity: UiIdentity::Eskiz, ..Prefs::default() });
    let img = h.render();
    let mut sidebar_colour = 0;
    for y in 160..620 {
        for x in 0..60 {
            let p = img.pixel(x, y);
            if chroma(Color32::from_rgb(p[0], p[1], p[2])) > 12 {
                sidebar_colour += 1;
            }
        }
    }
    assert_eq!(sidebar_colour, 0, "the side bar (sliders, undo, redo) is neutral");
    let (mut orange, mut other) = (0, 0);
    for y in 0..46 {
        // Left of the colour circle, which shows the painting colour.
        for x in 0..(1280 - 80) {
            let p = img.pixel(x, y);
            if chroma(Color32::from_rgb(p[0], p[1], p[2])) > 40 {
                if is_orange(p) {
                    orange += 1;
                } else {
                    other += 1;
                }
            }
        }
    }
    assert!(orange > 20, "the active tool glyph is orange");
    assert_eq!(other, 0, "no other colour in the top bar");

    // The gallery: only the "+" button is coloured in the header.
    let ctx = h.ctx.clone();
    h.app.open_gallery(&ctx);
    h.advance(0.6);
    h.settle_storage();
    let img = h.render();
    let (mut orange, mut other) = (0, 0);
    for y in 0..46 {
        for x in 0..1280 {
            let p = img.pixel(x, y);
            if chroma(Color32::from_rgb(p[0], p[1], p[2])) > 40 {
                if is_orange(p) {
                    orange += 1;
                } else {
                    other += 1;
                }
            }
        }
    }
    assert!(orange > 100, "the + button is the accent");
    assert_eq!(other, 0, "no other colour in the gallery header");
}

#[test]
fn eskiz_popovers_have_no_accent_tints_or_coloured_slider_pills() {
    for panel in [Panel::Layers, Panel::Brush, Panel::Actions] {
        let mut h = canvas_app("pop", Prefs { identity: UiIdentity::Eskiz, ..Prefs::default() });
        h.open_panel(panel);
        let body = h.app.popovers.body_rect().unwrap();
        let img = h.render();
        // Orange only for an active tab or tool: count its pixels; nothing blue-tinted either way.
        let mut orange = 0;
        for y in body.top() as usize..body.bottom() as usize {
            for x in body.left() as usize..body.right() as usize {
                if is_orange(img.pixel(x, y)) {
                    orange += 1;
                }
            }
        }
        // The panel surface itself (its corners, clear of the rounded edge) is neutral graphite.
        for (x, y) in [(body.left() + 6.0, body.top() + 6.0), (body.right() - 6.0, body.top() + 6.0), (body.left() + 6.0, body.bottom() - 6.0)] {
            let p = img.pixel(x as usize, y as usize);
            assert!(chroma(Color32::from_rgb(p[0], p[1], p[2])) <= 14, "{panel:?}: panel surface {p:?} is tinted");
        }
        assert!(orange < 1500, "{panel:?}: {orange} orange pixels, the accent is spent sparingly");
    }
}
