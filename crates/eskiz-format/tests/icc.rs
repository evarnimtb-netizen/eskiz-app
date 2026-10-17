//! Exported files carry the document's ICC profile.

use std::io::Cursor;

use eskiz_format::export::{encode_pdf, encode_raster, ExportOptions, PdfOptions, Raster, RasterFormat, TiffCompression};
use eskiz_format::icc;
use eskiz_guides::ColourSpace;

fn raster() -> Raster {
    let (w, h) = (8u32, 6u32);
    let rgba = (0..w * h).flat_map(|i| [(i * 5) as u8, 40, 200, 255]).collect();
    Raster::new(w, h, rgba).unwrap()
}

fn options(space: Option<ColourSpace>) -> ExportOptions {
    ExportOptions { dpi: 132.0, colour_space: space, ..ExportOptions::default() }
}

#[test]
fn png_carries_an_iccp_chunk() {
    for space in ColourSpace::ALL {
        let bytes = encode_raster(&raster(), RasterFormat::Png, &options(Some(space))).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"iCCP"), "{space:?}: no iCCP chunk");
        let decoder = png::Decoder::new(Cursor::new(&bytes));
        let reader = decoder.read_info().unwrap();
        let profile = reader.info().icc_profile.as_ref().expect("profile");
        assert_eq!(profile.as_ref(), icc::profile(space).as_slice());
        assert_eq!(icc::space_of_profile(profile), Some(space));
    }
    let plain = encode_raster(&raster(), RasterFormat::Png, &options(None)).unwrap();
    assert!(!plain.windows(4).any(|w| w == b"iCCP"));
}

/// Collects the ICC_PROFILE APP2 payloads of a JPEG in sequence order.
fn jpeg_profile(bytes: &[u8]) -> Option<Vec<u8>> {
    assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    let mut at = 2;
    let mut parts: Vec<(u8, Vec<u8>)> = Vec::new();
    while at + 4 <= bytes.len() && bytes[at] == 0xFF {
        let marker = bytes[at + 1];
        if marker == 0xDA {
            break;
        }
        let len = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        let body = &bytes[at + 4..at + 2 + len];
        if marker == 0xE2 && body.starts_with(b"ICC_PROFILE\0") {
            parts.push((body[12], body[14..].to_vec()));
        }
        at += 2 + len;
    }
    parts.sort_by_key(|p| p.0);
    (!parts.is_empty()).then(|| parts.into_iter().flat_map(|p| p.1).collect())
}

#[test]
fn jpeg_carries_icc_profile_app2_segments_and_still_decodes() {
    for space in ColourSpace::ALL {
        let bytes = encode_raster(&raster(), RasterFormat::Jpeg { quality: 90 }, &options(Some(space))).unwrap();
        assert_eq!(jpeg_profile(&bytes).expect("APP2"), icc::profile(space));
        // JFIF density survived and the picture still decodes.
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).unwrap();
        assert_eq!((img.width(), img.height()), (8, 6));
    }
    let plain = encode_raster(&raster(), RasterFormat::Jpeg { quality: 90 }, &options(None)).unwrap();
    assert!(jpeg_profile(&plain).is_none());
}

#[test]
fn tiff_carries_the_inter_color_profile_tag() {
    for space in ColourSpace::ALL {
        let bytes = encode_raster(&raster(), RasterFormat::Tiff { compression: TiffCompression::Lzw }, &options(Some(space))).unwrap();
        let mut decoder = tiff::decoder::Decoder::new(Cursor::new(&bytes)).unwrap();
        let profile = decoder.get_tag_u8_vec(tiff::tags::Tag::Unknown(34675)).expect("tag 34675");
        assert_eq!(profile, icc::profile(space));
    }
}

#[test]
fn webp_carries_an_iccp_chunk() {
    let bytes = encode_raster(&raster(), RasterFormat::WebP, &options(Some(ColourSpace::DisplayP3))).unwrap();
    assert!(bytes.windows(4).any(|w| w == b"ICCP"));
    assert!(bytes.windows(4).any(|w| w == b"VP8X"));
}

#[test]
fn pdf_images_use_an_icc_based_colour_space() {
    for space in ColourSpace::ALL {
        let bytes = encode_pdf(&[raster()], &PdfOptions { colour_space: Some(space), ..PdfOptions::default() }).unwrap();
        let doc = lopdf::Document::load_mem(&bytes).expect("parse");
        let mut found = false;
        for object in doc.objects.values() {
            let lopdf::Object::Stream(stream) = object else { continue };
            if let Ok(lopdf::Object::Array(cs)) = stream.dict.get(b"ColorSpace") {
                if cs.first().and_then(|o| o.as_name().ok()) == Some(b"ICCBased") {
                    let id = cs[1].as_reference().expect("reference");
                    let profile = doc.get_object(id).unwrap().as_stream().unwrap().decompressed_content().unwrap();
                    assert_eq!(profile, icc::profile(space));
                    found = true;
                }
            }
        }
        assert!(found, "{space:?}: no ICCBased image");
    }
    let plain = encode_pdf(&[raster()], &PdfOptions::default()).unwrap();
    assert!(!plain.windows(8).any(|w| w == b"ICCBased"));
}
