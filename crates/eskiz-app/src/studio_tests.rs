//! Brush Bench tests: snapshots (written to `target/snapshots` for review), and the editing,
//! saving and loading rules driven through real clicks and drags.

use eframe::egui::{self, pos2, Pos2, Rect, Vec2};
use eskiz_brushlib::grains::GrainKind;
use eskiz_brushlib::library_ref;
use eskiz_brushlib::tips::TipKind;
use eskiz_core::brushes::{Brush, Grain};

use crate::app::{Prefs, Tool};
use crate::brush_import::{BrushStore, MY_BRUSHES_SET};
use crate::brush_library::{BrushBank, BrushId};
use crate::document::Document;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless};
use crate::studio_settings::Section;

fn headless_sized(size: Vec2, light: bool) -> Headless {
    let prefs = Prefs { light, ..Prefs::default() };
    let mut h = Headless::new(size, Document::new(2048, 1536), prefs);
    // Keep the tests out of the user's real brush folders.
    let (imports, mine) = stores("iso");
    h.app.bank = BrushBank::with_stores(imports, mine);
    h
}

fn headless(light: bool) -> Headless {
    headless_sized(Vec2::new(1280.0, 860.0), light)
}

/// A pair of empty stores in fresh temp folders.
fn stores(tag: &str) -> (BrushStore, BrushStore) {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("eskiz-studio-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    (BrushStore::at(root.join("imports")), BrushStore::at(root.join("mine")))
}

fn theme(light: bool) -> &'static str {
    if light {
        "light"
    } else {
        "dark"
    }
}

fn drag(h: &mut Headless, from: Pos2, to: Pos2) {
    h.move_to(from);
    h.press(from, true);
    h.step();
    h.move_to(to);
    h.step();
    h.press(to, false);
    h.step();
}

/// A scribble over the pad: a slow wave with several samples per frame position.
fn scribble(h: &mut Headless, pad: Rect) {
    let n = 40;
    let pt = |i: usize| {
        let t = i as f32 / n as f32;
        pos2(pad.left() + pad.width() * (0.12 + 0.76 * t), pad.center().y + pad.height() * 0.22 * (t * std::f32::consts::TAU * 1.5).sin())
    };
    h.move_to(pt(0));
    h.press(pt(0), true);
    h.step();
    for i in 1..=n {
        h.move_to(pt(i));
        h.step();
    }
    h.press(pt(n), false);
    h.step();
}

/// Open the library, press Edit, and check the Studio opened.
fn open_studio(h: &mut Headless) {
    h.open_panel(Panel::Brush);
    let edit = h.app.library_layout.edit.expect("the library has an Edit button");
    h.click(edit.center());
    h.advance(0.3);
    assert!(h.app.studio.is_some(), "Edit opens the Brush Bench");
}

fn click_section(h: &mut Headless, section: Section) {
    let rect = h.app.studio.as_ref().unwrap().layout.sections.iter().find(|(s, _)| *s == section).unwrap().1;
    h.click(rect.center());
    h.advance(0.1);
    assert_eq!(h.app.studio.as_ref().unwrap().section, section);
}

fn slider_rect(h: &Headless, label: &str) -> Rect {
    h.app.studio.as_ref().unwrap().layout.sliders.iter().find(|(l, _)| *l == label).unwrap_or_else(|| panic!("no slider {label}")).1
}

/// Drag the slider `label` from `from` to `to` (0..1 along the track).
fn drag_slider(h: &mut Headless, label: &str, from: f32, to: f32) {
    let r = slider_rect(h, label);
    let (x0, x1) = (r.left() + 10.0, r.right() - 10.0);
    let y = r.top() + 41.0;
    drag(h, pos2(x0 + (x1 - x0) * from, y), pos2(x0 + (x1 - x0) * to, y));
}

fn user_brush_count(h: &Headless) -> usize {
    h.app.bank.sets.iter().find(|s| s.name == MY_BRUSHES_SET).map_or(0, |s| s.brushes.len())
}

