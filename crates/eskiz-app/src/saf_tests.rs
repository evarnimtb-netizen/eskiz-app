//! Host tests of the pure file bridge (`saf.rs`): MIME tables, names, the request table, the
//! cache rules and the debug hooks.

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;

use crate::animation_ui::AnimFormat;
use crate::export::ExportFormat;
use crate::file_access::{Delivered, PickPurpose, Picked};
use crate::saf::*;

const PURPOSES: [PickPurpose; 14] = [
    PickPurpose::OpenDocument,
    PickPurpose::ImportImage,
    PickPurpose::ImportFont,
    PickPurpose::ImportBrush,
    PickPurpose::Reference,
    PickPurpose::PaletteImport,
    PickPurpose::PaletteExport,
    PickPurpose::ExportBase,
    PickPurpose::ExportFolder,
    PickPurpose::Model3d,
    PickPurpose::Usdz,
    PickPurpose::GalleryImport,
    PickPurpose::GalleryShare,
    PickPurpose::ExportTree,
];

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("eskiz-saf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn every_open_purpose_lists_mime_types_and_save_purposes_one() {
    for p in PURPOSES {
        let m = mimes_for(p);
        let folder_or_base = matches!(p, PickPurpose::ExportBase | PickPurpose::ExportFolder | PickPurpose::GalleryShare | PickPurpose::ExportTree);
        assert_eq!(m.is_empty(), folder_or_base, "{p:?}");
        for t in &m {
            assert!(t.contains('/') && *t == t.to_lowercase(), "{t}");
        }
    }
    // Types Android does not know are offered through octet-stream and zip.
    for p in [PickPurpose::OpenDocument, PickPurpose::GalleryImport, PickPurpose::ImportBrush, PickPurpose::PaletteImport] {
        let m = mimes_for(p);
        assert!(m.contains(&"application/octet-stream"), "{p:?}");
    }
    assert!(mimes_for(PickPurpose::ImportImage).contains(&"image/vnd.adobe.photoshop"));
    assert!(mimes_for(PickPurpose::ImportImage).contains(&"model/gltf-binary"));
    assert!(mimes_for(PickPurpose::ImportImage).contains(&"model/vnd.usdz+zip"));
    assert_eq!(mimes_for(PickPurpose::Reference), ["image/png", "image/jpeg", "image/webp"]);
    assert!(mimes_for(PickPurpose::ImportFont).contains(&"font/ttf"));
    assert_eq!(mimes_for(PickPurpose::Model3d), ["model/gltf-binary"]);
}

#[test]
fn export_mime_types_cover_every_format() {
    let want = [
        (ExportFormat::Procreate, "application/octet-stream"),
        (ExportFormat::Psd, "image/vnd.adobe.photoshop"),
        (ExportFormat::Pdf, "application/pdf"),
        (ExportFormat::Png, "image/png"),
        (ExportFormat::Jpeg, "image/jpeg"),
        (ExportFormat::Tiff, "image/tiff"),
        (ExportFormat::WebP, "image/webp"),
        (ExportFormat::Layers, "image/png"),
    ];
    for f in ExportFormat::ALL {
        assert_eq!(mime_for_export(f), want.iter().find(|w| w.0 == f).unwrap().1, "{f:?}");
        assert_eq!(mimes_for(PickPurpose::SaveExport(f)), [mime_for_export(f)]);
    }
    assert_eq!(mime_for_anim(AnimFormat::Gif), "image/gif");
    assert_eq!(mime_for_anim(AnimFormat::Apng), "image/png");
    assert_eq!(mimes_for(PickPurpose::SaveAnim(AnimFormat::Gif)), ["image/gif"]);
    // .procreate must not claim zip: some providers would append ".zip".
    assert_ne!(mime_for_export(ExportFormat::Procreate), "application/zip");
}

#[test]
fn mime_by_name_and_common_mime() {
    assert_eq!(mime_for_name("a.PNG"), "image/png");
    assert_eq!(mime_for_name("a.jpg"), "image/jpeg");
    assert_eq!(mime_for_name("a.procreate"), "application/octet-stream");
    assert_eq!(mime_for_name("noext"), "application/octet-stream");
    assert_eq!(common_mime(&["a.png".into(), "b.png".into()]), "image/png");
    assert_eq!(common_mime(&["a.png".into(), "b.jpg".into()]), "*/*");
    assert_eq!(common_mime(&[]), "*/*");
}

