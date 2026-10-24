//! Stroke geometry: input smoothing, per-sample dynamics and dab spacing.
//!
//! Pipeline per stroke: [`normalise_sample`] -> [`Smoother`] (motion filter,
//! Smoothing, stabilisation) -> [`dynamics_step`] (per-sample channels and
//! taper) -> [`generate_segments`] (4-control interpolation and spacing) -> [`Dab`]s.
//! [`Stroke`] ties the stages together with committed and predicted passes.
//!
//! Numeric behaviour follows the JS, including its no-pressure floor
//! ([`NO_PRESSURE_MIN_FACTOR`]). Function boundaries mirror the JS names in snake_case.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use crate::brushes::{Brush, CurveSet, Points, Taper};

/// Floor for pressure factors when the input has no pressure sensor.
pub const NO_PRESSURE_MIN_FACTOR: f64 = 0.35;
/// Number of table entries for brush curves.
pub const CURVE_TABLE_SIZE: usize = 1024;
/// Upper bound on stamps per dab.
pub const MAX_STAMPS: usize = 16;
/// Spacing root-finder tolerance in px and iteration cap.
pub const SPACING_TOLERANCE: f64 = 0.01;
pub const SPACING_MAX_ITER: usize = 30;

/// Clamp `x` to `[lo, hi]` with the same NaN behaviour as the JS helper.
#[inline]
pub fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// Clamp to `[0, 1]`.
#[inline]
pub fn clamp01(x: f64) -> f64 {
    clamp(x, 0.0, 1.0)
}

/// Linear interpolation `a + (b - a) * t`.
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn sinc(u: f64) -> f64 {
    if u == 0.0 {
        1.0
    } else {
        let pu = PI * u;
        pu.sin() / pu
    }
}

// ---------------------------------------------------------------------------
// Seedable random source. mulberry32, bit-identical to the JS.
// ---------------------------------------------------------------------------

/// Deterministic uniform random source. The state is one `u32`, so it can be
/// copied cheaply for the predicted-sample pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    s: u32,
}

impl Rng {
    pub fn new(seed: u32) -> Self {
        Self { s: seed }
    }

