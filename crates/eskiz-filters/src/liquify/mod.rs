//! Liquify: a displacement field built from brush particles, previewed while
//! painting and resampled into the layer on commit.
//!
//! Follows the documented section 19 maths, with the Eskiz decisions of section 19A taking
//! precedence over the open items of section 19. Those decisions are: labelled directions for
//! every mode, a per-stroke deposit cap of 0.5 R, an accumulated-field cap of R, and momentum
//! default 0. The field runs at layer resolution (scale 1).

mod field;
mod particles;

pub use field::{cap_stroke_deposit, resolve, Accumulator, Field};
pub use particles::{
    base_e, displacement, emit_segment, is_dead, simulate, EmitContext, Particle, ParticleKind,
};

use crate::colour::ease_in_out;
use crate::image::{mask_at, mix_px, Image};
use crate::random::Rng;

/// Idle tick length in seconds.
pub const IDLE_DT: f32 = 1.0 / 120.0;
/// Decay base and tick length.
pub const DECAY_BASE: f32 = 0.968;
pub const DECAY_TICK: f32 = 0.008;
/// Particles whose decay falls below this stop.
pub const DECAY_FLOOR: f32 = 1e-5;
/// Smoothed-velocity rate.
pub const SMOOTHING_RATE: f32 = 40.0;
/// Reset fade duration in seconds.
pub const RESET_SECONDS: f32 = 0.3;
/// Added to momentum in the life and decay formulas.
pub const MOMENTUM_EPS: f32 = 0.001;
/// Safety cap on drain ticks.
pub const MAX_DRAIN_TICKS: usize = 10_000;
/// Per-stroke deposit cap as a fraction of R (§19A).
pub const STROKE_CAP_FRACTION: f32 = 0.5;
/// Accumulated-field cap as a fraction of R (§19A).
pub const FIELD_CAP_FRACTION: f32 = 1.0;

/// Liquify brush behaviours. Adjust is an action on the field.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Push,
    TwirlRight,
    TwirlLeft,
    Pinch,
    Expand,
    Crystals,
    Edge,
    Reconstruct,
}

/// Liquify settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub mode: Mode,
    /// Relative size in [0, 1]. Default 0.5 (placeholder).
    pub size: f32,
    /// Relative pressure in [0, 1]. Default 1.
    pub pressure: f32,
    /// Distortion, in [0, 1]. Default 0.
    pub distortion: f32,
    /// Momentum in [0, 1]. Default 0.
    pub momentum: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: Mode::Push,
            size: 0.5,
            pressure: 1.0,
            distortion: 0.0,
            momentum: 0.0,
        }
    }
}

/// One pen sample: position in layer pixels and normalised pressure.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeSample {
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// Brush radius `R = 1596 s^2.2 + 4` in work-space pixels.
pub fn radius(size: f32) -> f32 {
    1596.0 * size.clamp(0.0, 1.0).powf(2.2) + 4.0
}

/// Pressure factor `P = 0.99 p + 0.01`.
pub fn pressure_factor(pressure: f32) -> f32 {
    0.99 * pressure.clamp(0.0, 1.0) + 0.01
}

/// Quadratic ease-in-out.
pub fn ease(t: f32) -> f32 {
    ease_in_out(t)
}

/// Falloff `w = (1 - ease(sat(max(d_min, |d|) / R))) A`.
pub fn falloff(dist: f32, radius: f32, amount: f32, d_min: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    (1.0 - ease((dist.max(d_min) / radius).clamp(0.0, 1.0))) * amount
}

/// Ordinary particle decay `0.968^((dt/0.008)/(m + 0.001))`, set to 0 below `1e-5`.
pub fn decay_ordinary(dt: f32, momentum: f32) -> f32 {
    let d = DECAY_BASE.powf((dt / DECAY_TICK) / (momentum + MOMENTUM_EPS));
    if d < DECAY_FLOOR {
        0.0
    } else {
        d
    }
}

