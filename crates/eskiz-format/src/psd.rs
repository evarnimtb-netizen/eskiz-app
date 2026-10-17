//! Photoshop (PSD) reader and writer for 8-bit RGB and RGBA documents.
//!
//! Implemented from the public PSD file format description.
//!
//! * In memory, every layer is premultiplied RGBA8 at canvas size, as elsewhere in Eskiz.
//!   Files store straight colour, so [`to_bytes`] un-premultiplies on the way out and [`from_bytes`]
//!   premultiplies on the way in.
//! * Children lists are in file order, bottom to top. A [`PsdGroup`] owns its children the same way.
//! * [`write`] crops each layer to its non-transparent bounds, RLE-encodes each channel, and writes
//!   the merged composite of the visible layers. The file goes to a temporary name first and is
//!   renamed into place.
//! * [`read`] decodes RGB and RGBA layers stored raw or with RLE, rebuilds groups from `lsct`
//!   records, and falls back to the merged image when the file has no layer records.
//!
//! Limits: version 1 only, and RGB or RGBA only. [`read`] and [`from_bytes`] take 8-bit files;
//! CMYK, Lab, grayscale, indexed, 16-bit and 32-bit files are rejected with [`Error::Unsupported`].
//! 16-bit files go through [`to_bytes16`] and [`from_bytes16`], which carry the
//! 16-bit pixels beside the tree (see [`Layers16`]). Layer masks are skipped on read
//! and never written. ZIP-compressed channels are rejected. Colour profiles are not written.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use eskiz_guides::ColourSpace;

use crate::error::{Error, Result};

/// Largest canvas side, and largest layer side, that PSD version 1 allows.
const MAX_SIDE: u32 = 30_000;
/// Layer and group records are counted by a signed 16-bit value.
const MAX_RECORDS: usize = 32_767;
/// Layer names are cut to this many characters, then to `MAX_NAME_BYTES` bytes.
const MAX_NAME_CHARS: usize = 128;
const MAX_NAME_BYTES: usize = 255;
/// Smallest divisor used in the blend formulas.
const EPS: f32 = 1.0 / 16_384.0;
/// Resolution written when the document has none.
const DEFAULT_DPI: f64 = 72.0;
/// Name of the bounding divider that closes every group.
const DIVIDER_NAME: &str = "</Layer group>";
/// Pass-through blend key, used by group records that have Normal blend.
const PASS_KEY: [u8; 4] = *b"pass";

const FLAG_TRANSPARENCY_LOCKED: u8 = 0x01;
const FLAG_HIDDEN: u8 = 0x02;
const FLAG_ALWAYS: u8 = 0x08;

/// A whole PSD document: canvas size, resolution and the layer tree.
#[derive(Clone, Debug, PartialEq)]
pub struct PsdDocument {
    /// Canvas width in pixels, 1 to 30 000.
    pub width: u32,
    /// Canvas height in pixels, 1 to 30 000.
    pub height: u32,
    /// Resolution in pixels per inch.
    pub dpi: f64,
    /// The colour space of the pixels. When set, the file carries the matching ICC profile in an
    /// image resource block (ID 1039, `0x040F`); reading fills it from that block. `None` writes no
    /// profile.
    pub colour_space: Option<ColourSpace>,
    /// Top-level items, bottom to top.
    pub root: Vec<PsdNode>,
}

/// One item of the layer tree.
#[derive(Clone, Debug, PartialEq)]
pub enum PsdNode {
    Layer(PsdLayer),
    Group(PsdGroup),
}

/// A raster layer. Pixels cover the whole canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct PsdLayer {
    pub name: String,
    /// Opacity from 0 to 1.
    pub opacity: f32,
    pub blend: PsdBlend,
    pub hidden: bool,
    /// Clipped to the layer directly below it (its base). Ignored when there is no base.
    pub clipped: bool,
    /// Transparency protected.
    pub alpha_locked: bool,
    /// Locked (stored in the `lspf` tag, bit 1).
    pub locked: bool,
    /// Premultiplied RGBA8, `width * height * 4` bytes, row by row from the top.
    pub pixels: Vec<u8>,
}

/// A group (folder). Its children are bottom to top, like the document root.
#[derive(Clone, Debug, PartialEq)]
pub struct PsdGroup {
    pub name: String,
    /// Opacity from 0 to 1.
    pub opacity: f32,
    pub blend: PsdBlend,
    pub hidden: bool,
    /// Stored as a closed folder (`lsct` type 2) when true.
    pub collapsed: bool,
    pub children: Vec<PsdNode>,
}

/// Blend modes that PSD layers carry. The keys are the ones in the file format notes table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PsdBlend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Add,
    Lighten,
    Exclusion,
    Difference,
    Subtract,
    LinearBurn,
    ColorDodge,
    ColorBurn,
    Overlay,
    HardLight,
    Color,
    Luminosity,
    Hue,
    Saturation,
    SoftLight,
    Darken,
    HardMix,
    VividLight,
    LinearLight,
    PinLight,
    LighterColor,
    DarkerColor,
    Divide,
}

const BLEND_KEYS: [(PsdBlend, [u8; 4]); 26] = [
    (PsdBlend::Normal, *b"norm"),
    (PsdBlend::Multiply, *b"mul "),
    (PsdBlend::Screen, *b"scrn"),
    (PsdBlend::Add, *b"lddg"),
    (PsdBlend::Lighten, *b"lite"),
    (PsdBlend::Exclusion, *b"smud"),
    (PsdBlend::Difference, *b"diff"),
    (PsdBlend::Subtract, *b"fsub"),
    (PsdBlend::LinearBurn, *b"lbrn"),
    (PsdBlend::ColorDodge, *b"div "),
    (PsdBlend::ColorBurn, *b"idiv"),
    (PsdBlend::Overlay, *b"over"),
    (PsdBlend::HardLight, *b"hLit"),
    (PsdBlend::Color, *b"colr"),
    (PsdBlend::Luminosity, *b"lum "),
    (PsdBlend::Hue, *b"hue "),
    (PsdBlend::Saturation, *b"sat "),
    (PsdBlend::SoftLight, *b"sLit"),
    (PsdBlend::Darken, *b"dark"),
    (PsdBlend::HardMix, *b"hMix"),
    (PsdBlend::VividLight, *b"vLit"),
    (PsdBlend::LinearLight, *b"lLit"),
    (PsdBlend::PinLight, *b"pLit"),
    (PsdBlend::LighterColor, *b"lgCl"),
    (PsdBlend::DarkerColor, *b"dkCl"),
    (PsdBlend::Divide, *b"fdiv"),
];

impl PsdBlend {
    /// The four-character key written to a layer record.
    pub fn key(self) -> [u8; 4] {
        BLEND_KEYS
            .iter()
            .find(|(mode, _)| *mode == self)
            .map_or(*b"norm", |(_, key)| *key)
    }

    /// The mode for a blend key. Returns `None` for unknown keys, and for `pass`.
    pub fn from_key(key: [u8; 4]) -> Option<Self> {
        BLEND_KEYS
            .iter()
            .find(|(_, k)| *k == key)
            .map(|(mode, _)| *mode)
    }
}

// ---------------------------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------------------------

/// Reads a PSD file from disk.
pub fn read(path: impl AsRef<Path>) -> Result<PsdDocument> {
    from_bytes(&fs::read(path)?)
}

/// Reads a PSD from any reader, consuming it to the end.
pub fn read_from(mut reader: impl Read) -> Result<PsdDocument> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    from_bytes(&bytes)
}

/// Writes a PSD file. The bytes go to a temporary file in the same folder, which is then renamed
/// over `path`. A failed write leaves no file behind.
pub fn write(path: impl AsRef<Path>, doc: &PsdDocument) -> Result<()> {
    let bytes = to_bytes(doc)?;
    write_atomic(path.as_ref(), &bytes)
}

/// Writes a PSD to any writer.
pub fn write_to(mut writer: impl Write, doc: &PsdDocument) -> Result<()> {
    writer.write_all(&to_bytes(doc)?)?;
    Ok(())
}

/// Parses an 8-bit PSD held in memory. A 16-bit file is rejected ("unsupported channel depth");
/// use [`from_bytes16`] for those.
pub fn from_bytes(bytes: &[u8]) -> Result<PsdDocument> {
    parse(bytes, false).map(|(doc, _)| doc)
}

/// The 16-bit pixels of a PSD, one buffer per raster layer in depth-first order of the tree
/// (bottom to top, a group's children where the group is). Each is premultiplied RGBA16,
/// `width * height * 4` samples, row by row from the top. The matching [`PsdLayer::pixels`] of a
/// tree read by [`from_bytes16`] are empty.
pub type Layers16 = Vec<Vec<u16>>;

/// Parses a PSD of 8 or 16 bits per channel. An 8-bit file gives its layers' pixels in the tree
/// and an empty list; a 16-bit file gives empty `pixels` in the tree and the real ones in the list.
pub fn from_bytes16(bytes: &[u8]) -> Result<(PsdDocument, Layers16)> {
    parse(bytes, true)
}

