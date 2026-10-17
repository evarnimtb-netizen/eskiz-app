//! Brush settings and presets.
//!
//! Sizes are canvas pixels, angles are radians, and each response curve is a
//! list of control points over [0, 1] (identity is `[[0,0],[1,1]]`). Override
//! defaults with struct update syntax: `Brush { size: 40.0, ..Brush::default() }`.

/// Control points `[input, output]` of a response curve over [0, 1].
pub type Points = Vec<[f64; 2]>;

fn identity() -> Points {
    vec![[0.0, 0.0], [1.0, 1.0]]
}

fn constant_one() -> Points {
    vec![[0.0, 1.0], [1.0, 1.0]]
}

/// Brush tip shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    SoftRound,
    HardRound,
}

/// Grain texture. `None` disables grain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Grain {
    #[default]
    None,
    Noise,
    Paper,
}

/// Grain placement: `Texturized` is fixed in canvas space, `Moving` follows each stamp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GrainMode {
    #[default]
    Texturized,
    Moving,
}

/// Stroke accumulation: `Glaze` is max, `Buildup` is alpha-over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Accumulation {
    #[default]
    Glaze,
    Buildup,
}

/// Taper settings for one input type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Taper {
    /// Start ramp length in units of 512 px.
    pub start_length: f64,
    /// End ramp length in units of 512 px.
    pub end_length: f64,
    pub size: f64,
    pub opacity: f64,
    /// Taper shape in [-1, 1].
    pub shape: f64,
    /// How much the taper drives the pressure-driven channels.
    pub pressure: f64,
    /// Accepted for parity with the JS; live tip animation is not implemented.
    pub tip_animation: bool,
}

impl Default for Taper {
    fn default() -> Self {
        Self {
            start_length: 0.1,
            end_length: 0.1,
            size: 1.0,
            opacity: 0.0,
            shape: 0.0,
            pressure: 1.0,
            tip_animation: false,
        }
    }
}

/// Separate taper settings for pencil (pressure) and finger (no pressure) input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TaperPair {
    pub pencil: Taper,
    pub finger: Taper,
}

/// Wet mixing settings. Only used when `enabled`.
///
/// `attack`, `falloff_amount`, `load_amount` (charge), `dilution` and `tilt_angle` feed the
/// per-dab `mixture` (the share of fresh brush paint). The rest shape what the brush does with
/// the canvas paint it meets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wet {
    pub enabled: bool,
    pub attack: f64,
    pub falloff_amount: f64,
    pub load_amount: f64,
    pub dilution: f64,
    pub tilt_angle: f64,
    /// How strongly the brush picks up canvas paint, 0..1.
    pub pull: f64,
    /// Shape of the brush-versus-canvas transition, 0..1 (0.5 is linear).
    pub grade: f64,
    /// Softens the stroke's edges as it is laid, 0..1.
    pub wetness: f64,
    /// Lets the stroke spread beyond the tip's edge, 0..1.
    pub bleed: f64,
}

impl Default for Wet {
    fn default() -> Self {
        Self {
            enabled: false,
            attack: 0.0,
            falloff_amount: 0.0,
            load_amount: 0.0,
            dilution: 0.0,
            tilt_angle: 0.0,
            pull: 0.5,
            grade: 0.5,
            wetness: 0.0,
            bleed: 0.0,
        }
    }
}

/// User-editable response curves, as control points.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveSet {
    pub overall: Points,
    pub opacity: Points,
    pub size: Points,
    pub roundness: Points,
    pub bleed: Points,
    pub blur: Points,
    pub spacing_jitter: Points,
    /// Constant 1 by default.
    pub stamp_count: Points,
    /// Constant 1 by default.
    pub stamp_count_jitter: Points,
    pub attack: Points,
    pub load: Points,
    pub dilution: Points,
}

impl Default for CurveSet {
    fn default() -> Self {
        Self {
            overall: identity(),
            opacity: identity(),
            size: identity(),
            roundness: identity(),
            bleed: identity(),
            blur: identity(),
            spacing_jitter: identity(),
            stamp_count: constant_one(),
            stamp_count_jitter: constant_one(),
            attack: identity(),
            load: identity(),
            dilution: identity(),
        }
    }
}

/// Full brush settings. `Brush::default()` matches `DEFAULT_BRUSH` in brushes.js.
#[derive(Clone, Debug, PartialEq)]
pub struct Brush {
    pub name: &'static str,

    // Tip and grain.
    pub shape: Shape,
    pub invert_shape: bool,
    pub grain: Grain,
    pub grain_mode: GrainMode,
    pub grain_scale: f64,
    pub grain_depth: f64,
    pub grain_contrast: f64,
    pub grain_brightness: f64,
    pub invert_grain: bool,

    // Core dab settings.
    /// Diameter in px before dynamics.
    pub size: f64,
    pub opacity: f64,
    pub flow: f64,
    pub accumulation: Accumulation,
    pub wet_edges: f64,
    pub burnt_edges: f64,
    pub alpha_threshold: f64,

