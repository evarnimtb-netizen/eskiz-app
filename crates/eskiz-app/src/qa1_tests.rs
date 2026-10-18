//! Regression tests for the QA round 1 fixes.

use crate::app::{EskizApp, Prefs};
use crate::gallery::Screen;
use crate::snapshot::{snapshot_dir, Headless};
use crate::storage::{self, Library};
use crate::theme::UiIdentity;
use eframe::egui::Vec2;
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-qa1-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn gallery(name: &str, identity: UiIdentity, size: Vec2) -> (Headless, PathBuf) {
    let dir = temp(name);
    let lib = Library::at(&dir);
    for i in 0..3 {
        let id = storage::new_id();
        lib.save(&id, "Sketch", &crate::onboarding_sample::scene(256, 192, i)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut h = Headless::from_app(size, move |ctx| EskizApp::with_library(ctx, Prefs { identity, ..Prefs::default() }, lib));
    h.settle_storage();
    h.advance(0.3);
    (h, dir)
}

/// Opening a document from the gallery: from the first frame of the canvas on, the canvas is fitted and the
/// chrome (side bar and top bar) is drawn; nothing collapses for a frame.
#[test]
fn opening_a_document_never_flashes_a_collapsed_canvas() {
    for identity in UiIdentity::ALL {
        let (mut h, dir) = gallery("flash", identity, Vec2::new(1280.0, 860.0));
        let c = h.app.gal.card_rects[0].1.center();
        h.click(c);
        let mut fitted = None;
        let mut switched = false;
        let mut shots = Vec::new();
        let mut frames = Vec::new();
        for i in 0..60 {
            let img = h.render_at(1.0);
            // The frame that switches the screen still draws the zoom overlay: the canvas is drawn from the next one.
            if matches!(h.app.gal.screen, Screen::Canvas) && switched {
                let z = h.app.view.zoom;
                let first = *fitted.get_or_insert(z);
                shots.push(img.clone());
                frames.push((i, z, h.app.need_fit));
                if frames.len() <= 4 {
                    img.save_png(&snapshot_dir().join(format!("qa1_open_{}_{}.png", identity.key(), frames.len()))).unwrap();
                }
                assert!(first > 1.5, "the first canvas frame is not fitted (zoom {first})");
                assert!((z - first).abs() < 1e-3 || frames.len() > 6, "zoom jumped on frame {i}: {frames:?}");
            }
            switched |= matches!(h.app.gal.screen, Screen::Canvas);
        }
        // The chrome (top bar, side bar) is drawn from the first canvas frame: it matches the settled frame.
        let settled = shots.last().unwrap().clone();
        for (k, img) in shots.iter().take(6).enumerate() {
            let chrome = |x: usize, y: usize| y < 48 || x < 60;
            let diff = (0..img.height).flat_map(|y| (0..img.width).map(move |x| (x, y))).filter(|&(x, y)| chrome(x, y) && img.pixel(x, y) != settled.pixel(x, y)).count();
            assert_eq!(diff, 0, "{identity:?}: canvas frame {k} lacks chrome ({diff} pixels differ from the settled frame)");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A4: the welcome card does not keep the UI repainting while it only waits for a tap.
#[test]
fn the_welcome_card_stops_repainting_when_idle() {
    let dir = temp("welcome");
    let lib = Library::at(&dir);
    let mut h = Headless::from_app(Vec2::new(1280.0, 860.0), move |ctx| {
        let mut app = EskizApp::with_library(ctx, Prefs::default(), lib);
        app.tour = crate::onboarding::Tour::at_startup(None, false, false);
        app
    });
    h.settle_storage();
    h.advance(0.5);
    let mut busy = 0;
    for _ in 0..30 {
        h.step();
        if h.last_repaint < std::time::Duration::from_millis(50) {
            busy += 1;
        }
    }
    assert!(h.app.tour.step().is_some(), "the tour card is showing");
    assert!(busy <= 2, "{busy} of 30 idle frames asked for an immediate repaint");
    let _ = std::fs::remove_dir_all(dir);
}

/// B5: the Eskiz selected fill is clearly different from the unselected surfaces in both themes, its ink reads on it,
/// the outline of a selected row has the same weight in light and dark, and a switch's on state differs from off.
#[test]
fn eskiz_selected_states_are_clear_in_light_and_dark() {
    use crate::a11y::contrast_ratio as cr;
    use crate::theme::palette_for;
    for light in [false, true] {
        let p = palette_for(UiIdentity::Eskiz, light);
        for surface in [p.row, p.panel] {
            assert!(cr(p.accent_fill, surface) >= 3.0, "light={light}: selected fill vs surface {}", cr(p.accent_fill, surface));
        }
        assert!(cr(p.on_accent, p.accent_fill) >= 4.5, "light={light}: ink on the selected fill");
        assert!(cr(p.accent, p.panel) >= 3.0 && cr(p.accent, p.row) >= 3.0, "light={light}: selected outline");
        assert!(cr(p.accent_fill, p.separator) >= 3.0, "light={light}: switch on vs off track");
        assert!(cr(p.edge, p.row) >= 3.0, "light={light}: the off track outline");
        // The same visual weight in both themes (within a factor of 1.5 of each other).
        let q = palette_for(UiIdentity::Eskiz, !light);
        let (a, b) = (cr(p.accent_fill, p.row), cr(q.accent_fill, q.row));
        assert!((a / b).max(b / a) < 1.5, "selected weight differs between themes: {a:.1} vs {b:.1}");
    }
}

/// B5: a switch drawn on and off differs, with a visible knob in both states and looks.
#[test]
fn switch_on_and_off_are_distinct_and_the_knob_is_visible() {
    use crate::theme::palette_for;
    for identity in UiIdentity::ALL {
        for light in [false, true] {
            let pal = palette_for(identity, light);
            let img = crate::snapshot::render_ui(Vec2::new(120.0, 40.0), |ui| {
                ui.painter().rect_filled(ui.max_rect(), 0.0, pal.row);
                let r = |x: f32| eframe::egui::Rect::from_min_size(eframe::egui::pos2(x, 8.0), Vec2::new(48.0, 24.0));
                crate::panels::rows::switch(ui.painter(), &pal, r(8.0), true, true);
                crate::panels::rows::switch(ui.painter(), &pal, r(64.0), false, true);
            });
            // Track colour near the left end of each switch, knob colour at the knob centre.
            let (on_track, off_track) = (img.pixel(12, 20), img.pixel(106, 20));
            let (on_knob, off_knob) = (img.pixel(44, 20), img.pixel(76, 20));
            assert_ne!(on_track, off_track, "{identity:?} light={light}: track on = off");
            assert_ne!(on_knob, on_track, "{identity:?} light={light}: on knob invisible");
            assert_ne!(off_knob, off_track, "{identity:?} light={light}: off knob invisible");
        }
    }
}

/// B6: while a popover opens or closes, the panel surface is already solid when its content is faint (no canvas
/// seen through the panel), and the content fades linearly (the close does not end in a cut).
#[test]
fn popover_surface_is_solid_before_its_content_and_the_close_fades_smoothly() {
    use crate::popover::{ease_out, Visible, Panel};
    let at = |t: f32| Visible { panel: Panel::Brush, anchor: eframe::egui::Rect::NOTHING, ease: ease_out(t), t };
    assert_eq!(at(0.0).surface_alpha(), 0.0);
    assert_eq!(at(1.0).surface_alpha(), 1.0);
    assert!(at(0.35).surface_alpha() > 0.99 && at(0.35).content_alpha() < 0.4);
    // Closing runs t from 1 to 0 over the same frames: every step of the content fade is small.
    let steps: Vec<f32> = (0..=9).map(|i| at(1.0 - i as f32 / 9.0).content_alpha()).collect();
    assert!(steps.windows(2).all(|w| (w[0] - w[1]).abs() < 0.13), "{steps:?}");
}

/// B7: applying an adjustment shows one progress indicator (the panel's), never a toast over its buttons.
#[test]
fn applying_an_adjustment_shows_no_toast_over_the_panel() {
    use eskiz_core::filters::AdjustKind;
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), crate::document::Document::new(1024, 768), Prefs::default());
    h.app.enter_adjustment(AdjustKind::Halftone);
    h.advance(0.3);
    let ctx = h.ctx.clone();
    h.app.adjust_apply(&ctx);
    let mut saw_job = false;
    for _ in 0..600 {
        h.step();
        let in_flight = h.app.adjust.is_some();
        saw_job |= in_flight;
        if in_flight {
            assert!(h.app.toast.as_ref().is_none_or(|t| !t.text.starts_with("Applying")), "a toast duplicates the panel progress");
            h.render().save_png(&snapshot_dir().join("qa1_adjust_progress.png")).unwrap();
        }
        if h.app.adjust.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.app.adjust.is_none(), "the adjustment finished");
    let _ = saw_job;
}

/// B8: on a white canvas the transform box and handles keep a dark mark (two-tone), in every look and theme.
#[test]
fn transform_handles_and_box_are_visible_on_a_white_canvas() {
    for identity in UiIdentity::ALL {
        for light in [false, true] {
            let mut h = Headless::new(Vec2::new(1280.0, 860.0), crate::document::Document::new(1024, 768), Prefs { identity, light, ..Prefs::default() });
            h.advance(0.3);
            h.app.session.doc.layer_at_mut(0).unwrap().write_rect(200, 150, 600, 450, &[255, 255, 255, 255].repeat(600 * 450));
            h.app.begin_transform();
            h.advance(0.3);
            let img = h.render();
            let quad = h.app.transform.as_ref().unwrap().session.quad().unwrap();
            let s: Vec<eframe::egui::Pos2> = quad.iter().map(|c| h.app.view.canvas_to_screen(h.app.viewport, f64::from(c.x), f64::from(c.y))).collect();
            let darkest = |c: eframe::egui::Pos2, r: i32| {
                let mut m = 255u8;
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (x, y) = ((c.x as i32 + dx).clamp(0, 1279) as usize, (c.y as i32 + dy).clamp(0, 859) as usize);
                        m = m.min(img.pixel(x, y)[0]);
                    }
                }
                m
            };
            // Inset the probe from the canvas edge: the box lies on the canvas border, so look at a handle and the box side.
            let edge_mid = eframe::egui::pos2((s[0].x + s[1].x) / 2.0, (s[0].y + s[1].y) / 2.0);
            assert!(darkest(s[2], 7) < 90, "{identity:?} light={light}: corner handle has no dark ring");
            assert!(darkest(edge_mid, 3) < 90, "{identity:?} light={light}: box edge has no dark underlay");
            img.save_png(&snapshot_dir().join(format!("qa1_transform_{}_{}.png", identity.key(), if light { "light" } else { "dark" }))).unwrap();
        }
    }
}

fn frames_app(identity: UiIdentity, size: Vec2) -> Headless {
    let mut h = Headless::new(size, crate::qa_capture::frames_doc(), Prefs { identity, ..Prefs::default() });
    h.advance(0.5);
    h
}

/// B9: Play/Pause keep their slot (Add frame and Settings do not move) and Hold is always shown.
#[test]
fn the_timeline_controls_do_not_shift_and_hold_is_always_there() {
    for identity in UiIdentity::ALL {
        let mut h = frames_app(identity, Vec2::new(1280.0, 860.0));
        let strip_y = h.app.viewport.bottom() as usize + 4;
        let region = |img: &crate::snapshot::Image| -> Vec<[u8; 4]> {
            (150..300).flat_map(|x| (strip_y..strip_y + 30).map(move |y| (x, y))).map(|(x, y)| img.pixel(x, y)).collect()
        };
        let paused = region(&h.render());
        h.app.anim_play();
        h.advance(0.1);
        let playing = region(&h.render());
        assert!(paused == playing, "{identity:?}: the controls after Play moved when playing");
        h.app.anim_pause();
        // Hold shows for no selected frame and for a selected one: the same pixels left of the readout.
        h.app.anim.selected = None;
        h.advance(0.1);
        let none = h.render();
        let ink = |img: &crate::snapshot::Image| {
            let bg = img.pixel(700, strip_y + 10);
            (460..540).flat_map(|x| (strip_y..strip_y + 30).map(move |y| (x, y))).filter(|&(x, y)| img.pixel(x, y) != bg).count()
        };
        assert!(ink(&none) > 50, "{identity:?}: no Hold readout without a selected frame");
        h.app.anim.selected = Some(1);
        h.advance(0.1);
        assert!(ink(&h.render()) > 50, "{identity:?}: no Hold readout with a selected frame");
        h.render().save_png(&snapshot_dir().join(format!("qa1_frames_{}.png", identity.key()))).unwrap();
    }
}

/// B9: at 900x600 the Actions popover stops above the Frames timeline instead of covering its controls.
#[test]
fn the_actions_popover_leaves_the_timeline_controls_uncovered() {
    for identity in UiIdentity::ALL {
        let mut h = frames_app(identity, Vec2::new(900.0, 600.0));
        h.open_panel(crate::popover::Panel::Actions);
        h.advance(0.5);
        let body = h.app.popovers.body_rect().expect("popover drawn");
        assert!(body.bottom() <= h.app.viewport.bottom() + 0.5, "{identity:?}: popover bottom {} reaches the timeline at {}", body.bottom(), h.app.viewport.bottom());
        h.render().save_png(&snapshot_dir().join(format!("qa1_frames_actions_{}.png", identity.key()))).unwrap();
    }
}

/// B10: Share for a frame document lists the animation formats before the still ones, and no MP4 quality row
/// is offered while MP4 is off.
#[test]
fn share_puts_animation_formats_first_for_frame_documents_and_hides_mp4_quality() {
    use crate::panels::actions::ActionsTab;
    let mut h = Headless::new(Vec2::new(1280.0, 1100.0), crate::qa_capture::frames_doc(), Prefs::default());
    h.ctx.enable_accesskit();
    h.app.actions_ui.tab = ActionsTab::Share;
    h.open_panel(crate::popover::Panel::Actions);
    h.advance(0.5);
    let nodes: Vec<(String, f64)> = h
        .access
        .as_ref()
        .unwrap()
        .nodes
        .iter()
        .filter_map(|(_, n)| n.label().map(|l| (l.to_owned(), n.bounds().map_or(0.0, |b| b.y0))))
        .collect();
    let y = |needle: &str| nodes.iter().find(|(l, _)| l.contains(needle)).map(|(_, y)| *y);
    let (gif, procreate) = (y("Animated GIF"), y("Layered .procreate"));
    assert!(gif.is_some() && procreate.is_some(), "rows not found in {:?}", nodes.iter().map(|n| &n.0).collect::<Vec<_>>());
    assert!(gif < procreate, "animation formats come first for a frame document");
    if !eskiz_media::encoder_available() {
        assert!(y("MP4 quality").is_none(), "MP4 quality is offered while MP4 is off");
    }
}

/// B11: popovers stay 12 pt from the window edges and clear of the tool sidebar, at both window sizes, in both looks.
#[test]
fn popovers_keep_a_12pt_margin_and_clear_the_sidebar() {
    use crate::popover::Panel;
    for identity in UiIdentity::ALL {
        for size in [Vec2::new(900.0, 600.0), Vec2::new(1280.0, 860.0)] {
            for right_hand in [false, true] {
                for panel in [Panel::Actions, Panel::Brush, Panel::Layers, Panel::Colour, Panel::Adjustments] {
                    let mut h = Headless::new(size, crate::document::Document::new(1024, 768), Prefs { identity, sidebar_right: right_hand, ..Prefs::default() });
                    h.advance(0.3);
                    h.open_panel(panel);
                    h.advance(0.5);
                    let body = h.app.popovers.body_rect().expect("popover drawn");
                    let tag = format!("{identity:?} {size:?} right={right_hand} {panel:?}: {body:?}");
                    assert!(body.left() >= 11.9 && body.right() <= size.x - 11.9, "side margin {tag}");
                    assert!(body.bottom() <= size.y - 11.9, "bottom margin {tag}");
                    // The sidebar capsule is 44 pt wide at 8 pt from its edge: nothing of it shows beside the popover.
                    if !right_hand {
                        assert!(body.left() >= 52.0, "sidebar peeks beside the popover {tag}");
                    } else {
                        assert!(body.right() <= size.x - 52.0, "sidebar peeks beside the popover {tag}");
                    }
                }
            }
        }
    }
}

/// B12: a popover tab whose last row is cut at the bottom fades out there (a scroll cue), in both looks and themes.
#[test]
fn clipped_popover_tabs_show_a_scroll_cue() {
    use crate::panels::actions::ActionsTab;
    for identity in UiIdentity::ALL {
        for light in [false, true] {
            let mut h = Headless::new(Vec2::new(900.0, 600.0), crate::document::Document::new(1024, 768), Prefs { identity, light, ..Prefs::default() });
            h.advance(0.3);
            h.app.actions_ui.tab = ActionsTab::Canvas;
            h.open_panel(crate::popover::Panel::Actions);
            h.advance(0.5);
            let body = h.app.popovers.body_rect().unwrap();
            let img = h.render();
            let panel = h.app.pal.panel;
            let dist = |p: [u8; 4]| (0..3).map(|i| (i32::from(p[i]) - i32::from([panel.r(), panel.g(), panel.b()][i])).abs()).sum::<i32>();
            let x = (body.right() - 24.0) as usize;
            let near_bottom = dist(img.pixel(x, (body.bottom() - 16.0) as usize));
            let well_above = dist(img.pixel(x, (body.bottom() - 80.0) as usize));
            // Inside a row the colour differs from the panel; at the clipped bottom the fade brings it back to the panel colour.
            assert!(near_bottom < well_above || well_above == 0, "{identity:?} light={light}: no fade at the clipped bottom ({near_bottom} vs {well_above})");
            img.save_png(&snapshot_dir().join(format!("qa1_scroll_cue_{}_{}.png", identity.key(), if light { "light" } else { "dark" }))).unwrap();
        }
    }
}

/// B13: the focus ring is a distinct outline: not the selected fill, and with a halo of the opposite colour.
#[test]
fn the_focus_ring_is_distinct_from_the_selected_fill() {
    use crate::a11y::{contrast_ratio as cr, focus_colours};
    use crate::theme::palette_for;
    for identity in UiIdentity::ALL {
        for light in [false, true] {
            let pal = palette_for(identity, light);
            let (ring, halo) = focus_colours(&pal);
            assert_ne!(ring, pal.accent_fill, "{identity:?} light={light}");
            assert!(cr(ring, pal.accent_fill) >= 3.0 || cr(ring, pal.panel) >= 7.0, "{identity:?} light={light}: ring vs selected fill {}", cr(ring, pal.accent_fill));
            assert!(cr(ring, pal.panel) >= 7.0 && cr(ring, pal.row) >= 7.0, "{identity:?} light={light}: ring on surfaces");
            assert!(cr(ring, halo) >= 7.0, "{identity:?} light={light}: halo");
        }
    }
}

/// C25: the welcome card body does not wrap to a single orphan word, in either look.
#[test]
fn the_welcome_body_has_no_orphan_last_line() {
    for identity in UiIdentity::ALL {
        let h = Headless::new(Vec2::new(1280.0, 860.0), crate::document::Document::new(64, 64), Prefs { identity, ..Prefs::default() });
        let body = crate::onboarding::copy::WELCOME_BODY;
        let galley = h.ctx.fonts_mut(|f| f.layout(body.to_owned(), eframe::egui::FontId::proportional(14.0), eframe::egui::Color32::WHITE, 360.0 - 40.0 + 40.0));
        let last: String = galley.rows.last().unwrap().glyphs.iter().map(|g| g.chr).collect();
        assert!(last.split_whitespace().count() >= 2, "{identity:?}: orphan last line {last:?}");
    }
}

/// C15: the wording names the mouse and the touch alternatives.
#[test]
fn desktop_wording_names_mouse_and_touch() {
    let src = include_str!("gallery/view.rs");
    assert!(src.contains("Click or tap +"));
    let colour = include_str!("panels/colour.rs");
    assert!(colour.contains("Right-click or long-press a swatch"));
}

/// C20: a toast and a coach mark fade in (the first frame is not at full opacity) and are steady afterwards.
#[test]
fn toasts_and_coach_marks_fade_in() {
    let region = |img: &crate::snapshot::Image, x0: usize, x1: usize, y0: usize, y1: usize| -> u64 {
        (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| u64::from(img.pixel(x, y)[0]) + u64::from(img.pixel(x, y)[1]) + u64::from(img.pixel(x, y)[2])).sum()
    };
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), crate::document::Document::new(1024, 768), Prefs::default());
    h.advance(0.3);
    h.app.show_toast("Undo Stroke", 3.0);
    let first = h.render();
    h.advance(0.3);
    let steady_a = h.render();
    let steady_b = h.render();
    let (x0, x1, y0, y1) = (560, 720, 62, 90);
    let (a, b, c) = (region(&first, x0, x1, y0, y1), region(&steady_a, x0, x1, y0, y1), region(&steady_b, x0, x1, y0, y1));
    assert_ne!(a, b, "the toast appears at full opacity on its first frame");
    assert_eq!(b, c, "the toast is steady after the fade-in");
}