fn click_button(h: &mut Headless, pick: impl Fn(&crate::studio::StudioLayout) -> Option<Rect>) {
    let r = pick(&h.app.studio.as_ref().unwrap().layout).expect("button drawn");
    h.click(r.center());
    h.advance(0.2);
}

// ------------------------------------------------------------------ opening the Studio

#[test]
fn edit_button_opens_the_studio_on_the_tools_brush_and_closes_the_library() {
    let mut h = headless(false);
    let current = h.app.tool_brushes[0].clone();
    open_studio(&mut h);
    let st = h.app.studio.as_ref().unwrap();
    assert_eq!(st.target, current);
    assert!(!st.is_user_brush() && !st.dirty());
    let lb = h.app.bank.find(&current).unwrap();
    assert_eq!(st.working.brush, lb.brush, "the Studio shows the library brush's own settings");
    assert_eq!(st.working.tip, lb.tip);
    assert!(!h.app.popovers.is_open(), "the library closes behind the Studio");
    h.render().save_png(&snapshot_dir().join("studio_open_dark.png")).unwrap();
}

#[test]
fn tapping_the_chosen_brush_again_opens_the_studio_but_other_rows_just_select() {
    let mut h = headless(false);
    h.open_panel(Panel::Brush);
    let rows = h.app.library_layout.rows.clone();
    let (other, rect) = rows.iter().find(|(id, _)| *id != h.app.tool_brushes[0]).cloned().unwrap();
    h.click(rect.center());
    h.advance(0.2);
    assert_eq!(h.app.tool_brushes[0], other);
    assert!(h.app.studio.is_none(), "choosing a brush does not open the Studio");
    // Now it is the chosen one: a second tap opens it.
    let (_, rect) = h.app.library_layout.rows.iter().find(|(id, _)| *id == other).cloned().unwrap();
    h.click(rect.center());
    h.advance(0.2);
    assert_eq!(h.app.studio.as_ref().expect("a second tap opens the Studio").target, other);
}

