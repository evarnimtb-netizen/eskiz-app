//! Colour-disc mapping for the UI's colour disc.
//!
//! The disc has an outer hue ring and an inner disc:
//! - **Hue ring**: the polar angle of a point gives its hue.
//! - **Inner disc**: the angle gives the hue and the radius gives saturation, from 0 at
//!   the centre (white) to 1 at the inner rim. Brightness is not part of the disc. The
//!   UI sets it with a separate slider and passes it to [`ColourDisc::colour_at`].
//!
//! This is the "simpler acceptable mapping" from the design notes: the disc is a hue and
//! saturation picker, and brightness is a slider. Geometry is in disc-local
//! coordinates, with the centre at the origin and y down. The angle is
//! `atan2(y, x)` in degrees, so hue increases clockwise on screen.

use crate::colour::Hsb;
use crate::geom::Vec2;

/// Default inner-disc radius as a fraction of the outer radius.
pub const DEFAULT_INNER_RATIO: f64 = 0.7;

/// Geometry of the colour disc.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColourDisc {
    /// Outer radius of the hue ring, in disc-local units (pixels in the UI).
    pub outer_radius: f64,
    /// Inner-disc radius as a fraction of `outer_radius`, in `(0, 1)`.
    pub inner_ratio: f64,
}

/// What a point on the disc refers to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiscHit {
    /// On the hue ring.
    Ring { hue: f64 },
    /// On the inner disc.
    Inner { hue: f64, saturation: f64 },
    /// Outside the disc.
    Outside,
}

/// Polar angle of `p` in degrees, in `[0, 360)`.
pub fn angle_of(p: Vec2) -> f64 {
    p.y.atan2(p.x).to_degrees().rem_euclid(360.0)
}

impl ColourDisc {
    /// A disc of the given outer radius with the default inner ratio.
    pub fn new(outer_radius: f64) -> Self {
        Self {
            outer_radius,
            inner_ratio: DEFAULT_INNER_RATIO,
        }
    }

    /// Radius of the inner disc.
    pub fn inner_radius(&self) -> f64 {
        self.outer_radius * self.inner_ratio
    }

    /// Radius at which hues sit on the ring (the middle of the ring).
    pub fn hue_ring_radius(&self) -> f64 {
        (self.outer_radius + self.inner_radius()) / 2.0
    }

    /// Classifies a disc-local point.
    pub fn hit_test(&self, p: Vec2) -> DiscHit {
        let r = p.length();
        let inner = self.inner_radius();
        if r <= inner {
            let saturation = if inner > 0.0 {
                (r / inner).min(1.0)
            } else {
                0.0
            };
            DiscHit::Inner {
                hue: angle_of(p),
                saturation,
            }
        } else if r <= self.outer_radius {
            DiscHit::Ring { hue: angle_of(p) }
        } else {
            DiscHit::Outside
        }
    }

    /// Where a hue sits on the ring.
    pub fn hue_point(&self, hue: f64) -> Vec2 {
        Vec2::from_angle(hue.to_radians()) * self.hue_ring_radius()
    }

    /// Where a colour's hue and saturation sit on the inner disc. Brightness is ignored.
    /// Grey colours (saturation 0) map to the centre, and their hue is lost.
    pub fn point_for(&self, colour: Hsb) -> Vec2 {
        Vec2::from_angle(colour.h.to_radians()) * (colour.s * self.inner_radius())
    }

    /// The colour under `p` at the given brightness. Inner-disc points give their hue
    /// and saturation. Ring points give their hue at full saturation. Outside points
    /// give `None`.
    pub fn colour_at(&self, p: Vec2, brightness: f64) -> Option<Hsb> {
        match self.hit_test(p) {
            DiscHit::Inner { hue, saturation } => Some(Hsb::new(hue, saturation, brightness)),
            DiscHit::Ring { hue } => Some(Hsb::new(hue, 1.0, brightness)),
            DiscHit::Outside => None,
        }
    }
}
