//! Tests of the GPU display path: the app runs headlessly with the GPU
//! compositor attached to a real wgpu device, and after every operation the GPU composite must
//! equal the CPU compositor, which is the ground truth.
//!
//! They need a wgpu adapter (hardware or software) and print a skip message without one.
//! Set `ESKIZ_GPU_REQUIRE=1` to make a missing adapter a failure.
//!
//! `cargo test -p eskiz-app --release gpu_tests::latency_report -- --ignored --nocapture`
//! prints the before and after numbers quoted in INTEGRATION.md.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eskiz_core::filters::AdjustKind;
use eskiz_core::BlendMode;
use eskiz_gpu::GpuContext;
use eframe::egui::mutex::RwLock;
use eframe::egui::Vec2;
use eframe::egui_wgpu::{Renderer, RendererOptions, RenderState, SurfaceConfig};
use eframe::wgpu;

use crate::app::{EskizApp, Prefs};
use crate::document::Document;
use crate::gpu_canvas::GpuCanvas;
use crate::session::MaskFill;
use crate::snapshot::Headless;

/// A `RenderState` on a headless device, as eframe would hand to the app.
fn render_state() -> Option<RenderState> {
    let ctx = match GpuContext::new() {
        Ok(c) => c,
        Err(e) => {
            assert!(crate::compat::env_os("ESKIZ_GPU_REQUIRE").is_none(), "no GPU adapter: {e}");
            eprintln!("skipping GPU display test: {e}");
            return None;
        }
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let renderer = Renderer::new(&ctx.device, format, RendererOptions::default());
    Some(RenderState {
        adapter: ctx.adapter.clone(),
        available_adapters: Vec::new(),
        instance: wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle()),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        target_format: format,
        renderer: Arc::new(RwLock::new(renderer)),
        surface_config: SurfaceConfig::HIGH_THROUGHPUT,
    })
}

fn harness(state: Option<&RenderState>, w: usize, h: usize) -> Headless {
    Headless::from_app(Vec2::new(1280.0, 860.0), |ctx| {
        let mut app = EskizApp::new(ctx, Document::new(w, h), String::new(), Prefs::default());
        if let Some(state) = state {
            app.attach_gpu(ctx, state);
        }
        app
    })
}

fn gpu_canvas(h: &Headless) -> &GpuCanvas {
    h.app.gpu.as_ref().expect("the GPU display is attached").canvas()
}

/// The GPU composite must equal the CPU composite (within the 8-bit rounding of the two
/// pipelines).
fn assert_same(h: &mut Headless, what: &str) {
    h.step(); // the frame flushes the GPU canvas
    assert!(h.app.gpu_in_use(), "{what}: the GPU display should be in use");
    let cpu = h.app.session.flatten_display();
    let gpu = gpu_canvas(h).read_straight().unwrap();
    assert_eq!(cpu.len(), gpu.len(), "{what}");
    let bad = cpu
        .iter()
        .zip(&gpu)
        .filter(|(a, b)| a.abs_diff(**b) > 2)
        .count();
    assert_eq!(bad, 0, "{what}: {bad} bytes differ by more than 2 between the CPU and GPU composites");
}

/// Drag the pointer across the canvas between two canvas points.
fn drag(h: &mut Headless, a: (f64, f64), b: (f64, f64)) {
    let vp = h.app.viewport;
    let pa = h.app.view.canvas_to_screen(vp, a.0, a.1);
    let pb = h.app.view.canvas_to_screen(vp, b.0, b.1);
    h.move_to(pa);
    h.step();
    h.press(pa, true);
    h.step();
    for k in 1..=10 {
        let t = k as f32 / 10.0;
        h.move_to(pa + (pb - pa) * t);
        h.step();
    }
    h.press(pb, false);
    h.step();
}

/// Apply an `Edit` the way the panels do; gives the edit's item id.
macro_rules! edit {
    ($h:ident, $e:expr) => {{
        let e = $e.expect("the edit was accepted");
        $h.app.apply_refresh(e.refresh);
        e.id
    }};
}

