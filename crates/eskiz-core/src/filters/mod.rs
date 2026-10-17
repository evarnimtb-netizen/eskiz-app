//! Adjustment filters.
//!
//! The framework is an [`Adjustment`]: a type from the list, its
//! parameter values (each clamped to its stated range on every write), optional typed data
//! (curve points, gradient stops) and the interaction mode (Layer or Pencil. Filters
//! are pure functions on premultiplied RGBA8 (`width * height * 4` bytes). Live preview and
//! commit share one code path, [`Adjustment::render`]; the only difference is the [`Quality`]
//! flag, which lets the preview of an expensive filter run on a downscaled proxy.
//!
//! Opacity, Gaussian blur and Hue/Saturation/Brightness are
//! implemented here. Every other type renders through [`bridge`], which calls the CPU reference
//! filters of `eskiz-filters`.

pub mod bridge;
pub mod data;
pub mod gaussian;
pub mod hsb;
pub mod opacity;
pub mod proxy;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::pixel::Chan;

pub use proxy::{proxy_factor, proxy_safe, render_preview, upscale_layer, upscale_layer_over, Proxy};
pub use data::{curve_table, CurvePoints, GradientStops, GRADIENT_PRESETS};

/// Every adjustment type, in the order of the Adjustments list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdjustKind {
    Opacity,
    GaussianBlur,
    MotionBlur,
    PerspectiveBlur,
    Sharpen,
    Noise,
    ChromaticAberration,
    Glitch,
    Halftone,
    Bloom,
    Hsb,
    ColourBalance,
    Curves,
    GradientMap,
    Recolor,
    Clone,
}

/// One parameter of an adjustment: its slider range and default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// Periodic parameters (hue) wrap instead of clamping.
    pub periodic: bool,
    /// Discrete options. The value is the index into this list. Empty for a slider.
    pub choices: &'static [&'static str],
    /// Stored and used, but not offered as a control for the current settings.
    pub hidden: bool,
}

/// Most parameters any one adjustment has (colour balance has nine).
pub const MAX_PARAMS: usize = 12;

const fn slider(name: &'static str, min: f64, max: f64, default: f64) -> ParamSpec {
    ParamSpec { name, min, max, default, periodic: false, choices: &[], hidden: false }
}

