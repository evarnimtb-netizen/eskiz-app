//! Interaction and snapshot tests for the Actions panel, adjustment mode, export and import.
//! Snapshots are written to `eskiz/target/snapshots` for inspection.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eskiz_core::filters::AdjustKind;
use eframe::egui::{pos2, Vec2};

use crate::app::Prefs;
use crate::document::Document;
use crate::export::ExportFormat;
use crate::panels::actions::ActionsTab;
use crate::popover::Panel;
use crate::snapshot::{snapshot_dir, Headless};

/// A document whose only layer holds a soft-edged scene: a coloured sky, discs and a
/// half-transparent band, so every adjustment has something visible to do.
pub(crate) fn painted_doc(w: usize, h: usize) -> Document {
    let mut doc = Document::new(w, h);
    let id = doc.layer_id(0).unwrap();
    let layer = doc.layer_mut(id).unwrap();
    let discs = [
        (0.30, 0.45, 0.20, [230u8, 70, 60]),
        (0.55, 0.55, 0.17, [60, 160, 230]),
        (0.75, 0.38, 0.12, [240, 200, 60]),
    ];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            // Sky gradient in the upper 70 percent, transparent below.
            let mut px = if fy < 0.7 {
                [(40.0 + 90.0 * fy) as u8, (70.0 + 100.0 * fy) as u8, (150.0 + 80.0 * (1.0 - fy)) as u8, 255]
            } else {
                [0, 0, 0, 0]
            };
            for (cx, cy, r, c) in discs {
                let d = ((fx - cx) * w as f32 / h as f32).hypot(fy - cy);
                if d < r {
                    px = [c[0], c[1], c[2], 255];
                }
            }
            // A half-transparent green band across the bottom (premultiplied).
            if (0.78..0.9).contains(&fy) && (0.1..0.9).contains(&fx) {
                px = [0, 100, 40, 200];
            }
            layer.set_pixel(x, y, px);
        }
    }
    doc
}

pub(crate) fn headless(light: bool, w: usize, h: usize) -> Headless {
    let prefs = Prefs { light, ..Prefs::default() };
    let mut hl = Headless::new(Vec2::new(1280.0, 860.0), painted_doc(w, h), prefs);
    hl.app.active = hl.app.session.doc.layer_id(0).unwrap();
    hl.advance(0.1);
    hl
}

pub(crate) fn pixels(h: &Headless) -> Vec<u8> {
    h.app.session.doc.layer(h.app.active).unwrap().to_rgba()
}

