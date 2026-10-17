//! Headless tests for the file-picker wiring: every Browse/Choose button asks `MockFileAccess` for
//! the right kind of path, and the answer reaches the existing action. Snapshots of the Actions >
//! Add and Share panels (with the new buttons) are written to `target/snapshots`.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, accesskit, Pos2, Vec2};

use crate::app::Prefs;
use crate::document::Document;
use crate::export::{export_with, ExportFormat, ExportSettings};
use crate::file_access::{FileAccess, MockFileAccess, PickKind, PickPurpose, Picked};
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-pick-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn harness() -> (Headless, Arc<MockFileAccess>) {
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(400, 300), Prefs::default());
    h.ctx.enable_accesskit();
    let mock = Arc::new(MockFileAccess::new());
    let files: Arc<dyn FileAccess> = mock.clone();
    h.app.files = files;
    h.step();
    h.step();
    (h, mock)
}

fn actions(h: &mut Headless, tab: ActionsTab) {
    h.app.actions_ui.tab = tab;
    h.open_panel(Panel::Actions);
    h.step();
}

/// Centres of every button with the given accessible name, top to bottom.
fn buttons(h: &Headless, name: &str) -> Vec<Pos2> {
    let Some(update) = &h.access else {
        return Vec::new();
    };
    let mut found: Vec<Pos2> = update
        .nodes
        .iter()
        .filter(|(_, n)| n.label() == Some(name) && n.role() == accesskit::Role::Button)
        .filter_map(|(_, n)| n.bounds())
        .map(|r| Pos2::new(((r.x0 + r.x1) / 2.0) as f32, ((r.y0 + r.y1) / 2.0) as f32))
        .collect();
    found.sort_by(|a, b| a.y.total_cmp(&b.y));
    found
}

/// Click the `nth` button called `name`, then run a few frames so the pick is routed.
fn press(h: &mut Headless, name: &str, nth: usize) {
    let mut all = buttons(h, name);
    // Buttons low in a long panel start out scrolled off; scroll the panel until it is on screen.
    for _ in 0..6 {
        let visible = all.get(nth).is_some_and(|p| p.y < 640.0);
        if visible || all.is_empty() {
            break;
        }
        let at = Pos2::new(all[0].x, 400.0);
        h.scroll_at(at, -300.0);
        h.step();
        h.step();
        all = buttons(h, name);
    }
    let at = *all.get(nth).unwrap_or_else(|| panic!("no button #{nth} named {name:?}; found {all:?}"));
    h.click(at);
    for _ in 0..4 {
        h.step();
    }
}

fn toast(h: &Headless) -> String {
    h.app.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()
}