#[test]
fn gpu_composite_follows_every_kind_of_edit() {
    let Some(state) = render_state() else {
        return;
    };
    let mut h = harness(Some(&state), 640, 420);
    assert!(h.app.gpu_in_use());
    assert!(h.app.texture.is_none(), "no CPU texture while the GPU display is on");
    assert!(h.app.canvas_texture_id().is_some());
    assert_same(&mut h, "blank canvas");

    let a = h.app.active;
    h.app.colour = [200, 40, 40];
    let blank = h.app.session.flatten_display();
    drag(&mut h, (60.0, 80.0), (400.0, 120.0));
    assert_same(&mut h, "stroke on the first layer");
    assert_ne!(h.app.session.flatten_display(), blank, "the stroke painted something");
    // Only the tiles under the stroke were recomposited, not the canvas.
    let last = gpu_canvas(&h).last_flush();
    assert!(last.composited && !last.full && last.layers_uploaded == 1, "{last:?}");
    assert!(last.tiles <= 4, "{last:?}");

    // A second layer with its own stroke, then every property.
    let b = edit!(h, h.app.session.add_layer(Some(a))).unwrap();
    h.app.active = b;
    h.app.colour = [30, 80, 220];
    drag(&mut h, (100.0, 40.0), (500.0, 300.0));
    assert_same(&mut h, "stroke on a new layer");
    edit!(h, h.app.session.set_blend(b, BlendMode::Multiply));
    assert_same(&mut h, "multiply");
    edit!(h, h.app.session.set_opacity(b, 0.6));
    assert_same(&mut h, "opacity");
    edit!(h, h.app.session.set_visible(a, false));
    assert_same(&mut h, "base hidden");
    edit!(h, h.app.session.set_visible(a, true));
    assert_same(&mut h, "base shown");

    // Mask, clipping, order.
    edit!(h, h.app.session.add_mask(b, MaskFill::Reveal));
    assert_same(&mut h, "reveal-all mask");
    let (w, hh) = (h.app.session.doc.width, h.app.session.doc.height);
    let mut mask = eskiz_core::Mask::filled(w, hh, 255);
    for y in 0..hh {
        for x in 0..w / 2 {
            mask.data[y * w + x] = (x * 255 / (w / 2)) as u8;
        }
    }
    edit!(h, h.app.session.set_mask(b, Some(mask), "Edit Mask"));
    assert_same(&mut h, "gradient mask");
    edit!(h, h.app.session.set_clipped(b, true));
    assert_same(&mut h, "clipped");
    edit!(h, h.app.session.set_clipped(b, false));
    edit!(h, h.app.session.move_layer(b, false));
    assert_same(&mut h, "moved down");
    edit!(h, h.app.session.remove_mask(b));
    assert_same(&mut h, "mask removed");

    // Undo and redo of strokes, redo of an edit.
    h.app.colour = [20, 160, 60];
    h.app.active = a;
    drag(&mut h, (200.0, 300.0), (580.0, 360.0));
    assert_same(&mut h, "third stroke");
    for i in 0..3 {
        h.app.undo_action();
        assert_same(&mut h, &format!("undo {i}"));
    }
    for i in 0..3 {
        if let Some(r) = h.app.session.redo() {
            h.app.apply_refresh(r);
        }
        assert_same(&mut h, &format!("redo {i}"));
    }

    // Whole-layer and whole-canvas operations.
    edit!(h, h.app.session.fill_layer(a, [250, 240, 200]));
    assert_same(&mut h, "fill");
    edit!(h, h.app.session.duplicate_layer(b));
    assert_same(&mut h, "duplicate");
    edit!(h, h.app.session.flip_canvas(true));
    assert_same(&mut h, "flip");
    edit!(h, h.app.session.set_background_colour([0.2, 0.6, 0.4]));
    assert_same(&mut h, "background color");
    edit!(h, h.app.session.set_background_visible(false));
    assert_same(&mut h, "background hidden");
    edit!(h, h.app.session.set_background_visible(true));
    let top = *h.app.session.doc.layer_ids().last().unwrap();
    edit!(h, h.app.session.merge_down(top));
    h.app.active = h.app.session.valid_active(None);
    assert_same(&mut h, "merge down");
    edit!(h, h.app.session.delete_layer(h.app.active));
    h.app.active = h.app.session.valid_active(None);
    assert_same(&mut h, "delete");
}

