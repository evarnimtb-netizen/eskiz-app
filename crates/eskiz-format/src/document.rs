//! The `.procreate` document: ZIP package, `Document.archive`, the layer tree and tile assembly.
//!
//! Layer pixels are returned as premultiplied RGBA8, top row first, exactly as stored.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::codecs::{apple_lz4_declared_size, apple_lz4_decompress, lzo1x_decompress};
use crate::limits;
use crate::pack::{self, LayerOut, LayerTiles};
use crate::keyedarchive::{self, unarchive, ArchivedObject, NsObject, NsValue, Object};
use crate::{Error, Result};

/// App blend index to mode name.
/// Index 18 and 27 to 33 are not listed; 34 is an alias of Darken.
pub const BLEND_MODES: &[(i64, &str)] = &[
    (0, "Normal"),
    (1, "Multiply"),
    (2, "Screen"),
    (3, "Add"),
    (4, "Lighten"),
    (5, "Exclusion"),
    (6, "Difference"),
    (7, "Subtract"),
    (8, "Linear Burn"),
    (9, "Color Dodge"),
    (10, "Color Burn"),
    (11, "Overlay"),
    (12, "Hard Light"),
    (13, "Color"),
    (14, "Luminosity"),
    (15, "Hue"),
    (16, "Saturation"),
    (17, "Soft Light"),
    (19, "Darken"),
    (20, "Hard Mix"),
    (21, "Vivid Light"),
    (22, "Linear Light"),
    (23, "Pin Light"),
    (24, "Lighter Color"),
    (25, "Darker Color"),
    (26, "Divide"),
    (34, "Darken"),
];

/// Looks up the name of a blend mode index in [`BLEND_MODES`].
pub fn blend_mode_name(mode: i64) -> Option<&'static str> {
    BLEND_MODES
        .iter()
        .find(|(m, _)| *m == mode)
        .map(|(_, name)| *name)
}

const DEFAULT_TILE_SIZE: i64 = 256;
const MAX_TILE_SIZE: i64 = limits::MAX_TILE_SIDE as i64;

/// Largest payload accepted for one tile: a full tile of 16 bytes per pixel (the widest layout
/// recognised, so that an RGBA32F document is reported as unsupported rather than corrupt).
fn max_tile_payload(tile: usize) -> usize {
    tile * tile * 16
}

/// Decompresses one tile payload. The size the frame headers declare is checked against the
/// largest payload a tile can hold before anything is allocated, and the decoders count their
/// output and stop at that cap.
fn decompress_tile(raw: &[u8], codec: Codec, tile: usize) -> Result<Vec<u8>> {
    let max = max_tile_payload(tile);
    match codec {
        Codec::Lz4 => {
            let declared = apple_lz4_declared_size(raw);
            if declared > max {
                return Err(Error::Corrupt(format!(
                    "a {tile}-pixel tile declares {declared} bytes; at most {max} fit"
                )));
            }
            apple_lz4_decompress(raw, max)
        }
        Codec::Lzo => lzo1x_decompress(raw, max),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    /// `.lz4`: Apple framed LZ4.
    Lz4,
    /// `.chunk`: legacy LZO1X.
    Lzo,
}

#[derive(Debug, Clone)]
struct TileRef {
    member: String,
    codec: Codec,
    revision: u64,
}

/// The document's Frames object (`ValkyrieDocumentAnimation`,, plus the
/// two document flags that mark the bottom item as the background and the top item as the foreground.
///
/// Field values are as stored. Only `loop` (`playbackMode` 1) and the defaults are verified. The other
/// playback codes are the `[choice]` values used by `eskiz-app`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimationSpec {
    /// `animationMode`. 0 is off (4a). Any other value is Frames on.
    pub animation_mode: i64,
    /// `playbackMode`. 1 is loop (4a).
    pub playback_mode: i64,
    /// `playbackDirection`. 0 is forward (4a).
    pub playback_direction: i64,
    pub primary_mixed: bool,
    /// Frames per second, 1 to 60 (4a default 15).
    pub frame_rate: f64,
    /// `onionSkinCount` (4a default 12).
    pub onion_skin_count: i64,
    /// `onionSkinOpacity`, 0 to 1 (4a default 0.6).
    pub onion_skin_opacity: f64,
    pub onion_skin_enabled: bool,
    /// `skinBlendMode` (4a default 2). Kept as stored.
    pub skin_blend_mode: i64,
    /// `isLastItemAnimationBackground`: the bottom item is drawn beneath every frame.
    pub last_item_is_background: bool,
    /// `isFirstItemAnimationForeground`: the top item is drawn above every frame.
    pub first_item_is_foreground: bool,
}

impl Default for AnimationSpec {
    /// The defaults of a new document.
    fn default() -> Self {
        Self {
            animation_mode: 0,
            playback_mode: 1,
            playback_direction: 0,
            primary_mixed: false,
            frame_rate: 15.0,
            onion_skin_count: 12,
            onion_skin_opacity: 0.6,
            onion_skin_enabled: true,
            skin_blend_mode: 2,
            last_item_is_background: false,
            first_item_is_foreground: false,
        }
    }
}

impl AnimationSpec {
    /// Reads the `animation` object of the document root `root`. Missing keys take the 4a defaults.
    fn read(anim: &ArchivedObject, root: &ArchivedObject) -> Self {
        let d = Self::default();
        Self {
            animation_mode: anim.i64("animationMode").unwrap_or(d.animation_mode),
            playback_mode: anim.i64("playbackMode").unwrap_or(d.playback_mode),
            playback_direction: anim.i64("playbackDirection").unwrap_or(d.playback_direction),
            primary_mixed: anim.truthy("primaryMixed"),
            frame_rate: anim.f64("frameRate").unwrap_or(d.frame_rate),
            onion_skin_count: anim.i64("onionSkinCount").unwrap_or(d.onion_skin_count),
            onion_skin_opacity: anim.f64("onionSkinOpacity").unwrap_or(d.onion_skin_opacity),
            onion_skin_enabled: anim.get("onionSkinEnabled").map_or(d.onion_skin_enabled, Object::truthy),
            skin_blend_mode: anim.i64("skinBlendMode").unwrap_or(d.skin_blend_mode),
            last_item_is_background: root.truthy("isLastItemAnimationBackground"),
            first_item_is_foreground: root.truthy("isFirstItemAnimationForeground"),
        }
    }
}

/// A raster layer (`SilicaLayer`).
#[derive(Debug, Clone)]
pub struct Layer {
    /// Name of the layer's tile directory in the package.
    pub uuid: String,
    pub name: String,
    pub opacity: f64,
    pub hidden: bool,
    pub locked: bool,
    pub clipped: bool,
    /// Lock transparency (`preserve`).
    pub alpha_locked: bool,
    pub private: bool,
    pub layer_type: Option<i64>,
    /// Blend mode index, from `extendedBlend2`, else `extendedBlend`, else `blend`.
    /// `None` only when the archive stores null there.
    pub blend_mode: Option<i64>,
    /// Layer size in pixels. Defaults to the canvas size.
    pub width: u32,
    pub height: u32,
    pub mask: Option<Box<Layer>>,
    pub is_text: bool,
    /// The archived text source of a text layer (`text` key), if it could be read.
    pub text: Option<TextSpec>,
    /// Extra Frames frames this layer is held for (`animationHeldLength`).
    pub animation_hold: u32,
}

impl Layer {
    /// Blend mode name, e.g. `"Multiply"`; unknown indices give `"Unknown(n)"`.
    pub fn blend_name(&self) -> Option<String> {
        self.blend_mode.map(|mode| match blend_mode_name(mode) {
            Some(name) => name.to_owned(),
            None => format!("Unknown({mode})"),
        })
    }

    fn from_archived(a: &ArchivedObject, canvas: (u32, u32)) -> Layer {
        let dim = |key: &str, default: u32| match a.i64(key) {
            Some(n) if n > 0 && n <= i64::from(u32::MAX) => n as u32,
            _ => default,
        };
        let blend_mode = match ["extendedBlend2", "extendedBlend", "blend"]
            .iter()
            .find_map(|key| a.get(key))
        {
            Some(value) => value.as_i64(),
            None => Some(0),
        };
        let mask = a
            .archived("mask")
            .filter(|m| m.get("UUID").is_some_and(Object::truthy))
            .map(|m| Box::new(Layer::from_archived(m, canvas)));
        Layer {
            uuid: a.str("UUID").unwrap_or("").to_owned(),
            name: a.str("name").unwrap_or("").to_owned(),
            opacity: a.f64("opacity").unwrap_or(1.0),
            hidden: a.truthy("hidden"),
            locked: a.truthy("locked"),
            clipped: a.truthy("clipped"),
            alpha_locked: a.truthy("preserve"),
            private: a.truthy("private"),
            layer_type: a.i64("type"),
            blend_mode,
            width: dim("sizeWidth", canvas.0),
            height: dim("sizeHeight", canvas.1),
            mask,
            is_text: a.get("text").is_some_and(|v| !v.is_null()),
            text: a.archived("text").and_then(text_from_archived),
            animation_hold: hold(a),
        }
    }
}

/// A layer group (`SilicaGroup`).
#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub opacity: f64,
    pub hidden: bool,
    pub collapsed: bool,
    pub locked: bool,
    pub private: bool,
    pub children: Vec<Item>,
    /// Extra Frames frames the group is held for (`animationHeldLength`).
    pub animation_hold: u32,
}

/// One entry of the layer tree.
#[derive(Debug, Clone)]
pub enum Item {
    Layer(Layer),
    Group(Group),
}

