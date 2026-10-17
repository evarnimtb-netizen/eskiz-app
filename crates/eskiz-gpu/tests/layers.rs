//! Tiled layer storage: upload, download, sparsity and masks.
mod common;
use common::*;
use eskiz_gpu::{GpuError, GpuLayer, GpuMask, Rect, TILE};

#[test]
fn upload_download_round_trip_across_tiles() {
    let Some(ctx) = gpu("layers::round_trip") else { return };
    // Not a multiple of the tile size, so edge tiles are partial.
    let (w, h) = (600u32, 530u32);
    let mut rng = Rng::new(1);
    let px = random_premul(&mut rng, w as usize, h as usize, false);
    let mut layer = GpuLayer::new(w, h);
    assert_eq!(layer.tile_count(), 0);
    assert_eq!(layer.allocated_bytes(), 0);
    layer.upload_all(&ctx, &px).unwrap();
    assert_eq!(layer.tile_count(), 3 * 3);
    assert_eq!(layer.download_all(&ctx).unwrap(), px);

    // Sub-rectangle read-back straddling tile corners.
    let r = Rect::new(250, 255, 20, 3);
    let got = layer.download(&ctx, r).unwrap();
    let mut want = Vec::new();
    for y in r.y..r.y + r.h {
        let i = ((y * w + r.x) * 4) as usize;
        want.extend_from_slice(&px[i..i + (r.w * 4) as usize]);
    }
    assert_eq!(got, want);
}

#[test]
fn partial_upload_touches_only_its_tiles() {
    let Some(ctx) = gpu("layers::partial") else { return };
    let (w, h) = (1024u32, 1024u32);
    let mut layer = GpuLayer::new(w, h);
    let mut rng = Rng::new(2);
    let r = Rect::new(300, 700, 100, 60);
    let patch = random_premul(&mut rng, 100, 60, true);
    let dirty = layer.upload_rect(&ctx, r, &patch).unwrap();
    assert_eq!(dirty, r);
    // Rectangle lies within tile column 1 and row 2.
    assert_eq!(layer.tile_count(), 1);
    assert!(layer.has_tile((1, 2)));
    assert!(!layer.has_tile((0, 0)));
    assert_eq!(layer.download(&ctx, r).unwrap(), patch);
    // Everything else reads as transparent.
    let all = layer.download_all(&ctx).unwrap();
    let nonzero = all.iter().filter(|&&b| b != 0).count();
    assert_eq!(nonzero, patch.iter().filter(|&&b| b != 0).count());
}

#[test]
fn all_zero_uploads_stay_sparse_and_tiles_can_be_freed() {
    let Some(ctx) = gpu("layers::sparse") else { return };
    let mut layer = GpuLayer::new(2048, 2048);
    layer
        .upload_rect(&ctx, Rect::new(0, 0, 512, 512), &vec![0u8; 512 * 512 * 4])
        .unwrap();
    assert_eq!(layer.tile_count(), 0, "transparent data must not allocate");
    let mut rng = Rng::new(3);
    let px = random_premul(&mut rng, 256, 256, true);
    layer.upload_rect(&ctx, Rect::new(256, 256, 256, 256), &px).unwrap();
    assert_eq!(layer.tile_count(), 1);
    assert!(layer.allocated_bytes() > 0);
    // Writing zeros over an existing tile keeps it, and reads back as zero.
    layer
        .upload_rect(&ctx, Rect::new(256, 256, 256, 256), &vec![0u8; 256 * 256 * 4])
        .unwrap();
    assert!(layer.download_all(&ctx).unwrap().iter().all(|&b| b == 0));
    layer.free_tile((1, 1));
    assert_eq!(layer.tile_count(), 0);
    // A recycled slot is zeroed before reuse.
    let patch = random_premul(&mut rng, 4, 4, true);
    layer.upload_rect(&ctx, Rect::new(0, 0, 4, 4), &patch).unwrap();
    let t = layer.download(&ctx, Rect::new(0, 0, 256, 256)).unwrap();
    for y in 0..256usize {
        for x in 0..256usize {
            let i = (y * 256 + x) * 4;
            if x < 4 && y < 4 {
                assert_eq!(&t[i..i + 4], &patch[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4]);
            } else {
                assert_eq!(&t[i..i + 4], &[0, 0, 0, 0], "stale data at {x},{y}");
            }
        }
    }
}

#[test]
fn many_tiles_span_several_pages() {
    let Some(ctx) = gpu("layers::pages") else { return };
    let (w, h) = (256 * 9, 256 * 9);
    let mut rng = Rng::new(4);
    let px = random_premul(&mut rng, w as usize, h as usize, true);
    let mut layer = GpuLayer::new(w, h);
    layer.upload_all(&ctx, &px).unwrap();
    assert_eq!(layer.tile_count(), 81);
    assert_eq!(layer.download_all(&ctx).unwrap(), px);
    layer.clear();
    assert_eq!(layer.tile_count(), 0);
    assert_eq!(layer.allocated_bytes(), 0);
}

