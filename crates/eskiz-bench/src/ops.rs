//! Scenarios 5 and 6: layer operations and undo, through `Session`.

use std::time::Instant;

use eskiz_app::selection_tool::TransformTool;
use eskiz_app::session::Session;
use eskiz_core::composite_to_tiled;
use eskiz_core::filters::{AdjustKind, Adjustment, Quality};
use eskiz_select::{Interp, Mat3, Point};

use crate::ctx::Ctx;
use crate::fixtures::{self, Layout};
use crate::paint::POINTS;
use crate::report::{bench_with, cells, fmt_ms, mib, ms_since, rss, Timing};

pub fn layer_ops(cx: &mut Ctx) {
    let t = cx.report.table("5. Layer operations (Session)", &["canvas", "operation", "document", "n", "median ms", "p95 ms", "min ms", "notes"]);
    cx.report.intro(t, "Each operation is one undo step as the layers panel runs it. `duplicate` shares tiles (copy-on-write), so it should be close to free; `merge down`, `flatten` and `transform` touch every pixel.");
    for &n in &cx.sizes.clone() {
        if cx.reduced(n) || cx.quick_skip(n) {
            cx.report.row(t, &[cx.size_label(n), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "not run at this size (beyond the app's limit, or --quick)".into()]);
            continue;
        }
        if let Err(e) = cx.fits(fixtures::layer_bytes(n) * 4) {
            cx.report.row(t, &[cx.size_label(n), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
            continue;
        }
        cx.log(&format!("layer ops {n}²"));
        let iters = cx.iters;
        // Two full layers: duplicate, merge down, clear.
        let two = || Session::new(fixtures::document(n, Layout::FullN(2)));
        let dup = bench_with(iters, two, |mut s| {
            let top = fixtures::top_layer(&s.doc);
            s.duplicate_layer(top).expect("duplicate");
        });
        cx.report.row(t, &[cx.size_label(n), "duplicate layer (full)".into(), "2 full".into(), cells(&dup)[0].clone(), cells(&dup)[1].clone(), cells(&dup)[2].clone(), cells(&dup)[3].clone(), "tiles shared until written".into()]);
        let merge = bench_with(iters, two, |mut s| {
            let top = fixtures::top_layer(&s.doc);
            s.merge_down(top).expect("merge");
        });
        cx.report.row(t, &[cx.size_label(n), "merge down (full onto full, opacity 0.9)".into(), "2 full".into(), cells(&merge)[0].clone(), cells(&merge)[1].clone(), cells(&merge)[2].clone(), cells(&merge)[3].clone(), String::new()]);
        let clear = bench_with(iters, two, |mut s| {
            let top = fixtures::top_layer(&s.doc);
            s.clear_layer(top).expect("clear");
        });
        cx.report.row(t, &[cx.size_label(n), "clear layer (full)".into(), "2 full".into(), cells(&clear)[0].clone(), cells(&clear)[1].clone(), cells(&clear)[2].clone(), cells(&clear)[3].clone(), String::new()]);

        // Flatten: the composite into one tiled layer (what a flatten / merge-all would do).
        for layout in [Layout::SparseN(20), Layout::FullN(20)] {
            if cx.fits(layout.bytes(n) + fixtures::layer_bytes(n) * 2).is_err() {
                cx.report.row(t, &[cx.size_label(n), "flatten (composite_to_tiled)".into(), layout.label(), "-".into(), "-".into(), "-".into(), "-".into(), cx.fits(layout.bytes(n)).unwrap_err()]);
                continue;
            }
            let doc = fixtures::document(n, layout);
            let fl = crate::report::bench(iters, || composite_to_tiled(&doc, true));
            cx.report.row(t, &[cx.size_label(n), "flatten (composite_to_tiled)".into(), layout.label(), cells(&fl)[0].clone(), cells(&fl)[1].clone(), cells(&fl)[2].clone(), cells(&fl)[3].clone(), String::new()]);
        }

        // Flood fill: Pour Fill on a sparse layer, seeded in the empty area, so it fills nearly
        // the whole canvas (the worst case).
        let sparse = || Session::new(fixtures::document(n, Layout::SparseN(1)));
        let fill = bench_with(iters, sparse, |mut s| {
            let id = fixtures::first_layer(&s.doc);
            s.pour_fill(id, (8, 8), 0.1, [200, 40, 40]).expect("fill");
        });
        cx.report.row(t, &[cx.size_label(n), "flood fill (Pour Fill, fills ~whole canvas)".into(), "1 sparse".into(), cells(&fill)[0].clone(), cells(&fill)[1].clone(), cells(&fill)[2].clone(), cells(&fill)[3].clone(), String::new()]);

        // Full-layer transform: scale 1.1 and rotate 15 degrees about the centre, bicubic, commit.
        // The transform keeps the layer as RGBA8 (4 B/px); the commit adds its output and the new layer.
        let need = fixtures::layer_bytes(n) * 2 + (n * n) as u64 * 4 * 3;
        if let Err(e) = cx.fits(need) {
            cx.report.row(t, &[cx.size_label(n), "transform full layer (scale 1.1, rotate 15°, bicubic)".into(), "1 full".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
        } else {
            let one = || Session::new(fixtures::document(n, Layout::OneFull));
            let (mut t_start, mut t_render, mut t_next, mut t_commit) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let runs = if n >= 8192 { 1 } else { 2 };
            for i in 0..=runs {
                let mut s = one();
                let id = fixtures::first_layer(&s.doc);
                let t0 = Instant::now();
                let mut tool = TransformTool::start(&s, id, Interp::Bicubic).expect("transform");
                let a = ms_since(t0);
                let centre = Point { x: n as f32 / 2.0, y: n as f32 / 2.0 };
                tool.session.apply_gesture((Mat3::scale(1.1, 1.1) * Mat3::rotate_deg(15.0)).about(centre));
                let t1 = Instant::now();
                // The live preview the app draws (proxy of big canvases, bilinear).
                let preview = tool.preview(&s).expect("preview");
                let b = ms_since(t1);
                drop(preview);
                // The next frame of the same drag: the proxy of the source and the base are cached.
                tool.session.apply_gesture(Mat3::translate(3.0, 2.0));
                let t1b = Instant::now();
                let next = tool.preview(&s).expect("preview");
                let b2 = ms_since(t1b);
                drop(next);
                let t2 = Instant::now();
                tool.commit(&mut s, "Transform").expect("commit");
                let c = ms_since(t2);
                if i > 0 {
                    t_start.push(a);
                    t_render.push(b);
                    t_next.push(b2);
                    t_commit.push(c);
                }
            }
            let (ts, tr, tn, tc) = (Timing::from_ms(t_start), Timing::from_ms(t_render), Timing::from_ms(t_next), Timing::from_ms(t_commit));
            cx.report.row(
                t,
                &[
                    cx.size_label(n),
                    "transform full layer (scale 1.1, rotate 15°, bicubic)".into(),
                    "1 full".into(),
                    tc.n().to_string(),
                    fmt_ms(ts.median() + tr.median() + tc.median()),
                    "-".into(),
                    "-".into(),
                    format!("start (to_rgba + bounds) {} ms, first preview frame {} ms (builds the proxy), next frame {} ms, commit {} ms", fmt_ms(ts.median()), fmt_ms(tr.median()), fmt_ms(tn.median()), fmt_ms(tc.median())),
                ],
            );
        }
    }
}

pub fn undo(cx: &mut Ctx) {
    let t = cx.report.table("6. Undo and redo", &["canvas", "step", "undo ms", "redo ms", "undo bytes held", "process RSS", "notes"]);
    cx.report.intro(t, "`History` keeps at most 30 steps and 256 MiB of tiles (`history.rs`), counting each distinct tile once. A stroke's step holds the before and after tiles it touched; a full-layer filter's step holds two whole layers.");
    let brushes = fixtures::brushes();
    for &n in &cx.sizes.clone() {
        if cx.reduced(n) || cx.quick_skip(n) {
            cx.report.row(t, &[cx.size_label(n), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), "not run at this size (beyond the app's limit, or --quick)".into()]);
            continue;
        }
        if let Err(e) = cx.fits(fixtures::layer_bytes(n) * 4) {
            cx.report.row(t, &[cx.size_label(n), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
            continue;
        }
        cx.log(&format!("undo {n}²"));
        let mut session = Session::new(fixtures::document(n, Layout::OneFull));
        let id = fixtures::first_layer(&session.doc);
        let path = fixtures::stroke_path(n, POINTS);
        // A big airbrush stroke.
        let airbrush = &brushes[1];
        assert!(session.begin_stroke(id, &airbrush.settings));
        for s in &path {
            session.push(*s);
        }
        session.end_stroke();
        let t0 = Instant::now();
        session.undo().expect("undo");
        let u = ms_since(t0);
        let t1 = Instant::now();
        session.redo().expect("redo");
        let r = ms_since(t1);
        cx.report.row(t, &[cx.size_label(n), "big airbrush stroke (500 px, 2000 points)".into(), fmt_ms(u), fmt_ms(r), mib(session.undo_bytes() as u64), mib(rss().0), String::new()]);

        // A full-layer filter.
        let mut hsb = Adjustment::new(AdjustKind::Hsb);
        hsb.set(0, 0.2);
        let src = session.doc.layer(id).expect("layer").to_rgba();
        let out = hsb.render(&src, n, n, Quality::Full, None, None);
        drop(src);
        session.replace_layer_pixels(id, out, "HSB").expect("replace");
        let t0 = Instant::now();
        session.undo().expect("undo");
        let u = ms_since(t0);
        let t1 = Instant::now();
        session.redo().expect("redo");
        let r = ms_since(t1);
        cx.report.row(t, &[cx.size_label(n), "full-layer HSB adjustment".into(), fmt_ms(u), fmt_ms(r), mib(session.undo_bytes() as u64), mib(rss().0), "step holds the whole before and after layers".into()]);

        // Growth: N strokes with the ink brush and the airbrush alternating.
        let mut session = Session::new(fixtures::document(n, Layout::OneFull));
        let id = fixtures::first_layer(&session.doc);
        let rss0 = rss().0;
        let steps = if cx.quick { 10 } else { 20 };
        let mut bytes_after = Vec::new();
        for i in 0..steps {
            let b = &brushes[i % 2];
            let path = fixtures::stroke_path(n, 500);
            assert!(session.begin_stroke(id, &b.settings));
            for s in &path {
                let shifted = eskiz_core::geometry::Sample { x: (s.x + i as f64 * 17.0) % n as f64, ..*s };
                session.push(shifted);
            }
            session.end_stroke();
            bytes_after.push(session.undo_bytes());
        }
        let per = if steps > 1 { (bytes_after[steps - 1] as i64 - bytes_after[0] as i64) / (steps as i64 - 1) } else { 0 };
        cx.report.row(
            t,
            &[
                cx.size_label(n),
                format!("history after {steps} alternating ink / airbrush strokes (500 points each)"),
                "-".into(),
                "-".into(),
                format!("{} ({} steps, ~{} per stroke)", mib(bytes_after[steps - 1] as u64), session.undo_steps(), mib(per.max(0) as u64)),
                format!("{} (+{})", mib(rss().0), mib(rss().0.saturating_sub(rss0))),
                "the 256 MiB cap trims old steps once reached".into(),
            ],
        );
    }
}