fn parse(bytes: &[u8], allow16: bool) -> Result<(PsdDocument, Layers16)> {
    let mut r = Reader::new(bytes);
    if r.array::<4>()? != *b"8BPS" {
        return Err(Error::Format("not a Photoshop file".into()));
    }
    match r.read_u16()? {
        1 => {}
        2 => {
            return Err(Error::Unsupported(
                "large-format files are not supported (PSB version 2)".into(),
            ))
        }
        other => return Err(Error::Format(format!("unknown PSD version {other}"))),
    }
    r.skip(6)?;
    let channels = r.read_u16()?;
    let height = r.read_u32()?;
    let width = r.read_u32()?;
    let depth = r.read_u16()?;
    let mode = r.read_u16()?;

    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(Error::Format(format!(
            "invalid canvas size {width}x{height}"
        )));
    }
    if depth != 8 && !(allow16 && depth == 16) {
        return Err(Error::Unsupported(format!(
            "unsupported channel depth {depth} (only 8-bit is supported)"
        )));
    }
    let wide = depth == 16;
    match mode {
        3 => {}
        4 => {
            return Err(Error::Unsupported(
                "CMYK color mode is not supported".into(),
            ))
        }
        9 => {
            return Err(Error::Unsupported(
                "Lab color mode is not supported".into(),
            ))
        }
        other => {
            return Err(Error::Unsupported(format!(
                "unsupported color mode {other} (only RGB is supported)"
            )))
        }
    }
    if !(3..=4).contains(&channels) {
        return Err(Error::Unsupported(format!(
            "unsupported channel count {channels} for RGB"
        )));
    }

    let mode_len = r.read_u32()? as usize;
    r.skip(mode_len)?;
    let res_len = r.read_u32()? as usize;
    let resources = r.take(res_len)?;
    let dpi = parse_dpi(resources)
        .ok()
        .flatten()
        .filter(|d| d.is_finite() && *d > 0.0)
        .unwrap_or(DEFAULT_DPI);
    let colour_space = parse_resource(resources, RESOURCE_ICC_PROFILE)
        .as_deref()
        .and_then(crate::icc::space_of_profile);

    let lm_len = r.read_u32()? as usize;
    let lm = r.take(lm_len)?;
    let merged = r.rest();

    let (w, h) = (width as usize, height as usize);
    let mut layers16: Layers16 = Vec::new();
    let mut root = if lm.is_empty() {
        Vec::new()
    } else {
        let mut lmr = Reader::new(lm);
        let li_len = lmr.read_u32()? as usize;
        parse_layer_info(lmr.take(li_len)?, w, h, wide.then_some(&mut layers16))?
    };

    if root.is_empty() {
        let (pixels, pixels16) = if wide {
            (Vec::new(), Some(decode_merged16(merged, w, h, usize::from(channels))?))
        } else {
            (decode_merged(merged, w, h, usize::from(channels))?, None)
        };
        layers16.extend(pixels16);
        root.push(PsdNode::Layer(PsdLayer {
            name: "Merged image".into(),
            opacity: 1.0,
            blend: PsdBlend::Normal,
            hidden: false,
            clipped: false,
            alpha_locked: false,
            locked: false,
            pixels,
        }));
    }

    Ok((
        PsdDocument {
            width,
            height,
            dpi,
            colour_space,
            root,
        },
        layers16,
    ))
}

/// Encodes an 8-bit PSD in memory.
pub fn to_bytes(doc: &PsdDocument) -> Result<Vec<u8>> {
    encode(doc, None)
}

/// How the channels of a 16-bit PSD are stored. Both are in the format: raw samples (compression
/// code 0) or PackBits per row (code 1) over the big-endian bytes of the row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Compression16 {
    /// Compression code 0.
    Raw,
    /// Compression code 1.
    #[default]
    Rle,
}

/// Encodes a 16-bit PSD (depth 16, big-endian samples). `doc` supplies the canvas and the tree;
/// the `pixels` fields of its layers are ignored (they may be empty) and `layers` supplies the
/// pixels, one buffer per raster layer in depth-first order ([`Layers16`]).
pub fn to_bytes16(doc: &PsdDocument, layers: &[Vec<u16>], compression: Compression16) -> Result<Vec<u8>> {
    encode(doc, Some(Src16 { layers, next: 0, rle: compression == Compression16::Rle }))
}

/// Pixels of a 16-bit write: the layer buffers, the next one to hand out, and the compression.
struct Src16<'a> {
    layers: &'a [Vec<u16>],
    next: usize,
    rle: bool,
}

fn encode(doc: &PsdDocument, mut src16: Option<Src16<'_>>) -> Result<Vec<u8>> {
    if doc.width == 0 || doc.height == 0 || doc.width > MAX_SIDE || doc.height > MAX_SIDE {
        return Err(Error::InvalidInput(format!(
            "canvas size {}x{} is outside 1..={MAX_SIDE}",
            doc.width, doc.height
        )));
    }
    let (w, h) = (doc.width as usize, doc.height as usize);
    match &src16 {
        None => check_pixels(&doc.root, w * h * 4)?,
        Some(s) => {
            let mut count = 0;
            count_layers(&doc.root, &mut count);
            if s.layers.len() != count || s.layers.iter().any(|l| l.len() != w * h * 4) {
                return Err(Error::InvalidInput(format!(
                    "expected {count} layer buffers of {} samples",
                    w * h * 4
                )));
            }
        }
    }
    let rle16 = src16.as_ref().map(|s| s.rle);
    let floats = match &src16 {
        None => Floats::Own,
        Some(s) => {
            let mut refs = Vec::new();
            layer_refs(&doc.root, &mut refs);
            Floats::Wide(refs.into_iter().zip(s.layers.iter().map(Vec::as_slice)).collect())
        }
    };

    let mut entries = Vec::new();
    collect_entries(&doc.root, w, &mut entries, &mut src16);
    if entries.len() > MAX_RECORDS {
        return Err(Error::InvalidInput(format!(
            "{} layers and groups exceed the PSD limit of {MAX_RECORDS}",
            entries.len()
        )));
    }

    let merged = merged_section(&doc.root, w, h, &floats, rle16);
    Ok(assemble(doc, &entries, src16.is_some(), merged))
}

/// The file from its records: header, resources, layer and mask information, then the merged
/// image section.
fn assemble(doc: &PsdDocument, entries: &[Entry], wide: bool, merged: Vec<u8>) -> Vec<u8> {
    let mut records = Vec::new();
    let mut channel_data = Vec::new();
    for entry in entries {
        records.extend(record_bytes(entry));
        for (_, data) in &entry.channels {
            channel_data.extend_from_slice(data);
        }
    }

    // Layer info: count, records, channel data, padded to an even length.
    let mut layer_info = Vec::with_capacity(2 + records.len() + channel_data.len() + 1);
    layer_info.extend_from_slice(&(entries.len() as i16).to_be_bytes());
    layer_info.extend(records);
    layer_info.extend(channel_data);
    pad_even(&mut layer_info);

    // Layer and mask information section: its length field, the layer info block (with its own
    // length field), and the 4-byte global mask block.
    let mut section = Vec::with_capacity(layer_info.len() + 8);
    section.extend_from_slice(&(layer_info.len() as u32).to_be_bytes());
    section.extend(layer_info);
    section.extend_from_slice(&0u32.to_be_bytes());

    let dpi = if doc.dpi.is_finite() && doc.dpi > 0.0 {
        doc.dpi.min(65_535.0)
    } else {
        DEFAULT_DPI
    };
    let mut resources = resolution_block(dpi);
    if let Some(space) = doc.colour_space {
        resources.extend(icc_block(&crate::icc::profile(space)));
    }

    let mut out = Vec::with_capacity(section.len() + resources.len() + merged.len() + 64);
    out.extend_from_slice(b"8BPS");
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&[0u8; 6]);
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&doc.height.to_be_bytes());
    out.extend_from_slice(&doc.width.to_be_bytes());
    out.extend_from_slice(&(if wide { 16u16 } else { 8u16 }).to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // colour mode data: none for RGB
    out.extend_from_slice(&(resources.len() as u32).to_be_bytes());
    out.extend(resources);
    out.extend_from_slice(&(section.len() as u32).to_be_bytes());
    out.extend(section);
    out.extend(merged);
    out
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .ok_or_else(|| Error::InvalidInput("output path has no file name".into()))?;
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(file_name);
    tmp_name.push(".tmp");
    let tmp = dir.join(tmp_name);
    let result = fs::File::create(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    Ok(result?)
}

// ---------------------------------------------------------------------------------------------
// PackBits
// ---------------------------------------------------------------------------------------------

/// PackBits-encodes one row. Runs of two or more equal bytes become a repeat record, and everything
/// else is copied as literal records of up to 128 bytes. Empty input gives empty output.
pub fn packbits_encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() + src.len() / 128 + 1);
    let n = src.len();
    let mut i = 0;
    while i < n {
        let mut run = 1;
        while i + run < n && run < 128 && src[i + run] == src[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((257 - run) as u8);
            out.push(src[i]);
            i += run;
        } else {
            let start = i;
            let mut end = i + 1;
            while end < n && end - start < 128 {
                if end + 1 < n && src[end] == src[end + 1] {
                    break;
                }
                end += 1;
            }
            out.push((end - start - 1) as u8);
            out.extend_from_slice(&src[start..end]);
            i = end;
        }
    }
    out
}