    /// Uniform value in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        self.s = self.s.wrapping_add(0x6d2b_79f5);
        let mut t = self.s;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

// ---------------------------------------------------------------------------
// Brush curves: natural cubic spline, tabulated at 1024 samples.
// ---------------------------------------------------------------------------

/// Natural cubic spline through `(xs, ys)` with zero second derivative at the ends.
#[derive(Clone, Debug)]
pub struct NaturalSpline {
    xs: Vec<f64>,
    ys: Vec<f64>,
    h: Vec<f64>,
    m: Vec<f64>,
    constant: f64,
}

/// Build a natural cubic spline. Fewer than two points gives a constant.
#[allow(clippy::needless_range_loop)]
pub fn natural_cubic_spline(xs: &[f64], ys: &[f64]) -> NaturalSpline {
    let n = xs.len();
    if n < 2 {
        return NaturalSpline {
            xs: Vec::new(),
            ys: Vec::new(),
            h: Vec::new(),
            m: Vec::new(),
            constant: ys.first().copied().unwrap_or(0.0),
        };
    }
    let h: Vec<f64> = (0..n - 1).map(|i| xs[i + 1] - xs[i]).collect();
    let mut m = vec![0.0; n];
    if n > 2 {
        // Tridiagonal system solved with the Thomas algorithm.
        let mut a = vec![0.0; n];
        let mut b = vec![0.0; n];
        let mut c = vec![0.0; n];
        let mut d = vec![0.0; n];
        b[0] = 1.0;
        b[n - 1] = 1.0;
        for i in 1..n - 1 {
            a[i] = h[i - 1];
            b[i] = 2.0 * (h[i - 1] + h[i]);
            c[i] = h[i];
            d[i] = 6.0 * ((ys[i + 1] - ys[i]) / h[i] - (ys[i] - ys[i - 1]) / h[i - 1]);
        }
        for i in 1..n {
            let w = a[i] / b[i - 1];
            b[i] -= w * c[i - 1];
            d[i] -= w * d[i - 1];
        }
        m[n - 1] = d[n - 1] / b[n - 1];
        for i in (0..n - 1).rev() {
            m[i] = (d[i] - c[i] * m[i + 1]) / b[i];
        }
    }
    NaturalSpline {
        xs: xs.to_vec(),
        ys: ys.to_vec(),
        h,
        m,
        constant: 0.0,
    }
}

impl NaturalSpline {
    /// Evaluate the spline at `x` (extrapolates with the end segments).
    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if n < 2 {
            return self.constant;
        }
        let i = if x <= self.xs[0] {
            0
        } else if x >= self.xs[n - 1] {
            n - 2
        } else {
            let (mut lo, mut hi) = (0, n - 1);
            while hi - lo > 1 {
                let mid = (lo + hi) >> 1;
                if self.xs[mid] <= x {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let hh = self.h[i];
        let a = (self.xs[i + 1] - x) / hh;
        let b = (x - self.xs[i]) / hh;
        a * self.ys[i]
            + b * self.ys[i + 1]
            + (((a * a * a - a) * self.m[i] + (b * b * b - b) * self.m[i + 1]) * hh * hh) / 6.0
    }
}

/// One user-editable response curve, tabulated for fast lookup.
#[derive(Clone, Debug)]
pub struct BrushCurve {
    table: Vec<f64>,
    points: Points,
}

impl BrushCurve {
    /// Build from control points. Fewer than two points gives the identity.
    pub fn new(points: &[[f64; 2]]) -> Self {
        let mut pts: Points = if points.len() >= 2 {
            points.to_vec()
        } else {
            vec![[0.0, 0.0], [1.0, 1.0]]
        };
        pts.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
        let xs: Vec<f64> = pts.iter().map(|p| p[0]).collect();
        let ys: Vec<f64> = pts.iter().map(|p| p[1]).collect();
        let f = natural_cubic_spline(&xs, &ys);
        let denom = (CURVE_TABLE_SIZE - 1) as f64;
        let table = (0..CURVE_TABLE_SIZE)
            .map(|i| f.eval(i as f64 / denom))
            .collect();
        Self { table, points: pts }
    }

    /// The identity curve `[[0,0],[1,1]]`.
    pub fn identity() -> Self {
        Self::new(&[[0.0, 0.0], [1.0, 1.0]])
    }

    /// Sorted control points this curve was built from.
    pub fn points(&self) -> &[[f64; 2]] {
        &self.points
    }

    /// Evaluate at `p` (clamped to [0, 1]) with linear interpolation in the table.
    pub fn eval(&self, p: f64) -> f64 {
        let x = clamp01(p) * (CURVE_TABLE_SIZE - 1) as f64;
        let i = (x.floor() as usize).min(CURVE_TABLE_SIZE - 2);
        let f = x - i as f64;
        self.table[i] * (1.0 - f) + self.table[i + 1] * f
    }
}

/// The brush curves used by the engine, built once per stroke.
#[derive(Clone, Debug)]
pub struct Curves {
    pub overall: BrushCurve,
    pub opacity: BrushCurve,
    pub size: BrushCurve,
    pub roundness: BrushCurve,
    pub bleed: BrushCurve,
    pub blur: BrushCurve,
    pub spacing_jitter: BrushCurve,
    pub stamp_count: BrushCurve,
    pub stamp_count_jitter: BrushCurve,
    pub attack: BrushCurve,
    pub load: BrushCurve,
    pub dilution: BrushCurve,
}

impl Curves {
    /// Tabulate every curve in `set`.
    pub fn from_set(set: &CurveSet) -> Self {
        Self {
            overall: BrushCurve::new(&set.overall),
            opacity: BrushCurve::new(&set.opacity),
            size: BrushCurve::new(&set.size),
            roundness: BrushCurve::new(&set.roundness),
            bleed: BrushCurve::new(&set.bleed),
            blur: BrushCurve::new(&set.blur),
            spacing_jitter: BrushCurve::new(&set.spacing_jitter),
            stamp_count: BrushCurve::new(&set.stamp_count),
            stamp_count_jitter: BrushCurve::new(&set.stamp_count_jitter),
            attack: BrushCurve::new(&set.attack),
            load: BrushCurve::new(&set.load),
            dilution: BrushCurve::new(&set.dilution),
        }
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Pointer device class. Mouse and touch never carry a pressure sensor unless
/// the sample sets `has_pressure` explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PointerType {
    #[default]
    Pen,
    Mouse,
    Touch,
    Other,
}

/// One raw input sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// Canvas pixels.
    pub x: f64,
    pub y: f64,
    /// Timestamp in seconds.
    pub t: f64,
    /// Normalised pressure in [0, 1]; `None` when unavailable.
    pub pressure: Option<f64>,
    /// Altitude in radians (pi/2 = pen perpendicular to the screen). `None` = perpendicular.
    pub tilt: Option<f64>,
    /// Azimuth in radians. `None` = 0.
    pub azimuth: Option<f64>,
    /// Barrel roll in radians.
    pub roll: f64,
    pub pointer: PointerType,
    /// Explicit pressure-sensor flag. `None` derives it from the pointer type.
    pub has_pressure: Option<bool>,
    /// Look-ahead sample.
    pub predicted: bool,
}

/// Smoothed, normalised sample flowing through the pipeline.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NormSample {
    pub x: f64,
    pub y: f64,
    pub t: f64,
    /// Pressure after the global and overall curves.
    pub p: f64,
    pub has_pressure: bool,
    pub altitude_norm: f64,
    pub azimuth: f64,
    pub roll: f64,
    pub predicted: bool,
}

/// Tilt, altitude and azimuth settings applied during normalisation.
#[derive(Clone, Debug)]
pub struct NormEnv {
    /// Default `true`: pens (and unknown pointers) have a sensor.
    pub has_pressure: bool,
    pub canvas_rotation: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    pub global_pressure: Option<BrushCurve>,
    pub overall: Option<BrushCurve>,
}

impl Default for NormEnv {
    fn default() -> Self {
        Self {
            has_pressure: true,
            canvas_rotation: 0.0,
            flip_x: false,
            flip_y: false,
            global_pressure: None,
            overall: None,
        }
    }
}

/// Result of [`tilt_vector`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TiltVector {
    pub tx: f64,
    pub ty: f64,
    pub azimuth: f64,
    pub altitude_norm: f64,
}

/// Tilt vector: `m = altitude / (pi/2)`, rotated by the canvas rotation.
pub fn tilt_vector(altitude: Option<f64>, azimuth: Option<f64>, env: &NormEnv) -> TiltVector {
    let m = altitude.unwrap_or(FRAC_PI_2) / FRAC_PI_2;
    let ang = azimuth.unwrap_or(0.0) + env.canvas_rotation;
    let mut tx = ang.cos() * m;
    let mut ty = ang.sin() * m;
    if env.flip_x {
        tx = -tx;
    }
    if env.flip_y {
        ty = -ty;
    }
    TiltVector {
        tx,
        ty,
        azimuth: ty.atan2(tx),
        altitude_norm: tx.hypot(ty),
    }
}

/// Convert W3C tilt (degrees) to `(altitude, azimuth)` in radians, as the JS does.
pub fn altitude_azimuth_from_tilt(tilt_x_deg: f64, tilt_y_deg: f64) -> (f64, f64) {
    let tx = tilt_x_deg * PI / 180.0;
    let ty = tilt_y_deg * PI / 180.0;
    let vz = clamp(tx.cos() * ty.cos(), -1.0, 1.0);
    (vz.asin(), ty.sin().atan2(tx.sin()))
}

/// Normalisation. Global pressure curve first, then the overall curve.
/// Without a pressure sensor, `p` is 1 and `has_pressure` is false.
pub fn normalise_sample(raw: &Sample, env: &NormEnv) -> NormSample {
    let finite = raw.pressure.is_some_and(f64::is_finite);
    let sensed = match raw.has_pressure {
        Some(flag) => flag && finite,
        None => {
            env.has_pressure
                && raw.pointer != PointerType::Mouse
                && raw.pointer != PointerType::Touch
                && finite
        }
    };
    let mut p = 1.0;
    if sensed {
        p = clamp01(raw.pressure.unwrap_or(1.0));
        if let Some(c) = &env.global_pressure {
            p = c.eval(p);
        }
        if let Some(c) = &env.overall {
            p = c.eval(p);
        }
        p = clamp01(p);
    }
    let tilt = tilt_vector(raw.tilt, raw.azimuth, env);
    NormSample {
        x: raw.x,
        y: raw.y,
        t: raw.t,
        p,
        has_pressure: sensed,
        altitude_norm: tilt.altitude_norm,
        azimuth: tilt.azimuth,
        roll: raw.roll,
        predicted: raw.predicted,
    }
}

// ---------------------------------------------------------------------------
// FFT. In-place iterative radix-2 complex FFT.
// ---------------------------------------------------------------------------

/// In-place complex FFT of length `re.len()` (a power of two). `inverse` also divides by n.
pub fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while (j & bit) != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let ang = sign * TAU / len as f64;
        for k in 0..half {
            let wr = (ang * k as f64).cos();
            let wi = (ang * k as f64).sin();
            for i in (0..n).step_by(len) {
                let a = i + k;
                let b = a + half;
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
    if inverse {
        let scale = n as f64;
        for v in re.iter_mut() {
            *v /= scale;
        }
        for v in im.iter_mut() {
            *v /= scale;
        }
    }
}

/// Channel selector for the smoothing stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    X,
    Y,
    P,
}

impl Channel {
    pub fn get(self, s: &NormSample) -> f64 {
        match self {
            Channel::X => s.x,
            Channel::Y => s.y,
            Channel::P => s.p,
        }
    }

