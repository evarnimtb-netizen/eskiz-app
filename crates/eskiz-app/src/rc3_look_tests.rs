//! RC3 look snapshots: the canvas, gallery and popovers in both looks, written to
//! `target/snapshots/rc3/<tag>_<look>_<what>.png`. `ESKIZ_SNAP_TAG` (default `after`) names the
//! set, so the same test run on the previous commit gives the `before` set. Written against
//! API that exists on both sides of the RC3 change.

use eframe::egui::{vec2, Vec2};

use crate::app::{EskizApp, Prefs};
use crate::gallery::Screen;
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless};
use crate::storage::{self, Library};
use crate::theme::UiIdentity;

const WINDOW: Vec2 = vec2(1280.0, 860.0);

fn tag() -> String {
    std::env::var("ESKIZ_SNAP_TAG").unwrap_or_else(|_| "after".to_owned())
}

fn out(identity: UiIdentity, what: &str) -> std::path::PathBuf {
    let dir = snapshot_dir().join("rc3");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}_{}_{what}.png", tag(), identity.key()))
}

fn library(name: &str, n: usize) -> Library {
    let dir = std::env::temp_dir().join(format!("eskiz-rc3look-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = Library::at(&dir);
    for i in 0..n {
        let id = storage::new_id();
        let mut doc = storage::sample_document(400 + 40 * i, 300);
        doc.background = [1.0, 1.0, 1.0];
        lib.save(&id, ["Harbour at dusk", "Study in green", "Untitled"][i % 3], &doc).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    lib
}

fn gallery_app(identity: UiIdentity, name: &str) -> Headless {
    let lib = library(name, 3);
    let mut h = Headless::from_app(WINDOW, move |ctx| {
        EskizApp::with_library(ctx, Prefs { identity, ..Prefs::default() }, lib)
    });
    h.settle_storage();
    h.advance(0.4);
    h
}

fn canvas_app(identity: UiIdentity, name: &str) -> Headless {
    let mut h = gallery_app(identity, name);
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    h.advance(0.4);
    h.settle_storage();
    assert!(matches!(h.app.gal.screen, Screen::Canvas));
    h.advance(0.4);
    h
}

#[test]
fn look_snapshots_gallery_and_canvas() {
    for identity in UiIdentity::ALL {
        let mut h = gallery_app(identity, "g");
        h.render().save_png(&out(identity, "gallery")).unwrap();
        let mut h = canvas_app(identity, "c");
        // A stroke, so the canvas shows ink and the sidebar sliders are at work.
        let c = h.app.viewport.center();
        h.move_to(c);
        h.step();
        h.press(c, true);
        h.step();
        for i in 1..=30 {
            h.move_to(c + vec2(i as f32 * 6.0, (i as f32 * 0.4).sin() * 40.0));
            h.step();
        }
        h.press(c + vec2(180.0, 0.0), false);
        h.advance(0.3);
        h.render().save_png(&out(identity, "canvas")).unwrap();
    }
}

#[test]
fn look_snapshots_popovers() {
    for identity in UiIdentity::ALL {
        for (panel, name) in [(Panel::Actions, "actions-settings"), (Panel::Brush, "brush"), (Panel::Layers, "layers"), (Panel::Colour, "colour")] {
            let mut h = canvas_app(identity, "p");
            h.open_panel(panel);
            if panel == Panel::Actions {
                h.app.actions_ui.tab = ActionsTab::Prefs;
                h.advance(0.3);
            }
            h.render().save_png(&out(identity, name)).unwrap();
        }
    }
}
