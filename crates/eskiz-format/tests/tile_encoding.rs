//! Tile sample encodings: Eskiz's own keys, and detection from the tiles when they are absent.

use std::io::Cursor;

use eskiz_format::{Document, DocumentBuilder, LayerSpec, Node, TileEncoding};

const W: usize = 300;
const H: usize = 70;

/// A keyless 16-bit builder: the tiles are written as given, with no `bitsPerChannel` or
/// `channelFormat` key, as some files in the wild do.
fn keyless(tree: Vec<LayerSpec>) -> DocumentBuilder {
    let mut b = DocumentBuilder::new(W as u32, H as u32);
    b.bits_per_channel = 16;
    b.write_eskiz_keys = false;
    b.tree = tree.into_iter().map(Node::Layer).collect();
    b
}

fn open(b: &DocumentBuilder) -> Document<Cursor<Vec<u8>>> {
    Document::from_reader(Cursor::new(b.to_bytes().unwrap())).unwrap()
}

/// Opaque white (`0xFFFF`) on the left, and a horizontal gradient with a transparent band.
fn uint16_pixels() -> (Vec<u16>, Vec<u16>) {
    let white = vec![0xFFFFu16; W * H * 4];
    let mut gradient = vec![0u16; W * H * 4];
    for y in 0..H {
        for x in 20..W {
            let g = (x - 20) as u32 * 65_535 / (W as u32 - 21);
            let i = (y * W + x) * 4;
            gradient[i..i + 4].copy_from_slice(&[g as u16, (65_535 - g) as u16, (y as u32 * 900) as u16, 0xFFFF]);
        }
    }
    (white, gradient)
}

#[test]
fn a_keyless_uint16_document_is_detected_as_uint16_and_reads_back_exactly() {
    let (white, gradient) = uint16_pixels();
    let b = keyless(vec![
        LayerSpec { name: "gradient".into(), pixels16: Some(gradient.clone()), ..LayerSpec::default() },
        LayerSpec { name: "white".into(), pixels16: Some(white.clone()), ..LayerSpec::default() },
    ]);
    let mut doc = open(&b);
    assert_eq!(doc.declared_bits, None, "no Eskiz key in the archive");
    assert_eq!(doc.tile_encoding().unwrap(), TileEncoding::U16Unorm);
    assert_eq!(doc.bits_per_channel().unwrap(), 16);
    let layers = doc.layers.clone();
    assert_eq!(doc.layer_rgba16(&layers[0]).unwrap(), gradient);
    assert_eq!(doc.layer_rgba16(&layers[1]).unwrap(), white);
}

/// Half floats: `0x3C00` is 1.0, `0x3800` is 0.5 and `0x3400` is 0.25.
const HALF_ONE: u16 = 0x3C00;
const HALF_HALF: u16 = 0x3800;
const HALF_QUARTER: u16 = 0x3400;

#[test]
fn a_keyless_half_float_document_is_detected_as_f16_and_converted() {
    let white = vec![HALF_ONE; W * H * 4];
    let mut grey = vec![0u16; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 4;
            let px = match x {
                0..=99 => continue, // transparent
                100..=199 => [HALF_HALF, HALF_HALF, HALF_HALF, HALF_ONE], // mid grey, opaque
                _ => [HALF_QUARTER; 4],                                    // a quarter-alpha grey
            };
            grey[i..i + 4].copy_from_slice(&px);
        }
    }
    let b = keyless(vec![
        LayerSpec { name: "grey".into(), pixels16: Some(grey), ..LayerSpec::default() },
        LayerSpec { name: "white".into(), pixels16: Some(white), ..LayerSpec::default() },
    ]);
    let mut doc = open(&b);
    assert_eq!(doc.tile_encoding().unwrap(), TileEncoding::F16);
    let layers = doc.layers.clone();
    let grey = doc.layer_rgba16(&layers[0]).unwrap();
    let white = doc.layer_rgba16(&layers[1]).unwrap();
    // Half values scale to 0..65535: 1.0 is 65535, 0.5 is 32768, 0.25 is 16384.
    assert!(white.iter().all(|&v| v == 65_535), "opaque white");
    for (p, px) in grey.as_chunks::<4>().0.iter().enumerate() {
        let x = p % W;
        let want: [u16; 4] = match x {
            0..=99 => [0; 4],
            100..=199 => [32_768, 32_768, 32_768, 65_535],
            _ => [16_384; 4],
        };
        assert_eq!(*px, want, "pixel x={x}");
    }
    // The 8-bit reader rounds the same samples: 1.0 to 255, 0.5 to 128.
    let grey8 = doc.layer_rgba(&layers[0]).unwrap();
    assert_eq!(&grey8[(100) * 4..(100) * 4 + 4], &[128, 128, 128, 255]);
}

#[test]
fn a_half_document_with_premultiplied_colour_above_alpha_is_uint16() {
    // Every word is a valid half in [0, 1], but colour exceeds alpha when read as half, so the
    // half reading is rejected.
    let mut px = vec![0u16; W * H * 4];
    for i in (0..px.len()).step_by(4) {
        px[i..i + 4].copy_from_slice(&[HALF_ONE, HALF_ONE, HALF_ONE, HALF_QUARTER]);
    }
    let b = keyless(vec![LayerSpec { pixels16: Some(px.clone()), ..LayerSpec::default() }]);
    let mut doc = open(&b);
    assert_eq!(doc.tile_encoding().unwrap(), TileEncoding::U16Unorm);
    let layer = doc.layers[0].clone();
    assert_eq!(doc.layer_rgba16(&layer).unwrap(), px);
}