/// Decodes one PackBits row that must expand to exactly `expected` bytes. Trailing input after the
/// row is ignored. Returns [`Error::Corrupt`] when the input ends early or a record overruns the row.
pub fn packbits_decode(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    let mut i = 0;
    while out.len() < expected {
        let Some(&code) = src.get(i) else {
            return Err(Error::Corrupt("PackBits data ends early".into()));
        };
        i += 1;
        match code {
            0..=127 => {
                let len = usize::from(code) + 1;
                let Some(lit) = src.get(i..i + len) else {
                    return Err(Error::Corrupt("PackBits literal runs past the end".into()));
                };
                if out.len() + len > expected {
                    return Err(Error::Corrupt("PackBits literal overruns its row".into()));
                }
                out.extend_from_slice(lit);
                i += len;
            }
            128 => {}
            _ => {
                let count = 257 - usize::from(code);
                let Some(&value) = src.get(i) else {
                    return Err(Error::Corrupt("PackBits repeat has no value".into()));
                };
                i += 1;
                if out.len() + count > expected {
                    return Err(Error::Corrupt("PackBits repeat overruns its row".into()));
                }
                out.resize(out.len() + count, value);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Writer internals
// ---------------------------------------------------------------------------------------------

/// One layer or group record, ready to serialise.
struct Entry {
    name: String,
    /// Top, left, bottom, right.
    rect: [i32; 4],
    /// Channel ID and channel data (including the 2-byte compression code), in table order.
    channels: Vec<(i16, Vec<u8>)>,
    key: [u8; 4],
    opacity: u8,
    clipped: bool,
    flags: u8,
    /// Tagged blocks, already framed.
    tags: Vec<u8>,
}

fn check_pixels(nodes: &[PsdNode], expected: usize) -> Result<()> {
    for node in nodes {
        match node {
            PsdNode::Layer(layer) => {
                if layer.pixels.len() != expected {
                    return Err(Error::InvalidInput(format!(
                        "layer '{}' has {} bytes of pixels, expected {expected}",
                        layer.name,
                        layer.pixels.len()
                    )));
                }
            }
            PsdNode::Group(group) => check_pixels(&group.children, expected)?,
        }
    }
    Ok(())
}

/// Flattens the tree into records in file order: a group's divider, its children, then its folder.
fn collect_entries(nodes: &[PsdNode], w: usize, out: &mut Vec<Entry>, src16: &mut Option<Src16<'_>>) {
    for (index, node) in nodes.iter().enumerate() {
        match node {
            PsdNode::Layer(layer) => {
                let clipped = layer.clipped && has_base(nodes, index);
                let id = out.len() as u32 + 1;
                match src16 {
                    None => out.push(layer_entry(layer, clipped, id, w, Planes::Bytes(&layer.pixels), true)),
                    Some(s) => {
                        let px = &s.layers[s.next];
                        s.next += 1;
                        out.push(layer_entry(layer, clipped, id, w, Planes::Words(px), s.rle));
                    }
                }
            }
            PsdNode::Group(group) => {
                let id = out.len() as u32 + 1;
                out.push(divider_entry(id));
                collect_entries(&group.children, w, out, src16);
                let id = out.len() as u32 + 1;
                out.push(folder_entry(group, id));
            }
        }
    }
}

/// A clipped layer has a base only when the nearest layer below it that is not itself clipped is a
/// layer and not a group. Otherwise it is written unclipped.
fn has_base(nodes: &[PsdNode], index: usize) -> bool {
    for node in nodes[..index].iter().rev() {
        match node {
            PsdNode::Group(_) => return false,
            PsdNode::Layer(layer) if !layer.clipped => return true,
            PsdNode::Layer(_) => {}
        }
    }
    false
}

/// A layer's pixels as the writer receives them.
#[derive(Clone, Copy)]
enum Planes<'a> {
    Bytes(&'a [u8]),
    Words(&'a [u16]),
}

fn layer_entry(layer: &PsdLayer, clipped: bool, id: u32, w: usize, px: Planes<'_>, rle: bool) -> Entry {
    let (x0, y0, x1, y1) = match px {
        Planes::Bytes(b) => opaque_bounds(b, w),
        Planes::Words(b) => opaque_bounds(b, w),
    }
    .unwrap_or((0, 0, 0, 0));
    let (cw, ch) = (x1 - x0, y1 - y0);
    let channels = match px {
        Planes::Bytes(pixels) => {
            let mut planes = [
                vec![0u8; cw * ch],
                vec![0u8; cw * ch],
                vec![0u8; cw * ch],
                vec![0u8; cw * ch],
            ];
            for y in 0..ch {
                for x in 0..cw {
                    let s = ((y0 + y) * w + x0 + x) * 4;
                    let px = &pixels[s..s + 4];
                    let i = y * cw + x;
                    planes[0][i] = unpremul(px[0], px[3]);
                    planes[1][i] = unpremul(px[1], px[3]);
                    planes[2][i] = unpremul(px[2], px[3]);
                    planes[3][i] = px[3];
                }
            }
            vec![
                (0, rle_channel(&planes[0], cw, ch)),
                (1, rle_channel(&planes[1], cw, ch)),
                (2, rle_channel(&planes[2], cw, ch)),
                (-1, rle_channel(&planes[3], cw, ch)),
            ]
        }
        Planes::Words(pixels) => {
            let n = cw * ch;
            let mut planes = [vec![0u16; n], vec![0u16; n], vec![0u16; n], vec![0u16; n]];
            for y in 0..ch {
                for x in 0..cw {
                    let s = ((y0 + y) * w + x0 + x) * 4;
                    let px = &pixels[s..s + 4];
                    let i = y * cw + x;
                    planes[0][i] = unpremul16(px[0], px[3]);
                    planes[1][i] = unpremul16(px[1], px[3]);
                    planes[2][i] = unpremul16(px[2], px[3]);
                    planes[3][i] = px[3];
                }
            }
            [(0i16, 0usize), (1, 1), (2, 2), (-1, 3)]
                .into_iter()
                .map(|(id, k)| (id, channel16(&planes[k], cw, ch, rle)))
                .collect()
        }
    };
    let rect = if cw == 0 {
        [0; 4]
    } else {
        [y0 as i32, x0 as i32, y1 as i32, x1 as i32]
    };

    let mut tags = luni_tag(&layer.name);
    tags.extend(lyid_tag(id));
    if layer.alpha_locked || layer.locked {
        let bits = u32::from(layer.alpha_locked) | (u32::from(layer.locked) << 1);
        tags.extend(tagged(b"lspf", &bits.to_be_bytes()));
    }
    Entry {
        name: layer.name.clone(),
        rect,
        channels,
        key: layer.blend.key(),
        opacity: opacity_byte(layer.opacity),
        clipped,
        flags: flags_byte(layer.alpha_locked, layer.hidden),
        tags,
    }
}

fn divider_entry(id: u32) -> Entry {
    let mut tags = luni_tag(DIVIDER_NAME);
    tags.extend(lyid_tag(id));
    tags.extend(tagged(b"lsct", &section_payload(3, PASS_KEY)));
    Entry {
        name: DIVIDER_NAME.into(),
        rect: [0; 4],
        channels: Vec::new(),
        key: PASS_KEY,
        opacity: 255,
        clipped: false,
        flags: flags_byte(false, true),
        tags,
    }
}

fn folder_entry(group: &PsdGroup, id: u32) -> Entry {
    let key = group_key(group.blend);
    let kind = if group.collapsed { 2 } else { 1 };
    let mut tags = luni_tag(&group.name);
    tags.extend(lyid_tag(id));
    tags.extend(tagged(b"lsct", &section_payload(kind, key)));
    Entry {
        name: group.name.clone(),
        rect: [0; 4],
        channels: Vec::new(),
        key,
        opacity: opacity_byte(group.opacity),
        clipped: false,
        flags: flags_byte(false, group.hidden),
        tags,
    }
}

fn group_key(blend: PsdBlend) -> [u8; 4] {
    if blend == PsdBlend::Normal {
        PASS_KEY
    } else {
        blend.key()
    }
}

fn flags_byte(alpha_locked: bool, hidden: bool) -> u8 {
    let mut flags = FLAG_ALWAYS;
    if alpha_locked {
        flags |= FLAG_TRANSPARENCY_LOCKED;
    }
    if hidden {
        flags |= FLAG_HIDDEN;
    }
    flags
}

fn opacity_byte(opacity: f32) -> u8 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn record_bytes(entry: &Entry) -> Vec<u8> {
    let mut extra = Vec::with_capacity(entry.tags.len() + 64);
    extra.extend_from_slice(&0u32.to_be_bytes()); // layer mask data: none
    extra.extend_from_slice(&0u32.to_be_bytes()); // blending ranges: none
    extra.extend(pascal_name(&entry.name));
    extra.extend_from_slice(&entry.tags);

    let mut out = Vec::with_capacity(48 + entry.channels.len() * 6 + extra.len());
    for v in entry.rect {
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&(entry.channels.len() as u16).to_be_bytes());
    for (id, data) in &entry.channels {
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(&entry.key);
    out.push(entry.opacity);
    out.push(u8::from(entry.clipped));
    out.push(entry.flags);
    out.push(0); // filler
    out.extend_from_slice(&(extra.len() as u32).to_be_bytes());
    out.extend(extra);
    out
}

/// Clips a name to 128 characters, then to 255 bytes, on a character boundary.
fn clip_name(name: &str) -> &str {
    let mut s = match name.char_indices().nth(MAX_NAME_CHARS) {
        Some((i, _)) => &name[..i],
        None => name,
    };
    while s.len() > MAX_NAME_BYTES {
        let mut end = s.len() - 1;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s = &s[..end];
    }
    s
}

/// Pascal string: one length byte, the bytes, then zero padding to a multiple of 4.
fn pascal_name(name: &str) -> Vec<u8> {
    let name = clip_name(name);
    let mut out = Vec::with_capacity(4 + name.len());
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    let pad = (4 - out.len() % 4) % 4;
    out.resize(out.len() + pad, 0);
    out
}

fn tagged(key: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + payload.len() + 1);
    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(key);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    pad_even(&mut out);
    out
}

fn luni_tag(name: &str) -> Vec<u8> {
    let units: Vec<u16> = clip_name(name).encode_utf16().collect();
    let mut payload = Vec::with_capacity(4 + units.len() * 2);
    payload.extend_from_slice(&(units.len() as u32).to_be_bytes());
    for unit in units {
        payload.extend_from_slice(&unit.to_be_bytes());
    }
    tagged(b"luni", &payload)
}

fn lyid_tag(id: u32) -> Vec<u8> {
    tagged(b"lyid", &id.to_be_bytes())
}

/// Payload of an `lsct` tag: the section type, then the `8BIM` signature and blend key.
fn section_payload(kind: u32, key: [u8; 4]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(12);
    payload.extend_from_slice(&kind.to_be_bytes());
    payload.extend_from_slice(b"8BIM");
    payload.extend_from_slice(&key);
    payload
}

fn pad_even(bytes: &mut Vec<u8>) {
    if bytes.len() % 2 == 1 {
        bytes.push(0);
    }
}

/// Image resource ID of the ICC profile.
const RESOURCE_ICC_PROFILE: u16 = 1039;

/// The ICC profile image resource: `8BIM`, ID 1039, an empty Pascal name, the size and the profile,
/// padded to an even length.
fn icc_block(profile: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(profile.len() + 13);
    b.extend_from_slice(b"8BIM");
    b.extend_from_slice(&RESOURCE_ICC_PROFILE.to_be_bytes());
    b.extend_from_slice(&[0, 0]); // empty Pascal name, padded to even
    b.extend_from_slice(&(profile.len() as u32).to_be_bytes());
    b.extend_from_slice(profile);
    pad_even(&mut b);
    b
}

fn resolution_block(dpi: f64) -> Vec<u8> {
    let fixed = (dpi * 65_536.0).round() as u32;
    let mut b = Vec::with_capacity(28);
    b.extend_from_slice(b"8BIM");
    b.extend_from_slice(&1005u16.to_be_bytes());
    b.extend_from_slice(&[0, 0]); // empty Pascal name, padded to even
    b.extend_from_slice(&16u32.to_be_bytes());
    b.extend_from_slice(&fixed.to_be_bytes());
    b.extend_from_slice(&1u16.to_be_bytes()); // horizontal unit: pixels per inch
    b.extend_from_slice(&1u16.to_be_bytes()); // width unit: inch
    b.extend_from_slice(&fixed.to_be_bytes());
    b.extend_from_slice(&1u16.to_be_bytes()); // vertical unit: pixels per inch
    b.extend_from_slice(&1u16.to_be_bytes()); // height unit: inch
    b
}

/// Bounding box `(x0, y0, x1, y1)` of the pixels with alpha above zero. `x1` and `y1` are exclusive.
fn opaque_bounds<T: Copy + Default + PartialEq>(pixels: &[T], w: usize) -> Option<(usize, usize, usize, usize)> {
    let mut bounds: Option<(usize, usize, usize, usize)> = None;
    for (i, px) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if px[3] == T::default() {
            continue;
        }
        let (x, y) = (i % w, i / w);
        bounds = Some(match bounds {
            None => (x, y, x + 1, y + 1),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)),
        });
    }
    bounds
}

/// One channel as RLE: compression code 1, a row-count table, then the packed rows.
fn rle_channel(plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let (counts, packed) = rle_rows(plane, w, h);
    let mut out = Vec::with_capacity(2 + counts.len() * 2 + packed.len());
    out.extend_from_slice(&1u16.to_be_bytes());
    for count in counts {
        out.extend_from_slice(&count.to_be_bytes());
    }
    out.extend(packed);
    out
}

/// Row counts and packed rows for one plane. Empty when the plane has no width or height.
fn rle_rows(plane: &[u8], w: usize, h: usize) -> (Vec<u16>, Vec<u8>) {
    let mut counts = Vec::with_capacity(h);
    let mut packed = Vec::new();
    if w == 0 || h == 0 {
        return (counts, packed);
    }
    for row in plane.chunks(w).take(h) {
        let encoded = packbits_encode(row);
        counts.push(encoded.len() as u16);
        packed.extend(encoded);
    }
    (counts, packed)
}

fn unpremul(c: u8, a: u8) -> u8 {
    if a == 0 {
        return 0;
    }
    let v = (u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a);
    v.min(255) as u8
}

fn unpremul16(c: u16, a: u16) -> u16 {
    if a == 0 {
        return 0;
    }
    let v = (u64::from(c) * 65_535 + u64::from(a) / 2) / u64::from(a);
    v.min(65_535) as u16
}

fn premul16(c: u16, a: u16) -> u16 {
    ((u64::from(c) * u64::from(a) + 32_767) / 65_535) as u16
}

/// One 16-bit channel: compression code 1 with PackBits rows over the big-endian bytes (a row is
/// `2 * w` bytes), or code 0 with the raw big-endian samples.
fn channel16(plane: &[u16], w: usize, h: usize, rle: bool) -> Vec<u8> {
    let bytes: Vec<u8> = plane.iter().flat_map(|v| v.to_be_bytes()).collect();
    if !rle {
        let mut out = Vec::with_capacity(2 + bytes.len());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend(bytes);
        return out;
    }
    let (counts, packed) = rle_rows(&bytes, w * 2, h);
    let mut out = Vec::with_capacity(2 + counts.len() * 2 + packed.len());
    out.extend_from_slice(&1u16.to_be_bytes());
    for count in counts {
        out.extend_from_slice(&count.to_be_bytes());
    }
    out.extend(packed);
    out
}

fn premul(c: u8, a: u8) -> u8 {
    ((u32::from(c) * u32::from(a) + 127) / 255) as u8
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn merged_section(root: &[PsdNode], w: usize, h: usize, floats: &Floats<'_>, rle16: Option<bool>) -> Vec<u8> {
    let mut canvas = vec![0.0f32; w * h * 4];
    composite_nodes(root, &mut canvas, floats);
    if let Some(rle) = rle16 {
        return merged_section16(&canvas, w, h, rle);
    }
    let planes = straight_planes(&canvas);

    let mut counts = Vec::with_capacity(4 * h);
    let mut packed = Vec::new();
    for plane in &planes {
        let (plane_counts, plane_packed) = rle_rows(plane, w, h);
        counts.extend(plane_counts);
        packed.extend(plane_packed);
    }
    let mut out = Vec::with_capacity(2 + counts.len() * 2 + packed.len());
    out.extend_from_slice(&1u16.to_be_bytes());
    for count in counts {
        out.extend_from_slice(&count.to_be_bytes());
    }
    out.extend(packed);
    out
}

/// The merged image of a 16-bit file: four planes of big-endian samples, raw or PackBits rows.
fn merged_section16(canvas: &[f32], w: usize, h: usize, rle: bool) -> Vec<u8> {
    let n = canvas.len() / 4;
    let planes = wide_planes(canvas);
    if !rle {
        let mut out = Vec::with_capacity(2 + n * 8);
        out.extend_from_slice(&0u16.to_be_bytes());
        for plane in &planes {
            out.extend_from_slice(plane);
        }
        return out;
    }
    let mut counts = Vec::with_capacity(4 * h);
    let mut packed = Vec::new();
    for plane in &planes {
        let (plane_counts, plane_packed) = rle_rows(plane, w * 2, h);
        counts.extend(plane_counts);
        packed.extend(plane_packed);
    }
    let mut out = Vec::with_capacity(2 + counts.len() * 2 + packed.len());
    out.extend_from_slice(&1u16.to_be_bytes());
    for count in counts {
        out.extend_from_slice(&count.to_be_bytes());
    }
    out.extend(packed);
    out
}

/// Splits a premultiplied float canvas into straight R, G, B and A planes of big-endian 16-bit
/// samples.
fn wide_planes(canvas: &[f32]) -> [Vec<u8>; 4] {
    let n = canvas.len() / 4;
    let mut planes = [vec![0u8; n * 2], vec![0u8; n * 2], vec![0u8; n * 2], vec![0u8; n * 2]];
    let to16 = |v: f32| (v.clamp(0.0, 1.0) * 65_535.0).round() as u16;
    for (i, px) in canvas.as_chunks::<4>().0.iter().enumerate() {
        let a = to16(px[3]);
        let vals = [unpremul16(to16(px[0]), a), unpremul16(to16(px[1]), a), unpremul16(to16(px[2]), a), a];
        for (plane, v) in planes.iter_mut().zip(vals) {
            plane[i * 2..i * 2 + 2].copy_from_slice(&v.to_be_bytes());
        }
    }
    planes
}

/// Splits a premultiplied float canvas into straight R, G, B and A planes.
fn straight_planes(canvas: &[f32]) -> [Vec<u8>; 4] {
    let n = canvas.len() / 4;
    let mut planes = [vec![0u8; n], vec![0u8; n], vec![0u8; n], vec![0u8; n]];
    for (i, px) in canvas.as_chunks::<4>().0.iter().enumerate() {
        let a = to_u8(px[3]);
        planes[0][i] = unpremul(to_u8(px[0]), a);
        planes[1][i] = unpremul(to_u8(px[1]), a);
        planes[2][i] = unpremul(to_u8(px[2]), a);
        planes[3][i] = a;
    }
    planes
}

/// Where the compositor reads a layer's pixels from: the layer itself (8-bit), or a table of
/// 16-bit buffers keyed by the layer's address.
enum Floats<'a> {
    Own,
    Wide(std::collections::HashMap<*const PsdLayer, &'a [u16]>),
    /// Rows `y0..y1` of a layer, from the supplier: `index_of` maps a layer's address to its
    /// position in depth-first order.
    Stream {
        src: &'a dyn LayerPixels,
        index_of: &'a std::collections::HashMap<usize, usize>,
        y0: usize,
        y1: usize,
        wide: bool,
    },
}

impl Floats<'_> {
    /// Whether the layer is known to be fully transparent.
    fn is_blank(&self, layer: &PsdLayer) -> bool {
        match self {
            Floats::Stream { src, index_of, y0, y1, .. } => index_of
                .get(&(layer as *const PsdLayer as usize))
                .is_some_and(|&i| src.rows_blank(i, *y0, *y1)),
            _ => false,
        }
    }

    fn of(&self, layer: &PsdLayer) -> Vec<f32> {
        match self {
            Floats::Own => to_float(&layer.pixels),
            Floats::Wide(map) => map
                .get(&(layer as *const PsdLayer))
                .map_or_else(Vec::new, |p| p.iter().map(|&v| f32::from(v) / 65_535.0).collect()),
            Floats::Stream { src, index_of, y0, y1, wide } => match index_of.get(&(layer as *const PsdLayer as usize)) {
                None => Vec::new(),
                Some(&i) if *wide => src.rows16(i, *y0, *y1).iter().map(|&v| f32::from(v) / 65_535.0).collect(),
                Some(&i) => to_float(&src.rows8(i, *y0, *y1)),
            },
        }
    }
}

fn count_layers(nodes: &[PsdNode], n: &mut usize) {
    for node in nodes {
        match node {
            PsdNode::Layer(_) => *n += 1,
            PsdNode::Group(g) => count_layers(&g.children, n),
        }
    }
}

fn layer_refs(nodes: &[PsdNode], out: &mut Vec<*const PsdLayer>) {
    for node in nodes {
        match node {
            PsdNode::Layer(l) => out.push(l as *const PsdLayer),
            PsdNode::Group(g) => layer_refs(&g.children, out),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Compositing (merged image)
// ---------------------------------------------------------------------------------------------

/// Composites a list of items (bottom to top) onto `dst`, a premultiplied float canvas.
/// Groups are composited into their own transparent buffer, then placed with the group's opacity
/// and blend. Each clipped run is a base layer followed by the clipped layers above it.
fn composite_nodes(nodes: &[PsdNode], dst: &mut [f32], floats: &Floats<'_>) {
    let mut i = 0;
    while i < nodes.len() {
        match &nodes[i] {
            PsdNode::Group(group) => {
                if !group.hidden {
                    let mut buffer = vec![0.0f32; dst.len()];
                    composite_nodes(&group.children, &mut buffer, floats);
                    blend_into(dst, &buffer, group.blend, group.opacity);
                }
                i += 1;
            }
            PsdNode::Layer(base) => {
                let mut end = i + 1;
                while matches!(nodes.get(end), Some(PsdNode::Layer(l)) if l.clipped) {
                    end += 1;
                }
                // A blank base (and so everything clipped to it) changes nothing.
                if !base.hidden && !floats.is_blank(base) {
                    let mut src = floats.of(base);
                    for node in &nodes[i + 1..end] {
                        if let PsdNode::Layer(clip) = node {
                            if !clip.hidden {
                                apply_clip(&mut src, clip, floats);
                            }
                        }
                    }
                    blend_into(dst, &src, base.blend, base.opacity);
                }
                i = end;
            }
        }
    }
}

/// Mixes a clipped layer's colour into the base's colour, inside the base's alpha. The base alpha
/// is unchanged.
fn apply_clip(base: &mut [f32], clip: &PsdLayer, floats: &Floats<'_>) {
    let clip_px = floats.of(clip);
    for (b, c) in base
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(clip_px.as_chunks::<4>().0)
    {
        let (ba, ca) = (b[3], c[3]);
        if ba <= 0.0 || ca <= 0.0 {
            continue;
        }
        let weight = (ca * clip.opacity).clamp(0.0, 1.0);
        let cb = [b[0] / ba, b[1] / ba, b[2] / ba];
        let cs = [c[0] / ca, c[1] / ca, c[2] / ca];
        let mixed = blend_rgb(clip.blend, cb, cs);
        let out: [f32; 3] = std::array::from_fn(|k| cb[k] * (1.0 - weight) + weight * mixed[k]);
        b[0] = out[0] * ba;
        b[1] = out[1] * ba;
        b[2] = out[2] * ba;
    }
}

/// Source-over with a blend mode, on premultiplied pixels. `src` is multiplied by `opacity` first.
fn blend_into(dst: &mut [f32], src: &[f32], mode: PsdBlend, opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    for (d, s) in dst
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.as_chunks::<4>().0)
    {
        let sa = s[3] * opacity;
        if sa <= 0.0 {
            continue;
        }
        let da = d[3];
        let cs = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
        let cb = if da > 0.0 {
            [d[0] / da, d[1] / da, d[2] / da]
        } else {
            [0.0; 3]
        };
        let b = blend_rgb(mode, cb, cs);
        let out: [f32; 3] = std::array::from_fn(|k| {
            (1.0 - sa) * da * cb[k] + sa * (1.0 - da) * cs[k] + sa * da * b[k]
        });
        d[0] = out[0];
        d[1] = out[1];
        d[2] = out[2];
        d[3] = sa + da - sa * da;
    }
}

fn to_float(pixels: &[u8]) -> Vec<f32> {
    pixels.iter().map(|&v| f32::from(v) / 255.0).collect()
}

/// B(Cb, Cs) for straight colours in [0, 1].
fn blend_rgb(mode: PsdBlend, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    use PsdBlend::*;
    match mode {
        Color => set_lum(cs, lum(cb)),
        Luminosity => set_lum(cb, lum(cs)),
        Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        LighterColor => {
            if luma(cb) > luma(cs) {
                cb
            } else {
                cs
            }
        }
        DarkerColor => {
            if luma(cb) < luma(cs) {
                cb
            } else {
                cs
            }
        }
        _ => std::array::from_fn(|k| separable(mode, cb[k], cs[k])),
    }
}

/// Per-channel blend formulas from the file format notes (section 3.1 and 3.2).
fn separable(mode: PsdBlend, b: f32, s: f32) -> f32 {
    use PsdBlend::*;
    match mode {
        Normal => s,
        Multiply => b * s,
        Screen => b + s - b * s,
        Add => (b + s).min(1.0),
        Lighten => b.max(s),
        Exclusion => b + s - 2.0 * b * s,
        Difference => (b - s).abs(),
        Subtract => (b - s).max(0.0),
        LinearBurn => (b + s - 1.0).max(0.0),
        ColorDodge => (b / (1.0 - s).max(EPS)).min(1.0),
        ColorBurn => 1.0 - ((1.0 - b) / s.max(EPS)).min(1.0),
        Overlay => hard_light(s, b),
        HardLight => hard_light(b, s),
        SoftLight => {
            if s <= 0.5 {
                b * b * (1.0 - 2.0 * s) + 2.0 * b * s
            } else {
                2.0 * b * (1.0 - s) + b.max(0.0).sqrt() * (2.0 * s - 1.0)
            }
        }
        Darken => b.min(s),
        HardMix => {
            if b + s >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        VividLight => {
            if s < 0.5 {
                (1.0 - (1.0 - b) / (2.0 * s).max(EPS)).clamp(0.0, 1.0)
            } else {
                (b / (2.0 * (1.0 - s)).max(EPS)).clamp(0.0, 1.0)
            }
        }
        LinearLight => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
        PinLight => {
            if s < 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * s - 1.0)
            }
        }
        Divide => {
            if s <= 0.0 {
                if b > 0.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                (b / s).min(1.0)
            }
        }
        // Non-separable modes are handled by `blend_rgb`.
        Color | Luminosity | Hue | Saturation | LighterColor | DarkerColor => s,
    }
}

/// Hard Light with `b` as the backdrop and `s` as the source selector.
fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        2.0 * b * s
    } else {
        1.0 - 2.0 * (1.0 - b) * (1.0 - s)
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.30005 * c[0] + 0.58984 * c[1] + 0.10998 * c[2]
}

fn luma(c: [f32; 3]) -> f32 {
    0.29907 * c[0] + 0.58691 * c[1] + 0.11401 * c[2]
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 && l - n > 0.0 {
        let d = l - n;
        out = out.map(|v| l + (v - l) * l / d);
    }
    if x > 1.0 && x - l > 0.0 {
        let d = x - l;
        out = out.map(|v| l + (v - l) * (1.0 - l) / d);
    }
    out
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx <= mn {
        return [0.0; 3];
    }
    c.map(|v| (v - mn) * s / (mx - mn))
}

// ---------------------------------------------------------------------------------------------
// Reader internals
// ---------------------------------------------------------------------------------------------

/// Bounds-checked big-endian cursor over a byte slice.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::Format("unexpected end of data".into()));
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    /// Advances by up to `n` bytes and stops at the end of the buffer.
    fn skip_lenient(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n).min(self.buf.len());
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn read_u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn read_i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.array()?))
    }

    fn read_u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn read_i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    fn read_u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }
}

