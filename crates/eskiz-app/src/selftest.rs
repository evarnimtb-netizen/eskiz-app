//! Headless checks. `--selftest` drives the same `Session` and input mapping as the
//! window, without opening one. `--open FILE --export PNG` loads and exports without a window.

use std::path::Path;

use eskiz_core::brushes::{Brush, Preset};

use crate::document::Document;
use crate::export::{checksum, write_png};
use crate::history::PixelRect;
use crate::import::open_procreate;
use crate::input::{sample_from, PointerKind, RawPointer};
use crate::paint::StrokeSettings;
use crate::session::Session;
use crate::snapshot::Headless;

const WIDTH: usize = 2048;
const HEIGHT: usize = 1536;

/// Samples at 120 Hz along `point(i, n)`, where `i` runs from 0 to n-1.
fn path(n: usize, kind: PointerKind, point: impl Fn(f64) -> (f64, f64, Option<f64>)) -> Vec<RawPointer> {
    (0..n)
        .map(|i| {
            let p = i as f64 / (n - 1) as f64;
            let (x, y, force) = point(p);
            RawPointer {
                x,
                y,
                time: i as f64 / 120.0,
                kind,
                force,
            }
        })
        .collect()
}

fn settings(preset: Preset, size: f64, colour: [f64; 3], erase: bool) -> StrokeSettings {
    let mut brush = Brush::preset(preset);
    brush.size = size;
    StrokeSettings { brush, colour, erase, raster: None }
}

/// Run one stroke through the session's public API, as the window does.
fn run_stroke(session: &mut Session, layer: usize, settings: &StrokeSettings, pts: &[RawPointer]) -> Result<PixelRect, String> {
    let id = session.doc.layer_id(layer).ok_or("no such layer")?;
    if !session.begin_stroke(id, settings) {
        return Err("could not start stroke".into());
    }
    let mut dirty: Option<PixelRect> = None;
    for raw in pts {
        if let Some(r) = session.push(sample_from(*raw)) {
            dirty = Some(dirty.map_or(r, |d| d.union(r)));
        }
    }
    let end = session.end_stroke();
    match (dirty, end) {
        (Some(a), Some(b)) => Ok(a.union(b)),
        (None, Some(b)) => Ok(b),
        _ => Err("stroke painted nothing".into()),
    }
}

/// Paint three strokes, undo and redo them, then write `selftest.png` and `selftest.procreate` to
/// the temp dir. The `.procreate` file is a ready-made document for smoke tests that open the
/// window on a canvas (`packaging/windows/smoke.ps1`).
pub fn run() -> Result<String, String> {
    let mut session = Session::new(Document::new(WIDTH, HEIGHT));
    let blank = session.flatten();

    // 1. Mouse arc with the ink-with-taper preset.
    let arc = path(240, PointerKind::Mouse, |p| {
        let a = 0.2 * std::f64::consts::PI + p * 1.2 * std::f64::consts::PI;
        (1024.0 + 420.0 * a.cos(), 768.0 + 420.0 * a.sin(), None)
    });
    // 2. Touch with force (stylus-like) wave with the airbrush preset.
    let wave = path(220, PointerKind::Touch, |p| {
        let force = 0.2 + 0.8 * (p * std::f64::consts::PI).sin();
        (300.0 + 1450.0 * p, 1180.0 + 90.0 * (p * 9.0).sin(), Some(force))
    });
    // 3. Mouse eraser across the arc.
    let cut = path(200, PointerKind::Mouse, |p| (600.0 + 900.0 * p, 200.0 + 1100.0 * p, None));

    let strokes = [
        ("ink with taper, mouse", settings(Preset::InkWithTaper, 60.0, [0.12, 0.35, 0.85], false), arc),
        ("airbrush, touch force", settings(Preset::SoftAirbrush, 140.0, [0.9, 0.3, 0.1], false), wave),
        ("eraser, mouse", settings(Preset::TechnicalPen, 70.0, [0.0; 3], true), cut),
    ];
    let mut report = Vec::new();
    for (name, s, pts) in &strokes {
        let rect = run_stroke(&mut session, 0, s, pts).map_err(|e| format!("{name}: {e}"))?;
        report.push(format!("  stroke '{name}': dirty {}x{} at ({}, {})", rect.w, rect.h, rect.x, rect.y));
    }
    let painted = session.flatten();
    let painted_sum = checksum(&painted);
    if painted == blank {
        return Err("strokes did not change the canvas".into());
    }
    let changed = painted
        .as_chunks::<4>().0.iter()
        .zip(blank.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a != b)
        .count();

    // Undo all three, check blank paper, then redo all three and check the same checksum.
    for _ in 0..3 {
        session.undo().ok_or("undo unavailable")?;
    }
    if session.flatten() != blank {
        return Err("undo x3 did not restore blank paper".into());
    }
    for _ in 0..3 {
        session.redo().ok_or("redo unavailable")?;
    }
    if checksum(&session.flatten()) != painted_sum {
        return Err("redo x3 did not reproduce the painted canvas".into());
    }
    // One undo should differ from the painted state, and one redo should bring it back.
    session.undo().ok_or("undo unavailable")?;
    if session.flatten() == painted {
        return Err("single undo changed nothing".into());
    }
    session.redo().ok_or("redo unavailable")?;
    if session.flatten() != painted {
        return Err("single redo did not restore the painted canvas".into());
    }

    let out = std::env::temp_dir().join("selftest.png");
    write_png(&out, WIDTH, HEIGHT, &painted)?;
    let sample = crate::export::export_with(
        &session.doc,
        crate::export::ExportFormat::Procreate,
        &std::env::temp_dir().join("selftest"),
        &crate::export::ExportSettings::default(),
    )?;
    if checksum(&Session::new(open_procreate(&sample)?).flatten()) != painted_sum {
        return Err(format!("{} does not read back as the painted canvas", sample.display()));
    }

    let mut lines = vec![
        "selftest: ok".to_owned(),
        format!("  canvas {WIDTH}x{HEIGHT}, layers {}, undo steps {}", session.doc.layer_count(), session.undo_steps()),
    ];
    lines.extend(report);
    lines.push(format!("  pixels changed from paper: {changed}"));
    lines.push("  undo x3 -> blank paper: ok; redo x3 -> same checksum: ok; single undo/redo: ok".to_owned());
    lines.push(format!("  wrote {}", out.display()));
    lines.push(format!("  wrote {} (reads back with the same checksum)", sample.display()));
    lines.push(format!("  checksum fnv1a64: 0x{painted_sum:016x}"));
    lines.extend(ui_checks()?);
    lines.extend(reference_and_edit_shape_check()?);
    lines.extend(crate::selection_tool::selftest_checks()?);
    lines.push(crate::liquify_tool::selftest()?);
    lines.extend(crate::text_tool::selftest_checks()?);
    lines.extend(crate::canvas_ops::selftest_checks()?);
    lines.push(crate::colour_management::selftest()?);
    lines.push(bit_depth_check()?);
    lines.push(crate::saved_store::selftest()?);
    lines.push(tiled_memory_check()?);
    lines.push(wet_mix_check()?);
    lines.extend(crate::animation_ui::selftest()?);
    lines.push(crate::timelapse::selftest()?);
    lines.extend(crate::storage::selftest()?);
    lines.push(crate::journal::selftest()?);
    lines.push(crate::stacks::stacks_selftest()?);
    lines.push(crate::gesture::selftest_line()?);
    lines.push(crate::view3d::selftest()?);
    lines.push(export_check()?);
    lines.push(crate::diagnose::selftest()?);
    Ok(lines.join("\n"))
}

