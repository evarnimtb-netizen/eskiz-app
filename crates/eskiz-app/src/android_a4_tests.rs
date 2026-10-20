//! End-to-end tests of the A4 flows on the headless app, with `MockFileAccess` playing the Android
//! backend (`set_staged(true)`) and `MockSystemClipboard` the system clipboard: save to a folder,
//! save without a folder, share, failed commits, foreign paste and the desktop paths that must not
//! change.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{accesskit, Vec2};

use crate::app::Prefs;
use crate::deliver::ShareMode;
use crate::document::Document;
use crate::export::ExportFormat;
use crate::file_access::{Delivered, FileAccess, MockFileAccess, PickKind, PickPurpose, Picked};
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::saf::Inbox;
use crate::selection_tool::ClipAction;
use crate::snapshot::{snapshot_dir, Headless};
use crate::system_clipboard::{ClipState, MockSystemClipboard, SystemClipboard};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-a4-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A headless app on a 40 x 30 canvas whose file backend behaves like Android.
fn android(name: &str) -> (Headless, Arc<MockFileAccess>, PathBuf) {
    let dir = temp(name);
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(40, 30), Prefs::default());
    h.ctx.enable_accesskit();
    let mock = Arc::new(MockFileAccess::new());
    mock.set_staged(true);
    mock.set_share_root(dir.join("share"));
    let files: Arc<dyn FileAccess> = mock.clone();
    h.app.files = files;
    h.app.inbox = Arc::new(Inbox::default());
    h.step();
    h.step();
    (h, mock, dir)
}

fn toast(h: &Headless) -> String {
    h.app.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()
}

fn wait_for(h: &mut Headless, what: &str, mut done: impl FnMut(&mut Headless) -> bool) {
    for _ in 0..500 {
        h.step();
        if done(h) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting for {what} (toast: {:?})", toast(h));
}

fn exporting_done(h: &mut Headless) -> bool {
    !h.app.share_busy() && h.app.commits.is_empty()
}

#[test]
fn save_to_folder_asks_for_a_staging_path_and_commits_once() {
    let (mut h, mock, dir) = android("save");
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    mock.set_folder(Some("Documents/Eskiz"));
    mock.push(Picked::Path(stage.join("Art.png")));
    h.app.share_mode = ShareMode::SaveToFolder;
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    let req = mock.requests().pop().expect("a pick");
    assert_eq!(req.purpose, PickPurpose::SaveExport(ExportFormat::Png));
    assert!(matches!(req.kind, PickKind::SaveFile { ref suggested } if suggested == "Eskiz.png"), "{:?}", req.kind);
    wait_for(&mut h, "the save", exporting_done);
    assert_eq!(mock.commits(), [stage.join("Art.png")], "commit is called once with the written file");
    assert!(stage.join("Art.png").is_file());
    assert_eq!(toast(&h), "Saved Art.png");
    assert!(mock.shares().is_empty());
}

#[test]
fn save_without_a_folder_uses_the_create_picker_for_that_file() {
    let (mut h, mock, dir) = android("create");
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    mock.set_folder(None);
    mock.push(Picked::Path(stage.join("Chosen.psd")));
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Psd);
    assert_eq!(mock.requests().pop().unwrap().purpose, PickPurpose::SaveExport(ExportFormat::Psd));
    wait_for(&mut h, "the save", exporting_done);
    assert_eq!(mock.commits(), [stage.join("Chosen.psd")]);
    let head = fs::read(stage.join("Chosen.psd")).unwrap();
    assert_eq!(&head[..4], b"8BPS");
}

#[test]
fn a_cancelled_save_pick_exports_nothing() {
    let (mut h, mock, _dir) = android("cancel");
    mock.push(Picked::None);
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    for _ in 0..6 {
        h.step();
    }
    assert!(mock.commits().is_empty() && mock.shares().is_empty());
    assert!(!h.app.share_busy());
    assert!(!toast(&h).contains("Export"), "{}", toast(&h));
}

