//! Maps an `eskiz-format` document onto an Eskiz [`Document`], and decodes PNG and JPEG
//! pictures so they can be imported as a new layer ([`decode_image`], [`fit_to_canvas`]).

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use eskiz_core::canvas::Layer;
use eskiz_core::document::{Mask, Props};
use eskiz_core::{BlendMode, ColourSpace};
use eskiz_format::{Document as ProcDocument, Group, Item, Layer as ProcLayer, TilePixels};

use crate::document::Document;

/// Open a `.procreate` file. The layer tree is kept: groups (name, opacity, hidden, collapsed),
/// layer pixels, opacity, hidden, locked, clipped, lock transparency, private, blend mode, layer masks
/// and the background colour. Text layers are not rendered, and a group's blend mode, mask and
/// lock are not stored by the format, so they take their defaults.
pub fn open_procreate(path: &Path) -> Result<Document, String> {
    open_procreate_with(path, true)
}

/// [`open_procreate`]. With `direct` set, layers whose tiles match the engine's are decoded
/// straight into tiles; without it every layer goes through a canvas-sized buffer (the
/// reference the tests compare the direct path against).
fn open_procreate_with(path: &Path, direct: bool) -> Result<Document, String> {
    let mut src = ProcDocument::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (src.width as usize, src.height as usize);
    if w == 0 || h == 0 {
        return Err(format!("{}: empty canvas", path.display()));
    }
    let mut doc = Document::empty(w, h);
    doc.dpi = src.dpi.unwrap_or(doc.dpi);
    // The stored profile name decides the colour space; a file with none is sRGB.
    doc.colour_space = src.color_profile.as_deref().map(ColourSpace::from_profile_name).unwrap_or_default();
    // 16-bit archives open as 16-bit documents.
    if src.bits_per_channel().map_err(|e| format!("{}: {e}", path.display()))? == 16 {
        doc.depth = eskiz_core::Depth::U16;
    }
    let tree = std::mem::take(&mut src.tree);
    add_items(&mut doc, &mut src, None, &tree, direct)?;
    if doc.layer_count() == 0 {
        doc.add_layer_above(None);
    }
    if let Some([r, g, b, _]) = src.background_color {
        doc.background = [r, g, b].map(|v| f64::from(v).clamp(0.0, 1.0));
    }
    doc.background_visible = !src.background_hidden;
    if let Some(spec) = src.animation {
        doc.animation = crate::animation_ui::settings_from_spec(&spec);
        if spec.last_item_is_background {
            if let Some(bottom) = doc.root.first_mut() {
                bottom.props.anim_background = true;
            }
        }
        if spec.first_item_is_foreground {
            if let Some(top) = doc.root.last_mut() {
                top.props.anim_foreground = true;
            }
        }
    }
    Ok(doc)
}

fn extension_lower(path: &Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
}

/// Is this a picture Eskiz can import as a layer (by extension)?
pub fn is_image_file(path: &Path) -> bool {
    matches!(extension_lower(path).as_deref(), Some("png" | "jpg" | "jpeg" | "webp"))
}

/// Is this a layered file whose layers Eskiz can import (PSD or `.procreate`)?
pub fn is_layered_file(path: &Path) -> bool {
    matches!(extension_lower(path).as_deref(), Some("psd" | "procreate"))
}

/// Is this a file the Import action and drag-and-drop accept?
pub fn is_import_file(path: &Path) -> bool {
    is_image_file(path) || is_layered_file(path)
}

/// Decode a PNG, JPEG or WebP file to straight (unpremultiplied) RGBA8. The format is decided by
/// the file's first bytes, not its extension. Returns `(width, height, pixels)`.
pub fn decode_image(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let mut head = [0u8; 12];
    let n = File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let head = &head[..n];
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        decode_png(path)
    } else if head.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(path)
    } else if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        decode_webp(path)
    } else {
        Err(format!("{}: not a PNG, JPEG or WebP picture", path.display()))
    }
}

fn decode_webp(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let reader = image::ImageReader::open(path).map_err(|e| err(&e))?;
    let rgba = reader.decode().map_err(|e| err(&e))?.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    Ok((w, h, rgba.into_raw()))
}

