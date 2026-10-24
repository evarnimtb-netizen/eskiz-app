//! Unit tests for text layers: create, edit, rerender, rasterize, undo, the editing helpers and
//! the `.procreate` round trip. Tests that need glyphs skip themselves when no fonts are installed.

use std::fs;

use eskiz_text::{Affine, Attrs, Alignment, Color, FontFace, FontLibrary, Run, TextLayer};

use crate::session::Session;
use crate::text_tool::{blit, delete_range, from_spec, insert_at, render_text, to_spec, ADD_STEP, EDIT_STEP, RASTERIZE_STEP, STYLE_STEP};

const W: usize = 240;
const H: usize = 160;

fn session() -> Session {
    Session::new(eskiz_core::document::Document::new(W, H))
}

fn fonts_or_skip() -> Option<FontLibrary> {
    let fonts = FontLibrary::new();
    if fonts.families().is_empty() {
        eprintln!("skipped: no system fonts");
        None
    } else {
        Some(fonts)
    }
}

fn ink(session: &Session, id: u64) -> bool {
    session.doc.layer(id).is_some_and(|l| l.to_rgba().as_chunks::<4>().0.iter().any(|p| p[3] != 0))
}

#[test]
fn text_layer_create_edit_rerender_and_rasterize() {
    let Some(mut fonts) = fonts_or_skip() else { return };
    let mut s = session();
    let above = s.doc.layer_ids().last().copied();
    let layer = TextLayer::new("", Attrs::default(), 0.0);
    let blank = render_text(&mut fonts, &layer, W, H);
    let id = s.add_text_layer(above, layer, blank).unwrap().id.unwrap();
    assert_eq!(s.undo_name(), Some(ADD_STEP));
    assert!(s.doc.texts.contains_key(&id));
    assert!(!s.can_paint(id), "a text layer is edited through its source, not painted");

    // Typing: live changes under one name are one step.
    let mut t = s.doc.texts[&id].clone();
    insert_at(&mut t, 0, "He");
    let px = render_text(&mut fonts, &t, W, H);
    s.text_live(id, EDIT_STEP, t.clone(), px).unwrap();
    insert_at(&mut t, 2, "llo");
    let px = render_text(&mut fonts, &t, W, H);
    s.text_live(id, EDIT_STEP, t.clone(), px).unwrap();
    assert!(ink(&s, id), "rerendered pixels are on the layer while editing");
    let steps = s.undo_steps();
    s.text_commit().unwrap();
    assert_eq!(s.undo_steps(), steps + 1, "one typing session is one step");
    assert_eq!(s.undo_name(), Some(EDIT_STEP));
    assert_eq!(s.doc.texts[&id].text, "Hello");

    // Style: a different name is its own step.
    let mut big = s.doc.texts[&id].clone();
    big.set_attrs(0..big.text.len(), |a| a.size_pt = 40.0);
    let px = render_text(&mut fonts, &big, W, H);
    s.text_live(id, STYLE_STEP, big, px).unwrap();
    s.text_commit().unwrap();
    assert_eq!(s.undo_name(), Some(STYLE_STEP));

    // Rasterize: the source goes, the pixels stay, and undo brings the source back.
    s.rasterize_text(id).unwrap();
    assert!(!s.doc.texts.contains_key(&id));
    assert!(ink(&s, id));
    assert!(s.can_paint(id));
    assert_eq!(s.undo_name(), Some(RASTERIZE_STEP));
    s.undo().unwrap();
    assert_eq!(s.doc.texts[&id].text, "Hello");
    assert_eq!(s.doc.texts[&id].runs[0].attrs.size_pt, 40.0);

    // Undo the style, the typing and the creation in turn.
    s.undo().unwrap();
    assert_eq!(s.doc.texts[&id].runs[0].attrs.size_pt, 12.0);
    s.undo().unwrap();
    assert_eq!(s.doc.texts[&id].text, "");
    s.undo().unwrap();
    assert!(!s.doc.texts.contains_key(&id));
}

#[test]
fn text_layer_delete_and_undo_keep_the_source() {
    let mut s = session();
    let layer = TextLayer::new("abc", Attrs::default(), 0.0);
    let px = vec![0u8; W * H * 4];
    let id = s.add_text_layer(None, layer.clone(), px).unwrap().id.unwrap();
    s.delete_layer(id).unwrap();
    assert!(!s.doc.texts.contains_key(&id));
    s.undo().unwrap();
    assert_eq!(s.doc.texts.get(&id), Some(&layer));
}

#[test]
fn blit_clips_to_the_canvas() {
    // Raster pixel (0, 0) lands at x = -1 and is clipped; (1, 0) lands at canvas (0, 1).
    let raster = eskiz_text::Raster {
        origin: (-1, 1),
        width: 2,
        height: 2,
        pixels: vec![255, 255, 255, 255, 10, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0],
    };
    let out = blit(&raster, 2, 2);
    assert_eq!(out.len(), 16);
    assert_eq!(&out[0..8], &[0; 8], "row 0 is outside the raster");
    assert_eq!(&out[8..12], &[10, 0, 0, 255]);
    assert_eq!(&out[12..16], &[0, 0, 0, 0]);
}

