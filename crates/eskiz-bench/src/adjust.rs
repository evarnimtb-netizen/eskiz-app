//! Scenario 4: adjustments on a full layer, the way `adjust.rs` applies them (`Layer::to_rgba`,
//! `Adjustment::render`, `Session::replace_layer_pixels`), plus Liquify and the GPU shaders.

use std::time::Instant;

use eskiz_app::liquify_tool::LiquifyTool;
use eskiz_app::session::Session;
use eskiz_core::filters::{proxy_factor, render_preview, AdjustKind, Adjustment, CurvePoints, Proxy, Quality};
use eskiz_filters::curves::{Curve, CurveSet, CurvesParams};
use eskiz_filters::liquify::StrokeSample;
use eskiz_gpu::{Adjuster, GpuLayer};

use crate::ctx::Ctx;
use crate::fixtures::{self, Layout};
use crate::report::{fmt_ms, mib, ms_since, reset_peak, rss, Timing};

fn curves_points() -> CurvePoints {
    let mut c = CurvePoints::default();
    // An S-curve on the master channel and a lifted red channel.
    c.channels[0] = vec![[0.0, 0.0], [0.25, 0.18], [0.75, 0.85], [1.0, 1.0]];
    c.channels[1] = vec![[0.0, 0.05], [1.0, 1.0]];
    c
}

fn gpu_curves() -> CurvesParams {
    let pts = curves_points();
    let curve = |k: usize| Curve::from_table(pts.table(k));
    CurvesParams {
        target: CurveSet { master: curve(0), red: curve(1), green: curve(2), blue: curve(3) },
        ..CurvesParams::default()
    }
}

/// What a render hands to the session: pixels for a commit, a ready layer for a preview.
enum Rendered {
    Pixels(Vec<u8>),
    Layer(eskiz_core::Layer),
}

fn cpu_adjustments() -> Vec<(&'static str, Adjustment)> {
    let mut g_small = Adjustment::new(AdjustKind::GaussianBlur);
    g_small.set(0, 0.1); // 20 taps
    let mut g_large = Adjustment::new(AdjustKind::GaussianBlur);
    g_large.set(0, 1.0); // 64 taps
    let mut hsb = Adjustment::new(AdjustKind::Hsb);
    hsb.set(0, 0.15);
    hsb.set(1, 0.2);
    hsb.set(2, -0.05);
    let mut curves = Adjustment::new(AdjustKind::Curves);
    curves.set_curve_points(curves_points());
    vec![("Gaussian blur amount 0.1 (20 taps)", g_small), ("Gaussian blur amount 1.0 (64 taps)", g_large), ("Hue/Saturation/Brightness", hsb), ("Curves (via eskiz-filters f32 bridge)", curves)]
}

