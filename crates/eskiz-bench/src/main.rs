//! `eskiz-bench`: a large-canvas performance benchmark for Eskiz.
//!
//! It drives the engine crates and `eskiz-app`'s own `Session`, `GpuCanvas`, adjustment, Liquify,
//! transform and export paths headlessly, at canvas sizes from 2048² up to the largest the app
//! allows and beyond, and writes `target/bench-report.md`. It measures and reports only; it
//! changes nothing in the app. See the crate README.
//!
//! ```text
//! cargo run --release -p eskiz-bench -- --quick     # sizes up to 4096², about a minute
//! cargo run --release -p eskiz-bench                # full: 2048², 4096², 8000² (app max), 8192², 16384²
//! cargo run --release -p eskiz-bench -- --sizes 2048,4096 --only paint,composite
//! ```

mod adjust;
mod composite;
mod ctx;
mod files;
mod fixtures;
mod gfx;
mod ops;
mod paint;
mod report;

use std::time::Instant;

use ctx::Ctx;
use report::{gib, mem_available, mem_total, mib, reset_peak, rss, Iters, Report};

const USAGE: &str = "\
eskiz-bench [--quick] [--sizes 2048,4096,...] [--only paint,composite,display,adjust,ops,undo,files]
            [--out PATH] [--no-gpu] [--budget-fraction 0.55]

  --quick    sizes 2048² and 4096², fewer iterations (about a minute on a laptop)
  default    2048², 4096², 8000² (the app's largest canvas), 8192² and 16384² (engine only)";

struct Args {
    quick: bool,
    sizes: Vec<usize>,
    only: Vec<String>,
    out: Option<String>,
    no_gpu: bool,
    budget_fraction: f64,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { quick: false, sizes: Vec::new(), only: Vec::new(), out: None, no_gpu: false, budget_fraction: 0.55 };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--quick" => a.quick = true,
            "--no-gpu" => a.no_gpu = true,
            "--sizes" => {
                let v = it.next().ok_or("--sizes needs a list")?;
                a.sizes = v.split(',').map(|s| s.trim().parse::<usize>().map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
            }
            "--only" => {
                let v = it.next().ok_or("--only needs a list")?;
                a.only = v.split(',').map(|s| s.trim().to_owned()).collect();
            }
            "--out" => a.out = Some(it.next().ok_or("--out needs a path")?),
            "--budget-fraction" => a.budget_fraction = it.next().ok_or("--budget-fraction needs a number")?.parse().map_err(|e: std::num::ParseFloatError| e.to_string())?,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            s => return Err(format!("unknown option {s}\n{USAGE}")),
        }
    }
    if a.sizes.is_empty() {
        a.sizes = if a.quick { vec![2048, 4096] } else { vec![2048, 4096, 8000, 8192, 16384] };
    }
    Ok(a)
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_owned()))
        .unwrap_or_else(|| "unknown".into())
}

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("PRETTY_NAME=")).map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_owned()))
        .unwrap_or_else(|| std::env::consts::OS.into())
}

fn git(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}

