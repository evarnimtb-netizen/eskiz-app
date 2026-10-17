//! Layer blend modes and blended flattening.
//!
//! The app-level blend index (`extendedBlend2` in `.procreate` archives, see
//! the file format notes) maps to [`BlendMode`] through
//! [`BlendMode::from_index`]. The per-channel functions are the standard separable
//! and non-separable definitions. Formulas, the Pegtop Soft Light and the luminance weights
//! follow the W3C compositing definitions. Only the names and
//! index mapping come from the file format notes.
//!
//! Compositing uses an opaque backdrop, because [`flatten`] starts from white
//! paper and every step stays opaque:
//! `C = (1 - as) * Cb + as * B(Cb, Cs)`, where `as` is the source alpha after
//! layer opacity and `Cs` is its unpremultiplied colour.

use crate::canvas::Canvas;
use crate::compose::{premul_from_u8, to_u8};
use crate::geometry::clamp01;

/// A layer blend mode. `Normal` is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Add,
    Lighten,
    Exclusion,
    Difference,
    Subtract,
    LinearBurn,
    ColorDodge,
    ColorBurn,
    Overlay,
    HardLight,
    Color,
    Luminosity,
    Hue,
    Saturation,
    SoftLight,
    Darken,
    HardMix,
    VividLight,
    LinearLight,
    PinLight,
    LighterColor,
    DarkerColor,
    Divide,
}

impl BlendMode {
    /// Every user-facing mode, in index order.
    pub const ALL: [BlendMode; 26] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Add,
        BlendMode::Lighten,
        BlendMode::Exclusion,
        BlendMode::Difference,
        BlendMode::Subtract,
        BlendMode::LinearBurn,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::Overlay,
        BlendMode::HardLight,
        BlendMode::Color,
        BlendMode::Luminosity,
        BlendMode::Hue,
        BlendMode::Saturation,
        BlendMode::SoftLight,
        BlendMode::Darken,
        BlendMode::HardMix,
        BlendMode::VividLight,
        BlendMode::LinearLight,
        BlendMode::PinLight,
        BlendMode::LighterColor,
        BlendMode::DarkerColor,
        BlendMode::Divide,
    ];

    /// Map an app blend index to a mode. Index 34 is an alias of Darken. Index 18,
    /// the internal indices 27 to 33, and anything else return `None`.
    pub fn from_index(index: i64) -> Option<Self> {
        use BlendMode::*;
        Some(match index {
            0 => Normal,
            1 => Multiply,
            2 => Screen,
            3 => Add,
            4 => Lighten,
            5 => Exclusion,
            6 => Difference,
            7 => Subtract,
            8 => LinearBurn,
            9 => ColorDodge,
            10 => ColorBurn,
            11 => Overlay,
            12 => HardLight,
            13 => Color,
            14 => Luminosity,
            15 => Hue,
            16 => Saturation,
            17 => SoftLight,
            19 | 34 => Darken,
            20 => HardMix,
            21 => VividLight,
            22 => LinearLight,
            23 => PinLight,
            24 => LighterColor,
            25 => DarkerColor,
            26 => Divide,
            _ => return None,
        })
    }

    /// The user-facing blend index written to `.procreate` files (`extendedBlend2`): the
    /// inverse of [`BlendMode::from_index`] (Darken is 19). Differs from the position in
    /// [`BlendMode::ALL`], which the GPU shaders use.
    pub fn to_index(self) -> i64 {
        use BlendMode::*;
        match self {
            Normal => 0,
            Multiply => 1,
            Screen => 2,
            Add => 3,
            Lighten => 4,
            Exclusion => 5,
            Difference => 6,
            Subtract => 7,
            LinearBurn => 8,
            ColorDodge => 9,
            ColorBurn => 10,
            Overlay => 11,
            HardLight => 12,
            Color => 13,
            Luminosity => 14,
            Hue => 15,
            Saturation => 16,
            SoftLight => 17,
            Darken => 19,
            HardMix => 20,
            VividLight => 21,
            LinearLight => 22,
            PinLight => 23,
            LighterColor => 24,
            DarkerColor => 25,
            Divide => 26,
        }
    }

    /// Human-readable name.
    pub fn name(self) -> &'static str {
        use BlendMode::*;
        match self {
            Normal => "Normal",
            Multiply => "Multiply",
            Screen => "Screen",
            Add => "Add",
            Lighten => "Lighten",
            Exclusion => "Exclusion",
            Difference => "Difference",
            Subtract => "Subtract",
            LinearBurn => "Linear Burn",
            ColorDodge => "Color Dodge",
            ColorBurn => "Color Burn",
            Overlay => "Overlay",
            HardLight => "Hard Light",
            Color => "Color",
            Luminosity => "Luminosity",
            Hue => "Hue",
            Saturation => "Saturation",
            SoftLight => "Soft Light",
            Darken => "Darken",
            HardMix => "Hard Mix",
            VividLight => "Vivid Light",
            LinearLight => "Linear Light",
            PinLight => "Pin Light",
            LighterColor => "Lighter Color",
            DarkerColor => "Darker Color",
            Divide => "Divide",
        }
    }

    /// True for the per-channel (separable) modes.
    pub fn is_separable(self) -> bool {
        !matches!(
            self,
            BlendMode::Color | BlendMode::Luminosity | BlendMode::Hue | BlendMode::Saturation
                | BlendMode::LighterColor | BlendMode::DarkerColor
        )
    }
}

