//! Wet mixing: pickup, carry, depletion, and the dilution-0 regression.

mod common;

use common::{pen, plain_brush};
use eskiz_core::brushes::{Accumulation, Brush, Shape, Wet};
use eskiz_core::geometry::{Stroke, StrokeOptions};
use eskiz_core::{Layer, PaintOptions};

const W: usize = 420;
const H: usize = 64;

/// A stroke along y = 32 from `x0` to `x1`, `step` px per sample, at pressure 1.
fn stroke(brush: &Brush, x0: f64, x1: f64) -> Stroke {
    let mut s = Stroke::new(brush.clone(), StrokeOptions { seed: 3, ..StrokeOptions::default() });
    let n = ((x1 - x0) / 2.5) as usize;
    for i in 0..=n {
        s.add_sample(pen(x0 + i as f64 * 2.5, 32.0, i as f64 / 120.0, 1.0));
    }
    s.finish();
    s
}

/// Left of `edge` opaque red, right of it transparent.
fn red_then_bare(edge: usize) -> Layer {
    let mut l = Layer::new(W, H);
    for y in 0..H {
        for x in 0..edge {
            l.set_pixel(x, y, [230, 20, 20, 255]);
        }
    }
    l
}

fn wet_brush(wet: Wet) -> Brush {
    Brush { shape: Shape::HardRound, wet, ..plain_brush() }
}

/// Share of red in the premultiplied colour at `(x, 32)`.
fn redness(l: &Layer, x: usize) -> f64 {
    let p = l.pixel(x, 32);
    f64::from(p[0]) / (f64::from(p[0]) + f64::from(p[2])).max(1.0)
}

#[test]
fn a_wet_brush_carries_red_picked_up_from_the_canvas() {
    let blue = [0.1, 0.1, 0.9];
    let paint = |wet: Option<Wet>| {
        let mut l = red_then_bare(60);
        let brush = match wet {
            Some(w) => wet_brush(w),
            None => wet_brush(Wet::default()),
        };
        l.paint_stroke_with(&stroke(&brush, 20.0, 200.0), PaintOptions::new(blue));
        l
    };
    let diluted = |pull: f64| Wet { enabled: true, dilution: 0.3, pull, ..Wet::default() };

    let dry = paint(None);
    let off = paint(Some(diluted(0.0)));
    let low = paint(Some(diluted(0.15)));
    let high = paint(Some(diluted(1.0)));

    // Over the red itself, more pull takes more of the canvas colour into the stroke.
    assert!(redness(&high, 30) > redness(&off, 30) + 0.03, "in area {} vs {}", redness(&high, 30), redness(&off, 30));
    assert!(redness(&high, 30) > redness(&low, 30), "in area monotonic");
    // Just past the red area: a dry brush and a no-pull brush lay pure blue.
    assert!(redness(&dry, 75) < 0.2, "dry {}", redness(&dry, 75));
    assert!(redness(&off, 75) < 0.2, "no pull {}", redness(&off, 75));
    // With pull, the stroke carries red onto the bare canvas, and more of it with more pull.
    let (r_low, r_high) = (redness(&low, 75), redness(&high, 75));
    assert!(r_low > redness(&off, 75) + 0.05, "low {r_low}");
    assert!(r_high > r_low + 0.05, "high {r_high} vs low {r_low}");
    // The colour pulled onto the bare canvas fades again further along.
    assert!(redness(&high, 75) > redness(&high, 160) + 0.05);
}

#[test]
fn charge_depletion_fades_the_stroke_along_its_length() {
    let alpha_at = |l: &Layer, x: usize| f64::from(l.pixel(x, 32)[3]);
    let paint = |wet: Wet| {
        let mut l = Layer::new(W, H);
        l.paint_stroke_with(&stroke(&wet_brush(wet), 10.0, 400.0), PaintOptions::new([0.2, 0.5, 0.3]));
        l
    };
    // Paint runs out quickly: the tail is much fainter than the head.
    let fading = paint(Wet { enabled: true, falloff_amount: 0.5, pull: 0.0, ..Wet::default() });
    let head = alpha_at(&fading, 14);
    let tail = alpha_at(&fading, 380);
    assert!(head > 200.0, "head {head}");
    assert!(tail < head * 0.5, "tail {tail} vs head {head}");
    // A fuller charge keeps the stroke going longer.
    let charged = paint(Wet { enabled: true, falloff_amount: 0.5, load_amount: 0.95, pull: 0.0, ..Wet::default() });
    assert!(alpha_at(&charged, 380) > tail + 40.0, "charged {}", alpha_at(&charged, 380));
    // And with no fall-off nothing fades at all.
    let steady = paint(Wet { enabled: true, pull: 0.0, ..Wet::default() });
    assert!(alpha_at(&steady, 380) >= 250.0);
}

#[test]
fn dilution_zero_is_exactly_a_normal_brush() {
    // A busy base so pickup would show if it leaked in.
    let mut base = Layer::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let v = ((x * 7 + y * 13) % 200) as u8;
            base.set_pixel(x, y, [v, v / 2, 200 - v / 2, 255]);
        }
    }
    for accumulation in [Accumulation::Glaze, Accumulation::Buildup] {
        let normal = Brush { accumulation, opacity: 0.8, flow: 0.4, shape: Shape::SoftRound, ..plain_brush() };
        let wet = Brush {
            wet: Wet { enabled: true, pull: 1.0, wetness: 0.0, ..Wet::default() },
            ..normal.clone()
        };
        let mut a = base.clone();
        let mut b = base.clone();
        a.paint_stroke_with(&stroke(&normal, 20.0, 380.0), PaintOptions::new([0.9, 0.2, 0.1]));
        b.paint_stroke_with(&stroke(&wet, 20.0, 380.0), PaintOptions::new([0.9, 0.2, 0.1]));
        assert_eq!(a.to_rgba(), b.to_rgba(), "{accumulation:?}");
    }
}

#[test]
fn wetness_and_bleed_soften_and_spread_the_edge() {
    let paint = |wet: Wet| {
        let mut l = Layer::new(W, H);
        let brush = Brush { size: 16.0, ..wet_brush(wet) };
        l.paint_stroke_with(&stroke(&brush, 40.0, 300.0), PaintOptions::new([0.0, 0.0, 0.0]));
        l
    };
    let on = |wetness: f64, bleed: f64| Wet { enabled: true, wetness, bleed, ..Wet::default() };
    let sharp = paint(on(0.0, 0.0));
    let soft = paint(on(0.8, 0.0));
    let spread = paint(on(0.0, 0.8));
    // The row three pixels outside the hard edge (the stroke is 16 px tall, centred on y 32).
    let outside = |l: &Layer| u32::from(l.pixel(150, 32 - 8 - 3)[3]);
    let edge = |l: &Layer| u32::from(l.pixel(150, 32 - 8 + 2)[3]);
    assert_eq!(outside(&sharp), 0);
    assert!(outside(&soft) > 0 || edge(&soft) < edge(&sharp), "soft edge");
    assert!(outside(&spread) > 20, "bleed spreads paint outside: {}", outside(&spread));
    // The core of the stroke is untouched by softening.
    assert!(soft.pixel(150, 32)[3] >= 250);
}
