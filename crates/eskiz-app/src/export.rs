//! Export from the Share sheet, and small file-path helpers.
//!
//! [`export_with`] writes the document in one [`ExportFormat`] with the sheet's
//! [`ExportSettings`]: the layered `.procreate` package and PSD, PDF (one flattened page, or one
//! page per layer), flattened PNG, JPEG, TIFF and WebP, and one PNG per layer in a folder. The
//! raster work is done by `eskiz-format::export`, the layered files by `eskiz-format` too.
//! Every format that can carry one embeds the ICC profile of the document colour space;
//! the pixels are written as they are, tagged, never converted.
//! It takes a `&Document`, so the app can clone the document and run it on a background thread (`share.rs`).

use std::fmt::Display;
use std::path::{Path, PathBuf};

use eskiz_core::{composite, composite16, BlendMode, CompositeOpts, Depth, Node, NodeKind};
use eskiz_format::export::{
    export_layers_pdf, export_layers_png, export_pdf, export_raster, export_raster16, ExportOptions, LayerRaster, PdfOptions, PdfQuality,
    Raster, Raster16, RasterFormat, TiffCompression,
};
use eskiz_format::psd::{self, PsdBlend, PsdDocument, PsdGroup, PsdLayer, PsdNode};

use crate::document::Document;

/// Resolution written to exported images and packages when the sheet does not say otherwise.
pub const DEFAULT_DPI: u16 = 132;
/// JPEG quality, 0..=100.
pub const JPEG_QUALITY: u8 = 90;

/// What the Share sheet can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportFormat {
    /// Eskiz's layered package (`.procreate`).
    Procreate,
    /// Photoshop document with the layer tree.
    Psd,
    /// PDF: one flattened page, or one page per layer when [`ExportSettings::pdf_per_layer`] is set.
    Pdf,
    /// Lossless PNG.
    Png,
    /// JPEG at [`ExportSettings::jpeg_quality`], always flattened onto the paper colour.
    Jpeg,
    /// TIFF with LZW compression.
    Tiff,
    /// Lossless WebP.
    WebP,
    /// One PNG per visible layer, in a folder named by the path.
    Layers,
}

impl ExportFormat {
    pub const ALL: [Self; 8] = [Self::Procreate, Self::Psd, Self::Pdf, Self::Png, Self::Jpeg, Self::Tiff, Self::WebP, Self::Layers];

    /// The file extension, without the dot. Empty for the folder format.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Procreate => "procreate",
            Self::Psd => "psd",
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::WebP => "webp",
            Self::Layers => "",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Procreate => ".procreate",
            Self::Psd => "PSD",
            Self::Pdf => "PDF",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Tiff => "TIFF",
            Self::WebP => "WebP",
            Self::Layers => "layer PNGs",
        }
    }

    /// Does the path name a folder rather than a file?
    pub fn is_folder(self) -> bool {
        matches!(self, Self::Layers)
    }

    /// Can the output keep transparent pixels when the background is left out?
    fn keeps_alpha(self) -> bool {
        matches!(self, Self::Png | Self::Tiff | Self::WebP | Self::Pdf)
    }
}

/// The options on the Share sheet. [`Default`] is the sheet's starting state.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSettings {
    /// Output size as a percentage of the canvas (100 keeps it). Applies to PNG, JPEG, TIFF,
    /// WebP and the per-layer PNGs; PDF, PSD and `.procreate` are written at canvas size.
    pub scale_percent: f64,
    /// Leave the paper colour out so transparent pixels stay transparent (PNG, TIFF, WebP, PDF).
    /// Off flattens the picture onto the paper colour.
    pub transparent: bool,
    /// Pixels per inch, written into the file's resolution fields.
    pub dpi: f64,
    /// JPEG quality, 0 to 100.
    pub jpeg_quality: u8,
    /// PDF with one page per visible layer, instead of one flattened page.
    pub pdf_per_layer: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            scale_percent: 100.0,
            transparent: false,
            dpi: f64::from(DEFAULT_DPI),
            jpeg_quality: JPEG_QUALITY,
            pdf_per_layer: false,
        }
    }
}

impl ExportSettings {
    /// Checks the numbers the sheet can type in. The message is shown in a toast.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.scale_percent.is_finite() && (1.0..=400.0).contains(&self.scale_percent)) {
            return Err("Scale must be between 1 and 400 percent".to_owned());
        }
        if !(self.dpi.is_finite() && (1.0..=2400.0).contains(&self.dpi)) {
            return Err("Resolution must be between 1 and 2400 DPI".to_owned());
        }
        Ok(())
    }

    fn scale(&self) -> f64 {
        self.scale_percent / 100.0
    }
}