const fn choice(name: &'static str, choices: &'static [&'static str], default: f64) -> ParamSpec {
    ParamSpec { name, min: 0.0, max: (choices.len() - 1) as f64, default, periodic: false, choices, hidden: false }
}

const fn hidden(mut p: ParamSpec) -> ParamSpec {
    p.hidden = true;
    p
}

const OPACITY: &[ParamSpec] = &[slider("Amount", 0.0, 1.0, 1.0)];
const GAUSSIAN: &[ParamSpec] = &[slider("Amount", 0.0, 1.0, 0.6)];
const MOTION: &[ParamSpec] = &[
    slider("Vector X", -512.0, 512.0, 0.0),
    slider("Vector Y", -512.0, 512.0, 0.0),
    slider("Amount", 0.0, 1.0, 0.6),
];
const PERSPECTIVE: &[ParamSpec] = &[
    slider("Origin X", 0.0, 1.0, 0.5),
    slider("Origin Y", 0.0, 1.0, 0.5),
    slider("Direction X", -1.0, 1.0, 0.0),
    slider("Direction Y", -1.0, 1.0, 0.0),
    slider("Amount", 0.0, 1.0, 0.6),
];
const SHARPEN: &[ParamSpec] = &[slider("Amount", 0.0, 1.0, 0.0)];
const NOISE: &[ParamSpec] = &[
    slider("Scale", 0.0, 1.0, 0.0),
    slider("Amount", 0.0, 1.0, 0.23),
    slider("Octaves", 0.0, 1.0, 0.0),
    slider("Turbulence", 0.0, 1.0, 0.7),
    choice("Kind", &["Signed", "Absolute", "Billowy"], 0.0),
    choice("Multichannel", &["Off", "On"], 0.0),
    choice("Combine", &["Additive", "Mix"], 0.0),
];
const CHROMATIC: &[ParamSpec] = &[
    choice("Mode", &["Axial", "Transverse"], 0.0),
    slider("Origin X", 0.0, 1.0, 0.5),
    slider("Origin Y", 0.0, 1.0, 0.5),
    slider("Amount X", -0.1, 0.1, 0.01),
    slider("Amount Y", -0.1, 0.1, 0.01),
    slider("Inverse range", 0.01, 4.0, 1.0),
    slider("Falloff", -1.0, 1.0, 0.0),
    slider("Transition", 0.0, 2.0, 1.0),
    slider("Transparency", 0.0, 1.0, 0.0),
    slider("Strength", 0.0, 1.0, 0.0),
];
const GLITCH: &[ParamSpec] = &[choice("Preset", &["1", "2", "3", "4"], 0.0), slider("Amount", 0.0, 1.0, 0.5)];
const HALFTONE: &[ParamSpec] = &[
    slider("Amount", 0.0, 1.0, 0.12),
    slider("Softness", 0.0, 2.0, 0.5),
    slider("Anti-alias", 0.0, 2.0, 1.0),
    choice("Overlay", &["Off", "On"], 0.0),
    choice("White fill", &["Off", "On"], 0.0),
];
const BLOOM: &[ParamSpec] = &[
    slider("Upper", 0.0, 1.0, 0.8),
    slider("Lower", 0.0, 1.0, 0.6),
    slider("Burn", 0.0, 1.0, 0.346),
    slider("Spread", 0.0, 1.0, 0.14),
    slider("Amount", 0.0, 1.0, 0.5),
];
const HSB: &[ParamSpec] = &[
    ParamSpec { name: "Hue", min: -1.0, max: 1.0, default: 0.0, periodic: true, choices: &[], hidden: false },
    slider("Saturation", -1.0, 1.0, 0.0),
    slider("Brightness", -1.0, 1.0, 0.0),
];
const BALANCE: &[ParamSpec] = &[
    slider("Shadows R", -1.0, 1.0, 0.0),
    slider("Shadows G", -1.0, 1.0, 0.0),
    slider("Shadows B", -1.0, 1.0, 0.0),
    slider("Midtones R", -1.0, 1.0, 0.0),
    slider("Midtones G", -1.0, 1.0, 0.0),
    slider("Midtones B", -1.0, 1.0, 0.0),
    slider("Highlights R", -1.0, 1.0, 0.0),
    slider("Highlights G", -1.0, 1.0, 0.0),
    slider("Highlights B", -1.0, 1.0, 0.0),
];
/// Curves are edited with the curve widget, not with sliders.
const CURVES: &[ParamSpec] = &[];
const GRADIENT: &[ParamSpec] = &[slider("Amount", 0.0, 1.0, 1.0)];
const RECOLOR: &[ParamSpec] = &[
    slider("Source R", 0.0, 1.0, 1.0),
    slider("Source G", 0.0, 1.0, 0.0),
    slider("Source B", 0.0, 1.0, 0.0),
    slider("Target R", 0.0, 1.0, 0.0),
    slider("Target G", 0.0, 1.0, 0.0),
    slider("Target B", 0.0, 1.0, 1.0),
    slider("Tolerance", 0.0, 1.0, 0.2),
];
const CLONE: &[ParamSpec] = &[
    slider("Source X", 0.0, 1.0, 0.5),
    slider("Source Y", 0.0, 1.0, 0.5),
    slider("Opacity", 0.0, 1.0, 1.0),
    hidden(slider("Offset X", -4096.0, 4096.0, 0.0)),
    hidden(slider("Offset Y", -4096.0, 4096.0, 0.0)),
];

impl AdjustKind {
    /// The sixteen types, in list order.
    pub const ALL: [AdjustKind; 16] = [
        Self::Opacity,
        Self::GaussianBlur,
        Self::MotionBlur,
        Self::PerspectiveBlur,
        Self::Sharpen,
        Self::Noise,
        Self::ChromaticAberration,
        Self::Glitch,
        Self::Halftone,
        Self::Bloom,
        Self::Hsb,
        Self::ColourBalance,
        Self::Curves,
        Self::GradientMap,
        Self::Recolor,
        Self::Clone,
    ];

    /// Name in the list and in the banner.
    pub fn name(self) -> &'static str {
        match self {
            Self::Opacity => "Opacity",
            Self::GaussianBlur => "Gaussian blur",
            Self::MotionBlur => "Motion blur",
            Self::PerspectiveBlur => "Perspective blur",
            Self::Sharpen => "Sharpen",
            Self::Noise => "Noise",
            Self::ChromaticAberration => "Chromatic aberration",
            Self::Glitch => "Glitch",
            Self::Halftone => "Halftone",
            Self::Bloom => "Bloom",
            Self::Hsb => "Hue, saturation, brightness",
            Self::ColourBalance => "Color balance",
            Self::Curves => "Curves",
            Self::GradientMap => "Gradient map",
            Self::Recolor => "Recolor",
            Self::Clone => "Clone",
        }
    }

    /// Name of the undo step an applied adjustment makes, e.g. "Gaussian Blur".
    pub fn undo_name(self) -> &'static str {
        match self {
            Self::Opacity => "Opacity",
            Self::GaussianBlur => "Gaussian Blur",
            Self::MotionBlur => "Motion Blur",
            Self::PerspectiveBlur => "Perspective Blur",
            Self::Sharpen => "Sharpen",
            Self::Noise => "Noise",
            Self::ChromaticAberration => "Chromatic Aberration",
            Self::Glitch => "Glitch",
            Self::Halftone => "Halftone",
            Self::Bloom => "Bloom",
            Self::Hsb => "Hue, Saturation, Brightness",
            Self::ColourBalance => "Color Balance",
            Self::Curves => "Curves",
            Self::GradientMap => "Gradient Map",
            Self::Recolor => "Recolor",
            Self::Clone => "Clone",
        }
    }

    /// Is the filter available? Every type of the list is.
    pub fn implemented(self) -> bool {
        true
    }

    /// The parameters, in order. Parameters for other modes of the same type (the
    /// Transverse chromatic aberration) are listed too; [`Adjustment::visible_params`] filters.
    pub fn params(self) -> &'static [ParamSpec] {
        match self {
            Self::Opacity => OPACITY,
            Self::GaussianBlur => GAUSSIAN,
            Self::MotionBlur => MOTION,
            Self::PerspectiveBlur => PERSPECTIVE,
            Self::Sharpen => SHARPEN,
            Self::Noise => NOISE,
            Self::ChromaticAberration => CHROMATIC,
            Self::Glitch => GLITCH,
            Self::Halftone => HALFTONE,
            Self::Bloom => BLOOM,
            Self::Hsb => HSB,
            Self::ColourBalance => BALANCE,
            Self::Curves => CURVES,
            Self::GradientMap => GRADIENT,
            Self::Recolor => RECOLOR,
            Self::Clone => CLONE,
        }
    }

    /// Is this filter slow enough to need a proxy preview and a background commit?
    pub fn heavy(self) -> bool {
        !matches!(self, Self::Opacity | Self::Hsb)
    }
}