    pub fn set(self, s: &mut NormSample, v: f64) {
        match self {
            Channel::X => s.x = v,
            Channel::Y => s.y = v,
            Channel::P => s.p = v,
        }
    }
}

/// Point-reflection extension `E(a)` with drift.
pub fn extend_sample(buf: &[NormSample], ch: Channel, a: isize) -> f64 {
    let n = buf.len();
    if a >= 0 && (a as usize) < n {
        return ch.get(&buf[a as usize]);
    }
    let v0 = ch.get(&buf[0]);
    let vn = ch.get(&buf[n - 1]);
    let d = vn - v0;
    let sgn = if a >= 0 { 1.0 } else { -1.0 };
    let m = a.unsigned_abs();
    let q = m / n;
    let r = m - q * n;
    if q.is_multiple_of(2) {
        v0 + sgn * ((ch.get(&buf[r]) - v0) + q as f64 * d)
    } else {
        vn + sgn * ((vn - ch.get(&buf[n - 1 - r])) + (q as f64 - sgn) * d)
    }
}

// ---------------------------------------------------------------------------
// Motion filter: zero-phase low-pass with an FFT-designed gain.
// ---------------------------------------------------------------------------

/// Zero-phase motion filter. `amount` A and `expression` B are in [0, 1].
#[derive(Clone, Debug)]
pub struct MotionFilter {
    active: bool,
    n: usize,
    gain: Vec<f64>,
}

impl MotionFilter {
    /// Design the filter for `amount` (A), `expression` (B) and the input rate.
    /// `amount <= 0` gives the identity.
    pub fn new(amount: f64, expression: f64, points_per_second: f64) -> Self {
        if amount.is_nan() || amount <= 0.0 {
            return Self {
                active: false,
                n: 0,
                gain: Vec::new(),
            };
        }
        let r = points_per_second;
        let q = amount.powf(0.01);
        let f = q / 16.0 + (1.0 - q) * r;
        let x = (r / f) * (1.0 + 7.0 * expression);
        let mut n = 2usize;
        while (n as f64) < x {
            n *= 2;
        }
        let n = n.min(1024);
        let m = x.min((n - 1) as f64);
        let t = m.ceil() as usize;
        let fc = f / r;

        // Blackman-windowed sinc kernel, zero-padded to n.
        let mut re = vec![0.0; n];
        for (j, slot) in re.iter_mut().enumerate().take(t) {
            let jf = j as f64;
            let w = 0.35875 - 0.48829 * (2.0 * PI * jf / m).cos()
                + 0.14128 * (4.0 * PI * jf / m).cos()
                - 0.01168 * (6.0 * PI * jf / m).cos();
            *slot = w * 2.0 * fc * sinc(2.0 * fc * (jf + 1.0 - m / 2.0));
        }
        // Magnitude response only: this is what makes the filter zero-phase.
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im, false);
        let mut gain: Vec<f64> = re.iter().zip(&im).map(|(a, b)| a.hypot(*b)).collect();
        let max_g = gain.iter().copied().fold(0.0, f64::max);
        if max_g > 0.0 {
            for g in gain.iter_mut() {
                *g /= max_g;
            }
        }
        Self {
            active: true,
            n,
            gain,
        }
    }

    /// FFT size N (0 when inactive).
    pub fn fft_size(&self) -> usize {
        self.n
    }

    /// Filtered copy of `buf[c]`. Indices past the buffer use the `E()` extension,
    /// so points are filtered as soon as they arrive.
    pub fn apply(&self, buf: &[NormSample], c: usize) -> NormSample {
        let src = buf[c];
        if !self.active {
            return src;
        }
        let n = self.n;
        let h = n / 2;
        let nf = n as f64;
        let mut out = src;
        for ch in [Channel::X, Channel::Y, Channel::P] {
            let mut re = vec![0.0; n];
            let mut im = vec![0.0; n];
            for (i, slot) in re.iter_mut().enumerate() {
                let a = c as isize - h as isize + i as isize;
                let b = c as isize + h as isize - i as isize;
                let d = clamp(i as f64, nf / 4.0, 3.0 * nf / 4.0) - i as f64;
                let g = (1.0 - (PI * d.abs() / h as f64).cos()) / 2.0;
                *slot = (1.0 - g) * extend_sample(buf, ch, a) + g * extend_sample(buf, ch, b);
            }
            fft(&mut re, &mut im, false);
            for ((r, i), gk) in re.iter_mut().zip(im.iter_mut()).zip(&self.gain) {
                *r *= gk;
                *i *= gk;
            }
            fft(&mut re, &mut im, true);
            ch.set(&mut out, re[h]);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Smoothing
// ---------------------------------------------------------------------------

/// Derived Smoothing constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamParams {
    /// Location strength (1 = off).
    pub s: f64,
    /// Pressure strength (1 = off).
    pub sp: f64,
    /// Window length in samples (0 = disabled).
    pub w: f64,
}

/// Constants from `smoothing` and `pressure_smoothing`.
pub fn smoothing_params(
    smoothing: f64,
    pressure_smoothing: f64,
    points_per_second: f64,
) -> StreamParams {
    let s = 1.0 - 0.96 * smoothing.powf(0.125);
    let sp = 1.0 - 0.999 * pressure_smoothing.powf(0.0625);
    let w = if smoothing == 0.0 && pressure_smoothing == 0.0 {
        0.0
    } else {
        3.0 * (points_per_second / 60.0).max(1.0)
    };
    StreamParams { s, sp, w }
}

/// Persistent Smoothing accumulator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Acc {
    pub x: f64,
    pub y: f64,
    pub p: f64,
}

/// Run Smoothing over a block where the first `nc` points are committed and the
/// rest are predicted. Returns the smoothed block and the accumulator after the
/// committed points only (predicted points never advance persistent state).
pub fn smoothing_block(
    acc: Option<Acc>,
    block: &[NormSample],
    nc: usize,
    cfg: &StreamParams,
) -> (Vec<NormSample>, Option<Acc>) {
    let StreamParams { s, sp, w } = *cfg;
    let count = block.len();
    let mut out = Vec::with_capacity(count);
    let mut a = acc;
    let mut acc_committed = acc;
    for (i, pt) in block.iter().enumerate() {
        let cur = a.get_or_insert(Acc {
            x: pt.x,
            y: pt.y,
            p: pt.p,
        });
        let mut o = *pt;
        if w > 1.0 && (s < 1.0 || sp < 1.0) {
            let rem = (w - (count - i) as f64).max(0.0) / (w - 1.0);
            if s < 1.0 {
                let wf = rem.powf(1.0 - s);
                let frac = s + wf * (wf - s);
                cur.x += frac * (pt.x - cur.x);
                cur.y += frac * (pt.y - cur.y);
                o.x = cur.x;
                o.y = cur.y;
            }
            if sp < 1.0 {
                let wfp = rem.powf(1.0 - sp);
                let frac_p = sp + wfp * (wfp - sp);
                cur.p += frac_p * (pt.p - cur.p);
                o.p = cur.p;
            }
        }
        out.push(o);
        if i + 1 == nc {
            acc_committed = Some(*cur);
        }
    }
    (out, acc_committed)
}

// ---------------------------------------------------------------------------
// Stabilisation
// ---------------------------------------------------------------------------

/// `n = floor(Z^2.2 * 120)` using the larger of the brush and global values.
pub fn stabilization_radius(brush_z: f64, global_z: f64) -> usize {
    let z = brush_z.max(global_z);
    (z.powf(2.2) * 120.0).floor().max(0.0) as usize
}

/// Value of channel `ch` at index `j`, point-reflected past either end.
pub fn reflected_value(pts: &[NormSample], j: isize, ch: Channel) -> f64 {
    let l = pts.len() as isize;
    if (0..l).contains(&j) {
        return ch.get(&pts[j as usize]);
    }
    if j < 0 {
        let k = -j;
        return if k < l {
            2.0 * ch.get(&pts[0]) - ch.get(&pts[k as usize])
        } else {
            ch.get(&pts[0])
        };
    }
    let last = l - 1;
    let k = j - last;
    if k < l {
        2.0 * ch.get(&pts[last as usize]) - ch.get(&pts[(last - k) as usize])
    } else {
        ch.get(&pts[last as usize])
    }
}

/// Mean of the `2n + 1` neighbours centred on index `c`. `n = 0` is the identity.
pub fn stabilize_at(pts: &[NormSample], c: usize, n: usize) -> NormSample {
    let src = pts[c];
    if n == 0 {
        return src;
    }
    let (mut sx, mut sy, mut sp) = (0.0, 0.0, 0.0);
    let ci = c as isize;
    let ni = n as isize;
    for j in (ci - ni)..=(ci + ni) {
        sx += reflected_value(pts, j, Channel::X);
        sy += reflected_value(pts, j, Channel::Y);
        sp += reflected_value(pts, j, Channel::P);
    }
    let k = (2 * n + 1) as f64;
    NormSample {
        x: sx / k,
        y: sy / k,
        p: sp / k,
        ..src
    }
}

// ---------------------------------------------------------------------------
// Smoother: motion filter -> Smoothing -> stabilisation.
// ---------------------------------------------------------------------------

/// Settings for [`Smoother`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmootherOptions {
    /// Input rate. Zero or negative falls back to 120.
    pub points_per_second: f64,
    pub motion_amount: f64,
    pub motion_expression: f64,
    pub smoothing: f64,
    pub pressure_smoothing: f64,
    pub stabilization: f64,
    pub global_stabilization: f64,
}

