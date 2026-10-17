//! Colour types and conversions: gamma-encoded RGB floats, 8-bit RGB, hex strings,
//! HSB (HSV), and the colour history.
//!
//! HSB and HSV are the same model. The UI calls it HSB. Hue is in degrees, and
//! saturation and brightness are in `0..=1`. Hue 0 is red, and hue increases through
//! green and blue.

use std::fmt;
use std::str::FromStr;

/// Number of recent colours kept by [`ColourHistory`] by default.
pub const HISTORY_CAPACITY: usize = 10;

/// Gamma-encoded RGB with channels in `0..=1`. The colour space comes from context:
/// sRGB unless stated otherwise (see [`crate::colour_space`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub const fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }

    /// Quantises to 8 bits per channel. Channels are clamped to `0..=1` first.
    pub fn to_rgb8(self) -> Rgb8 {
        fn quantise(c: f64) -> u8 {
            (c.clamp(0.0, 1.0) * 255.0).round() as u8
        }
        Rgb8::new(quantise(self.r), quantise(self.g), quantise(self.b))
    }
}

/// 8-bit RGB, the form used by hex strings and the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb8 {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Converts to floats in `0..=1`.
    pub fn to_rgb(self) -> Rgb {
        Rgb::new(
            f64::from(self.r) / 255.0,
            f64::from(self.g) / 255.0,
            f64::from(self.b) / 255.0,
        )
    }

    /// Formats as `#RRGGBB` in upper case.
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// Error from parsing a hex colour string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseHexError {
    input: String,
}

impl fmt::Display for ParseHexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid hex color `{}`: expected #RGB or #RRGGBB",
            self.input
        )
    }
}

impl std::error::Error for ParseHexError {}

impl FromStr for Rgb8 {
    type Err = ParseHexError;

    /// Accepts `#RRGGBB`, `RRGGBB`, `#RGB` and `RGB`, in either case.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseHexError {
            input: s.to_owned(),
        };
        let body = s.strip_prefix('#').unwrap_or(s);
        if !body.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(err());
        }
        let nibble = |i: usize| u8::from_str_radix(&body[i..i + 1], 16).map_err(|_| err());
        let pair = |i: usize| u8::from_str_radix(&body[i..i + 2], 16).map_err(|_| err());
        match body.len() {
            3 => Ok(Rgb8::new(nibble(0)? * 17, nibble(1)? * 17, nibble(2)? * 17)),
            6 => Ok(Rgb8::new(pair(0)?, pair(2)?, pair(4)?)),
            _ => Err(err()),
        }
    }
}

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
