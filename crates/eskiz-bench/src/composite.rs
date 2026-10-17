//! Scenarios 2 and 3: compositing (CPU and GPU, full canvas and dirty rectangles) and the
//! display path (texture upload and the canvas quad at three zoom levels).

use eskiz_app::gpu_canvas::GpuCanvas;
use eskiz_app::history::PixelRect;
use eskiz_app::session::Session;
use eskiz_core::{composite, composite_rect, CompositeOpts};

use crate::ctx::Ctx;
use crate::fixtures::{self, Layout};
use crate::gfx::{VIEW_H, VIEW_W};
use crate::report::{bench, fmt_ms, mib, ms_since, Timing};

fn med_p95(t: &Timing) -> String {
    format!("{} ({})", fmt_ms(t.median()), fmt_ms(t.p95()))
}

/// A dirty rectangle of `edge` pixels near the canvas centre, tile-aligned like a dab's would not be.
fn dirty(n: usize, edge: usize) -> PixelRect {
    let edge = edge.min(n);
    let o = (n / 2).saturating_sub(edge / 2) + 37;
    let o = o.min(n - edge);
    PixelRect { x: o, y: o, w: edge, h: edge }
}

pub fn layouts(cx: &Ctx, n: usize) -> Vec<Layout> {
    if cx.quick && n > 2048 {
        vec![Layout::OneFull, Layout::SparseN(20)]
    } else if cx.quick {
        vec![Layout::OneFull, Layout::SparseN(20), Layout::FullN(20), Layout::MixedFull20]
    } else {
        vec![Layout::OneFull, Layout::SparseN(20), Layout::FullN(20), Layout::FullN(100), Layout::MixedFull20]
    }
}

