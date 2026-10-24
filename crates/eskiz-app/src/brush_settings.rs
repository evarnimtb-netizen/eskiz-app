//! Saves every setting of an eskiz-core [`Brush`] in a `.brush` bundle, and reads them back.
//!
//! A `.brush` file is a keyed archive of named settings (see `eskiz_format::brushes`). The format
//! crate maps only a handful of them to the engine. Eskiz keeps the rest, so that a brush edited
//! in the Brush Bench survives a restart exactly:
//!
//! * the settings the file format notes names (`maxSize`, `minSize`, `plotSpacing`, `plotSmoothing`,
//!   `shapeRotation`, `shapeRoundness` and the two tapers) are written under their own keys, so
//!   another reader of the format still gets a sensible brush;
//! * every Brush field is also written under `eskiz.<field>`, which is what Eskiz reads back. A
//!   key that is missing keeps the value the reader already had, and a key this version does not
//!   know is ignored, so files from an older or newer Eskiz still load;
//! * `eskiz.tipKind` and `eskiz.grainKind` name the generator of the tip and grain images, and
//!   `eskiz.grayPaint` says the PNGs are Eskiz's own grey masks (white is paint), so they are not
//!   run through the "dark shape on white paper" guess that foreign tips need.
//!
//! The nine response curves that can be edited are stored as `SettingValue::Curve`.

use std::collections::BTreeMap;

use eskiz_core::brushes::{Accumulation, Brush, Grain, GrainMode, Points, Shape, Taper, Wet};
use eskiz_format::brushes::{keys, ImportedBrush, SettingValue};

/// Prefix of the keys Eskiz adds to a brush file.
pub const PREFIX: &str = "eskiz.";
/// Prefix written by releases before the rename. Read and converted to [`PREFIX`], never written.
pub const LEGACY_PREFIX: &str = "easel."; // legacy-easel
/// Generator of the tip image (see `eskiz_brushlib::tips::TipKind::key`).
pub const TIP_KIND: &str = "eskiz.tipKind";
/// Generator of the grain image (see `eskiz_brushlib::grains::GrainKind::key`).
pub const GRAIN_KIND: &str = "eskiz.grainKind";
/// True when `Shape.png` and `Grain.png` are Eskiz grey masks and must be decoded as they are.
pub const GRAY_PAINT: &str = "eskiz.grayPaint";
/// Version of the `eskiz.*` keys.
pub const VERSION: &str = "eskiz.version";

/// A brush read from a file written by an older release: every `eskiz.*` setting is renamed to
/// `eskiz.*`. A key already present under the new name wins.
pub fn upgrade_legacy_keys(ib: &mut ImportedBrush) {
    let old: Vec<String> = ib.settings.keys().filter(|k| k.starts_with(LEGACY_PREFIX)).cloned().collect();
    for k in old {
        if let Some(v) = ib.settings.remove(&k) {
            let new = format!("{PREFIX}{}", &k[LEGACY_PREFIX.len()..]);
            ib.settings.entry(new).or_insert(v);
        }
    }
}

