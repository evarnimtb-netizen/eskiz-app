//! Particles: emission from stroke segments, simulation per tick,
//! and the displacement each particle contributes.
//!
//! Counts come from the spacing (`count = floor(L / spacing)`). Amounts are
//! `m x lerp(a, b, t) x f_type`, with the signed per-type factor `f_type` of §19.6. Directions
//! follow the Eskiz decisions of §19A.

use super::{decay_ordinary, decay_wake, spacing, Mode, StrokeSample, MOMENTUM_EPS};
use crate::random::Rng;
use std::f32::consts::TAU;

/// Particle kinds, numbered as in §19.2 and §19.6. Crystals are emitted as pinch particles
/// with jitter (§19A), so they have no kind of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    Push = 1,
    Reconstruct = 2,
    TwirlRight = 4,
    TwirlLeft = 5,
    Pinch = 6,
    Expand = 7,
    Wake = 9,
    Edge = 11,
}

impl ParticleKind {
    /// Field-kernel type number.
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Wake particles use the wake decay.
    pub fn is_wake_like(self) -> bool {
        self == ParticleKind::Wake
    }
}

/// One particle record. Cutoff and the per-record flag are not modelled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    pub pos: [f32; 2],
    pub prev: [f32; 2],
    pub vel: [f32; 2],
    pub life: f32,
    pub time: f32,
    pub radius: f32,
    /// Signed amount `m x lerp x f_type`.
    pub amount: f32,
    pub dir: [f32; 2],
    pub kind: ParticleKind,
    /// The stroke sample the particle came from. Pinch, expand and twirl act about this point
    /// (section 19A: twirl rotates content about the brush centre), not about the particle.
    pub centre: [f32; 2],
}

impl Particle {
    fn new(
        kind: ParticleKind,
        pos: [f32; 2],
        vel: [f32; 2],
        dir: [f32; 2],
        radius: f32,
        amount: f32,
    ) -> Self {
        Self {
            pos,
            prev: pos,
            vel,
            life: 1.0,
            time: 0.0,
            radius,
            amount,
            dir,
            kind,
            centre: pos,
        }
    }

    /// Set the point that pinch, expand and twirl act about.
    fn centred_on(mut self, c: [f32; 2]) -> Self {
        self.centre = c;
        self
    }
}

/// Offset contributed at point `p` by a particle at `c`, for falloff weight `w`.
/// Only the falloff shape is passed in; `amount` is the particle's signed amount. The offset
/// is backward: `out(p) = layer(p + offset)`.
///
/// - Push and wake: `-0.75 dir w amount`. A positive amount moves content along the stroke.
/// - Pinch and expand: `1.5 d w amount` with `d = p - c`. Pinch has a positive amount and pulls
///   content inwards. Expand has a negative amount and pushes content outwards.
/// - Twirl right and left: `4 w amount (d.y, -d.x)`. Right has a positive amount and left a
///   negative one. In screen coordinates (y down) a positive amount turns content clockwise.
/// - Edge: `1.5 w amount` times the perpendicular part of `d`, so content gathers onto the line.
/// - Reconstruct: no offset, only a weight.
///
/// These are the signed table of §19.6 with the sign conventions of §19A. The labelled
/// behaviour follows §19A; the table signs for expand and the twirls are not used.
pub fn displacement(
    kind: ParticleKind,
    p: [f32; 2],
    c: [f32; 2],
    dir: [f32; 2],
    w: f32,
    amount: f32,
) -> [f32; 2] {
    let d = [p[0] - c[0], p[1] - c[1]];
    let k = w * amount;
    match kind {
        ParticleKind::Push | ParticleKind::Wake => [-0.75 * dir[0] * k, -0.75 * dir[1] * k],
        ParticleKind::Pinch | ParticleKind::Expand => [1.5 * d[0] * k, 1.5 * d[1] * k],
        ParticleKind::TwirlRight | ParticleKind::TwirlLeft => [4.0 * d[1] * k, -4.0 * d[0] * k],
        ParticleKind::Edge => {
            let along = d[0] * dir[0] + d[1] * dir[1];
            let perp = [d[0] - along * dir[0], d[1] - along * dir[1]];
            [1.5 * perp[0] * k, 1.5 * perp[1] * k]
        }
        ParticleKind::Reconstruct => [0.0, 0.0],
    }
}

/// Base value `E` with the flag clear:
/// `E = max(T / count, L / (3000 count))`, where `T` is the tick time and `L` the segment length.
pub fn base_e(tick_dt: f32, seg_len: f32, count: usize) -> f32 {
    let n = count.max(1) as f32;
    (tick_dt / n).max(seg_len / (3000.0 * n))
}