/// Typed data an adjustment keeps beside its numbers.
#[derive(Clone, Debug, PartialEq)]
pub enum AdjustData {
    Curves(CurvePoints),
    Gradient(GradientStops),
}

/// An adjustment object: a type, its parameter values, optional typed data and the
/// interaction mode.
#[derive(Clone, Debug, PartialEq)]
pub struct Adjustment {
    pub kind: AdjustKind,
    values: [f64; MAX_PARAMS],
    /// Pencil mode: the effect applies through the painted mask. The mask itself is
    /// passed to [`Adjustment::render`]; this flag only records the mode (Gaussian blur becomes the
    /// pyramid blur).
    pub pencil: bool,
    data: Option<Arc<AdjustData>>,
}

/// How much work a render may skip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Live preview: the filter may run on a downscaled proxy and be scaled back up.
    Preview,
    /// Commit: full resolution.
    Full,
}

/// Progress of a long render, shared with the thread that shows it. `fraction()` is 0..=1.
#[derive(Debug, Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare `n` more units of work.
    pub fn add_total(&self, n: usize) {
        self.total.fetch_add(n, Ordering::Relaxed);
    }

    /// Report `n` units finished.
    pub fn add_done(&self, n: usize) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }

    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f32 / total as f32).min(1.0)
    }
}

impl Adjustment {
    /// The adjustment with every parameter at its default, in Layer mode.
    pub fn new(kind: AdjustKind) -> Self {
        let mut values = [0.0; MAX_PARAMS];
        for (v, p) in values.iter_mut().zip(kind.params()) {
            *v = p.default;
        }
        Self { kind, values, pencil: false, data: None }
    }

    /// Value of parameter `i` (0 when the type has fewer parameters).
    pub fn value(&self, i: usize) -> f64 {
        self.values.get(i).copied().unwrap_or(0.0)
    }