#[test]
fn invalid_arguments_are_errors() {
    let Some(ctx) = gpu("layers::errors") else { return };
    let mut layer = GpuLayer::new(100, 100);
    let e = layer.upload_rect(&ctx, Rect::new(90, 0, 20, 5), &vec![0; 20 * 5 * 4]);
    assert!(matches!(e, Err(GpuError::Invalid(_))));
    let e = layer.upload_rect(&ctx, Rect::new(0, 0, 10, 10), &[0u8; 12]);
    assert!(matches!(e, Err(GpuError::Invalid(_))));
    assert!(matches!(
        layer.download(&ctx, Rect::new(0, 0, 101, 1)),
        Err(GpuError::Invalid(_))
    ));
    assert_eq!(layer.download(&ctx, Rect::new(5, 5, 0, 0)).unwrap().len(), 0);
}

#[test]
fn masks_use_their_default_for_missing_tiles() {
    let Some(ctx) = gpu("layers::mask") else { return };
    let (w, h) = (700u32, 300u32);
    let mut reveal = GpuMask::new(w, h, 1.0);
    assert_eq!(reveal.tile_count(), 0);
    assert!(reveal.download(&ctx, Rect::new(0, 0, w, h)).unwrap().iter().all(|&b| b == 255));
    let mut rng = Rng::new(5);
    let data = random_mask(&mut rng, 40, 40);
    reveal.upload_rect(&ctx, Rect::new(260, 10, 40, 40), &data).unwrap();
    assert_eq!(reveal.tile_count(), 1);
    assert_eq!(reveal.download(&ctx, Rect::new(260, 10, 40, 40)).unwrap(), data);
    // The rest of that tile keeps the default.
    let beside = reveal.download(&ctx, Rect::new(300, 10, 10, 10)).unwrap();
    assert!(beside.iter().all(|&b| b == 255));

    let hide = GpuMask::new(w, h, 0.0);
    assert!(hide.download(&ctx, Rect::new(0, 0, w, h)).unwrap().iter().all(|&b| b == 0));
    assert_eq!(TILE, 256);
}

/// One whole tile of `px` (a `w x h` canvas), as `eskiz_core` stores it: 256 x 256, row-major.
fn tile_of(px: &[u8], w: u32, h: u32, tx: u32, ty: u32) -> Vec<u8> {
    let mut t = vec![0u8; (TILE * TILE * 4) as usize];
    for y in 0..TILE {
        for x in 0..TILE {
            let (cx, cy) = (tx * TILE + x, ty * TILE + y);
            if cx < w && cy < h {
                let s = ((cy * w + cx) * 4) as usize;
                let d = ((y * TILE + x) * 4) as usize;
                t[d..d + 4].copy_from_slice(&px[s..s + 4]);
            }
        }
    }
    t
}

#[test]
fn upload_tile_equals_upload_rect() {
    let Some(ctx) = gpu("layers::upload_tile") else { return };
    let (w, h) = (600u32, 530u32);
    let mut rng = Rng::new(11);
    let px = random_premul(&mut rng, w as usize, h as usize, false);

    let mut by_rect = GpuLayer::new(w, h);
    by_rect.upload_all(&ctx, &px).unwrap();

    // Whole tiles, edge tiles clipped to the canvas, straight from tile-sized data.
    let mut by_tile = GpuLayer::new(w, h);
    for ty in 0..3 {
        for tx in 0..3 {
            let t = tile_of(&px, w, h, tx, ty);
            let part = Rect::new(0, 0, (w - tx * TILE).min(TILE), (h - ty * TILE).min(TILE));
            by_tile.upload_tile(&ctx, (tx, ty), part, &t).unwrap();
        }
    }
    assert_eq!(by_tile.tile_count(), by_rect.tile_count());
    assert_eq!(by_tile.download_all(&ctx).unwrap(), px);
    assert_eq!(by_tile.download_all(&ctx).unwrap(), by_rect.download_all(&ctx).unwrap());

    // A part inside a tile, over a tile that already holds other data, and over a recycled slot.
    let patch = random_premul(&mut rng, w as usize, h as usize, true);
    let t = tile_of(&patch, w, h, 1, 1);
    let part = Rect::new(17, 31, 100, 40);
    by_tile.upload_tile(&ctx, (1, 1), part, &t).unwrap();
    let mut want = px.clone();
    for y in 0..part.h {
        for x in 0..part.w {
            let (cx, cy) = (TILE + part.x + x, TILE + part.y + y);
            let i = ((cy * w + cx) * 4) as usize;
            want[i..i + 4].copy_from_slice(&patch[i..i + 4]);
        }
    }
    assert_eq!(by_tile.download_all(&ctx).unwrap(), want);
    by_tile.free_tile((0, 0));
    let t = tile_of(&patch, w, h, 2, 2);
    by_tile.upload_tile(&ctx, (2, 2), Rect::new(0, 0, w - 2 * TILE, h - 2 * TILE), &t).unwrap();
    assert_eq!(by_tile.download(&ctx, Rect::new(2 * TILE, 2 * TILE, w - 2 * TILE, h - 2 * TILE)).unwrap().len(), ((w - 2 * TILE) * (h - 2 * TILE) * 4) as usize);

    // Empty data stays unallocated, a part outside the canvas is refused.
    let mut sparse = GpuLayer::new(w, h);
    sparse.upload_tile(&ctx, (0, 0), Rect::new(0, 0, TILE, TILE), &vec![0u8; (TILE * TILE * 4) as usize]).unwrap();
    assert_eq!(sparse.tile_count(), 0);
    assert!(matches!(
        sparse.upload_tile(&ctx, (2, 2), Rect::new(0, 0, TILE, TILE), &vec![1u8; (TILE * TILE * 4) as usize]),
        Err(GpuError::Invalid(_))
    ));
}

