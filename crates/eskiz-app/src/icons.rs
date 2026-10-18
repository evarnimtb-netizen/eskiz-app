//! Original thin-line icons.
//!
//! Every glyph is drawn with the egui painter as vector paths. Each is authored on a
//! 24 x 24 grid, scaled to the rectangle it is given, and stroked at 1.75 px (scaled
//! with the glyph). There are no image assets and no third-party fonts. `eskiz --icons`
//! opens a window that shows the whole set for review.

use eframe::egui::{self, epaint::PathShape, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, Vec2};

use crate::theme::{self, Palette};

/// Signature shared by every icon: `fn(painter, rect, colour)`.
pub type IconFn = fn(&Painter, Rect, Color32);

/// Stroke width at the 24-unit design size.
const STROKE: f32 = 1.75;

/// Drawing helper: maps 24-unit glyph coordinates into the target rectangle.
pub(crate) struct Grid<'a> {
    painter: &'a Painter,
    origin: Pos2,
    scale: f32,
    stroke: Stroke,
    /// Mirror x (used for the redo arrow).
    flip: bool,
}

impl<'a> Grid<'a> {
    pub(crate) fn new(painter: &'a Painter, rect: Rect, color: Color32) -> Self {
        let scale = rect.width().min(rect.height()) / 24.0;
        Self {
            painter,
            origin: rect.center() - Vec2::splat(12.0 * scale),
            scale,
            stroke: Stroke::new(STROKE * scale, color),
            flip: false,
        }
    }

    pub(crate) fn pt(&self, x: f32, y: f32) -> Pos2 {
        let x = if self.flip { 24.0 - x } else { x };
        self.origin + Vec2::new(x, y) * self.scale
    }

    pub(crate) fn points(&self, pts: &[(f32, f32)]) -> Vec<Pos2> {
        pts.iter().map(|&(x, y)| self.pt(x, y)).collect()
    }

    /// Open polyline with round caps (dots at the ends).
    pub(crate) fn line(&self, pts: &[(f32, f32)]) {
        let points = self.points(pts);
        if let (Some(&first), Some(&last)) = (points.first(), points.last()) {
            let r = self.stroke.width * 0.5;
            self.painter.circle_filled(first, r, self.stroke.color);
            self.painter.circle_filled(last, r, self.stroke.color);
        }
        self.painter.add(Shape::Path(PathShape::line(points, self.stroke)));
    }

    /// Closed polygon.
    pub(crate) fn closed(&self, pts: &[(f32, f32)]) {
        let points = self.points(pts);
        self.painter.add(Shape::Path(PathShape::closed_line(points, self.stroke)));
    }

    pub(crate) fn circle(&self, cx: f32, cy: f32, r: f32) {
        let centre = self.pt(cx, cy);
        self.painter.circle_stroke(centre, r * self.scale, self.stroke);
    }

}

/// Points on a circle arc, from `a0` to `a1` degrees (screen coordinates, y down).
pub(crate) fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let a = (a0 + (a1 - a0) * t).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// Points on a quadratic Bezier curve.
pub(crate) fn quad(p0: (f32, f32), c: (f32, f32), p2: (f32, f32), steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            (
                u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p2.0,
                u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p2.1,
            )
        })
        .collect()
}

/// Corners of a rectangle rotated by `deg` around its centre.
pub(crate) fn rotated_rect(cx: f32, cy: f32, hw: f32, hh: f32, deg: f32) -> [(f32, f32); 4] {
    let (s, c) = deg.to_radians().sin_cos();
    let rot = |x: f32, y: f32| (cx + x * c - y * s, cy + x * s + y * c);
    [rot(-hw, -hh), rot(hw, -hh), rot(hw, hh), rot(-hw, hh)]
}

