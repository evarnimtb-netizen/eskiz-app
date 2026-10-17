//! Typed data that the adjustments need beyond their numeric parameters: curve control points
//! and gradient stops. Both are small, so the adjustment keeps them behind an `Arc` and clones
//! them when the UI edits them.

/// Curve control points for the master curve and the red, green and blue curves, in that order.
/// Each curve is a list of `[x, y]` points in [0, 1]², sorted by `x`. The default is the
/// identity line from (0, 0) to (1, 1).
#[derive(Clone, Debug, PartialEq)]
pub struct CurvePoints {
    pub channels: [Vec<[f32; 2]>; 4],
}

/// The two endpoints of the identity curve.
pub const IDENTITY_POINTS: [[f32; 2]; 2] = [[0.0, 0.0], [1.0, 1.0]];

impl Default for CurvePoints {
    fn default() -> Self {
        Self { channels: std::array::from_fn(|_| IDENTITY_POINTS.to_vec()) }
    }
}

impl CurvePoints {
    /// Does every curve equal the identity?
    pub fn is_identity(&self) -> bool {
        self.channels.iter().all(|c| c.len() == 2 && c[0] == IDENTITY_POINTS[0] && c[1] == IDENTITY_POINTS[1])
    }

    /// The 256-entry table of channel `k` (0 master, 1 red, 2 green, 3 blue).
    pub fn table(&self, k: usize) -> [f32; 256] {
        curve_table(&self.channels[k])
    }
}

/// Sample a piecewise-linear curve through `points` at x = i / 256 for i in 0..256. Points
/// outside the first or last x are held at the end values. Outputs are clamped to [0, 1].
pub fn curve_table(points: &[[f32; 2]]) -> [f32; 256] {
    let mut pts: Vec<[f32; 2]> = points.iter().map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]).collect();
    if pts.is_empty() {
        pts = IDENTITY_POINTS.to_vec();
    }
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let first = pts[0];
    let last = pts[pts.len() - 1];
    std::array::from_fn(|i| {
        let x = i as f32 / 256.0;
        if x <= first[0] {
            return first[1];
        }
        if x >= last[0] {
            return last[1];
        }
        let j = pts.windows(2).position(|w| x >= w[0][0] && x <= w[1][0]).unwrap_or(0);
        let (a, b) = (pts[j], pts[(j + 1).min(pts.len() - 1)]);
        let span = b[0] - a[0];
        let t = if span > 0.0 { (x - a[0]) / span } else { 0.0 };
        (a[1] + (b[1] - a[1]) * t).clamp(0.0, 1.0)
    })
}

/// A gradient: colour stops at positions in [0, 1], sorted by position.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientStops {
    pub stops: Vec<(f32, [f32; 3])>,
}

/// One gradient stop: a position in [0, 1] and a colour.
pub type Stop = (f32, [f32; 3]);

/// A named preset gradient.
pub type GradientPreset = (&'static str, &'static [Stop]);

/// Preset gradients for the gradient map. Each entry is a name and its stops.
pub const GRADIENT_PRESETS: &[GradientPreset] = &[
    ("Grayscale", &[(0.0, [0.0, 0.0, 0.0]), (1.0, [1.0, 1.0, 1.0])]),
    ("Sepia", &[(0.0, [0.08, 0.04, 0.02]), (0.5, [0.55, 0.40, 0.27]), (1.0, [1.0, 0.95, 0.84])]),
    ("Duotone", &[(0.0, [0.05, 0.10, 0.35]), (1.0, [1.0, 0.62, 0.22])]),
    ("Neon", &[(0.0, [0.05, 0.0, 0.2]), (0.5, [0.9, 0.0, 0.6]), (1.0, [0.2, 1.0, 1.0])]),
];

impl Default for GradientStops {
    fn default() -> Self {
        Self::preset(0)
    }
}

impl GradientStops {
    /// The stops of preset `i` (wraps around the preset list).
    pub fn preset(i: usize) -> Self {
        let (_, stops) = GRADIENT_PRESETS[i % GRADIENT_PRESETS.len()];
        Self { stops: stops.to_vec() }
    }

    /// The 256-entry table indexed by luma (entry `i` is at `i / 255`), linear between stops and
    /// held at the end colours outside them.
    pub fn table(&self) -> Vec<[f32; 3]> {
        let mut stops = self.stops.clone();
        if stops.is_empty() {
            stops = vec![(0.0, [0.0; 3]), (1.0, [1.0; 3])];
        }
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        (0..256)
            .map(|i| {
                let t = i as f32 / 255.0;
                if t <= stops[0].0 {
                    return stops[0].1;
                }
                let last = stops[stops.len() - 1];
                if t >= last.0 {
                    return last.1;
                }
                let j = stops.windows(2).position(|w| t >= w[0].0 && t <= w[1].0).unwrap_or(0);
                let (a, b) = (stops[j], stops[(j + 1).min(stops.len() - 1)]);
                let span = b.0 - a.0;
                let u = if span > 0.0 { (t - a.0) / span } else { 0.0 };
                std::array::from_fn(|k| a.1[k] + (b.1[k] - a.1[k]) * u)
            })
            .collect()
    }
}