/// Incremental smoother. Committed points are processed once and never revisited.
#[derive(Clone, Debug)]
pub struct Smoother {
    motion: MotionFilter,
    sl: StreamParams,
    radius: usize,
    raw: Vec<NormSample>,
    mot: Vec<NormSample>,
    streamed: Vec<NormSample>,
    acc: Option<Acc>,
}

impl Smoother {
    pub fn new(o: &SmootherOptions) -> Self {
        let pps = if o.points_per_second > 0.0 {
            o.points_per_second
        } else {
            120.0
        };
        Self {
            motion: MotionFilter::new(o.motion_amount, o.motion_expression, pps),
            sl: smoothing_params(o.smoothing, o.pressure_smoothing, pps),
            radius: stabilization_radius(o.stabilization, o.global_stabilization),
            raw: Vec::new(),
            mot: Vec::new(),
            streamed: Vec::new(),
            acc: None,
        }
    }

    /// Feed newly committed raw samples and the current predicted samples.
    /// Returns `(committed, predicted)` smoothed samples. Predicted output is disposable.
    pub fn push(
        &mut self,
        new_raw: &[NormSample],
        pred_raw: &[NormSample],
    ) -> (Vec<NormSample>, Vec<NormSample>) {
        let mot_before = self.mot.len();
        for s in new_raw {
            self.raw.push(*s);
            let c = self.raw.len() - 1;
            let filtered = self.motion.apply(&self.raw, c);
            self.mot.push(filtered);
        }
        let new_mot: Vec<NormSample> = self.mot[mot_before..].to_vec();

        // Predicted motion outputs use committed history plus the predicted samples.
        let pred_mot: Vec<NormSample> = if pred_raw.is_empty() {
            Vec::new()
        } else {
            let mut buf = self.raw.clone();
            buf.extend_from_slice(pred_raw);
            let base = self.raw.len();
            (0..pred_raw.len())
                .map(|i| self.motion.apply(&buf, base + i))
                .collect()
        };

        let mut block = new_mot.clone();
        block.extend_from_slice(&pred_mot);
        let (out, acc_committed) = smoothing_block(self.acc, &block, new_mot.len(), &self.sl);
        self.acc = acc_committed;

        let str_before = self.streamed.len();
        self.streamed.extend_from_slice(&out[..new_mot.len()]);
        let pred_str = &out[new_mot.len()..];

        let committed: Vec<NormSample> = (str_before..self.streamed.len())
            .map(|c| stabilize_at(&self.streamed, c, self.radius))
            .collect();

        let predicted: Vec<NormSample> = if pred_str.is_empty() {
            Vec::new()
        } else {
            let base = self.streamed.len();
            let mut combined = self.streamed.clone();
            combined.extend_from_slice(pred_str);
            (0..pred_str.len())
                .map(|i| stabilize_at(&combined, base + i, self.radius))
                .collect()
        };
        (committed, predicted)
    }
}

// ---------------------------------------------------------------------------
// Per-sample dynamics
// ---------------------------------------------------------------------------

/// Running dynamics state, cloned for the predicted pass.
#[derive(Clone, Debug, PartialEq)]
pub struct DynState {
    pub n: usize,
    pub prev: Option<NormSample>,
    pub prev_p: f64,
    pub acc_len: f64,
    pub speed: f64,
}

impl Default for DynState {
    fn default() -> Self {
        Self {
            n: 0,
            prev: None,
            prev_p: 1.0,
            acc_len: 0.0,
            speed: 0.0,
        }
    }
}

/// One control point: a smoothed sample with its dynamics applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Control {
    pub x: f64,
    pub y: f64,
    pub t: f64,
    /// Size along the stroke axis, px.
    pub sx: f64,
    /// Size across the stroke axis, px.
    pub sy: f64,
    pub pressure: f64,
    pub has_pressure: bool,
    pub altitude_norm: f64,
    pub azimuth: f64,
    pub roll: f64,
    pub opacity: f64,
    pub mixture: f64,
    pub blur: f64,
    pub bleed: f64,
    pub speed: f64,
    pub acc_len: f64,
    pub seg_len: f64,
}

/// Shape power from the taper shape in [-1, 1].
pub fn taper_shape_power(shape: f64) -> f64 {
    if shape >= 0.0 {
        1.0 - 0.984_375 * shape
    } else {
        1.0 / (1.0 + 0.666_666_6 * shape)
    }
}

/// How far the pen leans, from 0 (perpendicular) to 1 (flat on the surface), from the
/// normalised altitude.
pub fn tilt_amount(altitude_norm: f64) -> f64 {
    clamp01(1.0 - altitude_norm)
}

/// Tilt modifiers as `(size, opacity, roundness)` factors for a pen at
/// `altitude_norm`. Each is 1 for an upright pen or a brush that ignores tilt.
///
/// * size: `tilt_size > 0` grows the tip up to `1 + 3 * tilt_size` when flat; `< 0` shrinks it
///   to `1 + tilt_size` when flat;
/// * opacity: `tilt_opacity > 0` is `1 - a * (1 - tilt)` (darker as the pen lies down),
///   `< 0` is `1 + a * tilt` (lighter);
/// * roundness: `1 - 0.9 * tilt_roundness * tilt`, a flatter tip as the pen lies down.
pub fn tilt_factors(brush: &Brush, altitude_norm: f64) -> (f64, f64, f64) {
    let t = tilt_amount(altitude_norm);
    let a = brush.tilt_size;
    let size = if a >= 0.0 { 1.0 + 3.0 * a * t } else { (1.0 + a * t).max(0.0) };
    let o = brush.tilt_opacity;
    let opacity = if o >= 0.0 { 1.0 - o * (1.0 - t) } else { 1.0 + o * t };
    let round = 1.0 - 0.9 * brush.tilt_roundness.clamp(0.0, 1.0) * t;
    (size, opacity, round)
}

fn taper_size_step(size: f64, amt: f64, tp: &Taper, unit: f64, sp: f64) -> f64 {
    lerp(size, 0.1 * unit, tp.size * (1.0 - amt.powf(sp)))
}

