//! Colour types and conversions: gamma-encoded RGB floats, 8-bit RGB, hex strings,
//! HSB (HSV), and the colour history.
//!
//! HSB and HSV are the same model. The UI calls it HSB. Hue is in degrees, and
//! saturation and brightness are in `0..=1`. Hue 0 is red, and hue increases through
//! green and blue.
//!
//! [`Rgb`], [`Rgb8`] and hex parsing are defined in `eskiz-types` and re-exported here.

pub use eskiz_types::colour::{ParseHexError, Rgb, Rgb8};

/// Number of recent colours kept by [`ColourHistory`] by default.
pub const HISTORY_CAPACITY: usize = 10;

/// Hue-saturation-brightness (HSV). `h` is in degrees `[0, 360)`. `s` and `b` are in
/// `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsb {
    pub h: f64,
    pub s: f64,
    pub b: f64,
}

/// Alias for [`Hsb`]. The two names are the same model.
pub type Hsv = Hsb;

impl Hsb {
    /// Builds a colour. Hue wraps into `[0, 360)`. Saturation and brightness are clamped.
    pub fn new(h: f64, s: f64, b: f64) -> Self {
        Self {
            h: h.rem_euclid(360.0),
            s: s.clamp(0.0, 1.0),
            b: b.clamp(0.0, 1.0),
        }
    }
}

/// HSB to gamma-encoded RGB.
pub fn hsb_to_rgb(c: Hsb) -> Rgb {
    let h = c.h.rem_euclid(360.0) / 60.0;
    let chroma = c.b * c.s;
    let x = chroma * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = c.b - chroma;
    Rgb::new(r1 + m, g1 + m, b1 + m)
}

/// Gamma-encoded RGB to HSB. Achromatic colours get hue 0.
pub fn rgb_to_hsb(c: Rgb) -> Hsb {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let delta = max - min;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta == 0.0 {
        0.0
    } else if max == c.r {
        60.0 * ((c.g - c.b) / delta).rem_euclid(6.0)
    } else if max == c.g {
        60.0 * ((c.b - c.r) / delta + 2.0)
    } else {
        60.0 * ((c.r - c.g) / delta + 4.0)
    };
    Hsb::new(h, s, max)
}

/// The most recent colours, newest first, with no duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColourHistory {
    items: Vec<Rgb8>,
    capacity: usize,
}

impl ColourHistory {
    /// Empty history with [`HISTORY_CAPACITY`] entries.
    pub fn new() -> Self {
        Self::with_capacity(HISTORY_CAPACITY)
    }

    /// Empty history with the given capacity (at least 1).
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            items: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// Records a colour as the newest entry. An earlier copy of it is removed first, so
    /// the list never repeats a colour. Entries past the capacity are dropped.
    pub fn push(&mut self, colour: Rgb8) {
        self.items.retain(|c| *c != colour);
        self.items.insert(0, colour);
        self.items.truncate(self.capacity);
    }

    /// The colours, newest first.
    pub fn items(&self) -> &[Rgb8] {
        &self.items
    }
}

impl Default for ColourHistory {
    fn default() -> Self {
        Self::new()
    }
}
