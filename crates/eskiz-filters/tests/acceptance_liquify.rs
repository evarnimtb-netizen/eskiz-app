//! Acceptance tests for Liquify (L1 to L10), plus the direction and magnitude decisions. Screen coordinates have y down.

use eskiz_filters::liquify::{
    self, displacement, pressure_factor, radius, resolve, smooth_velocity, smoothing_alpha,
    Accumulator, Field, Liquify, Mode, ParticleKind, Settings, StrokeSample, IDLE_DT,
};
use eskiz_filters::Image;

const W: usize = 40;
const H: usize = 24;

/// Transparent layer with one opaque dot.
fn dot_layer(x: usize, y: usize) -> Image {
    let mut img = Image::new(W, H);
    let i = (y * W + x) * 4;
    img.data[i..i + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
    img
}

/// Alpha-weighted centroid of an image.
fn centroid(img: &Image) -> [f32; 2] {
    let (mut sx, mut sy, mut sw) = (0.0f32, 0.0f32, 0.0f32);
    for y in 0..img.height {
        for x in 0..img.width {
            let a = img.px(x, y)[3];
            sx += x as f32 * a;
            sy += y as f32 * a;
            sw += a;
        }
    }
    [sx / sw, sy / sw]
}

/// Points along a horizontal line from `x0` to `x1`.
fn horizontal(y: f32, x0: f32, x1: f32, step: f32) -> Vec<(f32, f32)> {
    let n = ((x1 - x0) / step).round() as usize;
    (0..=n).map(|i| (x0 + i as f32 * step, y)).collect()
}

/// Run one stroke of `mode` along `path` (one tick per sample, then drain) and commit on `layer`.
fn stroke(mode: Mode, size: f32, dt: f32, path: &[(f32, f32)], layer: &Image) -> Image {
    let mut lq = Liquify::new(W, H);
    lq.settings = Settings {
        mode,
        size,
        pressure: 1.0,
        distortion: 0.0,
        momentum: 0.0,
    };
    lq.begin_stroke();
    for &(x, y) in path {
        lq.add_sample(
            StrokeSample {
                x,
                y,
                pressure: 1.0,
            },
            dt,
        );
        lq.tick(dt);
    }
    lq.end_stroke();
    lq.commit(layer, None)
}

fn ramp_layer(w: usize, h: usize) -> Image {
    Image::generate(w, h, |x, _| {
        let v = x as f32;
        [v, v, v, 1.0]
    })
}

#[test]
fn l1_radius_is_4_at_size_0_1600_at_size_1_and_about_351_at_half() {
    assert!((radius(0.0) - 4.0).abs() < 1e-4);
    assert!((radius(1.0) - 1600.0).abs() < 1e-3);
    assert!(
        (radius(0.5) - 351.0).abs() < 1.0,
        "radius(0.5) = {}",
        radius(0.5)
    );
}

#[test]
fn l2_pressure_0_gives_0_01_and_pressure_1_gives_1() {
    assert!((pressure_factor(0.0) - 0.01).abs() < 1e-6);
    assert!((pressure_factor(1.0) - 1.0).abs() < 1e-6);
}

#[test]
fn l3_momentum_1_gives_decay_near_0_968_and_momentum_0_gives_zero() {
    let d1 = liquify::decay_ordinary(0.008, 1.0);
    assert!((d1 - 0.968).abs() < 1e-3, "decay {d1}");
    assert_eq!(
        liquify::decay_ordinary(0.008, 0.0),
        0.0,
        "momentum 0 stops the particle"
    );
}

#[test]
fn l4_smoothed_velocity_uses_alpha_one_third_at_1_120_second() {
    assert!((smoothing_alpha(1.0 / 120.0) - 1.0 / 3.0).abs() < 1e-6);
    let v = smooth_velocity([0.0, 0.0], [3.0, 0.0], 1.0 / 120.0);
    assert!((v[0] - 1.0).abs() < 1e-5 && v[1] == 0.0);
}

#[test]
fn l5_reconstruct_weight_one_zeroes_small_fields_and_keeps_large_ones() {
    let mut acc = Accumulator::new(4, 4);
    let prev = Field::new(4, 4);
    acc.offset[5] = [0.2, 0.0];
    acc.weight[5] = 1.0;
    let out = resolve(&prev, &acc, None, f32::INFINITY);
    assert_eq!(
        out.data[5],
        [0.0, 0.0],
        "field length 0.2 is zeroed after attenuation"
    );
    acc.offset[5] = [1000.0, 0.0];
    let out = resolve(&prev, &acc, None, f32::INFINITY);
    assert!(
        out.data[5][0] > 0.5,
        "a large field survives attenuation: {:?}",
        out.data[5]
    );
}

// ---------------------------------------------------------------------------
// Directions per mode (§19A), measured on a dot in a commit
// ---------------------------------------------------------------------------

#[test]
fn push_moves_a_dot_along_the_stroke() {
    // Stroke left to right through a dot at (20, 10).
    let layer = dot_layer(20, 10);
    let out = stroke(
        Mode::Push,
        0.1,
        IDLE_DT,
        &horizontal(10.0, 5.0, 35.0, 1.0),
        &layer,
    );
    let c = centroid(&out);
    assert!(c[0] > 20.5, "dot should move along +x, centroid {c:?}");
    assert!(
        (c[1] - 10.0).abs() < 0.5,
        "no vertical drift, centroid {c:?}"
    );
}

#[test]
fn pinch_moves_a_dot_toward_the_stroke_centre() {
    // Stroke along y = 10. The dot sits 4 px below it at (20, 14).
    let layer = dot_layer(20, 14);
    let out = stroke(
        Mode::Pinch,
        0.1,
        0.05,
        &horizontal(10.0, 8.0, 32.0, 1.0),
        &layer,
    );
    let c = centroid(&out);
    assert!(
        c[1] < 13.5,
        "pinch should pull the dot up towards the stroke, centroid {c:?}"
    );
    assert!(
        c[1] > 10.0,
        "pinch should not overshoot the stroke, centroid {c:?}"
    );
}

#[test]
fn expand_moves_a_dot_away_from_the_stroke_centre() {
    let layer = dot_layer(20, 14);
    let out = stroke(
        Mode::Expand,
        0.1,
        0.05,
        &horizontal(10.0, 8.0, 32.0, 1.0),
        &layer,
    );
    let c = centroid(&out);
    assert!(
        c[1] > 14.5,
        "expand should push the dot down, away from the stroke, centroid {c:?}"
    );
}

/// Angle of a point around `centre`, in screen coordinates (y down). Increasing angle is
/// clockwise on screen.
fn angle_about(centre: [f32; 2], c: [f32; 2]) -> f32 {
    (c[1] - centre[1]).atan2(c[0] - centre[0])
}

/// Twirl test setup: the stroke runs along y = 12 through (20, 12). The dot sits 8 px below it,
/// at (20, 20), which is the twirl ring radius (0.6 R at size 0.1).
fn twirl_angles(mode: Mode) -> (f32, f32) {
    let centre = [20.0, 12.0];
    let layer = dot_layer(20, 20);
    let before = angle_about(centre, [20.0, 20.0]);
    let out = stroke(mode, 0.1, 0.1, &horizontal(12.0, 10.0, 30.0, 1.0), &layer);
    (before, angle_about(centre, centroid(&out)))
}

#[test]
fn twirl_right_rotates_content_clockwise_on_screen() {
    let (before, after) = twirl_angles(Mode::TwirlRight);
    assert!(
        after > before + 1e-3,
        "clockwise is increasing screen angle: {before} -> {after}"
    );
}

#[test]
fn twirl_left_rotates_content_counter_clockwise_on_screen() {
    let (before, after) = twirl_angles(Mode::TwirlLeft);
    assert!(
        after < before - 1e-3,
        "counter-clockwise is decreasing screen angle: {before} -> {after}"
    );
}

// ---------------------------------------------------------------------------
// Signed per-particle displacement (§19.6 table with §19A signs)
// ---------------------------------------------------------------------------

#[test]
fn l7_pinch_offset_samples_outward_and_expand_samples_inward() {
    let c = [10.0, 10.0];
    let p = [11.0, 10.0];
    let dir = [1.0, 0.0];
    // Backward offset: a positive x offset samples further out, which pulls content inwards.
    let pinch = displacement(ParticleKind::Pinch, p, c, dir, 0.5, 1.0);
    assert!(pinch[0] > 0.0, "pinch offset {pinch:?}");
    let expand = displacement(ParticleKind::Expand, p, c, dir, 0.5, -1.0);
    assert!(expand[0] < 0.0, "expand offset {expand:?}");
}

#[test]
fn l8_twirl_left_is_the_negative_of_twirl_right() {
    let c = [0.0, 0.0];
    let p = [0.0, 3.0];
    let dir = [1.0, 0.0];
    let right = displacement(ParticleKind::TwirlRight, p, c, dir, 1.0, 1.0);
    let left = displacement(ParticleKind::TwirlLeft, p, c, dir, 1.0, -1.0);
    assert!((right[0] + left[0]).abs() < 1e-6 && (right[1] + left[1]).abs() < 1e-6);
    // Right at a point south of the centre samples east, so content moves west (clockwise).
    assert!(right[0] > 0.0, "right offset {right:?}");
}

#[test]
fn push_offset_is_backward_along_the_stroke() {
    let off = displacement(
        ParticleKind::Push,
        [5.0, 0.0],
        [0.0, 0.0],
        [1.0, 0.0],
        1.0,
        1.0,
    );
    assert!(off[0] < 0.0, "push offset {off:?}");
}

#[test]
fn reconstruct_has_no_positional_offset() {
    let off = displacement(
        ParticleKind::Reconstruct,
        [5.0, 0.0],
        [0.0, 0.0],
        [1.0, 0.0],
        1.0,
        1.0,
    );
    assert_eq!(off, [0.0, 0.0]);
}

// ---------------------------------------------------------------------------
// Magnitude clamps and momentum (§19A), lifecycle (L9, L10), cancel
// ---------------------------------------------------------------------------

#[test]
fn momentum_defaults_to_zero() {
    assert_eq!(Settings::default().momentum, 0.0);
}

#[test]
fn huge_brush_stays_within_the_per_stroke_and_accumulated_caps() {
    // size 1 gives R = 1600, so the deposit cap is 800 px and the field cap is 1600 px.
    let r = radius(1.0);
    let mut lq = Liquify::new(W, H);
    lq.settings = Settings {
        mode: Mode::Push,
        size: 1.0,
        pressure: 1.0,
        distortion: 0.0,
        momentum: 0.0,
    };
    lq.begin_stroke();
    for &(x, y) in &horizontal(12.0, 2.0, 38.0, 1.0) {
        lq.add_sample(
            StrokeSample {
                x,
                y,
                pressure: 1.0,
            },
            IDLE_DT,
        );
        lq.tick(IDLE_DT);
    }
    lq.end_stroke();
    let max_deposit = lq
        .stroke_deposit()
        .iter()
        .map(|v| v[0].hypot(v[1]))
        .fold(0.0f32, f32::max);
    let max_field = lq
        .field()
        .data
        .iter()
        .map(|v| v[0].hypot(v[1]))
        .fold(0.0f32, f32::max);
    assert!(max_deposit > 0.0, "the huge brush should deposit something");
    assert!(
        max_deposit <= 0.5 * r + 1e-2,
        "per-stroke deposit {max_deposit} exceeds 0.5 R"
    );
    assert!(
        max_field <= r + 1e-2,
        "accumulated field {max_field} exceeds R"
    );
}

#[test]
fn l9_reset_fades_preview_to_zero_then_returns_to_idle() {
    let mut lq = Liquify::new(8, 8);
    lq.set_magnitude(0.8);
    lq.reset();
    assert!(lq.is_fading());
    lq.advance_fade(0.15);
    assert!(
        (lq.preview_magnitude() - 0.4).abs() < 1e-5,
        "half way: {}",
        lq.preview_magnitude()
    );
    lq.advance_fade(0.2);
    assert!(!lq.is_fading(), "fade should end after the reset duration");
    assert!(
        (lq.preview_magnitude() - 1.0).abs() < 1e-6,
        "idle magnitude"
    );
    assert!(lq.field().data.iter().all(|v| *v == [0.0, 0.0]));
    assert!(liquify::fade_magnitude(0.8, 0.3).abs() < 1e-6);
}

#[test]
fn l10_commit_with_magnitude_1_applies_full_field_and_0_5_halves_it() {
    let (w, h) = (20, 3);
    let layer = ramp_layer(w, h);
    let mut full = Liquify::new(w, h);
    let mut half = Liquify::new(w, h);
    for t in full.field_mut().data.iter_mut() {
        *t = [4.0, 0.0];
    }
    for t in half.field_mut().data.iter_mut() {
        *t = [4.0, 0.0];
    }
    half.set_magnitude(0.5);
    let out_full = full.commit(&layer, None);
    let out_half = half.commit(&layer, None);
    assert!(
        (out_full.px(2, 1)[0] - 6.0).abs() < 1e-4,
        "magnitude 1: {}",
        out_full.px(2, 1)[0]
    );
    assert!(
        (out_half.px(2, 1)[0] - 4.0).abs() < 1e-4,
        "magnitude 0.5: {}",
        out_half.px(2, 1)[0]
    );
}

#[test]
fn liquify_cancel_restores_the_field_from_before_the_stroke() {
    let mut lq = Liquify::new(16, 16);
    lq.begin_stroke();
    lq.add_sample(
        StrokeSample {
            x: 4.0,
            y: 8.0,
            pressure: 1.0,
        },
        IDLE_DT,
    );
    lq.add_sample(
        StrokeSample {
            x: 8.0,
            y: 8.0,
            pressure: 1.0,
        },
        IDLE_DT,
    );
    lq.tick(IDLE_DT);
    lq.cancel_stroke();
    assert!(lq.field().data.iter().all(|v| *v == [0.0, 0.0]));
    assert_eq!(lq.particle_count(), 0);
}