    // Spacing.
    pub spacing: f64,
    pub spacing_jitter: f64,
    pub spacing_speed: f64,

    // Smoothing.
    pub smoothing: f64,
    pub pressure_smoothing: f64,
    pub stabilization: f64,
    pub motion_amount: f64,
    pub motion_expression: f64,

    // Dynamics.
    /// Pressure-to-size amount, in [-1, 1].
    pub pressure_size: f64,
    /// Speed-to-size amount.
    pub speed_size: f64,
    /// Lower size clamp as a fraction of `size`.
    pub min_size: f64,
    /// Upper size clamp as a fraction of `size`.
    pub max_size: f64,
    pub pressure_roundness: f64,
    pub pressure_roundness_min: f64,
    pub shape_roundness: f64,
    pub pressure_opacity: f64,
    pub speed_opacity: f64,
    pub pressure_softening: bool,
    pub dynamics_blur: f64,
    pub bleed: f64,
    pub bleed_pressure: f64,

    // Pen tilt and barrel roll. All zero or off by default, so a brush ignores tilt
    // until it asks for it.
    /// Tilt-to-size in [-1, 1]: positive grows the tip as the pen lies down (up to 4x when
    /// flat), negative shrinks it.
    pub tilt_size: f64,
    /// Tilt-to-opacity in [-1, 1]: positive is darker when tilted, negative lighter.
    pub tilt_opacity: f64,
    /// Tilt-to-roundness in [0, 1]: flattens the tip as the pen lies down.
    pub tilt_roundness: f64,
    /// Rotate the tip to follow the pen azimuth.
    pub tilt_rotation: bool,
    /// Rotate the tip with the barrel roll.
    pub roll_rotation: bool,

    pub taper: TaperPair,

    // Random and per-stamp jitter.
    pub random_rotation: bool,
    pub shape_rotation: f64,
    pub follow_stroke: bool,
    pub rotation_jitter: f64,
    pub scatter: f64,
    pub size_jitter: f64,
    pub opacity_jitter: f64,
    pub stamp_count: f64,
    pub stamp_count_jitter: f64,
    pub hue_jitter: f64,
    pub saturation_jitter: f64,
    pub lightness_jitter: f64,
    pub darkness_jitter: f64,

    pub wet: Wet,
    pub curves: CurveSet,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            name: "Default",
            shape: Shape::SoftRound,
            invert_shape: false,
            grain: Grain::None,
            grain_mode: GrainMode::Texturized,
            grain_scale: 1.0,
            grain_depth: 0.0,
            grain_contrast: 0.0,
            grain_brightness: 0.0,
            invert_grain: false,
            size: 20.0,
            opacity: 1.0,
            flow: 1.0,
            accumulation: Accumulation::Glaze,
            wet_edges: 0.0,
            burnt_edges: 0.0,
            alpha_threshold: 0.0,
            spacing: 0.1,
            spacing_jitter: 0.0,
            spacing_speed: 0.0,
            smoothing: 0.3,
            pressure_smoothing: 0.0,
            stabilization: 0.0,
            motion_amount: 0.0,
            motion_expression: 0.0,
            pressure_size: 1.0,
            speed_size: 0.0,
            min_size: 0.0,
            max_size: 1.0,
            pressure_roundness: 1.0,
            pressure_roundness_min: 1.0,
            shape_roundness: 1.0,
            pressure_opacity: 0.3,
            speed_opacity: 0.0,
            pressure_softening: false,
            dynamics_blur: 0.0,
            bleed: 1.0,
            bleed_pressure: 0.0,
            tilt_size: 0.0,
            tilt_opacity: 0.0,
            tilt_roundness: 0.0,
            tilt_rotation: false,
            roll_rotation: false,
            taper: TaperPair::default(),
            random_rotation: false,
            shape_rotation: 0.0,
            follow_stroke: false,
            rotation_jitter: 0.0,
            scatter: 0.0,
            size_jitter: 0.0,
            opacity_jitter: 0.0,
            stamp_count: 0.0,
            stamp_count_jitter: 0.0,
            hue_jitter: 0.0,
            saturation_jitter: 0.0,
            lightness_jitter: 0.0,
            darkness_jitter: 0.0,
            wet: Wet::default(),
            curves: CurveSet::default(),
        }
    }
}

/// Presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    TechnicalPen,
    SoftAirbrush,
    Pencil,
    InkWithTaper,
    // Eskiz's own additions (varied from the four above).
    FineLiner,
    BrushPen,
    SoftPencil,
    Charcoal,
    HardAirbrush,
    Speckle,
    Gouache,
    Watercolour,
}

