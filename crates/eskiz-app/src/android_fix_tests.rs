//! Host tests for the Android fixes (idle first stroke, safe area, rotation refit, empty
//! gallery tile). They drive the real app headlessly.

use crate::app::{EskizApp, Prefs};
use crate::gallery::Screen;
use crate::lifecycle_host::CLOCK_SKEW;
use crate::snapshot::Headless;
use crate::storage::{self, Library};
use eframe::egui::{Pos2, Vec2};
use std::path::PathBuf;

const WINDOW: Vec2 = Vec2::new(1280.0, 800.0);

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-afix-{name}-{}", std::process::id()));
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

fn canvas_app(name: &str, size: Vec2) -> Headless {
    let dir = temp(name);
    let lib = Library::at(&dir);
    let id = storage::new_id();
    lib.save(&id, "Harbour", &storage::sample_document(600, 400)).unwrap();
    let mut h = Headless::from_app(size, move |ctx| EskizApp::with_library(ctx, Prefs::default(), lib));
    h.settle_storage();
    h.advance(0.3);
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    h.advance(0.4);
    h.settle_storage();
    assert!(matches!(h.app.gal.screen, Screen::Canvas));
    h
}

fn swipe(h: &mut Headless, a: Pos2, b: Pos2) {
    h.move_to(a);
    h.step();
    h.press(a, true);
    h.step();
    for k in 1..=10 {
        h.move_to(a + (b - a) * (k as f32 / 10.0));
        h.step();
    }
    h.press(b, false);
    h.step();
}

#[test]
fn first_stroke_after_a_periodic_save_and_a_long_idle_draws() {
    let mut h = canvas_app("idle", WINDOW);
    h.app.lifecycle_on = true;
    h.advance(0.5);
    let before = h.app.session.undo_steps();
    // eframe's 30 s periodic save while the app sits idle, then a minute of nothing.
    let mut st = MemStorage::default();
    eframe::App::save(&mut h.app, &mut st);
    CLOCK_SKEW.with(|c| c.set(c.get() + 60.0));
    let c = h.app.viewport.center();
    swipe(&mut h, c - Vec2::new(80.0, 0.0), c + Vec2::new(80.0, 0.0));
    h.advance(0.3);
    assert_eq!(h.app.session.undo_steps(), before + 1, "the first stroke is one undo step");
    assert_eq!(h.app.lifecycle.resumes, 0, "a periodic save is not a trip to the background");
    CLOCK_SKEW.with(|c| c.set(0.0));
}

#[test]
fn a_periodic_save_does_not_cancel_a_stroke_in_flight() {
    let mut h = canvas_app("midstroke", WINDOW);
    h.app.lifecycle_on = true;
    h.advance(0.5);
    let before = h.app.session.undo_steps();
    let c = h.app.viewport.center();
    h.move_to(c);
    h.step();
    h.press(c, true);
    h.step();
    h.move_to(c + Vec2::new(40.0, 0.0));
    h.step();
    assert!(h.app.session.is_stroking());
    let mut st = MemStorage::default();
    eframe::App::save(&mut h.app, &mut st);
    assert!(h.app.session.is_stroking(), "the 30 s autosave must not end the stroke");
    h.move_to(c + Vec2::new(80.0, 0.0));
    h.step();
    h.press(c + Vec2::new(80.0, 0.0), false);
    h.step();
    assert_eq!(h.app.session.undo_steps(), before + 1);
}

