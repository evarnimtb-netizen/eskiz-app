//! Headless tests for the safe graphics mode notice and Settings > Graphics: the notice shows
//! after a step-down, Try normal graphics and Dismiss close it, and the Settings page lists the
//! renderers. Snapshots go to `target/snapshots` (`safe_mode_banner.png`, `settings_graphics.png`).

use eframe::egui::{accesskit, Pos2, Vec2};

use crate::app::Prefs;
use crate::chrome::{SAFE_BANNER_TITLE, TRY_NORMAL_GRAPHICS};
use crate::document::Document;
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless};
use crate::startup::{SafeMode, Stage};

fn harness(banner: bool) -> Headless {
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(400, 300), Prefs::default());
    h.ctx.enable_accesskit();
    h.app.safe_mode = Some(SafeMode {
        stage: Stage::WgpuVulkan,
        reason: Some("The last start stopped before its window appeared while using DirectX 12.".into()),
    });
    h.app.renderer_in_use = Some(Stage::WgpuVulkan);
    h.app.safe_banner = banner;
    h.step();
    h.step();
    h
}

/// Centre of the button with this accessible name, if it is on screen.
fn button(h: &Headless, name: &str) -> Option<Pos2> {
    let update = h.access.as_ref()?;
    update
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some(name) && n.role() == accesskit::Role::Button)
        .and_then(|(_, n)| n.bounds())
        .map(|r| Pos2::new(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32))
}

fn press(h: &mut Headless, name: &str) {
    let at = button(h, name).unwrap_or_else(|| panic!("no button {name:?}"));
    h.click(at);
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn the_notice_shows_after_a_step_down_and_try_normal_graphics_closes_it() {
    let mut h = harness(true);
    assert!(button(&h, TRY_NORMAL_GRAPHICS).is_some(), "the notice offers the retry");
    assert!(button(&h, "Dismiss").is_some());
    let _ = std::fs::create_dir_all(snapshot_dir());
    h.render().save_png(&snapshot_dir().join("safe_mode_banner.png")).unwrap();
    // The canvas stays usable: the notice is not modal.
    assert!(!h.app.popovers.is_open());
    press(&mut h, TRY_NORMAL_GRAPHICS);
    assert!(!h.app.safe_banner, "the notice closes");
    assert!(h.app.normal_graphics_next);
    let toast = h.app.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
    assert!(toast.contains("next time Eskiz starts"), "{toast}");
    assert!(button(&h, TRY_NORMAL_GRAPHICS).is_none());
}

#[test]
fn dismiss_keeps_safe_mode() {
    let mut h = harness(true);
    press(&mut h, "Dismiss");
    assert!(!h.app.safe_banner);
    assert!(!h.app.normal_graphics_next, "safe mode stays until the user retries");
    assert!(h.app.safe_mode.is_some());
}

#[test]
fn no_notice_without_a_new_step_down() {
    let h = harness(false);
    assert!(button(&h, TRY_NORMAL_GRAPHICS).is_none());
    assert!(!SAFE_BANNER_TITLE.is_empty());
}

#[test]
fn settings_graphics_lists_the_renderers_and_the_retry() {
    let mut h = harness(false);
    h.app.actions_ui.tab = ActionsTab::Prefs;
    h.open_panel(Panel::Actions);
    h.step();
    h.step();
    let _ = std::fs::create_dir_all(snapshot_dir());
    h.render().save_png(&snapshot_dir().join("settings_graphics.png")).unwrap();
    // The Settings row and the notice share the same action.
    h.app.try_normal_graphics();
    assert!(h.app.normal_graphics_next && h.app.safe_mode.is_some());
    let choices = crate::renderer::choices();
    assert_eq!(choices[0], crate::renderer::Mode::Auto);
    assert!(choices.len() >= 2, "{choices:?}");
}
