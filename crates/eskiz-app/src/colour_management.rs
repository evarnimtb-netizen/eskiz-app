//! Colour management: the document colour space, converting a document between
//! sRGB and Display P3, and how a document is mapped to the screen.
//!
//! * [`plan_convert`] builds the undoable [`ConvertSpace`] operation behind Actions > Canvas >
//!   Convert to sRGB / Display P3. It converts every layer's pixels (through straight colour, since
//!   layers hold premultiplied RGBA8) and the background colour with
//!   [`eskiz_guides::PixelConverter`], and keeps the old pixels for undo. Layer masks are not
//!   colours and are left alone. One call to [`Session::apply_doc_op`] makes it one undo step.
//! * [`display_transform`] is the conversion from the document's space to what the screen shows,
//!   which is sRGB (the surface eframe configures is sRGB, and the operating system colour-manages
//!   that onto the real display). Both compositors apply it: the CPU path with
//!   [`apply_display_rgba8`], the GPU path through the 3x3 matrix in the compositor's output pass
//!   (see `eskiz_gpu::Scene::display`).
//! * [`show`] maps a colour picked in the document's space to the colour the interface should paint,
//!   so swatches and the colour disc look like what the canvas will show.
//!
//! Text layers keep their colours in the space they were typed in; converting a document does not
//! rewrite the source runs, only the rendered pixels.

use std::cell::Cell;

use eskiz_core::canvas::Layer;
use eskiz_core::document::{CanvasPixels, Document};
use eskiz_core::{ColourSpace, PixelConverter};

use crate::command::DocOp;

/// The document as a colour conversion sees it: space, background and every layer's pixels.
#[derive(Clone)]
struct Snapshot {
    space: ColourSpace,
    background: [f64; 3],
    pixels: CanvasPixels,
}

impl Snapshot {
    fn put(&self, doc: &mut Document) {
        doc.colour_space = self.space;
        doc.background = self.background;
        doc.restore_canvas(doc.width, doc.height, doc.dpi, &self.pixels);
    }
}

/// Converts the whole document to another colour space. Undo puts back the old pixels and space.
pub struct ConvertSpace {
    before: Snapshot,
    after: Snapshot,
}

impl DocOp for ConvertSpace {
    fn apply(&self, doc: &mut Document) {
        self.after.put(doc);
    }

    fn revert(&self, doc: &mut Document) {
        self.before.put(doc);
    }

    fn bytes(&self) -> usize {
        let tiles = |px: &CanvasPixels| px.layers.iter().map(|(_, l)| l.tile_bytes()).sum::<usize>();
        tiles(&self.before.pixels) + tiles(&self.after.pixels)
    }
}

/// One layer converted. Transparent tiles stay absent; the rest are rewritten.
pub fn convert_layer(layer: &Layer, conv: &PixelConverter) -> Layer {
    let mut out = layer.clone();
    out.map_allocated(|px| conv.convert_premultiplied(px));
    out
}

/// The conversion of `doc` to `target`. `None` when the document is already in that space.
pub fn plan_convert(doc: &Document, target: ColourSpace) -> Option<ConvertSpace> {
    if doc.colour_space == target {
        return None;
    }
    let conv = PixelConverter::new(doc.colour_space, target);
    let before = Snapshot {
        space: doc.colour_space,
        background: doc.background,
        pixels: doc.canvas_pixels(),
    };
    let mut after = before.clone();
    after.space = target;
    after.background = convert_f64(&conv, doc.background);
    for (_, layer) in &mut after.pixels.layers {
        *layer = convert_layer(layer, &conv);
    }
    Some(ConvertSpace { before, after })
}

/// A colour held as floats, converted through 8 bits (the precision the canvas has anyway).
fn convert_f64(conv: &PixelConverter, rgb: [f64; 3]) -> [f64; 3] {
    let bytes = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    conv.convert(bytes).map(|v| f64::from(v) / 255.0)
}

// ------------------------------------------------------------------ display

/// The conversion from `space` to the screen's space (sRGB). `None` when no conversion is needed.
pub fn display_transform(space: ColourSpace) -> Option<PixelConverter> {
    space.display_transform()
}

/// The linear-light matrix the GPU output pass applies for `space`, or `None` for sRGB documents.
pub fn display_matrix(space: ColourSpace) -> Option<[[f32; 3]; 3]> {
    display_transform(space).map(|t| t.matrix_f32())
}