#[test]
fn clean_name_table() {
    let long = "x".repeat(300);
    let cases: [(&str, &str, &str); 14] = [
        ("Art.procreate", "application/octet-stream", "Art.procreate"),
        ("a.PNG", "image/png", "a.PNG"),
        ("../../x.png", "image/png", "x.png"),
        ("..\\..\\x.png", "image/png", "x.png"),
        ("   ", "", "Untitled"),
        ("", "image/png", "Untitled.png"),
        ("photo", "image/jpeg", "photo.jpg"),
        ("photo", "application/x-unknown", "photo"),
        ("a\0b\nc.png", "image/png", "abc.png"),
        ("..", "", "Untitled"),
        ("my:file?.png", "", "myfile.png"),
        ("Zeichnung \u{fc}ber Tag.psd", "", "Zeichnung \u{fc}ber Tag.psd"),
        ("  spaced.png  ", "", "spaced.png"),
        ("dir/sub/model.glb", "model/gltf-binary", "model.glb"),
    ];
    for (display, mime, want) in cases {
        assert_eq!(clean_name(display, mime), want, "{display:?}");
    }
    let capped = clean_name(&format!("{long}.png"), "image/png");
    assert!(capped.len() <= MAX_NAME_BYTES, "{}", capped.len());
    assert!(capped.ends_with(".png"));
    // A cut never lands inside a multi-byte character.
    let uni = format!("{}.procreate", "\u{1F3A8}".repeat(60));
    let capped = clean_name(&uni, "");
    assert!(capped.len() <= MAX_NAME_BYTES && capped.ends_with(".procreate"));
    assert!(capped.is_char_boundary(capped.len() - ".procreate".len()));
    // Never a path component.
    for d in ["../a", "a/../b", "/etc/passwd", "a/b/c"] {
        let c = clean_name(d, "");
        assert!(!c.contains('/') && !c.contains(".."), "{c}");
    }
}

#[test]
fn export_names_come_from_the_title() {
    assert_eq!(export_file_name("Sunset", "png"), "Sunset.png");
    assert_eq!(export_file_name("My: Art/Work", "psd"), "My ArtWork.psd");
    assert_eq!(export_file_name("   ", "png"), "Untitled.png");
    assert_eq!(export_file_name("Sunset", ""), "Sunset");
    assert_eq!(export_file_name("Sunset.v2", "png"), "Sunset.png");
}

#[test]
fn extensions_are_checked_after_the_copy() {
    assert!(check_extension(PickPurpose::OpenDocument, "Art.procreate").is_ok());
    assert!(check_extension(PickPurpose::OpenDocument, "Art.PROCREATE").is_ok());
    assert_eq!(check_extension(PickPurpose::OpenDocument, "Art.zip").unwrap_err(), "That file is not a .procreate drawing.");
    assert!(check_extension(PickPurpose::ImportBrush, "x.brushset").is_ok());
    assert!(check_extension(PickPurpose::ImportBrush, "x.png").is_err());
    assert!(check_extension(PickPurpose::PaletteImport, "x.ase").is_ok());
    assert!(check_extension(PickPurpose::ImportFont, "x.ttf").is_ok());
    assert!(check_extension(PickPurpose::ImportFont, "x.exe").is_err());
    // Pictures are sniffed by the loader, not named.
    assert!(check_extension(PickPurpose::ImportImage, "whatever").is_ok());
    assert!(check_extension(PickPurpose::Reference, "x").is_ok());
}

#[test]
fn staging_paths_are_unique_per_request() {
    let cache = Path::new("/cache");
    assert_eq!(staging_dir(cache, 7), PathBuf::from("/cache/saf/7"));
    assert_ne!(staging_dir(cache, 7), staging_dir(cache, 8));
    assert_eq!(share_dir(cache, 3), PathBuf::from("/cache/share/3"));
    assert_eq!(clip_dir(cache), PathBuf::from("/cache/share/clip"));
}

#[test]
fn cleanup_rule_keeps_the_clip_and_fresh_files() {
    assert!(!is_stale(0, false));
    assert!(!is_stale(MAX_AGE_SECS, false));
    assert!(is_stale(MAX_AGE_SECS + 1, false));
    assert!(!is_stale(MAX_AGE_SECS * 10, true), "the current clipboard file is never deleted");
}