fn target_dir() -> std::path::PathBuf {
    // target/release/eskiz-bench -> target/
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()))
        .unwrap_or_else(|| "target".into())
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("eskiz-bench: {e}");
            std::process::exit(2);
        }
    };
    let out = args.out.clone().map_or_else(|| target_dir().join("bench-report.md"), std::path::PathBuf::from);
    let mut report = Report::new(&out, true);

    // Machine and build.
    let total = mem_total();
    let avail = mem_available();
    let budget = (avail as f64 * args.budget_fraction) as u64;
    let gpu = if args.no_gpu { None } else { eskiz_gpu::GpuContext::new().ok() };
    let gpu_desc = gpu.as_ref().map_or("none (no adapter with compute shaders)".to_owned(), |g| {
        format!("{} ; driver {} {} ; max texture edge {} px", g.describe(), g.info.driver, g.info.driver_info, g.max_canvas_edge())
    });
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    report.preamble.push(format!("- date: {}", chrono_free_now()));
    report.preamble.push(format!("- commit: `{}` {}", git(&["rev-parse", "--short", "HEAD"]), git(&["log", "-1", "--format=%s"])));
    report.preamble.push(format!("- build: {} (`opt-level` {}), {}", if cfg!(debug_assertions) { "debug" } else { "release" }, if cfg!(debug_assertions) { "dev" } else { "3" }, rustc_version()));
    report.preamble.push(format!("- OS: {}, {} {}", os_name(), std::env::consts::OS, std::env::consts::ARCH));
    report.preamble.push(format!("- CPU: {} ; {} hardware threads", cpu_model(), threads));
    report.preamble.push(format!(
        "- memory: {} total, {} available at start{}; the benchmark skips a scenario whose pixel data would exceed {} ({:.0}% of available)",
        gib(total),
        gib(avail),
        report::cgroup_headroom().map_or(String::new(), |h| format!(" (the process runs in a memory cgroup with {} of headroom, which is what the OOM killer enforces)", gib(h))),
        gib(budget),
        args.budget_fraction * 100.0
    ));
    report.preamble.push(format!("- GPU adapter: {gpu_desc}"));
    if gpu.as_ref().is_some_and(|g| g.kind == eskiz_gpu::AdapterKind::Software) {
        report.preamble.push("- **The GPU adapter is a software rasteriser: GPU timings show that rasteriser's cost and the CPU-side driver work, not a real GPU. Conclusions about the GPU path must come from the CPU-side numbers (uploads, submission, tile counts) and from real hardware.**".into());
    }
    report.preamble.push(format!("- mode: {}; sizes {}", if args.quick { "quick" } else { "full" }, args.sizes.iter().map(|s| format!("{s}²")).collect::<Vec<_>>().join(", ")));
    report.preamble.push(format!("- app canvas limit: {}² per side and {} megapixels (`eskiz_app::storage::MAX_SIDE`, `gallery::parse_custom_size`), so the largest square canvas the app creates is 8000² (64.0 Mpx); 8192² is refused by the Gallery and runs here against the engine crates only", eskiz_app::storage::MAX_SIDE, 64));
    report.preamble.push("- all timings are wall-clock on the main thread, median and p95 over the iterations shown after one warm-up run; inputs are deterministic (fixed seeds)".into());
    report.write();

    let display = gpu.as_ref().map(gfx::Display::new);
    let mut cx = Ctx {
        iters: if args.quick { Iters::quick() } else { Iters::full() },
        sizes: args.sizes.clone(),
        app_max_side: eskiz_app::storage::MAX_SIDE,
        app_max_area: 64_000_000,
        gpu,
        display,
        report,
        budget,
        quick: args.quick,
    };

    let want = |name: &str| args.only.is_empty() || args.only.iter().any(|o| o == name);
    type Scenario = fn(&mut Ctx);
    let scenarios: Vec<(&str, Scenario)> = vec![
        ("paint", paint::run),
        ("composite", composite::run),
        ("display", composite::display),
        ("adjust", adjust::run),
        ("ops", ops::layer_ops),
        ("undo", ops::undo),
        ("files", files::run),
    ];
    let mut peaks = Vec::new();
    let t_all = Instant::now();
    for (name, f) in scenarios {
        if !want(name) {
            continue;
        }
        reset_peak();
        let t0 = Instant::now();
        f(&mut cx);
        let (now, peak) = rss();
        peaks.push((name.to_owned(), peak, now, t0.elapsed().as_secs_f64()));
        cx.log(&format!("{name} done in {:.1} s, peak RSS {}", t0.elapsed().as_secs_f64(), mib(peak)));
    }
    memory(&mut cx, &peaks);
    cx.report.note(&format!("total benchmark wall time: {:.0} s", t_all.elapsed().as_secs_f64()));
    cx.report.write();
    println!("\nreport written to {}", out.display());
}

