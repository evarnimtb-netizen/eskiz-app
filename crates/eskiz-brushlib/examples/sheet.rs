//! Writes a PNG contact sheet with a stroke preview of every built-in preset.
//!
//! Run with `cargo run -p eskiz-brushlib --example sheet`. The sheet goes to the
//! system temp directory as `eskiz-brush-library-sheet.png`, and the path is
//! printed. Each set starts on a new row under a coloured header band.

use eskiz_brushlib::{library_ref, preview_stroke};
use image::{imageops, Rgba, RgbaImage};

const CELL_W: u32 = 300;
const CELL_H: u32 = 110;
const COLUMNS: u32 = 4;
const GAP: u32 = 8;
const HEADER: u32 = 22;
const BACKGROUND: Rgba<u8> = Rgba([28, 28, 32, 255]);

fn fill_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, colour: Rgba<u8>) {
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            img.put_pixel(px, py, colour);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sets = library_ref();

    let sheet_w = COLUMNS * (CELL_W + GAP) + GAP;
    let rows_total: u32 = sets
        .iter()
        .map(|s| (s.brushes.len() as u32).div_ceil(COLUMNS))
        .sum();
    let sheet_h = sets.len() as u32 * HEADER + rows_total * (CELL_H + GAP) + GAP;

    let mut sheet = RgbaImage::from_pixel(sheet_w, sheet_h, BACKGROUND);
    let mut y = GAP;
    for (set_index, set) in sets.iter().enumerate() {
        // A distinct hue per set, so sections are easy to tell apart.
        let shade = 70 + (set_index as u8).wrapping_mul(22);
        fill_rect(
            &mut sheet,
            GAP,
            y,
            sheet_w - 2 * GAP,
            HEADER - 4,
            Rgba([shade, shade / 2 + 40, 120, 255]),
        );
        y += HEADER;

        for (i, lb) in set.brushes.iter().enumerate() {
            let col = i as u32 % COLUMNS;
            let row = i as u32 / COLUMNS;
            let x = GAP + col * (CELL_W + GAP);
            let cell_y = y + row * (CELL_H + GAP);
            let preview = preview_stroke(lb, CELL_W, CELL_H);
            imageops::overlay(&mut sheet, &preview, i64::from(x), i64::from(cell_y));
        }
        let rows = (set.brushes.len() as u32).div_ceil(COLUMNS);
        y += rows * (CELL_H + GAP);
    }

    let path = std::env::temp_dir().join("eskiz-brush-library-sheet.png");
    sheet.save(&path)?;

    for set in sets {
        let names: Vec<&str> = set.brushes.iter().map(|b| b.name).collect();
        println!("{}: {}", set.name, names.join(", "));
    }
    println!("{}", path.display());
    Ok(())
}
