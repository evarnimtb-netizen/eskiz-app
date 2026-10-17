//! Snapshot and interaction tests for the Layers panel. The snapshots are written to
//! `eskiz/target/snapshots/layers_*.png` (cropped to the panel) for inspection.

use eframe::egui::{self, Pos2, Rect, Vec2};

use crate::app::Prefs;
use crate::document::Document;
use crate::panels::layers::Act;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless, Image};
use eskiz_core::blend::BlendMode;
use eskiz_core::document::{Mask, Props};

/// The ids of the demo document, named for the test that uses them.
#[allow(dead_code)]
struct Ids {
    paper: u64,
    hills: u64,
    sun: u64,
    group: u64,
    body: u64,
    head: u64,
    foliage: u64,
    highlights: u64,
    ink: u64,
    sketch: u64,
    notes: u64,
}

/// Fill an ellipse (premultiplied RGBA8) into layer pixels.
#[allow(clippy::too_many_arguments)]
pub(super) fn ellipse(px: &mut [u8], w: usize, h: usize, cx: f64, cy: f64, rx: f64, ry: f64, c: [u8; 4]) {
    let (x0, x1) = (((cx - rx).floor().max(0.0)) as usize, ((cx + rx).ceil() as usize).min(w));
    let (y0, y1) = (((cy - ry).floor().max(0.0)) as usize, ((cy + ry).ceil() as usize).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            let (dx, dy) = ((x as f64 - cx) / rx, (y as f64 - cy) / ry);
            if dx * dx + dy * dy <= 1.0 {
                let o = (y * w + x) * 4;
                let a = u32::from(c[3]);
                for k in 0..3 {
                    px[o + k] = (u32::from(c[k]) * a / 255) as u8;
                }
                px[o + 3] = c[3];
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rect_fill(px: &mut [u8], w: usize, h: usize, x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 4]) {
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            let o = (y * w + x) * 4;
            let a = u32::from(c[3]);
            for k in 0..3 {
                px[o + k] = (u32::from(c[k]) * a / 255) as u8;
            }
            px[o + 3] = c[3];
        }
    }
}

pub(super) fn blank(w: usize, h: usize) -> Vec<u8> {
    vec![0; w * h * 4]
}