/// Engine configuration shared by every stage of one stroke.
#[derive(Clone, Debug)]
pub struct Cfg {
    pub brush: Brush,
    pub curves: Curves,
    /// Input rate used for time normalisation.
    pub pps: f64,
    pub screen_scale: f64,
    pub distance_scale: f64,
}

impl Cfg {
    /// Build a configuration, tabulating the brush curves.
    pub fn new(brush: Brush, pps: f64, screen_scale: f64, distance_scale: f64) -> Self {
        let curves = Curves::from_set(&brush.curves);
        Self {
            brush,
            curves,
            pps,
            screen_scale,
            distance_scale,
        }
    }
}

/// Turn one smoothed sample into a [`Control`], advancing `st`.
///
/// `total_len` is `Some` only at pen-up (end taper). `last_seg_len` is the
/// length of the last few segments and clamps the end-taper ramp.
pub fn dynamics_step(
    st: &mut DynState,
    s: &NormSample,
    cfg: &Cfg,
    total_len: Option<f64>,
    last_seg_len: f64,
) -> Control {
    let brush = &cfg.brush;
    let curves = &cfg.curves;
    let d = cfg.distance_scale;
    let hasp = s.has_pressure;

    // Segment length and normalised time step.
    let mut seg_len = 0.0;
    let mut dt = 60.0 / cfg.pps;
    if let Some(prev) = st.prev {
        seg_len = (s.x - prev.x).hypot(s.y - prev.y);
        let dtt = (s.t - prev.t).abs();
        if dtt > 0.0 {
            dt = dtt * 60.0;
        }
    }
    st.acc_len += seg_len;
    let acc_len = st.acc_len;

    // 3.1 speed (0..1), low-passed. The first segment uses v directly.
    let mut v = 0.0;
    if st.prev.is_some() {
        let inner = ((seg_len * cfg.screen_scale) / dt - 25.0 * d) / (100.0 * d);
        v = clamp01(inner.max(0.0).sqrt());
    }
    let speed = if st.n <= 1 {
        v
    } else {
        (v + (3.0 / dt) * st.speed) / (3.0 / dt + 1.0)
    };

    // 3.2 pressure softening (optional).
    let mut p = s.p;
    if brush.pressure_softening && hasp && st.prev.is_some() && seg_len > 0.0 {
        let w = clamp(dt / seg_len, 0.2, 1.0);
        p = lerp(st.prev_p, p, w);
    }

    // 3.4 taper. Pencil and finger settings are separate; choose by hasPressure.
    let tp = if hasp {
        &brush.taper.pencil
    } else {
        &brush.taper.finger
    };
    let sp = taper_shape_power(tp.shape);
    let start_len = tp.start_length * 512.0;
    let start_amt = if start_len > 0.0 {
        clamp01(acc_len / start_len)
    } else {
        1.0
    };
    let mut end_amt = 1.0;
    if let Some(total) = total_len {
        if tp.end_length > 0.0 {
            let denom = (tp.end_length * 512.0).min(last_seg_len);
            end_amt = if denom > 0.0 {
                clamp01((total - acc_len) / denom)
            } else {
                1.0
            };
        }
    }
    // Pressure-driven channels are scaled by the taper.
    let pf = if hasp {
        (start_amt * end_amt).powf(sp) * tp.pressure + (1.0 - tp.pressure)
    } else {
        1.0
    };

    // 3.3 channels, each curve evaluated at the pressure p.
    let size_p = curves.size.eval(p) * pf;
    let opacity_p = curves.opacity.eval(p) * pf;
    let roundness_p = curves.roundness.eval(p) * pf;

    // Size.
    let base = brush.size;
    let dss = brush.speed_size;
    let speed_f = if dss > 0.0 {
        1.0 - dss * speed
    } else {
        1.0 + dss - dss * speed
    };
    let dps = brush.pressure_size;
    let press_f = if hasp {
        if dps >= 0.0 {
            1.0 - dps + dps * size_p
        } else {
            1.0 + dps * size_p
        }
    } else {
        (1.0 - dps.abs()).max(NO_PRESSURE_MIN_FACTOR)
    };
    let mut sx = press_f * speed_f * base;
    // A minimum above the maximum (an edited or imported brush) would make more pressure give a
    // smaller tip past a point: the two bounds are taken in order.
    let (lo, hi) = (brush.min_size.min(brush.max_size), brush.min_size.max(brush.max_size));
    sx = clamp(sx, lo * base, hi * base);
    let (tilt_size_f, tilt_opacity_f, tilt_round_f) = tilt_factors(brush, s.altitude_norm);
    sx *= tilt_size_f;
    sx = taper_size_step(sx, start_amt, tp, base, sp);
    sx = taper_size_step(sx, end_amt, tp, base, sp);
    let roundness = lerp(
        brush.pressure_roundness_min,
        brush.pressure_roundness,
        clamp01(roundness_p),
    );
    let sy = sx * roundness * brush.shape_roundness * tilt_round_f;

    // Opacity follows the same pattern.
    let dpo = brush.pressure_opacity;
    let dso = brush.speed_opacity;
    let speed_o = if dso > 0.0 {
        1.0 - dso * speed
    } else {
        1.0 + dso - dso * speed
    };
    let op_f = if hasp {
        if dpo >= 0.0 {
            1.0 - dpo + dpo * opacity_p
        } else {
            1.0 + dpo * opacity_p
        }
    } else {
        (1.0 - dpo.abs()).max(NO_PRESSURE_MIN_FACTOR)
    };
    let mut opacity = brush.opacity * op_f * speed_o * tilt_opacity_f;
    for amt in [start_amt, end_amt] {
        opacity *= 1.0 - (tp.opacity * (1.0 - amt.powf(sp))).powf(2.5);
    }
    let opacity = clamp01(opacity);

    // Blur and bleed.
    let blur = brush.dynamics_blur * curves.blur.eval(p);
    let bleed = brush.bleed * lerp(1.0, curves.bleed.eval(p), brush.bleed_pressure);

    // 3.3 wet mix (only for wet brushes).
    let mut mixture = 0.0;
    let w = &brush.wet;
    if w.enabled {
        let tilt_attack = clamp(
            1.0 - (s.altitude_norm - 0.190_985_93)
                / (w.tilt_angle * 0.809_014_1 + (1.0 - w.tilt_angle) * 0.190_985_93),
            1.0 / 1024.0,
            1.0,
        );
        let attack = p * tilt_attack * curves.attack.eval(p) * w.attack;
        let falloff = 1.0 - clamp01((acc_len * w.falloff_amount.powf(3.0) / 33.0).cbrt());
        let load = if w.load_amount > 0.0 {
            1.0 - clamp01((acc_len * (1.0 - w.load_amount).powf(3.0) / 33.0).cbrt())
        } else {
            0.0
        };
        let dil = (1.0 - curves.dilution.eval(p) * w.dilution).max(0.0);
        let mix = falloff * dil.powf(2.4) + curves.load.eval(p) * load;
        mixture = clamp01(mix.max(attack));
    }

    // Advance state.
    st.prev = Some(*s);
    st.prev_p = p;
    st.speed = speed;
    st.n += 1;

    Control {
        x: s.x,
        y: s.y,
        t: s.t,
        sx,
        sy,
        pressure: p,
        has_pressure: hasp,
        altitude_norm: s.altitude_norm,
        azimuth: s.azimuth,
        roll: s.roll,
        opacity,
        mixture,
        blur,
        bleed,
        speed,
        acc_len,
        seg_len,
    }
}