/// A numeric or boolean setting, borrowed so one list serves reading and writing.
enum Field<'a> {
    F(&'a mut f64),
    B(&'a mut bool),
}

/// Every numeric and boolean setting with its key (without the prefix). The destructuring has no
/// `..`, so adding a field to [`Brush`] stops this compiling until it is listed here.
fn fields(b: &mut Brush) -> Vec<(String, Field<'_>)> {
    let Brush {
        name: _,
        shape: _,
        invert_shape,
        grain: _,
        grain_mode: _,
        grain_scale,
        grain_depth,
        grain_contrast,
        grain_brightness,
        invert_grain,
        size,
        opacity,
        flow,
        accumulation: _,
        wet_edges,
        burnt_edges,
        alpha_threshold,
        spacing,
        spacing_jitter,
        spacing_speed,
        smoothing,
        pressure_smoothing,
        stabilization,
        motion_amount,
        motion_expression,
        pressure_size,
        speed_size,
        min_size,
        max_size,
        pressure_roundness,
        pressure_roundness_min,
        shape_roundness,
        pressure_opacity,
        speed_opacity,
        pressure_softening,
        dynamics_blur,
        bleed,
        bleed_pressure,
        tilt_size,
        tilt_opacity,
        tilt_roundness,
        tilt_rotation,
        roll_rotation,
        taper,
        random_rotation,
        shape_rotation,
        follow_stroke,
        rotation_jitter,
        scatter,
        size_jitter,
        opacity_jitter,
        stamp_count,
        stamp_count_jitter,
        hue_jitter,
        saturation_jitter,
        lightness_jitter,
        darkness_jitter,
        wet,
        curves: _,
    } = b;
    let mut v: Vec<(String, Field<'_>)> = Vec::new();
    macro_rules! f {
        ($($n:ident),* $(,)?) => { $( v.push((stringify!($n).to_owned(), Field::F($n))); )* };
    }
    macro_rules! t {
        ($($n:ident),* $(,)?) => { $( v.push((stringify!($n).to_owned(), Field::B($n))); )* };
    }
    f!(grain_scale, grain_depth, grain_contrast, grain_brightness);
    f!(size, opacity, flow, wet_edges, burnt_edges, alpha_threshold);
    f!(spacing, spacing_jitter, spacing_speed);
    f!(smoothing, pressure_smoothing, stabilization, motion_amount, motion_expression);
    f!(pressure_size, speed_size, min_size, max_size);
    f!(pressure_roundness, pressure_roundness_min, shape_roundness);
    f!(pressure_opacity, speed_opacity, dynamics_blur, bleed, bleed_pressure);
    f!(tilt_size, tilt_opacity, tilt_roundness);
    f!(shape_rotation, rotation_jitter, scatter, size_jitter, opacity_jitter);
    f!(stamp_count, stamp_count_jitter);
    f!(hue_jitter, saturation_jitter, lightness_jitter, darkness_jitter);
    t!(invert_shape, invert_grain, pressure_softening, tilt_rotation, roll_rotation);
    t!(random_rotation, follow_stroke);
    taper_fields("taper.pencil", &mut taper.pencil, &mut v);
    taper_fields("taper.finger", &mut taper.finger, &mut v);
    let Wet { enabled, attack, falloff_amount, load_amount, dilution, tilt_angle, pull, grade, wetness, bleed } = wet;
    v.push(("wet.enabled".to_owned(), Field::B(enabled)));
    v.push(("wet.attack".to_owned(), Field::F(attack)));
    v.push(("wet.falloff_amount".to_owned(), Field::F(falloff_amount)));
    v.push(("wet.load_amount".to_owned(), Field::F(load_amount)));
    v.push(("wet.dilution".to_owned(), Field::F(dilution)));
    v.push(("wet.tilt_angle".to_owned(), Field::F(tilt_angle)));
    v.push(("wet.pull".to_owned(), Field::F(pull)));
    v.push(("wet.grade".to_owned(), Field::F(grade)));
    v.push(("wet.wetness".to_owned(), Field::F(wetness)));
    v.push(("wet.bleed".to_owned(), Field::F(bleed)));
    v
}

fn taper_fields<'a>(prefix: &str, t: &'a mut Taper, out: &mut Vec<(String, Field<'a>)>) {
    let Taper { start_length, end_length, size, opacity, shape, pressure, tip_animation } = t;
    out.push((format!("{prefix}.start_length"), Field::F(start_length)));
    out.push((format!("{prefix}.end_length"), Field::F(end_length)));
    out.push((format!("{prefix}.size"), Field::F(size)));
    out.push((format!("{prefix}.opacity"), Field::F(opacity)));
    out.push((format!("{prefix}.shape"), Field::F(shape)));
    out.push((format!("{prefix}.pressure"), Field::F(pressure)));
    out.push((format!("{prefix}.tip_animation"), Field::B(tip_animation)));
}

/// Every response curve with its key (without the prefix and `curve.`).
fn curves(b: &mut Brush) -> Vec<(&'static str, &mut Points)> {
    let c = &mut b.curves;
    vec![
        ("overall", &mut c.overall),
        ("opacity", &mut c.opacity),
        ("size", &mut c.size),
        ("roundness", &mut c.roundness),
        ("bleed", &mut c.bleed),
        ("blur", &mut c.blur),
        ("spacing_jitter", &mut c.spacing_jitter),
        ("stamp_count", &mut c.stamp_count),
        ("stamp_count_jitter", &mut c.stamp_count_jitter),
        ("attack", &mut c.attack),
        ("load", &mut c.load),
        ("dilution", &mut c.dilution),
    ]
}

fn shape_key(s: Shape) -> &'static str {
    match s {
        Shape::SoftRound => "soft_round",
        Shape::HardRound => "hard_round",
    }
}

fn grain_key(g: Grain) -> &'static str {
    match g {
        Grain::None => "none",
        Grain::Noise => "noise",
        Grain::Paper => "paper",
    }
}

