//! The artwork of the welcome tour, drawn entirely in code (no image files): a sky gradient, a
//! sun and two ridges of hills. The same generator makes the demo art of the Gallery tests.

use crate::document::Document;

/// Width of the tour's sample canvas, pixels.
pub(crate) const SAMPLE_W: usize = 1600;
/// Height of the tour's sample canvas, pixels.
pub(crate) const SAMPLE_H: usize = 1000;

/// Sky top, sky bottom and hill colour of each of the five looks.
const PALETTES: [([f32; 3], [f32; 3], [f32; 3]); 5] = [
    ([0.98, 0.62, 0.35], [0.35, 0.22, 0.52], [0.10, 0.18, 0.30]),
    ([0.55, 0.80, 0.95], [0.95, 0.90, 0.75], [0.12, 0.40, 0.30]),
    ([0.08, 0.10, 0.28], [0.30, 0.20, 0.55], [0.05, 0.05, 0.12]),
    ([0.95, 0.45, 0.50], [0.98, 0.80, 0.45], [0.30, 0.15, 0.35]),
    ([0.70, 0.90, 0.75], [0.25, 0.55, 0.60], [0.08, 0.25, 0.30]),
];

/// A `w` x `h` landscape on one layer named "Scene". `seed` picks one of five looks and bends
/// the hills.
pub(crate) fn scene(w: usize, h: usize, seed: usize) -> Document {
    let mut doc = Document::new(w, h);
    let (top, bottom, hill) = PALETTES[seed % PALETTES.len()];
    let aspect = (w as f32 / h as f32).powi(2);
    // The ridges depend on the column only.
    let ridges: Vec<(f32, f32)> = (0..w)
        .map(|x| {
            let fx = x as f32 / w as f32;
            let r1 = 0.66 + 0.06 * (fx * 6.0 + seed as f32).sin() + 0.03 * (fx * 15.0).sin();
            let r2 = 0.80 + 0.05 * (fx * 4.0 + 2.0 * seed as f32).cos();
            (r1, r2)
        })
        .collect();
    let layer = doc.layer_at_mut(0).unwrap();
    for y in 0..h {
        let fy = y as f32 / h as f32;
        for (x, &(ridge1, ridge2)) in ridges.iter().enumerate() {
            let fx = x as f32 / w as f32;
            let sun = ((fx - 0.68).powi(2) * aspect + (fy - 0.38).powi(2)).sqrt() < 0.11;
            let mut px = [0u8; 4];
            for i in 0..3 {
                let mut c = top[i] + (bottom[i] - top[i]) * fy;
                if sun {
                    c = 1.0 - 0.1 * i as f32;
                }
                if fy > ridge1 {
                    c = hill[i] * 1.4;
                }
                if fy > ridge2 {
                    c = hill[i] * 0.7;
                }
                px[i] = (c.clamp(0.0, 1.0) * 255.0) as u8;
            }
            px[3] = 255;
            layer.set_pixel(x, y, px);
        }
    }
    doc.props_at_mut(0).unwrap().name = "Scene".to_owned();
    doc
}

/// The tour's sample document: 1600 x 1000, one layer named "Sample".
pub(crate) fn sample_document() -> Document {
    let mut doc = scene(SAMPLE_W, SAMPLE_H, 3);
    doc.props_at_mut(0).unwrap().name = "Sample".to_owned();
    doc
}