/// A document with a bit of everything, bottom to top: Paper, Hills, Sun, group "Characters"
/// (Body, Head), Foliage (masked), Highlights (clipped), Ink (transparency locked), Sketch (locked),
/// Notes (hidden).
fn demo(w: usize, h: usize) -> (Document, Ids) {
    let mut d = Document::empty(w, h);
    let (fw, fh) = (w as f64, h as f64);
    let mut paper = blank(w, h);
    rect_fill(&mut paper, w, h, 0, 0, w, h, [244, 236, 216, 255]);
    let paper = d.push_layer(None, Props::new("Paper"), None, paper);

    let mut px = blank(w, h);
    ellipse(&mut px, w, h, fw * 0.3, fh * 1.05, fw * 0.6, fh * 0.45, [86, 140, 96, 255]);
    ellipse(&mut px, w, h, fw * 0.85, fh * 1.0, fw * 0.5, fh * 0.35, [64, 112, 90, 255]);
    let hills = d.push_layer(None, Props::new("Hills"), None, px);

    let mut px = blank(w, h);
    ellipse(&mut px, w, h, fw * 0.72, fh * 0.28, fh * 0.14, fh * 0.14, [250, 170, 60, 255]);
    let sun = d.push_layer(None, Props::new("Sun"), None, px);

    let group = d.push_group(None, Props::new("Characters"), None);
    let mut px = blank(w, h);
    rect_fill(&mut px, w, h, (fw * 0.3) as usize, (fh * 0.5) as usize, (fw * 0.45) as usize, (fh * 0.85) as usize, [60, 90, 190, 255]);
    let body = d.push_layer(Some(group), Props::new("Body"), None, px);
    let mut px = blank(w, h);
    ellipse(&mut px, w, h, fw * 0.375, fh * 0.42, fh * 0.08, fh * 0.08, [240, 196, 160, 255]);
    let head = d.push_layer(Some(group), Props::new("Head"), None, px);

    let mut px = blank(w, h);
    for k in 0..7 {
        ellipse(&mut px, w, h, fw * (0.1 + 0.05 * k as f64), fh * (0.55 + 0.05 * (k % 3) as f64), fh * 0.09, fh * 0.09, [40, 120, 60, 255]);
    }
    let mut mask = Mask::filled(w, h, 255);
    for y in 0..h {
        for x in 0..w {
            mask.data[y * w + x] = (255.0 * (1.0 - x as f64 / fw)).clamp(0.0, 255.0) as u8;
        }
    }
    let foliage = d.push_layer(None, Props::new("Foliage"), Some(mask), px);

    let mut px = blank(w, h);
    for k in 0..7 {
        ellipse(&mut px, w, h, fw * (0.1 + 0.05 * k as f64) - 12.0, fh * (0.55 + 0.05 * (k % 3) as f64) - 14.0, fh * 0.03, fh * 0.03, [200, 255, 170, 255]);
    }
    let mut p = Props::new("Highlights");
    p.clipped = true;
    p.blend = BlendMode::Screen;
    p.opacity = 0.8;
    let highlights = d.push_layer(None, p, None, px);

    let mut px = blank(w, h);
    ellipse(&mut px, w, h, fw * 0.55, fh * 0.6, fw * 0.12, fh * 0.03, [30, 30, 40, 255]);
    let mut p = Props::new("Ink");
    p.alpha_lock = true;
    p.blend = BlendMode::Multiply;
    let ink = d.push_layer(None, p, None, px);

    let mut px = blank(w, h);
    ellipse(&mut px, w, h, fw * 0.5, fh * 0.5, fw * 0.3, fh * 0.2, [120, 120, 130, 90]);
    let mut p = Props::new("Sketch");
    p.locked = true;
    p.opacity = 0.55;
    let sketch = d.push_layer(None, p, None, px);

    let mut p = Props::new("Notes");
    p.visible = false;
    let notes = d.push_layer(None, p, None, blank(w, h));
    (
        d,
        Ids {
            paper,
            hills,
            sun,
            group,
            body,
            head,
            foliage,
            highlights,
            ink,
            sketch,
            notes,
        },
    )
}

pub(super) fn open(light: bool, doc: Document) -> Headless {
    let prefs = Prefs { light, ..Prefs::default() };
    let mut h = Headless::new(Vec2::new(1280.0, 860.0), doc, prefs);
    h.open_panel(Panel::Layers);
    h.advance(0.3);
    h
}

pub(super) fn crop(img: &Image, x: usize, y: usize, w: usize, h: usize) -> Image {
    let (w, h) = (w.min(img.width - x), h.min(img.height - y));
    let mut rgba = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        rgba.extend_from_slice(&img.rgba[(row * img.width + x) * 4..(row * img.width + x + w) * 4]);
    }
    Image { width: w, height: h, rgba }
}

pub(super) fn snap(h: &mut Headless, name: &str, light: bool) {
    let img = h.render();
    let theme = if light { "light" } else { "dark" };
    // The panel sits at the right edge, under the top bar.
    let c = crop(&img, 900, 44, 380, 800);
    c.save_png(&snapshot_dir().join(format!("layers_{name}_{theme}.png"))).unwrap();
}

pub(super) fn centre(r: Rect) -> Pos2 {
    r.center()
}

pub(super) fn geo(h: &Headless, id: u64) -> crate::panels::layer_card::CardGeo {
    h.app.layers_ui.geo(id).unwrap_or_else(|| panic!("card {id} was not drawn"))
}

/// Press at `pts[0]`, move through the rest and release at the last point, a frame per move.
pub(super) fn drag(h: &mut Headless, pts: &[Pos2]) {
    h.move_to(pts[0]);
    h.step();
    h.press(pts[0], true);
    h.step();
    for p in &pts[1..] {
        h.move_to(*p);
        h.step();
    }
    h.step();
    h.press(*pts.last().unwrap(), false);
    h.step();
    h.step();
}

/// `n` evenly spaced points from `a` to `b`.
pub(super) fn line(a: Pos2, b: Pos2, n: usize) -> Vec<Pos2> {
    (0..=n).map(|i| a + (b - a) * (i as f32 / n as f32)).collect()
}

