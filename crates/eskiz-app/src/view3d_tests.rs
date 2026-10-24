//! Tests for 3D painting: import of a generated cube with its checks, painting at the centre of
//! the view, GLB export and re-import, the orbit camera changing the lookup, the library round
//! trip, and a headless render of the 3D view (PNG in the snapshot folder).

use std::path::PathBuf;

use eskiz_3d::model::MapKind;
use eskiz_3d::{export_glb, load_gltf, load_gltf_slice, render_lookup, write_glb_bytes, ImportOptions, RenderOptions};
use eskiz_core::Brush;
use eframe::egui::Vec2;

use crate::app::{EskizApp, Prefs};
use crate::document::Document;
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless};
use crate::storage::Library;
use crate::view3d::{
    cube_glb, import_job, is_model_file, load_model_doc, paint_dabs, save_model_doc, stroke_dabs, ModelDoc, Outcome,
    ThreeD,
};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-3d-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The generated cube as a document, looking straight at the +Z face.
pub(crate) fn cube_doc() -> ModelDoc {
    let model = load_gltf_slice(&cube_glb(true), &ImportOptions::default()).unwrap().model;
    let mut doc = ModelDoc::new("cube".into(), "Cube".into(), model);
    doc.camera = doc.home;
    doc.camera.yaw = 0.0;
    doc.camera.pitch = 0.0;
    doc
}

/// Paint a short red stroke through the centre of a 256 x 256 view. Returns the texel that the
/// centre of the view shows, and its colour before and after.
fn paint_centre(doc: &mut ModelDoc) -> ((usize, usize), [u8; 4], [u8; 4]) {
    let lookup = render_lookup(&doc.model, &doc.camera, 256, 256, RenderOptions::default());
    let (u, v) = lookup.remap(128.0, 128.0).expect("the center of the view shows the cube");
    let set = &doc.model.texture_sets[0];
    let texel = ((u * set.width as f32) as usize, (v * set.height as f32) as usize);
    let before = set.map(MapKind::Colour).pixel(texel.0, texel.1);
    let brush = Brush { size: 40.0, ..Brush::default() };
    let points: Vec<(f64, f64)> = (0..21).map(|i| (118.0 + f64::from(i), 128.0)).collect();
    let dabs = stroke_dabs(&brush, &points);
    assert!(!dabs.is_empty(), "the stroke made dabs");
    paint_dabs(doc, &lookup, brush, [1.0, 0.0, 0.0], false, &dabs);
    let after = doc.model.texture_sets[0].map(MapKind::Colour).pixel(texel.0, texel.1);
    (texel, before, after)
}

#[test]
fn generated_cube_imports_with_clean_checks() {
    let dir = temp_dir("import");
    let path = dir.join("cube.glb");
    std::fs::write(&path, cube_glb(true)).unwrap();
    assert!(is_model_file(&path));
    match import_job(None, path) {
        Outcome::Opened { doc, banner, entry } => {
            assert_eq!(doc.model.triangle_count(), 12);
            assert_eq!(doc.model.texture_sets.len(), 1);
            assert!(banner.is_none(), "the atlas has no overlaps or repeats: {banner:?}");
            assert!(entry.is_none(), "no library, nothing saved");
        }
        Outcome::Failed(lines) => panic!("cube rejected: {lines:?}"),
    }
}

#[test]
fn a_model_without_uvs_is_rejected_with_a_message() {
    let dir = temp_dir("no-uvs");
    let path = dir.join("bare.glb");
    std::fs::write(&path, cube_glb(false)).unwrap();
    match import_job(None, path) {
        Outcome::Failed(lines) => assert!(!lines.is_empty(), "the error sheet needs a message"),
        Outcome::Opened { .. } => panic!("a fatal check must stop the import"),
    }
}

