//! The visual tour: one PNG per major screen and state, written to `target/snapshots/tour/`.
//!
//! Ignored by default because it only writes images. Run it with
//! `cargo test -p eskiz-app visual_tour -- --ignored`, then look at every PNG for layout and legibility.
//!
//! Each shot builds its own headless app, so no state leaks from one screen into the next.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use eskiz_core::filters::AdjustKind;
use eframe::egui::{vec2, Pos2, Vec2};

use crate::app::{EskizApp, Prefs};
use crate::document::Document;
use crate::panels::actions::ActionsTab;
use crate::panels::colour::ColourTab;
use crate::popover::Panel;
use crate::hold_shape::{self, Kind};
use crate::selection_tool::{EditMode, TransformMode};
use crate::snapshot::{snapshot_dir, Headless};
use crate::storage::{self, Library};
use crate::theme::UiIdentity;

/// The identity the running tour uses (the two tours take `TOUR_LOCK` so they never overlap).
static ESKIZ: AtomicBool = AtomicBool::new(false);
static TOUR_LOCK: Mutex<()> = Mutex::new(());

fn prefs(light: bool) -> Prefs {
    let identity = if ESKIZ.load(Ordering::SeqCst) { UiIdentity::Eskiz } else { UiIdentity::Classic };
    Prefs { light, identity, ..Prefs::default() }
}

/// The window every shot is rendered at, in points.
const WINDOW: Vec2 = vec2(1280.0, 800.0);
/// The size of the sample document that most shots show.
const CANVAS: (usize, usize) = (1024, 768);

/// Numbered output: `tour/NN_name.png`, in the order the shots are taken.
struct Tour {
    dir: PathBuf,
    shots: usize,
}

impl Tour {
    /// A fresh `tour` folder under the snapshot directory.
    fn new(folder: &str) -> Self {
        let dir = snapshot_dir().join(folder);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir, shots: 0 }
    }

    /// Let animations and toasts settle, then write the frame.
    fn shot(&mut self, h: &mut Headless, name: &str) {
        h.advance(0.4);
        self.shots += 1;
        let path = self.dir.join(format!("{:02}_{name}.png", self.shots));
        h.render().save_png(&path).unwrap();
    }
}

/// A window on a sample document, in the given theme.
fn canvas(light: bool) -> Headless {
    let mut h = Headless::new(WINDOW, Document::new(CANVAS.0, CANVAS.1), prefs(light));
    paint_sample(&mut h);
    h
}

/// Paint a gradient block and a warm disc, so that selections, transforms and the canvas have
/// something to show.
fn paint_sample(h: &mut Headless) {
    let (w, hgt) = CANVAS;
    let mut px = vec![0u8; w * hgt * 4];
    for y in 140..560 {
        for x in 200..700 {
            let t = (y - 140) as f32 / 420.0;
            let p = (y * w + x) * 4;
            px[p..p + 4].copy_from_slice(&[(40.0 + 60.0 * t) as u8, (110.0 - 40.0 * t) as u8, 200, 255]);
        }
    }
    for y in 80..360 {
        for x in 700..960 {
            let (dx, dy) = (x as f32 - 830.0, y as f32 - 220.0);
            if dx * dx + dy * dy < 120.0 * 120.0 {
                let p = (y * w + x) * 4;
                px[p..p + 4].copy_from_slice(&[240, 150, 60, 255]);
            }
        }
    }
    let id = h.app.session.doc.layer_id(0).unwrap();
    h.app.session.replace_layer_pixels(id, px, "Fill");
    h.app.needs_full = true;
    // Anchored areas size themselves on the frame after they first appear.
    h.render();
    h.render();
}

/// Screen point of a canvas point, for the headless app.
fn screen_of(h: &Headless, x: f64, y: f64) -> Pos2 {
    h.app.view.canvas_to_screen(h.app.viewport, x, y)
}

/// Drag the primary button along `pts`, one frame per point.
fn drag(h: &mut Headless, pts: &[Pos2]) {
    let Some(&first) = pts.first() else {
        return;
    };
    h.move_to(first);
    h.step();
    h.press(first, true);
    h.step();
    for &p in &pts[1..] {
        h.move_to(p);
        h.step();
    }
}

/// A gradient document, as a stand-in for a painted one.
fn art(w: usize, h: usize, seed: usize) -> Document {
    let mut doc = Document::new(w, h);
    let layer = doc.layer_at_mut(0).unwrap();
    for y in 0..h {
        for x in 0..w {
            let px = [
                ((x * 255) / w) as u8,
                ((y * 255) / h) as u8,
                (120 + 40 * seed) as u8,
                255,
            ];
            layer.set_pixel(x, y, px);
        }
    }
    doc
}

