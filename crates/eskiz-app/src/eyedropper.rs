//! Eyedropper: sampling the flattened canvas, and the loupe ring.
//!
//! The eyedropper starts from a touch-and-hold, from holding the Pick Color button, or from
//! Alt/Option with a click or drag. While it runs, the loupe shows the colour under the
//! pointer (top half) against the colour that was active before (bottom half). Release
//! commits the new colour.

use eframe::egui::{epaint::PathShape, Color32, Painter, Pos2, Shape, Stroke, Vec2};

/// Radius of the loupe ring, screen points.
pub const LOUPE_RADIUS: f32 = 44.0;

/// Average colour of the square of pixels within `radius` of `(x, y)` in a `w` x `h`
/// RGBA8 buffer. Pixels outside the buffer are ignored. `None` when `(x, y)` is outside.
pub fn sample_region(rgba: &[u8], w: usize, h: usize, x: i64, y: i64, radius: i64) -> Option<[u8; 3]> {
    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 || rgba.len() < w * h * 4 {
        return None;
    }
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for yy in (y - radius).max(0)..=(y + radius).min(h as i64 - 1) {
        for xx in (x - radius).max(0)..=(x + radius).min(w as i64 - 1) {
            let i = (yy as usize * w + xx as usize) * 4;
            r += u64::from(rgba[i]);
            g += u64::from(rgba[i + 1]);
            b += u64::from(rgba[i + 2]);
            n += 1;
        }
    }
    let avg = |s: u64| (s / n) as u8;
    Some([avg(r), avg(g), avg(b)])
}

/// Points of a half disc: the top half when `top` is true, else the bottom half.
fn half_disc(centre: Pos2, radius: f32, top: bool) -> Vec<Pos2> {
    let n = 40;
    let start = if top { std::f32::consts::PI } else { 0.0 };
    (0..=n)
        .map(|i| {
            let a = start + std::f32::consts::PI * i as f32 / n as f32;
            centre + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect()
}

/// Draw the loupe ring around `centre`: top half `new`, bottom half `previous`.
pub fn draw_loupe(painter: &Painter, centre: Pos2, new: Color32, previous: Color32, ring: Color32) {
    painter.add(Shape::convex_polygon(
        half_disc(centre, LOUPE_RADIUS, true),
        new,
        Stroke::NONE,
    ));
    painter.add(Shape::convex_polygon(
        half_disc(centre, LOUPE_RADIUS, false),
        previous,
        Stroke::NONE,
    ));
    // A dark outline keeps the ring visible on light and dark areas alike.
    let mut outline: Vec<Pos2> = (0..=64)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 64.0;
            centre + Vec2::new(a.cos(), a.sin()) * LOUPE_RADIUS
        })
        .collect();
    outline.push(outline[0]);
    painter.add(Shape::Path(PathShape::line(
        outline.clone(),
        Stroke::new(3.0, Color32::from_black_alpha(140)),
    )));
    painter.add(Shape::Path(PathShape::line(outline, Stroke::new(2.0, ring))));
    // Small centre tick shows the exact sampled point.
    painter.circle_stroke(centre, 3.0, Stroke::new(1.0, ring));
}

/// `#RRGGBB` for status text.
pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4 x 2 buffer: left half red, right half blue.
    fn stripes() -> Vec<u8> {
        let mut v = Vec::new();
        for _y in 0..2 {
            for x in 0..4 {
                if x < 2 {
                    v.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    v.extend_from_slice(&[0, 0, 255, 255]);
                }
            }
        }
        v
    }

    #[test]
    fn single_pixel_sample_is_exact() {
        let buf = stripes();
        assert_eq!(sample_region(&buf, 4, 2, 0, 0, 0), Some([255, 0, 0]));
        assert_eq!(sample_region(&buf, 4, 2, 3, 1, 0), Some([0, 0, 255]));
    }

    #[test]
    fn radius_averages_across_the_edge() {
        let buf = stripes();
        // Centred on x=1 with radius 1 the window covers x=0..=2: two red columns, one blue.
        let mixed = sample_region(&buf, 4, 2, 1, 0, 1).unwrap();
        assert!(mixed[0] > 160 && mixed[0] < 180 && mixed[2] > 80 && mixed[2] < 90, "{mixed:?}");
        // Radius 0 is exact, and a window wholly inside one colour is that colour.
        assert_eq!(sample_region(&buf, 4, 2, 0, 1, 0), Some([255, 0, 0]));
        assert_eq!(sample_region(&buf, 4, 2, 3, 0, 0), Some([0, 0, 255]));
    }

    #[test]
    fn outside_the_buffer_is_none() {
        let buf = stripes();
        assert_eq!(sample_region(&buf, 4, 2, -1, 0, 0), None);
        assert_eq!(sample_region(&buf, 4, 2, 4, 0, 0), None);
        assert_eq!(sample_region(&buf, 4, 2, 0, 2, 1), None);
        // A short buffer is refused rather than read out of bounds.
        assert_eq!(sample_region(&buf[..8], 4, 2, 0, 0, 0), None);
    }

    #[test]
    fn hex_formats_uppercase() {
        assert_eq!(hex([0x3b, 0x82, 0xf6]), "#3B82F6");
    }
}
