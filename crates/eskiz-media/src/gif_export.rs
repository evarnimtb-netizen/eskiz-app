//! Animated GIF export with NeuQuant palettes, optional Floyd-Steinberg dithering and 1-bit transparency.

use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use color_quant::NeuQuant;

use crate::error::{MediaError, Result};
use crate::frame::{common_size, Frame};
use crate::fsutil::write_atomic;

const PALETTE_ENTRIES: usize = 256;
const TRANSPARENT_INDEX: u8 = 255;

/// Whether the animation shares one palette or each frame has its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaletteMode {
    /// One palette learned from every frame. The default.
    #[default]
    Global,
    /// A palette per frame.
    PerFrame,
}

/// GIF export options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GifOptions {
    /// Number of times the animation repeats. 0 loops forever (the default).
    pub loop_count: u16,
    /// Global or per-frame palette.
    pub palette: PaletteMode,
    /// Floyd-Steinberg error diffusion when mapping colours to the palette.
    pub dither: bool,
    /// When on, pixels with alpha below `alpha_threshold` become transparent. The rest are composited over
    /// `background`. When off, every pixel is composited over `background`.
    pub transparent_background: bool,
    /// Alpha below this value is transparent when `transparent_background` is on (0 to 255, default 128).
    pub alpha_threshold: u8,
    /// Colour that partial and opaque-composited pixels are blended onto.
    pub background: [u8; 3],
    /// NeuQuant sampling factor, 1 (best, slowest) to 30 (fastest). Default 10.
    pub sample_factor: i32,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            loop_count: 0,
            palette: PaletteMode::Global,
            dither: false,
            transparent_background: false,
            alpha_threshold: 128,
            background: [255, 255, 255],
            sample_factor: 10,
        }
    }
}

/// GIF frame delay in centiseconds: `duration` rounded to the nearest centisecond, minimum 2.
pub fn gif_delay_centiseconds(duration: Duration) -> u16 {
    (duration.as_secs_f64() * 100.0)
        .round()
        .clamp(2.0, f64::from(u16::MAX)) as u16
}

/// A frame after compositing: opaque RGBA samples plus a transparency mask.
struct Prepared {
    rgba: Vec<u8>,
    transparent: Vec<bool>,
}

fn prepare(frame: &Frame, options: &GifOptions) -> Prepared {
    let pixels = frame.pixels();
    let count = pixels.len() / 4;
    let mut rgba = Vec::with_capacity(count * 4);
    let mut transparent = vec![false; count];
    for (i, px) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if options.transparent_background && px[3] < options.alpha_threshold {
            transparent[i] = true;
            rgba.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let alpha = u32::from(px[3]);
        for (&c, &bg) in px[..3].iter().zip(&options.background) {
            rgba.push(crate::frame::over_background(c, alpha, bg));
        }
        rgba.push(255);
    }
    Prepared { rgba, transparent }
}

/// Opaque samples for training a palette. Never empty.
fn opaque_samples<'a>(
    prepared: impl IntoIterator<Item = &'a Prepared>,
    background: [u8; 3],
) -> Vec<u8> {
    let mut out = Vec::new();
    for p in prepared {
        for (px, &skip) in p.rgba.as_chunks::<4>().0.iter().zip(&p.transparent) {
            if !skip {
                out.extend_from_slice(px);
            }
        }
    }
    if out.is_empty() {
        out.extend_from_slice(&[background[0], background[1], background[2], 255]);
    }
    out
}

/// A palette for one scope: the whole animation, or one frame.
enum Quantiser {
    /// At most `colours` distinct colours: the palette holds exactly those colours.
    Exact {
        palette: Vec<[u8; 3]>,
        index: HashMap<[u8; 3], u8>,
    },
    /// More distinct colours than fit: NeuQuant learns a palette.
    Learned(NeuQuant),
}

impl Quantiser {
    /// Builds a palette from RGBA `samples` (opaque pixels only).
    fn new(samples: &[u8], colours: usize, sample_factor: i32) -> Self {
        let mut palette: Vec<[u8; 3]> = Vec::new();
        let mut index: HashMap<[u8; 3], u8> = HashMap::new();
        for px in samples.as_chunks::<4>().0 {
            let key = [px[0], px[1], px[2]];
            if let Entry::Vacant(slot) = index.entry(key) {
                if palette.len() == colours {
                    return Self::Learned(NeuQuant::new(sample_factor, colours, samples));
                }
                slot.insert(palette.len() as u8);
                palette.push(key);
            }
        }
        Self::Exact { palette, index }
    }

    /// Nearest palette index for an opaque RGBA colour.
    fn index_of(&self, rgba: &[u8]) -> u8 {
        match self {
            Self::Exact { palette, index } => {
                let key = [rgba[0], rgba[1], rgba[2]];
                if let Some(&hit) = index.get(&key) {
                    return hit;
                }
                let dist = |c: &[u8; 3]| -> i32 {
                    c.iter()
                        .zip(&key)
                        .map(|(&a, &b)| (i32::from(a) - i32::from(b)).pow(2))
                        .sum()
                };
                palette
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, c)| dist(c))
                    .map_or(0, |(i, _)| i as u8)
            }
            Self::Learned(quant) => quant.index_of(rgba) as u8,
        }
    }

    /// RGB palette with exactly 256 entries. Unused entries are black.
    fn palette_rgb(&self) -> Vec<u8> {
        let mut rgb: Vec<u8> = match self {
            Self::Exact { palette, .. } => palette.iter().flatten().copied().collect(),
            Self::Learned(quant) => quant.color_map_rgb(),
        };
        rgb.resize(PALETTE_ENTRIES * 3, 0);
        rgb
    }
}

