//! Original thin-line icons for the Actions and Adjustments panels, drawn in code on the same
//! 24 x 24 grid as `icons.rs`. No image assets.

use eframe::egui::{Color32, Painter, Pos2, Rect, Stroke};

use crate::icons::{arc, quad, rotated_rect, rounded_square, Grid, IconFn};

/// Canvas: a sheet with ruler ticks along its top edge.
pub fn icon_canvas(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 20.5, 20.5, 2.5));
    g.line(&[(8.0, 3.5), (8.0, 6.5)]);
    g.line(&[(12.0, 3.5), (12.0, 8.0)]);
    g.line(&[(16.0, 3.5), (16.0, 6.5)]);
    g.line(&[(3.5, 8.0), (6.5, 8.0)]);
    g.line(&[(3.5, 12.0), (8.0, 12.0)]);
    g.line(&[(3.5, 16.0), (6.5, 16.0)]);
}

/// Share: an open box with an arrow leaving it.
pub fn icon_share(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(8.0, 10.0), (5.0, 10.0), (5.0, 20.0), (19.0, 20.0), (19.0, 10.0), (16.0, 10.0)]);
    g.line(&[(12.0, 15.0), (12.0, 3.0)]);
    g.line(&[(8.0, 7.0), (12.0, 3.0), (16.0, 7.0)]);
}

/// Preferences: three sliders with round knobs.
pub fn icon_prefs(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    for (y, knob) in [(6.5, 8.5), (12.0, 15.5), (17.5, 10.0)] {
        g.circle(knob, y, 2.3);
        g.line(&[(3.5, y), (knob - 2.3, y)]);
        g.line(&[(knob + 2.3, y), (20.5, y)]);
    }
}

/// Help: a question mark in a circle.
pub fn icon_help(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 9.0);
    let mut pts = arc(12.0, 9.8, 3.0, 180.0, 400.0, 14);
    pts.push((12.0, 14.0));
    pts.push((12.0, 15.0));
    g.line(&pts);
    g.line(&[(12.0, 17.8), (12.0, 17.8)]);
}

/// A picture: frame, sun and two hills.
pub fn icon_image(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 4.5, 20.5, 19.5, 2.5));
    g.circle(8.6, 9.6, 1.8);
    g.line(&[(4.5, 18.0), (10.0, 12.5), (13.5, 16.0), (16.5, 13.0), (19.5, 16.5)]);
}

/// A photo: camera body, lens and viewfinder bump.
pub fn icon_photo(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.0, 7.0, 21.0, 19.5, 2.5));
    g.line(&[(8.5, 7.0), (10.0, 4.5), (14.0, 4.5), (15.5, 7.0)]);
    g.circle(12.0, 13.2, 3.4);
}

/// Information: an "i" in a circle.
pub fn icon_info(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 9.0);
    g.line(&[(12.0, 7.6), (12.0, 7.6)]);
    g.line(&[(12.0, 11.0), (12.0, 17.0)]);
}

/// Flip horizontally: two triangles mirrored about a dashed axis.
pub fn icon_flip_h(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(12.0, 3.0), (12.0, 6.5)]);
    g.line(&[(12.0, 10.5), (12.0, 13.5)]);
    g.line(&[(12.0, 17.5), (12.0, 21.0)]);
    g.closed(&[(9.5, 7.0), (9.5, 18.0), (3.5, 18.0)]);
    g.closed(&[(14.5, 7.0), (14.5, 18.0), (20.5, 18.0)]);
}

/// Flip vertically: the same, turned a quarter.
pub fn icon_flip_v(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(3.0, 12.0), (6.5, 12.0)]);
    g.line(&[(10.5, 12.0), (13.5, 12.0)]);
    g.line(&[(17.5, 12.0), (21.0, 12.0)]);
    g.closed(&[(7.0, 9.5), (18.0, 9.5), (18.0, 3.5)]);
    g.closed(&[(7.0, 14.5), (18.0, 14.5), (18.0, 20.5)]);
}

