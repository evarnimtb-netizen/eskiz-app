//! Raster and PDF export of premultiplied RGBA8 pictures.
//!
//! Covers JPEG, PNG, TIFF, PDF and per-layer PNG, with atomic writes.
//!
//! * Pixels come in as [`Raster`], premultiplied RGBA8 like the rest of Eskiz. Straight-alpha
//!   formats (PNG, TIFF, WebP and the PDF alpha mask) un-premultiply on the way out.
//! * [`encode_raster`] returns the bytes for one [`RasterFormat`]; [`export_raster`] writes them
//!   atomically: a temporary file in the same folder, flushed, then renamed over the target.
//! * [`export_layers_png`] writes one PNG per layer, named `NN Layer name.png`.
//! * [`encode_pdf`] and [`export_pdf`] write one page per [`Raster`]. Each page embeds the colour
//!   as Flate-compressed RGB, with the alpha channel as a Flate-compressed SMask.
//!   [`export_layers_pdf`] makes one page per visible layer.
//!
//! Every output carries the document DPI. PNG writes it as a pHYs chunk, JPEG as JFIF density,
//! TIFF as X and Y resolution (inch), and PDF as the page size in points (`pixels * 72 / DPI`).
//!
//! When [`ExportOptions::colour_space`] (or [`PdfOptions::colour_space`]) is set, the file carries
//! the matching ICC profile from [`crate::icc`]: a PNG `iCCP` chunk, a JPEG `APP2` `ICC_PROFILE`
//! segment, the TIFF `InterColorProfile` tag, a WebP `ICCP` chunk, or an `ICCBased` colour space in
//! the PDF. The pixels are written as they are; converting them is the caller's business.
//!
//! 16-bit pictures ([`Raster16`]) export to PNG and TIFF at 16 bits per channel
//! ([`encode_raster16`]). JPEG, WebP and PDF cannot hold 16 bits, so those formats (and PDF
//! pages) convert down to 8 bits with an ordered dither first ([`Raster16::to_raster_dithered`]).
//!
//! Not implemented: cancellation and progress reporting. Pages in a
//! PDF are not composited from layers here; the caller passes finished rasters.

use std::borrow::Cow;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use flate2::write::ZlibEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, RgbaImage};
use pdf_writer::{Content, Filter, Name, Pdf, Rect, Ref};
use tiff::encoder::colortype::{RGB16, RGB8, RGBA16, RGBA8};
use tiff::encoder::{Compression, Rational, TiffEncoder};
use tiff::tags::{ResolutionUnit, Tag};

use eskiz_guides::ColourSpace;

use crate::error::{Error, Result};
use crate::icc;

/// Longest layer name kept in a file name, in characters.
const MAX_NAME_CHARS: usize = 100;
/// Fallback for a layer whose name has nothing left after sanitising.
const EMPTY_NAME: &str = "Layer";
/// TIFF tag 269, DocumentName. The `tiff` crate has no named variant for it.
const TAG_DOCUMENT_NAME: u16 = 269;
/// TIFF tag 34675, InterColorProfile.
const TAG_ICC_PROFILE: u16 = 34675;
/// Points per inch, used for the PDF page size.
const POINTS_PER_INCH: f64 = 72.0;
/// Pixels per metre per DPI: 1 inch is 0.0254 m.
const METRES_PER_INCH: f64 = 0.0254;

/// Premultiplied RGBA8 pixels, row-major with the top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    /// Width in pixels, at least 1.
    pub width: u32,
    /// Height in pixels, at least 1.
    pub height: u32,
    /// `width * height * 4` bytes, premultiplied.
    pub rgba: Vec<u8>,
}

impl Raster {
    /// Builds a raster, checking that the byte count matches the size.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let raster = Self {
            width,
            height,
            rgba,
        };
        check_raster(&raster)?;
        Ok(raster)
    }
}

/// Premultiplied RGBA16 pixels, row-major with the top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster16 {
    /// Width in pixels, at least 1.
    pub width: u32,
    /// Height in pixels, at least 1.
    pub height: u32,
    /// `width * height * 4` samples, premultiplied.
    pub rgba: Vec<u16>,
}

impl Raster16 {
    /// Builds a raster, checking that the sample count matches the size.
    pub fn new(width: u32, height: u32, rgba: Vec<u16>) -> Result<Self> {
        let raster = Self { width, height, rgba };
        raster.check()?;
        Ok(raster)
    }