#[test]
fn eskiz_keys_win_over_the_data() {
    // Every word is at most 0x3C00, which would read as half floats, but Eskiz's keys say uint16.
    let px: Vec<u16> = (0..W * H * 4).map(|i| ((i as u32 * 37) % 15_000) as u16).collect();
    let mut b = DocumentBuilder::new(W as u32, H as u32);
    b.bits_per_channel = 16;
    b.tree = vec![Node::Layer(LayerSpec { pixels16: Some(px.clone()), ..LayerSpec::default() })];
    let mut doc = open(&b);
    assert_eq!(doc.declared_bits, Some(16));
    assert_eq!(doc.tile_encoding().unwrap(), TileEncoding::U16Unorm);
    let layer = doc.layers[0].clone();
    assert_eq!(doc.layer_rgba16(&layer).unwrap(), px, "Eskiz files round-trip exactly");
}

#[test]
fn a_keyless_8_bit_document_has_no_16_bit_tiles() {
    let mut b = DocumentBuilder::new(64, 64);
    b.tree = vec![Node::Layer(LayerSpec { pixels: Some(vec![9; 64 * 64 * 4]), ..LayerSpec::default() })];
    let mut doc = open(&b);
    assert_eq!(doc.tile_encoding().unwrap(), TileEncoding::U8);
}

#[test]
fn a_keyless_16_bit_document_with_no_pixels_has_no_16_bit_tiles() {
    // The builder writes no tile for an empty layer, so there is nothing to tell the encodings apart.
    let b = keyless(vec![LayerSpec::default()]);
    let mut doc = open(&b);
    assert_eq!(doc.tile_encoding().unwrap(), TileEncoding::U8);
}

/// A 384 by 256 canvas with 256-pixel tiles: columns 0 and 1 are full tiles, and column 2 does
/// not exist, so the right-hand tiles are 128 pixels wide. The 16-bit tile at column 1 (128 by 256
/// clipped) is 262 144 bytes, exactly a full 8-bit 256-pixel tile, so its length alone cannot say
/// which depth it is.
const GW: usize = 384;
const GH: usize = 256;

/// A 16-bit value per channel at (x, y), kept premultiplied (colour at most alpha).
fn grid_px(x: usize, y: usize) -> [u16; 4] {
    let a = 0xFFFF - ((x * 7 + y * 3) % 512) as u16;
    [a / 2, a / 3, a / 5, a]
}

fn grid_layer(x_from: usize, x_to: usize) -> Vec<u16> {
    let mut px = vec![0u16; GW * GH * 4];
    for y in 0..GH {
        for x in x_from..x_to {
            let i = (y * GW + x) * 4;
            px[i..i + 4].copy_from_slice(&grid_px(x, y));
        }
    }
    px
}

#[test]
fn a_keyless_16_bit_document_with_a_half_width_edge_column_reads_back_exactly() {
    let px = grid_layer(0, GW);
    let mut b = DocumentBuilder::new(GW as u32, GH as u32);
    b.bits_per_channel = 16;
    b.write_eskiz_keys = false;
    b.tree = vec![Node::Layer(LayerSpec { name: "grid".into(), pixels16: Some(px.clone()), ..LayerSpec::default() })];
    let mut doc = open(&b);
    assert_eq!(doc.declared_bits, None);
    assert_eq!(doc.tile_count(&doc.layers[0].uuid.clone()), 2, "one full tile and one 128-wide edge tile");
    assert_eq!(doc.bits_per_channel().unwrap(), 16);
    let layer = doc.layers[0].clone();
    assert!(doc.layer_rgba16(&layer).unwrap() == px, "the edge column is read as 16-bit");
}

#[test]
fn a_keyless_8_bit_document_with_a_half_width_edge_column_reads_back_exactly() {
    let px8: Vec<u8> = grid_layer(0, GW).iter().map(|&v| down16_for_test(v)).collect();
    let mut b = DocumentBuilder::new(GW as u32, GH as u32);
    b.write_eskiz_keys = false;
    b.tree = vec![Node::Layer(LayerSpec { name: "grid".into(), pixels: Some(px8.clone()), ..LayerSpec::default() })];
    let mut doc = open(&b);
    assert_eq!(doc.bits_per_channel().unwrap(), 8);
    let layer = doc.layers[0].clone();
    assert!(doc.layer_rgba(&layer).unwrap() == px8, "the edge column is read as 8-bit");
}

#[test]
fn a_keyless_16_bit_edge_tile_takes_the_depth_decided_by_the_whole_document() {
    // The "edge" layer's only tile is the 128-wide column, which is ambiguous on its own. The
    // "full" layer's interior tile is not, and decides the depth for every layer.
    let edge = grid_layer(256, GW);
    let full = grid_layer(0, 256);
    let mut b = DocumentBuilder::new(GW as u32, GH as u32);
    b.bits_per_channel = 16;
    b.write_eskiz_keys = false;
    b.tree = vec![
        Node::Layer(LayerSpec { name: "edge".into(), pixels16: Some(edge.clone()), ..LayerSpec::default() }),
        Node::Layer(LayerSpec { name: "full".into(), pixels16: Some(full.clone()), ..LayerSpec::default() }),
    ];
    let mut doc = open(&b);
    let layers = doc.layers.clone();
    assert!(doc.layer_rgba16(&layers[0]).unwrap() == edge, "edge layer");
    assert!(doc.layer_rgba16(&layers[1]).unwrap() == full, "full layer");
}

fn down16_for_test(v: u16) -> u8 {
    ((u32::from(v) * 255 + 32_767) / 65_535) as u8
}