/// A square with rounded corners, as a closed point list.
pub(crate) fn rounded_square(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Vec<(f32, f32)> {
    let mut pts = Vec::new();
    pts.extend(arc(x1 - r, y0 + r, r, -90.0, 0.0, 4));
    pts.extend(arc(x1 - r, y1 - r, r, 0.0, 90.0, 4));
    pts.extend(arc(x0 + r, y1 - r, r, 90.0, 180.0, 4));
    pts.extend(arc(x0 + r, y0 + r, r, 180.0, 270.0, 4));
    pts
}

/// Side of a toolbar icon button, in points (inside the 36 to 44 px touch range).
pub(crate) const TOOL_BUTTON: f32 = 40.0;

/// A glyph button for a toolbar. `selected` fills it and tints the glyph in accent; a
/// disabled button shows the glyph dimmed and takes no clicks. `tip` is the old text label,
/// shown as the tooltip.
pub(crate) fn icon_button(ui: &mut egui::Ui, icon: IconFn, selected: bool, enabled: bool, tip: &str, pal: &Palette) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(TOOL_BUTTON), sense);
    let painter = ui.painter();
    if selected {
        pal.paint_selected(painter, rect, 10, 0.16);
    } else if enabled && resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(10), pal.row);
    }
    let colour = if !enabled {
        pal.disabled()
    } else if selected {
        pal.primary_text
    } else {
        pal.glyph_idle()
    };
    icon(painter, Rect::from_center_size(rect.center(), Vec2::splat(24.0)), colour);
    let resp = resp.on_hover_text(tip);
    crate::a11y::toggle(&resp, egui::WidgetType::Button, enabled, selected, tip);
    crate::a11y::focus_ring(ui.painter(), rect, &resp, pal, 10);
    resp
}

/// A non-interactive glyph in the same slot as [`icon_button`], for a value that has no
/// action of its own (such as the feather amount). The tooltip still names it.
pub(crate) fn icon_badge(ui: &mut egui::Ui, icon: IconFn, pal: &Palette, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(TOOL_BUTTON), Sense::hover());
    icon(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(24.0)), pal.text_dim);
    resp.on_hover_text(tip)
}

/// Wrench: an open-jaw head with a tapered handle.
pub fn icon_actions(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&arc(15.0, 8.0, 4.8, 40.0, 320.0, 20));
    g.line(&[(12.2, 11.4), (4.0, 19.6)]);
    g.line(&[(14.6, 8.6), (6.2, 16.8)]);
    g.circle(4.3, 19.7, 1.4);
}

/// The Eskiz Actions mark: a pointer nib inside a blueprint corner bracket (the app icon's
/// motif). Replaces the wrench in the Eskiz look.
pub fn icon_actions_eskiz(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(3.0, 9.0), (3.0, 3.0), (9.0, 3.0)]);
    g.closed(&[(7.0, 7.0), (20.0, 12.5), (14.0, 14.0), (12.5, 20.0)]);
}

/// Magic wand with a sparkle.
pub fn icon_adjustments(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(4.0, 20.0), (13.0, 11.0)]);
    g.line(&[(17.0, 2.5), (17.0, 9.5)]);
    g.line(&[(13.5, 6.0), (20.5, 6.0)]);
    g.line(&[(20.0, 13.0), (20.0, 18.0)]);
    g.line(&[(17.5, 15.5), (22.5, 15.5)]);
}

/// Stylised looped S ribbon, drawn as two edges so it reads as a ribbon.
pub fn icon_selection(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let curve = |dx: f32| -> Vec<(f32, f32)> {
        (0..=32)
            .map(|i| {
                let u = i as f32 / 32.0;
                let phi = u * std::f32::consts::TAU;
                (12.0 + 5.0 * phi.sin() + dx, 3.0 + 18.0 * u)
            })
            .collect()
    };
    g.line(&curve(-2.0));
    g.line(&curve(2.0));
}

/// Arrow cursor.
pub fn icon_transform(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[
        (6.0, 3.0),
        (6.0, 19.0),
        (10.2, 15.3),
        (12.8, 21.0),
        (15.2, 20.0),
        (12.7, 14.4),
        (18.5, 14.4),
    ]);
}

