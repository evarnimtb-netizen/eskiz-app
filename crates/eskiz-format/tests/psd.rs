//! PSD tests: PackBits edge cases, write-then-read round trips, length and header checks, the
//! merged-image composite, rejection of unsupported files, and atomic file writes.

use std::env;
use std::fs;

use eskiz_format::psd::{
    self, packbits_decode, packbits_encode, PsdBlend, PsdDocument, PsdGroup, PsdLayer, PsdNode,
};
use eskiz_format::Error;

const ALL_BLENDS: [PsdBlend; 26] = [
    PsdBlend::Normal,
    PsdBlend::Multiply,
    PsdBlend::Screen,
    PsdBlend::Add,
    PsdBlend::Lighten,
    PsdBlend::Exclusion,
    PsdBlend::Difference,
    PsdBlend::Subtract,
    PsdBlend::LinearBurn,
    PsdBlend::ColorDodge,
    PsdBlend::ColorBurn,
    PsdBlend::Overlay,
    PsdBlend::HardLight,
    PsdBlend::Color,
    PsdBlend::Luminosity,
    PsdBlend::Hue,
    PsdBlend::Saturation,
    PsdBlend::SoftLight,
    PsdBlend::Darken,
    PsdBlend::HardMix,
    PsdBlend::VividLight,
    PsdBlend::LinearLight,
    PsdBlend::PinLight,
    PsdBlend::LighterColor,
    PsdBlend::DarkerColor,
    PsdBlend::Divide,
];

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// Premultiplies a straight colour the same way the reader does.
fn premul_px(c: [u8; 3], a: u8) -> [u8; 4] {
    let p = |v: u8| ((u32::from(v) * u32::from(a) + 127) / 255) as u8;
    [p(c[0]), p(c[1]), p(c[2]), a]
}

/// A canvas-sized premultiplied image. Pixels outside `rect` (x0, y0, x1, y1) are transparent.
/// Inside, alpha varies from 0 to 255, so some pixels are fully transparent.
fn pattern(w: usize, h: usize, rect: (usize, usize, usize, usize), seed: u32) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    for y in rect.1..rect.3.min(h) {
        for x in rect.0..rect.2.min(w) {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let a = (state >> 24) as u8;
            let c = [
                (x as u8).wrapping_mul(3) ^ (state >> 8) as u8,
                (y as u8).wrapping_mul(5),
                (state >> 16) as u8,
            ];
            let i = (y * w + x) * 4;
            out[i..i + 4].copy_from_slice(&premul_px(c, a));
        }
    }
    out
}

fn opaque(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for px in out.as_chunks_mut::<4>().0.iter_mut() {
        px.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    out
}

fn layer(
    name: &str,
    w: usize,
    h: usize,
    rect: (usize, usize, usize, usize),
    seed: u32,
) -> PsdLayer {
    PsdLayer {
        name: name.into(),
        opacity: 1.0,
        blend: PsdBlend::Normal,
        hidden: false,
        clipped: false,
        alpha_locked: false,
        locked: false,
        pixels: pattern(w, h, rect, seed),
    }
}

fn doc(w: u32, h: u32, root: Vec<PsdNode>) -> PsdDocument {
    PsdDocument {
        width: w,
        height: h,
        dpi: 300.0,
        colour_space: None,
        root,
    }
}

fn round_trip(d: &PsdDocument) -> PsdDocument {
    let bytes = psd::to_bytes(d).expect("encode");
    psd::from_bytes(&bytes).expect("decode")
}

/// Pixels must match in alpha exactly. Colour may differ by 1 after premultiply and
/// un-premultiply, and fully transparent pixels must stay zero.
fn assert_pixels_close(expected: &[u8], actual: &[u8], what: &str) {
    assert_eq!(expected.len(), actual.len(), "{what}: length");
    for (i, (e, a)) in expected
        .as_chunks::<4>()
        .0
        .iter()
        .zip(actual.as_chunks::<4>().0)
        .enumerate()
    {
        assert_eq!(e[3], a[3], "{what}: alpha at pixel {i}");
        if e[3] == 0 {
            assert_eq!(a, &[0, 0, 0, 0], "{what}: transparent pixel {i}");
            continue;
        }
        for k in 0..3 {
            let diff = (i32::from(e[k]) - i32::from(a[k])).abs();
            assert!(
                diff <= 1,
                "{what}: pixel {i} channel {k}: {} vs {}",
                e[k],
                a[k]
            );
        }
    }
}

fn find_layer<'a>(nodes: &'a [PsdNode], name: &str) -> &'a PsdLayer {
    for node in nodes {
        match node {
            PsdNode::Layer(l) if l.name == name => return l,
            PsdNode::Group(g) => {
                if let Some(found) = try_find(&g.children, name) {
                    return found;
                }
            }
            _ => {}
        }
    }
    panic!("layer {name:?} not found");
}