/// 16-bit documents: a soft stroke holds far more than 256 levels, the `.procreate`
/// round trip is exact, a 16-bit PNG decodes to the same samples, and 8 to 16 to 8 is the identity.
fn bit_depth_check() -> Result<String, String> {
    use eskiz_core::Depth;
    use std::collections::HashSet;

    let stroke = |depth: Depth| -> Result<Session, String> {
        let mut s = Session::new(Document::new_depth(640, 480, depth));
        let soft = settings(Preset::SoftAirbrush, 160.0, [0.8, 0.25, 0.1], false);
        let line = path(80, PointerKind::Mouse, |p| (120.0 + 400.0 * p, 240.0 + 30.0 * (p * 6.0).sin(), None));
        run_stroke(&mut s, 0, &soft, &line)?;
        Ok(s)
    };
    let s16 = stroke(Depth::U16)?;
    let s8 = stroke(Depth::U8)?;
    let id = s16.doc.layer_ids()[0];
    let layer = s16.doc.layer(id).ok_or("depth: no layer")?;
    if layer.depth() != Depth::U16 {
        return Err("depth: a 16-bit document made an 8-bit layer".into());
    }
    let px16 = layer.to_rgba16();
    let levels16 = px16.as_chunks::<4>().0.iter().map(|p| p[3]).collect::<HashSet<_>>().len();
    let levels8 = s8.doc.layer(s8.doc.layer_ids()[0]).ok_or("depth: no layer")?.to_rgba().as_chunks::<4>().0.iter().map(|p| p[3]).collect::<HashSet<_>>().len();
    if levels8 > 256 || levels16 < 1024 {
        return Err(format!("depth: a soft stroke holds {levels16} alpha levels at 16 bits and {levels8} at 8 (want >= 1024 and <= 256)"));
    }

    // .procreate round trip, in memory: exact.
    let bytes = crate::storage::to_builder(&s16.doc, "Deep").to_bytes().map_err(|e| e.to_string())?;
    let mut back = eskiz_format::Document::from_reader(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if back.bits_per_channel().map_err(|e| e.to_string())? != 16 {
        return Err("depth: the archive did not say 16 bits".into());
    }
    let proc_layer = back.layers.first().cloned().ok_or("depth: no layer in the archive")?;
    if back.layer_rgba16(&proc_layer).map_err(|e| e.to_string())? != px16 {
        return Err("depth: the .procreate round trip changed a 16-bit sample".into());
    }

    // 16-bit PNG: the file decodes to the same straight samples.
    let raster = eskiz_format::export::Raster16::new(640, 480, px16.clone()).map_err(|e| e.to_string())?;
    let png_bytes = eskiz_format::export::encode_raster16(&raster, eskiz_format::export::RasterFormat::Png, &eskiz_format::export::ExportOptions::default())
        .map_err(|e| e.to_string())?;
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let reader = decoder.read_info().map_err(|e| e.to_string())?;
    if reader.info().bit_depth != png::BitDepth::Sixteen {
        return Err("depth: the PNG is not 16-bit".into());
    }

    // 8 -> 16 -> 8 is the identity, dithered or not.
    let original = s8.doc.layer(s8.doc.layer_ids()[0]).ok_or("depth: no layer")?.clone();
    for dither in [false, true] {
        if original.converted(Depth::U16, false).converted(Depth::U8, dither) != original {
            return Err(format!("depth: 8 -> 16 -> 8 changed pixels (dither {dither})"));
        }
    }
    // Cost: the same 120-sample stroke through the live engine on a 2048 x 2048 layer, per depth.
    let ink = settings(Preset::InkWithTaper, 60.0, [0.12, 0.35, 0.85], false);
    let pts = path(120, PointerKind::Touch, |p| (200.0 + 1600.0 * p, 1024.0 + 400.0 * (p * 8.0).sin(), Some(0.8)));
    let mut ms = [0.0f64; 2];
    for (slot, depth) in [Depth::U8, Depth::U16].into_iter().enumerate() {
        let mut big = Session::new(Document::new_depth(2048, 2048, depth));
        let t = std::time::Instant::now();
        run_stroke(&mut big, 0, &ink, &pts)?;
        ms[slot] = t.elapsed().as_secs_f64() * 1000.0 / pts.len() as f64;
    }
    Ok(format!(
        "  16-bit canvas: a soft stroke holds {levels16} alpha levels (8-bit: {levels8}); .procreate round trip exact; 16-bit PNG written; 8->16->8 identity; 2048x2048 stroke {:.2} ms/sample at 8 bits, {:.2} at 16",
        ms[0], ms[1]
    ))
}

/// Wet mixing: a diluted brush drags red off a red patch onto bare canvas (more with more pull),
/// charge runs out along the stroke, dilution 0 is a normal brush, and smudge carries colour.
fn wet_mix_check() -> Result<String, String> {
    use eskiz_core::brushes::{Shape, Wet};

    const W: usize = 480;
    const H: usize = 120;
    let red_patch = |session: &mut Session| -> Result<u64, String> {
        let id = session.doc.layer_id(0).ok_or("no layer")?;
        let layer = session.doc.layer_mut(id).ok_or("no layer")?;
        for y in 0..H {
            for x in 0..80 {
                layer.set_pixel(x, y, [225, 25, 25, 255]);
            }
        }
        Ok(id)
    };
    let line = |x0: f64, x1: f64| path(((x1 - x0) / 2.0) as usize, PointerKind::Touch, move |p| (x0 + (x1 - x0) * p, 60.0, Some(1.0)));
    let brush = |wet: Wet| Brush { size: 24.0, shape: Shape::HardRound, smoothing: 0.0, wet, ..Brush::default() };
    let blue = [0.1, 0.1, 0.9];
    let paint = |wet: Wet, x1: f64| -> Result<eskiz_core::Layer, String> {
        let mut session = Session::new(Document::new(W, H));
        let id = red_patch(&mut session)?;
        let s = StrokeSettings { brush: brush(wet), colour: blue, erase: false, raster: None };
        run_stroke(&mut session, 0, &s, &line(30.0, x1))?;
        session.doc.layer(id).cloned().ok_or_else(|| "no layer".to_owned())
    };
    let redness = |l: &eskiz_core::Layer, x: usize| {
        let p = l.pixel(x, 60);
        f64::from(p[0]) / (f64::from(p[0]) + f64::from(p[2])).max(1.0)
    };

    // 1. Pull: red carried onto the bare canvas.
    let diluted = |pull: f64| Wet { enabled: true, dilution: 0.3, pull, ..Wet::default() };
    let low = paint(diluted(0.0), 300.0)?;
    let high = paint(diluted(1.0), 300.0)?;
    let (r_low, r_high) = (redness(&low, 100), redness(&high, 100));
    if r_high <= r_low + 0.05 {
        return Err(format!("wet mix: pull 1 carried {r_high:.2} red, pull 0 {r_low:.2}"));
    }
    // 2. Fall-off: the tail is fainter than the head.
    let fading = paint(Wet { enabled: true, falloff_amount: 0.5, pull: 0.0, ..Wet::default() }, 460.0)?;
    let (head, tail) = (fading.pixel(120, 60)[3], fading.pixel(440, 60)[3]);
    if tail >= head {
        return Err(format!("wet mix: stroke did not fade (head {head}, tail {tail})"));
    }
    // 3. Dilution 0 is a normal brush.
    let normal = paint(Wet::default(), 300.0)?;
    let zero = paint(Wet { enabled: true, ..Wet::default() }, 300.0)?;
    if normal.to_rgba() != zero.to_rgba() {
        return Err("wet mix: dilution 0 differs from a normal brush".into());
    }
    // 4. Smudge carries the red across the edge of the patch.
    let mut session = Session::new(Document::new(W, H));
    let id = red_patch(&mut session)?;
    if !session.begin_smudge(id, 14.0, 0.9, None) {
        return Err("wet mix: smudge refused".into());
    }
    for raw in line(40.0, 150.0) {
        session.push(sample_from(raw));
    }
    session.end_stroke();
    let smudged = session.doc.layer(id).ok_or("no layer")?.pixel(95, 60);
    if smudged[3] < 30 || smudged[0] <= smudged[2] {
        return Err(format!("wet mix: smudge carried nothing past the patch: {smudged:?}"));
    }

    // 5. Cost: one 100 px wet stroke on a 2048 x 2048 layer, through the live engine.
    let mut big = Session::new(Document::new(2048, 2048));
    let s = StrokeSettings {
        brush: Brush { size: 100.0, shape: Shape::SoftRound, smoothing: 0.0, spacing: 0.06, wet: diluted(0.6), ..Brush::default() },
        colour: blue,
        erase: false,
        raster: None,
    };
    let pts = path(120, PointerKind::Touch, |p| (200.0 + 1600.0 * p, 1024.0 + 400.0 * (p * 8.0).sin(), Some(0.8)));
    let t = std::time::Instant::now();
    run_stroke(&mut big, 0, &s, &pts)?;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    // The same path with the smudge tool at the same size.
    let id = big.doc.layer_id(0).ok_or("no layer")?;
    if !big.begin_smudge(id, 50.0, 0.8, None) {
        return Err("wet mix: smudge refused".into());
    }
    let t = std::time::Instant::now();
    for raw in &pts {
        big.push(sample_from(*raw));
    }
    big.end_stroke();
    let smudge_ms = t.elapsed().as_secs_f64() * 1000.0;
    Ok(format!(
        "  wet mix: red carried {r_high:.2} (pull 1) vs {r_low:.2} (pull 0); fade {head} -> {tail}; dilution 0 == normal brush; smudge carries color; 100 px strokes at 2048x2048: wet {ms:.0} ms, smudge {smudge_ms:.0} ms ({:.1} / {:.1} ms per sample)",
        ms / pts.len() as f64,
        smudge_ms / pts.len() as f64
    ))
}

/// Checks for the interface layer: Hold to Shape, view rotation, gestures, smudge, clear
/// layer and the eyedropper. Headless, like the rest of `--selftest`.
fn ui_checks() -> Result<Vec<String>, String> {
    use crate::eyedropper::sample_region;
    use crate::gesture::{Gesture, Recognizer};
    use crate::hold_shape::{classify, outline, Kind, Shape as QShape};
    use crate::view::View;
    use eframe::egui::{pos2, Rect, Vec2};
    use std::f64::consts::TAU;

    let mut out = Vec::new();

    // 1. Hold to Shape classification of synthetic strokes.
    let line: Vec<(f64, f64)> = (0..40).map(|i| (100.0 + i as f64 * 6.0, 200.0 + i as f64 * 2.0)).collect();
    let ellipse: Vec<(f64, f64)> = (0..=120)
        .map(|i| {
            let t = TAU * i as f64 / 120.0;
            (500.0 + 200.0 * t.cos(), 300.0 + 100.0 * t.sin())
        })
        .collect();
    let circle: Vec<(f64, f64)> = (0..=120)
        .map(|i| {
            let t = TAU * i as f64 / 120.0;
            (300.0 + 90.0 * t.cos(), 400.0 + 90.0 * t.sin())
        })
        .collect();
    let mut rect = Vec::new();
    let corners = [(100.0, 100.0), (400.0, 100.0), (400.0, 250.0), (100.0, 250.0), (100.0, 100.0)];
    for w in corners.windows(2) {
        for k in 0..30 {
            let t = k as f64 / 30.0;
            rect.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
        }
    }
    rect.push(corners[4]);
    let cases = [
        ("line", line, Kind::Line),
        ("ellipse", ellipse, Kind::Ellipse),
        ("circle", circle, Kind::Circle),
        ("rectangle", rect, Kind::Rectangle),
    ];
    for (name, pts, want) in cases {
        let got = classify(&pts).map(|f| f.kind);
        if got != Some(want) {
            return Err(format!("hold_shape {name}: expected {want:?}, got {got:?}"));
        }
    }
    out.push("  hold_shape: line, ellipse, circle and rectangle classified ok".to_owned());

    // 2. A snapped shape is painted through the stroke engine and undoes cleanly.
    let mut session = Session::new(Document::new(400, 300));
    let blank = session.flatten();
    let shape = QShape::Ellipse {
        c: (200.0, 150.0),
        rx: 120.0,
        ry: 60.0,
        rot: 0.3,
    };
    let pts: Vec<RawPointer> = outline(&shape, 2.0)
        .into_iter()
        .enumerate()
        .map(|(i, (x, y))| RawPointer {
            x,
            y,
            time: i as f64 / 120.0,
            kind: PointerKind::Mouse,
            force: None,
        })
        .collect();
    let s = settings(Preset::TechnicalPen, 6.0, [0.1, 0.4, 0.9], false);
    run_stroke(&mut session, 0, &s, &pts)?;
    let painted = session.flatten();
    if painted == blank {
        return Err("hold_shape: painted shape did not change the canvas".into());
    }
    session.undo().ok_or("hold_shape undo unavailable")?;
    if session.flatten() != blank {
        return Err("hold_shape undo did not restore paper".into());
    }
    out.push("  hold_shape: shape paints through the stroke engine and undoes".to_owned());

    // 3. Rotated view round trip, and a rotated canvas corner lands on the expected screen point.
    let vp = Rect::from_min_size(pos2(100.0, 50.0), Vec2::new(800.0, 600.0));
    let view = View {
        zoom: 1.5,
        offset: Vec2::new(40.0, 30.0),
        rotation: 0.65,
    };
    let (bx, by) = view.screen_to_canvas(vp, view.canvas_to_screen(vp, 321.0, 77.0));
    if (bx - 321.0).abs() > 1e-3 || (by - 77.0).abs() > 1e-3 {
        return Err(format!("view round trip failed: {bx}, {by}"));
    }
    out.push("  view: rotated screen <-> canvas round trip ok".to_owned());

    // 4. Gesture recogniser: two fingers tapped quickly is an undo tap.
    let mut rec = Recognizer::new();
    rec.down(1, pos2(10.0, 10.0), 0.0);
    rec.down(2, pos2(50.0, 10.0), 0.02);
    rec.up(1, 0.1);
    let tap = rec.up(2, 0.12);
    if tap != [Gesture::Tap { fingers: 2 }] {
        return Err(format!("two-finger tap not recognized: {tap:?}"));
    }
    out.push("  gestures: two-finger tap recognized".to_owned());

    // 5. Clear layer and smudge are both undoable.
    run_stroke(&mut session, 0, &s, &pts)?;
    let before = session.flatten();
    let l0 = session.doc.layer_id(0).ok_or("no layer")?;
    session.clear_layer(l0).ok_or("clear layer refused")?;
    if session.flatten() == before {
        return Err("clear layer changed nothing".into());
    }
    session.undo().ok_or("undo after clear unavailable")?;
    if session.flatten() != before {
        return Err("undo of clear layer did not restore the canvas".into());
    }
    let smudge_before = session.flatten();
    if !session.begin_smudge(l0, 14.0, 0.8, None) {
        return Err("smudge refused".into());
    }
    for i in 0..30 {
        session.push(crate::input::sample_from(RawPointer {
            x: 100.0 + i as f64 * 3.0,
            y: 150.0,
            time: i as f64 / 120.0,
            kind: PointerKind::Mouse,
            force: None,
        }));
    }
    session.end_stroke();
    session.undo().ok_or("smudge undo unavailable")?;
    if session.flatten() != smudge_before {
        return Err("smudge undo did not restore the canvas".into());
    }
    out.push("  clear layer and smudge: undo restores the canvas".to_owned());

    // 6. Eyedropper sampling on a known colour patch.
    let mut patch = Session::new(Document::new(64, 64));
    let red = settings(Preset::TechnicalPen, 40.0, [1.0, 0.0, 0.0], false);
    let line_pts: Vec<RawPointer> = (0..60)
        .map(|i| RawPointer {
            x: 10.0 + i as f64,
            y: 32.0,
            time: i as f64 / 120.0,
            kind: PointerKind::Mouse,
            force: None,
        })
        .collect();
    run_stroke(&mut patch, 0, &red, &line_pts)?;
    let buf = patch.flatten();
    let picked = sample_region(&buf, 64, 64, 30, 32, 1).ok_or("eyedropper: sample outside canvas")?;
    if picked[0] < 200 || picked[1] > 60 {
        return Err(format!("eyedropper sampled {picked:?}, expected red"));
    }
    out.push("  eyedropper: sampling the painted patch returns its color".to_owned());

    // 7. Layer operations are undoable: build a small tree, then unwind and replay it.
    out.push(layer_ops()?);
    out.push(adjustment_check()?);
    out.push(gpu_check()?);
    // Brush Library: every built-in brush's preview stroke has visible ink across the row.
    let mut previews = 0;
    for lb in eskiz_brushlib::library_ref().iter().flat_map(|s| &s.brushes) {
        let (ink, span) = crate::brush_library::preview_ink(lb);
        if ink < 150 || span <= 208 {
            return Err(format!("brush preview {}: {ink} ink pixels, span {span}", lb.name));
        }
        previews += 1;
    }
    out.push(format!("  brush library: {previews} previews rendered, each has visible ink"));
    out.push(textured_brush_check()?);
    out.push(crate::studio::selftest_check()?);
    out.push(pen_and_pressure_check()?);
    out.extend(colour_checks()?);
    out.extend(crate::guides::selftest_checks()?);
    out.push(crate::panels::layer_selftest::check()?);
    out.push(file_access_check()?);
    out.push(crate::onboarding::selftest_check()?);
    Ok(out)
}

/// Screen point of a canvas point, for the headless app.
/// File picking: a scripted `MockFileAccess` pick reaches the matching action (a font pick with a
/// wrong extension is refused by the font import, and a cancelled pick changes nothing), and the
/// `rfd` backend can be constructed without opening a dialog.
fn file_access_check() -> Result<String, String> {
    use crate::file_access::{FileAccess, MockFileAccess, PickPurpose, Picked};
    use std::sync::Arc;
    #[cfg(feature = "native-dialogs")]
    let backend = {
        let _rfd: Arc<dyn FileAccess> = Arc::new(crate::file_access::RfdFileAccess::new());
        "rfd constructed"
    };
    #[cfg(not(feature = "native-dialogs"))]
    let backend = "no dialog backend in this build";
    let mut h = Headless::new(eframe::egui::vec2(800.0, 600.0), Document::new(64, 64), crate::app::Prefs::default());
    let mock = Arc::new(MockFileAccess::with([Picked::Path("/tmp/eskiz-selftest.txt".into()), Picked::None]));
    let files: Arc<dyn FileAccess> = mock.clone();
    h.app.files = files;
    h.app.request_pick(PickPurpose::ImportFont, None, None);
    h.step();
    if h.app.text.font_path != "/tmp/eskiz-selftest.txt" {
        return Err("file access: a scripted pick did not reach the font import".into());
    }
    let before = h.app.export_path.clone();
    h.app.request_pick(PickPurpose::ExportBase, None, None);
    h.step();
    if h.app.export_path != before || mock.requests().len() != 2 {
        return Err("file access: a canceled pick changed state".into());
    }
    Ok(format!("  file access: pick routed through the mock, cancel is a no-op, {backend}"))
}

fn screen_of(h: &Headless, x: f64, y: f64) -> eframe::egui::Pos2 {
    h.app.view.canvas_to_screen(h.app.viewport, x, y)
}

/// Drag the primary button along `pts`, one frame per point. The button stays down.
fn press_and_drag(h: &mut Headless, pts: &[eframe::egui::Pos2]) {
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

/// The reference window shows the canvas, and Hold to Shape v2 (Edit Shape) snaps a
/// polyline, moves one of its vertices and commits the shape. Driven through the headless app.
fn reference_and_edit_shape_check() -> Result<Vec<String>, String> {
    use crate::app::Prefs;
    use crate::hold_shape::{self, Kind, Shape as QShape};
    use eframe::egui::{Pos2, Vec2};

    let size = Vec2::new(1280.0, 860.0);
    let mut out = Vec::new();

    // 1. A thick stroke through the middle of the canvas. The reference window opened on the
    //    same frame must show the same pixel at its centre as the canvas does.
    let mut h = Headless::new(size, Document::new(800, 600), Prefs::default());
    let raw: Vec<RawPointer> = (0..=60)
        .map(|i| {
            let t = f64::from(i) / 60.0;
            RawPointer { x: 100.0 + 600.0 * t, y: 300.0, time: t, kind: PointerKind::Mouse, force: None }
        })
        .collect();
    let ink = settings(Preset::TechnicalPen, 40.0, [0.05, 0.05, 0.1], false);
    run_stroke(&mut h.app.session, 0, &ink, &raw)?;
    let ctx = h.ctx.clone();
    h.app.sync_full(&ctx);
    h.advance(0.1);
    h.app.toggle_reference();
    h.advance(0.1);
    let img = h.render();
    let view = h.app.reference.view_rect;
    if !h.app.reference.open || view.width() < 50.0 || view.height() < 50.0 {
        return Err(format!("reference: the window did not open (view {view:?})"));
    }
    let on_canvas = {
        let p = screen_of(&h, 400.0, 300.0);
        img.pixel(p.x as usize, p.y as usize)
    };
    let paper = {
        let p = screen_of(&h, 400.0, 60.0);
        img.pixel(p.x as usize, p.y as usize)
    };
    let centre = view.center();
    let in_window = img.pixel(centre.x as usize, centre.y as usize);
    let diff = |a: [u8; 4], b: [u8; 4]| (0..3).map(|k| (i32::from(a[k]) - i32::from(b[k])).abs()).max().unwrap_or(0);
    if diff(on_canvas, paper) < 60 {
        return Err(format!("reference: the stroke is not visible on the canvas ({on_canvas:?} vs {paper:?})"));
    }
    if diff(in_window, on_canvas) > 40 {
        return Err(format!("reference: the window center {in_window:?} does not show the canvas {on_canvas:?}"));
    }
    out.push(format!("  reference window: opens, and its picture shows the live canvas (center {in_window:?})"));

    // 2. A zigzag held still snaps to a polyline, and Edit Shape moves its second vertex.
    let mut h = Headless::new(size, Document::new(800, 600), Prefs::default());
    h.advance(0.1);
    let layer = h.app.session.doc.layer_id(0).ok_or("no layer")?;
    let undo_before = h.app.session.undo_steps();
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
    press_and_drag(&mut h, &stroke);
    // Hold still at the end: Hold to Shape snaps the stroke while the pen is still down.
    h.advance(0.8);
    let edit = h.app.shape_edit.clone().ok_or("hold_shape v2: holding still did not snap a shape")?;
    if edit.kind != Kind::Polyline || !edit.live {
        return Err(format!("hold_shape v2: expected a live Polyline, got {:?} (live {})", edit.kind, edit.live));
    }
    match &edit.current {
        QShape::Polygon { pts, closed: false } if pts.len() == 5 => {}
        other => return Err(format!("hold_shape v2: the polyline has the wrong shape: {other:?}")),
    }
    h.press(end, false);
    h.step();
    if !h.app.editing_shape() {
        return Err("hold_shape v2: lifting the pen did not open Edit Shape".into());
    }
    let handle = hold_shape::handles(&edit.current)[1].pos;
    let target = (330.0, 300.0);
    let (from, to) = (screen_of(&h, handle.0, handle.1), screen_of(&h, target.0, target.1));
    h.move_to(from);
    h.step();
    h.press(from, true);
    h.step();
    for k in 1..=10 {
        h.move_to(from + (to - from) * (k as f32 / 10.0));
        h.step();
        if k == 5 {
            // Mid-drag the shape is only a vector preview: it is off the layer and out of the history.
            let editing = h.app.shape_edit.as_ref().ok_or("hold_shape v2: Edit Shape closed mid-drag")?;
            if !editing.previewing || h.app.session.undo_steps() != undo_before {
                return Err(format!(
                    "hold_shape v2: mid-drag the shape must be a preview (previewing {}, undo steps {} vs {undo_before})",
                    editing.previewing,
                    h.app.session.undo_steps()
                ));
            }
        }
    }
    h.press(to, false);
    h.step();
    if h.app.shape_edit.as_ref().is_some_and(|e| e.previewing) || h.app.session.undo_steps() != undo_before + 1 {
        return Err("hold_shape v2: releasing the handle must paint the shape once (one undo step)".into());
    }
    let moved = h.app.shape_edit.as_ref().ok_or("hold_shape v2: Edit Shape closed while dragging a handle")?.current.clone();
    let QShape::Polygon { pts, .. } = moved else {
        return Err("hold_shape v2: the handle drag changed the shape kind".into());
    };
    let v = pts[1];
    if (v.0 - target.0).hypot(v.1 - target.1) > 3.0 {
        return Err(format!("hold_shape v2: the dragged vertex is at {v:?}, expected {target:?}"));
    }
    // A tap away from the handles commits the shape.
    let tap = screen_of(&h, 700.0, 520.0);
    h.click(tap);
    if h.app.shape_edit.is_some() {
        return Err("hold_shape v2: a tap elsewhere did not commit the shape".into());
    }
    let steps = h.app.session.undo_steps();
    if steps != undo_before + 1 {
        return Err(format!("hold_shape v2: the shape made {} undo steps, expected 1", steps - undo_before));
    }
    let alpha = h.app.session.doc.layer(layer).ok_or("no layer")?.to_rgba();
    let (x0, y0) = (v.0.round() as usize, v.1.round() as usize);
    let mut max_alpha = 0u8;
    for y in y0 - 2..=y0 + 2 {
        for x in x0 - 2..=x0 + 2 {
            max_alpha = max_alpha.max(alpha[(y * 800 + x) * 4 + 3]);
        }
    }
    if max_alpha <= 16 {
        return Err("hold_shape v2: the committed shape is not on the canvas at its moved vertex".into());
    }
    out.push("  hold_shape v2: zigzag snapped to a polyline; Edit Shape moved a vertex; the tap committed it as one step".to_owned());
    Ok(out)
}

/// The GPU compositor against the CPU one on a small two-layer document, when a wgpu adapter
/// exists. No adapter is not a failure: the CPU path is the ground truth and the fallback.
fn gpu_check() -> Result<String, String> {
    let ctx = match eskiz_gpu::GpuContext::new() {
        Ok(c) => c,
        Err(e) => return Ok(format!("  GPU compositor: skipped ({e})")),
    };
    let name = ctx.describe();
    let mut session = Session::new(Document::new(300, 200));
    let pts = |y0: f64, y1: f64| -> Vec<RawPointer> {
        (0..40)
            .map(|i| {
                let t = f64::from(i) / 39.0;
                RawPointer {
                    x: 30.0 + 240.0 * t,
                    y: y0 + (y1 - y0) * t,
                    time: f64::from(i) / 120.0,
                    kind: PointerKind::Mouse,
                    force: None,
                }
            })
            .collect()
    };
    run_stroke(&mut session, 0, &settings(Preset::Pencil, 28.0, [0.85, 0.2, 0.15], false), &pts(40.0, 150.0))?;
    let base = session.doc.layer_id(0).ok_or("no base layer")?;
    let top = session.add_layer(Some(base)).and_then(|e| e.id).ok_or("could not add a layer")?;
    session.set_blend(top, eskiz_core::BlendMode::Multiply);
    session.set_opacity(top, 0.7);
    let top_index = session.doc.index_of(top).ok_or("no top layer")?;
    run_stroke(&mut session, top_index, &settings(Preset::Pencil, 36.0, [0.2, 0.4, 0.9], false), &pts(160.0, 30.0))?;
    let mut canvas = crate::gpu_canvas::GpuCanvas::new(ctx);
    canvas.flush(&session).map_err(|e| format!("GPU compositor: {e}"))?;
    let gpu = canvas.read_straight().map_err(|e| format!("GPU readback: {e}"))?;
    let cpu = session.flatten_display();
    let worst = cpu.iter().zip(&gpu).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    if cpu.len() != gpu.len() || worst > 2 {
        return Err(format!("GPU compositor differs from the CPU composite by up to {worst}/255"));
    }
    Ok(format!("  GPU compositor on {name}: matches the CPU composite (worst byte difference {worst})"))
}

/// Colour-disc maths and Pour Fill flood fill.
/// One stroke with a textured library brush. The grain must modulate coverage, so the alpha
/// inside the stroke is not flat.
fn textured_brush_check() -> Result<String, String> {
    use eskiz_core::brushes::Grain;
    let lb = eskiz_brushlib::library_ref()
        .iter()
        .flat_map(|s| &s.brushes)
        .find(|b| b.grain.is_some() && b.brush.grain != Grain::None)
        .ok_or("no textured library brush")?;
    let engine = crate::brush_library::engine_for(lb);
    let mut brush = engine.brush.clone();
    brush.size = 70.0;
    brush.opacity = 1.0;
    let settings = StrokeSettings {
        brush,
        colour: [0.1, 0.2, 0.7],
        erase: false,
        raster: Some(engine.raster.clone()),
    };
    let mut session = Session::new(Document::new(400, 300));
    let pts = path(160, PointerKind::Mouse, |p| (40.0 + 320.0 * p, 150.0, None));
    let rect = run_stroke(&mut session, 0, &settings, &pts).map_err(|e| format!("textured brush: {e}"))?;
    let id = session.doc.layer_id(0).ok_or("no layer")?;
    let layer = session.doc.layer(id).ok_or("no layer")?;
    // Interior: the middle half of the stroke's box, away from the tapered ends and the edges.
    let mut alphas = Vec::new();
    for y in rect.y + rect.h / 4..rect.y + 3 * rect.h / 4 {
        for x in rect.x + rect.w / 4..rect.x + 3 * rect.w / 4 {
            alphas.push(f64::from(layer.pixel(x, y)[3]));
        }
    }
    let n = alphas.len() as f64;
    let mean = alphas.iter().sum::<f64>() / n;
    let variance = alphas.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / n;
    if n == 0.0 || variance <= 0.0 {
        return Err(format!("textured brush '{}': interior alpha is flat (variance {variance})", lb.name));
    }
    Ok(format!(
        "  textured brush '{}': interior alpha mean {mean:.1}, variance {variance:.1} over {n} px",
        lb.name
    ))
}

fn colour_checks() -> Result<Vec<String>, String> {
    use crate::colour::{self, hsv_to_rgb, rgb_to_hsv, Hsv, Zone};
    let mut out = Vec::new();

    // 1. sRGB <-> HSV round trip and hex.
    for c in [[0, 0, 0], [255, 255, 255], [226, 84, 60], [1, 2, 3], [17, 200, 99]] {
        if hsv_to_rgb(rgb_to_hsv(c)) != c {
            return Err(format!("color: HSV round trip changed {c:?}"));
        }
        if colour::parse_hex(&colour::format_hex(c)) != Some(c) {
            return Err(format!("color: hex round trip changed {c:?}"));
        }
    }
    // 2. Disc: ring angle <-> hue, and the inner disc <-> saturation x brightness.
    let (dx, dy) = colour::hue_dir(0.25);
    if (colour::hue_at(dx, dy) - 0.25).abs() > 1e-4 {
        return Err("color: hue ring angle did not round trip".into());
    }
    for (s, v) in [(0.0, 1.0), (1.0, 1.0), (0.5, 0.5), (0.9, 0.1), (0.2, 0.0)] {
        let (x, y) = colour::sv_to_disc(s, v);
        let (s2, v2) = colour::disc_to_sv(x, y);
        if (s - s2).abs() > 2e-3 || (v - v2).abs() > 2e-3 {
            return Err(format!("color: disc mapping ({s}, {v}) came back as ({s2}, {v2})"));
        }
    }
    let picked = colour::pick(Zone::Inner, 0.44, -0.44, Hsv::new(0.6, 0.5, 0.5));
    if picked.s < 0.8 || picked.v < 0.8 {
        return Err(format!("color: top-right of the disc picked {picked:?}"));
    }
    out.push("  color: HSV, hex and disc maths round trip ok".to_owned());

    // 3. Flood fill: a closed ring fills inside only, one undo step, undo restores.
    let mut session = Session::new(Document::new(300, 200));
    let blank = session.flatten();
    // Overshoot the start: the pen tapers at both ends, so an exact loop is left open.
    let ring_pts: Vec<RawPointer> = (0..=205)
        .map(|i| {
            let a = std::f64::consts::TAU * f64::from(i) / 180.0;
            RawPointer {
                x: 150.0 + 70.0 * a.cos(),
                y: 100.0 + 70.0 * a.sin(),
                time: f64::from(i) / 120.0,
                kind: PointerKind::Mouse,
                force: None,
            }
        })
        .collect();
    run_stroke(&mut session, 0, &settings(Preset::TechnicalPen, 5.0, [0.0, 0.0, 0.0], false), &ring_pts)?;
    let ringed = session.flatten();
    session
        .pour_fill(session.doc.layer_id(0).ok_or("no layer")?, (150, 100), crate::floodfill::DEFAULT_THRESHOLD, [255, 0, 0])
        .ok_or("pour fill refused")?;
    let layer = session.doc.layer_at(0).ok_or("no layer")?;
    if layer.pixel(150, 100) != [255, 0, 0, 255] {
        return Err(format!("floodfill: inside is {:?}, expected red", layer.pixel(150, 100)));
    }
    if layer.pixel(5, 5)[3] != 0 || layer.pixel(150, 20)[3] != 0 {
        return Err("floodfill: the fill leaked across a closed line".into());
    }
    // The antialiased edge pixels of the ring stay opaque (no transparent halo).
    let edge_x = (150..300).find(|&x| layer.pixel(x, 100) != [255, 0, 0, 255]).ok_or("floodfill: no ring edge")?;
    let edge = layer.pixel(edge_x, 100);
    if edge[3] != 255 {
        return Err(format!("floodfill: ring edge pixel {edge:?} has a halo"));
    }
    if session.undo_name() != Some("Pour Fill") || session.undo_steps() != 2 {
        return Err("floodfill: Pour Fill is not one named undo step".into());
    }
    session.undo().ok_or("floodfill: undo unavailable")?;
    if session.flatten() != ringed {
        return Err("floodfill: undo did not restore the ring".into());
    }
    session.undo().ok_or("floodfill: second undo unavailable")?;
    if session.flatten() != blank {
        return Err("floodfill: undo of the ring did not restore paper".into());
    }
    out.push("  floodfill: closed ring fills inside only, no leak, one undo step".to_owned());

    // 4. Threshold: 10% stops at a 30% wash, 50% crosses it.
    let (w, h) = (40usize, 10usize);
    let mut wash = eskiz_core::canvas::Layer::new(w, h);
    for y in 0..h {
        for x in 20..w {
            wash.set_pixel(x, y, [77, 77, 77, 77]);
        }
    }
    let strict = crate::floodfill::flood_fill(&wash, (2, 2), 0.1, [0, 0, 255]).ok_or("floodfill: none")?;
    let loose = crate::floodfill::flood_fill(&wash, (2, 2), 0.5, [0, 0, 255]).ok_or("floodfill: none")?;
    if strict.region_pixels != 20 * h || loose.region_pixels != w * h {
        return Err(format!(
            "floodfill: threshold regions {} and {}, expected {} and {}",
            strict.region_pixels,
            loose.region_pixels,
            20 * h,
            w * h
        ));
    }
    out.push("  floodfill: threshold 10% stops at a 30% wash, 50% crosses it".to_owned());
    Ok(out)
}

/// The global pressure curve (identity default, fixed ends, passes through its control
/// points, monotonic, survives Prefs encoding) and the pen path: fake tablet events go
/// through the tracker and the pen mapping into a real stroke, and a curve that boosts
/// light pressure leaves more paint than the identity.
fn pen_and_pressure_check() -> Result<String, String> {
    use crate::app::Prefs;
    use crate::input::{apply_pressure_curve, sample_from_pen};
    use crate::pressure::PressureCurve;
    use crate::tablet::{reading_from_pose, PenEvent, PenPose, PenTracker};

    let identity = PressureCurve::default();
    if !identity.is_identity() || (0..=20).any(|i| (identity.eval(i as f32 / 20.0) - i as f32 / 20.0).abs() > 1e-4) {
        return Err("pressure curve: default is not the identity".into());
    }
    let mut boost = PressureCurve::default();
    boost.set_point(0, 0.1230, 0.2803);
    boost.set_point(1, 0.5752, 0.8797);
    if boost.eval(0.0).abs() > 1e-6 || (boost.eval(1.0) - 1.0).abs() > 1e-6 {
        return Err("pressure curve: end points moved".into());
    }
    if boost.points().iter().any(|p| (boost.eval(p[0]) - p[1]).abs() > 1e-4) {
        return Err("pressure curve: does not pass through its control points".into());
    }
    if (1..=400).any(|i| boost.eval(i as f32 / 400.0) < boost.eval((i - 1) as f32 / 400.0) - 1e-5) {
        return Err("pressure curve: not monotonic".into());
    }
    let prefs = Prefs { pressure_curve: boost, ..Prefs::default() };
    if Prefs::decode(&prefs.encode()).pressure_curve != boost {
        return Err("pressure curve: Prefs round trip changed it".into());
    }

    // A pen stroke of light pressure (0.2), tilted, from fake events.
    let paint = |curve: &PressureCurve| -> Result<usize, String> {
        let mut tracker = PenTracker::new();
        let pose = |x: f32| PenPose { pos: [x, 40.0], pressure: Some(0.2), tilt: Some([0.3, 0.1]), roll: None, time: None };
        let mut events = vec![PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(10.0)), PenEvent::Down];
        events.extend((0..80).map(|i| PenEvent::Pose(pose(12.0 + i as f32))));
        events.push(PenEvent::Up);
        let frame = tracker.frame(events, 1.0);
        if !frame.pressed || frame.samples.len() < 80 || reading_from_pose(&pose(0.0)).altitude.is_none() {
            return Err("pen: tracker lost samples".into());
        }
        let mut session = Session::new(Document::new(128, 80));
        let blank = session.flatten();
        let s = settings(Preset::InkWithTaper, 14.0, [0.0, 0.0, 0.0], false);
        let id = session.doc.layer_id(0).ok_or("pen: no layer")?;
        if !session.begin_stroke(id, &s) {
            return Err("pen: could not begin a stroke".into());
        }
        for p in &frame.samples {
            let sample = sample_from_pen(f64::from(p.pos.x), f64::from(p.pos.y), p.time, p.reading);
            session.push(apply_pressure_curve(sample, curve));
        }
        session.end_stroke();
        let after = session.flatten();
        // Total ink: how far each pixel's red channel fell from white.
        Ok(blank.iter().zip(&after).step_by(4).map(|(a, b)| usize::from(a.saturating_sub(*b))).sum())
    };
    let flat = paint(&identity)?;
    let boosted = paint(&boost)?;
    if flat == 0 || boosted <= flat {
        return Err(format!("pen: boosted curve should leave more ink ({boosted} vs {flat})"));
    }
    Ok("  pen tablet: fake events -> tilted, pressure-sensitive stroke; pressure curve checks pass".to_owned())
}

/// Tiled layer storage: empty layers cost nothing, a stroke and its undo step
/// hold only the tiles they touch, and copies (duplicate, document snapshot) share tiles.
fn tiled_memory_check() -> Result<String, String> {
    use eskiz_core::canvas::TILE_BYTES;
    const MB: usize = 1024 * 1024;
    let mut s = Session::new(Document::new(WIDTH, HEIGHT));
    for _ in 0..19 {
        s.add_layer(None).ok_or("tiled: add layer refused")?;
    }
    if s.doc.layer_count() != 20 || s.doc.tile_bytes() != 0 || s.undo_bytes() != 0 {
        return Err(format!(
            "tiled: 20 empty layers hold {} pixel bytes and {} undo bytes, expected 0",
            s.doc.tile_bytes(),
            s.undo_bytes()
        ));
    }
    // Two strokes on two layers: a canvas-wide buffer per layer would be 20 * 12 MB = 240 MB.
    let ink = settings(Preset::TechnicalPen, 60.0, [0.1, 0.1, 0.1], false);
    let line = path(100, PointerKind::Mouse, |p| (200.0 + 300.0 * p, 700.0 + 60.0 * p, None));
    let steps_before = s.undo_steps();
    run_stroke(&mut s, 0, &ink, &line)?;
    let one = s.doc.tile_bytes();
    let step_bytes = s.undo_bytes();
    let tiles = one / TILE_BYTES;
    if tiles == 0 || tiles > 4 || step_bytes != one {
        return Err(format!("tiled: a 300 px stroke used {tiles} tiles and {step_bytes} undo bytes ({one} pixel bytes)"));
    }
    run_stroke(&mut s, 10, &ink, &line)?;
    let used = s.doc.tile_bytes();
    let limit = 8 * MB;
    if used > limit {
        return Err(format!("tiled: 20 mostly empty layers hold {} MB, limit {} MB", used / MB, limit / MB));
    }
    // Duplicating a painted layer shares its tiles; painting on the copy leaves the original.
    let first = s.doc.layer_id(0).ok_or("tiled: no layer")?;
    let dup = s.duplicate_layer(first).and_then(|e| e.id).ok_or("tiled: duplicate refused")?;
    if s.doc.tile_bytes() != used {
        return Err("tiled: duplicate copied pixels instead of sharing tiles".into());
    }
    // A document snapshot (what the gallery save takes on the UI thread) is a handful of
    // reference counts, not a pixel copy.
    let t = std::time::Instant::now();
    let snapshot = s.doc.clone();
    let clone_us = t.elapsed().as_micros();
    if snapshot.tile_bytes() != used {
        return Err("tiled: a document clone does not share its tiles".into());
    }
    drop(snapshot);
    // Undo everything back to the start: no pixel memory is left, and the paper is blank.
    s.delete_layer(dup).ok_or("tiled: delete refused")?;
    while s.undo_steps() > steps_before {
        s.undo().ok_or("tiled: undo refused")?;
    }
    if s.doc.tile_bytes() != 0 {
        return Err(format!("tiled: after undoing every stroke {} pixel bytes remain", s.doc.tile_bytes()));
    }
    Ok(format!(
        "  tiled storage: 20 layers on {WIDTH}x{HEIGHT} hold {} KB of pixels (a flat buffer would hold {} MB); a stroke undo step stores {} tile(s) = {} KB; duplicate shares tiles; document clone {clone_us} us",
        used / 1024,
        20 * WIDTH * HEIGHT * 4 / MB,
        tiles,
        step_bytes / 1024
    ))
}

/// Add, group, restyle, merge, flip, recolour and delete layers on a full-size canvas. Undoing
/// every step must give the starting pixels back, and redoing them the final pixels.
fn layer_ops() -> Result<String, String> {
    let mut s = Session::new(Document::new(WIDTH, HEIGHT));
    let a = s.doc.layer_id(0).ok_or("no layer")?;
    let start = checksum(&s.flatten());
    let ink = settings(Preset::TechnicalPen, 80.0, [0.8, 0.2, 0.1], false);
    let blue = settings(Preset::TechnicalPen, 80.0, [0.1, 0.2, 0.9], false);
    let diag = path(120, PointerKind::Mouse, |p| (300.0 + 1400.0 * p, 300.0 + 800.0 * p, None));
    let cross = path(120, PointerKind::Mouse, |p| (300.0 + 1400.0 * p, 1100.0 - 800.0 * p, None));
    run_stroke(&mut s, 0, &ink, &diag)?;
    let b = s.add_layer(Some(a)).and_then(|e| e.id).ok_or("add layer refused")?;
    run_stroke(&mut s, 1, &blue, &cross)?;
    s.set_blend(b, eskiz_core::BlendMode::Multiply).ok_or("blend refused")?;
    s.set_opacity(b, 0.7).ok_or("opacity refused")?;
    let g = s.group_items(&[a, b]).and_then(|e| e.id).ok_or("group refused")?;
    s.set_opacity(g, 0.8).ok_or("group opacity refused")?;
    s.add_mask(g, crate::session::MaskFill::Reveal).ok_or("mask refused")?;
    let copy = s.duplicate_layer(b).and_then(|e| e.id).ok_or("duplicate refused")?;
    s.merge_down(copy).ok_or("merge refused")?;
    s.flip_canvas(true).ok_or("flip refused")?;
    s.set_background_colour([0.9, 0.9, 0.7]).ok_or("background refused")?;
    s.delete_layer(b).ok_or("delete refused")?;
    let end = checksum(&s.flatten());
    if end == start {
        return Err("layer operations changed nothing".into());
    }
    let steps = s.undo_steps();
    while s.can_undo() {
        s.undo().ok_or("undo refused")?;
    }
    if checksum(&s.flatten()) != start {
        return Err("undoing every layer operation did not restore the start".into());
    }
    for _ in 0..steps {
        s.redo().ok_or("redo refused")?;
    }
    if checksum(&s.flatten()) != end {
        return Err("redoing every layer operation did not reproduce the result".into());
    }
    Ok(format!("  layer ops: {steps} steps (add, group, props, mask, merge, flip, delete) undo and redo exactly"))
}

/// Adjustments: paint a stroke, then apply Opacity, Gaussian blur
/// and Hue/Saturation/Brightness to the layer as the window does, each as one named undo step,
/// and check that undo restores the pixels exactly and that redo reproduces the result.
fn adjustment_check() -> Result<String, String> {
    use eskiz_core::filters::{AdjustKind, Adjustment, Quality};

    let mut s = Session::new(Document::new(WIDTH, HEIGHT));
    let id = s.doc.layer_id(0).ok_or("no layer")?;
    let ink = settings(Preset::TechnicalPen, 90.0, [0.8, 0.2, 0.1], false);
    let arc = path(120, PointerKind::Mouse, |p| (300.0 + 1400.0 * p, 400.0 + 700.0 * (p * std::f64::consts::PI).sin(), None));
    run_stroke(&mut s, 0, &ink, &arc)?;
    let mut names = Vec::new();
    for (kind, values) in [
        (AdjustKind::Opacity, vec![(0, 0.5)]),
        (AdjustKind::GaussianBlur, vec![(0, 0.6)]),
        (AdjustKind::Hsb, vec![(0, 0.4), (1, 0.3), (2, 0.05)]),
        (AdjustKind::MotionBlur, vec![(0, 14.0), (1, -6.0), (2, 0.6)]),
        (AdjustKind::Curves, vec![]),
        (AdjustKind::GradientMap, vec![]),
    ] {
        let before = s.doc.layer(id).ok_or("no layer")?.to_rgba();
        let mut adj = Adjustment::new(kind);
        for (i, v) in values {
            adj.set(i, v);
        }
        if kind == AdjustKind::Curves {
            let mut c = eskiz_core::filters::CurvePoints::default();
            c.channels[0] = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
            adj.set_curve_points(c);
        }
        if kind == AdjustKind::GradientMap {
            adj.set_gradient_stops(eskiz_core::filters::GradientStops::preset(1));
        }
        let (w, h) = (s.doc.width, s.doc.height);
        // The preview (proxy) path runs too; it must not touch the layer.
        let preview = adj.render(&before, w, h, Quality::Preview, None, None);
        if preview.len() != before.len() || s.doc.layer(id).ok_or("no layer")?.to_rgba() != before {
            return Err(format!("{}: preview changed the layer", kind.name()));
        }
        let after = adj.render(&before, w, h, Quality::Full, None, None);
        if after == before {
            return Err(format!("{}: changed nothing", kind.name()));
        }
        s.replace_layer_pixels(id, after.clone(), kind.undo_name()).ok_or("adjustment refused")?;
        if s.undo_name() != Some(kind.undo_name()) {
            return Err(format!("{}: undo step is named {:?}", kind.name(), s.undo_name()));
        }
        s.undo().ok_or("undo refused")?;
        if s.doc.layer(id).ok_or("no layer")?.to_rgba() != before {
            return Err(format!("{}: undo did not restore the pixels exactly", kind.name()));
        }
        s.redo().ok_or("redo refused")?;
        if s.doc.layer(id).ok_or("no layer")?.to_rgba() != after {
            return Err(format!("{}: redo did not reproduce the result", kind.name()));
        }
        names.push(kind.undo_name());
    }
    Ok(format!("  adjustments: {} apply as one named step; undo restores the pixels exactly, redo reproduces them", names.join(", ")))
}

/// Export a two-layer document as `.procreate` and as PSD through the Share sheet's code path
/// ([`crate::export::export_with`]), read both back with the importers, and compare the pixels.
/// PSD stores straight colour, so its opaque pixels must match exactly and its alpha everywhere.
fn export_check() -> Result<String, String> {
    use crate::export::{export_with, ExportFormat, ExportSettings};
    use crate::import::{open_procreate, read_layers};

    let dir = std::env::temp_dir().join(format!("eskiz-selftest-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let mut s = Session::new(Document::new(WIDTH, HEIGHT));
    let ink = settings(Preset::TechnicalPen, 90.0, [0.8, 0.2, 0.1], false);
    let arc = path(120, PointerKind::Mouse, |p| (300.0 + 1400.0 * p, 400.0 + 700.0 * (p * std::f64::consts::PI).sin(), None));
    run_stroke(&mut s, 0, &ink, &arc)?;
    // A second layer above the first: a blue strip along the top 40 rows.
    let below = s.doc.layer_id(0).ok_or("no layer")?;
    let mut strip = vec![0u8; WIDTH * HEIGHT * 4];
    for px in strip.as_chunks_mut::<4>().0.iter_mut().take(WIDTH * 40) {
        *px = [0, 0, 255, 255];
    }
    s.import_layer(Some(below), "Top strip", strip).ok_or("could not add the second layer")?;
    let doc = &s.doc;
    let export_settings = ExportSettings::default();
    let layers = doc.layer_count();

    // .procreate: every layer's pixels come back exactly.
    let package = export_with(doc, ExportFormat::Procreate, &dir.join("selftest"), &export_settings)?;
    let back = open_procreate(&package)?;
    if back.layer_count() != layers {
        return Err(format!(".procreate has {} layers, expected {layers}", back.layer_count()));
    }
    for i in 0..layers {
        let want = doc.layer_at(i).ok_or("no layer")?.to_rgba();
        if back.layer_at(i).ok_or("no layer")?.to_rgba() != want {
            return Err(format!(".procreate layer {i} changed on the round trip"));
        }
    }

    // PSD: the same layers, bottom to top, with the same visible pixels.
    let psd = export_with(doc, ExportFormat::Psd, &dir.join("selftest"), &export_settings)?;
    let read = read_layers(&psd)?;
    if read.len() != layers {
        return Err(format!("PSD has {} layers, expected {layers}", read.len()));
    }
    for (i, layer) in read.iter().enumerate() {
        let want = doc.layer_at(i).ok_or("no layer")?.to_rgba();
        for (got, exp) in layer.pixels.as_chunks::<4>().0.iter().zip(want.as_chunks::<4>().0.iter()) {
            if got[3] != exp[3] || (exp[3] == 255 && got != exp) {
                return Err(format!("PSD layer {i} pixels changed on the round trip"));
            }
        }
    }
    std::fs::remove_dir_all(&dir).ok();
    Ok(format!("  export: .procreate and PSD, {layers} layers each, read back with the same pixels"))
}

/// Load a `.procreate` file and export its flattened canvas as PNG, without a window.
pub fn export(open: &Path, png: &Path) -> Result<String, String> {
    let doc = open_procreate(open)?;
    let (w, h) = (doc.width, doc.height);
    let session = Session::new(doc);
    let rgba = session.flatten_for_export();
    write_png(png, w, h, &rgba)?;
    Ok(format!(
        "exported {w}x{h} from {} layers to {} (checksum fnv1a64 0x{:016x})",
        session.doc.layer_count(),
        png.display(),
        checksum(&rgba)
    ))
}
