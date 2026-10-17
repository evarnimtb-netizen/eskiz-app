//! Scenario 1: painting. One long stylus stroke through `Session` (pen-down, one `push` per
//! input sample, pen-up), exactly as the window drives it, with the app's library brushes.

use std::time::Instant;

use eskiz_app::gpu_canvas::GpuCanvas;
use eskiz_app::history::PixelRect;
use eskiz_app::session::Session;
use eskiz_core::geometry::Sample;
use eskiz_core::{Stroke, StrokeOptions};

use crate::ctx::Ctx;
use crate::fixtures::{self, BenchBrush, Layout};
use crate::report::{fmt_ms, mib, ms_since, reset_peak, rss, Timing};

/// Input samples per stroke.
pub const POINTS: usize = 2000;

/// One stroke's measurements.
struct StrokeRun {
    begin_ms: f64,
    /// Every `push`, in ms.
    events: Vec<f64>,
    /// The pushes that produced at least one dab (and so painted and returned a rectangle).
    dab_events: Vec<f64>,
    end_ms: f64,
    total_ms: f64,
    rect: Option<PixelRect>,
    /// Peak RSS of the process over the stroke, bytes.
    peak_rss: u64,
}

fn run_stroke(session: &mut Session, id: u64, brush: &BenchBrush, path: &[Sample], smudge: bool) -> StrokeRun {
    reset_peak();
    let t_all = Instant::now();
    let t = Instant::now();
    let started = if smudge {
        session.begin_smudge(id, brush.settings.brush.size / 2.0, 0.8, Some(brush.tip.clone()))
    } else {
        session.begin_stroke(id, &brush.settings)
    };
    assert!(started, "stroke refused");
    let begin_ms = ms_since(t);
    let mut events = Vec::with_capacity(path.len());
    let mut dab_events = Vec::new();
    let mut rect: Option<PixelRect> = None;
    for s in path {
        let t = Instant::now();
        let r = session.push(*s);
        let ms = ms_since(t);
        events.push(ms);
        if let Some(r) = r {
            dab_events.push(ms);
            rect = Some(rect.map_or(r, |a| a.union(r)));
        }
    }
    let t = Instant::now();
    let r = session.end_stroke();
    let end_ms = ms_since(t);
    if let Some(r) = r {
        rect = Some(rect.map_or(r, |a| a.union(r)));
    }
    StrokeRun { begin_ms, events, dab_events, end_ms, total_ms: ms_since(t_all), rect, peak_rss: rss().1 }
}

/// Dabs the brush engine generates for `path` (a replay of the geometry alone).
fn count_dabs(brush: &BenchBrush, path: &[Sample]) -> usize {
    let mut s = Stroke::new(brush.settings.brush.clone(), StrokeOptions { seed: 1, ..Default::default() });
    for p in path {
        s.add_sample(*p);
    }
    s.finish().dabs.len()
}

pub fn run(cx: &mut Ctx) {
    let brushes = fixtures::brushes();
    let t = cx.report.table(
        "1. Painting: one 2000-point stylus stroke (Session::begin_stroke / push / end_stroke)",
        &["brush", "canvas", "layers", "dabs", "begin ms", "event p50 ms", "event p95 ms", "dab-event p50 ms", "dab-event p95 ms", "event max ms", "pen-up ms", "stroke total ms (median)", "dabs/s", "stroke rect", "peak RSS", "notes"],
    );
    cx.report.intro(t, "Per-event cost is one `Session::push` (dab generation, coverage splat and the recomposite of the dab rectangle from the pen-down snapshot), without any display work. Target: under 8 ms. Wide-spaced brushes lay a dab only every few input events, so `dab-event` columns are the statistics over the events that painted something (the others cost microseconds). `begin` is the pen-down cost (snapshot clone, stroke-wide coverage buffer and, for wet brushes, the color buffer). `pen-up` replays the end taper and recomposites the whole stroke rectangle. Peak RSS is the process peak during the stroke.");
    for &n in &cx.sizes.clone() {
        let path = fixtures::stroke_path(n, POINTS);
        for (bi, brush) in brushes.iter().enumerate() {
            let wet = brush.settings.brush.wet.enabled;
            // The layer, its pen-down snapshot (shared tiles), a canvas-wide f32 coverage buffer and,
            // for wet brushes, the canvas-wide colour delta buffer.
            let need = fixtures::layer_bytes(n) * 2 + (n * n) as u64 * if wet { 16 } else { 4 };
            if let Err(e) = cx.fits(need) {
                let mut row = vec![brush.label.to_owned(), cx.size_label(n), "1".into()];
                row.extend(std::iter::repeat_n("-".to_owned(), 12));
                row.push(e);
                cx.report.row(t, &row);
                continue;
            }
            cx.log(&format!("paint {n}² {}", brush.label));
            let mut session = fixtures::session(n, Layout::OneFull);
            let id = fixtures::first_layer(&session.doc);
            let dabs = count_dabs(brush, &path);
            let runs: Vec<StrokeRun> = (0..=cx.iters.normal.min(if n >= 8192 || cx.quick { 2 } else { 3 }))
                .map(|_| {
                    let r = run_stroke(&mut session, id, brush, &path, false);
                    // Keep the document from filling up with undo tiles between runs.
                    let _ = session.undo();
                    r
                })
                .collect();
            push_rows(cx, t, RowSpec { label: brush.label, n, layers: 1, dabs, note: "" }, &runs[1..]);
            if bi == 0 {
                // The smudge tool, with the ink tip, on the same path.
                let runs: Vec<StrokeRun> = (0..=2)
                    .map(|_| {
                        let r = run_stroke(&mut session, id, brush, &path, true);
                        let _ = session.undo();
                        r
                    })
                    .collect();
                push_rows(cx, t, RowSpec { label: "smudge 30 px (ink tip)", n, layers: 1, dabs: 0, note: "dabs not counted (smudge has its own spacing)" }, &runs[1..]);
            }
        }
    }
    per_event_with_display(cx, &brushes);
}

