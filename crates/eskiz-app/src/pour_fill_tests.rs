//! Gesture and snapshot tests for Pour Fill, driven through the headless app.

use eframe::egui::{Pos2, Vec2};
use eskiz_core::brushes::{Brush, Preset};

use crate::app::Prefs;
use crate::pour_fill::DropPhase;
use crate::document::Document;
use crate::input::{sample_from, PointerKind, RawPointer};
use crate::paint::StrokeSettings;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless};

/// A window with a closed circle (and a closed square inside it) drawn on layer 0.
fn scene(light: bool) -> Headless {
    let prefs = Prefs { light, ..Prefs::default() };
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(1600, 1000), prefs);
    let mut brush = Brush::preset(Preset::TechnicalPen);
    brush.size = 9.0;
    let settings = StrokeSettings { brush, colour: [0.1, 0.1, 0.12], erase: false, raster: None };
    // The pen tapers at both ends, so each shape overshoots its start to close properly.
    let ring: Vec<(f64, f64)> = (0..=260)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / 240.0;
            (800.0 + 260.0 * a.cos(), 500.0 + 260.0 * a.sin())
        })
        .collect();
    let corners = [(800.0, 420.0), (880.0, 420.0), (880.0, 580.0), (720.0, 580.0), (720.0, 420.0), (840.0, 420.0)];
    let mut square = Vec::new();
    for w in corners.windows(2) {
        for k in 0..20 {
            let t = f64::from(k) / 20.0;
            square.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
        }
    }
    square.push(corners[5]);
    for pts in [ring, square] {
        assert!(h.app.session.begin_stroke(h.app.session.doc.layer_id(0).unwrap(), &settings));
        for (i, (x, y)) in pts.into_iter().enumerate() {
            let raw = RawPointer { x, y, time: i as f64 / 120.0, kind: PointerKind::Mouse, force: None };
            h.app.session.push(sample_from(raw));
        }
        h.app.session.end_stroke();
    }
    h.app.needs_full = true;
    h.app.colour = [226, 84, 60];
    h.advance(0.1);
    h
}

fn screen(h: &Headless, x: f64, y: f64) -> Pos2 {
    h.app.view.canvas_to_screen(h.app.viewport, x, y)
}

/// Drag from the colour circle to `to`, in steps, leaving the button held.
fn drag_to(h: &mut Headless, to: Pos2) {
    let from = h.button_rect(Panel::Colour).unwrap().center();
    h.move_to(from);
    h.press(from, true);
    h.step();
    for i in 1..=12 {
        h.move_to(from + (to - from) * (i as f32 / 12.0));
        h.step();
    }
}

