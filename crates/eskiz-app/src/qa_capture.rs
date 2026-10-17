//! QA capture harness: drives the real app UI headlessly and writes screenshots, frame
//! sequences, contact sheets, timings and notes for reviewers who only see the files.
//!
//! Ignored by default. Run with `tools/qa_capture.sh`, or
//! `ESKIZ_QA_OUT=target/qa cargo test -p eskiz-app --release qa_capture -- --ignored --test-threads=1`.
//!
//! Output: `<out>/<area>/` with `*.png` stills (2x pixels per point), `*_fNN.png` frame
//! sequences (1x, 60 Hz simulated steps, at most 40 frames), `*_sheet.png` contact sheets,
//! `timings.csv` and `notes.md`. Everything is deterministic: fixed simulated time, a sample
//! document painted in code, a temporary library with generated documents.
//!
//! Limits: the rasteriser is the CPU one in `snapshot.rs`, so there is no real GPU timing; the
//! timings are CPU time of the UI root (`App::update`) only.

use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{vec2, Pos2, Vec2};
use eskiz_core::filters::AdjustKind;
use eskiz_core::BlendMode;

use crate::app::{EskizApp, Prefs, Tool};
use crate::document::Document;
use crate::panels::actions::ActionsTab;
use crate::panels::colour::ColourTab;
use crate::popover::Panel;
use crate::selection_tool::{EditMode, Shape};
use crate::sheets::Sheet;
use crate::snapshot::Headless;
use crate::storage::{self, Library};
use crate::theme::UiIdentity;

const CANVAS: (usize, usize) = (1024, 768);
const BIG: Vec2 = vec2(1280.0, 860.0);
const MIN: Vec2 = vec2(900.0, 600.0);
const MAX_FRAMES: usize = 40;

/// One combination of look, theme and window size.
#[derive(Clone, Copy)]
struct Variant {
    identity: UiIdentity,
    light: bool,
    size: Vec2,
    right_hand: bool,
}

impl Variant {
    fn tag(&self) -> String {
        format!(
            "{}_{}_{}x{}{}",
            self.identity.key(),
            if self.light { "light" } else { "dark" },
            self.size.x as u32,
            self.size.y as u32,
            if self.right_hand { "_righthand" } else { "" }
        )
    }
    fn prefs(&self) -> Prefs {
        Prefs { identity: self.identity, light: self.light, sidebar_right: self.right_hand, ..Prefs::default() }
    }
}

fn all_variants() -> Vec<Variant> {
    let mut v = Vec::new();
    for size in [BIG, MIN] {
        for identity in UiIdentity::ALL {
            for light in [false, true] {
                v.push(Variant { identity, light, size, right_hand: false });
            }
        }
    }
    v
}

/// The starting state of a shot.
#[derive(Clone, Copy, PartialEq)]
enum Base {
    Canvas,
    Gallery(usize),
    Frames,
    Tour,
}

type Setup = fn(&mut Headless);

/// One screen to capture in every variant.
struct Shot {
    area: &'static str,
    name: &'static str,
    base: Base,
    /// What the file shows.
    shows: &'static str,
    /// The exact steps taken after the base state.
    steps: &'static str,
    setup: Setup,
}

/// An animated transition recorded as a frame sequence.
struct Seq {
    area: &'static str,
    name: &'static str,
    base: Base,
    shows: &'static str,
    steps: &'static str,
    /// Called before frame `i`, to queue input or change state.
    hook: fn(&mut Headless, usize),
}

// ---------------------------------------------------------------- helpers

fn out_root() -> PathBuf {
    let p = std::env::var("ESKIZ_QA_OUT").unwrap_or_else(|_| "target/qa".into());
    let p = PathBuf::from(p);
    if p.is_absolute() {
        p
    } else {
        // Relative to the workspace root (`eskiz/`), whatever cargo's working directory is.
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(p)
    }
}

fn sample_pixels() -> Vec<u8> {
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
    px
}

fn new_headless(v: &Variant, ppp: f32, make: impl FnOnce(&eframe::egui::Context) -> EskizApp) -> Headless {
    let mut h = Headless::from_app(v.size, make);
    h.ppp = ppp;
    h.advance(0.3);
    h
}

fn canvas_app(v: &Variant, ppp: f32) -> Headless {
    let mut h = new_headless(v, ppp, |ctx| EskizApp::new(ctx, Document::new(CANVAS.0, CANVAS.1), String::new(), v.prefs()));
    let id = h.app.session.doc.layer_id(0).unwrap();
    h.app.session.replace_layer_pixels(id, sample_pixels(), "Fill");
    h.app.needs_full = true;
    h.advance(0.2);
    h
}

fn art(w: usize, h: usize, seed: usize) -> Document {
    let mut doc = Document::new(w, h);
    let layer = doc.layer_at_mut(0).unwrap();
    for y in 0..h {
        for x in 0..w {
            layer.set_pixel(x, y, [((x * 255) / w) as u8, ((y * 255) / h) as u8, (120 + 40 * seed) as u8, 255]);
        }
    }
    doc
}

