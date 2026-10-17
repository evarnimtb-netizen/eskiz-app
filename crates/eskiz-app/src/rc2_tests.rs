//! Regression tests for the RC2 macOS smoke test: popover width, scroll leaking to the canvas.

use eframe::egui::{self, vec2, Event, Modifiers, Pos2, Rect, Vec2};

use crate::app::Prefs;
use crate::document::Document;
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::snapshot::Headless;
use crate::theme::UiIdentity;

fn app_with(size: Vec2, identity: UiIdentity) -> Headless {
    let mut h = Headless::new(size, Document::new(1024, 768), Prefs { identity, ..Prefs::default() });
    h.advance(0.5);
    h
}

fn view_of(h: &Headless) -> (f32, Vec2, f32) {
    (h.app.view.zoom, h.app.view.offset, h.app.view.rotation)
}

fn pinch_at(h: &mut Headless, pos: Pos2, factor: f32) {
    h.move_to(pos);
    h.step();
    h.event(Event::Zoom(factor));
    h.step();
    h.advance(0.3);
}

const PANELS: [Panel; 5] = [Panel::Actions, Panel::Adjustments, Panel::Brush, Panel::Layers, Panel::Colour];

/// Scroll, pinch and ctrl-wheel over any popover never reach the canvas; the same gestures on the
/// bare canvas still pan and zoom it.
#[test]
fn scroll_and_pinch_over_a_popover_never_move_the_canvas() {
    for panel in PANELS {
        let mut h = app_with(vec2(1280.0, 860.0), UiIdentity::Classic);
        h.open_panel(panel);
        h.app.actions_ui.tab = ActionsTab::Prefs;
        h.advance(0.2);
        let body = h.app.popovers.body_rect().expect("popover is open");
        let inside = body.center();
        let before = view_of(&h);
        h.scroll_at(inside, -120.0);
        h.scroll_at(inside, 120.0);
        pinch_at(&mut h, inside, 1.5);
        h.move_to(inside);
        h.step();
        h.event(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, 80.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::COMMAND });
        h.advance(0.4);
        assert_eq!(view_of(&h), before, "{panel:?}: the canvas moved while the pointer was over the popover");
    }
    // Control: the bare canvas, with nothing open, still scrolls and zooms.
    let mut h = app_with(vec2(1280.0, 860.0), UiIdentity::Classic);
    let spot = h.app.viewport.center();
    let before = view_of(&h);
    h.scroll_at(spot, -120.0);
    assert_ne!(view_of(&h).1, before.1, "the canvas pans under the wheel");
    let mid = view_of(&h);
    pinch_at(&mut h, spot, 1.5);
    assert_ne!(view_of(&h).0, mid.0, "the canvas zooms under a pinch");
}

/// The Actions Settings tab is long: scrolling over it scrolls the popover, not the canvas.
#[test]
fn actions_settings_scrolls_itself_and_leaves_the_canvas_alone() {
    let mut h = app_with(vec2(1280.0, 860.0), UiIdentity::Classic);
    h.open_panel(Panel::Actions);
    h.app.actions_ui.tab = ActionsTab::Prefs;
    h.advance(0.3);
    let body = h.app.popovers.body_rect().unwrap();
    let before_img = h.render();
    let before = view_of(&h);
    h.scroll_at(body.center(), -240.0);
    let after_img = h.render();
    assert_eq!(view_of(&h), before, "canvas view transform unchanged");
    assert!(h.app.popovers.is_open(), "the popover stays open");
    let r = body.shrink(6.0);
    let differs = (r.top() as usize..r.bottom() as usize).any(|y| {
        (r.left() as usize..r.right() as usize).any(|x| before_img.pixel(x, y) != after_img.pixel(x, y))
    });
    assert!(differs, "the popover content scrolled");
}

/// The side bar, a sheet or any other area over the canvas keeps its scroll too.
#[test]
fn scroll_over_the_sidebar_is_not_a_canvas_scroll() {
    let mut h = app_with(vec2(1280.0, 860.0), UiIdentity::Classic);
    let spot = Pos2::new(14.0, h.size.y / 2.0);
    assert!(crate::app::over_ui(&h.ctx, spot), "the side bar is at the left edge");
    let before = view_of(&h);
    h.scroll_at(spot, -150.0);
    pinch_at(&mut h, spot, 1.4);
    assert_eq!(view_of(&h), before);
    let _ = Rect::NOTHING;
}

/// Every popover, every Actions tab, both looks: the body is as wide as its content (the
/// panel's nominal width), never wider, and always inside the window.
#[test]
fn popovers_are_sized_to_their_content_in_both_looks() {
    let dir = crate::snapshot::snapshot_dir().join("rc2");
    std::fs::create_dir_all(&dir).unwrap();
    for identity in UiIdentity::ALL {
        for panel in PANELS {
            let tabs: Vec<ActionsTab> = if panel == Panel::Actions { ActionsTab::ALL.to_vec() } else { vec![ActionsTab::Add] };
            let mut h = app_with(vec2(1280.0, 860.0), identity);
            h.open_panel(panel);
            for tab in tabs {
                h.app.actions_ui.tab = tab;
                h.advance(0.3);
                let body = h.app.popovers.body_rect().unwrap();
                let name = if panel == Panel::Actions { format!("{panel:?}_{tab:?}") } else { format!("{panel:?}") };
                h.render().save_png(&dir.join(format!("{}_{name}.png", identity.key()))).unwrap();
                let nominal = panel.width();
                // Frame stroke adds two points to the nominal width.
                assert!(body.width() <= nominal + 2.5, "{} {name}: {} wide, nominal {nominal}", identity.key(), body.width());
                assert!(body.width() >= 300.0, "{} {name}: only {} wide", identity.key(), body.width());
                assert!(body.left() >= 0.0 && body.right() <= h.size.x);
            }
        }
    }
}

/// The cause of the over-wide popover on macOS: a long pen/tablet status message in Settings was
/// laid out on one line and widened the whole panel. It wraps, and nothing can widen a panel.
#[test]
fn a_long_status_line_in_settings_does_not_widen_the_actions_popover() {
    let dir = crate::snapshot::snapshot_dir().join("rc2");
    std::fs::create_dir_all(&dir).unwrap();
    for identity in UiIdentity::ALL {
        let mut h = app_with(vec2(1280.0, 860.0), identity);
        h.app.pen.status = "Tablet: the macOS pen backend could not start, trackpad force touch and mouse input are used instead".to_owned();
        h.open_panel(Panel::Actions);
        h.app.actions_ui.tab = ActionsTab::Prefs;
        // Scroll to the bottom, where the pressure curve and its status line are.
        h.advance(0.3);
        let body = h.app.popovers.body_rect().unwrap();
        h.scroll_at(body.center(), -2000.0);
        assert!(body.width() <= Panel::Actions.width() + 2.5, "{} wide", body.width());
        h.render().save_png(&dir.join(format!("{}_Prefs_long_status.png", identity.key()))).unwrap();
    }
}

/// A narrow window never gets a popover wider than the window minus its margins.
#[test]
fn a_popover_never_exceeds_a_narrow_window() {
    let mut h = app_with(vec2(400.0, 700.0), UiIdentity::Classic);
    h.open_panel(Panel::Brush);
    let body = h.app.popovers.body_rect().unwrap();
    assert!(body.left() >= 0.0 && body.right() <= 400.0, "{body:?}");
}