#[test]
fn editing_helpers_keep_runs_in_step_with_the_text() {
    let mut t = TextLayer::new("abcdef", Attrs::default(), 0.0);
    t.set_attrs(2..4, |a| a.size_pt = 30.0);
    // Typing inside the big run inherits its attributes.
    insert_at(&mut t, 3, "X");
    assert_eq!(t.text, "abcXdef");
    assert_eq!(t.attrs_at(3).unwrap().size_pt, 30.0);
    assert_eq!(t.runs.iter().map(|r| r.len).sum::<usize>(), t.text.len());
    // A delete across the run boundary keeps the runs summing to the text length.
    delete_range(&mut t, 1..5);
    assert_eq!(t.text, "aef");
    assert_eq!(t.runs.iter().map(|r| r.len).sum::<usize>(), 3);
    assert_eq!(t.attrs_at(0).unwrap().size_pt, Attrs::default().size_pt);
}

#[test]
fn spec_round_trip_keeps_every_attribute() {
    let attrs = Attrs {
        font_family: "Helvetica".to_owned(),
        font_face: FontFace::BoldItalic,
        size_pt: 33.5,
        kerning: 0.05,
        tracking: -0.02,
        leading: 1.25,
        baseline_shift_pt: -4.0,
        alignment: Alignment::Justified,
        outline_pt: 2.0,
        underline: true,
        color: Color::rgba(10, 20, 30, 200),
        all_caps: true,
        vertical: false,
        ..Attrs::default()
    };
    let mut layer = TextLayer::new("Hi\nthere", attrs, 180.0);
    layer.runs.push(Run { len: 0, attrs: Attrs::default() });
    layer.transform = Affine::new(1.0, 0.5, -0.5, 1.0, 12.0, 34.0);
    let spec = to_spec(&layer);
    let back = from_spec(&spec).expect("runs cover the text");
    assert_eq!(back.text, layer.text);
    assert_eq!(back.box_width, layer.box_width);
    assert_eq!(back.transform, layer.transform);
    assert_eq!(back.runs[0].attrs, layer.runs[0].attrs);
    // A spec whose runs do not cover the text is refused rather than guessed at.
    let mut broken = spec.clone();
    broken.text.push('!');
    assert!(from_spec(&broken).is_none());
}

#[test]
fn text_layer_survives_save_and_load_as_procreate() {
    let Some(mut fonts) = fonts_or_skip() else { return };
    let mut s = session();
    let mut layer = TextLayer::new("Eskiz", Attrs { size_pt: 30.0, ..Attrs::default() }, 0.0);
    layer.transform = Affine::translation(20.0, 30.0);
    let px = render_text(&mut fonts, &layer, W, H);
    let id = s.add_text_layer(None, layer.clone(), px).unwrap().id.unwrap();

    let builder = crate::storage::to_builder(&s.doc, "Text round trip");
    let dir = std::env::temp_dir().join(format!("eskiz-text-app-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("text.procreate");
    eskiz_format::document::write(&path, &builder).unwrap();
    let loaded = crate::import::open_procreate(&path).unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert_eq!(loaded.texts.len(), 1);
    let (_, text) = loaded.texts.iter().next().unwrap();
    assert_eq!(text, &layer);
    assert!(s.doc.texts.contains_key(&id));
}

/// The Edit Style window over a text layer, rendered headlessly for inspection.
#[test]
fn edit_style_panel_snapshot() {
    use eframe::egui::vec2;

    use crate::app::Prefs;
    use crate::document::Document;
    use crate::snapshot::{snapshot_dir, Headless};

    if fonts_or_skip().is_none() {
        return;
    }
    let mut h = Headless::new(vec2(900.0, 640.0), Document::new(600, 400), Prefs::default());
    h.app.add_text_action();
    h.app.text_replace(0..0, "Eskiz");
    h.advance(0.6);
    h.step();
    assert!(h.app.text_target().is_some(), "the Edit Style window has a target");
    assert!(h.app.session.doc.texts.values().any(|t| t.text == "Eskiz"));
    h.render().save_png(&snapshot_dir().join("text_edit_style_dark.png")).unwrap();
}

#[test]
fn undo_while_typing_keeps_the_caret_inside_the_text() {
    let mut h = crate::snapshot::Headless::new(
        eframe::egui::Vec2::new(800.0, 600.0),
        eskiz_core::document::Document::new(W, H),
        crate::app::Prefs::default(),
    );
    let px = vec![0u8; W * H * 4];
    let layer = TextLayer::new("", Attrs::default(), 0.0);
    let id = h.app.session.add_text_layer(None, layer, px.clone()).unwrap().id.unwrap();
    let mut t = h.app.session.doc.texts[&id].clone();
    insert_at(&mut t, 0, "Hello world");
    h.app.session.text_live(id, EDIT_STEP, t, px).unwrap();
    h.app.text.editing = Some(id);
    h.app.text.caret = 11;
    h.app.text.anchor = Some(5);
    h.app.undo_action();
    let len = h.app.session.doc.texts[&id].text.len();
    assert!(len < 11, "undo took the typing back");
    assert!(h.app.text.caret <= len, "caret {} past the text ({len} bytes)", h.app.text.caret);
    assert!(h.app.text.anchor.is_none_or(|a| a <= len));
}