    fn check(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::InvalidInput("raster has no pixels".into()));
        }
        let expected = self.width as usize * self.height as usize * 4;
        if self.rgba.len() != expected {
            return Err(Error::InvalidInput(format!(
                "raster of {}x{} needs {expected} samples, got {}",
                self.width,
                self.height,
                self.rgba.len()
            )));
        }
        Ok(())
    }

    /// This picture at 8 bits per channel, dithered: `(v + t) / 257` with a threshold `t` in
    /// `0..=256` that varies per pixel (interleaved gradient noise), the same for all four
    /// channels so the dither adds no colour noise and keeps `colour <= alpha`. A value that
    /// came from 8 bits (a multiple of 257) is never changed.
    pub fn to_raster_dithered(&self) -> Raster {
        let w = self.width as usize;
        let mut rgba = vec![0u8; self.rgba.len()];
        for (i, (s, o)) in self.rgba.as_chunks::<4>().0.iter().zip(rgba.as_chunks_mut::<4>().0.iter_mut()).enumerate() {
            let t = dither_threshold(i % w, i / w);
            for k in 0..4 {
                o[k] = ((u32::from(s[k]) + t) / 257).min(255) as u8;
            }
        }
        Raster {
            width: self.width,
            height: self.height,
            rgba,
        }
    }
}

/// Dither threshold in `0..=256` for a pixel (matches `eskiz_core::pixel::dither_threshold`).
fn dither_threshold(x: usize, y: usize) -> u32 {
    let n = (0.067_110_56 * x as f32 + 0.005_837_15 * y as f32).fract();
    let ign = (52.982_918 * n).fract();
    ((ign * 257.0) as u32).min(256)
}

/// One layer at canvas size, as the export functions receive it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerRaster {
    /// Layer name. It is sanitised before it is used in a file name.
    pub name: String,
    /// Whether the layer is visible. Hidden layers are exported only when asked for.
    pub visible: bool,
    /// The layer's pixels, at canvas size.
    pub raster: Raster,
}

/// Settings shared by every raster format.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportOptions {
    /// Document resolution, written into the file metadata. Must be finite and positive.
    pub dpi: f64,
    /// Output size relative to the canvas. `1.0` keeps the canvas size. Must be finite and positive.
    pub scale: f64,
    /// Keep alpha. PNG, TIFF and WebP then write RGBA. When `false`, the image is flattened onto
    /// `background` and written as RGB. JPEG is always flattened.
    pub transparent: bool,
    /// Background colour for flattening, as RGB.
    pub background: [u8; 3],
    /// Written to the TIFF DocumentName tag when set.
    pub document_name: Option<String>,
    /// The colour space of the pixels. When set, the matching ICC profile is embedded. `None`
    /// writes an untagged file.
    pub colour_space: Option<ColourSpace>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            dpi: 72.0,
            scale: 1.0,
            transparent: true,
            background: [255, 255, 255],
            document_name: None,
            colour_space: None,
        }
    }
}

/// TIFF compression. Both choices are lossless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TiffCompression {
    /// Raw samples.
    Uncompressed,
    /// LZW.
    Lzw,
}

/// An output format for a single raster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterFormat {
    /// Lossless PNG, with a pHYs chunk for the DPI.
    Png,
    /// JPEG, flattened onto the background colour. `quality` runs from 0 to 100. Values above
    /// 100 are clamped to 100, and 0 is raised to 1, the lowest quality the encoder supports.
    Jpeg {
        /// Quality, 0 to 100.
        quality: u8,
    },
    /// TIFF with X and Y resolution tags in inches.
    Tiff {
        /// Compression method.
        compression: TiffCompression,
    },
    /// Lossless WebP.
    WebP,
}

/// PDF image quality. Each level sets how many pixels the embedded image keeps, relative to the
/// canvas. The page size does not change. *(Eskiz choice; there is a quality
/// choice without fixed levels.)*
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfQuality {
    /// Half the canvas pixels in each direction.
    Low,
    /// Three quarters of the canvas pixels in each direction.
    Medium,
    /// Full canvas pixels.
    High,
}

impl PdfQuality {
    fn pixel_scale(self) -> f64 {
        match self {
            Self::Low => 0.5,
            Self::Medium => 0.75,
            Self::High => 1.0,
        }
    }
}

/// Settings for PDF export.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfOptions {
    /// Document resolution. Sets the page size in points: `pixels * 72 / dpi`.
    pub dpi: f64,
    /// Image quality.
    pub quality: PdfQuality,
    /// The colour space of the pixels. When set, pages use an `ICCBased` colour space with the
    /// matching profile instead of `DeviceRGB`.
    pub colour_space: Option<ColourSpace>,
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            dpi: 72.0,
            quality: PdfQuality::High,
            colour_space: None,
        }
    }
}

