//! Tests for `eskiz_format::export`: raster and PDF export, per-layer naming and atomic writes.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use eskiz_format::export::{
    encode_pdf, encode_raster, export_layers_pdf, export_layers_png, export_pdf, export_raster,
    layer_file_name, sanitise_name, ExportOptions, LayerRaster, PdfOptions, PdfQuality, Raster,
    RasterFormat, TiffCompression,
};
use eskiz_format::Error;

/// A scratch folder that is removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("eskiz-export-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn premultiply(straight: [u8; 4]) -> [u8; 4] {
    let a = straight[3];
    let mul = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
    [mul(straight[0]), mul(straight[1]), mul(straight[2]), a]
}

/// A mixed pattern: opaque, half-transparent and fully transparent pixels. Returns the premultiplied
/// raster and the straight-alpha pixels it came from.
fn pattern(w: u32, h: u32) -> (Raster, Vec<[u8; 4]>) {
    let mut rgba = Vec::new();
    let mut straight_pixels = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let a = match (x + y) % 3 {
                0 => 0,
                _ if (x * y) % 2 == 0 => 255,
                _ => 128,
            };
            let straight = [
                (x * 60 % 256) as u8,
                (y * 80 % 256) as u8,
                ((x + y) * 30 % 256) as u8,
                a as u8,
            ];
            rgba.extend(premultiply(straight));
            straight_pixels.push(straight);
        }
    }
    (Raster::new(w, h, rgba).unwrap(), straight_pixels)
}

/// A smooth opaque-to-transparent gradient, suited to lossy JPEG tests.
fn smooth(w: u32, h: u32) -> Raster {
    let mut rgba = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let a = (x * 255 / (w - 1)) as u8;
            let straight = [(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, 90, a];
            rgba.extend(premultiply(straight));
        }
    }
    Raster::new(w, h, rgba).unwrap()
}

/// Flattened colour of a premultiplied pixel over an opaque background channel.
fn flatten(premul: [u8; 4], bg: [u8; 3]) -> [u8; 3] {
    let a = u32::from(premul[3]);
    let mut out = [0u8; 3];
    for c in 0..3 {
        let behind = (u32::from(bg[c]) * (255 - a) + 127) / 255;
        out[c] = (u32::from(premul[c]) + behind).min(255) as u8;
    }
    out
}

fn decode_png(
    bytes: &[u8],
) -> (
    u32,
    u32,
    png::ColorType,
    Vec<u8>,
    Option<png::PixelDimensions>,
) {
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    let info = reader.info();
    let (width, height, color, dims) = (info.width, info.height, info.color_type, info.pixel_dims);
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buf).unwrap();
    (width, height, color, buf, dims)
}

fn assert_straight_close(decoded: &[u8], straight: &[[u8; 4]], tolerance: i32) {
    assert_eq!(decoded.len(), straight.len() * 4);
    for (i, expected) in straight.iter().enumerate() {
        let got = &decoded[i * 4..i * 4 + 4];
        assert_eq!(got[3], expected[3], "alpha differs at pixel {i}");
        if expected[3] == 0 {
            continue;
        }
        for c in 0..3 {
            let diff = (i32::from(got[c]) - i32::from(expected[c])).abs();
            assert!(
                diff <= tolerance,
                "pixel {i} channel {c}: got {}, want {}",
                got[c],
                expected[c]
            );
        }
    }
}

fn opts(dpi: f64) -> ExportOptions {
    ExportOptions {
        dpi,
        ..ExportOptions::default()
    }
}

fn decode_pixels(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let img = image::load_from_memory(bytes).unwrap().to_rgba8();
    (img.width(), img.height(), img.into_raw())
}

// ---- PNG -------------------------------------------------------------------------------------

#[test]
fn png_round_trip_keeps_straight_alpha() {
    let (raster, straight) = pattern(7, 5);
    let bytes = encode_raster(&raster, RasterFormat::Png, &opts(72.0)).unwrap();
    let (w, h, color, pixels, _) = decode_png(&bytes);
    assert_eq!((w, h), (7, 5));
    assert_eq!(color, png::ColorType::Rgba);
    assert_straight_close(&pixels, &straight, 2);
}

#[test]
fn png_without_transparency_is_rgb_flattened_over_background() {
    let (raster, straight) = pattern(7, 5);
    let options = ExportOptions {
        transparent: false,
        background: [20, 30, 40],
        ..opts(72.0)
    };
    let bytes = encode_raster(&raster, RasterFormat::Png, &options).unwrap();
    let (_, _, color, pixels, _) = decode_png(&bytes);
    assert_eq!(color, png::ColorType::Rgb);
    for (i, px) in straight.iter().enumerate() {
        let premul = premultiply(*px);
        let want = flatten(premul, [20, 30, 40]);
        for c in 0..3 {
            let diff = (i32::from(pixels[i * 3 + c]) - i32::from(want[c])).abs();
            assert!(diff <= 2, "pixel {i} channel {c}");
        }
    }
}