/// A library with `n` demo documents, in a temporary folder.
fn library_with(name: &str, n: usize) -> (Library, PathBuf) {
    let dir = std::env::temp_dir().join(format!("eskiz-tour-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = Library::at(&dir);
    let titles = ["Harbour at dusk", "Study in green", "Night sky"];
    for i in 0..n {
        lib.save(&storage::new_id(), titles[i % titles.len()], &art(192, 144, i)).unwrap();
        // Stagger modification times so the newest-first order is stable.
        std::thread::sleep(Duration::from_millis(20));
    }
    (lib, dir)
}

/// The gallery, opened on `lib` in the given theme, with its thumbnails loaded.
fn gallery_app(lib: &Library, light: bool) -> Headless {
    let lib = lib.clone();
    let mut h = Headless::from_app(WINDOW, move |ctx| {
        EskizApp::with_library(ctx, prefs(light), lib)
    });
    h.settle_storage();
    h.advance(0.3);
    h
}

/// Gallery screens: empty, three documents, the New canvas popover, and the light theme.
fn gallery_shots(t: &mut Tour) {
    let (empty, dir0) = library_with("empty", 0);
    let mut h = gallery_app(&empty, false);
    t.shot(&mut h, "gallery_empty_dark");
    std::fs::remove_dir_all(&dir0).ok();

    let (lib, dir) = library_with("three", 3);
    let mut h = gallery_app(&lib, false);
    assert_eq!(h.app.gal.entries.len(), 3, "three documents in the gallery");
    t.shot(&mut h, "gallery_three_docs_dark");

    h.click(h.app.gal.header.plus.center());
    h.advance(0.4);
    t.shot(&mut h, "gallery_new_canvas_popover_dark");

    let mut h = gallery_app(&lib, true);
    t.shot(&mut h, "gallery_three_docs_light");
    std::fs::remove_dir_all(&dir).ok();
}

/// The welcome tour cards over the empty Gallery.
fn onboarding_shots(t: &mut Tour) {
    let (empty, dir) = library_with("tour", 0);
    let lib = empty.clone();
    let eskiz = ESKIZ.load(Ordering::SeqCst);
    let identity = if eskiz { crate::theme::UiIdentity::Eskiz } else { crate::theme::UiIdentity::Classic };
    let mut h = Headless::from_app(Vec2::new(1280.0, 800.0), move |ctx| {
        let mut app = EskizApp::with_library(ctx, Prefs { identity, ..Prefs::default() }, lib);
        app.tour = crate::onboarding::Tour::at_startup(None, false, false);
        app
    });
    h.settle_storage();
    h.advance(0.2);
    t.shot(&mut h, "tour_card_welcome");
    h.app.tour.show_around();
    h.advance(0.2);
    t.shot(&mut h, "tour_card_sample");
    std::fs::remove_dir_all(&dir).ok();
}

/// The canvas in both themes.
fn canvas_shots(t: &mut Tour) {
    let mut h = canvas(false);
    t.shot(&mut h, "canvas_dark");
    let mut h = canvas(true);
    t.shot(&mut h, "canvas_light");
}

/// Actions (every tab), Adjustments list, Brush Library, Layers and Colour (every tab).
fn popover_shots(t: &mut Tour) {
    for (tab, name) in [
        (ActionsTab::Add, "actions_add_tab"),
        (ActionsTab::Canvas, "actions_canvas_tab"),
        (ActionsTab::ThreeD, "actions_3d_tab"),
        (ActionsTab::Share, "actions_share_tab"),
        (ActionsTab::Prefs, "actions_prefs_tab"),
        (ActionsTab::Help, "actions_help_tab"),
    ] {
        let mut h = canvas(false);
        h.app.actions_ui.tab = tab;
        h.open_panel(Panel::Actions);
        assert!(h.app.popovers.is_open(), "{name} is open");
        t.shot(&mut h, name);
    }
    // The Share tab again in the light theme, to check its option rows.
    let mut h = canvas(true);
    h.app.actions_ui.tab = ActionsTab::Share;
    h.open_panel(Panel::Actions);
    t.shot(&mut h, "actions_share_tab_light");

    for (panel, name) in [
        (Panel::Adjustments, "adjustments_list_dark"),
        (Panel::Brush, "brush_library_dark"),
        (Panel::Layers, "layers_dark"),
    ] {
        let mut h = canvas(false);
        h.open_panel(panel);
        assert!(h.app.popovers.is_open(), "{name} is open");
        t.shot(&mut h, name);
    }

    for (tab, name) in [
        (ColourTab::Wheel, "colour_disc_tab"),
        (ColourTab::Square, "colour_classic_tab"),
        (ColourTab::Swatches, "colour_palettes_tab"),
        (ColourTab::Schemes, "colour_harmony_tab"),
    ] {
        let mut h = canvas(false);
        h.app.colour_prev = [30, 60, 140];
        for c in [[200, 40, 40], [40, 160, 90], [250, 200, 40], [60, 90, 200], [120, 60, 160]] {
            h.app.colour_store.push_history(c);
        }
        h.app.colour_ui.tab = tab;
        h.open_panel(Panel::Colour);
        assert!(h.app.popovers.is_open(), "{name} is open");
        t.shot(&mut h, name);
    }
}

/// Each adjustment's banner, and the Perspective Blur origin handle on the canvas.
fn adjustment_shots(t: &mut Tour) {
    for (kind, name) in [
        (AdjustKind::Curves, "adjust_curves_banner"),
        (AdjustKind::GradientMap, "adjust_gradient_map_banner"),
        (AdjustKind::MotionBlur, "adjust_motion_blur_banner"),
        (AdjustKind::PerspectiveBlur, "adjust_perspective_blur_origin_handle"),
        (AdjustKind::Halftone, "adjust_halftone_banner"),
        (AdjustKind::Clone, "adjust_clone_banner"),
    ] {
        let mut h = canvas(false);
        assert!(h.app.enter_adjustment(kind), "{name} opens");
        h.step();
        t.shot(&mut h, name);
        h.app.adjust_cancel();
    }
}

/// The Liquify toolbar, with the brush cursor over the canvas.
fn liquify_shot(t: &mut Tour) {
    let mut h = canvas(false);
    assert!(h.app.enter_liquify(), "liquify opens");
    let centre = screen_of(&h, 450.0, 340.0);
    h.move_to(centre);
    h.step();
    t.shot(&mut h, "liquify_toolbar_and_cursor");
}

/// The Selection and Transform toolbars, and the warp mesh.
fn selection_and_transform_shots(t: &mut Tour) {
    let mut h = canvas(false);
    h.app.edit_mode = EditMode::Select;
    h.app.needs_full = true;
    h.render();
    t.shot(&mut h, "selection_toolbar");

    let mut h = canvas(false);
    h.app.begin_transform();
    assert!(h.app.transform.is_some(), "transform opens");
    h.render();
    t.shot(&mut h, "transform_toolbar");

    let mut h = canvas(false);
    h.app.begin_transform();
    if let Some(tt) = h.app.transform.as_mut() {
        tt.session.enter_warp().ok();
        tt.mode = TransformMode::Warp;
    }
    h.render();
    t.shot(&mut h, "transform_warp_mesh");
}

/// Crop & Resize with its handles, and the Canvas info window.
fn crop_and_info_shots(t: &mut Tour) {
    let mut h = canvas(false);
    h.app.open_crop();
    assert!(h.app.crop.is_some(), "crop opens");
    h.render();
    t.shot(&mut h, "crop_resize_handles");

    let mut h = canvas(false);
    h.app.canvas_info_open = true;
    h.render();
    t.shot(&mut h, "canvas_info_window");
}

/// Brush Bench, opened on the tool's brush.
fn studio_shot(t: &mut Tour) {
    let mut h = canvas(false);
    assert!(h.app.open_studio_for_library_tool(), "the Studio opens");
    assert!(h.app.studio.is_some());
    t.shot(&mut h, "brush_bench");
}

/// Text editing on the canvas, and the Edit Style window over the same text layer.
fn text_shots(t: &mut Tour) {
    let mut h = canvas(false);
    h.app.add_text_action();
    h.app.text_replace(0..0, "Eskiz");
    h.step();
    t.shot(&mut h, "text_editing");
    assert!(h.app.text_target().is_some(), "the Edit Style window has a target");
    h.advance(0.6);
    h.step();
    t.shot(&mut h, "text_edit_style_panel");
}

/// The Animation timeline with onion skins on, over three frames.
fn animation_shot(t: &mut Tour) {
    let mut doc = eskiz_core::Document::empty(160, 120);
    for (i, rgb) in [[230u8, 60, 60], [60, 200, 90], [60, 110, 230]].iter().enumerate() {
        let mut px = vec![0u8; 160 * 120 * 4];
        for y in 20..90 {
            for x in (20 + 30 * i)..(80 + 30 * i) {
                let p = (y * 160 + x) * 4;
                px[p..p + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        doc.push_layer(None, eskiz_core::Props::new(format!("Frame {}", i + 1)), None, px);
    }
    doc.animation.frames_enabled = true;
    let mut h = Headless::new(WINDOW, doc, prefs(false));
    h.advance(0.1);
    t.shot(&mut h, "animation_timeline_onion_skins");
}

/// The 3D view with its banner.
fn three_d_shot(t: &mut Tour) {
    let mut h = canvas(false);
    let mut doc = crate::view3d_tests::cube_doc();
    doc.camera = doc.home;
    h.app.three_d = Some(Box::new(crate::view3d::ThreeD::new(doc, Some("Drag to orbit. Paint on the model.".into()))));
    h.advance(0.1);
    t.shot(&mut h, "three_d_view");
}

/// The drawing-guide editor, and the reference window beside the canvas.
fn guide_and_reference_shots(t: &mut Tour) {
    let mut h = canvas(false);
    h.app.guides.editor_open = true;
    h.render();
    t.shot(&mut h, "guide_editor");

    let mut h = canvas(false);
    h.app.toggle_reference();
    assert!(h.app.reference.open, "the reference window opens");
    h.advance(0.1);
    t.shot(&mut h, "reference_window");
}

/// Full-screen mode, with the canvas alone and the "show UI" button.
fn fullscreen_shot(t: &mut Tour) {
    let mut h = canvas(false);
    h.app.toggle_fullscreen();
    assert!(h.app.fullscreen);
    h.advance(0.5);
    t.shot(&mut h, "fullscreen");
}

/// The eyedropper loupe, held over a painted patch.
fn eyedropper_shot(t: &mut Tour) {
    let mut h = canvas(false);
    let over = screen_of(&h, 450.0, 340.0);
    // A long press holds the pointer down; the loupe stays up until it is released.
    h.move_to(over);
    h.press(over, true);
    h.app.begin_eyedrop(over);
    h.step();
    t.shot(&mut h, "eyedropper_loupe");
    h.press(over, false);
    h.step();
}

/// Hold to Shape Edit Shape: a zigzag held still snaps to a polyline, and lifting the pen opens the
/// handles.
fn hold_shape_shot(t: &mut Tour) {
    let mut h = canvas(false);
    h.advance(0.1);
    let corners = [(200.0, 150.0), (330.0, 420.0), (470.0, 170.0), (600.0, 430.0), (700.0, 190.0)];
    let mut stroke: Vec<Pos2> = Vec::new();
    for w in corners.windows(2) {
        let (a, b) = (screen_of(&h, w[0].0, w[0].1), screen_of(&h, w[1].0, w[1].1));
        let n = ((b - a).length() / 4.0).ceil().max(1.0) as usize;
        for k in 0..n {
            stroke.push(a + (b - a) * (k as f32 / n as f32));
        }
    }
    let end = screen_of(&h, corners[4].0, corners[4].1);
    stroke.push(end);
    drag(&mut h, &stroke);
    // Hold still at the end: Hold to Shape snaps the stroke while the pen is still down.
    h.advance(0.8);
    let edit = h.app.shape_edit.clone().expect("holding still snaps a shape");
    assert_eq!(edit.kind, Kind::Polyline);
    h.press(end, false);
    h.step();
    assert!(h.app.editing_shape(), "lifting the pen opens Edit Shape");
    assert!(!hold_shape::handles(&edit.current).is_empty());
    t.shot(&mut h, "hold_shape_edit_shape_handles");
}

/// Toasts: the status overlay at the top centre, for an undo and a warning.
fn toast_shot(t: &mut Tour) {
    let mut h = canvas(false);
    h.app.show_toast("Undo Stroke", 3.0);
    h.step();
    t.shot(&mut h, "toast_undo");

    let mut h = canvas(true);
    h.app.show_toast("Select an editable layer to transform.", 3.0);
    h.step();
    t.shot(&mut h, "toast_warning_light");
}

/// Every major screen and state, written to `target/snapshots/<folder>/NN_name.png`.
fn run_tour(folder: &str, eskiz: bool) {
    let _guard = TOUR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    ESKIZ.store(eskiz, Ordering::SeqCst);
    let mut t = Tour::new(folder);
    gallery_shots(&mut t);
    onboarding_shots(&mut t);
    canvas_shots(&mut t);
    popover_shots(&mut t);
    adjustment_shots(&mut t);
    liquify_shot(&mut t);
    selection_and_transform_shots(&mut t);
    crop_and_info_shots(&mut t);
    studio_shot(&mut t);
    text_shots(&mut t);
    animation_shot(&mut t);
    three_d_shot(&mut t);
    guide_and_reference_shots(&mut t);
    fullscreen_shot(&mut t);
    eyedropper_shot(&mut t);
    hold_shape_shot(&mut t);
    toast_shot(&mut t);
    assert!(t.shots >= 40, "the tour wrote {} shots", t.shots);
    eprintln!("visual tour: {} shots in {}", t.shots, t.dir.display());
}

/// The Classic look: `target/snapshots/tour/`.
#[test]
#[ignore = "writes the visual tour PNGs; run with --ignored"]
fn visual_tour() {
    run_tour("tour", false);
}

/// The Eskiz look: `target/snapshots/tour_eskiz/`.
#[test]
#[ignore = "writes the visual tour PNGs; run with --ignored"]
fn visual_tour_eskiz() {
    run_tour("tour_eskiz", true);
}
