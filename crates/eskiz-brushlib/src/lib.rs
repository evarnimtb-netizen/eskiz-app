//! eskiz-brushlib: Eskiz's built-in brush library.
//!
//! The library is forty original presets in eight sets. Each preset pairs
//! eskiz-core [`Brush`] settings with a procedurally generated tip and, where
//! useful, a grain texture. Everything is generated in code and seeded, so the
//! same build always produces the same textures.
//!
//! - [`library_ref`] / [`library`]: the presets, with textures built once on first
//!   use and cached for the life of the process.
//! - [`preview_stroke`]: an opaque RGBA preview of one preset drawn along a sample
//!   S-curve, for the Brush Library panel.
//! - [`stroke_mask`]: the same stroke as an 8-bit alpha mask.
//! - [`tips`] and [`grains`]: the texture generators on their own.
//!
//! Behaviour comes from eskiz-core and public documentation. No brush,
//! texture or name here is taken from any other product.

pub mod grains;
mod noise;
mod presets;
mod preview;
pub mod tips;

use eskiz_core::brushes::Brush;
use grains::{grain, GrainKind, GRAIN_SIZE};
use image::GrayImage;
use presets::Recipe;
use std::sync::OnceLock;
use tips::{tip, TipKind, TIP_SIZE};

pub use preview::{preview_stroke, stroke_mask, INK, PAPER};

/// One preset ready to paint with.
#[derive(Clone, Debug)]
pub struct LibraryBrush {
    /// Stable identifier, unique across the library, such as `"inking.studio-ink"`.
    pub id: &'static str,
    /// Display name, unique across the library.
    pub name: &'static str,
    /// Engine settings (dynamics, spacing, taper, accumulation and so on).
    pub brush: Brush,
    /// Grayscale tip mask, white is paint.
    pub tip: GrayImage,
    /// Tileable grayscale grain, if the preset uses one.
    pub grain: Option<GrayImage>,
    /// The generator that made `tip`, when it came from one (a built-in preset, or a tip chosen
    /// in the Brush Bench). `None` for a tip read from a file.
    pub tip_kind: Option<TipKind>,
    /// The generator that made `grain`, when it came from one.
    pub grain_kind: Option<GrainKind>,
}

/// A named group of presets, shown as one section of the panel.
#[derive(Clone, Debug)]
pub struct BrushSet {
    pub name: &'static str,
    pub brushes: Vec<LibraryBrush>,
}

static LIBRARY: OnceLock<Vec<BrushSet>> = OnceLock::new();

/// The full library, borrowed. Textures are generated on the first call only.
pub fn library_ref() -> &'static [BrushSet] {
    LIBRARY.get_or_init(build_library)
}

/// Maps a set name from an older build (or a user brush library written by one) to its current
/// name. Current names and unknown names come back unchanged. Brush ids never changed, so
/// stored `lib:<id>` choices keep working; only display names moved.
pub fn canonical_set_name(name: &str) -> &str {
    // Compatibility reads: the old names are listed on purpose.
    match name {
        "Sketching" => "Graphite",
        "Inking" => "Inkwell",
        "Painting" => "Pigment",
        "Airbrushing" => "Mist",
        "Texture" => "Surfaces",
        "Charcoal" => "Char",
        "Watercolour" | "Watercolor" => "Washes",
        "Calligraphy" => "Nibs",
        other => other,
    }
}

/// Maps a preset name from an older build to its current name; see [`canonical_set_name`].
pub fn canonical_preset_name(name: &str) -> &str {
    match name {
        "Studio Ink" => "Draft Ink",
        "Technical Fineliner" => "Hairline Liner",
        "Fine Atomiser" => "Fine Atomizer",
        other => other,
    }
}

/// The full library as owned values. This clones the cached textures, so prefer
/// [`library_ref`] when a borrow is enough.
pub fn library() -> Vec<BrushSet> {
    library_ref().to_vec()
}

fn build_library() -> Vec<BrushSet> {
    presets::sets()
        .into_iter()
        .map(|set| BrushSet {
            name: set.name,
            brushes: set.recipes.into_iter().map(build_brush).collect(),
        })
        .collect()
}

fn build_brush(recipe: Recipe) -> LibraryBrush {
    let seed = seed_for(recipe.id);
    let mut brush = Brush {
        name: recipe.name,
        shape: match recipe.tip {
            TipKind::RoundSoft => eskiz_core::brushes::Shape::SoftRound,
            _ => eskiz_core::brushes::Shape::HardRound,
        },
        ..Brush::default()
    };
    if let Some((kind, moving)) = recipe.grain {
        brush.grain = match kind {
            GrainKind::PaperTooth => eskiz_core::brushes::Grain::Paper,
            _ => eskiz_core::brushes::Grain::Noise,
        };
        brush.grain_mode = if moving {
            eskiz_core::brushes::GrainMode::Moving
        } else {
            eskiz_core::brushes::GrainMode::Texturized
        };
    }
    (recipe.tune)(&mut brush);

    LibraryBrush {
        id: recipe.id,
        name: recipe.name,
        brush,
        tip: tip(recipe.tip, TIP_SIZE, seed),
        grain: recipe
            .grain
            .map(|(kind, _)| grain(kind, GRAIN_SIZE, seed ^ 0x6A09_E667_F3BC_C908)),
        tip_kind: Some(recipe.tip),
        grain_kind: recipe.grain.map(|(kind, _)| kind),
    }
}

/// A tip of `kind` at the library's tip size, with a seed fixed per kind. The Brush Bench uses
/// this when a tip is picked, so the same pick always gives the same tip.
pub fn generated_tip(kind: TipKind) -> GrayImage {
    tip(kind, TIP_SIZE, seed_for(kind.key()))
}

/// A grain of `kind` at the library's grain size, with a seed fixed per kind.
pub fn generated_grain(kind: GrainKind) -> GrayImage {
    grain(kind, GRAIN_SIZE, seed_for(kind.key()))
}

/// FNV-1a over the preset id, so every preset gets its own stable seed.
fn seed_for(id: &str) -> u64 {
    id.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn old_names_read_as_new_and_new_names_stay() {
        assert_eq!(canonical_set_name("Sketching"), "Graphite");
        assert_eq!(canonical_set_name("Watercolour"), "Washes");
        assert_eq!(canonical_set_name("Nibs"), "Nibs");
        assert_eq!(canonical_set_name("My Brushes"), "My Brushes");
        assert_eq!(canonical_preset_name("Studio Ink"), "Draft Ink");
        assert_eq!(canonical_preset_name("Technical Fineliner"), "Hairline Liner");
        assert_eq!(canonical_preset_name("Draft Ink"), "Draft Ink");
    }

    #[test]
    fn every_old_set_name_maps_to_a_built_in_set() {
        let names: Vec<&str> = library_ref().iter().map(|s| s.name).collect();
        for old in ["Sketching", "Inking", "Painting", "Airbrushing", "Texture", "Charcoal", "Watercolour", "Calligraphy"] {
            assert!(names.contains(&canonical_set_name(old)), "{old}");
        }
    }
}