/// Wake decay `0.968^((dt/0.008) C)`.
pub fn decay_wake(dt: f32, c: f32) -> f32 {
    let d = DECAY_BASE.powf((dt / DECAY_TICK) * c);
    if d < DECAY_FLOOR {
        0.0
    } else {
        d
    }
}

/// Smoothing rate `min(40 dt, 1)`.
pub fn smoothing_alpha(dt: f32) -> f32 {
    (SMOOTHING_RATE * dt).min(1.0)
}

/// Smoothed velocity update `v <- v + alpha (v_measured - v)`.
pub fn smooth_velocity(smoothed: [f32; 2], measured: [f32; 2], dt: f32) -> [f32; 2] {
    let a = smoothing_alpha(dt);
    [
        smoothed[0] + a * (measured[0] - smoothed[0]),
        smoothed[1] + a * (measured[1] - smoothed[1]),
    ]
}

/// Particle spacing `max(1, G R 0.0425)` with G = 1.
/// It sets the count only and does not multiply the amount.
pub fn spacing(radius: f32) -> f32 {
    (radius * 0.0425).max(1.0)
}

/// Reconstruct attenuation `0.75^(r 25 m)`.
pub fn reconstruct_attenuation(weight: f32, mask: f32) -> f32 {
    0.75f32.powf(weight * 25.0 * mask)
}

/// Reset fade magnitude `M_start (1 - progress / D)`, zero after `D`.
pub fn fade_magnitude(start: f32, elapsed: f32) -> f32 {
    let progress = (elapsed / RESET_SECONDS).clamp(0.0, 1.0);
    start * (1.0 - progress)
}

#[derive(Clone, Copy, Debug)]
struct Fade {
    start: f32,
    elapsed: f32,
}

/// Liquify state for one layer: the accurate field, the particles of the current stroke, the
/// per-stroke deposit total and the adjust magnitude.
#[derive(Clone, Debug)]
pub struct Liquify {
    width: usize,
    height: usize,
    /// Settings used for the next stroke.
    pub settings: Settings,
    field: Field,
    snapshot: Option<Field>,
    particles: Vec<Particle>,
    stroke_deposit: Vec<[f32; 2]>,
    rng: Rng,
    stroke_count: u64,
    last: Option<StrokeSample>,
    magnitude: f32,
    fade: Option<Fade>,
}

