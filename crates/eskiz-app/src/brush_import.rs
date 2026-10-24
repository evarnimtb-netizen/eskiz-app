//! Brush import: reads `.brush`, `.brushset` and `.abr` files, maps the
//! settings onto eskiz-core brushes, and keeps imported brushes in the app data folder.
//!
//! The mapping is deliberately small. Size, spacing, Smoothing smoothing and the two tapers come
//! from `eskiz_format::brushes::engine_params`. The tip and grain PNGs are decoded to grey for
//! `ShapeTexture::from_gray` and `GrainTexture::from_gray`, where white is paint. Other dynamics
//! are not mapped yet.
//!
//! Each imported brush is stored as one `.brush` bundle in [`BrushStore`], written by the format
//! crate, so the set survives restarts and the files can be copied to another machine.

use std::fs;
use std::path::{Path, PathBuf};

use eskiz_brushlib::grains::GrainKind;
use eskiz_brushlib::tips::TipKind;
use eskiz_brushlib::LibraryBrush;
use eskiz_core::brushes::{Brush, Grain, GrainMode, Shape, Taper, TaperPair};
use eskiz_format::brushes::{self, engine_params, ImportedBrush, TaperParams};
use image::GrayImage;

use crate::brush_settings;

/// The name of the set that holds imported brushes.
pub const IMPORTED_SET: &str = "Imported";
/// The name of the set that holds brushes made or saved in the Brush Bench.
pub const MY_BRUSHES_SET: &str = "My Brushes";
/// Extensions the importer accepts.
pub const EXTENSIONS: [&str; 3] = ["brush", "brushset", "abr"];
/// Edge length of the generated round tip for brushes without a tip PNG.
const ROUND_TIP: u32 = 128;

/// True for a file the brush importer reads, judged by its extension.
pub fn is_brush_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Reads every brush in a file. A `.brushset` or `.abr` may hold several. Returns the brushes,
/// and one note for each part that was skipped.
pub fn read_file(path: &Path) -> Result<(Vec<ImportedBrush>, Vec<String>), String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let hint = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let read_err = |e: eskiz_format::Error| format!("{}: {e}", path.display());
    match ext.as_str() {
        "brush" => Ok((vec![brushes::read_brush(hint, &bytes).map_err(read_err)?], Vec::new())),
        "brushset" => {
            let set = brushes::read_brushset(hint, &bytes).map_err(read_err)?;
            Ok((set.brushes, set.failures))
        }
        "abr" => {
            let abr = brushes::read_abr(&bytes).map_err(read_err)?;
            Ok((abr.brushes, abr.warnings))
        }
        _ => Err(format!("{}: not a brush file", path.display())),
    }
}

/// `ib` with the keys of older releases (`eskiz.*`) renamed to the current ones.
fn upgraded(ib: &ImportedBrush) -> std::borrow::Cow<'_, ImportedBrush> {
    if ib.settings.keys().any(|k| k.starts_with(brush_settings::LEGACY_PREFIX)) {
        let mut copy = ib.clone();
        brush_settings::upgrade_legacy_keys(&mut copy);
        std::borrow::Cow::Owned(copy)
    } else {
        std::borrow::Cow::Borrowed(ib)
    }
}

/// Engine settings for an imported brush, from the values `engine_params` reads.
pub fn engine_brush(ib: &ImportedBrush) -> Brush {
    let ib = &upgraded(ib);
    let p = engine_params(ib);
    let mut brush = Brush {
        name: "Imported brush",
        shape: Shape::HardRound,
        size: p.size,
        min_size: p.min_size,
        spacing: p.spacing,
        smoothing: p.smoothing,
        taper: TaperPair {
            finger: taper(&p.taper),
            pencil: taper(&p.pencil_taper),
        },
        ..Brush::default()
    };
    if ib.grain_png.is_some() {
        brush.grain = Grain::Noise;
        brush.grain_mode = GrainMode::Texturized;
    }
    // A brush saved by the Brush Bench carries every setting under `eskiz.*` keys.
    crate::brush_settings::apply_settings(ib, &mut brush);
    brush
}

fn taper(t: &TaperParams) -> Taper {
    Taper {
        start_length: t.start_length,
        end_length: t.end_length,
        size: t.size,
        opacity: t.opacity,
        shape: t.shape,
        pressure: t.pressure,
        tip_animation: false,
    }
}

