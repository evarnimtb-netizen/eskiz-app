//! The Brush Bench's settings, as data: which section each brush setting lives in, its
//! label, range, unit and how to read and write it. The panel (`studio`) only draws this
//! table, so adding a setting to the engine means adding one line here.
//!
//! Every setting of `eskiz_core::Brush` that the engine acts on is listed in exactly one
//! section; a test below checks that no field is missing. The tip and grain images come from
//! `eskiz-brushlib` (the Shape and Grain sections pick a generator), or stay as the brush brought
//! them when they were imported.

use eskiz_core::brushes::{Brush, CurveSet, Points};

use crate::icons::IconFn;
use crate::icons_brush as ib;

/// The Studio's left column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    StrokePath,
    Smoothing,
    Taper,
    Shape,
    Grain,
    Rendering,
    WetMix,
    ColourDynamics,
    Dynamics,
    Pencil,
    Properties,
    About,
}

impl Section {
    pub const ALL: [Section; 12] = [
        Section::StrokePath,
        Section::Smoothing,
        Section::Taper,
        Section::Shape,
        Section::Grain,
        Section::Rendering,
        Section::WetMix,
        Section::ColourDynamics,
        Section::Dynamics,
        Section::Pencil,
        Section::Properties,
        Section::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::StrokePath => "Stroke path",
            Section::Smoothing => "Smoothing",
            Section::Taper => "Taper",
            Section::Shape => "Shape",
            Section::Grain => "Grain",
            Section::Rendering => "Rendering",
            Section::WetMix => "Wet mix",
            Section::ColourDynamics => "Color dynamics",
            Section::Dynamics => "Dynamics",
            Section::Pencil => "Pressure and tilt",
            Section::Properties => "Properties",
            Section::About => "About this brush",
        }
    }

    /// One line under the section title.
    pub fn blurb(self) -> &'static str {
        match self {
            Section::StrokePath => "How far apart the stamps sit, and how they scatter.",
            Section::Smoothing => "Smooth the line as you draw, with a lag that catches up at the tip.",
            Section::Taper => "How the stroke thins at its start and end.",
            Section::Shape => "The tip that is stamped along the stroke.",
            Section::Grain => "A texture that paint shows through.",
            Section::Rendering => "How stamps build up into a stroke, and how its edge looks.",
            Section::WetMix => "How much paint mixes with the color already on the layer.",
            Section::ColourDynamics => "Random color shifts. Each stroke picks its own.",
            Section::Dynamics => "What pressure and speed do to size and opacity.",
            Section::Pencil => "Pen tilt and roll, and the pressure response curves.",
            Section::Properties => "The size and opacity the brush starts with.",
            Section::About => "Name and where this brush comes from.",
        }
    }

    pub fn icon(self) -> IconFn {
        match self {
            Section::StrokePath => ib::icon_path,
            Section::Smoothing => ib::icon_smoothing,
            Section::Taper => ib::icon_taper,
            Section::Shape => ib::icon_shape,
            Section::Grain => ib::icon_grain,
            Section::Rendering => ib::icon_rendering,
            Section::WetMix => ib::icon_wet,
            Section::ColourDynamics => ib::icon_colour_dynamics,
            Section::Dynamics => ib::icon_dynamics,
            Section::Pencil => ib::icon_pencil_tilt,
            Section::Properties => ib::icon_properties,
            Section::About => ib::icon_about,
        }
    }
}

/// How a slider's value is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    /// 0..1 shown as 0..100 %.
    Percent,
    /// Canvas pixels.
    Pixels,
    /// An angle stored in radians, shown in degrees.
    Degrees,
    /// A plain number with this many decimals.
    Plain(usize),
    /// A multiplier, like `1.50x`.
    Times,
    /// A whole number.
    Count,
}

impl Unit {
    /// The text shown beside a slider for `v`.
    pub fn format(self, v: f64) -> String {
        match self {
            Unit::Percent => format!("{:.0}%", v * 100.0),
            Unit::Pixels if v < 10.0 => format!("{v:.1} px"),
            Unit::Pixels => format!("{v:.0} px"),
            Unit::Degrees => format!("{:.0}\u{b0}", v.to_degrees()),
            Unit::Plain(d) => format!("{v:.d$}"),
            Unit::Times => format!("{v:.2}\u{d7}"),
            Unit::Count => format!("{v:.0}"),
        }
    }
}