/// Paintbrush: handle, ferrule and tip.
pub fn icon_brush(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(20.5, 3.5), (12.5, 11.5)]);
    g.closed(&[(12.3, 9.9), (14.1, 11.7), (10.6, 15.2), (8.8, 13.4)]);
    g.line(&[(8.8, 13.4), (7.6, 17.0), (4.2, 20.6)]);
    g.line(&[(10.6, 15.2), (9.0, 18.4), (5.6, 19.6)]);
}

/// Pointing finger, for smudging.
pub fn icon_smudge(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut pts = vec![(9.5, 13.5), (9.5, 4.5)];
    pts.extend(arc(11.0, 4.5, 1.5, 180.0, 360.0, 8));
    pts.extend([
        (12.5, 11.5),
        (16.5, 12.5),
        (18.2, 15.5),
        (16.5, 20.0),
        (10.5, 21.0),
        (7.0, 18.0),
        (6.2, 15.0),
        (9.0, 13.0),
    ]);
    g.line(&pts);
}

/// Eraser block, rotated, with its wrapper band.
pub fn icon_eraser(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let corners = rotated_rect(12.0, 12.0, 9.5, 5.5, -35.0);
    g.closed(&corners);
    // Band across the block, one third of the way along.
    let (s, co) = (-35.0f32).to_radians().sin_cos();
    let at = |x: f32, y: f32| (12.0 + x * co - y * s, 12.0 + x * s + y * co);
    let x0 = -3.5;
    g.line(&[at(x0, -5.5), at(x0, 5.5)]);
}

/// Two stacked squares.
pub fn icon_layers(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(8.0, 3.0, 21.0, 16.0, 2.5));
    g.closed(&rounded_square(3.0, 8.0, 16.0, 21.0, 2.5));
}

/// Plus sign.
pub fn icon_plus(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(12.0, 5.0), (12.0, 19.0)]);
    g.line(&[(5.0, 12.0), (19.0, 12.0)]);
}

/// Bin with lid and handle.
pub fn icon_trash(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(4.0, 6.0), (20.0, 6.0)]);
    g.line(&[(9.0, 6.0), (9.0, 3.5), (15.0, 3.5), (15.0, 6.0)]);
    g.line(&[(6.0, 6.0), (7.2, 20.5), (16.8, 20.5), (18.0, 6.0)]);
    g.line(&[(10.0, 10.0), (10.0, 17.0)]);
    g.line(&[(14.0, 10.0), (14.0, 17.0)]);
}

/// Tick mark.
pub fn icon_check(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(5.0, 12.5), (10.0, 17.5), (19.0, 7.0)]);
}

/// Eye: almond outline with an iris.
pub fn icon_eye(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut top = quad((2.0, 12.0), (12.0, 0.0), (22.0, 12.0), 12);
    top.extend(quad((22.0, 12.0), (12.0, 24.0), (2.0, 12.0), 12));
    g.closed(&top);
    g.circle(12.0, 12.0, 3.0);
}

/// Curved undo arrow.
pub fn icon_undo(p: &Painter, r: Rect, c: Color32) {
    undo_arrow(p, r, c, false);
}

/// Curved redo arrow (mirror of undo).
pub fn icon_redo(p: &Painter, r: Rect, c: Color32) {
    undo_arrow(p, r, c, true);
}

fn undo_arrow(p: &Painter, r: Rect, c: Color32, flip: bool) {
    let mut g = Grid::new(p, r, c);
    g.flip = flip;
    let mut shaft = vec![(4.0, 9.0), (14.0, 9.0)];
    shaft.extend(arc(14.0, 15.0, 6.0, -90.0, 90.0, 16));
    shaft.push((9.0, 21.0));
    g.line(&shaft);
    g.line(&[(8.0, 5.0), (4.0, 9.0), (8.0, 13.0)]);
}

/// Small rounded square: the modify button glyph.
pub fn icon_modify(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(5.0, 5.0, 19.0, 19.0, 3.0));
}

/// Brush set: pen nib (Inkwell).
pub fn icon_set_inking(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(12.0, 21.0), (6.5, 10.5), (8.5, 3.5), (15.5, 3.5), (17.5, 10.5)]);
    g.line(&[(12.0, 21.0), (12.0, 11.8)]);
    g.circle(12.0, 10.4, 1.3);
}

