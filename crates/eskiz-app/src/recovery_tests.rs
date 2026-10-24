//! App-level recovery tests: a crash with unsaved work offers the restore sheet at the next
//! start, Restore brings the work back, Discard removes it, and a failed save keeps the
//! document dirty until a later save works. Headless: no window or GPU is needed.

use crate::app::{EskizApp, Prefs};
use crate::gallery::Screen;
use crate::snapshot::{snapshot_dir, Headless};
use crate::storage::{self, Library, FAIL_WRITES_UNDER};
use eframe::egui::Vec2;
use std::fs;
use std::path::PathBuf;

const WINDOW: Vec2 = Vec2::new(1280.0, 800.0);
const RED: [u8; 4] = [200, 30, 30, 255];

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-recovery-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// A library with one saved document. Returns the library and the document's ID.
fn library_with_one(name: &str) -> (Library, PathBuf, String) {
    let dir = temp(name);
    let lib = Library::at(&dir);
    let id = storage::new_id();
    lib.save(&id, "Harbour", &storage::sample_document(600, 400)).unwrap();
    (lib, dir, id)
}

fn start(lib: &Library) -> Headless {
    let lib = lib.clone();
    let mut h = Headless::from_app(WINDOW, move |ctx| EskizApp::with_library(ctx, Prefs::default(), lib));
    h.settle_storage();
    h.advance(0.3);
    h
}

/// Open the first card and wait for the canvas.
fn open_first(h: &mut Headless) {
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    h.advance(0.4);
    h.settle_storage();
    assert!(matches!(h.app.gal.screen, Screen::Canvas));
}

/// Quiet jobs (removing a journal) are not counted as in flight, so wait for the file system.
fn gone(path: &std::path::Path) -> bool {
    for _ in 0..400 {
        if !path.exists() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn paint_red(h: &mut Headless) {
    h.app.session.doc.layer_at_mut(0).unwrap().set_pixel(3, 4, RED);
    h.app.session.mark_edited();
}

#[test]
fn a_crash_with_unsaved_work_offers_the_sheet_and_restore_brings_it_back() {
    let (lib, dir, id) = library_with_one("crash");
    {
        let mut h = start(&lib);
        assert!(h.app.gal.offers.is_empty(), "a clean start offers nothing");
        h.app.gal.autosave_secs = 10_000.0; // only the journal runs
        open_first(&mut h);
        paint_red(&mut h);
        assert!(h.app.is_dirty());
        // Quiet for longer than the stroke debounce: the journal is written in the background.
        h.advance(1.5);
        h.settle_storage();
        h.advance(0.2);
        h.settle_storage();
        assert!(lib.recovery_root().join(&id).exists(), "a recovery copy exists");
        assert_ne!(lib.load(&id).unwrap().layer_at(0).unwrap().pixel(3, 4), RED, "the package was not rewritten");
        // The crash: the app is dropped with no final save.
    }

    let mut h = start(&lib);
    assert_eq!(h.app.gal.offers.len(), 1, "the sheet is offered at the next start");
    assert_eq!(h.app.gal.offers[0].title, "Harbour");
    h.advance(0.2);
    h.render().save_png(&snapshot_dir().join("recovery_sheet.png")).unwrap();

    // Restore.
    h.click(h.app.gal.recovery_buttons.1.center());
    h.settle_storage();
    h.advance(0.5);
    h.settle_storage();
    assert!(h.app.gal.offers.is_empty());
    assert!(matches!(h.app.gal.screen, Screen::Canvas), "the restored document is on the canvas");
    assert_eq!(h.app.session.doc.layer_at(0).unwrap().pixel(3, 4), RED, "the unsaved stroke is back");
    // It is saved at once, which removes the journal.
    h.advance(0.5);
    h.settle_storage();
    assert_eq!(lib.load(&id).unwrap().layer_at(0).unwrap().pixel(3, 4), RED);
    assert!(!h.app.is_dirty());
    assert!(gone(&lib.recovery_root().join(&id)), "the journal is gone after the save");
    let _ = fs::remove_dir_all(&dir);
}

/// eframe's key-value storage, in memory.
#[derive(Default)]
struct MemStorage(std::collections::HashMap<String, String>);

impl eframe::Storage for MemStorage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }
    fn flush(&mut self) {}
    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }
}