pub(super) fn order(h: &Headless) -> Vec<u64> {
    h.app.session.doc.layer_ids()
}

pub(super) fn toast(h: &Headless) -> String {
    h.app.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()
}

pub(super) fn select(h: &mut Headless, id: u64) {
    let g = geo(h, id);
    h.click(Pos2::new(g.name.left() + 4.0, g.card.center().y));
    assert_eq!(h.app.active, id);
}

/// Open the thumbnail menu of the selected layer `id`.
pub(super) fn open_menu(h: &mut Headless, id: u64) {
    select(h, id);
    let g = geo(h, id);
    h.click(centre(g.thumb));
    assert_eq!(h.app.layers_ui.menu, Some(id), "the menu should be open");
    h.advance(0.1);
}

pub(super) fn menu_item(h: &mut Headless, act: Act) {
    let cell = h.app.layers_ui.menu_cells.iter().find(|(a, _)| *a == act).map(|(_, r)| *r).expect("menu item");
    assert!(h.app.popovers.contains(centre(cell)), "menu item {act:?} is outside the panel");
    h.click(centre(cell));
    h.advance(0.1);
}

#[test]
fn writes_layer_panel_snapshots() {
    for light in [false, true] {
        let (doc, ids) = demo(1024, 768);
        let mut h = open(light, doc);
        h.app.active = ids.foliage;
        h.advance(0.2);
        snap(&mut h, "list", light);

        // Inline opacity and blend list on the selected layer.
        let g = geo(&h, ids.ink);
        h.click(centre(g.letter));
        h.advance(0.3);
        snap(&mut h, "inline", light);
        let g = geo(&h, ids.ink);
        h.click(centre(g.letter));
        h.advance(0.2);

        // Swipe actions.
        let g = geo(&h, ids.sketch);
        let from = centre(g.card);
        drag(&mut h, &line(from, from - Vec2::new(110.0, 0.0), 8));
        h.advance(0.3);
        assert_eq!(h.app.layers_ui.revealed, Some(ids.sketch));
        snap(&mut h, "swipe", light);
        let g = geo(&h, ids.ink);
        h.click(centre(g.card));
        h.advance(0.3);

        // The thumbnail menu.
        open_menu(&mut h, ids.foliage);
        snap(&mut h, "menu", light);
        h.click(Pos2::new(960.0, 92.0));
        h.advance(0.2);

        // The background colour picker.
        h.click(centre(h.app.layers_ui.bg_swatch.unwrap()));
        h.advance(0.3);
        snap(&mut h, "picker", light);
        h.click(centre(h.app.layers_ui.bg_swatch.unwrap()));
        h.advance(0.2);
        assert!(!h.app.layers_ui.picker);

        // Renaming in place.
        open_menu(&mut h, ids.foliage);
        menu_item(&mut h, Act::Rename);
        h.advance(0.2);
        h.event(egui::Event::Text("Foliage (back)".to_owned()));
        h.advance(0.1);
        snap(&mut h, "rename", light);
        h.event(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        h.advance(0.2);

        // A drag in progress: the card lifts, and a line (or a highlighted group) shows the drop.
        let from = centre(geo(&h, ids.foliage).card);
        for (name, to) in [
            ("drag", Pos2::new(from.x + 12.0, geo(&h, ids.head).card.bottom() + 3.0)),
            ("dragin", centre(geo(&h, ids.group).card)),
        ] {
            h.move_to(from);
            h.step();
            h.press(from, true);
            h.step();
            for p in line(from, to, 12).into_iter().skip(1) {
                h.move_to(p);
                h.step();
            }
            h.step();
            assert!(h.app.layers_ui.drag.is_some(), "{name}: the card should be lifted");
            snap(&mut h, name, light);
            // Escape puts it back: nothing changes.
            let before = order(&h);
            h.event(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            h.step();
            assert!(h.app.layers_ui.drag.is_none(), "{name}: Escape should drop the lifted card");
            h.press(to, false);
            h.step();
            h.step();
            assert_eq!(order(&h), before, "{name}: Escape should cancel the drag");
            assert!(h.app.layers_ui.drag.is_none());
        }
        let _ = (ids.group, ids.body, ids.head, ids.highlights, ids.sketch, ids.notes);
    }
}