fn try_find<'a>(nodes: &'a [PsdNode], name: &str) -> Option<&'a PsdLayer> {
    nodes.iter().find_map(|node| match node {
        PsdNode::Layer(l) if l.name == name => Some(l),
        PsdNode::Group(g) => try_find(&g.children, name),
        _ => None,
    })
}

fn group<'a>(nodes: &'a [PsdNode], name: &str) -> &'a PsdGroup {
    nodes
        .iter()
        .find_map(|node| match node {
            PsdNode::Group(g) if g.name == name => Some(g),
            PsdNode::Group(g) => try_group(&g.children, name),
            _ => None,
        })
        .unwrap_or_else(|| panic!("group {name:?} not found"))
}

fn try_group<'a>(nodes: &'a [PsdNode], name: &str) -> Option<&'a PsdGroup> {
    nodes.iter().find_map(|node| match node {
        PsdNode::Group(g) if g.name == name => Some(g),
        PsdNode::Group(g) => try_group(&g.children, name),
        _ => None,
    })
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Byte offsets of the sections of a file written by this module, computed from its header.
struct Layout {
    section_len_at: usize,
    section_len: usize,
    layer_info_len: usize,
    merged_at: usize,
}

fn layout(bytes: &[u8]) -> Layout {
    let res_len = be32(bytes, 30) as usize;
    let section_len_at = 34 + res_len;
    let section_len = be32(bytes, section_len_at) as usize;
    let layer_info_len = be32(bytes, section_len_at + 4) as usize;
    Layout {
        section_len_at,
        section_len,
        layer_info_len,
        merged_at: section_len_at + 4 + section_len,
    }
}

/// Decodes the merged image from an encoded file into straight RGBA8 bytes.
fn merged_rgba(bytes: &[u8], w: usize, h: usize) -> Vec<u8> {
    let at = layout(bytes).merged_at;
    assert_eq!(
        u16::from_be_bytes([bytes[at], bytes[at + 1]]),
        1,
        "merged is RLE"
    );
    let counts_at = at + 2;
    let mut pos = counts_at + 4 * h * 2;
    let mut out = vec![0u8; w * h * 4];
    for c in 0..4 {
        for row in 0..h {
            let count_at = counts_at + (c * h + row) * 2;
            let n = u16::from_be_bytes([bytes[count_at], bytes[count_at + 1]]) as usize;
            let decoded = packbits_decode(&bytes[pos..pos + n], w).expect("merged row");
            pos += n;
            for (x, v) in decoded.into_iter().enumerate() {
                out[((row * w) + x) * 4 + c] = v;
            }
        }
    }
    out
}

