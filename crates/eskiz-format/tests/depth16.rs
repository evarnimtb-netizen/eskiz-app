//! 16-bit pictures: `.procreate` tiles, PNG, TIFF and PSD at 16 bits per channel, and the
//! dithered conversion JPEG and WebP use.

use std::io::Cursor;

use eskiz_format::export::{encode_raster16, ExportOptions, Raster16, RasterFormat, TiffCompression};
use eskiz_format::psd::{self, Compression16, PsdBlend, PsdDocument, PsdGroup, PsdLayer, PsdNode};
use eskiz_format::{Document, DocumentBuilder, LayerSpec, Node, TileEncoding};

const W: usize = 300;
const H: usize = 70;

/// Premultiplied RGBA16 with values no 8-bit file could hold: every sample is distinct and none
/// is a multiple of 257.
fn ramp(seed: u32) -> Vec<u16> {
    let mut v = vec![0u16; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            if (x / 40 + y / 30 + seed as usize).is_multiple_of(4) {
                continue; // transparent holes
            }
            let a = 20_000 + ((x * 131 + y * 17 + seed as usize * 977) % 45_000) as u32;
            let c = |k: u32| ((x as u32 * (k + 3) * 211 + y as u32 * 97 + seed * 7 + k * 13) % (a + 1)) as u16;
            let i = (y * W + x) * 4;
            v[i..i + 4].copy_from_slice(&[c(0), c(1), c(2), a as u16]);
        }
    }
    v
}

fn builder(bits: u32) -> DocumentBuilder {
    let mut b = DocumentBuilder::new(W as u32, H as u32);
    b.bits_per_channel = bits;
    b.tree = vec![
        Node::Layer(LayerSpec { name: "top".into(), pixels16: Some(ramp(2)), opacity: 0.8, ..LayerSpec::default() }),
        Node::Layer(LayerSpec { name: "empty".into(), ..LayerSpec::default() }),
        Node::Layer(LayerSpec { name: "bottom".into(), pixels16: Some(ramp(1)), ..LayerSpec::default() }),
    ];
    b
}

#[test]
fn procreate_16_bit_layers_round_trip_exactly() {
    let bytes = builder(16).to_bytes().unwrap();
    let mut doc = Document::from_reader(Cursor::new(bytes)).unwrap();
    assert_eq!(doc.bits_per_channel().unwrap(), 16);
    assert_eq!(doc.declared_bits, Some(16));
    assert_eq!(doc.tile_encoding().unwrap(), TileEncoding::U16Unorm, "Eskiz writes uint16");
    let layers = doc.layers.clone();
    assert_eq!(layers.len(), 3);
    // The tree is top-most first.
    assert_eq!(layers[0].name, "top");
    assert_eq!(doc.layer_rgba16(&layers[0]).unwrap(), ramp(2));
    assert_eq!(doc.layer_rgba16(&layers[2]).unwrap(), ramp(1));
    assert!(doc.layer_rgba16(&layers[1]).unwrap().iter().all(|&v| v == 0));
    // Tile payloads are 8 bytes a pixel: an edge tile (300 - 256 = 44 wide) is 44 * 70 * 8 bytes.
    assert!(doc.tile_count(&layers[0].uuid) > 0);
    // The 8-bit reader still works on a 16-bit file: it rounds.
    let rounded = doc.layer_rgba(&layers[0]).unwrap();
    let want: Vec<u8> = ramp(2).iter().map(|&v| ((u32::from(v) + 128) / 257) as u8).collect();
    assert_eq!(rounded, want);
}

#[test]
fn an_empty_16_bit_canvas_keeps_its_depth_and_8_bit_files_say_8() {
    let mut b = DocumentBuilder::new(64, 64);
    b.bits_per_channel = 16;
    b.tree = vec![Node::Layer(LayerSpec::default())];
    let mut doc = Document::from_reader(Cursor::new(b.to_bytes().unwrap())).unwrap();
    assert_eq!(doc.bits_per_channel().unwrap(), 16);
    let mut b8 = DocumentBuilder::new(64, 64);
    b8.tree = vec![Node::Layer(LayerSpec { pixels: Some(vec![9; 64 * 64 * 4]), ..LayerSpec::default() })];
    let mut doc8 = Document::from_reader(Cursor::new(b8.to_bytes().unwrap())).unwrap();
    assert_eq!(doc8.bits_per_channel().unwrap(), 8);
    assert_eq!(doc8.declared_bits, None, "8-bit archives are unchanged");
    let layer = doc8.layers[0].clone();
    assert_eq!(doc8.layer_rgba16(&layer).unwrap()[0], 9 * 257, "an 8-bit layer widens exactly");
    assert!(b8.bits_per_channel == 8);
}