/// Crop: two overlapping corner brackets.
pub fn icon_crop(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(7.0, 3.0), (7.0, 17.0), (21.0, 17.0)]);
    g.line(&[(3.0, 7.0), (17.0, 7.0), (17.0, 21.0)]);
}

/// Film strip, for animated exports.
pub fn icon_film(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.0, 4.5, 21.0, 19.5, 2.0));
    g.line(&[(7.5, 4.5), (7.5, 19.5)]);
    g.line(&[(16.5, 4.5), (16.5, 19.5)]);
    for y in [9.0, 12.0, 15.0] {
        g.line(&[(3.0, y), (7.5, y)]);
        g.line(&[(16.5, y), (21.0, y)]);
    }
}

/// A JPEG file: a folded sheet with a hill line.
pub fn icon_file_image(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(5.5, 3.5), (14.5, 3.5), (19.0, 8.0), (19.0, 20.5), (5.5, 20.5)]);
    g.line(&[(14.5, 3.5), (14.5, 8.0), (19.0, 8.0)]);
    g.line(&[(7.5, 17.5), (11.0, 13.0), (13.5, 16.0), (15.0, 14.5), (17.0, 17.5)]);
}

/// A folder, for opening a file.
pub fn icon_folder(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(3.0, 6.0), (9.5, 6.0), (11.5, 8.5), (21.0, 8.5), (21.0, 19.0), (3.0, 19.0)]);
}

/// Sun: light interface.
pub fn icon_sun(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 4.0);
    for k in 0..8 {
        let a = (k as f32 * 45.0).to_radians();
        let (s, co) = a.sin_cos();
        g.line(&[(12.0 + 6.8 * co, 12.0 + 6.8 * s), (12.0 + 9.4 * co, 12.0 + 9.4 * s)]);
    }
}

/// Eskiz look: the pointer nib with a sparkle (a new coat of paint), distinct from the sun.
pub fn icon_eskiz_look(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(4.0, 10.5), (15.0, 15.0), (10.5, 19.0), (6.0, 19.0)]);
    g.line(&[(8.0, 15.0), (12.0, 11.5)]);
    g.line(&[(18.0, 3.0), (18.0, 9.0)]);
    g.line(&[(15.0, 6.0), (21.0, 6.0)]);
}

/// Window with its side panel on the right: right-hand interface.
pub fn icon_sidebar_right(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 4.5, 20.5, 19.5, 2.5));
    g.line(&[(15.5, 4.5), (15.5, 19.5)]);
    g.line(&[(18.0, 9.5), (18.0, 14.5)]);
}

/// Brush cursor: a ring with a centre dot.
pub fn icon_cursor(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 8.0);
    g.line(&[(12.0, 12.0), (12.0, 12.0)]);
}

/// Eyedropper: a squeeze bulb, a collar and a glass tube.
pub fn icon_eyedropper(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rotated_rect(17.2, 6.8, 3.6, 1.7, 45.0));
    g.line(&[(12.2, 9.6), (14.4, 11.8)]);
    g.closed(&[(10.4, 10.6), (13.4, 13.6), (7.2, 19.8), (4.0, 20.2), (4.4, 16.8)]);
}

/// Hold to Shape: a rough loop snapping to an ellipse.
pub fn icon_hold_shape(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut pts = Vec::new();
    for k in 0..=24 {
        let a = (k as f32 / 24.0 * 360.0).to_radians();
        pts.push((12.0 + 8.5 * a.cos(), 11.0 + 6.0 * a.sin()));
    }
    g.line(&pts);
    g.line(&[(15.0, 20.5), (21.0, 20.5)]);
}

/// Reference window: a picture frame holding a small landscape.
pub fn icon_reference(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(3.5, 4.5), (20.5, 4.5), (20.5, 19.5), (3.5, 19.5)]);
    g.circle(8.5, 8.5, 1.8);
    g.line(&[(5.5, 16.0), (10.0, 11.5), (13.0, 14.5), (15.5, 12.0), (18.5, 15.0)]);
}