#[test]
fn a_failed_commit_shows_the_error_and_no_exported_toast() {
    let (mut h, mock, dir) = android("failed");
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    mock.push(Picked::Path(stage.join("Art.png")));
    mock.push_commit(Delivered::Failed("No space left on device".into()));
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    wait_for(&mut h, "the failure", |h| toast(h).starts_with("Could not save"));
    assert_eq!(toast(&h), "Could not save: No space left on device");
    assert!(!toast(&h).contains("Exported"));
}

#[test]
fn a_commit_that_answers_later_holds_back_the_exported_toast() {
    // A backend whose commit answers after a few frames.
    struct Slow {
        inner: MockFileAccess,
        tx: std::sync::Mutex<Option<std::sync::mpsc::Sender<Delivered>>>,
    }
    impl FileAccess for Slow {
        fn pick(&self, req: crate::file_access::PickRequest) -> std::sync::mpsc::Receiver<Picked> {
            self.inner.pick(req)
        }
        fn commit(&self, _written: &std::path::Path) -> std::sync::mpsc::Receiver<Delivered> {
            let (tx, rx) = std::sync::mpsc::channel();
            *self.tx.lock().unwrap() = Some(tx);
            rx
        }
        fn staged(&self) -> bool {
            true
        }
    }
    let dir = temp("slow");
    let slow = Arc::new(Slow { inner: MockFileAccess::new(), tx: std::sync::Mutex::new(None) });
    slow.inner.push(Picked::Path(dir.join("Art.png")));
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(40, 30), Prefs::default());
    let files: Arc<dyn FileAccess> = slow.clone();
    h.app.files = files;
    h.step();
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    wait_for(&mut h, "the commit to start", |_| slow.tx.lock().unwrap().is_some());
    h.step();
    assert_eq!(toast(&h), "Saving...", "no Exported toast while the copy runs");
    slow.tx.lock().unwrap().take().unwrap().send(Delivered::Saved { name: "Art (1).png".into() }).unwrap();
    wait_for(&mut h, "the result", |h| toast(h).starts_with("Saved"));
    assert_eq!(toast(&h), "Saved Art (1).png", "the name the provider reports");
}

#[test]
fn share_writes_to_the_share_cache_and_calls_the_share_sheet() {
    let (mut h, mock, dir) = android("share");
    h.app.share_mode = ShareMode::Share;
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    wait_for(&mut h, "the share", |_| !mock.shares().is_empty());
    assert!(mock.requests().is_empty(), "no picker in Share mode");
    assert!(mock.commits().is_empty());
    let (files, mime) = mock.shares().pop().unwrap();
    assert_eq!(mime, "image/png");
    assert_eq!(files.len(), 1);
    assert!(files[0].starts_with(dir.join("share")) && files[0].file_name().unwrap() == "Eskiz.png");
    assert!(files[0].is_file());
    let _ = h.app.share_busy();
}

#[test]
fn share_layer_pngs_sends_every_png() {
    let (mut h, mock, _dir) = android("share-layers");
    h.app.session.doc.add_layer_above(None);
    h.app.share_mode = ShareMode::Share;
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Layers);
    wait_for(&mut h, "the share", |_| !mock.shares().is_empty());
    let (files, mime) = mock.shares().pop().unwrap();
    assert!(files.len() >= 2, "{files:?}");
    assert!(files.iter().all(|f| f.extension().is_some_and(|e| e == "png")));
    assert_eq!(mime, "image/png");
}

#[test]
fn share_pdf_and_jpeg_use_their_own_mime_types() {
    for (format, mime) in [(ExportFormat::Pdf, "application/pdf"), (ExportFormat::Jpeg, "image/jpeg"), (ExportFormat::Procreate, "application/octet-stream")] {
        let (mut h, mock, _dir) = android("share-mime");
        h.app.share_mode = ShareMode::Share;
        let ctx = h.ctx.clone();
        h.app.start_export(&ctx, format);
        wait_for(&mut h, "the share", |_| !mock.shares().is_empty());
        assert_eq!(mock.shares()[0].1, mime, "{format:?}");
    }
}

#[test]
fn a_share_sheet_that_cannot_open_says_so() {
    let (mut h, mock, _dir) = android("share-fail");
    mock.fail_share("Sharing is not available: no app");
    h.app.share_mode = ShareMode::Share;
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    wait_for(&mut h, "the failure", |h| toast(h).starts_with("Sharing is not available"));
}

