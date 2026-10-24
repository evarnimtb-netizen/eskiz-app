//! Crash-recovery journal and verified saves: simulated crashes, discard, failed writes, the
//! `.bak` rollback and incremental snapshots. No window is needed.

use super::*;
use crate::storage::{self, meta_signature, sample_document, FAIL_NEXT_CHECK, FAIL_NEXT_WRITE};
use std::io::ErrorKind;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-journal-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn library(name: &str) -> (Library, PathBuf) {
    let dir = temp(name);
    (Library::at(&dir), dir)
}

fn same_pixels(a: &Document, b: &Document) -> bool {
    a.layer_count() == b.layer_count()
        && a.layer_ids() == b.layer_ids()
        && (0..a.layer_count()).all(|i| a.layer_at(i).map(Layer::to_rgba) == b.layer_at(i).map(Layer::to_rgba))
}

fn tile_files(dir: &Path) -> usize {
    fs::read_dir(dir).map(|d| d.flatten().filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some(TILE_EXT)).count()).unwrap_or(0)
}

#[test]
fn simulated_crash_then_restore_gives_identical_layers() {
    let (lib, _dir) = library("crash");
    let mut doc = sample_document(600, 400);
    let id = storage::new_id();
    lib.save(&id, "Crash", &doc).unwrap();
    let mut planner = Planner::based_on(&doc);
    let root = lib.recovery_root().to_owned();

    // Work after the save: paint two layers, add a layer, change a property, then snapshot.
    paint(&mut doc, 0, 5, 5, 300, 300);
    execute(&planner.plan(&doc, &root, &id, "Crash")).unwrap();
    doc.add_layer_above(None);
    paint(&mut doc, 2, 100, 100, 200, 200);
    doc.props_at_mut(1).unwrap().opacity = 0.4;
    execute(&planner.plan(&doc, &root, &id, "Crash")).unwrap();

    // The crash: the app is gone, only the files remain.
    drop(planner);
    let back = restore(&root, &lib, &id).unwrap();
    assert!(same_pixels(&back, &doc), "restored layers differ from the crashed document");
    assert_eq!(meta_signature(&back), meta_signature(&doc));
}

#[test]
fn restore_after_a_canvas_size_change_does_not_use_the_old_package() {
    let (lib, _dir) = library("resized");
    let doc = sample_document(600, 400);
    let id = storage::new_id();
    lib.save(&id, "Resized", &doc).unwrap();
    let mut planner = Planner::based_on(&doc);
    let root = lib.recovery_root().to_owned();
    // Crop & Resize, then a crash before the next save: the same layers on a smaller canvas.
    let cropped = sample_document(300, 200);
    assert_eq!(cropped.layer_ids(), doc.layer_ids());
    execute(&planner.plan(&cropped, &root, &id, "Resized")).unwrap();
    let back = restore(&root, &lib, &id).unwrap();
    assert_eq!((back.width, back.height), (300, 200));
    assert!(same_pixels(&back, &cropped), "the layers come from the journal, not the old package");
}

#[test]
fn restore_of_an_unsaved_document_needs_no_package() {
    let (lib, dir) = library("unsaved");
    let doc = sample_document(300, 200);
    let root = lib.recovery_root().to_owned();
    let mut planner = Planner::unsaved();
    execute(&planner.plan(&doc, &root, "fresh-1", "Fresh")).unwrap();
    let back = restore(&root, &lib, "fresh-1").unwrap();
    assert!(same_pixels(&back, &doc));
    let _ = dir;
}

#[test]
fn journal_writes_only_the_changed_tiles() {
    let (lib, dir) = library("incremental");
    let mut doc = sample_document(1024, 768); // 4 x 3 tiles per layer
    let id = storage::new_id();
    lib.save(&id, "Inc", &doc).unwrap();
    let mut planner = Planner::based_on(&doc);
    let root = lib.recovery_root().to_owned();

    paint(&mut doc, 0, 10, 10, 20, 20);
    let first = planner.plan(&doc, &root, &id, "Inc");
    assert_eq!(first.tiles.len(), 1, "one edited tile is one journal tile");
    execute(&first).unwrap();

    // Nothing changed: no tile is written, and the manifest is still rewritten.
    let idle = planner.plan(&doc, &root, &id, "Inc");
    assert!(idle.tiles.is_empty());
    execute(&idle).unwrap();

    // Two tiles on one layer, one tile on another.
    paint(&mut doc, 1, 300, 10, 10, 10); // tile (1, 0)
    paint(&mut doc, 1, 600, 600, 10, 10); // tile (2, 2)
    paint(&mut doc, 0, 10, 10, 2, 2); // tile (0, 0) again
    let third = planner.plan(&doc, &root, &id, "Inc");
    assert_eq!(third.tiles.len(), 3);
    execute(&third).unwrap();

    // Only the listed tile files remain on disk; old ones were removed.
    assert_eq!(tile_files(&root.join(&id)), third.keep.len());
    let back = restore(&root, &lib, &id).unwrap();
    assert!(same_pixels(&back, &doc));
    let _ = dir;
}

