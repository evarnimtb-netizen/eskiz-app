//! Guided Layer: snaps a stroke to the active guide, then mirrors it through the
//! active symmetry.
//!
//! Snapping per guide:
//! - [`SnapGuide::Grid`]: each sample moves to the nearest grid intersection. There is
//!   no lock.
//! - [`SnapGuide::Isometric`] and [`SnapGuide::Perspective`]: the stroke becomes a
//!   straight line through the stroke's first sample (the anchor). Samples closer
//!   than `lock_distance` to the anchor pass through unchanged. Once the drag reaches
//!   that distance, one direction is chosen and locked for the rest of the stroke:
//!   the nearest isometric axis, or the line to the vanishing point that best matches
//!   the drag. Later samples are projected onto that line. If no direction can be
//!   chosen, the stroke stays unsnapped.
//!
//! Symmetry runs after snapping, so copies are mirrors of the snapped stroke.

use std::borrow::Cow;

use crate::geom::{Rect, Sample, Segment, Vec2};
use crate::grid::{Grid, Isometric};
use crate::guide::{GuideRender, GuideStyle};
use crate::perspective::Perspective;
use crate::symmetry::Symmetry;

/// Default distance, in canvas pixels, a drag must travel before its direction locks.
pub const DEFAULT_LOCK_DISTANCE: f64 = 16.0;

/// The guide that stroke samples snap to.
#[derive(Debug, Clone, PartialEq)]
pub enum SnapGuide {
    Grid(Grid),
    Isometric(Isometric),
    Perspective(Perspective),
}

impl GuideRender for SnapGuide {
    fn style(&self) -> GuideStyle {
        match self {
            SnapGuide::Grid(g) => g.style(),
            SnapGuide::Isometric(g) => g.style(),
            SnapGuide::Perspective(g) => g.style(),
        }
    }

    fn lines(&self, viewport: Rect) -> Vec<Segment> {
        match self {
            SnapGuide::Grid(g) => g.lines(viewport),
            SnapGuide::Isometric(g) => g.lines(viewport),
            SnapGuide::Perspective(g) => g.lines(viewport),
        }
    }
}

/// Guided Layer settings: an optional snap guide and an optional symmetry.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawingAssist {
    pub snap: Option<SnapGuide>,
    pub symmetry: Option<Symmetry>,
    /// Drag distance, in pixels, before the isometric or perspective direction locks.
    pub lock_distance: f64,
}

impl Default for DrawingAssist {
    /// No guide, no symmetry, and [`DEFAULT_LOCK_DISTANCE`].
    fn default() -> Self {
        Self {
            snap: None,
            symmetry: None,
            lock_distance: DEFAULT_LOCK_DISTANCE,
        }
    }
}

impl DrawingAssist {
    /// Number of strokes output for each input stroke: the symmetry copy count, or 1.
    pub fn output_count(&self) -> usize {
        self.symmetry.as_ref().map_or(1, Symmetry::copy_count)
    }

    /// Starts an incremental stroke. Use this to get stable output while a stroke is
    /// still being drawn.
    pub fn begin_stroke(&self) -> StrokeAssist<'_> {
        StrokeAssist {
            assist: Cow::Borrowed(self),
            anchor: None,
            lock: Lock::Pending,
        }
    }

    /// Like [`begin_stroke`](Self::begin_stroke), but the stroke keeps its own copy of the
    /// settings, so it can be stored across frames (for example by a stroke engine).
    pub fn begin_stroke_owned(&self) -> StrokeAssist<'static> {
        StrokeAssist {
            assist: Cow::Owned(self.clone()),
            anchor: None,
            lock: Lock::Pending,
        }
    }

    /// Assists a whole stroke. Returns one `Vec<Sample>` per output stroke, in symmetry
    /// copy order. The result is the same as pushing the samples one at a time through
    /// [`begin_stroke`](Self::begin_stroke).
    pub fn assist(&self, samples: &[Sample]) -> Vec<Vec<Sample>> {
        let mut stroke = self.begin_stroke();
        let mut out: Vec<Vec<Sample>> = (0..self.output_count())
            .map(|_| Vec::with_capacity(samples.len()))
            .collect();
        for &sample in samples {
            for (dst, guided) in out.iter_mut().zip(stroke.push(sample)) {
                dst.push(guided);
            }
        }
        out
    }
}

/// The direction lock for the current stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Lock {
    /// Still inside the lock distance, or the drag has no direction yet.
    Pending,
    /// Locked to the line through the anchor with this unit direction.
    Axis(Vec2),
    /// Tried to lock but found no usable direction. Samples pass through unchanged.
    Free,
}

/// State for one stroke, so output stays stable while the stroke grows.
#[derive(Debug, Clone)]
pub struct StrokeAssist<'a> {
    assist: Cow<'a, DrawingAssist>,
    anchor: Option<Vec2>,
    lock: Lock,
}

impl StrokeAssist<'_> {
    /// Assists one more sample. Returns one sample per output stroke.
    pub fn push(&mut self, sample: Sample) -> Vec<Sample> {
        let anchor = *self.anchor.get_or_insert(sample.pos);
        let pos = self.snap_point(anchor, sample.pos);
        let guided = sample.with_pos(pos);
        match &self.assist.symmetry {
            None => vec![guided],
            Some(symmetry) => symmetry
                .transform(pos)
                .into_iter()
                .map(|p| guided.with_pos(p))
                .collect(),
        }
    }

    fn snap_point(&mut self, anchor: Vec2, p: Vec2) -> Vec2 {
        let assist: &DrawingAssist = &self.assist;
        match &assist.snap {
            None => p,
            Some(SnapGuide::Grid(grid)) => grid.snap(p),
            Some(guide) => {
                let drag = p - anchor;
                if self.lock == Lock::Pending
                    && drag.length() >= assist.lock_distance
                    && drag.normalised().is_some()
                {
                    let choice = match guide {
                        SnapGuide::Isometric(iso) => drag.normalised().map(|d| iso.nearest_axis(d)),
                        SnapGuide::Perspective(persp) => persp.best_direction(anchor, drag),
                        SnapGuide::Grid(_) => None,
                    };
                    self.lock = choice.map_or(Lock::Free, Lock::Axis);
                }
                match self.lock {
                    Lock::Axis(dir) => anchor + dir * drag.dot(dir),
                    Lock::Pending | Lock::Free => p,
                }
            }
        }
    }
}