#[test]
fn layers_save_asks_for_a_staging_folder_and_commits_the_folder() {
    let (mut h, mock, dir) = android("layers-save");
    h.app.session.doc.add_layer_above(None);
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    mock.push(Picked::Path(stage.clone()));
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Layers);
    assert_eq!(mock.requests().pop().unwrap().purpose, PickPurpose::ExportFolder);
    wait_for(&mut h, "the save", exporting_done);
    assert_eq!(mock.commits(), [stage.join("Eskiz layers")]);
    assert!(stage.join("Eskiz layers").is_dir());
    assert!(fs::read_dir(stage.join("Eskiz layers")).unwrap().count() >= 2);
}

#[test]
fn the_export_folder_button_asks_for_a_tree() {
    let (mut h, mock, _dir) = android("tree");
    mock.push(Picked::Path(PathBuf::from("Documents/Eskiz")));
    h.app.actions_ui.tab = ActionsTab::Share;
    h.open_panel(Panel::Actions);
    h.step();
    h.step();
    let names = button_names(&h);
    assert!(names.iter().any(|n| n == "Choose..."), "{names:?}");
    h.app.request_pick(PickPurpose::ExportTree, None, None);
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(mock.requests().pop().unwrap().purpose, PickPurpose::ExportTree);
    assert!(toast(&h).contains("Documents/Eskiz"), "{}", toast(&h));
}

fn button_names(h: &Headless) -> Vec<String> {
    h.access
        .as_ref()
        .map(|u| u.nodes.iter().filter(|(_, n)| n.role() == accesskit::Role::Button).filter_map(|(_, n)| n.label().map(str::to_owned)).collect())
        .unwrap_or_default()
}

#[test]
fn the_android_share_tab_has_a_switch_and_no_path_field() {
    let (mut h, mock, _dir) = android("tab");
    mock.set_folder(Some("Documents/Eskiz"));
    h.app.actions_ui.tab = ActionsTab::Share;
    h.open_panel(Panel::Actions);
    h.step();
    h.step();
    h.render().save_png(&snapshot_dir().join("android_share_tab.png")).unwrap();
    let names = button_names(&h);
    for want in ["Save to folder", "Share", "Change..."] {
        assert!(names.iter().any(|n| n == want), "no {want:?} in {names:?}");
    }
    assert!(!names.iter().any(|n| n == "Choose folder..."), "the typed-path pickers are desktop only");
    // Share mode hides the folder line.
    h.app.share_mode = ShareMode::Share;
    h.step();
    h.step();
    h.render().save_png(&snapshot_dir().join("android_share_tab_share.png")).unwrap();
    assert!(!button_names(&h).iter().any(|n| n == "Change..."));
    // No folder chosen: the same button says Choose.
    mock.set_folder(None);
    h.app.share_mode = ShareMode::SaveToFolder;
    h.step();
    h.step();
    assert!(button_names(&h).iter().any(|n| n == "Choose..."));
}

#[test]
fn the_desktop_share_tab_is_unchanged() {
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(40, 30), Prefs::default());
    h.ctx.enable_accesskit();
    h.step();
    h.app.actions_ui.tab = ActionsTab::Share;
    h.open_panel(Panel::Actions);
    h.step();
    h.step();
    let names = button_names(&h);
    for want in ["Choose...", "Choose folder..."] {
        assert!(names.iter().any(|n| n == want), "no {want:?} in {names:?}");
    }
    assert!(!names.iter().any(|n| n == "Save to folder"));
}

#[test]
fn desktop_exports_keep_their_toast_and_never_commit_or_share() {
    let dir = temp("desktop");
    let mock = Arc::new(MockFileAccess::new());
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(40, 30), Prefs::default());
    let files: Arc<dyn FileAccess> = mock.clone();
    h.app.files = files;
    h.app.export_path = dir.join("art").display().to_string();
    h.step();
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    wait_for(&mut h, "the export", |h| toast(h).starts_with("Exported"));
    assert_eq!(toast(&h), format!("Exported PNG to {}", dir.join("art.png").display()));
    assert!(dir.join("art.png").is_file());
    assert!(mock.commits().is_empty() && mock.shares().is_empty() && mock.requests().is_empty());
}

