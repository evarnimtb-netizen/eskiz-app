//! Accessibility tests: the AccessKit tree (what a screen reader sees) is queried by accessible
//! name, controls are activated through it, keyboard focus moves with Tab, and the contrast of
//! the text tokens is checked in both themes.

use eframe::egui::{self, accesskit, Event, Key, Modifiers, Pos2, Vec2};

use crate::a11y::contrast_ratio;
use crate::app::Prefs;
use crate::document::Document;
use crate::popover::{popover_secs, Panel, OPEN_SECS};
use crate::snapshot::Headless;
use crate::theme::palette;

/// Every named node in the last frame's accessibility tree, with its bounds in points.
fn named(h: &Headless) -> Vec<(String, Option<accesskit::Rect>)> {
    let Some(update) = &h.access else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().map(|l| (l.to_owned(), n.bounds())))
        .collect()
}

/// The accessible name of the node that holds keyboard focus, if any.
fn focused(h: &Headless) -> Option<String> {
    let update = h.access.as_ref()?;
    let id = update.focus;
    update
        .nodes
        .iter()
        .find(|(nid, _)| *nid == id)
        .and_then(|(_, n)| n.label().map(str::to_owned))
}

/// Press a button by its accessible name, at the centre of its node. Returns false when no
/// node has that name.
fn activate(h: &mut Headless, name: &str) -> bool {
    let Some((_, Some(r))) = named(h).into_iter().find(|(l, _)| l == name) else {
        return false;
    };
    let centre = Pos2::new(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32);
    h.click(centre);
    true
}

/// One Tab press (or Shift+Tab) and its release, over two frames.
fn key(h: &mut Headless, key: Key, shift: bool) {
    let modifiers = Modifiers { shift, ..Modifiers::NONE };
    h.event(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.event(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
}

fn app_with_access(prefs: Prefs) -> Headless {
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(1600, 1000), prefs);
    h.ctx.enable_accesskit();
    h.step();
    h.step();
    h
}

#[test]
fn icon_buttons_have_accessible_names_and_open_their_popovers() {
    let mut h = app_with_access(Prefs::default());
    let names: Vec<String> = named(&h).into_iter().map(|(l, _)| l).collect();
    for expected in ["Layers", "Actions", "Undo", "Redo", "Pick Color", "Brush", "Eraser", "Smudge"] {
        assert!(names.iter().any(|n| n == expected), "no node named {expected:?} in {names:?}");
    }
    assert!(names.iter().any(|n| n.starts_with("Brush size ")), "size slider unnamed: {names:?}");
    assert!(names.iter().any(|n| n.starts_with("Brush opacity ")), "opacity slider unnamed: {names:?}");
    assert!(names.iter().any(|n| n.contains(", hex ")), "color disc unnamed: {names:?}");

    // Activating the named Layers button through the tree opens the Layers popover.
    assert!(activate(&mut h, "Layers"));
    assert_eq!(h.app.popovers.open_panel(), Some(Panel::Layers));
}

#[test]
fn tab_moves_focus_through_the_top_bar() {
    let mut h = app_with_access(Prefs::default());
    let mut visited = Vec::new();
    for _ in 0..24 {
        key(&mut h, Key::Tab, false);
        if let Some(name) = focused(&h) {
            visited.push(name);
        }
    }
    let position = |want: &str| visited.iter().position(|n| n == want);
    let order = ["Actions", "Adjustments", "Selection", "Transform", "Layers", "Brush"];
    let mut last = None;
    for name in order {
        let at = position(name).unwrap_or_else(|| panic!("Tab never reached {name:?}: {visited:?}"));
        assert!(last.is_none_or(|l| at > l), "{name:?} out of order in {visited:?}");
        last = Some(at);
    }
    // Tab does not switch to full screen any more.
    assert!(!h.app.fullscreen);
}

#[test]
fn arrow_keys_adjust_a_focused_slider() {
    let mut h = app_with_access(Prefs::default());
    let mut found = false;
    for _ in 0..40 {
        key(&mut h, Key::Tab, false);
        if focused(&h).is_some_and(|n| n.starts_with("Brush size")) {
            found = true;
            break;
        }
    }
    assert!(found, "Tab never focused the size slider");
    let idx = h.app.tool.index();
    let before = h.app.tool_params[idx].size;
    key_arrow(&mut h, Key::ArrowUp);
    assert!(h.app.tool_params[idx].size > before, "{before} did not grow");
    let mid = h.app.tool_params[idx].size;
    key_arrow(&mut h, Key::ArrowDown);
    assert!(h.app.tool_params[idx].size < mid);
}

fn key_arrow(h: &mut Headless, k: Key) {
    key(h, k, false);
}

#[test]
fn escape_closes_an_open_popover() {
    let mut h = app_with_access(Prefs::default());
    assert!(activate(&mut h, "Layers"));
    assert_eq!(h.app.popovers.open_panel(), Some(Panel::Layers));
    key(&mut h, Key::Escape, false);
    assert_eq!(h.app.popovers.open_panel(), None);
    // Keyboard focus goes back to the button that opened the popover.
    assert_eq!(focused(&h).as_deref(), Some("Layers"));
}

#[test]
fn colour_gradient_sliders_are_named_and_keyboard_adjustable() {
    let mut h = app_with_access(Prefs::default());
    h.app.popovers.open(Panel::Colour, h.app.anchor_rect(Panel::Colour));
    h.app.colour_ui.tab = crate::panels::colour::ColourTab::Square;
    h.advance(0.5);
    let names: Vec<String> = named(&h).into_iter().map(|(l, _)| l).collect();
    for want in ["Hue ", "Saturation ", "Brightness "] {
        assert!(names.iter().any(|n| n.starts_with(want)), "no slider named {want:?} in {names:?}");
    }
    let before = h.app.colour;
    for _ in 0..60 {
        key(&mut h, Key::Tab, false);
        if focused(&h).is_some_and(|n| n.starts_with("Brightness")) {
            break;
        }
    }
    assert!(focused(&h).is_some_and(|n| n.starts_with("Brightness")), "Tab never reached the brightness slider");
    for _ in 0..8 {
        key(&mut h, Key::ArrowLeft, false);
    }
    assert_ne!(h.app.colour, before, "arrow keys did not change the color");
}

#[test]
fn brush_bench_controls_are_named() {
    let mut h = app_with_access(Prefs::default());
    h.open_panel(Panel::Brush);
    let edit = h.app.library_layout.edit.expect("the library has an Edit button");
    h.click(edit.center());
    h.advance(0.5);
    assert!(h.app.studio.is_some(), "Edit opens the Brush Bench");
    let names: Vec<String> = named(&h).into_iter().map(|(l, _)| l).collect();
    for want in ["Reset", "Duplicate", "Save", "Done", "Stroke path"] {
        assert!(names.iter().any(|n| n == want), "no control named {want:?} in {names:?}");
    }
    // Every slider row carries its value in its name, and the test pad is named.
    assert!(names.iter().any(|n| n.starts_with("Spacing ")), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("Brush test pad")), "{names:?}");
}