/// `path` with the export extension. A known picture or document extension that is already
/// there is replaced (`a.png` exported as PDF gives `a.pdf`); any other dot is part of the name
/// (`Sketch v1.2` gives `Sketch v1.2.png`). Windows device names get a leading underscore.
pub(crate) fn with_export_extension(path: &Path, ext: &str) -> PathBuf {
    const KNOWN: [&str; 11] = ["procreate", "psd", "pdf", "png", "jpg", "jpeg", "tif", "tiff", "webp", "gif", "mp4"];
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("Untitled");
    let stem = match name.rsplit_once('.') {
        Some((stem, old)) if !stem.is_empty() && KNOWN.contains(&old.to_ascii_lowercase().as_str()) => stem,
        _ => name,
    };
    let stem = stem.trim_end_matches([' ', '.']);
    let stem = if stem.is_empty() { "Untitled" } else { stem };
    path.with_file_name(format!("{}.{ext}", crate::storage::avoid_device_name(stem)))
}

/// Write `doc` to `path` in `format` with `settings` and return what was written: the file (its
/// extension is set from the format), or the folder for [`ExportFormat::Layers`]. Creates the
/// parent folder when needed.
pub fn export_with(doc: &Document, format: ExportFormat, path: &Path, settings: &ExportSettings) -> Result<PathBuf, String> {
    settings.validate()?;
    let path = if format.is_folder() { path.to_path_buf() } else { with_export_extension(path, format.extension()) };
    let folder = if format.is_folder() { Some(path.as_path()) } else { path.parent() };
    if let Some(dir) = folder.filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| fail(dir, e))?;
    }
    let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_owned();
    let dpi = settings.dpi;
    match format {
        ExportFormat::Procreate => {
            let bytes = crate::storage::to_builder(doc, &title).to_bytes().map_err(|e| e.to_string())?;
            crate::storage::write_atomic(&path, &bytes)?;
        }
        ExportFormat::Psd => {
            // The layers are read from their tiles one at a time as they are written, never all
            // at once as canvas-sized buffers.
            let tree = psd_tree(doc, dpi, false);
            let pixels = DocPixels::new(doc);
            if doc.depth == Depth::U16 {
                // Depth 16, RLE channels: the layers' own 16-bit samples, no trip through 8 bits.
                let bytes = psd::to_bytes_streamed(&tree, &pixels, Some(psd::Compression16::Rle)).map_err(|e| fail(&path, e))?;
                crate::storage::write_atomic(&path, &bytes)?;
            } else {
                psd::write_streamed(&path, &tree, &pixels, None).map_err(|e| fail(&path, e))?;
            }
        }
        ExportFormat::Pdf => {
            let options = PdfOptions { dpi, quality: PdfQuality::High, colour_space: Some(doc.colour_space) };
            if settings.pdf_per_layer {
                export_layers_pdf(&path, &layer_rasters(doc), &options, false).map_err(|e| fail(&path, e))?;
            } else {
                // A PDF page holds 8 bits per channel: a 16-bit canvas is dithered down.
                let page = if doc.depth == Depth::U16 {
                    flattened16(doc, settings.transparent).to_raster_dithered()
                } else {
                    flattened(doc, settings.transparent)
                };
                export_pdf(&path, &[page], &options).map_err(|e| fail(&path, e))?;
            }
        }
        ExportFormat::Png | ExportFormat::Jpeg | ExportFormat::Tiff | ExportFormat::WebP => {
            let raster_format = match format {
                ExportFormat::Jpeg => RasterFormat::Jpeg { quality: settings.jpeg_quality },
                ExportFormat::Tiff => RasterFormat::Tiff { compression: TiffCompression::Lzw },
                ExportFormat::WebP => RasterFormat::WebP,
                _ => RasterFormat::Png,
            };
            // The raster already holds the paper colour unless the background is kept transparent,
            // so the writers keep its alpha channel (JPEG flattens on its own).
            let keep_alpha = settings.transparent && format.keeps_alpha();
            let options = ExportOptions {
                dpi,
                scale: settings.scale(),
                transparent: true,
                background: paper(doc),
                document_name: Some(title),
                colour_space: Some(doc.colour_space),
            };
            if doc.depth == Depth::U16 {
                // PNG and TIFF keep 16 bits; JPEG and WebP dither down to 8.
                export_raster16(&path, &flattened16(doc, keep_alpha), raster_format, &options).map_err(|e| fail(&path, e))?;
            } else {
                export_raster(&path, &flattened(doc, keep_alpha), raster_format, &options).map_err(|e| fail(&path, e))?;
            }
        }
        ExportFormat::Layers => {
            let options = ExportOptions {
                dpi,
                scale: settings.scale(),
                transparent: true,
                background: paper(doc),
                document_name: None,
                colour_space: Some(doc.colour_space),
            };
            if doc.depth == Depth::U16 {
                eskiz_format::export::export_layers_png16(&path, &layer_rasters16(doc), &options, false).map_err(|e| fail(&path, e))?;
            } else {
                export_layers_png(&path, &layer_rasters(doc), &options, false).map_err(|e| fail(&path, e))?;
            }
        }
    }
    Ok(path)
}