#[test]
fn the_palette_export_commits_on_android_and_stays_local_on_desktop() {
    let (mut h, mock, dir) = android("palette");
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    mock.push(Picked::Path(stage.join("palette.swatches")));
    h.app.request_pick(PickPurpose::PaletteExport, Some("palette.swatches".into()), None);
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(mock.commits(), [stage.join("palette.swatches")]);
    assert!(stage.join("palette.swatches").is_file());
    assert_eq!(toast(&h), "Saved palette.swatches");
}

#[test]
fn glb_save_on_android_commits_the_model_file() {
    // A 3D document is needed; without one the pick is routed but no export starts. This checks
    // the route does not set the typed export path on Android.
    let (mut h, mock, dir) = android("glb");
    let before = h.app.export_path.clone();
    mock.push(Picked::Path(dir.join("model.glb")));
    h.app.request_pick(PickPurpose::Model3d, Some("model.glb".into()), None);
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.app.export_path, before);
    assert!(mock.commits().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Clipboard

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: [u8; 4]) {
    image::RgbaImage::from_pixel(w, h, image::Rgba(rgba)).save(path).unwrap();
}

fn with_clipboard(name: &str) -> (Headless, Arc<MockSystemClipboard>, PathBuf) {
    let dir = temp(name);
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(40, 30), Prefs::default());
    let clip = Arc::new(MockSystemClipboard::in_dir(dir.join("clip")));
    let sys: Arc<dyn SystemClipboard> = clip.clone();
    h.app.sysclip = sys;
    h.app.inbox = Arc::new(Inbox::default());
    h.step();
    h.step();
    (h, clip, dir)
}

#[test]
fn copy_all_also_puts_a_png_of_the_canvas_on_the_system_clipboard() {
    let (mut h, clip, _dir) = with_clipboard("copy");
    h.app.clip_action(ClipAction::CopyAll);
    wait_for(&mut h, "the system copy", |_| !clip.puts().is_empty());
    let (path, serial) = clip.puts().pop().unwrap();
    assert_eq!(serial, 1);
    assert_eq!(clip.state(), ClipState::Ours(1));
    let (w, hh, _) = crate::import::decode_image(&path).unwrap();
    assert_eq!((w, hh), (40, 30));
    // The own clip is untouched.
    assert!(h.app.sel.clip.is_some());
}

#[test]
fn a_second_copy_replaces_the_clip_file() {
    let (mut h, clip, _dir) = with_clipboard("copy2");
    h.app.clip_action(ClipAction::CopyAll);
    wait_for(&mut h, "copy 1", |_| clip.puts().len() == 1);
    h.app.clip_action(ClipAction::CopyAll);
    wait_for(&mut h, "copy 2", |_| clip.puts().len() == 2);
    let puts = clip.puts();
    assert_eq!(puts[1].1, 2);
    assert!(puts[1].0.is_file() && !puts[0].0.exists(), "the older clip file is removed");
}

#[test]
fn desktop_copy_does_not_touch_any_system_clipboard() {
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(40, 30), Prefs::default());
    h.step();
    h.app.clip_action(ClipAction::CopyAll);
    h.step();
    assert!(h.app.sel.clip.is_some());
    assert!(!h.app.clips.busy());
    assert_eq!(h.app.sysclip.state(), ClipState::None);
}

