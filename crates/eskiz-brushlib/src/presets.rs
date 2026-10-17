//! The built-in preset recipes. Each recipe names an original brush, picks a
//! generated tip and optional grain, and tunes eskiz-core [`Brush`] fields.
//!
//! Recipes are plain data. [`crate::library`] turns them into textures and
//! brushes on first use.

use crate::grains::GrainKind;
use crate::tips::TipKind;
use eskiz_core::brushes::{Accumulation, Brush, Taper, TaperPair, Wet};
use std::f64::consts::FRAC_PI_4;

/// One preset before its textures are generated.
pub(crate) struct Recipe {
    pub id: &'static str,
    pub name: &'static str,
    pub tip: TipKind,
    /// Grain kind and whether it moves with each stamp (`true`) or is fixed to the
    /// canvas (`false`).
    pub grain: Option<(GrainKind, bool)>,
    /// Applies the brush's dynamics, spacing, accumulation and grain depth.
    pub tune: fn(&mut Brush),
}

/// A named set of recipes.
pub(crate) struct SetRecipe {
    pub name: &'static str,
    pub recipes: Vec<Recipe>,
}

/// Set both pencil and finger taper to the same start and end lengths (units of
/// 512 px,) and shape.
fn taper(b: &mut Brush, start: f64, end: f64, shape: f64) {
    let t = Taper {
        start_length: start,
        end_length: end,
        shape,
        ..Taper::default()
    };
    b.taper = TaperPair {
        pencil: t,
        finger: t,
    };
}

/// Wet-mixing settings for painterly presets.
fn wet(attack: f64, falloff: f64, load: f64, dilution: f64, tilt: f64) -> Wet {
    Wet {
        enabled: true,
        attack,
        falloff_amount: falloff,
        load_amount: load,
        dilution,
        tilt_angle: tilt,
        ..Wet::default()
    }
}

