//! Back (A4): what the Back button closes on the headless app, row by row, in the design's order;
//! that Escape and Back agree for the rows Escape already handles; and that Back on the idle
//! gallery belongs to the system.

use eframe::egui::{Event, Key, Modifiers, Vec2};

use eskiz_core::filters::AdjustKind;
use crate::app::Prefs;
use crate::back::{BackStep, GalleryBack};
use crate::document::Document;
use crate::onboarding::{Gate, Tour};
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::selection_tool::EditMode;
use crate::sheets::Sheet;
use crate::snapshot::Headless;

fn canvas() -> Headless {
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(80, 60), Prefs::default());
    // No tour on a canvas test unless a test starts one.
    h.app.tour.skip();
    h.step();
    h.step();
    h
}

fn back(h: &mut Headless) -> bool {
    let ctx = h.ctx.clone();
    let did = h.app.system_back(&ctx);
    h.step();
    h.step();
    did
}

fn esc(h: &mut Headless) {
    for pressed in [true, false] {
        h.event(Event::Key { key: Key::Escape, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        h.step();
    }
    h.advance(0.2);
}

#[test]
fn an_idle_canvas_goes_back_to_the_gallery_and_the_idle_gallery_belongs_to_the_system() {
    let mut h = canvas();
    assert_eq!(h.app.back_step(), Some(BackStep::Canvas));
    assert!(back(&mut h));
    assert!(h.app.gallery_active());
    assert_eq!(h.app.back_step(), None, "Back on the idle gallery is left to the system");
    assert!(!back(&mut h), "nothing to do: the system takes it");
}

#[test]
fn the_tour_is_skipped_first() {
    let mut h = canvas();
    h.app.tour = Tour::at_startup(None, false, false);
    h.app.tour.poll(Gate { gallery: true, recovery_open: false, library_empty: Some(true) });
    assert!(h.app.tour.step().is_some());
    assert_eq!(h.app.back_step(), Some(BackStep::Tour));
    assert!(back(&mut h));
    assert!(h.app.tour.step().is_none());
    assert!(!h.app.gallery_active(), "one Back skips the tour, it does not also leave the canvas");
}

#[test]
fn the_tour_ends_the_same_way_with_escape_and_with_back() {
    let mut a = canvas();
    let mut b = canvas();
    for h in [&mut a, &mut b] {
        // The tour cards (and their Escape) live on the gallery.
        assert!(back(h));
        h.app.tour = Tour::at_startup(None, false, false);
        h.app.tour.poll(Gate { gallery: true, recovery_open: false, library_empty: Some(true) });
        h.step();
    }
    esc(&mut a);
    back(&mut b);
    assert_eq!(a.app.tour.step().is_none(), b.app.tour.step().is_none());
}

#[test]
fn text_editing_is_committed_and_closed() {
    let mut h = canvas();
    h.app.text.editing = Some(h.app.active);
    assert_eq!(h.app.back_step(), Some(BackStep::TextEdit));
    assert!(back(&mut h));
    assert!(h.app.text.editing.is_none());
}

#[test]
fn layer_menu_closes_before_the_layers_popover_and_the_popover_before_the_canvas() {
    let mut h = canvas();
    h.open_panel(Panel::Layers);
    h.step();
    h.app.layers_ui.menu = Some(h.app.active);
    assert_eq!(h.app.back_step(), Some(BackStep::LayerInner));
    assert!(back(&mut h));
    assert!(h.app.layers_ui.menu.is_none() && h.app.popovers.is_open(), "the menu goes first");
    assert_eq!(h.app.back_step(), Some(BackStep::Popover));
    assert!(back(&mut h));
    assert!(!h.app.popovers.is_open());
    assert!(!h.app.gallery_active());
    assert_eq!(h.app.back_step(), Some(BackStep::Canvas));
}

#[test]
fn a_popover_closes_on_back() {
    let mut h = canvas();
    h.app.actions_ui.tab = ActionsTab::Share;
    h.open_panel(Panel::Actions);
    h.step();
    assert!(h.app.popovers.is_open());
    assert!(back(&mut h));
    assert!(!h.app.popovers.is_open());
}

#[test]
fn a_sheet_closes_before_the_popover_under_it() {
    let mut h = canvas();
    h.open_panel(Panel::Actions);
    h.app.sheet = Some(Sheet::About);
    h.step();
    assert_eq!(h.app.back_step(), Some(BackStep::Sheet));
    assert!(back(&mut h));
    assert!(h.app.sheet.is_none());
    assert!(h.app.popovers.is_open());
}

#[test]
fn escape_and_back_agree_on_a_sheet() {
    let mut a = canvas();
    let mut b = canvas();
    for h in [&mut a, &mut b] {
        h.app.sheet = Some(Sheet::Cheatsheet);
        h.step();
    }
    esc(&mut a);
    back(&mut b);
    assert_eq!(a.app.sheet.is_none(), b.app.sheet.is_none());
    assert!(b.app.sheet.is_none());
}

#[test]
fn canvas_info_and_the_guide_close_on_back() {
    let mut h = canvas();
    h.app.canvas_info_open = true;
    assert_eq!(h.app.back_step(), Some(BackStep::Sheet));
    assert!(back(&mut h));
    assert!(!h.app.canvas_info_open);
    h.app.guide.open = true;
    assert!(back(&mut h));
    assert!(!h.app.guide.open);
}

#[test]
fn the_studio_closes_on_back_like_escape() {
    for use_back in [false, true] {
        let mut h = canvas();
        h.open_panel(Panel::Brush);
        let edit = h.app.library_layout.edit.expect("an Edit button");
        h.click(edit.center());
        h.advance(0.3);
        assert!(h.app.studio.is_some());
        if use_back {
            assert_eq!(h.app.back_step(), Some(BackStep::Studio));
            assert!(back(&mut h));
        } else {
            esc(&mut h);
        }
        assert!(h.app.studio.is_none(), "use_back={use_back}");
    }
}

#[test]
fn an_adjustment_is_cancelled_like_escape() {
    for use_back in [false, true] {
        let mut h = canvas();
        let before = h.app.session.doc.layer(h.app.active).unwrap().to_rgba();
        assert!(h.app.enter_adjustment(AdjustKind::Opacity));
        h.app.adjust_set(0, 0.3);
        h.step();
        assert!(h.app.adjusting());
        if use_back {
            assert_eq!(h.app.back_step(), Some(BackStep::Edit));
            assert!(back(&mut h));
        } else {
            esc(&mut h);
        }
        assert!(!h.app.adjusting(), "use_back={use_back}");
        assert_eq!(h.app.session.doc.layer(h.app.active).unwrap().to_rgba(), before, "cancelled, not applied");
    }
}

#[test]
fn selection_mode_is_left_like_escape() {
    for use_back in [false, true] {
        let mut h = canvas();
        h.app.toggle_select_mode();
        assert_eq!(h.app.edit_mode, EditMode::Select);
        if use_back {
            assert_eq!(h.app.back_step(), Some(BackStep::Edit));
            assert!(back(&mut h));
        } else {
            esc(&mut h);
        }
        assert_eq!(h.app.edit_mode, EditMode::Off, "use_back={use_back}");
    }
}

#[test]
fn a_transform_in_progress_is_cancelled() {
    let mut h = canvas();
    h.app.toggle_select_mode();
    h.app.begin_transform();
    if h.app.transform.is_none() {
        // Nothing to transform on an empty layer: nothing to test.
        return;
    }
    assert_eq!(h.app.back_step(), Some(BackStep::Edit));
    assert!(back(&mut h));
    assert!(h.app.transform.is_none() && h.app.edit_mode == EditMode::Off);
}

#[test]
fn liquify_is_cancelled() {
    let mut h = canvas();
    if !h.app.enter_liquify() {
        return;
    }
    assert_eq!(h.app.back_step(), Some(BackStep::Edit));
    assert!(back(&mut h));
    assert!(h.app.liquify.is_none());
}

#[test]
fn the_3d_view_closes_before_the_canvas_rule_applies() {
    use crate::view3d::ThreeD;
    let mut h = canvas();
    h.app.three_d = Some(Box::new(ThreeD::new(crate::view3d_tests::cube_doc(), None)));
    h.advance(0.1);
    assert_eq!(h.app.back_step(), Some(BackStep::ThreeD));
    assert!(back(&mut h));
    assert!(h.app.three_d.is_none());
}

#[test]
fn gallery_states_close_one_at_a_time() {
    let mut h = canvas();
    assert!(back(&mut h));
    assert!(h.app.gallery_active());
    h.app.gal.enter_select_for_test();
    assert_eq!(h.app.back_step(), Some(BackStep::Gallery(GalleryBack::Select)));
    assert!(back(&mut h));
    assert_eq!(h.app.back_step(), None, "select mode left, nothing else open");
}

#[test]
fn the_back_log_names_match_what_the_script_waits_for() {
    assert_eq!(BackStep::Popover.log_name(), "popover");
    assert_eq!(BackStep::Canvas.log_name(), "gallery");
}

#[test]
fn back_never_acts_while_the_gallery_zooms_into_a_drawing() {
    let mut h = canvas();
    let ctx = h.ctx.clone();
    h.app.open_gallery(&ctx);
    h.app.new_canvas(0, 64, 64, eframe::egui::Rect::from_center_size(eframe::egui::pos2(300.0, 300.0), Vec2::splat(50.0)));
    assert_eq!(h.app.back_step(), Some(BackStep::Opening));
    assert!(back(&mut h), "the press is taken and ignored, not passed to the system");
    assert!(h.app.gallery_active());
}