/// Encodes one raster in the given format and returns the file bytes.
///
/// The raster is scaled by `options.scale` first. Scaling works on premultiplied values, so edges
/// do not pick up dark fringes.
pub fn encode_raster(
    raster: &Raster,
    format: RasterFormat,
    options: &ExportOptions,
) -> Result<Vec<u8>> {
    check_raster(raster)?;
    check_dpi(options.dpi)?;
    let (out_w, out_h) = scaled_size(raster.width, raster.height, options.scale)?;
    let premultiplied = scale_premultiplied(raster, out_w, out_h);
    let premultiplied: &[u8] = &premultiplied;
    let bg = options.background;
    let dpi = options.dpi;

    match format {
        RasterFormat::Png => {
            if options.transparent {
                png_bytes(
                    out_w,
                    out_h,
                    &straight_rgba(premultiplied),
                    png::ColorType::Rgba,
                    dpi,
                    options.colour_space,
                )
            } else {
                let rgb = flatten_rgb(premultiplied, bg);
                png_bytes(out_w, out_h, &rgb, png::ColorType::Rgb, dpi, options.colour_space)
            }
        }
        RasterFormat::Jpeg { quality } => {
            let rgb = flatten_rgb(premultiplied, bg);
            jpeg_bytes(out_w, out_h, &rgb, quality, dpi, options.colour_space)
        }
        RasterFormat::Tiff { compression } => {
            tiff_bytes(out_w, out_h, premultiplied, options, compression)
        }
        RasterFormat::WebP => {
            let (data, color) = if options.transparent {
                (straight_rgba(premultiplied), ExtendedColorType::Rgba8)
            } else {
                (flatten_rgb(premultiplied, bg), ExtendedColorType::Rgb8)
            };
            let mut out = Vec::new();
            let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
            if let Some(space) = options.colour_space {
                image::ImageEncoder::set_icc_profile(&mut encoder, icc::profile(space)).map_err(encode_error)?;
            }
            encoder.encode(&data, out_w, out_h, color).map_err(encode_error)?;
            Ok(out)
        }
    }
}

/// Encodes a 16-bit raster. PNG and TIFF keep 16 bits per channel; JPEG and WebP convert to 8 bits
/// with dithering first (see [`Raster16::to_raster_dithered`]).
///
/// The raster is scaled by `options.scale` first, at 16-bit precision.
pub fn encode_raster16(raster: &Raster16, format: RasterFormat, options: &ExportOptions) -> Result<Vec<u8>> {
    raster.check()?;
    check_dpi(options.dpi)?;
    if matches!(format, RasterFormat::Jpeg { .. } | RasterFormat::WebP) {
        return encode_raster(&raster.to_raster_dithered(), format, options);
    }
    let (out_w, out_h) = scaled_size(raster.width, raster.height, options.scale)?;
    let scaled: Cow<'_, [u16]> = if (out_w, out_h) == (raster.width, raster.height) {
        Cow::Borrowed(&raster.rgba)
    } else {
        let source = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_raw(raster.width, raster.height, raster.rgba.clone())
            .expect("raster size was checked before scaling");
        Cow::Owned(image::imageops::resize(&source, out_w, out_h, FilterType::Triangle).into_raw())
    };
    let premultiplied: &[u16] = &scaled;
    let bg = options.background.map(|v| u16::from(v) * 257);
    match format {
        RasterFormat::Png => {
            if options.transparent {
                png_bytes16(out_w, out_h, &straight_rgba16(premultiplied), png::ColorType::Rgba, options)
            } else {
                png_bytes16(out_w, out_h, &flatten_rgb16(premultiplied, bg), png::ColorType::Rgb, options)
            }
        }
        RasterFormat::Tiff { compression } => tiff_bytes16(out_w, out_h, premultiplied, options, compression),
        RasterFormat::Jpeg { .. } | RasterFormat::WebP => unreachable!("converted to 8 bits above"),
    }
}

/// Encodes a 16-bit raster and writes it to `path` atomically. See [`encode_raster16`].
pub fn export_raster16(path: impl AsRef<Path>, raster: &Raster16, format: RasterFormat, options: &ExportOptions) -> Result<()> {
    let bytes = encode_raster16(raster, format, options)?;
    write_atomic(path.as_ref(), &bytes)
}

/// Encodes a raster and writes it to `path` atomically. An existing file is replaced.
pub fn export_raster(
    path: impl AsRef<Path>,
    raster: &Raster,
    format: RasterFormat,
    options: &ExportOptions,
) -> Result<()> {
    let bytes = encode_raster(raster, format, options)?;
    write_atomic(path.as_ref(), &bytes)
}