// ---------------------------------------------------------------------------
// Interpolation over four controls
// ---------------------------------------------------------------------------

fn unwrap_near(v: f64, reference: f64) -> f64 {
    let mut r = v;
    // A non-finite or huge angle (from a pen driver) would never come within a turn by steps
    // of TAU: reduce it first.
    if !(r - reference).is_finite() {
        return reference;
    }
    if (r - reference).abs() > 64.0 * TAU {
        r = reference + (r - reference).rem_euclid(TAU);
    }
    while r - reference > PI {
        r -= TAU;
    }
    while r - reference < -PI {
        r += TAU;
    }
    r
}

/// Blend four controls at `t` in [0, 1].
///
/// The curve runs from `c2` (t = 0) to `c1` (t = 1). `c3` is the oldest and `c0`
/// the look-ahead. Angles are unwrapped before blending. The fields that are not
/// interpolated (`t`, `has_pressure`, `seg_len`) are copied from `c2`.
pub fn interpolate_window(
    c3: &Control,
    c2: &Control,
    c1: &Control,
    c0: &Control,
    t: f64,
) -> Control {
    let th = t * FRAC_PI_2;
    let cs = th.cos();
    let sn = th.sin();
    let w0 = 0.25 - cs / 4.0; // current
    let w1 = 0.25 + sn / 4.0;
    let w2 = 0.25 + cs / 4.0;
    let w3 = 0.25 - sn / 4.0; // oldest
    let blend = |get: fn(&Control) -> f64, angle: bool| -> f64 {
        let mut a3 = get(c3);
        let a2 = get(c2);
        let mut a1 = get(c1);
        let mut a0 = get(c0);
        if angle {
            a3 = unwrap_near(a3, a2);
            a1 = unwrap_near(a1, a2);
            a0 = unwrap_near(a0, a2);
        }
        w0 * a0 + w1 * a1 + w2 * a2 + w3 * a3
    };
    Control {
        x: blend(|c| c.x, false),
        y: blend(|c| c.y, false),
        t: c2.t,
        sx: blend(|c| c.sx, false),
        sy: blend(|c| c.sy, false),
        pressure: blend(|c| c.pressure, false),
        has_pressure: c2.has_pressure,
        altitude_norm: blend(|c| c.altitude_norm, false),
        azimuth: blend(|c| c.azimuth, true),
        roll: blend(|c| c.roll, true),
        opacity: blend(|c| c.opacity, false),
        mixture: blend(|c| c.mixture, false),
        blur: blend(|c| c.blur, false),
        bleed: blend(|c| c.bleed, false),
        speed: blend(|c| c.speed, false),
        acc_len: blend(|c| c.acc_len, false),
        seg_len: c2.seg_len,
    }
}

// ---------------------------------------------------------------------------
// Spacing and per-dab values
// ---------------------------------------------------------------------------

/// Spacing diameter `D(sz)` with k = 1. `sz` is the dab diameter in px.
pub fn spacing_diameter(sz: f64, spacing: f64) -> f64 {
    (1.5 * sz) / 128.0 + 0.5 + spacing * (2.0 * sz + 1.0)
}

/// One stamp: a dab with its per-stamp jitter applied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dab {
    /// Centre in canvas px.
    pub x: f64,
    pub y: f64,
    /// Quad size, px (`sx` along the stroke axis, `sy` across it).
    pub sx: f64,
    pub sy: f64,
    /// Rotation in radians.
    pub angle: f64,
    pub opacity: f64,
    pub flow: f64,
    pub mixture: f64,
    pub blur: f64,
    pub bleed: f64,
    pub pressure: f64,
    pub speed: f64,
    pub acc_len: f64,
    /// Curve parameter of the dab along its segment.
    pub t: f64,
    /// Stamp index within the dab, `0..MAX_STAMPS`.
    pub stamp: usize,
    /// Grain offset for moving grain.
    pub grain_x: f64,
    pub grain_y: f64,
}

/// Last placed dab (spacing reference).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LastDab {
    pub x: f64,
    pub y: f64,
    pub sx: f64,
    pub sy: f64,
}

/// Spacing and generation state. `strokeRotation` comes.
#[derive(Clone, Debug)]
pub struct GenState {
    pub seg: usize,
    pub t: f64,
    pub last: Option<LastDab>,
    pub rng: Rng,
    pub stroke_rotation: f64,
}

/// Build the stamps for one dab at curve position `tt`.
/// Draw order is fixed so the committed, predicted and replayed passes agree.
fn make_stamps(v: &Control, tt: f64, dir: f64, g: &mut GenState, cfg: &Cfg, out: &mut Vec<Dab>) {
    let brush = &cfg.brush;
    let curves = &cfg.curves;
    let stroke_rot = g.stroke_rotation;
    let rng = &mut g.rng;
    let p = v.pressure;
    let stamp_rnd = rng.next_f64();
    let raw = curves.stamp_count.eval(p)
        * (1.0 - stamp_rnd * curves.stamp_count_jitter.eval(p) * brush.stamp_count_jitter)
        * brush.stamp_count;
    let count = clamp(1.0 + raw.floor(), 1.0, MAX_STAMPS as f64) as usize;
    let sz = v.sx.min(v.sy);
    let mut angle_base =
        brush.shape_rotation + stroke_rot + if brush.follow_stroke { dir } else { 0.0 };
    // The tip follows the pen azimuth and the barrel roll when asked to.
    if brush.tilt_rotation {
        angle_base += v.azimuth;
    }
    if brush.roll_rotation {
        angle_base += v.roll;
    }
    for k in 0..count {
        let ox = (rng.next_f64() * 2.0 - 1.0) * brush.scatter * sz;
        let oy = (rng.next_f64() * 2.0 - 1.0) * brush.scatter * sz;
        let s_j = 1.0 + (rng.next_f64() * 2.0 - 1.0) * brush.size_jitter;
        let o_j = 1.0 + (rng.next_f64() * 2.0 - 1.0) * brush.opacity_jitter;
        let r_j = (rng.next_f64() * 2.0 - 1.0) * brush.rotation_jitter * PI;
        let grain_x = rng.next_f64();
        let grain_y = rng.next_f64();
        out.push(Dab {
            x: v.x + ox,
            y: v.y + oy,
            sx: (v.sx * s_j).max(0.0),
            sy: (v.sy * s_j).max(0.0),
            angle: angle_base + r_j,
            opacity: clamp01(v.opacity * o_j),
            flow: brush.flow,
            mixture: v.mixture,
            blur: v.blur,
            bleed: v.bleed,
            pressure: p,
            speed: v.speed,
            acc_len: v.acc_len,
            t: tt,
            stamp: k,
            grain_x,
            grain_y,
        });
    }
}