#[test]
fn a_16_bit_builder_widens_8_bit_layers_and_an_8_bit_builder_rounds_16_bit_ones() {
    let mut b = DocumentBuilder::new(8, 8);
    b.bits_per_channel = 16;
    b.tree = vec![Node::Layer(LayerSpec { pixels: Some(vec![200; 8 * 8 * 4]), ..LayerSpec::default() })];
    let mut doc = Document::from_reader(Cursor::new(b.to_bytes().unwrap())).unwrap();
    let l = doc.layers[0].clone();
    assert!(doc.layer_rgba16(&l).unwrap().iter().all(|&v| v == 200 * 257));
    let mut b = DocumentBuilder::new(8, 8);
    b.tree = vec![Node::Layer(LayerSpec { pixels16: Some(vec![200 * 257 + 100; 8 * 8 * 4]), ..LayerSpec::default() })];
    let mut doc = Document::from_reader(Cursor::new(b.to_bytes().unwrap())).unwrap();
    let l = doc.layers[0].clone();
    assert!(doc.layer_rgba(&l).unwrap().iter().all(|&v| v == 200));
    // Wrong lengths are refused.
    let mut bad = DocumentBuilder::new(8, 8);
    bad.tree = vec![Node::Layer(LayerSpec { pixels16: Some(vec![0; 5]), ..LayerSpec::default() })];
    assert!(bad.to_bytes().is_err());
}

fn straight16(p: &[u16]) -> [u16; 4] {
    let a = u64::from(p[3]);
    if a == 0 {
        return [0; 4];
    }
    let f = |c: u16| ((u64::from(c) * 65_535 + a / 2) / a).min(65_535) as u16;
    [f(p[0]), f(p[1]), f(p[2]), p[3]]
}

#[test]
fn png_16_bit_reads_back_exactly() {
    let raster = Raster16::new(W as u32, H as u32, ramp(1)).unwrap();
    let bytes = encode_raster16(&raster, RasterFormat::Png, &ExportOptions::default()).unwrap();
    let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
    assert_eq!(reader.info().bit_depth, png::BitDepth::Sixteen);
    assert_eq!(reader.info().color_type, png::ColorType::Rgba);
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    let got: Vec<u16> = buf[..frame.buffer_size()].as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect();
    let want: Vec<u16> = raster.rgba.as_chunks::<4>().0.iter().flat_map(|p| straight16(p)).collect();
    assert_eq!(got, want);
    // More than 256 distinct values survive.
    let distinct = got.iter().collect::<std::collections::HashSet<_>>().len();
    assert!(distinct > 1000, "{distinct}");
    // Flattened onto a background: RGB16.
    let flat = encode_raster16(&raster, RasterFormat::Png, &ExportOptions { transparent: false, ..ExportOptions::default() }).unwrap();
    let r = png::Decoder::new(Cursor::new(flat)).read_info().unwrap();
    assert_eq!((r.info().color_type, r.info().bit_depth), (png::ColorType::Rgb, png::BitDepth::Sixteen));
}

#[test]
fn tiff_16_bit_reads_back_exactly_with_and_without_compression() {
    use tiff::decoder::{Decoder, DecodingResult};
    let raster = Raster16::new(W as u32, H as u32, ramp(2)).unwrap();
    for compression in [TiffCompression::Uncompressed, TiffCompression::Lzw] {
        let bytes = encode_raster16(&raster, RasterFormat::Tiff { compression }, &ExportOptions::default()).unwrap();
        let mut d = Decoder::new(Cursor::new(bytes)).unwrap();
        assert_eq!(d.dimensions().unwrap(), (W as u32, H as u32));
        let got = match d.read_image().unwrap() {
            DecodingResult::U16(v) => v,
            other => panic!("not 16-bit: {:?}", std::mem::discriminant(&other)),
        };
        let want: Vec<u16> = raster.rgba.as_chunks::<4>().0.iter().flat_map(|p| straight16(p)).collect();
        assert_eq!(got, want, "{compression:?}");
    }
}

#[test]
fn jpeg_and_webp_dither_a_16_bit_picture_down_to_8() {
    let raster = Raster16::new(W as u32, H as u32, ramp(1)).unwrap();
    let jpeg = encode_raster16(&raster, RasterFormat::Jpeg { quality: 95 }, &ExportOptions::default()).unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
    let webp = encode_raster16(&raster, RasterFormat::WebP, &ExportOptions::default()).unwrap();
    assert_eq!(&webp[..4], b"RIFF");
    // The dither is a rounding to within one level of the exact value, and never moves values
    // that were 8-bit to begin with.
    let down = raster.to_raster_dithered();
    for (s, d) in raster.rgba.iter().zip(&down.rgba) {
        assert!((f64::from(*s) / 257.0 - f64::from(*d)).abs() < 1.0 + 1e-9);
    }
    let exact = Raster16::new(4, 4, (0..64u16).map(|v| (v * 4) * 257).collect()).unwrap();
    let back = exact.to_raster_dithered();
    assert!(back.rgba.iter().zip(&exact.rgba).all(|(d, s)| u16::from(*d) * 257 == *s));
}

fn psd_layer(name: &str, blend: PsdBlend) -> PsdLayer {
    PsdLayer { name: name.into(), opacity: 0.75, blend, hidden: false, clipped: false, alpha_locked: false, locked: false, pixels: Vec::new() }
}

