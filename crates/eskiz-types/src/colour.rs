//! Colour values: gamma-encoded RGB floats, 8-bit RGB and hex strings.
//!
//! `eskiz-guides` re-exports these from `eskiz_guides::colour`, next to the HSB maths and the
//! colour history.

use std::fmt;
use std::str::FromStr;

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
