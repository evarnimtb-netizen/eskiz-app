//! Writes the files `android/files_smoke.sh` pushes to the emulator's Download folder.
//!
//! Run: `ESKIZ_FIXTURES_DIR=/some/dir cargo test -p eskiz-app --lib -- --ignored write_android_fixtures`
//! (the script does this itself when the directory is missing). Writes `fixture.procreate`
//! (three layers with known pixels), `fixture.png`, `fixture.brushset`, `fixture.swatches` and
//! `ref.png`, the flattened PNG export of the fixture that the script compares the app's saves with.

use std::path::PathBuf;

use eskiz_core::{Document, Props};
use eskiz_format::brushes::{write_brushset, ImportedBrush};
use eskiz_format::palettes::{write_swatches, Palette, Swatch};

use crate::export::{export_document, ExportFormat};

pub const WIDTH: usize = 96;
pub const HEIGHT: usize = 64;

/// A 96 x 64 document with three layers: an opaque diagonal gradient, a half-transparent red bar
/// and a small opaque blue square. Everything is a function of the position, so the file is the
/// same on every run.
pub fn fixture_document() -> Document {
    let mut doc = Document::empty(WIDTH, HEIGHT);
    let mut base = Vec::with_capacity(WIDTH * HEIGHT * 4);
    let mut bar = vec![0u8; WIDTH * HEIGHT * 4];
    let mut square = vec![0u8; WIDTH * HEIGHT * 4];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            base.extend_from_slice(&[(x * 255 / (WIDTH - 1)) as u8, (y * 255 / (HEIGHT - 1)) as u8, 96, 255]);
            let at = (y * WIDTH + x) * 4;
            if (20..28).contains(&y) {
                // Premultiplied 50 % red.
                bar[at..at + 4].copy_from_slice(&[128, 0, 0, 128]);
            }
            if (40..56).contains(&y) && (60..76).contains(&x) {
                square[at..at + 4].copy_from_slice(&[0, 0, 255, 255]);
            }
        }
    }
    doc.push_layer(None, Props::new("Base"), None, base);
    doc.push_layer(None, Props::new("Bar"), None, bar);
    doc.push_layer(None, Props::new("Square"), None, square);
    doc
}

fn swatches() -> Palette {
    let sw = |name: &str, rgb: [f32; 3]| Swatch { name: Some(name.to_owned()), rgba: [rgb[0], rgb[1], rgb[2], 1.0] };
    Palette {
        name: "Eskiz Test Palette".to_owned(),
        swatches: vec![sw("Rust", [0.72, 0.25, 0.05]), sw("Sky", [0.25, 0.55, 0.9]), sw("Moss", [0.3, 0.45, 0.2])],
    }
}

#[test]
fn the_fixture_document_is_stable() {
    let doc = fixture_document();
    assert_eq!(doc.root.len(), 3);
    let a = std::env::temp_dir().join(format!("eskiz-fix-a-{}.png", std::process::id()));
    let b = std::env::temp_dir().join(format!("eskiz-fix-b-{}.png", std::process::id()));
    export_document(&doc, ExportFormat::Png, &a).unwrap();
    export_document(&fixture_document(), ExportFormat::Png, &b).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    let _ = std::fs::remove_file(a);
    let _ = std::fs::remove_file(b);
}

#[test]
#[ignore = "writes files for android/files_smoke.sh"]
fn write_android_fixtures() {
    let dir = PathBuf::from(std::env::var("ESKIZ_FIXTURES_DIR").expect("set ESKIZ_FIXTURES_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let doc = fixture_document();
    export_document(&doc, ExportFormat::Procreate, &dir.join("fixture.procreate")).unwrap();
    export_document(&doc, ExportFormat::Png, &dir.join("ref.png")).unwrap();
    // The image to import is the same picture, under another name.
    std::fs::copy(dir.join("ref.png"), dir.join("fixture.png")).unwrap();
    let brush = ImportedBrush { name: "Fixture Brush".into(), ..ImportedBrush::default() };
    std::fs::write(dir.join("fixture.brushset"), write_brushset("Eskiz Test Set", &[brush]).unwrap()).unwrap();
    std::fs::write(dir.join("fixture.swatches"), write_swatches(&swatches()).unwrap()).unwrap();
    println!("fixtures written to {}", dir.display());
}