/// An open `.procreate` package.
pub struct Document<R = File> {
    zip: ZipArchive<R>,
    prefix: String,
    pub name: Option<String>,
    /// Canvas size in pixels.
    pub width: u32,
    pub height: u32,
    /// Tile edge in pixels.
    pub tile_size: u32,
    pub version: Option<i64>,
    pub orientation: Option<i64>,
    pub flipped_horizontally: bool,
    pub flipped_vertically: bool,
    pub background_hidden: bool,
    /// Background colour as RGBA, from four little-endian f32 values.
    pub background_color: Option<[f32; 4]>,
    pub dpi: Option<f64>,
    /// Time spent on the artwork, in seconds.
    pub tracked_time: Option<f64>,
    pub stroke_count: Option<i64>,
    pub author: Option<String>,
    /// ICC profile name.
    pub color_profile: Option<String>,
    /// The layer tree, top-most first, as stored.
    pub tree: Vec<Item>,
    /// All raster layers, flattened from the tree in depth-first order.
    pub layers: Vec<Layer>,
    /// The stored flattened composite, if present.
    pub composite: Option<Layer>,
    /// The Frames object, if the package has one.
    pub animation: Option<AnimationSpec>,
    /// Bits per channel the archive declares (`bitsPerChannel`, written by Eskiz). `None` for
    /// archives without the key; [`Document::bits_per_channel`] then reads it from a tile.
    pub declared_bits: Option<u32>,
    /// The encoding of 16-bit samples when the archive says so (Eskiz's `channelFormat` key), or
    /// the one found by scanning the tiles once ([`Document::tile_encoding`]).
    encoding: Option<TileEncoding>,
    /// Bytes per pixel of the tiles, decided once for the document ([`TileDepth`]); cached.
    depth: Option<TileDepth>,
    tiles: HashMap<String, HashMap<(i64, i64), TileRef>>,
}

/// The bytes per pixel of a document's tile payloads: 4 for 8-bit, 8 for 16-bit.
///
/// It is decided once for the whole document, not per tile.
/// Payloads hold either the clipped tile or a full `T x T` one, so a 16-bit edge tile can be
/// exactly as long as an 8-bit full tile (128 by 256 at 8 bytes is 256 by 256 at 4 bytes), and
/// its length alone cannot say which it is. The depth comes from, in order:
///
/// 1. Eskiz's `bitsPerChannel` key, when present.
/// 2. Any tile whose length fits exactly one reading (see [`unambiguous_bpp`]). An interior tile
///    always does; so does an edge tile that is not also a full tile of the other depth. The first
///    such tile in the document decides.
/// 3. Otherwise [`TileDepth::Guess`]: every tile is read with its own guess, 4 bytes first, which
///    is the only case where a 16-bit edge tile can be misread. It needs a document whose tiles
///    are all ambiguous, such as a single canvas narrower than a tile in which the one tile is
///    a 16-bit half-width column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TileDepth {
    /// Every tile is this many bytes per pixel.
    Known(usize),
    /// No tile is unambiguous: each tile is read with its own guess.
    Guess,
}

/// How the samples of a document's tiles are encoded.
///
/// Tiles hold raw texture bytes with no per-pixel conversion, so the 16-bit
/// tiles are either `uint16` unorm (format 1) or half float (format 3), and the archive does not
/// say which. Eskiz's own files say it with `channelFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileEncoding {
    /// 8 bits per channel. The document has no 16-bit tiles.
    U8,
    /// 16-bit unsigned normalised integers: `0xFFFF` is 1.0 (format 1, and every Eskiz file).
    U16Unorm,
    /// 16-bit IEEE half floats in [0, 1]: `0x3C00` is 1.0 (format 3).
    F16,
}

impl Document<File> {
    /// Opens a `.procreate` file.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_reader(File::open(path)?)
    }
}

impl<R: Read + Seek> Document<R> {
    /// Opens a `.procreate` package from any seekable reader holding the ZIP container.
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        let mut zip = ZipArchive::new(reader)?;
        limits::check_zip(&zip, file_len)?;
        let names: Vec<String> = zip
            .file_names()
            .map(|n| n.map(|s| s.into_owned()))
            .collect::<std::result::Result<_, ZipError>>()?;

        // some exporters wrap the package in a top-level folder
        let prefix = if names.iter().any(|n| n == "Document.archive") {
            String::new()
        } else {
            names
                .iter()
                .find_map(|n| n.strip_suffix("/Document.archive"))
                .map(|dir| format!("{dir}/"))
                .ok_or_else(|| Error::NotProcreate("no Document.archive entry".into()))?
        };

        let archive_bytes = read_member(&mut zip, &format!("{prefix}Document.archive"))?
            .ok_or_else(|| Error::NotProcreate("Document.archive is missing".into()))?;
        let root = unarchive(&archive_bytes)?;
        let a = root
            .as_archived()
            .ok_or_else(|| Error::Format("Document.archive root is not an object".into()))?;

        let tile_size = match a.i64("tileSize") {
            None | Some(0) => DEFAULT_TILE_SIZE,
            Some(t) => t,
        };
        if !(1..=MAX_TILE_SIZE).contains(&tile_size) {
            return Err(Error::Format(format!("tile size {tile_size} is out of range")));
        }
        let (width, height) = match parse_cg_size(a.get("size")) {
            // `as` saturates, so negative or NaN sizes become 0
            Some((w, h)) => (w as u32, h as u32),
            None => (0, 0),
        };
        limits::check_canvas(width, height, "the canvas")?;
        let canvas = (width, height);

        let items: &[Object] = match a.get("unwrappedLayers") {
            Some(v) if !v.is_null() => v.as_array().unwrap_or(&[]),
            _ => a.array("layers").unwrap_or(&[]),
        };
        let tree: Vec<Item> = items.iter().filter_map(|o| make_item(o, canvas)).collect();
        let item_count = count_items(&tree);
        if item_count > limits::MAX_LAYERS {
            return Err(limits::exceeded(format!(
                "the document has {item_count} layers and groups; the most Eskiz opens is {}",
                limits::MAX_LAYERS
            )));
        }
        let mut layers = Vec::new();
        collect_layers(&tree, &mut layers);
        // A layer's mask is kept at canvas size (one byte per pixel) whatever size the file
        // declares for the layer, so many tiny masked layers count as many canvases.
        let canvas_px = u64::from(canvas.0).saturating_mul(u64::from(canvas.1));
        let layer_bytes: u64 = layers
            .iter()
            .map(|l| {
                let pixels = u64::from(l.width).saturating_mul(u64::from(l.height)).saturating_mul(4);
                pixels.saturating_add(if l.mask.is_some() { canvas_px } else { 0 })
            })
            .fold(0, u64::saturating_add);
        if layer_bytes > limits::MAX_DOCUMENT_DECODED_BYTES {
            return Err(limits::exceeded(format!(
                "the document's {} layers need {layer_bytes} bytes at full size; the most Eskiz opens is {}",
                layers.len(),
                limits::MAX_DOCUMENT_DECODED_BYTES
            )));
        }

        let background_color = a.data("backgroundColor").filter(|b| b.len() == 16).map(|b| {
            let mut color = [0f32; 4];
            let (chunks, _) = b.as_chunks::<4>();
            for (slot, chunk) in color.iter_mut().zip(chunks) {
                *slot = f32::from_le_bytes(*chunk);
            }
            color
        });
        let composite = a
            .archived("composite")
            .filter(|c| c.get("UUID").is_some_and(Object::truthy))
            .map(|c| Layer::from_archived(c, canvas));

        for layer in layers.iter().chain(composite.iter()) {
            limits::check_canvas(layer.width, layer.height, &format!("layer {}", layer.uuid))?;
            if let Some(mask) = &layer.mask {
                limits::check_canvas(mask.width, mask.height, &format!("mask {}", mask.uuid))?;
            }
        }

