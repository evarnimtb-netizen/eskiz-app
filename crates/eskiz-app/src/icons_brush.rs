//! Original thin-line icons for the Brush Library and the Brush Bench, drawn in code on the
//! same 24 x 24 grid as `icons.rs`. No image assets.

use eframe::egui::{Color32, Painter, Rect};

use crate::icons::{arc, quad, rotated_rect, rounded_square, Grid};

/// Brush set: a stick of charcoal with a smudge of dust (Char).
pub fn icon_set_charcoal(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rotated_rect(10.5, 10.5, 2.8, 8.0, 45.0));
    g.line(&[(13.5, 17.5), (15.5, 19.5), (20.5, 19.5)]);
    for (x, y) in [(17.5, 15.0), (20.5, 16.0), (19.0, 13.0)] {
        g.circle(x, y, 0.5);
    }
}

/// Brush set: a broad nib (Nibs).
pub fn icon_set_calligraphy(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(5.0, 20.0), (4.5, 14.0), (13.0, 4.0), (20.0, 11.0), (10.0, 19.5)]);
    g.line(&[(4.8, 19.4), (11.0, 13.0)]);
    g.circle(11.5, 12.5, 1.2);
}

/// Brush set: a woven texture (Textures).
pub fn icon_set_textures(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 20.5, 20.5, 3.0));
    for k in [9.2, 14.8] {
        g.line(&[(k, 3.5), (k, 20.5)]);
        g.line(&[(3.5, k), (20.5, k)]);
    }
}

/// Brush set: a heart (My brushes).
pub fn icon_set_mine(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut pts = vec![(12.0, 20.0)];
    pts.extend(quad((12.0, 20.0), (2.5, 13.5), (5.0, 7.5), 8));
    pts.extend(quad((5.0, 7.5), (8.0, 3.8), (12.0, 8.0), 8));
    pts.extend(quad((12.0, 8.0), (16.0, 3.8), (19.0, 7.5), 8));
    pts.extend(quad((19.0, 7.5), (21.5, 13.5), (12.0, 20.0), 8));
    g.closed(&pts);
}

/// Studio: stroke path, a wavy line with spaced dabs.
pub fn icon_path(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&quad((3.5, 16.0), (9.0, 2.0), (12.0, 12.0), 10));
    g.line(&quad((12.0, 12.0), (15.0, 22.0), (20.5, 8.0), 10));
    for (x, y) in [(3.5, 19.5), (9.5, 19.5), (15.5, 19.5), (21.0, 19.5)] {
        g.circle(x, y, 0.7);
    }
}

/// Studio: Smoothing, a rough line and its smoothed twin.
pub fn icon_smoothing(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(3.5, 14.0), (6.5, 8.5), (9.0, 15.5), (12.0, 7.5), (15.0, 14.5), (17.5, 9.0), (20.5, 12.0)]);
    g.line(&quad((3.5, 18.5), (12.0, 13.0), (20.5, 18.5), 12));
}

/// Studio: taper, a stroke that thins at both ends.
pub fn icon_taper(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut top = quad((3.5, 12.0), (8.0, 4.5), (12.0, 7.0), 6);
    top.extend(quad((12.0, 7.0), (16.0, 4.5), (20.5, 12.0), 6));
    let mut bottom = quad((20.5, 12.0), (16.0, 19.5), (12.0, 17.0), 6);
    bottom.extend(quad((12.0, 17.0), (8.0, 19.5), (3.5, 12.0), 6));
    top.extend(bottom);
    g.closed(&top);
}

/// Studio: shape, a circle and a square overlapping.
pub fn icon_shape(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(9.5, 9.5, 5.5);
    g.closed(&rounded_square(10.5, 10.5, 20.5, 20.5, 1.5));
}

/// Studio: grain, a tile of scattered speckle.
pub fn icon_grain(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 20.5, 20.5, 3.0));
    for (x, y) in [(8.0, 8.0), (13.5, 7.0), (17.0, 11.0), (7.5, 13.5), (12.0, 12.5), (16.5, 17.0), (10.0, 17.5)] {
        g.circle(x, y, 0.55);
    }
}

/// Studio: rendering, a stack of two glazes.
pub fn icon_rendering(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(12.0, 3.5), (20.5, 8.0), (12.0, 12.5), (3.5, 8.0)]);
    g.line(&[(3.5, 12.0), (12.0, 16.5), (20.5, 12.0)]);
    g.line(&[(3.5, 16.0), (12.0, 20.5), (20.5, 16.0)]);
}

/// Studio: wet mix, two drops running together.
pub fn icon_wet(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut a = vec![(8.5, 4.5)];
    a.extend(arc(8.5, 12.0, 5.0, -37.0, 217.0, 20));
    g.closed(&a);
    let mut b = vec![(16.5, 9.5)];
    b.extend(arc(16.5, 16.0, 4.0, -37.0, 217.0, 16));
    g.closed(&b);
}