/// Write `doc` with the default settings (the sheet's starting state). See [`export_with`].
pub fn export_document(doc: &Document, format: ExportFormat, path: &Path) -> Result<PathBuf, String> {
    export_with(doc, format, path, &ExportSettings::default())
}

fn fail(path: &Path, e: impl Display) -> String {
    format!("{}: {e}", path.display())
}

/// The paper colour as bytes, for flattening. Used when the background is left in.
fn paper(doc: &Document) -> [u8; 3] {
    doc.background.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The whole canvas as premultiplied RGBA8 for the raster writers. With `keep_alpha` the paper
/// colour is left out, so transparent pixels stay transparent; otherwise it is composited in.
fn flattened(doc: &Document, keep_alpha: bool) -> Raster {
    let straight = composite(doc, CompositeOpts { skip_private: true, background: !keep_alpha });
    Raster {
        width: doc.width as u32,
        height: doc.height as u32,
        rgba: premultiply(&straight),
    }
}

/// [`flattened`] for a 16-bit document: the same composite at 16 bits per channel.
fn flattened16(doc: &Document, keep_alpha: bool) -> Raster16 {
    let straight = composite16(doc, CompositeOpts { skip_private: true, background: !keep_alpha });
    let mut rgba = Vec::with_capacity(straight.len());
    for p in straight.as_chunks::<4>().0 {
        let a = u64::from(p[3]);
        for &c in &p[..3] {
            rgba.push(((u64::from(c) * a + 32_767) / 65_535) as u16);
        }
        rgba.push(p[3]);
    }
    Raster16 {
        width: doc.width as u32,
        height: doc.height as u32,
        rgba,
    }
}

/// Straight RGBA8 to premultiplied RGBA8 (rounded).
pub fn premultiply(straight: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(straight.len());
    for p in straight.as_chunks::<4>().0 {
        let a = u32::from(p[3]);
        for &c in &p[..3] {
            out.push(((u32::from(c) * a + 127) / 255) as u8);
        }
        out.push(p[3]);
    }
    out
}

/// Every layer, bottom to top, with groups flattened into the list. Hidden layers are included
/// with `visible: false`, so the caller decides.
fn layer_rasters(doc: &Document) -> Vec<LayerRaster> {
    fn collect(doc: &Document, nodes: &[Node], out: &mut Vec<LayerRaster>) {
        for node in nodes {
            match &node.kind {
                NodeKind::Layer => {
                    if let Some(layer) = doc.layer(node.id) {
                        out.push(LayerRaster {
                            name: node.props.name.clone(),
                            visible: node.props.visible,
                            raster: Raster {
                                width: doc.width as u32,
                                height: doc.height as u32,
                                rgba: layer.to_rgba(),
                            },
                        });
                    }
                }
                NodeKind::Group { children, .. } => collect(doc, children, out),
            }
        }
    }
    let mut out = Vec::new();
    collect(doc, &doc.root, &mut out);
    out
}

/// [`layer_rasters`] at 16 bits per channel.
fn layer_rasters16(doc: &Document) -> Vec<eskiz_format::export::LayerRaster16> {
    fn collect(doc: &Document, nodes: &[Node], out: &mut Vec<eskiz_format::export::LayerRaster16>) {
        for node in nodes {
            match &node.kind {
                NodeKind::Layer => {
                    if let Some(layer) = doc.layer(node.id) {
                        out.push(eskiz_format::export::LayerRaster16 {
                            name: node.props.name.clone(),
                            visible: node.props.visible,
                            raster: Raster16 {
                                width: doc.width as u32,
                                height: doc.height as u32,
                                rgba: layer.to_rgba16(),
                            },
                        });
                    }
                }
                NodeKind::Group { children, .. } => collect(doc, children, out),
            }
        }
    }
    let mut out = Vec::new();
    collect(doc, &doc.root, &mut out);
    out
}

/// Every layer's 16-bit pixels in depth-first order, bottom to top, for [`psd::to_bytes16`].
pub fn psd_layers16(doc: &Document) -> Vec<Vec<u16>> {
    fn collect(doc: &Document, nodes: &[Node], out: &mut Vec<Vec<u16>>) {
        for node in nodes {
            match &node.kind {
                NodeKind::Layer => out.push(doc.layer(node.id).map_or_else(|| vec![0; doc.width * doc.height * 4], |l| l.to_rgba16())),
                NodeKind::Group { children, .. } => collect(doc, children, out),
            }
        }
    }
    let mut out = Vec::new();
    collect(doc, &doc.root, &mut out);
    out
}

/// The raster layers of a document as the streamed PSD writer's pixel supplier: rows of a layer
/// are read from its tiles when asked for. Layers are numbered in depth-first order, bottom to
/// top, as [`psd_layers16`] lists them.
pub struct DocPixels<'a> {
    width: usize,
    layers: Vec<Option<&'a eskiz_core::Layer>>,
}

