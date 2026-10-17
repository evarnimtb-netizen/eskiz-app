//! Colour panel: a current/previous split swatch, three tabs at the
//! bottom (Disc, Classic, Palettes) and a History row of the last 10 used colours.
//!
//! The Disc and Classic pickers are drawn as a few small meshes with vertex colours (a hue
//! ring, a brightness ring, the saturation x brightness disc, one quad for the square), so
//! a frame costs a few thousand vertices and no per-pixel painter calls. The geometry and
//! mapping maths live in [`crate::colour`].

use eframe::egui::{
    self, epaint::Mesh, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, TextEdit, UiBuilder, Vec2,
};

use crate::app::EskizApp;
use crate::chrome::small_label;
use crate::colour::{
    self, format_hex, hsv_to_rgb, parse_hex, rgb_to_hsv, Hsv, Zone, BRIGHT_RING, HISTORY_LEN, HUE_RING, INNER_DISC,
};
use crate::theme::Palette;

/// Height of the tab content area, points. All three tabs use it so the panel keeps its size.
const CONTENT_H: f32 = 268.0;
/// Palette grid: columns, cell size and gap.
const PAL_COLS: usize = 10;
const PAL_CELL: f32 = 28.0;
const PAL_GAP: f32 = 4.0;

/// The three tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColourTab {
    #[default]
    Wheel,
    Square,
    Swatches,
    Schemes,
}

impl ColourTab {
    const ALL: [(ColourTab, &'static str); 4] = [
        (ColourTab::Wheel, "Wheel"),
        (ColourTab::Square, "Square"),
        (ColourTab::Swatches, "Swatches"),
        (ColourTab::Schemes, "Schemes"),
    ];
}

/// State of the Colour panel that is not the colour itself.
#[derive(Clone, Debug)]
pub struct ColourUi {
    pub tab: ColourTab,
    /// The colour being edited as HSV. Kept so that hue survives passing through grey or
    /// black, where RGB cannot represent it.
    pub hsv: Hsv,
    /// Which disc zone the current drag started in.
    zone: Option<Zone>,
    /// Which slider or square is being dragged (Classic tab).
    classic_drag: bool,
    hex_text: String,
    /// Disc centre and radius from the last frame (screen points), for tests.
    pub(crate) disc: Option<(Pos2, f32)>,
    /// Screen rectangles of the "previous" swatch half and the palette "+" cell, for tests.
    pub(crate) prev_rect: Option<Rect>,
    pub(crate) plus_rect: Option<Rect>,
}

impl Default for ColourUi {
    fn default() -> Self {
        Self {
            tab: ColourTab::Wheel,
            hsv: rgb_to_hsv([30, 60, 140]),
            zone: None,
            classic_drag: false,
            hex_text: String::new(),
            disc: None,
            prev_rect: None,
            plus_rect: None,
        }
    }
}

/// The colour to paint for a colour in the document's space (see [`crate::colour_management::show`]).
fn c32(c: [u8; 3]) -> Color32 {
    let [r, g, b] = crate::colour_management::show(c);
    Color32::from_rgb(r, g, b)
}

/// Black or white text, whichever reads on `c`.
fn text_on(c: [u8; 3]) -> Color32 {
    let lum = 0.299 * f32::from(c[0]) + 0.587 * f32::from(c[1]) + 0.114 * f32::from(c[2]);
    if lum > 150.0 {
        Color32::from_black_alpha(190)
    } else {
        Color32::from_white_alpha(230)
    }
}

/// A round "loupe" handle: a ring in the colour it points at.
fn loupe(painter: &egui::Painter, p: Pos2, fill: Color32) {
    painter.circle_filled(p + Vec2::new(0.0, 1.5), 12.0, Color32::from_black_alpha(70));
    painter.circle_filled(p, 10.0, fill);
    painter.circle_stroke(p, 10.0, Stroke::new(3.0, Color32::WHITE));
    painter.circle_stroke(p, 11.75, Stroke::new(1.0, Color32::from_black_alpha(110)));
}

fn offset(centre: Pos2, dir: (f32, f32), r: f32) -> Pos2 {
    centre + Vec2::new(dir.0, dir.1) * r
}

/// A ring segment from angle `a0` to `a1` (clockwise from the top), coloured by `colour_at(t)`
/// with `t` running 0..1 along it.
fn arc_mesh(centre: Pos2, r0: f32, r1: f32, a0: f32, a1: f32, segs: usize, colour_at: impl Fn(f32) -> Color32) -> Mesh {
    let mut m = Mesh::default();
    for i in 0..=segs {
        let t = i as f32 / segs as f32;
        let dir = colour::dir_from_angle(a0 + (a1 - a0) * t);
        let c = colour_at(t);
        m.colored_vertex(offset(centre, dir, r0), c);
        m.colored_vertex(offset(centre, dir, r1), c);
        if i > 0 {
            let k = (i * 2) as u32;
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k + 1, k);
        }
    }
    m
}