/// Every preset, grouped into the eight sets of the Brush Library panel.
pub(crate) fn sets() -> Vec<SetRecipe> {
    vec![
        SetRecipe {
            name: "Graphite",
            recipes: vec![
                Recipe {
                    id: "sketching.graphite-draft",
                    name: "Graphite Draft",
                    tip: TipKind::RoundHard,
                    grain: Some((GrainKind::PaperTooth, false)),
                    tune: |b| {
                        b.size = 5.0;
                        b.spacing = 0.06;
                        b.smoothing = 0.35;
                        b.pressure_size = 0.45;
                        b.pressure_opacity = 0.75;
                        b.opacity = 0.85;
                        b.grain_depth = 0.55;
                        b.grain_contrast = 0.3;
                        taper(b, 0.12, 0.2, 0.0);
                    },
                },
                Recipe {
                    id: "sketching.feather-sketch",
                    name: "Feather Sketch",
                    tip: TipKind::RoundSoft,
                    grain: None,
                    tune: |b| {
                        b.size = 3.5;
                        b.spacing = 0.05;
                        b.smoothing = 0.4;
                        b.pressure_size = 0.6;
                        b.pressure_opacity = 0.5;
                        b.opacity = 0.6;
                        b.flow = 0.8;
                    },
                },
                Recipe {
                    id: "sketching.blunt-lead",
                    name: "Blunt Lead",
                    tip: TipKind::TexturedRound,
                    grain: Some((GrainKind::PaperTooth, true)),
                    tune: |b| {
                        b.size = 9.0;
                        b.spacing = 0.07;
                        b.smoothing = 0.3;
                        b.pressure_size = 0.5;
                        b.pressure_opacity = 0.9;
                        b.grain_depth = 0.6;
                        b.grain_contrast = 0.2;
                    },
                },
                Recipe {
                    id: "sketching.construction-lead",
                    name: "Construction Lead",
                    tip: TipKind::RoundHard,
                    grain: None,
                    tune: |b| {
                        b.size = 1.8;
                        b.spacing = 0.04;
                        b.smoothing = 0.5;
                        b.pressure_size = 0.2;
                        b.pressure_opacity = 0.15;
                        b.opacity = 0.5;
                    },
                },
                Recipe {
                    id: "sketching.smudge-stick",
                    name: "Smudge Stick",
                    tip: TipKind::RoundSoft,
                    grain: Some((GrainKind::PaperTooth, false)),
                    tune: |b| {
                        b.size = 34.0;
                        b.spacing = 0.08;
                        b.opacity = 0.3;
                        b.flow = 0.2;
                        b.accumulation = Accumulation::Buildup;
                        b.pressure_size = 0.3;
                        b.pressure_opacity = 0.6;
                        b.grain_depth = 0.4;
                    },
                },
            ],
        },
        SetRecipe {
            name: "Inkwell",
            recipes: vec![
                Recipe {
                    id: "inking.studio-ink",
                    name: "Draft Ink",
                    tip: TipKind::RoundHard,
                    grain: None,
                    tune: |b| {
                        b.size = 8.0;
                        b.spacing = 0.04;
                        b.smoothing = 0.5;
                        b.pressure_size = 0.8;
                        b.pressure_opacity = 0.1;
                        taper(b, 0.3, 0.3, 0.0);
                    },
                },
                Recipe {
                    id: "inking.technical-fineliner",
                    name: "Hairline Liner",
                    tip: TipKind::RoundHard,
                    grain: None,
                    tune: |b| {
                        b.size = 2.2;
                        b.spacing = 0.03;
                        b.smoothing = 0.6;
                        b.pressure_size = 0.05;
                        b.pressure_opacity = 0.0;
                    },
                },
                Recipe {
                    id: "inking.brush-pen-stroke",
                    name: "Brush Pen Stroke",
                    tip: TipKind::RoundHard,
                    grain: None,
                    tune: |b| {
                        b.size = 22.0;
                        b.spacing = 0.03;
                        b.smoothing = 0.45;
                        b.pressure_size = 1.0;
                        b.min_size = 0.12;
                        b.speed_size = -0.2;
                        b.curves.size = vec![[0.0, 0.15], [0.6, 1.0], [1.0, 1.0]];
                        taper(b, 0.35, 0.25, 0.4);
                    },
                },
                Recipe {
                    id: "inking.sumi-edge",
                    name: "Sumi Edge",
                    tip: TipKind::TexturedRound,
                    grain: None,
                    tune: |b| {
                        b.size = 14.0;
                        b.spacing = 0.04;
                        b.smoothing = 0.5;
                        b.pressure_size = 0.7;
                        b.pressure_opacity = 0.2;
                        taper(b, 0.2, 0.2, 0.0);
                    },
                },
                Recipe {
                    id: "inking.mapping-nib",
                    name: "Mapping Nib",
                    tip: TipKind::Flat,
                    grain: None,
                    tune: |b| {
                        b.size = 9.0;
                        b.spacing = 0.03;
                        b.smoothing = 0.35;
                        b.shape_rotation = FRAC_PI_4 * 0.6;
                        b.pressure_size = 0.5;
                        b.pressure_opacity = 0.1;
                    },
                },
            ],
        },
        SetRecipe {
            name: "Pigment",
            recipes: vec![
                Recipe {
                    id: "painting.soft-oil-round",
                    name: "Soft Oil Round",
                    tip: TipKind::RoundSoft,
                    grain: Some((GrainKind::PaperTooth, false)),
                    tune: |b| {
                        b.size = 30.0;
                        b.spacing = 0.08;
                        b.pressure_size = 0.4;
                        b.pressure_opacity = 0.5;
                        b.grain_depth = 0.35;
                        b.wet = wet(0.25, 0.4, 0.3, 0.2, 0.5);
                    },
                },
                Recipe {
                    id: "painting.dry-gouache",
                    name: "Dry Gouache",
                    tip: TipKind::TexturedRound,
                    grain: Some((GrainKind::PaperTooth, true)),
                    tune: |b| {
                        b.size = 24.0;
                        b.spacing = 0.1;
                        b.pressure_size = 0.5;
                        b.pressure_opacity = 0.6;
                        b.opacity = 0.9;
                        b.grain_depth = 0.7;
                        b.grain_contrast = 0.25;
                        b.grain_brightness = 0.05;
                        b.wet = wet(0.15, 0.6, 0.5, 0.0, 0.3);
                    },
                },
                Recipe {
                    id: "painting.impasto-flat",
                    name: "Impasto Flat",
                    tip: TipKind::Flat,
                    grain: Some((GrainKind::Concrete, false)),
                    tune: |b| {
                        b.size = 28.0;
                        b.spacing = 0.12;
                        b.speed_size = -0.2;
                        b.follow_stroke = true;
                        b.pressure_size = 0.6;
                        b.pressure_opacity = 0.2;
                        b.grain_depth = 0.5;
                        b.grain_contrast = 0.4;
                    },
                },
                Recipe {
                    id: "painting.glaze-veil",
                    name: "Glaze Veil",
                    tip: TipKind::RoundSoft,
                    grain: None,
                    tune: |b| {
                        b.size = 36.0;
                        b.spacing = 0.05;
                        b.smoothing = 0.25;
                        b.opacity = 0.35;
                        b.flow = 0.6;
                        b.pressure_size = 0.1;
                        b.pressure_opacity = 0.8;
                    },
                },
                Recipe {
                    id: "painting.bristle-wash",
                    name: "Bristle Wash",
                    tip: TipKind::Bristle,
                    grain: Some((GrainKind::CanvasWeave, false)),
                    tune: |b| {
                        b.size = 34.0;
                        b.spacing = 0.07;
                        b.scatter = 0.05;
                        b.follow_stroke = true;
                        b.opacity = 0.9;
                        b.flow = 0.8;
                        b.pressure_opacity = 0.55;
                        b.grain_depth = 0.4;
                        b.grain_scale = 0.8;
                        b.wet = wet(0.2, 0.5, 0.4, 0.1, 0.4);
                    },
                },
            ],
        },
        SetRecipe {
            name: "Mist",
            recipes: vec![
                Recipe {
                    id: "airbrushing.velvet-mist",
                    name: "Velvet Mist",
                    tip: TipKind::RoundSoft,
                    grain: None,
                    tune: |b| {
                        b.size = 110.0;
                        b.spacing = 0.05;
                        b.smoothing = 0.2;
                        b.opacity = 0.6;
                        b.flow = 0.15;
                        b.accumulation = Accumulation::Buildup;
                        b.pressure_opacity = 1.0;
                        b.pressure_size = 0.0;
                    },
                },
                Recipe {
                    id: "airbrushing.fine-atomiser",
                    name: "Fine Atomizer",
                    tip: TipKind::SprayDots,
                    grain: None,
                    tune: |b| {
                        b.size = 40.0;
                        b.spacing = 0.06;
                        b.smoothing = 0.2;
                        b.flow = 0.25;
                        b.accumulation = Accumulation::Buildup;
                        b.pressure_opacity = 0.9;
                        b.scatter = 0.3;
                        b.size_jitter = 0.2;
                    },
                },
                Recipe {
                    id: "airbrushing.speckle-spray",
                    name: "Speckle Spray",
                    tip: TipKind::SprayDots,
                    grain: None,
                    tune: |b| {
                        b.size = 60.0;
                        b.spacing = 0.04;
                        b.opacity = 0.8;
                        b.flow = 0.5;
                        b.accumulation = Accumulation::Buildup;
                        b.pressure_size = 0.3;
                        b.pressure_opacity = 0.6;
                        b.scatter = 0.6;
                    },
                },
                Recipe {
                    id: "airbrushing.broad-aerograph",
                    name: "Broad Aerograph",
                    tip: TipKind::RoundSoft,
                    grain: None,
                    tune: |b| {
                        b.size = 90.0;
                        b.spacing = 0.05;
                        b.opacity = 0.7;
                        b.flow = 0.1;
                        b.accumulation = Accumulation::Buildup;
                        b.pressure_size = 0.2;
                        b.pressure_opacity = 0.5;
                        b.speed_opacity = 0.35;
                    },
                },
                Recipe {
                    id: "airbrushing.splatter-gun",
                    name: "Splatter Gun",
                    tip: TipKind::Splatter,
                    grain: None,
                    tune: |b| {
                        b.size = 50.0;
                        b.spacing = 0.18;
                        b.scatter = 0.8;
                        b.size_jitter = 0.5;
                        b.opacity_jitter = 0.3;
                        b.rotation_jitter = 0.5;
                        b.flow = 0.9;
                        b.pressure_size = 0.4;
                        b.pressure_opacity = 0.6;
                    },
                },
            ],
        },
        SetRecipe {
            name: "Surfaces",
            recipes: vec![
                Recipe {
                    id: "texture.canvas-dry-brush",
                    name: "Canvas Dry Brush",
                    tip: TipKind::Flat,
                    grain: Some((GrainKind::CanvasWeave, false)),
                    tune: |b| {
                        b.size = 26.0;
                        b.spacing = 0.09;
                        b.grain_scale = 0.5;
                        b.opacity = 0.95;
                        b.pressure_size = 0.4;
                        b.pressure_opacity = 0.7;
                        b.grain_depth = 0.85;
                        b.grain_contrast = 0.45;
                    },
                },
                Recipe {
                    id: "texture.paper-tooth-rub",
                    name: "Paper Tooth Rub",
                    tip: TipKind::TexturedRound,
                    grain: Some((GrainKind::PaperTooth, true)),
                    tune: |b| {
                        b.size = 22.0;
                        b.spacing = 0.1;
                        b.smoothing = 0.3;
                        b.pressure_size = 0.3;
                        b.pressure_opacity = 0.8;
                        b.grain_depth = 0.9;
                        b.grain_contrast = 0.35;
                    },
                },
                Recipe {
                    id: "texture.concrete-drag",
                    name: "Concrete Drag",
                    tip: TipKind::Flat,
                    grain: Some((GrainKind::Concrete, false)),
                    tune: |b| {
                        b.size = 40.0;
                        b.spacing = 0.1;
                        b.smoothing = 0.3;
                        b.opacity = 0.9;
                        b.pressure_size = 0.3;
                        b.pressure_opacity = 0.5;
                        b.grain_depth = 1.0;
                        b.grain_contrast = 0.6;
                        b.grain_brightness = -0.05;
                    },
                },
                Recipe {
                    id: "texture.cross-hatch-stamp",
                    name: "Cross-hatch Stamp",
                    tip: TipKind::Square,
                    grain: Some((GrainKind::CrossHatch, false)),
                    tune: |b| {
                        b.size = 18.0;
                        b.spacing = 0.12;
                        b.grain_scale = 0.5;
                        b.opacity = 0.95;
                        b.follow_stroke = true;
                        b.pressure_size = 0.4;
                        b.pressure_opacity = 0.4;
                        b.grain_depth = 0.8;
                        b.grain_contrast = 0.2;
                    },
                },
                Recipe {
                    id: "texture.stipple-field",
                    name: "Stipple Field",
                    tip: TipKind::SprayDots,
                    grain: Some((GrainKind::PaperTooth, true)),
                    tune: |b| {
                        b.size = 12.0;
                        b.spacing = 0.14;
                        b.smoothing = 0.1;
                        b.flow = 0.9;
                        b.scatter = 0.25;
                        b.pressure_size = 0.3;
                        b.pressure_opacity = 0.5;
                        b.grain_depth = 0.5;
                        b.grain_contrast = 0.3;
                    },
                },
            ],
        },
        SetRecipe {
            name: "Char",
            recipes: vec![
                Recipe {
                    id: "charcoal.compressed-charcoal",
                    name: "Compressed Charcoal",
                    tip: TipKind::TexturedRound,
                    grain: Some((GrainKind::CharcoalNoise, false)),
                    tune: |b| {
                        b.size = 26.0;
                        b.spacing = 0.08;
                        b.smoothing = 0.2;
                        b.pressure_size = 0.6;
                        b.pressure_opacity = 0.9;
                        b.grain_scale = 0.6;
                        b.grain_depth = 1.0;
                        b.grain_contrast = 0.8;
                    },
                },
                Recipe {
                    id: "charcoal.vine-stick",
                    name: "Vine Stick",
                    tip: TipKind::ChalkEdge,
                    grain: Some((GrainKind::CharcoalNoise, true)),
                    tune: |b| {
                        b.size = 14.0;
                        b.spacing = 0.06;
                        b.min_size = 0.3;
                        b.pressure_size = 0.8;
                        b.pressure_opacity = 0.6;
                        b.grain_scale = 0.5;
                        b.grain_depth = 0.8;
                        b.grain_contrast = 0.7;
                        taper(b, 0.3, 0.3, 0.0);
                    },
                },
                Recipe {
                    id: "charcoal.soft-willow",
                    name: "Soft Willow",
                    tip: TipKind::TexturedRound,
                    grain: Some((GrainKind::CharcoalNoise, false)),
                    tune: |b| {
                        b.size = 40.0;
                        b.spacing = 0.1;
                        b.opacity = 0.7;
                        b.flow = 0.6;
                        b.pressure_size = 0.4;
                        b.pressure_opacity = 0.5;
                        b.grain_scale = 0.8;
                        b.grain_depth = 0.5;
                        b.grain_contrast = 0.5;
                    },
                },
                Recipe {
                    id: "charcoal.smear-sheet",
                    name: "Smear Sheet",
                    tip: TipKind::RoundSoft,
                    grain: Some((GrainKind::CharcoalNoise, false)),
                    tune: |b| {
                        b.size = 50.0;
                        b.spacing = 0.07;
                        b.opacity = 0.7;
                        b.flow = 0.25;
                        b.accumulation = Accumulation::Buildup;
                        b.pressure_opacity = 0.8;
                        b.grain_scale = 0.8;
                        b.grain_depth = 0.35;
                        b.grain_contrast = 0.4;
                    },
                },
                Recipe {
                    id: "charcoal.charcoal-block",
                    name: "Charcoal Block",
                    tip: TipKind::Square,
                    grain: Some((GrainKind::CharcoalNoise, false)),
                    tune: |b| {
                        b.size = 32.0;
                        b.spacing = 0.09;
                        b.smoothing = 0.25;
                        b.pressure_size = 0.4;
                        b.pressure_opacity = 0.5;
                        b.grain_scale = 0.6;
                        b.grain_depth = 1.0;
                        b.grain_contrast = 0.9;
                    },
                },
            ],
        },
        SetRecipe {
            name: "Washes",
            recipes: vec![
                Recipe {
                    id: "watercolour.wet-bloom-round",
                    name: "Wet Bloom Round",
                    tip: TipKind::RoundSoft,
                    grain: Some((GrainKind::WatercolourBloom, true)),
                    tune: |b| {
                        b.size = 44.0;
                        b.spacing = 0.07;
                        b.flow = 0.7;
                        b.opacity = 0.8;
                        b.pressure_opacity = 0.5;
                        b.wet_edges = 0.6;
                        b.grain_scale = 1.2;
                        b.grain_depth = 0.6;
                        b.grain_contrast = 0.4;
                        b.wet = wet(0.35, 0.6, 0.2, 0.25, 0.4);
                    },
                },
                Recipe {
                    id: "watercolour.dry-pigment-edge",
                    name: "Dry Pigment Edge",
                    tip: TipKind::TexturedRound,
                    grain: Some((GrainKind::PaperTooth, false)),
                    tune: |b| {
                        b.size = 30.0;
                        b.spacing = 0.09;
                        b.opacity = 0.9;
                        b.pressure_opacity = 0.6;
                        b.wet_edges = 0.7;
                        b.burnt_edges = 0.25;
                        b.grain_depth = 0.5;
                        b.grain_contrast = 0.3;
                        b.wet = wet(0.1, 0.5, 0.3, 0.1, 0.4);
                    },
                },
                Recipe {
                    id: "watercolour.layered-wash",
                    name: "Layered Wash",
                    tip: TipKind::RoundSoft,
                    grain: Some((GrainKind::WatercolourBloom, false)),
                    tune: |b| {
                        b.size = 60.0;
                        b.spacing = 0.06;
                        b.opacity = 0.4;
                        b.flow = 0.5;
                        b.accumulation = Accumulation::Buildup;
                        b.wet_edges = 0.3;
                        b.pressure_size = 0.2;
                        b.pressure_opacity = 0.4;
                        b.grain_depth = 0.4;
                        b.grain_contrast = 0.35;
                    },
                },
                Recipe {
                    id: "watercolour.granulation-sponge",
                    name: "Granulation Sponge",
                    tip: TipKind::SprayDots,
                    grain: Some((GrainKind::Concrete, true)),
                    tune: |b| {
                        b.size = 40.0;
                        b.spacing = 0.12;
                        b.opacity = 0.7;
                        b.flow = 0.5;
                        b.scatter = 0.4;
                        b.accumulation = Accumulation::Buildup;
                        b.pressure_opacity = 0.6;
                        b.grain_depth = 0.3;
                        b.grain_contrast = 0.5;
                        b.wet = wet(0.2, 0.4, 0.2, 0.3, 0.4);
                    },
                },
                Recipe {
                    id: "watercolour.bleed-brush",
                    name: "Bleed Brush",
                    tip: TipKind::RoundSoft,
                    grain: None,
                    tune: |b| {
                        b.size = 28.0;
                        b.spacing = 0.05;
                        b.smoothing = 0.5;
                        b.bleed = 0.5;
                        b.wet_edges = 0.8;
                        b.burnt_edges = 0.1;
                        b.pressure_opacity = 0.5;
                        b.wet = wet(0.2, 0.3, 0.1, 0.4, 0.4);
                    },
                },
            ],
        },
        SetRecipe {
            name: "Nibs",
            recipes: vec![
                Recipe {
                    id: "calligraphy.broad-nib-script",
                    name: "Broad Nib Script",
                    tip: TipKind::Flat,
                    grain: None,
                    tune: |b| {
                        b.size = 18.0;
                        b.spacing = 0.03;
                        b.smoothing = 0.3;
                        b.shape_rotation = FRAC_PI_4;
                        b.pressure_size = 0.7;
                        b.pressure_opacity = 0.1;
                        taper(b, 0.2, 0.2, 0.2);
                    },
                },
                Recipe {
                    id: "calligraphy.pointed-pen-script",
                    name: "Pointed Pen Script",
                    tip: TipKind::Leaf,
                    grain: None,
                    tune: |b| {
                        b.size = 9.0;
                        b.spacing = 0.04;
                        b.smoothing = 0.4;
                        b.min_size = 0.1;
                        b.pressure_size = 0.9;
                        taper(b, 0.35, 0.35, 0.5);
                    },
                },
                Recipe {
                    id: "calligraphy.italic-chisel",
                    name: "Italic Chisel",
                    tip: TipKind::Flat,
                    grain: None,
                    tune: |b| {
                        b.size = 14.0;
                        b.spacing = 0.02;
                        b.smoothing = 0.35;
                        b.shape_rotation = 0.7;
                        b.speed_size = 0.3;
                        b.pressure_size = 0.2;
                        taper(b, 0.15, 0.15, 0.0);
                    },
                },
                Recipe {
                    id: "calligraphy.reed-quill",
                    name: "Reed Quill",
                    tip: TipKind::Leaf,
                    grain: None,
                    tune: |b| {
                        b.size = 16.0;
                        b.spacing = 0.04;
                        b.smoothing = 0.3;
                        b.min_size = 0.2;
                        b.pressure_size = 0.6;
                        taper(b, 0.15, 0.4, -0.2);
                    },
                },
                Recipe {
                    id: "calligraphy.ribbon-marker",
                    name: "Ribbon Marker",
                    tip: TipKind::Flat,
                    grain: None,
                    tune: |b| {
                        b.size = 30.0;
                        b.spacing = 0.04;
                        b.smoothing = 0.5;
                        b.flow = 1.0;
                        b.opacity = 0.9;
                        b.pressure_size = 0.1;
                        b.pressure_opacity = 0.2;
                        taper(b, 0.05, 0.05, 0.0);
                    },
                },
            ],
        },
    ]
}