/// Step frames until the worker thread has delivered every requested preview.
fn settle(h: &mut Headless) {
    let start = std::time::Instant::now();
    loop {
        h.step();
        if !h.app.pour_previewer.busy() {
            return;
        }
        assert!(start.elapsed().as_secs() < 30, "the Pour Fill worker never finished");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn pixel(h: &Headless, x: usize, y: usize) -> [u8; 4] {
    h.app.session.doc.layer_at(0).unwrap().pixel(x, y)
}

#[test]
fn dwell_then_horizontal_drag_sets_the_threshold_and_release_commits() {
    let mut h = scene(false);
    let target = screen(&h, 1000.0, 500.0); // inside the circle, outside the square
    drag_to(&mut h, target);
    assert!(matches!(h.app.pour_fill.map(|s| s.phase), Some(DropPhase::Carry { .. })));
    h.advance(0.5);
    match h.app.pour_fill.map(|s| s.phase) {
        Some(DropPhase::Dropped { percent, seed, .. }) => {
            assert_eq!(percent, 50);
            assert_eq!(seed, (1000, 500));
        }
        other => panic!("expected a preview, got {other:?}"),
    }
    // The preview is computed on a worker thread; once it has arrived it is on the layer,
    // but nothing is in the undo history yet.
    settle(&mut h);
    assert_eq!(pixel(&h, 1000, 500), [226, 84, 60, 255]);
    assert_eq!(h.app.session.undo_steps(), 2);
    h.render().save_png(&snapshot_dir().join("pour_fill_preview_dark.png")).unwrap();

    // 40 points to the right is +20%.
    h.move_to(target + Vec2::new(40.0, 0.0));
    h.step();
    match h.app.pour_fill.map(|s| s.phase) {
        Some(DropPhase::Dropped { percent, .. }) => assert_eq!(percent, 70),
        other => panic!("{other:?}"),
    }
    settle(&mut h);
    h.render().save_png(&snapshot_dir().join("pour_fill_threshold_dark.png")).unwrap();

    h.press(target + Vec2::new(40.0, 0.0), false);
    h.step();
    assert!(h.app.pour_fill.is_none());
    assert_eq!(h.app.session.undo_steps(), 3);
    assert_eq!(h.app.session.undo_name(), Some("Pour Fill"));
    // Inside the circle is red; the square's inside and the outside are not.
    assert_eq!(pixel(&h, 1000, 500), [226, 84, 60, 255]);
    assert_eq!(pixel(&h, 800, 500)[3], 0, "inside the inner square must stay empty");
    assert_eq!(pixel(&h, 100, 100)[3], 0, "outside the circle must stay empty");
    assert_eq!(h.app.colour_store.history[0], [226, 84, 60]);
    h.advance(0.3);
    h.render().save_png(&snapshot_dir().join("pour_fill_done_dark.png")).unwrap();

    // One undo takes the fill away.
    h.app.undo_action();
    assert_eq!(pixel(&h, 1000, 500)[3], 0);
    assert_eq!(h.app.session.undo_steps(), 2);
}

#[test]
fn releasing_without_a_pause_fills_at_the_default_threshold() {
    let mut h = scene(true);
    let target = screen(&h, 1000.0, 500.0);
    drag_to(&mut h, target);
    h.press(target, false);
    h.step();
    assert_eq!(pixel(&h, 1000, 500), [226, 84, 60, 255]);
    assert_eq!(h.app.session.undo_name(), Some("Pour Fill"));
    h.advance(0.3);
    h.render().save_png(&snapshot_dir().join("pour_fill_done_light.png")).unwrap();
}

#[test]
fn releasing_off_the_canvas_cancels() {
    let mut h = scene(false);
    let before = h.app.session.flatten();
    // Back over the top bar.
    let bar = h.button_rect(Panel::Colour).unwrap().center() - Vec2::new(200.0, 0.0);
    drag_to(&mut h, bar);
    h.advance(0.5);
    h.press(bar, false);
    h.step();
    assert!(h.app.pour_fill.is_none());
    assert_eq!(h.app.session.flatten(), before);
    assert_eq!(h.app.session.undo_steps(), 2);
    // A click on the colour circle still opens the panel.
    h.open_panel(Panel::Colour);
    assert!(h.app.popovers.is_open());
}

/// The drop point and the filled region follow a rotated view (0, 33, 90 and 180 degrees).
#[test]
fn pour_fill_drops_on_the_canvas_pixel_under_a_rotated_view() {
    for deg in [0.0f32, 33.0, 90.0, 180.0] {
        let mut h = scene(false);
        let vp = h.app.viewport;
        h.app.view.rotate_at(vp, vp.center(), deg.to_radians());
        h.advance(0.1);
        // Inside the circle, outside the square.
        let target = screen(&h, 1000.0, 500.0);
        drag_to(&mut h, target);
        h.advance(0.5);
        match h.app.pour_fill.map(|s| s.phase) {
            Some(DropPhase::Dropped { seed, .. }) => assert_eq!(seed, (1000, 500), "{deg}"),
            other => panic!("{deg}: expected a preview, got {other:?}"),
        }
        settle(&mut h);
        h.press(target, false);
        h.step();
        assert_eq!(pixel(&h, 1000, 500), [226, 84, 60, 255], "{deg}");
        assert_eq!(pixel(&h, 800, 500)[3], 0, "{deg}: the inner square stays empty");
        assert_eq!(pixel(&h, 100, 100)[3], 0, "{deg}: outside the circle stays empty");
    }
}