        let tiles = build_tile_index(&names, &prefix);
        Ok(Self {
            zip,
            prefix,
            name: a.str("name").map(str::to_owned),
            width,
            height,
            tile_size: tile_size as u32,
            version: a.i64("version"),
            orientation: a.i64("orientation"),
            flipped_horizontally: a.truthy("flippedHorizontally"),
            flipped_vertically: a.truthy("flippedVertically"),
            background_hidden: a.truthy("backgroundHidden"),
            background_color,
            dpi: a.f64("SilicaDocumentArchiveDPIKey"),
            tracked_time: a.f64("SilicaDocumentTrackedTimeKey"),
            stroke_count: a.i64("strokeCount"),
            author: a.str("authorName").map(str::to_owned),
            color_profile: a
                .archived("colorProfile")
                .and_then(|c| c.str("SiColorProfileArchiveICCNameKey"))
                .map(str::to_owned),
            tree,
            layers,
            composite,
            animation: a.archived("animation").map(|anim| AnimationSpec::read(anim, a)),
            declared_bits: a.i64("bitsPerChannel").and_then(|b| u32::try_from(b).ok()),
            encoding: match a.str("channelFormat") {
                Some("uint16") => Some(TileEncoding::U16Unorm),
                Some("float16") => Some(TileEncoding::F16),
                _ => None,
            },
            depth: None,
            tiles,
        })
    }

    /// The embedded `QuickLook/Thumbnail.png` (or `.jpg`) bytes, if the package has one.
    pub fn thumbnail(&mut self) -> Result<Option<Vec<u8>>> {
        for name in ["QuickLook/Thumbnail.png", "QuickLook/Thumbnail.jpg"] {
            let full = format!("{}{name}", self.prefix);
            if let Some(bytes) = read_member(&mut self.zip, &full)? {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }

    /// Number of stored tiles for a layer UUID (the highest revision of each position).
    pub fn tile_count(&self, uuid: &str) -> usize {
        self.tiles.get(uuid).map_or(0, HashMap::len)
    }

    /// Decodes a layer into premultiplied RGBA8 at the layer's size (top row first).
    /// Missing tiles are transparent. Edge tiles are clipped to the layer bounds. The tiles of a
    /// 16-bit document are rounded to 8 bits; use [`Document::layer_rgba16`] to keep them.
    pub fn layer_rgba(&mut self, layer: &Layer) -> Result<Vec<u8>> {
        self.assemble_rgba(&layer.uuid, layer.width, layer.height, 0x00)
    }

    /// Decodes a layer into premultiplied RGBA16 (top row first), exactly for a 16-bit document
    /// and widened (`v * 257`) for an 8-bit one.
    pub fn layer_rgba16(&mut self, layer: &Layer) -> Result<Vec<u16>> {
        self.assemble_rgba16(&layer.uuid, layer.width, layer.height, 0x0000)
    }

    /// Decodes a layer straight into tiles, in parallel: the tiles that hold any non-zero sample,
    /// each `tile_size x tile_size` pixels (row stride `tile_size`) with a zeroed margin beyond
    /// the layer's edge, as 8 or 16 bits per channel (`bits`). Missing tiles are transparent and
    /// are not returned, nor are tiles that decode to all zeros.
    ///
    /// The pixels equal those of [`Document::layer_rgba`] (`bits` 8) or
    /// [`Document::layer_rgba16`] (`bits` 16) cut into tiles, without building the canvas-sized
    /// buffer. Errors are the ones those methods give; when several tiles are damaged the first
    /// in grid order is reported.
    pub fn layer_tiles(&mut self, layer: &Layer, bits: u32) -> Result<Vec<DecodedTile>> {
        if bits != 8 && bits != 16 {
            return Err(Error::Unsupported(format!("{bits} bits per channel tiles")));
        }
        let float = self.tile_encoding()? == TileEncoding::F16;
        let depth = self.tile_depth()?;
        let placed = self.placed_tiles(&layer.uuid, layer.width, layer.height);
        let job = TileJob {
            uuid: &layer.uuid,
            t: self.tile_size as usize,
            // A known depth is tried alone. The guess tries 4 bytes first, then 8.
            candidates: match depth {
                TileDepth::Known(b) => [b, b],
                TileDepth::Guess => [4, 8],
            },
            bits,
            float,
        };
        let decode = |p: &PlacedTile, raw: Result<Vec<u8>>| -> Result<Option<DecodedTile>> {
            let payload = decompress_tile(&raw?, p.tile.codec, job.t)?;
            Ok(job.convert(p, payload)?.map(|pixels| DecodedTile { col: p.col, row: p.row, pixels }))
        };
        let read = |zip: &mut ZipArchive<R>, p: &PlacedTile| -> Result<Vec<u8>> {
            read_member(zip, &p.tile.member)?.ok_or_else(|| Error::Corrupt(format!("tile {}", p.tile.member)))
        };
        let threads = crate::pack::worker_count(placed.len());
        let mut results: Vec<Result<Option<DecodedTile>>> = Vec::with_capacity(placed.len());
        if threads <= 1 || placed.len() < 3 {
            for p in &placed {
                let raw = read(&mut self.zip, p);
                results.push(decode(p, raw));
            }
        } else {
            // This thread reads the (stored) members one after another; the others decompress.
            let zip = &mut self.zip;
            let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, Result<Vec<u8>>)>(threads * 2);
            let rx = std::sync::Mutex::new(rx);
            let slots: Vec<Option<Result<Option<DecodedTile>>>> = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        scope.spawn(|| {
                            let mut mine = Vec::new();
                            loop {
                                let msg = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                                let Ok((i, raw)) = msg else { break };
                                mine.push((i, decode(&placed[i], raw)));
                            }
                            mine
                        })
                    })
                    .collect();
                for (i, p) in placed.iter().enumerate() {
                    if tx.send((i, read(zip, p))).is_err() {
                        break;
                    }
                }
                drop(tx);
                let mut slots: Vec<Option<Result<Option<DecodedTile>>>> = (0..placed.len()).map(|_| None).collect();
                for w in workers {
                    for (i, r) in w.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)) {
                        slots[i] = Some(r);
                    }
                }
                slots
            });
            results.extend(slots.into_iter().map(|s| s.unwrap_or_else(|| Err(Error::Corrupt("tile was not decoded".into())))));
        }
        let mut out = Vec::new();
        for r in results {
            if let Some(tile) = r? {
                out.push(tile);
            }
        }
        Ok(out)
    }

    /// Decodes a layer's mask into premultiplied RGBA8 at the mask's size (top row first), or
    /// `None` when the layer has no mask. Masks use inverted fill: a missing tile reads as
    /// 0xFF, not 0x00.
    pub fn mask_rgba(&mut self, layer: &Layer) -> Result<Option<Vec<u8>>> {
        let Some(mask) = layer.mask.as_ref() else {
            return Ok(None);
        };
        // A mask with no tiles shows the whole layer: it is no mask, and nothing is allocated.
        if self.placed_tiles(&mask.uuid, mask.width, mask.height).is_empty() {
            return Ok(None);
        }
        self.assemble_rgba(&mask.uuid, mask.width, mask.height, 0xFF).map(Some)
    }

    /// Decodes the stored flattened composite at canvas size, or `None` if there is none.
    pub fn composite_rgba(&mut self) -> Result<Option<Vec<u8>>> {
        let Some(uuid) = self.composite.as_ref().map(|c| c.uuid.clone()) else {
            return Ok(None);
        };
        if !self.tiles.contains_key(&uuid) {
            return Ok(None);
        }
        self.assemble_rgba(&uuid, self.width, self.height, 0x00).map(Some)
    }

    /// Bits per channel of the document's pixels: 8 or 16. Taken from the `bitsPerChannel` key
    /// when the archive has one, otherwise from the document's tile depth ([`TileDepth`]). Only
    /// when no tile is unambiguous is the first tile's own guess used: readers
    /// infer the channel size from the payload length).
    pub fn bits_per_channel(&mut self) -> Result<u32> {
        if let Some(b) = self.declared_bits.filter(|b| *b == 8 || *b == 16) {
            return Ok(b);
        }
        match self.tile_depth()? {
            TileDepth::Known(4) => Ok(8),
            TileDepth::Known(_) => Ok(16),
            TileDepth::Guess => {
                for (uuid, w, h) in self.tile_targets() {
                    let mut bpp = None;
                    self.each_tile(&uuid, w, h, true, &mut |_, _, _, _, _, b, _| {
                        bpp = Some(b);
                        true
                    })?;
                    if let Some(b) = bpp {
                        return Ok(if b == 8 { 16 } else { 8 });
                    }
                }
                Ok(8)
            }
        }
    }

    /// The encoding of the document's 16-bit samples.
    ///
    /// When the archive has Eskiz's `channelFormat` key (`uint16` or `float16`) that value is
    /// used. Otherwise the tiles are scanned once and the result is cached. The rule, applied to
    /// every non-empty 16-bit tile of the document (layers, masks and the composite):
    ///
    /// 1. Any channel word above `0x3C00` (1.0 as a half) is not a valid half in [0, 1], so the
    ///    document is [`TileEncoding::U16Unorm`].
    /// 2. A premultiplied colour above its alpha when both are read as half is not premultiplied
    ///    in that reading, so it is [`TileEncoding::U16Unorm`].
    /// 3. Otherwise every word is a half in [0, 1] with premultiplied colour at most alpha, and
    ///    it is [`TileEncoding::F16`]. As uint16, such a document's alpha would stay at or below
    ///    23% even where opaque, which is implausible; that is why the half reading is chosen.
    /// 4. A 16-bit tile with only zero samples is no evidence either way, so when that is all
    ///    there is the document is [`TileEncoding::U16Unorm`] (the prefer-uint16 default).
    ///
    /// A document with no 16-bit tile at all is [`TileEncoding::U8`]. The scan stops as soon as
    /// rule 1 or 2 applies. Rule 3 can misread a genuine uint16 document whose every word is at
    /// or below `0x3C00` (very faint, very dark content); no key or data separates that case.
    pub fn tile_encoding(&mut self) -> Result<TileEncoding> {
        if let Some(encoding) = self.encoding {
            return Ok(encoding);
        }
        let mut evidence = Evidence::default();
        for (uuid, w, h) in self.tile_targets() {
            // A layer that fails to decode adds no evidence here; reading that layer reports the error.
            let _ = self.each_tile(&uuid, w, h, false, &mut |_, _, _, _, _, bpp, px| {
                bpp != 8 || evidence.observe(px)
            });
            if evidence.settled() {
                break;
            }
        }
        let encoding = evidence.encoding();
        self.encoding = Some(encoding);
        Ok(encoding)
    }

    fn assemble_rgba(&mut self, uuid: &str, width: u32, height: u32, fill: u8) -> Result<Vec<u8>> {
        let (w, h) = (width as usize, height as usize);
        let mut out = alloc_filled(w, h, 4, fill)?;
        let float = self.tile_encoding()? == TileEncoding::F16;
        self.each_tile(uuid, width, height, false, &mut |x0, y0, tw, th, pw, bpp, px| {
            for y in 0..th {
                let dst = ((y0 + y) * w + x0) * 4;
                if bpp == 4 {
                    let src = y * pw * 4;
                    out[dst..dst + tw * 4].copy_from_slice(&px[src..src + tw * 4]);
                } else {
                    // a 16-bit tile in an 8-bit read: round to nearest
                    let src = y * pw * 8;
                    for k in 0..tw * 4 {
                        out[dst + k] = down16(sample16(&px[src + k * 2..], float));
                    }
                }
            }
            true
        })?;
        Ok(out)
    }

    fn assemble_rgba16(&mut self, uuid: &str, width: u32, height: u32, fill: u16) -> Result<Vec<u16>> {
        let (w, h) = (width as usize, height as usize);
        let len = w
            .checked_mul(h)
            .and_then(|p| p.checked_mul(4))
            .ok_or_else(|| Error::Unsupported(format!("{width}x{height} layer is too large")))?;
        let mut out: Vec<u16> = Vec::new();
        out.try_reserve_exact(len)
            .map_err(|_| Error::Unsupported(format!("cannot allocate {len} samples for a {width}x{height} layer")))?;
        out.resize(len, fill);
        let float = self.tile_encoding()? == TileEncoding::F16;
        self.each_tile(uuid, width, height, false, &mut |x0, y0, tw, th, pw, bpp, px| {
            for y in 0..th {
                let dst = ((y0 + y) * w + x0) * 4;
                if bpp == 4 {
                    let src = y * pw * 4;
                    for k in 0..tw * 4 {
                        out[dst + k] = u16::from(px[src + k]) * 257;
                    }
                } else {
                    let src = y * pw * 8;
                    for k in 0..tw * 4 {
                        out[dst + k] = sample16(&px[src + k * 2..], float);
                    }
                }
            }
            true
        })?;
        Ok(out)
    }

    /// The layer directories whose tiles the document has, as `(uuid, width, height)`: the
    /// layers, their masks and the composite.
    fn tile_targets(&self) -> Vec<(String, u32, u32)> {
        let mut targets = Vec::new();
        for layer in self.layers.iter().chain(self.composite.iter()) {
            targets.push((layer.uuid.clone(), layer.width, layer.height));
            if let Some(mask) = &layer.mask {
                targets.push((mask.uuid.clone(), mask.width, mask.height));
            }
        }
        targets
    }

    /// The tiles of a layer directory that fall on the canvas, in grid order, each with its
    /// clipped size. Positions left of, above or past the canvas are skipped.
    fn placed_tiles(&self, uuid: &str, width: u32, height: u32) -> Vec<PlacedTile> {
        let Some(map) = self.tiles.get(uuid) else {
            return Vec::new();
        };
        let (w, h) = (width as usize, height as usize);
        let t = self.tile_size as usize;
        let mut placed: Vec<PlacedTile> = map
            .iter()
            .filter_map(|(&(col, row), tile)| {
                let (Ok(c), Ok(r)) = (usize::try_from(col), usize::try_from(row)) else {
                    return None;
                };
                let (x0, y0) = match (c.checked_mul(t), r.checked_mul(t)) {
                    (Some(x), Some(y)) if x < w && y < h => (x, y),
                    _ => return None,
                };
                Some(PlacedTile {
                    col: c,
                    row: r,
                    x0,
                    y0,
                    tw: (w - x0).min(t),
                    th: (h - y0).min(t),
                    tile: tile.clone(),
                })
            })
            .collect();
        placed.sort_by_key(|p| (p.row, p.col));
        placed
    }

    /// Reads and decompresses one tile payload.
    fn decode_tile(&mut self, tile: &TileRef) -> Result<Vec<u8>> {
        let raw = read_member(&mut self.zip, &tile.member)?
            .ok_or_else(|| Error::Corrupt(format!("tile {}", tile.member)))?;
        decompress_tile(&raw, tile.codec, self.tile_size as usize)
    }

    /// The document's [`TileDepth`], decided on first use and cached.
    fn tile_depth(&mut self) -> Result<TileDepth> {
        if let Some(depth) = self.depth {
            return Ok(depth);
        }
        let depth = self.decide_depth()?;
        self.depth = Some(depth);
        Ok(depth)
    }

    /// Decides the [`TileDepth`]: the `bitsPerChannel` key when there is one, otherwise the first
    /// unambiguous tile in the document. A tile that fails to decode adds no evidence here;
    /// reading its layer reports the error.
    fn decide_depth(&mut self) -> Result<TileDepth> {
        if let Some(b) = self.declared_bits.filter(|b| *b == 8 || *b == 16) {
            return Ok(TileDepth::Known((b / 2) as usize));
        }
        let t = self.tile_size as usize;
        for (uuid, w, h) in self.tile_targets() {
            for p in self.placed_tiles(&uuid, w, h) {
                let Ok(pixels) = self.decode_tile(&p.tile) else {
                    continue;
                };
                if let Some(bpp) = unambiguous_bpp(p.tw, p.th, t, pixels.len()) {
                    return Ok(TileDepth::Known(bpp));
                }
            }
        }
        Ok(TileDepth::Guess)
    }

    /// Decodes every tile of a layer directory and hands it to `f` as
    /// `(x0, y0, tile width, tile height, payload row width, bytes per pixel, payload)`. With
    /// `first_only` it stops after the first tile. `f` returns `false` to stop early.
    ///
    /// Every tile is read at the document's [`TileDepth`]. A payload is the clipped tile or the
    /// full `T x T` tile at that depth; any other length is an error.
    fn each_tile(
        &mut self,
        uuid: &str,
        width: u32,
        height: u32,
        first_only: bool,
        f: &mut TileFn<'_>,
    ) -> Result<()> {
        let depth = self.tile_depth()?;
        let t = self.tile_size as usize;
        // A known depth is tried alone. The guess tries 4 bytes first, then 8.
        let candidates: [usize; 2] = match depth {
            TileDepth::Known(b) => [b, b],
            TileDepth::Guess => [4, 8],
        };
        for p in self.placed_tiles(uuid, width, height) {
            let pixels = self.decode_tile(&p.tile)?;
            let found = candidates.iter().find_map(|&bpp| {
                if pixels.len() == p.tw * p.th * bpp {
                    Some((bpp, p.tw))
                } else if pixels.len() == t * t * bpp {
                    Some((bpp, t))
                } else {
                    None
                }
            });
            let Some((bpp, pw)) = found else {
                let other_bpp = [1usize, 2, 16]
                    .into_iter()
                    .find(|b| pixels.len() == p.tw * p.th * b || pixels.len() == t * t * b);
                return Err(match other_bpp {
                    Some(bpp) => Error::Unsupported(format!(
                        "layer {uuid} uses {bpp} bytes per pixel; only RGBA8 (4) and RGBA16 (8) are supported"
                    )),
                    None => Error::Corrupt(format!(
                        "tile {}~{} of layer {uuid} has {} bytes, expected {} or {}",
                        p.col,
                        p.row,
                        pixels.len(),
                        p.tw * p.th * 4,
                        t * t * 4
                    )),
                });
            };
            let keep_going = f(p.x0, p.y0, p.tw, p.th, pw, bpp, &pixels);
            if first_only || !keep_going {
                break;
            }
        }
        Ok(())
    }
}