#[test]
fn discard_removes_the_journal() {
    let (lib, dir) = library("discard");
    let mut doc = sample_document(300, 200);
    let id = storage::new_id();
    lib.save(&id, "Discard", &doc).unwrap();
    let root = lib.recovery_root().to_owned();
    let mut planner = Planner::based_on(&doc);
    paint(&mut doc, 0, 1, 1, 4, 4);
    execute(&planner.plan(&doc, &root, &id, "Discard")).unwrap();
    assert!(root.join(&id).join(MANIFEST).exists());

    discard(&root, &id).unwrap();
    assert!(!root.join(&id).exists(), "discard removes the journal");
    // Discarding again is harmless.
    discard(&root, &id).unwrap();
    let _ = dir;
}

#[test]
fn scan_offers_only_journals_newer_than_their_package() {
    let (lib, dir) = library("scan");
    let mut doc = sample_document(300, 200);
    let id = storage::new_id();
    lib.save(&id, "Scan", &doc).unwrap();
    let root = lib.recovery_root().to_owned();
    std::thread::sleep(Duration::from_millis(20));
    let mut planner = Planner::based_on(&doc);
    paint(&mut doc, 0, 1, 1, 4, 4);
    execute(&planner.plan(&doc, &root, &id, "Scan")).unwrap();
    let offers = scan(&root, &lib);
    assert_eq!(offers.len(), 1);
    assert_eq!(offers[0].title, "Scan");

    // A save after the journal makes it stale: it is removed, not offered.
    std::thread::sleep(Duration::from_millis(20));
    lib.save(&id, "Scan", &doc).unwrap();
    assert!(scan(&root, &lib).is_empty());
    assert!(!root.join(&id).exists());
    let _ = dir;
}

#[test]
fn a_failed_save_keeps_the_old_file_intact_and_leaves_no_temporary_file() {
    let (lib, dir) = library("failed-save");
    let doc = sample_document(300, 200);
    let id = storage::new_id();
    lib.save(&id, "Keep", &doc).unwrap();
    let before = fs::read(lib.root().join(format!("{id}.procreate"))).unwrap();

    let mut edited = doc.clone();
    paint(&mut edited, 0, 0, 0, 50, 50);
    FAIL_NEXT_WRITE.with(|c| c.set(Some(ErrorKind::PermissionDenied)));
    let err = lib.save(&id, "Keep", &edited).unwrap_err();
    assert!(err.contains("permission denied"), "message: {err}");

    let after = fs::read(lib.root().join(format!("{id}.procreate"))).unwrap();
    assert_eq!(before, after, "the old package is unchanged");
    assert!(same_pixels(&lib.load(&id).unwrap(), &doc));
    let leftovers: Vec<_> = fs::read_dir(lib.root()).unwrap().flatten().map(|e| e.file_name()).collect();
    assert!(!leftovers.iter().any(|n| n.to_string_lossy().ends_with(".tmp")), "{leftovers:?}");
    let _ = dir;
}

#[test]
fn a_full_disk_gives_a_plain_message() {
    let (lib, dir) = library("disk-full");
    let doc = sample_document(300, 200);
    FAIL_NEXT_WRITE.with(|c| c.set(Some(ErrorKind::StorageFull)));
    let err = lib.save(&storage::new_id(), "Full", &doc).unwrap_err();
    assert!(err.contains("the disk is full"), "message: {err}");
    let _ = dir;
}

#[test]
fn a_replacement_that_fails_its_check_is_rolled_back_from_bak() {
    let (lib, dir) = library("rollback");
    let first = sample_document(300, 200);
    let id = storage::new_id();
    lib.save(&id, "Roll", &first).unwrap();

    let mut second = first.clone();
    paint(&mut second, 0, 0, 0, 120, 120);
    FAIL_NEXT_CHECK.with(|c| c.set(true));
    assert!(lib.save(&id, "Roll", &second).is_err());

    assert!(same_pixels(&lib.load(&id).unwrap(), &first), "the previous version is back");
    assert!(!lib.root().join(format!(".{id}.procreate.bak")).exists(), "no backup is left behind");
    let _ = dir;
}

#[test]
fn load_falls_back_to_the_bak_copy_when_the_package_is_damaged() {
    let (lib, dir) = library("bak-load");
    let doc = sample_document(300, 200);
    let id = storage::new_id();
    lib.save(&id, "Bak", &doc).unwrap();
    let package = lib.root().join(format!("{id}.procreate"));
    fs::copy(&package, lib.root().join(format!(".{id}.procreate.bak"))).unwrap();
    fs::write(&package, b"not a package").unwrap();
    assert!(same_pixels(&lib.load(&id).unwrap(), &doc));
    let _ = dir;
}

