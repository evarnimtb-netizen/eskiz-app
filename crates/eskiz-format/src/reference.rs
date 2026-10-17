//! The dense writer that `pack` replaced, kept as the baseline the tests compare against.

use std::borrow::Cow;

use crate::document::{down16, premultiply, DocumentBuilder, LayerSpec, Node};
use crate::pack::{CompositeOut, LayerOut, THUMBNAIL_MAX};
use crate::codecs::apple_lz4_compress;
use crate::{Error, Result};

/// Premultiplied `over`: `dst = src * opacity + dst * (1 - src_alpha * opacity)`.
fn over(dst: &mut [u8], src: &[u8], opacity: f64) {
    let k = opacity.clamp(0.0, 1.0) as f32;
    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0.iter()) {
        let inv = 1.0 - f32::from(s[3]) / 255.0 * k;
        for (dc, sc) in d.iter_mut().zip(s.iter()) {
            let v = f32::from(*sc) * k + f32::from(*dc) * inv;
            *dc = v.round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Flattens the visible layers (normal blend, layer and group opacity) into a premultiplied
/// canvas. Blend modes, clipping and lock transparency are not applied.
fn composite_tree(nodes: &[Node], w: usize, h: usize) -> Vec<u8> {
    let mut acc = vec![0u8; w * h * 4];
    // the tree is stored top-most first, so paint from the end
    for node in nodes.iter().rev() {
        match node {
            Node::Layer(layer) => {
                if layer.hidden {
                    continue;
                }
                if let Some(px) = &layer.pixels {
                    over(&mut acc, px, layer.opacity);
                } else if let Some(px16) = &layer.pixels16 {
                    let px: Vec<u8> = px16.iter().map(|&v| down16(v)).collect();
                    over(&mut acc, &px, layer.opacity);
                }
            }
            Node::Group(group) => {
                if !group.hidden {
                    let inner = composite_tree(&group.children, w, h);
                    over(&mut acc, &inner, group.opacity);
                }
            }
        }
    }
    acc
}


/// PNG thumbnail of the canvas over an optional background, longest side at most 1024 pixels,
/// downscaled with a box filter on premultiplied values.
fn thumbnail_png(canvas: &[u8], w: usize, h: usize, background: Option<[f32; 4]>) -> Result<Vec<u8>> {
    let base = match background {
        Some(bg) => {
            let px = premultiply(bg);
            let mut buf: Vec<u8> = px.iter().cycle().take(w * h * 4).copied().collect();
            over(&mut buf, canvas, 1.0);
            buf
        }
        None => canvas.to_vec(),
    };
    let longest = w.max(h);
    let (ow, oh) = if longest <= THUMBNAIL_MAX {
        (w, h)
    } else {
        let scale = THUMBNAIL_MAX as f64 / longest as f64;
        (
            ((w as f64 * scale).round() as usize).max(1),
            ((h as f64 * scale).round() as usize).max(1),
        )
    };
    let mut out = vec![0u8; ow * oh * 4];
    for oy in 0..oh {
        let (y0, y1) = (oy * h / oh, ((oy + 1) * h / oh).max(oy * h / oh + 1));
        for ox in 0..ow {
            let (x0, x1) = (ox * w / ow, ((ox + 1) * w / ow).max(ox * w / ow + 1));
            let mut sum = [0u64; 4];
            let mut count = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let o = (y * w + x) * 4;
                    for (s, b) in sum.iter_mut().zip(&base[o..o + 4]) {
                        *s += u64::from(*b);
                    }
                    count += 1;
                }
            }
            let avg = sum.map(|s| s as f64 / count as f64);
            let alpha = avg[3] / 255.0;
            let o = (oy * ow + ox) * 4;
            for c in 0..3 {
                out[o + c] = if alpha > 0.0 {
                    (avg[c] / alpha).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                };
            }
            out[o + 3] = avg[3].round() as u8;
        }
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, ow as u32, oh as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(encode_error)?;
        writer.write_image_data(&out).map_err(encode_error)?;
    }
    Ok(png_bytes)
}

fn encode_error(e: impl std::fmt::Display) -> Error {
    Error::Encode(e.to_string())
}


/// A layer's pixels at the depth being written.
pub(crate) enum Pixels<'a> {
    U8(Cow<'a, [u8]>),
    U16(Cow<'a, [u16]>),
}

/// Bounding box `(x0, y0, x1, y1)` of the non-transparent pixels, or `None` if there are none.
pub(crate) fn content_rect<T: Copy + Default + PartialEq>(px: &[T], w: usize) -> Option<(usize, usize, usize, usize)> {
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
        if p.iter().any(|&b| b != T::default()) {
            let (x, y) = (i % w, i / w);
            bounds = Some(match bounds {
                None => (x, y, x + 1, y + 1),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
            });
        }
    }
    bounds
}

