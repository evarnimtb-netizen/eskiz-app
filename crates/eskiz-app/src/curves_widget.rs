//! Curve editor for the Curves adjustment: a square grid with control points.
//!
//! Click an empty spot to add a point, drag a point to move it (the end points keep their
//! x position), and right-click or double-click an inner point to remove it. The curve is drawn
//! through the same 256-entry table the filter uses ([`eskiz_core::filters::curve_table`]).

use eskiz_core::filters::curve_table;
use eframe::egui::{self, vec2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke};

/// Edit `points` (sorted by x, in [0, 1]²). Returns true when the points changed.
pub fn curves_editor(ui: &mut egui::Ui, id: Id, points: &mut Vec<[f32; 2]>, curve: Color32, background: Color32, grid: Color32) -> bool {
    let side = ui.available_width().clamp(160.0, 260.0);
    let (rect, bg) = ui.allocate_exact_size(vec2(side, side), Sense::click());
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect_filled(rect, CornerRadius::same(6), background);
    for i in 1..4 {
        let f = i as f32 / 4.0;
        let (x, y) = (rect.left() + f * side, rect.top() + f * side);
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0, grid));
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0, grid));
    }
    painter.line_segment([rect.left_bottom(), rect.right_top()], Stroke::new(1.0, grid));

    let to_screen = |x: f32, y: f32| Pos2::new(rect.left() + x * side, rect.bottom() - y * side);
    let mut changed = false;
    let mut remove: Option<usize> = None;
    let mut any_point_hovered = false;
    let n = points.len();
    for i in 0..n {
        let p = to_screen(points[i][0], points[i][1]);
        let hit = Rect::from_center_size(p, vec2(18.0, 18.0));
        let resp = ui.interact(hit, id.with(("point", i)), Sense::click_and_drag());
        any_point_hovered |= resp.hovered() || resp.dragged();
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let x = ((pos.x - rect.left()) / side).clamp(0.0, 1.0);
                let y = ((rect.bottom() - pos.y) / side).clamp(0.0, 1.0);
                let end = i == 0 || i + 1 == n;
                let nx = if end {
                    points[i][0]
                } else {
                    let lo = points[i - 1][0];
                    let hi = points[i + 1][0];
                    x.clamp(lo, hi)
                };
                if (points[i][0] - nx).abs() > 1e-6 || (points[i][1] - y).abs() > 1e-6 {
                    points[i] = [nx, y];
                    changed = true;
                }
            }
        }
        let inner = i != 0 && i + 1 != n;
        if inner && (resp.secondary_clicked() || resp.double_clicked()) {
            remove = Some(i);
        }
    }
    if let Some(i) = remove {
        points.remove(i);
        changed = true;
    } else if bg.clicked() && !any_point_hovered {
        if let Some(pos) = bg.interact_pointer_pos() {
            let x = ((pos.x - rect.left()) / side).clamp(0.0, 1.0);
            let y = ((rect.bottom() - pos.y) / side).clamp(0.0, 1.0);
            points.push([x, y]);
            changed = true;
        }
    }
    if changed {
        points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    }

    let table = curve_table(points);
    let line: Vec<Pos2> = table
        .iter()
        .enumerate()
        .map(|(i, &y)| to_screen(i as f32 / 256.0, y))
        .collect();
    painter.add(egui::Shape::line(line, Stroke::new(2.0, curve)));
    for p in points.iter() {
        let c = to_screen(p[0], p[1]);
        painter.circle_filled(c, 5.0, curve);
        painter.circle_stroke(c, 5.0, Stroke::new(1.0, Color32::from_black_alpha(140)));
    }
    changed
}