#[test]
fn png_dpi_is_in_phys_chunk() {
    let (raster, _) = pattern(4, 4);
    let bytes = encode_raster(&raster, RasterFormat::Png, &opts(300.0)).unwrap();
    let (_, _, _, _, dims) = decode_png(&bytes);
    let dims = dims.expect("pHYs chunk missing");
    assert_eq!(dims.unit, png::Unit::Meter);
    // 300 DPI is 11811 pixels per metre.
    assert_eq!(dims.xppu, 11811);
    assert_eq!(dims.yppu, 11811);
}

#[test]
fn scale_changes_output_size() {
    let (raster, _) = pattern(8, 6);
    let options = ExportOptions {
        scale: 0.5,
        ..opts(72.0)
    };
    let bytes = encode_raster(&raster, RasterFormat::Png, &options).unwrap();
    let (w, h, _, _, _) = decode_png(&bytes);
    assert_eq!((w, h), (4, 3));
}

// ---- TIFF ------------------------------------------------------------------------------------

fn decode_tiff(bytes: &[u8]) -> (u32, u32, Vec<u8>, tiff::decoder::Decoder<Cursor<&[u8]>>) {
    let mut decoder = tiff::decoder::Decoder::new(Cursor::new(bytes)).unwrap();
    let (w, h) = decoder.dimensions().unwrap();
    let pixels = match decoder.read_image().unwrap() {
        tiff::decoder::DecodingResult::U8(v) => v,
        other => panic!("expected 8-bit samples, got {other:?}"),
    };
    (w, h, pixels, decoder)
}

#[test]
fn tiff_round_trip_for_both_compressions() {
    let (raster, straight) = pattern(9, 4);
    for compression in [TiffCompression::Uncompressed, TiffCompression::Lzw] {
        let format = RasterFormat::Tiff { compression };
        let bytes = encode_raster(&raster, format, &opts(300.0)).unwrap();
        let (w, h, pixels, _) = decode_tiff(&bytes);
        assert_eq!((w, h), (9, 4));
        assert_straight_close(&pixels, &straight, 2);
    }
}

#[test]
fn tiff_carries_resolution_and_document_name() {
    let (raster, _) = pattern(3, 3);
    let options = ExportOptions {
        document_name: Some("Sketch".into()),
        ..opts(300.0)
    };
    let bytes = encode_raster(
        &raster,
        RasterFormat::Tiff {
            compression: TiffCompression::Lzw,
        },
        &options,
    )
    .unwrap();
    let (_, _, _, mut decoder) = decode_tiff(&bytes);
    let x = decoder.get_tag(tiff::tags::Tag::XResolution).unwrap();
    let y = decoder.get_tag(tiff::tags::Tag::YResolution).unwrap();
    assert_eq!(rational(&x), 300.0);
    assert_eq!(rational(&y), 300.0);
    assert_eq!(
        decoder
            .get_tag_u32(tiff::tags::Tag::ResolutionUnit)
            .unwrap(),
        2,
        "unit is inch"
    );
    assert_eq!(
        decoder
            .get_tag_ascii_string(tiff::tags::Tag::Unknown(269))
            .unwrap(),
        "Sketch"
    );
}

fn rational(value: &tiff::decoder::ifd::Value) -> f64 {
    match value {
        tiff::decoder::ifd::Value::Rational(n, d) => f64::from(*n) / f64::from(*d),
        other => panic!("expected a rational, got {other:?}"),
    }
}

#[test]
fn tiff_without_transparency_is_rgb() {
    let (raster, _) = pattern(3, 2);
    let options = ExportOptions {
        transparent: false,
        ..opts(72.0)
    };
    let bytes = encode_raster(
        &raster,
        RasterFormat::Tiff {
            compression: TiffCompression::Uncompressed,
        },
        &options,
    )
    .unwrap();
    let (_, _, pixels, _) = decode_tiff(&bytes);
    assert_eq!(pixels.len(), 3 * 2 * 3);
}

// ---- WebP ------------------------------------------------------------------------------------