/// The pixels of a dense layer at `bits` per channel.
pub(crate) fn source(spec: &LayerSpec, bits: u32) -> Option<Pixels<'_>> {
    if bits == 16 {
        match (&spec.pixels16, &spec.pixels) {
            (Some(p), _) => Some(Pixels::U16(Cow::Borrowed(p))),
            (None, Some(p)) => Some(Pixels::U16(Cow::Owned(p.iter().map(|&v| u16::from(v) * 257).collect()))),
            (None, None) => None,
        }
    } else {
        match (&spec.pixels, &spec.pixels16) {
            (Some(p), _) => Some(Pixels::U8(Cow::Borrowed(p))),
            (None, Some(p)) => Some(Pixels::U8(Cow::Owned(p.iter().map(|&v| down16(v)).collect()))),
            (None, None) => None,
        }
    }
}

/// Writes one `<col>~<row>.lz4` per tile that is not fully transparent.
pub(crate) fn write_tiles(uuid: &str, px: &Pixels, w: usize, h: usize, t: usize, files: &mut Vec<(String, Vec<u8>)>) {
    for row in 0..h.div_ceil(t) {
        for col in 0..w.div_ceil(t) {
            let (x0, y0) = (col * t, row * t);
            let (tw, th) = (t.min(w - x0), t.min(h - y0));
            let buf: Vec<u8> = match px {
                Pixels::U8(p) => {
                    let mut buf = Vec::with_capacity(tw * th * 4);
                    for y in 0..th {
                        let o = ((y0 + y) * w + x0) * 4;
                        buf.extend_from_slice(&p[o..o + tw * 4]);
                    }
                    buf
                }
                Pixels::U16(p) => {
                    let mut buf = Vec::with_capacity(tw * th * 8);
                    for y in 0..th {
                        let o = ((y0 + y) * w + x0) * 4;
                        buf.extend(p[o..o + tw * 4].iter().flat_map(|v| v.to_le_bytes()));
                    }
                    buf
                }
            };
            if buf.iter().all(|&b| b == 0) {
                continue;
            }
            files.push((format!("{uuid}/{col}~{row}.lz4"), apple_lz4_compress(&buf)));
        }
    }
}

fn layer_out(spec: &LayerSpec, uuid: &str, w: usize, h: usize, t: usize, bits: u32) -> LayerOut {
    let source = source(spec, bits);
    let content = source.as_ref().and_then(|px| match px {
        Pixels::U8(p) => content_rect(p, w),
        Pixels::U16(p) => content_rect(p, w),
    });
    let mut out = LayerOut { files: Vec::new(), content };
    if let Some(px) = &source {
        write_tiles(uuid, px, w, h, t, &mut out.files);
    }
    out
}

/// The tile pass of the dense writer: every layer from its dense buffer, the composite and the
/// thumbnail from canvas-sized buffers.
pub(crate) fn tiles(pass: &crate::document::TilePass<'_>) -> Result<(Vec<LayerOut>, CompositeOut)> {
    let g = pass.grid;
    let layers: Vec<LayerOut> = pass.layers.iter().map(|(spec, uuid)| layer_out(spec, uuid, g.w, g.h, g.t, pass.bits)).collect();
    let composite_px = composite_tree(pass.tree, g.w, g.h);
    let layer = pass.write_composite.then(|| {
        let spec = LayerSpec { pixels: Some(composite_px.clone()), ..LayerSpec::default() };
        layer_out(&spec, pass.composite_uuid, g.w, g.h, g.t, pass.bits)
    });
    let thumbnail = thumbnail_png(&composite_px, g.w, g.h, pass.background)?;
    Ok((layers, CompositeOut { layer, thumbnail }))
}

impl DocumentBuilder {
    /// [`DocumentBuilder::to_bytes`] by the dense writer, with a given composite UUID.
    pub(crate) fn to_bytes_reference(&self, composite_uuid: &str) -> Result<Vec<u8>> {
        self.build(Some(composite_uuid.to_owned()), tiles)
    }