/// A tile of a layer on the canvas: its grid position, its origin, its clipped size, and the
/// payload that holds it.
struct PlacedTile {
    col: usize,
    row: usize,
    x0: usize,
    y0: usize,
    tw: usize,
    th: usize,
    tile: TileRef,
}

/// The pixels of one decoded tile (see [`Document::layer_tiles`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TilePixels {
    /// `tile_size * tile_size * 4` premultiplied RGBA8 samples.
    U8(Box<[u8]>),
    /// `tile_size * tile_size * 4` premultiplied RGBA16 samples.
    U16(Box<[u16]>),
}

/// A tile of a layer with pixels in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTile {
    pub col: usize,
    pub row: usize,
    pub pixels: TilePixels,
}

/// What converting one tile's payload needs (see [`Document::layer_tiles`]).
struct TileJob<'a> {
    uuid: &'a str,
    t: usize,
    candidates: [usize; 2],
    bits: u32,
    float: bool,
}

impl TileJob<'_> {
    /// The payload's bytes per pixel and row width in pixels (the clipped tile or the full one),
    /// or the error [`Document::each_tile`] reports for it.
    fn shape(&self, p: &PlacedTile, len: usize) -> Result<(usize, usize)> {
        let t = self.t;
        let found = self.candidates.iter().find_map(|&bpp| {
            if len == p.tw * p.th * bpp {
                Some((bpp, p.tw))
            } else if len == t * t * bpp {
                Some((bpp, t))
            } else {
                None
            }
        });
        if let Some(found) = found {
            return Ok(found);
        }
        let uuid = self.uuid;
        let other_bpp = [1usize, 2, 16].into_iter().find(|b| len == p.tw * p.th * b || len == t * t * b);
        Err(match other_bpp {
            Some(bpp) => Error::Unsupported(format!(
                "layer {uuid} uses {bpp} bytes per pixel; only RGBA8 (4) and RGBA16 (8) are supported"
            )),
            None => Error::Corrupt(format!(
                "tile {}~{} of layer {uuid} has {len} bytes, expected {} or {}",
                p.col,
                p.row,
                p.tw * p.th * 4,
                t * t * 4
            )),
        })
    }

    /// The tile's pixels at the requested depth in a `t x t` tile with a zero margin; `None` if
    /// they are all zero.
    fn convert(&self, p: &PlacedTile, mut payload: Vec<u8>) -> Result<Option<TilePixels>> {
        let (bpp, pw) = self.shape(p, payload.len())?;
        let (t, tw, th) = (self.t, p.tw, p.th);
        let samples = t * t * 4;
        if self.bits == 8 {
            let out: Vec<u8> = if bpp == 4 && tw == t {
                // whole rows: the payload is the tile (plus rows below the layer's edge)
                payload.truncate(th * t * 4);
                payload.resize(samples, 0);
                payload
            } else if bpp == 4 {
                let mut out = vec![0u8; samples];
                for y in 0..th {
                    let src = y * pw * 4;
                    out[y * t * 4..y * t * 4 + tw * 4].copy_from_slice(&payload[src..src + tw * 4]);
                }
                out
            } else {
                let mut out = vec![0u8; samples];
                for y in 0..th {
                    let src = y * pw * 8;
                    for k in 0..tw * 4 {
                        out[y * t * 4 + k] = down16(sample16(&payload[src + k * 2..], self.float));
                    }
                }
                out
            };
            if out.iter().all(|&v| v == 0) {
                return Ok(None);
            }
            Ok(Some(TilePixels::U8(out.into_boxed_slice())))
        } else {
            let mut out = vec![0u16; samples];
            for y in 0..th {
                let dst = y * t * 4;
                if bpp == 4 {
                    let src = y * pw * 4;
                    for k in 0..tw * 4 {
                        out[dst + k] = u16::from(payload[src + k]) * 257;
                    }
                } else {
                    let src = y * pw * 8;
                    for k in 0..tw * 4 {
                        out[dst + k] = sample16(&payload[src + k * 2..], self.float);
                    }
                }
            }
            if out.iter().all(|&v| v == 0) {
                return Ok(None);
            }
            Ok(Some(TilePixels::U16(out.into_boxed_slice())))
        }
    }
}

