//! 16-bit-per-channel layers and documents: storage, painting, wet mixing, compositing,
//! adjustments and conversion. The 8-bit behaviour is covered by the other test files.

mod common;

use std::collections::HashSet;

use eskiz_core::brushes::{Brush, Preset};
use eskiz_core::composite::{composite, composite16, composite_rect16, composite_to_tiled, CompositeOpts};
use eskiz_core::document::{Document, Props};
use eskiz_core::filters::{AdjustKind, Adjustment, Quality};
use eskiz_core::geometry::{PointerType, Sample, Stroke, StrokeOptions};
use eskiz_core::pixel::{down16, up8};
use eskiz_core::{BlendMode, Depth, Layer, PaintOptions, TILE};

fn soft_stroke(preset: Preset, size: f64) -> Stroke {
    let mut brush = Brush::preset(preset);
    brush.size = size;
    let mut s = Stroke::new(brush, StrokeOptions { seed: 3, ..StrokeOptions::default() });
    for i in 0..120 {
        let p = f64::from(i) / 120.0;
        s.add_sample(Sample {
            x: 60.0 + 380.0 * p,
            y: 120.0 + 40.0 * (p * 5.0).sin(),
            t: f64::from(i) / 120.0,
            pressure: Some(0.5 + 0.5 * (p * std::f64::consts::PI).sin()),
            has_pressure: Some(true),
            pointer: PointerType::Pen,
            ..Sample::default()
        });
    }
    s.finish();
    s
}

fn alpha_levels16(l: &Layer) -> usize {
    l.to_rgba16().as_chunks::<4>().0.iter().map(|p| p[3]).collect::<HashSet<_>>().len()
}

fn alpha_levels8(l: &Layer) -> usize {
    l.to_rgba().as_chunks::<4>().0.iter().map(|p| p[3]).collect::<HashSet<_>>().len()
}

#[test]
fn layers_store_the_depth_they_were_made_with() {
    let l8 = Layer::new(300, 300);
    let l16 = Layer::new_depth(300, 300, Depth::U16);
    assert_eq!((l8.depth(), l16.depth()), (Depth::U8, Depth::U16));
    let mut l16 = l16;
    l16.set_pixel16(299, 299, [1, 2, 3, 40_000]);
    assert_eq!(l16.pixel16(299, 299), [1, 2, 3, 40_000]);
    assert_eq!(l16.tile_count(), 1);
    assert_eq!(l16.tile_bytes(), 2 * eskiz_core::TILE_BYTES, "a 16-bit tile is twice an 8-bit one");
    // The 8-bit view rounds; writing 8-bit widens exactly.
    assert_eq!(l16.pixel(299, 299), [0, 0, 0, down16(40_000)]);
    l16.set_pixel(0, 0, [255, 128, 7, 255]);
    assert_eq!(l16.pixel16(0, 0), [65_535, 128 * 257, 7 * 257, 65_535]);
    // A tile of the wrong depth is refused.
    let mut other = Layer::new_depth(300, 300, Depth::U8);
    other.set_tile(0, 0, l16.tile(0, 0).cloned());
    assert_eq!(other.tile_count(), 0);
}

#[test]
fn rect_io_and_flip_are_exact_at_16_bits() {
    let (w, h) = (TILE + 40, 30);
    let data: Vec<u16> = (0..w * h * 4).map(|i| ((i as u32).wrapping_mul(2_654_435_761u32) >> 12) as u16 | 1).collect();
    let mut l = Layer::from_rgba16(w, h, &data);
    assert_eq!(l.to_rgba16(), data);
    assert_eq!(l.read_rect16(250, 3, 20, 5), {
        let mut v = Vec::new();
        for y in 3..8 {
            v.extend_from_slice(&data[(y * w + 250) * 4..(y * w + 270) * 4]);
        }
        v
    });
    let before = l.clone();
    l.flip(true);
    assert_ne!(l, before);
    l.flip(true);
    assert_eq!(l, before, "flipping twice restores every 16-bit sample");
}