/// Brush set: pencil (Graphite).
pub fn icon_set_sketching(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(16.5, 3.5), (20.5, 7.5), (9.5, 18.5), (4.0, 20.0), (5.5, 14.5)]);
    g.line(&[(13.8, 6.2), (17.8, 10.2)]);
    g.line(&[(5.5, 14.5), (9.5, 18.5)]);
}

/// Brush set: spray nozzle with a scatter of dots (Mist).
pub fn icon_set_airbrushing(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[(3.5, 9.5), (9.5, 7.5), (9.5, 16.5), (3.5, 14.5)]);
    for (x, y) in [(13.5, 7.5), (17.5, 5.5), (20.5, 9.0), (14.5, 12.0), (18.5, 13.5), (13.5, 16.5), (20.0, 18.0), (16.5, 9.5)] {
        g.circle(x, y, 0.6);
    }
}

/// Brush set: paint drop (Painting).
pub fn icon_set_painting(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    let mut pts = vec![(12.0, 3.0)];
    pts.extend(arc(12.0, 14.5, 6.5, -34.4, 214.4, 28));
    g.closed(&pts);
    g.line(&arc(12.0, 14.5, 3.5, 100.0, 165.0, 6));
}

/// Padlock: rounded body with a shackle.
pub fn icon_lock(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(5.0, 10.5, 19.0, 21.0, 2.5));
    let mut shackle = vec![(8.0, 10.5), (8.0, 8.0)];
    shackle.extend(arc(12.0, 8.0, 4.0, 180.0, 360.0, 10));
    shackle.push((16.0, 10.5));
    g.line(&shackle);
    g.line(&[(12.0, 14.5), (12.0, 17.0)]);
}

/// Chevron pointing right (a collapsed group).
pub fn icon_chevron_right(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(9.0, 5.0), (16.0, 12.0), (9.0, 19.0)]);
}

/// Chevron pointing down (an expanded group).
pub fn icon_chevron_down(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(5.0, 9.0), (12.0, 16.0), (19.0, 9.0)]);
}

/// Folder with a tab: a group of layers.
pub fn icon_folder(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&[
        (3.0, 6.5),
        (3.0, 19.0),
        (21.0, 19.0),
        (21.0, 8.5),
        (11.5, 8.5),
        (9.5, 5.0),
        (4.5, 5.0),
    ]);
}

/// Hook arrow that points down and to the right: marks a clipped layer.
pub fn icon_clip_hook(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.line(&[(5.0, 4.0), (5.0, 15.0), (18.0, 15.0)]);
    g.line(&[(14.0, 11.0), (18.0, 15.0), (14.0, 19.0)]);
}

/// Rounded square with a hole: the layer-mask glyph.
pub fn icon_mask(p: &Painter, r: Rect, c: Color32) {
    let g = Grid::new(p, r, c);
    g.closed(&rounded_square(3.0, 3.0, 21.0, 21.0, 3.5));
    g.circle(12.0, 12.0, 5.0);
}

/// Every icon, with the name shown in the review window.
pub const ALL: [(&str, IconFn); 21] = [
    ("actions (wrench)", icon_actions),
    ("adjustments (wand)", icon_adjustments),
    ("selection (S ribbon)", icon_selection),
    ("transform (arrow)", icon_transform),
    ("brush", icon_brush),
    ("smudge (finger)", icon_smudge),
    ("eraser", icon_eraser),
    ("layers", icon_layers),
    ("plus", icon_plus),
    ("trash", icon_trash),
    ("check", icon_check),
    ("eye", icon_eye),
    ("undo", icon_undo),
    ("redo", icon_redo),
    ("pick color (square)", icon_modify),
    ("lock", icon_lock),
    ("chevron right", icon_chevron_right),
    ("chevron down", icon_chevron_down),
    ("folder (group)", icon_folder),
    ("clip hook", icon_clip_hook),
    ("mask", icon_mask),
];