enum Kind {
    Layer,
    Divider,
    Folder { collapsed: bool },
}

#[derive(Default)]
struct Tags {
    luni: Option<String>,
    lsct: Option<u32>,
    lspf: Option<u32>,
}

/// A layer record before its channel data is decoded.
struct RawRecord {
    rect: [i32; 4],
    /// Channel ID and length, in table order.
    channels: Vec<(i16, usize)>,
    key: [u8; 4],
    opacity: u8,
    clipped: bool,
    flags: u8,
    name: String,
    kind: Kind,
    lspf: Option<u32>,
}

fn parse_layer_info(li: &[u8], w: usize, h: usize, mut wide: Option<&mut Layers16>) -> Result<Vec<PsdNode>> {
    if li.len() < 2 {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(li);
    let count = r.read_i16()?.unsigned_abs() as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        records.push(read_record(&mut r)?);
    }

    // Channel data follows all records, in record order and then channel-table order.
    let mut starts = Vec::with_capacity(count);
    let mut offset = r.pos;
    for rec in &records {
        starts.push(offset);
        offset += rec.channels.iter().map(|(_, len)| len).sum::<usize>();
    }
    if offset > li.len() {
        return Err(Error::Format(
            "layer channel data runs past the layer info block".into(),
        ));
    }

    let mut stack: Vec<Vec<PsdNode>> = vec![Vec::new()];
    for (rec, start) in records.iter().zip(starts) {
        match rec.kind {
            Kind::Divider => stack.push(Vec::new()),
            Kind::Folder { collapsed } => {
                if stack.len() < 2 {
                    return Err(Error::Format("unbalanced layer group markers".into()));
                }
                let children = stack.pop().unwrap_or_default();
                let group = PsdGroup {
                    name: rec.name.clone(),
                    opacity: f32::from(rec.opacity) / 255.0,
                    blend: PsdBlend::from_key(rec.key).unwrap_or_default(),
                    hidden: rec.flags & FLAG_HIDDEN != 0,
                    collapsed,
                    children,
                };
                top_frame(&mut stack)?.push(PsdNode::Group(group));
            }
            Kind::Layer => {
                let layer = match wide.as_deref_mut() {
                    Some(out) => {
                        let (layer, px) = decode_layer16(rec, li, start, w, h)?;
                        out.push(px);
                        layer
                    }
                    None => decode_layer(rec, li, start, w, h)?,
                };
                top_frame(&mut stack)?.push(PsdNode::Layer(layer));
            }
        }
    }
    if stack.len() != 1 {
        return Err(Error::Format("unclosed layer group".into()));
    }
    Ok(stack.pop().unwrap_or_default())
}