#[test]
fn webp_lossless_round_trip() {
    let (raster, straight) = pattern(10, 6);
    let bytes = encode_raster(&raster, RasterFormat::WebP, &opts(72.0)).unwrap();
    let (w, h, pixels) = decode_pixels(&bytes);
    assert_eq!((w, h), (10, 6));
    // The WebP encoder may move partly transparent colours by one step.
    assert_straight_close(&pixels, &straight, 1);
}

// ---- JPEG ------------------------------------------------------------------------------------

#[test]
fn jpeg_round_trip_within_tolerance() {
    let raster = smooth(48, 32);
    let bytes = encode_raster(
        &raster,
        RasterFormat::Jpeg { quality: 95 },
        &ExportOptions {
            background: [0, 0, 0],
            ..opts(72.0)
        },
    )
    .unwrap();
    let (w, h, pixels) = decode_pixels(&bytes);
    assert_eq!((w, h), (48, 32));
    let mut worst = 0;
    let mut total = 0i64;
    for (i, px) in raster.rgba.as_chunks::<4>().0.iter().enumerate() {
        // The background is black, so JPEG sees the premultiplied colour over black.
        let want = flatten([px[0], px[1], px[2], px[3]], [0, 0, 0]);
        for c in 0..3 {
            let diff = (i32::from(pixels[i * 4 + c]) - i32::from(want[c])).abs();
            worst = worst.max(diff);
            total += i64::from(diff);
        }
    }
    let mean = total as f64 / (48.0 * 32.0 * 3.0);
    assert!(mean < 3.0, "mean error {mean}");
    assert!(worst < 16, "worst error {worst}");
}

#[test]
fn jpeg_flattens_transparency_onto_background() {
    let (raster, _) = pattern(12, 9);
    let bg = [10, 200, 30];
    let options = ExportOptions {
        background: bg,
        ..opts(72.0)
    };
    let bytes = encode_raster(&raster, RasterFormat::Jpeg { quality: 100 }, &options).unwrap();
    let (_, _, pixels) = decode_pixels(&bytes);
    for (i, px) in raster.rgba.as_chunks::<4>().0.iter().enumerate() {
        let want = flatten([px[0], px[1], px[2], px[3]], bg);
        for c in 0..3 {
            let diff = (i32::from(pixels[i * 4 + c]) - i32::from(want[c])).abs();
            assert!(
                diff <= 12,
                "pixel {i} channel {c}: got {}, want {}",
                pixels[i * 4 + c],
                want[c]
            );
        }
    }
}

#[test]
fn jpeg_fully_transparent_image_is_background_colour() {
    let raster = Raster::new(8, 8, vec![0; 8 * 8 * 4]).unwrap();
    let bg = [200, 120, 40];
    let options = ExportOptions {
        background: bg,
        ..opts(72.0)
    };
    let bytes = encode_raster(&raster, RasterFormat::Jpeg { quality: 100 }, &options).unwrap();
    let (_, _, pixels) = decode_pixels(&bytes);
    for px in pixels.as_chunks::<4>().0 {
        for c in 0..3 {
            assert!(
                (i32::from(px[c]) - i32::from(bg[c])).abs() <= 6,
                "got {px:?}"
            );
        }
    }
}

#[test]
fn jpeg_dpi_is_in_jfif_density_fields() {
    let raster = smooth(16, 16);
    let bytes = encode_raster(&raster, RasterFormat::Jpeg { quality: 90 }, &opts(300.0)).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "SOI");
    let at = bytes
        .windows(5)
        .position(|w| w == b"JFIF\0")
        .expect("JFIF header");
    assert_eq!(bytes[at + 7], 1, "density units are dots per inch");
    assert_eq!(
        u16::from_be_bytes([bytes[at + 8], bytes[at + 9]]),
        300,
        "X density"
    );
    assert_eq!(
        u16::from_be_bytes([bytes[at + 10], bytes[at + 11]]),
        300,
        "Y density"
    );
}

// ---- layer names -----------------------------------------------------------------------------

#[test]
fn sanitising_removes_path_separators_and_control_characters() {
    assert_eq!(sanitise_name("Sketch/Ink: \u{1}2?"), "SketchInk 2");
    assert_eq!(sanitise_name("../../etc"), "etc");
    assert_eq!(sanitise_name("   "), "Layer");
    assert_eq!(sanitise_name(""), "Layer");
    assert_eq!(sanitise_name("a\\b"), "ab");
    assert_eq!(sanitise_name("Keep  spaces").as_str(), "Keep  spaces");
    assert_eq!(sanitise_name(&"x".repeat(300)).chars().count(), 100);
}