#[test]
fn groups_with_their_own_opacity_fall_back_to_the_cpu_display() {
    let Some(state) = render_state() else {
        return;
    };
    let mut h = harness(Some(&state), 320, 240);
    let a = h.app.active;
    h.app.colour = [200, 40, 40];
    drag(&mut h, (40.0, 60.0), (280.0, 100.0));
    let b = edit!(h, h.app.session.add_layer(Some(a))).unwrap();
    h.app.active = b;
    drag(&mut h, (40.0, 140.0), (280.0, 180.0));
    // A plain group is shown by the GPU.
    let g = edit!(h, h.app.session.group_items(&[a, b])).unwrap();
    assert_same(&mut h, "plain group");
    // Group opacity is not supported by the GPU compositor: the CPU texture takes over.
    edit!(h, h.app.session.set_opacity(g, 0.5));
    h.step();
    assert!(!h.app.gpu_in_use());
    assert!(h.app.texture.is_some(), "the CPU texture is drawn");
    let id = h.app.canvas_texture_id().unwrap();
    assert_eq!(Some(id), h.app.texture.as_ref().map(|t| t.id()));
    drag(&mut h, (40.0, 200.0), (280.0, 220.0));
    // And back.
    edit!(h, h.app.session.set_opacity(g, 1.0));
    h.step();
    assert!(h.app.gpu_in_use());
    assert_same(&mut h, "back on the GPU");
}

/// The CPU's answer for "the document with the open adjustment previewed on the active layer".
fn cpu_adjusted_composite(h: &Headless) -> Vec<u8> {
    use eskiz_core::filters::Quality;
    let id = h.app.active;
    let (w, hh) = (h.app.session.doc.width, h.app.session.doc.height);
    let src = h.app.session.doc.layer(id).unwrap().to_rgba();
    let out = h.app.adjustment().unwrap().render(&src, w, hh, Quality::Full, None, None);
    let mut doc = h.app.session.doc.clone();
    doc.put_layer(id, eskiz_core::Layer::from_rgba(w, hh, out));
    eskiz_core::composite(&doc, eskiz_core::CompositeOpts::default())
}

#[test]
fn adjustment_preview_runs_on_the_gpu_and_matches_the_cpu_render() {
    let Some(state) = render_state() else {
        return;
    };
    let mut h = harness(Some(&state), 320, 240);
    h.app.colour = [210, 90, 40];
    drag(&mut h, (30.0, 60.0), (290.0, 120.0));
    h.app.colour = [30, 150, 90];
    drag(&mut h, (60.0, 160.0), (260.0, 90.0));
    let ctx = h.ctx.clone();
    for (kind, values) in [
        (AdjustKind::Hsb, vec![(0usize, 0.3f64), (1, 0.4), (2, -0.1)]),
        (AdjustKind::Opacity, vec![(0, 0.5)]),
        (AdjustKind::GaussianBlur, vec![(0, 0.3)]),
    ] {
        let layer_before = h.app.session.doc.layer(h.app.active).unwrap().to_rgba();
        let composite_before = h.app.session.flatten_display();
        assert!(h.app.enter_adjustment(kind));
        for (i, v) in values {
            h.app.adjust_set(i, v);
        }
        h.step();
        h.step();
        // The preview is on the GPU: the session holds no CPU preview, the layer is untouched,
        // and the composite on screen equals the CPU render of the same adjustment.
        assert!(!h.app.session.has_preview(), "{kind:?}: no CPU preview when the GPU does it");
        assert_eq!(h.app.session.doc.layer(h.app.active).unwrap().to_rgba(), layer_before, "{kind:?}");
        let shown = gpu_canvas(&h).read_straight().unwrap();
        let expected = cpu_adjusted_composite(&h);
        assert_ne!(shown, composite_before, "{kind:?}: the preview changes the picture");
        let bad = shown.iter().zip(&expected).filter(|(a, b)| a.abs_diff(**b) > 3).count();
        assert_eq!(bad, 0, "{kind:?}: {bad} bytes of the GPU preview differ from the CPU render by more than 3");

        // Apply is the CPU render; the picture after it is the one that was previewed.
        h.app.adjust_apply(&ctx);
        for _ in 0..400 {
            h.step();
            if !h.app.adjusting() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!h.app.adjusting());
        assert_same(&mut h, &format!("{kind:?} applied"));
        let applied = gpu_canvas(&h).read_straight().unwrap();
        let bad = applied.iter().zip(&expected).filter(|(a, b)| a.abs_diff(**b) > 3).count();
        assert_eq!(bad, 0, "{kind:?}: the applied result differs from the preview in {bad} bytes");
        h.app.undo_action();
        assert_same(&mut h, &format!("{kind:?} undone"));
        assert_eq!(h.app.session.flatten_display(), composite_before, "{kind:?}: undo restores the picture");
    }
}

