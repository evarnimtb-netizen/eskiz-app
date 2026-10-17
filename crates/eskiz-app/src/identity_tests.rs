//! The Eskiz identity behind `Prefs::identity`: live switching, the split sidebar pills, and
//! that Classic keeps its single capsule.

use eframe::egui::{vec2, Vec2};

use crate::app::Prefs;
use crate::document::Document;
use crate::snapshot::Headless;
use crate::theme::{self, UiIdentity};

const WINDOW: Vec2 = vec2(1280.0, 800.0);

fn window(identity: UiIdentity) -> Headless {
    let mut h = Headless::new(WINDOW, Document::new(1024, 768), Prefs { identity, ..Prefs::default() });
    h.advance(0.5);
    h
}

/// Runs of the panel colour down the left edge of the sidebar (x = 14, clear of the sliders).
fn panel_runs(h: &mut Headless, panel: [u8; 3]) -> usize {
    let img = h.render();
    let near = |p: [u8; 4]| (0..3).all(|i| (i32::from(p[i]) - i32::from(panel[i])).abs() <= 6);
    // Count runs of at least 10 pixels (the rounded ends anti-alias into short runs).
    let (mut runs, mut len) = (0, 0);
    for y in 80..721 {
        if y < 720 && near(img.pixel(14, y)) {
            len += 1;
        } else {
            runs += usize::from(len >= 10);
            len = 0;
        }
    }
    runs
}

#[test]
fn eskiz_sidebar_is_split_into_pills_and_classic_stays_one_capsule() {
    let classic = theme::palette_for(UiIdentity::Classic, false).panel;
    let eskiz = theme::palette_for(UiIdentity::Eskiz, false).panel;
    let mut c = window(UiIdentity::Classic);
    assert_eq!(panel_runs(&mut c, [classic.r(), classic.g(), classic.b()]), 1, "Classic: one capsule");
    let mut e = window(UiIdentity::Eskiz);
    assert!(panel_runs(&mut e, [eskiz.r(), eskiz.g(), eskiz.b()]) >= 4, "Eskiz: size pill, Pick Color, opacity pill, undo and redo");
}

#[test]
fn switching_the_identity_applies_palette_and_fonts_live() {
    let mut h = window(UiIdentity::Classic);
    assert_eq!(h.app.pal, theme::palette_for(UiIdentity::Classic, false));
    assert!(!theme::is_eskiz(&h.ctx));
    h.app.prefs.identity = UiIdentity::Eskiz;
    h.advance(0.3);
    assert_eq!(h.app.pal, theme::palette_for(UiIdentity::Eskiz, false));
    assert!(theme::is_eskiz(&h.ctx));
    h.app.prefs.identity = UiIdentity::Classic;
    h.advance(0.3);
    assert_eq!(h.app.pal, theme::palette_for(UiIdentity::Classic, false));
    assert!(!theme::is_eskiz(&h.ctx));
}

#[test]
fn eskiz_top_bar_uses_the_navy_chrome() {
    let mut h = window(UiIdentity::Eskiz);
    let p = h.render().pixel(640, 6);
    // Bar #121829 at 92% over the #242424 surround.
    assert!(p[2] > p[0] && p[0] < 0x20, "navy bar, got {p:?}");
}

/// Switching the look at any moment must not panic: fonts installed with `set_fonts` only
/// take effect next frame, so no text may name a family the current frame lacks.
#[test]
fn switching_the_look_with_panels_open_never_panics() {
    use crate::popover::Panel;
    for start in UiIdentity::ALL {
        let mut h = window(start);
        for panel in [Panel::Actions, Panel::Layers, Panel::Brush, Panel::Colour] {
            h.open_panel(panel);
            for _ in 0..3 {
                let next = if h.app.prefs.identity == UiIdentity::Eskiz { UiIdentity::Classic } else { UiIdentity::Eskiz };
                h.app.prefs.identity = next;
                for _ in 0..4 {
                    h.step();
                }
                assert_eq!(theme::is_eskiz(&h.ctx), next == UiIdentity::Eskiz);
            }
        }
        // Light and identity changing in the same frame.
        h.app.prefs.light = !h.app.prefs.light;
        h.app.prefs.identity = UiIdentity::Eskiz;
        h.advance(0.2);
    }
}

#[test]
fn starting_with_the_eskiz_look_never_panics_on_first_frame() {
    let mut h = Headless::new(WINDOW, Document::new(64, 64), Prefs { identity: UiIdentity::Eskiz, ..Prefs::default() });
    h.open_panel(crate::popover::Panel::Actions);
    h.advance(0.3);
}