/// The bytes per pixel that a `len`-byte payload of a `tw` by `th` tile can only be, on a grid of
/// `t`-pixel tiles. A payload fits a depth when it is the clipped tile (`tw * th` pixels) or the
/// full one (`t * t`) at that depth. `Some` only when exactly one of 4 and 8 fits: every valid
/// 8-bit payload fits 4, so a payload that fits only 8 is 16-bit, and the reverse. A payload that
/// fits both is `None`. A 16-bit 128 by 256 edge tile is 262 144 bytes, the same as a full 256 by
/// 256 tile at 4 bytes, so it fits both.
fn unambiguous_bpp(tw: usize, th: usize, t: usize, len: usize) -> Option<usize> {
    let fits = |bpp: usize| len == tw * th * bpp || len == t * t * bpp;
    match (fits(4), fits(8)) {
        (true, false) => Some(4),
        (false, true) => Some(8),
        _ => None,
    }
}

/// Callback of [`Document::each_tile`]: `(x0, y0, tile width, tile height, payload row width, bytes per pixel, payload)`.
/// It returns `false` to stop the walk.
type TileFn<'a> = dyn FnMut(usize, usize, usize, usize, usize, usize, &[u8]) -> bool + 'a;

/// The largest word a half float in [0, 1] can have: `1.0` is `0x3C00`.
const HALF_ONE: u16 = 0x3C00;

/// What the 16-bit tiles of a document say about their encoding (see
/// [`Document::tile_encoding`]).
#[derive(Default)]
struct Evidence {
    /// A 16-bit tile was seen, empty or not.
    sixteen_bit: bool,
    /// A non-zero 16-bit sample was seen.
    nonzero: bool,
    /// A word above [`HALF_ONE`]: no valid half in [0, 1] has one.
    above_one: bool,
    /// A premultiplied colour above its alpha, both read as half.
    over_alpha: bool,
}

impl Evidence {
    /// Folds in one 16-bit tile payload (8 bytes a pixel). Returns `false` once the answer is
    /// settled, so the scan can stop.
    fn observe(&mut self, px: &[u8]) -> bool {
        self.sixteen_bit = true;
        for p in px.as_chunks::<8>().0 {
            let w = [0, 2, 4, 6].map(|i| u16::from_le_bytes([p[i], p[i + 1]]));
            if w == [0; 4] {
                continue;
            }
            self.nonzero = true;
            if w.iter().any(|&v| v > HALF_ONE) {
                self.above_one = true;
                return false;
            }
            let alpha = half_to_f32(w[3]);
            if w[..3].iter().any(|&c| half_to_f32(c) > alpha) {
                self.over_alpha = true;
                return false;
            }
        }
        true
    }

    fn settled(&self) -> bool {
        self.above_one || self.over_alpha
    }

    fn encoding(&self) -> TileEncoding {
        if !self.sixteen_bit {
            TileEncoding::U8
        } else if self.settled() || !self.nonzero {
            TileEncoding::U16Unorm
        } else {
            TileEncoding::F16
        }
    }
}

/// A zero-initialised-by-`fill` buffer of `w * h * channels` bytes, failing cleanly when it cannot
/// be allocated.
fn alloc_filled(w: usize, h: usize, channels: usize, fill: u8) -> Result<Vec<u8>> {
    let len = w
        .checked_mul(h)
        .and_then(|p| p.checked_mul(channels))
        .ok_or_else(|| Error::Unsupported(format!("{w}x{h} layer is too large")))?;
    let mut out = Vec::new();
    out.try_reserve_exact(len)
        .map_err(|_| Error::Unsupported(format!("cannot allocate {len} bytes for a {w}x{h} layer")))?;
    out.resize(len, fill);
    Ok(out)
}

/// One little-endian 16-bit sample as a 0..65535 value: `uint16` as it is, a half float
/// (`float`) as `value * 65535` rounded and clamped.
fn sample16(b: &[u8], float: bool) -> u16 {
    let raw = u16::from_le_bytes([b[0], b[1]]);
    if float {
        (half_to_f32(raw) * 65535.0 + 0.5).clamp(0.0, 65535.0) as u16
    } else {
        raw
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = u32::from((h >> 10) & 0x1F);
    let man = u32::from(h & 0x03FF);
    let bits = match exp {
        0 if man == 0 => sign,
        0 => {
            let shift = man.leading_zeros() - 21;
            sign | ((113 - shift) << 23) | (((man << shift) & 0x03FF) << 13)
        }
        0x1F => sign | 0x7F80_0000 | (man << 13),
        _ => sign | ((exp + 112) << 23) | (man << 13),
    };
    f32::from_bits(bits)
}

pub(crate) fn down16(v: u16) -> u8 {
    ((u32::from(v) + 128) / 257) as u8
}

fn read_member<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<Option<Vec<u8>>> {
    limits::read_entry(zip, name, limits::MAX_ENTRY_BYTES)
}

/// The stored hold of a layer or group, in extra frames. Negative or missing values are 0.
fn hold(a: &ArchivedObject) -> u32 {
    a.i64("animationHeldLength").map_or(0, |n| n.clamp(0, i64::from(u32::MAX)) as u32)
}

fn make_item(obj: &Object, canvas: (u32, u32)) -> Option<Item> {
    let a = obj.as_archived()?;
    if a.has("children") && !a.has("UUID") {
        let children = a
            .array("children")
            .unwrap_or(&[])
            .iter()
            .filter_map(|c| make_item(c, canvas))
            .collect();
        Some(Item::Group(Group {
            name: a.str("name").unwrap_or("").to_owned(),
            opacity: a.f64("opacity").unwrap_or(1.0),
            hidden: a.truthy("isHidden"),
            collapsed: a.truthy("isCollapsed"),
            locked: a.truthy("isLocked"),
            private: a.truthy("isPrivate"),
            children,
            animation_hold: hold(a),
        }))
    } else {
        Some(Item::Layer(Layer::from_archived(a, canvas)))
    }
}

/// Layers and groups in the tree, at every level.
fn count_items(items: &[Item]) -> usize {
    items
        .iter()
        .map(|item| match item {
            Item::Layer(_) => 1,
            Item::Group(group) => 1 + count_items(&group.children),
        })
        .sum()
}

fn collect_layers(items: &[Item], out: &mut Vec<Layer>) {
    for item in items {
        match item {
            Item::Layer(layer) => out.push(layer.clone()),
            Item::Group(group) => collect_layers(&group.children, out),
        }
    }
}

/// Parses a CGSize stored as `"{w, h}"` (or as a two-element array).
fn parse_cg_size(value: Option<&Object>) -> Option<(f64, f64)> {
    match value? {
        Object::String(s) => {
            let inner = s.trim().strip_prefix('{')?.strip_suffix('}')?;
            let (w, h) = inner.split_once(',')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        }
        Object::Array(items) if items.len() == 2 => {
            Some((items[0].as_f64()?, items[1].as_f64()?))
        }
        _ => None,
    }
}

/// Index of tile payloads per layer directory: position to the highest-revision member.
fn build_tile_index(names: &[String], prefix: &str) -> HashMap<String, HashMap<(i64, i64), TileRef>> {
    let mut index: HashMap<String, HashMap<(i64, i64), TileRef>> = HashMap::new();
    for name in names {
        let Some(rel) = name.strip_prefix(prefix) else {
            continue;
        };
        let Some((dir, file)) = rel.split_once('/') else {
            continue;
        };
        let Some((col, row, codec, revision)) = parse_tile_name(file) else {
            continue;
        };
        let layer = index.entry(dir.to_owned()).or_default();
        match layer.get(&(col, row)) {
            Some(old) if old.revision >= revision => {}
            _ => {
                layer.insert(
                    (col, row),
                    TileRef {
                        member: name.clone(),
                        codec,
                        revision,
                    },
                );
            }
        }
    }
    index
}

/// Parses `<col>~<row>.lz4[.<rev>]` or `<col>~<row>.chunk[.<rev>]`.
fn parse_tile_name(file: &str) -> Option<(i64, i64, Codec, u64)> {
    let (col, rest) = file.split_once('~')?;
    let (row, rest) = rest.split_once('.')?;
    let (codec, revision) = match rest.split_once('.') {
        Some((codec, rev)) => (codec, Some(rev)),
        None => (rest, None),
    };
    let codec = match codec {
        "lz4" => Codec::Lz4,
        "chunk" => Codec::Lzo,
        _ => return None,
    };
    let revision = match revision {
        Some(rev) => parse_digits(rev)?,
        None => 0,
    };
    Some((parse_int(col)?, parse_int(row)?, codec, revision))
}

fn parse_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_digits(s: &str) -> Option<u64> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

// ------------------------------------------------------------------------------ writer

/// Layer and group versions written to `Document.archive`. Guessed: the file format notes do not give them.
const LAYER_VERSION: i64 = 1;
const DOCUMENT_VERSION: i64 = 2;

/// A raster layer to write.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSpec {
    pub name: String,
    /// Name of the layer's tile directory. `None` generates a fresh UUID when writing.
    /// Must be unique within the document and must not contain `/`.
    pub uuid: Option<String>,
    /// Opacity from 0 to 1.
    pub opacity: f64,
    /// Blend mode index (`0` is Normal). Indices 18, 27 to 33 and above 34
    /// are rejected.
    pub blend_mode: i64,
    pub hidden: bool,
    pub locked: bool,
    pub clipped: bool,
    /// Lock transparency (`preserve`).
    pub alpha_locked: bool,
    pub private: bool,
    /// Premultiplied RGBA8 at canvas size, top row first. `None` is fully transparent.
    pub pixels: Option<Vec<u8>>,
    /// Premultiplied RGBA16 at canvas size (`width * height * 4` samples), top row first. Used
    /// instead of `pixels` when set. A 16-bit builder writes it as it is; an 8-bit builder
    /// rounds it. `pixels` alone is widened (`v * 257`) by a 16-bit builder.
    pub pixels16: Option<Vec<u16>>,
    /// The layer's pixels as tiles, read in place without a canvas-sized buffer. Used instead of
    /// `pixels` and `pixels16` when set; the source's tile edge must equal the builder's tile
    /// size. The archive is the same as for the equivalent dense buffer.
    pub tiles: Option<LayerTiles>,
    /// The text source, for a text layer. Its pixels are the last rendering of it.
    pub text: Option<TextSpec>,
    /// Extra Frames frames this layer is held for.
    pub animation_hold: u32,
    /// The layer mask: one grey byte per canvas pixel, top row first (255 shows the layer, 0
    /// hides it). Written as its own tile directory with inverted fill.
    pub mask: Option<Vec<u8>>,
}