/// Studio: colour dynamics, a palette with dabs.
pub fn icon_colour_dynamics(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut pts = arc(12.0, 12.0, 9.0, 30.0, 340.0, 30);
    pts.extend(quad((340f32.to_radians().cos() * 9.0 + 12.0, 340f32.to_radians().sin() * 9.0 + 12.0), (17.0, 14.0), (14.0, 16.5), 4));
    pts.extend(quad((14.0, 16.5), (12.0, 21.0), (30f32.to_radians().cos() * 9.0 + 12.0, 30f32.to_radians().sin() * 9.0 + 12.0), 4));
    g.closed(&pts);
    for (x, y) in [(8.0, 9.0), (12.5, 6.8), (16.5, 9.5)] {
        g.circle(x, y, 1.1);
    }
}

/// Studio: dynamics, a response curve.
pub fn icon_dynamics(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(4.0, 4.0), (4.0, 20.0), (20.0, 20.0)]);
    g.line(&quad((6.5, 18.0), (12.0, 18.0), (18.0, 5.5), 12));
}

/// Studio: Apple-Pencil-style pressure and tilt, a leaning pencil over its arc.
pub fn icon_pencil_tilt(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rotated_rect(14.0, 9.5, 1.9, 7.0, 35.0));
    g.line(&[(9.2, 16.5), (7.5, 20.5), (11.5, 19.0)]);
    g.line(&arc(7.5, 20.5, 6.0, -80.0, -10.0, 8));
}

/// Studio: properties, three sliders.
pub fn icon_properties(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    for (y, knob) in [(6.5, 8.5), (12.0, 15.5), (17.5, 10.0)] {
        g.circle(knob, y, 2.3);
        g.line(&[(3.5, y), (knob - 2.3, y)]);
        g.line(&[(knob + 2.3, y), (20.5, y)]);
    }
}

/// Studio: about, an "i" in a circle.
pub fn icon_about(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 9.0);
    g.line(&[(12.0, 11.0), (12.0, 17.0)]);
    g.line(&[(12.0, 7.6), (12.0, 7.6)]);
}

/// A pencil over a short baseline: Edit.
pub fn icon_edit(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(15.5, 4.0), (20.0, 8.5), (9.0, 19.5), (4.0, 20.5), (5.0, 15.5)]);
    g.line(&[(13.0, 6.5), (17.5, 11.0)]);
}

/// A counter-clockwise arrow: Reset.
pub fn icon_reset(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&arc(12.0, 12.5, 7.5, -60.0, 230.0, 24));
    g.line(&[(5.0, 3.5), (5.0, 8.5), (10.0, 8.5)]);
}

/// Two overlapping sheets: Duplicate.
pub fn icon_duplicate(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(8.5, 8.5, 20.5, 20.5, 2.5));
    g.line(&[(15.5, 8.5), (15.5, 5.5), (13.0, 3.5), (5.5, 3.5), (3.5, 5.5), (3.5, 13.0), (5.5, 15.5), (8.5, 15.5)]);
}

/// A cross: Close.
pub fn icon_close(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(6.0, 6.0), (18.0, 18.0)]);
    g.line(&[(18.0, 6.0), (6.0, 18.0)]);
}

/// A sheet being wiped: Clear the drawing pad.
pub fn icon_clear(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 20.5, 20.5, 3.0));
    g.line(&[(7.5, 16.5), (16.5, 7.5)]);
    g.line(&[(9.5, 18.5), (18.5, 9.5)]);
}

/// Every Brush Library and Studio icon, for the review window.
pub const ALL: [(&str, crate::icons::IconFn); 21] = [
    ("set: charcoal", icon_set_charcoal),
    ("set: calligraphy", icon_set_calligraphy),
    ("set: textures", icon_set_textures),
    ("set: my brushes", icon_set_mine),
    ("studio: stroke path", icon_path),
    ("studio: smoothing", icon_smoothing),
    ("studio: taper", icon_taper),
    ("studio: shape", icon_shape),
    ("studio: grain", icon_grain),
    ("studio: rendering", icon_rendering),
    ("studio: wet mix", icon_wet),
    ("studio: color", icon_colour_dynamics),
    ("studio: dynamics", icon_dynamics),
    ("studio: pencil", icon_pencil_tilt),
    ("studio: properties", icon_properties),
    ("studio: about", icon_about),
    ("edit", icon_edit),
    ("reset", icon_reset),
    ("duplicate (sheets)", icon_duplicate),
    ("close", icon_close),
    ("clear pad", icon_clear),
];