#[test]
fn provider_root_rule() {
    let root = Path::new("/cache/share");
    assert!(inside_root(root, Path::new("/cache/share/1/a.png")));
    assert!(!inside_root(root, Path::new("/cache/share")));
    assert!(!inside_root(root, Path::new("/cache/other/a.png")));
    assert!(!inside_root(root, Path::new("/cache/share-evil/a.png")));
    assert!(!inside_root(root, Path::new("/etc/passwd")));
}

#[test]
fn request_ids_are_unique_positive_and_small() {
    let mut t = Table::new();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..50 {
        let (tx, _rx) = channel();
        let id = t.alloc(Pending::Tree { tx });
        assert!((1..=0xFFFF).contains(&id));
        assert!(seen.insert(id), "id {id} reused while live");
    }
    assert_eq!(t.len(), 50);
}

#[test]
fn ids_wrap_without_reusing_live_ones() {
    let mut t = Table::new();
    let (tx, _rx) = channel();
    let first = t.alloc(Pending::Tree { tx });
    t.next = 0xFFFD;
    let (tx2, _rx2) = channel();
    let a = t.alloc(Pending::Tree { tx: tx2 });
    let (tx3, _rx3) = channel();
    let b = t.alloc(Pending::Tree { tx: tx3 });
    let (tx4, _rx4) = channel();
    let c = t.alloc(Pending::Tree { tx: tx4 });
    assert_eq!((a, b), (0xFFFE, 1 + 1), "wraps past the live id {first}");
    assert_ne!(c, first);
}

#[test]
fn an_open_pick_is_renamed_and_answered_once() {
    let dir = tmp("open");
    let copy = dir.join("in");
    std::fs::write(&copy, b"data").unwrap();
    let mut t = Table::new();
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Open { purpose: PickPurpose::OpenDocument, dir: dir.clone(), tx });
    t.resolve_pick(id, STATUS_OK, copy.to_str().unwrap(), "../Art.procreate", "application/octet-stream");
    assert_eq!(rx.try_recv().unwrap(), Picked::Path(dir.join("Art.procreate")));
    assert!(dir.join("Art.procreate").exists() && !copy.exists());
    // A second answer for the same id finds nothing.
    t.resolve_pick(id, STATUS_OK, "x", "y", "z");
    assert!(rx.try_recv().is_err());
    // (The notice queue is process-wide and other tests add to it: only look for ours.)
    assert!(take_notices().iter().any(|n| n == INTERRUPTED));
}

#[test]
fn a_wrong_extension_is_refused_and_the_copy_removed() {
    let dir = tmp("wrongext");
    let copy = dir.join("in");
    std::fs::write(&copy, b"data").unwrap();
    let mut t = Table::new();
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Open { purpose: PickPurpose::OpenDocument, dir: dir.clone(), tx });
    t.resolve_pick(id, STATUS_OK, copy.to_str().unwrap(), "notes.txt", "text/plain");
    assert_eq!(rx.try_recv().unwrap(), Picked::Unavailable("That file is not a .procreate drawing.".into()));
    assert!(!copy.exists());
}

#[test]
fn cancel_and_error_results() {
    let mut t = Table::new();
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Open { purpose: PickPurpose::ImportImage, dir: PathBuf::from("/nope"), tx });
    t.resolve_pick(id, STATUS_CANCELLED, "", "", "");
    assert_eq!(rx.try_recv().unwrap(), Picked::None);
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Open { purpose: PickPurpose::ImportImage, dir: PathBuf::from("/nope"), tx });
    t.resolve_pick(id, STATUS_ERROR, "", "Not enough space to copy the file.", "");
    assert_eq!(rx.try_recv().unwrap(), Picked::Unavailable("Not enough space to copy the file.".into()));
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Open { purpose: PickPurpose::ImportImage, dir: PathBuf::from("/nope"), tx });
    t.resolve_pick(id, STATUS_ERROR, "", "  ", "");
    assert_eq!(rx.try_recv().unwrap(), Picked::Unavailable("The file could not be read.".into()));
    assert!(t.is_empty());
}