/// A layer read from a file: its name and premultiplied RGBA8 pixels at the file's canvas size.
#[derive(Debug, Clone, PartialEq)]
pub struct FileLayer {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// The layers of a PSD or `.procreate` file, bottom to top. Groups are flattened into the list, and
/// hidden layers are included.
pub fn read_layers(path: &Path) -> Result<Vec<FileLayer>, String> {
    use eskiz_format::psd::PsdNode;
    match extension_lower(path).as_deref() {
        Some("psd") => {
            let doc = eskiz_format::psd::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            fn walk(nodes: &[PsdNode], w: usize, h: usize, out: &mut Vec<FileLayer>) {
                for node in nodes {
                    match node {
                        PsdNode::Layer(layer) => out.push(FileLayer {
                            name: layer.name.clone(),
                            width: w,
                            height: h,
                            pixels: layer.pixels.clone(),
                        }),
                        PsdNode::Group(group) => walk(&group.children, w, h, out),
                    }
                }
            }
            let mut out = Vec::new();
            walk(&doc.root, doc.width as usize, doc.height as usize, &mut out);
            Ok(out)
        }
        Some("procreate") => {
            let doc = open_procreate(path)?;
            fn walk(doc: &Document, nodes: &[eskiz_core::Node], out: &mut Vec<FileLayer>) {
                for node in nodes {
                    match &node.kind {
                        eskiz_core::NodeKind::Layer => {
                            if let Some(layer) = doc.layer(node.id) {
                                out.push(FileLayer {
                                    name: node.props.name.clone(),
                                    width: doc.width,
                                    height: doc.height,
                                    pixels: layer.to_rgba(),
                                });
                            }
                        }
                        eskiz_core::NodeKind::Group { children, .. } => walk(doc, children, out),
                    }
                }
            }
            let mut out = Vec::new();
            walk(&doc, &doc.root, &mut out);
            Ok(out)
        }
        _ => Err(format!("{}: not a PSD or .procreate file", path.display())),
    }
}

fn decode_png(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image is too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let data = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err(format!("{}: palette image was not expanded", path.display())),
    };
    if rgba.len() != w * h * 4 {
        return Err(format!("{}: unexpected pixel data", path.display()));
    }
    Ok((w, h, rgba))
}

fn decode_jpeg(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(&bytes), options);
    let rgba = decoder.decode().map_err(|e| format!("{}: {e:?}", path.display()))?;
    let info = decoder.info().ok_or_else(|| format!("{}: no image header", path.display()))?;
    let (w, h) = (usize::from(info.width), usize::from(info.height));
    if rgba.len() != w * h * 4 {
        return Err(format!("{}: unsupported JPEG color layout", path.display()));
    }
    Ok((w, h, rgba))
}

/// Place a picture on a transparent `cw x ch` canvas, centred. A picture larger than the canvas
/// is scaled down to fit (aspect kept, area averaging); a smaller one keeps its size. `rgba` is
/// straight RGBA8; the result is premultiplied, ready to be a layer's pixels.
pub fn fit_to_canvas(rgba: &[u8], iw: usize, ih: usize, cw: usize, ch: usize) -> Vec<u8> {
    if rgba.len() != iw * ih * 4 {
        return vec![0u8; cw * ch * 4];
    }
    fit_premul(&crate::export::premultiply(rgba), iw, ih, cw, ch)
}

/// [`fit_to_canvas`] for premultiplied pixels, such as a PSD or `.procreate` layer.
pub fn fit_premul(premul: &[u8], iw: usize, ih: usize, cw: usize, ch: usize) -> Vec<u8> {
    let mut out = vec![0u8; cw * ch * 4];
    if iw == 0 || ih == 0 || cw == 0 || ch == 0 || premul.len() != iw * ih * 4 {
        return out;
    }
    let scale = (cw as f64 / iw as f64).min(ch as f64 / ih as f64).min(1.0);
    let (nw, nh) = (((iw as f64 * scale).round() as usize).clamp(1, cw), ((ih as f64 * scale).round() as usize).clamp(1, ch));
    let fitted = if (nw, nh) == (iw, ih) { premul.to_vec() } else { resize_area(premul, iw, ih, nw, nh) };
    let (ox, oy) = ((cw - nw) / 2, (ch - nh) / 2);
    for y in 0..nh {
        let dst = ((oy + y) * cw + ox) * 4;
        out[dst..dst + nw * 4].copy_from_slice(&fitted[y * nw * 4..(y + 1) * nw * 4]);
    }
    out
}