impl<'a> DocPixels<'a> {
    pub fn new(doc: &'a Document) -> Self {
        fn collect<'a>(doc: &'a Document, nodes: &[Node], out: &mut Vec<Option<&'a eskiz_core::Layer>>) {
            for node in nodes {
                match &node.kind {
                    NodeKind::Layer => out.push(doc.layer(node.id)),
                    NodeKind::Group { children, .. } => collect(doc, children, out),
                }
            }
        }
        let mut layers = Vec::new();
        collect(doc, &doc.root, &mut layers);
        Self { width: doc.width, layers }
    }
}

impl psd::LayerPixels for DocPixels<'_> {
    fn rows8(&self, index: usize, y0: usize, y1: usize) -> Vec<u8> {
        match self.layers[index] {
            Some(l) => l.read_rect(0, y0, self.width, y1 - y0),
            None => vec![0; self.width * (y1 - y0) * 4],
        }
    }

    fn rows16(&self, index: usize, y0: usize, y1: usize) -> Vec<u16> {
        match self.layers[index] {
            Some(l) => l.read_rect16(0, y0, self.width, y1 - y0),
            None => vec![0; self.width * (y1 - y0) * 4],
        }
    }

    fn rows_blank(&self, index: usize, y0: usize, y1: usize) -> bool {
        match self.layers[index] {
            // no tile allocated under the rows
            Some(l) => (y0 / eskiz_core::TILE..=(y1.max(y0 + 1) - 1) / eskiz_core::TILE)
                .all(|ty| (0..l.tiles_x()).all(|tx| l.tile(tx, ty).is_none())),
            None => true,
        }
    }
}

/// The layer tree as a PSD document. Layer pixels are canvas-sized premultiplied RGBA8, as the
/// PSD writer expects. Masks are not written (the PSD writer does not store them).
pub fn psd_document(doc: &Document, dpi: f64) -> PsdDocument {
    psd_tree(doc, dpi, true)
}

/// [`psd_document`], with the layers' pixel buffers filled only when `dense` is set. Without
/// them the tree is for the streamed writer ([`DocPixels`]), which reads the pixels itself.
fn psd_tree(doc: &Document, dpi: f64, dense: bool) -> PsdDocument {
    fn nodes(doc: &Document, items: &[Node], dense: bool) -> Vec<PsdNode> {
        items
            .iter()
            .map(|node| {
                let p = &node.props;
                let opacity = p.opacity as f32;
                let blend = psd_blend(p.blend);
                let hidden = !p.visible;
                match &node.kind {
                    NodeKind::Layer => PsdNode::Layer(PsdLayer {
                        name: p.name.clone(),
                        opacity,
                        blend,
                        hidden,
                        clipped: p.clipped,
                        alpha_locked: p.alpha_lock,
                        locked: p.locked,
                        // A 16-bit document's pixels travel beside the tree (see `psd_layers16`).
                        pixels: if doc.depth == Depth::U16 || !dense {
                            Vec::new()
                        } else {
                            doc.layer(node.id)
                                .map_or_else(|| vec![0; doc.width * doc.height * 4], |l| l.to_rgba())
                        },
                    }),
                    NodeKind::Group { children, collapsed } => PsdNode::Group(PsdGroup {
                        name: p.name.clone(),
                        opacity,
                        blend,
                        hidden,
                        collapsed: *collapsed,
                        children: nodes(doc, children, dense),
                    }),
                }
            })
            .collect()
    }
    PsdDocument {
        width: doc.width as u32,
        height: doc.height as u32,
        dpi,
        colour_space: Some(doc.colour_space),
        root: nodes(doc, &doc.root, dense),
    }
}