/// Writes one PNG per layer into `dir`, creating the folder if needed, and returns the paths.
///
/// `layers` is bottom to top. The number in each file name is the layer's position in that list,
/// starting at 1 for the bottom layer. Hidden layers are skipped unless `include_hidden` is set.
/// Each PNG is full canvas size (times `options.scale`), and keeps its alpha, so the pixels
/// outside a layer are transparent. `options.transparent` does not apply here.
///
/// A name that already exists in `dir` gets a numeric suffix, for example `01 Sketch 2.png`.
pub fn export_layers_png(
    dir: impl AsRef<Path>,
    layers: &[LayerRaster],
    options: &ExportOptions,
    include_hidden: bool,
) -> Result<Vec<PathBuf>> {
    let dir = dir.as_ref();
    let layer_options = ExportOptions {
        transparent: true,
        ..options.clone()
    };
    fs::create_dir_all(dir).map_err(Error::Io)?;
    let mut written = Vec::new();
    for (index, layer) in layers.iter().enumerate() {
        if !layer.visible && !include_hidden {
            continue;
        }
        let bytes = encode_raster(&layer.raster, RasterFormat::Png, &layer_options)?;
        let file_name = layer_file_name(index, layers.len(), &layer.name);
        let path = unique_path(dir, &file_name);
        write_atomic(&path, &bytes)?;
        written.push(path);
    }
    Ok(written)
}

/// One layer at canvas size, 16 bits per channel: see [`export_layers_png16`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerRaster16 {
    /// Layer name. It is sanitised before it is used in a file name.
    pub name: String,
    /// Whether the layer is visible. Hidden layers are exported only when asked for.
    pub visible: bool,
    /// The layer's pixels, at canvas size.
    pub raster: Raster16,
}

/// [`export_layers_png`] for 16-bit layers: one 16-bit PNG per layer.
pub fn export_layers_png16(
    dir: impl AsRef<Path>,
    layers: &[LayerRaster16],
    options: &ExportOptions,
    include_hidden: bool,
) -> Result<Vec<PathBuf>> {
    let dir = dir.as_ref();
    let layer_options = ExportOptions {
        transparent: true,
        ..options.clone()
    };
    fs::create_dir_all(dir).map_err(Error::Io)?;
    let mut written = Vec::new();
    for (index, layer) in layers.iter().enumerate() {
        if !layer.visible && !include_hidden {
            continue;
        }
        let bytes = encode_raster16(&layer.raster, RasterFormat::Png, &layer_options)?;
        let file_name = layer_file_name(index, layers.len(), &layer.name);
        let path = unique_path(dir, &file_name);
        write_atomic(&path, &bytes)?;
        written.push(path);
    }
    Ok(written)
}

/// The file name for one layer: a 1-based position, then the sanitised name, then `.png`.
///
/// `index` is the layer's position from the bottom, and `count` is the number of layers. The
/// number is padded to at least two digits, and to as many digits as `count` needs.
pub fn layer_file_name(index: usize, count: usize, name: &str) -> String {
    let width = count.to_string().len().max(2);
    format!("{:0width$} {}.png", index + 1, sanitise_name(name))
}

