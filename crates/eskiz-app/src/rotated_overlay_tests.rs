//! Every on-canvas overlay and handle on a rotated view (0, 33, 90 and 180 degrees): synthetic
//! pointer events drag the handles, and the canvas-space result is asserted. Snapshots of Crop &
//! Resize and Transform at 33 degrees are written to `target/snapshots/`.
//!
//! Mouse, pen and touch all reach the canvas as the same `FrameInput` pointer (the pen adds its
//! samples), so one synthetic pointer drives every path.

use crate::app::Prefs;
use crate::document::Document;
use crate::selection_tool::{EditMode, Shape, TransformMode};
use crate::snapshot::{snapshot_dir, Headless};
use eframe::egui::{vec2, Pos2, Vec2};

const ANGLES: [f32; 4] = [0.0, 33.0, 90.0, 180.0];

/// A window with an 800 x 600 canvas, zoomed out a little and turned by `deg` about its middle.
fn rotated(deg: f32) -> Headless {
    let mut h = Headless::new(vec2(1280.0, 860.0), Document::new(800, 600), Prefs::default());
    h.advance(0.5);
    let vp = h.app.viewport;
    h.app.view.zoom_at(vp, vp.center(), 0.7);
    h.app.view.rotate_at(vp, vp.center(), deg.to_radians());
    h.advance(0.1);
    h
}

fn at(h: &Headless, x: f64, y: f64) -> Pos2 {
    h.app.view.canvas_to_screen(h.app.viewport, x, y)
}

fn canvas_at(h: &Headless, p: Pos2) -> (f64, f64) {
    h.app.view.screen_to_canvas(h.app.viewport, p)
}

/// Press at `from`, move to `to` in steps, and release only when `release` is set.
fn drag_with(h: &mut Headless, from: Pos2, to: Pos2, release: bool) {
    h.move_to(from);
    h.step();
    h.press(from, true);
    h.step();
    for i in 1..=12 {
        h.move_to(from + (to - from) * (i as f32 / 12.0));
        h.step();
    }
    if release {
        h.press(to, false);
        h.step();
        h.advance(0.1);
    }
}

fn drag(h: &mut Headless, from: Pos2, to: Pos2) {
    drag_with(h, from, to, true);
}