impl Liquify {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            settings: Settings::default(),
            field: Field::new(width, height),
            snapshot: None,
            particles: Vec::new(),
            stroke_deposit: vec![[0.0; 2]; width * height],
            rng: Rng::new(0),
            stroke_count: 0,
            last: None,
            magnitude: 1.0,
            fade: None,
        }
    }

    pub fn field(&self) -> &Field {
        &self.field
    }

    /// Mutable access to the accurate field (used for tests and for loading a stored field).
    pub fn field_mut(&mut self) -> &mut Field {
        &mut self.field
    }

    /// Offset deposited at each texel during the current stroke, before any fade (§19A cap).
    pub fn stroke_deposit(&self) -> &[[f32; 2]] {
        &self.stroke_deposit
    }

    pub fn particle_count(&self) -> usize {
        self.particles.len()
    }

    /// Adjust magnitude in [0, 1]. Default 1.
    pub fn magnitude(&self) -> f32 {
        self.magnitude
    }

    pub fn set_magnitude(&mut self, m: f32) {
        self.magnitude = m.clamp(0.0, 1.0);
    }

    /// Magnitude shown in the preview: the fading value during a reset, otherwise the magnitude.
    pub fn preview_magnitude(&self) -> f32 {
        match self.fade {
            Some(f) => fade_magnitude(f.start, f.elapsed),
            None => self.magnitude,
        }
    }

    pub fn is_fading(&self) -> bool {
        self.fade.is_some()
    }

    /// Begin a stroke: snapshot the field, clear particles and per-stroke deposits, and seed the
    /// random source per stroke so that replay is deterministic.
    pub fn begin_stroke(&mut self) {
        self.snapshot = Some(self.field.clone());
        self.particles.clear();
        self.stroke_deposit.iter_mut().for_each(|v| *v = [0.0; 2]);
        self.last = None;
        self.rng = Rng::new(self.stroke_count);
        self.stroke_count += 1;
    }

    /// Add a pen sample taken `dt` seconds after the previous one. Particles are emitted for the
    /// segment from the previous sample. The first sample has no segment, so it
    /// emits nothing and only starts the segment.
    pub fn add_sample(&mut self, s: StrokeSample, dt: f32) {
        if let Some(prev) = self.last {
            let ctx = EmitContext {
                radius: radius(self.settings.size),
                pressure: pressure_factor(self.settings.pressure),
                chaos: self.settings.distortion.clamp(0.0, 1.0),
                dt,
            };
            emit_segment(
                &ctx,
                self.settings.mode,
                prev,
                s,
                &mut self.rng,
                &mut self.particles,
            );
        }
        self.last = Some(s);
    }

    /// One simulation tick: simulate particles, rasterise them, apply the per-stroke cap, resolve
    /// the field and remove dead particles.
    pub fn tick(&mut self, dt: f32) {
        if self.particles.is_empty() {
            return;
        }
        let momentum = self.settings.momentum.clamp(0.0, 1.0);
        for p in &mut self.particles {
            simulate(p, dt, momentum);
        }
        let mut acc = Accumulator::new(self.width, self.height);
        for p in &self.particles {
            acc.splat(p);
        }
        let r = radius(self.settings.size);
        cap_stroke_deposit(&mut acc, &mut self.stroke_deposit, STROKE_CAP_FRACTION * r);
        self.field = resolve(&self.field, &acc, None, FIELD_CAP_FRACTION * r);
        self.particles.retain(|p| !is_dead(p));
    }

    /// Run idle ticks at `IDLE_DT` until no particles remain.
    pub fn drain(&mut self) {
        let mut n = 0;
        while !self.particles.is_empty() && n < MAX_DRAIN_TICKS {
            self.tick(IDLE_DT);
            n += 1;
        }
    }

    /// End a stroke: drain the particles and drop the snapshot.
    pub fn end_stroke(&mut self) {
        self.drain();
        self.snapshot = None;
        self.last = None;
    }

    /// Cancel a stroke: discard its particles and restore the field from before the stroke.
    pub fn cancel_stroke(&mut self) {
        self.particles.clear();
        self.stroke_deposit.iter_mut().for_each(|v| *v = [0.0; 2]);
        if let Some(snap) = self.snapshot.take() {
            self.field = snap;
        }
        self.last = None;
    }

    /// Bake the magnitude into the field when the adjust slider is released.
    pub fn bake(&mut self) {
        if self.magnitude < 1.0 {
            self.field.scale(self.magnitude);
            self.magnitude = 1.0;
        }
    }

    /// Start the reset fade from the current magnitude. The field is zeroed.
    pub fn reset(&mut self) {
        self.particles.clear();
        self.field.zero();
        self.fade = Some(Fade {
            start: self.magnitude,
            elapsed: 0.0,
        });
    }

    /// Advance the reset fade. When it completes, the magnitude returns to 1 (idle).
    pub fn advance_fade(&mut self, dt: f32) {
        if let Some(f) = &mut self.fade {
            f.elapsed += dt;
            if f.elapsed >= RESET_SECONDS {
                self.fade = None;
                self.magnitude = 1.0;
            }
        }
    }

    /// Commit: resample the layer through the field:
    /// `out(p) = mix(layer(p + magnitude field(p)), layer(p), mask(p))`, with bilinear sampling.
    pub fn commit(&self, layer: &Image, mask: Option<&[f32]>) -> Image {
        let m = self.magnitude;
        Image::generate(layer.width, layer.height, |x, y| {
            let i = y * layer.width + x;
            let f = self.field.sample_texel(x as f32, y as f32);
            let sampled = layer.sample_texel(x as f32 + m * f[0], y as f32 + m * f[1]);
            mix_px(layer.px(x, y), sampled, mask_at(mask, i))
        })
    }
}