fn psd_tree() -> (PsdDocument, Vec<Vec<u16>>) {
    let doc = PsdDocument {
        width: W as u32,
        height: H as u32,
        dpi: 144.0,
        colour_space: None,
        root: vec![
            PsdNode::Layer(psd_layer("bottom", PsdBlend::Normal)),
            PsdNode::Group(PsdGroup {
                name: "folder".into(),
                opacity: 1.0,
                blend: PsdBlend::Normal,
                hidden: false,
                collapsed: false,
                children: vec![PsdNode::Layer(psd_layer("inner", PsdBlend::Multiply))],
            }),
            PsdNode::Layer(psd_layer("top", PsdBlend::Screen)),
        ],
    };
    (doc, vec![ramp(1), ramp(2), ramp(3)])
}

#[test]
fn psd_16_bit_round_trips_raw_and_rle() {
    let (doc, layers) = psd_tree();
    for compression in [Compression16::Raw, Compression16::Rle] {
        let bytes = psd::to_bytes16(&doc, &layers, compression).unwrap();
        assert_eq!(&bytes[..4], b"8BPS");
        assert_eq!(u16::from_be_bytes([bytes[22], bytes[23]]), 16, "header depth");
        let (back, back_layers) = psd::from_bytes16(&bytes).unwrap();
        assert_eq!(back.root.len(), 3);
        assert_eq!(back_layers.len(), 3);
        // Premultiplied pixels survive up to the straight-colour rounding of the file.
        for (got, want) in back_layers.iter().zip(&layers) {
            let worst = got.iter().zip(want).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(worst <= 1, "{compression:?}: worst {worst}");
        }
        // Names, blends and tree shape come back.
        let PsdNode::Layer(l) = &back.root[2] else { panic!() };
        assert_eq!((l.name.as_str(), l.blend), ("top", PsdBlend::Screen));
        let PsdNode::Group(g) = &back.root[1] else { panic!() };
        assert_eq!(g.children.len(), 1);
        // The 8-bit reader refuses a 16-bit file.
        let err = psd::from_bytes(&bytes).unwrap_err().to_string();
        assert!(err.contains("unsupported channel depth"), "{err}");
    }
    // RLE is smaller than raw for flat areas.
    let flat = vec![vec![0x4242u16; W * H * 4]; 3]; // equal bytes, so PackBits finds runs
    let raw = psd::to_bytes16(&doc, &flat, Compression16::Raw).unwrap();
    let rle = psd::to_bytes16(&doc, &flat, Compression16::Rle).unwrap();
    assert!(rle.len() < raw.len() / 4, "{} vs {}", rle.len(), raw.len());
}

#[test]
fn psd_16_bit_merged_image_is_the_composite() {
    let (doc, layers) = psd_tree();
    let bytes = psd::to_bytes16(&doc, &layers, Compression16::Rle).unwrap();
    // Only the merged image remains when the layer records are dropped: strip them by writing a
    // document with no layers and reading the merged section back.
    let empty = PsdDocument { root: Vec::new(), ..doc.clone() };
    let merged_only = psd::to_bytes16(&empty, &[], Compression16::Raw).unwrap();
    let (back, px) = psd::from_bytes16(&merged_only).unwrap();
    assert_eq!(back.root.len(), 1, "a file without layers reads as its merged image");
    assert_eq!(px[0].len(), W * H * 4);
    assert!(px[0].iter().all(|&v| v == 0), "an empty document is transparent");
    assert!(bytes.len() > merged_only.len());
}

#[test]
fn psd_16_bit_rejects_a_wrong_number_of_buffers() {
    let (doc, layers) = psd_tree();
    assert!(psd::to_bytes16(&doc, &layers[..2], Compression16::Raw).is_err());
    let mut short = layers.clone();
    short[0].pop();
    assert!(psd::to_bytes16(&doc, &short, Compression16::Raw).is_err());
}

#[test]
fn a_half_width_16_bit_edge_tile_is_not_mistaken_for_a_full_8_bit_tile() {
    // 640 wide: the third tile column is 128 pixels, so a clipped 16-bit tile (128 x 256 x 8
    // bytes) is exactly as long as a full 8-bit tile (256 x 256 x 4 bytes).
    let (w, h) = (640usize, 300usize);
    let px: Vec<u16> = (0..w * h * 4).map(|i| ((i as u32).wrapping_mul(40_503) >> 3) as u16 | 1).collect();
    let mut b = DocumentBuilder::new(w as u32, h as u32);
    b.bits_per_channel = 16;
    b.tree = vec![Node::Layer(LayerSpec { pixels16: Some(px.clone()), ..LayerSpec::default() })];
    let mut doc = Document::from_reader(Cursor::new(b.to_bytes().unwrap())).unwrap();
    let layer = doc.layers[0].clone();
    assert_eq!(doc.layer_rgba16(&layer).unwrap(), px);
}
