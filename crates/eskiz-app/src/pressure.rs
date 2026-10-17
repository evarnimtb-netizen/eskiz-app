//! The global pressure curve.
//!
//! An editable spline through four control points. The two end points are fixed at (0, 0)
//! and (1, 1); the two inner points are editable. Input is raw pressure (touch force or pen
//! pressure), output is effective pressure. The default curve is the identity.
//!
//! The spline is a monotone cubic Hermite interpolant (Fritsch-Carlson), so a curve whose
//! control points rise left to right never overshoots or reverses: more pressure never
//! gives less effect. The editor keeps the inner points ordered so that holds by
//! construction.

/// Smallest horizontal gap between neighbouring control points.
pub const MIN_GAP: f32 = 0.02;

/// Four control points: fixed (0, 0), two editable inner points, fixed (1, 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PressureCurve {
    mid: [[f32; 2]; 2],
}

impl Default for PressureCurve {
    /// The identity: inner points sit on the diagonal.
    fn default() -> Self {
        Self {
            mid: [[1.0 / 3.0, 1.0 / 3.0], [2.0 / 3.0, 2.0 / 3.0]],
        }
    }
}

impl PressureCurve {
    /// Build from the two inner points, clamping them into a valid, ordered position.
    /// Non-finite input gives the identity.
    pub fn from_mid(mid: [[f32; 2]; 2]) -> Self {
        if mid.iter().flatten().any(|v| !v.is_finite()) {
            return Self::default();
        }
        let mut c = Self { mid };
        c.set_point(0, mid[0][0], mid[0][1]);
        c.set_point(1, mid[1][0], mid[1][1]);
        c.set_point(0, c.mid[0][0], c.mid[0][1]);
        c
    }

    /// The four control points in order, end points included.
    pub fn points(&self) -> [[f32; 2]; 4] {
        [[0.0, 0.0], self.mid[0], self.mid[1], [1.0, 1.0]]
    }

    /// The editable inner points.
    pub fn mid(&self) -> [[f32; 2]; 2] {
        self.mid
    }

    /// Move inner point `i` (0 or 1), clamped so the points stay ordered in x and
    /// non-decreasing in y (which keeps the curve monotonic).
    pub fn set_point(&mut self, i: usize, x: f32, y: f32) {
        if i > 1 || !x.is_finite() || !y.is_finite() {
            return;
        }
        let (x_lo, x_hi, y_lo, y_hi) = if i == 0 {
            (MIN_GAP, self.mid[1][0] - MIN_GAP, 0.0, self.mid[1][1])
        } else {
            (self.mid[0][0] + MIN_GAP, 1.0 - MIN_GAP, self.mid[0][1], 1.0)
        };
        self.mid[i] = [x.clamp(x_lo, x_hi.max(x_lo)), y.clamp(y_lo, y_hi.max(y_lo))];
    }

    /// True when the curve is (numerically) the identity.
    pub fn is_identity(&self) -> bool {
        self.points().iter().all(|p| (p[0] - p[1]).abs() < 1e-4)
    }