#[test]
fn painting_the_centre_changes_the_base_colour_at_its_uv() {
    let mut doc = cube_doc();
    let (texel, before, after) = paint_centre(&mut doc);
    // The +Z face is cell (1, 1) of the 3 x 2 atlas, so its centre is at UV (0.5, 0.75).
    let set = &doc.model.texture_sets[0];
    // The view is sampled at a pixel corner, so allow a few texels of slack (1/64 of the map).
    let slack = set.width / 64;
    assert!(
        texel.0.abs_diff(set.width / 2) <= slack && texel.1.abs_diff(set.height * 3 / 4) <= slack,
        "the center shows the +Z face center, got {texel:?}"
    );
    assert_ne!(after, before, "painting changed the texel");
    assert!(after[0] > after[1] && after[0] > after[2] && after[3] > 0, "red paint: {after:?}");
    assert!(doc.dirty);
}

#[test]
fn glb_export_reimports_with_the_same_geometry() {
    let dir = temp_dir("glb");
    let doc = cube_doc();
    let path = dir.join("cube.glb");
    export_glb(&doc.model, &path).unwrap();
    let back = load_gltf(&path, &ImportOptions::default()).unwrap().model;
    assert_eq!(back.triangle_count(), doc.model.triangle_count());
    assert_eq!(back.texture_sets.len(), doc.model.texture_sets.len());
    let (lo, hi) = doc.model.bounds().unwrap();
    let (blo, bhi) = back.bounds().unwrap();
    assert!((lo - blo).abs().max_element() < 1e-4 && (hi - bhi).abs().max_element() < 1e-4);
    let in_memory = write_glb_bytes(&doc.model).unwrap();
    assert!(load_gltf_slice(&in_memory, &ImportOptions::default()).is_ok());
}

#[test]
fn orbiting_the_camera_changes_the_lookup() {
    let mut doc = cube_doc();
    let before = render_lookup(&doc.model, &doc.camera, 128, 128, RenderOptions::default());
    doc.camera.orbit(0.6, 0.0);
    let after = render_lookup(&doc.model, &doc.camera, 128, 128, RenderOptions::default());
    assert_ne!(before.front.uv, after.front.uv, "a new view shows other surface points");
    assert_ne!(before.remap(64.0, 64.0), after.remap(64.0, 64.0), "the center now shows another face");
}

#[test]
fn saved_3d_document_reopens_with_its_paint() {
    let dir = temp_dir("library");
    let lib = Library::at(dir.clone());
    let mut doc = cube_doc();
    let (texel, _, painted) = paint_centre(&mut doc);
    doc.paint_through = true;
    let entry = save_model_doc(&lib, &doc).unwrap();
    assert_eq!((entry.width, entry.height), (1024, 1024));
    assert!(lib.list().iter().any(|e| e.id == doc.id), "the gallery lists the document");
    assert!(lib.is_model_doc(&doc.id));
    let back = load_model_doc(&lib, &doc.id).unwrap();
    assert_eq!(back.title, "Cube");
    assert!(back.paint_through);
    assert!(!back.dirty);
    assert_eq!(back.model.texture_sets[0].map(MapKind::Colour).pixel(texel.0, texel.1), painted);
}

/// Names in the library folder that only a save in progress should leave behind.
fn leftovers(dir: &std::path::Path) -> Vec<String> {
    let names = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned());
    names.filter(|n| n.ends_with(".bak") || n.ends_with(".tmp") || n.ends_with(".writing")).collect()
}

/// The colour of `texel` in the first texture set of the saved document `id`.
fn saved_colour(lib: &Library, id: &str, texel: (usize, usize)) -> [u8; 4] {
    load_model_doc(lib, id).unwrap().model.texture_sets[0].map(MapKind::Colour).pixel(texel.0, texel.1)
}