/// A numeric setting drawn as a slider.
#[derive(Clone, Copy)]
pub struct SliderDef {
    pub label: &'static str,
    pub help: &'static str,
    pub min: f64,
    pub max: f64,
    pub unit: Unit,
    /// Map the track logarithmically (sizes).
    pub log: bool,
    /// Snap to whole multiples of this (0 = continuous).
    pub step: f64,
    pub get: fn(&Brush) -> f64,
    pub set: fn(&mut Brush, f64),
}

impl SliderDef {
    /// The value in a fresh `Brush::default()`.
    pub fn default_value(&self) -> f64 {
        (self.get)(&Brush::default())
    }

    /// Position of `v` along the track, 0..1.
    pub fn to_unit(&self, v: f64) -> f64 {
        let t = if self.log {
            (v.max(self.min).ln() - self.min.ln()) / (self.max.ln() - self.min.ln())
        } else {
            (v - self.min) / (self.max - self.min)
        };
        t.clamp(0.0, 1.0)
    }

    /// The value at track position `t`, snapped to `step`.
    pub fn from_unit(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        let v = if self.log {
            (self.min.ln() + t * (self.max.ln() - self.min.ln())).exp()
        } else {
            self.min + t * (self.max - self.min)
        };
        let v = if self.step > 0.0 { (v / self.step).round() * self.step } else { v };
        v.clamp(self.min, self.max)
    }
}

/// An on/off setting.
#[derive(Clone, Copy)]
pub struct ToggleDef {
    pub label: &'static str,
    pub help: &'static str,
    pub get: fn(&Brush) -> bool,
    pub set: fn(&mut Brush, bool),
}

/// The response curves the Studio can edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CurveId {
    Overall,
    Size,
    Opacity,
    Roundness,
    Attack,
    Load,
    Dilution,
}

impl CurveId {
    pub fn label(self) -> &'static str {
        match self {
            CurveId::Overall => "Overall",
            CurveId::Size => "Size",
            CurveId::Opacity => "Opacity",
            CurveId::Roundness => "Roundness",
            CurveId::Attack => "Attack",
            CurveId::Load => "Charge",
            CurveId::Dilution => "Dilution",
        }
    }

    pub fn points(self, c: &CurveSet) -> &Points {
        match self {
            CurveId::Overall => &c.overall,
            CurveId::Size => &c.size,
            CurveId::Opacity => &c.opacity,
            CurveId::Roundness => &c.roundness,
            CurveId::Attack => &c.attack,
            CurveId::Load => &c.load,
            CurveId::Dilution => &c.dilution,
        }
    }

    pub fn points_mut(self, c: &mut CurveSet) -> &mut Points {
        match self {
            CurveId::Overall => &mut c.overall,
            CurveId::Size => &mut c.size,
            CurveId::Opacity => &mut c.opacity,
            CurveId::Roundness => &mut c.roundness,
            CurveId::Attack => &mut c.attack,
            CurveId::Load => &mut c.load,
            CurveId::Dilution => &mut c.dilution,
        }
    }
}

/// Curves offered under Pressure and tilt.
pub const PRESSURE_CURVES: &[CurveId] = &[CurveId::Overall, CurveId::Size, CurveId::Opacity, CurveId::Roundness];
/// Curves offered under Wet mix.
pub const WET_CURVES: &[CurveId] = &[CurveId::Attack, CurveId::Load, CurveId::Dilution];