    /// [`DocumentBuilder::to_bytes`] by the tile writer, with a given composite UUID.
    pub(crate) fn to_bytes_with_uuid(&self, composite_uuid: &str) -> Result<Vec<u8>> {
        self.build(Some(composite_uuid.to_owned()), |pass| {
            let layers = crate::pack::encode_layers(&pass.layers, &pass.grid, pass.bits);
            let composite = crate::pack::composite_and_thumbnail(
                pass.tree,
                &pass.grid,
                pass.bits,
                pass.write_composite.then_some(pass.composite_uuid),
                pass.background,
            )?;
            Ok((layers, composite))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::document::{GroupSpec, Node};
    use crate::pack::{LayerTiles, TileSource};

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn chance(&mut self, percent: usize) -> bool {
            self.below(100) < percent
        }
    }

    /// Tiles in a map; absent keys are transparent.
    struct MapSource {
        edge: usize,
        bits: u32,
        t8: HashMap<(usize, usize), Vec<u8>>,
        t16: HashMap<(usize, usize), Vec<u16>>,
    }

    impl TileSource for MapSource {
        fn edge(&self) -> usize {
            self.edge
        }
        fn bits(&self) -> u32 {
            self.bits
        }
        fn tile8(&self, col: usize, row: usize) -> Option<&[u8]> {
            self.t8.get(&(col, row)).map(Vec::as_slice)
        }
        fn tile16(&self, col: usize, row: usize) -> Option<&[u16]> {
            self.t16.get(&(col, row)).map(Vec::as_slice)
        }
    }

    /// A random tiled layer and the dense in-bounds pixels it stands for.
    struct RandomLayer {
        source: MapSource,
        dense8: Vec<u8>,
        dense16: Vec<u16>,
    }

    fn random_layer(rng: &mut Rng, w: usize, h: usize, edge: usize, bits: u32) -> RandomLayer {
        let mut source = MapSource { edge, bits, t8: HashMap::new(), t16: HashMap::new() };
        let (cols, rows) = (w.div_ceil(edge), h.div_ceil(edge));
        let density = [0, 15, 50, 90][rng.below(4)];
        let mut dense8 = vec![0u8; w * h * 4];
        let mut dense16 = vec![0u16; w * h * 4];
        for row in 0..rows {
            for col in 0..cols {
                if !rng.chance(100 - density) {
                    // allocated; maybe all zero, maybe sparse, maybe full
                    let kind = rng.below(10);
                    let mut t = vec![0u16; edge * edge * 4];
                    let (x0, y0) = (col * edge, row * edge);
                    let (tw, th) = (edge.min(w - x0), edge.min(h - y0));
                    match kind {
                        0 | 1 => {} // allocated but all zero
                        2..=5 => {
                            // a patch
                            let (px, py) = (rng.below(tw), rng.below(th));
                            let (pw, ph) = (1 + rng.below(tw - px), 1 + rng.below(th - py));
                            for y in py..py + ph {
                                for x in px..px + pw {
                                    for c in 0..4 {
                                        t[(y * edge + x) * 4 + c] = rng.next() as u16;
                                    }
                                }
                            }
                        }
                        _ => {
                            for y in 0..th {
                                for x in 0..tw {
                                    // runs of equal pixels make the LZ4 side interesting
                                    let v = if rng.chance(30) { 0 } else { rng.next() as u16 };
                                    for c in 0..4 {
                                        t[(y * edge + x) * 4 + c] = if c == 3 && v != 0 { 0xFFFF } else { v };
                                    }
                                }
                            }
                        }
                    }
                    // in-bounds pixels go to the dense copies
                    for y in 0..th {
                        for x in 0..tw {
                            for c in 0..4 {
                                let v = t[(y * edge + x) * 4 + c];
                                let o = ((y0 + y) * w + x0 + x) * 4 + c;
                                if bits == 8 {
                                    dense8[o] = v as u8;
                                } else {
                                    dense16[o] = v;
                                }
                            }
                        }
                    }
                    // garbage beyond the canvas edge must be ignored
                    if rng.chance(40) {
                        for y in 0..edge {
                            for x in 0..edge {
                                if x >= tw || y >= th {
                                    for c in 0..4 {
                                        t[(y * edge + x) * 4 + c] = 1 + rng.below(255) as u16;
                                    }
                                }
                            }
                        }
                    }
                    if bits == 8 {
                        source.t8.insert((col, row), t.iter().map(|&v| v as u8).collect());
                    } else {
                        source.t16.insert((col, row), t);
                    }
                }
            }
        }
        RandomLayer { source, dense8, dense16 }
    }

    /// A pair of builders with the same pictures: one with tile sources, one with dense buffers.
    fn random_builders(rng: &mut Rng, case: usize) -> (DocumentBuilder, DocumentBuilder) {
        let big = case.is_multiple_of(17);
        let edge = if case % 5 == 4 { [32, 64, 100][rng.below(3)] } else { 256 };
        let (w, h) = if big {
            (1030 + rng.below(1500), 1030 + rng.below(900))
        } else if edge == 256 {
            [(1, 1), (255, 255), (256, 256), (257, 1), (1, 300), (513, 270)][rng.below(6)]
        } else {
            (1 + rng.below(330), 1 + rng.below(250))
        };
        let (w, h) = if edge == 256 && !big && rng.chance(50) { (1 + rng.below(700), 1 + rng.below(600)) } else { (w, h) };
        let bits = if rng.chance(50) { 8 } else { 16 };
        let mut tiled = DocumentBuilder::new(w as u32, h as u32);
        tiled.tile_size = edge as u32;
        tiled.bits_per_channel = bits;
        tiled.write_eskiz_keys = bits == 8 || rng.chance(70);
        tiled.background_hidden = rng.chance(30);
        tiled.background_color = [rng.below(256) as f32 / 255.0, 0.5, rng.below(256) as f32 / 255.0, [1.0, 0.5, 0.0][rng.below(3)]];
        let mut dense = tiled.clone();
        let mut counter = 0usize;
        let layers = 1 + rng.below(if big { 3 } else { 6 });
        let mut specs: Vec<(Node, Node)> = Vec::new();
        for _ in 0..layers {
            specs.push(random_node(rng, w, h, edge, bits, 2, &mut counter, big));
        }
        // random grouping: wrap a run of nodes in a group
        let pairs = group_some(rng, specs);
        tiled.tree = pairs.iter().map(|p| p.0.clone()).collect();
        dense.tree = pairs.into_iter().map(|p| p.1).collect();
        (tiled, dense)
    }

    fn group_some(rng: &mut Rng, nodes: Vec<(Node, Node)>) -> Vec<(Node, Node)> {
        if nodes.len() >= 2 && rng.chance(60) {
            let n = nodes.len();
            let take = 1 + rng.below(n);
            let (inner, rest): (Vec<_>, Vec<_>) = {
                let mut v = nodes;
                let rest = v.split_off(take);
                (v, rest)
            };
            let opacity = [0.0, 1.0, 0.5, 0.37][rng.below(4)];
            let hidden = rng.chance(15);
            let g = |children: Vec<Node>| Node::Group(GroupSpec { name: "g".into(), opacity, hidden, children, ..GroupSpec::default() });
            let tiled_g = g(inner.iter().map(|p| p.0.clone()).collect());
            let dense_g = g(inner.into_iter().map(|p| p.1).collect());
            let mut out = vec![(tiled_g, dense_g)];
            out.extend(rest);
            out
        } else {
            nodes
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn random_node(rng: &mut Rng, w: usize, h: usize, edge: usize, bits: u32, depth: usize, counter: &mut usize, big: bool) -> (Node, Node) {
        if depth > 0 && rng.chance(20) {
            let kids: Vec<(Node, Node)> = (0..1 + rng.below(3)).map(|_| random_node(rng, w, h, edge, bits, depth - 1, counter, big)).collect();
            let opacity = [0.0, 1.0, 0.5, 0.81][rng.below(4)];
            let hidden = rng.chance(15);
            let g = |children: Vec<Node>| Node::Group(GroupSpec { name: "g".into(), opacity, hidden, children, ..GroupSpec::default() });
            return (g(kids.iter().map(|p| p.0.clone()).collect()), g(kids.into_iter().map(|p| p.1).collect()));
        }
        *counter += 1;
        let base = LayerSpec {
            name: format!("L{counter}"),
            uuid: Some(format!("LAYER-{counter}")),
            opacity: [0.0, 1.0, 1.0, 0.5, 0.123, 0.9][rng.below(6)],
            hidden: rng.chance(10),
            ..LayerSpec::default()
        };
        let kind = rng.below(10);
        // the layer's own tile depth: usually the archive's, sometimes the other
        let src_bits = if rng.chance(75) { bits } else { 24 - bits };
        let (mut a, mut b) = (base.clone(), base);
        match kind {
            0 => {} // no pixels at all
            1 => {
                // dense only, both buffers possibly
                let px8: Vec<u8> = (0..w * h * 4).map(|_| if rng.chance(60) { 0 } else { rng.next() as u8 }).collect();
                let px16: Vec<u16> = (0..w * h * 4).map(|_| if rng.chance(60) { 0 } else { rng.next() as u16 }).collect();
                for s in [&mut a, &mut b] {
                    match rng.below(3) {
                        0 => s.pixels = Some(px8.clone()),
                        1 => s.pixels16 = Some(px16.clone()),
                        _ => {
                            s.pixels = Some(px8.clone());
                            s.pixels16 = Some(px16.clone());
                        }
                    }
                }
                // both builders of the pair must use the same dense variant
                b.pixels = a.pixels.clone();
                b.pixels16 = a.pixels16.clone();
            }
            _ => {
                let layer = random_layer(rng, w, h, edge, src_bits);
                if src_bits == 8 {
                    b.pixels = Some(layer.dense8.clone());
                } else {
                    b.pixels16 = Some(layer.dense16.clone());
                }
                a.tiles = Some(LayerTiles::new(layer.source));
            }
        }
        if big {
            // keep the dense reference cheap: big canvases get sparse content only
            if a.pixels.is_some() || a.pixels16.is_some() {
                a.pixels = None;
                a.pixels16 = None;
                b.pixels = None;
                b.pixels16 = None;
            }
        }
        (Node::Layer(a), Node::Layer(b))
    }

    #[test]
    fn tile_writer_matches_the_dense_writer_byte_for_byte() {
        let mut rng = Rng(0xEA5E1);
        let mut members = 0usize;
        for case in 0..160 {
            let (tiled, dense) = random_builders(&mut rng, case);
            let uuid = "COMPOSITE-UUID";
            let reference = dense.to_bytes_reference(uuid).unwrap();
            let from_dense = dense.to_bytes_with_uuid(uuid).unwrap();
            let from_tiles = tiled.to_bytes_with_uuid(uuid).unwrap();
            let what = format!(
                "case {case}: {}x{} edge {} bits {} keys {} bg_hidden {}",
                tiled.width, tiled.height, tiled.tile_size, tiled.bits_per_channel, tiled.write_eskiz_keys, tiled.background_hidden
            );
            assert!(reference == from_dense, "dense: {what}");
            assert!(reference == from_tiles, "tiles: {what}");
            members += reference.windows(4).filter(|w| w == b".lz4").count();
        }
        // each member name occurs in the local header and the central directory
        assert!(members > 2000, "the random documents should hold plenty of tiles ({members})");
    }

    #[test]
    fn tile_writer_output_reads_back() {
        // A package from tile sources opens and decodes to the dense pixels.
        let mut rng = Rng(7);
        let (w, h) = (300usize, 270usize);
        let layer = random_layer(&mut rng, w, h, 256, 8);
        let mut b = DocumentBuilder::new(w as u32, h as u32);
        b.tree = vec![Node::Layer(LayerSpec {
            uuid: Some("A".into()),
            tiles: Some(LayerTiles::new(layer.source)),
            ..LayerSpec::default()
        })];
        let bytes = b.to_bytes().unwrap();
        let mut doc = crate::Document::from_reader(std::io::Cursor::new(bytes)).unwrap();
        let l = doc.layers[0].clone();
        assert_eq!(doc.layer_rgba(&l).unwrap(), layer.dense8);
    }

    #[test]
    fn tile_source_with_the_wrong_edge_is_refused() {
        let src = MapSource { edge: 128, bits: 8, t8: HashMap::new(), t16: HashMap::new() };
        let mut b = DocumentBuilder::new(300, 300);
        b.tree = vec![Node::Layer(LayerSpec { tiles: Some(LayerTiles::new(src)), ..LayerSpec::default() })];
        assert!(matches!(b.to_bytes(), Err(Error::InvalidInput(_))));
    }

    #[test]
    fn thumbnail_boxes_partition_the_canvas() {
        use crate::pack::Thumb;
        for (w, h) in [(1usize, 1usize), (1030, 7), (2048, 2048), (4096, 1000), (1025, 1025), (3000, 1), (8000, 8000)] {
            let t = Thumb::new(w, h, [0; 4]);
            let _ = &t; // construction checks the maps are well formed
            let (ow, oh) = crate::pack::thumbnail_size(w, h);
            assert!(ow <= w && oh <= h);
            for (len, n) in [(w, ow), (h, oh)] {
                let mut next_start = 0;
                for o in 0..n {
                    let start = o * len / n;
                    let end = ((o + 1) * len / n).max(start + 1);
                    assert_eq!(start, next_start, "boxes tile the axis");
                    next_start = end;
                }
                assert_eq!(next_start, len);
            }
        }
    }
}