impl Default for LayerSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            uuid: None,
            opacity: 1.0,
            blend_mode: 0,
            hidden: false,
            locked: false,
            clipped: false,
            alpha_locked: false,
            private: false,
            pixels: None,
            pixels16: None,
            tiles: None,
            text: None,
            animation_hold: 0,
            mask: None,
        }
    }
}

/// A layer group to write.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupSpec {
    pub name: String,
    pub opacity: f64,
    pub hidden: bool,
    pub collapsed: bool,
    pub locked: bool,
    pub private: bool,
    /// Children, top-most first.
    pub children: Vec<Node>,
    /// Extra Frames frames the group is held for.
    pub animation_hold: u32,
}

impl Default for GroupSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            opacity: 1.0,
            hidden: false,
            collapsed: false,
            locked: false,
            private: false,
            children: Vec::new(),
            animation_hold: 0,
        }
    }
}

/// One entry of the layer tree.
// A tree holds a handful of nodes, so the size of a layer spec does not matter here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Layer(LayerSpec),
    Group(GroupSpec),
}

/// Describes a document and writes it as a `.procreate` package.
///
/// Defaults from [`DocumentBuilder::new`]: tile size 256, orientation 1, opaque white background
/// shown, DPI 132, no author or colour profile, and zero time and strokes. The default background
/// and DPI are not given by the file format notes.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentBuilder {
    pub name: String,
    /// Canvas size in pixels.
    pub width: u32,
    pub height: u32,
    /// Tile edge in pixels.
    pub tile_size: u32,
    pub orientation: i64,
    /// Background colour as straight (not premultiplied) RGBA in 0..1.
    pub background_color: [f32; 4],
    pub background_hidden: bool,
    pub dpi: f64,
    /// Time spent on the artwork, in seconds.
    pub tracked_time: f64,
    pub stroke_count: i64,
    pub author: Option<String>,
    /// ICC profile name.
    pub color_profile: Option<String>,
    /// The layer tree, top-most first.
    pub tree: Vec<Node>,
    /// Frames settings and the background and foreground flags.
    pub animation: AnimationSpec,
    /// Bits per channel of the stored tiles: 8 (RGBA8, 4 bytes per pixel, the default) or 16
    /// (RGBA16, 8 bytes per pixel, little-endian `uint16`). A 16-bit archive also records
    /// `bitsPerChannel` and `channelFormat` = `uint16`, so even an empty canvas keeps its depth.
    pub bits_per_channel: u32,
    /// Whether a 16-bit archive carries Eskiz's `bitsPerChannel` and `channelFormat` keys (the
    /// default). Without them, readers must detect the sample encoding from the tiles
    /// ([`Document::tile_encoding`]), and the archive has no composite: the builder only has an
    /// 8-bit composite to write, which would not be in the layers' encoding.
    pub write_eskiz_keys: bool,
}

impl DocumentBuilder {
    /// A document of `width` by `height` pixels with the defaults listed on the type.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            name: "Untitled".into(),
            width,
            height,
            tile_size: DEFAULT_TILE_SIZE as u32,
            orientation: 1,
            background_color: [1.0; 4],
            background_hidden: false,
            dpi: 132.0,
            tracked_time: 0.0,
            stroke_count: 0,
            author: None,
            color_profile: None,
            tree: Vec::new(),
            animation: AnimationSpec::default(),
            bits_per_channel: 8,
            write_eskiz_keys: true,
        }
    }

    /// Builds the complete package in memory.
    ///
    /// Fails with [`Error::InvalidInput`] for a zero canvas, an out-of-range tile size, a pixel
    /// buffer of the wrong length, an invalid blend mode, a non-finite opacity, or duplicate or
    /// malformed UUIDs.
    ///
    /// Layers are cut into tiles, checked, compressed and measured in parallel, straight from the
    /// layer's [`LayerSpec::tiles`] when it has them, and the flattened composite and the
    /// thumbnail are made a tile at a time, so no canvas-sized buffer is built.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.build(None, |pass| {
            let layers = pack::encode_layers(&pass.layers, &pass.grid, pass.bits);
            let composite = pack::composite_and_thumbnail(
                pass.tree,
                &pass.grid,
                pass.bits,
                pass.write_composite.then_some(pass.composite_uuid),
                pass.background,
            )?;
            Ok((layers, composite))
        })
    }

    /// Validates the builder, runs `tiles` to produce the layers' tile members, the composite
    /// and the thumbnail, and assembles the package.
    pub(crate) fn build(
        &self,
        composite_uuid: Option<String>,
        tiles: impl FnOnce(&TilePass<'_>) -> Result<(Vec<LayerOut>, pack::CompositeOut)>,
    ) -> Result<Vec<u8>> {
        let (w, h) = (self.width as usize, self.height as usize);
        if self.width == 0 || self.height == 0 {
            return invalid("canvas size must be non-zero");
        }
        if !(1..=MAX_TILE_SIZE as u32).contains(&self.tile_size) {
            return invalid(format!("tile size {} is out of range", self.tile_size));
        }
        if self.bits_per_channel != 8 && self.bits_per_channel != 16 {
            return invalid(format!("{} bits per channel is not supported (8 or 16)", self.bits_per_channel));
        }
        validate_nodes(&self.tree, w * h * 4, self.tile_size as usize)?;

        let mut uuids = Vec::new();
        collect_uuids(&self.tree, &mut uuids);
        let mut seen = HashSet::new();
        for uuid in &uuids {
            if !seen.insert(uuid.as_str()) {
                return invalid(format!("UUID {uuid} is used by more than one layer"));
            }
        }

        let composite_uuid = composite_uuid.unwrap_or_else(fresh_uuid);
        let tile = self.tile_size as usize;
        let mut specs = Vec::new();
        collect_specs(&self.tree, &mut specs);
        let pass = TilePass {
            layers: specs.into_iter().zip(uuids.iter().map(String::as_str)).collect(),
            grid: pack::Grid::new(w, h, tile),
            bits: self.bits_per_channel,
            tree: &self.tree,
            // The composite is 8-bit here, so it is only written with Eskiz's keys: without them
            // its widened samples would not be in the encoding the layers use.
            write_composite: self.write_eskiz_keys,
            composite_uuid: &composite_uuid,
            background: (!self.background_hidden).then_some(self.background_color),
        };
        let (layer_outs, composite_out) = tiles(&pass)?;
        let mut archive = Archive {
            width: self.width,
            height: self.height,
            grid: pass.grid,
            bits: pass.bits,
            uuids: uuids.into_iter(),
            outs: layer_outs.into_iter(),
            flat: Vec::new(),
            files: Vec::new(),
        };
        let tree = archive.items(&self.tree);
        let thumbnail = composite_out.thumbnail;
        let composite = composite_out.layer.map(|out| archive.layer(&LayerSpec::default(), &composite_uuid, out));

        let background = self.background_color;
        let mut doc = NsObject::new("SilicaDocument")
            .field("name", text(&self.name))
            .field("tileSize", NsValue::Int(tile as i64))
            .field("version", NsValue::Int(DOCUMENT_VERSION))
            .field("legacyBehaviour", NsValue::Int(0))
            .field("featureSet", NsValue::Int(0))
            .field("size", text(&format!("{{{}, {}}}", self.width, self.height)))
            .field(
                "unwrappedLayers",
                NsValue::Array { items: tree, mutable: true },
            )
            .field(
                "layers",
                NsValue::Array { items: archive.flat, mutable: false },
            )
            .field(
                "userSavedSelectionMasks",
                NsValue::Array { items: Vec::new(), mutable: true },
            )
            .field("backgroundColor", NsValue::Data(f32_bytes(&background)))
            .field("backgroundColorHSBA", NsValue::Data(f32_bytes(&hsba(background))))
            .field("backgroundHidden", NsValue::Bool(self.background_hidden))
            .field("orientation", NsValue::Int(self.orientation))
            .field("flippedHorizontally", NsValue::Bool(false))
            .field("flippedVertically", NsValue::Bool(false))
            .field(
                "animation",
                NsValue::Object(
                    NsObject::new("ValkyrieDocumentAnimation")
                        // defaults
                        .field("animationMode", NsValue::Int(self.animation.animation_mode))
                        .field("playbackMode", NsValue::Int(self.animation.playback_mode))
                        .field("playbackDirection", NsValue::Int(self.animation.playback_direction))
                        .field("primaryMixed", NsValue::Bool(self.animation.primary_mixed))
                        .field("frameRate", NsValue::Real(self.animation.frame_rate))
                        .field("onionSkinCount", NsValue::Int(self.animation.onion_skin_count))
                        .field("onionSkinOpacity", NsValue::Real(self.animation.onion_skin_opacity))
                        .field("onionSkinEnabled", NsValue::Bool(self.animation.onion_skin_enabled))
                        .field("skinBlendMode", NsValue::Int(self.animation.skin_blend_mode)),
                ),
            )
            .field("videoEnabled", NsValue::Bool(false))
            .field("SilicaDocumentVideoPurgedKey", NsValue::Bool(false))
            .field("videoCodecKey", NsValue::Int(0))
            .field("SilicaDocumentTrackedTimeKey", NsValue::Real(self.tracked_time))
            .field("SilicaDocumentArchiveDPIKey", NsValue::Real(self.dpi))
            .field("SilicaDocumentArchiveUnitKey", NsValue::Int(0))
            // stored inline so that readers see a plain integer
            .field("strokeCount", NsValue::Int(self.stroke_count))
            .field(
                "drawingguide",
                NsValue::Object(
                    NsObject::new("ValkyrieDrawingGuide")
                        .field("size", text(&format!("{{{}, {}}}", self.width, self.height)))
                        .field("active", NsValue::Bool(false))
                        .field("type", NsValue::Int(0))
                        .field("symmetryType", NsValue::Int(0))
                        .field("mirrored", NsValue::Bool(false))
                        .field("horizonAngle", NsValue::Real(0.0)),
                ),
            )
            .field("isLastItemAnimationBackground", NsValue::Bool(self.animation.last_item_is_background))
            .field("isFirstItemAnimationForeground", NsValue::Bool(self.animation.first_item_is_foreground))
            .field("referenceOrientation", NsValue::Int(0))
            .field("referenceVisibility", NsValue::Bool(false))
            .field("referenceSize", text("{0, 0}"))
            .field("referenceCorner", NsValue::Int(0))
            .field("referenceDocked", NsValue::Bool(false))
            .field("reference3D", NsValue::Bool(false))
            .field("faceBackgroundHidden", NsValue::Bool(false))
            .field("referenceAnchor", NsValue::Int(0))
            .field("referenceKind", text("{0, 0}"));
        if self.bits_per_channel == 16 && self.write_eskiz_keys {
            doc = doc
                .field("bitsPerChannel", NsValue::Int(16))
                .field("channelFormat", text("uint16"));
        }
        if let Some(composite) = composite {
            doc = doc.field("composite", composite);
        }
        if let Some(author) = &self.author {
            doc = doc.field("authorName", text(author));
        }
        if let Some(profile) = &self.color_profile {
            doc = doc.field(
                "colorProfile",
                NsValue::Object(
                    NsObject::new("ValkyrieColorProfile")
                        .field("SiColorProfileArchiveICCNameKey", text(profile)),
                ),
            );
        }
        let archive_bytes = keyedarchive::write(&NsValue::Object(doc))?;

        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let total: usize = archive.files.iter().map(|(name, data)| name.len() + data.len() + 200).sum();
        let mut zip = ZipWriter::new(Cursor::new(Vec::with_capacity(total + archive_bytes.len() + thumbnail.len() + 1024)));
        zip.start_file("Document.archive", deflated)?;
        zip.write_all(&archive_bytes)?;
        zip.start_file("QuickLook/Thumbnail.png", stored)?;
        zip.write_all(&thumbnail)?;
        for (name, data) in &archive.files {
            zip.start_file(name.as_str(), stored)?;
            zip.write_all(data)?;
        }
        Ok(zip.finish()?.into_inner())
    }
}