    /// Write parameter `i`. Values are clamped to their range on every write; a
    /// periodic parameter wraps by adding or subtracting the range width.
    pub fn set(&mut self, i: usize, value: f64) {
        let Some(p) = self.kind.params().get(i) else {
            return;
        };
        let mut v = if value.is_finite() { value } else { p.default };
        if p.periodic {
            let span = p.max - p.min;
            while v > p.max {
                v -= span;
            }
            while v < p.min {
                v += span;
            }
        }
        self.values[i] = v.clamp(p.min, p.max);
    }

    /// Put every parameter back to its default. The mode is kept.
    pub fn reset(&mut self) {
        let pencil = self.pencil;
        *self = Self::new(self.kind);
        self.pencil = pencil;
    }

    /// The parameter indices the banner offers for the current settings, in order. Hidden
    /// parameters are left out, and the Axial or Transverse chromatic parameters follow the mode.
    pub fn visible_params(&self) -> Vec<usize> {
        let params = self.kind.params();
        (0..params.len())
            .filter(|&i| !params[i].hidden)
            .filter(|&i| {
                if self.kind != AdjustKind::ChromaticAberration {
                    return true;
                }
                let transverse = self.value(0) >= 0.5;
                if transverse {
                    matches!(i, 0 | 3 | 4 | 8 | 9)
                } else {
                    matches!(i, 0..=7)
                }
            })
            .collect()
    }

    /// Indices of the origin (or source) point that the canvas shows as a handle, in normalised
    /// coordinates, when the type has one.
    pub fn handle_indices(&self) -> Option<(usize, usize)> {
        match self.kind {
            AdjustKind::PerspectiveBlur | AdjustKind::Clone => Some((0, 1)),
            AdjustKind::ChromaticAberration if self.value(0) < 0.5 => Some((1, 2)),
            _ => None,
        }
    }

    /// The curve points, or the identity when none are set.
    pub fn curve_points(&self) -> CurvePoints {
        match self.data.as_deref() {
            Some(AdjustData::Curves(c)) => c.clone(),
            _ => CurvePoints::default(),
        }
    }

    /// Replace the curve points.
    pub fn set_curve_points(&mut self, points: CurvePoints) {
        self.data = Some(Arc::new(AdjustData::Curves(points)));
    }

    /// The gradient stops, or the default preset when none are set.
    pub fn gradient_stops(&self) -> GradientStops {
        match self.data.as_deref() {
            Some(AdjustData::Gradient(g)) => g.clone(),
            _ => GradientStops::default(),
        }
    }

    /// Replace the gradient stops.
    pub fn set_gradient_stops(&mut self, stops: GradientStops) {
        self.data = Some(Arc::new(AdjustData::Gradient(stops)));
    }

    /// Does rendering change nothing? Opacity's identity is amount 1; the other types are at
    /// their identity when their strength parameter is zero.
    pub fn is_identity(&self) -> bool {
        match self.kind {
            AdjustKind::Opacity => self.values[0] >= 1.0,
            AdjustKind::GaussianBlur => gaussian::kernel(self.values[0]).taps == 1,
            AdjustKind::Hsb => self.values[..3].iter().all(|v| *v == 0.0),
            AdjustKind::MotionBlur => (self.value(0) == 0.0 && self.value(1) == 0.0) || self.value(2) == 0.0,
            AdjustKind::PerspectiveBlur => self.value(4) == 0.0,
            AdjustKind::Sharpen => self.value(0) == 0.0,
            AdjustKind::Noise => self.value(1) == 0.0,
            AdjustKind::ChromaticAberration => {
                if self.value(0) >= 0.5 {
                    self.value(9) == 0.0
                } else {
                    self.value(3) == 0.0 && self.value(4) == 0.0
                }
            }
            AdjustKind::Glitch => self.value(1) == 0.0,
            AdjustKind::Bloom => self.value(4) == 0.0,
            AdjustKind::ColourBalance => self.values[..9].iter().all(|v| *v == 0.0),
            AdjustKind::Curves => match self.data.as_deref() {
                Some(AdjustData::Curves(c)) => c.is_identity(),
                _ => true,
            },
            AdjustKind::GradientMap => self.value(0) == 0.0,
            AdjustKind::Halftone | AdjustKind::Recolor | AdjustKind::Clone => false,
        }
    }