/// One row of a section.
#[derive(Clone, Copy)]
pub enum Control {
    /// A small heading over the rows below it.
    Group(&'static str),
    Slider(SliderDef),
    Toggle(ToggleDef),
    /// A dim explanation.
    Note(&'static str),
    /// The grid of tips.
    TipPicker,
    /// The grid of grains.
    GrainPicker,
    /// Moving or texturized grain.
    GrainMode,
    /// Glaze or build-up.
    Accumulation,
    /// The pencil / touch taper switch.
    TaperTabs,
    /// A curve editor with a chip per curve.
    Curves(&'static [CurveId]),
    /// The name, origin and actions (About only).
    AboutCard,
}

macro_rules! slider {
    ($label:expr, $help:expr, $min:expr, $max:expr, $unit:expr, $($path:tt)+) => {
        Control::Slider(SliderDef {
            label: $label,
            help: $help,
            min: $min,
            max: $max,
            unit: $unit,
            log: false,
            step: 0.0,
            get: |b: &Brush| b.$($path)+,
            set: |b: &mut Brush, v: f64| b.$($path)+ = v,
        })
    };
}

macro_rules! toggle {
    ($label:expr, $help:expr, $($path:tt)+) => {
        Control::Toggle(ToggleDef {
            label: $label,
            help: $help,
            get: |b: &Brush| b.$($path)+,
            set: |b: &mut Brush, v: bool| b.$($path)+ = v,
        })
    };
}

/// The rows of a taper tab. `finger` selects the touch taper over the pencil taper.
fn taper_rows(finger: bool) -> Vec<Control> {
    macro_rules! pair {
        ($field:ident, $label:expr, $help:expr, $min:expr, $max:expr) => {
            if finger {
                slider!($label, $help, $min, $max, Unit::Percent, taper.finger.$field)
            } else {
                slider!($label, $help, $min, $max, Unit::Percent, taper.pencil.$field)
            }
        };
    }
    vec![
        Control::TaperTabs,
        Control::Group("Start"),
        pair!(start_length, "Length", "How far the start ramp reaches, as a share of 512 px.", 0.0, 1.0),
        Control::Group("End"),
        pair!(end_length, "Length", "How far the end ramp reaches, as a share of 512 px.", 0.0, 1.0),
        Control::Group("Effect"),
        pair!(size, "Size", "How much the tip shrinks at the ends.", 0.0, 1.0),
        pair!(opacity, "Opacity", "How much the stroke fades at the ends.", 0.0, 1.0),
        pair!(shape, "Shape", "Negative tapers sharply and late, positive gently and early.", -1.0, 1.0),
        pair!(pressure, "Pressure", "How much the taper also scales pressure-driven size and opacity.", 0.0, 1.0),
    ]
}

/// The rows of `section`. `finger` picks the touch taper tab.
pub fn controls(section: Section, finger: bool) -> Vec<Control> {
    match section {
        Section::StrokePath => vec![
            Control::Group("Spacing"),
            slider!("Spacing", "Distance between stamps, as a share of the tip size.", 0.0, 1.0, Unit::Percent, spacing),
            slider!("Spacing jitter", "Random change to the distance between stamps.", 0.0, 1.0, Unit::Percent, spacing_jitter),
            slider!("Speed spacing", "Stamps spread out as you draw faster.", 0.0, 1.0, Unit::Percent, spacing_speed),
            Control::Group("Scatter"),
            slider!("Scatter", "How far stamps land from the line, as a share of the tip size.", 0.0, 3.0, Unit::Percent, scatter),
            Control::Slider(SliderDef {
                label: "Extra stamps",
                help: "More stamps at every step. Zero is one stamp.",
                min: 0.0,
                max: 15.0,
                unit: Unit::Count,
                log: false,
                step: 1.0,
                get: |b| b.stamp_count,
                set: |b, v| b.stamp_count = v,
            }),
            slider!("Stamp count jitter", "Random change to the number of stamps.", 0.0, 1.0, Unit::Percent, stamp_count_jitter),
        ],
        Section::Smoothing => vec![
            Control::Group("Smoothing"),
            slider!("Amount", "Smooths the line. The newest part still reaches the pen tip.", 0.0, 1.0, Unit::Percent, smoothing),
            slider!("Pressure", "Smooths changes in pressure along the line.", 0.0, 1.0, Unit::Percent, pressure_smoothing),
            Control::Group("Stabilization"),
            slider!("Amount", "Averages the line over its neighbors for very steady curves.", 0.0, 1.0, Unit::Percent, stabilization),
            Control::Group("Motion filtering"),
            slider!("Amount", "Filters out shake. High values lag behind the pen.", 0.0, 1.0, Unit::Percent, motion_amount),
            slider!("Expression", "How much of the quick, deliberate movement it keeps.", 0.0, 1.0, Unit::Percent, motion_expression),
        ],
        Section::Taper => taper_rows(finger),
        Section::Shape => vec![
            Control::TipPicker,
            toggle!("Invert shape", "Paint where the tip is empty.", invert_shape),
            Control::Group("Orientation"),
            slider!("Rotation", "Turns the tip.", -std::f64::consts::PI, std::f64::consts::PI, Unit::Degrees, shape_rotation),
            toggle!("Follow stroke", "Turn the tip to point along the line.", follow_stroke),
            toggle!("Randomize", "Start every stroke with a random rotation.", random_rotation),
            slider!("Rotation jitter", "Random turn of every stamp.", 0.0, 1.0, Unit::Percent, rotation_jitter),
            Control::Group("Proportions"),
            slider!("Roundness", "Squash the tip. 100% keeps it as drawn.", 0.02, 1.0, Unit::Percent, shape_roundness),
            slider!("Pressure roundness", "How far pressure moves the tip toward its full shape.", 0.0, 1.0, Unit::Percent, pressure_roundness),
            slider!("Minimum roundness", "The roundness at the lightest pressure.", 0.0, 1.0, Unit::Percent, pressure_roundness_min),
        ],
        Section::Grain => vec![
            Control::GrainPicker,
            Control::GrainMode,
            Control::Group("Texture"),
            Control::Slider(SliderDef {
                label: "Scale",
                help: "The size of the texture.",
                min: 0.1,
                max: 4.0,
                unit: Unit::Times,
                log: true,
                step: 0.0,
                get: |b| b.grain_scale,
                set: |b, v| b.grain_scale = v,
            }),
            slider!("Depth", "How strongly the grain shows. At zero it has no effect.", 0.0, 1.0, Unit::Percent, grain_depth),
            slider!("Contrast", "Sharpens or softens the grain.", -1.0, 2.0, Unit::Percent, grain_contrast),
            slider!("Brightness", "Lets more paint through (up) or less (down).", -1.0, 1.0, Unit::Percent, grain_brightness),
            toggle!("Invert grain", "Swap the paint and the gaps.", invert_grain),
        ],
        Section::Rendering => vec![
            Control::Accumulation,
            Control::Group("Flow"),
            slider!("Flow", "How much paint each stamp lays down.", 0.0, 1.0, Unit::Percent, flow),
            Control::Group("Edges"),
            slider!("Wet edges", "Darken the rim of a stroke, like watercolor.", 0.0, 1.0, Unit::Percent, wet_edges),
            slider!("Burnt edges", "Darken the outermost band of a stroke.", 0.0, 1.0, Unit::Percent, burnt_edges),
            slider!("Alpha threshold", "Cut away the faint outer part of a stroke for a hard edge.", 0.0, 1.0, Unit::Percent, alpha_threshold),
        ],
        Section::WetMix => {
            let mut v = vec![
                toggle!("Wet mix", "Blend this brush's paint with the color underneath.", wet.enabled),
                Control::Note("Paint thinned by dilution and fall-off lets the color underneath, and what the brush has picked up, show through."),
                Control::Group("Mixing"),
                slider!("Attack", "How quickly the brush picks up color from the layer.", 0.0, 1.0, Unit::Percent, wet.attack),
                slider!("Fall-off", "How soon the paint runs out along a stroke.", 0.0, 1.0, Unit::Percent, wet.falloff_amount),
                slider!("Charge", "How much paint the brush carries.", 0.0, 1.0, Unit::Percent, wet.load_amount),
                slider!("Dilution", "Thins the paint with water.", 0.0, 1.0, Unit::Percent, wet.dilution),
                slider!("Tilt angle", "How far the pen must tilt before it picks up color.", 0.0, 1.0, Unit::Percent, wet.tilt_angle),
                Control::Group("Pickup"),
                slider!("Pull", "How much color the brush drags up from the layer and carries along.", 0.0, 1.0, Unit::Percent, wet.pull),
                slider!("Grade", "How quickly the brush's own color gives way to the picked-up color. Half is even.", 0.0, 1.0, Unit::Percent, wet.grade),
                Control::Group("Edges"),
                slider!("Wetness", "Softens the edge of the stroke as it is laid.", 0.0, 1.0, Unit::Percent, wet.wetness),
                slider!("Bleed", "Lets the paint spread beyond the edge of the tip.", 0.0, 1.0, Unit::Percent, wet.bleed),
                Control::Group("Response"),
            ];
            v.push(Control::Curves(WET_CURVES));
            v
        }
        Section::ColourDynamics => vec![
            Control::Group("Per stroke"),
            slider!("Hue", "Random shift around the color wheel.", 0.0, 1.0, Unit::Percent, hue_jitter),
            slider!("Saturation", "Random gain in saturation.", 0.0, 1.0, Unit::Percent, saturation_jitter),
            slider!("Lightness", "Random gain in lightness.", 0.0, 1.0, Unit::Percent, lightness_jitter),
            slider!("Darkness", "Random loss of lightness.", 0.0, 1.0, Unit::Percent, darkness_jitter),
        ],
        Section::Dynamics => vec![
            Control::Group("Pressure"),
            slider!("Size", "Press harder for a bigger tip (positive) or a smaller one (negative).", -1.0, 1.0, Unit::Percent, pressure_size),
            slider!("Opacity", "Press harder for more paint (positive) or less (negative).", -1.0, 1.0, Unit::Percent, pressure_opacity),
            toggle!("Smooth pressure spikes", "Blend pressure across very short steps.", pressure_softening),
            Control::Group("Speed"),
            slider!("Size", "Draw faster for a smaller tip (positive) or a bigger one (negative).", -1.0, 1.0, Unit::Percent, speed_size),
            slider!("Opacity", "Draw faster for less paint (positive) or more (negative).", -1.0, 1.0, Unit::Percent, speed_opacity),
            Control::Group("Jitter"),
            slider!("Size jitter", "Random size of every stamp.", 0.0, 1.0, Unit::Percent, size_jitter),
            slider!("Opacity jitter", "Random opacity of every stamp.", 0.0, 1.0, Unit::Percent, opacity_jitter),
        ],
        Section::Pencil => vec![
            Control::Group("Tilt"),
            slider!("Size", "Lean the pen for a bigger tip (positive) or a smaller one (negative).", -1.0, 1.0, Unit::Percent, tilt_size),
            slider!("Opacity", "Lean the pen for more paint (positive) or less (negative).", -1.0, 1.0, Unit::Percent, tilt_opacity),
            slider!("Flatten", "Lean the pen to flatten the tip, like the side of a lead.", 0.0, 1.0, Unit::Percent, tilt_roundness),
            toggle!("Turn with the pen", "Point the tip the way the pen leans.", tilt_rotation),
            Control::Group("Barrel roll"),
            toggle!("Turn with the roll", "Rotate the tip as the pen rolls between your fingers.", roll_rotation),
            Control::Note("Tilt and roll need a pen that reports them. A mouse or finger counts as upright."),
            Control::Group("Pressure curves"),
            Control::Curves(PRESSURE_CURVES),
        ],
        Section::Properties => vec![
            Control::Group("Size"),
            Control::Slider(SliderDef {
                label: "Size",
                help: "The tip width when the brush is chosen.",
                min: 1.0,
                max: 400.0,
                unit: Unit::Pixels,
                log: true,
                step: 0.0,
                get: |b| b.size,
                set: |b, v| b.size = v,
            }),
            slider!("Minimum size", "The smallest the tip gets, as a share of the size.", 0.0, 1.0, Unit::Percent, min_size),
            slider!("Maximum size", "The largest pressure and speed can make it, as a share of the size.", 0.0, 2.0, Unit::Percent, max_size),
            Control::Group("Paint"),
            slider!("Opacity", "The opacity when the brush is chosen.", 0.0, 1.0, Unit::Percent, opacity),
        ],
        Section::About => vec![Control::AboutCard],
    }
}

/// Every slider and toggle of every section (both taper tabs), for checks and tools.
pub fn all_controls() -> Vec<(Section, Control)> {
    let mut out = Vec::new();
    for s in Section::ALL {
        for finger in [false, true] {
            for c in controls(s, finger) {
                out.push((s, c));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush_settings::{to_settings, PREFIX};

    /// Brush settings the Studio has no control for: the name has its own field, and the engine
    /// does not act on these yet (wet mixing is saved with the brush, see the Wet mix note).
    const NOT_EDITED: &[&str] = &["dynamics_blur", "bleed", "bleed_pressure", "tip_animation"];
    /// Settings with a bespoke control instead of a slider or toggle.
    const PICKED: &[&str] = &["shape", "grain", "grain_mode", "accumulation"];

    /// Change every slider and toggle one at a time and see which file keys change.
    fn touched_keys() -> Vec<String> {
        let base = Brush::default();
        let base_settings = to_settings(&base);
        let mut keys = Vec::new();
        for (_, c) in all_controls() {
            let mut b = base.clone();
            match c {
                Control::Slider(s) => {
                    let to = if ((s.get)(&base) - s.max).abs() > 1e-9 { s.max } else { s.min };
                    (s.set)(&mut b, to);
                }
                Control::Toggle(t) => (t.set)(&mut b, !(t.get)(&base)),
                _ => continue,
            }
            for (k, v) in to_settings(&b) {
                if base_settings.get(&k) != Some(&v) {
                    keys.push(k);
                }
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    #[test]
    fn every_engine_setting_has_a_control() {
        let keys = touched_keys();
        for key in to_settings(&Brush::default()).keys() {
            let Some(key) = key.strip_prefix(PREFIX) else { continue };
            if key.starts_with("curve.") || PICKED.contains(&key) || key == "version" {
                continue;
            }
            if NOT_EDITED.iter().any(|n| key == *n || key.ends_with(&format!(".{n}"))) {
                continue;
            }
            assert!(keys.iter().any(|k| k == &format!("{PREFIX}{key}")), "no Studio control edits `{key}`");
        }
    }

    #[test]
    fn every_curve_is_editable_or_deliberately_left_out() {
        // The documented first-version curves plus the wet curves are editable; the rest of the
        // engine's curve set stays at identity.
        let editable: Vec<CurveId> = PRESSURE_CURVES.iter().chain(WET_CURVES).copied().collect();
        assert_eq!(editable.len(), 7);
        let mut set = CurveSet::default();
        for id in &editable {
            id.points_mut(&mut set).push([0.5, 0.5]);
        }
        for id in editable {
            assert_eq!(id.points(&set).len(), 3, "{id:?}");
        }
    }

    #[test]
    fn slider_ranges_contain_the_values_of_every_library_brush() {
        let mut brushes = vec![("Default", Brush::default())];
        brushes.extend(eskiz_brushlib::library_ref().iter().flat_map(|s| &s.brushes).map(|b| (b.name, b.brush.clone())));
        for (sec, c) in all_controls() {
            let Control::Slider(s) = c else { continue };
            assert!(s.min < s.max, "{} has an empty range", s.label);
            for (name, b) in &brushes {
                let v = (s.get)(b);
                assert!(
                    v >= s.min - 1e-9 && v <= s.max + 1e-9,
                    "{:?} / {} = {v} of {name} is outside {}..{}",
                    sec,
                    s.label,
                    s.min,
                    s.max
                );
            }
        }
    }

    #[test]
    fn sliders_map_both_ways_and_snap() {
        for (_, c) in all_controls() {
            let Control::Slider(s) = c else { continue };
            assert!((s.from_unit(0.0) - s.min).abs() < 1e-9, "{}", s.label);
            assert!((s.from_unit(1.0) - s.max).abs() < 1e-9, "{}", s.label);
            if s.step == 0.0 {
                let mid = s.from_unit(0.37);
                assert!((s.to_unit(mid) - 0.37).abs() < 1e-9, "{}", s.label);
            }
        }
        let count = controls(Section::StrokePath, false)
            .into_iter()
            .find_map(|c| match c {
                Control::Slider(s) if s.label == "Extra stamps" => Some(s),
                _ => None,
            })
            .unwrap();
        assert_eq!(count.from_unit(0.5), 8.0);
    }

    #[test]
    fn units_format_as_expected() {
        assert_eq!(Unit::Percent.format(0.256), "26%");
        assert_eq!(Unit::Pixels.format(24.0), "24 px");
        assert_eq!(Unit::Pixels.format(4.25), "4.2 px");
        assert_eq!(Unit::Degrees.format(std::f64::consts::FRAC_PI_2), "90\u{b0}");
        assert_eq!(Unit::Times.format(1.5), "1.50\u{d7}");
        assert_eq!(Unit::Count.format(3.0), "3");
        assert_eq!(Unit::Plain(2).format(0.5), "0.50");
    }

    #[test]
    fn each_section_has_rows_and_the_taper_tabs_edit_different_tapers() {
        for s in Section::ALL {
            assert!(!controls(s, false).is_empty(), "{s:?} is empty");
            assert!(!s.label().is_empty() && !s.blurb().is_empty());
        }
        let first = |finger: bool| {
            controls(Section::Taper, finger)
                .into_iter()
                .find_map(|c| match c {
                    Control::Slider(s) => Some(s),
                    _ => None,
                })
                .unwrap()
        };
        let mut b = Brush::default();
        (first(true).set)(&mut b, 0.9);
        assert_eq!(b.taper.finger.start_length, 0.9);
        assert_eq!(b.taper.pencil.start_length, Brush::default().taper.pencil.start_length);
    }
}