/// Writes `builder` to `path` atomically: the package goes to `<path>.tmp`, which is then renamed
/// over `path`. On failure the temporary file is removed.
pub fn write(path: impl AsRef<Path>, builder: &DocumentBuilder) -> Result<()> {
    let path = path.as_ref();
    let bytes = builder.to_bytes()?;
    Ok(crate::fsutil::write_atomic(path, &bytes)?)
}

fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::InvalidInput(msg.into()))
}

fn text(s: &str) -> NsValue {
    NsValue::String(s.to_owned())
}

/// Blend mode indices a document may use: the 26 user-facing modes and the
/// alias 34 (Darken). Indices 18 and 27 to 33 are internal and never written.
fn is_valid_blend(mode: i64) -> bool {
    matches!(mode, 0..=17 | 19..=26 | 34)
}

/// The legacy-safe `blend`, `extendedBlend` and the true `extendedBlend2` for a mode.
fn blend_fields(mode: i64) -> (i64, i64, i64) {
    let blend = match mode {
        20..=33 => 0,
        34 => 19,
        m => m,
    };
    let extended = if mode == 34 { 19 } else { mode };
    (blend, extended, mode)
}

fn validate_nodes(nodes: &[Node], canvas_bytes: usize, tile: usize) -> Result<()> {
    for node in nodes {
        match node {
            Node::Layer(layer) => {
                if !is_valid_blend(layer.blend_mode) {
                    return invalid(format!("blend mode {} is not valid", layer.blend_mode));
                }
                if !layer.opacity.is_finite() {
                    return invalid(format!("opacity of layer {:?} is not finite", layer.name));
                }
                if let Some(px) = &layer.pixels {
                    if px.len() != canvas_bytes {
                        return invalid(format!(
                            "layer {:?} has {} bytes of pixels, expected {canvas_bytes}",
                            layer.name,
                            px.len()
                        ));
                    }
                }
                if let Some(px) = &layer.pixels16 {
                    if px.len() != canvas_bytes {
                        return invalid(format!(
                            "layer {:?} has {} 16-bit samples, expected {canvas_bytes}",
                            layer.name,
                            px.len()
                        ));
                    }
                }
                if let Some(mask) = &layer.mask {
                    if mask.len() != canvas_bytes / 4 {
                        return invalid(format!(
                            "layer {:?} has a mask of {} bytes, expected {}",
                            layer.name,
                            mask.len(),
                            canvas_bytes / 4
                        ));
                    }
                }
                if let Some(tiles) = &layer.tiles {
                    if tiles.0.edge() != tile || !matches!(tiles.0.bits(), 8 | 16) {
                        return invalid(format!(
                            "layer {:?} has tiles of edge {} at {} bits, expected edge {tile} at 8 or 16 bits",
                            layer.name,
                            tiles.0.edge(),
                            tiles.0.bits()
                        ));
                    }
                }
                if let Some(uuid) = &layer.uuid {
                    if uuid.is_empty() || uuid.contains('/') {
                        return invalid(format!("UUID {uuid:?} is not a valid directory name"));
                    }
                }
            }
            Node::Group(group) => {
                if !group.opacity.is_finite() {
                    return invalid(format!("opacity of group {:?} is not finite", group.name));
                }
                validate_nodes(&group.children, canvas_bytes, tile)?;
            }
        }
    }
    Ok(())
}

/// Every raster layer of the tree in depth-first order (the order of [`collect_uuids`]).
fn collect_specs<'a>(nodes: &'a [Node], out: &mut Vec<&'a LayerSpec>) {
    for node in nodes {
        match node {
            Node::Layer(layer) => out.push(layer),
            Node::Group(group) => collect_specs(&group.children, out),
        }
    }
}

/// What the tile pass is given: the layers in archive order, the
/// grid, the depth, the layer tree for the composite and the thumbnail.
pub(crate) struct TilePass<'a> {
    pub layers: Vec<(&'a LayerSpec, &'a str)>,
    pub grid: pack::Grid,
    pub bits: u32,
    pub tree: &'a [Node],
    pub write_composite: bool,
    pub composite_uuid: &'a str,
    /// Background colour (straight RGBA), `None` when hidden.
    pub background: Option<[f32; 4]>,
}

/// Collects one UUID per layer in depth-first order, generating the missing ones.
fn collect_uuids(nodes: &[Node], out: &mut Vec<String>) {
    for node in nodes {
        match node {
            Node::Layer(layer) => out.push(layer.uuid.clone().unwrap_or_else(fresh_uuid)),
            Node::Group(group) => collect_uuids(&group.children, out),
        }
    }
}

/// Archive objects for a tree, using the UUIDs in depth-first order and the tile members that
/// the tile pass made for each layer.
struct Archive {
    width: u32,
    height: u32,
    /// The tile grid and depth, for layer masks.
    grid: pack::Grid,
    bits: u32,
    uuids: std::vec::IntoIter<String>,
    /// The tile members of each raster layer in depth-first order.
    outs: std::vec::IntoIter<LayerOut>,
    /// Every raster layer in depth-first order, for the document's `layers` key.
    flat: Vec<NsValue>,
    /// `(member name, payload)` for every tile written.
    files: Vec<(String, Vec<u8>)>,
}