#[test]
fn a_flush_for_a_panic_writes_the_last_planned_state() {
    let (lib, dir) = library("panic");
    let mut doc = sample_document(600, 400);
    let id = storage::new_id();
    lib.save(&id, "Panic", &doc).unwrap();
    let root = lib.recovery_root().to_owned();
    let mut planner = Planner::based_on(&doc);
    paint(&mut doc, 0, 50, 50, 100, 100);
    execute(&planner.plan(&doc, &root, &id, "Panic")).unwrap();
    // The panic happens after more work that was never planned: the flush writes the planned state.
    write_now(&root, &id, "Panic", &doc).unwrap();
    let back = restore(&root, &lib, &id).unwrap();
    assert!(same_pixels(&back, &doc));
    let _ = dir;
}

#[test]
fn restore_skips_a_missing_tile_and_counts_it() {
    let (lib, dir) = library("missing-tile");
    let doc = sample_document(300, 200);
    let root = lib.recovery_root().to_owned();
    let mut planner = Planner::unsaved();
    execute(&planner.plan(&doc, &root, "fresh-2", "Fresh")).unwrap();
    let tile = fs::read_dir(root.join("fresh-2")).unwrap().flatten().find(|e| e.path().extension().and_then(|x| x.to_str()) == Some(TILE_EXT)).unwrap();
    fs::remove_file(tile.path()).unwrap();
    assert!(restore(&root, &lib, "fresh-2").is_ok());
    let (_, skipped) = restore_counted(&root, &lib, "fresh-2").unwrap();
    assert_eq!(skipped, 1);
    let _ = dir;
}

#[test]
fn a_panic_flush_uses_tile_names_the_worker_never_writes() {
    let (lib, dir) = library("flush-names");
    let doc = sample_document(300, 200);
    let root = lib.recovery_root().to_owned();
    let mut planner = Planner::unsaved();
    let plan = planner.plan(&doc, &root, "fresh-3", "Fresh");
    let worker_names: Vec<String> = plan.tiles.iter().map(|(n, _)| n.clone()).collect();
    write_now(&root, "fresh-3", "Fresh", &doc).unwrap();
    for entry in fs::read_dir(root.join("fresh-3")).unwrap().flatten() {
        assert!(!worker_names.contains(&entry.file_name().to_string_lossy().into_owned()));
    }
    let _ = dir;
}

#[test]
fn layer_masks_and_group_flags_survive_a_save() {
    let (lib, _dir) = library("masks");
    let (w, h) = (300, 200); // more than one tile across and down
    let mut doc = sample_document(w, h);
    let layer = doc.layer_id(0).unwrap();
    let mut mask = eskiz_core::Mask::filled(w, h, 255);
    for y in 0..h {
        for x in 0..120 {
            mask.data[y * w + x] = if y < 50 { 0 } else { (x * 2) as u8 };
        }
    }
    doc.node_mut(layer).unwrap().mask = Some(mask.clone());
    let props = eskiz_core::Props { locked: true, private: true, ..eskiz_core::Props::new("grp") };
    let group = doc.push_group(None, props, None);
    doc.push_layer(Some(group), eskiz_core::Props::new("inside"), None, vec![200; w * h * 4]);
    let id = storage::new_id();
    lib.save(&id, "Masks", &doc).unwrap();
    let back = lib.load(&id).unwrap();
    let masked = back.root.iter().find(|n| n.mask.is_some()).expect("the layer keeps its mask");
    assert_eq!(masked.mask.as_ref().unwrap().data, mask.data);
    let g = back.root.iter().find(|n| n.is_group()).expect("the group is saved");
    assert!(g.props.locked && g.props.private, "the group keeps its lock and private flags");
}

#[test]
fn a_mask_added_after_the_save_is_restored_from_the_journal() {
    let (lib, _dir) = library("journal-mask");
    let mut doc = sample_document(300, 200);
    let id = storage::new_id();
    lib.save(&id, "Mask", &doc).unwrap();
    let root = lib.recovery_root().to_owned();
    let mut planner = Planner::based_on(&doc);
    let layer = doc.layer_id(0).unwrap();
    let mut mask = eskiz_core::Mask::filled(300, 200, 255);
    mask.data[..300 * 50].fill(0);
    doc.node_mut(layer).unwrap().mask = Some(mask.clone());
    execute(&planner.plan(&doc, &root, &id, "Mask")).unwrap();
    // An unchanged mask keeps its file; a plan after it still lists it.
    paint(&mut doc, 1, 10, 10, 20, 20);
    let second = planner.plan(&doc, &root, &id, "Mask");
    assert!(second.masks.is_empty(), "an unchanged mask is not written again");
    execute(&second).unwrap();
    let back = restore(&root, &lib, &id).unwrap();
    assert_eq!(back.node(layer).and_then(|n| n.mask.as_ref()).map(|m| &m.data), Some(&mask.data));
    // The panic flush works from the planner's base too.
    write_from(planner.clone(), &root, &id, "Mask", &doc).unwrap();
    let back = restore(&root, &lib, &id).unwrap();
    assert!(same_pixels(&back, &doc));
    assert_eq!(back.node(layer).and_then(|n| n.mask.as_ref()).map(|m| &m.data), Some(&mask.data));
}