#[test]
fn a_create_pick_answers_a_staging_path_and_remembers_the_destination() {
    let dir = tmp("create");
    let mut t = Table::new();
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Create { dir: dir.join("req"), tx });
    let dest = t.resolve_pick(id, STATUS_OK, "", "Sunset (1).png", "image/png");
    assert_eq!(rx.try_recv().unwrap(), Picked::Path(dir.join("req").join("Sunset (1).png")));
    assert_eq!(dest, Some((dir.join("req"), Dest::Doc(id))));
    assert!(dir.join("req").is_dir());
}

#[test]
fn a_tree_pick_answers_the_folder_name_and_a_stage_pick_a_folder() {
    let dir = tmp("tree");
    let mut t = Table::new();
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Tree { tx });
    t.resolve_pick(id, STATUS_OK, "", "Documents/Eskiz", "");
    assert_eq!(rx.try_recv().unwrap(), Picked::Path(PathBuf::from("Documents/Eskiz")));
    let (tx, rx) = channel();
    let id = t.alloc(Pending::TreeStage { dir: dir.join("s"), tx });
    let dest = t.resolve_pick(id, STATUS_OK, "", "Documents/Eskiz", "");
    assert_eq!(rx.try_recv().unwrap(), Picked::Path(dir.join("s")));
    assert_eq!(dest, Some((dir.join("s"), Dest::Tree)));
}

#[test]
fn commit_results() {
    let mut t = Table::new();
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Commit { tx });
    t.resolve_commit(id, STATUS_OK, "Sunset (1).png");
    assert_eq!(rx.try_recv().unwrap(), Delivered::Saved { name: "Sunset (1).png".into() });
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Commit { tx });
    t.resolve_commit(id, STATUS_ERROR, "No space left");
    assert_eq!(rx.try_recv().unwrap(), Delivered::Failed("No space left".into()));
    // An id of another kind, or an unknown one, is an interrupted request.
    t.resolve_commit(9999, STATUS_OK, "x");
    assert!(take_notices().iter().any(|n| n == INTERRUPTED));
}

#[test]
fn clip_results() {
    let mut t = Table::new();
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Clip { tx });
    t.resolve_clip(id, STATUS_OK, "/cache/in.png", "");
    assert_eq!(rx.try_recv().unwrap(), Ok(PathBuf::from("/cache/in.png")));
    let (tx, rx) = channel();
    let id = t.alloc(Pending::Clip { tx });
    t.resolve_clip(id, STATUS_ERROR, "", "The image could not be read.");
    assert_eq!(rx.try_recv().unwrap(), Err("The image could not be read.".into()));
}

#[test]
fn hooks_parse() {
    assert_eq!(parse_hook("open_document"), Some(Hook::OpenDocument));
    assert_eq!(parse_hook(" paste "), Some(Hook::Paste));
    assert_eq!(parse_hook("new_canvas"), Some(Hook::NewCanvas));
    assert_eq!(parse_hook("back_state"), Some(Hook::BackState));
    assert_eq!(parse_hook("gallery_share_first"), Some(Hook::GalleryShareFirst));
    assert_eq!(
        parse_hook("export:png:save"),
        Some(Hook::Export { format: HookFormat::Still(ExportFormat::Png), share: false })
    );
    assert_eq!(
        parse_hook("export:procreate:save"),
        Some(Hook::Export { format: HookFormat::Still(ExportFormat::Procreate), share: false })
    );
    assert_eq!(parse_hook("export:gif:share"), Some(Hook::Export { format: HookFormat::Anim(AnimFormat::Gif), share: true }));
    for bad in ["", "export", "export:png", "export:png:x", "export:bmp:save", "export:png:save:x", "rm -rf", "Open_Document"] {
        assert_eq!(parse_hook(bad), None, "{bad:?}");
    }
}

#[test]
fn hooks_only_run_in_debuggable_builds() {
    assert!(hooks_allowed(true));
    assert!(!hooks_allowed(false));
}

#[test]
fn queues_for_hooks_and_back() {
    // A private inbox, so tests running in parallel cannot drain each other's entries.
    let inbox = Inbox::default();
    inbox.push_hook("paste");
    inbox.push_hook("copy_all");
    assert_eq!(inbox.take_hooks(), ["paste", "copy_all"]);
    assert!(inbox.take_hooks().is_empty());
    inbox.push_back();
    inbox.push_back();
    assert_eq!(inbox.take_backs(), 2);
    assert_eq!(inbox.take_backs(), 0);
}