fn assert_same_tree(expected: &[PsdNode], actual: &[PsdNode]) {
    assert_eq!(expected.len(), actual.len(), "child count");
    for (e, a) in expected.iter().zip(actual) {
        match (e, a) {
            (PsdNode::Layer(e), PsdNode::Layer(a)) => {
                assert_eq!(e.name, a.name, "layer name");
                assert_eq!(e.blend, a.blend, "blend of {}", e.name);
                assert!(
                    (e.opacity - a.opacity).abs() <= 1.0 / 255.0,
                    "opacity of {}",
                    e.name
                );
                assert_eq!(e.hidden, a.hidden, "hidden of {}", e.name);
                assert_eq!(e.clipped, a.clipped, "clipped of {}", e.name);
                assert_eq!(e.alpha_locked, a.alpha_locked, "lock transparency of {}", e.name);
                assert_eq!(e.locked, a.locked, "lock of {}", e.name);
                assert_eq!(e.pixels.len(), a.pixels.len());
                assert_pixels_close(&e.pixels, &a.pixels, &e.name);
            }
            (PsdNode::Group(e), PsdNode::Group(a)) => {
                assert_eq!(e.name, a.name, "group name");
                assert_eq!(e.blend, a.blend, "group blend of {}", e.name);
                assert!(
                    (e.opacity - a.opacity).abs() <= 1.0 / 255.0,
                    "group opacity"
                );
                assert_eq!(e.hidden, a.hidden, "group hidden of {}", e.name);
                assert_eq!(e.collapsed, a.collapsed, "group collapsed of {}", e.name);
                assert_same_tree(&e.children, &a.children);
            }
            _ => panic!("node kind differs"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// PackBits
// ---------------------------------------------------------------------------------------------

#[test]
fn packbits_empty_input_gives_empty_output() {
    assert!(packbits_encode(&[]).is_empty());
    assert!(packbits_decode(&[], 0).expect("empty row").is_empty());
}

#[test]
fn packbits_single_byte_is_a_literal() {
    assert_eq!(packbits_encode(&[7]), vec![0, 7]);
    assert_eq!(packbits_decode(&[0, 7], 1).unwrap(), vec![7]);
}

#[test]
fn packbits_two_equal_bytes_use_a_repeat_record() {
    let encoded = packbits_encode(&[9, 9]);
    assert_eq!(encoded, vec![0xFF, 9]);
    assert_eq!(packbits_decode(&encoded, 2).unwrap(), vec![9, 9]);
}

#[test]
fn packbits_literal_limits_at_128_and_129() {
    for len in [127usize, 128, 129, 256, 257] {
        // No two neighbours are equal, so the whole input is literal.
        let src: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let encoded = packbits_encode(&src);
        assert_eq!(packbits_decode(&encoded, len).unwrap(), src, "len {len}");
        assert!(encoded.len() <= len + len / 128 + 1);
    }
}

#[test]
fn packbits_run_limits_at_128_and_129() {
    for len in [127usize, 128, 129, 300, 1000] {
        let src = vec![0xABu8; len];
        let encoded = packbits_encode(&src);
        assert_eq!(packbits_decode(&encoded, len).unwrap(), src, "run {len}");
    }
    // A run of exactly 128 is one record: count byte 129 and the value.
    assert_eq!(packbits_encode(&[5u8; 128]), vec![129, 5]);
}

#[test]
fn packbits_mixed_runs_and_literals_round_trip() {
    let mut src = Vec::new();
    src.extend_from_slice(&[1, 2, 3]);
    src.extend_from_slice(&[4; 10]);
    src.extend_from_slice(&[5, 6]);
    src.extend_from_slice(&[7; 200]);
    src.extend_from_slice(&[8, 8, 9]);
    let encoded = packbits_encode(&src);
    assert_eq!(packbits_decode(&encoded, src.len()).unwrap(), src);
}

#[test]
fn packbits_pseudo_random_round_trip() {
    let mut state = 12_345u32;
    for trial in 0..50 {
        let len = 1 + trial * 37;
        let src: Vec<u8> = (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                // Bias toward repeats so both record kinds appear.
                let v = (state >> 16) as u8;
                if v.is_multiple_of(4) {
                    0
                } else {
                    v
                }
            })
            .collect();
        let encoded = packbits_encode(&src);
        assert_eq!(
            packbits_decode(&encoded, len).unwrap(),
            src,
            "trial {trial}"
        );
    }
}

#[test]
fn packbits_no_op_code_is_skipped() {
    // 0x80 is a no-op record, followed by a literal of one byte.
    assert_eq!(packbits_decode(&[0x80, 0, 42], 1).unwrap(), vec![42]);
}

#[test]
fn packbits_rejects_truncated_and_overflowing_rows() {
    assert!(matches!(
        packbits_decode(&[1, 0], 2),
        Err(Error::Corrupt(_))
    ));
    assert!(matches!(packbits_decode(&[0], 1), Err(Error::Corrupt(_))));
    assert!(matches!(
        packbits_decode(&[0xFE], 1),
        Err(Error::Corrupt(_))
    ));
    // A repeat of 4 overruns a row of 3.
    assert!(matches!(
        packbits_decode(&[0xFD, 1], 3),
        Err(Error::Corrupt(_))
    ));
    // A literal of 2 overruns a row of 1.
    assert!(matches!(
        packbits_decode(&[1, 1, 2], 1),
        Err(Error::Corrupt(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------------------------

#[test]
fn odd_sized_canvases_round_trip() {
    for (w, h) in [(1u32, 1u32), (7, 3), (3, 7), (301, 199)] {
        let (wu, hu) = (w as usize, h as usize);
        let root = vec![PsdNode::Layer(layer(
            "only",
            wu,
            hu,
            (0, 0, wu, hu),
            w * 31 + h,
        ))];
        let d = doc(w, h, root);
        let back = round_trip(&d);
        assert_eq!((back.width, back.height), (w, h));
        assert_same_tree(&d.root, &back.root);
        assert!((back.dpi - 300.0).abs() < 1e-3, "dpi {}", back.dpi);
    }
}

#[test]
fn non_square_canvases_keep_width_and_height_order() {
    for (w, h) in [(2048u32, 1536u32), (1536, 2048)] {
        let (wu, hu) = (w as usize, h as usize);
        let root = vec![PsdNode::Layer(layer(
            "frame",
            wu,
            hu,
            (10, 20, wu - 30, hu - 40),
            7,
        ))];
        let d = doc(w, h, root);
        let bytes = psd::to_bytes(&d).unwrap();
        // Header: height at offset 14, width at offset 18.
        assert_eq!(be32(&bytes, 14), h, "height field");
        assert_eq!(be32(&bytes, 18), w, "width field");
        let back = psd::from_bytes(&bytes).unwrap();
        assert_eq!((back.width, back.height), (w, h));
        assert_same_tree(&d.root, &back.root);
    }
}

#[test]
fn layers_are_cropped_to_their_opaque_bounds() {
    let (w, h) = (64usize, 48usize);
    let l = layer("crop", w, h, (5, 9, 20, 30), 3);
    let bytes = psd::to_bytes(&doc(w as u32, h as u32, vec![PsdNode::Layer(l)])).unwrap();
    // Layer record: count is at the start of layer info, then the rectangle (top, left, bottom, right).
    let li = layout(&bytes).section_len_at + 4 + 4; // start of the layer info count
    assert_eq!(i16::from_be_bytes([bytes[li], bytes[li + 1]]), 1);
    let rect: Vec<i32> = (0..4)
        .map(|k| i32::from_be_bytes(bytes[li + 2 + k * 4..li + 6 + k * 4].try_into().unwrap()))
        .collect();
    assert!(rect[0] >= 9 && rect[1] >= 5, "rect {rect:?}");
    assert!(rect[2] <= 30 && rect[3] <= 20, "rect {rect:?}");
    let back = psd::from_bytes(&bytes).unwrap();
    let PsdNode::Layer(back_layer) = &back.root[0] else {
        panic!("expected a layer");
    };
    assert_eq!(back_layer.pixels.len(), w * h * 4);
}

#[test]
fn length_fields_are_exact() {
    let (w, h) = (40usize, 30usize);
    let root = vec![
        PsdNode::Layer(layer("a", w, h, (0, 0, w, h), 1)),
        PsdNode::Layer(layer("b", w, h, (3, 3, 17, 11), 2)),
    ];
    let bytes = psd::to_bytes(&doc(w as u32, h as u32, root)).unwrap();
    let lay = layout(&bytes);
    // Section = its own length field is not counted: layer info block (with its length field) plus
    // the 4-byte global mask block. Layer info length is even.
    assert_eq!(lay.section_len, 4 + lay.layer_info_len + 4);
    assert_eq!(lay.layer_info_len % 2, 0);
    // The merged image begins exactly where the section ends.
    assert_eq!(lay.merged_at, lay.section_len_at + 4 + lay.section_len);
    assert!(bytes.len() > lay.merged_at);
}

#[test]
fn groups_hidden_clipping_and_unicode_names_round_trip() {
    let (w, h) = (90usize, 60usize);
    let full = (0, 0, w, h);
    let mut hidden_clip = layer("hidden clip", w, h, (10, 10, 40, 40), 11);
    hidden_clip.hidden = true;
    hidden_clip.clipped = true;
    hidden_clip.blend = PsdBlend::Hue;

    let mut child_hidden = layer("child hidden", w, h, (0, 0, 20, 20), 5);
    child_hidden.hidden = true;
    let mut child_screen = layer("child screen", w, h, (30, 5, 80, 50), 6);
    child_screen.blend = PsdBlend::Screen;
    child_screen.opacity = 0.25;

    let deep = PsdNode::Group(PsdGroup {
        name: "inner closed".into(),
        opacity: 0.75,
        blend: PsdBlend::Multiply,
        hidden: false,
        collapsed: true,
        children: vec![PsdNode::Layer(PsdLayer {
            blend: PsdBlend::Overlay,
            ..layer("deep", w, h, (1, 1, 50, 50), 8)
        })],
    });

    let mut clip = layer("clip one", w, h, (5, 5, 85, 55), 9);
    clip.clipped = true;
    clip.blend = PsdBlend::SoftLight;
    clip.opacity = 0.5;

    let mut base = layer("base for clip", w, h, full_rect(w, h), 10);
    base.alpha_locked = true;

    let folder = PsdNode::Group(PsdGroup {
        name: "Folder A".into(),
        opacity: 0.5,
        blend: PsdBlend::Multiply,
        hidden: false,
        collapsed: false,
        children: vec![
            PsdNode::Layer(child_screen),
            PsdNode::Layer(child_hidden),
            deep,
        ],
    });

    let mut unicode = layer("日本語 レイヤー 🎨 é", w, h, (2, 2, 60, 58), 12);
    unicode.locked = true;

    let root = vec![
        PsdNode::Layer(layer("Background", w, h, full, 1)),
        folder,
        PsdNode::Layer(base),
        PsdNode::Layer(clip),
        PsdNode::Layer(hidden_clip),
        PsdNode::Layer(unicode),
    ];
    let d = doc(w as u32, h as u32, root);
    let back = round_trip(&d);
    assert_same_tree(&d.root, &back.root);

    let folder = group(&back.root, "Folder A");
    assert_eq!(folder.children.len(), 3);
    assert!(find_layer(&back.root, "child hidden").hidden);
    assert!(group(&back.root, "inner closed").collapsed);
    assert_eq!(
        try_group(&back.root, "Folder A").map(|g| g.children.len()),
        Some(3)
    );
}

fn full_rect(w: usize, h: usize) -> (usize, usize, usize, usize) {
    (0, 0, w, h)
}

#[test]
fn group_children_come_back_in_order() {
    let (w, h) = (16usize, 16usize);
    let kids = (0..3)
        .map(|i| PsdNode::Layer(layer(&format!("child {i}"), w, h, (0, 0, w, h), i as u32)))
        .collect();
    let root = vec![PsdNode::Group(PsdGroup {
        name: "three".into(),
        opacity: 1.0,
        blend: PsdBlend::Normal,
        hidden: false,
        collapsed: false,
        children: kids,
    })];
    let back = round_trip(&doc(w as u32, h as u32, root.clone()));
    let names: Vec<&str> = match &back.root[0] {
        PsdNode::Group(g) => g
            .children
            .iter()
            .map(|c| match c {
                PsdNode::Layer(l) => l.name.as_str(),
                PsdNode::Group(_) => panic!("unexpected group"),
            })
            .collect(),
        PsdNode::Layer(_) => panic!("expected a group"),
    };
    assert_eq!(names, ["child 0", "child 1", "child 2"]);
    assert_same_tree(&root, &back.root);
}

#[test]
fn all_blend_modes_round_trip_and_normal_is_the_default() {
    let (w, h) = (12usize, 9usize);
    let root: Vec<PsdNode> = ALL_BLENDS
        .iter()
        .enumerate()
        .map(|(i, &mode)| {
            PsdNode::Layer(PsdLayer {
                blend: mode,
                ..layer(&format!("mode {i}"), w, h, (0, 0, w, h), i as u32)
            })
        })
        .collect();
    let back = round_trip(&doc(w as u32, h as u32, root.clone()));
    assert_same_tree(&root, &back.root);
    for (i, node) in back.root.iter().enumerate() {
        let PsdNode::Layer(l) = node else {
            panic!("layer expected")
        };
        assert_eq!(l.blend, ALL_BLENDS[i]);
    }
    assert_eq!(PsdBlend::default(), PsdBlend::Normal);
}

#[test]
fn blend_keys_match_the_spec_table() {
    let expected = [
        (PsdBlend::Normal, "norm"),
        (PsdBlend::Multiply, "mul "),
        (PsdBlend::ColorDodge, "div "),
        (PsdBlend::ColorBurn, "idiv"),
        (PsdBlend::Divide, "fdiv"),
        (PsdBlend::Darken, "dark"),
        (PsdBlend::Lighten, "lite"),
        (PsdBlend::Exclusion, "smud"),
        (PsdBlend::Add, "lddg"),
        (PsdBlend::Hue, "hue "),
        (PsdBlend::Luminosity, "lum "),
        (PsdBlend::Saturation, "sat "),
        (PsdBlend::DarkerColor, "dkCl"),
    ];
    for (mode, key) in expected {
        let bytes: [u8; 4] = key.as_bytes().try_into().unwrap();
        assert_eq!(mode.key(), bytes, "{mode:?}");
        assert_eq!(PsdBlend::from_key(bytes), Some(mode));
    }
    assert_eq!(PsdBlend::from_key(*b"pass"), None);
}

#[test]
fn unknown_blend_key_reads_as_normal() {
    let (w, h) = (8usize, 8usize);
    let mut l = layer("multiply", w, h, (0, 0, w, h), 4);
    l.blend = PsdBlend::Multiply;
    let mut bytes = psd::to_bytes(&doc(w as u32, h as u32, vec![PsdNode::Layer(l)])).unwrap();
    let at = bytes
        .windows(8)
        .position(|win| win == b"8BIMmul ")
        .expect("multiply key present");
    bytes[at + 4..at + 8].copy_from_slice(b"zzzz");
    let back = psd::from_bytes(&bytes).unwrap();
    let PsdNode::Layer(l) = &back.root[0] else {
        panic!("layer")
    };
    assert_eq!(l.blend, PsdBlend::Normal);
}

#[test]
fn opacity_is_rounded_to_a_byte() {
    let (w, h) = (4usize, 4usize);
    let mut half = layer("half", w, h, (0, 0, w, h), 1);
    half.opacity = 0.5;
    let mut full = layer("full", w, h, (0, 0, w, h), 2);
    full.opacity = 1.0;
    let bytes = psd::to_bytes(&doc(
        w as u32,
        h as u32,
        vec![PsdNode::Layer(half), PsdNode::Layer(full)],
    ))
    .unwrap();
    let back = psd::from_bytes(&bytes).unwrap();
    let PsdNode::Layer(a) = &back.root[0] else {
        panic!()
    };
    let PsdNode::Layer(b) = &back.root[1] else {
        panic!()
    };
    assert!((a.opacity - 128.0 / 255.0).abs() < 1e-6, "{}", a.opacity);
    assert!((b.opacity - 1.0).abs() < 1e-6);
}

#[test]
fn long_names_are_cut_to_128_characters() {
    let (w, h) = (4usize, 4usize);
    let name: String = "n".repeat(300);
    let root = vec![PsdNode::Layer(layer(&name, w, h, (0, 0, w, h), 1))];
    let back = round_trip(&doc(w as u32, h as u32, root));
    let PsdNode::Layer(l) = &back.root[0] else {
        panic!()
    };
    assert_eq!(l.name.chars().count(), 128);
    assert!(l.name.chars().all(|c| c == 'n'));
}

#[test]
fn empty_layers_and_empty_documents_round_trip() {
    let (w, h) = (10usize, 6usize);
    let empty = layer("nothing", w, h, (0, 0, 0, 0), 1);
    let back = round_trip(&doc(
        w as u32,
        h as u32,
        vec![PsdNode::Layer(empty.clone())],
    ));
    assert_eq!(back.root.len(), 1);
    assert_pixels_close(
        &empty.pixels,
        match &back.root[0] {
            PsdNode::Layer(l) => &l.pixels,
            PsdNode::Group(_) => panic!(),
        },
        "empty",
    );

    // With no layers the reader falls back to the merged image, which is transparent here.
    let none = round_trip(&doc(w as u32, h as u32, Vec::new()));
    assert_eq!(none.root.len(), 1);
    let PsdNode::Layer(merged) = &none.root[0] else {
        panic!()
    };
    assert_eq!(merged.name, "Merged image");
    assert!(merged.pixels.iter().all(|&v| v == 0));
}

#[test]
fn hidden_layers_keep_pixels_and_visibility_flags() {
    let (w, h) = (6usize, 5usize);
    let mut hidden = layer("hidden", w, h, (0, 0, w, h), 3);
    hidden.hidden = true;
    let shown = layer("shown", w, h, (0, 0, w, h), 4);
    let root = vec![
        PsdNode::Layer(hidden.clone()),
        PsdNode::Layer(shown.clone()),
    ];
    let back = round_trip(&doc(w as u32, h as u32, root));
    let PsdNode::Layer(a) = &back.root[0] else {
        panic!()
    };
    let PsdNode::Layer(b) = &back.root[1] else {
        panic!()
    };
    assert!(a.hidden);
    assert!(!b.hidden);
    assert_pixels_close(&hidden.pixels, &a.pixels, "hidden pixels");
}

// ---------------------------------------------------------------------------------------------
// Merged image
// ---------------------------------------------------------------------------------------------

#[test]
fn merged_image_is_the_composite_of_visible_layers() {
    let (w, h) = (5usize, 4usize);
    let red = PsdLayer {
        pixels: opaque(w, h, [255, 0, 0]),
        ..layer("red", w, h, (0, 0, w, h), 0)
    };
    let blue_half = PsdLayer {
        pixels: opaque(w, h, [0, 0, 255]),
        opacity: 0.5,
        ..layer("blue", w, h, (0, 0, w, h), 0)
    };
    let green_hidden = PsdLayer {
        pixels: opaque(w, h, [0, 255, 0]),
        hidden: true,
        ..layer("green", w, h, (0, 0, w, h), 0)
    };
    let bytes = psd::to_bytes(&doc(
        w as u32,
        h as u32,
        vec![
            PsdNode::Layer(red),
            PsdNode::Layer(blue_half),
            PsdNode::Layer(green_hidden),
        ],
    ))
    .unwrap();
    let merged = merged_rgba(&bytes, w, h);
    // Blue at 50 percent over red gives about (128, 0, 128) with full alpha.
    for px in merged.as_chunks::<4>().0 {
        assert!((i32::from(px[0]) - 127).abs() <= 2, "red {px:?}");
        assert_eq!(px[1], 0, "green must be hidden: {px:?}");
        assert!((i32::from(px[2]) - 128).abs() <= 2, "blue {px:?}");
        assert_eq!(px[3], 255);
    }
}

#[test]
fn merged_image_applies_blend_modes() {
    let (w, h) = (3usize, 2usize);
    let backdrop = PsdLayer {
        pixels: opaque(w, h, [128, 128, 128]),
        ..layer("back", w, h, (0, 0, w, h), 0)
    };
    let multiply = PsdLayer {
        pixels: opaque(w, h, [255, 0, 0]),
        blend: PsdBlend::Multiply,
        ..layer("mul", w, h, (0, 0, w, h), 0)
    };
    let bytes = psd::to_bytes(&doc(
        w as u32,
        h as u32,
        vec![PsdNode::Layer(backdrop), PsdNode::Layer(multiply)],
    ))
    .unwrap();
    let merged = merged_rgba(&bytes, w, h);
    // Multiply of (0.5, 0.5, 0.5) with (1, 0, 0) gives (0.5, 0, 0).
    for px in merged.as_chunks::<4>().0 {
        assert!((i32::from(px[0]) - 128).abs() <= 2, "{px:?}");
        assert_eq!(px[1], 0);
        assert_eq!(px[2], 0);
    }
}

// ---------------------------------------------------------------------------------------------
// Rejection and reading unsupported files
// ---------------------------------------------------------------------------------------------

fn sample_bytes() -> Vec<u8> {
    let (w, h) = (4usize, 4usize);
    psd::to_bytes(&doc(
        w as u32,
        h as u32,
        vec![PsdNode::Layer(layer("x", w, h, (0, 0, w, h), 1))],
    ))
    .unwrap()
}

#[test]
fn sixteen_bit_files_are_unsupported() {
    let mut bytes = sample_bytes();
    bytes[22..24].copy_from_slice(&16u16.to_be_bytes());
    match psd::from_bytes(&bytes) {
        Err(Error::Unsupported(msg)) => assert!(msg.contains("depth"), "{msg}"),
        other => panic!("expected unsupported depth, got {other:?}"),
    }
}

#[test]
fn cmyk_and_lab_files_are_unsupported() {
    for (mode, word) in [(4u16, "CMYK"), (9u16, "Lab")] {
        let mut bytes = sample_bytes();
        bytes[24..26].copy_from_slice(&mode.to_be_bytes());
        match psd::from_bytes(&bytes) {
            Err(Error::Unsupported(msg)) => assert!(msg.contains(word), "{msg}"),
            other => panic!("expected unsupported {word}, got {other:?}"),
        }
    }
}

#[test]
fn large_format_and_non_psd_files_are_rejected() {
    let mut bytes = sample_bytes();
    bytes[4..6].copy_from_slice(&2u16.to_be_bytes());
    assert!(matches!(
        psd::from_bytes(&bytes),
        Err(Error::Unsupported(_))
    ));

    assert!(matches!(
        psd::from_bytes(b"not a psd file at all, just text"),
        Err(Error::Format(_))
    ));
    assert!(matches!(
        psd::from_bytes(&sample_bytes()[..20]),
        Err(Error::Format(_))
    ));
}

#[test]
fn writer_rejects_bad_inputs() {
    let (w, h) = (4usize, 4usize);
    let mut bad = layer("bad", w, h, (0, 0, w, h), 1);
    bad.pixels.pop();
    assert!(matches!(
        psd::to_bytes(&doc(4, 4, vec![PsdNode::Layer(bad)])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        psd::to_bytes(&doc(0, 4, Vec::new())),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        psd::to_bytes(&doc(30_001, 4, Vec::new())),
        Err(Error::InvalidInput(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

#[test]
fn write_then_read_from_disk_leaves_no_temporary_file() {
    let dir = env::temp_dir().join(format!("eskiz-psd-test-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doc.psd");
    let (w, h) = (9usize, 7usize);
    let d = doc(
        w as u32,
        h as u32,
        vec![PsdNode::Layer(layer("disk", w, h, (1, 1, 8, 6), 2))],
    );
    psd::write(&path, &d).unwrap();
    let back = psd::read(&path).unwrap();
    assert_same_tree(&d.root, &back.root);

    let mut tmp_found = false;
    for entry in fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name();
        if name.to_string_lossy().ends_with(".tmp") {
            tmp_found = true;
        }
    }
    assert!(!tmp_found, "temporary file left behind");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn write_to_and_read_from_streams_round_trip() {
    let (w, h) = (6usize, 6usize);
    let d = doc(
        w as u32,
        h as u32,
        vec![PsdNode::Layer(layer("stream", w, h, (0, 0, w, h), 5))],
    );
    let mut buf = Vec::new();
    psd::write_to(&mut buf, &d).unwrap();
    let back = psd::read_from(buf.as_slice()).unwrap();
    assert_same_tree(&d.root, &back.root);
}

// ---------------------------------------------------------------------------------------------
// Cross-check against the independent `psd` crate (dev-dependency)
// ---------------------------------------------------------------------------------------------

#[test]
fn independent_psd_reader_agrees_with_our_output() {
    use ::psd::Psd as ExternalPsd;

    let (w, h) = (37usize, 23usize);
    let mut hidden = layer("hidden child", w, h, (2, 2, 30, 20), 21);
    hidden.hidden = true;
    let mut screen = layer("screen child", w, h, (5, 3, 35, 22), 22);
    screen.blend = PsdBlend::Screen;
    screen.opacity = 0.5;
    let mut clipped = layer("clipped", w, h, (10, 10, 20, 20), 23);
    clipped.clipped = true;
    let root = vec![
        PsdNode::Layer(layer("base", w, h, (0, 0, w, h), 20)),
        PsdNode::Layer(clipped),
        PsdNode::Group(PsdGroup {
            name: "Group".into(),
            opacity: 1.0,
            blend: PsdBlend::Normal,
            hidden: false,
            collapsed: false,
            children: vec![PsdNode::Layer(screen), PsdNode::Layer(hidden)],
        }),
        PsdNode::Layer(layer("名前 ✓", w, h, (1, 1, 5, 5), 24)),
    ];
    let d = doc(w as u32, h as u32, root);
    let bytes = psd::to_bytes(&d).unwrap();

    let ext = ExternalPsd::from_bytes(&bytes).expect("independent reader accepts the file");
    assert_eq!((ext.width(), ext.height()), (w as u32, h as u32));

    let names: Vec<&str> = ext.layers().iter().map(|l| l.name()).collect();
    // The independent reader lists layers top first, the reverse of the file order.
    assert_eq!(
        names,
        ["名前 ✓", "hidden child", "screen child", "clipped", "base"]
    );
    assert_eq!(ext.groups().len(), 1, "one group");

    let find = |n: &str| {
        ext.layers()
            .iter()
            .find(|l| l.name() == n)
            .unwrap_or_else(|| panic!("{n} missing"))
    };
    // The independent crate reads flag bit 1 as "visible", the opposite of our reading. Photoshop
    // fixtures in that crate (all visible layers) have bit 1 clear, so bit 1 set means hidden. The
    // crate's answer is therefore the inverse of our hidden flag here.
    assert!(!find("base").visible());
    assert!(find("hidden child").visible());
    // The crate's `is_clipping_mask` is true for the base layer (clip byte 0), the inverse of the
    // meaning. Its clipping fixture has byte 0 on the base and byte 1 on the clipped layers, which is
    // what we write.
    assert!(find("base").is_clipping_mask());
    assert!(!find("clipped").is_clipping_mask());
    assert_eq!(format!("{:?}", find("screen child").blend_mode()), "Screen");
    assert_eq!(find("screen child").opacity(), 128);
    assert!(find("screen child").parent_id().is_some());
    assert!(find("base").parent_id().is_none());

    // Every layer's pixels, compared in premultiplied form. `rgba()` on a layer returns a canvas-sized
    // image with the layer placed at its rectangle and transparent elsewhere.
    for ext_layer in ext.layers() {
        let ours = try_find(&d.root, ext_layer.name()).expect("layer exists in our tree");
        let left = ext_layer.layer_left() as usize;
        let top = ext_layer.layer_top() as usize;
        let (lw, lh) = (ext_layer.width() as usize, ext_layer.height() as usize);
        let theirs = ext_layer.rgba();
        assert_eq!(theirs.len(), w * h * 4, "{}: rgba length", ext_layer.name());
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let our_px = &ours.pixels[i..i + 4];
                let t = &theirs[i..i + 4];
                let inside = x >= left && y >= top && x < left + lw && y < top + lh;
                if !inside {
                    assert_eq!(
                        our_px[3],
                        0,
                        "{} outside its rect at ({x},{y})",
                        ext_layer.name()
                    );
                    assert_eq!(
                        t[3],
                        0,
                        "{} outside its rect at ({x},{y})",
                        ext_layer.name()
                    );
                    continue;
                }
                assert_eq!(our_px[3], t[3], "{} alpha at ({x},{y})", ext_layer.name());
                let expect = premul_px([t[0], t[1], t[2]], t[3]);
                for (e, o) in expect[..3].iter().zip(&our_px[..3]) {
                    assert!(
                        (i32::from(*e) - i32::from(*o)).abs() <= 1,
                        "{} color at ({x},{y})",
                        ext_layer.name()
                    );
                }
            }
        }
    }

    // The merged image, compared straight.
    let merged_theirs = ext.rgba();
    let merged_ours = merged_rgba(&bytes, w, h);
    assert_eq!(merged_theirs.len(), merged_ours.len());
    for (t, o) in merged_theirs
        .as_chunks::<4>()
        .0
        .iter()
        .zip(merged_ours.as_chunks::<4>().0)
    {
        assert_eq!(t[3], o[3], "merged alpha");
        if o[3] == 0 {
            continue;
        }
        for k in 0..3 {
            assert!(
                (i32::from(t[k]) - i32::from(o[k])).abs() <= 2,
                "merged color {t:?} {o:?}"
            );
        }
    }
}

#[test]
fn icc_profile_resource_round_trips_and_is_0x040f() {
    use eskiz_format::icc;
    use eskiz_guides::ColourSpace;
    for space in ColourSpace::ALL {
        let mut d = doc(4, 4, vec![PsdNode::Layer(layer("L", 4, 4, (0, 0, 4, 4), 1))]);
        d.colour_space = Some(space);
        let bytes = psd::to_bytes(&d).expect("encode");
        // The resource block is in the file: "8BIM", 0x040F, empty name, size, then the profile.
        let profile = icc::profile(space);
        let mut needle = b"8BIM\x04\x0F\x00\x00".to_vec();
        needle.extend((profile.len() as u32).to_be_bytes());
        needle.extend(&profile);
        assert!(bytes.windows(needle.len()).any(|w| w == needle.as_slice()), "{space:?}: ICC block missing");
        assert_eq!(psd::from_bytes(&bytes).expect("decode").colour_space, Some(space));
    }
    let plain = psd::to_bytes(&doc(2, 2, vec![PsdNode::Layer(layer("L", 2, 2, (0, 0, 2, 2), 1))])).expect("encode");
    assert_eq!(psd::from_bytes(&plain).expect("decode").colour_space, None);
}