/// The PSD blend mode for an Eskiz blend mode. The two lists are the same modes in the same order.
fn psd_blend(mode: BlendMode) -> PsdBlend {
    match mode {
        BlendMode::Normal => PsdBlend::Normal,
        BlendMode::Multiply => PsdBlend::Multiply,
        BlendMode::Screen => PsdBlend::Screen,
        BlendMode::Add => PsdBlend::Add,
        BlendMode::Lighten => PsdBlend::Lighten,
        BlendMode::Exclusion => PsdBlend::Exclusion,
        BlendMode::Difference => PsdBlend::Difference,
        BlendMode::Subtract => PsdBlend::Subtract,
        BlendMode::LinearBurn => PsdBlend::LinearBurn,
        BlendMode::ColorDodge => PsdBlend::ColorDodge,
        BlendMode::ColorBurn => PsdBlend::ColorBurn,
        BlendMode::Overlay => PsdBlend::Overlay,
        BlendMode::HardLight => PsdBlend::HardLight,
        BlendMode::Color => PsdBlend::Color,
        BlendMode::Luminosity => PsdBlend::Luminosity,
        BlendMode::Hue => PsdBlend::Hue,
        BlendMode::Saturation => PsdBlend::Saturation,
        BlendMode::SoftLight => PsdBlend::SoftLight,
        BlendMode::Darken => PsdBlend::Darken,
        BlendMode::HardMix => PsdBlend::HardMix,
        BlendMode::VividLight => PsdBlend::VividLight,
        BlendMode::LinearLight => PsdBlend::LinearLight,
        BlendMode::PinLight => PsdBlend::PinLight,
        BlendMode::LighterColor => PsdBlend::LighterColor,
        BlendMode::DarkerColor => PsdBlend::DarkerColor,
        BlendMode::Divide => PsdBlend::Divide,
    }
}

/// Flattened JPEG with the sheet's quality, straight RGB8 flattened on `background`. Kept for the
/// tests that write fixtures by hand.
pub fn flatten_on_background(rgba: &[u8], background: [f64; 3]) -> Vec<u8> {
    let bg = background.map(|v| v.clamp(0.0, 1.0) * 255.0);
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3);
    for p in rgba.as_chunks::<4>().0 {
        let a = f64::from(p[3]) / 255.0;
        for c in 0..3 {
            out.push((f64::from(p[c]) * a + bg[c] * (1.0 - a)).round() as u8);
        }
    }
    out
}

/// Write RGB8 pixels (`width * height * 3` bytes) as a baseline JPEG with the given quality
/// and the resolution in the JFIF density fields.
pub fn write_jpeg(path: &Path, width: usize, height: usize, rgb: &[u8], quality: u8, dpi: u16) -> Result<(), String> {
    if rgb.len() != width * height * 3 {
        return Err(format!("pixel buffer is {} bytes, expected {}", rgb.len(), width * height * 3));
    }
    let (w16, h16) = (
        u16::try_from(width).map_err(|_| "image is too wide for JPEG (65535 px maximum)".to_owned())?,
        u16::try_from(height).map_err(|_| "image is too tall for JPEG (65535 px maximum)".to_owned())?,
    );
    let mut bytes = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut bytes, quality);
    encoder.set_density(jpeg_encoder::Density::Inch { x: dpi, y: dpi });
    encoder
        .encode(rgb, w16, h16, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    crate::storage::write_atomic(path, &bytes)
}

/// Write RGBA8 pixels (`width * height * 4` bytes) as an 8-bit RGBA PNG.
pub fn write_png(path: &Path, width: usize, height: usize, rgba: &[u8]) -> Result<(), String> {
    write_png_dpi(path, width, height, rgba, None)
}