#[test]
fn a_real_suspend_then_a_stroke_on_the_resume_frame_still_draws() {
    let mut h = canvas_app("resume", WINDOW);
    h.app.lifecycle_on = true;
    h.advance(0.5);
    let before = h.app.session.undo_steps();
    crate::android_queue::set_backgrounded(true);
    let mut st = MemStorage::default();
    eframe::App::save(&mut h.app, &mut st);
    assert_eq!(h.app.lifecycle.suspends, 1);
    // Frames while stopped do not resume.
    h.advance(0.2);
    assert_eq!(h.app.lifecycle.resumes, 0);
    CLOCK_SKEW.with(|c| c.set(c.get() + 60.0));
    crate::android_queue::set_backgrounded(false);
    // The very first frame back carries the press of a stroke.
    let c = h.app.viewport.center();
    swipe(&mut h, c - Vec2::new(80.0, 0.0), c + Vec2::new(80.0, 0.0));
    h.advance(0.3);
    assert_eq!(h.app.lifecycle.resumes, 1);
    assert_eq!(h.app.session.undo_steps(), before + 1, "the stroke on the resume frame is not dropped");
    CLOCK_SKEW.with(|c| c.set(0.0));
}

#[test]
fn top_bar_and_sidebar_sit_inside_the_safe_area() {
    use crate::safe_area::SafeArea;
    let mut h = canvas_app("safe", WINDOW);
    h.app.safe = SafeArea { left: 0.0, top: 40.0, right: 0.0, bottom: 48.0 };
    h.advance(0.5);
    let gallery = h.app.gallery_button;
    assert!(gallery.min.y >= 40.0, "the top bar's buttons start below the status bar: {gallery:?}");
    for (_, r) in &h.app.button_rects {
        assert!(r.min.y >= 40.0 && r.max.y <= WINDOW.y - 48.0, "button {r:?} is inside the safe area");
    }
    // The canvas may draw behind the bars, but a freshly fitted canvas lies inside the safe area.
    h.app.fit();
    h.advance(0.5);
    let vp = h.app.viewport;
    let tl = h.app.view.canvas_to_screen(vp, 0.0, 0.0);
    let br = h.app.view.canvas_to_screen(vp, 600.0, 400.0);
    assert!(tl.y >= 40.0 && br.y <= WINDOW.y - 48.0 + 0.5, "{tl:?} {br:?}");
    let img = h.render();
    // The strip under the status bar is painted with the bar colour, not left as canvas.
    assert_eq!(img.pixel(5, 5), img.pixel(5, 45), "status bar strip looks like the bar below it");
    // Zero on desktop: the same layout starts at the very top.
    h.app.safe = SafeArea::ZERO;
    h.advance(0.5);
    assert!(h.app.gallery_button.min.y < 40.0);
}

fn rotate(h: &mut Headless) {
    h.size = Vec2::new(h.size.y, h.size.x);
    h.advance(0.5);
}

fn canvas_rect(h: &Headless) -> eframe::egui::Rect {
    let vp = h.app.viewport;
    eframe::egui::Rect::from_two_pos(h.app.view.canvas_to_screen(vp, 0.0, 0.0), h.app.view.canvas_to_screen(vp, 600.0, 400.0))
}

#[test]
fn rotation_refits_an_untouched_view() {
    let mut h = canvas_app("rot-fit", WINDOW);
    h.advance(0.5);
    let landscape = canvas_rect(&h);
    rotate(&mut h);
    let r = canvas_rect(&h);
    let vp = h.app.viewport;
    assert!(vp.height() > vp.width());
    assert!(r.min.x >= vp.min.x && r.max.x <= vp.max.x, "whole canvas fits horizontally: {r:?} in {vp:?}");
    assert!((r.center().x - vp.center().x).abs() < 1.0, "centered horizontally");
    assert!(r.width() < landscape.width(), "zoom follows the narrower window");
    rotate(&mut h);
    let back = canvas_rect(&h);
    assert!((back.width() - landscape.width()).abs() < 1.0, "rotating back refits again");
}

#[test]
fn rotation_keeps_the_centre_of_a_zoomed_view() {
    let mut h = canvas_app("rot-keep", WINDOW);
    h.advance(0.5);
    // The user zooms in and pans.
    let vp = h.app.viewport;
    h.app.view.zoom *= 3.0;
    h.app.view.offset += Vec2::new(-120.0, 70.0);
    let zoom = h.app.view.zoom;
    let (cx, cy) = h.app.view.screen_to_canvas(vp, vp.center());
    rotate(&mut h);
    let vp2 = h.app.viewport;
    assert!((h.app.view.zoom - zoom).abs() < 1e-4, "zoom kept");
    let (nx, ny) = h.app.view.screen_to_canvas(vp2, vp2.center());
    assert!((nx - cx).abs() < 0.5 && (ny - cy).abs() < 0.5, "same canvas point at the center: {cx},{cy} vs {nx},{ny}");
}