#[test]
fn a_soft_stroke_has_far_more_than_256_levels_at_16_bits() {
    let stroke = soft_stroke(Preset::SoftAirbrush, 150.0);
    let mut l8 = Layer::new(500, 240);
    let mut l16 = Layer::new_depth(500, 240, Depth::U16);
    l8.paint_stroke_with(&stroke, PaintOptions::new([0.8, 0.3, 0.1]));
    l16.paint_stroke_with(&stroke, PaintOptions::new([0.8, 0.3, 0.1]));
    let (n8, n16) = (alpha_levels8(&l8), alpha_levels16(&l16));
    assert!(n8 <= 256, "{n8}");
    assert!(n16 > 2_000, "only {n16} alpha levels at 16 bits");
    // Same stroke, same picture: the 16-bit result rounded to 8 bits is within one level.
    let (a, b) = (l8.to_rgba(), l16.to_rgba());
    let worst = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
    assert!(worst <= 1, "16-bit paint differs from 8-bit by {worst}");
}

#[test]
fn many_dabs_do_not_accumulate_8_bit_rounding_error() {
    // Fifty faint passes over the same pixels: each rounds once at 8 bits and accumulates the
    // error; at 16 bits the rounding is 257 times finer.
    let mut brush = Brush::preset(Preset::TechnicalPen);
    brush.size = 40.0;
    brush.opacity = 0.03;
    let mut s = Stroke::new(brush, StrokeOptions::default());
    for i in 0..10 {
        s.add_sample(Sample { x: 50.0 + f64::from(i), y: 50.0, t: f64::from(i) / 120.0, ..Sample::default() });
    }
    s.finish();
    let mut l8 = Layer::new(100, 100);
    let mut l16 = Layer::new_depth(100, 100, Depth::U16);
    for _ in 0..50 {
        l8.paint_stroke_with(&s, PaintOptions::new([0.5, 0.5, 0.5]));
        l16.paint_stroke_with(&s, PaintOptions::new([0.5, 0.5, 0.5]));
    }
    let exact = l16.pixel_unit(50, 50)[3];
    let rounded = l8.pixel_unit(50, 50)[3];
    assert!(exact > 0.0);
    assert!((exact - rounded).abs() < 0.03, "{exact} vs {rounded}");
}

#[test]
fn wet_brushes_mix_at_16_bits() {
    use eskiz_core::brushes::{Shape, Wet};
    let brush = Brush {
        shape: Shape::HardRound,
        wet: Wet { enabled: true, dilution: 0.3, pull: 1.0, ..Wet::default() },
        ..common::plain_brush()
    };
    let mut s = Stroke::new(brush, StrokeOptions { seed: 3, ..StrokeOptions::default() });
    for i in 0..=72 {
        s.add_sample(common::pen(20.0 + f64::from(i) * 2.5, 32.0, f64::from(i) / 120.0, 1.0));
    }
    s.finish();
    let mut seen = Vec::new();
    for depth in [Depth::U8, Depth::U16] {
        let mut l = Layer::new_depth(420, 64, depth);
        for y in 0..64 {
            for x in 0..60 {
                l.set_pixel(x, y, [230, 20, 20, 255]);
            }
        }
        l.paint_stroke_with(&s, PaintOptions::new([0.1, 0.1, 0.9]));
        // Past the red block the stroke still carries some of it, as on the 8-bit path.
        let p = l.pixel_unit(75, 32);
        assert!(p[3] > 0.3, "{depth:?}: painted {p:?}");
        assert!(p[0] / (p[0] + p[2]) > 0.2, "{depth:?}: red was carried past the block ({p:?})");
        seen.push(p);
    }
    for c in 0..4 {
        assert!((seen[0][c] - seen[1][c]).abs() < 0.02, "8-bit {:?} vs 16-bit {:?}", seen[0], seen[1]);
    }
}

fn banded_doc(depth: Depth) -> Document {
    let mut d = Document::empty_depth(300, 40, depth);
    d.background = [0.9, 0.85, 0.7];
    let mut base = Layer::new_depth(300, 40, depth);
    let mut top = Layer::new_depth(300, 40, depth);
    for y in 0..40 {
        for x in 0..300 {
            let v = x as f64 / 299.0;
            let a = 0.4 + 0.6 * (y as f64 / 39.0);
            base.update_rect_unit(None, x, y, x + 1, y + 1, |_, _, _| [v * a, 0.2 * a, (1.0 - v) * a, a]);
            top.update_rect_unit(None, x, y, x + 1, y + 1, |_, _, _| [0.5 * 0.7, 0.6 * 0.7, 0.1 * 0.7, 0.7]);
        }
    }
    d.push_raster(None, Props::new("base"), None, base);
    let mut p = Props::new("top");
    p.blend = BlendMode::Multiply;
    p.opacity = 0.6;
    d.push_raster(None, p, None, top);
    d
}