fn library_with(n: usize) -> (Library, PathBuf) {
    let dir = std::env::temp_dir().join(format!("eskiz-qa-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = Library::at(&dir);
    let titles = ["Harbour at dusk", "Study in green", "Night sky"];
    for i in 0..n {
        lib.save(&storage::new_id(), titles[i % 3], &art(192, 144, i)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    (lib, dir)
}

fn frames_doc() -> eskiz_core::Document {
    let mut doc = eskiz_core::Document::empty(160, 120);
    for (i, rgb) in [[230u8, 60, 60], [60, 200, 90], [60, 110, 230], [240, 190, 40]].iter().enumerate() {
        let mut px = vec![0u8; 160 * 120 * 4];
        for y in 20..90 {
            for x in (20 + 25 * i)..(80 + 25 * i) {
                let p = (y * 160 + x) * 4;
                px[p..p + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        doc.push_layer(None, eskiz_core::Props::new(format!("Frame {}", i + 1)), None, px);
    }
    doc.animation.frames_enabled = true;
    doc
}

/// Build the base state. Returns the temp library folder to remove afterwards, if any.
fn make_base(base: Base, v: &Variant, ppp: f32) -> (Headless, Option<PathBuf>) {
    match base {
        Base::Canvas => (canvas_app(v, ppp), None),
        Base::Gallery(n) => {
            let (lib, dir) = library_with(n);
            let mut h = new_headless(v, ppp, |ctx| EskizApp::with_library(ctx, v.prefs(), lib));
            h.settle_storage();
            h.advance(0.3);
            (h, Some(dir))
        }
        Base::Frames => {
            let mut h = new_headless(v, ppp, |ctx| EskizApp::new(ctx, frames_doc(), String::new(), v.prefs()));
            h.advance(0.2);
            (h, None)
        }
        Base::Tour => {
            let (lib, dir) = library_with(0);
            let mut h = new_headless(v, ppp, |ctx| {
                let mut app = EskizApp::with_library(ctx, v.prefs(), lib);
                app.tour = crate::onboarding::Tour::at_startup(None, false, false);
                app
            });
            h.settle_storage();
            h.advance(0.2);
            (h, Some(dir))
        }
    }
}

fn screen_of(h: &Headless, x: f64, y: f64) -> Pos2 {
    h.app.view.canvas_to_screen(h.app.viewport, x, y)
}

fn drag(h: &mut Headless, pts: &[Pos2]) {
    let Some(&first) = pts.first() else { return };
    h.move_to(first);
    h.step();
    h.press(first, true);
    h.step();
    for &p in &pts[1..] {
        h.move_to(p);
        h.step();
    }
    h.press(*pts.last().unwrap(), false);
    h.advance(0.15);
}

/// A smooth curve in canvas space, resampled so the stroke is dense.
fn curve(h: &Headless, x0: f64, y0: f64, amp: f64, len: f64) -> Vec<Pos2> {
    (0..=48)
        .map(|i| {
            let t = f64::from(i) / 48.0;
            screen_of(h, x0 + t * len, y0 + (t * 6.0).sin() * amp)
        })
        .collect()
}

fn brush_by_set(h: &mut Headless, set: usize, idx: usize) {
    let sets = h.app.bank.sets.clone();
    let s = &sets[set.min(sets.len() - 1)];
    let b = &s.brushes[idx.min(s.brushes.len() - 1)];
    let t = h.app.tool.index();
    h.app.tool_brushes[t] = b.id.clone();
}

fn open(h: &mut Headless, panel: Panel) {
    h.open_panel(panel);
}

fn layer_ids(h: &Headless) -> Vec<u64> {
    h.app.session.doc.layer_ids()
}

// ---------------------------------------------------------------- recorder

struct Rec {
    root: PathBuf,
    notes: String,
    timings: String,
    files: usize,
}

impl Rec {
    fn new(area: &'static str) -> Self {
        let root = out_root().join(area);
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut notes = String::new();
        let _ = writeln!(notes, "# {area}\n");
        let _ = writeln!(
            notes,
            "Stills are 2x pixels per point (1280x860 pt = 2560x1720 px; 900x600 pt = 1800x1200 px). \
             File names end in `<look>_<dark|light>_<W>x<H>` (points). Frame sequences are 1x, one frame per 1/60 s of \
             simulated time, until the UI stops asking for repaints (cap {MAX_FRAMES}).\n"
        );
        let timings = "interaction,look,theme,window,frames_to_settle,max_update_ms,mean_update_ms,idle_repaints_per_30_frames\n".to_owned();
        Self { root, notes, timings, files: 0 }
    }

    fn save(&mut self, img: &crate::snapshot::Image, file: &str) {
        img.save_png(&self.root.join(file)).unwrap();
        self.files += 1;
    }

    fn finish(self) -> usize {
        std::fs::write(self.root.join("notes.md"), &self.notes).unwrap();
        std::fs::write(self.root.join("timings.csv"), &self.timings).unwrap();
        self.files + 2
    }
}

fn downscale(img: &crate::snapshot::Image, f: usize) -> crate::snapshot::Image {
    let (w, h) = (img.width / f, img.height / f);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let mut s = 0u32;
                for dy in 0..f {
                    for dx in 0..f {
                        s += u32::from(img.rgba[((y * f + dy) * img.width + x * f + dx) * 4 + c]);
                    }
                }
                rgba[(y * w + x) * 4 + c] = (s / (f * f) as u32) as u8;
            }
            rgba[(y * w + x) * 4 + 3] = 255;
        }
    }
    crate::snapshot::Image { width: w, height: h, rgba }
}

/// Tile `frames` (each downscaled by `f`) into a sheet, 5 columns, 2 px grey gaps.
fn contact_sheet(frames: &[crate::snapshot::Image], f: usize) -> crate::snapshot::Image {
    let tiles: Vec<_> = frames.iter().map(|i| downscale(i, f)).collect();
    let (tw, th) = (tiles[0].width, tiles[0].height);
    let cols = 5usize.min(tiles.len());
    let rows = tiles.len().div_ceil(cols);
    let (w, h) = (cols * (tw + 2) + 2, rows * (th + 2) + 2);
    let mut rgba = vec![128u8; w * h * 4];
    for (k, t) in tiles.iter().enumerate() {
        let (ox, oy) = (2 + (k % cols) * (tw + 2), 2 + (k / cols) * (th + 2));
        for y in 0..th {
            let src = &t.rgba[y * tw * 4..(y + 1) * tw * 4];
            let d = ((oy + y) * w + ox) * 4;
            rgba[d..d + tw * 4].copy_from_slice(src);
        }
    }
    for p in rgba.as_chunks_mut::<4>().0 {
        p[3] = 255;
    }
    crate::snapshot::Image { width: w, height: h, rgba }
}

fn run_seq(rec: &mut Rec, s: &Seq, v: &Variant) -> Result<(), String> {
    let (mut h, dir) = make_base(s.base, v, 1.0);
    let name = format!("{}_{}", s.name, v.identity.key());
    let mut frames = Vec::new();
    let mut times: Vec<Duration> = Vec::new();
    let mut quiet = 0;
    let theme = if v.light { "light" } else { "dark" };
    for i in 0..MAX_FRAMES {
        (s.hook)(&mut h, i);
        let img = h.render_at(1.0);
        times.push(h.last_update);
        rec.save(&img, &format!("{name}_f{i:02}.png"));
        frames.push(img);
        if h.last_repaint > Duration::from_millis(100) && i >= 3 {
            quiet += 1;
        } else {
            quiet = 0;
        }
        if quiet >= 2 && i >= 6 {
            break;
        }
    }
    if frames.len() > 1 {
        let sheet = contact_sheet(&frames, 4);
        rec.save(&sheet, &format!("{name}_sheet.png"));
    }
    let mut idle = 0;
    for _ in 0..30 {
        h.step();
        if h.last_repaint < Duration::from_millis(50) {
            idle += 1;
        }
    }
    let ms: Vec<f64> = times.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    let max = ms.iter().copied().fold(0.0, f64::max);
    let mean = ms.iter().sum::<f64>() / ms.len() as f64;
    let _ = writeln!(
        rec.timings,
        "{},{},{theme},{}x{},{},{max:.2},{mean:.2},{idle}",
        s.name,
        v.identity.key(),
        v.size.x as u32,
        v.size.y as u32,
        frames.len()
    );
    if let Some(d) = dir {
        let _ = std::fs::remove_dir_all(d);
    }
    Ok(())
}

fn seq_variants() -> Vec<Variant> {
    UiIdentity::ALL.iter().map(|&identity| Variant { identity, light: false, size: BIG, right_hand: false }).collect()
}

fn run_area(area: &'static str, shots: &[Shot], seqs: &[Seq], variants: &[Variant]) -> usize {
    let mut rec = Rec::new(area);
    for s in shots.iter().filter(|s| s.area == area) {
        let _ = writeln!(rec.notes, "## {}\n\n{}\n\nSteps: {}\n\nFiles: `{}__<variant>.png`\n", s.name, s.shows, s.steps, s.name);
        for v in variants {
            let r = catch_unwind(AssertUnwindSafe(|| {
                let (mut h, dir) = make_base(s.base, v, 2.0);
                (s.setup)(&mut h);
                h.advance(0.5);
                let img = h.render_at(2.0);
                if let Some(d) = dir {
                    let _ = std::fs::remove_dir_all(d);
                }
                img
            }));
            match r {
                Ok(img) => rec.save(&img, &format!("{}__{}.png", s.name, v.tag())),
                Err(e) => {
                    let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_default();
                    let _ = writeln!(rec.notes, "- **FAILED** `{}` in {}: {msg}\n", s.name, v.tag());
                    eprintln!("qa_capture: {area}/{} failed in {}: {msg}", s.name, v.tag());
                }
            }
        }
    }
    for s in seqs.iter().filter(|s| s.area == area) {
        let _ = writeln!(
            rec.notes,
            "## Sequence `{}`\n\n{}\n\nSteps: {}\n\nFiles: `{}_<look>_f00.png ..`, contact sheet `{}_<look>_sheet.png` (frames at 1/4 size, 5 per row, left to right then down).\n",
            s.name, s.shows, s.steps, s.name, s.name
        );
        for v in seq_variants() {
            let r = catch_unwind(AssertUnwindSafe(|| run_seq(&mut rec, s, &v)));
            if r.is_err() {
                let _ = writeln!(rec.notes, "- **FAILED** sequence `{}` ({})\n", s.name, v.identity.key());
            }
        }
    }
    rec.finish()
}

// ---------------------------------------------------------------- shots

fn stroke_demo(h: &mut Headless, tool: Tool, set: usize, idx: usize, rgb: [u8; 3], y: f64) {
    h.app.set_tool(tool);
    brush_by_set(h, set, idx);
    h.app.colour = rgb;
    let pts = curve(h, 220.0, y, 28.0, 520.0);
    drag(h, &pts);
}

fn paint_several(h: &mut Headless) {
    h.app.tool_params[0].size = 22.0;
    stroke_demo(h, Tool::Brush, 0, 0, [200, 40, 40], 600.0);
    stroke_demo(h, Tool::Brush, 1, 0, [40, 160, 90], 650.0);
    stroke_demo(h, Tool::Brush, 2, 1, [250, 200, 40], 700.0);
}

fn shots() -> Vec<Shot> {
    use Base::*;
    macro_rules! s {
        ($a:expr, $n:expr, $b:expr, $shows:expr, $steps:expr, $f:expr) => {
            Shot { area: $a, name: $n, base: $b, shows: $shows, steps: $steps, setup: $f }
        };
    }
    vec![
        // gallery
        s!("gallery", "gallery_empty", Gallery(0), "The gallery with no documents.", "Launch with an empty library.", |_| {}),
        s!("gallery", "gallery_three_docs", Gallery(3), "The gallery with three generated documents.", "Launch with three saved documents; thumbnails loaded.", |_| {}),
        s!("gallery", "gallery_new_canvas", Gallery(3), "The New canvas popover over the gallery.", "Click the + button in the gallery header.", |h| {
            h.click(h.app.gal.header.plus.center());
            h.advance(0.4);
        }),
        // painting
        s!("painting", "canvas_sample", Canvas, "The sample document before painting.", "Open the sample (gradient block, orange disc).", |_| {}),
        s!("painting", "strokes_three_brushes", Canvas, "Three strokes with three brushes from three sets.", "Brush tool, 22 px: red stroke (set 1, brush 1), green (set 2), yellow (set 3), dragged with the mouse.", paint_several),
        s!("painting", "undo_one", Canvas, "After painting three strokes and one Undo.", "Paint three strokes, then Undo once.", |h| {
            paint_several(h);
            h.app.undo_action();
        }),
        s!("painting", "undo_redo", Canvas, "Undo then Redo: the third stroke is back.", "Paint three strokes, Undo, Redo.", |h| {
            paint_several(h);
            h.app.undo_action();
            h.app.redo_action();
        }),
        s!("painting", "hold_to_shape", Canvas, "A zigzag held still snaps to a polyline (Hold to Shape).", "Drag a zigzag, hold still 0.8 s with the button down.", |h| {
            let pts: Vec<Pos2> = [(220.0, 150.0), (330.0, 420.0), (470.0, 170.0), (600.0, 430.0), (700.0, 190.0)]
                .windows(2)
                .flat_map(|w| {
                    let (a, b) = (screen_of(h, w[0].0, w[0].1), screen_of(h, w[1].0, w[1].1));
                    (0..25).map(move |k| a + (b - a) * (k as f32 / 25.0))
                })
                .collect();
            let end = screen_of(h, 700.0, 190.0);
            h.move_to(pts[0]);
            h.step();
            h.press(pts[0], true);
            for p in pts {
                h.move_to(p);
                h.step();
            }
            h.advance(0.8);
            let _ = end;
        }),
        s!("painting", "smudge", Canvas, "A smudge stroke across the gradient block's edge.", "Smudge tool, drag across the block's right edge.", |h| {
            h.app.set_tool(Tool::Smudge);
            h.app.tool_params[1].size = 60.0;
            let pts: Vec<Pos2> = (0..30).map(|i| screen_of(h, 640.0 + f64::from(i) * 4.0, 250.0 + f64::from(i) * 5.0)).collect();
            drag(h, &pts);
        }),
        s!("painting", "eraser", Canvas, "An eraser stroke through the block.", "Eraser tool, drag a diagonal through the block.", |h| {
            h.app.set_tool(Tool::Eraser);
            h.app.tool_params[2].size = 50.0;
            let pts: Vec<Pos2> = (0..30).map(|i| screen_of(h, 240.0 + f64::from(i) * 12.0, 180.0 + f64::from(i) * 10.0)).collect();
            drag(h, &pts);
        }),
        s!("painting", "pour_fill", Canvas, "Pour Fill dropped into the orange disc.", "Press on the disc with the active colour, drag colour in, release.", |h| {
            h.app.colour = [30, 160, 120];
            let p = screen_of(h, 830.0, 220.0);
            h.move_to(p);
            h.step();
            h.app.begin_pour_fill(p);
            h.advance(0.3);
            h.press(p, false);
            h.advance(0.3);
        }),
        // brushes
        s!("brushes", "brush_library", Canvas, "The Brush library popover.", "Tap the brush button.", |h| open(h, Panel::Brush)),
        s!("brushes", "brush_library_smudge", Canvas, "Brush library opened for the Smudge tool.", "Select Smudge, tap the brush button.", |h| {
            h.app.set_tool(Tool::Smudge);
            h.app.library_tool = Tool::Smudge;
            open(h, Panel::Brush);
        }),
        s!("brushes", "brush_library_second_set", Canvas, "Brush library with the second set's brushes in view.", "Open the library, pick a different brush in set 2.", |h| {
            brush_by_set(h, 1, 0);
            open(h, Panel::Brush);
        }),
        s!("brushes", "brush_bench", Canvas, "Brush Bench opened on the tool's brush.", "Open Brush Bench from the library tool.", |h| {
            h.app.open_studio_for_library_tool();
            h.advance(0.3);
        }),
        // layers
        s!("layers", "layers_panel", Canvas, "The Layers popover with the default layers.", "Tap the layers button.", |h| open(h, Panel::Layers)),
        s!("layers", "layers_many", Canvas, "Layers popover with five layers.", "Add four layers, open the panel.", |h| {
            for _ in 0..4 {
                h.app.session.add_layer(None);
            }
            open(h, Panel::Layers);
        }),
        s!("layers", "layers_blend_multiply", Canvas, "A layer set to Multiply over the sample.", "Add a layer, fill it, set blend Multiply; open the panel.", |h| {
            h.app.session.add_layer(None);
            let ids = layer_ids(h);
            let id = *ids.last().unwrap();
            h.app.session.fill_layer(id, [240, 120, 120]);
            h.app.session.set_blend(id, BlendMode::Multiply);
            h.app.needs_full = true;
            open(h, Panel::Layers);
        }),
        s!("layers", "layers_blend_overlay_screen", Canvas, "Two layers with Overlay and Screen blend.", "Two filled layers, blend Overlay and Screen.", |h| {
            for (rgb, mode) in [([230u8, 200, 40], BlendMode::Overlay), ([40u8, 120, 230], BlendMode::Screen)] {
                h.app.session.add_layer(None);
                let id = *layer_ids(h).last().unwrap();
                h.app.session.fill_layer(id, rgb);
                h.app.session.set_blend(id, mode);
            }
            h.app.needs_full = true;
            h.advance(0.2);
        }),
        s!("layers", "layers_reordered", Canvas, "Layer order changed: the filled layer moved below.", "Add and fill a layer, move it down one step, open the panel.", |h| {
            h.app.session.add_layer(None);
            let id = *layer_ids(h).last().unwrap();
            h.app.session.fill_layer(id, [200, 60, 160]);
            h.app.session.move_layer(id, false);
            h.app.needs_full = true;
            open(h, Panel::Layers);
        }),
        s!("layers", "layers_lock_transparency", Canvas, "Lock Transparency on the sample layer; a stroke only recolours existing pixels.", "Alpha-lock layer 1, paint a red stroke across the block edge.", |h| {
            let id = h.app.session.doc.layer_id(0).unwrap();
            h.app.session.set_alpha_lock(id, true);
            stroke_demo(h, Tool::Brush, 0, 0, [220, 40, 40], 300.0);
            h.app.tool_params[0].size = 60.0;
            let pts = curve(h, 150.0, 420.0, 20.0, 700.0);
            drag(h, &pts);
        }),
        s!("layers", "layers_clip_below", Canvas, "Clip to Layer Below: a fill clipped to the block.", "Add a layer, fill it, enable Clip, open the panel.", |h| {
            h.app.session.add_layer(None);
            let id = *layer_ids(h).last().unwrap();
            h.app.session.fill_layer(id, [250, 230, 60]);
            h.app.session.set_clipped(id, true);
            h.app.needs_full = true;
            open(h, Panel::Layers);
        }),
        // colors
        s!("colors", "colour_wheel", Canvas, "Colour popover, Wheel tab.", "Tap the colour circle.", |h| {
            h.app.colour_ui.tab = ColourTab::Wheel;
            open(h, Panel::Colour);
        }),
        s!("colors", "colour_square", Canvas, "Colour popover, Square tab.", "Open Colour, Square tab.", |h| {
            h.app.colour_ui.tab = ColourTab::Square;
            open(h, Panel::Colour);
        }),
        s!("colors", "colour_swatches", Canvas, "Colour popover, Swatches tab with history.", "Push five colours to history, open Swatches.", |h| {
            for c in [[200, 40, 40], [40, 160, 90], [250, 200, 40], [60, 90, 200], [120, 60, 160]] {
                h.app.colour_store.push_history(c);
            }
            h.app.colour_ui.tab = ColourTab::Swatches;
            open(h, Panel::Colour);
        }),
        s!("colors", "colour_schemes", Canvas, "Colour popover, Schemes tab.", "Open Colour, Schemes tab.", |h| {
            h.app.colour_ui.tab = ColourTab::Schemes;
            open(h, Panel::Colour);
        }),
        s!("colors", "pick_colour", Canvas, "Pick Color loupe held over the orange disc.", "Press on the disc and start the eyedropper; hold.", |h| {
            let p = screen_of(h, 830.0, 220.0);
            h.move_to(p);
            h.press(p, true);
            h.app.begin_eyedrop(p);
            h.advance(0.2);
        }),
        // selection-transform
        s!("selection-transform", "selection_rect", Canvas, "Selection toolbar and a rectangle selection.", "Edit mode Select, shape Rectangle, drag over the block.", |h| {
            h.app.edit_mode = EditMode::Select;
            h.app.sel.shape = Shape::Rectangle;
            h.app.needs_full = true;
            h.advance(0.2);
            drag(h, &[screen_of(h, 260.0, 200.0), screen_of(h, 500.0, 330.0), screen_of(h, 620.0, 450.0)]);
        }),
        s!("selection-transform", "selection_ellipse", Canvas, "Ellipse selection.", "Shape Ellipse, drag over the disc.", |h| {
            h.app.edit_mode = EditMode::Select;
            h.app.sel.shape = Shape::Ellipse;
            h.advance(0.2);
            drag(h, &[screen_of(h, 720.0, 100.0), screen_of(h, 800.0, 220.0), screen_of(h, 940.0, 340.0)]);
        }),
        s!("selection-transform", "selection_auto", Canvas, "Automatic (flood) selection of the disc.", "Shape Automatic, tap the disc.", |h| {
            h.app.edit_mode = EditMode::Select;
            h.app.sel.shape = Shape::Automatic;
            h.advance(0.2);
            h.click(screen_of(h, 830.0, 220.0));
            h.advance(0.3);
        }),
        s!("selection-transform", "transform_uniform", Canvas, "Transform with handles.", "Begin transform on the layer.", |h| {
            h.app.begin_transform();
            h.advance(0.3);
        }),
        s!("selection-transform", "adjustments_list", Canvas, "Adjustments popover (list).", "Tap the adjustments button.", |h| open(h, Panel::Adjustments)),
        s!("selection-transform", "adjustment_curves", Canvas, "Curves adjustment banner.", "Pick Curves from Adjustments.", |h| {
            h.app.enter_adjustment(AdjustKind::Curves);
            h.advance(0.3);
        }),
        s!("selection-transform", "adjustment_halftone_applied", Canvas, "Halftone adjustment applied to the layer.", "Enter Halftone, apply.", |h| {
            h.app.enter_adjustment(AdjustKind::Halftone);
            h.advance(0.3);
            let ctx = h.ctx.clone();
            h.app.adjust_apply(&ctx);
            h.advance(0.5);
        }),
        // actions-settings
        s!("actions-settings", "actions_add", Canvas, "Actions, Add tab.", "Open Actions.", |h| {
            h.app.actions_ui.tab = ActionsTab::Add;
            open(h, Panel::Actions);
        }),
        s!("actions-settings", "actions_canvas", Canvas, "Actions, Canvas tab.", "Open Actions, Canvas tab.", |h| {
            h.app.actions_ui.tab = ActionsTab::Canvas;
            open(h, Panel::Actions);
        }),
        s!("actions-settings", "actions_3d", Canvas, "Actions, 3D tab.", "Open Actions, 3D tab.", |h| {
            h.app.actions_ui.tab = ActionsTab::ThreeD;
            open(h, Panel::Actions);
        }),
        s!("actions-settings", "actions_share", Canvas, "Actions, Share tab.", "Open Actions, Share tab.", |h| {
            h.app.actions_ui.tab = ActionsTab::Share;
            open(h, Panel::Actions);
        }),
        s!("actions-settings", "actions_prefs", Canvas, "Actions, Prefs tab.", "Open Actions, Prefs tab.", |h| {
            h.app.actions_ui.tab = ActionsTab::Prefs;
            open(h, Panel::Actions);
        }),
        s!("actions-settings", "actions_help", Canvas, "Actions, Help tab.", "Open Actions, Help tab.", |h| {
            h.app.actions_ui.tab = ActionsTab::Help;
            open(h, Panel::Actions);
        }),
        s!("actions-settings", "help_user_guide", Canvas, "The User Guide window.", "Open the guide at the first chapter.", |h| {
            h.app.open_user_guide("");
            h.advance(0.4);
        }),
        s!("actions-settings", "help_welcome_tour_replay", Canvas, "Welcome tour replayed from Help.", "Call Help > Welcome tour (replay).", |h| {
            h.app.tour.replay();
            h.advance(0.4);
        }),
        s!("actions-settings", "about", Canvas, "The About sheet.", "Open About.", |h| {
            h.app.sheet = Some(Sheet::About);
            h.advance(0.4);
        }),
        s!("actions-settings", "licenses", Canvas, "About > Licenses.", "Open Licenses.", |h| {
            h.app.sheet = Some(Sheet::Licenses);
            h.advance(0.4);
        }),
        s!("actions-settings", "cheatsheet", Canvas, "Shortcuts cheat sheet.", "Open the cheat sheet.", |h| {
            h.app.sheet = Some(Sheet::Cheatsheet);
            h.advance(0.4);
        }),
        s!("actions-settings", "gestures_sheet", Canvas, "Gesture controls sheet.", "Open the Gestures sheet.", |h| {
            h.app.sheet = Some(Sheet::Gestures);
            h.advance(0.4);
        }),
        // frames
        s!("frames", "frames_timeline", Frames, "Frames timeline with four frames.", "Open a four-frame document with Frames on.", |_| {}),
        s!("frames", "frames_second_frame", Frames, "Second frame shown.", "Show frame 2.", |h| {
            h.app.anim_show(1);
            h.advance(0.3);
        }),
        s!("frames", "frames_playing", Frames, "Playback in progress, a few frames in.", "Press play, run 0.4 s.", |h| {
            h.app.anim_play();
            h.advance(0.4);
        }),
        s!("frames", "frames_paused", Frames, "Playback paused.", "Play 0.5 s then pause.", |h| {
            h.app.anim_play();
            h.advance(0.5);
            h.app.anim_pause();
            h.advance(0.3);
        }),
        s!("frames", "frames_onion_on", Frames, "Onion skins enabled with 2 frames each side.", "Turn onion skin on, count 2.", |h| {
            h.app.session.doc.animation.onion_enabled = true;
            h.app.anim.canvas_key = None;
            h.app.anim_show(1);
            h.advance(0.4);
        }),
        s!("frames", "frames_canvas_actions", Frames, "Actions > Canvas tab with the animation rows.", "Open Actions, Canvas tab.", |h| {
            h.app.actions_ui.tab = ActionsTab::Canvas;
            open(h, Panel::Actions);
        }),
        // import-export
        s!("import-export", "share_tab", Canvas, "Share tab: export format rows.", "Open Actions, Share.", |h| {
            h.app.actions_ui.tab = ActionsTab::Share;
            open(h, Panel::Actions);
        }),
        s!("import-export", "share_tab_scrolled", Canvas, "Share tab scrolled to the animation/timelapse rows.", "Open Share, scroll down.", |h| {
            h.app.actions_ui.tab = ActionsTab::Share;
            open(h, Panel::Actions);
            let c = h.app.popovers.body_rect().map_or(vec2(640.0, 400.0).to_pos2(), |r| r.center());
            h.scroll_at(c, -600.0);
        }),
        s!("import-export", "add_tab_import", Canvas, "Add tab with its import rows (stops at the file dialog).", "Open Actions, Add.", |h| {
            h.app.actions_ui.tab = ActionsTab::Add;
            open(h, Panel::Actions);
        }),
        s!("import-export", "import_path_typed", Canvas, "Add tab with a path typed in the import field.", "Type a path in the import box.", |h| {
            h.app.actions_ui.tab = ActionsTab::Add;
            h.app.actions_ui.import_path = "/home/user/Pictures/reference.png".into();
            open(h, Panel::Actions);
        }),
        s!("import-export", "export_frames_actions", Frames, "Share tab for a frames document.", "Open Share on a frames document.", |h| {
            h.app.actions_ui.tab = ActionsTab::Share;
            open(h, Panel::Actions);
        }),
        s!("import-export", "gallery_import_menu", Gallery(3), "Gallery with a selected document.", "Open the gallery, tap select.", |h| {
            h.advance(0.3);
        }),
        // onboarding-a11y
        s!("onboarding-a11y", "welcome_card_1", Tour, "Welcome card 1 over the empty gallery.", "First launch.", |_| {}),
        s!("onboarding-a11y", "welcome_card_2", Tour, "Welcome card 2 (sample).", "Press Show me around.", |h| {
            h.app.tour.show_around();
            h.advance(0.3);
        }),
        s!("onboarding-a11y", "coach_mark_paint", Canvas, "First coach mark over the canvas.", "Tour sample opened, coach mark 'Paint'.", |h| {
            h.app.tour = crate::onboarding::Tour::at_startup(None, false, false);
            h.app.tour.show_around();
            h.app.tour.sample_opened();
            h.advance(0.6);
        }),
        s!("onboarding-a11y", "canvas_main", Canvas, "Main canvas chrome.", "Open the sample.", |_| {}),
        s!("onboarding-a11y", "keyboard_focus_topbar", Canvas, "Keyboard focus on the top bar.", "Press Tab twice.", |h| {
            for _ in 0..2 {
                h.event(eframe::egui::Event::Key {
                    key: eframe::egui::Key::Tab,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: eframe::egui::Modifiers::NONE,
                });
                h.step();
            }
            h.advance(0.3);
        }),
        s!("onboarding-a11y", "keyboard_focus_popover", Canvas, "Keyboard focus inside the Layers popover.", "Open Layers, press Tab.", |h| {
            open(h, Panel::Layers);
            h.event(eframe::egui::Event::Key {
                key: eframe::egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: eframe::egui::Modifiers::NONE,
            });
            h.advance(0.3);
        }),
        s!("onboarding-a11y", "toast", Canvas, "A status toast.", "Show 'Undo Stroke'.", |h| h.app.show_toast("Undo Stroke", 5.0)),
    ]
}

// ---------------------------------------------------------------- sequences

fn seqs() -> Vec<Seq> {
    macro_rules! q {
        ($a:expr, $n:expr, $b:expr, $shows:expr, $steps:expr, $f:expr) => {
            Seq { area: $a, name: $n, base: $b, shows: $shows, steps: $steps, hook: $f }
        };
    }
    fn tap(h: &mut Headless, i: usize, panel: Panel) {
        let c = h.app.anchor_rect(panel).center();
        match i {
            0 => {
                h.move_to(c);
                h.press(c, true);
            }
            1 => h.press(c, false),
            _ => {}
        }
    }
    vec![
        q!("layers", "popover_layers_open", Base::Canvas, "Layers popover opening.", "Press and release on the layers button.", |h, i| tap(h, i, Panel::Layers)),
        q!("layers", "popover_layers_close", Base::Canvas, "Layers popover closing.", "Open it, settle, tap the button again.", |h, i| {
            if i == 0 {
                h.open_panel(Panel::Layers);
            }
            if i >= 1 {
                tap(h, i - 1, Panel::Layers);
            }
        }),
        q!("colors", "popover_colour_open", Base::Canvas, "Colour popover opening.", "Tap the colour circle.", |h, i| tap(h, i, Panel::Colour)),
        q!("brushes", "popover_brush_open", Base::Canvas, "Brush library opening.", "Tap the brush button.", |h, i| tap(h, i, Panel::Brush)),
        q!("actions-settings", "popover_actions_open", Base::Canvas, "Actions opening.", "Tap Actions.", |h, i| tap(h, i, Panel::Actions)),
        q!("actions-settings", "popover_actions_close", Base::Canvas, "Actions closing.", "Open it, settle, tap again.", |h, i| {
            if i == 0 {
                h.open_panel(Panel::Actions);
            }
            if i >= 1 {
                tap(h, i - 1, Panel::Actions);
            }
        }),
        q!("selection-transform", "popover_adjustments_open", Base::Canvas, "Adjustments opening.", "Tap Adjustments.", |h, i| tap(h, i, Panel::Adjustments)),
        q!("gallery", "gallery_open_document", Base::Gallery(3), "Opening a document from the gallery (zoom into canvas).", "Click the first thumbnail.", |h, i| {
            if i == 0 {
                let c = h.app.gal.card_rects[0].1.center();
                h.move_to(c);
                h.press(c, true);
            }
            if i == 1 {
                let c = h.app.gal.card_rects[0].1.center();
                h.press(c, false);
            }
        }),
        q!("gallery", "gallery_new_canvas_popover", Base::Gallery(3), "New canvas popover opening.", "Click +.", |h, i| {
            let c = h.app.gal.header.plus.center();
            match i {
                0 => {
                    h.move_to(c);
                    h.press(c, true);
                }
                1 => h.press(c, false),
                _ => {}
            }
        }),
        q!("onboarding-a11y", "toast_show_hide", Base::Canvas, "A toast appearing and fading.", "Show a 0.5 s toast.", |h, i| {
            if i == 0 {
                h.app.show_toast("Undo Stroke", 0.5);
            }
        }),
        q!("onboarding-a11y", "welcome_card_advance", Base::Tour, "Welcome card 1 to 2.", "Advance the tour card.", |h, i| {
            if i == 0 {
                h.app.tour.show_around();
            }
        }),
        q!("onboarding-a11y", "coach_mark_enter", Base::Canvas, "A coach mark appearing.", "Enter the first coach mark.", |h, i| {
            if i == 0 {
                h.app.tour = crate::onboarding::Tour::at_startup(None, false, false);
                h.app.tour.show_around();
                h.app.tour.sample_opened();
            }
        }),
        q!("frames", "frames_toggle_timeline", Base::Canvas, "The Frames timeline sliding in.", "Toggle Frames on.", |h, i| {
            if i == 0 {
                h.app.toggle_frames();
            }
        }),
        q!("frames", "frames_playback", Base::Frames, "Playback over 40 frames.", "Press play.", |h, i| {
            if i == 0 {
                h.app.anim_play();
            }
        }),
        q!("painting", "stroke_drawing", Base::Canvas, "A stroke being drawn.", "Press, drag along a curve, release.", |h, i| {
            if i == 0 {
                h.app.tool_params[0].size = 22.0;
            }
            let p = screen_of(h, 220.0 + f64::from(i as u32) * 12.0, 600.0 + (f64::from(i as u32) * 0.4).sin() * 28.0);
            match i {
                0 => {
                    h.move_to(p);
                    h.press(p, true);
                }
                1..=38 => h.move_to(p),
                _ => h.press(p, false),
            }
        }),
    ]
}

// ---------------------------------------------------------------- entry points

const AREAS: [&str; 10] = [
    "gallery",
    "painting",
    "brushes",
    "layers",
    "colors",
    "selection-transform",
    "actions-settings",
    "frames",
    "import-export",
    "onboarding-a11y",
];

/// The special onboarding-a11y stills: Right-hand interface, large window.
fn extra_variants(area: &str) -> Vec<Variant> {
    if area == "onboarding-a11y" {
        let mut v = Vec::new();
        for identity in UiIdentity::ALL {
            v.push(Variant { identity, light: false, size: BIG, right_hand: true });
            v.push(Variant { identity, light: false, size: vec2(1920.0, 1200.0), right_hand: false });
        }
        v
    } else {
        Vec::new()
    }
}

#[test]
#[ignore = "writes the QA capture; run with tools/qa_capture.sh"]
fn qa_capture() {
    let shots = shots();
    let seqs = seqs();
    let only = std::env::var("ESKIZ_QA_AREA").ok();
    let mut manifest = String::from("# QA capture manifest\n\n| area | files |\n|---|---|\n");
    for area in AREAS {
        if only.as_deref().is_some_and(|o| o != area) {
            continue;
        }
        let mut variants = all_variants();
        variants.extend(extra_variants(area));
        let n = run_area(area, &shots, &seqs, &variants);
        eprintln!("qa_capture: {area}: {n} files");
        let _ = writeln!(manifest, "| {area} | {n} |");
    }
    std::fs::write(out_root().join("MANIFEST.part"), manifest).unwrap();
}