#[test]
fn upload_tile16_equals_upload_rect16() {
    let Some(ctx) = gpu("layers::upload_tile16") else { return };
    let (w, h) = (300u32, 270u32);
    let mut rng = Rng::new(12);
    let px8 = random_premul(&mut rng, w as usize, h as usize, false);
    let px: Vec<u16> = px8.iter().map(|&v| (u16::from(v) * 257).saturating_add(rng.below(200) as u16)).collect();
    let mut by_rect = GpuLayer::new_depth(w, h, eskiz_gpu::Depth::U16);
    by_rect.upload_rect16(&ctx, Rect::new(0, 0, w, h), &px).unwrap();
    let mut by_tile = GpuLayer::new_depth(w, h, eskiz_gpu::Depth::U16);
    for ty in 0..2u32 {
        for tx in 0..2u32 {
            let mut t = vec![0u16; (TILE * TILE * 4) as usize];
            for y in 0..TILE {
                for x in 0..TILE {
                    let (cx, cy) = (tx * TILE + x, ty * TILE + y);
                    if cx < w && cy < h {
                        let s = ((cy * w + cx) * 4) as usize;
                        let d = ((y * TILE + x) * 4) as usize;
                        t[d..d + 4].copy_from_slice(&px[s..s + 4]);
                    }
                }
            }
            let part = Rect::new(0, 0, (w - tx * TILE).min(TILE), (h - ty * TILE).min(TILE));
            by_tile.upload_tile16(&ctx, (tx, ty), part, &t).unwrap();
        }
    }
    assert_eq!(by_tile.download16(&ctx, Rect::new(0, 0, w, h)).unwrap(), by_rect.download16(&ctx, Rect::new(0, 0, w, h)).unwrap());
}

/// Upload cost of a fully painted layer: the old route (copy each tile out with `read_rect`,
/// `upload_rect` it) against `upload_tile` from the tile's own samples, with and without
/// handing the queued writes to the GPU every few tiles. Prints milliseconds and the process's
/// peak resident memory growth.
/// `cargo test --release -p eskiz-gpu --test layers upload_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn upload_cost() {
    let Some(ctx) = gpu("layers::upload_cost") else { return };
    let side = 4096usize;
    let mut rng = Rng::new(21);
    let px = random_premul(&mut rng, 512, 512, true);
    let mut cpu = eskiz_core::Layer::new(side, side);
    for ty in 0..side / 256 {
        for tx in 0..side / 256 {
            cpu.write_rect(tx * 256, ty * 256, 256, 256, &px[..256 * 256 * 4]);
        }
    }
    let tiles = side / 256;
    let rss = || -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
            .unwrap_or(0)
    };
    let variant = std::env::var("UPLOAD_VARIANT").unwrap_or_else(|_| "all".into());
    for round in 0..2 {
        let before = rss();
        let t = std::time::Instant::now();
        let mut layer = GpuLayer::new(side as u32, side as u32);
        for ty in 0..tiles {
            for tx in 0..tiles {
                let at = Rect::new(tx as u32 * 256, ty as u32 * 256, 256, 256);
                match variant.as_str() {
                    "old" => {
                        let data = cpu.read_rect(tx * 256, ty * 256, 256, 256);
                        layer.upload_rect(&ctx, at, &data).unwrap();
                    }
                    _ => {
                        let tile = cpu.tile(tx, ty).unwrap();
                        layer.upload_tile(&ctx, (tx as u32, ty as u32), Rect::new(0, 0, 256, 256), tile.data()).unwrap();
                        if variant == "flush" && (ty * tiles + tx) % 24 == 23 /* the cadence tried first; the app uses 512 */ {
                            ctx.flush_uploads();
                        }
                    }
                }
            }
        }
        let queued_ms = t.elapsed().as_secs_f64() * 1e3;
        ctx.wait_idle();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        println!("variant {variant} round {round}: {side}^2 full layer, 256 tiles: {queued_ms:.0} ms to queue, {ms:.0} ms to finish (peak RSS {before} -> {} KiB)", rss());
        drop(layer);
    }
}