#[test]
fn escape_closes_the_studio_and_it_blocks_the_canvas() {
    let mut h = headless(false);
    open_studio(&mut h);
    // A press on the canvas behind the sheet paints nothing.
    let layer_pixels = |h: &Headless| h.app.session.doc.layer(h.app.active).unwrap().to_rgba();
    let before = layer_pixels(&h);
    drag(&mut h, pos2(30.0, 300.0), pos2(60.0, 330.0));
    assert!(layer_pixels(&h) == before, "the canvas heard a stroke through the Studio");
    h.event(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
    h.advance(0.2);
    assert!(h.app.studio.is_none());
}

// ------------------------------------------------------------------ the sheet

#[test]
fn every_section_snapshot_dark_and_a_few_light() {
    let dir = snapshot_dir();
    for light in [false, true] {
        let mut h = headless(light);
        open_studio(&mut h);
        let sections: &[Section] = if light {
            &[Section::StrokePath, Section::Shape, Section::Grain, Section::Taper]
        } else {
            &Section::ALL
        };
        for &s in sections {
            click_section(&mut h, s);
            h.advance(0.2);
            let name = format!("studio_{}_{}.png", format!("{s:?}").to_lowercase(), theme(light));
            h.render().save_png(&dir.join(name)).unwrap();
        }
    }
}

#[test]
fn the_sheet_fits_a_small_window_and_its_header_buttons_do_not_overlap() {
    let mut h = headless_sized(Vec2::new(900.0, 600.0), false);
    open_studio(&mut h);
    let st = h.app.studio.as_ref().unwrap();
    let sheet = st.layout.sheet.unwrap();
    assert!(sheet.left() >= 0.0 && sheet.right() <= 900.0 && sheet.top() >= 0.0 && sheet.bottom() <= 600.0, "{sheet:?}");
    let l = &st.layout;
    let buttons = [l.reset.unwrap(), l.duplicate.unwrap(), l.save.unwrap(), l.done.unwrap()];
    for (i, a) in buttons.iter().enumerate() {
        assert!(a.right() <= sheet.right() && a.left() >= l.name.unwrap().right(), "{a:?} overlaps the name field");
        for b in &buttons[i + 1..] {
            assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
        }
    }
    for s in [Section::Shape, Section::Pencil] {
        click_section(&mut h, s);
    }
    h.render().save_png(&snapshot_dir().join("studio_small_window.png")).unwrap();
}

#[test]
fn every_section_lists_sliders_inside_the_sheet() {
    let mut h = headless(false);
    open_studio(&mut h);
    for s in Section::ALL {
        click_section(&mut h, s);
        h.advance(0.1);
        let st = h.app.studio.as_ref().unwrap();
        let sheet = st.layout.sheet.unwrap();
        for (label, r) in &st.layout.sliders {
            assert!(r.left() >= sheet.left() && r.right() <= sheet.right(), "{label} sticks out sideways");
            assert!(r.width() > 150.0, "{label} is too narrow");
        }
    }
}

// ------------------------------------------------------------------ editing and saving

#[test]
fn a_slider_edits_the_working_copy_only_until_save_makes_a_my_brushes_entry() {
    let mut h = headless(false);
    open_studio(&mut h);
    let original = h.app.bank.find(&h.app.tool_brushes[0]).unwrap().brush.clone();
    let id = h.app.tool_brushes[0].clone();
    drag_slider(&mut h, "Spacing", 0.0, 0.5);
    let st = h.app.studio.as_ref().unwrap();
    assert!((st.working.brush.spacing - 0.5).abs() < 0.03, "spacing {}", st.working.brush.spacing);
    assert!(st.dirty());
    // Nothing is saved yet, and the library brush is untouched.
    assert_eq!(user_brush_count(&h), 0);
    assert_eq!(h.app.bank.find(&id).unwrap().brush, original);
    assert_eq!(h.app.tool_brushes[0], id);

    click_button(&mut h, |l| l.save);
    let st = h.app.studio.as_ref().unwrap();
    assert!(st.is_user_brush() && !st.dirty());
    assert_eq!(user_brush_count(&h), 1);
    let BrushId::User(_) = &h.app.tool_brushes[0] else { panic!("Save makes the new brush the tool's brush") };
    let spacing = st.working.brush.spacing;
    assert_eq!(h.app.stroke_settings().brush.spacing, spacing);
    assert_eq!(h.app.bank.find(&id).unwrap().brush, original, "the built-in brush is never changed");
    // Saving again edits the same entry instead of making another.
    drag_slider(&mut h, "Spacing", 0.5, 0.9);
    click_button(&mut h, |l| l.save);
    assert_eq!(user_brush_count(&h), 1);
    h.render().save_png(&snapshot_dir().join("studio_saved_dark.png")).unwrap();
}

#[test]
fn spacing_and_tilt_size_survive_a_save_and_a_reload() {
    let dirs = stores("persist");
    let mut h = headless(false);
    h.app.bank = BrushBank::with_stores(dirs.0.clone(), dirs.1.clone());
    open_studio(&mut h);
    let source = h.app.studio.as_ref().unwrap().target.clone();
    let original = h.app.bank.find(&source).unwrap();
    h.app.studio_edit_brush(|b| {
        b.spacing = 0.37;
        b.tilt_size = 0.65;
        b.tilt_rotation = true;
    });
    h.app.studio_rename("Persistent pen".into());
    let id = h.app.studio_save().expect("saved");
    assert!(dirs.1.contains("Persistent pen"), "one .brush file per brush");

    let fresh = BrushBank::with_stores(dirs.0.clone(), dirs.1.clone());
    let lb = fresh.find(&id).expect("the reloaded bank lists the brush");
    assert_eq!((lb.brush.spacing, lb.brush.tilt_size, lb.brush.tilt_rotation), (0.37, 0.65, true));
    // Everything else, and the tip and grain images, came back too.
    let mut expect = original.brush.clone();
    expect.spacing = 0.37;
    expect.tilt_size = 0.65;
    expect.tilt_rotation = true;
    let mut got = lb.brush.clone();
    got.name = expect.name;
    assert_eq!(got, expect);
    assert_eq!(lb.tip, original.tip);
    assert_eq!(lb.grain, original.grain);
    assert_eq!(lb.tip_kind, original.tip_kind);
    assert_eq!(lb.grain_kind, original.grain_kind);
    let set = fresh.sets.iter().find(|s| s.name == MY_BRUSHES_SET).unwrap();
    assert_eq!(set.brushes.len(), 1);
    assert_eq!(set.brushes[0].name, "Persistent pen");
}

#[test]
fn a_tip_with_bright_corners_is_not_flipped_by_a_save() {
    // The importer inverts a tip whose corners are bright (dark shape on white paper). Eskiz's
    // own masks are marked, so a square tip must come back as it was.
    let mut h = headless(false);
    open_studio(&mut h);
    h.app.studio_edit(|w| w.set_tip(TipKind::Square));
    let tip = h.app.studio.as_ref().unwrap().working.tip.clone();
    assert!(tip.get_pixel(tip.width() / 2, tip.height() / 2)[0] > 200);
    let id = h.app.studio_save().unwrap();
    let back = h.app.bank.find(&id).unwrap();
    assert_eq!(back.tip, tip);
    assert_eq!(back.tip_kind, Some(TipKind::Square));
}

#[test]
fn reset_goes_back_to_the_opened_or_last_saved_state() {
    let mut h = headless(false);
    open_studio(&mut h);
    let opened = h.app.studio.as_ref().unwrap().working.clone();
    h.app.studio_edit(|w| {
        w.brush.size = 90.0;
        w.set_grain(Some(GrainKind::Concrete));
    });
    h.app.studio_rename("Changed".into());
    h.advance(0.2);
    assert!(h.app.studio.as_ref().unwrap().dirty());
    click_button(&mut h, |l| l.reset);
    let st = h.app.studio.as_ref().unwrap();
    assert_eq!(st.working, opened);
    assert_eq!(st.name, st.saved_name);
    assert!(!st.dirty());

    // After a save, Reset goes back to the saved state.
    h.app.studio_edit_brush(|b| b.flow = 0.4);
    h.app.studio_save();
    h.app.studio_edit_brush(|b| b.flow = 0.9);
    h.app.studio_reset();
    assert_eq!(h.app.studio.as_ref().unwrap().working.brush.flow, 0.4);
}

#[test]
fn double_tap_resets_one_slider_to_its_default() {
    let mut h = headless(false);
    open_studio(&mut h);
    click_section(&mut h, Section::Pencil);
    h.app.studio_edit_brush(|b| b.tilt_roundness = 0.8);
    h.advance(0.2);
    let r = slider_rect(&h, "Flatten");
    h.click(r.center());
    h.click(r.center());
    h.advance(0.1);
    assert_eq!(h.app.studio.as_ref().unwrap().working.brush.tilt_roundness, 0.0);
}

#[test]
fn duplicate_saves_a_copy_and_goes_on_editing_the_copy() {
    let mut h = headless(false);
    open_studio(&mut h);
    let source = h.app.studio.as_ref().unwrap().target.clone();
    let name = h.app.studio.as_ref().unwrap().name.clone();
    click_button(&mut h, |l| l.duplicate);
    let st = h.app.studio.as_ref().unwrap();
    assert!(st.is_user_brush());
    assert_eq!(st.name, format!("{name} copy"));
    assert_eq!(user_brush_count(&h), 1);
    assert_eq!(h.app.tool_brushes[0], st.target);
    // Duplicating a copy makes another one.
    click_button(&mut h, |l| l.duplicate);
    assert_eq!(user_brush_count(&h), 2);
    assert_eq!(h.app.studio.as_ref().unwrap().name, format!("{name} copy 2"));
    assert!(h.app.bank.find(&source).is_some());
    // Both are listed in My Brushes in the library.
    h.app.close_studio();
    h.advance(0.2);
    h.open_panel(Panel::Brush);
    let last = h.app.library_layout.sets.len() - 1;
    h.click(h.app.library_layout.sets[last].center());
    h.advance(0.4);
    assert_eq!(h.app.library_layout.rows.len(), 2);
    h.render().save_png(&snapshot_dir().join("brush_library_my_brushes.png")).unwrap();
}

#[test]
fn size_and_opacity_edits_reach_the_tools_sliders_on_save() {
    let mut h = headless(false);
    open_studio(&mut h);
    h.app.studio_edit_brush(|b| {
        b.size = 77.0;
        b.opacity = 0.4;
    });
    assert_ne!(h.app.tool_params[0].size, 77.0, "nothing changes before Save");
    h.app.studio_save();
    assert_eq!(h.app.tool_params[0].size, 77.0);
    assert!((h.app.tool_params[0].opacity - 0.4).abs() < 1e-6);
    // A save without such an edit leaves the tool's own sliders alone.
    h.app.tool_params[0].size = 30.0;
    h.app.studio_edit_brush(|b| b.flow = 0.5);
    h.app.studio_save();
    assert_eq!(h.app.tool_params[0].size, 30.0);
}

#[test]
fn tip_and_grain_pickers_choose_from_the_generated_textures() {
    let mut h = headless(false);
    open_studio(&mut h);
    click_section(&mut h, Section::Shape);
    h.advance(0.1);
    let cells = h.app.studio.as_ref().unwrap().layout.cells.clone();
    assert_eq!(cells.iter().filter(|c| c.0).count(), TipKind::ALL.len());
    let (_, idx, rect) = cells.iter().find(|(tip, i, _)| *tip && TipKind::ALL[*i] == TipKind::Bristle).cloned().unwrap();
    assert_eq!(TipKind::ALL[idx], TipKind::Bristle);
    h.click(rect.center());
    h.advance(0.1);
    let w = &h.app.studio.as_ref().unwrap().working;
    assert_eq!(w.tip_kind, Some(TipKind::Bristle));
    assert_eq!(w.tip, eskiz_brushlib::generated_tip(TipKind::Bristle));

    click_section(&mut h, Section::Grain);
    h.advance(0.1);
    let cells = h.app.studio.as_ref().unwrap().layout.cells.clone();
    assert_eq!(cells.iter().filter(|c| !c.0).count(), GrainKind::ALL.len() + 1, "None plus every grain");
    // Cell 0 is None; the cell after it is the first real grain.
    let (_, _, rect) = cells.iter().filter(|c| !c.0).nth(2).cloned().unwrap();
    h.click(rect.center());
    h.advance(0.1);
    let w = &h.app.studio.as_ref().unwrap().working;
    assert_eq!(w.grain_kind, Some(GrainKind::ALL[1]));
    assert!(w.grain.is_some() && w.brush.grain != Grain::None && w.brush.grain_depth > 0.0);
    let (_, _, none) = cells.iter().find(|c| !c.0).cloned().unwrap();
    h.click(none.center());
    h.advance(0.1);
    let w = &h.app.studio.as_ref().unwrap().working;
    assert!(w.grain.is_none() && w.brush.grain == Grain::None);
}

#[test]
fn taper_tabs_edit_the_pencil_and_touch_tapers_separately() {
    let mut h = headless(false);
    open_studio(&mut h);
    click_section(&mut h, Section::Taper);
    h.advance(0.1);
    let before = h.app.studio.as_ref().unwrap().working.brush.taper;
    let touch = h.app.studio.as_ref().unwrap().layout.segments.iter().find(|(l, _)| l == "Touch").unwrap().1;
    h.click(touch.center());
    h.advance(0.2);
    assert!(h.app.studio.as_ref().unwrap().finger_taper);
    drag_slider(&mut h, "Size", 1.0, 0.2);
    let b = &h.app.studio.as_ref().unwrap().working.brush;
    assert!(b.taper.finger.size < before.finger.size, "{:?}", b.taper);
    assert_eq!(b.taper.pencil, before.pencil);
    h.render().save_png(&snapshot_dir().join("studio_taper_touch_dark.png")).unwrap();
}

#[test]
fn a_curve_can_be_edited() {
    let mut h = headless(false);
    open_studio(&mut h);
    click_section(&mut h, Section::Pencil);
    h.advance(0.2);
    let sheet = h.app.studio.as_ref().unwrap().layout.sheet.unwrap();
    h.scroll_at(pos2(sheet.left() + 400.0, sheet.center().y), -1000.0);
    h.render().save_png(&snapshot_dir().join("studio_pencil_curves_dark.png")).unwrap();
    h.app.studio_edit_brush(|b| b.curves.overall = vec![[0.0, 0.0], [0.5, 0.8], [1.0, 1.0]]);
    h.advance(0.1);
    assert_eq!(h.app.studio.as_ref().unwrap().working.brush.curves.overall.len(), 3);
    h.render().save_png(&snapshot_dir().join("studio_pencil_curves_edited_dark.png")).unwrap();
}

#[test]
fn deleting_a_my_brushes_entry_removes_its_file_and_falls_back() {
    let dirs = stores("delete");
    let mut h = headless(false);
    h.app.bank = BrushBank::with_stores(dirs.0.clone(), dirs.1.clone());
    open_studio(&mut h);
    let id = h.app.studio_save().unwrap();
    let BrushId::User(key) = &id else { panic!() };
    assert!(dirs.1.contains(key));
    click_section(&mut h, Section::About);
    h.advance(0.2);
    h.render().save_png(&snapshot_dir().join("studio_about_dark.png")).unwrap();
    h.app.studio_delete();
    assert!(h.app.studio.is_none());
    assert!(!dirs.1.contains(key));
    assert_eq!(user_brush_count(&h), 0);
    assert_eq!(h.app.tool_brushes[0], crate::brush_library::default_brush(0));
}

#[test]
fn a_library_brush_is_never_deleted_or_overwritten() {
    let mut h = headless(false);
    open_studio(&mut h);
    h.app.studio_delete();
    assert!(h.app.studio.is_some(), "Delete is for My Brushes only");
}

// ------------------------------------------------------------------ imported brushes

#[test]
fn an_imported_brush_opens_edits_and_saves_without_losing_its_other_settings() {
    use eskiz_format::brushes::{keys, ImportedBrush, SettingValue};
    let mut h = headless(false);
    let mut ib = ImportedBrush { name: "Foreign ink".into(), ..ImportedBrush::default() };
    ib.settings.insert(keys::PLOT_SPACING.into(), SettingValue::Number(0.3));
    ib.settings.insert("someOtherKey".into(), SettingValue::Number(7.0));
    let id = h.app.bank.add_imported(&ib).unwrap();
    h.app.tool_brushes[0] = id.clone();
    assert!(h.app.open_studio(Tool::Brush, id.clone()));
    assert_eq!(h.app.studio.as_ref().unwrap().working.brush.spacing, 0.3);
    h.app.studio_edit_brush(|b| b.tilt_size = 0.5);
    let saved = h.app.studio_save().unwrap();
    assert_ne!(saved, id, "Save copies an imported brush into My Brushes");
    let src = h.app.bank.source(&saved).unwrap();
    assert_eq!(src.settings.get("someOtherKey"), Some(&SettingValue::Number(7.0)));
    assert_eq!(src.name, "Foreign ink");
    assert_eq!(h.app.bank.find(&saved).unwrap().brush.tilt_size, 0.5);
    assert!(h.app.bank.find(&id).is_some(), "the imported brush is still in the Imported set");
}

// ------------------------------------------------------------------ the pad

#[test]
fn the_pad_paints_with_the_brush_and_clear_wipes_it() {
    let mut h = headless(false);
    open_studio(&mut h);
    let pad = h.app.studio.as_ref().unwrap().layout.pad.unwrap();
    assert!(pad.width() > 250.0 && pad.height() > 250.0, "{pad:?}");
    assert_eq!(h.app.studio.as_ref().unwrap().pad.ink_pixels(8), 0);
    scribble(&mut h, pad);
    let ink = h.app.studio.as_ref().unwrap().pad.ink_pixels(64);
    assert!(ink > 300, "a scribble leaves ink on the pad ({ink})");
    assert_eq!(h.app.studio.as_ref().unwrap().pad.strokes, 1);
    h.render().save_png(&snapshot_dir().join("studio_pad_scribble_dark.png")).unwrap();
    // The canvas behind it was not touched.
    assert!(h.app.session.doc.layer(h.app.active).unwrap().to_rgba().iter().all(|b| *b == 0));
    click_button(&mut h, |l| l.clear);
    assert_eq!(h.app.studio.as_ref().unwrap().pad.ink_pixels(8), 0);
}

#[test]
fn a_setting_changes_what_the_pad_paints_and_what_the_strip_shows() {
    let mut h = headless(false);
    open_studio(&mut h);
    let pad = h.app.studio.as_ref().unwrap().layout.pad.unwrap();
    let rev = h.app.studio.as_ref().unwrap().rev;
    scribble(&mut h, pad);
    let first = h.app.studio.as_ref().unwrap().pad.rgba();
    let ink_first = h.app.studio.as_ref().unwrap().pad.ink_pixels(64);
    h.app.studio_edit(|w| {
        w.brush.size = 70.0;
        w.set_tip(TipKind::ChalkEdge);
        w.set_grain(Some(GrainKind::CanvasWeave));
        w.brush.grain_depth = 1.0;
    });
    h.advance(0.3);
    assert_ne!(h.app.studio.as_ref().unwrap().rev, rev);
    h.app.studio.as_mut().unwrap().pad.clear();
    h.advance(0.1);
    scribble(&mut h, pad);
    let second = h.app.studio.as_ref().unwrap().pad.rgba();
    assert_ne!(first, second, "a new tip and grain paint a different picture");
    assert!(h.app.studio.as_ref().unwrap().pad.ink_pixels(64) > ink_first, "a bigger tip leaves more ink");
    h.render().save_png(&snapshot_dir().join("studio_pad_edited_dark.png")).unwrap();
}

#[test]
fn the_pad_follows_pen_tilt() {
    use eskiz_core::geometry::{PointerType, Sample};
    let lb = &library_ref()[0].brushes[0];
    let w = crate::studio::StudioBrush::from_library(lb);
    let mut pad = crate::studio::Pad::default();
    pad.ensure(300, 160, 1);
    let brush = Brush { size: 16.0, tilt_size: 1.0, ..w.brush.clone() };
    let raster = {
        let mut sb = w.clone();
        sb.brush = brush.clone();
        crate::brush_library::engine_for(&sb.library_brush()).raster
    };
    let mut ink_for = |alt: f64| {
        pad.clear();
        pad.begin(&brush, raster.clone());
        for i in 0..40 {
            pad.push(Sample {
                x: 30.0 + f64::from(i) * 6.0,
                y: 80.0,
                t: f64::from(i) / 120.0,
                pressure: Some(0.8),
                tilt: Some(alt),
                azimuth: Some(0.0),
                pointer: PointerType::Pen,
                has_pressure: Some(true),
                ..Sample::default()
            });
        }
        pad.end();
        pad.ink_pixels(64)
    };
    let upright = ink_for(std::f64::consts::FRAC_PI_2);
    let leaning = ink_for(0.6);
    assert!(leaning > upright * 2, "a leaning pen paints a fatter line ({leaning} vs {upright})");
}

// ------------------------------------------------------------------ the tablet reaches the engine

#[test]
fn tablet_tilt_and_azimuth_reach_the_stroke_engine() {
    use crate::input::{apply_pressure_curve, sample_from_pen};
    use crate::pressure::PressureCurve;
    use crate::tablet::{reading_from_pose, PenPose};
    use eskiz_core::geometry::{Stroke, StrokeOptions};

    // A brush that asks for tilt size and azimuth-aligned tips.
    let brush = Brush { size: 20.0, tilt_size: 0.8, tilt_rotation: true, pressure_size: 0.0, ..Brush::default() };
    let run = |tilt: [f32; 2]| {
        let mut s = Stroke::new(brush.clone(), StrokeOptions::default());
        for i in 0..30 {
            let pose = PenPose { pos: [20.0 + i as f32 * 3.0, 40.0], pressure: Some(0.7), tilt: Some(tilt), roll: None, time: None };
            let sample = sample_from_pen(f64::from(pose.pos[0]), f64::from(pose.pos[1]), f64::from(i) / 120.0, reading_from_pose(&pose));
            s.add_sample(apply_pressure_curve(sample, &PressureCurve::default()));
        }
        s.finish().dabs
    };
    let upright = run([0.0, 0.0]);
    let leaning = run([0.9, 0.0]);
    let mid = |d: &[eskiz_core::Dab]| d[d.len() / 2];
    assert!(mid(&leaning).sx > mid(&upright).sx * 1.5, "tilt grows the tip: {} vs {}", mid(&leaning).sx, mid(&upright).sx);
    // Leaning towards +x: azimuth 0. Leaning towards +y: azimuth 90 degrees.
    let toward_y = run([0.0, 0.9]);
    assert!(mid(&leaning).angle.abs() < 1e-6, "{}", mid(&leaning).angle);
    assert!((mid(&toward_y).angle - std::f64::consts::FRAC_PI_2).abs() < 1e-6, "{}", mid(&toward_y).angle);
}

// ------------------------------------------------------------------ previews

#[test]
fn saving_refreshes_the_cached_preview_of_that_brush() {
    let mut h = headless(false);
    open_studio(&mut h);
    h.app.studio_edit_brush(|b| b.spacing = 0.2);
    let id = h.app.studio_save().unwrap();
    h.app.close_studio();
    h.advance(0.2);
    h.open_panel(Panel::Brush);
    let last = h.app.library_layout.sets.len() - 1;
    h.click(h.app.library_layout.sets[last].center());
    h.advance(0.5);
    let scale = crate::brush_library::BrushPreviews::scale_for(h.ctx.pixels_per_point());
    assert!(h.app.previews.is_cached(&id, scale));
    // Save over it with a visible change: the stale preview is dropped, then drawn again.
    h.app.open_studio(Tool::Brush, id.clone());
    h.app.studio_edit_brush(|b| b.size = 90.0);
    h.app.studio_save();
    assert!(!h.app.previews.is_cached(&id, scale));
    h.app.close_studio();
    h.advance(0.2);
    h.open_panel(Panel::Brush);
    h.advance(0.5);
    assert!(h.app.previews.is_cached(&id, scale));
}

#[test]
fn prefs_remember_a_my_brushes_choice() {
    let mut h = headless(false);
    open_studio(&mut h);
    let id = h.app.studio_save().unwrap();
    let text = h.app.brush_choice_text();
    assert!(text.starts_with("usr:"), "{text}");
    h.app.tool_brushes[0] = crate::brush_library::default_brush(0);
    h.app.set_brush_choice(&text);
    assert_eq!(h.app.tool_brushes[0], id);
}

#[test]
fn saving_a_built_in_brush_twice_numbers_the_second_copy() {
    let mut h = headless(false);
    open_studio(&mut h);
    let source = h.app.studio.as_ref().unwrap().target.clone();
    let first = h.app.studio_save().unwrap();
    h.app.studio.as_mut().unwrap().target = source;
    let second = h.app.studio_save().unwrap();
    assert_ne!(first, second);
    let set = h.app.bank.sets.iter().find(|s| s.name == MY_BRUSHES_SET).unwrap();
    let names: Vec<String> = set.brushes.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names.len(), 2);
    assert_eq!(names[1], format!("{} 2", names[0]), "the rows can be told apart");
    assert_eq!(h.app.studio.as_ref().unwrap().name, names[1], "the Studio shows the new name");
}

#[test]
fn a_missing_my_brushes_choice_falls_back_to_the_default() {
    let mut h = headless(false);
    h.app.set_brush_choice("usr:Gone|lib:no.such.brush|");
    assert_eq!(h.app.tool_brushes[0], crate::brush_library::default_brush(0), "a brush that is not in the bank is not kept");
}