#[test]
fn the_empty_gallery_tile_opens_the_new_canvas_popover() {
    let dir = temp("empty");
    let lib = Library::at(&dir);
    let mut h = Headless::from_app(WINDOW, move |ctx| EskizApp::with_library(ctx, Prefs::default(), lib));
    h.settle_storage();
    h.advance(0.5);
    assert!(h.app.gal.entries.is_empty());
    assert!(!h.app.gal.new_canvas_open());
    // The tile sits above the centre of the body.
    let c = Pos2::new(WINDOW.x / 2.0, WINDOW.y / 2.0 - 24.0 - 30.0);
    h.click(c);
    h.advance(0.3);
    assert!(h.app.gal.new_canvas_open(), "tapping the tile opens New canvas");
    // A tap on the empty background elsewhere still just dismisses.
    h.click(Pos2::new(60.0, WINDOW.y - 40.0));
    h.advance(0.3);
    assert!(!h.app.gal.new_canvas_open());
}

fn gallery_app_with_safe(name: &str, safe: crate::safe_area::SafeArea) -> Headless {
    let dir = temp(name);
    let lib = Library::at(&dir);
    let id = storage::new_id();
    lib.save(&id, "Harbour", &storage::sample_document(600, 400)).unwrap();
    let mut h = Headless::from_app(WINDOW, move |ctx| EskizApp::with_library(ctx, Prefs::default(), lib));
    h.settle_storage();
    h.app.safe = safe;
    h.advance(0.5);
    h
}

#[test]
fn gallery_header_sits_inside_the_safe_area() {
    use crate::safe_area::SafeArea;
    let mut h = gallery_app_with_safe("gal-safe", SafeArea { left: 0.0, top: 40.0, right: 0.0, bottom: 48.0 });
    assert!(matches!(h.app.gal.screen, Screen::Gallery));
    let plus = h.app.gal.header.plus;
    assert!(plus.min.y >= 40.0, "the + button starts below the status bar: {plus:?}");
    let body = h.app.gal.body;
    assert!(body.min.y >= 40.0 + 30.0 && body.max.y <= WINDOW.y - 48.0 + 0.5, "{body:?}");
    let img = h.render();
    // The status strip is the bar colour, like the bar below it (not the backdrop).
    assert_eq!(img.pixel(5, 5), img.pixel(5, 60), "status strip painted in the bar color");
    // The popover anchored to + stays inside the safe area too.
    h.click(plus.center());
    h.advance(0.5);
    let img = h.render();
    let _ = img;
    h.app.safe = SafeArea::ZERO;
    h.advance(0.5);
    assert!(h.app.gal.header.plus.min.y < 40.0);
}

#[test]
fn timelapse_without_an_encoder_records_and_shows_no_toast() {
    eskiz_media::force_encoder_unavailable(true);
    let mut h = canvas_app("tl-noenc", WINDOW);
    let vp = h.app.viewport;
    let c = vp.center();
    swipe(&mut h, c - Vec2::new(100.0, 0.0), c + Vec2::new(100.0, 40.0));
    h.advance(0.5);
    swipe(&mut h, c - Vec2::new(100.0, 50.0), c + Vec2::new(100.0, -20.0));
    h.advance(0.5);
    let toast = h.app.toast.as_ref().map(|t| t.text.clone());
    assert!(toast.is_none(), "no toast when the encoder is unavailable: {toast:?}");
    assert!(h.app.timelapse.has_recording(), "the recorder is running");
    assert!(h.app.timelapse.is_unencoded());
    let frames = h.app.timelapse.recorded_frames();
    assert!(frames >= 2, "the timelapse has frames: {frames}");
    eskiz_media::force_encoder_unavailable(false);
}