// ---------------------------------------------------------------------------
// Separable formulas: B(cb, cs) per channel, values in 0..1.
// ---------------------------------------------------------------------------

fn multiply(cb: f64, cs: f64) -> f64 {
    cb * cs
}

fn screen(cb: f64, cs: f64) -> f64 {
    cb + cs - cb * cs
}

fn hard_light(cb: f64, cs: f64) -> f64 {
    if cs <= 0.5 {
        multiply(cb, 2.0 * cs)
    } else {
        screen(cb, 2.0 * cs - 1.0)
    }
}

/// Zero-divisor floor (2^-14). Using it instead of special-casing keeps the
/// dodge, burn and divide results continuous, so f32 and f64 evaluation agree.
const DIV_FLOOR: f64 = 1.0 / 16384.0;

fn color_dodge(cb: f64, cs: f64) -> f64 {
    (cb / (1.0 - cs).max(DIV_FLOOR)).min(1.0)
}

fn color_burn(cb: f64, cs: f64) -> f64 {
    1.0 - ((1.0 - cb) / cs.max(DIV_FLOOR)).min(1.0)
}

/// Soft Light, Pegtop form. This is
/// deliberately not the W3C formula.
fn soft_light(cb: f64, cs: f64) -> f64 {
    if cs <= 0.5 {
        cb * cb * (1.0 - 2.0 * cs) + 2.0 * cb * cs
    } else {
        2.0 * cb * (1.0 - cs) + cb.sqrt() * (2.0 * cs - 1.0)
    }
}