fn top_frame(stack: &mut [Vec<PsdNode>]) -> Result<&mut Vec<PsdNode>> {
    stack
        .last_mut()
        .ok_or_else(|| Error::Format("unbalanced layer group markers".into()))
}

fn read_record(r: &mut Reader) -> Result<RawRecord> {
    let top = r.read_i32()?;
    let left = r.read_i32()?;
    let bottom = r.read_i32()?;
    let right = r.read_i32()?;
    let channel_count = usize::from(r.read_u16()?);
    let mut channels = Vec::with_capacity(channel_count);
    for _ in 0..channel_count {
        let id = r.read_i16()?;
        let len = r.read_u32()? as usize;
        channels.push((id, len));
    }
    if r.array::<4>()? != *b"8BIM" {
        return Err(Error::Format("bad layer record signature".into()));
    }
    let key = r.array::<4>()?;
    let opacity = r.read_u8()?;
    let clipped = r.read_u8()? != 0;
    let flags = r.read_u8()?;
    r.read_u8()?; // filler
    let extra_len = r.read_u32()? as usize;
    let extra = r.take(extra_len)?;
    let (pascal, tags) = parse_extra(extra)?;

    let kind = match tags.lsct {
        Some(3) => Kind::Divider,
        Some(1) => Kind::Folder { collapsed: false },
        Some(2) => Kind::Folder { collapsed: true },
        _ => Kind::Layer,
    };
    Ok(RawRecord {
        rect: [top, left, bottom, right],
        channels,
        key,
        opacity,
        clipped,
        flags,
        name: tags.luni.unwrap_or(pascal),
        kind,
        lspf: tags.lspf,
    })
}