/// Icon sizes used in the UI.
pub const GLYPH: f32 = 24.0;

/// Every glyph the viewer shows: the core set, then the selection, brush, guide and tool sets.
fn review_items() -> Vec<(&'static str, IconFn)> {
    let mut items = ALL.to_vec();
    items.extend_from_slice(&crate::icons_selection::ALL);
    items.extend_from_slice(&crate::icons_brush::ALL);
    items.extend_from_slice(&crate::icons_guides::ALL);
    items.extend_from_slice(&crate::icons_tools::ALL);
    items
}

/// Viewer for `eskiz --icons`: every icon at large and at UI size, in both modes.
struct Viewer {
    pal: Palette,
    items: Vec<(&'static str, IconFn)>,
}

impl eframe::App for Viewer {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        draw_review(ui, self.pal, &self.items);
    }
}

/// The review sheet: every glyph large and at UI size, in the active and the plain colour.
fn draw_review(ui: &mut egui::Ui, pal: Palette, items: &[(&'static str, IconFn)]) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading("Eskiz icons");
        ui.label("Thin-line vector glyphs, drawn in code. Large: 72 px. Small: 24 px, active in accent.");
        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for &(name, draw) in items {
                    let (cell, _) = ui.allocate_exact_size(Vec2::new(170.0, 190.0), egui::Sense::hover());
                    let painter = ui.painter_at(cell);
                    painter.rect_filled(cell, 12.0, pal.panel);
                    let big = Rect::from_center_size(Pos2::new(cell.center().x, cell.top() + 60.0), Vec2::splat(72.0));
                    draw(&painter, big, pal.text);
                    let small = Rect::from_center_size(
                        Pos2::new(cell.center().x - 30.0, cell.top() + 130.0),
                        Vec2::splat(GLYPH),
                    );
                    draw(&painter, small, theme::ACCENT);
                    let plain = Rect::from_center_size(
                        Pos2::new(cell.center().x + 30.0, cell.top() + 130.0),
                        Vec2::splat(GLYPH),
                    );
                    draw(&painter, plain, pal.text_dim);
                    painter.text(
                        Pos2::new(cell.center().x, cell.bottom() - 14.0),
                        egui::Align2::CENTER_CENTER,
                        name,
                        egui::FontId::proportional(11.0),
                        pal.text_dim,
                    );
                }
            });
        });
    });
}

/// Open the icon review window. Blocks until it closes.
pub fn run_viewer() -> Result<(), String> {
    crate::renderer::run_native(
        "Eskiz icons",
        || eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("Eskiz icons")
                .with_inner_size([900.0, 700.0]),
            ..eframe::NativeOptions::default()
        },
        |cc, _mode| {
            let pal = theme::palette(false);
            theme::apply(&cc.egui_ctx, &pal);
            let app: Box<dyn eframe::App> = Box::new(Viewer { pal, items: review_items() });
            Ok(app)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_has_requested_endpoints() {
        let pts = arc(0.0, 0.0, 2.0, 0.0, 90.0, 4);
        assert_eq!(pts.len(), 5);
        assert!((pts[0].0 - 2.0).abs() < 1e-5 && pts[0].1.abs() < 1e-5);
        assert!(pts[4].0.abs() < 1e-4 && (pts[4].1 - 2.0).abs() < 1e-4);
    }

    #[test]
    fn review_items_have_unique_names_across_sets() {
        let items = review_items();
        let mut names: Vec<&str> = items.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), items.len());
    }

    /// Writes `icons_review.png`: the whole set, as the `--icons` window shows it.
    #[test]
    fn writes_the_review_sheet_snapshot() {
        let pal = theme::palette(false);
        let items = review_items();
        let img = crate::snapshot::render_ui(Vec2::new(900.0, 2900.0), |ui| draw_review(ui, pal, &items));
        let path = crate::snapshot::snapshot_dir().join("icons_review.png");
        img.save_png(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
    }

    #[test]
    fn every_icon_is_listed_once() {
        let mut names: Vec<&str> = ALL.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }
}