    /// Render the adjustment over `src` (premultiplied RGBA8, `width * height * 4` bytes).
    /// Preview and commit are the same pass; `quality` only lets a preview run cheaper. `mask` is
    /// the per-pixel weight (`None` means 1 everywhere): the painted Pencil mask, the selection, or
    /// both multiplied. Each filter mixes its output with `src` by the mask.
    pub fn render<T: Chan>(&self, src: &[T], width: usize, height: usize, quality: Quality, progress: Option<&Progress>, mask: Option<&[f32]>) -> Vec<T> {
        debug_assert_eq!(src.len(), width * height * 4);
        let mut out = match self.kind {
            AdjustKind::Opacity => opacity::apply_t(src, self.values[0]),
            AdjustKind::GaussianBlur if !self.pencil => gaussian::blur_amount(src, width, height, self.values[0], quality, progress),
            AdjustKind::Hsb => hsb::apply_t(src, self.values[0], self.values[1], self.values[2]),
            _ => return bridge::render(self, src, width, height, quality, mask),
        };
        mix_bytes(src, &mut out, mask);
        out
    }
}

/// Smallest buffer (in samples) worth splitting across threads.
const PAR_MIN: usize = 1 << 18;

/// Threads for a job of `rows` rows: one for small jobs, otherwise the machine's, capped at 8.
pub(crate) fn thread_count(samples: usize, rows: usize) -> usize {
    if samples < PAR_MIN {
        return 1;
    }
    crate::parallelism::cpus().clamp(1, 8).min(rows.max(1))
}

/// Run `f(first_row, src_band, out_band)` over contiguous bands of whole rows, on scoped threads.
/// `src` and `out` hold `row_len` samples per row (the same count). Small buffers run on the
/// calling thread. Every output sample belongs to exactly one band, so the result does not depend
/// on the thread count.
pub(crate) fn par_rows<S: Sync, U: Send>(src: &[S], out: &mut [U], row_len: usize, f: impl Fn(usize, &[S], &mut [U]) + Sync) {
    debug_assert_eq!(src.len(), out.len());
    let rows = out.len().checked_div(row_len).unwrap_or(0);
    let n = thread_count(out.len(), rows);
    if n <= 1 {
        f(0, src, out);
        return;
    }
    let per = rows.div_ceil(n);
    std::thread::scope(|scope| {
        for (i, (s, o)) in src.chunks(per * row_len).zip(out.chunks_mut(per * row_len)).enumerate() {
            let f = &f;
            scope.spawn(move || f(i * per, s, o));
        }
    });
}

/// [`par_rows`] with no source: `f(first_row, out_band)`.
pub(crate) fn par_out<U: Send>(out: &mut [U], row_len: usize, f: impl Fn(usize, &mut [U]) + Sync) {
    let rows = out.len().checked_div(row_len).unwrap_or(0);
    let n = thread_count(out.len(), rows);
    if n <= 1 {
        f(0, out);
        return;
    }
    let per = rows.div_ceil(n);
    std::thread::scope(|scope| {
        for (i, o) in out.chunks_mut(per * row_len).enumerate() {
            let f = &f;
            scope.spawn(move || f(i * per, o));
        }
    });
}

/// `out = mix(src, out, mask)` on premultiplied RGBA8. `None` leaves `out` as it is.
pub(crate) fn mix_bytes<T: Chan>(src: &[T], out: &mut [T], mask: Option<&[f32]>) {
    let Some(mask) = mask else {
        return;
    };
    let n = (src.len() / 4).min(mask.len()) * 4;
    let (src, out) = (&src[..n], &mut out[..n]);
    // Bands of whole pixels: 4 samples each, so a band of `BAND` pixels is `BAND * 4` samples.
    const BAND: usize = 4096;
    par_rows(src, out, BAND * 4, |first, s, o| {
        for (j, (sp, op)) in s.as_chunks::<4>().0.iter().zip(o.as_chunks_mut::<4>().0.iter_mut()).enumerate() {
            let t = mask[first * BAND + j].clamp(0.0, 1.0);
            if t >= 1.0 {
                continue;
            }
            for k in 0..4 {
                let (a, b) = (sp[k].to_f32(), op[k].to_f32());
                op[k] = T::from_f32_round(a + (b - a) * t);
            }
        }
    });
}

/// Keep the alpha of `before` in `after` (lock transparency): colour changes, coverage does not.
/// Transparent pixels stay as they were. Both buffers are premultiplied RGBA8.
pub fn keep_alpha(before: &[u8], after: &mut [u8]) {
    keep_alpha_t(before, after);
}

