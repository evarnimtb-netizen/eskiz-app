# eskiz-bench

A large-canvas performance benchmark for Eskiz. It is a measuring instrument only: it drives
the engine crates and `eskiz-app`'s own `Session`, `GpuCanvas`, adjustment, Liquify, transform,
undo and export code headlessly, the way the window does, and reports numbers. It does not
change the app.

Results and the ranked list of bottlenecks are in [`../../BENCHMARKS.md`](../../BENCHMARKS.md).

## Running

Always use a release build; the pixel loops are meaningless unoptimised.

```sh
cd eskiz
cargo run --release -p eskiz-bench -- --quick   # about a minute on a laptop (3 min on a 2-core server)
cargo run --release -p eskiz-bench              # full: 2048², 4096², 8000², 8192², 16384²
```

Options:

| option | meaning |
|---|---|
| `--quick` | 2048² for every scenario, 4096² for the stroke, compositing and display measurements; two timed iterations; no 100-layer document |
| `--sizes 2048,4096,...` | canvas edges to run (any list) |
| `--only paint,composite,display,adjust,ops,undo,files,interact` | run a subset of the scenarios |
| `--out PATH` | where to write the Markdown report (default `target/bench-report.md`) |
| `--no-gpu` | skip every GPU and display measurement |
| `--budget-fraction 0.55` | the share of available memory a scenario's pixel data may take before it is skipped |

The report is printed as it goes (one Markdown table per scenario) and rewritten to
`target/bench-report.md` after every row, so an out-of-memory kill still leaves the rows so far.

## What it measures

Sizes: 2048², 4096², 8000² (the largest canvas the Gallery allows: 8192 px per side and 64
megapixels, so 8192² itself is refused), then 8192² and 16384² against the engine crates only.
A scenario whose pixel data would not fit the memory budget is reported as `skipped: needs ~N GiB`
instead of thrashing (the budget is a fraction of `MemAvailable`, capped by the memory cgroup's
headroom when the process runs in one). Everything above 8192² runs only painting, compositing
and memory. Each scenario frees what it allocated and resets the peak-RSS counter, so the peaks
in table 8b are per scenario; if a run is OOM-killed anyway, run the scenarios one process at a
time with `--only`.

1. **Painting**: one 2000-point stylus stroke per brush (hard ink, 500 px soft airbrush, a
   textured dry brush, a wet watercolor, and the smudge tool) through
   `Session::begin_stroke` / `push` / `end_stroke`; per-event p50 / p95, dabs/s, pen-down and
   pen-up cost, peak RSS. Then the same with the display refresh included, on the CPU path
   (`flatten_rect_display` + `ColorImage` + egui partial texture upload) and the GPU path
   (`GpuCanvas::mark_rect` + `flush`).
2. **Compositing**: full canvas and 256² / 1024² dirty rectangles, CPU (`eskiz_core::composite`)
   and GPU (`GpuCanvas::flush`), for 1 full layer, 20 sparse, 20 full, 100 full and 20 full
   with mixed blend modes, clipping and groups.
3. **Display**: the full and partial egui texture upload of the CPU path, the GPU path's native
   texture, and the textured canvas quad drawn at fit-to-screen, 100% and 400% into a
   2560x1440 target through `egui_wgpu::Renderer`.
4. **Adjustments**: Gaussian blur (20 and 64 taps), Hue/Saturation/Brightness, Curves and
   Liquify on a full layer at preview and apply quality, split into `to_rgba`, render and
   `replace_layer`; the GPU shaders for the first three.
5. **Layer operations**: duplicate, merge down, flatten, clear, flood fill, a full-layer
   scale + rotate transform.
6. **Undo**: undo / redo of a big stroke and of a full-layer filter; history growth over 10 to
   20 strokes.
7. **Files**: `.procreate` save and load, the Gallery's verified save, PSD and PNG export, with
   output sizes.
8. **Memory**: tile bytes per layer (sparse and full), the working buffers each path allocates,
   and the peak RSS of every scenario.
9. **Interactions** (`--only interact`): one step of the Pour Fill threshold drag (the UI-thread plan and write, and the worker-thread fill) and one frame of an Edit Shape drag (re-rasterize versus the vector preview).

Inputs are deterministic: fixed pattern seeds, fixed stroke paths, fixed brush seeds. Every
measurement runs once to warm up, then 2 (quick) or 5 (full) times; slow measurements get
fewer iterations. Tables show median and p95 (and min where useful).

GPU timings are only meaningful on real hardware. On a machine whose only adapter is a software
rasteriser (llvmpipe) the report says so in its preamble and those rows show the rasteriser's
cost plus the CPU-side work of the GPU path.

## Tests

`cargo test -p eskiz-bench` runs one 300 x 300 smoke test (fixtures build, a stroke paints, the
report renders). The real sizes are never part of `cargo test`.