fn fill(h: &mut Headless, x: std::ops::Range<usize>, y: std::ops::Range<usize>, rgba: [u8; 4]) {
    let (w, hh) = (h.app.session.doc.width, h.app.session.doc.height);
    let id = h.app.session.doc.layer_id(0).unwrap();
    let mut px = h.app.session.doc.layer(id).unwrap().to_rgba();
    assert_eq!(px.len(), w * hh * 4);
    for yy in y {
        for xx in x.clone() {
            px[(yy * w + xx) * 4..(yy * w + xx) * 4 + 4].copy_from_slice(&rgba);
        }
    }
    h.app.session.replace_layer_pixels(id, px, "Fill");
    h.app.needs_full = true;
    h.advance(0.1);
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

// ------------------------------------------------------------------------------ Crop & Resize

#[test]
fn crop_handles_follow_a_rotated_view_and_crop_the_right_pixels() {
    for deg in ANGLES {
        let mut h = rotated(deg);
        fill(&mut h, 150..154, 100..104, [255, 0, 0, 255]);
        // The sheet sits at the top right; keep the canvas clear of it.
        h.app.view.pan(vec2(-230.0, 0.0));
        h.app.open_crop();
        h.advance(0.1);
        // Bottom-right corner handle: the canvas corner (800, 600), pulled to (600, 450).
        let (from, to) = (at(&h, 800.0, 600.0), at(&h, 600.0, 450.0));
        drag(&mut h, from, to);
        let e = h.app.crop.as_ref().unwrap().edges;
        assert!(e[0] == 0 && e[1] == 0 && (e[2] - 600).abs() <= 1 && (e[3] - 450).abs() <= 1, "{deg}: corner drag gave {e:?}");
        // Left edge handle, dragged sideways and a little off-axis: only the left edge moves.
        let (from, to) = (at(&h, 0.0, 225.0), at(&h, 100.0, 225.0));
        drag(&mut h, from, to + vec2(0.0, 6.0));
        let e = h.app.crop.as_ref().unwrap().edges;
        assert!(e[1] == 0 && (e[3] - 450).abs() <= 1 && (e[0] - 100).abs() <= 8, "{deg}: left edge gave {e:?}");
        // Top edge handle.
        let mid = (e[0] + e[2]) as f64 / 2.0;
        let (from, to) = (at(&h, mid, 0.0), at(&h, mid, 50.0));
        drag(&mut h, from, to);
        let e = h.app.crop.as_ref().unwrap().edges;
        assert!((e[1] - 50).abs() <= 1, "{deg}: top edge gave {e:?}");
        assert_eq!(h.app.crop.as_ref().unwrap().rect()[2], e[2] - e[0]);
        // Applying keeps exactly the pixels inside the edges.
        let st = h.app.crop.clone().unwrap();
        h.app.apply_crop(&st).unwrap();
        let d = &h.app.session.doc;
        assert_eq!((d.width as i64, d.height as i64), (e[2] - e[0], e[3] - e[1]), "{deg}");
        let px = d.layer(d.layer_id(0).unwrap()).unwrap();
        let (mx, my) = ((150 - e[0]) as usize, (100 - e[1]) as usize);
        assert_eq!(px.pixel(mx, my), [255, 0, 0, 255], "{deg}: the marked pixel moved to ({mx},{my})");
        assert_eq!(px.pixel(mx + 10, my + 10)[3], 0, "{deg}");
    }
}

#[test]
fn crop_and_resize_snapshot_at_33_degrees() {
    let mut h = rotated(33.0);
    fill(&mut h, 200..600, 150..450, [60, 120, 200, 255]);
    h.app.open_crop();
    h.advance(0.1);
    let (from, to) = (at(&h, 800.0, 600.0), at(&h, 700.0, 520.0));
    drag(&mut h, from, to);
    h.render().save_png(&snapshot_dir().join("crop_resize_33deg.png")).unwrap();
}

// ---------------------------------------------------------------------- selection tools

fn select_drag(h: &mut Headless, shape: Shape, a: Vec2, b: Vec2) {
    h.app.edit_mode = EditMode::Select;
    h.app.sel.shape = shape;
    h.advance(0.1);
    let c = h.app.viewport.center();
    drag(h, c + a, c + b);
}

fn selected(h: &Headless, off: Vec2) -> bool {
    let (x, y) = canvas_at(h, h.app.viewport.center() + off);
    h.app.session.selection().value_at(x.floor() as usize, y.floor() as usize) > 0.5
}

#[test]
fn rectangle_and_ellipse_selections_follow_the_drag_on_screen() {
    for deg in ANGLES {
        // The box the pointer drew: screen (-120,-60) to (100,70) about the middle.
        let mut h = rotated(deg);
        select_drag(&mut h, Shape::Rectangle, vec2(-120.0, -60.0), vec2(100.0, 70.0));
        for p in [vec2(-100.0, -40.0), vec2(80.0, 50.0), vec2(0.0, 0.0), vec2(-100.0, 50.0)] {
            assert!(selected(&h, p), "{deg}: rectangle should hold {p:?}");
        }
        for p in [vec2(-140.0, 0.0), vec2(0.0, -80.0), vec2(120.0, 0.0), vec2(0.0, 90.0)] {
            assert!(!selected(&h, p), "{deg}: rectangle should not hold {p:?}");
        }
        let mut h = rotated(deg);
        select_drag(&mut h, Shape::Ellipse, vec2(-120.0, -60.0), vec2(100.0, 70.0));
        for p in [vec2(-10.0, 5.0), vec2(40.0, 5.0), vec2(-60.0, 5.0)] {
            assert!(selected(&h, p), "{deg}: ellipse should hold {p:?}");
        }
        for p in [vec2(-100.0, -55.0), vec2(80.0, 60.0), vec2(-150.0, 5.0)] {
            assert!(!selected(&h, p), "{deg}: ellipse should not hold {p:?}");
        }
    }
}

#[test]
fn freehand_selection_follows_a_rotated_view() {
    for deg in ANGLES {
        let mut h = rotated(deg);
        h.app.edit_mode = EditMode::Select;
        h.app.sel.shape = Shape::Freehand;
        h.advance(0.1);
        // A triangle in canvas space, traced on screen.
        let tri = [(300.0, 200.0), (500.0, 200.0), (400.0, 400.0), (300.0, 200.0)];
        let pts: Vec<Pos2> = tri.iter().map(|&(x, y)| at(&h, x, y)).collect();
        h.move_to(pts[0]);
        h.step();
        h.press(pts[0], true);
        h.step();
        for w in pts.windows(2) {
            for i in 1..=8 {
                h.move_to(w[0] + (w[1] - w[0]) * (i as f32 / 8.0));
                h.step();
            }
        }
        h.press(pts[3], false);
        h.advance(0.2);
        let sel = h.app.session.selection();
        assert!(sel.value_at(400, 260) > 0.5, "{deg}: inside the triangle");
        assert!(sel.value_at(320, 380) < 0.5 && sel.value_at(480, 380) < 0.5, "{deg}: outside it");
    }
}

#[test]
fn polygon_selection_closes_on_its_first_point_on_a_rotated_view() {
    for deg in ANGLES {
        let mut h = rotated(deg);
        h.app.edit_mode = EditMode::Select;
        h.app.sel.shape = Shape::Polygon;
        h.advance(0.1);
        for (x, y) in [(300.0, 200.0), (500.0, 200.0), (400.0, 400.0), (300.0, 200.0)] {
            let p = at(&h, x, y);
            h.click(p);
        }
        h.advance(0.2);
        let sel = h.app.session.selection();
        assert!(sel.value_at(400, 260) > 0.5 && sel.value_at(320, 380) < 0.5, "{deg}");
    }
}

// ------------------------------------------------------------------------------- Transform

fn transform_fixture(deg: f32) -> Headless {
    let mut h = rotated(deg);
    fill(&mut h, 250..550, 150..450, [200, 80, 40, 255]);
    h.app.begin_transform();
    assert!(h.app.transform.is_some());
    h.advance(0.1);
    h
}

fn quad(h: &Headless) -> [eskiz_select::geom::Point; 4] {
    h.app.transform.as_ref().unwrap().session.quad().unwrap()
}

fn set_mode(h: &mut Headless, m: TransformMode) {
    let t = h.app.transform.as_mut().unwrap();
    t.mode = m;
    if m == TransformMode::Warp {
        t.session.enter_warp().unwrap();
    }
    h.advance(0.1);
}

fn dist(a: eskiz_select::geom::Point, b: eskiz_select::geom::Point) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

#[test]
fn transform_freeform_and_distort_corners_land_on_the_pointer() {
    for deg in ANGLES {
        for mode in [TransformMode::Free, TransformMode::Distort] {
            let mut h = transform_fixture(deg);
            set_mode(&mut h, mode);
            let before = quad(&h);
            let target = (before[2].x + 40.0, before[2].y + 30.0);
            let from = at(&h, f64::from(before[2].x), f64::from(before[2].y));
            let to = at(&h, f64::from(target.0), f64::from(target.1));
            drag(&mut h, from, to);
            let after = quad(&h);
            assert!(close(after[2].x, target.0, 1.5) && close(after[2].y, target.1, 1.5), "{deg} {mode:?}: corner at {:?}", after[2]);
            assert!(dist(after[0], before[0]) < 0.5, "{deg} {mode:?}: the opposite corner stays");
            if mode == TransformMode::Distort {
                assert!(dist(after[1], before[1]) < 0.5 && dist(after[3], before[3]) < 0.5, "{deg}: distort moves one corner");
            }
        }
    }
}

#[test]
fn transform_uniform_corner_keeps_the_aspect_and_grows() {
    for deg in ANGLES {
        let mut h = transform_fixture(deg);
        set_mode(&mut h, TransformMode::Uniform);
        let before = quad(&h);
        let (w0, h0) = (dist(before[0], before[1]), dist(before[0], before[3]));
        let from = at(&h, f64::from(before[2].x), f64::from(before[2].y));
        let to = at(&h, f64::from(before[2].x) + 60.0, f64::from(before[2].y) + 60.0);
        drag(&mut h, from, to);
        let after = quad(&h);
        let (w1, h1) = (dist(after[0], after[1]), dist(after[0], after[3]));
        assert!(w1 > w0 * 1.05 && h1 > h0 * 1.05, "{deg}: grew from {w0}x{h0} to {w1}x{h1}");
        assert!(((w1 / h1) / (w0 / h0) - 1.0).abs() < 0.01, "{deg}: aspect {w0}x{h0} to {w1}x{h1}");
    }
}

#[test]
fn transform_warp_control_points_land_on_the_pointer() {
    for deg in ANGLES {
        let mut h = transform_fixture(deg);
        set_mode(&mut h, TransformMode::Warp);
        let p0 = h.app.transform.as_ref().unwrap().session.warp().unwrap().ctrl[0][0];
        let target = (p0.x + 30.0, p0.y + 20.0);
        let from = at(&h, f64::from(p0.x), f64::from(p0.y));
        let to = at(&h, f64::from(target.0), f64::from(target.1));
        drag(&mut h, from, to);
        let p1 = h.app.transform.as_ref().unwrap().session.warp().unwrap().ctrl[0][0];
        assert!(close(p1.x, target.0, 1.5) && close(p1.y, target.1, 1.5), "{deg}: control point at {p1:?}");
    }
}

#[test]
fn transform_move_and_rotate_handles_follow_the_screen_drag() {
    for deg in ANGLES {
        // Move: the quad shifts by the drag, expressed in canvas pixels.
        let mut h = transform_fixture(deg);
        let before = quad(&h);
        let c = at(&h, 400.0, 300.0);
        let d = vec2(30.0, 50.0);
        drag(&mut h, c, c + d);
        let after = quad(&h);
        let (a, b) = (canvas_at(&h, c), canvas_at(&h, c + d));
        let (ex, ey) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
        assert!(close(after[0].x - before[0].x, ex, 1.5) && close(after[0].y - before[0].y, ey, 1.5), "{deg}: moved ({},{}) want ({ex},{ey})", after[0].x - before[0].x, after[0].y - before[0].y);

        // Rotate: the handle sits outside the top edge on screen; a 30 degree turn of the pointer
        // about the middle turns the box 30 degrees.
        let mut h = transform_fixture(deg);
        h.app.transform.as_mut().unwrap().snap = false;
        let before = quad(&h);
        let corners: Vec<Pos2> = before.iter().map(|p| at(&h, f64::from(p.x), f64::from(p.y))).collect();
        let mid = Pos2::new((corners[0].x + corners[1].x) / 2.0, (corners[0].y + corners[1].y) / 2.0);
        let centre = Pos2::new(corners.iter().map(|p| p.x).sum::<f32>() / 4.0, corners.iter().map(|p| p.y).sum::<f32>() / 4.0);
        let handle = mid + (mid - centre).normalized() * crate::selection_tool::ROTATE_HANDLE_PT;
        let (s, co) = 30f32.to_radians().sin_cos();
        let v = handle - centre;
        let end = centre + vec2(v.x * co - v.y * s, v.x * s + v.y * co);
        drag(&mut h, handle, end);
        let after = quad(&h);
        let angle = |q: &[eskiz_select::geom::Point; 4]| (q[1].y - q[0].y).atan2(q[1].x - q[0].x).to_degrees();
        let turned = (angle(&after) - angle(&before) + 540.0).rem_euclid(360.0) - 180.0;
        assert!((turned - 30.0).abs() < 1.5, "{deg}: turned {turned}");
    }
}

#[test]
fn transform_snapshot_at_33_degrees() {
    let mut h = transform_fixture(33.0);
    let q = quad(&h);
    let (from, to) = (at(&h, f64::from(q[2].x), f64::from(q[2].y)), at(&h, f64::from(q[2].x) + 50.0, f64::from(q[2].y) + 20.0));
    drag(&mut h, from, to);
    h.render().save_png(&snapshot_dir().join("transform_33deg.png")).unwrap();
}

// -------------------------------------------------------------------------- Hold to Shape

#[test]
fn edit_shape_handles_follow_a_rotated_view() {
    use crate::app::ShapeEdit;
    use crate::hold_shape::{self, Kind, Shape as HShape};
    for deg in ANGLES {
        let mut h = rotated(deg);
        let shape = HShape::Rect { c: (400.0, 300.0), w: 200.0, h: 120.0, rot: 0.0 };
        h.app.shape_edit = Some(ShapeEdit {
            base: shape.clone(),
            current: shape.clone(),
            kind: Kind::Rectangle,
            anchor: (400.0, 300.0),
            base_dist: 10.0,
            base_angle: 0.0,
            live: false,
            drag: None,
            edits: h.app.session.edits(),
            previewing: false,
            layer: h.app.active,
            tool: h.app.tool,
        });
        h.advance(0.1);
        // Handle 3: the bottom-right corner (500, 360). Drag it to (560, 400).
        let hs = hold_shape::handles(&shape);
        let (from, to) = (at(&h, hs[3].pos.0, hs[3].pos.1), at(&h, 560.0, 400.0));
        drag_with(&mut h, from, to, false);
        let cur = h.app.shape_edit.as_ref().expect("still editing").current.clone();
        let p = hold_shape::handles(&cur)[3].pos;
        assert!((p.0 - 560.0).abs() < 2.0 && (p.1 - 400.0).abs() < 2.0, "{deg}: corner at {p:?}");
        h.press(to, false);
        h.step();
    }
}

// ---------------------------------------------------------------------------------- Text

#[test]
fn text_box_caret_and_width_handle_follow_a_rotated_view() {
    use crate::text_tool::caret_geometry;
    use eskiz_text::TextLayerExt;
    if eskiz_text::FontLibrary::new().families().is_empty() {
        eprintln!("skipped: no system fonts");
        return;
    }
    for deg in ANGLES {
        let mut h = rotated(deg);
        h.app.add_text_action();
        h.advance(0.1);
        let id = h.app.text.editing.expect("editing a new text layer");
        h.app.text_replace(0..0, "Hello");
        h.advance(0.1);
        let layer = h.app.session.doc.texts.get(&id).unwrap().clone();
        let layout = layer.layout(h.app.text.fonts());
        let t = layer.transform;
        // A click between "Hel" and "lo" puts the caret at 3.
        let (x, top, bottom) = caret_geometry(&layout, 3, 24.0);
        let (cx, cy) = t.apply(x, (top + bottom) / 2.0);
        let p = at(&h, f64::from(cx), f64::from(cy));
        h.click(p);
        h.advance(0.1);
        assert_eq!(h.app.text.caret, 3, "{deg}");
        // The width handle: dragged to 150 layer units from the box origin.
        let hd = h.app.width_handle(&layer, &layout).unwrap();
        let (from, off) = h.app.width_knob(&layer, &layout).unwrap();
        let (tx, ty) = t.apply(150.0, hd[1]);
        let to = at(&h, f64::from(tx), f64::from(ty)) + off;
        drag(&mut h, from, to);
        let w = h.app.session.doc.texts.get(&id).unwrap().box_width;
        assert!((w - 150.0).abs() < 2.0, "{deg}: box width {w}");
    }
}

#[test]
fn a_click_at_the_end_of_a_short_word_places_the_caret_instead_of_dragging_the_width() {
    use eskiz_text::TextLayerExt;
    if eskiz_text::FontLibrary::new().families().is_empty() {
        eprintln!("skipped: no system fonts");
        return;
    }
    // Zoomed out, the word is only a few screen points wide: the knob must still not take the click.
    for (deg, zoom) in [(0.0, 1.0), (0.0, 0.25), (90.0, 0.25)] {
        let mut h = rotated(deg);
        h.app.view.zoom *= zoom;
        h.app.add_text_action();
        h.advance(0.1);
        let id = h.app.text.editing.expect("editing a new text layer");
        h.app.text_replace(0..0, "Hi");
        h.advance(0.1);
        let layer = h.app.session.doc.texts.get(&id).unwrap().clone();
        let layout = layer.layout(h.app.text.fonts());
        let t = layer.transform;
        let width = layer.box_width;
        // A click just inside the right end of the last glyph.
        let b = layout.bounds;
        let (cx, cy) = t.apply(b.x1 - 0.5, (b.y0 + b.y1) / 2.0);
        let p = at(&h, f64::from(cx), f64::from(cy));
        h.click(p);
        h.advance(0.1);
        assert_eq!(h.app.text.caret, 2, "{deg} at zoom {zoom}: the caret goes after the last letter");
        let now = h.app.session.doc.texts.get(&id).unwrap().box_width;
        assert!((now - width).abs() < 0.01, "{deg} at zoom {zoom}: the box width did not change");
    }
}

// ------------------------------------------------------------------------------ Pick Color

#[test]
fn pick_color_samples_the_canvas_under_the_loupe() {
    for deg in ANGLES {
        let mut h = rotated(deg);
        fill(&mut h, 0..400, 0..600, [255, 0, 0, 255]);
        fill(&mut h, 400..800, 0..600, [0, 0, 255, 255]);
        // Holding the Pick Color button starts the eyedropper; the loupe then follows the pointer.
        let button = h.app.modify_rect.center();
        h.press(button, true);
        h.step();
        h.advance(0.5);
        assert!(h.app.eyedrop.is_some(), "{deg}: the hold starts the eyedropper");
        for (x, y, want) in [(700.0, 300.0, [0, 0, 255]), (100.0, 300.0, [255, 0, 0])] {
            let p = at(&h, x, y);
            h.move_to(p);
            h.step();
            assert_eq!(h.app.eyedrop.and_then(|e| e.pos), Some(p), "{deg}: the loupe sits under the pointer");
            assert_eq!(h.app.eyedrop.and_then(|e| e.preview), Some(want), "{deg}: colour under the loupe at ({x},{y})");
        }
        let end = at(&h, 100.0, 300.0);
        h.press(end, false);
        h.step();
        assert_eq!(h.app.colour, [255, 0, 0], "{deg}: release commits the colour under the loupe");
    }
}

// -------------------------------------------------------------------------------- Guides

#[test]
fn guide_lines_sit_on_their_canvas_positions() {
    for deg in ANGLES {
        let mut h = rotated(deg);
        {
            let s = &mut h.app.guides.settings;
            s.visible = true;
            s.opacity = 1.0;
            s.thickness = 3.0;
            s.grid_size = 100.0;
            s.colour = [255, 0, 0];
        }
        h.advance(0.1);
        let img = h.render();
        let redness = |p: Pos2| {
            let mut best = 0i32;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let c = img.pixel((p.x.round() as i32 + dx) as usize, (p.y.round() as i32 + dy) as usize);
                    best = best.max(i32::from(c[0]) - i32::from(c[1]));
                }
            }
            best
        };
        assert!(redness(at(&h, 300.0, 250.0)) > 100, "{deg}: vertical guide x=300");
        assert!(redness(at(&h, 250.0, 300.0)) > 100, "{deg}: horizontal guide y=300");
        assert!(redness(at(&h, 250.0, 250.0)) < 40, "{deg}: nothing between the lines");
    }
}