/// Makes a layer name safe to use in a file name.
///
/// Path separators, the Windows-reserved characters `: * ? " < > |` and control characters are
/// removed. Leading and trailing spaces and dots are trimmed, so a name cannot be `..`. The name
/// is cut to 100 characters. An empty result becomes `Layer`. Spaces inside the name are kept.
pub fn sanitise_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .take(MAX_NAME_CHARS)
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c.is_whitespace() || c == '.');
    if trimmed.is_empty() {
        EMPTY_NAME.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Encodes a PDF with one page per raster. Every raster must be the same size.
///
/// Each page is the canvas size in points, `pixels * 72 / dpi`. The image is Flate-compressed RGB,
/// with the alpha channel as a Flate-compressed SMask. An empty slice is an error, since a PDF
/// needs at least one page.
pub fn encode_pdf(pages: &[Raster], options: &PdfOptions) -> Result<Vec<u8>> {
    let first = pages
        .first()
        .ok_or_else(|| Error::InvalidInput("a PDF needs at least one page".into()))?;
    check_dpi(options.dpi)?;
    for page in pages {
        check_raster(page)?;
        if (page.width, page.height) != (first.width, first.height) {
            return Err(Error::InvalidInput(
                "all PDF pages must be the same size".into(),
            ));
        }
    }

    let page_w = points(first.width, options.dpi);
    let page_h = points(first.height, options.dpi);
    let (img_w, img_h) = scaled_size(first.width, first.height, options.quality.pixel_scale())?;

    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    pdf.catalog(catalog_id).pages(page_tree_id);

    let mut page_ids = Vec::with_capacity(pages.len());
    let mut next_id = 3;
    let mut alloc = || {
        let id = Ref::new(next_id);
        next_id += 1;
        id
    };

    // One ICC stream shared by every page's image.
    let icc_id = match options.colour_space {
        Some(space) => {
            let id = alloc();
            let data = deflate(&icc::profile(space))?;
            let mut stream = pdf.icc_profile(id, &data);
            stream.n(3);
            stream.alternate().device_rgb();
            stream.filter(Filter::FlateDecode);
            drop(stream);
            Some(id)
        }
        None => None,
    };

    for page in pages {
        let page_id = alloc();
        let content_id = alloc();
        let image_id = alloc();
        let mask_id = alloc();

        let premultiplied = scale_premultiplied(page, img_w, img_h);
        let (rgb, alpha) = split_straight(&premultiplied);
        let rgb_z = deflate(&rgb)?;
        let alpha_z = deflate(&alpha)?;

        let mut image = pdf.image_xobject(image_id, &rgb_z);
        image.filter(Filter::FlateDecode);
        image
            .width(img_w as i32)
            .height(img_h as i32)
            .bits_per_component(8)
            .s_mask(mask_id);
        match icc_id {
            Some(id) => {
                image.color_space().icc_based(id);
            }
            None => {
                image.color_space().device_rgb();
            }
        }
        drop(image);

        let mut mask = pdf.image_xobject(mask_id, &alpha_z);
        mask.filter(Filter::FlateDecode);
        mask.width(img_w as i32)
            .height(img_h as i32)
            .bits_per_component(8);
        mask.color_space().device_gray();
        drop(mask);

        let mut content = Content::new();
        content.save_state();
        content.transform([page_w, 0.0, 0.0, page_h, 0.0, 0.0]);
        content.x_object(Name(b"Im1"));
        content.restore_state();
        pdf.stream(content_id, &content.finish());

        let mut page_obj = pdf.page(page_id);
        page_obj.media_box(Rect::new(0.0, 0.0, page_w, page_h));
        page_obj.parent(page_tree_id);
        page_obj.contents(content_id);
        page_obj
            .resources()
            .x_objects()
            .pair(Name(b"Im1"), image_id);
        drop(page_obj);

        page_ids.push(page_id);
    }

    pdf.pages(page_tree_id)
        .count(page_ids.len() as i32)
        .kids(page_ids);
    Ok(pdf.finish())
}

/// Encodes the pages and writes them to `path` atomically.
pub fn export_pdf(path: impl AsRef<Path>, pages: &[Raster], options: &PdfOptions) -> Result<()> {
    let bytes = encode_pdf(pages, options)?;
    write_atomic(path.as_ref(), &bytes)
}

/// Writes a PDF with one page per visible layer, in bottom-to-top order.
///
/// Hidden layers are left out unless `include_hidden` is set. Returns an error if no layer ends up
/// on a page.
pub fn export_layers_pdf(
    path: impl AsRef<Path>,
    layers: &[LayerRaster],
    options: &PdfOptions,
    include_hidden: bool,
) -> Result<()> {
    let pages: Vec<Raster> = layers
        .iter()
        .filter(|layer| layer.visible || include_hidden)
        .map(|layer| layer.raster.clone())
        .collect();
    if pages.is_empty() {
        return Err(Error::InvalidInput("no layers to export".into()));
    }
    export_pdf(path, &pages, options)
}

// ---- helpers ---------------------------------------------------------------------------------

fn check_raster(raster: &Raster) -> Result<()> {
    if raster.width == 0 || raster.height == 0 {
        return Err(Error::InvalidInput("raster has no pixels".into()));
    }
    let expected = raster.width as usize * raster.height as usize * 4;
    if raster.rgba.len() != expected {
        return Err(Error::InvalidInput(format!(
            "raster of {}x{} needs {expected} bytes, got {}",
            raster.width,
            raster.height,
            raster.rgba.len()
        )));
    }
    Ok(())
}

fn check_dpi(dpi: f64) -> Result<()> {
    if dpi.is_finite() && dpi > 0.0 {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "DPI must be positive, got {dpi}"
        )))
    }
}

/// Output size after scaling. Each side is rounded to the nearest pixel and is at least 1.
fn scaled_size(width: u32, height: u32, scale: f64) -> Result<(u32, u32)> {
    if !(scale.is_finite() && scale > 0.0) {
        return Err(Error::InvalidInput(format!(
            "scale must be positive, got {scale}"
        )));
    }
    let side = |n: u32| -> Result<u32> {
        let scaled = (f64::from(n) * scale).round().max(1.0);
        if scaled > f64::from(u32::MAX) {
            return Err(Error::InvalidInput("scaled size is too large".into()));
        }
        Ok(scaled as u32)
    };
    Ok((side(width)?, side(height)?))
}