/// [`write_png`] with the resolution stored in a `pHYs` chunk (dots per inch).
pub fn write_png_dpi(path: &Path, width: usize, height: usize, rgba: &[u8], dpi: Option<u16>) -> Result<(), String> {
    if rgba.len() != width * height * 4 {
        return Err(format!(
            "pixel buffer is {} bytes, expected {}",
            rgba.len(),
            width * height * 4
        ));
    }
    // Encoded in memory, then written in one step: a failed export never leaves half a file.
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    if let Some(dpi) = dpi {
        let per_metre = (f64::from(dpi) / 0.0254).round() as u32;
        encoder.set_pixel_dims(Some(png::PixelDimensions {
            xppu: per_metre,
            yppu: per_metre,
            unit: png::Unit::Meter,
        }));
    }
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writer.finish().map_err(|e| format!("{}: {e}", path.display()))?;
    eskiz_format::fsutil::write_atomic(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The user's home directory from `HOME` or `USERPROFILE`.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// The folder exports start in: the user's Pictures, else Documents, else the home folder.
pub fn default_export_folder() -> PathBuf {
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    ["Pictures", "Documents"]
        .iter()
        .map(|d| home.join(d))
        .find(|d| d.is_dir())
        .unwrap_or(home)
}

/// Default export location: `eskiz_export.png` in [`default_export_folder`].
pub fn default_export_path() -> PathBuf {
    default_export_folder().join("eskiz_export.png")
}

/// Expand a leading `~/` in a path typed into a text field.
pub fn expand_home(text: &str) -> PathBuf {
    let trimmed = text.trim();
    // `~\` is how a path is typed on Windows.
    let rest = trimmed.strip_prefix("~/").or_else(|| if cfg!(windows) { trimmed.strip_prefix("~\\") } else { None });
    if let (Some(rest), Some(home)) = (rest, home_dir()) {
        return home.join(rest);
    }
    if trimmed == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }
    PathBuf::from(trimmed)
}

/// FNV-1a 64-bit hash, used as a stable checksum of pixel data.
pub fn checksum(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eskiz-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn painted() -> Document {
        let mut doc = Document::new(40, 30);
        let id = doc.layer_id(0).unwrap();
        let layer = doc.layer_mut(id).unwrap();
        for y in 0..30 {
            for x in 0..40 {
                layer.set_pixel(x, y, if x < 20 { [200, 0, 0, 255] } else { [0, 0, 0, 0] });
            }
        }
        doc
    }

    /// Two layers: a red left half, and a blue top strip on a layer above it that is hidden.
    fn two_layers() -> Document {
        let mut doc = painted();
        let above = doc.add_layer_above(doc.layer_id(0));
        doc.props_at_mut(1).unwrap().name = "Strip".to_owned();
        let layer = doc.layer_mut(above).unwrap();
        for x in 0..40 {
            layer.set_pixel(x, 0, [0, 0, 255, 255]);
        }
        doc
    }

    #[test]
    fn png_export_reads_back_with_pixels_and_dpi() {
        let dir = temp_dir("png");
        let doc = painted();
        let path = export_document(&doc, ExportFormat::Png, &dir.join("art.jpeg")).unwrap();
        assert_eq!(path, dir.join("art.png"), "the extension follows the format");
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let dims = reader.info().pixel_dims.expect("pHYs chunk");
        assert_eq!(dims.unit, png::Unit::Meter);
        assert!((i64::from(dims.xppu) - 5197).abs() <= 1, "132 dpi is about 5197 px/m");
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (40, 30));
        assert_eq!(&buf[..4], &[200, 0, 0, 255]);
        // The right half is empty layer over the white background.
        assert_eq!(&buf[30 * 4..30 * 4 + 4], &[255, 255, 255, 255]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn png_with_transparent_background_keeps_the_empty_half_clear() {
        let dir = temp_dir("png-transparent");
        let doc = painted();
        let settings = ExportSettings { transparent: true, ..ExportSettings::default() };
        let path = export_with(&doc, ExportFormat::Png, &dir.join("clear"), &settings).unwrap();
        let (w, h, rgba) = crate::import::decode_image(&path).unwrap();
        assert_eq!((w, h), (40, 30));
        assert_eq!(&rgba[..4], &[200, 0, 0, 255]);
        assert_eq!(rgba[(5 * 40 + 30) * 4 + 3], 0, "the empty half has no alpha");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scale_halves_the_png_and_is_validated() {
        let dir = temp_dir("scale");
        let doc = painted();
        let half = ExportSettings { scale_percent: 50.0, ..ExportSettings::default() };
        let path = export_with(&doc, ExportFormat::Png, &dir.join("half"), &half).unwrap();
        assert_eq!(crate::import::decode_image(&path).unwrap().0, 20);
        let bad = ExportSettings { scale_percent: 0.0, ..ExportSettings::default() };
        assert!(export_with(&doc, ExportFormat::Png, &dir.join("bad"), &bad).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn jpeg_export_decodes_close_to_the_art() {
        let dir = temp_dir("jpeg");
        let doc = painted();
        let path = export_document(&doc, ExportFormat::Jpeg, &dir.join("art")).unwrap();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("jpg"));
        let (w, h, rgba) = crate::import::decode_image(&path).unwrap();
        assert_eq!((w, h), (40, 30));
        let near = |p: &[u8], e: [u8; 3]| (0..3).all(|c| (i32::from(p[c]) - i32::from(e[c])).abs() < 12);
        assert!(near(&rgba[(15 * 40 + 5) * 4..], [200, 0, 0]), "left half red");
        assert!(near(&rgba[(15 * 40 + 35) * 4..], [255, 255, 255]), "right half on the white background");
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG magic");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tiff_and_webp_exports_read_back_at_canvas_size() {
        let dir = temp_dir("tiff-webp");
        let doc = painted();
        let tiff = export_document(&doc, ExportFormat::Tiff, &dir.join("art")).unwrap();
        assert_eq!(tiff.extension().and_then(|e| e.to_str()), Some("tif"));
        let magic = std::fs::read(&tiff).unwrap();
        assert!(magic.starts_with(b"II*\0") || magic.starts_with(b"MM\0*"), "TIFF magic");
        let webp = export_document(&doc, ExportFormat::WebP, &dir.join("art")).unwrap();
        let (w, h, rgba) = crate::import::decode_image(&webp).unwrap();
        assert_eq!((w, h), (40, 30));
        assert_eq!(&rgba[..4], &[200, 0, 0, 255], "lossless WebP keeps the red");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn procreate_export_opens_again_with_the_same_pixels() {
        let dir = temp_dir("package");
        let doc = painted();
        let path = export_document(&doc, ExportFormat::Procreate, &dir.join("art.png")).unwrap();
        assert_eq!(path, dir.join("art.procreate"));
        let back = crate::import::open_procreate(&path).unwrap();
        assert_eq!((back.width, back.height, back.layer_count()), (40, 30, 1));
        assert_eq!(back.layer_at(0).unwrap().to_rgba(), doc.layer_at(0).unwrap().to_rgba());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn psd_export_reads_back_with_the_layer_tree() {
        let dir = temp_dir("psd");
        let doc = two_layers();
        let path = export_document(&doc, ExportFormat::Psd, &dir.join("art")).unwrap();
        assert_eq!(path, dir.join("art.psd"));
        let back = psd::read(&path).unwrap();
        assert_eq!((back.width, back.height), (40, 30));
        // Bottom to top: the painted layer, then the strip.
        assert_eq!(back.root.len(), 2);
        let PsdNode::Layer(bottom) = &back.root[0] else { panic!("a layer") };
        assert_ne!(bottom.name, "Strip");
        assert_eq!(&bottom.pixels[..4], &[200, 0, 0, 255], "the painted pixels come back");
        let PsdNode::Layer(top) = &back.root[1] else { panic!("a layer") };
        assert_eq!(top.name, "Strip");
        assert_eq!(&top.pixels[..4], &[0, 0, 255, 255]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A document of random layers (patches over several tiles, empty layers, groups, hidden,
    /// clipped, blend modes) at the given depth.
    fn random_doc(seed: u64, width: usize, height: usize, depth: Depth) -> Document {
        let mut state = seed;
        let mut next = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let mut doc = Document::empty_depth(width, height, depth);
        let modes = [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay, BlendMode::Color, BlendMode::Divide];
        let mut group: Option<u64> = None;
        for i in 0..(2 + next() % 4) {
            let mut props = eskiz_core::document::Props::new(format!("layer {i}"));
            props.opacity = [1.0, 0.5, 0.25, 0.8][(next() % 4) as usize];
            props.blend = modes[(next() % 6) as usize];
            props.visible = next() % 6 != 0;
            props.clipped = next() % 4 == 0;
            props.alpha_lock = next() % 5 == 0;
            let mut layer = eskiz_core::Layer::new_depth(width, height, depth);
            for _ in 0..(next() % 4) {
                let (x, y) = ((next() as usize) % width, (next() as usize) % height);
                let (w, h) = (1 + (next() as usize) % (width - x).min(300), 1 + (next() as usize) % (height - y).min(300));
                let mut rect = Vec::with_capacity(w * h * 4);
                for _ in 0..w * h {
                    let a = (next() % 256) as u8;
                    rect.extend([(next() % (u64::from(a) + 1)) as u8, (next() % (u64::from(a) + 1)) as u8, (next() % (u64::from(a) + 1)) as u8, a]);
                }
                layer.write_rect(x, y, w, h, &rect);
            }
            if next() % 3 == 0 && group.is_none() {
                group = Some(doc.push_group(None, eskiz_core::document::Props::new("group"), None));
            }
            doc.push_raster(group, props, None, layer);
            if next() % 3 == 0 {
                group = None;
            }
        }
        doc
    }

    #[test]
    fn streamed_psd_bytes_equal_the_dense_writers() {
        for (case, (w, h)) in [(40usize, 30usize), (1, 1), (257, 300), (513, 70), (300, 600)].into_iter().enumerate() {
            for depth in [Depth::U8, Depth::U16] {
                let doc = random_doc(case as u64 * 7 + 1, w, h, depth);
                let pixels = DocPixels::new(&doc);
                let tree = psd_tree(&doc, 144.0, false);
                let (dense, streamed) = if depth == Depth::U16 {
                    (
                        psd::to_bytes16(&psd_document(&doc, 144.0), &psd_layers16(&doc), psd::Compression16::Rle).unwrap(),
                        psd::to_bytes_streamed(&tree, &pixels, Some(psd::Compression16::Rle)).unwrap(),
                    )
                } else {
                    (psd::to_bytes(&psd_document(&doc, 144.0)).unwrap(), psd::to_bytes_streamed(&tree, &pixels, None).unwrap())
                };
                assert!(dense == streamed, "{w}x{h} at {} bits", depth.bits());
            }
        }
    }

    #[test]
    fn pdf_exports_flattened_or_per_layer() {
        let dir = temp_dir("pdf");
        let doc = two_layers();
        let flat = export_document(&doc, ExportFormat::Pdf, &dir.join("flat")).unwrap();
        assert!(std::fs::read(&flat).unwrap().starts_with(b"%PDF"));
        let per = ExportSettings { pdf_per_layer: true, ..ExportSettings::default() };
        let layered = export_with(&doc, ExportFormat::Pdf, &dir.join("layers"), &per).unwrap();
        assert!(std::fs::read(&layered).unwrap().starts_with(b"%PDF"));
        assert!(std::fs::metadata(&layered).unwrap().len() > std::fs::metadata(&flat).unwrap().len() / 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn layer_pngs_go_into_a_folder_one_per_visible_layer() {
        let dir = temp_dir("layers");
        let doc = two_layers();
        let folder = export_document(&doc, ExportFormat::Layers, &dir.join("parts")).unwrap();
        assert_eq!(folder, dir.join("parts"));
        let files: Vec<_> = std::fs::read_dir(&folder).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 2, "both layers are visible");
        let mut names: Vec<String> = files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        assert!(names.iter().all(|n| n.ends_with(".png")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn psd_blend_modes_follow_the_document() {
        assert_eq!(psd_blend(BlendMode::Multiply), PsdBlend::Multiply);
        assert_eq!(psd_blend(BlendMode::Divide), PsdBlend::Divide);
    }

    #[test]
    fn jpeg_drops_alpha_onto_the_background_colour() {
        let rgb = flatten_on_background(&[255, 0, 0, 0, 0, 0, 255, 255], [0.0, 1.0, 0.0]);
        assert_eq!(rgb, vec![0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn checksum_matches_known_fnv1a_values() {
        // Published FNV-1a 64-bit test vectors.
        assert_eq!(checksum(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(checksum(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(checksum(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn expand_home_keeps_plain_paths() {
        assert_eq!(expand_home("  /tmp/x.png "), PathBuf::from("/tmp/x.png"));
    }

    #[test]
    fn write_png_rejects_wrong_length() {
        let err = write_png(Path::new("/nonexistent/none.png"), 2, 2, &[0; 3]).unwrap_err();
        assert!(err.contains("expected 16"));
    }

    #[test]
    fn write_png_round_trips_through_the_png_crate() {
        let dir = std::env::temp_dir().join(format!("eskiz-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let rgba: Vec<u8> = (0..(3 * 2 * 4)).map(|v| (v * 10) as u8).collect();
        write_png(&path, 3, 2, &rgba).unwrap();
        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&buf[..rgba.len()], &rgba[..]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