#[test]
fn resaving_a_3d_document_replaces_it_and_leaves_nothing_behind() {
    let dir = temp_dir("resave");
    let lib = Library::at(dir.clone());
    let mut doc = cube_doc();
    let (texel, before, _) = paint_centre(&mut doc);
    let first = doc.model.texture_sets[0].map(MapKind::Colour).pixel(texel.0, texel.1);
    save_model_doc(&lib, &cube_doc()).unwrap();
    assert_eq!(saved_colour(&lib, &doc.id, texel), before);
    // A folder from an older build's save is cleared up too.
    std::fs::create_dir_all(lib.model_dir(&doc.id).with_extension("writing")).unwrap();
    doc.title = "Painted cube".into();
    save_model_doc(&lib, &doc).unwrap();
    let back = load_model_doc(&lib, &doc.id).unwrap();
    assert_eq!(back.title, "Painted cube");
    assert_eq!(back.model.texture_sets[0].map(MapKind::Colour).pixel(texel.0, texel.1), first);
    assert_eq!(lib.list().iter().filter(|e| e.id == doc.id).count(), 1);
    assert!(leftovers(&dir).is_empty(), "left behind: {:?}", leftovers(&dir));
}

#[test]
fn a_failed_3d_save_keeps_the_last_good_copy() {
    use crate::storage::{FAIL_NEXT_CHECK, FAIL_NEXT_WRITE};
    let dir = temp_dir("failed-save");
    let lib = Library::at(dir.clone());
    let mut doc = cube_doc();
    save_model_doc(&lib, &doc).unwrap();
    let (texel, before, painted) = paint_centre(&mut doc);
    assert_ne!(before, painted);

    // The new folder is written in full, swapped in, and then fails its check: the old one returns.
    FAIL_NEXT_CHECK.with(|c| c.set(true));
    assert!(save_model_doc(&lib, &doc).is_err());
    assert_eq!(saved_colour(&lib, &doc.id, texel), before, "the original document is intact");
    assert!(leftovers(&dir).is_empty(), "left behind: {:?}", leftovers(&dir));

    // Writing the new folder fails (disk full): the old one is never touched.
    FAIL_NEXT_WRITE.with(|c| c.set(Some(std::io::ErrorKind::StorageFull)));
    let err = save_model_doc(&lib, &doc).unwrap_err();
    assert!(err.contains("disk is full"), "{err}");
    assert_eq!(saved_colour(&lib, &doc.id, texel), before);
    assert!(leftovers(&dir).is_empty(), "left behind: {:?}", leftovers(&dir));

    // The first save of a new document fails its check: nothing is left under its name.
    let mut fresh = cube_doc();
    fresh.id = "fresh".into();
    FAIL_NEXT_CHECK.with(|c| c.set(true));
    assert!(save_model_doc(&lib, &fresh).is_err());
    assert!(!lib.model_dir("fresh").exists());

    save_model_doc(&lib, &doc).unwrap();
    assert_eq!(saved_colour(&lib, &doc.id, texel), painted, "the next save goes through");
}