fn index_frame(
    prep: &Prepared,
    quant: &Quantiser,
    palette: &[u8],
    size: (usize, usize),
    dither: bool,
) -> Vec<u8> {
    let (width, height) = size;
    if !dither {
        return prep
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(&prep.transparent)
            .map(|(px, &skip)| {
                if skip {
                    TRANSPARENT_INDEX
                } else {
                    quant.index_of(px)
                }
            })
            .collect();
    }
    let mut error = vec![[0f32; 3]; width * height];
    let mut out = vec![TRANSPARENT_INDEX; width * height];
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if prep.transparent[i] {
                continue;
            }
            let px = &prep.rgba[i * 4..i * 4 + 4];
            let mut want = [0f32; 3];
            for (w, (&p, &e)) in want.iter_mut().zip(px.iter().zip(&error[i])) {
                *w = (f32::from(p) + e).clamp(0.0, 255.0);
            }
            let index = quant.index_of(&[
                want[0].round() as u8,
                want[1].round() as u8,
                want[2].round() as u8,
                255,
            ]);
            out[i] = index;
            let chosen = &palette[index as usize * 3..index as usize * 3 + 3];
            let mut delta = [0f32; 3];
            for (d, (&w, &c)) in delta.iter_mut().zip(want.iter().zip(chosen)) {
                *d = w - f32::from(c);
            }
            let spread = [
                (1isize, 0usize, 7.0f32 / 16.0),
                (-1, 1, 3.0 / 16.0),
                (0, 1, 5.0 / 16.0),
                (1, 1, 1.0 / 16.0),
            ];
            for (dx, dy, weight) in spread {
                let nx = x as isize + dx;
                let ny = y + dy;
                if nx >= 0 && (nx as usize) < width && ny < height {
                    let j = ny * width + nx as usize;
                    for (e, &d) in error[j].iter_mut().zip(&delta) {
                        *e += d * weight;
                    }
                }
            }
        }
    }
    out
}

/// Encodes frames as an animated GIF.
///
/// Every frame must share one size, and the GIF side limit is 65535 px. Frame delays are the frame durations
/// rounded to centiseconds, with a minimum of 2.
pub fn encode_gif<W: Write>(writer: W, frames: &[Frame], options: &GifOptions) -> Result<()> {
    let (width, height) = common_size(frames)?;
    if width > u32::from(u16::MAX) || height > u32::from(u16::MAX) {
        return Err(MediaError::InvalidSettings(
            "GIF sides are limited to 65535 px".into(),
        ));
    }
    if !(1..=30).contains(&options.sample_factor) {
        return Err(MediaError::InvalidSettings(format!(
            "NeuQuant sample factor {} is outside 1..=30",
            options.sample_factor
        )));
    }
    let colours = if options.transparent_background {
        PALETTE_ENTRIES - 1
    } else {
        PALETTE_ENTRIES
    };
    let size = (width as usize, height as usize);
    let prepared: Vec<Prepared> = frames.iter().map(|f| prepare(f, options)).collect();

    let global = match options.palette {
        PaletteMode::Global => {
            let samples = opaque_samples(prepared.iter(), options.background);
            Some(Quantiser::new(&samples, colours, options.sample_factor))
        }
        PaletteMode::PerFrame => None,
    };
    let global_rgb = global
        .as_ref()
        .map(Quantiser::palette_rgb)
        .unwrap_or_default();

    let mut encoder = gif::Encoder::new(writer, width as u16, height as u16, &global_rgb)?;
    encoder.set_repeat(if options.loop_count == 0 {
        gif::Repeat::Infinite
    } else {
        gif::Repeat::Finite(options.loop_count)
    })?;

    for (frame, prep) in frames.iter().zip(&prepared) {
        let (indices, local_palette) = match &global {
            Some(quant) => (
                index_frame(prep, quant, &global_rgb, size, options.dither),
                None,
            ),
            None => {
                let samples = opaque_samples(std::iter::once(prep), options.background);
                let quant = Quantiser::new(&samples, colours, options.sample_factor);
                let rgb = quant.palette_rgb();
                (
                    index_frame(prep, &quant, &rgb, size, options.dither),
                    Some(rgb),
                )
            }
        };
        let gif_frame = gif::Frame {
            width: width as u16,
            height: height as u16,
            delay: gif_delay_centiseconds(frame.duration()),
            transparent: options.transparent_background.then_some(TRANSPARENT_INDEX),
            palette: local_palette,
            buffer: Cow::Owned(indices),
            ..gif::Frame::default()
        };
        encoder.write_frame(&gif_frame)?;
    }
    Ok(())
}

/// Encodes frames to a GIF file. The file is written atomically.
pub fn write_gif_file(path: &Path, frames: &[Frame], options: &GifOptions) -> Result<()> {
    write_atomic(path, |file: &mut File| encode_gif(file, frames, options))
}