/// Scenario 8: memory. Measured tile bytes per layer, the working buffers each path allocates,
/// and the peak RSS of every scenario, plus what the numbers mean for an 8 GB laptop.
fn memory(cx: &mut Ctx, peaks: &[(String, u64, u64, f64)]) {
    let t = cx.report.table("8. Memory: bytes per layer and per working buffer", &["canvas", "item", "bytes", "notes"]);
    cx.report.intro(t, "Tiles are 256x256 RGBA8 (256 KiB each; 512 KiB at 16 bit) and only allocated where a layer has pixels. The working buffers are what one operation allocates on top of the layers, from the code paths named.");
    for &n in &cx.sizes.clone() {
        let px = (n * n) as u64;
        let full = fixtures::layer_bytes(n);
        let sparse = if cx.fits(fixtures::layer_bytes(n) / 4).is_ok() {
            let mut l = eskiz_core::Layer::new(n, n);
            let doc = fixtures::document(n, fixtures::Layout::SparseN(1));
            if let Some(src) = doc.layer(fixtures::first_layer(&doc)) {
                l = src.clone();
            }
            Some((l.tile_bytes() as u64, l.tile_count()))
        } else {
            None
        };
        cx.report.row(t, &[cx.size_label(n), "one fully painted 8-bit layer".into(), mib(full), format!("{} tiles; 16-bit: {}", px.div_ceil(65536), mib(full * 2))]);
        if let Some((b, tiles)) = sparse {
            cx.report.row(t, &[cx.size_label(n), "one sparse layer (three ink strokes)".into(), mib(b), format!("{tiles} tiles allocated")]);
        }
        cx.report.row(t, &[cx.size_label(n), "stroke in progress: coverage buffer (f32 per canvas pixel)".into(), mib(px * 4), "`PaintEngine::begin_paths` -> `CoverageBuffer::new(0, 0, width, height)` (paint.rs); allocated zeroed, so pages are touched only where the stroke goes, and rebuilt again at pen-up".into()]);
        cx.report.row(t, &[cx.size_label(n), "wet stroke in progress: color delta buffer (3 x f32 per pixel)".into(), mib(px * 12), "`WetStroke::new` (wet.rs), on top of the coverage buffer".into()]);
        cx.report.row(t, &[cx.size_label(n), "adjustment open: flat RGBA8 copy of the layer".into(), mib(px * 4), "`adjust.rs::enter_adjustment_mode` `layer.to_rgba()`; preview and apply each produce another".into()]);
        cx.report.row(t, &[cx.size_label(n), "Gaussian blur working set (f32 bands of 32 rows + kernel reach, per thread)".into(), mib(std::thread::available_parallelism().map_or(1, |c| c.get().min(8)) as u64 * 8000 * 4 * 4 * (32 + 126)), "`filters/gaussian.rs::blur`: a band at a time, so the f32 working set does not grow with the canvas height (worst case shown: 64 taps, 8000 px wide)".into()]);
        cx.report.row(t, &[cx.size_label(n), "bridged filter: f32 `Image` in and out (Noise, Sharpen, Halftone, Bloom, Clone, motion/perspective blur, ...)".into(), mib(px * 32), "`filters/bridge.rs::render_whole`; Curves, Gradient Map, Recolor and Color Balance run in 64-row strips (a few MiB per thread) instead".into()]);
        cx.report.row(t, &[cx.size_label(n), "Liquify commit: f32 source + f32 field + f32 output".into(), mib(px * 40), "`liquify_tool.rs::commit`".into()]);
        cx.report.row(t, &[cx.size_label(n), "transform open: RGBA8 source (+ f32 selection when there is one); commit: + output + new layer".into(), format!("{} / {}", mib(px * 4), mib(px * 12)), "`selection_tool.rs::TransformTool::start` / `eskiz_select::session::commit_premul`".into()]);
        cx.report.row(t, &[cx.size_label(n), "CPU display: flattened RGBA8 + egui `ColorImage` + GPU texture".into(), mib(px * 12), "`app.rs::sync_full`".into()]);
        cx.report.row(t, &[cx.size_label(n), "GPU mirror per layer (tiles) + compositor output".into(), format!("{} + {}", mib(full), mib(px * 4)), "`gpu_canvas.rs`: every layer's tiles are duplicated on the GPU; on a unified-memory Mac that is system RAM too".into()]);
    }
    let t2 = cx.report.table("8b. Peak resident memory per scenario (this process)", &["scenario", "peak RSS", "RSS after", "wall time s"]);
    for (name, peak, now, secs) in peaks {
        cx.report.row(t2, &[name.clone(), mib(*peak), mib(*now), format!("{secs:.1}")]);
    }
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".into())
}

/// `YYYY-MM-DD HH:MM UTC` without a date crate.
fn chrono_free_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = secs / 86400;
    let (h, m) = ((secs % 86400) / 3600, (secs % 3600) / 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02} UTC")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny end-to-end smoke test at 300 x 300: fixtures build, a stroke paints and the report
    /// renders. Keeps `cargo test` fast; the real sizes run from the binary.
    #[test]
    fn smoke() {
        let n = 300;
        let doc = fixtures::document(n, fixtures::Layout::SparseN(2));
        assert_eq!(doc.layer_count(), 2);
        let mixed = fixtures::document(n, fixtures::Layout::MixedFull20);
        assert_eq!(mixed.layer_count(), 20);
        let mut session = eskiz_app::session::Session::new(fixtures::document(n, fixtures::Layout::OneFull));
        let id = fixtures::first_layer(&session.doc);
        let brush = &fixtures::brushes()[0];
        assert!(session.begin_stroke(id, &brush.settings));
        let path = fixtures::stroke_path(n, 50);
        let mut painted = false;
        for s in &path {
            painted |= session.push(*s).is_some();
        }
        assert!(session.end_stroke().is_some() || painted);
        let out = std::env::temp_dir().join(format!("eskiz-bench-smoke-{}.md", std::process::id()));
        let mut r = Report::new(&out, false);
        let t = r.table("t", &["a", "b"]);
        r.row(t, &["1".into(), "2".into()]);
        assert!(r.markdown().contains("| 1 | 2 |"));
        let _ = std::fs::remove_file(out);
        assert!(chrono_free_now().len() >= 16);
    }
}