#[test]
fn a_foreign_image_pastes_as_one_paste_layer_and_one_undo_step() {
    let (mut h, clip, dir) = with_clipboard("foreign");
    let png = dir.join("foreign.png");
    write_png(&png, 64, 48, [10, 200, 30, 255]);
    clip.copy_foreign(png.clone());
    let layers = h.app.session.doc.layer_count();
    let undo_before = h.app.session.undo_name().map(str::to_owned);
    h.app.clip_action(ClipAction::Paste);
    wait_for(&mut h, "the paste", |h| h.app.session.doc.layer_count() > layers);
    assert_eq!(h.app.session.doc.layer_count(), layers + 1);
    assert_eq!(h.app.session.undo_name(), Some("Paste"));
    // 64 x 48 is larger than the 40 x 30 canvas: fitted (scaled down, centred).
    let id = h.app.active;
    let node = h.app.session.doc.node(id).unwrap();
    assert_eq!(node.props.name, "Paste");
    let pixels = h.app.session.doc.layer(id).unwrap().to_rgba();
    let centre = &pixels[(15 * 40 + 20) * 4..(15 * 40 + 20) * 4 + 4];
    assert_eq!(centre, &[10, 200, 30, 255]);
    // Finish the transform that pasting starts, then undo: the layer is gone, in one step.
    h.app.cancel_transform();
    h.app.undo_action();
    assert_eq!(h.app.session.doc.layer_count(), layers);
    assert_eq!(h.app.session.undo_name().map(str::to_owned), undo_before);
    assert!(!png.exists(), "the cache copy is removed after decoding");
}

#[test]
fn a_smaller_foreign_image_keeps_its_size_and_sits_in_the_middle() {
    let (mut h, clip, dir) = with_clipboard("foreign-small");
    let png = dir.join("small.png");
    write_png(&png, 8, 6, [200, 0, 0, 255]);
    clip.copy_foreign(png);
    let layers = h.app.session.doc.layer_count();
    h.app.clip_action(ClipAction::Paste);
    wait_for(&mut h, "the paste", |h| h.app.session.doc.layer_count() > layers);
    let id = h.app.active;
    let px = h.app.session.doc.layer(id).unwrap().to_rgba();
    let at = |x: usize, y: usize| px[(y * 40 + x) * 4 + 3];
    assert_eq!(at(20, 15), 255);
    assert_eq!(at(2, 2), 0);
    assert_eq!(at(20 - 5, 15), 0, "outside the 8 x 6 picture");
}

#[test]
fn an_unreadable_foreign_image_says_so_and_changes_nothing() {
    let (mut h, clip, _dir) = with_clipboard("foreign-bad");
    clip.copy_broken("The image could not be read.");
    let layers = h.app.session.doc.layer_count();
    h.app.clip_action(ClipAction::Paste);
    wait_for(&mut h, "the failure", |h| toast(h).starts_with("Could not paste"));
    assert_eq!(h.app.session.doc.layer_count(), layers);
}

#[test]
fn paste_uses_the_own_clip_when_the_system_clipboard_has_our_image_or_nothing() {
    for state in [ClipState::None, ClipState::TextOnly, ClipState::Ours(1)] {
        let (mut h, clip, _dir) = with_clipboard("own");
        h.app.clip_action(ClipAction::CopyAll);
        wait_for(&mut h, "the system copy", |_| !clip.puts().is_empty());
        clip.set_state(state);
        let layers = h.app.session.doc.layer_count();
        h.app.clip_action(ClipAction::Paste);
        h.step();
        assert_eq!(h.app.session.doc.layer_count(), layers + 1, "{state:?}: pasted at once, in place");
        assert_eq!(h.app.session.undo_name(), Some("Paste"));
    }
}

#[test]
fn paste_is_enabled_for_a_foreign_image_even_with_no_own_clip() {
    let (mut h, clip, dir) = with_clipboard("enabled");
    assert!(!h.app.can_paste());
    let png = dir.join("x.png");
    write_png(&png, 4, 4, [1, 2, 3, 255]);
    clip.copy_foreign(png);
    h.advance(1.0);
    assert!(h.app.can_paste());
}

#[test]
fn desktop_paste_with_nothing_copied_still_says_so() {
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(40, 30), Prefs::default());
    h.step();
    h.app.clip_action(ClipAction::Paste);
    assert_eq!(toast(&h), "Nothing copied yet.");
}

// ---------------------------------------------------------------------------------------------
// Bridge: notices, hooks, Back

#[test]
fn notices_from_java_become_toasts() {
    let (mut h, _mock, _dir) = android("notice");
    h.app.inbox.notice(crate::saf::INTERRUPTED);
    h.step();
    assert_eq!(toast(&h), crate::saf::INTERRUPTED);
}

