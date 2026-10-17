//! A compact colour picker for the Layers panel's background colour: a saturation x
//! brightness square, a hue bar and a row of paper presets. It shares the colour maths of
//! the Colour panel ([`crate::colour`]); only the small layout is its own.

use eframe::egui::{
    self, epaint::Mesh, Color32, CornerRadius, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2,
};

use crate::colour::{hsv_to_rgb, Hsv};
use crate::theme::{self, Palette};

/// Height of the picker, points.
pub const PICKER_H: f32 = 190.0;
const PAD: f32 = 10.0;
const SV_H: f32 = 100.0;
const HUE_H: f32 = 22.0;

/// Paper presets: white, cream, two greys, charcoal, black, pale blue, blush.
pub const PRESETS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [245, 239, 224],
    [217, 217, 217],
    [138, 138, 138],
    [43, 43, 45],
    [0, 0, 0],
    [220, 233, 247],
    [247, 225, 225],
];

/// What the picker did this frame.
#[derive(Default)]
pub struct PickerOut {
    /// A new colour from dragging the square or the hue bar.
    pub dragged: Option<[u8; 3]>,
    /// The drag ended (or the square or bar was tapped).
    pub ended: bool,
    /// A preset was tapped.
    pub preset: Option<[u8; 3]>,
}

/// Where the parts are inside a picker rectangle.
pub fn parts(rect: Rect) -> (Rect, Rect, Rect) {
    let w = rect.width() - 2.0 * PAD;
    let sv = Rect::from_min_size(rect.min + Vec2::splat(PAD), Vec2::new(w, SV_H));
    let hue = Rect::from_min_size(Pos2::new(sv.left(), sv.bottom() + 10.0), Vec2::new(w, HUE_H));
    let presets = Rect::from_min_size(Pos2::new(sv.left(), hue.bottom() + 10.0), Vec2::new(w, 28.0));
    (sv, hue, presets)
}

fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn loupe(p: &egui::Painter, at: Pos2, fill: Color32) {
    p.circle_filled(at, 9.0, fill);
    p.circle_stroke(at, 9.0, Stroke::new(2.5, Color32::WHITE));
    p.circle_stroke(at, 10.5, Stroke::new(1.0, Color32::from_black_alpha(90)));
}

/// Draw the picker in `rect` and handle it. `hsv` is the colour being edited (kept as HSV so
/// the hue survives passing through grey).
pub fn compact_picker(ui: &mut egui::Ui, pal: &Palette, rect: Rect, hsv: &mut Hsv) -> PickerOut {
    let mut out = PickerOut::default();
    let p = ui.painter().clone();
    let frame = CornerRadius::same(12);
    p.add(theme::shadow(pal).as_shape(rect, frame));
    p.rect_filled(rect, frame, pal.panel);
    p.rect_stroke(rect, frame, Stroke::new(1.0, pal.separator), StrokeKind::Inside);
    let (sv, hue, presets) = parts(rect);

    // Saturation x brightness: bilinear vertex colours give exactly v * (white .. hue).
    let pure = c32(hsv_to_rgb(Hsv::new(hsv.h, 1.0, 1.0)));
    let mut m = Mesh::default();
    m.colored_vertex(sv.left_top(), Color32::WHITE);
    m.colored_vertex(sv.right_top(), pure);
    m.colored_vertex(sv.right_bottom(), Color32::BLACK);
    m.colored_vertex(sv.left_bottom(), Color32::BLACK);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
    p.rect_stroke(sv, CornerRadius::same(2), Stroke::new(1.0, pal.hairline), StrokeKind::Outside);

    let r = ui.interact(sv, Id::new("bg-picker-sv"), Sense::click_and_drag());
    if r.is_pointer_button_down_on() {
        if let Some(pos) = r.interact_pointer_pos() {
            hsv.s = ((pos.x - sv.left()) / sv.width()).clamp(0.0, 1.0);
            hsv.v = (1.0 - (pos.y - sv.top()) / sv.height()).clamp(0.0, 1.0);
            out.dragged = Some(hsv_to_rgb(*hsv));
        }
    }
    out.ended |= r.drag_stopped() || r.clicked();
    let at = Pos2::new(sv.left() + sv.width() * hsv.s, sv.top() + sv.height() * (1.0 - hsv.v));
    loupe(&p, at, c32(hsv_to_rgb(*hsv)));

    // Hue bar.
    let mut m = Mesh::default();
    for i in 0..=6 {
        let t = i as f32 / 6.0;
        let c = c32(hsv_to_rgb(Hsv::new(t.min(0.999), 1.0, 1.0)));
        let x = hue.left() + hue.width() * t;
        m.colored_vertex(Pos2::new(x, hue.top()), c);
        m.colored_vertex(Pos2::new(x, hue.bottom()), c);
        if i > 0 {
            let k = (i * 2) as u32;
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k + 1, k);
        }
    }
    p.add(Shape::mesh(m));
    p.rect_stroke(hue, CornerRadius::same(2), Stroke::new(1.0, pal.hairline), StrokeKind::Outside);
    let r = ui.interact(hue, Id::new("bg-picker-hue"), Sense::click_and_drag());
    if r.is_pointer_button_down_on() {
        if let Some(pos) = r.interact_pointer_pos() {
            hsv.h = ((pos.x - hue.left()) / hue.width()).clamp(0.0, 0.999);
            out.dragged = Some(hsv_to_rgb(*hsv));
        }
    }
    out.ended |= r.drag_stopped() || r.clicked();
    loupe(&p, Pos2::new(hue.left() + hue.width() * hsv.h, hue.center().y), pure);

    // Presets.
    let n = PRESETS.len() as f32;
    let gap = 6.0;
    let cell = (presets.width() - gap * (n - 1.0)) / n;
    for (i, c) in PRESETS.iter().enumerate() {
        let b = Rect::from_min_size(
            Pos2::new(presets.left() + i as f32 * (cell + gap), presets.top()),
            Vec2::new(cell, presets.height()),
        );
        p.rect_filled(b, CornerRadius::same(7), c32(*c));
        p.rect_stroke(b, CornerRadius::same(7), Stroke::new(1.0, Color32::from_gray(128).gamma_multiply(0.5)), StrokeKind::Inside);
        if ui.interact(b, Id::new(("bg-preset", i)), Sense::click()).clicked() {
            out.preset = Some(*c);
        }
    }
    out
}