/// Generate dabs for segments `g.seg ..= j_to` of `ctrl`.
///
/// Each dab is placed by a regula-falsi (Illinois) search along the current
/// segment for the point at distance `step` from the last dab. `g` is mutated.
pub fn generate_segments(ctrl: &[Control], g: &mut GenState, cfg: &Cfg, j_to: isize) -> Vec<Dab> {
    let mut out = Vec::new();
    let n = ctrl.len();
    if n < 2 {
        return out;
    }
    let last_j = j_to.min(n as isize - 2);
    let brush = &cfg.brush;
    let curves = &cfg.curves;
    while (g.seg as isize) <= last_j {
        let j = g.seg;
        let c3 = ctrl[j.saturating_sub(1)];
        let c2 = ctrl[j];
        let c1 = ctrl[j + 1];
        let c0 = ctrl[(j + 2).min(n - 1)];
        let dir = (c1.y - c2.y).atan2(c1.x - c2.x);
        let at = |tt: f64| interpolate_window(&c3, &c2, &c1, &c0, tt);
        let mut guard = 0;
        while g.t <= 1.0 && guard < 10_000 {
            guard += 1;
            let Some(last) = g.last else {
                // First dab of the stroke: placed at the start of the curve.
                let tt = g.t;
                let v = at(tt);
                make_stamps(&v, tt, dir, g, cfg, &mut out);
                g.last = Some(LastDab {
                    x: v.x,
                    y: v.y,
                    sx: v.sx,
                    sy: v.sy,
                });
                continue;
            };
            let last_sz = last.sx.min(last.sy);
            // One fresh random number per dab attempt.
            let r_j = g.rng.next_f64();
            let f = |tt: f64| -> f64 {
                let v = at(tt);
                let dist = (v.x - last.x).hypot(v.y - last.y);
                let sz = v.sx.min(v.sy);
                let jj = v.speed * brush.spacing_speed
                    + r_j * curves.spacing_jitter.eval(v.pressure) * brush.spacing_jitter;
                let step = ((spacing_diameter(last_sz, brush.spacing)
                    + spacing_diameter(sz, brush.spacing))
                    / 2.0)
                    * (1.0 + jj);
                dist - step
            };
            let mut a = g.t;
            let mut fa = f(a);
            let root = if fa >= 0.0 {
                a
            } else {
                let mut b = 1.0;
                let mut fb = f(b);
                if fb < 0.0 {
                    // No further dab in this segment.
                    g.t = 1.0;
                    break;
                }
                let mut side = 0i32;
                let mut x = b;
                for _ in 0..SPACING_MAX_ITER {
                    x = clamp((a * fb - b * fa) / (fb - fa), a, b);
                    let fx = f(x);
                    if fx.abs() < SPACING_TOLERANCE {
                        break;
                    }
                    if fx < 0.0 {
                        a = x;
                        fa = fx;
                        if side == -1 {
                            fb *= 0.5;
                        }
                        side = -1;
                    } else {
                        b = x;
                        fb = fx;
                        if side == 1 {
                            fa *= 0.5;
                        }
                        side = 1;
                    }
                }
                x
            };
            let v = at(root);
            make_stamps(&v, root, dir, g, cfg, &mut out);
            g.last = Some(LastDab {
                x: v.x,
                y: v.y,
                sx: v.sx,
                sy: v.sy,
            });
            g.t = root;
        }
        g.seg += 1;
        g.t = 0.0;
    }
    out
}

// ---------------------------------------------------------------------------
// Colour jitter
// ---------------------------------------------------------------------------

