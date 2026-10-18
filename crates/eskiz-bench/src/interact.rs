//! Scenario 9: the two drag interactions that used to lag on a big canvas.
//!
//! * Pour Fill: dragging sideways after the drop changes the threshold 1% per 2 px, and each
//!   step re-ran the flood fill on the UI thread. Before = `Session::pour_fill_preview` (plan,
//!   fill and write in one call). After = what the UI thread does now: `pour_fill_plan` (a few
//!   `Arc` clones), and, when the worker thread is done, `pour_fill_apply` (the write).
//!   The fill itself (`PourPlan::compute`) now runs on a worker thread.
//! * Edit Shape: dragging a handle of a Hold-to-Shape shape re-rasterized the shape every frame
//!   (undo the stroke, paint a new one). Now the drag draws a vector preview (`hold_shape::outline`,
//!   a few thousand points) and the shape is rasterized once on release.

use eskiz_app::hold_shape::{self, Shape};
use eskiz_app::input::{sample_from, PointerKind, RawPointer};
use eskiz_app::session::Session;

use crate::ctx::Ctx;
use crate::fixtures::{self, Layout};
use crate::report::{bench_with, cells};

fn circle_stroke(s: &mut Session, id: u64, settings: &eskiz_app::paint::StrokeSettings, shape: &Shape) {
    assert!(s.begin_stroke(id, settings));
    let mut outline = hold_shape::outline(shape, 2.0);
    let extra = (outline.len() / 6).max(4).min(outline.len().saturating_sub(1));
    let more = outline[1..=extra].to_vec();
    outline.extend(more);
    for (i, (x, y)) in outline.into_iter().enumerate() {
        let raw = RawPointer { x, y, time: i as f64 / 120.0, kind: PointerKind::Mouse, force: None };
        let _ = s.push(sample_from(raw));
    }
    let _ = s.end_stroke();
}

pub fn run(cx: &mut Ctx) {
    let t = cx.report.table("9. Drag interactions: Pour Fill threshold drag and Edit Shape drag", &["canvas", "interaction", "n", "median ms", "p95 ms", "min ms", "notes"]);
    cx.report.intro(t, "One row is the cost of one step of the drag on the thread that draws the window (the display refresh that follows is not included: it is the same before and after). `before` is the old per-step cost, `after` what the UI thread does now.");
    let sizes: Vec<usize> = cx.sizes.iter().copied().filter(|&n| n >= 2048 && !cx.reduced(n)).collect();
    for n in sizes {
        if cx.quick && n > 4096 {
            continue;
        }
        if let Err(e) = cx.fits(fixtures::layer_bytes(n) * 4) {
            cx.report.row(t, &[cx.size_label(n), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
            continue;
        }
        cx.log(&format!("interactions {n}²"));
        let iters = cx.iters;
        let label = cx.size_label(n);
        let row = |cx: &mut Ctx, what: &str, tm: &crate::report::Timing, note: &str| {
            let c = cells(tm);
            cx.report.row(t, &[label.clone(), what.into(), c[0].clone(), c[1].clone(), c[2].clone(), c[3].clone(), note.into()]);
        };

        // ---- Pour Fill: a sparse layer seeded in the empty area fills ~the whole canvas. ----
        let sparse = || {
            let mut s = Session::new(fixtures::document(n, Layout::SparseN(1)));
            let id = fixtures::first_layer(&s.doc);
            s.pour_fill_preview(id, (8, 8), 0.50, [200, 40, 40]).expect("first preview");
            s
        };
        let mut pct = 50;
        let before = bench_with(iters, sparse, |mut s| {
            let id = fixtures::first_layer(&s.doc);
            pct = if pct >= 60 { 50 } else { pct + 1 };
            s.pour_fill_preview(id, (8, 8), pct as f32 / 100.0, [200, 40, 40]).expect("preview");
        });
        row(cx, "Pour Fill threshold step, before (fill + write on the UI thread)", &before, "whole-canvas region");
        // After: the UI thread's share. Measure the three parts once per iteration.
        let (mut plan_ms, mut compute_ms, mut apply_ms) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..(iters.normal.max(3)) {
            let mut s = sparse();
            let id = fixtures::first_layer(&s.doc);
            let t0 = std::time::Instant::now();
            let plan = s.pour_fill_plan(id, (8, 8), (51 + i % 8) as f32 / 100.0, [200, 40, 40]).expect("plan");
            plan_ms.push(crate::report::ms_since(t0));
            let t1 = std::time::Instant::now();
            let filled = plan.compute().expect("compute");
            compute_ms.push(crate::report::ms_since(t1));
            let t2 = std::time::Instant::now();
            s.pour_fill_apply(filled).expect("apply");
            apply_ms.push(crate::report::ms_since(t2));
        }
        let plan_t = crate::report::Timing::from_ms(plan_ms);
        let apply_t = crate::report::Timing::from_ms(apply_ms);
        let compute_t = crate::report::Timing::from_ms(compute_ms);
        row(cx, "Pour Fill threshold step, after: plan (UI thread)", &plan_t, "copies a few Arcs");
        row(cx, "Pour Fill threshold step, after: apply (UI thread, once per finished fill)", &apply_t, "writes the region into the layer");
        row(cx, "Pour Fill threshold step, after: fill (worker thread)", &compute_t, "off the UI thread; the window keeps drawing");

        // ---- Edit Shape: a circle of radius n/4 painted with a 30 px hard ink brush. ----
        let brush = fixtures::brushes().remove(0);
        let shape_at = |k: usize| Shape::Ellipse { c: (n as f64 / 2.0, n as f64 / 2.0), rx: n as f64 / 4.0 + k as f64, ry: n as f64 / 4.0, rot: 0.0 };
        let base = || {
            let mut s = Session::new(fixtures::document(n, Layout::SparseN(1)));
            let id = fixtures::first_layer(&s.doc);
            circle_stroke(&mut s, id, &brush.settings, &shape_at(0));
            s
        };
        let mut k = 0;
        let old = bench_with(iters, base, |mut s| {
            let id = fixtures::first_layer(&s.doc);
            k = (k + 7) % 90;
            let _ = s.undo();
            circle_stroke(&mut s, id, &brush.settings, &shape_at(k));
        });
        row(cx, "Edit Shape drag frame, before (undo + re-rasterize)", &old, "ellipse r = n/4, 30 px brush");
        let mut k = 0;
        let new = crate::report::bench(iters, || {
            k = (k + 7) % 90;
            let pts = hold_shape::outline(&shape_at(k), 2.0);
            std::hint::black_box(pts.len())
        });
        row(cx, "Edit Shape drag frame, after (vector preview: outline only)", &new, "the polyline is drawn by egui");
        let release = bench_with(iters, || {
            let mut s = base();
            let _ = s.undo();
            s
        }, |mut s| {
            let id = fixtures::first_layer(&s.doc);
            circle_stroke(&mut s, id, &brush.settings, &shape_at(40));
        });
        row(cx, "Edit Shape release (rasterize once)", &release, "once per drag, not per frame");
    }
}
