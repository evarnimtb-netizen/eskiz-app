//! Bits per channel: converting a whole document between 8 and 16
//! bits as one undo step, and the labels the interface uses.
//!
//! * 8 to 16 is exact (`v * 257`), so a round trip back to 8 is the identity.
//! * 16 to 8 rounds to nearest, or dithers (`dither`) so a smooth gradient that 16 bits holds
//!   does not turn into bands: see `eskiz_core::pixel::dither_threshold`.
//! * Layer masks are 8-bit at both depths and are left alone.
//! * Undo keeps both sets of tiles, so the history budget sees the 16-bit side at twice the size.

use eskiz_core::document::{CanvasPixels, Document};
use eskiz_core::Depth;

use crate::command::DocOp;

/// Converts the whole document to another depth. Undo puts back the old layers and depth.
pub struct ConvertDepth {
    before: CanvasPixels,
    after: CanvasPixels,
}

impl DocOp for ConvertDepth {
    fn apply(&self, doc: &mut Document) {
        doc.restore_canvas(doc.width, doc.height, doc.dpi, &self.after);
    }

    fn revert(&self, doc: &mut Document) {
        doc.restore_canvas(doc.width, doc.height, doc.dpi, &self.before);
    }

    fn bytes(&self) -> usize {
        let tiles = |px: &CanvasPixels| px.layers.iter().map(|(_, l)| l.tile_bytes()).sum::<usize>();
        tiles(&self.before) + tiles(&self.after)
    }
}

/// The conversion of `doc` to `target`. `None` when the document already has that depth.
pub fn plan_convert_depth(doc: &Document, target: Depth, dither: bool) -> Option<ConvertDepth> {
    if doc.depth == target {
        return None;
    }
    let before = doc.canvas_pixels();
    let mut after = before.clone();
    after.depth = target;
    for (_, layer) in &mut after.layers {
        *layer = layer.converted(target, dither);
    }
    Some(ConvertDepth { before, after })
}

