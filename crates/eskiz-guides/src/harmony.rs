//! Colour harmonies: the companion colours for a base colour, given as hue offsets.

use crate::colour::Hsb;

/// Harmony modes, with the hue offsets they use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harmony {
    /// The opposite hue, +180 degrees.
    Complementary,
    /// The two hues beside the complement, +150 and +210 degrees.
    SplitComplementary,
    /// The two neighbours, -30 and +30 degrees.
    Analogous,
    /// Two hues 120 degrees either side, +120 and +240 (= -120) degrees.
    Triadic,
    /// A rectangle: +60, +180 and +240 degrees.
    Tetradic,
}

impl Harmony {
    /// Hue offsets of the companion colours, in degrees, relative to the base.
    pub fn offsets(self) -> &'static [f64] {
        match self {
            Harmony::Complementary => &[180.0],
            Harmony::SplitComplementary => &[150.0, 210.0],
            Harmony::Analogous => &[-30.0, 30.0],
            Harmony::Triadic => &[120.0, 240.0],
            Harmony::Tetradic => &[60.0, 180.0, 240.0],
        }
    }
}

/// Companion colours for `base`. Each keeps the base's saturation and brightness. The
/// base itself is not included.
pub fn harmony(base: Hsb, mode: Harmony) -> Vec<Hsb> {
    mode.offsets()
        .iter()
        .map(|offset| Hsb::new(base.h + offset, base.s, base.b))
        .collect()
}