pub fn run(cx: &mut Ctx) {
    let t = cx.report.table(
        "2. Compositing: full canvas and dirty rectangles, CPU (eskiz_core::composite) and GPU (GpuCanvas::flush)",
        &["canvas", "layers", "tile bytes", "CPU full ms", "CPU 256² ms", "CPU 1024² ms", "GPU first flush (upload all) ms", "GPU full ms", "GPU 256² ms", "GPU 1024² ms", "notes"],
    );
    cx.report.intro(t, "Cells are `median (p95)`. The CPU figures are what the CPU display path pays (`flatten_display` / `flatten_rect_display` minus the color-management pass, which is a no-op for sRGB documents). The GPU dirty-rectangle figures include the upload of that rectangle of one layer, as a stroke in progress would need, and the wait for the GPU. GPU timings here are on a software Vulkan adapter and are not representative of real hardware.");
    for &n in &cx.sizes.clone() {
        for layout in layouts(cx, n) {
            let need = layout.bytes(n) + fixtures::layer_bytes(n);
            let label = layout.label();
            if let Err(e) = cx.fits(need) {
                cx.report.row(t, &[cx.size_label(n), label, "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
                continue;
            }
            cx.log(&format!("composite {n}² {label}"));
            crate::report::reset_peak();
            let session = Session::new(fixtures::document(n, layout));
            let doc = &session.doc;
            let bytes = mib(doc.tile_bytes() as u64);
            let opts = CompositeOpts::default();
            let full = bench(cx.iters, || composite(doc, opts));
            let r256 = dirty(n, 256);
            let r1024 = dirty(n, 1024);
            let c256 = bench(cx.iters, || composite_rect(doc, r256.x, r256.y, r256.w, r256.h, opts));
            let c1024 = bench(cx.iters, || composite_rect(doc, r1024.x, r1024.y, r1024.w, r1024.h, opts));

            let mut gpu_cells = vec!["-".to_owned(); 4];
            let mut note = String::new();
            // The GPU mirror duplicates every tile (in host RAM on a software adapter), the upload
            // goes through staging buffers of the same size until the queue has run, and the
            // output adds a canvas.
            if let Err(e) = cx.fits(layout.bytes(n) * 3 + fixtures::layer_bytes(n) * 3) {
                note = format!("GPU {e}");
            } else if let Some(handle) = cx.gpu_handle() {
                let mut canvas = GpuCanvas::new(handle);
                if !canvas.fits(n, n) {
                    note = "GPU: canvas larger than the adapter's texture limit".into();
                } else if !eskiz_app::gpu_canvas::supports(doc) {
                    note = "GPU: document has groups the GPU compositor cannot show".into();
                } else {
                    let t0 = std::time::Instant::now();
                    canvas.mark_all();
                    match canvas.flush(&session) {
                        Err(e) => note = format!("GPU flush failed: {e}"),
                        Ok(_) => {
                            canvas.wait_idle();
                            gpu_cells[0] = fmt_ms(ms_since(t0));
                            let gfull = bench(cx.iters, || {
                                canvas.mark_all();
                                let _ = canvas.flush(&session);
                                canvas.wait_idle();
                            });
                            let id = fixtures::top_layer(doc);
                            let g256 = bench(cx.iters, || {
                                canvas.mark_rect(r256, id);
                                let _ = canvas.flush(&session);
                                canvas.wait_idle();
                            });
                            let g1024 = bench(cx.iters, || {
                                canvas.mark_rect(r1024, id);
                                let _ = canvas.flush(&session);
                                canvas.wait_idle();
                            });
                            gpu_cells[1] = med_p95(&gfull);
                            gpu_cells[2] = med_p95(&g256);
                            gpu_cells[3] = med_p95(&g1024);
                        }
                    }
                }
            } else {
                note = "no GPU adapter".into();
            }
            cx.report.row(
                t,
                &[cx.size_label(n), label, bytes, med_p95(&full), med_p95(&c256), med_p95(&c1024), gpu_cells[0].clone(), gpu_cells[1].clone(), gpu_cells[2].clone(), gpu_cells[3].clone(), note],
            );
            drop(session);
            cx.gpu_cleanup();
        }
    }
}

/// Scenario 3: what the window does per frame to show the canvas.
pub fn display(cx: &mut Ctx) {
    let t = cx.report.table(
        "3. Display: canvas texture upload and the canvas quad at fit / 100% / 400% zoom (2560x1440 viewport)",
        &["canvas", "layers", "path", "full refresh: composite ms", "ColorImage convert ms", "egui texture upload ms", "partial 512² refresh total ms", "draw fit-to-screen ms", "draw 100% ms", "draw 400% ms", "notes"],
    );
    cx.report.intro(t, "CPU path = `sync_full` / `refresh` in `app.rs`: composite on the CPU, `ColorImage::from_rgba_unmultiplied`, `TextureHandle::set` / `set_partial` (an egui-wgpu `write_texture` of the whole canvas or the rectangle). GPU path = `gpu_display.rs`: the compositor output is registered as a native texture, so there is no upload; a full refresh is the GPU recomposite from table 2. The draw is the single textured quad `draw_canvas` emits, sampled with egui's linear filter and no mipmaps, into a 2560x1440 target. The draw numbers come from a software rasteriser (llvmpipe) and show its fragment cost, not a real GPU's.");
    let (Some(gpu), Some(_)) = (cx.gpu.as_ref(), cx.display.as_ref()) else {
        cx.report.row(t, &["-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "no GPU adapter: the display path could not be driven headlessly".into()]);
        return;
    };
    let max_edge = gpu.max_canvas_edge() as usize;
    for &n in &cx.sizes.clone() {
        if cx.reduced(n) {
            cx.report.row(t, &[cx.size_label(n), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "not run at this size (beyond the app's limit; see the memory table for the buffer sizes)".into()]);
            continue;
        }
        let layouts: &[Layout] = if cx.quick_skip(n) { &[Layout::OneFull] } else { &[Layout::OneFull, Layout::SparseN(20)] };
        for &layout in layouts {
            let need = layout.bytes(n) + fixtures::layer_bytes(n) * 3;
            let label = layout.label();
            if let Err(e) = cx.fits(need) {
                cx.report.row(t, &[cx.size_label(n), label, "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), e]);
                continue;
            }
            if n > max_edge {
                cx.report.row(t, &[cx.size_label(n), label, "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), "-".into(), format!("canvas larger than the adapter's {max_edge} px texture limit: egui cannot hold the texture")]);
                continue;
            }
            cx.log(&format!("display {n}² {label}"));
            crate::report::reset_peak();
            let fit = (VIEW_W as f32 / n as f32).min(VIEW_H as f32 / n as f32) * 0.95;
            let mut session = Session::new(fixtures::document(n, layout));

            // CPU path.
            let mut comp = Vec::new();
            let mut conv = Vec::new();
            let mut up = Vec::new();
            let mut tex = None;
            let iters = cx.iters.normal.min(3);
            for _ in 0..=iters {
                let t0 = std::time::Instant::now();
                let rgba = session.flatten_display();
                comp.push(ms_since(t0));
                let gpu = cx.gpu.as_ref().expect("gpu");
                let disp = cx.display.as_mut().expect("display");
                let (id, c, u) = disp.upload_full(gpu, n, n, &rgba);
                conv.push(c);
                up.push(u);
                tex = Some(id);
            }
            let tex = tex.expect("uploaded");
            let (comp, conv, up) = (Timing::from_ms(comp[1..].to_vec()), Timing::from_ms(conv[1..].to_vec()), Timing::from_ms(up[1..].to_vec()));
            let r = dirty(n, 512);
            let partial = bench(cx.iters, || {
                let rgba = session.flatten_rect_display(r);
                let gpu = cx.gpu.as_ref().expect("gpu");
                let disp = cx.display.as_mut().expect("display");
                disp.upload_partial(gpu, tex, [r.x, r.y], r.w, r.h, &rgba);
            });
            let mut draws = Vec::new();
            for zoom in [fit, 1.0, 4.0] {
                let d = bench(cx.iters, || {
                    let gpu = cx.gpu.as_ref().expect("gpu");
                    let disp = cx.display.as_mut().expect("display");
                    disp.draw(gpu, tex, n, n, zoom)
                });
                draws.push(med_p95(&d));
            }
            cx.report.row(
                t,
                &[cx.size_label(n), label.clone(), "CPU (egui texture)".into(), med_p95(&comp), med_p95(&conv), med_p95(&up), med_p95(&partial), draws[0].clone(), draws[1].clone(), draws[2].clone(), format!("fit zoom {:.3}", fit)],
            );

            // The same path with the fused premultiplied output `sync_full` / `refresh` use now.
            {
                let mut comp = Vec::new();
                let mut conv = Vec::new();
                let mut up = Vec::new();
                for _ in 0..=iters {
                    let t0 = std::time::Instant::now();
                    let px = session.display_pixels();
                    comp.push(ms_since(t0));
                    let gpu = cx.gpu.as_ref().expect("gpu");
                    let disp = cx.display.as_mut().expect("display");
                    let (_, c, u) = disp.upload_full_premul(gpu, n, n, px);
                    conv.push(c);
                    up.push(u);
                }
                let (comp, conv, up) = (Timing::from_ms(comp[1..].to_vec()), Timing::from_ms(conv[1..].to_vec()), Timing::from_ms(up[1..].to_vec()));
                let partial = bench(cx.iters, || {
                    let px = session.display_pixels_rect(r);
                    let gpu = cx.gpu.as_ref().expect("gpu");
                    let disp = cx.display.as_mut().expect("display");
                    disp.upload_partial_premul(gpu, tex, [r.x, r.y], r.w, r.h, px);
                });
                cx.report.row(
                    t,
                    &[cx.size_label(n), label.clone(), "CPU (egui texture), fused premultiplied composite".into(), med_p95(&comp), med_p95(&conv), med_p95(&up), med_p95(&partial), "-".into(), "-".into(), "-".into(), "composite includes the color conversion; convert is only the wrap into a ColorImage".into()],
                );
            }

            // GPU path: the compositor's output texture, no upload.
            if cx.fits(layout.bytes(n) * 3 + fixtures::layer_bytes(n) * 4).is_err() {
                continue;
            }
            let Some(handle) = cx.gpu_handle() else { continue };
            let mut canvas = GpuCanvas::new(handle);
            if !canvas.fits(n, n) {
                continue;
            }
            canvas.mark_all();
            if canvas.flush(&session).is_err() {
                continue;
            }
            canvas.wait_idle();
            let Some(view) = canvas.output_view() else { continue };
            let native = {
                let gpu = cx.gpu.as_ref().expect("gpu");
                cx.display.as_mut().expect("display").register(gpu, view)
            };
            let gfull = bench(cx.iters, || {
                canvas.mark_all();
                let _ = canvas.flush(&session);
                canvas.wait_idle();
            });
            let id = fixtures::top_layer(&session.doc);
            let gpart = bench(cx.iters, || {
                canvas.mark_rect(r, id);
                let _ = canvas.flush(&session);
                canvas.wait_idle();
            });
            let mut draws = Vec::new();
            for zoom in [fit, 1.0, 4.0] {
                let d = bench(cx.iters, || {
                    let gpu = cx.gpu.as_ref().expect("gpu");
                    let disp = cx.display.as_mut().expect("display");
                    disp.draw(gpu, native, n, n, zoom)
                });
                draws.push(med_p95(&d));
            }
            cx.report.row(
                t,
                &[cx.size_label(n), label, "GPU (native texture, software adapter)".into(), med_p95(&gfull), "0 (none)".into(), "0 (none)".into(), med_p95(&gpart), draws[0].clone(), draws[1].clone(), draws[2].clone(), "full refresh = GPU recomposite; partial = rect upload + tile recomposite".into()],
            );
            drop(canvas);
            if let Some(disp) = cx.display.as_mut() {
                disp.renderer.free_texture(&native);
                disp.renderer.free_texture(&tex);
            }
            cx.gpu_cleanup();
        }
    }
}