#[test]
fn suspend_writes_the_journal_at_once_and_a_kill_afterwards_is_recoverable() {
    let (lib, dir, id) = library_with_one("suspend");
    {
        let mut h = start(&lib);
        h.app.gal.autosave_secs = 10_000.0;
        open_first(&mut h);
        paint_red(&mut h);
        // No frames pass: the periodic journal has not run. Suspend writes it synchronously.
        assert!(!lib.recovery_root().join(&id).join("journal.json").exists());
        let mut storage = MemStorage::default();
        h.app.lifecycle_suspend(&mut storage);
        assert!(lib.recovery_root().join(&id).join("journal.json").exists(), "journal on disk when suspend returns");
        assert!(storage.0.contains_key(crate::app::PREF_KEY), "prefs persisted on suspend");
        assert_eq!(h.app.lifecycle.phase(), crate::lifecycle::Phase::Suspended);
        // The process is killed here: no further save.
    }
    let mut h = start(&lib);
    assert_eq!(h.app.gal.offers.len(), 1, "the unsaved document is offered at the next start");
    h.advance(0.2);
    h.click(h.app.gal.recovery_buttons.1.center());
    h.settle_storage();
    h.advance(0.5);
    h.settle_storage();
    assert_eq!(h.app.session.doc.layer_at(0).unwrap().pixel(3, 4), RED, "the unsaved stroke is back");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn many_suspend_resume_cycles_on_the_real_app_keep_the_stroke() {
    let (lib, dir, id) = library_with_one("cycles");
    let mut h = start(&lib);
    h.app.gal.autosave_secs = 10_000.0;
    open_first(&mut h);
    paint_red(&mut h);
    for _ in 0..50 {
        let mut storage = MemStorage::default();
        h.app.lifecycle_suspend(&mut storage);
        // A frame long after the suspend is a resume: layers are uploaded again.
        let ctx = h.ctx.clone();
        h.app.lifecycle_resume(&ctx);
    }
    assert_eq!(h.app.session.doc.layer_at(0).unwrap().pixel(3, 4), RED);
    assert!(lib.recovery_root().join(&id).join("journal.json").exists());
    let dirs = fs::read_dir(lib.recovery_root()).unwrap().count();
    assert_eq!(dirs, 1, "one journal folder, not one per cycle");
    assert_eq!((h.app.lifecycle.suspends, h.app.lifecycle.resumes), (50, 50));
    h.advance(0.3);
    assert_eq!(h.app.session.doc.layer_at(0).unwrap().pixel(3, 4), RED);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn discard_removes_the_recovery_copy_and_keeps_the_saved_package() {
    let (lib, dir, id) = library_with_one("discard");
    let before = lib.load(&id).unwrap().layer_at(0).unwrap().pixel(3, 4);
    {
        let mut h = start(&lib);
        h.app.gal.autosave_secs = 10_000.0;
        open_first(&mut h);
        paint_red(&mut h);
        h.advance(1.5);
        h.settle_storage();
        h.advance(0.2);
        h.settle_storage();
    }
    let mut h = start(&lib);
    assert_eq!(h.app.gal.offers.len(), 1);
    h.advance(0.2);
    h.click(h.app.gal.recovery_buttons.0.center());
    h.settle_storage();
    h.advance(0.3);
    h.settle_storage();
    assert!(h.app.gal.offers.is_empty());
    assert!(matches!(h.app.gal.screen, Screen::Gallery), "discard stays on the gallery");
    assert!(gone(&lib.recovery_root().join(&id)));
    assert_eq!(lib.load(&id).unwrap().layer_at(0).unwrap().pixel(3, 4), before);
    // Nothing is offered at the following start.
    drop(h);
    assert!(start(&lib).app.gal.offers.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_save_keeps_the_document_dirty_and_the_old_file_until_a_save_works() {
    let (lib, dir, id) = library_with_one("failed-save");
    let package = dir.join(format!("{id}.procreate"));
    let before = fs::read(&package).unwrap();
    let mut h = start(&lib);
    h.app.gal.autosave_secs = 2.0;
    open_first(&mut h);
    paint_red(&mut h);
    assert!(h.app.is_dirty());

    // The disk refuses writes: the autosave fails.
    FAIL_WRITES_UNDER.lock().unwrap().push(dir.clone());
    h.advance(2.5);
    h.settle_storage();
    h.advance(0.2);
    assert!(h.app.is_dirty(), "a failed save must leave the document dirty");
    assert!(h.app.toast.as_ref().is_some_and(|t| t.text.starts_with("Could not save")), "the user is told");
    assert_eq!(fs::read(&package).unwrap(), before, "the old package is untouched");
    // Leaving the canvas does not pretend it was saved either.
    h.click(h.app.gallery_button.center());
    h.settle_storage();
    assert!(h.app.is_dirty());

    // The work stays in the document (the failing folder holds the journal too), and the disk
    // working again lets the next autosave succeed.
    FAIL_WRITES_UNDER.lock().unwrap().retain(|p| p != &dir);
    h.advance(2.5);
    h.settle_storage();
    h.advance(0.2);
    h.settle_storage();
    assert!(!h.app.is_dirty(), "clean once a save has worked; toast {:?}", h.app.toast.as_ref().map(|t| t.text.clone()));
    assert_eq!(lib.load(&id).unwrap().layer_at(0).unwrap().pixel(3, 4), RED, "saved once the disk works");
    let _ = fs::remove_dir_all(&dir);
}
