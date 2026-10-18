//! RC5: toasts size to their text, side bar insets, canvas rotation, Hold to Shape drift.
//! Snapshots go to `target/snapshots/rc5/`.

use crate::app::{EskizApp, Prefs};
use crate::gallery::Screen;
use crate::snapshot::Headless;
use crate::storage::{self, Library};
use crate::theme::UiIdentity;
use eframe::egui::{self, vec2, Id, Vec2};
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-rc5-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The app on the canvas of a saved document.
pub(crate) fn canvas_app(name: &str, size: Vec2, prefs: Prefs) -> Headless {
    let dir = temp(name);
    let lib = Library::at(&dir);
    let id = storage::new_id();
    lib.save(&id, "Harbour", &storage::sample_document(600, 400)).unwrap();
    let mut h = Headless::from_app(size, move |ctx| EskizApp::with_library(ctx, prefs, lib));
    h.settle_storage();
    h.advance(0.3);
    let card = h.app.gal.card_rects[0].1;
    h.click(card.center());
    h.advance(0.4);
    h.settle_storage();
    assert!(matches!(h.app.gal.screen, Screen::Canvas));
    h.advance(0.5);
    h
}

pub(crate) fn matrix() -> Vec<(UiIdentity, bool, Vec2)> {
    let mut v = Vec::new();
    for id in [UiIdentity::Classic, UiIdentity::Eskiz] {
        for light in [false, true] {
            for size in [vec2(900.0, 600.0), vec2(1280.0, 860.0)] {
                v.push((id, light, size));
            }
        }
    }
    v
}

fn tag(id: UiIdentity, light: bool, size: Vec2) -> String {
    format!("{}_{}_{}", if id == UiIdentity::Eskiz { "eskiz" } else { "classic" }, if light { "light" } else { "dark" }, size.x as u32)
}

fn out_dir() -> PathBuf {
    let d = crate::snapshot::snapshot_dir().join("rc5");
    let _ = std::fs::create_dir_all(&d);
    d
}

const TOASTS: &[&str] = &[
    "Created Untitled copy",
    "Undo Stroke",
    "Moved to Untitled copy",
    "Duplicated Harbour",
    "Rotation 90°",
    "Select an editable layer to transform.",
    "Leave the 3D view to add pictures to a 2D canvas.",
    "Export folder set. Choose a format below.",
    "No brush could be imported from that file.",
    "Could not delete the brush: the file is read-only and cannot be removed from the library right now",
    "Supercalifragilisticexpialidocious-and-an-unbreakable-long-word-with-no-spaces-at-all-in-it-really",
];