/// One table row of a stroke measurement.
struct RowSpec<'a> {
    label: &'a str,
    n: usize,
    layers: usize,
    dabs: usize,
    note: &'a str,
}

fn push_rows(cx: &mut Ctx, t: usize, spec: RowSpec<'_>, runs: &[StrokeRun]) {
    let RowSpec { label, n, layers, dabs, note } = spec;
    let events: Vec<f64> = runs.iter().flat_map(|r| r.events.iter().copied()).collect();
    let ev = Timing::from_ms(events);
    let dab_ev = Timing::from_ms(runs.iter().flat_map(|r| r.dab_events.iter().copied()).collect());
    let peak = runs.iter().map(|r| r.peak_rss).max().unwrap_or(0);
    let begin = Timing::from_ms(runs.iter().map(|r| r.begin_ms).collect());
    let end = Timing::from_ms(runs.iter().map(|r| r.end_ms).collect());
    let total = Timing::from_ms(runs.iter().map(|r| r.total_ms).collect());
    let dabs_per_s = if dabs > 0 { format!("{:.0}", dabs as f64 / (total.median() / 1000.0)) } else { "-".into() };
    let rect = runs[0].rect.map_or("-".to_owned(), |r| format!("{}x{}", r.w, r.h));
    let mut notes = note.to_owned();
    if dab_ev.p95() > 8.0 {
        if !notes.is_empty() {
            notes.push_str("; ");
        }
        notes.push_str("**dab-event p95 over the 8 ms target**");
    }
    cx.report.row(
        t,
        &[
            label.into(),
            cx.size_label(n),
            layers.to_string(),
            if dabs > 0 { dabs.to_string() } else { "-".into() },
            fmt_ms(begin.median()),
            fmt_ms(ev.median()),
            fmt_ms(ev.p95()),
            fmt_ms(dab_ev.median()),
            fmt_ms(dab_ev.p95()),
            fmt_ms(ev.max()),
            fmt_ms(end.median()),
            fmt_ms(total.median()),
            dabs_per_s,
            rect,
            mib(peak),
            notes,
        ],
    );
}