#[test]
fn hooks_run_the_same_paths_as_the_buttons() {
    let (mut h, mock, _dir) = android("hooks");
    h.app.inbox.push_hook("open_document");
    h.app.inbox.push_hook("choose_folder");
    h.app.inbox.push_hook("not_a_hook");
    h.step();
    let purposes: Vec<PickPurpose> = mock.requests().iter().map(|r| r.purpose).collect();
    assert_eq!(purposes, [PickPurpose::OpenDocument, PickPurpose::ExportTree]);
    h.app.inbox.push_hook("export:png:share");
    wait_for(&mut h, "the share", |_| !mock.shares().is_empty());
    assert_eq!(h.app.share_mode, ShareMode::Share);
    h.app.inbox.push_hook("export:png:save");
    h.step();
    assert_eq!(h.app.share_mode, ShareMode::SaveToFolder);
    assert_eq!(mock.requests().last().unwrap().purpose, PickPurpose::SaveExport(ExportFormat::Png));
}

#[test]
fn open_path_hook_opens_a_document_without_a_picker() {
    let (mut h, mock, dir) = android("hook-open-path");
    let file = dir.join("fixture.procreate");
    crate::export::export_document(&crate::android_fixtures::fixture_document(), crate::export::ExportFormat::Procreate, &file).unwrap();
    h.app.inbox.push_hook(format!("open_path:{}", file.display()));
    h.step();
    assert_eq!((h.app.session.doc.width, h.app.session.doc.height), (crate::android_fixtures::WIDTH, crate::android_fixtures::HEIGHT));
    assert_eq!(h.app.session.doc.layer_count(), 3);
    assert!(mock.requests().is_empty(), "no picker was opened");
}

#[test]
fn skip_tour_hook_ends_the_welcome_tour_for_good() {
    let (mut h, _mock, _dir) = android("hook-skip-tour");
    h.app.tour = crate::onboarding::Tour::at_startup(None, false, false);
    h.app.inbox.push_hook("skip_tour");
    for _ in 0..6 {
        h.step();
    }
    assert!(h.app.tour.step().is_none());
    assert!(!h.app.tour.is_modal());
}

#[test]
fn copy_all_and_paste_hooks_use_the_clipboard() {
    let (mut h, clip, _dir) = with_clipboard("hook-clip");
    h.app.inbox.push_hook("copy_all");
    wait_for(&mut h, "the copy", |_| !clip.puts().is_empty());
    let layers = h.app.session.doc.layer_count();
    h.app.inbox.push_hook("paste");
    h.step();
    assert_eq!(h.app.session.doc.layer_count(), layers + 1);
}

#[test]
fn the_back_registration_follows_whether_back_has_something_to_close() {
    let (mut h, _m, _d) = android("backreg");
    let back = Arc::new(crate::back::MockBack::default());
    let b: Arc<dyn crate::back::BackBridge> = back.clone();
    h.app.back_bridge = b;
    h.app.back_sent = None;
    h.step();
    // The canvas is open: Back goes to the gallery.
    assert_eq!(back.calls(), [true]);
    h.step();
    h.step();
    assert_eq!(back.calls(), [true], "sent only when the value changes");
    h.app.popovers.close();
    h.app.inbox.push_back();
    h.step();
    h.step();
    assert!(h.app.gallery_active(), "Back on the canvas opens the gallery");
    // The gallery with nothing open: Back belongs to the system.
    assert_eq!(back.calls().last(), Some(&false));
    // A late Back finds nothing and is ignored.
    h.app.inbox.push_back();
    h.step();
    assert!(h.app.gallery_active());
}

#[test]
fn staging_picks_do_not_set_the_typed_export_path() {
    let (mut h, mock, dir) = android("typed");
    let before = h.app.export_path.clone();
    let stage = dir.join("s");
    fs::create_dir_all(&stage).unwrap();
    mock.push(Picked::Path(stage.join("a.png")));
    let ctx = h.ctx.clone();
    h.app.start_export(&ctx, ExportFormat::Png);
    wait_for(&mut h, "the save", exporting_done);
    assert_eq!(h.app.export_path, before);
}