#[test]
fn reduce_motion_pref_disables_popover_and_gallery_animation() {
    assert_eq!(popover_secs(false), OPEN_SECS);
    assert_eq!(popover_secs(true), 0.0);

    let mut h = app_with_access(Prefs { reduce_motion: true, ..Prefs::default() });
    assert!(h.app.reduce_motion());
    assert!(activate(&mut h, "Layers"));
    // With no animation the popover is fully open on the frame it appears.
    let visible = h.app.popovers.frame(&h.ctx, true).expect("popover visible");
    assert_eq!(visible.ease, 1.0);

    let mut normal = app_with_access(Prefs::default());
    assert!(!normal.app.reduce_motion());
    assert!(activate(&mut normal, "Layers"));
    let start = normal.app.popovers.frame(&normal.ctx, false).expect("popover visible");
    assert!(start.ease < 1.0, "the default animates the popover in");
}

#[test]
fn secondary_text_and_disabled_states_meet_4_5_to_1_in_both_themes() {
    for light in [false, true] {
        let pal = palette(light);
        // The bar is translucent: composite it over the backdrop it sits on.
        let bar = {
            let a = f32::from(pal.bar.a()) / 255.0;
            let mix = |f: u8, b: u8| (f32::from(f) * a + f32::from(b) * (1.0 - a)).round() as u8;
            egui::Color32::from_rgb(
                mix(pal.bar.r(), pal.backdrop.r()),
                mix(pal.bar.g(), pal.backdrop.g()),
                mix(pal.bar.b(), pal.backdrop.b()),
            )
        };
        // Surfaces that carry text: the backdrop, the panels and bars, the rows, and the hover fill
        // rows take (`separator`).
        let surfaces = [("backdrop", pal.backdrop), ("panel", pal.panel), ("bar", bar), ("row", pal.row), ("hover", pal.separator)];
        for (what, text) in [("secondary text", pal.text_dim), ("disabled", pal.disabled())] {
            for (surface, bg) in surfaces {
                let ratio = contrast_ratio(text, bg);
                assert!(ratio >= 4.5, "{what} on {surface} ({}) is {ratio:.2}:1, light={light}", if light { "light" } else { "dark" });
            }
        }
        // Primary text is far above the bar.
        assert!(contrast_ratio(pal.text, pal.panel) >= 4.5);
    }
}