#[test]
fn an_interrupted_3d_save_is_recovered_from_its_backup() {
    let dir = temp_dir("interrupted");
    let lib = Library::at(dir.clone());
    let mut doc = cube_doc();
    save_model_doc(&lib, &doc).unwrap();
    let (texel, before, painted) = paint_centre(&mut doc);
    let (folder, bak, tmp) = (lib.model_dir(&doc.id), lib.model_bak(&doc.id), lib.model_tmp(&doc.id));

    // Cut off after the old folder became the backup and before the new one moved in: the
    // gallery still lists the document, and it opens with its last saved paint.
    std::fs::rename(&folder, &bak).unwrap();
    std::fs::create_dir_all(tmp.join("layers")).unwrap();
    assert!(lib.list().iter().any(|e| e.id == doc.id), "the gallery finds the backup");
    assert_eq!(saved_colour(&lib, &doc.id, texel), before);
    assert!(!bak.exists());

    // The same, found by opening the document rather than by the gallery.
    std::fs::rename(&folder, &bak).unwrap();
    assert_eq!(saved_colour(&lib, &doc.id, texel), before);
    assert!(folder.is_dir() && !bak.exists());

    // Cut off after the new folder moved in and before the backup was removed: the new copy
    // checks out, so it stays and the backup goes.
    let old = dir.join("old-copy");
    copy_tree(&folder, &old);
    save_model_doc(&lib, &doc).unwrap();
    copy_tree(&old, &bak);
    assert_eq!(saved_colour(&lib, &doc.id, texel), painted);
    assert!(!bak.exists());

    // A damaged folder beside a backup: the backup wins.
    copy_tree(&old, &bak);
    std::fs::write(folder.join("model.glb"), b"glTF").unwrap();
    assert_eq!(saved_colour(&lib, &doc.id, texel), before);
    assert!(!bak.exists());

    // The next save also settles a leftover backup before it writes.
    copy_tree(&old, &bak);
    std::fs::remove_dir_all(&folder).unwrap();
    save_model_doc(&lib, &doc).unwrap();
    assert_eq!(saved_colour(&lib, &doc.id, texel), painted);
    std::fs::remove_dir_all(&old).unwrap();
    assert!(leftovers(&dir).is_empty(), "left behind: {:?}", leftovers(&dir));
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &dst);
        } else {
            std::fs::copy(entry.path(), dst).unwrap();
        }
    }
}

#[test]
fn headless_view_renders_the_model_with_a_banner() {
    let out = snapshot_dir();
    std::fs::create_dir_all(&out).unwrap();
    let mut h = Headless::from_app(Vec2::new(640.0, 400.0), |ctx| {
        EskizApp::new(ctx, Document::new(64, 64), String::new(), Prefs::default())
    });
    let mut doc = cube_doc();
    doc.camera = doc.home;
    h.app.three_d = Some(Box::new(ThreeD::new(doc, Some("Test banner: texture repeating".into()))));
    h.advance(0.1);
    let image = h.render();
    image.save_png(&out.join("view3d_cube.png")).unwrap();
    assert!(h.app.three_d.is_some());
}

#[test]
fn actions_panel_shows_the_3d_and_share_tabs() {
    let out = snapshot_dir();
    std::fs::create_dir_all(&out).unwrap();
    let mut h = Headless::from_app(Vec2::new(1000.0, 560.0), |ctx| {
        EskizApp::new(ctx, Document::new(64, 64), String::new(), Prefs::default())
    });
    let mut doc = cube_doc();
    doc.camera = doc.home;
    h.app.three_d = Some(Box::new(ThreeD::new(doc, None)));
    h.app.actions_ui.tab = ActionsTab::ThreeD;
    h.open_panel(Panel::Actions);
    h.advance(0.1);
    h.render().save_png(&out.join("actions_three_d.png")).unwrap();
    h.app.actions_ui.tab = ActionsTab::Share;
    h.advance(0.1);
    h.render().save_png(&out.join("actions_three_d_share.png")).unwrap();
    assert!(h.app.three_d.is_some());
}

#[test]
fn text_tool_is_refused_while_the_3d_view_is_open() {
    let out = snapshot_dir();
    std::fs::create_dir_all(&out).unwrap();
    let mut h = Headless::from_app(Vec2::new(1000.0, 560.0), |ctx| {
        EskizApp::new(ctx, Document::new(64, 64), String::new(), Prefs::default())
    });
    let layers = h.app.session.doc.texts.len();
    let mut doc = cube_doc();
    doc.camera = doc.home;
    h.app.three_d = Some(Box::new(ThreeD::new(doc, None)));
    h.app.add_text_action();
    assert!(h.app.text.editing.is_none(), "no text editing in the 3D view");
    assert_eq!(h.app.session.doc.texts.len(), layers, "no text layer was added");
    // The Add tab still lists the text rows (greyed out); this snapshot shows them next to the 3D tab.
    h.app.actions_ui.tab = ActionsTab::Add;
    h.open_panel(Panel::Actions);
    h.advance(0.1);
    h.render().save_png(&out.join("actions_add_in_three_d.png")).unwrap();
}