impl Archive {
    fn items(&mut self, nodes: &[Node]) -> Vec<NsValue> {
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            match node {
                Node::Layer(layer) => {
                    let uuid = self.uuids.next().unwrap_or_default();
                    let tiles = self.outs.next().unwrap_or_default();
                    let value = self.layer(layer, &uuid, tiles);
                    self.flat.push(value.clone());
                    out.push(value);
                }
                Node::Group(group) => {
                    let children = self.items(&group.children);
                    out.push(NsValue::Object(
                        NsObject::new("SilicaGroup")
                            .field("name", text(&group.name))
                            .field("children", NsValue::Array { items: children, mutable: true })
                            .field("opacity", NsValue::Real(group.opacity))
                            .field("isCollapsed", NsValue::Bool(group.collapsed))
                            .field("isHidden", NsValue::Bool(group.hidden))
                            .field("isLocked", NsValue::Bool(group.locked))
                            .field("isClipped", NsValue::Bool(false))
                            .field("isPrivate", NsValue::Bool(group.private))
                            .field("animationHeldLength", NsValue::Int(i64::from(group.animation_hold))),
                    ));
                }
            }
        }
        out
    }

    /// Adds the layer's tile members and returns its `SilicaLayer` object.
    fn layer(&mut self, spec: &LayerSpec, uuid: &str, out: LayerOut) -> NsValue {
        let content = out.content;
        self.files.extend(out.files);
        let (blend, extended, extended2) = blend_fields(spec.blend_mode);
        let rect = match content {
            Some((x0, y0, x1, y1)) => {
                let origin = format!("{{{}, {}}}", x0, y0);
                let size = format!("{{{}, {}}}", x1 - x0, y1 - y0);
                format!("{{{}, {}}}", origin, size)
            }
            None => "{{0, 0}, {0, 0}}".to_owned(),
        };
        let object = NsObject::new("SilicaLayer")
                .field("UUID", text(uuid))
                .field("name", text(&spec.name))
                .field("type", NsValue::Int(0))
                .field("version", NsValue::Int(LAYER_VERSION))
                .field("sizeWidth", NsValue::Int(i64::from(self.width)))
                .field("sizeHeight", NsValue::Int(i64::from(self.height)))
                .field("opacity", NsValue::Real(spec.opacity))
                .field("blend", NsValue::Int(blend))
                .field("extendedBlend", NsValue::Int(extended))
                .field("extendedBlend2", NsValue::Int(extended2))
                .field("hidden", NsValue::Bool(spec.hidden))
                .field("clipped", NsValue::Bool(spec.clipped))
                .field("private", NsValue::Bool(spec.private))
                .field("locked", NsValue::Bool(spec.locked))
                .field("preserve", NsValue::Bool(spec.alpha_locked))
                .field("contentsRect", text(&rect))
                .field("contentsRectValid", NsValue::Bool(content.is_some()))
                .field("perspectiveAssisted", NsValue::Bool(false))
                .field("animationHeldLength", NsValue::Int(i64::from(spec.animation_hold)))
                .field("transform", NsValue::Data(identity_transform()));
        let object = match &spec.mask {
            Some(mask) => {
                let mask_uuid = fresh_uuid();
                let out = pack::encode_mask(mask, &self.grid, self.bits, &mask_uuid);
                self.files.extend(out.files);
                object.field(
                    "mask",
                    NsValue::Object(
                        NsObject::new("SilicaLayer")
                            .field("UUID", text(&mask_uuid))
                            .field("name", text(""))
                            .field("type", NsValue::Int(0))
                            .field("version", NsValue::Int(LAYER_VERSION))
                            .field("sizeWidth", NsValue::Int(i64::from(self.width)))
                            .field("sizeHeight", NsValue::Int(i64::from(self.height)))
                            .field("opacity", NsValue::Real(1.0))
                            .field("blend", NsValue::Int(0))
                            .field("extendedBlend", NsValue::Int(0))
                            .field("extendedBlend2", NsValue::Int(0))
                            .field("hidden", NsValue::Bool(false))
                            .field("clipped", NsValue::Bool(false))
                            .field("private", NsValue::Bool(false))
                            .field("locked", NsValue::Bool(false))
                            .field("preserve", NsValue::Bool(false))
                            .field("transform", NsValue::Data(identity_transform())),
                    ),
                )
            }
            None => object,
        };
        NsValue::Object(match &spec.text {
            Some(t) => object.field("text", text_value(t)),
            None => object,
        })
    }
}

fn identity_transform() -> Vec<u8> {
    let mut m = [0f64; 16];
    for i in [0, 5, 10, 15] {
        m[i] = 1.0;
    }
    m.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// RGBA (straight) to HSBA, the order `backgroundColorHSBA` uses.
fn hsba(rgba: [f32; 4]) -> [f32; 4] {
    let [r, g, b, a] = rgba;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let hue = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let sat = if max == 0.0 { 0.0 } else { d / max };
    [hue, sat, max, a]
}

/// Premultiplied bytes of a straight RGBA colour.
pub(crate) fn premultiply(rgba: [f32; 4]) -> [u8; 4] {
    let a = rgba[3].clamp(0.0, 1.0);
    let mut out = [0u8; 4];
    for (o, c) in out.iter_mut().zip(rgba.iter()).take(3) {
        *o = (c.clamp(0.0, 1.0) * a * 255.0).round() as u8;
    }
    out[3] = (a * 255.0).round() as u8;
    out
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A new UUID string (`XXXXXXXX-XXXX-4XXX-8XXX-XXXXXXXXXXXX`), unique within the process.
fn fresh_uuid() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut state = now ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (u64::from(std::process::id()) << 32);
    let a = splitmix64(&mut state);
    let b = splitmix64(&mut state);
    format!(
        "{:08X}-{:04X}-4{:03X}-8{:03X}-{:012X}",
        a >> 32,
        (a >> 16) & 0xFFFF,
        a & 0xFFF,
        (b >> 48) & 0xFFF,
        b & 0xFFFF_FFFF_FFFF
    )
}

/// The archived source of a text layer (the `text` key of a `SilicaLayer`).
///
/// The `.procreate` `Text` object holds `width`, `attributedString` and `transform`. Eskiz stores the
/// string in `attributedString` as `NS.string`, with one dictionary per run (`len` in bytes, and
/// `attributes` keyed by the `eskiz-text` storage keys). The meaning of the keys is `eskiz-text`'s
/// concern; this crate only moves them.
#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    /// Wrap width in layer units (zero means no wrapping).
    pub width: f64,
    pub text: String,
    /// Runs in order. Their lengths add up to the byte length of `text`.
    pub runs: Vec<TextRunSpec>,
    /// Placement in document pixels, as `[a, b, c, d, tx, ty]`.
    pub transform: [f64; 6],
}

/// One run of a text layer: its length in bytes and its attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRunSpec {
    pub len: usize,
    pub attributes: Vec<(String, TextValue)>,
}

/// An attribute value, the three kinds Eskiz stores: text, a number or a flag.
#[derive(Debug, Clone, PartialEq)]
pub enum TextValue {
    Text(String),
    Number(f64),
    Flag(bool),
}

fn text_value(t: &TextSpec) -> NsValue {
    let runs = t
        .runs
        .iter()
        .map(|run| {
            let attributes = run
                .attributes
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        TextValue::Text(s) => NsValue::String(s.clone()),
                        TextValue::Number(n) => NsValue::Real(*n),
                        TextValue::Flag(b) => NsValue::Bool(*b),
                    };
                    (key.clone(), value)
                })
                .collect();
            NsValue::Dict {
                entries: vec![
                    ("len".to_owned(), NsValue::Int(run.len as i64)),
                    ("attributes".to_owned(), NsValue::Dict { entries: attributes, mutable: false }),
                ],
                mutable: false,
            }
        })
        .collect();
    let attributed = NsObject::new("NSAttributedString")
        .field("NS.string", NsValue::String(t.text.clone()))
        .field("runs", NsValue::Array { items: runs, mutable: false });
    let transform = t.transform.iter().map(|v| NsValue::Real(*v)).collect();
    NsValue::Object(
        NsObject::new("Text")
            .field("width", NsValue::Real(t.width))
            .field("attributedString", NsValue::Object(attributed))
            .field("transform", NsValue::Array { items: transform, mutable: false }),
    )
}

fn dict_get<'a>(entries: &'a [(Object, Object)], key: &str) -> Option<&'a Object> {
    entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
}

/// Reads a `Text` object written by [`text_value`]. Malformed runs are skipped; a missing
/// transform is the identity.
fn text_from_archived(a: &ArchivedObject) -> Option<TextSpec> {
    let attributed = a.archived("attributedString")?;
    let text = attributed.str("NS.string").unwrap_or("").to_owned();
    let runs = attributed
        .array("runs")
        .unwrap_or(&[])
        .iter()
        .filter_map(|run| {
            let Object::Dict(entries) = run else { return None };
            let len = dict_get(entries, "len")?.as_i64()?;
            let Object::Dict(attrs) = dict_get(entries, "attributes")? else { return None };
            let attributes = attrs
                .iter()
                .filter_map(|(key, value)| {
                    let key = key.as_str()?.to_owned();
                    let value = match value {
                        Object::String(s) => TextValue::Text(s.clone()),
                        Object::Bool(b) => TextValue::Flag(*b),
                        other => TextValue::Number(other.as_f64()?),
                    };
                    Some((key, value))
                })
                .collect();
            Some(TextRunSpec { len: usize::try_from(len).ok()?, attributes })
        })
        .collect();
    let mut transform = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    if let Some(values) = a.array("transform").filter(|v| v.len() == 6) {
        for (slot, value) in transform.iter_mut().zip(values) {
            *slot = value.as_f64().unwrap_or(*slot);
        }
    }
    Some(TextSpec {
        width: a.f64("width").unwrap_or(0.0),
        text,
        runs,
        transform,
    })
}
