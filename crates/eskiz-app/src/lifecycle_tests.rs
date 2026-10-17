//! Lifecycle policy: call order with a recording host, and the real journal behind a
//! simulated app (suspend, resume, repeated cycles, kill then recover). No window needed.

use super::*;
use crate::journal::paint;
use crate::storage::{self, sample_document, Library};
use eskiz_core::canvas::Layer;
use std::path::PathBuf;

#[derive(Default)]
struct Rec {
    calls: Vec<&'static str>,
    journal_fails: bool,
}

impl Host for Rec {
    fn stop_stroke(&mut self) {
        self.calls.push("stop_stroke");
    }
    fn write_journal(&mut self) -> Result<(), String> {
        self.calls.push("journal");
        if self.journal_fails {
            Err("disk full".to_owned())
        } else {
            Ok(())
        }
    }
    fn persist_prefs(&mut self) {
        self.calls.push("prefs");
    }
    fn recreate_surface(&mut self) {
        self.calls.push("surface");
    }
    fn reupload_layers(&mut self) {
        self.calls.push("upload");
    }
}

#[test]
fn suspend_stops_stroke_then_writes_journal_then_prefs() {
    let (mut lc, mut h) = (Lifecycle::default(), Rec::default());
    lc.suspend(&mut h, 10.0);
    assert_eq!(h.calls, ["stop_stroke", "journal", "prefs"]);
    assert_eq!(lc.phase(), Phase::Suspended);
    assert_eq!(lc.suspends, 1);
}

#[test]
fn prefs_are_persisted_even_when_the_journal_fails() {
    let (mut lc, mut h) = (Lifecycle::default(), Rec { journal_fails: true, ..Rec::default() });
    lc.suspend(&mut h, 0.0);
    assert!(h.calls.contains(&"prefs"));
    assert_eq!(lc.last_error.as_deref(), Some("disk full"));
    h.journal_fails = false;
    lc.suspend(&mut h, 0.5);
    assert_eq!(lc.last_error, None);
}

#[test]
fn a_frame_after_a_gap_resumes_and_uploads_layers() {
    let (mut lc, mut h) = (Lifecycle::default(), Rec::default());
    lc.suspend(&mut h, 10.0);
    h.calls.clear();
    lc.frame(&mut h, 10.0 + RESUME_GAP_SECS + 5.0);
    assert_eq!(h.calls, ["surface", "upload"]);
    assert_eq!((lc.phase(), lc.resumes), (Phase::Active, 1));
}

#[test]
fn a_periodic_save_does_not_reupload() {
    let (mut lc, mut h) = (Lifecycle::default(), Rec::default());
    lc.suspend(&mut h, 10.0);
    h.calls.clear();
    lc.frame(&mut h, 10.2);
    assert!(h.calls.is_empty());
    assert_eq!((lc.phase(), lc.resumes), (Phase::Active, 0));
    // Frames while active do nothing, and an explicit resume while active is a no-op.
    lc.frame(&mut h, 99.0);
    lc.resume(&mut h);
    assert!(h.calls.is_empty());
}

#[test]
fn double_suspend_counts_once_and_resumes_once() {
    let (mut lc, mut h) = (Lifecycle::default(), Rec::default());
    lc.suspend(&mut h, 1.0);
    lc.suspend(&mut h, 2.0);
    assert_eq!(lc.suspends, 1);
    h.calls.clear();
    lc.frame(&mut h, 30.0);
    lc.frame(&mut h, 31.0);
    assert_eq!(h.calls, ["surface", "upload"]);
}

// ---- the real journal behind a simulated app

/// A document on the canvas, with its planner, as `EskizApp` holds them.
struct SimApp {
    doc: Document,
    planner: Planner,
    root: PathBuf,
    id: String,
    /// A stroke in flight: pixels painted but not committed.
    stroke: Option<(usize, usize)>,
    uploads: usize,
    prefs_writes: usize,
    dirty: bool,
}