/// An imported brush as a library brush: its engine settings, plus a grey tip and grain. A brush
/// without a tip PNG gets a generated round tip, and one without a grain PNG gets no grain.
/// Fails when a PNG cannot be decoded or is not square.
pub fn library_brush(ib: &ImportedBrush) -> Result<LibraryBrush, String> {
    let ib = &upgraded(ib);
    // Eskiz's own grey masks are decoded as they are; foreign PNGs get the paper-colour guess.
    let own = ib.flag(brush_settings::GRAY_PAINT).unwrap_or(false);
    let decode = |png: &[u8]| decode_gray_png_with(png, !own);
    let tip = match &ib.shape_png {
        Some(png) => decode(png).map_err(|e| format!("tip: {e}"))?,
        None => round_tip(ROUND_TIP),
    };
    let grain = match &ib.grain_png {
        Some(png) => Some(decode(png).map_err(|e| format!("grain: {e}"))?),
        None => None,
    };
    let kind = |name: &str| ib.text(name);
    Ok(LibraryBrush {
        id: "",
        name: "",
        brush: engine_brush(ib),
        tip_kind: if ib.shape_png.is_some() { kind(brush_settings::TIP_KIND).and_then(TipKind::from_key) } else { None },
        grain_kind: kind(brush_settings::GRAIN_KIND).and_then(GrainKind::from_key).filter(|_| grain.is_some()),
        tip,
        grain,
    })
}

/// An 8-bit grey PNG of `img`, white is paint. [`decode_gray_png_with`] reads it back exactly.
pub fn encode_gray_png(img: &GrayImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width(), img.height());
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(img.as_raw()).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// A generated round tip: white at the centre, falling to black at the rim.
fn round_tip(n: u32) -> GrayImage {
    let nf = f64::from(n);
    GrayImage::from_fn(n, n, |x, y| {
        let dx = (f64::from(x) + 0.5) / nf * 2.0 - 1.0;
        let dy = (f64::from(y) + 0.5) / nf * 2.0 - 1.0;
        let a = ((1.0 - dx.hypot(dy)) * nf / 2.0 + 0.5).clamp(0.0, 1.0);
        image::Luma([(a * 255.0).round() as u8])
    })
}

/// Decodes a square PNG to grey, where white is paint.
///
/// Colour becomes luma and alpha multiplies it, so transparent areas are not paint. A tip whose
/// opaque border is bright is a dark shape on white paper, so it is inverted.
pub fn decode_gray_png(bytes: &[u8]) -> Result<GrayImage, String> {
    decode_gray_png_with(bytes, true)
}

/// [`decode_gray_png`], with the dark-shape-on-paper inversion optional. Eskiz's own brush files
/// pass `false`: their masks are already white-is-paint, and a tip with bright corners (a square)
/// must not be flipped.
pub fn decode_gray_png_with(bytes: &[u8], guess_inversion: bool) -> Result<GrayImage, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    // Check the header before allocating: a small file can claim a huge image.
    let (hw, hh) = (reader.info().width, reader.info().height);
    let max = eskiz_format::limits::MAX_BRUSH_TIP_SIDE;
    if hw > max || hh > max {
        return Err(format!("{hw} x {hh} is larger than the {max} pixel limit for a brush image"));
    }
    let size = reader.output_buffer_size().ok_or("the image is too large")?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || w != h {
        return Err(format!("{w} x {h} is not a square image"));
    }
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err("indexed color was not expanded".into()),
    };
    let data = &buf[..w * h * channels];
    // (luma, alpha) of the pixel at index `i` of the image.
    let sample = |i: usize| -> (u8, u8) {
        let px = &data[i * channels..(i + 1) * channels];
        match channels {
            1 => (px[0], 255),
            2 => (px[0], px[1]),
            3 => (luma(px[0], px[1], px[2]), 255),
            _ => (luma(px[0], px[1], px[2]), px[3]),
        }
    };
    let mut gray: Vec<u8> = (0..w * h)
        .map(|i| {
            let (l, a) = sample(i);
            (u16::from(l) * u16::from(a) / 255) as u8
        })
        .collect();
    let corners = [0, w - 1, (h - 1) * w, w * h - 1];
    if guess_inversion && corners.iter().all(|&i| {
        let (l, a) = sample(i);
        a == 255 && l >= 128
    }) {
        gray.iter_mut().for_each(|g| *g = 255 - *g);
    }
    GrayImage::from_raw(w as u32, h as u32, gray).ok_or_else(|| "bad image buffer".to_owned())
}

/// Rec. 601 luma of an sRGB pixel, as a byte.
fn luma(r: u8, g: u8, b: u8) -> u8 {
    ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29 + 128) / 256) as u8
}

/// Imported brushes on disk: one `.brush` bundle per brush.
#[derive(Clone, Debug)]
pub struct BrushStore {
    root: PathBuf,
}

impl BrushStore {
    /// A store in `root`. The folder is created on the first save.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The store in the app data folder (see `storage::brushes_dir`).
    pub fn default_location() -> Self {
        Self::at(crate::storage::brushes_dir())
    }