/// Particle count along a segment: `floor(L / step)`, and 1 for a moving segment whose floor is
/// zero. Zero for a stationary pen.
fn count_along(len: f32, step: f32) -> usize {
    if len <= 0.0 {
        0
    } else {
        ((len / step).floor() as usize).max(1)
    }
}

/// Inputs shared by the emission routine.
#[derive(Clone, Copy, Debug)]
pub struct EmitContext {
    /// Brush radius R in pixels.
    pub radius: f32,
    /// Pressure factor P (multiplies each sample's pen force).
    pub pressure: f32,
    /// Distortion (chaos) in [0, 1].
    pub chaos: f32,
    /// Tick time T of the current sample.
    pub dt: f32,
}

/// Twirl ring and inner offsets.
const TWIRL_RING: f32 = 1.3 * 0.46;
const TWIRL_INNER: f32 = 0.75 * 0.66 * 0.46;

/// Emit particles for one stroke segment from `prev` to `cur`.
/// A stationary pen emits nothing, since the count is zero.
pub fn emit_segment(
    ctx: &EmitContext,
    mode: Mode,
    prev: StrokeSample,
    cur: StrokeSample,
    rng: &mut Rng,
    out: &mut Vec<Particle>,
) {
    let d = [cur.x - prev.x, cur.y - prev.y];
    let len = d[0].hypot(d[1]);
    if len <= 0.0 {
        return;
    }
    let dir = [d[0] / len, d[1] / len];
    let r = ctx.radius;
    let a0 = ctx.pressure * prev.pressure;
    let a1 = ctx.pressure * cur.pressure;
    let vel = if ctx.dt > 0.0 {
        [d[0] / ctx.dt, d[1] / ctx.dt]
    } else {
        [0.0, 0.0]
    };
    let ramp = |t: f32| a0 + (a1 - a0) * t;
    let point = |t: f32| [prev.x + d[0] * t, prev.y + d[1] * t];
    match mode {
        Mode::Push => {
            // Base particles: f = +L / count (type 1).
            let n = count_along(len, spacing(r));
            let f = len / n as f32;
            for k in 0..n {
                let t = (k + 1) as f32 / n as f32;
                out.push(Particle::new(
                    ParticleKind::Push,
                    point(t),
                    vel,
                    dir,
                    r,
                    ramp(t) * f,
                ));
            }
            // Wake particles: two per step, the second a sign-flipped twin (type 9, f = L / count).
            let steps = count_along(len, r * 0.0625);
            let count_w = 2 * steps;
            let fw = len / count_w as f32;
            for k in 0..steps {
                let t = (k + 1) as f32 / steps as f32;
                let jitter = wake_jitter(rng, ctx.chaos, r);
                let base = point(t);
                let pos = [base[0] + jitter[0], base[1] + jitter[1]];
                let amount = ramp(t) * fw;
                out.push(Particle::new(ParticleKind::Wake, pos, vel, dir, r, amount));
                out.push(Particle::new(
                    ParticleKind::Wake,
                    pos,
                    vel,
                    [-dir[0], -dir[1]],
                    r,
                    amount,
                ));
            }
        }
        Mode::Pinch | Mode::Expand => {
            // Base particle at the sample, f = +E (pinch, type 6) or -E (expand, type 7).
            let (kind, sign) = if mode == Mode::Pinch {
                (ParticleKind::Pinch, 1.0)
            } else {
                (ParticleKind::Expand, -1.0)
            };
            let e1 = base_e(ctx.dt, len, 1);
            out.push(Particle::new(
                kind,
                [cur.x, cur.y],
                vel,
                dir,
                r,
                sign * a1 * e1,
            ).centred_on([cur.x, cur.y]));
            if ctx.chaos > 0.0 {
                // Burst rings: 2 rings of 6 points at radii 0.5 R and R (placeholder, open).
                let eb = base_e(ctx.dt, len, 12);
                for &rad in &[0.5 * r, r] {
                    for k in 0..6 {
                        let th = TAU * k as f32 / 6.0;
                        let pos = [cur.x + rad * th.cos(), cur.y + rad * th.sin()];
                        out.push(Particle::new(kind, pos, vel, dir, r, sign * a1 * eb).centred_on([cur.x, cur.y]));
                    }
                }
            }
        }
        Mode::TwirlRight | Mode::TwirlLeft => {
            // 5 outer ring particles, each with 3 inner particles: 20 per sample.
            let (kind, sign) = if mode == Mode::TwirlRight {
                (ParticleKind::TwirlRight, 1.0)
            } else {
                (ParticleKind::TwirlLeft, -1.0)
            };
            let e = base_e(ctx.dt, len, 20);
            let amount = sign * a1 * e;
            for k in 0..5 {
                let th = TAU * k as f32 / 5.0;
                let outer = [
                    cur.x + TWIRL_RING * r * th.cos(),
                    cur.y + TWIRL_RING * r * th.sin(),
                ];
                out.push(Particle::new(kind, outer, vel, dir, r, amount).centred_on([cur.x, cur.y]));
                for j in 0..3 {
                    let phi = th + TAU * j as f32 / 3.0;
                    let inner = [
                        outer[0] + TWIRL_INNER * r * phi.cos(),
                        outer[1] + TWIRL_INNER * r * phi.sin(),
                    ];
                    out.push(Particle::new(kind, inner, vel, dir, r, amount).centred_on([cur.x, cur.y]));
                }
            }
        }
        Mode::Edge => {
            // Two particles at the sample, one per direction. f = L / count with count = 2.
            let f = len / 2.0;
            for (k, &sign) in [1.0f32, -1.0].iter().enumerate() {
                let t = (k + 1) as f32 / 2.0;
                let pdir = [dir[0] * sign, dir[1] * sign];
                out.push(Particle::new(
                    ParticleKind::Edge,
                    [cur.x, cur.y],
                    vel,
                    pdir,
                    r,
                    ramp(t) * f,
                ));
            }
        }
        Mode::Reconstruct => {
            // One particle, typed reconstruct (type 2), f = +E.
            let e1 = base_e(ctx.dt, len, 1);
            out.push(Particle::new(
                ParticleKind::Reconstruct,
                [cur.x, cur.y],
                vel,
                dir,
                r,
                a1 * e1,
            ));
        }
        Mode::Crystals => {
            // Pinch particles along the segment with radial jitter (§19A).
            let size = (r * 0.25 * (1.0 - ctx.chaos)).max(1.0);
            let n = count_along(len, size);
            let e = base_e(ctx.dt, len, n);
            for k in 0..n {
                let t = (k + 1) as f32 / n as f32;
                let jitter = crystal_jitter(rng, ctx.chaos, r);
                let base = point(t);
                let pos = [base[0] + jitter[0], base[1] + jitter[1]];
                out.push(Particle::new(
                    ParticleKind::Pinch,
                    pos,
                    vel,
                    dir,
                    r,
                    ramp(t) * e,
                ));
            }
        }
    }
}

