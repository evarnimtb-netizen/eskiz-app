//! Original thin-line icons for the drawing aids (guide, assist, symmetry, reference), drawn
//! in code on the same 24 x 24 grid as `icons.rs`. No image assets.

use eframe::egui::{Color32, Painter, Rect};

use crate::icons::{rounded_square, Grid, IconFn};

/// Drawing guide: rays meeting at a vanishing point on the horizon.
pub fn icon_guide(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(3.5, 10.0), (20.5, 10.0)]);
    for x in [4.5, 9.0, 15.0, 19.5] {
        g.line(&[(12.0, 10.0), (x, 20.5)]);
    }
    g.line(&[(12.0, 10.0), (12.0, 20.5)]);
    g.circle(12.0, 10.0, 1.4);
    g.line(&[(6.5, 15.5), (17.5, 15.5)]);
}

/// Edit Drawing Guide: a grid with a handle.
pub fn icon_guide_edit(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 20.5, 20.5, 2.5));
    g.line(&[(9.2, 3.5), (9.2, 20.5)]);
    g.line(&[(14.8, 3.5), (14.8, 20.5)]);
    g.line(&[(3.5, 9.2), (20.5, 9.2)]);
    g.line(&[(3.5, 14.8), (20.5, 14.8)]);
    g.circle(14.8, 14.8, 2.2);
}

/// Guided layer: a pencil above a ruler edge.
pub fn icon_assist(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(3.5, 16.5), (3.5, 20.5), (20.5, 20.5), (20.5, 16.5)]);
    for x in [7.0, 10.5, 14.0, 17.5] {
        g.line(&[(x, 20.5), (x, 18.0)]);
    }
    g.closed(&[(9.0, 13.5), (16.5, 3.5), (19.5, 5.8), (12.0, 15.8)]);
    g.line(&[(9.0, 13.5), (8.5, 16.0), (12.0, 15.8)]);
}

/// Symmetry: a dashed axis with two mirrored triangles.
pub fn icon_symmetry(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    for y in [3.5, 8.0, 12.5, 17.0] {
        g.line(&[(12.0, y), (12.0, y + 2.5)]);
    }
    g.closed(&[(10.0, 6.0), (3.5, 12.0), (10.0, 19.5)]);
    g.closed(&[(14.0, 6.0), (20.5, 12.0), (14.0, 19.5)]);
}

/// Reference: a small picture window over a larger one.
pub fn icon_reference(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 16.0, 14.0, 2.0));
    g.closed(&rounded_square(9.0, 9.5, 20.5, 20.5, 2.0));
    g.line(&[(10.5, 18.5), (13.5, 14.5), (16.0, 17.0), (18.0, 15.0), (19.5, 17.5)]);
}

/// Every icon here, for tests.
pub const ALL: [(&str, IconFn); 5] = [
    ("guide", icon_guide),
    ("guide edit", icon_guide_edit),
    ("assist", icon_assist),
    ("symmetry", icon_symmetry),
    ("reference", icon_reference),
];
