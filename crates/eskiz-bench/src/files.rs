//! Scenario 7: files. `.procreate` save and load, PSD and PNG export, through the app's own
//! export and import functions and the Gallery's verified save.

use std::path::PathBuf;
use std::time::Instant;

use eskiz_app::export::{export_document, ExportFormat};
use eskiz_app::import::open_procreate;
use eskiz_app::storage::Library;

use crate::ctx::Ctx;
use crate::fixtures::{self, Layout};
use crate::report::{fmt_ms, mib, ms_since};

fn file_size(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

pub fn run(cx: &mut Ctx) {
    let t = cx.report.table("7. Files", &["canvas", "document", "operation", "n", "median ms", "min ms", "output size", "notes"]);
    cx.report.intro(t, "`export_document` is the Share sheet's path (`.procreate` = LZ4 tiles in a ZIP; PSD = RLE channels per layer; PNG = the flattened composite). `Library::save` is the Gallery's autosave: it writes the package, reopens it to verify it, and renders a thumbnail. `open_procreate` is the Gallery's load.");
    let dir: PathBuf = std::env::temp_dir().join(format!("eskiz-bench-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for &n in &cx.sizes.clone() {
        if cx.reduced(n) || cx.quick_skip(n) {
            cx.report.row(t, &[cx.size_label(n), "-".into(), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), "not run at this size (beyond the app's limit, or --quick)".into()]);
            continue;
        }
        for layout in [Layout::SparseN(20), Layout::OneFull] {
            // Export flattens to one buffer, PSD builds per-layer channel buffers.
            let need = layout.bytes(n) + fixtures::layer_bytes(n) * 3;
            let label = layout.label();
            if let Err(e) = cx.fits(need) {
                cx.report.row(t, &[cx.size_label(n), label, "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
                continue;
            }
            cx.log(&format!("files {n}² {label}"));
            let doc = fixtures::document(n, layout);
            let runs = if n >= 8192 || cx.quick { 1 } else { 2 };
            let base = dir.join(format!("doc-{n}-{}", layout.layers()));
            let mut procreate_path = None;
            for (op, format) in [("save .procreate (export_document)", ExportFormat::Procreate), ("export PSD", ExportFormat::Psd), ("export PNG", ExportFormat::Png)] {
                // The PSD writer flattens every layer to its own buffer first.
                if format == ExportFormat::Psd {
                    if let Err(e) = cx.fits(layout.bytes(n) + fixtures::layer_bytes(n) * (layout.layers() as u64 + 2)) {
                        cx.report.row(t, &[cx.size_label(n), label.clone(), op.into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
                        continue;
                    }
                }
                let mut ms = Vec::new();
                let mut out = None;
                let mut err = None;
                for _ in 0..=runs {
                    let t0 = Instant::now();
                    match export_document(&doc, format, &base) {
                        Ok(p) => {
                            ms.push(ms_since(t0));
                            out = Some(p);
                        }
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }
                match (out, err) {
                    (Some(p), None) => {
                        let tm = crate::report::Timing::from_ms(ms[1..].to_vec());
                        cx.report.row(t, &[cx.size_label(n), label.clone(), op.into(), tm.n().to_string(), fmt_ms(tm.median()), fmt_ms(tm.min()), mib(file_size(&p)), String::new()]);
                        if format == ExportFormat::Procreate {
                            procreate_path = Some(p);
                        } else {
                            let _ = std::fs::remove_file(&p);
                        }
                    }
                    (_, Some(e)) => cx.report.row(t, &[cx.size_label(n), label.clone(), op.into(), "-".into(), "-".into(), "-".into(), "-".into(), format!("failed: {e}")]),
                    _ => {}
                }
            }
            if let Some(p) = procreate_path {
                let mut ms = Vec::new();
                let mut err = None;
                for _ in 0..=runs {
                    let t0 = Instant::now();
                    match open_procreate(&p) {
                        Ok(d) => {
                            ms.push(ms_since(t0));
                            drop(d);
                        }
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }
                match err {
                    None => {
                        let tm = crate::report::Timing::from_ms(ms[1..].to_vec());
                        cx.report.row(t, &[cx.size_label(n), label.clone(), "load .procreate (open_procreate)".into(), tm.n().to_string(), fmt_ms(tm.median()), fmt_ms(tm.min()), mib(file_size(&p)), String::new()]);
                    }
                    Some(e) => cx.report.row(t, &[cx.size_label(n), label.clone(), "load .procreate".into(), "-".into(), "-".into(), "-".into(), "-".into(), format!("failed: {e}")]),
                }
                let _ = std::fs::remove_file(&p);
            }
            // The Gallery's verified save (package + verification reopen + thumbnail).
            let lib = Library::at(dir.join("library"));
            let id = format!("bench-{n}-{}", layout.layers());
            let mut ms = Vec::new();
            let mut err = None;
            for _ in 0..=runs {
                let t0 = Instant::now();
                match lib.save(&id, "Bench", &doc) {
                    Ok(_) => ms.push(ms_since(t0)),
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
            match err {
                None => {
                    let tm = crate::report::Timing::from_ms(ms[1..].to_vec());
                    cx.report.row(t, &[cx.size_label(n), label.clone(), "Gallery save (Library::save: write, verify, thumbnail)".into(), tm.n().to_string(), fmt_ms(tm.median()), fmt_ms(tm.min()), "-".into(), "the autosave path (every 30 s while dirty)".into()]);
                }
                Some(e) => cx.report.row(t, &[cx.size_label(n), label.clone(), "Gallery save".into(), "-".into(), "-".into(), "-".into(), "-".into(), format!("failed: {e}")]),
            }
            let _ = lib.delete(&id);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