/// Wake jitter: length `C L_j (2 + 2u)` with `L_j` proportional to sqrt(chaos). The
/// constant is a placeholder (C = 1) and the direction is random.
fn wake_jitter(rng: &mut Rng, chaos: f32, radius: f32) -> [f32; 2] {
    if chaos <= 0.0 {
        return [0.0, 0.0];
    }
    let lj = chaos.sqrt() * radius * 0.0625;
    let len = lj * (2.0 + 2.0 * rng.next_f32());
    let th = TAU * rng.next_f32();
    [len * th.cos(), len * th.sin()]
}

/// Crystal jitter: a random radial offset whose size scales with sqrt(chaos) (§19A, placeholder).
fn crystal_jitter(rng: &mut Rng, chaos: f32, radius: f32) -> [f32; 2] {
    if chaos <= 0.0 {
        return [0.0, 0.0];
    }
    let len = chaos.sqrt() * radius * 0.25 * rng.next_f32();
    let th = TAU * rng.next_f32();
    [len * th.cos(), len * th.sin()]
}

/// Simulate one tick for one particle:
/// `previous = x; x += v dt; v *= decay`, with the ordinary or wake decay and the life update.
pub fn simulate(p: &mut Particle, dt: f32, momentum: f32) {
    let wake = p.kind.is_wake_like();
    let decay = if wake {
        decay_wake(dt, 1.0)
    } else {
        decay_ordinary(dt, momentum)
    };
    p.life = if wake {
        (p.life - dt).max(0.0)
    } else {
        (p.life - dt / (momentum + MOMENTUM_EPS)).max(0.0)
    };
    p.prev = p.pos;
    p.pos = [p.pos[0] + p.vel[0] * dt, p.pos[1] + p.vel[1] * dt];
    p.vel = [p.vel[0] * decay, p.vel[1] * decay];
    p.time = dt;
}

/// A particle is removed when its life is 0 and its speed is below a small threshold.
pub fn is_dead(p: &Particle) -> bool {
    p.life <= 0.0 && p.vel[0].hypot(p.vel[1]) < 1e-3
}