/// The other depth, for the "Convert to ..." row.
pub fn other(depth: Depth) -> Depth {
    match depth {
        Depth::U8 => Depth::U16,
        Depth::U16 => Depth::U8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    fn painted(depth: Depth) -> Session {
        let mut doc = Document::new_depth(300, 40, depth);
        let id = doc.layer_id(0).unwrap();
        let l = doc.layer_mut(id).unwrap();
        for x in 0..300 {
            let v = (x * 255 / 299) as u8;
            l.set_pixel(x, 5, [v, v / 2, 0, v.max(1)]);
        }
        Session::new(doc)
    }

    #[test]
    fn convert_up_and_back_is_the_identity_and_one_undo_step_each() {
        let mut s = painted(Depth::U8);
        let id = s.doc.layer_id(0).unwrap();
        let original = s.doc.layer(id).unwrap().to_rgba();
        let op = plan_convert_depth(&s.doc, Depth::U16, false).expect("8 to 16");
        s.apply_doc_op("Convert to 16-bit", Box::new(op)).expect("applied");
        assert_eq!(s.doc.depth, Depth::U16);
        assert_eq!(s.doc.layer(id).unwrap().depth(), Depth::U16);
        assert_eq!(s.doc.layer(id).unwrap().to_rgba(), original, "widening keeps every 8-bit value");
        let op = plan_convert_depth(&s.doc, Depth::U8, true).expect("16 to 8");
        s.apply_doc_op("Convert to 8-bit", Box::new(op)).expect("applied");
        assert_eq!(s.doc.depth, Depth::U8);
        assert_eq!(s.doc.layer(id).unwrap().to_rgba(), original, "8 to 16 to 8 is the identity, dithered or not");
        // Undo twice: back to 16, then back to 8, each exact.
        s.undo().expect("undo");
        assert_eq!(s.doc.depth, Depth::U16);
        s.undo().expect("undo");
        assert_eq!(s.doc.depth, Depth::U8);
        assert_eq!(s.doc.layer(id).unwrap().to_rgba(), original);
        s.redo().expect("redo");
        assert_eq!(s.doc.depth, Depth::U16);
    }

    #[test]
    fn same_depth_plans_nothing_and_undo_accounts_the_16_bit_size() {
        let s = painted(Depth::U16);
        assert!(plan_convert_depth(&s.doc, Depth::U16, false).is_none());
        let up = plan_convert_depth(&painted(Depth::U8).doc, Depth::U16, false).unwrap();
        let tiles = painted(Depth::U8).doc.layer(1).unwrap().tile_count();
        assert_eq!(up.bytes(), tiles * (Depth::U8.tile_bytes() + Depth::U16.tile_bytes()));
    }

    /// A 16-bit document with a smooth, many-level layer in it.
    fn deep_doc() -> Document {
        let mut doc = Document::new_depth(300, 60, Depth::U16);
        let id = doc.layer_id(0).unwrap();
        let l = doc.layer_mut(id).unwrap();
        for y in 0..60 {
            for x in 0..300 {
                let a = 20_000 + (x * 131 + y * 17) as u16 % 45_000;
                let c = |k: usize| ((x * (k + 3) * 211 + y * 97 + k * 13) as u32 % (u32::from(a) + 1)) as u16;
                l.set_pixel16(x, y, [c(0), c(1), c(2), a]);
            }
        }
        doc
    }

    #[test]
    fn a_16_bit_document_saves_and_opens_exactly() {
        let doc = deep_doc();
        let id = doc.layer_id(0).unwrap();
        let dir = std::env::temp_dir().join(format!("eskiz-depth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deep.procreate");
        let bytes = crate::storage::to_builder(&doc, "Deep").to_bytes().unwrap();
        crate::storage::write_atomic(&path, &bytes).unwrap();
        let back = crate::import::open_procreate(&path).unwrap();
        assert_eq!(back.depth, Depth::U16);
        let l = back.layer(back.layer_id(0).unwrap()).unwrap();
        assert_eq!(l.depth(), Depth::U16);
        assert_eq!(l.to_rgba16(), doc.layer(id).unwrap().to_rgba16(), "every 16-bit sample survives");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fill_clear_and_pour_fill_keep_16_bits() {
        let mut s = Session::new(deep_doc());
        let id = s.doc.layer_id(0).unwrap();
        s.fill_layer(id, [10, 200, 90]).expect("fill");
        let l = s.doc.layer(id).unwrap();
        assert_eq!(l.depth(), Depth::U16);
        assert_eq!(l.pixel16(7, 7), [10 * 257, 200 * 257, 90 * 257, 65_535]);
        s.undo().unwrap();
        assert_ne!(s.doc.layer(id).unwrap().pixel16(7, 7)[3], 0);
        s.clear_layer(id).expect("clear");
        assert!(s.doc.layer(id).unwrap().is_blank());
        s.undo().unwrap();
        // Pour Fill fills a region of a 16-bit layer with the colour widened exactly.
        s.clear_layer(id).unwrap();
        s.pour_fill(id, (5, 5), 0.5, [255, 0, 0]).expect("drop");
        let l = s.doc.layer(id).unwrap();
        assert_eq!(l.depth(), Depth::U16);
        assert_eq!(l.pixel16(150, 30), [65_535, 0, 0, 65_535]);
    }

    #[test]
    fn exports_keep_16_bits_where_the_format_can() {
        use crate::export::{export_with, ExportFormat, ExportSettings};
        let doc = deep_doc();
        let dir = std::env::temp_dir().join(format!("eskiz-depth-exp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let settings = ExportSettings::default();
        // PNG: 16-bit.
        let png_path = export_with(&doc, ExportFormat::Png, &dir.join("deep"), &settings).unwrap();
        let reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&png_path).unwrap())).read_info().unwrap();
        assert_eq!(reader.info().bit_depth, png::BitDepth::Sixteen);
        // TIFF: written (the 16-bit samples are checked by eskiz-format's own tests).
        let tiff_path = export_with(&doc, ExportFormat::Tiff, &dir.join("deep"), &settings).unwrap();
        assert!(matches!(&std::fs::read(&tiff_path).unwrap()[..2], b"II" | b"MM"));
        // PSD: depth 16, and our reader gets the same pixels back (within the straight-colour rounding).
        let psd_path = export_with(&doc, ExportFormat::Psd, &dir.join("deep"), &settings).unwrap();
        let bytes = std::fs::read(&psd_path).unwrap();
        assert_eq!(u16::from_be_bytes([bytes[22], bytes[23]]), 16);
        let (_, layers) = eskiz_format::psd::from_bytes16(&bytes).unwrap();
        let want = doc.layer(doc.layer_id(0).unwrap()).unwrap().to_rgba16();
        let worst = layers[0].iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "{worst}");
        // JPEG, WebP and PDF are written (dithered to 8 bits).
        for f in [ExportFormat::Jpeg, ExportFormat::WebP, ExportFormat::Pdf] {
            let p = export_with(&doc, f, &dir.join("deep"), &settings).unwrap();
            assert!(std::fs::metadata(p).unwrap().len() > 100);
        }
        // Per-layer PNGs are 16-bit too.
        let folder = export_with(&doc, ExportFormat::Layers, &dir.join("layers"), &settings).unwrap();
        let first = std::fs::read_dir(&folder).unwrap().next().unwrap().unwrap().path();
        let reader = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(first).unwrap())).read_info().unwrap();
        assert_eq!(reader.info().bit_depth, png::BitDepth::Sixteen);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_memory_counts_the_16_bit_tiles_at_twice_the_size() {
        let mut s8 = Session::new(Document::new_depth(300, 60, Depth::U8));
        let mut s16 = Session::new(Document::new_depth(300, 60, Depth::U16));
        for s in [&mut s8, &mut s16] {
            let id = s.doc.layer_id(0).unwrap();
            s.fill_layer(id, [1, 2, 3]).unwrap();
        }
        assert_eq!(s16.undo_bytes(), 2 * s8.undo_bytes());
        assert_eq!(s16.doc.tile_bytes(), 2 * s8.doc.tile_bytes());
    }

    #[test]
    fn smudge_and_commit_adjustments_do_not_quantise_a_16_bit_layer() {
        // A smudge dab on a 16-bit layer mixes at 16 bits: values stay off the 8-bit grid.
        let mut doc = Document::new_depth(120, 60, Depth::U16);
        let id = doc.layer_id(0).unwrap();
        let l = doc.layer_mut(id).unwrap();
        for y in 0..60 {
            for x in 0..120 {
                let v = (x * 540) as u16;
                l.set_pixel16(x, y, [v, v / 2, v / 3, 65_535]);
            }
        }
        let off_grid = |l: &eskiz_core::Layer| {
            l.to_rgba16().as_chunks::<4>().0.iter().filter(|p| p[0] % 257 != 0).count()
        };
        assert!(off_grid(doc.layer(id).unwrap()) > 1000);
        // Opacity at 90 percent through the same generic filter the adjustment commit uses.
        let mut adj = eskiz_core::filters::Adjustment::new(eskiz_core::filters::AdjustKind::Opacity);
        adj.set(0, 0.9);
        let src = doc.layer(id).unwrap().to_rgba16();
        let out = crate::adjust::render_masked(&adj, &src, 120, 60, eskiz_core::filters::Quality::Full, None, None, None);
        let mut s = Session::new(doc);
        s.replace_layer(id, eskiz_core::Layer::from_rgba16(120, 60, out), "Opacity").expect("applied");
        assert_eq!(s.doc.layer(id).unwrap().depth(), Depth::U16);
        assert!(off_grid(s.doc.layer(id).unwrap()) > 1000, "the committed layer is still 16-bit");
        // An 8-bit layer handed to replace_layer is brought to the document's depth.
        s.replace_layer(id, eskiz_core::Layer::from_rgba(120, 60, vec![9; 120 * 60 * 4]), "Eight").expect("applied");
        assert_eq!(s.doc.layer(id).unwrap().depth(), Depth::U16);
        assert_eq!(s.doc.layer(id).unwrap().pixel16(0, 0), [9 * 257; 4]);
    }
}
