//! Raster and layer tests: single-dab coverage, accumulation modes, stroke
//! painting and erase, and flattening.

mod common;

use common::{plain_brush, straight_line};
use eskiz_core::brushes::{Accumulation, Brush, Shape};
use eskiz_core::canvas::{Canvas, Layer, PaintOptions};
use eskiz_core::compose::flatten;
use eskiz_core::geometry::{Dab, Stroke, StrokeOptions};
use eskiz_core::raster::{CoverageBuffer, DabRaster};

/// Round dab of `size` px centred on `(x, y)`.
fn dab(x: f64, y: f64, size: f64, opacity: f64) -> Dab {
    Dab {
        x,
        y,
        sx: size,
        sy: size,
        angle: 0.0,
        opacity,
        flow: 1.0,
        ..Dab::default()
    }
}

/// Coverage buffer over pixels `[30, 70)^2`, the area used by the single-dab tests.
fn buffer() -> CoverageBuffer {
    CoverageBuffer::new(30, 30, 40, 40)
}

fn hard_brush() -> Brush {
    Brush {
        shape: Shape::HardRound,
        ..Brush::default()
    }
}

#[test]
fn single_hard_dab_is_solid_inside_and_clear_outside() {
    // Dab centre at a pixel centre (50.5, 50.5) with radius 10 px.
    let raster = DabRaster::new(&hard_brush());
    let mut cov = buffer();
    cov.splat(&[dab(50.5, 50.5, 20.0, 1.0)], &raster);
    assert!(cov.get(50, 50) > 0.99, "center {}", cov.get(50, 50));
    // pixel centre 59.5 is 9 px from the centre, inside the disc
    assert!(cov.get(59, 50) > 0.99, "inside edge {}", cov.get(59, 50));
    // pixel centre 61.5 is 11 px away, outside the quad
    assert_eq!(cov.get(61, 50), 0.0);
}

#[test]
fn single_soft_dab_has_expected_centre_and_half_radius_alpha() {
    let raster = DabRaster::new(&Brush {
        shape: Shape::SoftRound,
        ..Brush::default()
    });
    let mut cov = buffer();
    cov.splat(&[dab(50.5, 50.5, 20.0, 1.0)], &raster);
    let centre = cov.get(50, 50);
    assert!((centre - 1.0).abs() < 0.01, "center {centre}");
    // pixel centre 55.5 is 5 px from the centre: half the radius, smoothstep(0.5) = 0.5
    let half = cov.get(55, 50);
    assert!((half - 0.5).abs() < 0.03, "half radius {half}");
    // pixel centre 59.5 is 9 px away: almost no coverage
    let edge = cov.get(59, 50);
    assert!(edge < 0.05, "edge {edge}");
}

#[test]
fn dab_alpha_is_opacity_times_flow() {
    let raster = DabRaster::new(&hard_brush());
    let mut d = dab(50.5, 50.5, 20.0, 0.5);
    d.flow = 0.6;
    let mut cov = buffer();
    cov.splat(&[d], &raster);
    assert!(
        (cov.get(50, 50) - 0.3).abs() < 0.01,
        "center {}",
        cov.get(50, 50)
    );
}

#[test]
fn glaze_overlap_stays_at_stroke_opacity_while_buildup_accumulates() {
    let d = dab(50.5, 50.5, 20.0, 0.5);

    let glaze = DabRaster::new(&Brush {
        accumulation: Accumulation::Glaze,
        ..hard_brush()
    });
    let mut g = buffer();
    g.splat(&[d, d], &glaze);
    assert!(
        (g.get(50, 50) - 0.5).abs() < 0.01,
        "glaze {}",
        g.get(50, 50)
    );

    let buildup = DabRaster::new(&Brush {
        accumulation: Accumulation::Buildup,
        ..hard_brush()
    });
    let mut b = buffer();
    b.splat(&[d, d], &buildup);
    assert!(
        (b.get(50, 50) - 0.75).abs() < 0.01,
        "buildup {}",
        b.get(50, 50)
    );
}

#[test]
fn stroke_paints_its_path_and_erase_removes_it() {
    let mut brush = plain_brush();
    brush.shape = Shape::HardRound;
    brush.size = 8.0;
    let samples = straight_line(17, 2.5, 32.0, 1.0, 10.0);

    let mut paint = Stroke::new(brush.clone(), StrokeOptions::default());
    paint.add_samples(&samples, &[]);
    paint.finish();

    let mut layer = Layer::new(64, 64);
    layer.paint_stroke(&paint, [1.0, 0.0, 0.0]);
    let mid = layer.pixel(30, 32);
    assert!(mid[3] >= 250, "alpha {mid:?}");
    assert!(
        mid[0] >= 250 && mid[1] == 0 && mid[2] == 0,
        "color {mid:?}"
    );
    assert_eq!(
        layer.pixel(30, 5),
        [0, 0, 0, 0],
        "off-path pixel stays clear"
    );

    let mut eraser = Stroke::new(brush, StrokeOptions::default());
    eraser.add_samples(&samples, &[]);
    eraser.finish();
    layer.paint_stroke_with(
        &eraser,
        PaintOptions {
            erase: true,
            ..PaintOptions::new([0.0; 3])
        },
    );
    assert_eq!(layer.pixel(30, 32)[3], 0, "erase clears the path");
}

#[test]
fn flatten_composites_layers_over_white_paper() {
    let mut brush = plain_brush();
    brush.shape = Shape::HardRound;
    brush.size = 8.0;
    let mut stroke = Stroke::new(brush, StrokeOptions::default());
    stroke.add_samples(&straight_line(17, 2.5, 32.0, 1.0, 10.0), &[]);
    stroke.finish();

    let mut canvas = Canvas::new(64, 64);
    let idx = canvas.add_layer();
    canvas.layers[idx].paint_stroke(&stroke, [1.0, 0.0, 0.0]);

    let px = flatten(&canvas);
    assert_eq!(px.len(), 64 * 64 * 4);
    let at = |x: usize, y: usize| {
        let i = (y * 64 + x) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    assert_eq!(at(30, 32), [255, 0, 0, 255], "red stroke over white");
    assert_eq!(at(30, 5), [255, 255, 255, 255], "untouched area is paper");
}