/// Pressure curve: axes and an S curve.
pub fn icon_curve(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(4.0, 3.5), (4.0, 20.0), (20.5, 20.0)]);
    let mut pts = quad((6.5, 17.5), (11.0, 17.5), (12.5, 12.0), 8);
    pts.extend(quad((12.5, 12.0), (14.0, 6.5), (19.5, 6.5), 8));
    g.line(&pts);
}

/// Two fingertips: pinch, rotate and tap gestures.
pub fn icon_fingers(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(8.0, 15.5, 3.0);
    g.circle(16.0, 8.5, 3.0);
    g.line(&[(10.2, 13.4), (13.8, 10.6)]);
}

/// Clock: hold to repeat.
pub fn icon_clock(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 9.0);
    g.line(&[(12.0, 6.5), (12.0, 12.0), (15.8, 14.4)]);
}

/// Keyboard: a row of keys and a space bar.
pub fn icon_keyboard(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(2.5, 6.0, 21.5, 18.0, 2.5));
    for x in [6.5, 10.0, 13.5, 17.0] {
        g.line(&[(x, 10.0), (x, 10.0)]);
    }
    g.line(&[(7.5, 14.2), (16.5, 14.2)]);
}

/// Pen palm: a hand outline stub, used for palm rejection.
pub fn icon_hand(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut pts = vec![(6.5, 12.5), (6.5, 7.5)];
    pts.extend(arc(8.5, 7.5, 2.0, 180.0, 360.0, 6));
    pts.extend([(10.5, 5.0)]);
    pts.extend(arc(12.5, 5.0, 2.0, 180.0, 360.0, 6));
    pts.extend([(14.5, 6.5)]);
    pts.extend(arc(16.5, 6.5, 2.0, 180.0, 360.0, 6));
    pts.extend([(18.5, 12.0), (18.5, 15.5)]);
    pts.extend(arc(13.5, 15.5, 5.0, 0.0, 90.0, 6));
    pts.extend([(11.0, 20.5), (8.5, 20.5), (6.5, 16.5), (6.5, 12.5)]);
    g.line(&pts);
}

/// Opacity: a droplet half filled by hatching.
pub fn icon_droplet(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut pts = vec![(12.0, 3.5)];
    pts.extend(arc(12.0, 14.0, 6.5, -50.0, 230.0, 20).into_iter().rev());
    g.closed(&pts);
    g.line(&[(6.0, 15.5), (18.0, 15.5)]);
}

/// Blur: concentric soft rings.
pub fn icon_blur(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 2.6);
    g.circle(12.0, 12.0, 6.0);
    g.circle(12.0, 12.0, 9.2);
}

/// Hue, saturation, brightness: a colour wheel with a centre dot and a sun mark.
pub fn icon_hsb(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 9.0);
    g.line(&[(12.0, 12.0), (12.0, 3.0)]);
    g.line(&[(12.0, 12.0), (19.8, 16.5)]);
    g.line(&[(12.0, 12.0), (4.2, 16.5)]);
}

/// Every icon here, for the review window and for tests.
pub const ALL: [(&str, IconFn); 33] = [
    ("eskiz look", icon_eskiz_look),
    ("canvas", icon_canvas),
    ("share", icon_share),
    ("prefs", icon_prefs),
    ("help", icon_help),
    ("image", icon_image),
    ("photo", icon_photo),
    ("info", icon_info),
    ("flip horizontal", icon_flip_h),
    ("flip vertical", icon_flip_v),
    ("crop", icon_crop),
    ("film", icon_film),
    ("file image", icon_file_image),
    ("folder", icon_folder),
    ("sun", icon_sun),
    ("sidebar right", icon_sidebar_right),
    ("cursor", icon_cursor),
    ("eyedropper", icon_eyedropper),
    ("hold_shape", icon_hold_shape),
    ("curve", icon_curve),
    ("fingers", icon_fingers),
    ("clock", icon_clock),
    ("keyboard", icon_keyboard),
    ("hand", icon_hand),
    ("droplet", icon_droplet),
    ("blur", icon_blur),
    ("hsb", icon_hsb),
    ("reference", icon_reference),
    ("sharpen", icon_sharpen),
    ("noise", icon_noise),
    ("chromatic aberration", icon_chromatic),
    ("glitch", icon_glitch),
    ("halftone", icon_halftone),
];