/// The per-event cost once the display refresh is included: the CPU path recomposites the dab
/// rectangle (`flatten_rect_display`), converts it to a `ColorImage` and uploads it as a partial
/// egui texture; the GPU path uploads the rectangle of the stroke layer and recomposites its
/// tiles on the GPU (`GpuCanvas::mark_rect` + `flush`), then waits for the GPU.
fn per_event_with_display(cx: &mut Ctx, brushes: &[BenchBrush]) {
    let t = cx.report.table(
        "1b. Painting: per-event cost with the display refresh (what the frame actually costs)",
        &["brush", "canvas", "layers", "display path", "dab events", "push p50 ms", "push + refresh p50 ms", "push + refresh p95 ms", "refresh share", "notes"],
    );
    cx.report.intro(t, "Only the events that laid a dab are counted (the others do no display work). The window refreshes the dirty rectangle once per frame, not per event; at 120 Hz input and 60 Hz display two events share one refresh, so a real frame costs about two `push` plus one refresh. GPU figures are on a software adapter (llvmpipe) here and only show the CPU-side cost of the GPU path (uploads, submission) plus a slow software composite; see the machine notes.");
    let points = if cx.quick { 600 } else { 1000 };
    for &n in &cx.sizes.clone() {
        if cx.reduced(n) || cx.quick_skip(n) {
            continue;
        }
        let path = fixtures::stroke_path(n, points);
        for layout in [Layout::OneFull, Layout::SparseN(20)] {
            if cx.fits(layout.bytes(n) + fixtures::layer_bytes(n) * 2).is_err() {
                continue;
            }
            for brush in brushes.iter().take(2) {
                cx.log(&format!("paint+display {n}² {} {}", layout.label(), brush.label));
                // CPU path.
                let mut session = fixtures::session(n, layout);
                let id = fixtures::top_layer(&session.doc);
                let (mut push, mut both) = (Vec::new(), Vec::new());
                let mut tex = None;
                if let (Some(gpu), Some(disp)) = (cx.gpu.as_ref(), cx.display.as_mut()) {
                    let full = session.flatten_display();
                    let (tid, _, _) = disp.upload_full(gpu, n, n, &full);
                    tex = Some(tid);
                }
                assert!(session.begin_stroke(id, &brush.settings));
                for s in &path {
                    let t0 = Instant::now();
                    let r = session.push(*s);
                    let p = ms_since(t0);
                    if let Some(r) = r {
                        let rgba = session.flatten_rect_display(r);
                        match (cx.gpu.as_ref(), cx.display.as_mut(), tex) {
                            (Some(gpu), Some(disp), Some(tid)) => {
                                disp.upload_partial(gpu, tid, [r.x, r.y], r.w, r.h, &rgba);
                            }
                            _ => {
                                let _ = eframe::egui::ColorImage::from_rgba_unmultiplied([r.w, r.h], &rgba);
                            }
                        }
                    }
                    if r.is_some() {
                        push.push(p);
                        both.push(ms_since(t0));
                    }
                }
                session.end_stroke();
                let (tp, tb) = (Timing::from_ms(push), Timing::from_ms(both));
                let share = format!("{:.0}%", 100.0 * (1.0 - tp.median() / tb.median().max(1e-9)));
                let note = if tex.is_some() { "composite_rect + ColorImage + egui partial upload" } else { "composite_rect + ColorImage (no GPU for the upload)" };
                cx.report.row(t, &[brush.label.into(), cx.size_label(n), layout.layers().to_string(), "CPU".into(), tp.n().to_string(), fmt_ms(tp.median()), fmt_ms(tb.median()), fmt_ms(tb.p95()), share, note.into()]);

                // GPU path (the mirror duplicates every tile in GPU memory, which is host RAM here).
                if cx.fits(layout.bytes(n) * 3 + fixtures::layer_bytes(n) * 4).is_err() {
                    continue;
                }
                let Some(handle) = cx.gpu_handle() else { continue };
                let mut canvas = GpuCanvas::new(handle);
                if !canvas.fits(n, n) {
                    cx.report.row(t, &[brush.label.into(), cx.size_label(n), layout.layers().to_string(), "GPU".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "canvas larger than the adapter's texture limit".into()]);
                    continue;
                }
                let mut session = fixtures::session(n, layout);
                canvas.mark_all();
                if let Err(e) = canvas.flush(&session) {
                    cx.report.row(t, &[brush.label.into(), cx.size_label(n), layout.layers().to_string(), "GPU".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), format!("GPU flush failed: {e}")]);
                    continue;
                }
                canvas.wait_idle();
                let (mut push, mut both) = (Vec::new(), Vec::new());
                assert!(session.begin_stroke(id, &brush.settings));
                for s in &path {
                    let t0 = Instant::now();
                    let r = session.push(*s);
                    let p = ms_since(t0);
                    if let Some(r) = r {
                        canvas.mark_rect(r, id);
                        let _ = canvas.flush(&session);
                        canvas.wait_idle();
                        push.push(p);
                        both.push(ms_since(t0));
                    }
                }
                session.end_stroke();
                let (tp, tb) = (Timing::from_ms(push), Timing::from_ms(both));
                let share = format!("{:.0}%", 100.0 * (1.0 - tp.median() / tb.median().max(1e-9)));
                cx.report.row(t, &[brush.label.into(), cx.size_label(n), layout.layers().to_string(), "GPU (software adapter)".into(), tp.n().to_string(), fmt_ms(tp.median()), fmt_ms(tb.median()), fmt_ms(tb.p95()), share, "rect upload + tile recomposite + wait".into()]);
                drop(canvas);
                cx.gpu_cleanup();
            }
        }
    }
}