/// Parses the extra data of a layer record: mask data, blending ranges, the Pascal name, and the
/// tagged blocks. Tagged blocks that are malformed end the scan without an error.
fn parse_extra(extra: &[u8]) -> Result<(String, Tags)> {
    let mut r = Reader::new(extra);
    let mask_len = r.read_u32()? as usize;
    r.skip(mask_len)?;
    let blend_len = r.read_u32()? as usize;
    r.skip(blend_len)?;
    let name_len = usize::from(r.read_u8()?);
    let name = String::from_utf8_lossy(r.take(name_len)?).into_owned();
    r.skip_lenient((4 - (1 + name_len) % 4) % 4);

    let mut tags = Tags::default();
    while r.remaining() >= 12 {
        let Some(skip) = tag_start(r.rest()) else {
            break;
        };
        r.skip_lenient(skip);
        if r.remaining() < 12 {
            break;
        }
        let sig = r.array::<4>()?;
        let key = r.array::<4>()?;
        let len = if sig == *b"8B64" {
            r.read_u64()? as usize
        } else {
            r.read_u32()? as usize
        };
        let Ok(data) = r.take(len) else {
            break;
        };
        if len % 2 == 1 {
            r.skip_lenient(1);
        }
        match &key {
            b"luni" => tags.luni = parse_luni(data),
            b"lsct" => tags.lsct = read_be_u32(data),
            b"lspf" => tags.lspf = read_be_u32(data),
            _ => {}
        }
    }
    Ok((name, tags))
}

/// Offset of the next tagged block signature, within the first four bytes of `rest`.
fn tag_start(rest: &[u8]) -> Option<usize> {
    (0..4).find(|&k| {
        rest.len() >= k + 4 && (&rest[k..k + 4] == b"8BIM" || &rest[k..k + 4] == b"8B64")
    })
}

fn read_be_u32(data: &[u8]) -> Option<u32> {
    match data {
        [a, b, c, d, ..] => Some(u32::from_be_bytes([*a, *b, *c, *d])),
        _ => None,
    }
}