#[test]
fn the_16_bit_composite_matches_the_8_bit_one_and_keeps_more_levels() {
    let d8 = banded_doc(Depth::U8);
    let d16 = banded_doc(Depth::U16);
    let c8 = composite(&d8, CompositeOpts::default());
    let c16 = composite(&d16, CompositeOpts::default());
    let worst = c8.iter().zip(&c16).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
    assert!(worst <= 2, "display composites differ by {worst}");
    let wide = composite16(&d16, CompositeOpts::default());
    let levels = wide.as_chunks::<4>().0.iter().map(|p| p[0]).collect::<HashSet<_>>().len();
    assert!(levels > 256, "{levels} red levels at 16 bits");
    // A rectangle of the 16-bit composite equals the same rectangle of the whole.
    let rect = composite_rect16(&d16, 100, 5, 50, 10, CompositeOpts::default());
    for y in 0..10 {
        assert_eq!(&rect[y * 200..(y + 1) * 200], &wide[((5 + y) * 300 + 100) * 4..((5 + y) * 300 + 150) * 4]);
    }
    // The tiled composite keeps the document's depth.
    assert_eq!(composite_to_tiled(&d16, false).depth(), Depth::U16);
    assert_eq!(composite_to_tiled(&d8, false).depth(), Depth::U8);
}

#[test]
fn clip_groups_and_masks_work_at_16_bits() {
    let mut d = banded_doc(Depth::U16);
    let ids = d.layer_ids();
    d.node_mut(ids[1]).unwrap().props.clipped = true;
    d.node_mut(ids[1]).unwrap().mask = Some(eskiz_core::Mask::filled(300, 40, 128));
    let wide = composite16(&d, CompositeOpts::default());
    assert!(wide.iter().any(|&v| v != 0));
    let mut d8 = banded_doc(Depth::U8);
    let ids8 = d8.layer_ids();
    d8.node_mut(ids8[1]).unwrap().props.clipped = true;
    d8.node_mut(ids8[1]).unwrap().mask = Some(eskiz_core::Mask::filled(300, 40, 128));
    let narrow = composite(&d8, CompositeOpts::default());
    let worst = narrow.iter().zip(composite(&d, CompositeOpts::default())).map(|(a, b)| a.abs_diff(b)).max().unwrap();
    assert!(worst <= 2, "{worst}");
}

fn disc(depth: Depth) -> (Layer, usize) {
    let (w, h) = (200usize, 200usize);
    let mut l = Layer::new_depth(w, h, depth);
    for y in 0..h {
        for x in 0..w {
            let r = (((x as f64 - 100.0).powi(2) + (y as f64 - 100.0).powi(2)).sqrt() / 60.0).min(1.0);
            let a = if r < 1.0 { 1.0 } else { 0.0 };
            l.update_rect_unit(None, x, y, x + 1, y + 1, |_, _, _| [0.6 * a, 0.3 * a, 0.9 * a, a]);
        }
    }
    (l, w * h)
}

#[test]
fn a_blur_of_a_16_bit_layer_has_no_banding_and_does_not_round_trip_through_8_bits() {
    let adj = Adjustment::new(AdjustKind::GaussianBlur);
    let (l8, _) = disc(Depth::U8);
    let (l16, _) = disc(Depth::U16);
    let out8 = adj.render(&l8.to_rgba(), 200, 200, Quality::Full, None, None);
    let out16 = adj.render(&l16.to_rgba16(), 200, 200, Quality::Full, None, None);
    let n8 = out8.as_chunks::<4>().0.iter().map(|p| p[3]).collect::<HashSet<_>>().len();
    let n16 = out16.as_chunks::<4>().0.iter().map(|p| p[3]).collect::<HashSet<_>>().len();
    assert!(n8 <= 256);
    assert!(n16 > 256, "the blurred edge holds only {n16} alpha levels at 16 bits");
    // The two agree to within a level when the 16-bit result is rounded.
    let worst = out8.iter().zip(&out16).map(|(a, b)| a.abs_diff(down16(*b))).max().unwrap();
    assert!(worst <= 1, "{worst}");
    // Other bridge filters run in f32 without an 8-bit round trip: Sharpen at 16 bits.
    let sharp = Adjustment::new(AdjustKind::Sharpen);
    let s16 = sharp.render(&l16.to_rgba16(), 200, 200, Quality::Full, None, None);
    assert_eq!(s16.len(), 200 * 200 * 4);
}