/// Premultiplied pixels at `out_w` by `out_h`. A raster already at that size is borrowed.
fn scale_premultiplied(raster: &Raster, out_w: u32, out_h: u32) -> Cow<'_, [u8]> {
    if (out_w, out_h) == (raster.width, raster.height) {
        return Cow::Borrowed(&raster.rgba);
    }
    // Triangle filtering is a convex blend, so premultiplied values stay at or below alpha.
    let source = RgbaImage::from_raw(raster.width, raster.height, raster.rgba.clone())
        .expect("raster size was checked before scaling");
    let resized = image::imageops::resize(&source, out_w, out_h, FilterType::Triangle);
    Cow::Owned(resized.into_raw())
}

/// Un-premultiplies a channel. Values above the alpha are clamped to 255.
fn unpremultiply(colour: u8, alpha: u8) -> u8 {
    if alpha == 0 {
        return 0;
    }
    let (c, a) = (u32::from(colour), u32::from(alpha));
    ((c * 255 + a / 2) / a).min(255) as u8
}

/// Composites a premultiplied channel over an opaque background channel.
fn over_background(colour: u8, alpha: u8, background: u8) -> u8 {
    let behind = (u32::from(background) * (255 - u32::from(alpha)) + 127) / 255;
    (u32::from(colour) + behind).min(255) as u8
}

/// Straight-alpha RGBA bytes from premultiplied RGBA bytes.
fn straight_rgba(premultiplied: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(premultiplied.len());
    for px in premultiplied.as_chunks::<4>().0 {
        let a = px[3];
        out.extend([
            unpremultiply(px[0], a),
            unpremultiply(px[1], a),
            unpremultiply(px[2], a),
            a,
        ]);
    }
    out
}

/// Flattened RGB bytes over an opaque background colour.
fn flatten_rgb(premultiplied: &[u8], background: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(premultiplied.len() / 4 * 3);
    for px in premultiplied.as_chunks::<4>().0 {
        let a = px[3];
        for channel in 0..3 {
            out.push(over_background(px[channel], a, background[channel]));
        }
    }
    out
}

/// Straight RGB bytes and a separate alpha plane, for the PDF image and its SMask.
fn split_straight(premultiplied: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let pixels = premultiplied.len() / 4;
    let mut rgb = Vec::with_capacity(pixels * 3);
    let mut alpha = Vec::with_capacity(pixels);
    for px in premultiplied.as_chunks::<4>().0 {
        let a = px[3];
        rgb.extend([
            unpremultiply(px[0], a),
            unpremultiply(px[1], a),
            unpremultiply(px[2], a),
        ]);
        alpha.push(a);
    }
    (rgb, alpha)
}

/// Un-premultiplies a 16-bit channel. Values above the alpha are clamped to 65535.
fn unpremultiply16(colour: u16, alpha: u16) -> u16 {
    if alpha == 0 {
        return 0;
    }
    let (c, a) = (u64::from(colour), u64::from(alpha));
    ((c * 65_535 + a / 2) / a).min(65_535) as u16
}

/// Straight-alpha RGBA16 samples from premultiplied RGBA16 samples.
fn straight_rgba16(premultiplied: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(premultiplied.len());
    for px in premultiplied.as_chunks::<4>().0 {
        let a = px[3];
        out.extend([unpremultiply16(px[0], a), unpremultiply16(px[1], a), unpremultiply16(px[2], a), a]);
    }
    out
}

/// Flattened RGB16 samples over an opaque background colour.
fn flatten_rgb16(premultiplied: &[u16], background: [u16; 3]) -> Vec<u16> {
    let mut out = Vec::with_capacity(premultiplied.len() / 4 * 3);
    for px in premultiplied.as_chunks::<4>().0 {
        let a = u64::from(px[3]);
        for channel in 0..3 {
            let behind = (u64::from(background[channel]) * (65_535 - a) + 32_767) / 65_535;
            out.push((u64::from(px[channel]) + behind).min(65_535) as u16);
        }
    }
    out
}

fn png_bytes16(width: u32, height: u32, data: &[u16], color: png::ColorType, options: &ExportOptions) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let ppm = pixels_per_metre(options.dpi);
    let mut info = png::Info::with_size(width, height);
    info.color_type = color;
    info.bit_depth = png::BitDepth::Sixteen;
    info.pixel_dims = Some(png::PixelDimensions {
        xppu: ppm,
        yppu: ppm,
        unit: png::Unit::Meter,
    });
    info.icc_profile = options.colour_space.map(|s| Cow::Owned(icc::profile(s)));
    let encoder = png::Encoder::with_info(&mut out, info).map_err(encode_error)?;
    let mut writer = encoder.write_header().map_err(encode_error)?;
    let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_be_bytes()).collect();
    writer.write_image_data(&bytes).map_err(encode_error)?;
    writer.finish().map_err(encode_error)?;
    Ok(out)
}