/// RGB in 0..1 to HSL `(h, s, l)`, each in 0..1.
pub fn rgb_to_hsl(r: f64, g: f64, b: f64) -> [f64; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let mut h = 0.0;
    let mut s = 0.0;
    if max != min {
        let d = max - min;
        s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        h = if max == r {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        h /= 6.0;
    }
    [h, s, l]
}

/// HSL in 0..1 to RGB in 0..1.
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> [f64; 3] {
    if s == 0.0 {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t0: f64| -> f64 {
        let mut t = t0;
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// Stroke-level colour offsets.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StrokeMeta {
    pub stroke_rotation: f64,
    pub hue_offset: f64,
    pub saturation_offset: f64,
    pub lightness_offset: f64,
    pub darkness_offset: f64,
}

/// Apply a stroke's colour jitter to an RGB colour in 0..1.
pub fn jitter_colour(rgb: [f64; 3], meta: &StrokeMeta) -> [f64; 3] {
    let [h0, s0, l0] = rgb_to_hsl(rgb[0], rgb[1], rgb[2]);
    let h = (h0 + meta.hue_offset + 1.0) % 1.0;
    let s = clamp01(s0 + meta.saturation_offset);
    let l = clamp01(l0 + meta.lightness_offset - meta.darkness_offset);
    hsl_to_rgb(h, s, l)
}

/// The colour a stroke paints with: `rgb` with the stroke's colour jitter applied.
/// A brush with no colour jitter returns `rgb` untouched.
pub fn stroke_colour(brush: &Brush, rgb: [f64; 3], meta: &StrokeMeta) -> [f64; 3] {
    if brush.hue_jitter == 0.0
        && brush.saturation_jitter == 0.0
        && brush.lightness_jitter == 0.0
        && brush.darkness_jitter == 0.0
    {
        rgb
    } else {
        jitter_colour(rgb, meta)
    }
}

// ---------------------------------------------------------------------------
// Stroke driver
// ---------------------------------------------------------------------------

/// Per-stroke options.
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeOptions {
    /// Input sample rate. Zero or negative falls back to 120.
    pub points_per_second: f64,
    /// Zoom-dependent screen scale (1 at 100%).
    pub screen_scale: f64,
    /// Distance scale for speed (1 at 100% zoom).
    pub distance_scale: f64,
    /// Seed for all stroke randomness.
    pub seed: u32,
    /// Whether the input device reports pressure. Default `true`.
    pub has_pressure: bool,
    pub canvas_rotation: f64,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Global stabilisation, combined with the brush value by max.
    pub stabilization: f64,
    /// Optional global pressure curve control points.
    pub global_pressure_curve: Option<Points>,
}

impl Default for StrokeOptions {
    fn default() -> Self {
        Self {
            points_per_second: 120.0,
            screen_scale: 1.0,
            distance_scale: 1.0,
            seed: 1,
            has_pressure: true,
            canvas_rotation: 0.0,
            flip_x: false,
            flip_y: false,
            stabilization: 0.0,
            global_pressure_curve: None,
        }
    }
}

/// Dabs produced by one [`Stroke::add_samples`] call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrokeUpdate {
    /// Final dabs, appended to [`Stroke::dabs`].
    pub committed_dabs: Vec<Dab>,
    /// Disposable dabs from the look-ahead samples. Rebuilt on every update.
    pub predicted_dabs: Vec<Dab>,
}

/// Replayed tail of a stroke after pen-up (end taper applied).
#[derive(Clone, Debug, PartialEq)]
pub struct TailRedraw {
    /// First index whose dab differs from the committed list.
    pub from: usize,
    /// Dabs from `from` onwards.
    pub dabs: Vec<Dab>,
}

/// Result of [`Stroke::finish`].
#[derive(Clone, Debug, PartialEq)]
pub struct StrokeEnd {
    /// The complete dab list with the end taper applied.
    pub dabs: Vec<Dab>,
    /// `None` when the stroke had no samples or was already finished.
    pub tail_redraw: Option<TailRedraw>,
}

fn same_dab(a: &Dab, b: &Dab) -> bool {
    (a.x - b.x).abs() < 1e-9
        && (a.y - b.y).abs() < 1e-9
        && (a.sx - b.sx).abs() < 1e-9
        && (a.sy - b.sy).abs() < 1e-9
        && (a.opacity - b.opacity).abs() < 1e-9
}

fn or_default(x: f64, default: f64) -> f64 {
    if x <= 0.0 || x.is_nan() {
        default
    } else {
        x
    }
}

/// One stroke, built incrementally:
///
/// ```text
/// let mut s = Stroke::new(brush, StrokeOptions::default());
/// let update = s.add_samples(&committed, &predicted); // or s.add_sample(sample)
/// let end = s.finish();                              // pen-up, end taper applied
/// ```
///
/// Committed dabs are final. `predicted_dabs` must be replaced on every call.
/// `finish` replays the whole stroke with the end taper and returns the full list.
#[derive(Clone, Debug)]
pub struct Stroke {
    cfg: Cfg,
    meta: StrokeMeta,
    norm_env: NormEnv,
    smoother: Smoother,
    dyn_state: DynState,
    /// Generation state at the start of the stroke, used again by `finish`.
    fresh: GenState,
    g: GenState,
    ctrl: Vec<Control>,
    smoothed: Vec<NormSample>,
    dabs: Vec<Dab>,
    ended: bool,
}

impl Stroke {
    /// Start a stroke with `brush` and `opts`.
    pub fn new(brush: Brush, opts: StrokeOptions) -> Self {
        let pps = or_default(opts.points_per_second, 120.0);
        let screen_scale = or_default(opts.screen_scale, 1.0);
        let distance_scale = or_default(opts.distance_scale, 1.0);
        let cfg = Cfg::new(brush, pps, screen_scale, distance_scale);

        // 4.4 stroke-level randomness, drawn in a fixed order.
        let mut rng = Rng::new(opts.seed);
        let stroke_rotation = if cfg.brush.random_rotation {
            (rng.next_f64() * 2.0 - 1.0) * PI
        } else {
            0.0
        };
        let meta = StrokeMeta {
            stroke_rotation,
            hue_offset: (rng.next_f64() - 0.5) * cfg.brush.hue_jitter,
            saturation_offset: rng.next_f64() * cfg.brush.saturation_jitter,
            lightness_offset: rng.next_f64() * cfg.brush.lightness_jitter,
            darkness_offset: rng.next_f64() * cfg.brush.darkness_jitter,
        };
        let fresh = GenState {
            seg: 0,
            t: 0.0,
            last: None,
            rng,
            stroke_rotation,
        };

        let norm_env = NormEnv {
            has_pressure: opts.has_pressure,
            canvas_rotation: opts.canvas_rotation,
            flip_x: opts.flip_x,
            flip_y: opts.flip_y,
            global_pressure: opts
                .global_pressure_curve
                .as_ref()
                .map(|pts| BrushCurve::new(pts)),
            overall: Some(cfg.curves.overall.clone()),
        };
        let smoother = Smoother::new(&SmootherOptions {
            points_per_second: pps,
            motion_amount: cfg.brush.motion_amount,
            motion_expression: cfg.brush.motion_expression,
            smoothing: cfg.brush.smoothing,
            pressure_smoothing: cfg.brush.pressure_smoothing,
            stabilization: cfg.brush.stabilization,
            global_stabilization: opts.stabilization,
        });

        Self {
            cfg,
            meta,
            norm_env,
            smoother,
            dyn_state: DynState::default(),
            g: fresh.clone(),
            fresh,
            ctrl: Vec::new(),
            smoothed: Vec::new(),
            dabs: Vec::new(),
            ended: false,
        }
    }

    /// The brush this stroke uses.
    pub fn brush(&self) -> &Brush {
        &self.cfg.brush
    }

    /// Stroke-level randomness and colour offsets.
    pub fn meta(&self) -> &StrokeMeta {
        &self.meta
    }

    /// Committed dabs so far.
    pub fn dabs(&self) -> &[Dab] {
        &self.dabs
    }

    /// Feed one committed sample. Equivalent to `add_samples(&[sample], &[])`.
    pub fn add_sample(&mut self, sample: Sample) -> StrokeUpdate {
        self.add_samples(&[sample], &[])
    }

    /// Feed committed samples (final) and predicted samples (look-ahead).
    ///
    /// # Panics
    /// Panics if called after [`Stroke::finish`].
    pub fn add_samples(&mut self, committed: &[Sample], predicted: &[Sample]) -> StrokeUpdate {
        assert!(!self.ended, "Stroke::add_samples called after finish()");
        let nc: Vec<NormSample> = committed
            .iter()
            .map(|s| normalise_sample(s, &self.norm_env))
            .collect();
        let np: Vec<NormSample> = predicted
            .iter()
            .map(|s| normalise_sample(s, &self.norm_env))
            .collect();
        let (new_committed, new_predicted) = self.smoother.push(&nc, &np);

        for s in &new_committed {
            self.smoothed.push(*s);
            let c = dynamics_step(&mut self.dyn_state, s, &self.cfg, None, 0.0);
            self.ctrl.push(c);
        }
        let committed_dabs = generate_segments(
            &self.ctrl,
            &mut self.g,
            &self.cfg,
            self.ctrl.len() as isize - 3,
        );
        self.dabs.extend_from_slice(&committed_dabs);

        // Predicted pass: disposable copies of the dynamics and generation state.
        let mut predicted_dabs = Vec::new();
        if !new_predicted.is_empty() {
            let base = self.ctrl.len();
            let mut dyn_copy = self.dyn_state.clone();
            for s in &new_predicted {
                let c = dynamics_step(&mut dyn_copy, s, &self.cfg, None, 0.0);
                self.ctrl.push(c);
            }
            let mut g_copy = self.g.clone();
            let j_to = self.ctrl.len() as isize - 2;
            predicted_dabs = generate_segments(&self.ctrl, &mut g_copy, &self.cfg, j_to);
            self.ctrl.truncate(base);
        }
        StrokeUpdate {
            committed_dabs,
            predicted_dabs,
        }
    }

    /// Pen-up. Replays the committed samples with the end taper and
    /// regenerates every dab. Calling it again returns the same dabs with no tail.
    pub fn finish(&mut self) -> StrokeEnd {
        if self.ended {
            return StrokeEnd {
                dabs: self.dabs.clone(),
                tail_redraw: None,
            };
        }
        self.ended = true;
        let n = self.ctrl.len();
        if n == 0 {
            return StrokeEnd {
                dabs: Vec::new(),
                tail_redraw: None,
            };
        }
        let total_len = self.ctrl[n - 1].acc_len;
        let last_seg_len: f64 = self.ctrl[n.saturating_sub(8)..]
            .iter()
            .map(|c| c.seg_len)
            .sum();

        let mut dyn_state = DynState::default();
        let ctrl: Vec<Control> = self
            .smoothed
            .iter()
            .map(|s| dynamics_step(&mut dyn_state, s, &self.cfg, Some(total_len), last_seg_len))
            .collect();
        let mut g = self.fresh.clone();
        let dabs = generate_segments(&ctrl, &mut g, &self.cfg, n as isize - 2);

        let mut from = 0;
        while from < dabs.len() && from < self.dabs.len() && same_dab(&dabs[from], &self.dabs[from])
        {
            from += 1;
        }
        let tail = dabs[from..].to_vec();
        self.ctrl = ctrl;
        self.dabs = dabs.clone();
        StrokeEnd {
            dabs,
            tail_redraw: Some(TailRedraw { from, dabs: tail }),
        }
    }
}