#[test]
fn opacity_and_hsb_adjustments_work_at_16_bits() {
    let (l16, _) = disc(Depth::U16);
    let src = l16.to_rgba16();
    let mut op = Adjustment::new(AdjustKind::Opacity);
    op.set(0, 0.5);
    let half = op.render(&src, 200, 200, Quality::Full, None, None);
    let centre = (100 * 200 + 100) * 4;
    assert_eq!(half[centre + 3], 32_768);
    let mut hsb = Adjustment::new(AdjustKind::Hsb);
    hsb.set(2, 0.2);
    let brighter = hsb.render(&src, 200, 200, Quality::Full, None, None);
    assert!(brighter[centre] > src[centre]);
    assert_eq!(brighter[centre + 3], src[centre + 3], "alpha is untouched");
}

#[test]
fn converting_a_document_up_and_down_is_exact_and_dithering_never_disturbs_8_bit_values() {
    let mut d = banded_doc(Depth::U8);
    let ids = d.layer_ids();
    let original: Vec<Vec<u8>> = ids.iter().map(|&i| d.layer(i).unwrap().to_rgba()).collect();
    d.convert_depth(Depth::U16, false);
    assert_eq!(d.depth, Depth::U16);
    assert!(ids.iter().all(|&i| d.layer(i).unwrap().depth() == Depth::U16));
    assert_eq!(d.layer(ids[0]).unwrap().pixel16(10, 10), d.layer(ids[0]).unwrap().pixel(10, 10).map(up8));
    let tiles: usize = ids.iter().map(|&i| d.layer(i).unwrap().tile_count()).sum();
    assert_eq!(d.tile_bytes(), tiles * 2 * eskiz_core::TILE_BYTES);
    d.convert_depth(Depth::U8, true);
    assert_eq!(d.depth, Depth::U8);
    for (i, &id) in ids.iter().enumerate() {
        assert_eq!(d.layer(id).unwrap().to_rgba(), original[i]);
    }
}

#[test]
fn dithering_a_smooth_16_bit_ramp_avoids_8_bit_bands() {
    // A very slow ramp, 0.40 to 0.41 over 600 px: 8-bit rounding makes 3 long bands.
    let (w, h) = (600usize, 6usize);
    let mut l = Layer::new_depth(w, h, Depth::U16);
    for y in 0..h {
        for x in 0..w {
            let v = (0.40 + 0.01 * x as f64 / (w - 1) as f64) * 65_535.0;
            l.set_pixel16(x, y, [v as u16, v as u16, v as u16, 65_535]);
        }
    }
    let plain = l.converted(Depth::U8, false);
    let dithered = l.converted(Depth::U8, true);
    let longest_run = |l: &Layer| {
        let (mut best, mut run, mut last) = (0, 0, 256u16);
        for x in 0..w {
            let v = u16::from(l.pixel(x, 2)[0]);
            run = if v == last { run + 1 } else { 1 };
            last = v;
            best = best.max(run);
        }
        best
    };
    assert!(longest_run(&plain) > 100, "plain rounding bands: {}", longest_run(&plain));
    assert!(longest_run(&dithered) < 60, "dithering breaks the bands up: {}", longest_run(&dithered));
    // The local average still follows the ramp.
    let mean = |l: &Layer, x0: usize, x1: usize| (x0..x1).flat_map(|x| (0..h).map(move |y| (x, y))).map(|(x, y)| f64::from(l.pixel(x, y)[0])).sum::<f64>() / ((x1 - x0) * h) as f64;
    for x0 in [0usize, 300, 500] {
        let want = (0.40 + 0.01 * (x0 as f64 + 50.0) / (w - 1) as f64) * 255.0;
        assert!((mean(&dithered, x0, x0 + 100) - want).abs() < 0.2, "{x0}");
    }
}