fn tiff_bytes16(width: u32, height: u32, premultiplied: &[u16], options: &ExportOptions, compression: TiffCompression) -> Result<Vec<u8>> {
    let method = match compression {
        TiffCompression::Uncompressed => Compression::Uncompressed,
        TiffCompression::Lzw => Compression::Lzw,
    };
    let resolution = Rational {
        n: (options.dpi * 1000.0).round() as u32,
        d: 1000,
    };
    let mut out = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut out).map_err(encode_error)?.with_compression(method);
    if options.transparent {
        let data = straight_rgba16(premultiplied);
        let mut image = encoder.new_image::<RGBA16>(width, height).map_err(encode_error)?;
        write_tiff_metadata(&mut image, resolution, options)?;
        image.write_data(&data).map_err(encode_error)?;
    } else {
        let bg = options.background.map(|v| u16::from(v) * 257);
        let data = flatten_rgb16(premultiplied, bg);
        let mut image = encoder.new_image::<RGB16>(width, height).map_err(encode_error)?;
        write_tiff_metadata(&mut image, resolution, options)?;
        image.write_data(&data).map_err(encode_error)?;
    }
    Ok(out.into_inner())
}

fn encode_error(err: impl std::fmt::Display) -> Error {
    Error::Encode(err.to_string())
}

/// Pixels per metre for a DPI value, for the PNG pHYs chunk.
fn pixels_per_metre(dpi: f64) -> u32 {
    (dpi / METRES_PER_INCH)
        .round()
        .clamp(1.0, f64::from(u32::MAX)) as u32
}

/// Page length in points for a pixel count at the given DPI.
fn points(pixels: u32, dpi: f64) -> f32 {
    (f64::from(pixels) * POINTS_PER_INCH / dpi) as f32
}

fn png_bytes(
    width: u32,
    height: u32,
    data: &[u8],
    color: png::ColorType,
    dpi: f64,
    space: Option<ColourSpace>,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let ppm = pixels_per_metre(dpi);
    let mut info = png::Info::with_size(width, height);
    info.color_type = color;
    info.bit_depth = png::BitDepth::Eight;
    info.pixel_dims = Some(png::PixelDimensions {
        xppu: ppm,
        yppu: ppm,
        unit: png::Unit::Meter,
    });
    // The iCCP chunk comes before the image data; the encoder writes it after the header.
    info.icc_profile = space.map(|s| Cow::Owned(icc::profile(s)));
    let encoder = png::Encoder::with_info(&mut out, info).map_err(encode_error)?;
    let mut writer = encoder.write_header().map_err(encode_error)?;
    writer.write_image_data(data).map_err(encode_error)?;
    writer.finish().map_err(encode_error)?;
    Ok(out)
}

fn jpeg_bytes(width: u32, height: u32, rgb: &[u8], quality: u8, dpi: f64, space: Option<ColourSpace>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let quality = quality.clamp(1, 100);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .encode(rgb, width, height, ExtendedColorType::Rgb8)
        .map_err(encode_error)?;
    set_jfif_density(&mut out, dpi)?;
    if let Some(space) = space {
        insert_jpeg_icc(&mut out, &icc::profile(space))?;
    }
    Ok(out)
}

/// Bytes of profile data one APP2 segment can hold: the 65 535-byte segment length, less the 2-byte
/// length field and the 14-byte `ICC_PROFILE\0` header with its sequence number and count.
const ICC_BYTES_PER_SEGMENT: usize = 65_535 - 2 - 14;

/// Inserts the profile as `ICC_PROFILE` APP2 segments straight after the JFIF APP0 segment, as the
/// ICC specification (annex B.4) describes: a 12-byte identifier, then the 1-based sequence number
/// and the segment count, then the profile bytes.
fn insert_jpeg_icc(jpeg: &mut Vec<u8>, profile: &[u8]) -> Result<()> {
    if jpeg.len() < 4 || jpeg[..4] != [0xFF, 0xD8, 0xFF, 0xE0] {
        return Err(encode_error("JPEG encoder wrote no APP0 segment"));
    }
    let app0_len = usize::from(u16::from_be_bytes([jpeg[4], jpeg[5]]));
    let at = 4 + app0_len;
    if at > jpeg.len() {
        return Err(encode_error("JPEG APP0 segment is truncated"));
    }
    let chunks: Vec<&[u8]> = profile.chunks(ICC_BYTES_PER_SEGMENT).collect();
    let mut segments = Vec::with_capacity(profile.len() + chunks.len() * 18);
    for (i, chunk) in chunks.iter().enumerate() {
        segments.extend([0xFF, 0xE2]);
        segments.extend(((chunk.len() + 2 + 14) as u16).to_be_bytes());
        segments.extend(b"ICC_PROFILE\0");
        segments.extend([(i + 1) as u8, chunks.len() as u8]);
        segments.extend(*chunk);
    }
    jpeg.splice(at..at, segments);
    Ok(())
}