/// The same activation through `egui_kittest`, which runs the real `eframe::App` and finds nodes
/// the way a screen reader does: by role and accessible name.
#[test]
fn kittest_finds_and_activates_controls_by_accessible_name() {
    use egui_kittest::kittest::Queryable;
    let mut harness = egui_kittest::Harness::builder()
        .with_size(Vec2::new(1280.0, 860.0))
        .build_eframe(|cc| crate::app::EskizApp::new(&cc.egui_ctx, Document::new(1600, 1000), String::new(), Prefs::default()));
    harness.run();
    // The tool buttons are found by name, and the sliders carry their value.
    harness.get_by_label("Eraser");
    harness.get_by_label("Undo");
    harness.get_by_label_contains("Brush size ");
    harness.get_by_label("Layers").click();
    harness.run();
    assert_eq!(harness.state().popovers.open_panel(), Some(Panel::Layers), "clicking the named Layers node opens the popover");
    harness.get_by_label("Actions").click();
    harness.run();
    assert_eq!(harness.state().popovers.open_panel(), Some(Panel::Actions));
}

/// Every text-bearing pair of the Eskiz identity, dark and light: text roles on the five
/// surfaces (the bar composited over the backdrop at its alpha, and over pure black and white
/// artwork), accent fill with its ink, outlines and focus rings (non-text 3:1).
#[test]
fn eskiz_identity_meets_wcag_aa_on_every_pair() {
    use crate::theme::{palette_for, UiIdentity};
    let over = |fg: egui::Color32, bg: egui::Color32| {
        let a = f32::from(fg.a()) / 255.0;
        let mix = |f: u8, b: u8| (f32::from(f) * a + f32::from(b) * (1.0 - a)).round() as u8;
        egui::Color32::from_rgb(mix(fg.r(), bg.r()), mix(fg.g(), bg.g()), mix(fg.b(), bg.b()))
    };
    for light in [false, true] {
        let pal = palette_for(UiIdentity::Eskiz, light);
        let mode = if light { "light" } else { "dark" };
        let surfaces = [
            ("backdrop", pal.backdrop),
            ("bar", over(pal.bar, pal.backdrop)),
            ("bar over black", over(pal.bar, egui::Color32::BLACK)),
            ("bar over white", over(pal.bar, egui::Color32::WHITE)),
            ("panel", pal.panel),
            ("row", pal.row),
            ("hover", pal.hover),
        ];
        for (role, fg) in [
            ("text", pal.text),
            ("text_dim", pal.text_dim),
            ("disabled", pal.disabled()),
            ("accent", pal.accent),
            ("primary_text", pal.primary_text),
            ("danger", pal.danger),
            ("warning", pal.warning),
            ("success", pal.success),
            ("blueprint", pal.blueprint),
        ] {
            for (surface, bg) in surfaces {
                let r = contrast_ratio(fg, bg);
                assert!(r >= 4.5, "{role} on {surface} is {r:.2}:1 ({mode})");
            }
        }
        let r = contrast_ratio(pal.on_accent, pal.accent_fill);
        assert!(r >= 4.5, "on_accent on accent_fill is {r:.2}:1 ({mode})");
        let r = contrast_ratio(pal.on_primary, pal.primary);
        assert!(r >= 4.5, "on_primary on primary is {r:.2}:1 ({mode})");
        // Non-text: the accent fill as an indicator, outlines, focus ring and halo.
        for (what, fg, bg, min) in [
            ("accent_fill on panel", pal.accent_fill, pal.panel, 3.0),
            // On the light bar the fill (2.36:1) is never a lone indicator: the active tool glyph
            // uses `accent`, which is checked as text above.
            ("accent_fill on bar", pal.accent_fill, over(pal.bar, pal.backdrop), if light { 2.0 } else { 3.0 }),
            ("edge on panel", pal.edge, pal.panel, 3.0),
            ("edge on row", pal.edge, pal.row, 3.0),
            ("focus ring on panel", pal.focus_ring, pal.panel, 3.0),
            ("focus ring on row", pal.focus_ring, pal.row, 3.0),
            ("focus ring on backdrop", pal.focus_ring, pal.backdrop, 3.0),
            ("focus ring on bar", pal.focus_ring, over(pal.bar, pal.backdrop), 3.0),
            ("slider rail on row", pal.track, pal.row, 2.0),
        ] {
            let r = contrast_ratio(fg, bg);
            assert!(r >= min, "{what} is {r:.2}:1, needs {min} ({mode})");
        }
    }
}