    /// The store of My Brushes, in a folder of its own beside the imported ones.
    pub fn user_location() -> Self {
        Self::at(crate::storage::user_brushes_dir())
    }

    /// Is there a file for `key`?
    pub fn contains(&self, key: &str) -> bool {
        self.root.join(format!("{}.brush", safe_name(key))).exists()
    }

    /// Every stored brush as `(key, brush)`, sorted by key. The key is the file name without its
    /// extension. Unreadable files are skipped.
    pub fn load(&self) -> Vec<(String, ImportedBrush)> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out: Vec<(String, ImportedBrush)> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "brush"))
            .filter_map(|p| {
                let key = p.file_stem()?.to_str()?.to_owned();
                let bytes = fs::read(&p).ok()?;
                let brush = brushes::read_brush(&key, &bytes).ok()?;
                Some((key, brush))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Writes `brush` as a new file and returns its key. The key never clashes with a file that
    /// is already there.
    pub fn save(&self, brush: &ImportedBrush) -> Result<String, String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let bytes = brushes::write_brush(brush).map_err(|e| e.to_string())?;
        let base = safe_name(&brush.name);
        let mut key = base.clone();
        let mut n = 2;
        while self.root.join(format!("{key}.brush")).exists() {
            key = format!("{base} {n}");
            n += 1;
        }
        crate::storage::write_atomic(&self.root.join(format!("{key}.brush")), &bytes)?;
        Ok(key)
    }

    /// Writes `brush` over the file `key`, or creates it. For a brush that is edited in place.
    pub fn write(&self, key: &str, brush: &ImportedBrush) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let bytes = brushes::write_brush(brush).map_err(|e| e.to_string())?;
        crate::storage::write_atomic(&self.root.join(format!("{}.brush", safe_name(key))), &bytes)
    }

    /// Deletes the file `key`. A file that is already gone is not an error.
    pub fn remove(&self, key: &str) -> Result<(), String> {
        match fs::remove_file(self.root.join(format!("{}.brush", safe_name(key)))) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// A file-name-safe version of a brush name. Brush keys are stored in prefs, so they never hold
/// the `|` separator.
pub fn safe_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "Brush".to_owned()
    } else {
        clean.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eskiz_format::brushes::{keys, SettingValue};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eskiz-brush-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// An RGBA PNG of `w x h` pixels, `px(x, y)` giving each pixel.
    fn png_rgba(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&px(x, y));
            }
        }
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            writer.write_image_data(&data).unwrap();
        }
        out
    }

    #[test]
    fn a_huge_brush_image_header_is_refused_before_decoding() {
        // A 1 x 1 PNG whose header claims 40000 x 40000: it must not allocate the claimed size.
        let mut png = png_rgba(1, 1, |_, _| [255; 4]);
        png[16..20].copy_from_slice(&40_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&40_000u32.to_be_bytes());
        // The IHDR CRC covers its type and data (bytes 12..29).
        let mut crc = 0xffff_ffffu32;
        for &byte in &png[12..29] {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
            }
        }
        png[29..33].copy_from_slice(&(!crc).to_be_bytes());
        let err = decode_gray_png(&png).unwrap_err();
        assert!(err.contains("limit"), "{err}");
    }

    #[test]
    fn brushes_saved_with_the_old_key_prefix_still_load() {
        // A brush as an older release wrote it: every setting under `eskiz.*`.
        let mut ib = ImportedBrush { name: "Old".into(), ..ImportedBrush::default() };
        ib.settings.insert("easel.version".into(), SettingValue::Number(1.0)); // legacy-easel
        ib.settings.insert("easel.shape".into(), SettingValue::Text("hard_round".into())); // legacy-easel
        ib.settings.insert("easel.grayPaint".into(), SettingValue::Bool(true)); // legacy-easel
        ib.settings.insert("easel.tipKind".into(), SettingValue::Text("round".into())); // legacy-easel
        let b = engine_brush(&ib);
        assert_eq!(b.shape, Shape::HardRound);
        let mut migrated = ib.clone();
        brush_settings::upgrade_legacy_keys(&mut migrated);
        assert_eq!(migrated.flag(brush_settings::GRAY_PAINT), Some(true));
        assert!(migrated.settings.keys().all(|k| !k.starts_with(brush_settings::LEGACY_PREFIX)));
        // New files use the new prefix and are read as before.
        let mut fresh = ImportedBrush::default();
        fresh.settings.extend(brush_settings::to_settings(&b));
        assert!(fresh.settings.keys().filter(|k| k.starts_with("eskiz.")).count() > 5);
        assert_eq!(engine_brush(&fresh).shape, Shape::HardRound);
    }

    #[test]
    fn engine_settings_come_from_the_file() {
        let mut ib = ImportedBrush { name: "Test".into(), ..ImportedBrush::default() };
        ib.settings.insert(keys::MAX_SIZE.into(), SettingValue::Number(42.0));
        ib.settings.insert(keys::MIN_SIZE.into(), SettingValue::Number(10.5));
        ib.settings.insert(keys::PLOT_SPACING.into(), SettingValue::Number(0.2));
        ib.settings.insert(keys::PLOT_SMOOTHING.into(), SettingValue::Number(0.6));
        ib.settings.insert(keys::TAPER_START_LENGTH.into(), SettingValue::Number(0.3));
        let b = engine_brush(&ib);
        assert_eq!(b.size, 42.0);
        assert!((b.min_size - 10.5 / 42.0).abs() < 1e-12);
        assert_eq!(b.spacing, 0.2);
        assert_eq!(b.smoothing, 0.6);
        assert_eq!(b.taper.finger.start_length, 0.3);
        assert_eq!(b.grain, Grain::None, "no grain PNG, no grain");
    }

    #[test]
    fn a_grain_png_turns_grain_on_and_missing_settings_keep_defaults() {
        let ib = ImportedBrush { grain_png: Some(png_rgba(1, 1, |_, _| [255, 255, 255, 255])), ..ImportedBrush::default() };
        let b = engine_brush(&ib);
        assert_eq!(b.grain, Grain::Noise);
        assert_eq!(b.grain_mode, GrainMode::Texturized);
        let plain = engine_brush(&ImportedBrush::default());
        assert_eq!(plain.spacing, 0.1, "format default spacing");
        assert_eq!(plain.size, 20.0, "format default size");
    }

    #[test]
    fn transparent_and_opaque_pixels_decode_to_grey() {
        // Opaque white centre, transparent border: the border is not paint.
        let png = png_rgba(4, 4, |x, y| if (1..3).contains(&x) && (1..3).contains(&y) { [255, 255, 255, 255] } else { [255, 255, 255, 0] });
        let g = decode_gray_png(&png).unwrap();
        assert_eq!(g.get_pixel(0, 0)[0], 0, "transparent corner");
        assert_eq!(g.get_pixel(1, 1)[0], 255, "opaque white center");
        // Dark shape on opaque white paper: the paper corners are bright, so the tip is inverted.
        let png = png_rgba(4, 4, |x, y| if (1..3).contains(&x) && (1..3).contains(&y) { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        let g = decode_gray_png(&png).unwrap();
        assert_eq!(g.get_pixel(0, 0)[0], 0);
        assert_eq!(g.get_pixel(1, 1)[0], 255, "black shape becomes paint");
    }

    #[test]
    fn non_square_tips_are_refused() {
        let png = png_rgba(4, 2, |_, _| [255; 4]);
        assert!(decode_gray_png(&png).unwrap_err().contains("not a square"));
        assert!(library_brush(&ImportedBrush { shape_png: Some(png), ..ImportedBrush::default() }).is_err());
    }

    #[test]
    fn a_brush_without_textures_gets_a_round_tip() {
        let lb = library_brush(&ImportedBrush::default()).unwrap();
        assert_eq!(lb.tip.width(), ROUND_TIP);
        assert_eq!(lb.tip.get_pixel(ROUND_TIP / 2, ROUND_TIP / 2)[0], 255);
        assert_eq!(lb.tip.get_pixel(0, 0)[0], 0);
        assert!(lb.grain.is_none());
    }

    #[test]
    fn store_round_trips_a_brush_and_never_overwrites() {
        let dir = temp_dir("store");
        let store = BrushStore::at(&dir);
        let mut ib = ImportedBrush { name: "Sumi | ink".into(), ..ImportedBrush::default() };
        ib.settings.insert(keys::PLOT_SPACING.into(), SettingValue::Number(0.25));
        let k1 = store.save(&ib).unwrap();
        let k2 = store.save(&ib).unwrap();
        assert_ne!(k1, k2, "a second save gets its own file");
        assert!(!k1.contains('|'), "keys never hold the prefs separator: {k1}");
        let loaded = store.load();
        assert_eq!(loaded.len(), 2);
        let (key, back) = &loaded[0];
        assert_eq!(key, &k1);
        assert_eq!(back.name, "Sumi | ink", "the name inside the file is kept");
        assert_eq!(back.number(keys::PLOT_SPACING), Some(0.25));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn safe_name_keeps_plain_names_and_cleans_the_rest() {
        assert_eq!(safe_name("Soft Ink-2"), "Soft Ink-2");
        assert_eq!(safe_name("a|b/c"), "a_b_c");
        assert_eq!(safe_name("  "), "Brush");
    }
}