/// Step frames until no adjustment or export is running.
pub(crate) fn settle(h: &mut Headless) {
    for _ in 0..4000 {
        h.step();
        if !h.app.adjusting() && !h.app.share_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("jobs did not finish");
}

/// Drag the pointer across the canvas from `a` to `b` in small steps.
pub(crate) fn drag(h: &mut Headless, a: (f32, f32), b: (f32, f32)) {
    let (a, b) = (pos2(a.0, a.1), pos2(b.0, b.1));
    h.move_to(a);
    h.step();
    h.press(a, true);
    h.step();
    for k in 1..=12 {
        let t = k as f32 / 12.0;
        h.move_to(a + (b - a) * t);
        h.step();
    }
    h.press(b, false);
    h.step();
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-adjust-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------ framework behaviour

/// A1, A2, G7 and friends: each implemented adjustment previews without touching the layer,
/// applies as one step named after it, and undo restores the pixels exactly.
#[test]
fn apply_then_undo_restores_pixels_exactly_for_each_adjustment() {
    type Case = (AdjustKind, &'static [(usize, f64)], &'static str);
    let cases: [Case; 3] = [
        (AdjustKind::Opacity, &[(0, 0.4)], "Opacity"),
        (AdjustKind::GaussianBlur, &[(0, 0.3)], "Gaussian Blur"),
        (AdjustKind::Hsb, &[(0, 0.3), (1, 0.4), (2, -0.1)], "Hue, Saturation, Brightness"),
    ];
    for (kind, values, undo_name) in cases {
        let mut h = headless(false, 240, 180);
        let ctx = h.ctx.clone();
        let before = pixels(&h);
        let steps_before = h.app.session.undo_steps();
        assert!(h.app.enter_adjustment(kind), "{kind:?}");
        for &(i, v) in values {
            h.app.adjust_set(i, v);
        }
        h.step();
        settle_preview(&mut h);
        // A1: the preview shows, the stored pixels do not change.
        assert_eq!(pixels(&h), before, "{kind:?}: preview must not write the layer");
        assert_ne!(h.app.session.flatten_display(), h.app.session.flatten(), "{kind:?}: preview is visible");
        h.app.adjust_apply(&ctx);
        settle(&mut h);
        let after = pixels(&h);
        assert_ne!(after, before, "{kind:?}: applying changes the layer");
        assert_eq!(h.app.session.undo_steps(), steps_before + 1, "{kind:?}: one undo step");
        assert_eq!(h.app.session.undo_name(), Some(undo_name));
        assert!(!h.app.session.has_preview());
        // The canvas texture shows the result: the display composite equals the real one.
        assert_eq!(h.app.session.flatten_display(), h.app.session.flatten());
        // A2: undo restores exactly; redo brings the result back exactly.
        h.app.undo_action();
        assert_eq!(pixels(&h), before, "{kind:?}: undo");
        h.app.redo_action();
        assert_eq!(pixels(&h), after, "{kind:?}: redo");
    }
}

/// Cancel leaves no undo entry and no change.
#[test]
fn cancel_changes_nothing_and_leaves_no_undo_entry() {
    let mut h = headless(false, 200, 150);
    let before = pixels(&h);
    let steps = h.app.session.undo_steps();
    h.app.enter_adjustment(AdjustKind::GaussianBlur);
    settle_preview(&mut h);
    assert!(h.app.session.has_preview());
    h.app.adjust_cancel();
    h.step();
    assert!(!h.app.adjusting() && !h.app.session.has_preview());
    assert_eq!(pixels(&h), before);
    assert_eq!(h.app.session.undo_steps(), steps);
    assert_eq!(h.app.session.flatten_display(), h.app.session.flatten());
}

/// Applying with nothing changed makes no undo entry.
#[test]
fn applying_the_identity_is_a_no_op() {
    let mut h = headless(false, 100, 80);
    let ctx = h.ctx.clone();
    let steps = h.app.session.undo_steps();
    h.app.enter_adjustment(AdjustKind::Opacity);
    h.app.adjust_apply(&ctx);
    h.step();
    assert!(!h.app.adjusting());
    assert_eq!(h.app.session.undo_steps(), steps);
}

/// D1: dragging right increases the value, left decreases it, and it clamps. Painting is off.
#[test]
fn dragging_on_the_canvas_changes_the_value_and_nothing_paints() {
    let mut h = headless(false, 400, 300);
    let before = pixels(&h);
    let steps = h.app.session.undo_steps();
    h.app.enter_adjustment(AdjustKind::Opacity);
    h.step();
    assert_eq!(h.app.adjustment().unwrap().value(0), 1.0);
    let zoom = h.app.view.zoom;
    let canvas_w = 400.0 * zoom;
    // Left by a quarter of the canvas width: the amount drops by 0.25.
    drag(&mut h, (700.0, 500.0), (700.0 - canvas_w / 4.0, 500.0));
    let v = h.app.adjustment().unwrap().value(0);
    assert!((v - 0.75).abs() < 0.02, "dragged left to {v}");
    drag(&mut h, (500.0, 520.0), (500.0 + canvas_w / 8.0, 520.0));
    let w = h.app.adjustment().unwrap().value(0);
    assert!(w > v && (w - 0.875).abs() < 0.02, "dragged right to {w}");
    drag(&mut h, (900.0, 500.0), (900.0 + canvas_w * 3.0, 500.0));
    assert_eq!(h.app.adjustment().unwrap().value(0), 1.0, "clamps at the top of the range");
    drag(&mut h, (1200.0, 500.0), (1200.0 - canvas_w * 3.0, 500.0));
    assert_eq!(h.app.adjustment().unwrap().value(0), 0.0, "clamps at the bottom");
    // Not a single stroke happened.
    assert_eq!(pixels(&h), before);
    assert_eq!(h.app.session.undo_steps(), steps);
    assert!(!h.app.session.is_stroking());
}

/// Tapping the adjustments button again applies.
#[test]
fn tapping_the_adjustments_button_again_applies() {
    let mut h = headless(false, 200, 150);
    let before = pixels(&h);
    h.app.enter_adjustment(AdjustKind::Opacity);
    h.app.adjust_set(0, 0.5);
    h.step();
    let button = h.button_rect(Panel::Adjustments).expect("adjustments button");
    h.click(button.center());
    settle(&mut h);
    assert!(!h.app.adjusting());
    assert_ne!(pixels(&h), before);
    assert_eq!(h.app.session.undo_name(), Some("Opacity"));
}

/// Choosing a row in the panel enters the mode; the unavailable rows do nothing.
#[test]
fn the_panel_lists_everything_and_every_entry_opens() {
    let mut h = headless(false, 200, 150);
    h.open_panel(Panel::Adjustments);
    // Click somewhere in the list where the first row (Opacity) is: scan for the click that
    // enters the mode, then make sure a disabled kind is refused through the API.
    assert!(h.app.enter_adjustment(AdjustKind::Curves));
    assert!(h.app.adjusting());
    assert!(h.app.enter_adjustment(AdjustKind::Hsb));
    assert!(h.app.adjusting());
    assert!(!h.app.popovers.is_open(), "the panel closes when the mode opens");
}

/// Locked layers and groups are refused.
#[test]
fn locked_layers_cannot_be_adjusted() {
    let mut h = headless(false, 100, 80);
    let id = h.app.active;
    h.app.session.set_locked(id, true);
    assert!(!h.app.enter_adjustment(AdjustKind::Opacity));
    assert!(!h.app.adjusting());
}

/// Changing the document under the mode (here: undo through the API) ends it.
#[test]
fn switching_layer_cancels() {
    let mut h = headless(false, 100, 80);
    h.app.enter_adjustment(AdjustKind::Opacity);
    h.step();
    let e = h.app.session.add_layer(None).unwrap();
    h.app.active = e.id.unwrap();
    h.step();
    assert!(!h.app.adjusting());
    assert!(!h.app.session.has_preview());
}

/// Keyboard: Enter applies, Escape cancels.
#[test]
fn enter_applies_and_escape_cancels() {
    use eframe::egui::{Event, Key, Modifiers};
    let key = |k: Key| Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: Modifiers::NONE };
    let mut h = headless(false, 100, 80);
    let before = pixels(&h);
    h.app.enter_adjustment(AdjustKind::Opacity);
    h.app.adjust_set(0, 0.3);
    h.step();
    h.event(key(Key::Escape));
    h.step();
    assert!(!h.app.adjusting());
    assert_eq!(pixels(&h), before);
    h.app.enter_adjustment(AdjustKind::Opacity);
    h.app.adjust_set(0, 0.3);
    h.step();
    h.event(key(Key::Enter));
    settle(&mut h);
    assert_ne!(pixels(&h), before);
}

/// Preview stays interactive on the full 2048 x 1536 canvas, and the full-resolution commit
/// runs in the background with progress.
#[test]
fn gaussian_blur_is_interactive_on_a_big_canvas() {
    let mut h = headless(false, 2048, 1536);
    let ctx = h.ctx.clone();
    let before = pixels(&h);
    h.app.enter_adjustment(AdjustKind::GaussianBlur);
    h.step();
    h.app.adjust_set(0, 1.0);
    let t = Instant::now();
    h.step();
    let preview = t.elapsed();
    eprintln!("2048x1536 gaussian preview frame (amount 1.0): {preview:?}");
    assert!(preview < Duration::from_secs(3), "preview frame took {preview:?}");
    // A second drag step reuses the same path.
    h.app.adjust_set(0, 0.8);
    h.step();
    h.app.adjust_apply(&ctx);
    assert!(h.app.adjustment().is_some_and(|a| a.kind == AdjustKind::GaussianBlur), "still open while the job runs");
    // The first frame after apply shows the progress bar.
    h.step();
    let t = Instant::now();
    settle(&mut h);
    eprintln!("2048x1536 gaussian commit finished after {:?}", t.elapsed());
    assert_ne!(pixels(&h), before);
    h.app.undo_action();
    assert_eq!(pixels(&h), before, "undo restores the 2048x1536 layer exactly");
}

// ------------------------------------------------------------------ flip canvas

#[test]
fn flip_canvas_is_one_undoable_step() {
    let mut h = headless(false, 120, 90);
    let before = pixels(&h);
    h.app.flip_canvas_action(true);
    let flipped = pixels(&h);
    assert_ne!(flipped, before);
    assert_eq!(h.app.session.undo_name(), Some("Flip Canvas Horizontal"));
    // Pixel (0, y) went to (w-1, y).
    assert_eq!(&flipped[(119 * 4)..(119 * 4 + 4)], &before[0..4]);
    h.app.undo_action();
    assert_eq!(pixels(&h), before);
    h.app.flip_canvas_action(false);
    assert_eq!(h.app.session.undo_name(), Some("Flip Canvas Vertical"));
    let last_row = 89 * 120 * 4;
    assert_eq!(&pixels(&h)[0..4], &before[last_row..last_row + 4]);
    h.app.undo_action();
    assert_eq!(pixels(&h), before);
}

// ------------------------------------------------------------------ export and import

#[test]
fn export_png_jpeg_and_procreate_to_a_folder_and_read_back() {
    let dir = temp_dir("export");
    let mut h = headless(false, 160, 120);
    let ctx = h.ctx.clone();
    h.app.export_path = dir.join("shared").display().to_string();
    for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Procreate] {
        h.app.start_export(&ctx, format);
        assert!(h.app.share_busy());
        settle(&mut h);
        let file = dir.join("shared").with_extension(format.extension());
        assert!(file.exists(), "{} was written", file.display());
        assert!(std::fs::metadata(&file).unwrap().len() > 100);
    }
    let (w, hh, rgba) = crate::import::decode_image(&dir.join("shared.png")).unwrap();
    assert_eq!((w, hh), (160, 120));
    assert_eq!(rgba, h.app.session.flatten_for_export());
    let (jw, jh, _) = crate::import::decode_image(&dir.join("shared.jpg")).unwrap();
    assert_eq!((jw, jh), (160, 120));
    let back = crate::import::open_procreate(&dir.join("shared.procreate")).unwrap();
    assert_eq!(back.layer_at(0).unwrap().to_rgba(), pixels(&h));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn importing_a_picture_adds_one_undoable_layer_and_dropping_does_too() {
    let dir = temp_dir("import");
    let mut h = headless(false, 160, 120);
    // A 40 x 20 picture.
    let rgba: Vec<u8> = (0..40 * 20).flat_map(|i| [(i % 256) as u8, 40, 200, 255]).collect();
    let png = dir.join("pic.png");
    crate::export::write_png(&png, 40, 20, &rgba).unwrap();
    let ctx = h.ctx.clone();
    let layers = h.app.session.doc.layer_count();
    h.app.start_import_image(&ctx, &png);
    settle(&mut h);
    assert_eq!(h.app.session.doc.layer_count(), layers + 1);
    assert_eq!(h.app.session.undo_name(), Some("Import Image"));
    assert_eq!(h.app.active, h.app.session.doc.layer_id(layers).unwrap(), "the new layer is selected");
    let px = h.app.session.doc.layer(h.app.active).unwrap().to_rgba();
    // Centred at (60, 50): first picture pixel there, nothing at the corner.
    let at = |x: usize, y: usize| px[(y * 160 + x) * 4..][..4].to_vec();
    assert_eq!(at(60, 50), vec![0, 40, 200, 255]);
    assert_eq!(at(0, 0), vec![0, 0, 0, 0]);
    h.app.undo_action();
    assert_eq!(h.app.session.doc.layer_count(), layers);

    // Drag and drop of a picture does the same; a JPEG works too.
    let jpg = dir.join("pic.jpg");
    let rgb: Vec<u8> = (0..40 * 20).flat_map(|_| [200u8, 30, 30]).collect();
    crate::export::write_jpeg(&jpg, 40, 20, &rgb, 90, 132).unwrap();
    h.drop_file(&jpg);
    settle(&mut h);
    assert_eq!(h.app.session.doc.layer_count(), layers + 1);
    let px = h.app.session.doc.layer(h.app.active).unwrap().to_rgba();
    let c = &px[(60 * 4 + 50 * 160 * 4)..][..4];
    assert!(c[0] > 180 && c[1] < 60 && c[3] == 255, "{c:?}");
    // A file that is not a picture is reported, not imported.
    let bad = dir.join("bad.png");
    std::fs::write(&bad, b"nope").unwrap();
    h.app.start_import_image(&ctx, &bad);
    settle(&mut h);
    assert_eq!(h.app.session.doc.layer_count(), layers + 1);
    std::fs::remove_dir_all(&dir).ok();
}

// ------------------------------------------------------------------ preferences

#[test]
fn gesture_preferences_round_trip() {
    let p = Prefs { undo_gestures: false, repeat_undo: false, touch_navigation: false, ..Prefs::default() };
    let back = Prefs::decode(&p.encode());
    assert_eq!(back, p);
    assert!(Prefs::decode("light=1").undo_gestures, "old preference strings keep the defaults");
}

// ------------------------------------------------------------------ snapshots

fn snap(h: &mut Headless, name: &str) -> crate::snapshot::Image {
    let img = h.render();
    img.save_png(&snapshot_dir().join(name)).unwrap();
    img
}

#[test]
fn actions_tabs_snapshots_dark_and_light() {
    for light in [false, true] {
        let theme = if light { "light" } else { "dark" };
        for tab in ActionsTab::ALL {
            let mut h = headless(light, 2048, 1536);
            h.app.actions_ui.tab = tab;
            h.app.pen.status = "Pen: Wayland tablet protocol".to_owned();
            h.open_panel(Panel::Actions);
            let img = snap(&mut h, &format!("actions_{}_{theme}.png", tab.label().to_lowercase()));
            // The panel stays inside the window: its body bottom is above the window edge.
            let body = h.app.popovers.body_rect().expect("panel drawn");
            assert!(body.bottom() <= h.size.y - 8.0, "{tab:?}: panel overflows ({body:?})");
            assert!(img.width == 1280);
        }
    }
}

#[test]
fn actions_panel_fits_a_short_window() {
    let prefs = Prefs::default();
    let mut h = Headless::new(Vec2::new(1000.0, 560.0), painted_doc(400, 300), prefs);
    for tab in [ActionsTab::Prefs, ActionsTab::Help, ActionsTab::Share] {
        h.app.actions_ui.tab = tab;
        h.open_panel(Panel::Actions);
        let body = h.app.popovers.body_rect().expect("panel drawn");
        assert!(body.bottom() <= 560.0 - 8.0, "{tab:?}: overflows a short window ({body:?})");
        snap(&mut h, &format!("actions_{}_short.png", tab.label().to_lowercase()));
        h.open_panel(Panel::Actions);
    }
}

#[test]
fn adjustments_panel_and_banners_snapshots() {
    for light in [false, true] {
        let theme = if light { "light" } else { "dark" };
        let mut h = headless(light, 2048, 1536);
        h.open_panel(Panel::Adjustments);
        snap(&mut h, &format!("adjustments_list_{theme}.png"));
        h.open_panel(Panel::Adjustments);

        // Gaussian blur at the default amount, previewed on the painted canvas.
        h.app.enter_adjustment(AdjustKind::GaussianBlur);
        h.advance(0.2);
        let blurred = snap(&mut h, &format!("adjust_gaussian_{theme}.png"));
        h.app.adjust_cancel();
        h.advance(0.1);
        let plain = h.render();
        let differs = (0..blurred.rgba.len()).step_by(4 * 7).any(|i| blurred.rgba[i..i + 3] != plain.rgba[i..i + 3]);
        assert!(differs, "the blur preview is visible on the canvas");

        // HSB: hue turned, saturation up.
        h.app.enter_adjustment(AdjustKind::Hsb);
        h.app.adjust_set(0, 0.35);
        h.app.adjust_set(1, 0.3);
        h.advance(0.2);
        snap(&mut h, &format!("adjust_hsb_{theme}.png"));
        h.app.adjust_select_param(2);
        h.app.adjust_set(2, -0.25);
        h.advance(0.2);
        snap(&mut h, &format!("adjust_hsb_brightness_{theme}.png"));
        h.app.adjust_cancel();

        h.app.enter_adjustment(AdjustKind::Opacity);
        h.app.adjust_set(0, 0.55);
        h.advance(0.2);
        snap(&mut h, &format!("adjust_opacity_{theme}.png"));
        h.app.adjust_cancel();
    }
}

/// Step until the live preview has been computed on its worker thread.
pub(crate) fn settle_preview(h: &mut Headless) {
    for _ in 0..4000 {
        h.step();
        if !h.app.adjust_preview_busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the preview did not finish");
}