#[test]
fn cancelling_an_adjustment_leaves_no_gpu_preview() {
    let Some(state) = render_state() else {
        return;
    };
    let mut h = harness(Some(&state), 200, 150);
    h.app.colour = [210, 90, 40];
    drag(&mut h, (20.0, 40.0), (180.0, 90.0));
    // Cancel leaves no trace, GPU preview or not.
    let before = h.app.session.flatten_display();
    assert!(h.app.enter_adjustment(AdjustKind::Hsb));
    h.app.adjust_set(2, 0.5);
    h.step();
    h.step();
    assert_ne!(gpu_canvas(&h).read_straight().unwrap(), before);
    h.app.adjust_cancel();
    assert_same(&mut h, "after cancel");
    assert_eq!(h.app.session.flatten_display(), before);
}

#[test]
fn a_new_document_with_another_size_recreates_the_output() {
    let Some(state) = render_state() else {
        return;
    };
    let mut h = harness(Some(&state), 300, 200);
    let first = h.app.canvas_texture_id();
    assert!(first.is_some());
    let doc = Document::new(500, 260);
    h.app.session.replace_document(doc);
    h.app.active = h.app.session.valid_active(None);
    h.app.need_fit = true;
    h.app.needs_full = true;
    assert_same(&mut h, "new size");
    assert_eq!(gpu_canvas(&h).size(), (500, 260));
    assert_eq!(h.app.canvas_texture_id(), first, "the egui texture id is reused");
}

/// Saturated and mid colours in vertical bands, some translucent, for the colour-space tests.
fn colour_bands(w: usize, h: usize) -> Vec<u8> {
    let band = w / 8;
    (0..w * h)
        .flat_map(|i| match (i % w) / band.max(1) {
            0 => [255, 0, 0, 255],
            1 => [0, 255, 0, 255],
            2 => [200, 80, 60, 255],
            3 => [255, 200, 0, 255],
            4 => [20, 90, 200, 160],
            5 => [40, 160, 120, 255],
            6 => [128, 128, 128, 255],
            _ => [10, 20, 240, 255],
        })
        .collect()
}

/// the GPU output pass applies the P3-to-sRGB display matrix, and it agrees with the
/// CPU display (`flatten_display`). Switching the document back to sRGB switches it off again.
#[test]
fn display_p3_documents_match_the_cpu_display() {
    let Some(state) = render_state() else {
        return;
    };
    let mut h = harness(Some(&state), 320, 200);
    let id = h.app.session.doc.layer_ids()[0];
    let edit = h.app.session.replace_layer_pixels(id, colour_bands(320, 200), "Bands").expect("bands");
    h.app.apply_refresh(edit.refresh);
    assert_same(&mut h, "sRGB document");
    let srgb_view = gpu_canvas(&h).read_straight().unwrap();

    h.app.session.doc.colour_space = eskiz_core::ColourSpace::DisplayP3;
    h.app.gpu.as_mut().expect("gpu").canvas_mut().mark_all();
    assert_same(&mut h, "Display P3 document");
    let p3_view = gpu_canvas(&h).read_straight().unwrap();
    // The matrix really ran: the mid colours (band 2, x = 80..120) changed on screen...
    let at = |buf: &[u8], x: usize| {
        let i = (100 * 320 + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2]]
    };
    assert_ne!(at(&p3_view, 100), at(&srgb_view, 100), "P3 (200, 80, 60) is shown differently");
    // ... while P3 red, which is outside sRGB, clips to sRGB red.
    assert_eq!(at(&p3_view, 20), [255, 0, 0]);
    // The CPU path agrees with the GPU path on the same pixel.
    let cpu = h.app.session.flatten_display();
    let cpu_px = at(&cpu, 100);
    assert!(cpu_px.iter().zip(at(&p3_view, 100)).all(|(a, b)| a.abs_diff(b) <= 2), "{cpu_px:?}");

    h.app.session.doc.colour_space = eskiz_core::ColourSpace::Srgb;
    assert_same(&mut h, "back to sRGB");
    assert_eq!(gpu_canvas(&h).read_straight().unwrap(), srgb_view);
}