/// The saturation x brightness disc for hue `h`: saturation grows to the right,
/// brightness upwards, bent into a circle (see [`colour::disc_to_sv`]).
fn disc_mesh(centre: Pos2, radius: f32, h: f32) -> Mesh {
    const RINGS: usize = 24;
    const SEGS: usize = 96;
    let mut m = Mesh::default();
    let colour = |x: f32, y: f32| {
        let (s, v) = colour::disc_to_sv(x, y);
        c32(hsv_to_rgb(Hsv::new(h, s, v)))
    };
    m.colored_vertex(centre, colour(0.0, 0.0));
    for k in 1..=RINGS {
        let r = k as f32 / RINGS as f32;
        for j in 0..SEGS {
            let (x, y) = colour::dir_from_angle(std::f32::consts::TAU * j as f32 / SEGS as f32);
            m.colored_vertex(centre + Vec2::new(x, y) * (r * radius), colour(x * r, y * r));
        }
    }
    let at = |k: usize, j: usize| (1 + (k - 1) * SEGS + j % SEGS) as u32;
    for j in 0..SEGS {
        m.add_triangle(0, at(1, j), at(1, j + 1));
    }
    for k in 1..RINGS {
        for j in 0..SEGS {
            m.add_triangle(at(k, j), at(k + 1, j), at(k + 1, j + 1));
            m.add_triangle(at(k, j), at(k + 1, j + 1), at(k, j + 1));
        }
    }
    m
}

/// A horizontal gradient bar through `stops`, evenly spaced.
fn gradient_bar(painter: &egui::Painter, rect: Rect, stops: &[Color32]) {
    let mut m = Mesh::default();
    let n = stops.len();
    for (i, c) in stops.iter().enumerate() {
        let x = rect.left() + rect.width() * i as f32 / (n - 1) as f32;
        m.colored_vertex(Pos2::new(x, rect.top()), *c);
        m.colored_vertex(Pos2::new(x, rect.bottom()), *c);
        if i > 0 {
            let k = (i * 2) as u32;
            m.add_triangle(k - 2, k - 1, k);
            m.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(Shape::mesh(m));
}

/// What a slider did this frame.
#[derive(Default)]
struct SliderEvent {
    started: bool,
    changed: bool,
    stopped: bool,
}

/// A gradient slider with a round thumb. `value` is 0..1.
/// `name` is its accessible name ("Hue"); the value is added to it as "Hue 40 degrees".
fn gradient_slider(ui: &mut egui::Ui, pal: &Palette, id: Id, rect: Rect, value: &mut f32, stops: &[Color32], (thumb, name): (Color32, &str)) -> SliderEvent {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let mut ev = SliderEvent::default();
    let inset = 10.0;
    let travel = (rect.left() + inset, rect.right() - inset);
    if resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            ev.started = ui.input(|i| i.pointer.primary_pressed());
            let v = ((p.x - travel.0) / (travel.1 - travel.0)).clamp(0.0, 1.0);
            if (v - *value).abs() > 1e-6 || ev.started {
                *value = v;
                ev.changed = true;
            }
        }
    }
    // With keyboard focus, the arrow keys move the value in 1 % steps (5 % with Shift).
    if resp.has_focus() {
        crate::a11y::keep_arrows(ui, id);
        let step = ui.input_mut(|i| {
            let big = i.modifiers.shift;
            let up = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp)
                | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight);
            let down = i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown)
                | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft);
            let unit = if big { 0.05 } else { 0.01 };
            if up {
                unit
            } else if down {
                -unit
            } else {
                0.0
            }
        });
        if step != 0.0 {
            *value = (*value + step).clamp(0.0, 1.0);
            ev.started = true;
            ev.changed = true;
            ev.stopped = true;
        }
    }
    ev.stopped |= resp.drag_stopped() || resp.clicked();
    let shown = if name == "Hue" { format!("{name} {:.0} degrees", *value * 360.0) } else { format!("{name} {:.0}%", *value * 100.0) };
    crate::a11y::slider(&resp, true, shown);
    crate::a11y::focus_ring(ui.painter(), rect, &resp, pal, 6);
    let bar = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 12.0));
    gradient_bar(ui.painter(), bar, stops);
    ui.painter().rect_stroke(bar, CornerRadius::same(2), Stroke::new(1.0, pal.hairline), StrokeKind::Outside);
    let thumb_at = Pos2::new(travel.0 + (travel.1 - travel.0) * *value, rect.center().y);
    loupe(ui.painter(), thumb_at, thumb);
    ev
}