/// A byte string stored with the TIFF type UNDEFINED, which is what the `InterColorProfile` tag
/// calls for (the `tiff` crate writes a plain `&[u8]` as BYTE).
struct Undefined<'a>(&'a [u8]);

impl tiff::encoder::TiffValue for Undefined<'_> {
    const BYTE_LEN: u8 = 1;
    const FIELD_TYPE: tiff::tags::Type = tiff::tags::Type::UNDEFINED;

    fn count(&self) -> usize {
        self.0.len()
    }

    fn data(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(self.0)
    }
}

/// Sets the JFIF density fields to DPI, in dots per inch (units = 1).
///
/// The JFIF APP0 segment follows the SOI marker. Its layout is: the identifier `JFIF\0` (5
/// bytes), the version (2 bytes), the units (1 byte), the X density (2 bytes, big-endian) and the Y
/// density (2 bytes, big-endian).
fn set_jfif_density(bytes: &mut [u8], dpi: f64) -> Result<()> {
    let search = &bytes[..bytes.len().min(32)];
    let at = search
        .windows(5)
        .position(|window| window == b"JFIF\0")
        .ok_or_else(|| encode_error("JPEG encoder wrote no JFIF header"))?;
    let units = at + 7;
    if units + 5 > bytes.len() {
        return Err(encode_error("JPEG JFIF header is truncated"));
    }
    let density = dpi.round().clamp(1.0, f64::from(u16::MAX)) as u16;
    bytes[units] = 1;
    bytes[units + 1..units + 3].copy_from_slice(&density.to_be_bytes());
    bytes[units + 3..units + 5].copy_from_slice(&density.to_be_bytes());
    Ok(())
}

fn tiff_bytes(
    width: u32,
    height: u32,
    premultiplied: &[u8],
    options: &ExportOptions,
    compression: TiffCompression,
) -> Result<Vec<u8>> {
    let method = match compression {
        TiffCompression::Uncompressed => Compression::Uncompressed,
        TiffCompression::Lzw => Compression::Lzw,
    };
    let resolution = Rational {
        n: (options.dpi * 1000.0).round() as u32,
        d: 1000,
    };
    let mut out = Cursor::new(Vec::new());
    let mut encoder = TiffEncoder::new(&mut out)
        .map_err(encode_error)?
        .with_compression(method);

    if options.transparent {
        let data = straight_rgba(premultiplied);
        let mut image = encoder
            .new_image::<RGBA8>(width, height)
            .map_err(encode_error)?;
        write_tiff_metadata(&mut image, resolution, options)?;
        image.write_data(&data).map_err(encode_error)?;
    } else {
        let data = flatten_rgb(premultiplied, options.background);
        let mut image = encoder
            .new_image::<RGB8>(width, height)
            .map_err(encode_error)?;
        write_tiff_metadata(&mut image, resolution, options)?;
        image.write_data(&data).map_err(encode_error)?;
    }
    Ok(out.into_inner())
}

fn write_tiff_metadata<C, W>(
    image: &mut tiff::encoder::ImageEncoder<'_, W, C, tiff::encoder::TiffKindStandard>,
    resolution: Rational,
    options: &ExportOptions,
) -> Result<()>
where
    C: tiff::encoder::colortype::ColorType,
    W: Write + std::io::Seek,
{
    image.resolution(ResolutionUnit::Inch, resolution);
    if let Some(space) = options.colour_space {
        let profile = icc::profile(space);
        image
            .encoder()
            .write_tag(Tag::Unknown(TAG_ICC_PROFILE), Undefined(&profile))
            .map_err(encode_error)?;
    }
    if let Some(name) = &options.document_name {
        image
            .encoder()
            .write_tag(Tag::Unknown(TAG_DOCUMENT_NAME), name.as_str())
            .map_err(encode_error)?;
    }
    Ok(())
}

fn deflate(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).map_err(Error::Io)?;
    encoder.finish().map_err(Error::Io)
}

/// The path for `file_name` in `dir`, with `" 2"`, `" 3"` and so on added before the extension if
/// the name is taken.
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let first = dir.join(file_name);
    if !first.exists() {
        return first;
    }
    let stem = file_name.strip_suffix(".png").unwrap_or(file_name);
    let mut n = 2u32;
    loop {
        let candidate = dir.join(format!("{stem} {n}.png"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Writes `bytes` to a temporary file in the same folder, flushes it, then renames it over
/// `path`. On failure the temporary file is removed and `path` is left untouched.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.file_name().is_none() {
        return Err(Error::InvalidInput(format!("no file name in {}", path.display())));
    }
    crate::fsutil::write_atomic(path, bytes).map_err(Error::Io)
}