impl Preset {
    /// Every preset, in library order.
    pub const ALL: [Preset; 12] = [
        Preset::TechnicalPen,
        Preset::InkWithTaper,
        Preset::FineLiner,
        Preset::BrushPen,
        Preset::Pencil,
        Preset::SoftPencil,
        Preset::Charcoal,
        Preset::SoftAirbrush,
        Preset::HardAirbrush,
        Preset::Speckle,
        Preset::Gouache,
        Preset::Watercolour,
    ];
}

impl Brush {
    /// Build one of the section 8 presets on top of the defaults.
    pub fn preset(preset: Preset) -> Self {
        match preset {
            Preset::TechnicalPen => Self {
                name: "Technical pen",
                shape: Shape::HardRound,
                spacing: 0.05,
                pressure_size: 1.0,
                smoothing: 0.5,
                ..Self::default()
            },
            Preset::SoftAirbrush => Self {
                name: "Soft airbrush",
                shape: Shape::SoftRound,
                size: 120.0,
                opacity: 0.6,
                flow: 0.15,
                accumulation: Accumulation::Buildup,
                pressure_opacity: 1.0,
                ..Self::default()
            },
            Preset::Pencil => Self {
                name: "Pencil",
                shape: Shape::HardRound,
                grain: Grain::Noise,
                grain_mode: GrainMode::Texturized,
                grain_depth: 0.8,
                spacing: 0.08,
                pressure_opacity: 1.0,
                ..Self::default()
            },
            Preset::InkWithTaper => {
                let ink = Taper {
                    start_length: 0.3,
                    end_length: 0.3,
                    size: 1.0,
                    ..Taper::default()
                };
                Self {
                    name: "Ink with taper",
                    shape: Shape::HardRound,
                    taper: TaperPair {
                        pencil: ink,
                        finger: ink,
                    },
                    ..Self::default()
                }
            }
            Preset::FineLiner => Self {
                name: "Fine liner",
                shape: Shape::HardRound,
                size: 8.0,
                spacing: 0.04,
                smoothing: 0.6,
                pressure_size: 0.4,
                ..Self::default()
            },
            Preset::BrushPen => {
                let tip = Taper {
                    start_length: 0.2,
                    end_length: 0.5,
                    size: 1.0,
                    shape: 0.4,
                    ..Taper::default()
                };
                Self {
                    name: "Brush pen",
                    shape: Shape::HardRound,
                    size: 34.0,
                    spacing: 0.04,
                    smoothing: 0.7,
                    pressure_size: 1.0,
                    min_size: 0.08,
                    taper: TaperPair { pencil: tip, finger: tip },
                    ..Self::default()
                }
            }
            Preset::SoftPencil => Self {
                name: "Soft pencil",
                shape: Shape::SoftRound,
                size: 26.0,
                grain: Grain::Paper,
                grain_mode: GrainMode::Texturized,
                grain_depth: 0.6,
                spacing: 0.07,
                pressure_opacity: 1.0,
                opacity: 0.85,
                ..Self::default()
            },
            Preset::Charcoal => Self {
                name: "Charcoal",
                shape: Shape::SoftRound,
                size: 48.0,
                grain: Grain::Noise,
                grain_mode: GrainMode::Texturized,
                grain_depth: 1.0,
                grain_contrast: 0.3,
                spacing: 0.1,
                pressure_size: 0.6,
                pressure_opacity: 1.0,
                accumulation: Accumulation::Buildup,
                flow: 0.5,
                ..Self::default()
            },
            Preset::HardAirbrush => Self {
                name: "Hard airbrush",
                shape: Shape::HardRound,
                size: 80.0,
                spacing: 0.03,
                opacity: 0.8,
                flow: 0.2,
                accumulation: Accumulation::Buildup,
                pressure_opacity: 1.0,
                ..Self::default()
            },
            Preset::Speckle => Self {
                name: "Speckle",
                shape: Shape::HardRound,
                size: 10.0,
                spacing: 0.9,
                scatter: 1.2,
                stamp_count: 3.0,
                size_jitter: 0.6,
                opacity_jitter: 0.4,
                pressure_size: 0.5,
                ..Self::default()
            },
            Preset::Gouache => Self {
                name: "Gouache",
                shape: Shape::HardRound,
                size: 44.0,
                spacing: 0.06,
                pressure_size: 0.5,
                min_size: 0.4,
                pressure_opacity: 0.1,
                wet_edges: 0.35,
                smoothing: 0.5,
                ..Self::default()
            },
            Preset::Watercolour => Self {
                name: "Watercolor",
                shape: Shape::SoftRound,
                size: 60.0,
                opacity: 0.55,
                flow: 0.35,
                accumulation: Accumulation::Glaze,
                wet_edges: 0.5,
                pressure_opacity: 0.8,
                pressure_size: 0.4,
                min_size: 0.5,
                ..Self::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_listed_once_with_unique_names() {
        let mut names: Vec<&str> = Preset::ALL.iter().map(|p| Brush::preset(*p).name).collect();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n);
    }
}