fn parse_luni(data: &[u8]) -> Option<String> {
    let count = read_be_u32(data)? as usize;
    let units: Vec<u16> = data[4..]
        .as_chunks::<2>()
        .0
        .iter()
        .take(count)
        .map(|pair| u16::from_be_bytes(*pair))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

fn decode_layer(rec: &RawRecord, li: &[u8], start: usize, w: usize, h: usize) -> Result<PsdLayer> {
    let [top, left, bottom, right] = rec.rect;
    let rw = (i64::from(right) - i64::from(left)).max(0) as usize;
    let rh = (i64::from(bottom) - i64::from(top)).max(0) as usize;
    if rw > MAX_SIDE as usize || rh > MAX_SIDE as usize {
        return Err(Error::Format("layer rectangle is too large".into()));
    }

    let mut planes: [Option<Vec<u8>>; 4] = [None, None, None, None];
    let mut offset = start;
    for &(id, len) in &rec.channels {
        let data = li
            .get(offset..offset + len)
            .ok_or_else(|| Error::Format("layer channel data is out of range".into()))?;
        offset += len;
        let slot = match id {
            0 => Some(0),
            1 => Some(1),
            2 => Some(2),
            -1 => Some(3),
            _ => None,
        };
        if let Some(slot) = slot {
            planes[slot] = Some(decode_channel(data, rw, rh)?);
        }
    }

    let mut pixels = vec![0u8; w * h * 4];
    for y in 0..rh {
        let cy = i64::from(top) + y as i64;
        if cy < 0 || cy >= h as i64 {
            continue;
        }
        for x in 0..rw {
            let cx = i64::from(left) + x as i64;
            if cx < 0 || cx >= w as i64 {
                continue;
            }
            let i = y * rw + x;
            let a = planes[3].as_ref().map_or(255, |p| p[i]);
            let c = |plane: &Option<Vec<u8>>| plane.as_ref().map_or(0, |p| p[i]);
            let dst = (cy as usize * w + cx as usize) * 4;
            pixels[dst] = premul(c(&planes[0]), a);
            pixels[dst + 1] = premul(c(&planes[1]), a);
            pixels[dst + 2] = premul(c(&planes[2]), a);
            pixels[dst + 3] = a;
        }
    }

    let lspf = rec.lspf.unwrap_or(0);
    Ok(PsdLayer {
        name: rec.name.clone(),
        opacity: f32::from(rec.opacity) / 255.0,
        blend: PsdBlend::from_key(rec.key).unwrap_or_default(),
        hidden: rec.flags & FLAG_HIDDEN != 0,
        clipped: rec.clipped,
        alpha_locked: rec.flags & FLAG_TRANSPARENCY_LOCKED != 0 || lspf & 1 != 0,
        locked: lspf & 2 != 0,
        pixels,
    })
}

/// [`decode_layer`] for a 16-bit file: the layer without pixels, and its premultiplied RGBA16.
fn decode_layer16(rec: &RawRecord, li: &[u8], start: usize, w: usize, h: usize) -> Result<(PsdLayer, Vec<u16>)> {
    let [top, left, bottom, right] = rec.rect;
    let rw = (i64::from(right) - i64::from(left)).max(0) as usize;
    let rh = (i64::from(bottom) - i64::from(top)).max(0) as usize;
    if rw > MAX_SIDE as usize || rh > MAX_SIDE as usize {
        return Err(Error::Format("layer rectangle is too large".into()));
    }
    let mut planes: [Option<Vec<u16>>; 4] = [None, None, None, None];
    let mut offset = start;
    for &(id, len) in &rec.channels {
        let data = li
            .get(offset..offset + len)
            .ok_or_else(|| Error::Format("layer channel data is out of range".into()))?;
        offset += len;
        let slot = match id {
            0 => Some(0),
            1 => Some(1),
            2 => Some(2),
            -1 => Some(3),
            _ => None,
        };
        if let Some(slot) = slot {
            planes[slot] = Some(decode_channel16(data, rw, rh)?);
        }
    }
    let mut pixels = vec![0u16; w * h * 4];
    for y in 0..rh {
        let cy = i64::from(top) + y as i64;
        if cy < 0 || cy >= h as i64 {
            continue;
        }
        for x in 0..rw {
            let cx = i64::from(left) + x as i64;
            if cx < 0 || cx >= w as i64 {
                continue;
            }
            let i = y * rw + x;
            let a = planes[3].as_ref().map_or(65_535, |p| p[i]);
            let c = |plane: &Option<Vec<u16>>| plane.as_ref().map_or(0, |p| p[i]);
            let dst = (cy as usize * w + cx as usize) * 4;
            pixels[dst] = premul16(c(&planes[0]), a);
            pixels[dst + 1] = premul16(c(&planes[1]), a);
            pixels[dst + 2] = premul16(c(&planes[2]), a);
            pixels[dst + 3] = a;
        }
    }
    let lspf = rec.lspf.unwrap_or(0);
    let layer = PsdLayer {
        name: rec.name.clone(),
        opacity: f32::from(rec.opacity) / 255.0,
        blend: PsdBlend::from_key(rec.key).unwrap_or_default(),
        hidden: rec.flags & FLAG_HIDDEN != 0,
        clipped: rec.clipped,
        alpha_locked: rec.flags & FLAG_TRANSPARENCY_LOCKED != 0 || lspf & 1 != 0,
        locked: lspf & 2 != 0,
        pixels: Vec::new(),
    };
    Ok((layer, pixels))
}

/// Decodes one 16-bit channel plane of `w * h` samples (big-endian). Compression 0 is raw and 1
/// is PackBits over rows of `2 * w` bytes.
fn decode_channel16(data: &[u8], w: usize, h: usize) -> Result<Vec<u16>> {
    let mut r = Reader::new(data);
    let bytes = match r.read_u16()? {
        0 => r.take(w * h * 2)?.to_vec(),
        1 => {
            let mut counts = Vec::with_capacity(h);
            for _ in 0..h {
                counts.push(usize::from(r.read_u16()?));
            }
            let mut out = Vec::with_capacity(w * h * 2);
            for count in counts {
                out.extend(packbits_decode(r.take(count)?, w * 2)?);
            }
            out
        }
        2 | 3 => return Err(Error::Unsupported("ZIP-compressed layer data is not supported".into())),
        other => return Err(Error::Unsupported(format!("unknown channel compression {other}"))),
    };
    Ok(bytes.as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect())
}

/// [`decode_merged`] for a 16-bit file, to premultiplied RGBA16.
fn decode_merged16(data: &[u8], w: usize, h: usize, channels: usize) -> Result<Vec<u16>> {
    let mut r = Reader::new(data);
    let compression = r.read_u16()?;
    let mut planes: Vec<Vec<u16>> = Vec::with_capacity(channels);
    let words = |b: &[u8]| -> Vec<u16> { b.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect() };
    match compression {
        0 => {
            for _ in 0..channels {
                planes.push(words(r.take(w * h * 2)?));
            }
        }
        1 => {
            let mut counts = Vec::with_capacity(channels * h);
            for _ in 0..channels * h {
                counts.push(usize::from(r.read_u16()?));
            }
            for c in 0..channels {
                let mut plane = Vec::with_capacity(w * h * 2);
                for &count in &counts[c * h..(c + 1) * h] {
                    plane.extend(packbits_decode(r.take(count)?, w * 2)?);
                }
                planes.push(words(&plane));
            }
        }
        other => return Err(Error::Unsupported(format!("unknown merged-image compression {other}"))),
    }
    let mut out = vec![0u16; w * h * 4];
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let a = if channels >= 4 { planes[3][i] } else { 65_535 };
        px[0] = premul16(planes[0][i], a);
        px[1] = premul16(planes[1][i], a);
        px[2] = premul16(planes[2][i], a);
        px[3] = a;
    }
    Ok(out)
}

/// Decodes one channel plane of `w * h` bytes. Compression 0 is raw and 1 is RLE.
fn decode_channel(data: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    match r.read_u16()? {
        0 => Ok(r.take(w * h)?.to_vec()),
        1 => {
            let mut counts = Vec::with_capacity(h);
            for _ in 0..h {
                counts.push(usize::from(r.read_u16()?));
            }
            let mut out = Vec::with_capacity(w * h);
            for count in counts {
                let row = r.take(count)?;
                out.extend(packbits_decode(row, w)?);
            }
            Ok(out)
        }
        2 | 3 => Err(Error::Unsupported(
            "ZIP-compressed layer data is not supported".into(),
        )),
        other => Err(Error::Unsupported(format!(
            "unknown channel compression {other}"
        ))),
    }
}

/// Decodes the merged image (planes in R, G, B, A order) to premultiplied RGBA8.
fn decode_merged(data: &[u8], w: usize, h: usize, channels: usize) -> Result<Vec<u8>> {
    let mut r = Reader::new(data);
    let compression = r.read_u16()?;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(channels);
    match compression {
        0 => {
            for _ in 0..channels {
                planes.push(r.take(w * h)?.to_vec());
            }
        }
        1 => {
            let mut counts = Vec::with_capacity(channels * h);
            for _ in 0..channels * h {
                counts.push(usize::from(r.read_u16()?));
            }
            for c in 0..channels {
                let mut plane = Vec::with_capacity(w * h);
                for &count in &counts[c * h..(c + 1) * h] {
                    let row = r.take(count)?;
                    plane.extend(packbits_decode(row, w)?);
                }
                planes.push(plane);
            }
        }
        other => {
            return Err(Error::Unsupported(format!(
                "unknown merged-image compression {other}"
            )))
        }
    }

    let mut out = vec![0u8; w * h * 4];
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let a = if channels >= 4 { planes[3][i] } else { 255 };
        px[0] = premul(planes[0][i], a);
        px[1] = premul(planes[1][i], a);
        px[2] = premul(planes[2][i], a);
        px[3] = a;
    }
    Ok(out)
}

/// Reads the ResolutionInfo block (ID 1005) from the image resources. `Ok(None)` if absent.
fn parse_dpi(res: &[u8]) -> Result<Option<f64>> {
    Ok(parse_resource(res, 1005).and_then(|data| read_be_u32(&data).map(|fixed| f64::from(fixed) / 65_536.0)))
}

/// The data of image resource `wanted`, if the resource section holds it.
fn parse_resource(res: &[u8], wanted: u16) -> Option<Vec<u8>> {
    fn walk(res: &[u8], wanted: u16) -> Result<Option<Vec<u8>>> {
        let mut r = Reader::new(res);
        while r.remaining() >= 12 {
            if r.array::<4>()? != *b"8BIM" {
                break;
            }
            let id = r.read_u16()?;
            let name_len = usize::from(r.read_u8()?);
            r.skip(name_len)?;
            if (1 + name_len) % 2 == 1 {
                r.skip(1)?;
            }
            let size = r.read_u32()? as usize;
            let data = r.take(size)?;
            if size % 2 == 1 {
                r.skip_lenient(1);
            }
            if id == wanted {
                return Ok(Some(data.to_vec()));
            }
        }
        Ok(None)
    }
    walk(res, wanted).ok().flatten()
}