/// A capital T: a crossbar and a stem, in the same thin line as the other glyphs.
pub fn icon_text(p: &Painter, r: Rect, c: Color32) {
    let stroke = Stroke::new(1.75, c);
    let top = r.top() + r.height() * 0.22;
    let cx = r.center().x;
    p.line_segment([Pos2::new(r.left() + r.width() * 0.18, top), Pos2::new(r.right() - r.width() * 0.18, top)], stroke);
    p.line_segment([Pos2::new(cx, top), Pos2::new(cx, r.bottom() - r.height() * 0.18)], stroke);
}

/// Motion blur: three streaks of different lengths, all running one way.
pub fn icon_motion_blur(p: &Painter, r: Rect, c: Color32) {
    let stroke = Stroke::new(1.75, c);
    for (y, start) in [(0.30, 0.10), (0.50, 0.28), (0.70, 0.10)] {
        let (x0, x1, yy) = (r.left() + r.width() * start, r.left() + r.width() * 0.92, r.top() + r.height() * y);
        p.line_segment([Pos2::new(x0, yy), Pos2::new(x1, yy)], stroke);
    }
}

/// Perspective blur: rays leaving one origin dot, as the blur radiates from it.
pub fn icon_perspective_blur(p: &Painter, r: Rect, c: Color32) {
    let stroke = Stroke::new(1.75, c);
    let o = r.center();
    p.circle_filled(o, 2.4, c);
    for (x, y) in [(0.08, 0.12), (0.92, 0.12), (0.08, 0.88), (0.92, 0.88), (0.5, 0.06)] {
        let end = Pos2::new(r.left() + r.width() * x, r.top() + r.height() * y);
        p.line_segment([o + (end - o) * 0.35, end], stroke);
    }
}

/// Sharpen: a crisp step edge, with a hard corner where the edge turns.
pub fn icon_sharpen(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(3.5, 17.5), (10.5, 17.5), (10.5, 6.5), (20.5, 6.5)]);
}

/// Noise: scattered grains of equal size, filling the square.
pub fn icon_noise(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let s = r.width().min(r.height()) / 24.0;
    for (x, y) in [(5.0, 6.5), (12.0, 4.5), (18.5, 7.0), (8.5, 11.5), (15.5, 12.0), (4.5, 17.5), (11.0, 17.0), (19.0, 17.5)] {
        p.circle_filled(g.pt(x, y), 1.1 * s, c);
    }
}

/// Chromatic aberration: two lenses of a colour fringe, one shifted against the other.
pub fn icon_chromatic(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(9.0, 12.0, 6.5);
    g.circle(15.0, 12.0, 6.5);
}

/// Glitch: three slices of a block, the middle one shifted sideways.
pub fn icon_glitch(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(3.5, 4.5), (15.5, 4.5), (15.5, 9.5), (3.5, 9.5)]);
    g.closed(&[(8.5, 11.0), (20.5, 11.0), (20.5, 15.0), (8.5, 15.0)]);
    g.closed(&[(4.5, 16.5), (14.5, 16.5), (14.5, 20.5), (4.5, 20.5)]);
}

/// Halftone: a grid of dots that grow from the top left to the bottom right.
pub fn icon_halftone(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let s = r.width().min(r.height()) / 24.0;
    for (row, y) in [6.0, 12.0, 18.0].into_iter().enumerate() {
        for (col, x) in [6.0, 12.0, 18.0].into_iter().enumerate() {
            let radius = 0.9 + 0.35 * (row + col) as f32;
            p.circle_filled(g.pt(x, y), radius * s, c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique() {
        let mut names: Vec<&str> = ALL.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }
}