fn grain_mode_key(m: GrainMode) -> &'static str {
    match m {
        GrainMode::Texturized => "texturized",
        GrainMode::Moving => "moving",
    }
}

fn accumulation_key(a: Accumulation) -> &'static str {
    match a {
        Accumulation::Glaze => "glaze",
        Accumulation::Buildup => "buildup",
    }
}

fn key(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// All settings of `brush` as `.brush` settings: the interop keys and the `eskiz.*` keys.
pub fn to_settings(brush: &Brush) -> BTreeMap<String, SettingValue> {
    use SettingValue::{Bool, Curve, Number, Text};
    let mut b = brush.clone();
    let mut out = BTreeMap::new();
    out.insert(VERSION.to_owned(), Number(1.0));
    out.insert(key("shape"), Text(shape_key(b.shape).to_owned()));
    out.insert(key("grain"), Text(grain_key(b.grain).to_owned()));
    out.insert(key("grain_mode"), Text(grain_mode_key(b.grain_mode).to_owned()));
    out.insert(key("accumulation"), Text(accumulation_key(b.accumulation).to_owned()));
    for (name, field) in fields(&mut b) {
        out.insert(
            key(&name),
            match field {
                Field::F(v) => Number(*v),
                Field::B(v) => Bool(*v),
            },
        );
    }
    for (name, pts) in curves(&mut b) {
        out.insert(key(&format!("curve.{name}")), Curve(pts.clone()));
    }

    // The settings the interop format names.
    let k = |s: &str, v: f64| (s.to_owned(), Number(v));
    let tp = |t: &Taper, names: [&str; 5]| {
        [
            k(names[0], t.start_length),
            k(names[1], t.end_length),
            k(names[2], t.size),
            k(names[3], t.opacity),
            k(names[4], t.shape),
        ]
    };
    out.extend([
        k(keys::MAX_SIZE, brush.size),
        k(keys::MIN_SIZE, brush.min_size * brush.size),
        k(keys::PLOT_SPACING, brush.spacing),
        k(keys::PLOT_SMOOTHING, brush.smoothing),
        k(keys::SHAPE_ROTATION, brush.shape_rotation),
        k(keys::SHAPE_ROUNDNESS, brush.shape_roundness),
        k(keys::TAPER_PRESSURE, brush.taper.finger.pressure),
    ]);
    out.extend(tp(
        &brush.taper.finger,
        [keys::TAPER_START_LENGTH, keys::TAPER_END_LENGTH, keys::TAPER_SIZE, keys::TAPER_OPACITY, keys::TAPER_SHAPE],
    ));
    out.extend(tp(
        &brush.taper.pencil,
        [
            keys::PENCIL_TAPER_START_LENGTH,
            keys::PENCIL_TAPER_END_LENGTH,
            keys::PENCIL_TAPER_SIZE,
            keys::PENCIL_TAPER_OPACITY,
            keys::PENCIL_TAPER_SHAPE,
        ],
    ));
    out
}

/// Overlays the `eskiz.*` settings of `ib` on `brush`. Keys the file lacks leave `brush` as it
/// was; a value of the wrong type or a non-finite number is ignored.
/// The range of each setting's Studio control. Imported values are clamped to it: a file could
/// ask for a stabiliser that takes seconds per sample, a spacing that never advances or a grain
/// scale that overflows the texture lookup.
const RANGES: &[(&str, f64, f64)] = &[
    ("size", 0.1, crate::app::MAX_SIZE as f64),
    ("opacity", 0.0, 1.0),
    ("flow", 0.0, 1.0),
    ("wet_edges", 0.0, 1.0),
    ("burnt_edges", 0.0, 1.0),
    ("alpha_threshold", 0.0, 1.0),
    ("grain_scale", 0.1, 4.0),
    ("grain_depth", 0.0, 1.0),
    ("grain_contrast", -1.0, 2.0),
    ("grain_brightness", -1.0, 1.0),
    ("spacing", 0.0, 1.0),
    ("spacing_jitter", 0.0, 1.0),
    ("spacing_speed", 0.0, 1.0),
    ("scatter", 0.0, 3.0),
    ("stamp_count_jitter", 0.0, 1.0),
    ("smoothing", 0.0, 1.0),
    ("pressure_smoothing", 0.0, 1.0),
    ("stabilization", 0.0, 1.0),
    ("motion_amount", 0.0, 1.0),
    ("motion_expression", 0.0, 1.0),
    ("shape_rotation", -std::f64::consts::PI, std::f64::consts::PI),
    ("rotation_jitter", 0.0, 1.0),
    ("shape_roundness", 0.02, 1.0),
    ("pressure_roundness", 0.0, 1.0),
    ("pressure_roundness_min", 0.0, 1.0),
    ("pressure_size", -1.0, 1.0),
    ("pressure_opacity", -1.0, 1.0),
    ("speed_size", -1.0, 1.0),
    ("speed_opacity", -1.0, 1.0),
    ("size_jitter", 0.0, 1.0),
    ("opacity_jitter", 0.0, 1.0),
    ("tilt_size", -1.0, 1.0),
    ("tilt_opacity", -1.0, 1.0),
    ("tilt_roundness", 0.0, 1.0),
    ("min_size", 0.0, 1.0),
    ("max_size", 0.0, 2.0),
    ("hue_jitter", 0.0, 1.0),
    ("saturation_jitter", 0.0, 1.0),
    ("lightness_jitter", 0.0, 1.0),
    ("darkness_jitter", 0.0, 1.0),
    ("wet.attack", 0.0, 1.0),
    ("wet.falloff_amount", 0.0, 1.0),
    ("wet.load_amount", 0.0, 1.0),
    ("wet.dilution", 0.0, 1.0),
    ("wet.tilt_angle", 0.0, 1.0),
    ("wet.pull", 0.0, 1.0),
    ("wet.grade", 0.0, 1.0),
    ("wet.wetness", 0.0, 1.0),
    ("wet.bleed", 0.0, 1.0),
];

pub fn apply_settings(ib: &ImportedBrush, brush: &mut Brush) {
    let text = |name: &str| ib.text(&key(name)).map(str::to_owned);
    if let Some(v) = text("shape") {
        match v.as_str() {
            "soft_round" => brush.shape = Shape::SoftRound,
            "hard_round" => brush.shape = Shape::HardRound,
            _ => {}
        }
    }
    if let Some(v) = text("grain") {
        match v.as_str() {
            "none" => brush.grain = Grain::None,
            "noise" => brush.grain = Grain::Noise,
            "paper" => brush.grain = Grain::Paper,
            _ => {}
        }
    }
    if let Some(v) = text("grain_mode") {
        match v.as_str() {
            "texturized" => brush.grain_mode = GrainMode::Texturized,
            "moving" => brush.grain_mode = GrainMode::Moving,
            _ => {}
        }
    }
    if let Some(v) = text("accumulation") {
        match v.as_str() {
            "glaze" => brush.accumulation = Accumulation::Glaze,
            "buildup" => brush.accumulation = Accumulation::Buildup,
            _ => {}
        }
    }
    for (name, field) in fields(brush) {
        match field {
            Field::F(slot) => {
                if let Some(v) = ib.number(&key(&name)).filter(|v| v.is_finite()) {
                    *slot = match RANGES.iter().find(|(n, ..)| *n == name) {
                        Some(&(_, lo, hi)) => v.clamp(lo, hi),
                        None => v,
                    };
                }
            }
            Field::B(slot) => {
                if let Some(v) = ib.flag(&key(&name)) {
                    *slot = v;
                }
            }
        }
    }
    for (name, pts) in curves(brush) {
        if let Some(SettingValue::Curve(p)) = ib.settings.get(&key(&format!("curve.{name}"))) {
            // Two points at the same x would make the curve's spline divide by zero.
            let mut xs: Vec<f64> = p.iter().map(|q| q[0]).collect();
            xs.sort_by(f64::total_cmp);
            let distinct = xs.windows(2).all(|w| w[0] < w[1]);
            if p.len() >= 2 && distinct && p.iter().all(|q| q[0].is_finite() && q[1].is_finite()) {
                *pts = p.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eskiz_brushlib::library_ref;
    use eskiz_format::brushes::{read_brush, write_brush};

    #[test]
    fn every_library_brush_round_trips_through_a_brush_file() {
        for lb in library_ref().iter().flat_map(|s| &s.brushes) {
            let ib = ImportedBrush { name: lb.name.into(), settings: to_settings(&lb.brush), ..ImportedBrush::default() };
            let bytes = write_brush(&ib).unwrap();
            let back = read_brush("x", &bytes).unwrap();
            let mut got = Brush::default();
            apply_settings(&back, &mut got);
            got.name = lb.brush.name;
            assert_eq!(got, lb.brush, "{} changed in the file", lb.name);
        }
    }

    #[test]
    fn interop_keys_carry_the_main_settings() {
        let b = Brush { size: 40.0, min_size: 0.25, spacing: 0.3, smoothing: 0.7, ..Brush::default() };
        let s = to_settings(&b);
        assert_eq!(s.get(keys::MAX_SIZE), Some(&SettingValue::Number(40.0)));
        assert_eq!(s.get(keys::MIN_SIZE), Some(&SettingValue::Number(10.0)));
        assert_eq!(s.get(keys::PLOT_SPACING), Some(&SettingValue::Number(0.3)));
        let ib = ImportedBrush { settings: s, ..ImportedBrush::default() };
        let p = eskiz_format::brushes::engine_params(&ib);
        assert_eq!((p.size, p.min_size, p.spacing, p.smoothing), (40.0, 0.25, 0.3, 0.7));
    }

    #[test]
    fn a_missing_or_bad_key_keeps_the_value_already_there() {
        let mut ib = ImportedBrush::default();
        ib.settings.insert(key("spacing"), SettingValue::Number(f64::NAN));
        ib.settings.insert(key("size"), SettingValue::Text("big".into()));
        ib.settings.insert(key("curve.size"), SettingValue::Curve(vec![[0.0, 0.0]]));
        let mut b = Brush { spacing: 0.4, size: 33.0, ..Brush::default() };
        apply_settings(&ib, &mut b);
        assert_eq!((b.spacing, b.size), (0.4, 33.0));
        assert_eq!(b.curves.size, Brush::default().curves.size);
    }

    #[test]
    fn tilt_and_curves_survive() {
        let mut b = Brush { tilt_size: 0.6, tilt_rotation: true, roll_rotation: true, ..Brush::default() };
        b.curves.overall = vec![[0.0, 0.0], [0.4, 0.7], [1.0, 1.0]];
        let ib = ImportedBrush { settings: to_settings(&b), ..ImportedBrush::default() };
        let mut back = Brush::default();
        apply_settings(&ib, &mut back);
        assert_eq!(back, Brush { name: back.name, ..b });
    }
}
