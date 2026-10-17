//! Symmetry guides: vertical, horizontal, quadrant and radial.
//!
//! A symmetry produces a fixed number of copies of every point. Copy 0 is always the
//! point itself. The copy count is the number of strokes that guided layer outputs.
//!
//! Mirror options:
//! - `Quadrant { mirrored: true }`: reflections across both axes.
//!   `Quadrant { mirrored: false }`: 90-degree rotations.
//! - `Radial { mirrored: false }`: N rotations by `2*pi*k/N`.
//! - `Radial { mirrored: true }`: copy `k` is rotated by `2*pi*k/N`, and odd copies are
//!   first reflected across the horizontal axis through the centre. With N = 2 this is
//!   a vertical mirror. In general this is a kaleidoscope, not an exact dihedral group
//!   when N is odd.

use std::f64::consts::{PI, TAU};

use crate::geom::{line_through, ray_from, Rect, Sample, Segment, Vec2};
use crate::guide::{GuideRender, GuideStyle};

/// The kind of mirror symmetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymmetryKind {
    /// Mirror across the vertical axis through the centre (2 copies).
    Vertical,
    /// Mirror across the horizontal axis through the centre (2 copies).
    Horizontal,
    /// Both axes (4 copies). `mirrored` picks reflections or 90-degree rotations.
    Quadrant { mirrored: bool },
    /// `segments` copies around the centre. See the module docs for `mirrored`.
    Radial { segments: u32, mirrored: bool },
}

/// A symmetry guide with its centre.
#[derive(Debug, Clone, PartialEq)]
pub struct Symmetry {
    pub kind: SymmetryKind,
    pub centre: Vec2,
    pub style: GuideStyle,
}

impl Symmetry {
    pub fn new(kind: SymmetryKind, centre: Vec2) -> Self {
        Self {
            kind,
            centre,
            style: GuideStyle::default(),
        }
    }

    /// Number of copies each point produces.
    pub fn copy_count(&self) -> usize {
        match self.kind {
            SymmetryKind::Vertical | SymmetryKind::Horizontal => 2,
            SymmetryKind::Quadrant { .. } => 4,
            SymmetryKind::Radial { segments, .. } => segments.max(1) as usize,
        }
    }

    /// All copies of `p`. The first entry is `p` itself. The length is [`copy_count`](Self::copy_count).
    pub fn transform(&self, p: Vec2) -> Vec<Vec2> {
        let d = p - self.centre;
        let copies: Vec<Vec2> = match self.kind {
            SymmetryKind::Vertical => vec![d, Vec2::new(-d.x, d.y)],
            SymmetryKind::Horizontal => vec![d, Vec2::new(d.x, -d.y)],
            SymmetryKind::Quadrant { mirrored: true } => {
                vec![
                    d,
                    Vec2::new(-d.x, d.y),
                    Vec2::new(d.x, -d.y),
                    Vec2::new(-d.x, -d.y),
                ]
            }
            SymmetryKind::Quadrant { mirrored: false } => vec![
                d,
                d.rotate(PI / 2.0),
                d.rotate(PI),
                d.rotate(3.0 * PI / 2.0),
            ],
            SymmetryKind::Radial { segments, mirrored } => {
                let n = segments.max(1);
                (0..n)
                    .map(|k| {
                        let base = if mirrored && k % 2 == 1 {
                            Vec2::new(d.x, -d.y)
                        } else {
                            d
                        };
                        base.rotate(TAU * f64::from(k) / f64::from(n))
                    })
                    .collect()
            }
        };
        copies.into_iter().map(|c| c + self.centre).collect()
    }

    /// Mirrors whole samples. Output `k` is the stroke made from copy `k` of every sample.
    /// Pressure is kept. Each output has `samples.len()` entries.
    pub fn apply(&self, samples: &[Sample]) -> Vec<Vec<Sample>> {
        let mut out = vec![Vec::with_capacity(samples.len()); self.copy_count()];
        for sample in samples {
            for (stroke, pos) in out.iter_mut().zip(self.transform(sample.pos)) {
                stroke.push(sample.with_pos(pos));
            }
        }
        out
    }
}

impl GuideRender for Symmetry {
    fn style(&self) -> GuideStyle {
        self.style
    }

    fn lines(&self, viewport: Rect) -> Vec<Segment> {
        let c = self.centre;
        let vertical = || line_through(c, Vec2::new(0.0, 1.0), viewport);
        let horizontal = || line_through(c, Vec2::new(1.0, 0.0), viewport);
        match self.kind {
            SymmetryKind::Vertical => vertical().into_iter().collect(),
            SymmetryKind::Horizontal => horizontal().into_iter().collect(),
            SymmetryKind::Quadrant { .. } => vertical().into_iter().chain(horizontal()).collect(),
            SymmetryKind::Radial { segments, mirrored } => {
                let n = segments.max(1);
                let nf = f64::from(n);
                (0..n)
                    .filter_map(|k| {
                        let kf = f64::from(k);
                        if mirrored {
                            // Mirror axes sit every PI/N radians.
                            line_through(c, Vec2::from_angle(PI * kf / nf), viewport)
                        } else {
                            ray_from(c, Vec2::from_angle(TAU * kf / nf), viewport)
                        }
                    })
                    .collect()
            }
        }
    }
}