    /// Effective pressure for raw pressure `x`. Input is clamped to 0..1.
    pub fn eval(&self, x: f32) -> f32 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let p = self.points();
        let secant: [f32; 3] = std::array::from_fn(|k| (p[k + 1][1] - p[k][1]) / (p[k + 1][0] - p[k][0]));
        // Tangents: end tangents follow the end secants, inner ones average (zero when the
        // neighbouring secants disagree in sign, or either is flat).
        let mut m = [secant[0], 0.0, 0.0, secant[2]];
        for k in 1..3 {
            let (a, b) = (secant[k - 1], secant[k]);
            m[k] = if a * b > 0.0 { (a + b) * 0.5 } else { 0.0 };
        }
        // Fritsch-Carlson limiter keeps each segment monotonic.
        for k in 0..3 {
            if secant[k] == 0.0 {
                m[k] = 0.0;
                m[k + 1] = 0.0;
                continue;
            }
            let (a, b) = (m[k] / secant[k], m[k + 1] / secant[k]);
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                m[k] = t * a * secant[k];
                m[k + 1] = t * b * secant[k];
            }
        }
        let seg = if x <= p[1][0] {
            0
        } else if x <= p[2][0] {
            1
        } else {
            2
        };
        let (x0, x1) = (p[seg][0], p[seg + 1][0]);
        let h = x1 - x0;
        let t = ((x - x0) / h).clamp(0.0, 1.0);
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p[seg][1]
            + (t3 - 2.0 * t2 + t) * h * m[seg]
            + (-2.0 * t3 + 3.0 * t2) * p[seg + 1][1]
            + (t3 - t2) * h * m[seg + 1];
        y.clamp(0.0, 1.0)
    }

    /// `x1,y1,x2,y2` for [`crate::app::Prefs::encode`].
    pub fn encode(&self) -> String {
        format!("{},{},{},{}", self.mid[0][0], self.mid[0][1], self.mid[1][0], self.mid[1][1])
    }

    /// Parse [`PressureCurve::encode`] output. `None` when malformed.
    pub fn decode(text: &str) -> Option<Self> {
        let v: Vec<f32> = text.split(',').map(|s| s.trim().parse::<f32>()).collect::<Result<_, _>>().ok()?;
        if v.len() != 4 || v.iter().any(|x| !x.is_finite()) {
            return None;
        }
        Some(Self::from_mid([[v[0], v[1]], [v[2], v[3]]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_identity() {
        let c = PressureCurve::default();
        assert!(c.is_identity());
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            assert!((c.eval(x) - x).abs() < 1e-4, "x={x} -> {}", c.eval(x));
        }
    }

    #[test]
    fn endpoints_are_fixed() {
        let c = PressureCurve::from_mid([[0.1, 0.6], [0.5, 0.7]]);
        assert!(c.eval(0.0).abs() < 1e-6);
        assert!((c.eval(1.0) - 1.0).abs() < 1e-6);
        assert_eq!(c.eval(-3.0), 0.0);
        assert_eq!(c.eval(7.0), 1.0);
        assert_eq!(c.eval(f32::NAN), 0.0);
    }

    #[test]
    fn passes_through_control_points() {
        let c = PressureCurve::from_mid([[0.1230, 0.2803], [0.5752, 0.8797]]);
        for p in c.points() {
            assert!((c.eval(p[0]) - p[1]).abs() < 1e-4, "{p:?} -> {}", c.eval(p[0]));
        }
    }

    #[test]
    fn monotonic_for_many_valid_curves() {
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..200 {
            let c = PressureCurve::from_mid([[rnd(), rnd()], [rnd(), rnd()]]);
            let mut last = -1.0;
            for i in 0..=400 {
                let y = c.eval(i as f32 / 400.0);
                assert!(y >= last - 1e-5, "{c:?} dips at {i}: {y} < {last}");
                assert!((0.0..=1.0).contains(&y));
                last = y;
            }
        }
    }

    #[test]
    fn dragging_a_point_changes_the_curve() {
        let mut c = PressureCurve::default();
        let before = c.eval(0.3);
        c.set_point(0, 0.2, 0.5);
        assert!(c.eval(0.3) > before + 0.05);
        assert!(!c.is_identity());
    }

    #[test]
    fn points_stay_ordered() {
        let mut c = PressureCurve::default();
        c.set_point(0, 0.9, 0.9);
        let p = c.points();
        assert!(p[1][0] < p[2][0] && p[1][1] <= p[2][1]);
        c.set_point(1, 0.0, 0.0);
        let p = c.points();
        assert!(p[1][0] < p[2][0] && p[1][1] <= p[2][1]);
        c.set_point(0, f32::NAN, 0.5);
        assert_eq!(c.points(), p);
    }

    #[test]
    fn encode_round_trips_and_rejects_garbage() {
        let c = PressureCurve::from_mid([[0.123, 0.28], [0.575, 0.88]]);
        assert_eq!(PressureCurve::decode(&c.encode()), Some(c));
        assert_eq!(PressureCurve::decode("1,2,3"), None);
        assert_eq!(PressureCurve::decode("a,b,c,d"), None);
        assert_eq!(PressureCurve::decode("nan,0,1,1"), None);
    }
}