#[test]
fn without_a_gpu_the_cpu_path_is_unchanged() {
    let mut h = harness(None, 200, 150);
    assert!(!h.app.gpu_in_use());
    assert!(h.app.texture.is_some());
    h.app.colour = [10, 10, 10];
    drag(&mut h, (20.0, 30.0), (160.0, 90.0));
    assert!(h.app.session.flatten_display().iter().any(|&b| b != 255));
}

/// Display-update latency of the two paths during a stroke, on this machine's GPU. Not a test:
/// run it with `--ignored --nocapture` (release build for meaningful numbers).
///
/// Per pointer sample it times the brush engine (CPU, the same for both paths) and the display
/// update, which is what differs: for the CPU path `refresh` (flatten the rectangle, build the
/// egui texture delta) plus an upload of that rectangle to a GPU texture, which is what
/// egui-wgpu does with the delta; for the GPU path the dirty-tile upload and the composite,
/// waited on until the GPU has finished.
#[test]
#[ignore = "benchmark"]
fn latency_report() {
    use crate::input::{sample_from, PointerKind, RawPointer};
    let Some(state) = render_state() else {
        return;
    };
    for &(w, hh, layers, brush) in &[
        (2048usize, 1536usize, 6usize, 24.0f32),
        (2048, 1536, 6, 250.0),
        (4096, 4096, 12, 24.0),
        (4096, 4096, 12, 250.0),
    ] {
        for gpu in [false, true] {
            let mut h = harness(gpu.then_some(&state), w, hh);
            for _ in 1..layers {
                let id = h.app.session.add_layer(None).and_then(|e| e.id).unwrap();
                // Give every layer some content so the composite has work to do.
                let mut px = vec![0u8; w * hh * 4];
                for (i, p) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    if (i / w / 64 + i % w / 64) % 3 == 0 {
                        *p = [40, 30, 20, 60];
                    }
                }
                h.app.session.doc.layer_mut(id).unwrap().write_rect(0, 0, w, hh, &px);
                h.app.session.mark_edited();
                h.app.active = id;
                if id.is_multiple_of(2) {
                    h.app.session.set_blend(id, BlendMode::Multiply);
                }
            }
            h.app.tool_params[crate::app::Tool::Brush.index()].size = brush;
            h.app.needs_full = true;
            h.step();
            h.step();
            if gpu {
                gpu_canvas(&h).wait_idle();
            }
            h.app.colour = [30, 80, 200];
            // Scratch texture standing in for the egui texture of the CPU path.
            let scratch = state.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scratch"),
                size: wgpu::Extent3d { width: w as u32, height: hh as u32, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let ctx = h.ctx.clone();
            let settings = h.app.stroke_settings();
            let active = h.app.active;
            assert!(h.app.session.begin_stroke(active, &settings));
            let mut display_ms = Vec::new();
            let mut brush_ms = Vec::new();
            let (cx, cy) = (w as f64 / 2.0, hh as f64 / 2.0);
            for k in 0..240 {
                let t = f64::from(k) * 0.1;
                let raw = RawPointer {
                    x: cx + 260.0 * t.cos() * (1.0 + t * 0.02),
                    y: cy + 160.0 * (t * 1.3).sin(),
                    time: f64::from(k) / 120.0,
                    kind: PointerKind::Mouse,
                    force: None,
                };
                let t0 = Instant::now();
                let rect = h.app.session.push(sample_from(raw));
                let t1 = Instant::now();
                let Some(rect) = rect else {
                    continue;
                };
                h.app.refresh(rect);
                if gpu {
                    h.app.gpu_flush(&ctx);
                    gpu_canvas(&h).wait_idle();
                } else {
                    // What egui-wgpu does with the texture delta.
                    let rgba = h.app.session.flatten_rect_display(rect);
                    state.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &scratch,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x: rect.x as u32, y: rect.y as u32, z: 0 },
                            aspect: wgpu::TextureAspect::All,
                        },
                        &rgba,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(rect.w as u32 * 4),
                            rows_per_image: Some(rect.h as u32),
                        },
                        wgpu::Extent3d { width: rect.w as u32, height: rect.h as u32, depth_or_array_layers: 1 },
                    );
                    state.queue.submit([]);
                    let _ = state.device.poll(wgpu::PollType::wait_indefinitely());
                }
                brush_ms.push((t1 - t0).as_secs_f64() * 1e3);
                display_ms.push(t1.elapsed().as_secs_f64() * 1e3);

            }
            let _ = h.app.session.end_stroke();
            // A property change recomposites the whole canvas.
            let mut full_ms = Vec::new();
            for i in 0..8 {
                let opacity = 0.3 + f64::from(i) * 0.08;
                let e = h.app.session.set_opacity(active, opacity).unwrap();
                h.app.apply_refresh(e.refresh);
                let t = Instant::now();
                if gpu {
                    h.app.sync_full(&ctx);
                    h.app.gpu_flush(&ctx);
                    gpu_canvas(&h).wait_idle();
                } else {
                    let rgba = h.app.session.flatten_display();
                    let image = eframe::egui::ColorImage::from_rgba_unmultiplied([w, hh], &rgba);
                    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                    state.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &scratch,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(w as u32 * 4),
                            rows_per_image: Some(hh as u32),
                        },
                        wgpu::Extent3d { width: w as u32, height: hh as u32, depth_or_array_layers: 1 },
                    );
                    state.queue.submit([]);
                    let _ = state.device.poll(wgpu::PollType::wait_indefinitely());
                }
                full_ms.push(t.elapsed().as_secs_f64() * 1e3);
                h.app.needs_full = false;
            }
            let stat = |v: &mut Vec<f64>| {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let mean = v.iter().sum::<f64>() / v.len() as f64;
                format!("mean {mean:.2}, median {:.2}, p95 {:.2}, max {:.2}", v[v.len() / 2], v[v.len() * 95 / 100], v[v.len() - 1])
            };
            println!(
                "{w}x{hh}, {layers} layers, brush {brush}: {}: display update ms: {}; (brush engine ms: {})",
                if gpu { "GPU compositor" } else { "CPU compositor + upload" },
                stat(&mut display_ms),
                stat(&mut brush_ms),
            );
            println!("    full recomposite after an opacity change, ms: {}", stat(&mut full_ms));
        }
    }
}

/// A driver that stops answering: the frame returns at once, the app drops to the CPU compositor and says so.
#[test]
fn a_stalled_driver_switches_the_app_to_the_cpu_compositor() {
    let Some(state) = render_state() else { return };
    let mut h = harness(Some(&state), 256, 256);
    h.step();
    assert!(h.app.gpu_in_use());
    gpu_canvas(&h).context().mark_stalled();
    assert!(!gpu_canvas(&h).wait_idle(), "a stalled context does not wait");
    let t = Instant::now();
    h.step();
    assert!(t.elapsed() < Duration::from_secs(2), "the UI thread must not block on the driver");
    assert!(!h.app.gpu_in_use(), "the CPU compositor takes over");
    assert!(h.app.gpu.is_none());
    let toast = h.app.toast.as_ref().map(|t| t.text.clone()).unwrap_or_default();
    assert!(toast.contains("Graphics driver is slow"), "{toast}");
    h.step();
    assert!(!h.app.gpu_in_use());
}