// ---------------------------------------------------------------------------- Liquify

#[test]
fn liquify_ring_and_push_follow_a_rotated_view() {
    for deg in ANGLES {
        let mut h = rotated(deg);
        fill(&mut h, 350..450, 250..350, [255, 0, 0, 255]);
        assert!(h.app.enter_liquify());
        h.advance(0.1);
        // The ring is drawn around the pointer, whichever way the canvas is turned.
        let p = at(&h, 400.0, 300.0);
        h.move_to(p);
        h.step();
        let img = h.render();
        let r = eskiz_filters::liquify::radius(h.app.liquify.as_ref().unwrap().settings().size) * h.app.view.zoom;
        let bright = (0..360).any(|a| {
            let t = (a as f32).to_radians();
            let q = p + vec2(t.cos(), t.sin()) * r;
            let c = img.pixel(q.x.round() as usize, q.y.round() as usize);
            c[0] > 235 && c[1] > 235 && c[2] > 235
        });
        assert!(bright, "{deg}: ring at radius {r}");
        // A push along canvas +x moves the red block that way.
        let to = at(&h, 520.0, 300.0);
        drag(&mut h, p, to);
        h.app.liquify_done();
        h.advance(0.2);
        let layer = h.app.session.doc.layer(h.app.session.doc.layer_id(0).unwrap()).unwrap();
        let w = h.app.session.doc.width;
        let rgba = layer.to_rgba();
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 0..600 {
            for x in 0..w {
                let a = f64::from(rgba[(y * w + x) * 4 + 3]);
                sx += x as f64 * a;
                sy += y as f64 * a;
                n += a;
            }
        }
        let (cx, cy) = (sx / n, sy / n);
        assert!(cx > 405.0 && (cy - 300.0).abs() < 8.0, "{deg}: centroid moved to ({cx:.1},{cy:.1})");
    }
}

// ---------------------------------------------------------------------------- Brush cursor

#[test]
fn brush_cursor_ring_stays_around_the_pointer_at_every_angle() {
    for deg in ANGLES {
        let mut h = rotated(deg);
        let p = h.app.viewport.center() + vec2(40.0, 25.0);
        h.move_to(p);
        h.step();
        let img = h.render();
        let r = (h.app.tool_size() / 2.0 * h.app.view.zoom).max(1.0);
        let bright = (0..360).any(|a| {
            let t = (a as f32).to_radians();
            let q = p + vec2(t.cos(), t.sin()) * r;
            let c = img.pixel(q.x.round() as usize, q.y.round() as usize);
            c[0] > 235 && c[1] > 235 && c[2] > 235
        });
        assert!(bright, "{deg}: ring at radius {r}");
    }
}
