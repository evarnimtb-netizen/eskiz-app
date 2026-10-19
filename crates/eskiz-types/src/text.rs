//! Text layer data: attributed runs, colour, the placement transform and the
//! wrap width.
//!
//! This is the data half of `eskiz-text`'s model, kept here so `eskiz-core` can store text
//! layers (`Document::texts`) without depending on fonts. `eskiz_text::model` re-exports it, and
//! the methods that need fonts (`layout`, `rasterise`, `rasterise_at_scale`) are on the trait
//! `eskiz_text::TextLayerExt`.

use std::ops::Range;

/// Default em size in points.
pub const DEFAULT_SIZE_PT: f32 = 12.0;
/// Allowed size range in points.
pub const SIZE_RANGE: (f32, f32) = (1.0, 1000.0);
/// Allowed kerning and tracking range, in em fractions.
pub const SPACING_RANGE: (f32, f32) = (-0.5, 2.0);
/// Allowed leading range, a multiplier of the font line height.
pub const LEADING_RANGE: (f32, f32) = (0.5, 4.0);

/// Straight-alpha colour with 8 bits per channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const BLACK: Color = Color::rgba(0, 0, 0, 255);
    pub const WHITE: Color = Color::rgba(255, 255, 255, 255);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Formats the colour as `#rrggbbaa`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }

    /// Parses `#rrggbbaa`. Any other shape returns `None`.
    pub fn from_hex(text: &str) -> Option<Self> {
        let hex = text.strip_prefix('#')?;
        if hex.len() != 8 || !hex.is_ascii() {
            return None;
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        Some(Self::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?))
    }
}

/// Two-dimensional affine transform: `x' = a*x + c*y + tx`, `y' = b*x + d*y + ty`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    pub const fn new(a: f32, b: f32, c: f32, d: f32, tx: f32, ty: f32) -> Self {
        Self { a, b, c, d, tx, ty }
    }

    /// Pure translation.
    pub const fn translation(tx: f32, ty: f32) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }

    /// Applies the transform to a point.
    pub fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    /// The transform followed by a uniform scale of `s`.
    pub fn scaled(self, s: f32) -> Self {
        Self::new(
            self.a * s,
            self.b * s,
            self.c * s,
            self.d * s,
            self.tx * s,
            self.ty * s,
        )
    }

    /// Uniform scale factor of the linear part, `sqrt(|det|)`.
    pub fn linear_scale(self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Paragraph alignment. Justified stretches every line except the last of a paragraph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Alignment {
    #[default]
    Left,
    Centre,
    Right,
    Justified,
}

/// Writing direction. Only left-to-right is implemented (right-to-left is an optional `[choice]`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WritingDirection {
    #[default]
    LeftToRight,
}

/// Font style within a family.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FontFace {
    #[default]
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl FontFace {
    pub fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }

    pub fn is_italic(self) -> bool {
        matches!(self, Self::Italic | Self::BoldItalic)
    }

    /// Style name used in storage: `Regular`, `Bold`, `Italic` or `BoldItalic`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Regular => "Regular",
            Self::Bold => "Bold",
            Self::Italic => "Italic",
            Self::BoldItalic => "BoldItalic",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Regular, Self::Bold, Self::Italic, Self::BoldItalic]
            .into_iter()
            .find(|face| face.name() == name)
    }
}

/// The attributes of one run of text.
#[derive(Clone, Debug, PartialEq)]
pub struct Attrs {
    /// Family name. The empty string means the system sans-serif default.
    pub font_family: String,
    pub font_face: FontFace,
    /// Em size in points.
    pub size_pt: f32,
    /// Extra advance after each character, as an em fraction.
    pub kerning: f32,
    /// Extra advance after each character, as an em fraction. Added to kerning.
    pub tracking: f32,
    /// Baseline-to-baseline distance multiplier, applied to the font line height.
    pub leading: f32,
    /// Raises (positive) or lowers the run, in points.
    pub baseline_shift_pt: f32,
    pub alignment: Alignment,
    /// Outline stroke width in points. Zero means off.
    pub outline_pt: f32,
    pub underline: bool,
    pub color: Color,
    /// Displays and rasterises the run in capitals. The stored text is unchanged.
    pub all_caps: bool,
    /// Characters stack top to bottom in columns, and columns run right to left.
    pub vertical: bool,
    pub writing_direction: WritingDirection,
}

impl Default for Attrs {
    fn default() -> Self {
        Self {
            font_family: String::new(),
            font_face: FontFace::Regular,
            size_pt: DEFAULT_SIZE_PT,
            kerning: 0.0,
            tracking: 0.0,
            leading: 1.0,
            baseline_shift_pt: 0.0,
            alignment: Alignment::Left,
            outline_pt: 0.0,
            underline: false,
            color: Color::BLACK,
            all_caps: false,
            vertical: false,
            writing_direction: WritingDirection::LeftToRight,
        }
    }
}

impl Attrs {
    /// Returns the attributes with every numeric value clamped to its range.
    pub fn clamped(mut self) -> Self {
        self.size_pt = clamp_or(self.size_pt, SIZE_RANGE, DEFAULT_SIZE_PT);
        self.kerning = clamp_or(self.kerning, SPACING_RANGE, 0.0);
        self.tracking = clamp_or(self.tracking, SPACING_RANGE, 0.0);
        self.leading = clamp_or(self.leading, LEADING_RANGE, 1.0);
        if !self.baseline_shift_pt.is_finite() {
            self.baseline_shift_pt = 0.0;
        }
        self.outline_pt = if self.outline_pt.is_finite() {
            self.outline_pt.max(0.0)
        } else {
            0.0
        };
        self
    }