impl Host for SimApp {
    fn stop_stroke(&mut self) {
        self.stroke = None;
    }
    fn write_journal(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        journal_sync(&mut self.planner, &self.doc, &self.root, &self.id, "Sketch")
    }
    fn persist_prefs(&mut self) {
        self.prefs_writes += 1;
    }
    fn recreate_surface(&mut self) {}
    fn reupload_layers(&mut self) {
        self.uploads += self.doc.layer_count();
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-lifecycle-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn pixels(d: &Document) -> Vec<Vec<u8>> {
    (0..d.layer_count()).map(|i| d.layer_at(i).map(Layer::to_rgba).unwrap_or_default()).collect()
}

fn sim(name: &str) -> (SimApp, Library, PathBuf) {
    let dir = temp(name);
    let lib = Library::at(&dir);
    let doc = sample_document(300, 200);
    let id = storage::new_id();
    lib.save(&id, "Sketch", &doc).unwrap();
    let app = SimApp {
        planner: Planner::based_on(&doc),
        doc,
        root: lib.recovery_root().to_owned(),
        id,
        stroke: None,
        uploads: 0,
        prefs_writes: 0,
        dirty: false,
    };
    (app, lib, dir)
}

#[test]
fn suspend_writes_a_journal_that_restores_the_unsaved_document() {
    let (mut app, lib, dir) = sim("restore");
    paint(&mut app.doc, 0, 10, 10, 120, 80);
    app.dirty = true;
    let mut lc = Lifecycle::default();
    lc.suspend(&mut app, 1.0);
    assert_eq!(lc.last_error, None);
    assert_eq!(app.prefs_writes, 1);
    let back = journal::restore(lib.recovery_root(), &lib, &app.id).unwrap();
    assert_eq!(pixels(&back), pixels(&app.doc));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stroke_in_flight_is_stopped_before_the_journal() {
    let (mut app, _lib, dir) = sim("stroke");
    app.stroke = Some((1, 2));
    app.dirty = true;
    Lifecycle::default().suspend(&mut app, 0.0);
    assert!(app.stroke.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn fifty_suspend_resume_cycles_lose_no_strokes_and_leak_no_files() {
    let (mut app, lib, dir) = sim("cycles");
    let mut lc = Lifecycle::default();
    let mut now = 0.0;
    let mut tile_files_seen = 0;
    for i in 0..50usize {
        // One more stroke per cycle, on alternating layers, at moving positions.
        paint(&mut app.doc, i % 2, (i * 5) % 250, (i * 3) % 150, 8, 8);
        app.dirty = true;
        now += 10.0;
        lc.suspend(&mut app, now);
        now += 20.0;
        lc.frame(&mut app, now);
        // The journal on disk always equals the document as of this suspend.
        let back = journal::restore(lib.recovery_root(), &lib, &app.id).unwrap();
        assert_eq!(pixels(&back), pixels(&app.doc), "cycle {i}");
        let files = std::fs::read_dir(lib.recovery_root().join(&app.id)).unwrap().count();
        tile_files_seen = tile_files_seen.max(files);
    }
    assert_eq!((lc.suspends, lc.resumes), (50, 50));
    assert_eq!(app.uploads, 50 * app.doc.layer_count());
    // The document is 300x200: a handful of tiles per layer, never 50 cycles' worth.
    assert!(tile_files_seen < 40, "journal directory grew to {tile_files_seen} files");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn kill_after_suspend_then_recover_offers_and_restores_the_work() {
    let (mut app, lib, dir) = sim("kill");
    paint(&mut app.doc, 1, 40, 40, 60, 60);
    app.dirty = true;
    let mut lc = Lifecycle::default();
    lc.suspend(&mut app, 3.0);
    let expect = pixels(&app.doc);
    let id = app.id.clone();
    drop(app); // the process is killed: no exit handler runs

    // Next start: a fresh Library value over the same data folder.
    let again = Library::at(&dir);
    let pending = journal::scan(again.recovery_root(), &again);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, id);
    let back = journal::restore(again.recovery_root(), &again, &id).unwrap();
    assert_eq!(pixels(&back), expect);
    drop(lib);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_clean_document_writes_no_journal() {
    let (mut app, lib, dir) = sim("clean");
    Lifecycle::default().suspend(&mut app, 0.0);
    assert!(!lib.recovery_root().join(&app.id).exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unsaved_new_document_is_recovered_without_a_package() {
    let dir = temp("unsaved");
    let lib = Library::at(&dir);
    let mut doc = sample_document(200, 150);
    paint(&mut doc, 0, 0, 0, 50, 50);
    let id = storage::new_id();
    let mut app = SimApp { planner: Planner::unsaved(), doc, root: lib.recovery_root().to_owned(), id, stroke: None, uploads: 0, prefs_writes: 0, dirty: true };
    Lifecycle::default().suspend(&mut app, 0.0);
    let back = journal::restore(lib.recovery_root(), &lib, &app.id).unwrap();
    assert_eq!(pixels(&back), pixels(&app.doc));
    let _ = std::fs::remove_dir_all(dir);
}
