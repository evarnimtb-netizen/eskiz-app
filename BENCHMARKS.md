# Eskiz large-canvas benchmarks

Measured with `crates/eskiz-bench` (see its README), 15 October 2026.
The harness drives Eskiz's own code paths headlessly (`Session`, `GpuCanvas`, the adjustment,
Liquify, transform, undo and export code of `eskiz-app`) and reports only; nothing in the app was
changed for these numbers.

**Read the machine section first.** This is a 2-core x86 server with a software Vulkan adapter
(llvmpipe). CPU timings are what they are on one 2.1 GHz Xeon core (an Apple M3 performance core
is roughly 2 to 3 times faster per thread, and has 8 cores to this machine's 2). GPU timings are
the software rasteriser's cost and say nothing about real GPUs; they are reported and labelled,
and only their CPU-side components (uploads, submission, memory) are used in the conclusions.

## 1. Machine, build, method

- machine: Intel Xeon @ 2.10 GHz, 2 hardware threads, 7.84 GiB RAM, no swap, Linux 6.18, Ubuntu 24.04.5
- the process runs in a memory cgroup limited to **5.84 GiB** (the OOM killer enforces that, not the 7.84 GiB of `/proc/meminfo`; two runs were killed before the harness learned to read it, see §1.1)
- GPU adapter: `llvmpipe (LLVM 20.1.2, 256 bits) (Vulkan, software)`, Mesa 25.2.8, max texture edge 16384 px
- build: release (`opt-level` 3), rustc 1.99.0; `CARGO_INCREMENTAL=0`
- sizes: 2048², 4096², **8000² (the app's largest canvas: `storage::MAX_SIDE` = 8192 px per side and `gallery::parse_custom_size` caps the area at 64 Mpx, so 8192² is refused by the Gallery)**, then 8192² and 16384² against the engine crates only (an 8192² `.procreate` can still be *imported*; 16384² is the adapter's texture limit)
- layer configurations: 1 fully painted layer; 20 sparse layers (three ink strokes each); 20 and 100 fully painted layers; 20 fully painted layers with Multiply/Screen/Overlay/Soft Light/Color/Luminosity/Add blend modes, opacity 0.6 to 0.98, every fifth layer clipped, ten of them in two groups
- every measurement: one warm-up run, then 5 timed runs (2 once a run takes over 2 s, 1 over 15 s); tables show median (p95); inputs are deterministic (fixed pattern seeds, one fixed 2000-point stylus spiral with pressure, fixed brush seeds)
- memory: a scenario whose pixel data would exceed 50 to 55% of the available memory is reported as `skipped: needs ~N GiB` instead of thrashing; peak RSS is read from `/proc/self/status` after `malloc_trim`, so it covers one scenario
- brushes are the app's own library brushes with their real tip and grain textures (`eskiz_app::brush_library::engine_for`): Draft Ink at 30 px, Broad Aerograph at 500 px, Canvas Dry Brush (texturized grain) at 80 px, Wet Bloom Round (wet mixing) at 120 px, plus the smudge tool with the ink tip

### 1.1 How the run went

The full run had to be split. The first process ran all of painting (tables 1 and 1b) and was
killed by the cgroup OOM killer while building the "100 full layers at 2048²" document for the
GPU: 1.6 GiB of tiles, a 1.6 GiB GPU mirror and 1.6 GiB of wgpu staging buffers for the upload
(the software adapter keeps its textures in host shared memory, which counted against the same
limit). A second process was killed during the 8000² Gaussian blur at full quality (2.9 GiB working
set on top of 1.5 GiB of shared memory still held by earlier GPU work). After that the harness was
taught to cap its budget by the cgroup headroom and to free GPU resources promptly, and the
remaining scenarios ran one process each (`--only`), with the 8000² adjustments in a separate
`--no-gpu` process with a larger budget. Both kills are themselves a finding (§4, items 5 and 9):
the GPU path doubles the document's memory and a full-quality 8000² filter needs 3 to 4 GiB.

`--quick` (2048² for everything, 4096² for painting, compositing and the display) takes about
3 minutes on this machine (roughly a minute on a current laptop); the full run plus the re-runs took about 2.5 hours.

## 2. Headline numbers

All CPU, single thread, 2.1 GHz Xeon; "event" = one `Session::push` of a stylus sample that laid at
least one dab; the target is 8 ms per event.

| | 2048² | 4096² | 8000² (app max) | 8192² | 16384² |
|---|---|---|---|---|---|
| stroke latency, hard ink 30 px, p50 / p95 per event | 0.06 / 0.11 ms | 0.08 / 0.15 ms | 0.15 / 0.29 ms | 0.13 / 0.27 ms | 0.28 / 0.50 ms |
| stroke latency, soft airbrush 500 px, p50 / p95 per dab event | 15.8 / 21.1 ms | 15.9 / 22.9 ms | 16.1 / 21.1 ms | 16.6 / 21.8 ms | 17.0 / 22.2 ms |
| stroke latency, textured 80 px / wet 120 px, p50 per dab event | 0.52 / 0.56 ms | 0.64 / 0.67 ms | 0.63 / 0.78 ms | 0.59 / 0.79 ms | 0.99 / skipped (6 GiB) |
| pen-up hitch, hard ink / airbrush | 131 / 814 ms | 356 ms / 1.7 s | 1.1 / 3.9 s | 1.1 / 3.9 s | 4.3 / 10.6 s |
| event + CPU display refresh, hard ink, p50 (1 layer / 20 sparse) | 0.10 / 0.11 ms | 0.16 / 0.16 ms | 0.25 / 0.29 ms | 0.26 ms | not run |
| frame cost: full CPU recomposite, 1 full layer | 139 ms | 576 ms | 2.7 s | 2.6 s | 10.3 s |
| frame cost: full CPU recomposite, 20 sparse / 20 full layers | 110 ms / 1.4 s | 432 ms / 6.2 s | 1.6 s / skipped (5 GiB) | 1.7 s / skipped | skipped |
| frame cost: CPU recomposite of a 256² / 1024² dirty rect (20 sparse) | 1.8 / 39 ms | 1.8 / 38 ms | 1.4 / 33 ms | 2.2 / 35 ms | 2.1 / 44 ms (1 full) |
| CPU display full refresh (composite + ColorImage + texture upload), 1 full layer | 183 ms | 743 ms | 2.9 s | 2.9 s | not run |
| GPU path first upload of the document (20 sparse), software adapter, CPU side included | 413 ms | 861 ms | 2.2 s | 2.1 s | skipped |
| Gaussian blur 64 taps, apply / HSB apply / Curves apply (render only) | 0.52 / 0.11 / 0.34 s | 2.0 / 0.52 / 1.4 s | 8.3 / 2.1 / 6.3 s | ≈ 8000² | not run |
| full-layer transform (scale + rotate), one preview frame / commit | 0.48 / 0.78 s | 2.1 / 3.2 s | skipped (needs 4.3 GiB) | skipped | not run |
| flood fill of a nearly empty layer | 0.39 s | 1.4 s | 5.0 s | 6.4 s | not run |
| save `.procreate`, 20 sparse / 1 full layer | 2.1 / 0.39 s | 8.1 / 1.5 s | 29 / 5.3 s | 30 / 5.2 s | not run |
| load `.procreate`, 20 sparse / 1 full layer | 0.45 / 0.06 s | 2.1 / 0.25 s | 7.4 / 1.4 s | 7.4 / 1.3 s | not run |
| undo steps kept after 20 strokes (256 MiB history cap) | 20 | 10 | 4 | 4 | not run |
| one fully painted 8-bit layer / one sparse layer (3 strokes) | 16 / 2 MiB | 64 / 6 MiB | 244 / 12 MiB | 256 / 12 MiB | 1024 / 32 MiB |
| peak RSS of an adjustment (table 4, worst) | 0.37 GiB (Curves) | 1.2 GiB (Curves) | 4.1 GiB (Curves) | ≈ 8000² | not run |

Memory ceiling on this machine (5.84 GiB cgroup): a 20-layer fully painted 8000² document (4.9 GiB
of tiles) cannot be built; 20 fully painted 4096² layers (1.25 GiB) composite on the CPU but not
on the GPU path (which needs tiles ×2 plus staging ≈ 3.9 GiB); a single 8000² Curves or Gaussian
apply peaks at 3.2 to 4.2 GiB. §6 translates this to an 8 GB M3.

## 3. Results

Cells are `median (p95)` unless the header says otherwise. Rows marked "over app limit" ran
against the engine crates only. GPU rows are on the software adapter: see §1.

### 1. Painting: one 2000-point stylus stroke (Session::begin_stroke / push / end_stroke)

- **Status: Fixed: parallel 128 px blocks, flat plan, Normal fast paths. Full composite 2048² one layer 124 → 23.7 ms (5.2x), 20 sparse 122 → 38.7 ms, 4096² one layer 567 → 113 ms; 20 full layers 2048² 1478 → 255 ms. Output is byte-identical.**

Per-event cost is one `Session::push` (dab generation, coverage splat and the recomposite of the dab rectangle from the pen-down snapshot), without any display work. Target: under 8 ms. Wide-spaced brushes lay a dab only every few input events, so `dab-event` columns are the statistics over the events that painted something (the others cost microseconds). `begin` is the pen-down cost (snapshot clone, stroke-wide coverage buffer and, for wet brushes, the colour buffer). `pen-up` replays the end taper and recomposites the whole stroke rectangle. Peak RSS is the process peak during the stroke.

| brush | canvas | layers | dabs | begin ms | event p50 ms | event p95 ms | dab-event p50 ms | dab-event p95 ms | event max ms | pen-up ms | stroke total ms (median) | dabs/s | stroke rect | peak RSS | notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| hard ink 30 px | 2048² | 1 | 2530 | 9.25 | 0.059 | 0.102 | 0.062 | 0.105 | 0.490 | 130.5 | 258.2 | 9800 | 1559x1180 | 171 MiB |  |
| smudge 30 px (ink tip) | 2048² | 1 | - | 0.005 | 0.055 | 0.108 | 0.055 | 0.108 | 0.934 | 0.814 | 148.1 | - | 1572x1188 | 144 MiB | dabs not counted (smudge has its own spacing) |
| soft airbrush 500 px | 2048² | 1 | 115 | 7.72 | 0.001 | 13.5 | 15.8 | 21.1 | 27.1 | 814.1 | 2658 | 43 | 1932x1620 | 183 MiB | **dab-event p95 over the 8 ms target** |
| textured dry brush 80 px | 2048² | 1 | 442 | 7.89 | 0.001 | 0.615 | 0.517 | 0.739 | 1.49 | 203.9 | 442.7 | 998 | 1603x1228 | 173 MiB |  |
| wet watercolour 120 px | 2048² | 1 | 568 | 7.65 | 0.002 | 0.893 | 0.561 | 1.25 | 1.87 | 266.0 | 606.3 | 937 | 1591x1232 | 191 MiB |  |
| hard ink 30 px | 4096² | 1 | 5006 | 0.132 | 0.082 | 0.149 | 0.082 | 0.149 | 0.759 | 355.6 | 519.1 | 9643 | 3099x2336 | 252 MiB |  |
| smudge 30 px (ink tip) | 4096² | 1 | - | 0.007 | 0.115 | 0.227 | 0.115 | 0.227 | 9.29 | 1.47 | 293.4 | - | 3112x2344 | 208 MiB | dabs not counted (smudge has its own spacing) |
| soft airbrush 500 px | 4096² | 1 | 227 | 0.189 | 0.001 | 16.4 | 15.9 | 22.9 | 25.7 | 1724 | 5732 | 40 | 3519x2776 | 289 MiB | **dab-event p95 over the 8 ms target** |
| textured dry brush 80 px | 4096² | 1 | 876 | 0.222 | 0.005 | 0.881 | 0.644 | 0.960 | 3.98 | 777.8 | 1284 | 682 | 3144x2383 | 258 MiB |  |
| wet watercolour 120 px | 4096² | 1 | 1124 | 0.201 | 0.098 | 1.20 | 0.667 | 1.40 | 6.64 | 661.8 | 1379 | 815 | 3131x2389 | 296 MiB |  |
| hard ink 30 px | 8000² | 1 | 9729 | 0.212 | 0.148 | 0.293 | 0.148 | 0.293 | 2.22 | 1091 | 1416 | 6869 | 6034x4539 | 566 MiB |  |
| smudge 30 px (ink tip) | 8000² | 1 | - | 0.018 | 0.206 | 0.473 | 0.206 | 0.473 | 8.35 | 3.15 | 483.6 | - | 6047x4547 | 427 MiB | dabs not counted (smudge has its own spacing) |
| soft airbrush 500 px | 8000² | 1 | 440 | 0.159 | 0.002 | 18.0 | 16.1 | 21.1 | 32.1 | 3929 | 11011 | 40 | 6454x4979 | 664 MiB | **dab-event p95 over the 8 ms target** |
| textured dry brush 80 px | 8000² | 1 | 1700 | 0.213 | 0.571 | 0.903 | 0.634 | 0.928 | 3.20 | 2523 | 3539 | 480 | 6078x4586 | 571 MiB |  |
| wet watercolour 120 px | 8000² | 1 | 2178 | 0.162 | 0.655 | 1.53 | 0.778 | 1.60 | 4.68 | 1862 | 3158 | 690 | 6066x4592 | 641 MiB |  |
| hard ink 30 px | 8192² (over app limit, engine only) | 1 | 9962 | 0.161 | 0.133 | 0.268 | 0.133 | 0.268 | 2.03 | 1136 | 1431 | 6963 | 6177x4648 | 575 MiB |  |
| smudge 30 px (ink tip) | 8192² (over app limit, engine only) | 1 | - | 0.037 | 0.225 | 0.389 | 0.225 | 0.389 | 1.35 | 3.12 | 490.6 | - | 6192x4656 | 433 MiB | dabs not counted (smudge has its own spacing) |
| soft airbrush 500 px | 8192² (over app limit, engine only) | 1 | 450 | 0.271 | 0.002 | 18.8 | 16.6 | 21.8 | 29.0 | 3928 | 11730 | 38 | 6600x5088 | 677 MiB | **dab-event p95 over the 8 ms target** |
| textured dry brush 80 px | 8192² (over app limit, engine only) | 1 | 1741 | 0.193 | 0.550 | 0.806 | 0.589 | 0.842 | 12.3 | 2524 | 3468 | 502 | 6223x4694 | 580 MiB |  |
| wet watercolour 120 px | 8192² (over app limit, engine only) | 1 | 2230 | 0.235 | 0.685 | 1.41 | 0.785 | 1.47 | 3.86 | 1893 | 3323 | 671 | 6210x4700 | 658 MiB |  |
| hard ink 30 px | 16384² (over app limit, engine only) | 1 | 19828 | 0.319 | 0.281 | 0.497 | 0.281 | 0.497 | 2.85 | 4292 | 4834 | 4102 | 12337x9271 | 1767 MiB |  |
| smudge 30 px (ink tip) | 16384² (over app limit, engine only) | 1 | - | 0.056 | 0.424 | 0.781 | 0.424 | 0.781 | 3.85 | 5.53 | 936.0 | - | 12352x9279 | 1266 MiB | dabs not counted (smudge has its own spacing) |
| soft airbrush 500 px | 16384² (over app limit, engine only) | 1 | 897 | 0.265 | 0.021 | 20.6 | 17.0 | 22.2 | 47.3 | 10617 | 26532 | 34 | 12759x9711 | 1969 MiB | **dab-event p95 over the 8 ms target** |
| textured dry brush 80 px | 16384² (over app limit, engine only) | 1 | 3472 | 0.274 | 0.968 | 1.57 | 0.990 | 1.58 | 3.44 | 9838 | 11766 | 295 | 12381x9318 | 1776 MiB |  |
| wet watercolour 120 px | 16384² (over app limit, engine only) | 1 | - | - | - | - | - | - | - | - | - | - | - | - | skipped: needs ~6.00 GiB (budget 3.91 GiB) |

### 1b. Painting: per-event cost with the display refresh (what the frame actually costs)

Only the events that laid a dab are counted (the others do no display work). The window refreshes the dirty rectangle once per frame, not per event; at 120 Hz input and 60 Hz display two events share one refresh, so a real frame costs about two `push` plus one refresh. GPU figures are on a software adapter (llvmpipe) here and only show the CPU-side cost of the GPU path (uploads, submission) plus a slow software composite; see the machine notes.

| brush | canvas | layers | display path | dab events | push p50 ms | push + refresh p50 ms | push + refresh p95 ms | refresh share | notes |
|---|---|---|---|---|---|---|---|---|---|
| hard ink 30 px | 2048² | 1 | CPU | 998 | 0.069 | 0.100 | 0.156 | 31% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 2048² | 1 | GPU (software adapter) | 998 | 0.088 | 1.85 | 4.50 | 95% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 2048² | 1 | CPU | 113 | 17.2 | 25.8 | 30.5 | 33% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 2048² | 1 | GPU (software adapter) | 113 | 17.4 | 34.8 | 46.2 | 50% | rect upload + tile recomposite + wait |
| hard ink 30 px | 2048² | 20 | CPU | 998 | 0.067 | 0.105 | 0.172 | 36% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 2048² | 20 | GPU (software adapter) | 998 | 0.099 | 5.17 | 13.6 | 98% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 2048² | 20 | CPU | 113 | 16.1 | 27.5 | 32.5 | 41% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 2048² | 20 | GPU (software adapter) | 113 | 16.8 | 48.8 | 63.2 | 66% | rect upload + tile recomposite + wait |
| hard ink 30 px | 4096² | 1 | CPU | 998 | 0.120 | 0.162 | 0.252 | 26% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 4096² | 1 | GPU (software adapter) | 998 | 0.180 | 2.02 | 3.68 | 91% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 4096² | 1 | CPU | 225 | 19.0 | 28.9 | 35.7 | 34% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 4096² | 1 | GPU (software adapter) | 225 | 16.9 | 35.1 | 49.1 | 52% | rect upload + tile recomposite + wait |
| hard ink 30 px | 4096² | 20 | CPU | 998 | 0.110 | 0.156 | 0.243 | 29% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 4096² | 20 | GPU (software adapter) | 998 | 0.155 | 3.86 | 8.75 | 96% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 4096² | 20 | CPU | 225 | 16.1 | 25.9 | 32.8 | 38% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 4096² | 20 | GPU (software adapter) | 225 | 17.6 | 47.0 | 70.7 | 63% | rect upload + tile recomposite + wait |
| hard ink 30 px | 8000² | 1 | CPU | 998 | 0.195 | 0.253 | 0.368 | 23% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 8000² | 1 | GPU (software adapter) | 998 | 0.238 | 1.96 | 3.84 | 88% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 8000² | 1 | CPU | 438 | 15.7 | 23.9 | 30.1 | 35% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 8000² | 1 | GPU (software adapter) | 438 | 16.1 | 34.0 | 45.3 | 52% | rect upload + tile recomposite + wait |
| hard ink 30 px | 8000² | 20 | CPU | 998 | 0.221 | 0.293 | 0.479 | 25% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 8000² | 20 | GPU (software adapter) | 998 | 0.275 | 4.80 | 10.8 | 94% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 8000² | 20 | CPU | 438 | 15.8 | 25.1 | 32.2 | 37% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 8000² | 20 | GPU (software adapter) | 438 | 16.7 | 38.8 | 52.6 | 57% | rect upload + tile recomposite + wait |
| hard ink 30 px | 8192² (over app limit, engine only) | 1 | CPU | 998 | 0.201 | 0.262 | 0.393 | 23% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 8192² (over app limit, engine only) | 1 | GPU (software adapter) | 998 | 0.259 | 2.15 | 4.97 | 88% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 8192² (over app limit, engine only) | 1 | CPU | 448 | 15.8 | 23.8 | 31.8 | 34% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 8192² (over app limit, engine only) | 1 | GPU (software adapter) | 448 | 16.9 | 35.1 | 45.5 | 52% | rect upload + tile recomposite + wait |
| hard ink 30 px | 8192² (over app limit, engine only) | 20 | CPU | 998 | 0.261 | 0.346 | 0.623 | 25% | composite_rect + ColorImage + egui partial upload |
| hard ink 30 px | 8192² (over app limit, engine only) | 20 | GPU (software adapter) | 998 | 0.256 | 4.78 | 9.86 | 95% | rect upload + tile recomposite + wait |
| soft airbrush 500 px | 8192² (over app limit, engine only) | 20 | CPU | 448 | 14.7 | 23.3 | 30.3 | 37% | composite_rect + ColorImage + egui partial upload |
| soft airbrush 500 px | 8192² (over app limit, engine only) | 20 | GPU (software adapter) | 448 | 16.9 | 38.6 | 54.1 | 56% | rect upload + tile recomposite + wait |

### 2. Compositing: full canvas and dirty rectangles, CPU (eskiz_core::composite) and GPU (GpuCanvas::flush)

- **Status: Fixed: pen-up replays only the changed tail. 4096² pen-up: ink 475 → 7.1 ms, airbrush 2497 → 17 ms, textured 1042 → 4.5 ms; 8000² ink 1523 → 12 ms. Wet strokes keep the full replay (4096² 1030 → 788 ms).**

Cells are `median (p95)`. The CPU figures are what the CPU display path pays (`flatten_display` / `flatten_rect_display` minus the colour-management pass, which is a no-op for sRGB documents). The GPU dirty-rectangle figures include the upload of that rectangle of one layer, as a stroke in progress would need, and the wait for the GPU. GPU timings here are on a software Vulkan adapter and are not representative of real hardware.

| canvas | layers | tile bytes | CPU full ms | CPU 256² ms | CPU 1024² ms | GPU first flush (upload all) ms | GPU full ms | GPU 256² ms | GPU 1024² ms | notes |
|---|---|---|---|---|---|---|---|---|---|---|
| 2048² | 1 full | 16 MiB | 139.2 (145.9) | 2.51 (2.64) | 33.1 (33.2) | 243.8 | 72.4 (82.2) | 8.54 (12.0) | 50.4 (73.0) |  |
| 2048² | 20 sparse | 72 MiB | 110.0 (120.6) | 1.82 (2.11) | 38.5 (48.5) | 412.9 | 161.8 (182.3) | 19.8 (25.0) | 93.1 (94.3) |  |
| 2048² | 20 full | 320 MiB | 1412 (1434) | 20.1 (20.3) | 375.9 (411.9) | 1633 | 665.9 (701.8) | 47.3 (50.6) | 246.3 (285.0) |  |
| 2048² | 100 full | 1600 MiB | 7545 (7545) | 172.5 (176.2) | 2150 (2150) | - | - | - | - | GPU skipped: needs ~4.73 GiB (budget 2.57 GiB) |
| 2048² | 20 full, mixed blend + groups | 320 MiB | 2638 (2638) | 40.0 (52.6) | 629.9 (747.5) | 2054 | 1074 (1209) | 70.2 (71.3) | 405.1 (596.8) |  |
| 4096² | 1 full | 64 MiB | 575.5 (646.6) | 3.15 (3.17) | 49.6 (50.2) | 598.3 | 290.3 (328.0) | 5.98 (6.55) | 31.8 (48.2) |  |
| 4096² | 20 sparse | 142 MiB | 432.4 (600.8) | 1.84 (1.97) | 37.9 (38.1) | 861.1 | 425.7 (452.3) | 13.6 (14.7) | 105.3 (114.2) |  |
| 4096² | 20 full | 1280 MiB | 6245 (6245) | 25.2 (27.0) | 371.0 (457.3) | - | - | - | - | GPU skipped: needs ~3.94 GiB (budget 2.57 GiB) |
| 4096² | 100 full | - | - | - | - | - | - | - | - | skipped: needs ~6.31 GiB (budget 2.57 GiB) |
| 4096² | 20 full, mixed blend + groups | 1280 MiB | 9251 (9251) | 32.4 (32.7) | 559.9 (623.4) | - | - | - | - | GPU skipped: needs ~3.94 GiB (budget 2.57 GiB) |
| 8000² | 1 full | 256 MiB | 2694 (2694) | 2.02 (2.20) | 33.8 (34.3) | 2292 | 1182 (1418) | 5.37 (6.36) | 37.5 (44.7) |  |
| 8000² | 20 sparse | 304 MiB | 1613 (1703) | 1.38 (2.75) | 32.6 (34.5) | 2203 | 1417 (1500) | 9.70 (12.9) | 78.4 (86.3) |  |
| 8000² | 20 full | - | - | - | - | - | - | - | - | skipped: needs ~5.01 GiB (budget 2.57 GiB) |
| 8000² | 100 full | - | - | - | - | - | - | - | - | skipped: needs ~24.08 GiB (budget 2.57 GiB) |
| 8000² | 20 full, mixed blend + groups | - | - | - | - | - | - | - | - | skipped: needs ~5.01 GiB (budget 2.57 GiB) |
| 8192² (over app limit, engine only) | 1 full | 256 MiB | 2572 (2572) | 2.07 (2.14) | 33.7 (35.8) | 1978 | 1233 (1448) | 8.75 (10.1) | 61.2 (69.2) |  |
| 8192² (over app limit, engine only) | 20 sparse | 308 MiB | 1733 (1862) | 2.19 (2.29) | 35.2 (48.1) | 2107 | 1307 (1357) | 4.39 (5.35) | 37.1 (52.0) |  |
| 8192² (over app limit, engine only) | 20 full | - | - | - | - | - | - | - | - | skipped: needs ~5.25 GiB (budget 2.57 GiB) |
| 8192² (over app limit, engine only) | 100 full | - | - | - | - | - | - | - | - | skipped: needs ~25.25 GiB (budget 2.57 GiB) |
| 8192² (over app limit, engine only) | 20 full, mixed blend + groups | - | - | - | - | - | - | - | - | skipped: needs ~5.25 GiB (budget 2.57 GiB) |
| 16384² (over app limit, engine only) | 1 full | 1024 MiB | 10287 (10287) | 2.05 (2.21) | 44.0 (54.2) | - | - | - | - | GPU skipped: needs ~6.00 GiB (budget 2.57 GiB) |
| 16384² (over app limit, engine only) | 20 sparse | - | - | - | - | - | - | - | - | skipped: needs ~2.67 GiB (budget 2.57 GiB) |
| 16384² (over app limit, engine only) | 20 full | - | - | - | - | - | - | - | - | skipped: needs ~21.00 GiB (budget 2.57 GiB) |
| 16384² (over app limit, engine only) | 100 full | - | - | - | - | - | - | - | - | skipped: needs ~101.00 GiB (budget 2.57 GiB) |
| 16384² (over app limit, engine only) | 20 full, mixed blend + groups | - | - | - | - | - | - | - | - | skipped: needs ~21.00 GiB (budget 2.57 GiB) |

### 3. Display: canvas texture upload and the canvas quad at fit / 100% / 400% zoom (2560x1440 viewport)

- **Status: Fixed: 500 px airbrush dab p50 24.3 → 7.05 ms at 4096² (p95 34.1 → 9.5 ms); textured 80 px 0.77 → 0.42 ms. Pixel-identical to the old output.**

CPU path = `sync_full` / `refresh` in `app.rs`: composite on the CPU, `ColorImage::from_rgba_unmultiplied`, `TextureHandle::set` / `set_partial` (an egui-wgpu `write_texture` of the whole canvas or the rectangle). GPU path = `gpu_display.rs`: the compositor output is registered as a native texture, so there is no upload; a full refresh is the GPU recomposite from table 2. The draw is the single textured quad `draw_canvas` emits, sampled with egui's linear filter and no mipmaps, into a 2560x1440 target. The draw numbers come from a software rasteriser (llvmpipe) and show its fragment cost, not a real GPU's.

| canvas | layers | path | full refresh: composite ms | ColorImage convert ms | egui texture upload ms | partial 512² refresh total ms | draw fit-to-screen ms | draw 100% ms | draw 400% ms | notes |
|---|---|---|---|---|---|---|---|---|---|---|
| 2048² | 1 full | CPU (egui texture) | 151.7 (163.1) | 6.97 (8.54) | 24.1 (37.6) | 12.4 (13.2) | 17.9 (30.7) | 25.6 (27.6) | 31.1 (79.2) | fit zoom 0.668 |
| 2048² | 1 full | GPU (native texture, software adapter) | 62.4 (68.2) | 0 (none) | 0 (none) | 10.4 (12.1) | 17.2 (17.9) | 23.4 (24.6) | 22.8 (23.2) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 2048² | 20 sparse | CPU (egui texture) | 129.2 (172.4) | 17.5 (19.7) | 26.2 (38.6) | 10.9 (12.5) | 17.2 (18.5) | 23.5 (28.5) | 24.5 (27.1) | fit zoom 0.668 |
| 2048² | 20 sparse | GPU (native texture, software adapter) | 181.7 (295.3) | 0 (none) | 0 (none) | 41.0 (50.6) | 18.1 (19.5) | 23.7 (28.0) | 23.3 (24.0) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 4096² | 1 full | CPU (egui texture) | 604.7 (619.6) | 76.4 (84.3) | 62.0 (73.7) | 9.41 (10.3) | 26.2 (28.1) | 34.5 (38.0) | 24.4 (29.7) | fit zoom 0.334 |
| 4096² | 1 full | GPU (native texture, software adapter) | 324.9 (337.3) | 0 (none) | 0 (none) | 13.6 (14.1) | 29.0 (30.8) | 35.2 (36.0) | 23.1 (25.8) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 4096² | 20 sparse | CPU (egui texture) | 468.1 (489.0) | 68.1 (72.3) | 33.7 (38.4) | 10.3 (10.6) | 27.9 (28.8) | 32.3 (34.7) | 24.9 (26.3) | fit zoom 0.334 |
| 4096² | 20 sparse | GPU (native texture, software adapter) | 424.9 (470.9) | 0 (none) | 0 (none) | 26.6 (29.1) | 20.2 (22.6) | 27.7 (30.1) | 26.0 (27.3) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 8000² | 1 full | CPU (egui texture) | 2314 (2633) | 266.4 (284.1) | 277.1 (279.6) | 8.67 (10.6) | 29.2 (32.7) | 30.6 (35.7) | 23.2 (23.7) | fit zoom 0.171 |
| 8000² | 1 full | GPU (native texture, software adapter) | 1283 (1350) | 0 (none) | 0 (none) | 13.7 (14.6) | 26.9 (29.2) | 32.5 (37.0) | 27.3 (30.7) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 8000² | 20 sparse | CPU (egui texture) | 1741 (1841) | 288.5 (326.8) | 139.0 (141.4) | 8.22 (8.93) | 31.6 (34.3) | 38.1 (39.4) | 30.4 (31.3) | fit zoom 0.171 |
| 8000² | 20 sparse | GPU (native texture, software adapter) | 1247 (1446) | 0 (none) | 0 (none) | 10.6 (10.7) | 22.7 (24.8) | 31.4 (33.9) | 24.8 (26.0) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 8192² (over app limit, engine only) | 1 full | CPU (egui texture) | 2406 (2548) | 364.2 (394.1) | 159.6 (194.1) | 10.8 (11.5) | 26.8 (29.5) | 33.0 (34.0) | 25.2 (28.4) | fit zoom 0.167 |
| 8192² (over app limit, engine only) | 1 full | GPU (native texture, software adapter) | 1325 (1352) | 0 (none) | 0 (none) | 13.4 (13.7) | 28.5 (29.3) | 33.6 (35.6) | 28.8 (30.3) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 8192² (over app limit, engine only) | 20 sparse | CPU (egui texture) | 1653 (1721) | 262.2 (274.3) | 148.9 (171.9) | 7.78 (8.64) | 23.6 (23.9) | 31.4 (32.8) | 24.4 (24.9) | fit zoom 0.167 |
| 8192² (over app limit, engine only) | 20 sparse | GPU (native texture, software adapter) | 1317 (1552) | 0 (none) | 0 (none) | 17.2 (18.9) | 22.1 (24.3) | 34.4 (37.0) | 24.5 (25.3) | full refresh = GPU recomposite; partial = rect upload + tile recomposite |
| 16384² (over app limit, engine only) | - | - | - | - | - | - | - | - | - | not run at this size (beyond the app's limit; see the memory table for the buffer sizes) |

### 4. Adjustments on one fully painted layer

- **Status: Fixed: tiles are written and read in place, in parallel. 4096² 20 sparse layers: save 8726 → 585 ms (14.9x), load 1479 → 110 ms, Gallery save 7850 → 899 ms; 8000² 20 sparse layers: save 29.3 s → 0.35 s for the export and 1.95 s for `Library::save`.**

CPU rows follow `adjust.rs`: the layer is flattened to one RGBA8 buffer at pen-down (`to_rgba`), every slider change renders a preview (`Quality::Preview`), and Apply renders at `Quality::Full` and writes the buffer back as one undo step. Liquify follows `liquify_tool.rs`: a 1-megapixel proxy grid is warped for the preview (`view`) and the full-resolution field is resampled on commit. GPU rows are `eskiz_gpu::Adjuster::apply` on an uploaded layer plus the wait (software adapter).

| canvas | adjustment | path | quality | Layer::to_rgba ms | render ms | replace_layer ms | total ms | peak RSS | notes |
|---|---|---|---|---|---|---|---|---|---|
| 2048² | Gaussian blur amount 0.1 (20 taps) | CPU | preview | 5.76 | 258.6 | 4.95 | 268.7 | 177 MiB |  |
| 2048² | Gaussian blur amount 0.1 (20 taps) | CPU | full (apply) | 5.20 | 247.2 | 9.33 | 261.7 | 317 MiB |  |
| 2048² | Gaussian blur amount 1.0 (64 taps) | CPU | preview | 29.0 | 249.7 | 16.6 | 271.3 | 189 MiB |  |
| 2048² | Gaussian blur amount 1.0 (64 taps) | CPU | full (apply) | 5.28 | 522.7 | 7.80 | 535.8 | 317 MiB |  |
| 2048² | Hue/Saturation/Brightness | CPU | preview | 28.1 | 112.8 | 4.50 | 145.5 | 189 MiB |  |
| 2048² | Hue/Saturation/Brightness | CPU | full (apply) | 5.01 | 114.7 | 14.2 | 124.4 | 189 MiB |  |
| 2048² | Curves (via eskiz-filters f32 bridge) | CPU | preview | 6.54 | 358.0 | 4.83 | 368.2 | 381 MiB |  |
| 2048² | Curves (via eskiz-filters f32 bridge) | CPU | full (apply) | 15.9 | 344.6 | 5.03 | 354.5 | 381 MiB |  |
| 2048² | Liquify push, one brush step | CPU | preview (proxy grid) | 47.4 | 14.9 | 123.8 | 186.0 | 383 MiB | to_rgba = enter (proxy build), render = one simulation step, replace = preview warp + upscale (changed: true) |
| 2048² | Liquify commit (full-resolution warp) | CPU | full (apply) | - | 312.5 | - | 312.5 | 383 MiB | 16777216 bytes out |
| 2048² | Gaussian blur amount 0.1 | GPU (software adapter) | preview | - | 740.9 | - | 740.9 | - | software adapter; layer upload once: 10.3 ms |
| 2048² | Gaussian blur amount 1.0 | GPU (software adapter) | preview | - | 2701 | - | 2701 | - | software adapter; layer upload once: 10.3 ms |
| 2048² | Hue/Saturation/Brightness | GPU (software adapter) | preview | - | 137.6 | - | 137.6 | - | software adapter; layer upload once: 10.3 ms |
| 2048² | Curves | GPU (software adapter) | preview | - | 138.7 | - | 138.7 | - | software adapter; layer upload once: 10.3 ms |
| 4096² | Gaussian blur amount 0.1 (20 taps) | CPU | preview | 40.3 | 930.0 | 39.9 | 1002 | 422 MiB |  |
| 4096² | Gaussian blur amount 0.1 (20 taps) | CPU | full (apply) | 33.9 | 958.0 | 49.9 | 1026 | 854 MiB |  |
| 4096² | Gaussian blur amount 1.0 (64 taps) | CPU | preview | 24.8 | 863.3 | 26.1 | 911.2 | 470 MiB |  |
| 4096² | Gaussian blur amount 1.0 (64 taps) | CPU | full (apply) | 24.2 | 1980 | 36.2 | 2040 | 982 MiB |  |
| 4096² | Hue/Saturation/Brightness | CPU | preview | 21.9 | 444.1 | 18.5 | 482.1 | 470 MiB |  |
| 4096² | Hue/Saturation/Brightness | CPU | full (apply) | 22.7 | 517.0 | 46.4 | 564.1 | 470 MiB |  |
| 4096² | Curves (via eskiz-filters f32 bridge) | CPU | preview | 23.8 | 1382 | 45.5 | 1448 | 1174 MiB |  |
| 4096² | Curves (via eskiz-filters f32 bridge) | CPU | full (apply) | 24.1 | 1357 | 36.4 | 1418 | 1238 MiB |  |
| 4096² | Liquify push, one brush step | CPU | preview (proxy grid) | 163.3 | 23.3 | 453.7 | 640.4 | 1159 MiB | to_rgba = enter (proxy build), render = one simulation step, replace = preview warp + upscale (changed: true) |
| 4096² | Liquify commit (full-resolution warp) | CPU | full (apply) | - | 1058 | - | 1058 | 1159 MiB | 67108864 bytes out |
| 4096² | Gaussian blur amount 0.1 | GPU (software adapter) | preview | - | 3460 | - | 3460 | - | software adapter; layer upload once: 36.7 ms |
| 4096² | Gaussian blur amount 1.0 | GPU (software adapter) | preview | - | 11687 | - | 11687 | - | software adapter; layer upload once: 36.7 ms |
| 4096² | Hue/Saturation/Brightness | GPU (software adapter) | preview | - | 550.0 | - | 550.0 | - | software adapter; layer upload once: 36.7 ms |
| 4096² | Curves | GPU (software adapter) | preview | - | 688.8 | - | 688.8 | - | software adapter; layer upload once: 36.7 ms |
| 8000² | Gaussian blur amount 0.1 (20 taps) | CPU | preview | 147.2 | 3783 | 195.3 | 4114 | 1053 MiB |  |
| 8000² | Gaussian blur amount 0.1 (20 taps) | CPU | full (apply) | 144.4 | 3871 | 167.9 | 4114 | 2718 MiB |  |
| 8000² | Gaussian blur amount 1.0 (64 taps) | CPU | preview | 175.1 | 2844 | 85.0 | 3006 | 1257 MiB |  |
| 8000² | Gaussian blur amount 1.0 (64 taps) | CPU | full (apply) | 168.2 | 8337 | 217.5 | 8723 | 3210 MiB |  |
| 8000² | Hue/Saturation/Brightness | CPU | preview | 133.6 | 2252 | 167.7 | 2551 | 1261 MiB |  |
| 8000² | Hue/Saturation/Brightness | CPU | full (apply) | 75.9 | 2078 | 283.2 | 2233 | 1261 MiB |  |
| 8000² | Curves (via eskiz-filters f32 bridge) | CPU | preview | 232.5 | 6584 | 260.6 | 7077 | 3943 MiB |  |
| 8000² | Curves (via eskiz-filters f32 bridge) | CPU | full (apply) | 165.7 | 6332 | 233.5 | 6681 | 4187 MiB |  |
| 8000² | Liquify push, one brush step | CPU | preview (proxy grid) | 1041 | 28.6 | 1778 | 2848 | 3763 MiB | to_rgba = enter (proxy build), render = one simulation step, replace = preview warp + upscale (changed: true) |
| 8000² | Liquify commit (full-resolution warp) | CPU | full (apply) | - | 5083 | - | 5083 | 3763 MiB | 256000000 bytes out |
| 8192² (over app limit, engine only) | all | - | - | - | - | - | - | - | not run: within 2.4% of the 8000² pixel count, see the 8000² rows (and `--sizes 8192` to run them) |
| 16384² (over app limit, engine only) | all | - | - | - | - | - | - | not run at this size (beyond the app's limit) |

### 5. Layer operations (Session)

- **Status: Fixed: previews run on a viewport-sized proxy, passes are threaded, no whole-layer `f32` image. Slider change at 8000²: HSB 3158 → 202 ms, Curves 8094 → 178 ms, Gaussian 5666 → 143 ms; 4096² Curves 1845 → 85 ms. Apply at 8000² Curves 8862 → 1372 ms. Peak RSS at 8000² about 1.3 GiB (goal was under 1 GiB; not met).**

Each operation is one undo step as the layers panel runs it. `duplicate` shares tiles (copy-on-write), so it should be close to free; `merge down`, `flatten` and `transform` touch every pixel.

| canvas | operation | document | n | median ms | p95 ms | min ms | notes |
|---|---|---|---|---|---|---|---|
| 2048² | duplicate layer (full) | 2 full | 5 | 2.59 | 3.58 | 2.26 | tiles shared until written |
| 2048² | merge down (full onto full, opacity 0.9) | 2 full | 5 | 202.7 | 258.1 | 175.5 |  |
| 2048² | clear layer (full) | 2 full | 5 | 2.32 | 3.59 | 2.07 |  |
| 2048² | flatten (composite_to_tiled) | 20 sparse | 5 | 87.7 | 90.4 | 85.0 |  |
| 2048² | flatten (composite_to_tiled) | 20 full | 5 | 1444 | 1640 | 1363 |  |
| 2048² | flood fill (Pour Fill, fills ~whole canvas) | 1 sparse | 5 | 390.2 | 408.2 | 280.3 |  |
| 2048² | transform full layer (scale 1.1, rotate 15°, bicubic) | 1 full | 2 | 1352 | - | - | start (to_rgba + f32 image) 91.5 ms, one preview render 476.3 ms, commit 784.1 ms |
| 4096² | duplicate layer (full) | 2 full | 5 | 2.95 | 4.05 | 2.36 | tiles shared until written |
| 4096² | merge down (full onto full, opacity 0.9) | 2 full | 5 | 798.0 | 896.2 | 705.5 |  |
| 4096² | clear layer (full) | 2 full | 5 | 3.20 | 4.80 | 2.50 |  |
| 4096² | flatten (composite_to_tiled) | 20 sparse | 5 | 359.1 | 457.1 | 307.0 |  |
| 4096² | flatten (composite_to_tiled) | 20 full | 2 | 6042 | 6042 | 5513 |  |
| 4096² | flood fill (Pour Fill, fills ~whole canvas) | 1 sparse | 5 | 1360 | 1591 | 1311 |  |
| 4096² | transform full layer (scale 1.1, rotate 15°, bicubic) | 1 full | 2 | 5693 | - | - | start (to_rgba + f32 image) 379.0 ms, one preview render 2122 ms, commit 3193 ms |
| 8000² | duplicate layer (full) | 2 full | 5 | 13.0 | 13.8 | 12.9 | tiles shared until written |
| 8000² | merge down (full onto full, opacity 0.9) | 2 full | 2 | 3060 | 3060 | 2918 |  |
| 8000² | clear layer (full) | 2 full | 5 | 13.9 | 17.6 | 13.5 |  |
| 8000² | flatten (composite_to_tiled) | 20 sparse | 5 | 1228 | 1351 | 1181 |  |
| 8000² | flatten (composite_to_tiled) | 20 full | - | - | - | - | skipped: needs ~4.77 GiB (budget 2.52 GiB) |
| 8000² | flood fill (Pour Fill, fills ~whole canvas) | 1 sparse | 2 | 4953 | 4953 | 4787 |  |
| 8000² | transform full layer (scale 1.1, rotate 15°, bicubic) | 1 full | - | - | - | - | skipped: needs ~4.29 GiB (budget 2.52 GiB) |
| 8192² (over app limit, engine only) | duplicate layer (full) | 2 full | 5 | 21.7 | 25.1 | 15.6 | tiles shared until written |
| 8192² (over app limit, engine only) | merge down (full onto full, opacity 0.9) | 2 full | 2 | 3451 | 3451 | 3295 |  |
| 8192² (over app limit, engine only) | clear layer (full) | 2 full | 5 | 15.5 | 24.7 | 14.4 |  |
| 8192² (over app limit, engine only) | flatten (composite_to_tiled) | 20 sparse | 5 | 1269 | 1463 | 1197 |  |
| 8192² (over app limit, engine only) | flatten (composite_to_tiled) | 20 full | - | - | - | - | skipped: needs ~5.00 GiB (budget 2.52 GiB) |
| 8192² (over app limit, engine only) | flood fill (Pour Fill, fills ~whole canvas) | 1 sparse | 2 | 6404 | 6404 | 6316 |  |
| 8192² (over app limit, engine only) | transform full layer (scale 1.1, rotate 15°, bicubic) | 1 full | - | - | - | - | skipped: needs ~4.50 GiB (budget 2.52 GiB) |
| 16384² (over app limit, engine only) | all | - | - | - | - | - | not run at this size (beyond the app's limit) |

### 6. Undo and redo

- **Status: Fixed: byte-backed session, bounding-box resampling. 4096² first preview frame 3551 → 234 ms, next frames 129 ms, commit 5801 → 1353 ms; 8000² now runs (427 ms first frame, 6.1 s commit).**

`History` keeps at most 30 steps and 256 MiB of tiles (`history.rs`), counting each distinct tile once. A stroke's step holds the before and after tiles it touched; a full-layer filter's step holds two whole layers.

| canvas | step | undo ms | redo ms | undo bytes held | process RSS | notes |
|---|---|---|---|---|---|---|
| 2048² | big airbrush stroke (500 px, 2000 points) | 0.005 | 0.002 | 24 MiB | 168 MiB |  |
| 2048² | full-layer HSB adjustment | 0.002 | 0.002 | 44 MiB | 174 MiB | step holds the whole before and after layers |
| 2048² | history after 20 alternating ink / airbrush strokes (500 points each) | - | - | 200 MiB (20 steps, ~10 MiB per stroke) | 391 MiB (+203 MiB) | the 256 MiB cap trims old steps once reached |
| 4096² | big airbrush stroke (500 px, 2000 points) | 0.005 | 0.003 | 62 MiB | 268 MiB |  |
| 4096² | full-layer HSB adjustment | 0.003 | 0.003 | 159 MiB | 272 MiB | step holds the whole before and after layers |
| 4096² | history after 20 alternating ink / airbrush strokes (500 points each) | - | - | 253 MiB (10 steps, ~12 MiB per stroke) | 599 MiB (+262 MiB) | the 256 MiB cap trims old steps once reached |
| 8000² | big airbrush stroke (500 px, 2000 points) | 0.009 | 0.005 | 124 MiB | 446 MiB |  |
| 8000² | full-layer HSB adjustment | 0.013 | 0.012 | 512 MiB | 680 MiB | step holds the whole before and after layers |
| 8000² | history after 20 alternating ink / airbrush strokes (500 points each) | - | - | 242 MiB (4 steps, ~10 MiB per stroke) | 1133 MiB (+263 MiB) | the 256 MiB cap trims old steps once reached |
| 8192² (over app limit, engine only) | big airbrush stroke (500 px, 2000 points) | 0.006 | 0.004 | 128 MiB | 972 MiB |  |
| 8192² (over app limit, engine only) | full-layer HSB adjustment | 0.013 | 0.012 | 512 MiB | 972 MiB | step holds the whole before and after layers |
| 8192² (over app limit, engine only) | history after 20 alternating ink / airbrush strokes (500 points each) | - | - | 243 MiB (4 steps, ~10 MiB per stroke) | 1408 MiB (+271 MiB) | the 256 MiB cap trims old steps once reached |
| 16384² (over app limit, engine only) | all | - | - | - | - | not run at this size (beyond the app's limit) |

### 7. Files

- **Status: Fixed: scanline span fill. Near-whole-canvas Pour Fill 2048² 276.9 → 19.5 ms, 4096² 1849 → 133.9 ms (about 14x). 8000² was not re-measured.**

`export_document` is the Share sheet's path (`.procreate` = LZ4 tiles in a ZIP; PSD = RLE channels per layer; PNG = the flattened composite). `Library::save` is the Gallery's autosave: it writes the package, reopens it to verify it, and renders a thumbnail. `open_procreate` is the Gallery's load.

| canvas | document | operation | n | median ms | min ms | output size | notes |
|---|---|---|---|---|---|---|---|
| 2048² | 20 sparse | save .procreate (export_document) | 2 | 2073 | 2024 | 2 MiB |  |
| 2048² | 20 sparse | export PSD | 2 | 1546 | 1462 | 4 MiB |  |
| 2048² | 20 sparse | export PNG | 2 | 216.0 | 193.9 | 0 MiB |  |
| 2048² | 20 sparse | load .procreate (open_procreate) | 2 | 446.8 | 413.1 | 2 MiB |  |
| 2048² | 20 sparse | Gallery save (Library::save: write, verify, thumbnail) | 2 | 2485 | 2263 | - | the autosave path (every 30 s while dirty) |
| 2048² | 1 full | save .procreate (export_document) | 2 | 389.5 | 302.5 | 9 MiB |  |
| 2048² | 1 full | export PSD | 2 | 312.5 | 309.6 | 7 MiB |  |
| 2048² | 1 full | export PNG | 2 | 245.3 | 239.5 | 0 MiB |  |
| 2048² | 1 full | load .procreate (open_procreate) | 2 | 56.8 | 54.0 | 9 MiB |  |
| 2048² | 1 full | Gallery save (Library::save: write, verify, thumbnail) | 2 | 584.6 | 528.0 | - | the autosave path (every 30 s while dirty) |
| 4096² | 20 sparse | save .procreate (export_document) | 2 | 8078 | 7631 | 3 MiB |  |
| 4096² | 20 sparse | export PSD | 2 | 5468 | 5450 | 12 MiB |  |
| 4096² | 20 sparse | export PNG | 2 | 812.9 | 808.2 | 1 MiB |  |
| 4096² | 20 sparse | load .procreate (open_procreate) | 2 | 2111 | 1914 | 3 MiB |  |
| 4096² | 20 sparse | Gallery save (Library::save: write, verify, thumbnail) | 2 | 8140 | 8057 | - | the autosave path (every 30 s while dirty) |
| 4096² | 1 full | save .procreate (export_document) | 2 | 1528 | 1307 | 31 MiB |  |
| 4096² | 1 full | export PSD | 2 | 1577 | 1387 | 24 MiB |  |
| 4096² | 1 full | export PNG | 2 | 1096 | 1073 | 1 MiB |  |
| 4096² | 1 full | load .procreate (open_procreate) | 2 | 252.5 | 239.2 | 31 MiB |  |
| 4096² | 1 full | Gallery save (Library::save: write, verify, thumbnail) | 2 | 2103 | 2101 | - | the autosave path (every 30 s while dirty) |
| 8000² | 20 sparse | save .procreate (export_document) | 2 | 29288 | 29148 | 8 MiB |  |
| 8000² | 20 sparse | export PSD | - | - | - | - | skipped: needs ~5.64 GiB (budget 2.52 GiB) |
| 8000² | 20 sparse | export PNG | 2 | 3160 | 3112 | 2 MiB |  |
| 8000² | 20 sparse | load .procreate (open_procreate) | 2 | 7378 | 7067 | 8 MiB |  |
| 8000² | 20 sparse | Gallery save (Library::save: write, verify, thumbnail) | 2 | 30932 | 30623 | - | the autosave path (every 30 s while dirty) |
| 8000² | 1 full | save .procreate (export_document) | 2 | 5263 | 5087 | 108 MiB |  |
| 8000² | 1 full | export PSD | 2 | 6646 | 6555 | 86 MiB |  |
| 8000² | 1 full | export PNG | 2 | 4146 | 4118 | 3 MiB |  |
| 8000² | 1 full | load .procreate (open_procreate) | 2 | 1375 | 1152 | 108 MiB |  |
| 8000² | 1 full | Gallery save (Library::save: write, verify, thumbnail) | 2 | 8730 | 8367 | - | the autosave path (every 30 s while dirty) |
| 8192² (over app limit, engine only) | 20 sparse | save .procreate (export_document) | 1 | 30301 | 30301 | 9 MiB |  |
| 8192² (over app limit, engine only) | 20 sparse | export PSD | - | - | - | - | skipped: needs ~5.92 GiB (budget 2.52 GiB) |
| 8192² (over app limit, engine only) | 20 sparse | export PNG | 1 | 3174 | 3174 | 2 MiB |  |
| 8192² (over app limit, engine only) | 20 sparse | load .procreate (open_procreate) | 1 | 7413 | 7413 | 9 MiB |  |
| 8192² (over app limit, engine only) | 20 sparse | Gallery save (Library::save: write, verify, thumbnail) | 1 | 30955 | 30955 | - | the autosave path (every 30 s while dirty) |
| 8192² (over app limit, engine only) | 1 full | save .procreate (export_document) | 1 | 5208 | 5208 | 115 MiB |  |
| 8192² (over app limit, engine only) | 1 full | export PSD | 1 | 6982 | 6982 | 90 MiB |  |
| 8192² (over app limit, engine only) | 1 full | export PNG | 1 | 4488 | 4488 | 3 MiB |  |
| 8192² (over app limit, engine only) | 1 full | load .procreate (open_procreate) | 1 | 1252 | 1252 | 115 MiB |  |
| 8192² (over app limit, engine only) | 1 full | Gallery save (Library::save: write, verify, thumbnail) | 1 | 7992 | 7992 | - | the autosave path (every 30 s while dirty) |
| 16384² (over app limit, engine only) | - | all | - | - | - | - | not run at this size (beyond the app's limit) |

### 8. Memory: bytes per layer and per working buffer

- **Status: Fixed: the byte cap scales with canvas and RAM (eight layers' worth, at most a sixth of physical memory, at most 4 GiB, never below 256 MiB). 4096² holds 20 steps instead of 10; an 8000² canvas on 8 GiB gets about 1.3 GiB, about 5x as many steps.**

Tiles are 256x256 RGBA8 (256 KiB each; 512 KiB at 16 bit) and only allocated where a layer has pixels. The working buffers are what one operation allocates on top of the layers, from the code paths named.

| canvas | item | bytes | notes |
|---|---|---|---|
| 2048² | one fully painted 8-bit layer | 16 MiB | 64 tiles; 16-bit: 32 MiB |
| 2048² | one sparse layer (three ink strokes) | 2 MiB | 9 tiles allocated |
| 2048² | stroke in progress: coverage buffer (f32 per canvas pixel) | 16 MiB | `PaintEngine::begin_paths` -> `CoverageBuffer::new(0, 0, width, height)` (paint.rs); allocated zeroed, so pages are touched only where the stroke goes, and rebuilt again at pen-up |
| 2048² | wet stroke in progress: colour delta buffer (3 x f32 per pixel) | 48 MiB | `WetStroke::new` (wet.rs), on top of the coverage buffer |
| 2048² | adjustment open: flat RGBA8 copy of the layer | 16 MiB | `adjust.rs::enter_adjustment_mode` `layer.to_rgba()`; preview and apply each produce another |
| 2048² | Gaussian blur working set (f32 input + f32 middle pass) | 128 MiB | `filters/gaussian.rs::blur`: `input` and `mid` are `Vec<f32>` of 4 channels |
| 2048² | bridged filter (Curves, Noise, ...): f32 `Image` in and out | 128 MiB | `filters/bridge.rs::render` |
| 2048² | Liquify commit: f32 source + f32 field + f32 output | 160 MiB | `liquify_tool.rs::commit` |
| 2048² | transform open: f32 source image + f32 footprint mask; commit: + layer, rendered, output | 80 MiB / 272 MiB | `selection_tool.rs::TransformTool::start` / `eskiz_select::session::commit` |
| 2048² | CPU display: flattened RGBA8 + egui `ColorImage` + GPU texture | 48 MiB | `app.rs::sync_full` |
| 2048² | GPU mirror per layer (tiles) + compositor output | 16 MiB + 16 MiB | `gpu_canvas.rs`: every layer's tiles are duplicated on the GPU; on a unified-memory Mac that is system RAM too |
| 4096² | one fully painted 8-bit layer | 64 MiB | 256 tiles; 16-bit: 128 MiB |
| 4096² | one sparse layer (three ink strokes) | 6 MiB | 25 tiles allocated |
| 4096² | stroke in progress: coverage buffer (f32 per canvas pixel) | 64 MiB | `PaintEngine::begin_paths` -> `CoverageBuffer::new(0, 0, width, height)` (paint.rs); allocated zeroed, so pages are touched only where the stroke goes, and rebuilt again at pen-up |
| 4096² | wet stroke in progress: colour delta buffer (3 x f32 per pixel) | 192 MiB | `WetStroke::new` (wet.rs), on top of the coverage buffer |
| 4096² | adjustment open: flat RGBA8 copy of the layer | 64 MiB | `adjust.rs::enter_adjustment_mode` `layer.to_rgba()`; preview and apply each produce another |
| 4096² | Gaussian blur working set (f32 input + f32 middle pass) | 512 MiB | `filters/gaussian.rs::blur`: `input` and `mid` are `Vec<f32>` of 4 channels |
| 4096² | bridged filter (Curves, Noise, ...): f32 `Image` in and out | 512 MiB | `filters/bridge.rs::render` |
| 4096² | Liquify commit: f32 source + f32 field + f32 output | 640 MiB | `liquify_tool.rs::commit` |
| 4096² | transform open: f32 source image + f32 footprint mask; commit: + layer, rendered, output | 320 MiB / 1088 MiB | `selection_tool.rs::TransformTool::start` / `eskiz_select::session::commit` |
| 4096² | CPU display: flattened RGBA8 + egui `ColorImage` + GPU texture | 192 MiB | `app.rs::sync_full` |
| 4096² | GPU mirror per layer (tiles) + compositor output | 64 MiB + 64 MiB | `gpu_canvas.rs`: every layer's tiles are duplicated on the GPU; on a unified-memory Mac that is system RAM too |
| 8000² | one fully painted 8-bit layer | 244 MiB | 977 tiles; 16-bit: 488 MiB |
| 8000² | one sparse layer (three ink strokes) | 12 MiB | 48 tiles allocated |
| 8000² | stroke in progress: coverage buffer (f32 per canvas pixel) | 244 MiB | `PaintEngine::begin_paths` -> `CoverageBuffer::new(0, 0, width, height)` (paint.rs); allocated zeroed, so pages are touched only where the stroke goes, and rebuilt again at pen-up |
| 8000² | wet stroke in progress: colour delta buffer (3 x f32 per pixel) | 732 MiB | `WetStroke::new` (wet.rs), on top of the coverage buffer |
| 8000² | adjustment open: flat RGBA8 copy of the layer | 244 MiB | `adjust.rs::enter_adjustment_mode` `layer.to_rgba()`; preview and apply each produce another |
| 8000² | Gaussian blur working set (f32 input + f32 middle pass) | 1953 MiB | `filters/gaussian.rs::blur`: `input` and `mid` are `Vec<f32>` of 4 channels |
| 8000² | bridged filter (Curves, Noise, ...): f32 `Image` in and out | 1953 MiB | `filters/bridge.rs::render` |
| 8000² | Liquify commit: f32 source + f32 field + f32 output | 2441 MiB | `liquify_tool.rs::commit` |
| 8000² | transform open: f32 source image + f32 footprint mask; commit: + layer, rendered, output | 1221 MiB / 4150 MiB | `selection_tool.rs::TransformTool::start` / `eskiz_select::session::commit` |
| 8000² | CPU display: flattened RGBA8 + egui `ColorImage` + GPU texture | 732 MiB | `app.rs::sync_full` |
| 8000² | GPU mirror per layer (tiles) + compositor output | 244 MiB + 244 MiB | `gpu_canvas.rs`: every layer's tiles are duplicated on the GPU; on a unified-memory Mac that is system RAM too |
| 8192² (over app limit, engine only) | one fully painted 8-bit layer | 256 MiB | 1024 tiles; 16-bit: 512 MiB |
| 8192² (over app limit, engine only) | one sparse layer (three ink strokes) | 12 MiB | 50 tiles allocated |
| 8192² (over app limit, engine only) | stroke in progress: coverage buffer (f32 per canvas pixel) | 256 MiB | `PaintEngine::begin_paths` -> `CoverageBuffer::new(0, 0, width, height)` (paint.rs); allocated zeroed, so pages are touched only where the stroke goes, and rebuilt again at pen-up |
| 8192² (over app limit, engine only) | wet stroke in progress: colour delta buffer (3 x f32 per pixel) | 768 MiB | `WetStroke::new` (wet.rs), on top of the coverage buffer |
| 8192² (over app limit, engine only) | adjustment open: flat RGBA8 copy of the layer | 256 MiB | `adjust.rs::enter_adjustment_mode` `layer.to_rgba()`; preview and apply each produce another |
| 8192² (over app limit, engine only) | Gaussian blur working set (f32 input + f32 middle pass) | 2048 MiB | `filters/gaussian.rs::blur`: `input` and `mid` are `Vec<f32>` of 4 channels |
| 8192² (over app limit, engine only) | bridged filter (Curves, Noise, ...): f32 `Image` in and out | 2048 MiB | `filters/bridge.rs::render` |
| 8192² (over app limit, engine only) | Liquify commit: f32 source + f32 field + f32 output | 2560 MiB | `liquify_tool.rs::commit` |
| 8192² (over app limit, engine only) | transform open: f32 source image + f32 footprint mask; commit: + layer, rendered, output | 1280 MiB / 4352 MiB | `selection_tool.rs::TransformTool::start` / `eskiz_select::session::commit` |
| 8192² (over app limit, engine only) | CPU display: flattened RGBA8 + egui `ColorImage` + GPU texture | 768 MiB | `app.rs::sync_full` |
| 8192² (over app limit, engine only) | GPU mirror per layer (tiles) + compositor output | 256 MiB + 256 MiB | `gpu_canvas.rs`: every layer's tiles are duplicated on the GPU; on a unified-memory Mac that is system RAM too |
| 16384² (over app limit, engine only) | one fully painted 8-bit layer | 1024 MiB | 4096 tiles; 16-bit: 2048 MiB |
| 16384² (over app limit, engine only) | one sparse layer (three ink strokes) | 32 MiB | 128 tiles allocated |
| 16384² (over app limit, engine only) | stroke in progress: coverage buffer (f32 per canvas pixel) | 1024 MiB | `PaintEngine::begin_paths` -> `CoverageBuffer::new(0, 0, width, height)` (paint.rs); allocated zeroed, so pages are touched only where the stroke goes, and rebuilt again at pen-up |
| 16384² (over app limit, engine only) | wet stroke in progress: colour delta buffer (3 x f32 per pixel) | 3072 MiB | `WetStroke::new` (wet.rs), on top of the coverage buffer |
| 16384² (over app limit, engine only) | adjustment open: flat RGBA8 copy of the layer | 1024 MiB | `adjust.rs::enter_adjustment_mode` `layer.to_rgba()`; preview and apply each produce another |
| 16384² (over app limit, engine only) | Gaussian blur working set (f32 input + f32 middle pass) | 8192 MiB | `filters/gaussian.rs::blur`: `input` and `mid` are `Vec<f32>` of 4 channels |
| 16384² (over app limit, engine only) | bridged filter (Curves, Noise, ...): f32 `Image` in and out | 8192 MiB | `filters/bridge.rs::render` |
| 16384² (over app limit, engine only) | Liquify commit: f32 source + f32 field + f32 output | 10240 MiB | `liquify_tool.rs::commit` |
| 16384² (over app limit, engine only) | transform open: f32 source image + f32 footprint mask; commit: + layer, rendered, output | 5120 MiB / 17408 MiB | `selection_tool.rs::TransformTool::start` / `eskiz_select::session::commit` |
| 16384² (over app limit, engine only) | CPU display: flattened RGBA8 + egui `ColorImage` + GPU texture | 3072 MiB | `app.rs::sync_full` |
| 16384² (over app limit, engine only) | GPU mirror per layer (tiles) + compositor output | 1024 MiB + 1024 MiB | `gpu_canvas.rs`: every layer's tiles are duplicated on the GPU; on a unified-memory Mac that is system RAM too |

### 8b. Peak resident memory per scenario (this process)

| scenario | peak RSS | RSS after | wall time s | run |
|---|---|---|---|---|
| paint | 1969 MiB (16384² airbrush stroke; per-row peaks in table 1) | - | ~690 | first process (killed later by the memory cgroup during `composite 2048² 100 full`, see the notes) |
| composite | 2220 MiB | 455 MiB | 283.3 | one process per scenario |
| display | 2565 MiB | 482 MiB | 126.7 | one process per scenario |
| adjust | 1159 MiB | 309 MiB | 94.9 | one process per scenario |
| ops | 1454 MiB | 240 MiB | 194.8 | one process per scenario |
| undo | 1510 MiB | 1345 MiB | 373.9 | one process per scenario |
| files | 2803 MiB | 397 MiB | 617.8 | one process per scenario |
| adjust | 3763 MiB | 507 MiB | 120.4 | 8000² adjustments, `--no-gpu --budget-fraction 0.8` |

### 9. Drag interactions: Pour Fill threshold drag, Edit Shape drag, Frames timeline (rc5)

`eskiz-bench --only interact --sizes 2048,4096 --no-gpu` (release, 2 cores, no GPU). One row is one step of the drag on the thread that draws the window; the display refresh that follows is the same before and after and is not included. Run on the 2-core cloud server, so a laptop is about twice as fast.

| canvas | interaction | median ms | p95 ms |
|---|---|---|---|
| 4096² | Pour Fill threshold step, **before** the change (fill + write + a layer clone on the UI thread, measured before rc5) | 150 to 171 | 190 |
| 4096² | same, synchronous path today (the final exact fill on release still uses it): the write skips tiles that did not change | 119.5 | 148.9 |
| 4096² | Pour Fill step **after**: plan on the UI thread | 0.006 | 0.008 |
| 4096² | Pour Fill step **after**: write the finished fill into the layer (UI thread, once per finished fill; only changed tiles are copied) | 18.2 | 19.6 |
| 4096² | Pour Fill step **after**: the flood fill itself (worker thread, newest request wins) | 95.2 | 106.0 |
| 4096² | Edit Shape drag frame, **before** (undo the stroke + re-rasterize a r = n/4 ellipse with a 30 px brush) | 149.8 | 172.4 |
| 4096² | Edit Shape drag frame, **after** (vector outline for the preview) | 0.050 | 0.060 |
| 4096² | Edit Shape release (rasterize once, per drag not per frame) | 144.8 | 219.0 |
| 2048² | Pour Fill step before / after (plan + write on the UI thread) | 22.5 / 3.5 | 23.5 / 3.6 |
| 2048² | Edit Shape drag frame before / after | 74.1 / 0.055 | 114.0 / 0.071 |

Cause and fix. Pour Fill: every 1% of threshold (2 px of pointer movement) re-ran `flood_fill` on the layer, cloned it, and wrote the whole region on the UI thread. Now `Session::pour_fill_plan` snapshots the layer (a few `Arc` clones), `PourPlan::compute` runs on a worker thread (`PourPreviewer`, one job at a time, newest request wins), and `pour_fill_apply` writes the result over the preview already shown, so `Layer::write_rect` leaves tiles that did not change alone (no copy-on-write, no new memory). Releasing makes one exact fill at the final threshold. Edit Shape: every pointer move undid the shape's stroke and painted a new one; now the drag only moves a vector outline, and the shape is painted once on release, as the same single "Stroke" undo step.

B9, the Frames timeline toggle (test `b9_toggling_the_timeline_renders_no_thumbnail_on_the_ui_thread`, 1536 x 1024, 4 frames, unoptimised test build): the first frame after turning the timeline on composited the Frames canvas view (488 ms in the test build; 19 ms in the QA round's measurement) and two full-canvas thumbnails on the UI thread. Now the first view and all thumbnails are composited on worker threads and the regular canvas stays on screen until the view arrives: the UI thread spends 0.02 ms (frame list) + 0.10 ms (canvas view hand-off) + 0.02 ms (start thumbnails) on the toggle.

Not measured here (no Mac with a real GPU): the stroke path with `ESKIZ_TRACE_STROKES=1`, frame pacing (present mode, repaint during strokes, tile uploads per frame, any CPU composite fallback) and `eskiz-bench --quick` on a real GPU. Run them on the maintainer's Mac and add the numbers here.

## 4. Ranked bottlenecks

**Update for 0.1: all twelve are fixed or addressed** (#9 partly). The `Status` line under each heading gives the measured result; the original analysis below is kept as measured before the work. The after-numbers come from a 2-vCPU x86 box with a software GPU, so they are not comparable to the table in section 3 digit for digit.

Ranked by how much of the large-canvas experience each one costs, weighting what happens on
every stroke or frame above what happens once per operation. Line numbers are on commit `6f4de28`.
Estimated gains are for this x86 machine; multi-threading gains scale with the M3's 8 cores.

### 1. The CPU compositor is single-threaded, per-pixel `f64`, and generic over every case

- **Numbers.** Full recomposite of one fully painted layer: 139 ms at 2048², 576 ms at 4096², 2.7 s at 8000², 10.3 s at 16384² (table 2). 20 full layers: 1.4 s at 2048², 6.2 s at 4096²; with blend modes and groups 2.6 s and 9.3 s; 100 layers at 2048²: 7.5 s. That is 30 to 55 million layer-pixels per second (18 to 34 ns per layer-pixel). A 1024² dirty rectangle of a 20-full-layer document costs 370 to 380 ms. Every `Refresh::Everything` on the CPU display path pays this (layer property change, reorder, undo of a layer op, opening a document, ending an adjustment); the GPU path still pays it for thumbnails on save (`storage.rs:518`), merge down, flatten and every export.
- **Cause.** `crates/eskiz-core/src/composite.rs:59-93` walks 128² blocks on one thread and allocates a fresh `vec![bg; bw * bh]` per block; `draw_span` (`:330-353`) calls `straight()` (two divisions, `fin01` per channel) and `over()` (`:421-436`, four `fin01`, a `mask_at` lookup and a branch per pixel) for every pixel of every span, in `f64`, whatever the blend mode, opacity or mask. Nothing in this file is parallel.
- **Fix.** (a) Composite blocks in parallel with `std::thread::scope` (the crate already does this in `filters/gaussian.rs:126`); blocks are independent. (b) A fast path for the overwhelmingly common span (Normal, no mask): premultiplied source-over on `u8`/`f32` without unpremultiply and re-clamp, written as a tight loop over the span slice so it vectorises. (c) Reuse one block buffer per thread. (d) For `composite_rect` of a dirty rectangle, pre-resolve the layer list once instead of per block (`resolve` + the `clipped` `Vec` at `:256-260` run per block).
- **Gain.** ~1.9× from two threads here, 4 to 6× on an M3; 2 to 4× from the fast path. Together: 4096² one layer 576 ms → 50 to 80 ms; 20 full layers 6.2 s → under 1 s; the 1024² dirty rect of 20 layers 375 ms → ~40 ms, which is what makes a 20-layer 4096² document paintable on the CPU path at all.

### 2. Pen-up replays the whole stroke: coverage rebuilt from zero, every dab re-splatted, whole rectangle recomposited

- **Numbers.** `end_stroke` costs 131 ms (ink) / 814 ms (airbrush) at 2048², 356 ms / 1.7 s at 4096², 1.1 / 3.9 s at 8000², 4.3 / 10.6 s at 16384² (table 1, "pen-up"); the textured brush is 2.5 s at 8000². It is the same order as the whole stroke took to paint (ink: 131 of 258 ms; airbrush: 814 of 2658 ms), so every stroke ends with a hitch that grows with the stroke's bounding box.
- **Cause.** `crates/eskiz-app/src/paint.rs:214-223`: for a non-wet brush, `end` replaces the coverage with a new canvas-sized `CoverageBuffer`, re-splats *all* final dabs and recomposites the union of every rectangle the stroke touched, unconditionally. The end taper only changes the last dabs, and the engine says which: `Stroke::finish` returns `StrokeEnd::tail_redraw` with the first changed index (`crates/eskiz-core/src/geometry.rs:1586-1602`), and the wet path already compares `finals == a.live_dabs` (`paint.rs:197`) to skip the replay entirely.
- **Fix.** Keep the live dabs for the dry path too; if `finish` changed nothing, return the command without any re-splat or recomposite; otherwise clear and re-splat only the tail's rectangle (dabs from `tail_redraw.from` on, plus the earlier dabs that overlap that rectangle) and recomposite that rectangle. Also drop the second canvas-sized allocation (`:218`).
- **Gain.** Pen-up goes from O(whole stroke) to O(tail), 10 to 100×: 1.7 s → ~20 ms for the 4096² airbrush, 4.3 s → ~50 ms for the 16384² ink stroke. The remaining pen-up cost is `Change::tiles_between` and `share_unchanged_in_rect`, both tile-count proportional.

### 3. A 500 px soft dab costs 16 ms, at every canvas size

- **Numbers.** Soft airbrush 500 px: 15.8 to 17 ms p50 and 21 to 23 ms p95 per dab-laying event, 25 to 47 ms max, at every size from 2048² to 16384² (table 1): twice the 8 ms target, so at 120 Hz input the stroke lags behind the pen and the display gets 30 to 40 updates per second (table 1b: 24 to 29 ms per event with the refresh). The textured 80 px brush is 0.5 to 1 ms and the wet 120 px brush 0.6 to 0.8 ms, so this is the dab area: ~64 ns per pixel of the dab's bounding box.
- **Cause.** Two full passes over the dab's bounding box per dab, both scalar and per pixel: `CoverageBuffer::splat_one_with` (`crates/eskiz-core/src/raster.rs:338-376`) calls `tip_value` (`:401-429`: rotate, bounds test, bilinear `ShapeTexture::sample`) for every pixel; then `Active::composite` (`crates/eskiz-app/src/paint.rs:284-312`) rebuilds a `PixelOpts` and calls `stroke_composite_pixel` per pixel through `update_rect`'s closure, which computes `(4.5 a).sin()` for every covered pixel even when `wet_edges == 0` (`crates/eskiz-core/src/compose.rs:66-71`) and calls `texturized_tone` per pixel (`paint.rs:297`). All in `f64`.
- **Fix.** (a) Skip the wet-edge `sin` when `wet_edges == 0` and hoist the grain-off case. (b) For the built-in round tips, a 1-D radial profile lookup instead of 2-D bilinear sampling (the rotation is irrelevant for a round tip). (c) `f32` math and a span-oriented inner loop (one tile row at a time, as `update_rect` already iterates) so it vectorises. (d) Split dabs wider than ~128 px across threads by rows. (e) Composite only the pixels whose coverage the dab changed (the dab's own rectangle, which it already does) but skip rows the tip did not reach.
- **Gain.** 3 to 5× from (a)-(c): 16 ms → 3 to 5 ms, under the target; (d) halves it again on this machine and quarters it on an M3.

### 4. Saving and loading `.procreate` builds a dense canvas buffer per layer and compresses one tile at a time with a slow LZ4

- **Numbers.** Save of 20 sparse layers: 2.1 s at 2048², 8.1 s at 4096², **29 s at 8000²**; the Gallery's autosave path (write, reopen to verify, thumbnail) 2.5 / 8.1 / 31 s; one full layer 0.39 / 1.5 / 5.3 s. Load: 0.45 / 2.1 / 7.4 s for 20 sparse, 0.06 / 0.25 / 1.4 s for one full layer (table 7). The sparse document has 20 × 12 MiB = 240 MiB of tiles at 8000² and saves 5.5× slower than the 244 MiB full layer: the cost is per canvas pixel per layer, not per stored tile. Autosave runs every 30 s while the document is dirty, on a worker thread: at 8000² the worker is busy essentially all the time and the save is still running when the next one is due.
- **Cause.** `crates/eskiz-app/src/storage.rs:496-499` flattens every layer with `Layer::to_rgba()` (a dense `w × h × 4` buffer, 256 MiB per layer at 8000², and the builder holds all of them at once; only the pages of allocated tiles become resident, but every page is then read); `crates/eskiz-format/src/document.rs:1534-1565` then copies each tile out of it, scans it byte by byte for all-zero, and compresses it; `crates/eskiz-format/src/codecs.rs:612-646` allocates and zeroes a `vec![0usize; 65536]` (512 KiB) hash table **per 64 KiB input chunk** and extends matches byte by byte, which is about 45 MB/s (5 to 6 ms per 256 KiB tile). The reader does the mirror image: `assemble_rgba` builds the dense buffer and `Layer::from_rgba` re-scans it (`crates/eskiz-app/src/import.rs:316-`). `Library::save` (`storage.rs:188-198`) then reopens and fully decodes the package (`verify_package`) and runs a full CPU composite for the thumbnail (`flatten_doc`, `:518`).
- **Fix.** Write from the layer's allocated tiles directly (`Layer::tile`, skipping `None`), no dense buffer and no zero scan; read tiles straight into `Tile`s with `Layer::set_tile`; compress and decompress tiles in parallel; in the encoder keep one hash table of `u32` (256 KiB) per thread, or use a fast LZ4 crate; verify by checking the ZIP directory and tile headers rather than decoding every tile; make the thumbnail from a downscaled composite.
- **Gain.** 10 to 30× for sparse documents (the normal case), 3 to 5× for full layers: the 8000² 20-layer autosave from 31 s to about 1 to 2 s.

### 5. Adjustments flatten the whole layer, run single-threaded, and the bridged types convert the whole layer to `f32` twice

- **Numbers.** Per slider change (preview) at 4096²: HSB 0.44 s, Curves 1.4 s, Gaussian 0.86 to 0.93 s; at 8000²: HSB 2.3 s, Curves 6.6 s, Gaussian 2.8 to 3.8 s. Apply at 8000²: Gaussian 64 taps 8.3 s, Curves 6.3 s with a **4.2 GiB** peak, Gaussian 3.2 GiB (table 4). Only Gaussian has a cheaper preview (a proxy); HSB and Curves render the full layer for every slider change.
- **Cause.** `crates/eskiz-app/src/adjust.rs:212` copies the layer to a dense buffer on entry and every render produces another; `crates/eskiz-core/src/filters/hsb.rs:50-64` is one thread doing a YUV rotation per pixel (~30 ns/px); `crates/eskiz-core/src/filters/bridge.rs:21-27` converts the whole layer to an `eskiz_filters::Image` of `f32` (16 B/px) and back for Curves and every other bridged type, so a 256-entry lookup table costs 6 s and 2 GiB of `f32` at 8000²; `crates/eskiz-core/src/filters/gaussian.rs:112-` keeps `input` and `mid` as `Vec<f32>` (32 B/px, 2 GiB at 8000²) and caps its threads at 8 but keeps the two conversions serial.
- **Fix.** Render previews on a proxy no larger than the viewport for every type (as Liquify's grid and Gaussian's `preview_scale` already do), apply at full resolution only on commit and off the UI thread (already a worker for heavy kinds, `adjust.rs:362-376`); thread the per-pixel filters by rows (rayon is already a dependency of `eskiz-filters`); run Curves and the other LUT-style filters on the `u8` data directly, tile by tile; for the bridged types convert and filter one tile row at a time rather than the whole layer.
- **Gain.** Previews 10 to 50× (slider changes at 8000² from 2 to 7 s to under 0.2 s); apply 2 to 4× from threads; peak memory from 4.2 GiB to under 1 GiB, which is what makes 8000² adjustments possible on 8 GB at all.

### 6. The transform preview resamples the whole canvas at full resolution, single-threaded, for every gesture frame

- **Numbers.** One preview render: 0.48 s at 2048², 2.1 s at 4096²; commit 0.78 / 3.2 s; start (flatten to `f32`) 92 / 379 ms (table 5). At 8000² the transform needs ~4.3 GiB and was skipped: `TransformSession` holds the source as `f32` (16 B/px), a `f32` footprint mask (4 B/px), and commit adds the layer, the rendered image and the output at 16 B/px each (table 8).
- **Cause.** `crates/eskiz-select/src/resample.rs:23-45` samples every destination pixel of the canvas (bicubic, 16 taps) on one thread, even where the transformed quad is not; `crates/eskiz-app/src/selection_tool.rs:256` builds the session from a full `f32` image; `crates/eskiz-select/src/session.rs:169-197` renders and composites three more full-canvas `f32` images on commit.
- **Fix.** Resample only the bounding box of the transformed quad; parallel rows; bilinear (or a proxy) while the finger is down and bicubic once on commit; keep the source as RGBA8 tiles and convert per tile.
- **Gain.** Preview 20 to 50× (2.1 s → under 60 ms at 4096²); memory 4× lower, so 8000² transforms fit.

### 7. Flood fill walks one pixel at a time through a tile lookup

- **Numbers.** A near-whole-canvas Pour Fill on an almost empty layer: 0.39 s at 2048², 1.4 s at 4096², 5.0 s at 8000², 6.4 s at 8192² (table 5): ~80 ns per pixel plus a byte of state and a `u32` of membership per canvas pixel (320 MiB at 8000²).
- **Cause.** `crates/eskiz-app/src/floodfill.rs:106-139`: a stack of single pixels, each visit calling the `px(n)` closure (a `Layer::pixel` tile lookup) and pushing into `members` / `ring` vectors; the bounding box is then recomputed by iterating every member, and `read(rect)` copies the rectangle.
- **Fix.** Scanline span fill over tile rows (`Layer::row_spans`), treating an unallocated tile as one uniform span (an empty layer, the common Pour Fill target, is then one span per tile row), with the bounding box tracked as spans are added.
- **Gain.** 5 to 10×; the whole-canvas fill at 8000² from 5 s to about 0.5 s.

### 8. Undo depth collapses with canvas size

- **Numbers.** A 500 px airbrush stroke step holds 24 / 62 / 124 MiB at 2048² / 4096² / 8000²; a full-layer adjustment step 44 / 159 / 512 MiB. With the 256 MiB cap, 20 mixed strokes leave 20 steps at 2048², 10 at 4096² and **4 at 8000²** (table 6); a single 8000² filter step is twice the cap. Undo and redo themselves are free (tile pointer swaps, under 20 µs).
- **Cause.** `crates/eskiz-app/src/history.rs:95-96`: `DEFAULT_MAX_STEPS = 30`, `DEFAULT_MAX_BYTES = 256 MiB`, independent of canvas size and of the machine; `Session::replace_layer` (`session.rs:536`) stores whole before/after layers for a filter.
- **Fix.** Scale the byte cap with the canvas and available memory (e.g. 4 to 8 full layers' worth, bounded by a fraction of physical RAM); compress idle undo tiles with LZ4 on a background thread (the tile data is exactly what the file format stores); for filter steps keep only the before tiles (the after is the live layer until the next change).
- **Gain.** 8000² documents keep 20 to 30 steps instead of 4; no speed cost on the undo itself.

### 9. The GPU path doubles the document's memory and uploads through per-tile staging copies (hardware-independent)

- **Status: Partly fixed: tile upload reads tile bytes directly; `read_rect` + `upload_rect` about 10% less. The GPU mirror still doubles the document's memory, and the real GPU cost is not measured on this software adapter.**

- **Numbers.** The first flush of a document stages every tile: 20 sparse layers at 8000² take 2.2 s on this software adapter (table 2); building the GPU mirror of 100 full 2048² layers (1.6 GiB) was what pushed the first run over the 5.84 GiB cgroup limit (tiles + mirror + staging ≈ 4.8 GiB). On a unified-memory Mac the mirror is system RAM too.
- **Cause.** `crates/eskiz-app/src/gpu_canvas.rs:135-169` (`sync_layer`) reads each tile back out of the CPU layer (`read_rect`) and issues one `queue.write_texture` per tile (`crates/eskiz-gpu/src/tile.rs:277-`), each of which copies into a wgpu staging buffer that lives until the queue has run; nothing frees staging between layers. The mirror keeps every tile of every layer resident (`GpuLayer`), including the ones the stroke never touches. `supports()` (`gpu_canvas.rs:102-115`) sends any document with a group that has opacity, a blend mode or a mask to the CPU path, which is bottleneck 1.
- **Fix.** Upload from the tile's own `Arc<Tile>` bytes (no `read_rect` copy), batch tiles of one layer into one `write_texture` per tile row or use a staging belt that is submitted and reused per layer; consider evicting GPU tiles of layers that are far from the view or not visible; finish group opacity/blend/mask in the GPU compositor so large documents never fall back to the CPU display.
- **Gain.** Upload time 2 to 3× lower (CPU side), transient memory down by the staging size (≈ tile bytes); the real GPU composite cost cannot be judged here.

### 10. The CPU display path converts and uploads the whole canvas on every "Everything" refresh

- **Status: Fixed: the separate RGBA8-to-`Color32` pass (65 to 72 ms at 4096²) is gone; skipping unchanged regions of an "Everything" refresh was left out on purpose.**

- **Numbers.** On top of the composite (bottleneck 1): `ColorImage::from_rgba_unmultiplied` 76 ms at 4096², 266 ms at 8000²; the egui texture upload 62 ms and 277 ms; 0.74 s and 2.9 s total per full refresh of one layer (table 3). Partial refreshes are fine (8 to 12 ms for 512² at every size). The canvas is drawn from one texture with linear filtering and no mipmaps (`app.rs:38`, `TEX_OPTIONS`), so at fit-to-screen an 8000² canvas is minified 6× by point sampling (aliasing, and on a real GPU a cache-unfriendly fetch pattern; on llvmpipe the quad costs 17 to 38 ms per frame at every zoom, which is not representative).
- **Cause.** `crates/eskiz-app/src/app.rs:600-612` (`sync_full`) composites, converts straight RGBA8 to `Color32` (a second full pass with a premultiply per pixel) and `TextureHandle::set`s the whole canvas; `:617-632` (`refresh`) does the same per rectangle, which is why the partial path is cheap.
- **Fix.** Produce premultiplied `Color32` directly from the compositor (it has premultiplied `f32` pixels in hand: `composite.rs:81-87` unpremultiplies them only to have egui premultiply again); upload only the rectangles that changed even after an "Everything" refresh by diffing tile epochs; add a mip chain or a downscaled copy for zoomed-out views (`egui` `TextureOptions` has `mipmap_mode`).
- **Gain.** Full refresh 2 to 3× cheaper (the conversion pass disappears), and at 8000² the 2.9 s becomes the composite alone; correct minification.

### 11. Stroke buffers are canvas-sized, and allocated twice per stroke

- **Status: Fixed: coverage buffers are lazily allocated 64x64 chunks. Peak RSS 8000² ink 464 → 381 MiB, airbrush 565 → 490 MiB.**

- **Numbers.** `CoverageBuffer` is `f32` per canvas pixel: 64 MiB at 4096², 244 MiB at 8000², 1 GiB at 16384²; a wet brush adds a `3 × f32` delta buffer: 192 / 732 MiB / 3 GiB (table 8). Peak RSS of one stroke at 16384²: 1.8 GiB (ink), 2.0 GiB (airbrush); the wet brush was skipped at 16384² (6 GiB). The pen-up rebuild allocates a second coverage buffer while the first is alive.
- **Cause.** `crates/eskiz-app/src/paint.rs:135` (`CoverageBuffer::new(0, 0, width, height)`) and `:218` (again at pen-up); `crates/eskiz-core/src/wet.rs:196` (`delta: vec![0.0; w * h * 3]`). The pages are zero-mapped lazily, so only the stroke's bounding box becomes resident, but a stroke across the canvas touches it all, and the "begin" cost of the first stroke after a document opens (7 to 9 ms at 2048², page faults) comes from here.
- **Fix.** Size the coverage and delta buffers to the stroke's bounding box and grow them as the stroke extends (`CoverageBuffer` already has an origin and `get` returns 0 outside it), or store coverage in sparse tiles like the layer.
- **Gain.** Per-stroke memory from canvas-sized to stroke-sized (a 500 px wide stroke across 8000² is ~15% of the canvas); no second buffer at pen-up (with bottleneck 2's fix).

### 12. Liquify upscales its 1-megapixel proxy to the full canvas on every frame

- **Status: Fixed: the warped proxy is shown at proxy size. Preview step 4096² 798 → 62 ms, 8000² 2459 → 239 ms; enter 8000² 1278 → 165 ms; commit 8000² 6270 → 4870 ms.**

- **Numbers.** One brush step at 4096²: simulation 23 ms, preview warp + upscale 454 ms; at 8000²: 29 ms and 1.8 s, and entering the mode costs 1.0 s (table 4). Commit: 1.1 s / 5.1 s, holding 40 B/px (2.4 GiB at 8000²).
- **Cause.** `crates/eskiz-app/src/liquify_tool.rs:297-310` (`view`) warps the proxy and then `upscale_rgba8` (`:84-100`) writes a full-resolution RGBA8 canvas for `Session::set_preview` every frame; `:144-146` builds the proxy from a `to_rgba` of the whole layer; `:314-326` (`commit`) materialises `f32` source, field and output at full resolution.
- **Fix.** Display the warped proxy at the proxy's size (upload the small image and let the GPU scale it under the view transform); upsample the field, not the image, and warp tile by tile on commit.
- **Gain.** Per-frame cost from 1.8 s to ~30 ms at 8000²; commit memory 3× lower.

## 5. Verdict: which canvas sizes feel interactive

**Update for 0.1: the bottlenecks behind this verdict are fixed** (see section 4). Short version after the fixes,: at 4096² a full composite of one layer is 113 ms and of 20 sparse layers 133 ms, pen-up is under 20 ms for dry brushes, adjustment sliders update in 80 to 200 ms, transform preview frames take 129 to 234 ms, flood fill 134 ms and a 20-layer save 0.6 s; at 8000² pen-up is 12 to 25 ms, adjustment previews 0.2 s, and a transform now runs. Remaining weak spots: full composites of many fully painted layers (20 full layers at 2048² still 255 ms, 1086 ms with mixed blends and groups), wet-brush pen-up (0.3 to 0.8 s) and the 8000² transform commit (6 s). The text below is the verdict as it stood before the fixes.

On this machine (one 2.1 GHz Xeon core per task, software GPU). An M3 is roughly 2 to 3× faster
per thread, so divide the CPU figures by that; the memory figures do not change.

- **2048² (4 Mpx):** interactive. Every brush except the 500 px airbrush paints at well under 1 ms per event, the airbrush at 16 ms per dab (sluggish but usable). Full recomposites take 110 to 140 ms on the CPU path (a visible hitch on every layer-property change and on the end of every adjustment) and are fine on the GPU path. Pen-up hitches of 130 to 800 ms are noticeable. Adjustment sliders update in 0.1 to 0.4 s. Saves take 0.4 to 2.5 s (background). Undo keeps 20 steps.
- **4096² (16.7 Mpx):** painting with small and medium brushes stays interactive (0.08 ms per event; 0.16 ms with the CPU display refresh; textured and wet brushes under 1 ms); the 500 px airbrush is at 16 ms per dab and the pen-up hitch is 0.4 to 1.7 s. Everything whole-canvas is not interactive: 0.4 to 0.6 s for a full recomposite of 1 to 20 sparse layers on the CPU (6 s for 20 fully painted layers), 0.4 to 1.4 s per adjustment slider change, 2.1 s per transform preview frame, 1.4 s flood fill, 1.5 to 8 s saves. Undo keeps 10 strokes. With bottlenecks 1 to 3 fixed, 4096² would be comfortable; today it is "paint, but don't touch the layers panel or the adjustments while you wait".
- **8000² (64 Mpx, the app's maximum) and 8192²:** stroke events are still cheap (0.15 ms; 0.25 ms with the refresh), so a small brush on the GPU path can feel fine while the stroke is in progress; everything else is seconds: pen-up 1 to 4 s, full recomposite 1.6 to 2.7 s (CPU), adjustment previews 2 to 7 s with 3 to 4 GiB peaks, flood fill 5 s, saves 5 to 30 s with a 30 s autosave interval, loads 1.4 to 7.4 s, 4 undo steps, and the transform cannot run within 4 GiB. Not practical today; with the ranked fixes it becomes a "large but workable" size on a 16 GB machine and marginal on 8 GB (§6).
- **16384² (268 Mpx):** 1 GiB per fully painted layer, 10 s per full recomposite, 4 to 11 s pen-up, 2 GiB for one stroke, 6 GiB for one wet stroke, 8 GiB for one Gaussian blur. Not feasible on 8 GB whatever the code does; the Gallery is right to refuse it.

## 6. What this means for an 8 GB Apple M3

The numbers below are derived from the measured bytes per layer and the working-set multipliers
in table 8, not measured on a Mac. Assume macOS, the window, egui, fonts and the brush library
leave about 5 GB for documents and working sets before the system starts compressing memory, and
that memory pressure (not speed) is what ends a session.

- **Memory per layer, as the app stands.** A fully painted 8-bit layer costs its tiles twice on the GPU path (CPU tiles + GPU mirror, both in unified memory): 128 MiB at 4096², 488 MiB at 8000². A sparse layer (a few strokes) costs 12 to 24 MiB at 4096² and 24 MiB at 8000². An undo history adds up to 256 MiB.
- **Working sets of single operations.** Gaussian blur or Curves: 32 B/px (0.5 GiB at 4096², 2 GiB at 8000², peaks measured at 1.2 and 4.2 GiB including the layer copies); transform: 20 B/px open, 68 B/px to commit (1.1 GiB measured at 4096², 4.3 GiB at 8000²); Liquify commit 40 B/px (2.4 GiB at 8000²); a wet stroke 16 B/px (0.7 GiB at 8000²); a `.procreate` save of 20 layers materialises 4 B/px per layer in turn.
- **Max practical canvas and layer count, as the app stands.**
  - 4096²: about 20 fully painted layers (2.5 GiB with the GPU mirror) plus the 1.2 GiB transform or 0.5 GiB blur working set fits in 5 GB. Sparse documents (the normal case) can have 100+ layers.
  - 8000²: 4 to 5 fully painted layers (2.0 to 2.4 GiB) leave room for a blur or Curves apply (3 to 4 GiB peak) only just; a transform does not fit at any layer count; 20 to 30 sparse layers are fine for painting, but every adjustment is a memory gamble. Treat 8000² as "a few layers, no transform, no Curves" on 8 GB.
  - 16384²: not feasible (1 GiB per layer, 2 GiB per stroke).
- **With the fixes in §4** (tile-wise filters and transforms, proxy previews, stroke-sized buffers, a scaled undo cap): working sets drop to about one extra layer, and 8000² with 8 to 10 fully painted layers (or dozens of sparse ones) becomes practical on 8 GB; 4096² with 30+ full layers likewise. 16384² stays out of reach on 8 GB.

## 7. What could not be measured, and why

- **Real GPU timings.** The only adapter is llvmpipe (software Vulkan). Every GPU row in tables 1b, 2, 3 and 4 shows the rasteriser's cost plus the CPU side of the GPU path; the GPU compositor, GPU adjustments and the canvas quad at the three zoom levels (17 to 38 ms per frame here) need a hardware adapter to judge. The CPU-side components (uploads, staging memory, tile counts, fallbacks) are the only GPU conclusions drawn.
- **Group opacity, blend mode and mask on the GPU path.** `gpu_canvas::supports` falls back to the CPU display for such documents; the "20 full, mixed blend + groups" document used Normal groups at opacity 1 so the GPU rows exist.
- **20 and 100 fully painted layers at 8000² and above, and 100 at 4096².** They need 5, 21, 24 and 6.3 GiB of tiles; the process is limited to 5.84 GiB. 20 fully painted 4096² layers (1.25 GiB) ran on the CPU but not on the GPU path (3.9 GiB with mirror and staging).
- **Wet brush at 16384²** (6 GiB for coverage plus delta buffer on top of the layer), the **transform at 8000² and above** (4.3 GiB), **PSD export of 20 layers at 8000²** (5.6 GiB of per-layer buffers), and **GPU adjustments at 8000²** (the 8000² adjustment run had to be `--no-gpu` to fit): skipped with the memory they would need.
- **Adjustments, layer operations, undo and files at 16384²**: not run; the app cannot create that size and the scaling is already visible from 8000² to 8192². The 8192² adjustments were not re-run after the OOM kill; they are within 2.4% of the pixel count of 8000².
- **The actual window frame**: egui layout, the event loop, vsync and stylus input latency end to end. There is no display here; the display path was driven headlessly through `egui_wgpu::Renderer` on the same device, which measures the texture upload and the quad draw but not the frame pacing of `eframe`.
- **16-bit documents**: not benchmarked; their tiles are exactly twice the bytes and the 16-bit paint loop goes through `f64` as the 8-bit one does.
- **Apple M3 numbers**: §6 is an estimate from the measured bytes, not a measurement.
- **Two runs were OOM-killed** before the harness read the cgroup limit (§1.1); their completed rows were kept (table 1 and 1b come from the first process) and the rest re-ran one scenario per process. The first process's peak RSS per scenario is therefore in table 1's per-row column rather than in table 8b.

## 8. Reproducing

```sh
cd eskiz
cargo run --release -p eskiz-bench -- --quick        # about a minute on a laptop, 3 minutes on this machine
cargo run --release -p eskiz-bench                   # the full set, hours on this machine
cargo run --release -p eskiz-bench -- --only adjust --sizes 8000 --no-gpu --budget-fraction 0.8
```

The report is written to `target/bench-report.md` after every row. See `crates/eskiz-bench/README.md`
for the options and the scenario list.
