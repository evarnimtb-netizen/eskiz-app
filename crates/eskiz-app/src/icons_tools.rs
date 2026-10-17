//! Original thin-line glyphs for the toolbar labels that had no icon yet: the selection
//! combine and file actions, the shape and transform modes, and the guide types. Drawn in
//! code on the same 24 x 24 grid as `icons.rs`. No image assets.

use eframe::egui::{Color32, Painter, Rect, Shape, Stroke};

use crate::icons::{arc, quad, rounded_square, Grid, IconFn};

/// Minus sign: Subtract.
pub fn icon_minus(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(5.0, 12.0), (19.0, 12.0)]);
}

/// Disc with its left half filled: Invert.
pub fn icon_invert(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(12.0, 12.0, 8.5);
    let half = arc(12.0, 12.0, 8.5, 90.0, 270.0, 16);
    p.add(Shape::convex_polygon(g.points(&half), c, Stroke::NONE));
}

/// Open tray with an arrow pointing down into it: Save.
pub fn icon_save(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(5.0, 14.0), (5.0, 19.5), (19.0, 19.5), (19.0, 14.0)]);
    g.line(&[(12.0, 3.5), (12.0, 15.0)]);
    g.line(&[(8.0, 11.0), (12.0, 15.0), (16.0, 11.0)]);
}

/// Open tray with an arrow pointing up out of it: Load.
pub fn icon_load(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(5.0, 14.0), (5.0, 19.5), (19.0, 19.5), (19.0, 14.0)]);
    g.line(&[(12.0, 15.0), (12.0, 3.5)]);
    g.line(&[(8.0, 7.0), (12.0, 3.5), (16.0, 7.0)]);
}

/// A square whose edge dissolves into dots: Feather.
pub fn icon_feather(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(7.0, 7.0, 17.0, 17.0, 1.5));
    for (x, y) in [
        (3.5, 12.0),
        (20.5, 12.0),
        (12.0, 3.5),
        (12.0, 20.5),
        (5.0, 5.0),
        (19.0, 5.0),
        (5.0, 19.0),
        (19.0, 19.0),
    ] {
        g.circle(x, y, 0.8);
    }
}

/// Pentagon with a vertex at each corner: the Polygon shape.
pub fn icon_polygon(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(12.0, 3.5), (20.0, 9.0), (17.0, 20.0), (7.0, 20.0), (4.0, 9.0)]);
}

/// Quadrilateral with a dot at each corner: Freeform transform.
pub fn icon_freeform(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let corners = [(5.0, 7.5), (19.0, 4.5), (20.0, 18.5), (6.5, 20.0)];
    g.closed(&corners);
    for (x, y) in corners {
        g.circle(x, y, 1.2);
    }
}

/// A square with a diagonal arrow out of its corner: Uniform scale.
pub fn icon_uniform(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(3.5, 3.5), (13.5, 3.5), (13.5, 13.5), (3.5, 13.5)]);
    g.line(&[(13.5, 13.5), (20.5, 20.5)]);
    g.line(&[(14.5, 20.5), (20.5, 20.5), (20.5, 14.5)]);
}

/// A trapezoid with an outward pull at each top corner: Distort.
pub fn icon_distort(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(7.5, 6.5), (16.5, 6.5), (20.5, 19.5), (3.5, 19.5)]);
    g.line(&[(5.0, 3.5), (2.5, 6.0)]);
    g.line(&[(19.0, 3.5), (21.5, 6.0)]);
}

/// A framed grid whose inner lines bend: Warp.
pub fn icon_warp(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(3.5, 3.5), (20.5, 3.5), (20.5, 20.5), (3.5, 20.5)]);
    g.line(&quad((3.5, 9.0), (12.0, 13.0), (20.5, 9.0), 10));
    g.line(&quad((3.5, 15.0), (12.0, 11.0), (20.5, 15.0), 10));
    g.line(&quad((9.0, 3.5), (13.0, 12.0), (9.0, 20.5), 10));
    g.line(&quad((15.0, 3.5), (11.0, 12.0), (15.0, 20.5), 10));
}

/// A horseshoe magnet with its tips marked: Snap.
pub fn icon_snap(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(4.0, 4.0), (4.0, 12.0)]);
    g.line(&[(20.0, 4.0), (20.0, 12.0)]);
    g.line(&arc(12.0, 12.0, 8.0, 0.0, 180.0, 16));
    g.line(&[(4.0, 8.0), (8.0, 8.0)]);
    g.line(&[(16.0, 8.0), (20.0, 8.0)]);
}

/// A clockwise arc with an arrowhead at its end: Rotate.
pub fn icon_rotate(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let end = 140.0f32;
    g.line(&arc(12.0, 12.0, 8.0, -90.0, end, 24));
    let (s, co) = end.to_radians().sin_cos();
    let tip = (12.0 + 8.0 * co, 12.0 + 8.0 * s);
    // Travel direction at the end of a clockwise arc, and the way back from the tip.
    let back = (s, -co);
    let arm = |deg: f32| {
        let (sn, cs) = deg.to_radians().sin_cos();
        (tip.0 + (back.0 * cs - back.1 * sn) * 4.0, tip.1 + (back.0 * sn + back.1 * cs) * 4.0)
    };
    g.line(&[arm(-40.0), tip, arm(40.0)]);
}

/// Four corner brackets around a centre square: Fit.
pub fn icon_fit(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(3.5, 8.5), (3.5, 3.5), (8.5, 3.5)]);
    g.line(&[(15.5, 3.5), (20.5, 3.5), (20.5, 8.5)]);
    g.line(&[(20.5, 15.5), (20.5, 20.5), (15.5, 20.5)]);
    g.line(&[(8.5, 20.5), (3.5, 20.5), (3.5, 15.5)]);
    g.closed(&[(8.5, 8.5), (15.5, 8.5), (15.5, 15.5), (8.5, 15.5)]);
}

/// A plain grid of four cells: the 2D grid guide.
pub fn icon_grid(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 20.5, 20.5, 2.5));
    g.line(&[(9.2, 3.5), (9.2, 20.5)]);
    g.line(&[(14.8, 3.5), (14.8, 20.5)]);
    g.line(&[(3.5, 9.2), (20.5, 9.2)]);
    g.line(&[(3.5, 14.8), (20.5, 14.8)]);
}

/// A hexagon with the three inner edges of a cube: the isometric guide.
pub fn icon_isometric(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(12.0, 3.0), (20.0, 7.5), (20.0, 16.5), (12.0, 21.0), (4.0, 16.5), (4.0, 7.5)]);
    g.line(&[(4.0, 7.5), (12.0, 12.0), (20.0, 7.5)]);
    g.line(&[(12.0, 12.0), (12.0, 21.0)]);
}

/// Every glyph in this module, for tests and the icon viewer.
pub const ALL: [(&str, IconFn); 15] = [
    ("select subtract", icon_minus),
    ("select invert", icon_invert),
    ("select save", icon_save),
    ("select load", icon_load),
    ("select feather", icon_feather),
    ("select polygon", icon_polygon),
    ("transform freeform", icon_freeform),
    ("transform uniform", icon_uniform),
    ("transform distort", icon_distort),
    ("transform warp", icon_warp),
    ("transform snap", icon_snap),
    ("transform rotate", icon_rotate),
    ("transform fit", icon_fit),
    ("guide grid", icon_grid),
    ("guide isometric", icon_isometric),
];

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