#[test]
fn layer_file_names_are_numbered_from_the_bottom() {
    assert_eq!(layer_file_name(0, 3, "Background"), "01 Background.png");
    assert_eq!(layer_file_name(1, 12, "Ink"), "02 Ink.png");
    assert_eq!(layer_file_name(11, 12, "Top"), "12 Top.png");
    // Padding grows with the layer count.
    assert_eq!(layer_file_name(0, 120, "a"), "001 a.png");
}

fn layers(bottom_visible: bool) -> Vec<LayerRaster> {
    let mut canvas = vec![0u8; 6 * 4 * 4];
    // A 2x2 painted square in the middle, so most of each layer is transparent.
    for y in 1..3 {
        for x in 2..4 {
            let i = (y * 6 + x) * 4;
            canvas[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
    }
    let raster = Raster::new(6, 4, canvas).unwrap();
    vec![
        LayerRaster {
            name: "Background".into(),
            visible: bottom_visible,
            raster: raster.clone(),
        },
        LayerRaster {
            name: "Hidden".into(),
            visible: false,
            raster: raster.clone(),
        },
        LayerRaster {
            name: "Sketch/Ink: \u{1}2?".into(),
            visible: true,
            raster,
        },
    ]
}

#[test]
fn per_layer_png_names_hidden_and_sanitised() {
    let dir = Scratch::new("layers");
    let out = dir.path().join("out");
    let written = export_layers_png(&out, &layers(true), &opts(72.0), false).unwrap();
    let names: Vec<String> = written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["01 Background.png", "03 SketchInk 2.png"]);
    for path in &written {
        assert_eq!(path.parent().unwrap(), out, "file stays inside the folder");
    }

    let with_hidden =
        export_layers_png(dir.path().join("all"), &layers(true), &opts(72.0), true).unwrap();
    assert_eq!(with_hidden.len(), 3);
    assert!(with_hidden[1].ends_with("02 Hidden.png"));
}

#[test]
fn per_layer_png_is_canvas_size_with_transparent_surroundings() {
    let dir = Scratch::new("surround");
    let written = export_layers_png(dir.path(), &layers(true), &opts(72.0), false).unwrap();
    let bytes = fs::read(&written[0]).unwrap();
    let (w, h, color, pixels, _) = decode_png(&bytes);
    assert_eq!((w, h), (6, 4));
    assert_eq!(color, png::ColorType::Rgba);
    assert_eq!(pixels[3], 0, "top-left corner is transparent");
    let centre = (6 + 2) * 4;
    assert_eq!(&pixels[centre..centre + 4], &[255, 0, 0, 255]);
}

#[test]
fn per_layer_png_collision_gets_numeric_suffix() {
    let dir = Scratch::new("collide");
    fs::write(dir.path().join("01 Background.png"), b"old").unwrap();
    let written = export_layers_png(dir.path(), &layers(true), &opts(72.0), false).unwrap();
    assert!(
        written[0].ends_with("01 Background 2.png"),
        "got {:?}",
        written[0]
    );
    assert_eq!(
        fs::read(dir.path().join("01 Background.png")).unwrap(),
        b"old"
    );
}

// ---- PDF -------------------------------------------------------------------------------------

fn pdf_page_media_boxes(bytes: &[u8]) -> Vec<Vec<f32>> {
    let doc = lopdf::Document::load_mem(bytes).unwrap();
    doc.get_pages()
        .values()
        .map(|id| {
            let page = doc.get_object(*id).unwrap().as_dict().unwrap();
            page.get(b"MediaBox")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_float().unwrap())
                .collect()
        })
        .collect()
}

#[test]
fn pdf_page_count_and_size_from_dpi() {
    let page = Raster::new(2048, 1536, vec![0; 2048 * 1536 * 4]).unwrap();
    let options = PdfOptions {
        dpi: 300.0,
        quality: PdfQuality::High,
        colour_space: None,
    };
    let bytes = encode_pdf(&[page.clone(), page], &options).unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    let boxes = pdf_page_media_boxes(&bytes);
    assert_eq!(boxes.len(), 2);
    for media in &boxes {
        assert_eq!(media.len(), 4);
        assert!((media[2] - 491.52).abs() < 0.01, "width {}", media[2]);
        assert!((media[3] - 368.64).abs() < 0.01, "height {}", media[3]);
    }
}

#[test]
fn pdf_embeds_rgb_with_alpha_smask_and_flate() {
    let (raster, _) = pattern(16, 8);
    let bytes = encode_pdf(&[raster], &PdfOptions::default()).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/SMask"), "alpha is an SMask");
    assert!(text.contains("/FlateDecode"));
    assert!(text.contains("/DeviceRGB"));
    assert!(text.contains("/DeviceGray"));
}