/// Area-averaging resize of premultiplied RGBA8 (exact box weights, separable).
fn resize_area(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    // Weights of each source index for one destination index along an axis.
    fn spans(s: usize, d: usize) -> Vec<Vec<(usize, f32)>> {
        let ratio = s as f64 / d as f64;
        (0..d)
            .map(|i| {
                let (a, b) = (i as f64 * ratio, (i + 1) as f64 * ratio);
                let (first, last) = (a.floor() as usize, ((b.ceil() as usize).min(s)).max(a.floor() as usize + 1));
                (first..last)
                    .map(|k| {
                        let w = (b.min((k + 1) as f64) - a.max(k as f64)).max(0.0);
                        (k, (w / ratio) as f32)
                    })
                    .collect()
            })
            .collect()
    }
    let (xs, ys) = (spans(sw, dw), spans(sh, dh));
    let mut tmp = vec![0.0f32; dw * sh * 4];
    for y in 0..sh {
        for (x, taps) in xs.iter().enumerate() {
            let o = (y * dw + x) * 4;
            for &(k, w) in taps {
                let i = (y * sw + k) * 4;
                for c in 0..4 {
                    tmp[o + c] += w * f32::from(src[i + c]);
                }
            }
        }
    }
    let mut out = vec![0u8; dw * dh * 4];
    for (y, taps) in ys.iter().enumerate() {
        for x in 0..dw {
            let mut acc = [0.0f32; 4];
            for &(k, w) in taps {
                let i = (k * dw + x) * 4;
                for c in 0..4 {
                    acc[c] += w * tmp[i + c];
                }
            }
            let o = (y * dw + x) * 4;
            for c in 0..4 {
                out[o + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Add `items` (stored top-most first) under `parent`, bottom to top.
fn add_items(
    doc: &mut Document,
    src: &mut ProcDocument,
    parent: Option<u64>,
    items: &[Item],
    direct: bool,
) -> Result<(), String> {
    for item in items.iter().rev() {
        match item {
            Item::Layer(layer) => add_layer(doc, src, parent, layer, direct)?,
            Item::Group(group) => add_group(doc, src, parent, group, direct)?,
        }
    }
    Ok(())
}

fn add_group(doc: &mut Document, src: &mut ProcDocument, parent: Option<u64>, group: &Group, direct: bool) -> Result<(), String> {
    let n = doc.layer_count() + 1;
    let mut props = Props::new(if group.name.is_empty() {
        format!("Group {n}")
    } else {
        group.name.clone()
    });
    props.opacity = group.opacity.clamp(0.0, 1.0);
    props.visible = !group.hidden;
    props.hold = group.animation_hold;
    let id = doc.push_group(parent, props, None);
    if let Some(eskiz_core::Node {
        kind: eskiz_core::NodeKind::Group { collapsed, .. },
        ..
    }) = doc.node_mut(id)
    {
        *collapsed = group.collapsed;
    }
    add_items(doc, src, Some(id), &group.children, direct)
}

fn add_layer(doc: &mut Document, src: &mut ProcDocument, parent: Option<u64>, layer: &ProcLayer, direct: bool) -> Result<(), String> {
    let (w, h) = (doc.width, doc.height);
    // A layer without tiles is blank: do not assemble a canvas-sized buffer of zeros for it.
    let pixels = if src.tile_count(&layer.uuid) == 0 {
        Layer::new(w, h)
    } else {
        let (lw, lh) = (layer.width as usize, layer.height as usize);
        if direct && (lw, lh) == (w, h) && src.tile_size as usize == eskiz_core::TILE {
            // Decode the tiles in parallel straight into the layer: no canvas-sized buffer, and
            // no rescan for empty tiles. The layer has the document's depth, as `from_rgba` /
            // `from_rgba16` below would give.
            let depth = doc.depth;
            let tiles = src
                .layer_tiles(layer, depth.bits())
                .map_err(|e| format!("layer '{}': {e}", layer.name))?;
            let mut pixels = Layer::new_depth(w, h, depth);
            for t in tiles {
                let tile = match t.pixels {
                    TilePixels::U8(d) => eskiz_core::Tile::U8(d),
                    TilePixels::U16(d) => eskiz_core::Tile::U16(d),
                };
                pixels.set_tile(t.col, t.row, Some(std::sync::Arc::new(tile)));
            }
            pixels
        } else if doc.depth == eskiz_core::Depth::U16 && (lw, lh) == (w, h) {
            // Exact 16-bit samples, no trip through 8 bits.
            let rgba = src
                .layer_rgba16(layer)
                .map_err(|e| format!("layer '{}': {e}", layer.name))?;
            Layer::from_rgba16(w, h, rgba)
        } else {
            let rgba = src
                .layer_rgba(layer)
                .map_err(|e| format!("layer '{}': {e}", layer.name))?;
            if (lw, lh) == (w, h) {
                Layer::from_rgba(w, h, rgba)
            } else {
                Layer::from_rgba(w, h, place(&rgba, lw, lh, w, h, 0))
            }
        }
    };
    let mask = match src
        .mask_rgba(layer)
        .map_err(|e| format!("mask of layer '{}': {e}", layer.name))?
    {
        Some(m) => {
            let mw = layer.mask.as_ref().map_or(w, |m| m.width as usize);
            let mh = layer.mask.as_ref().map_or(h, |m| m.height as usize);
            // Mask value from the red channel (the mask is grey). Where the mask tile does not
            // reach the canvas, the mask shows the layer (255).
            let placed = place(&m, mw, mh, w, h, 255);
            Some(Mask {
                data: placed.as_chunks::<4>().0.iter().map(|p| p[0]).collect(),
            })
        }
        None => None,
    };
    let mut props = Props::new(if layer.name.is_empty() {
        format!("Layer {}", doc.layer_count() + 1)
    } else {
        layer.name.clone()
    });
    props.visible = !layer.hidden;
    props.opacity = layer.opacity.clamp(0.0, 1.0);
    props.blend = layer.blend_mode.and_then(BlendMode::from_index).unwrap_or_default();
    props.clipped = layer.clipped;
    props.alpha_lock = layer.alpha_locked;
    props.locked = layer.locked;
    props.private = layer.private;
    props.hold = layer.animation_hold;
    let id = doc.push_raster(parent, props, mask, pixels);
    if let Some(text) = layer.text.as_ref().and_then(crate::text_tool::from_spec) {
        doc.texts.insert(id, text);
    }
    Ok(())
}

/// Copy a `sw x sh` RGBA buffer to the origin of a `w x h` buffer filled with `fill`.
/// Anything outside the canvas is dropped.
fn place(src: &[u8], sw: usize, sh: usize, w: usize, h: usize, fill: u8) -> Vec<u8> {
    let mut out = vec![fill; w * h * 4];
    let copy_w = w.min(sw);
    for row in 0..h.min(sh) {
        let dst = row * w * 4;
        let from = row * sw * 4;
        out[dst..dst + copy_w * 4].copy_from_slice(&src[from..from + copy_w * 4]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_flags_and_background_are_imported() {
        use eskiz_format::{DocumentBuilder, GroupSpec, LayerSpec, Node as N};
        let mut b = DocumentBuilder::new(8, 8);
        b.background_color = [0.5, 0.25, 0.0, 1.0];
        b.background_hidden = true;
        let px = vec![255u8; 8 * 8 * 4];
        // Top-most first, as stored.
        b.tree = vec![
            N::Layer(LayerSpec {
                name: "top".into(),
                blend_mode: 1,
                clipped: true,
                alpha_locked: true,
                opacity: 0.5,
                pixels: Some(px.clone()),
                ..LayerSpec::default()
            }),
            N::Group(GroupSpec {
                name: "grp".into(),
                opacity: 0.25,
                collapsed: true,
                children: vec![
                    N::Layer(LayerSpec { name: "inner top".into(), hidden: true, private: true, ..LayerSpec::default() }),
                    N::Layer(LayerSpec { name: "inner bottom".into(), locked: true, pixels: Some(px), ..LayerSpec::default() }),
                ],
                ..GroupSpec::default()
            }),
        ];
        let dir = std::env::temp_dir().join(format!("eskiz-import-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.procreate");
        eskiz_format::document::write(&path, &b).unwrap();
        let doc = open_procreate(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(doc.root.len(), 2);
        let g = &doc.root[0];
        assert_eq!(g.props.name, "grp");
        assert_eq!(g.props.opacity, 0.25);
        assert!(matches!(g.kind, eskiz_core::NodeKind::Group { collapsed: true, .. }));
        let names: Vec<&str> = g.children().iter().map(|n| n.props.name.as_str()).collect();
        assert_eq!(names, ["inner bottom", "inner top"], "bottom to top");
        assert!(g.children()[0].props.locked);
        assert!(g.children()[1].props.private && !g.children()[1].props.visible);
        let top = &doc.root[1];
        assert_eq!(top.props.blend, BlendMode::Multiply);
        assert!(top.props.clipped && top.props.alpha_lock);
        assert_eq!(top.props.opacity, 0.5);
        assert!(!doc.background_visible);
        assert_eq!(doc.background, [0.5, 0.25, 0.0]);
        assert!(doc.layer(top.id).unwrap().to_rgba().iter().all(|&v| v == 255));
    }

    /// A small deterministic generator for the equivalence tests.
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
    }

    /// Premultiplied pixels of a `w x h` canvas with patches here and there. `limit16` keeps
    /// 16-bit samples small (colour at most alpha, at most 0x3C00) so that a keyless file reads
    /// as half floats.
    fn random_pixels(rng: &mut Rng, w: usize, h: usize, wide: bool, half: bool) -> (Vec<u8>, Vec<u16>) {
        let mut p8 = vec![0u8; w * h * 4];
        let mut p16 = vec![0u16; w * h * 4];
        let patches = rng.below(6);
        for _ in 0..patches {
            let (px, py) = (rng.below(w), rng.below(h));
            let (pw, ph) = (1 + rng.below((w - px).min(300)), 1 + rng.below((h - py).min(300)));
            let solid = rng.below(3) == 0;
            for y in py..py + ph {
                for x in px..px + pw {
                    let o = (y * w + x) * 4;
                    if rng.below(8) == 0 {
                        continue; // holes
                    }
                    if wide && half {
                        let a = 1 + rng.below(0x3C00) as u16;
                        for c in 0..3 {
                            p16[o + c] = rng.below(a as usize + 1) as u16;
                        }
                        p16[o + 3] = a;
                    } else if wide {
                        for c in 0..4 {
                            p16[o + c] = if solid { 0xFFFF } else { rng.next() as u16 };
                        }
                    } else {
                        for c in 0..4 {
                            p8[o + c] = if solid { 255 } else { rng.next() as u8 };
                        }
                    }
                }
            }
        }
        (p8, p16)
    }

    fn random_package(rng: &mut Rng, case: usize) -> Vec<u8> {
        use eskiz_format::{DocumentBuilder, GroupSpec, LayerSpec, Node as N};
        let (w, h) = match case % 6 {
            0 => (1 + rng.below(40), 1 + rng.below(40)),
            1 => (256, 256),
            2 => (257, 511),
            _ => (1 + rng.below(700), 1 + rng.below(600)),
        };
        let mut b = DocumentBuilder::new(w as u32, h as u32);
        b.bits_per_channel = if rng.below(2) == 0 { 8 } else { 16 };
        // keyless 16-bit files make the reader detect the sample encoding from the tiles
        let keyless = b.bits_per_channel == 16 && rng.below(3) == 0;
        b.write_eskiz_keys = !keyless;
        let half = keyless && rng.below(2) == 0;
        if case % 7 == 3 {
            b.tile_size = [64, 100, 128][rng.below(3)] as u32; // not the engine's tile: the old path
        }
        let wide = b.bits_per_channel == 16;
        let layer = |rng: &mut Rng, name: &str| {
            let (p8, p16) = random_pixels(rng, w, h, wide, half);
            let mut spec = LayerSpec { name: name.into(), opacity: 0.5, ..LayerSpec::default() };
            match rng.below(5) {
                0 => {} // an empty layer
                _ if wide => spec.pixels16 = Some(p16),
                _ => spec.pixels = Some(p8),
            }
            N::Layer(spec)
        };
        let top = layer(rng, "top");
        let inner_a = layer(rng, "inner a");
        let inner_b = layer(rng, "inner b");
        let bottom = layer(rng, "bottom");
        b.tree = vec![
            top,
            N::Group(GroupSpec { name: "g".into(), children: vec![inner_a, inner_b], ..GroupSpec::default() }),
            bottom,
        ];
        b.to_bytes().unwrap()
    }

    fn same_nodes(a: &Document, b: &Document, x: &[eskiz_core::Node], y: &[eskiz_core::Node], what: &str) {
        assert_eq!(x.len(), y.len(), "{what}");
        for (p, q) in x.iter().zip(y) {
            assert_eq!(p.props.name, q.props.name, "{what}");
            assert_eq!(p.props.opacity, q.props.opacity, "{what}");
            assert_eq!(p.mask.as_ref().map(|m| &m.data), q.mask.as_ref().map(|m| &m.data), "{what}: mask");
            match (&p.kind, &q.kind) {
                (eskiz_core::NodeKind::Layer, eskiz_core::NodeKind::Layer) => {
                    let (l, m) = (a.layer(p.id).unwrap(), b.layer(q.id).unwrap());
                    assert_eq!((l.width, l.height, l.depth()), (m.width, m.height, m.depth()), "{what}: layer shape");
                    assert!(l == m, "{what}: pixels of '{}' differ", p.props.name);
                    assert_eq!(l.tile_count(), m.tile_count(), "{what}: tile count of '{}'", p.props.name);
                    for ty in 0..l.tiles_y() {
                        for tx in 0..l.tiles_x() {
                            assert_eq!(l.tile(tx, ty).is_some(), m.tile(tx, ty).is_some(), "{what}: tile {tx},{ty} of '{}'", p.props.name);
                            // not just equal pixels: the same samples in the same representation
                            assert!(l.tile(tx, ty) == m.tile(tx, ty), "{what}: tile {tx},{ty}");
                        }
                    }
                }
                (eskiz_core::NodeKind::Group { .. }, eskiz_core::NodeKind::Group { .. }) => {
                    same_nodes(a, b, p.children(), q.children(), what);
                }
                _ => panic!("{what}: node kinds differ"),
            }
        }
    }

    #[test]
    fn direct_tile_loader_matches_the_dense_loader() {
        let dir = std::env::temp_dir().join(format!("eskiz-direct-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut rng = Rng(0xD1EC7);
        for case in 0..90 {
            let bytes = random_package(&mut rng, case);
            let path = dir.join(format!("c{case}.procreate"));
            std::fs::write(&path, &bytes).unwrap();
            let direct = open_procreate_with(&path, true).unwrap();
            let dense = open_procreate_with(&path, false).unwrap();
            let what = format!("case {case} ({}x{}, {})", direct.width, direct.height, direct.depth.bits());
            assert_eq!((direct.width, direct.height, direct.depth), (dense.width, dense.height, dense.depth), "{what}");
            same_nodes(&direct, &dense, &direct.root, &dense.root, &what);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn direct_and_dense_loaders_report_the_same_damage() {
        let mut rng = Rng(5);
        let dir = std::env::temp_dir().join(format!("eskiz-direct-damage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut bytes, at) = loop {
            let bytes = random_package(&mut rng, 1);
            if let Some(at) = bytes.windows(4).position(|w| w == b"bv41" || w == b"bv4-") {
                break (bytes, at);
            }
        };
        // damage the first tile's frame magic (a stored member): the zip CRC or the codec objects
        bytes[at + 1] ^= 0x20;
        let path = dir.join("bad.procreate");
        std::fs::write(&path, &bytes).unwrap();
        let direct = open_procreate_with(&path, true);
        let dense = open_procreate_with(&path, false);
        std::fs::remove_dir_all(&dir).ok();
        let (direct, dense) = (direct.expect_err("damage detected"), dense.expect_err("damage detected"));
        assert_eq!(direct, dense);
    }

    /// Timing of the load phases on a 4096 x 4096 painted layer. Run with
    /// `cargo test --release -p eskiz-app --lib load_phase_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing probe, not a check"]
    fn load_phase_timing() {
        use std::time::Instant;
        let n = 4096usize;
        let mut doc = Document::new(n, n);
        let id = doc.layer_id(0).unwrap();
        let mut px = vec![0u8; n * n * 4];
        for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % n, i / n);
            let v = ((x / 7 + y / 5) % 251) as u8;
            p.copy_from_slice(&[v, v / 2, v / 3, 255]);
        }
        doc.layer_mut(id).unwrap().write_rect(0, 0, n, n, &px);
        let dir = std::env::temp_dir().join(format!("eskiz-load-timing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.procreate");
        let t = Instant::now();
        crate::export::export_document(&doc, crate::export::ExportFormat::Procreate, &path).unwrap();
        println!("save {:?}, {} bytes", t.elapsed(), std::fs::metadata(&path).unwrap().len());
        let t = Instant::now();
        let mut src = ProcDocument::open(&path).unwrap();
        println!("open {:?}", t.elapsed());
        let t = Instant::now();
        let bits = src.bits_per_channel().unwrap();
        println!("bits {bits} {:?}", t.elapsed());
        let layer = src.layers[0].clone();
        let t = Instant::now();
        let tiles = src.layer_tiles(&layer, 8).unwrap();
        println!("layer_tiles {:?} ({} tiles)", t.elapsed(), tiles.len());
        let t = Instant::now();
        drop(tiles);
        println!("drop {:?}", t.elapsed());
        let t = Instant::now();
        let back = open_procreate(&path).unwrap();
        println!("open_procreate {:?}", t.elapsed());
        drop(back);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fit_keeps_a_small_picture_centred_and_premultiplies() {
        let px = [200u8, 100, 50, 128].repeat(2 * 2);
        let out = fit_to_canvas(&px, 2, 2, 6, 4);
        assert_eq!(out.len(), 6 * 4 * 4);
        let at = |x: usize, y: usize| &out[(y * 6 + x) * 4..(y * 6 + x) * 4 + 4];
        assert_eq!(at(2, 1), &[100, 50, 25, 128], "premultiplied, centered at (2,1)");
        assert_eq!(at(3, 2), &[100, 50, 25, 128]);
        assert_eq!(at(0, 0), &[0, 0, 0, 0]);
        assert_eq!(at(4, 1), &[0, 0, 0, 0]);
    }

    #[test]
    fn fit_scales_a_large_picture_down_to_the_canvas() {
        // 8x4 opaque, left half red, right half blue, onto a 4x4 canvas: scaled to 4x2, centred.
        let mut px = Vec::new();
        for _y in 0..4 {
            for x in 0..8 {
                px.extend_from_slice(if x < 4 { &[255, 0, 0, 255] } else { &[0, 0, 255, 255] });
            }
        }
        let out = fit_to_canvas(&px, 8, 4, 4, 4);
        let at = |x: usize, y: usize| &out[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(at(0, 0), &[0, 0, 0, 0], "letterbox above");
        assert_eq!(at(1, 1), &[255, 0, 0, 255]);
        assert_eq!(at(3, 2), &[0, 0, 255, 255]);
        assert_eq!(at(1, 3), &[0, 0, 0, 0]);
    }

    #[test]
    fn png_pictures_decode_and_other_files_are_refused() {
        let dir = std::env::temp_dir().join(format!("eskiz-decode-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.png");
        let rgba: Vec<u8> = (0..(3 * 2 * 4)).map(|v| (v * 9) as u8).collect();
        crate::export::write_png(&path, 3, 2, &rgba).unwrap();
        assert_eq!(decode_image(&path).unwrap(), (3, 2, rgba));
        let text = dir.join("t.png");
        std::fs::write(&text, b"hello").unwrap();
        assert!(decode_image(&text).is_err());
        assert!(is_image_file(&path) && is_image_file(Path::new("A.JPEG")) && !is_image_file(Path::new("a.procreate")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn place_crops_and_pads() {
        let src: Vec<u8> = (0..(3 * 2 * 4) as u8).collect();
        let out = place(&src, 3, 2, 2, 3, 9);
        assert_eq!(out.len(), 2 * 3 * 4);
        assert_eq!(&out[0..8], &src[0..8]);
        assert_eq!(&out[8..16], &src[12..20]);
        assert!(out[16..].iter().all(|&v| v == 9));
    }
}