fn wait_for(h: &mut Headless, what: &str, mut done: impl FnMut(&mut Headless) -> bool) {
    for _ in 0..400 {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn write_png(path: &PathBuf) {
    let img = image::RgbaImage::from_pixel(32, 24, image::Rgba([200, 60, 40, 255]));
    img.save(path).unwrap();
}

#[test]
fn import_browse_asks_for_an_open_dialog_and_imports_the_picture() {
    let dir = temp("import");
    let png = dir.join("pic.png");
    write_png(&png);
    let (mut h, mock) = harness();
    mock.push(Picked::Path(png.clone()));
    actions(&mut h, ActionsTab::Add);
    let layers = h.app.session.doc.layer_count();
    press(&mut h, "Browse...", 0);
    let req = mock.requests().pop().expect("a request");
    assert_eq!(req.purpose, PickPurpose::ImportImage);
    assert_eq!(req.kind, PickKind::OpenFile);
    assert!(req.filters[0].extensions.contains(&"psd") && req.filters[0].extensions.contains(&"png"));
    assert_eq!(h.app.actions_ui.import_path, png.display().to_string(), "the typed field shows the pick");
    wait_for(&mut h, "the import", |h| h.app.session.doc.layer_count() > layers);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn font_browse_routes_to_the_font_import() {
    let (mut h, mock) = harness();
    // A wrong extension is rejected by the import itself, which proves the route without
    // copying a font into the user's data folder.
    mock.push(Picked::Path("/tmp/not-a-font.txt".into()));
    actions(&mut h, ActionsTab::Add);
    press(&mut h, "Browse...", 1);
    let req = mock.requests().pop().unwrap();
    assert_eq!((req.purpose, req.kind), (PickPurpose::ImportFont, PickKind::OpenFile));
    assert_eq!(req.filters[0].extensions, ["ttf", "otf", "ttc"]);
    assert_eq!(h.app.text.font_path, "/tmp/not-a-font.txt");
    assert!(toast(&h).contains("Fonts must be"), "{}", toast(&h));
}

#[test]
fn open_browse_replaces_the_drawing() {
    let dir = temp("open");
    let file = dir.join("art.procreate");
    export_with(&Document::new(321, 123), ExportFormat::Procreate, &file, &ExportSettings::default()).unwrap();
    let (mut h, mock) = harness();
    mock.push(Picked::Path(file.clone()));
    actions(&mut h, ActionsTab::Add);
    press(&mut h, "Browse...", 2);
    let req = mock.requests().pop().unwrap();
    assert_eq!((req.purpose, req.kind), (PickPurpose::OpenDocument, PickKind::OpenFile));
    assert_eq!(req.filters[0].extensions, ["procreate"]);
    wait_for(&mut h, "the document to load", |h| h.app.session.doc.width == 321 && h.app.session.doc.height == 123);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reference_browse_shows_the_picture() {
    let dir = temp("reference");
    let png = dir.join("ref.png");
    write_png(&png);
    let (mut h, mock) = harness();
    mock.push(Picked::Path(png));
    actions(&mut h, ActionsTab::Canvas);
    press(&mut h, "Browse...", 0);
    let req = mock.requests().pop().unwrap();
    assert_eq!((req.purpose, req.kind), (PickPurpose::Reference, PickKind::OpenFile));
    assert!(h.app.reference.image.is_some(), "the picture is loaded");
    assert!(h.app.reference.open);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reference_window_has_its_own_browse_button() {
    let dir = temp("refwin");
    let png = dir.join("ref.png");
    write_png(&png);
    let (mut h, mock) = harness();
    h.app.reference.open = true;
    h.app.reference.source = crate::reference::RefSource::Image;
    h.step();
    h.step();
    mock.push(Picked::Path(png));
    press(&mut h, "Browse...", 0);
    assert_eq!(mock.requests().pop().unwrap().purpose, PickPurpose::Reference);
    assert!(h.app.reference.image.is_some());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn share_choose_sets_the_base_name_without_an_extension_and_export_writes_the_file() {
    let dir = temp("share");
    let (mut h, mock) = harness();
    mock.push(Picked::Path(dir.join("drawing.png")));
    actions(&mut h, ActionsTab::Share);
    press(&mut h, "Choose...", 0);
    let req = mock.requests().pop().unwrap();
    assert_eq!(req.purpose, PickPurpose::ExportBase);
    assert!(matches!(req.kind, PickKind::SaveFile { .. }));
    assert_eq!(PathBuf::from(&h.app.export_path), dir.join("drawing"), "no extension is kept");
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Jpeg);
    let want = dir.join("drawing.jpg");
    wait_for(&mut h, "the export", |_| want.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn share_choose_folder_sets_the_export_folder() {
    let dir = temp("sharefolder");
    let (mut h, mock) = harness();
    mock.push(Picked::Path(dir.clone()));
    actions(&mut h, ActionsTab::Share);
    press(&mut h, "Choose folder...", 0);
    let req = mock.requests().pop().unwrap();
    assert_eq!((req.purpose, req.kind), (PickPurpose::ExportFolder, PickKind::Folder));
    assert_eq!(PathBuf::from(&h.app.export_path), dir);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cancel_and_unavailable_change_nothing_but_the_toast() {
    let (mut h, mock) = harness();
    actions(&mut h, ActionsTab::Share);
    let before = h.app.export_path.clone();
    mock.push(Picked::None);
    press(&mut h, "Choose...", 0);
    assert_eq!(h.app.export_path, before);
    assert!(toast(&h).is_empty(), "cancel is silent: {}", toast(&h));
    mock.push(Picked::Unavailable("No file dialog here.".into()));
    press(&mut h, "Choose...", 0);
    assert_eq!(h.app.export_path, before);
    assert_eq!(toast(&h), "No file dialog here.");
    assert!(h.app.pending_picks.is_empty());
}

#[test]
fn stale_receivers_are_dropped_and_repeat_requests_replace_the_old_one() {
    let (mut h, _mock) = harness();
    let ctx = h.ctx.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    drop(tx);
    h.app.pending_picks.push(crate::file_access::PendingPick::for_test(PickPurpose::ImportFont, rx));
    h.app.poll_picks(&ctx);
    assert!(h.app.pending_picks.is_empty(), "a disconnected receiver is dropped");
    assert!(toast(&h).is_empty());
    // Two requests for one purpose leave one pending entry.
    let slow = Arc::new(SlowAccess::default());
    let files: Arc<dyn FileAccess> = slow.clone();
    h.app.files = files;
    h.app.request_pick(PickPurpose::Reference, None, None);
    h.app.request_pick(PickPurpose::Reference, None, None);
    assert_eq!(h.app.pending_picks.len(), 1);
    h.app.poll_picks(&ctx);
    assert_eq!(h.app.pending_picks.len(), 1, "still waiting, nothing routed");
}

/// Never answers: keeps its senders alive so the receivers stay open.
#[derive(Default)]
struct SlowAccess(std::sync::Mutex<Vec<std::sync::mpsc::Sender<Picked>>>);

impl FileAccess for SlowAccess {
    fn pick(&self, _req: crate::file_access::PickRequest) -> std::sync::mpsc::Receiver<Picked> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.0.lock().unwrap().push(tx);
        rx
    }
}

#[test]
fn palette_browse_import_and_export() {
    let dir = temp("palette");
    let (mut h, mock) = harness();
    h.app.colour_store.palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    h.app.guides.palette_window_open = true;
    h.step();
    h.step();
    let file = dir.join("mine.aco");
    mock.push(Picked::Path(file.clone()));
    press(&mut h, "Export...", 0);
    let req = mock.requests().pop().unwrap();
    assert_eq!(req.purpose, PickPurpose::PaletteExport);
    assert!(matches!(req.kind, PickKind::SaveFile { .. }));
    assert!(file.exists(), "the palette file was written");
    h.app.colour_store.palette.clear();
    mock.push(Picked::Path(file));
    press(&mut h, "Import...", 0);
    let req = mock.requests().pop().unwrap();
    assert_eq!((req.purpose, req.kind), (PickPurpose::PaletteImport, PickKind::OpenFile));
    assert_eq!(h.app.colour_store.palette.len(), 3);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn model_and_texture_exports_route_through_the_picker() {
    let dir = temp("threed");
    let (mut h, mock) = harness();
    // Without a 3D document the pick only records the location.
    mock.push(Picked::Path(dir.join("m.glb")));
    h.app.request_pick(PickPurpose::Model3d, None, None);
    h.step();
    assert_eq!(PathBuf::from(&h.app.export_path), dir.join("m"));
    mock.push(Picked::Path(dir.clone()));
    h.app.request_pick(PickPurpose::ExportFolder, None, None);
    h.step();
    assert_eq!(PathBuf::from(&h.app.export_path), dir);
    let reqs = mock.requests();
    assert_eq!(reqs[0].kind, PickKind::SaveFile { suggested: "model.glb".into() });
    assert_eq!(reqs[0].filters[0].extensions, ["glb"]);
    assert_eq!(reqs[1].kind, PickKind::Folder);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn usdz_pick_is_a_save_dialog_with_a_usdz_filter() {
    let dir = temp("usdz");
    let (mut h, mock) = harness();
    mock.push(Picked::Path(dir.join("m.usdz")));
    h.app.request_pick(PickPurpose::Usdz, None, None);
    h.step();
    assert_eq!(PathBuf::from(&h.app.export_path), dir.join("m"));
    let req = mock.requests().pop().unwrap();
    assert_eq!(req.kind, PickKind::SaveFile { suggested: "model.usdz".into() });
    assert_eq!(req.filters[0].extensions, ["usdz"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn writes_filepicker_snapshots() {
    for (tab, name) in [(ActionsTab::Add, "filepicker_actions_add.png"), (ActionsTab::Share, "filepicker_share.png")] {
        let (mut h, _mock) = harness();
        actions(&mut h, tab);
        h.render().save_png(&snapshot_dir().join(name)).unwrap();
        assert!(!buttons(&h, if tab == ActionsTab::Add { "Browse..." } else { "Choose..." }).is_empty());
    }
    let _ = egui::Id::NULL;
}

#[test]
fn brush_browse_routes_to_the_brush_import() {
    let (mut h, mock) = harness();
    mock.push(Picked::Path("/nonexistent/none.brush".into()));
    h.open_panel(Panel::Brush);
    h.app.brush_ui.adding = true;
    h.step();
    h.step();
    press(&mut h, "Browse...", 0);
    let req = mock.requests().pop().unwrap();
    assert_eq!((req.purpose, req.kind), (PickPurpose::ImportBrush, PickKind::OpenFile));
    assert_eq!(req.filters[0].extensions, ["brush", "brushset", "abr"]);
    assert_eq!(h.app.brush_ui.path, "/nonexistent/none.brush");
    assert!(toast(&h).contains("Could not import"), "{}", toast(&h));
}