/// Maps straight RGBA8 from the document's space to the screen, in place (the CPU display path).
pub fn apply_display_rgba8(space: ColourSpace, rgba: &mut [u8]) {
    if let Some(t) = display_transform(space) {
        t.convert_straight_rgba8(rgba);
    }
}

thread_local! {
    /// The colour space the interface is painting colours for this frame. The interface is single
    /// threaded, so a thread-local saves threading the document through every swatch.
    static UI_SPACE: Cell<ColourSpace> = const { Cell::new(ColourSpace::Srgb) };
}

/// Set once per frame from the active document.
pub fn set_ui_space(space: ColourSpace) {
    UI_SPACE.with(|s| s.set(space));
}

/// The colour space of the colours the interface is showing.
pub fn ui_space() -> ColourSpace {
    UI_SPACE.with(Cell::get)
}

/// The colour to paint for a colour chosen in the document's space.
pub fn show(rgb: [u8; 3]) -> [u8; 3] {
    match ui_space().display_transform() {
        Some(t) => t.convert(rgb),
        None => rgb,
    }
}

// ------------------------------------------------------------------ self test

/// Colour management checks for `--selftest`: conversion and its undo, the display matrix, the
/// `.procreate` colour profile, ICC profiles in exports, and the GPU display against the CPU one.
pub fn selftest() -> Result<String, String> {
    use crate::session::Session;
    use eskiz_format::export::{encode_raster, ExportOptions, Raster, RasterFormat};

    // 1. Conversion with undo.
    let doc = Document::new(64, 48);
    let id = doc.layer_ids()[0];
    let px: Vec<[u8; 4]> = (0..64 * 48)
        .map(|i| match (i % 64) / 16 {
            0 => [200, 60, 40, 255],
            1 => [30, 170, 90, 255],
            2 => [90, 90, 90, 255],
            _ => [20, 60, 200, 255],
        })
        .collect();
    let mut s = Session::new(doc);
    s.replace_layer_pixels(id, px.as_flattened().to_vec(), "Bands").ok_or("color: bands refused")?;
    let original = s.doc.layer(id).ok_or("color: no layer")?.to_rgba();
    let op = plan_convert(&s.doc, ColourSpace::DisplayP3).ok_or("color: no conversion planned")?;
    s.apply_doc_op("Convert to Display P3", Box::new(op)).ok_or("color: convert refused")?;
    let p3 = s.doc.layer(id).ok_or("color: no layer")?.to_rgba();
    if s.doc.colour_space != ColourSpace::DisplayP3 || p3 == original {
        return Err("color: converting to Display P3 changed nothing".to_owned());
    }
    // A grey stays grey through the matrix.
    let grey = &p3[32 * 4..32 * 4 + 3];
    if grey.iter().any(|&c| c.abs_diff(grey[0]) > 1) {
        return Err(format!("color: gray became {grey:?}"));
    }
    s.undo().ok_or("color: convert undo unavailable")?;
    if s.doc.colour_space != ColourSpace::Srgb || s.doc.layer(id).ok_or("color: no layer")?.to_rgba() != original {
        return Err("color: undo of the conversion did not restore the document".to_owned());
    }
    s.redo().ok_or("color: convert redo unavailable")?;

    // 2. The display matrix: P3 red is outside sRGB and clips to sRGB red; sRGB needs no matrix.
    let m = display_matrix(ColourSpace::DisplayP3).ok_or("color: no display matrix for P3")?;
    if display_matrix(ColourSpace::Srgb).is_some() || m[0][0] < 1.2 || m[1][0] > 0.0 {
        return Err(format!("color: unexpected display matrix {m:?}"));
    }
    let mut red = [255, 0, 0, 255];
    apply_display_rgba8(ColourSpace::DisplayP3, &mut red);
    if red != [255, 0, 0, 255] {
        return Err(format!("color: P3 red displayed as {red:?}"));
    }

    // 3. The .procreate profile round trip, in memory.
    let bytes = crate::storage::to_builder(&s.doc, "Color").to_bytes().map_err(|e| e.to_string())?;
    let back = eskiz_format::Document::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if back.color_profile.as_deref() != Some("Display P3") {
        return Err(format!("color: stored profile is {:?}", back.color_profile));
    }

    // 4. ICC profiles in the exports.
    let raster = Raster::new(8, 8, vec![200; 8 * 8 * 4]).map_err(|e| e.to_string())?;
    let options = ExportOptions { colour_space: Some(ColourSpace::DisplayP3), ..ExportOptions::default() };
    let profile = eskiz_format::icc::profile(ColourSpace::DisplayP3);
    let has = |b: &[u8]| b.windows(profile.len()).any(|w| w == profile.as_slice());
    let jpeg = encode_raster(&raster, RasterFormat::Jpeg { quality: 90 }, &options).map_err(|e| e.to_string())?;
    let tiff = encode_raster(&raster, RasterFormat::Tiff { compression: eskiz_format::export::TiffCompression::Lzw }, &options)
        .map_err(|e| e.to_string())?;
    let png = encode_raster(&raster, RasterFormat::Png, &options).map_err(|e| e.to_string())?;
    if !has(&jpeg) || !has(&tiff) || !png.windows(4).any(|w| w == b"iCCP") {
        return Err("color: an export is missing its ICC profile".to_owned());
    }

    // 5. GPU display against the CPU display for a P3 document.
    let gpu = match eskiz_gpu::GpuContext::new() {
        Err(e) => format!("GPU display skipped ({e})"),
        Ok(ctx) => {
            let mut canvas = crate::gpu_canvas::GpuCanvas::new(ctx);
            canvas.flush(&s).map_err(|e| format!("color: GPU compositor: {e}"))?;
            let gpu = canvas.read_straight().map_err(|e| format!("color: GPU readback: {e}"))?;
            let cpu = s.flatten_display();
            let worst = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            if cpu.len() != gpu.len() || worst > 2 {
                return Err(format!("color: GPU display differs from the CPU display by up to {worst}/255"));
            }
            format!("GPU display matches CPU (worst {worst})")
        }
    };
    Ok(format!(
        "  color management: sRGB<->P3 convert + undo ok; P3 red clips to sRGB red; .procreate profile \"Display P3\"; ICC in PNG/JPEG/TIFF; {gpu}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    fn painted(space: ColourSpace) -> Session {
        let mut doc = Document::new(40, 30);
        doc.colour_space = space;
        let mut s = Session::new(doc);
        let id = s.doc.layer_ids()[0];
        let px: Vec<[u8; 4]> = (0..40 * 30)
            .map(|i| match i % 40 {
                0..=9 => [200, 40, 40, 255],
                10..=19 => [30, 160, 90, 255],
                20..=29 => [50, 60, 70, 128],
                _ => [0, 0, 0, 0],
            })
            .collect();
        s.replace_layer_pixels(id, px.as_flattened().to_vec(), "Pattern").unwrap();
        s
    }

    #[test]
    fn the_selftest_line_passes() {
        let line = selftest().expect("color selftest");
        assert!(line.contains("color management"), "{line}");
    }

    #[test]
    fn converting_to_p3_changes_pixels_background_and_space_in_one_undo_step() {
        let mut s = painted(ColourSpace::Srgb);
        s.doc.background = [0.8, 0.2, 0.2];
        let id = s.doc.layer_ids()[0];
        let before = s.doc.layer(id).unwrap().to_rgba();
        let steps_before = s.undo_steps();

        let op = plan_convert(&s.doc, ColourSpace::DisplayP3).expect("sRGB to P3");
        s.apply_doc_op("Convert to Display P3", Box::new(op)).expect("applied");
        assert_eq!(s.doc.colour_space, ColourSpace::DisplayP3);
        let after = s.doc.layer(id).unwrap().to_rgba();
        assert_ne!(after, before);
        // A saturated sRGB red is less saturated in P3: red falls, green and blue rise.
        let want = PixelConverter::new(ColourSpace::Srgb, ColourSpace::DisplayP3).convert([200, 40, 40]);
        assert_eq!(&after[..3], &want);
        assert!(s.doc.background != [0.8, 0.2, 0.2]);
        // Transparent pixels stay transparent, and alpha is untouched everywhere.
        assert_eq!(&after[30 * 4..30 * 4 + 4], &[0, 0, 0, 0]);
        assert!(before.chunks(4).zip(after.chunks(4)).all(|(a, b)| a[3] == b[3]));
        assert_eq!(s.undo_steps(), steps_before + 1, "one undo step");

        s.undo().expect("undo");
        assert_eq!(s.doc.colour_space, ColourSpace::Srgb);
        assert_eq!(s.doc.layer(id).unwrap().to_rgba(), before);
        assert_eq!(s.doc.background, [0.8, 0.2, 0.2]);
        s.redo().expect("redo");
        assert_eq!(s.doc.colour_space, ColourSpace::DisplayP3);
        assert_eq!(s.doc.layer(id).unwrap().to_rgba(), after);
    }

    #[test]
    fn converting_to_the_same_space_is_a_no_op() {
        let s = painted(ColourSpace::DisplayP3);
        assert!(plan_convert(&s.doc, ColourSpace::DisplayP3).is_none());
    }

    #[test]
    fn display_transform_exists_only_for_p3() {
        assert!(display_matrix(ColourSpace::Srgb).is_none());
        let m = display_matrix(ColourSpace::DisplayP3).expect("P3 matrix");
        // Rows of a linear P3 -> sRGB matrix sum to 1: white maps to white.
        for row in m {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-4, "{row:?}");
        }
        let mut px = [255u8, 0, 0, 255, 10, 20, 30, 128];
        apply_display_rgba8(ColourSpace::DisplayP3, &mut px);
        assert_eq!(&px[..4], &[255, 0, 0, 255], "P3 red clips to sRGB red");
        assert_eq!(px[7], 128, "alpha is untouched");
        let mut same = [255u8, 0, 0, 255];
        apply_display_rgba8(ColourSpace::Srgb, &mut same);
        assert_eq!(same, [255, 0, 0, 255]);
    }

    #[test]
    fn ui_colours_follow_the_document_space() {
        set_ui_space(ColourSpace::Srgb);
        assert_eq!(show([10, 200, 30]), [10, 200, 30]);
        set_ui_space(ColourSpace::DisplayP3);
        let shown = show([10, 200, 30]);
        assert_ne!(shown, [10, 200, 30]);
        // A saturated P3 green is outside sRGB: red clips to 0.
        assert_eq!(shown[0], 0);
        set_ui_space(ColourSpace::Srgb);
    }

    // ---------------------------------------------------------------- .procreate and exports

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("eskiz-colour-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_doc(space: ColourSpace) -> Document {
        let mut s = painted(space);
        s.doc.background = [0.9, 0.9, 0.8];
        s.doc
    }

    #[test]
    fn procreate_round_trip_keeps_the_colour_space() {
        let dir = scratch("roundtrip");
        for space in ColourSpace::ALL {
            let doc = sample_doc(space);
            let path = dir.join(format!("{}.procreate", space.label().replace(' ', "-")));
            crate::storage::write_atomic(&path, &crate::storage::to_builder(&doc, "Color").to_bytes().unwrap()).unwrap();
            // The package stores the ICC profile name under the format's colour-profile key.
            let raw = eskiz_format::Document::open(&path).unwrap();
            assert_eq!(raw.color_profile.as_deref(), Some(space.profile_name()));
            let back = crate::import::open_procreate(&path).unwrap();
            assert_eq!(back.colour_space, space);
            assert_eq!(back.layer_at(0).unwrap().to_rgba(), doc.layer_at(0).unwrap().to_rgba());
        }
        // A package with no colour profile at all (older files) opens as sRGB.
        let mut plain = eskiz_format::DocumentBuilder::new(4, 4);
        plain.color_profile = None;
        let path = dir.join("plain.procreate");
        eskiz_format::document::write(&path, &plain).unwrap();
        assert_eq!(crate::import::open_procreate(&path).unwrap().colour_space, ColourSpace::Srgb);
        // The profile is part of what "unsaved changes" means.
        let a = sample_doc(ColourSpace::Srgb);
        let mut b = a.clone();
        b.colour_space = ColourSpace::DisplayP3;
        assert_ne!(crate::storage::meta_signature(&a), crate::storage::meta_signature(&b));
        std::fs::remove_dir_all(&dir).ok();
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn exports_carry_the_documents_icc_profile() {
        use crate::export::{export_document, ExportFormat};
        let dir = scratch("exports");
        for space in ColourSpace::ALL {
            let doc = sample_doc(space);
            let profile = eskiz_format::icc::profile(space);
            let tag = space.label().replace(' ', "-");
            let write = |format: ExportFormat| {
                let path = export_document(&doc, format, &dir.join(format!("{tag}-{}", format.extension()))).unwrap();
                std::fs::read(path).unwrap()
            };
            // PNG: an iCCP chunk whose decoded profile is ours.
            let png_bytes = write(ExportFormat::Png);
            let reader = png::Decoder::new(std::io::Cursor::new(&png_bytes)).read_info().unwrap();
            assert_eq!(reader.info().icc_profile.as_deref(), Some(profile.as_slice()), "{space:?} PNG");
            // JPEG, TIFF and PSD store the profile bytes as they are.
            for format in [ExportFormat::Jpeg, ExportFormat::Tiff, ExportFormat::Psd] {
                assert!(contains(&write(format), &profile), "{space:?} {format:?}: profile bytes are in the file");
            }
            // PSD: in the image resource block 0x040F, and it reads back.
            let psd_bytes = write(ExportFormat::Psd);
            assert!(contains(&psd_bytes, b"8BIM\x04\x0F"));
            assert_eq!(eskiz_format::psd::from_bytes(&psd_bytes).unwrap().colour_space, Some(space));
            // PDF and WebP: the colour space and chunk names.
            assert!(contains(&write(ExportFormat::Pdf), b"ICCBased"));
            assert!(contains(&write(ExportFormat::WebP), b"ICCP"));
            // The layered package stores the profile name.
            let pkg = dir.join(format!("{tag}-pkg.procreate"));
            crate::storage::write_atomic(&pkg, &write(ExportFormat::Procreate)).unwrap();
            assert_eq!(crate::import::open_procreate(&pkg).unwrap().colour_space, space);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------------------------------------------------------------- screens

    #[test]
    fn canvas_information_and_the_colour_panel_name_the_space() {
        use crate::app::Prefs;
        use crate::popover::Panel;
        use crate::snapshot::{snapshot_dir, Headless};
        use eframe::egui::Vec2;

        let mut doc = Document::new(1600, 1000);
        doc.colour_space = ColourSpace::DisplayP3;
        let mut h = Headless::new(Vec2::new(1280.0, 860.0), doc, Prefs::default());
        h.app.colour = [214, 92, 64];
        h.app.canvas_info_open = true;
        h.advance(0.2);
        assert_eq!(ui_space(), ColourSpace::DisplayP3, "the frame tells the interface the document's space");
        h.render().save_png(&snapshot_dir().join("canvas_info_p3.png")).unwrap();
        h.app.canvas_info_open = false;
        h.open_panel(Panel::Colour);
        h.app.colour_ui.tab = crate::panels::colour::ColourTab::Square;
        h.advance(0.2);
        h.render().save_png(&snapshot_dir().join("colour_classic_p3.png")).unwrap();
        // The chosen colour is a document-space value: showing it does not change it.
        assert_eq!(h.app.colour, [214, 92, 64]);
        set_ui_space(ColourSpace::Srgb);
    }

    #[test]
    fn actions_canvas_converts_the_document_and_undo_reverts_it() {
        use crate::app::Prefs;
        use crate::panels::actions::ActionsTab;
        use crate::popover::Panel;
        use crate::snapshot::{snapshot_dir, Headless};
        use eframe::egui::Vec2;

        let doc = sample_doc(ColourSpace::Srgb);
        let id = doc.layer_id(0).unwrap();
        let before = doc.layer(id).unwrap().to_rgba();
        let mut h = Headless::new(Vec2::new(1280.0, 860.0), doc, Prefs::default());
        h.app.actions_ui.tab = ActionsTab::Canvas;
        h.open_panel(Panel::Actions);
        h.render().save_png(&snapshot_dir().join("actions_canvas_srgb_convert.png")).unwrap();

        h.app.convert_colour_space_action(ColourSpace::DisplayP3);
        h.advance(0.2);
        assert_eq!(h.app.session.doc.colour_space, ColourSpace::DisplayP3);
        assert_eq!(h.app.session.undo_name(), Some("Convert to Display P3"));
        let converted = h.app.session.doc.layer(id).unwrap().to_rgba();
        assert_ne!(converted, before);
        h.render().save_png(&snapshot_dir().join("actions_canvas_p3_convert.png")).unwrap();

        // Converting to the space it is already in does nothing and adds no undo step.
        let steps = h.app.session.undo_steps();
        h.app.convert_colour_space_action(ColourSpace::DisplayP3);
        assert_eq!(h.app.session.undo_steps(), steps);

        h.app.session.undo().expect("undo");
        assert_eq!(h.app.session.doc.colour_space, ColourSpace::Srgb);
        assert_eq!(h.app.session.doc.layer(id).unwrap().to_rgba(), before);
        set_ui_space(ColourSpace::Srgb);
    }
}