impl EskizApp {
    pub(crate) fn colour_body(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        // Keep the HSV in step with colours that changed elsewhere (eyedropper, history).
        if hsv_to_rgb(self.colour_ui.hsv) != self.colour {
            self.colour_ui.hsv = rgb_to_hsv(self.colour);
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Color").size(17.0).strong());
            crate::user_guide::help_link(ui, &pal, crate::user_guide::Link::Colour);
        });
        ui.add_space(6.0);
        self.colour_swatch(ui, &pal);
        ui.add_space(8.0);
        let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), CONTENT_H), Sense::hover());
        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            // Without this a tab that only paints would let the layout cursor jump back.
            ui.set_min_size(area.size());
            match self.colour_ui.tab {
                ColourTab::Wheel => self.disc_tab(ui, area, &pal),
                ColourTab::Square => self.classic_tab(ui, area, &pal),
                ColourTab::Swatches => self.palettes_tab(ui, area, &pal),
                ColourTab::Schemes => self.harmony_tab(ui, area, &pal),
            }
        });
        ui.add_space(6.0);
        self.history_row(ui, &pal);
        ui.add_space(10.0);
        self.tab_bar(ui, &pal);
    }

    /// The colour is being changed by the user: remember where it came from.
    fn begin_colour_edit(&mut self) {
        self.colour_prev = self.colour;
    }

    /// An edit finished: the colour counts as used.
    fn end_colour_edit(&mut self) {
        let c = self.colour;
        self.note_colour_used(c);
    }

    fn set_hsv(&mut self, hsv: Hsv) {
        self.colour_ui.hsv = hsv;
        self.colour = hsv_to_rgb(hsv);
    }

    /// Choose an exact colour (palette, history, hex, previous swatch).
    pub(crate) fn choose_colour(&mut self, c: [u8; 3]) {
        if c != self.colour {
            self.begin_colour_edit();
        }
        self.colour = c;
        self.colour_ui.hsv = rgb_to_hsv(c);
        self.end_colour_edit();
    }

    /// Large split swatch: current on the left, previous on the right (tap it to restore).
    fn colour_swatch(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 60.0), Sense::hover());
        let (left, right) = {
            let mid = rect.center().x;
            (
                Rect::from_min_max(rect.min, Pos2::new(mid, rect.max.y)),
                Rect::from_min_max(Pos2::new(mid, rect.min.y), rect.max),
            )
        };
        let resp = ui.interact(right, Id::new("colour-previous"), Sense::click());
        crate::a11y::named(&resp, egui::WidgetType::Button, true, format!("Previous color, {}", crate::a11y::colour_label(self.colour_prev)));
        self.colour_ui.prev_rect = Some(right);
        let r = 10;
        let (cur, prev) = (self.colour, self.colour_prev);
        let p = ui.painter();
        p.rect_filled(left, CornerRadius { nw: r, sw: r, ne: 0, se: 0 }, c32(cur));
        p.rect_filled(right, CornerRadius { nw: 0, sw: 0, ne: r, se: r }, c32(prev));
        p.rect_stroke(rect, CornerRadius::same(r), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
        p.text(left.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, "Current", FontId::proportional(11.0), text_on(cur));
        p.text(
            left.left_bottom() + Vec2::new(10.0, -8.0),
            Align2::LEFT_BOTTOM,
            format_hex(cur),
            FontId::monospace(12.0),
            text_on(cur),
        );
        p.text(right.right_top() + Vec2::new(-10.0, 8.0), Align2::RIGHT_TOP, "Previous", FontId::proportional(11.0), text_on(prev));
        p.text(
            right.right_bottom() + Vec2::new(-10.0, -8.0),
            Align2::RIGHT_BOTTOM,
            format_hex(prev),
            FontId::monospace(12.0),
            text_on(prev),
        );
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        if resp.clicked() {
            // Swap, so a second tap goes back.
            let (now, before) = (self.colour, self.colour_prev);
            self.colour = before;
            self.colour_ui.hsv = rgb_to_hsv(before);
            self.colour_prev = now;
            self.end_colour_edit();
        }
    }

    // ------------------------------------------------------------------ Disc

    pub(crate) fn disc_tab(&mut self, ui: &mut egui::Ui, area: Rect, pal: &Palette) {
        let radius = (area.height() / 2.0 - 2.0).min(area.width() / 2.0);
        let centre = area.center();
        let square = Rect::from_center_size(centre, Vec2::splat(radius * 2.0 + 24.0));
        let resp = ui.interact(square, Id::new("colour-disc"), Sense::click_and_drag());
        crate::a11y::named(&resp, egui::WidgetType::Other, true, "Color wheel, hue and brightness");
        self.colour_ui.disc = Some((centre, radius));

        // Input.
        if resp.is_pointer_button_down_on() {
            if let Some(p) = resp.interact_pointer_pos() {
                let d = (p - centre) / radius;
                if ui.input(|i| i.pointer.primary_pressed()) {
                    self.colour_ui.zone = colour::zone_at(d.length());
                    if self.colour_ui.zone.is_some() {
                        self.begin_colour_edit();
                    }
                }
                if let Some(zone) = self.colour_ui.zone {
                    let hsv = colour::pick(zone, d.x, d.y, self.colour_ui.hsv);
                    self.set_hsv(hsv);
                }
            }
        }
        if (resp.drag_stopped() || resp.clicked()) && self.colour_ui.zone.take().is_some() {
            self.end_colour_edit();
        }

        // Drawing.
        let hsv = self.colour_ui.hsv;
        let painter = ui.painter();
        let (hr0, hr1) = (HUE_RING.0 * radius, HUE_RING.1 * radius);
        painter.add(Shape::mesh(arc_mesh(centre, hr0, hr1, 0.0, std::f32::consts::TAU, 180, |t| {
            c32(hsv_to_rgb(Hsv::new(t, 1.0, 1.0)))
        })));
        painter.circle_stroke(centre, hr1, Stroke::new(2.0, pal.panel));
        painter.circle_stroke(centre, hr0, Stroke::new(2.0, pal.panel));

        let (br0, br1) = (BRIGHT_RING.0 * radius, BRIGHT_RING.1 * radius);
        let sweep = 0.75 * std::f32::consts::PI;
        let full = c32(hsv_to_rgb(Hsv::new(hsv.h, hsv.s, 1.0)));
        painter.add(Shape::mesh(arc_mesh(centre, br0, br1, -sweep, sweep, 72, |t| {
            Color32::from_rgb(0, 0, 0).lerp_to_gamma(full, t)
        })));
        let cap = (br1 - br0) / 2.0;
        let mid = (br0 + br1) / 2.0;
        painter.circle_filled(offset(centre, colour::brightness_dir(0.0), mid), cap, Color32::BLACK);
        painter.circle_filled(offset(centre, colour::brightness_dir(1.0), mid), cap, full);

        painter.add(Shape::mesh(disc_mesh(centre, INNER_DISC * radius, hsv.h)));
        painter.circle_stroke(centre, INNER_DISC * radius, Stroke::new(2.0, pal.panel));

        // Handles.
        let cur = c32(self.colour);
        let hue_pos = offset(centre, colour::hue_dir(hsv.h), (hr0 + hr1) / 2.0);
        loupe(painter, hue_pos, c32(hsv_to_rgb(Hsv::new(hsv.h, 1.0, 1.0))));
        let b_pos = offset(centre, colour::brightness_dir(hsv.v), mid);
        let grey = (hsv.v * 255.0).round() as u8;
        loupe(painter, b_pos, Color32::from_rgb(grey, grey, grey));
        let (dx, dy) = colour::sv_to_disc(hsv.s, hsv.v);
        loupe(painter, centre + Vec2::new(dx, dy) * (INNER_DISC * radius), cur);
    }

    // ------------------------------------------------------------------ Classic

    fn classic_tab(&mut self, ui: &mut egui::Ui, area: Rect, pal: &Palette) {
        let w = area.width();
        let square = Rect::from_min_size(area.min, Vec2::new(w, 140.0));
        let hsv = self.colour_ui.hsv;

        // Saturation x brightness square: one quad. White at top-left, the pure hue at
        // top-right, black along the bottom; the bilinear blend is exactly S x V.
        let pure = c32(hsv_to_rgb(Hsv::new(hsv.h, 1.0, 1.0)));
        let mut m = Mesh::default();
        m.colored_vertex(square.left_top(), Color32::WHITE);
        m.colored_vertex(square.right_top(), pure);
        m.colored_vertex(square.right_bottom(), Color32::BLACK);
        m.colored_vertex(square.left_bottom(), Color32::BLACK);
        m.add_triangle(0, 1, 2);
        m.add_triangle(0, 2, 3);
        ui.painter().add(Shape::mesh(m));
        ui.painter().rect_stroke(square, CornerRadius::same(2), Stroke::new(1.0, pal.hairline), StrokeKind::Outside);

        let resp = ui.interact(square, Id::new("colour-square"), Sense::click_and_drag());
        if resp.is_pointer_button_down_on() {
            if let Some(p) = resp.interact_pointer_pos() {
                if ui.input(|i| i.pointer.primary_pressed()) {
                    self.begin_colour_edit();
                    self.colour_ui.classic_drag = true;
                }
                let s = ((p.x - square.left()) / square.width()).clamp(0.0, 1.0);
                let v = (1.0 - (p.y - square.top()) / square.height()).clamp(0.0, 1.0);
                self.set_hsv(Hsv::new(hsv.h, s, v));
            }
        }
        if (resp.drag_stopped() || resp.clicked()) && std::mem::take(&mut self.colour_ui.classic_drag) {
            self.end_colour_edit();
        }
        let hsv = self.colour_ui.hsv;
        let handle = Pos2::new(square.left() + square.width() * hsv.s, square.top() + square.height() * (1.0 - hsv.v));
        loupe(&ui.painter_at(square.expand(14.0)), handle, c32(self.colour));

        // Sliders: hue, saturation, brightness.
        let row = |i: usize| Rect::from_min_size(Pos2::new(area.left(), square.bottom() + 8.0 + i as f32 * 26.0), Vec2::new(w, 26.0));
        let hue_stops: Vec<Color32> = (0..=6).map(|i| c32(hsv_to_rgb(Hsv::new(i as f32 / 6.0, 1.0, 1.0)))).collect();
        let sat_stops = [c32(hsv_to_rgb(Hsv::new(hsv.h, 0.0, hsv.v))), c32(hsv_to_rgb(Hsv::new(hsv.h, 1.0, hsv.v)))];
        let bri_stops = [Color32::BLACK, c32(hsv_to_rgb(Hsv::new(hsv.h, hsv.s, 1.0)))];
        let labels = [("H", hsv.h * 360.0, "°"), ("S", hsv.s * 100.0, "%"), ("B", hsv.v * 100.0, "%")];
        for (i, (name, value, unit)) in labels.iter().enumerate() {
            let r = row(i);
            ui.painter().text(r.left_center() + Vec2::new(2.0, 0.0), Align2::LEFT_CENTER, *name, FontId::proportional(13.0), pal.text_dim);
            ui.painter().text(
                r.right_center() - Vec2::new(2.0, 0.0),
                Align2::RIGHT_CENTER,
                format!("{value:.0}{unit}"),
                FontId::proportional(12.0),
                pal.text,
            );
        }
        let bar = |i: usize| {
            let r = row(i);
            Rect::from_min_max(Pos2::new(r.left() + 18.0, r.top()), Pos2::new(r.right() - 44.0, r.bottom()))
        };
        let mut h = hsv.h;
        let mut s = hsv.s;
        let mut v = hsv.v;
        let hue_thumb = c32(hsv_to_rgb(Hsv::new(hsv.h, 1.0, 1.0)));
        let e0 = gradient_slider(ui, pal, Id::new("colour-h"), bar(0), &mut h, &hue_stops, (hue_thumb, "Hue"));
        let e1 = gradient_slider(ui, pal, Id::new("colour-s"), bar(1), &mut s, &sat_stops, (c32(self.colour), "Saturation"));
        let e2 = gradient_slider(ui, pal, Id::new("colour-b"), bar(2), &mut v, &bri_stops, (c32(self.colour), "Brightness"));
        if e0.started || e1.started || e2.started {
            self.begin_colour_edit();
        }
        if e0.changed || e1.changed || e2.changed {
            // The hue slider reaches 1.0 at its right end, which is red again.
            self.set_hsv(Hsv { h: h.min(0.9999), s, v });
        }
        if e0.stopped || e1.stopped || e2.stopped {
            self.end_colour_edit();
        }

        // Hex field.
        let hex_rect = Rect::from_min_size(Pos2::new(area.left(), area.bottom() - 28.0), Vec2::new(w, 28.0));
        self.hex_field(ui, hex_rect, pal);
    }

    fn hex_field(&mut self, ui: &mut egui::Ui, rect: Rect, pal: &Palette) {
        let id = Id::new("colour-hex");
        let focused = ui.memory(|m| m.has_focus(id));
        if !focused {
            self.colour_ui.hex_text = format_hex(self.colour);
        }
        let mut text = std::mem::take(&mut self.colour_ui.hex_text);
        ui.painter().text(rect.left_center() + Vec2::new(2.0, 0.0), Align2::LEFT_CENTER, "Hex", FontId::proportional(13.0), pal.text_dim);
        let field = Rect::from_min_size(rect.min + Vec2::new(40.0, 0.0), Vec2::new(110.0, rect.height()));
        let resp = ui.put(
            field,
            TextEdit::singleline(&mut text)
                .id(id)
                .font(FontId::monospace(13.0))
                .char_limit(7)
                .vertical_align(egui::Align::Center)
                .desired_width(100.0),
        );
        // The hex value is in the document's colour space; say which.
        ui.painter().text(
            field.right_center() + Vec2::new(10.0, 0.0),
            Align2::LEFT_CENTER,
            self.session.doc.colour_space.label(),
            FontId::proportional(12.0),
            pal.text_dim,
        );
        let digits = text.trim().trim_start_matches('#').len();
        if resp.changed() && digits == 6 {
            if let Some(c) = parse_hex(&text) {
                self.begin_colour_edit();
                self.colour = c;
                self.colour_ui.hsv = rgb_to_hsv(c);
            }
        }
        if resp.lost_focus() {
            match parse_hex(&text) {
                Some(c) => self.choose_colour(c),
                None => text = format_hex(self.colour),
            }
        }
        self.colour_ui.hex_text = text;
    }

    // ------------------------------------------------------------------ Palettes

    fn palettes_tab(&mut self, ui: &mut egui::Ui, area: Rect, pal: &Palette) {
        let count = self.colour_store.palette.len();
        ui.horizontal(|ui| {
            small_label(ui, pal, &format!("{count} colors. Right-click a swatch to remove it."));
            if ui.small_button("Files").on_hover_text("Import, export or take a palette from the canvas").clicked() {
                self.guides.palette_window_open = true;
            }
        });
        ui.add_space(4.0);
        let mut chosen = None;
        let mut removed = None;
        let mut add = false;
        egui::ScrollArea::vertical()
            .max_height(area.height() - 26.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let origin = ui.cursor().min;
                let cell = |i: usize| {
                    let (col, row) = (i % PAL_COLS, i / PAL_COLS);
                    Rect::from_min_size(
                        origin + Vec2::new(col as f32 * (PAL_CELL + PAL_GAP), row as f32 * (PAL_CELL + PAL_GAP)),
                        Vec2::splat(PAL_CELL),
                    )
                };
                let rows = (count + 1).div_ceil(PAL_COLS);
                ui.allocate_space(Vec2::new(area.width(), rows as f32 * (PAL_CELL + PAL_GAP)));
                for i in 0..count {
                    let colour = self.colour_store.palette[i];
                    let r = cell(i);
                    let resp = ui.interact(r, Id::new(("pal-swatch", i)), Sense::click());
                    crate::a11y::toggle(&resp, egui::WidgetType::Button, true, colour == self.colour, format!("Palette color {}, {}", i + 1, crate::a11y::colour_label(colour)));
                    ui.painter().rect_filled(r, CornerRadius::same(6), c32(colour));
                    ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, pal.hairline), StrokeKind::Inside);
                    if colour == self.colour {
                        ui.painter().rect_stroke(r.expand(2.0), CornerRadius::same(8), Stroke::new(2.0, pal.accent), StrokeKind::Outside);
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        chosen = Some(colour);
                    }
                    if resp.secondary_clicked() {
                        removed = Some(i);
                    }
                }
                // The "+" cell adds the current colour.
                let r = cell(count);
                let resp = ui.interact(r, Id::new("pal-add"), Sense::click());
                crate::a11y::named(&resp, egui::WidgetType::Button, true, "Add current color to palette");
                self.colour_ui.plus_rect = Some(r);
                let col = if resp.hovered() { pal.text } else { pal.text_dim };
                ui.painter().rect_filled(r, CornerRadius::same(6), pal.row);
                ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, col), StrokeKind::Inside);
                let c = r.center();
                ui.painter().line_segment([c - Vec2::new(6.0, 0.0), c + Vec2::new(6.0, 0.0)], Stroke::new(1.75, col));
                ui.painter().line_segment([c - Vec2::new(0.0, 6.0), c + Vec2::new(0.0, 6.0)], Stroke::new(1.75, col));
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
                add = resp.clicked();
            });
        if let Some(c) = chosen {
            self.choose_colour(c);
        }
        if let Some(i) = removed {
            self.colour_store.remove_from_palette(i);
        }
        if add {
            let c = self.colour;
            if self.colour_store.add_to_palette(c) {
                self.show_toast("Added to palette", 1.0);
            } else {
                self.show_toast("Already in the palette", 1.0);
            }
        }
    }

    // ------------------------------------------------------------------ History and tabs

    fn history_row(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        small_label(ui, pal, "History");
        ui.add_space(2.0);
        let d = 26.0;
        let gap = (ui.available_width() - d * HISTORY_LEN as f32) / (HISTORY_LEN as f32 - 1.0);
        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), d), Sense::hover());
        let mut chosen = None;
        for i in 0..HISTORY_LEN {
            let c = Pos2::new(row.left() + d / 2.0 + i as f32 * (d + gap), row.center().y);
            match self.colour_store.history.get(i).copied() {
                Some(colour) => {
                    let r = Rect::from_center_size(c, Vec2::splat(d));
                    let resp = ui.interact(r, Id::new(("hist", i)), Sense::click());
                    crate::a11y::toggle(&resp, egui::WidgetType::Button, true, colour == self.colour, format!("Recent color {}, {}", i + 1, crate::a11y::colour_label(colour)));
                    ui.painter().circle_filled(c, d / 2.0, c32(colour));
                    ui.painter().circle_stroke(c, d / 2.0, Stroke::new(1.0, pal.hairline));
                    if colour == self.colour {
                        ui.painter().circle_stroke(c, d / 2.0 + 2.0, Stroke::new(2.0, pal.accent));
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        chosen = Some(colour);
                    }
                }
                None => {
                    ui.painter().circle_stroke(c, d / 2.0 - 1.0, Stroke::new(1.0, pal.separator));
                }
            }
        }
        if let Some(c) = chosen {
            self.choose_colour(c);
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(10), pal.row);
        let seg = rect.width() / ColourTab::ALL.len() as f32;
        for (i, (tab, name)) in ColourTab::ALL.iter().enumerate() {
            let r = Rect::from_min_size(rect.min + Vec2::new(seg * i as f32, 0.0), Vec2::new(seg, rect.height()));
            let resp = ui.interact(r, Id::new(("colour-tab", i)), Sense::click());
            crate::a11y::toggle(&resp, egui::WidgetType::SelectableLabel, true, self.colour_ui.tab == *tab, *name);
            let selected = self.colour_ui.tab == *tab;
            if selected {
                ui.painter().rect_filled(r.shrink(3.0), CornerRadius::same(8), pal.accent_fill);
            }
            let colour = if selected { pal.on_accent } else { pal.text_dim };
            ui.painter().text(r.center(), Align2::CENTER_CENTER, *name, FontId::proportional(13.0), colour);
            if resp.clicked() {
                self.colour_ui.tab = *tab;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Prefs;
    use crate::document::Document;
    use crate::popover::Panel;
    use crate::snapshot::{snapshot_dir, Headless};

    fn open(light: bool) -> Headless {
        let prefs = Prefs { light, ..Prefs::default() };
        let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(1600, 1000), prefs);
        h.app.colour = [214, 92, 64];
        h.app.colour_prev = [30, 60, 140];
        for c in [[200, 40, 40], [40, 160, 90], [250, 200, 40], [60, 90, 200], [120, 60, 160]] {
            h.app.colour_store.push_history(c);
        }
        h.open_panel(Panel::Colour);
        h
    }

    #[test]
    fn writes_colour_panel_snapshots() {
        let dir = snapshot_dir();
        for light in [false, true] {
            let theme = if light { "light" } else { "dark" };
            let mut h = open(light);
            for (tab, name) in [(ColourTab::Wheel, "disc"), (ColourTab::Square, "classic"), (ColourTab::Swatches, "palettes")] {
                h.app.colour_ui.tab = tab;
                h.advance(0.2);
                h.render().save_png(&dir.join(format!("colour_{name}_{theme}.png"))).unwrap();
            }
        }
    }

    #[test]
    fn disc_clicks_pick_hue_brightness_and_saturation() {
        let mut h = open(false);
        h.app.colour_ui.tab = ColourTab::Wheel;
        h.advance(0.1);
        let (c, r) = h.app.colour_ui.disc.expect("disc drawn");
        // Hue ring, a quarter turn clockwise from the top: hue 0.25.
        h.click(c + Vec2::new(r * 0.91, 0.0));
        assert!((h.app.colour_ui.hsv.h - 0.25).abs() < 0.01, "{:?}", h.app.colour_ui.hsv);
        // Brightness ring, straight up: half brightness.
        h.click(c + Vec2::new(0.0, -r * 0.745));
        assert!((h.app.colour_ui.hsv.v - 0.5).abs() < 0.02, "{:?}", h.app.colour_ui.hsv);
        // Inner disc, up and to the right: vivid.
        h.click(c + Vec2::new(r * 0.44, -r * 0.44));
        let hsv = h.app.colour_ui.hsv;
        assert!(hsv.s > 0.85 && hsv.v > 0.85, "{hsv:?}");
        assert_eq!(h.app.colour, hsv_to_rgb(hsv));
        // The picks are in the history, newest first, and the old colour became "previous".
        assert_eq!(h.app.colour_store.history[0], h.app.colour);
        assert_ne!(h.app.colour_prev, h.app.colour);
    }

    #[test]
    fn tapping_previous_restores_it_and_tapping_again_swaps_back() {
        let mut h = open(false);
        let (cur, prev) = (h.app.colour, h.app.colour_prev);
        let tap = h.app.colour_ui.prev_rect.expect("swatch drawn").center();
        h.click(tap);
        assert_eq!(h.app.colour, prev);
        assert_eq!(h.app.colour_prev, cur);
        h.click(tap);
        assert_eq!(h.app.colour, cur);
    }

    #[test]
    fn palette_plus_adds_the_current_colour() {
        let mut h = open(false);
        h.app.colour_ui.tab = ColourTab::Swatches;
        h.advance(0.1);
        assert_eq!(h.app.colour_store.palette.len(), 30);
        let plus = h.app.colour_ui.plus_rect.expect("plus drawn").center();
        h.click(plus);
        assert_eq!(h.app.colour_store.palette.len(), 31, "plus at {plus:?}");
        assert_eq!(h.app.colour_store.palette[30], h.app.colour);
    }
}
