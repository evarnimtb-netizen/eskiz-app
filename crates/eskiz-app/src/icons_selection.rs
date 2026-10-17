//! Original thin-line icons for the selection tools and the copy menu, drawn in code on the
//! same 24 x 24 grid as `icons.rs`. No image assets.

use eframe::egui::{Color32, Painter, Rect};

use crate::icons::{arc, quad, rounded_square, Grid, IconFn};

/// A dashed straight run from `a` to `b` in `n` dashes (the first and last touch the ends).
fn dashed(g: &Grid, a: (f32, f32), b: (f32, f32), n: usize) {
    for i in 0..n {
        let (t0, t1) = (i as f32 / n as f32, (i as f32 + 0.55) / n as f32);
        let at = |t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        g.line(&[at(t0), at(t1)]);
    }
}

/// Automatic: a wand with a spark at its tip.
pub fn icon_wand(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(4.5, 19.5), (14.0, 10.0)]);
    g.line(&[(6.5, 21.0), (16.0, 11.5)]);
    g.line(&[(4.5, 19.5), (6.5, 21.0)]);
    // Spark.
    g.line(&[(17.5, 3.5), (17.5, 9.0)]);
    g.line(&[(14.75, 6.25), (20.25, 6.25)]);
    g.line(&[(15.6, 4.35), (19.4, 8.15)]);
    g.line(&[(19.4, 4.35), (15.6, 8.15)]);
    g.line(&[(20.5, 14.0), (20.5, 14.0)]);
    g.line(&[(9.0, 4.0), (9.0, 4.0)]);
}

/// Freehand: a loop of rope with a trailing end.
pub fn icon_lasso(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut loop_pts = quad((7.0, 15.0), (1.5, 8.0), (10.0, 4.0), 10);
    loop_pts.extend(quad((10.0, 4.0), (19.0, 2.0), (20.0, 9.0), 10));
    loop_pts.extend(quad((20.0, 9.0), (21.0, 15.5), (12.0, 15.5), 10));
    loop_pts.extend(quad((12.0, 15.5), (9.5, 15.5), (7.0, 15.0), 6));
    g.line(&loop_pts);
    g.line(&quad((9.0, 15.8), (9.5, 19.5), (6.0, 20.5), 8));
}

/// Rectangle: a dashed box.
pub fn icon_select_rect(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    dashed(&g, (4.0, 5.0), (20.0, 5.0), 4);
    dashed(&g, (20.0, 5.0), (20.0, 19.0), 3);
    dashed(&g, (20.0, 19.0), (4.0, 19.0), 4);
    dashed(&g, (4.0, 19.0), (4.0, 5.0), 3);
}

/// Ellipse: a dashed oval.
pub fn icon_select_ellipse(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let n = 9;
    for i in 0..n {
        let a0 = i as f32 * 360.0 / n as f32;
        let pts: Vec<(f32, f32)> = arc(12.0, 12.0, 8.0, a0, a0 + 24.0, 4)
            .into_iter()
            .map(|(x, y)| (12.0 + (x - 12.0) * 1.15, 12.0 + (y - 12.0) * 0.82))
            .collect();
        g.line(&pts);
    }
}

/// Cut: scissors.
pub fn icon_cut(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.circle(6.5, 18.0, 2.7);
    g.circle(17.5, 18.0, 2.7);
    g.line(&[(8.4, 16.0), (18.5, 3.5)]);
    g.line(&[(15.6, 16.0), (5.5, 3.5)]);
}

/// Copy: two sheets, one behind the other.
pub fn icon_copy(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(8.5, 8.5, 20.0, 20.5, 2.0));
    g.line(&[(15.0, 8.5), (15.0, 6.0), (13.0, 4.0), (6.0, 4.0), (4.0, 6.0), (4.0, 14.0), (6.0, 16.0), (8.5, 16.0)]);
}

/// Copy All: sheets stacked under a frame, for the whole picture.
pub fn icon_copy_all(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(6.0, 3.5, 20.5, 15.5, 2.0));
    g.line(&[(3.5, 8.0), (3.5, 17.5), (5.5, 19.5), (15.0, 19.5)]);
    g.line(&[(8.5, 12.5), (11.0, 9.5), (13.0, 11.5), (15.5, 8.5), (18.0, 12.0)]);
}

/// Duplicate: a sheet and its copy shifted aside, with a plus.
pub fn icon_duplicate(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.5, 3.5, 14.5, 14.5, 2.0));
    g.line(&[(18.0, 8.0), (20.5, 8.0), (20.5, 20.5), (8.0, 20.5), (8.0, 18.0)]);
    g.line(&[(9.0, 6.5), (9.0, 11.5)]);
    g.line(&[(6.5, 9.0), (11.5, 9.0)]);
}

/// Cut & Paste: scissors with an arrow into a sheet.
pub fn icon_cut_paste(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(11.0, 11.0, 21.0, 21.0, 2.0));
    g.circle(4.7, 15.5, 1.9);
    g.circle(4.7, 20.0, 1.9);
    g.line(&[(6.2, 16.2), (13.5, 9.0)]);
    g.line(&[(6.2, 19.3), (13.5, 12.5)]);
    g.line(&[(10.0, 4.0), (16.0, 4.0)]);
    g.line(&[(13.5, 1.8), (16.0, 4.0), (13.5, 6.2)]);
}

/// Paste: a clipboard with a sheet.
pub fn icon_paste(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(4.5, 5.0, 19.5, 21.0, 2.5));
    g.closed(&rounded_square(8.5, 2.5, 15.5, 7.5, 1.6));
    g.line(&[(8.5, 12.0), (15.5, 12.0)]);
    g.line(&[(8.5, 15.5), (13.5, 15.5)]);
}

/// Every icon here, for tests and the icon viewer.
pub const ALL: [(&str, IconFn); 10] = [
    ("select wand", icon_wand),
    ("select lasso", icon_lasso),
    ("select rect", icon_select_rect),
    ("select ellipse", icon_select_ellipse),
    ("cut", icon_cut),
    ("copy", icon_copy),
    ("copy all", icon_copy_all),
    ("duplicate (copy)", icon_duplicate),
    ("cut paste", icon_cut_paste),
    ("paste", icon_paste),
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