#[test]
fn animation_playback_and_onion_are_off_while_the_3d_view_is_open() {
    let mut h = Headless::from_app(Vec2::new(1000.0, 560.0), |ctx| {
        EskizApp::new(ctx, Document::new(64, 64), String::new(), Prefs::default())
    });
    h.app.session.doc.animation.frames_enabled = true;
    h.app.session.doc.push_layer(None, eskiz_core::Props::new("two"), None, vec![255; 64 * 64 * 4]);
    h.advance(0.1);
    // Control: in 2D, play starts playback and the Frames view is drawn.
    h.app.anim_play();
    assert!(h.app.anim.playback.as_ref().is_some_and(|p| p.is_playing()));
    h.app.anim_pause();
    h.app.anim.playback = None;

    let mut doc = cube_doc();
    doc.camera = doc.home;
    h.app.three_d = Some(Box::new(ThreeD::new(doc, None)));
    h.app.anim_play();
    assert!(h.app.anim.playback.is_none(), "play is refused in the 3D view");
    let tex = h.app.canvas_texture().map(|t| t.id());
    assert_eq!(tex, h.app.texture.as_ref().map(|t| t.id()), "no onion view in 3D");
    h.advance(0.1);
    assert!(h.app.three_d.is_some());
}

#[cfg(feature = "usd")]
#[test]
fn a_usdz_exported_from_the_cube_imports_through_the_app_path() {
    let dir = temp_dir("usdz-import");
    let glb = eskiz_3d::load_gltf_slice(&cube_glb(true), &eskiz_3d::ImportOptions::default()).unwrap();
    let path = dir.join("cube.usdz");
    eskiz_3d::export_usdz(&glb.model, &path).unwrap();
    assert!(is_model_file(&path));
    match import_job(None, path) {
        Outcome::Opened { doc, .. } => {
            assert_eq!(doc.model.triangle_count(), 12);
            assert_eq!(doc.model.texture_sets.len(), 1);
        }
        Outcome::Failed(lines) => panic!("usdz rejected: {lines:?}"),
    }
}

#[cfg(feature = "usd")]
#[test]
fn an_unsupported_usd_feature_shows_a_clear_message() {
    let dir = temp_dir("usd-unsupported");
    let path = dir.join("scene.usda");
    std::fs::write(&path, "#usda 1.0\ndef PointInstancer \"P\"\n{\n}\n").unwrap();
    match import_job(None, path) {
        Outcome::Failed(lines) => assert!(lines.join(" ").contains("instancing"), "{lines:?}"),
        Outcome::Opened { .. } => panic!("instancing must be refused"),
    }
}

/// Opening a model saves the drawing it hides, and closing the 3D view gives the drawing back its
/// save target (autosave and the exit save need it).
#[test]
fn opening_a_model_saves_the_drawing_and_keeps_its_save_target() {
    let dir = temp_dir("open-model-saves");
    let lib = Library::at(dir.clone());
    let app_lib = lib.clone();
    let mut h = Headless::from_app(Vec2::new(1024.0, 700.0), move |ctx| {
        EskizApp::with_library(ctx, Prefs::default(), app_lib)
    });
    h.settle_storage();
    h.app.set_external_document("Sketch".into());
    let layer = h.app.session.doc.layer_id(0).unwrap();
    h.app.session.pour_fill(layer, (5, 5), 0.5, [255, 0, 0]).unwrap();
    assert!(h.app.is_dirty());
    h.app.open_three_d(cube_doc(), None);
    h.settle_storage();
    assert!(lib.list().iter().any(|e| e.title == "Sketch"), "the drawing was saved before the 3D view");
    h.app.close_three_d();
    assert!(h.app.gal.current.is_some(), "the drawing has its save target back");
}