    /// Extra advance after each character at the run's own size: size x (kerning + tracking).
    pub fn letter_spacing_pt(&self) -> f32 {
        self.size_pt * (self.kerning + self.tracking)
    }
}

fn clamp_or(value: f32, (lo, hi): (f32, f32), fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        fallback
    }
}

/// A run of text that shares one set of attributes. `len` is in bytes of [`TextLayer::text`].
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub len: usize,
    pub attrs: Attrs,
}

/// A text layer: the rich string, its wrap width and its placement transform.
///
/// The invariant is that the run lengths add up to `text.len()`, and every run boundary falls on
/// a character boundary. [`TextLayer::set_attrs`] keeps it. Paragraphs are separated by `'\n'`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLayer {
    pub text: String,
    pub runs: Vec<Run>,
    /// Wrap width in layer units (1 unit = 1 point at scale 1). Zero means no wrapping.
    pub box_width: f32,
    /// Placement of the layer in document pixels.
    pub transform: Affine,
}

impl TextLayer {
    /// Creates a layer whose whole text carries one set of attributes.
    pub fn new(text: impl Into<String>, attrs: Attrs, box_width: f32) -> Self {
        let text = text.into();
        let len = text.len();
        Self {
            text,
            runs: vec![Run {
                len,
                attrs: attrs.clamped(),
            }],
            box_width: if box_width.is_finite() {
                box_width.max(0.0)
            } else {
                0.0
            },
            transform: Affine::IDENTITY,
        }
    }

    /// The attributes of the run that contains `byte`. Past the end, the last run is used.
    pub fn attrs_at(&self, byte: usize) -> Option<&Attrs> {
        let mut pos = 0usize;
        for run in &self.runs {
            if byte < pos + run.len {
                return Some(&run.attrs);
            }
            pos += run.len;
        }
        self.runs.last().map(|run| &run.attrs)
    }

    /// Applies `edit` to the attributes of the characters in `range` only.
    ///
    /// Runs are split at the range edges and merged again when neighbours end up equal. A range
    /// that is out of bounds or not on character boundaries is ignored.
    pub fn set_attrs(&mut self, range: Range<usize>, mut edit: impl FnMut(&mut Attrs)) {
        let valid = range.start <= range.end
            && range.end <= self.text.len()
            && self.text.is_char_boundary(range.start)
            && self.text.is_char_boundary(range.end);
        if !valid || self.runs.is_empty() {
            return;
        }
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len() + 2);
        let mut pos = 0usize;
        for run in &self.runs {
            let (start, end) = (pos, pos + run.len);
            pos = end;
            let a = range.start.clamp(start, end);
            let b = range.end.clamp(start, end);
            push_run(&mut out, a - start, run.attrs.clone());
            let mut edited = run.attrs.clone();
            if b > a {
                edit(&mut edited);
                edited = edited.clamped();
            }
            push_run(&mut out, b - a, edited);
            push_run(&mut out, end - b, run.attrs.clone());
        }
        if out.is_empty() {
            out.push(Run {
                len: 0,
                attrs: self.runs[0].attrs.clone(),
            });
        }
        let mut merged: Vec<Run> = Vec::with_capacity(out.len());
        for run in out {
            match merged.last_mut() {
                Some(last) if last.attrs == run.attrs => last.len += run.len,
                _ => merged.push(run),
            }
        }
        self.runs = merged;
    }
}

fn push_run(out: &mut Vec<Run>, len: usize, attrs: Attrs) {
    if len > 0 {
        out.push(Run { len, attrs });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_hex_round_trips() {
        let c = Color::rgba(1, 2, 254, 255);
        assert_eq!(Color::from_hex(&c.to_hex()), Some(c));
        assert_eq!(Color::from_hex("#12345"), None);
        assert_eq!(Color::from_hex("#ééééé"), None);
    }

    #[test]
    fn set_attrs_splits_and_merges_runs() {
        let mut layer = TextLayer::new("abcdef", Attrs::default(), 0.0);
        layer.set_attrs(1..3, |a| a.size_pt = 20.0);
        assert_eq!(layer.runs.len(), 3);
        assert_eq!(layer.attrs_at(0).unwrap().size_pt, DEFAULT_SIZE_PT);
        assert_eq!(layer.attrs_at(2).unwrap().size_pt, 20.0);
        assert_eq!(layer.attrs_at(4).unwrap().size_pt, DEFAULT_SIZE_PT);
        layer.set_attrs(1..3, |a| a.size_pt = DEFAULT_SIZE_PT);
        assert_eq!(layer.runs.len(), 1);
        assert_eq!(layer.runs[0].len, 6);
    }

    #[test]
    fn set_attrs_ignores_bad_ranges_and_clamps() {
        let mut layer = TextLayer::new("é", Attrs::default(), 0.0);
        layer.set_attrs(1..2, |a| a.size_pt = 5.0);
        assert_eq!(layer.runs.len(), 1);
        layer.set_attrs(0..2, |a| a.leading = 99.0);
        assert_eq!(layer.runs[0].attrs.leading, LEADING_RANGE.1);
    }
}