/// Separable blend of one channel. `None` for non-separable modes.
fn separable(mode: BlendMode, cb: f64, cs: f64) -> f64 {
    use BlendMode::*;
    match mode {
        Normal => cs,
        Multiply => multiply(cb, cs),
        Screen => screen(cb, cs),
        Add => (cb + cs).min(1.0),
        Lighten => cb.max(cs),
        Exclusion => cb + cs - 2.0 * cb * cs,
        Difference => (cb - cs).abs(),
        Subtract => (cb - cs).max(0.0),
        LinearBurn => (cb + cs - 1.0).max(0.0),
        ColorDodge => color_dodge(cb, cs),
        ColorBurn => color_burn(cb, cs),
        Overlay => hard_light(cs, cb),
        HardLight => hard_light(cb, cs),
        SoftLight => soft_light(cb, cs),
        Darken => cb.min(cs),
        HardMix => {
            if cb + cs >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        VividLight => {
            if cs <= 0.5 {
                color_burn(cb, 2.0 * cs)
            } else {
                color_dodge(cb, 2.0 * cs - 1.0)
            }
        }
        LinearLight => (cb + 2.0 * cs - 1.0).clamp(0.0, 1.0),
        PinLight => {
            if cs <= 0.5 {
                cb.min(2.0 * cs)
            } else {
                cb.max(2.0 * cs - 1.0)
            }
        }
        Divide => (cb / cs.max(DIV_FLOOR)).min(1.0),
        Color | Luminosity | Hue | Saturation | LighterColor | DarkerColor => cs,
    }
}

// ---------------------------------------------------------------------------
// Non-separable formulas (W3C: Lum, ClipColor, SetLum, Sat, SetSat).
// ---------------------------------------------------------------------------

/// Luminance for Color, Luminosity, Hue, Saturation and ClipColor. These are the
/// reference weights, not the W3C 0.30 / 0.59 / 0.11.
fn lum(c: [f64; 3]) -> f64 {
    0.30005 * c[0] + 0.58984 * c[1] + 0.10998 * c[2]
}

/// Luma for Lighter Color and Darker Color only.
fn luma(c: [f64; 3]) -> f64 {
    0.29907 * c[0] + 0.58691 * c[1] + 0.11401 * c[2]
}

fn sat(c: [f64; 3]) -> f64 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn clip_color(c: [f64; 3]) -> [f64; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut out = c;
    if n < 0.0 && l - n > 1e-12 {
        for v in &mut out {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 && x - l > 1e-12 {
        for v in &mut out {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    out
}

fn set_lum(c: [f64; 3], l: f64) -> [f64; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn set_sat(c: [f64; 3], s: f64) -> [f64; 3] {
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&a, &b| c[a].total_cmp(&c[b]));
    let (imin, imid, imax) = (idx[0], idx[1], idx[2]);
    let mut out = [0.0; 3];
    if c[imax] > c[imin] {
        out[imid] = (c[imid] - c[imin]) * s / (c[imax] - c[imin]);
        out[imax] = s;
    }
    out
}

/// Non-separable and selection modes on whole RGB triples.
fn non_separable(mode: BlendMode, cb: [f64; 3], cs: [f64; 3]) -> [f64; 3] {
    match mode {
        BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        BlendMode::Color => set_lum(cs, lum(cb)),
        BlendMode::Luminosity => set_lum(cb, lum(cs)),
        // Darker / Lighter Color keep whichever whole colour has the lower / higher luma.
        // On an exact tie the source colour wins.
        BlendMode::DarkerColor => {
            if luma(cs) <= luma(cb) {
                cs
            } else {
                cb
            }
        }
        BlendMode::LighterColor => {
            if luma(cs) >= luma(cb) {
                cs
            } else {
                cb
            }
        }
        _ => {
            let mut out = [0.0; 3];
            for c in 0..3 {
                out[c] = separable(mode, cb[c], cs[c]);
            }
            out
        }
    }
}

/// The blend function B(Cb, Cs) for straight colours in 0..1 (no alpha).
/// Inputs are clamped to 0..1; NaN counts as 0.
pub fn blend_colour(mode: BlendMode, cb: [f64; 3], cs: [f64; 3]) -> [f64; 3] {
    let cb = cb.map(clamp01);
    let cs = cs.map(clamp01);
    if mode == BlendMode::Normal {
        cs
    } else if mode.is_separable() {
        let mut out = [0.0; 3];
        for c in 0..3 {
            out[c] = separable(mode, cb[c], cs[c]);
        }
        out
    } else {
        non_separable(mode, cb, cs)
    }
}

/// Blend one source pixel over an opaque backdrop.
///
/// `src` is premultiplied RGBA in 0..1 with layer opacity already applied.
/// Returns the new opaque RGB.
pub fn blend_pixel(backdrop: [f64; 3], src: [f64; 4], mode: BlendMode) -> [f64; 3] {
    let a = clamp01(src[3]);
    if a <= 0.0 {
        return backdrop;
    }
    let cs = [
        clamp01(src[0] / a),
        clamp01(src[1] / a),
        clamp01(src[2] / a),
    ];
    let cb = backdrop.map(clamp01);
    let b = blend_colour(mode, cb, cs);
    let mut out = [0.0; 3];
    for c in 0..3 {
        out[c] = (1.0 - a) * cb[c] + a * b[c];
    }
    out
}

/// Flatten a rectangle of the canvas over white paper, using `modes[i]` for layer `i`
/// (missing entries mean Normal). Hidden layers are skipped. Returns RGBA8 for
/// `w * h` pixels, row-major, with alpha 255.
///
/// `(x0, y0)` is the top-left corner. The rectangle is clipped to the canvas.
pub fn flatten_rect(
    canvas: &Canvas,
    modes: &[BlendMode],
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
) -> Vec<u8> {
    let x1 = (x0 + w).min(canvas.width);
    let y1 = (y0 + h).min(canvas.height);
    let x0 = x0.min(x1);
    let y0 = y0.min(y1);
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        for x in x0..x1 {
            let mut acc = [1.0f64; 3];
            for (li, layer) in canvas.layers.iter().enumerate() {
                if !layer.visible {
                    continue;
                }
                let px = premul_from_u8(&layer.pixel(x, y));
                let op = clamp01(layer.opacity);
                let src = [px[0] * op, px[1] * op, px[2] * op, px[3] * op];
                let mode = modes.get(li).copied().unwrap_or_default();
                acc = blend_pixel(acc, src, mode);
            }
            out.extend(acc.iter().map(|&v| to_u8(v)));
            out.push(255);
        }
    }
    out
}

/// Flatten the whole canvas. See [`flatten_rect`].
pub fn flatten(canvas: &Canvas, modes: &[BlendMode]) -> Vec<u8> {
    flatten_rect(canvas, modes, 0, 0, canvas.width, canvas.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose;

    const EPS: f64 = 1e-9;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn index_mapping_matches_spec_table() {
        assert_eq!(BlendMode::from_index(0), Some(BlendMode::Normal));
        assert_eq!(BlendMode::from_index(10), Some(BlendMode::ColorBurn));
        assert_eq!(BlendMode::from_index(19), Some(BlendMode::Darken));
        assert_eq!(BlendMode::from_index(34), Some(BlendMode::Darken));
        assert_eq!(BlendMode::from_index(26), Some(BlendMode::Divide));
        assert_eq!(BlendMode::from_index(18), None);
        assert_eq!(BlendMode::from_index(27), None);
        assert_eq!(BlendMode::from_index(35), None);
        assert_eq!(BlendMode::from_index(-1), None);
        for mode in BlendMode::ALL {
            assert!(!mode.name().is_empty());
            assert_eq!(BlendMode::from_index(mode.to_index()), Some(mode));
        }
    }

    #[test]
    fn separable_basics() {
        assert!(close(multiply(0.5, 0.5), 0.25));
        assert!(close(screen(0.5, 0.5), 0.75));
        assert!(close(separable(BlendMode::Add, 0.6, 0.7), 1.0));
        assert!(close(separable(BlendMode::Subtract, 0.2, 0.7), 0.0));
        assert!(close(separable(BlendMode::Difference, 0.2, 0.7), 0.5));
        assert!(close(separable(BlendMode::Exclusion, 0.5, 0.5), 0.5));
        assert!(close(separable(BlendMode::LinearBurn, 0.6, 0.7), 0.3));
        assert!(close(separable(BlendMode::Darken, 0.3, 0.8), 0.3));
        assert!(close(separable(BlendMode::Lighten, 0.3, 0.8), 0.8));
    }

    #[test]
    fn dodge_and_burn_edge_cases() {
        assert!(close(color_dodge(0.0, 0.7), 0.0));
        assert!(close(color_dodge(0.5, 1.0), 1.0));
        assert!(close(color_dodge(0.25, 0.5), 0.5));
        assert!(close(color_burn(1.0, 0.2), 1.0));
        assert!(close(color_burn(0.5, 0.0), 0.0));
        assert!(close(color_burn(0.5, 0.5), 0.0));
    }

    #[test]
    fn overlay_and_hard_light_swap_roles() {
        // Overlay(cb, cs) = HardLight(cs, cb).
        assert!(close(separable(BlendMode::Overlay, 0.25, 0.5), 0.25));
        assert!(close(separable(BlendMode::Overlay, 0.75, 0.5), 0.75));
        assert!(close(separable(BlendMode::HardLight, 0.5, 0.25), 0.25));
        assert!(close(separable(BlendMode::HardLight, 0.5, 0.75), 0.75));
    }

    #[test]
    fn soft_light_formula() {
        // Pegtop: cs <= 0.5 gives cb^2 (1 - 2cs) + 2 cb cs.
        assert!(close(soft_light(0.5, 0.25), 0.375));
        // cs > 0.5 gives 2 cb (1 - cs) + sqrt(cb) (2cs - 1).
        assert!(close(soft_light(0.25, 0.75), 0.375));
        // The forms differ only for cb < 0.25 with cs > 0.5: Pegtop gives
        // sqrt(0.1) here, W3C's polynomial branch would give 0.296.
        assert!(close(soft_light(0.1, 1.0), 0.1f64.sqrt()));
        // Pegtop form.
        let expect = 2.0 * 0.5 * 0.25 + 0.5f64.sqrt() * 0.5;
        assert!(close(soft_light(0.5, 0.75), expect));
    }

    #[test]
    fn vivid_linear_and_pin_light() {
        assert!(close(separable(BlendMode::LinearLight, 0.5, 0.75), 1.0));
        assert!(close(separable(BlendMode::LinearLight, 0.2, 0.25), 0.0));
        assert!(close(separable(BlendMode::LinearLight, 0.2, 0.75), 0.7));
        assert!(close(separable(BlendMode::PinLight, 0.6, 0.25), 0.5));
        assert!(close(separable(BlendMode::PinLight, 0.3, 0.75), 0.5));
        assert!(close(separable(BlendMode::HardMix, 0.6, 0.5), 1.0));
        assert!(close(separable(BlendMode::HardMix, 0.2, 0.5), 0.0));
        assert!(close(separable(BlendMode::Divide, 0.25, 0.5), 0.5));
        assert!(close(separable(BlendMode::Divide, 0.5, 0.0), 1.0));
        assert!(close(separable(BlendMode::Divide, 0.0, 0.0), 0.0));
    }

    #[test]
    fn luminosity_and_color_preserve_lum_of_backdrop_or_source() {
        let grey = [0.5, 0.5, 0.5];
        let red = [1.0, 0.0, 0.0];
        // Luminosity takes the source luminance (0.30005 for pure red) on the backdrop.
        let l = non_separable(BlendMode::Luminosity, grey, red);
        // The reference weights sum to 0.99987, so the match is within 2e-4, not exact.
        assert!((lum(l) - 0.30005).abs() < 2e-4);
        // Color keeps the backdrop luminance (0.5) with the source hue.
        let col = non_separable(BlendMode::Color, grey, red);
        assert!((lum(col) - 0.5).abs() < 2e-4);
    }

    #[test]
    fn hue_and_saturation_on_grey_backdrop() {
        let grey = [0.4, 0.4, 0.4];
        let src = [0.9, 0.2, 0.1];
        // A grey backdrop has no saturation, so Hue and Saturation both give grey (within the
        // 1.3e-4 by which the standard luminance weights miss summing to 1).
        let h = non_separable(BlendMode::Hue, grey, src);
        let s = non_separable(BlendMode::Saturation, grey, src);
        for c in h.iter().chain(s.iter()) {
            assert!((*c - 0.4).abs() < 2e-4, "{c}");
        }
        assert!((h[0] - h[1]).abs() < 1e-12 && (h[1] - h[2]).abs() < 1e-12);
    }

    #[test]
    fn darker_and_lighter_colour_pick_by_luminance() {
        let dark = [0.1, 0.1, 0.1];
        let light = [0.9, 0.9, 0.9];
        assert_eq!(non_separable(BlendMode::DarkerColor, light, dark), dark);
        assert_eq!(non_separable(BlendMode::LighterColor, dark, light), light);
        assert_eq!(non_separable(BlendMode::DarkerColor, dark, light), dark);
    }

    #[test]
    fn blend_pixel_normal_is_source_over_on_opaque_backdrop() {
        // 50% black over white: 0.5 grey.
        let out = blend_pixel([1.0; 3], [0.0, 0.0, 0.0, 0.5], BlendMode::Normal);
        for c in out {
            assert!(close(c, 0.5));
        }
        // Zero alpha leaves the backdrop unchanged.
        assert_eq!(blend_pixel([0.2, 0.3, 0.4], [0.0; 4], BlendMode::Multiply), [0.2, 0.3, 0.4]);
    }

    fn test_canvas() -> Canvas {
        let mut c = Canvas::new(5, 4);
        for k in 0..3 {
            let i = c.add_layer();
            let layer = &mut c.layers[i];
            layer.opacity = [1.0, 0.6, 0.35][k];
            for p in 0..(5 * 4) {
                // Varied premultiplied pixels, with some transparent ones.
                let a = if (p + k) % 4 == 0 { 0 } else { ((p * 37 + k * 91) % 256) as u8 };
                let r = ((p * 13 + k * 5) % 256) as u8;
                let g = ((p * 7 + k * 29) % 256) as u8;
                let b = ((p * 3 + k * 53) % 256) as u8;
                let pre = |v: u8| ((u16::from(v) * u16::from(a)) / 255) as u8;
                layer.set_pixel(p % 5, p / 5, [pre(r), pre(g), pre(b), a]);
            }
        }
        c.layers[1].visible = false;
        c
    }

    #[test]
    fn normal_flatten_matches_compose_flatten() {
        let c = test_canvas();
        let modes = vec![BlendMode::Normal; c.layers.len()];
        let ours = flatten(&c, &modes);
        let theirs = compose::flatten(&c);
        assert_eq!(ours.len(), theirs.len());
        for (a, b) in ours.iter().zip(&theirs) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 1, "{a} vs {b}");
        }
    }

    #[test]
    fn rect_flatten_is_a_crop_of_the_full_flatten() {
        let c = test_canvas();
        let modes = [BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay];
        let full = flatten(&c, &modes);
        let rect = flatten_rect(&c, &modes, 1, 1, 3, 2);
        let mut expect = Vec::new();
        for y in 1..3 {
            for x in 1..4 {
                let i = (y * 5 + x) * 4;
                expect.extend_from_slice(&full[i..i + 4]);
            }
        }
        assert_eq!(rect, expect);
    }

    #[test]
    fn multiply_layer_darkens_white_paper() {
        let mut c = Canvas::new(1, 1);
        let i = c.add_layer();
        c.layers[i].set_pixel(0, 0, [128, 128, 128, 255]);
        let out = flatten(&c, &[BlendMode::Multiply]);
        assert_eq!(out[0], 128);
        assert_eq!(out[3], 255);
        // Normal with the same layer gives the same answer on white paper.
        assert_eq!(flatten(&c, &[BlendMode::Normal])[0], 128);
        // Multiply over a dark backdrop stays dark.
        let mut d = Canvas::new(1, 1);
        let a = d.add_layer();
        d.layers[a].set_pixel(0, 0, [64, 64, 64, 255]);
        let b = d.add_layer();
        d.layers[b].set_pixel(0, 0, [128, 128, 128, 255]);
        let out = flatten(&d, &[BlendMode::Normal, BlendMode::Multiply]);
        assert!((i32::from(out[0]) - 32).abs() <= 1);
    }

    #[test]
    fn hidden_layers_and_zero_opacity_are_skipped() {
        let mut c = Canvas::new(1, 1);
        let i = c.add_layer();
        c.layers[i].set_pixel(0, 0, [0, 0, 0, 255]);
        c.layers[i].visible = false;
        assert_eq!(flatten(&c, &[BlendMode::Normal]), vec![255, 255, 255, 255]);
        c.layers[i].visible = true;
        c.layers[i].opacity = 0.0;
        assert_eq!(flatten(&c, &[BlendMode::Multiply]), vec![255, 255, 255, 255]);
    }

    #[test]
    fn missing_modes_default_to_normal() {
        let c = test_canvas();
        let ours = flatten(&c, &[]);
        let theirs = compose::flatten(&c);
        for (a, b) in ours.iter().zip(&theirs) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 1);
        }
    }
}