/// [`keep_alpha`] for either depth.
pub fn keep_alpha_t<T: Chan>(before: &[T], after: &mut [T]) {
    let n = before.len().min(after.len()) / 4 * 4;
    par_rows(&before[..n], &mut after[..n], 4096 * 4, |_, b, a| {
        for (old, new) in b.as_chunks::<4>().0.iter().zip(a.as_chunks_mut::<4>().0.iter_mut()) {
            if old[3].is_zero() {
                new.copy_from_slice(old);
            } else if new[3].is_zero() {
                new[..3].fill(T::default());
                new[3] = old[3];
            } else {
                let k = old[3].raw() / new[3].raw();
                for c in &mut new[..3] {
                    *c = T::from_f64_round((c.raw() * k).round().min(old[3].raw()));
                }
                new[3] = old[3];
            }
        }
    });
}

/// Box-average `src` down by the integer factor `k`. Returns the pixels and the new size
/// (`ceil(width / k)` by `ceil(height / k)`; edge blocks average what they cover). Rows of the
/// result run on several threads.
pub fn downscale<T: Chan>(src: &[T], width: usize, height: usize, k: usize) -> (Vec<T>, usize, usize) {
    let k = k.max(1);
    let (pw, ph) = (width.div_ceil(k), height.div_ceil(k));
    let mut out = vec![T::default(); pw * ph * 4];
    // The thread count follows the work (the source size), the bands follow the output rows.
    let n = thread_count(src.len(), ph);
    let per = ph.div_ceil(n.max(1)).max(1);
    let band = |first: usize, o: &mut [T]| {
        for (r, orow) in o.chunks_mut(pw * 4).enumerate() {
            let py = first + r;
            let (y0, y1) = (py * k, ((py + 1) * k).min(height));
            for px in 0..pw {
                let (x0, x1) = (px * k, ((px + 1) * k).min(width));
                let mut acc = [0u64; 4];
                for y in y0..y1 {
                    let row = &src[(y * width + x0) * 4..(y * width + x1) * 4];
                    for p in row.as_chunks::<4>().0 {
                        for (a, v) in acc.iter_mut().zip(p) {
                            *a += v.raw() as u64;
                        }
                    }
                }
                let cnt = ((y1 - y0) * (x1 - x0)) as u64;
                for (c, a) in acc.iter().enumerate() {
                    orow[px * 4 + c] = T::from_f32_round(((a + cnt / 2) / cnt) as f32);
                }
            }
        }
    };
    if n <= 1 {
        band(0, &mut out);
    } else {
        std::thread::scope(|scope| {
            for (i, o) in out.chunks_mut(per * pw * 4).enumerate() {
                let band = &band;
                scope.spawn(move || band(i * per, o));
            }
        });
    }
    (out, pw, ph)
}

/// Bilinear upscale of a proxy made by [`downscale`] back to `width x height`. Rows run on
/// several threads.
pub fn upscale<T: Chan>(src: &[T], pw: usize, ph: usize, width: usize, height: usize, k: usize) -> Vec<T> {
    let mut out = vec![T::default(); width * height * 4];
    upscale_into(src, pw, ph, width, 0, &mut out, k);
    out
}

/// [`upscale`] for the rows `first_row..` that `out` holds (`out.len() / (width * 4)` rows of a
/// `width`-wide image).
pub fn upscale_into<T: Chan>(src: &[T], pw: usize, ph: usize, width: usize, first_row: usize, out: &mut [T], k: usize) {
    let kf = k.max(1) as f32;
    // Per-column sample positions, shared by every row.
    let cols: Vec<(usize, usize, f32)> = (0..width)
        .map(|x| {
            let fx = ((x as f32 + 0.5) / kf - 0.5).clamp(0.0, (pw - 1) as f32);
            let x0 = fx.floor() as usize;
            (x0, (x0 + 1).min(pw - 1), fx.fract())
        })
        .collect();
    par_out(out, width * 4, |first, o| {
        for (r, orow) in o.chunks_mut(width * 4).enumerate() {
            let y = first_row + first + r;
            let fy = ((y as f32 + 0.5) / kf - 0.5).clamp(0.0, (ph - 1) as f32);
            let (y0, ty) = (fy.floor() as usize, fy.fract());
            let y1 = (y0 + 1).min(ph - 1);
            let (r0, r1) = (&src[y0 * pw * 4..(y0 + 1) * pw * 4], &src[y1 * pw * 4..(y1 + 1) * pw * 4]);
            for (px, &(x0, x1, tx)) in orow.as_chunks_mut::<4>().0.iter_mut().zip(&cols) {
                for c in 0..4 {
                    let (a, b) = (r0[x0 * 4 + c].to_f32(), r0[x1 * 4 + c].to_f32());
                    let (d, e) = (r1[x0 * 4 + c].to_f32(), r1[x1 * 4 + c].to_f32());
                    let top = a + (b - a) * tx;
                    let bot = d + (e - d) * tx;
                    px[c] = T::from_f32_round(top + (bot - top) * ty);
                }
            }
        }
    });
}