pub fn run(cx: &mut Ctx) {
    let t = cx.report.table(
        "4. Adjustments on one fully painted layer",
        &["canvas", "adjustment", "path", "quality", "Layer::to_rgba ms", "render ms", "replace_layer ms", "total ms", "peak RSS", "notes"],
    );
    cx.report.intro(t, "CPU rows follow `adjust.rs`: the layer is flattened to one RGBA8 buffer at pen-down (`to_rgba`), every slider change renders a preview (`Quality::Preview`), and Apply renders at `Quality::Full` and writes the buffer back as one undo step. Liquify follows `liquify_tool.rs`: a 1-megapixel proxy grid is warped for the preview (`view`) and the full-resolution field is resampled on commit. GPU rows are `eskiz_gpu::Adjuster::apply` on an uploaded layer plus the wait (software adapter).");
    for &n in &cx.sizes.clone() {
        if cx.reduced(n) || cx.quick_skip(n) {
            cx.report.row(t, &[cx.size_label(n), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "not run at this size (beyond the app's limit, or --quick)".into()]);
            continue;
        }
        // The layer, its flat copy, the output and the rebuilt layer, plus scratch (the blur works in bands).
        if let Err(e) = cx.fits(fixtures::layer_bytes(n) * 4 + (n * n) as u64 * 4) {
            cx.report.row(t, &[cx.size_label(n), "all".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
            continue;
        }
        let mut session = Session::new(fixtures::document(n, Layout::OneFull));
        let id = fixtures::first_layer(&session.doc);
        for (label, adj) in cpu_adjustments() {
            for quality in [Quality::Preview, Quality::Full] {
                cx.log(&format!("adjust {n}² {label} {quality:?}"));
                let iters = if n >= 8192 { 1 } else { 2 };
                let (mut t_rgba, mut t_render, mut t_replace, mut t_total) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                reset_peak();
                for i in 0..=iters {
                    let t0 = Instant::now();
                    let src = session.doc.layer(id).expect("layer").to_rgba();
                    // A preview of a big canvas renders on a proxy as the app does (`adjust.rs`),
                    // at the zoom that fits a 2560x1440 viewport; the proxy is built once per
                    // factor when the mode opens, so it counts with the entry cost.
                    let k = if quality == Quality::Preview { proxy_factor(adj.kind, adj.pencil, n, n, 1300.0 / n as f32) } else { 1 };
                    let proxy = (k > 1).then(|| Proxy::build(&src, None, n, n, k));
                    let a = ms_since(t0);
                    let t1 = Instant::now();
                    // The app builds the preview layer on the worker thread; so does this row.
                    let out = match &proxy {
                        Some(p) => Rendered::Layer(render_preview(&adj, p, None, n, n)),
                        None if quality == Quality::Preview => Rendered::Layer(eskiz_core::Layer::from_rgba(n, n, adj.render(&src, n, n, quality, None, None))),
                        None => Rendered::Pixels(adj.render(&src, n, n, quality, None, None)),
                    };
                    let b = ms_since(t1);
                    let t2 = Instant::now();
                    let c = if quality == Quality::Full {
                        let Rendered::Pixels(out) = out else { unreachable!() };
                        session.replace_layer_pixels(id, out, "Adjust").expect("replace");
                        let c = ms_since(t2);
                        let _ = session.undo();
                        c
                    } else {
                        // The preview goes to `Session::set_preview` (a `Layer::from_rgba`); the same cost.
                        let Rendered::Layer(out) = out else { unreachable!() };
                        session.set_preview_layer(id, out);
                        let c = ms_since(t2);
                        session.clear_preview();
                        c
                    };
                    if i > 0 {
                        t_rgba.push(a);
                        t_render.push(b);
                        t_replace.push(c);
                        t_total.push(a + b + c);
                    }
                }
                let m = |v: Vec<f64>| fmt_ms(Timing::from_ms(v).median());
                let q = match quality {
                    Quality::Preview => "preview",
                    Quality::Full => "full (apply)",
                };
                cx.report.row(t, &[cx.size_label(n), label.into(), "CPU".into(), q.into(), m(t_rgba), m(t_render), m(t_replace), m(t_total), mib(rss().1), String::new()]);
            }
        }
        liquify(cx, t, &session, id, n);
        gpu(cx, t, &session, id, n);
    }
}

fn liquify(cx: &mut Ctx, t: usize, session: &Session, id: u64, n: usize) {
    cx.log(&format!("liquify {n}²"));
    // Liquify's commit reads the stored RGBA8 and writes its output; nothing else is full size.
    if let Err(e) = cx.fits(fixtures::layer_bytes(n) * 2 + (n * n) as u64 * 8) {
        cx.report.row(t, &[cx.size_label(n), "Liquify push, one brush step".into(), "CPU".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
        return;
    }
    reset_peak();
    let pixels = session.doc.layer(id).expect("layer").to_rgba();
    let t0 = Instant::now();
    let mut tool = LiquifyTool::new(id, n, n, &pixels, None);
    let enter = ms_since(t0);
    tool.begin_stroke();
    let c = n as f32 / 2.0;
    tool.frame(0.0, Some(StrokeSample { x: c, y: c, pressure: 1.0 }));
    // One brush step: the pen moved a quarter radius at 60 Hz.
    let t1 = Instant::now();
    tool.frame(1.0 / 60.0, Some(StrokeSample { x: c + 40.0, y: c + 10.0, pressure: 1.0 }));
    let step = ms_since(t1);
    let t2 = Instant::now();
    let view = tool.view_layer();
    let preview = ms_since(t2);
    let shown = view.is_some();
    tool.end_stroke();
    let t3 = Instant::now();
    let out = tool.commit(&pixels);
    let commit = ms_since(t3);
    cx.report.row(
        t,
        &[
            cx.size_label(n),
            "Liquify push, one brush step".into(),
            "CPU".into(),
            "preview (proxy grid)".into(),
            fmt_ms(enter),
            fmt_ms(step),
            fmt_ms(preview),
            fmt_ms(enter + step + preview),
            mib(rss().1),
            format!("to_rgba = enter (proxy build), render = one simulation step, replace = preview warp + upscale (changed: {shown})"),
        ],
    );
    cx.report.row(t, &[cx.size_label(n), "Liquify commit (full-resolution warp)".into(), "CPU".into(), "full (apply)".into(), "-".into(), fmt_ms(commit), "-".into(), fmt_ms(commit), mib(rss().1), format!("{} bytes out", out.len())]);
}

fn gpu(cx: &mut Ctx, t: usize, session: &Session, id: u64, n: usize) {
    let Some(ctx) = cx.gpu.as_ref() else { return };
    if n > ctx.max_canvas_edge() as usize {
        cx.report.row(t, &[cx.size_label(n), "GPU adjustments".into(), "GPU".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "canvas larger than the adapter's texture limit".into()]);
        return;
    }
    if let Err(e) = cx.fits(fixtures::layer_bytes(n) * 5) {
        cx.report.row(t, &[cx.size_label(n), "GPU adjustments".into(), "GPU".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
        return;
    }
    cx.log(&format!("gpu adjust {n}²"));
    let pixels = session.doc.layer(id).expect("layer").to_rgba();
    let mut layer = GpuLayer::new(n as u32, n as u32);
    let t0 = Instant::now();
    if let Err(e) = layer.upload_all(ctx, &pixels) {
        cx.report.row(t, &[cx.size_label(n), "GPU adjustments".into(), "GPU".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), format!("upload failed: {e}")]);
        return;
    }
    ctx.wait_idle();
    let upload = ms_since(t0);
    let mut adjuster = Adjuster::new(ctx);
    let adjs = [
        ("Gaussian blur amount 0.1", eskiz_gpu::Adjustment::Gaussian(eskiz_gpu::GaussianParams { amount: 0.1 })),
        ("Gaussian blur amount 1.0", eskiz_gpu::Adjustment::Gaussian(eskiz_gpu::GaussianParams { amount: 1.0 })),
        ("Hue/Saturation/Brightness", eskiz_gpu::Adjustment::Hsb(eskiz_gpu::HsbParams { hue: 0.15, saturation: 0.2, brightness: -0.05 })),
        ("Curves", eskiz_gpu::Adjustment::Curves(Box::new(gpu_curves()))),
    ];
    for (label, adj) in adjs {
        let mut ms = Vec::new();
        let mut err = None;
        for i in 0..=cx.iters.normal.min(2) {
            let t1 = Instant::now();
            match adjuster.apply(ctx, &layer, None, &adj) {
                Ok(out) => {
                    ctx.wait_idle();
                    drop(out);
                    if i > 0 {
                        ms.push(ms_since(t1));
                    }
                }
                Err(e) => {
                    err = Some(e.to_string());
                    break;
                }
            }
        }
        let note = match err {
            Some(e) => format!("failed: {e}"),
            None => format!("software adapter; layer upload once: {} ms", fmt_ms(upload)),
        };
        let tm = Timing::from_ms(ms);
        cx.report.row(t, &[cx.size_label(n), label.into(), "GPU (software adapter)".into(), "preview".into(), "-".into(), fmt_ms(tm.median()), "-".into(), fmt_ms(tm.median()), "-".into(), note]);
    }
    drop(adjuster);
    drop(layer);
    ctx.wait_idle();
}
