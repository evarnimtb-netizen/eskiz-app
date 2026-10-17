//! Stroke-level test through the public API: a straight stroke fed one sample
//! at a time produces dabs evenly spaced by the spacing diameter.

mod common;

use common::{plain_brush, straight_line};
use eskiz_core::geometry::{spacing_diameter, Stroke, StrokeOptions};

#[test]
fn straight_stroke_produces_evenly_spaced_dabs_per_spacing() {
    for spacing in [0.05, 0.1] {
        let mut brush = plain_brush();
        brush.spacing = spacing;
        let want = spacing_diameter(20.0, spacing);

        let mut stroke = Stroke::new(brush, StrokeOptions::default());
        for sample in straight_line(200, 2.0, 100.0, 1.0, 10.0) {
            stroke.add_sample(sample);
        }
        let dabs = stroke.finish().dabs;
        assert!(
            dabs.len() > 30,
            "only {} dabs at spacing {spacing}",
            dabs.len()
        );

        let gaps: Vec<f64> = dabs
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
            .collect();
        // Interior gaps only: the first and last dab sit on the stroke ends.
        for (i, g) in gaps.iter().enumerate().skip(1).take(gaps.len() - 2) {
            assert!(
                (g - want).abs() < 0.05,
                "gap {i}: {g} vs {want} (spacing {spacing})"
            );
        }

        // The dab count matches the path length to within a few dabs.
        let path = 199.0 * 2.0;
        let expected = path / want;
        assert!(
            (dabs.len() as f64 - expected).abs() < 3.0,
            "{} dabs, expected about {expected:.1} (spacing {spacing})",
            dabs.len()
        );
    }
}