/// Luma, Rec. 601 weights as the design notes give them (1.8).
pub fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.299 * r + 0.586_914 * g + 0.114_014 * b
}

#[cfg(test)]
mod par_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_has_sixteen_types_and_all_are_implemented() {
        assert_eq!(AdjustKind::ALL.len(), 16);
        assert!(AdjustKind::ALL.iter().all(|k| k.implemented()));
    }

    #[test]
    fn values_clamp_and_hue_wraps() {
        let mut a = Adjustment::new(AdjustKind::Hsb);
        a.set(1, 7.0);
        assert_eq!(a.value(1), 1.0);
        a.set(2, -3.0);
        assert_eq!(a.value(2), -1.0);
        a.set(0, 1.25);
        assert!((a.value(0) - (-0.75)).abs() < 1e-12, "hue 1.25 wraps to -0.75");
        a.set(0, -1.5);
        assert!((a.value(0) - 0.5).abs() < 1e-12);
        let mut o = Adjustment::new(AdjustKind::Opacity);
        assert_eq!(o.value(0), 1.0);
        o.set(0, 2.0);
        assert_eq!(o.value(0), 1.0);
        o.set(0, -1.0);
        assert_eq!(o.value(0), 0.0);
    }

    #[test]
    fn defaults_follow_the_table() {
        assert_eq!(Adjustment::new(AdjustKind::GaussianBlur).value(0), 0.6);
        assert_eq!(Adjustment::new(AdjustKind::Opacity).value(0), 1.0);
        assert!(Adjustment::new(AdjustKind::Opacity).is_identity());
        assert!(!Adjustment::new(AdjustKind::GaussianBlur).is_identity());
        assert!(Adjustment::new(AdjustKind::Hsb).is_identity());
        assert!(Adjustment::new(AdjustKind::Curves).is_identity());
        assert!(!Adjustment::new(AdjustKind::Recolor).is_identity());
    }

    #[test]
    fn transverse_chromatic_hides_the_axial_parameters() {
        let mut a = Adjustment::new(AdjustKind::ChromaticAberration);
        assert_eq!(a.visible_params(), vec![0, 1, 2, 3, 4, 5, 6, 7]);
        a.set(0, 1.0);
        assert_eq!(a.visible_params(), vec![0, 3, 4, 8, 9]);
        assert_eq!(a.handle_indices(), None);
    }

    #[test]
    fn mix_bytes_follows_the_mask() {
        let src = [10u8, 10, 10, 255, 10, 10, 10, 255];
        let mut out = [200u8, 200, 200, 255, 200, 200, 200, 255];
        mix_bytes(&src, &mut out, Some(&[0.0, 1.0]));
        assert_eq!(&out[..4], &src[..4]);
        assert_eq!(&out[4..], &[200, 200, 200, 255]);
    }

    #[test]
    fn downscale_then_upscale_keeps_a_flat_colour() {
        let src = vec![200u8; 10 * 7 * 4];
        let (p, pw, ph) = downscale(&src, 10, 7, 4);
        assert_eq!((pw, ph), (3, 2));
        assert!(p.iter().all(|&v| v == 200));
        assert!(upscale(&p, pw, ph, 10, 7, 4).iter().all(|&v| v == 200));
    }

    #[test]
    fn keep_alpha_restores_coverage() {
        let before = [100, 0, 0, 200, 0, 0, 0, 0];
        let mut after = [50, 0, 0, 100, 9, 9, 9, 255];
        keep_alpha(&before, &mut after);
        assert_eq!(after[3], 200);
        assert_eq!(&after[4..], &[0, 0, 0, 0]);
    }
}