#[test]
fn pdf_quality_sets_image_pixels_not_page_size() {
    let raster = smooth(64, 32);
    let low = encode_pdf(
        std::slice::from_ref(&raster),
        &PdfOptions {
            dpi: 72.0,
            quality: PdfQuality::Low,
            colour_space: None,
        },
    )
    .unwrap();
    let text = String::from_utf8_lossy(&low);
    assert!(text.contains("/Width 32"), "half-size image");
    let boxes = pdf_page_media_boxes(&low);
    assert!(
        (boxes[0][2] - 64.0).abs() < 0.01,
        "page size is still the canvas size"
    );
}

#[test]
fn pdf_rejects_empty_and_mismatched_pages() {
    let a = Raster::new(4, 4, vec![0; 64]).unwrap();
    let b = Raster::new(5, 4, vec![0; 80]).unwrap();
    assert!(matches!(
        encode_pdf(&[], &PdfOptions::default()),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        encode_pdf(&[a, b], &PdfOptions::default()),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn per_layer_pdf_has_one_page_per_visible_layer() {
    let dir = Scratch::new("pdf-layers");
    let path = dir.path().join("layers.pdf");
    export_layers_pdf(&path, &layers(true), &PdfOptions::default(), false).unwrap();
    assert_eq!(pdf_page_media_boxes(&fs::read(&path).unwrap()).len(), 2);

    export_layers_pdf(&path, &layers(true), &PdfOptions::default(), true).unwrap();
    assert_eq!(pdf_page_media_boxes(&fs::read(&path).unwrap()).len(), 3);

    let none_visible = layers(false)
        .into_iter()
        .map(|l| LayerRaster {
            visible: false,
            ..l
        })
        .collect::<Vec<_>>();
    let err = export_layers_pdf(&path, &none_visible, &PdfOptions::default(), false);
    assert!(matches!(err, Err(Error::InvalidInput(_))));
}

#[test]
fn flattened_pdf_is_one_page_on_disk() {
    let dir = Scratch::new("pdf-flat");
    let path = dir.path().join("flat.pdf");
    let (raster, _) = pattern(10, 10);
    export_pdf(&path, &[raster], &PdfOptions::default()).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    assert_eq!(pdf_page_media_boxes(&bytes).len(), 1);
}

// ---- atomic writes ---------------------------------------------------------------------------

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn export_leaves_no_temporary_files_and_replaces_targets() {
    let dir = Scratch::new("atomic");
    let target = dir.path().join("out.png");
    fs::write(&target, b"stale").unwrap();
    let (raster, _) = pattern(4, 4);
    export_raster(&target, &raster, RasterFormat::Png, &opts(72.0)).unwrap();
    assert_eq!(entries(dir.path()), ["out.png"]);
    let (_, _, _, _, _) = decode_png(&fs::read(&target).unwrap());

    export_pdf(
        dir.path().join("out.pdf"),
        &[raster],
        &PdfOptions::default(),
    )
    .unwrap();
    assert_eq!(entries(dir.path()), ["out.pdf", "out.png"]);
}

#[test]
fn failed_rename_removes_temporary_file() {
    let dir = Scratch::new("failed-rename");
    // A directory where the target should go: the rename fails, and the temp file must go too.
    let target = dir.path().join("blocked.png");
    fs::create_dir(&target).unwrap();
    let (raster, _) = pattern(4, 4);
    let result = export_raster(&target, &raster, RasterFormat::Png, &opts(72.0));
    assert!(result.is_err());
    assert_eq!(entries(dir.path()), ["blocked.png"]);
}

#[test]
fn failed_write_to_missing_folder_creates_nothing() {
    let dir = Scratch::new("missing-folder");
    let target = dir.path().join("missing").join("out.png");
    let (raster, _) = pattern(4, 4);
    assert!(export_raster(&target, &raster, RasterFormat::Png, &opts(72.0)).is_err());
    assert_eq!(entries(dir.path()), Vec::<String>::new());
}

#[test]
fn invalid_options_are_rejected() {
    let (raster, _) = pattern(4, 4);
    let png = RasterFormat::Png;
    assert!(matches!(
        encode_raster(&raster, png, &opts(0.0)),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        encode_raster(&raster, png, &opts(f64::NAN)),
        Err(Error::InvalidInput(_))
    ));
    let zero_scale = ExportOptions {
        scale: 0.0,
        ..opts(72.0)
    };
    assert!(matches!(
        encode_raster(&raster, png, &zero_scale),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        Raster::new(4, 4, vec![0; 10]),
        Err(Error::InvalidInput(_))
    ));
}