// ---------------------------------------------------------------------------------------------
// Streamed writer
// ---------------------------------------------------------------------------------------------

/// Where the streamed writer gets layer pixels from, so that no layer has to exist as a dense
/// canvas-sized buffer until it is written. Raster layers are numbered in depth-first order of
/// the tree (the order of [`Layers16`]); every layer has the canvas size.
pub trait LayerPixels: Sync {
    /// Rows `y0..y1` of raster layer `index`, premultiplied RGBA8: `(y1 - y0) * width * 4` bytes.
    fn rows8(&self, index: usize, y0: usize, y1: usize) -> Vec<u8>;
    /// Rows `y0..y1` of raster layer `index`, premultiplied RGBA16: `(y1 - y0) * width * 4`
    /// samples.
    fn rows16(&self, index: usize, y0: usize, y1: usize) -> Vec<u16>;
    /// Whether rows `y0..y1` of raster layer `index` are known to be fully transparent (all
    /// zero). A `true` lets the writer skip reading and blending them; the default, `false`, is
    /// always correct.
    fn rows_blank(&self, _index: usize, _y0: usize, _y1: usize) -> bool {
        false
    }
}

/// Layers whose pixels are being turned into channels at once (each is a dense buffer while it
/// is), at most.
const STREAM_LAYERS_IN_FLIGHT: usize = 4;
/// Rows of the merged image composited at once.
const MERGED_BAND_ROWS: usize = 64;

/// [`to_bytes`] (`depth16` `None`) or [`to_bytes16`] (`Some`) with the layer pixels taken from
/// `pixels` instead of from the document: the `pixels` fields of `doc`'s layers are ignored. The
/// bytes are the same as the dense writers give for the same pixels.
///
/// Layers are written a few at a time, and the merged image is composited in bands of rows, so
/// peak memory does not grow with the number of layers.
pub fn to_bytes_streamed(doc: &PsdDocument, pixels: &dyn LayerPixels, depth16: Option<Compression16>) -> Result<Vec<u8>> {
    if doc.width == 0 || doc.height == 0 || doc.width > MAX_SIDE || doc.height > MAX_SIDE {
        return Err(Error::InvalidInput(format!(
            "canvas size {}x{} is outside 1..={MAX_SIDE}",
            doc.width, doc.height
        )));
    }
    let (w, h) = (doc.width as usize, doc.height as usize);
    let wide = depth16.map(|c| c == Compression16::Rle);

    let mut plan = Vec::new();
    let mut next_index = 0;
    plan_entries(&doc.root, &mut plan, &mut next_index);
    if plan.len() > MAX_RECORDS {
        return Err(Error::InvalidInput(format!(
            "{} layers and groups exceed the PSD limit of {MAX_RECORDS}",
            plan.len()
        )));
    }

    let layer_jobs: Vec<usize> = plan.iter().enumerate().filter(|(_, p)| matches!(p, Plan::Layer { .. })).map(|(i, _)| i).collect();
    let mut built = crate::pack::par_map_with_threads(
        STREAM_LAYERS_IN_FLIGHT,
        layer_jobs.len(),
        || (),
        |(), k| {
            let Plan::Layer { layer, clipped, id, index } = &plan[layer_jobs[k]] else {
                unreachable!("layer jobs point at layers")
            };
            Some(match wide {
                // a blank layer has no opaque pixel: the writer never looks at its buffer
                None if pixels.rows_blank(*index, 0, h) => layer_entry(layer, *clipped, *id, w, Planes::Bytes(&[]), true),
                Some(rle) if pixels.rows_blank(*index, 0, h) => layer_entry(layer, *clipped, *id, w, Planes::Words(&[]), rle),
                None => {
                    let px = pixels.rows8(*index, 0, h);
                    layer_entry(layer, *clipped, *id, w, Planes::Bytes(&px), true)
                }
                Some(rle) => {
                    let px = pixels.rows16(*index, 0, h);
                    layer_entry(layer, *clipped, *id, w, Planes::Words(&px), rle)
                }
            })
        },
    );
    let mut entries = Vec::with_capacity(plan.len());
    let mut jobs = layer_jobs.iter().zip(built.iter_mut());
    let mut next_job = jobs.next();
    for (i, item) in plan.into_iter().enumerate() {
        match item {
            Plan::Entry(entry) => entries.push(entry),
            Plan::Layer { .. } => {
                let (at, slot) = next_job.take().expect("a built entry per layer");
                debug_assert_eq!(*at, i);
                entries.push(slot.take().expect("built once"));
                next_job = jobs.next();
            }
        }
    }

    let merged = merged_streamed(&doc.root, w, h, pixels, wide);
    Ok(assemble(doc, &entries, depth16.is_some(), merged))
}

/// [`to_bytes_streamed`] to a file: the bytes go to a temporary file in the same folder, which
/// is then renamed over `path`.
pub fn write_streamed(path: impl AsRef<Path>, doc: &PsdDocument, pixels: &dyn LayerPixels, depth16: Option<Compression16>) -> Result<()> {
    let bytes = to_bytes_streamed(doc, pixels, depth16)?;
    write_atomic(path.as_ref(), &bytes)
}

enum Plan<'a> {
    Entry(Entry),
    Layer { layer: &'a PsdLayer, clipped: bool, id: u32, index: usize },
}

/// [`collect_entries`] without the pixels: group records are built, layers are planned.
fn plan_entries<'a>(nodes: &'a [PsdNode], out: &mut Vec<Plan<'a>>, next_index: &mut usize) {
    for (index, node) in nodes.iter().enumerate() {
        match node {
            PsdNode::Layer(layer) => {
                let clipped = layer.clipped && has_base(nodes, index);
                let id = out.len() as u32 + 1;
                out.push(Plan::Layer { layer, clipped, id, index: *next_index });
                *next_index += 1;
            }
            PsdNode::Group(group) => {
                let id = out.len() as u32 + 1;
                out.push(Plan::Entry(divider_entry(id)));
                plan_entries(&group.children, out, next_index);
                let id = out.len() as u32 + 1;
                out.push(Plan::Entry(folder_entry(group, id)));
            }
        }
    }
}

/// The rows of one band of the merged image, per plane: row counts and packed rows (RLE), or
/// the raw plane bytes.
struct Band {
    counts: [Vec<u16>; 4],
    packed: [Vec<u8>; 4],
}

/// [`merged_section`] / `merged_section16` in bands of rows. Compositing and the plane
/// conversion work pixel by pixel and RLE rows are independent, so the bytes are the same.
fn merged_streamed(root: &[PsdNode], w: usize, h: usize, src: &dyn LayerPixels, wide: Option<bool>) -> Vec<u8> {
    let mut refs = Vec::new();
    layer_refs(root, &mut refs);
    let index_of: std::collections::HashMap<usize, usize> = refs.iter().enumerate().map(|(i, &p)| (p as usize, i)).collect();
    let bands: Vec<(usize, usize)> = (0..h.div_ceil(MERGED_BAND_ROWS))
        .map(|b| (b * MERGED_BAND_ROWS, ((b + 1) * MERGED_BAND_ROWS).min(h)))
        .collect();
    let is_wide = wide.is_some();
    let raw16 = wide == Some(false);
    let threads = crate::pack::worker_cpus();
    let results: Vec<Band> = crate::pack::par_map_with_threads(
        threads,
        bands.len(),
        || (),
        |(), b| {
            let (y0, y1) = bands[b];
            let rows = y1 - y0;
            let floats = Floats::Stream { src, index_of: &index_of, y0, y1, wide: is_wide };
            let mut canvas = vec![0.0f32; w * rows * 4];
            composite_nodes(root, &mut canvas, &floats);
            let mut band = Band { counts: Default::default(), packed: Default::default() };
            if is_wide {
                let planes = wide_planes(&canvas);
                for (p, plane) in planes.into_iter().enumerate() {
                    if raw16 {
                        band.packed[p] = plane;
                    } else {
                        (band.counts[p], band.packed[p]) = rle_rows(&plane, w * 2, rows);
                    }
                }
            } else {
                for (p, plane) in straight_planes(&canvas).iter().enumerate() {
                    (band.counts[p], band.packed[p]) = rle_rows(plane, w, rows);
                }
            }
            band
        },
    );
    if raw16 {
        let mut out = Vec::with_capacity(2 + w * h * 8);
        out.extend_from_slice(&0u16.to_be_bytes());
        for p in 0..4 {
            for band in &results {
                out.extend_from_slice(&band.packed[p]);
            }
        }
        return out;
    }
    let packed_len: usize = results.iter().map(|b| b.packed.iter().map(Vec::len).sum::<usize>()).sum();
    let mut out = Vec::with_capacity(2 + 8 * h + packed_len);
    out.extend_from_slice(&1u16.to_be_bytes());
    for p in 0..4 {
        for band in &results {
            for count in &band.counts[p] {
                out.extend_from_slice(&count.to_be_bytes());
            }
        }
    }
    for p in 0..4 {
        for band in &results {
            out.extend_from_slice(&band.packed[p]);
        }
    }
    out
}
