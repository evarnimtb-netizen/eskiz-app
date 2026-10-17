//! Regression tests for the 0.1.0 release-candidate smoke test (mouse strokes that vanished).

use eframe::egui::{vec2, Pos2, Vec2};

use crate::app::Prefs;
use crate::document::Document;
use crate::gesture::Gesture;
use crate::snapshot::Headless;

const WINDOW: Vec2 = vec2(1280.0, 800.0);

fn app() -> Headless {
    let prefs = Prefs { eyedrop_hold: true, hold_shape: true, ..Prefs::default() };
    let mut h = Headless::new(WINDOW, Document::new(1024, 768), prefs);
    h.advance(0.5);
    h
}

fn stroke_to(h: &mut Headless, from: Pos2, pts: &[Pos2]) {
    h.move_to(from);
    h.step();
    h.press(from, true);
    h.step();
    for &p in pts {
        h.move_to(p);
        h.step();
    }
}

fn wave(c: Pos2) -> Vec<Pos2> {
    (1..=40).map(|i| c + vec2(i as f32 * 6.0, (i as f32 * 0.5).sin() * 40.0)).collect()
}

/// A mouse stroke that pauses (both "hold" features on) is never lost: it stays a stroke or
/// becomes a shape, and either way one undo step of ink remains after release.
#[test]
fn a_mouse_stroke_that_pauses_is_never_lost() {
    for secs in [0.5, 1.0, 2.5] {
        let mut h = app();
        let before = h.app.session.undo_steps();
        let c = h.app.viewport.center() - vec2(120.0, 0.0);
        let pts = wave(c);
        stroke_to(&mut h, c, &pts);
        h.advance(secs);
        assert!(h.app.eyedrop.is_none(), "a pause mid-stroke never starts the eyedropper");
        h.press(*pts.last().unwrap(), false);
        h.advance(0.3);
        assert_eq!(h.app.session.undo_steps(), before + 1, "the stroke survived a {secs} s pause");
    }
}

/// The one-finger hold gesture does not cancel a stroke that already moved, but does start
/// the eyedropper when the press has been still since the start.
#[test]
fn eyedropper_hold_never_cancels_a_stroke_that_moved() {
    let mut h = app();
    let c = h.app.viewport.center();
    let ctx = h.ctx.clone();
    let pts: Vec<Pos2> = (1..=8).map(|i| c + vec2(i as f32 * 10.0, 0.0)).collect();
    stroke_to(&mut h, c, &pts);
    assert!(h.app.session.is_stroking());
    h.app.on_gesture(&ctx, Gesture::EyedropperHold { pos: c }, 1.0);
    h.step();
    assert!(h.app.eyedrop.is_none() && h.app.session.is_stroking(), "the moved stroke continues");
    h.press(*pts.last().unwrap(), false);
    h.advance(0.3);
    assert_eq!(h.app.session.undo_steps(), 1);

    // Held still from the start: the eyedropper takes over.
    let mut h = app();
    let c = h.app.viewport.center();
    let ctx = h.ctx.clone();
    stroke_to(&mut h, c, &[]);
    assert!(!h.app.stroke_has_moved());
    h.app.on_gesture(&ctx, Gesture::EyedropperHold { pos: c }, 1.0);
    assert!(h.app.eyedrop.is_some());
}


/// Every Actions tab in both looks, snapshotted. On a desktop the popover rows are compact;
/// the Settings title no longer runs under its switch and the subtitle is not cut off.
#[test]
fn actions_popover_is_compact_on_desktop_in_both_looks() {
    use crate::panels::actions::ActionsTab;
    use crate::popover::Panel;
    use crate::theme::UiIdentity;
    let dir = crate::snapshot::snapshot_dir().join("rc1");
    std::fs::create_dir_all(&dir).unwrap();
    for identity in UiIdentity::ALL {
        let mut h = Headless::new(vec2(1280.0, 860.0), Document::new(1024, 768), Prefs { identity, ..Prefs::default() });
        h.advance(0.5);
        h.open_panel(Panel::Actions);
        for tab in ActionsTab::ALL {
            h.app.actions_ui.tab = tab;
            h.advance(0.2);
            h.render().save_png(&dir.join(format!("actions_{}_{tab:?}.png", identity.key()))).unwrap();
        }
        assert!(crate::panels::rows::is_compact(&h.ctx));
    }
}

/// A row title that is too long for its switch is shortened, and a long subtitle wraps to a
/// second line instead of being cut off.
#[test]
fn long_row_text_stays_clear_of_the_switch() {
    use crate::panels::rows::{list_row, Trailing};
    let pal = crate::theme::palette(false);
    let mut heights = Vec::new();
    for (label, sub) in [("Short", None), ("Show welcome again on next launch", Some("The tour opens once, then switches itself off"))] {
        let mut h = 0.0;
        let img = crate::snapshot::render_ui(vec2(300.0, 100.0), |ui| {
            h = list_row(ui, &pal, crate::icons_actions::icon_help, label, sub, true, Trailing::Switch(true)).rect.height();
        });
        heights.push(h);
        if sub.is_some() {
            let name = crate::snapshot::snapshot_dir().join("rc1").join("long_row.png");
            std::fs::create_dir_all(name.parent().unwrap()).unwrap();
            img.save_png(&name).unwrap();
        }
    }
    assert!(heights[1] > heights[0], "the wrapped subtitle makes the row taller");
}