/// Toasts sit on one line up to ~360 pt, wrap only between words beyond that, and clear the top bar.
#[test]
fn toasts_size_to_their_text_in_every_look() {
    for (id, light, size) in matrix() {
        let mut h = canvas_app("toast", size, Prefs { identity: id, light, ..Prefs::default() });
        for (i, text) in TOASTS.iter().enumerate() {
            h.app.show_toast(*text, 30.0);
            h.advance(0.3);
            h.render();
            let rect = h.ctx.memory(|m| m.area_rect(Id::new("toast"))).expect("toast area");
            let t = tag(id, light, size);
            assert!(text.starts_with("Supercali") || rect.width() <= 360.0 + 30.0, "{t} {text}: {rect:?}");
            assert!(rect.top() >= crate::app::BAR_HEIGHT, "{t}: clear of the top bar");
            let one_line = text.len() < 40;
            if one_line {
                assert!(rect.height() < 40.0, "{t} `{text}`: {rect:?} is one line");
                assert!(rect.width() > 60.0 || text.len() < 8, "{t} `{text}` is not a sliver: {rect:?}");
            }
            // No row breaks inside a word (the unbreakable word may only overflow).
            let galley = {
                let ui_text = *text;
                let mut g = None;
                let ctx = h.ctx.clone();
                ctx.fonts_mut(|f| {
                    let mut job = egui::text::LayoutJob::single_section(ui_text.to_owned(), egui::TextFormat::simple(egui::FontId::proportional(13.0), egui::Color32::WHITE));
                    job.wrap.max_width = 360.0;
                    g = Some(f.layout_job(job));
                });
                g.unwrap()
            };
            for row in &galley.rows[..galley.rows.len().saturating_sub(1)] {
                let last = row.glyphs.last().map(|g| g.chr);
                if text.contains(' ') && !text.starts_with("Supercali") {
                    assert!(last.is_some_and(|c| c.is_whitespace()), "{text}: row ends mid-word");
                }
            }
            if i == 0 || text.len() > 90 {
                h.render().save_png(&out_dir().join(format!("toast{i}_{t}.png"))).unwrap();
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Item 2: side bar insets

/// Rows (in points) from the pill's top edge to the handle, and from the redo glyph to the bottom edge.
fn sidebar_insets(h: &mut Headless, right: bool) -> (f32, f32, std::path::PathBuf) {
    let ppp = 2.0;
    let img = h.render_at(ppp);
    let w = h.size.x;
    let x_in = if right { w - 8.0 - 22.0 } else { 8.0 + 22.0 };
    let px = |x: f32, y: usize| img.pixel((x * ppp) as usize, y);
    // The pill's rows, from the layout rule in `chrome::sidebar` (centred, 60 % of the height).
    let sh = h.size.y;
    let ph = (sh * 0.6).clamp(260.0, sh - crate::app::BAR_HEIGHT - 16.0);
    let min_top = crate::app::BAR_HEIGHT + 8.0;
    let ptop = (sh / 2.0 - ph / 2.0).clamp(min_top, (sh - ph - 8.0).max(min_top));
    let (top, bottom) = ((ptop * ppp) as usize, ((ptop + ph) * ppp) as usize - 1);
    let panel = px(x_in - 14.0, top + 56);
    let dev = |p: [u8; 4]| (0..3).map(|i| (i32::from(p[i]) - i32::from(panel[i])).abs()).sum::<i32>();
    let x0 = ((x_in - 5.0) * ppp) as usize;
    let x1 = ((x_in + 5.0) * ppp) as usize;
    let row_has_ink = |y: usize| (x0..x1).any(|x| dev(img.pixel(x, y)) > 14);
    // First ink row below the pill's top hairline (skip 6 px of stroke and corner), and last ink row above the bottom.
    let first_ink = (top + 6..top + 80).find(|&y| row_has_ink(y)).expect("handle ink");
    let last_ink = (bottom.saturating_sub(100)..bottom - 6).rev().find(|&y| row_has_ink(y)).expect("redo ink");
    let top_inset = (first_ink - top) as f32 / ppp;
    let bottom_inset = (bottom - last_ink) as f32 / ppp;
    let path = out_dir().join(format!("sidebar_{}.png", if right { "right" } else { "left" }));
    (top_inset, bottom_inset, path)
}

#[test]
fn side_bar_top_and_bottom_insets_are_equal_and_moderate() {
    for (id, light, size) in matrix() {
        for right in [false, true] {
            let mut h = canvas_app("sidebar", size, Prefs { identity: id, light, sidebar_right: right, ..Prefs::default() });
            let (top, bottom, path) = sidebar_insets(&mut h, right);
            let t = tag(id, light, size);
            let img = h.render_at(1.0);
            if size.x == 900.0 {
                img.save_png(&path.with_file_name(format!("sidebar_{t}_{}.png", if right { "right" } else { "left" }))).unwrap();
            }
            assert!((9.0..=14.0).contains(&top), "{t} right={right}: handle inset {top}");
            assert!((9.0..=14.0).contains(&bottom), "{t} right={right}: redo inset {bottom}");
            assert!((top - bottom).abs() <= 2.0, "{t} right={right}: {top} vs {bottom}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Item 4: canvas rotation

use crate::document::Document;
use eframe::egui::{pos2, Event, Key, Modifiers};

fn bare(size: Vec2) -> Headless {
    let mut h = Headless::new(size, Document::new(800, 600), Prefs::default());
    h.advance(0.5);
    h
}

fn toast_text(h: &Headless) -> String {
    h.app.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default()
}

fn key(h: &mut Headless, key: Key, modifiers: Modifiers) {
    h.event(Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers });
    h.step();
    h.event(Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers });
    h.step();
}

const CMD_ALT: Modifiers = Modifiers { alt: true, ..Modifiers::COMMAND };

#[test]
fn quarter_turns_reset_and_toast_from_the_shortcuts() {
    let mut h = bare(vec2(1280.0, 860.0));
    let centre = h.app.viewport.center();
    let under_centre = h.app.view.screen_to_canvas(h.app.viewport, centre);
    key(&mut h, Key::ArrowRight, CMD_ALT);
    assert!((h.app.view.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-4, "{}", h.app.view.rotation);
    assert_eq!(toast_text(&h), "Rotation 90°");
    let still = h.app.view.screen_to_canvas(h.app.viewport, centre);
    assert!((still.0 - under_centre.0).abs() < 0.5 && (still.1 - under_centre.1).abs() < 0.5, "turns about the middle");
    key(&mut h, Key::ArrowRight, CMD_ALT);
    key(&mut h, Key::ArrowRight, CMD_ALT);
    assert_eq!(toast_text(&h), "Rotation 270°");
    key(&mut h, Key::ArrowRight, CMD_ALT);
    assert!(h.app.view.rotation.abs() < 1e-4, "four right turns are upright: {}", h.app.view.rotation);
    key(&mut h, Key::ArrowLeft, CMD_ALT);
    assert_eq!(toast_text(&h), "Rotation 270°");
    key(&mut h, Key::ArrowLeft, CMD_ALT);
    assert_eq!(toast_text(&h), "Rotation 180°");
    key(&mut h, Key::Num0, CMD_ALT);
    assert!(h.app.view.rotation == 0.0);
    assert_eq!(toast_text(&h), "Rotation 0°");
    // From a free angle a quarter turn goes to the next quarter.
    h.app.view.rotation = 0.3;
    key(&mut h, Key::ArrowRight, CMD_ALT);
    assert!((h.app.view.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
    h.app.view.rotation = 0.3;
    key(&mut h, Key::ArrowLeft, CMD_ALT);
    assert!(h.app.view.rotation.abs() < 1e-4);
}

#[test]
fn the_trackpad_rotate_gesture_turns_and_snaps_the_view() {
    let mut h = bare(vec2(1280.0, 860.0));
    let on_canvas = h.app.viewport.center();
    h.move_to(on_canvas);
    h.step();
    h.event(Event::Rotate(0.3));
    h.step();
    assert!((h.app.view.rotation - 0.3).abs() < 1e-4, "{}", h.app.view.rotation);
    assert_eq!(toast_text(&h), "Rotation 17°");
    // Turning back into the band around 0 sticks to exactly 0 and says so.
    h.advance(0.1);
    h.event(Event::Rotate(-0.25));
    h.step();
    assert_eq!(h.app.view.rotation, 0.0);
    // The gesture continues: leaving the band lets go of the snap.
    h.event(Event::Rotate(-0.05));
    h.step();
    assert_eq!(h.app.view.rotation, 0.0, "still stuck inside the band");
    for _ in 0..4 {
        h.event(Event::Rotate(-0.05));
        h.step();
    }
    assert!(h.app.view.rotation < -0.1, "free again: {}", h.app.view.rotation);
    // Near 90 degrees it snaps too.
    h.app.view.rotation = 0.0;
    h.advance(0.1);
    h.event(Event::Rotate(std::f32::consts::FRAC_PI_2 + 0.04));
    h.step();
    assert!((h.app.view.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
}

#[test]
fn r_and_drag_rotates_with_a_pen_or_mouse_and_snaps() {
    let mut h = bare(vec2(1280.0, 860.0));
    let p = h.app.viewport.center();
    h.event(Event::Key { key: Key::R, physical_key: Some(Key::R), pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.move_to(p);
    h.step();
    h.press(p, true);
    h.step();
    for i in 1..=30 {
        h.move_to(pos2(p.x + i as f32 * 5.0, p.y));
        h.step();
    }
    h.press(pos2(p.x + 150.0, p.y), false);
    h.step();
    assert!(h.app.view.rotation > 1.0, "dragging right turned the view clockwise: {}", h.app.view.rotation);
    assert!(!h.app.session.is_stroking(), "R-drag does not paint");
    assert!(toast_text(&h).starts_with("Rotation"));
    h.event(Event::Key { key: Key::R, physical_key: Some(Key::R), pressed: false, repeat: false, modifiers: Modifiers::NONE });
    h.step();
}

#[test]
fn painting_follows_a_rotated_view() {
    for deg in [90.0f32, 180.0, 270.0, 33.0] {
        let mut h = bare(vec2(1280.0, 860.0));
        let idx = h.app.tool.index();
        h.app.tool_params[idx].size = 10.0;
        let vp = h.app.viewport;
        h.app.rotate_view_quarter(1);
        if deg != 90.0 {
            h.app.view.rotation = deg.to_radians();
        }
        h.advance(0.1);
        let a = h.app.view.canvas_to_screen(vp, 300.0, 300.0);
        let b = h.app.view.canvas_to_screen(vp, 500.0, 300.0);
        let base = h.app.session.display_pixels();
        h.move_to(a);
        h.step();
        h.press(a, true);
        h.step();
        for i in 1..=20 {
            h.move_to(a + (b - a) * (i as f32 / 20.0));
            h.step();
        }
        h.press(b, false);
        h.step();
        h.advance(0.3);
        let px = h.app.session.display_pixels();
        let w = h.app.session.doc.width;
        let ink = |x: usize, y: usize| px[y * w + x] != base[y * w + x];
        assert!(ink(400, 300), "{deg}: ink on the canvas line");
        assert!(ink(330, 300) && ink(470, 300), "{deg}: the whole line, in canvas space");
        assert!(!ink(400, 360) && !ink(300, 450), "{deg}: no ink off the line");
    }
}

#[test]
fn the_brush_cursor_stays_under_the_pointer_on_a_rotated_view() {
    let mut h = bare(vec2(1280.0, 860.0));
    h.app.rotate_view_quarter(-1);
    let p = h.app.viewport.center() + vec2(40.0, 25.0);
    h.move_to(p);
    h.step();
    let img = h.render();
    // The cursor ring is white 1 px stroked around the pointer: some pixel at the ring radius is bright.
    let r = (h.app.tool_size() / 2.0 * h.app.view.zoom).max(1.0);
    let bright = (0..360).any(|a| {
        let t = (a as f32).to_radians();
        let q = p + vec2(t.cos(), t.sin()) * r;
        let c = img.pixel(q.x.round() as usize, q.y.round() as usize);
        c[0] > 235 && c[1] > 235 && c[2] > 235
    });
    assert!(bright, "ring drawn around the pointer at radius {r}");
}

#[test]
fn selection_and_transform_follow_a_rotated_view() {
    use crate::selection_tool::EditMode;
    let mut h = bare(vec2(1280.0, 860.0));
    h.app.rotate_view_quarter(1);
    h.advance(0.1);
    let vp = h.app.viewport;
    // Rectangle selection from canvas (200,100) to (400,250), dragged on screen.
    h.app.edit_mode = EditMode::Select;
    h.app.sel.shape = crate::selection_tool::Shape::Rectangle;
    h.advance(0.1);
    let a = h.app.view.canvas_to_screen(vp, 200.0, 100.0);
    let b = h.app.view.canvas_to_screen(vp, 400.0, 250.0);
    h.move_to(a);
    h.step();
    h.press(a, true);
    h.step();
    for i in 1..=10 {
        h.move_to(a + (b - a) * (i as f32 / 10.0));
        h.step();
    }
    h.press(b, false);
    h.advance(0.2);
    let r = h.app.session.selection().bounds().expect("a selection");
    let near = |v: usize, want: usize| v.abs_diff(want) <= 2;
    assert!(near(r.x, 200) && near(r.y, 100) && near(r.w, 200) && near(r.h, 150), "selection {r:?}");

    // Transform: dragging inside moves the quad by the on-screen drag, in canvas terms.
    let mut h = bare(vec2(1280.0, 860.0));
    h.app.rotate_view_quarter(1);
    let id = h.app.session.doc.layer_id(0).unwrap();
    let mut px = vec![0u8; 800 * 600 * 4];
    for y in 150..450 {
        for x in 250..550 {
            px[(y * 800 + x) * 4..(y * 800 + x) * 4 + 4].copy_from_slice(&[200, 80, 40, 255]);
        }
    }
    h.app.session.replace_layer_pixels(id, px, "Fill");
    h.app.needs_full = true;
    h.advance(0.1);
    let vp = h.app.viewport;
    h.app.begin_transform();
    assert!(h.app.transform.is_some());
    h.advance(0.1);
    let before = h.app.transform.as_ref().unwrap().session.quad().unwrap();
    let c = h.app.view.canvas_to_screen(vp, 400.0, 300.0);
    h.move_to(c);
    h.step();
    h.press(c, true);
    h.step();
    for i in 1..=10 {
        h.move_to(c + vec2(0.0, 5.0 * i as f32));
        h.step();
    }
    h.press(c + vec2(0.0, 50.0), false);
    h.advance(0.1);
    let after = h.app.transform.as_ref().unwrap().session.quad().unwrap();
    // View rotated 90 degrees clockwise: a screen drag down is a canvas move toward +x.
    let zoom = h.app.view.zoom;
    let dx = after[0].x - before[0].x;
    let dy = after[0].y - before[0].y;
    assert!((dx - 50.0 / zoom).abs() < 2.0 && dy.abs() < 2.0, "quad moved ({dx},{dy}), zoom {zoom}");
}

// ---------------------------------------------------------------------------------------------
// C17: the first segment of a snapped open polyline keeps its width

#[test]
fn the_first_segment_of_a_snapped_polyline_is_not_a_hairline() {
    // Brushes with a start taper: without the fix their first segment ran 2 px wide against 8-10.
    for id in ["inking.studio-ink", "inking.brush-pen-stroke", "painting.soft-oil-round", "inking.mapping-nib", "charcoal.vine-stick"] {
        let mut h = bare(vec2(1280.0, 860.0));
        let idx = h.app.tool.index();
        h.app.tool_brushes[idx] = crate::brush_library::BrushId::Library(id);
        h.app.tool_params[idx].size = 14.0;
        let base = h.app.session.display_pixels();
        let shape = crate::hold_shape::Shape::Polygon { pts: vec![(100.0, 300.0), (400.0, 300.0), (400.0, 100.0)], closed: false };
        assert!(h.app.paint_shape(&shape, 1.0));
        h.advance(0.2);
        let px = h.app.session.display_pixels();
        let w = h.app.session.doc.width;
        let thickness = |x: usize| (250..350).filter(|&y| px[y * w + x] != base[y * w + x]).count();
        let (near_start, middle) = (thickness(104), thickness(250));
        assert!(middle >= 6, "{id}: the line has a body: {middle}");
        assert!(near_start * 10 >= middle * 8, "{id}: first segment keeps its width: {near_start} vs {middle}");
    }
}

// ---------------------------------------------------------------------------------------------
// Item 5: Hold to Shape with a slippery pen, end to end through the pointer

/// Draw `points` with the pointer, hold at the end while the pen jitters and creeps, and return
/// the shape kind that Hold to Shape made (if any).
fn draw_and_hold(points: &[(f32, f32)], seed: u64, drift: f32) -> Option<crate::hold_shape::Kind> {
    let mut h = bare(vec2(1280.0, 860.0));
    let vp = h.app.viewport;
    let at = |h: &Headless, p: (f32, f32)| h.app.view.canvas_to_screen(vp, f64::from(p.0), f64::from(p.1));
    let first = at(&h, points[0]);
    h.move_to(first);
    h.step();
    h.press(first, true);
    h.step();
    for &p in points {
        let s = at(&h, p);
        h.move_to(s);
        h.step();
    }
    // The hold: 1.2 s (72 frames) of jitter of +-1 point and a creep of `drift` points in total.
    let end = at(&h, *points.last().unwrap());
    let mut x = seed.wrapping_mul(2_654_435_761) | 1;
    let mut rnd = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        (x >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
    };
    for i in 0..72 {
        let creep = drift * i as f32 / 72.0;
        h.move_to(end + vec2(creep + rnd(), rnd()));
        h.step();
    }
    h.app.shape_edit.as_ref().map(|e| e.kind)
}

#[test]
fn hold_to_shape_survives_a_drifting_pen() {
    use crate::hold_shape::Kind;
    let mut rng = 12345u64;
    let mut noise = move |amp: f32| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        ((rng >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0) * amp
    };
    for amp in [0.5f32, 1.5, 3.0] {
        let circle: Vec<(f32, f32)> = (0..=100)
            .map(|i| {
                let t = std::f32::consts::TAU * i as f32 / 100.0;
                (400.0 + 120.0 * t.cos() + noise(amp), 300.0 + 120.0 * t.sin() + noise(amp))
            })
            .collect();
        for drift in [0.0f32, 4.0, 8.0] {
            let kind = draw_and_hold(&circle, 7, drift);
            assert_eq!(kind, Some(Kind::Circle), "circle, jitter {amp}, drift {drift}");
        }
    }
    let rect: Vec<(f32, f32)> = {
        let c = [(250.0, 200.0), (550.0, 200.0), (550.0, 400.0), (250.0, 400.0), (250.0, 200.0)];
        let mut v = Vec::new();
        for w in c.windows(2) {
            for k in 0..30 {
                let t = k as f32 / 30.0;
                v.push((w[0].0 + (w[1].0 - w[0].0) * t + noise(1.5), w[0].1 + (w[1].1 - w[0].1) * t + noise(1.5)));
            }
        }
        v.push((250.0, 200.0));
        v
    };
    assert_eq!(draw_and_hold(&rect, 3, 6.0), Some(Kind::Rectangle), "a rectangle stays a rectangle");
}
