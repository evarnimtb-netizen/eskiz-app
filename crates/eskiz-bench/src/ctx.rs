//! Shared benchmark state: sizes, iteration counts, the GPU, the memory budget and the report.

use eskiz_gpu::GpuContext;

use crate::gfx::Display;
use crate::report::{gib, Iters, Report};

pub struct Ctx {
    pub iters: Iters,
    /// Canvas edges to run, in pixels.
    pub sizes: Vec<usize>,
    /// The largest canvas edge the app's Gallery offers (`eskiz_app::storage::MAX_SIDE`).
    pub app_max_side: usize,
    /// The app's area limit in pixels (`parse_custom_size`).
    pub app_max_area: usize,
    pub gpu: Option<GpuContext>,
    pub display: Option<Display>,
    pub report: Report,
    /// Pixel bytes a scenario may allocate before it is skipped instead of thrashing.
    pub budget: u64,
    pub quick: bool,
}

impl Ctx {
    /// A second handle on the same device, for a `GpuCanvas` (which takes its context by value).
    pub fn gpu_handle(&self) -> Option<GpuContext> {
        self.gpu.as_ref().map(|g| GpuContext::from_parts(g.adapter.clone(), g.device.clone(), g.queue.clone()))
    }

    /// Let the device free what was dropped (wgpu destroys resources on a poll).
    pub fn gpu_cleanup(&self) {
        if let Some(g) = &self.gpu {
            g.wait_idle();
        }
    }

    /// `Err("skipped: needs ~N GiB")` when `bytes` of pixel data would not fit the budget.
    pub fn fits(&self, bytes: u64) -> Result<(), String> {
        if bytes > self.budget {
            Err(format!("skipped: needs ~{} (budget {})", gib(bytes), gib(self.budget)))
        } else {
            Ok(())
        }
    }

    /// Canvases far beyond the app's limit run only the painting, compositing and memory
    /// scenarios (the app cannot create them; the rest would only repeat the scaling).
    pub fn reduced(&self, n: usize) -> bool {
        n > 8192
    }

    /// In `--quick` mode only the stroke, compositing and display measurements run above 2048²;
    /// the rest of the scenarios scale the same way and would push the run well past a minute.
    pub fn quick_skip(&self, n: usize) -> bool {
        self.quick && n > 2048
    }

    /// Whether a canvas edge is one the app itself would create.
    pub fn app_allows(&self, n: usize) -> bool {
        n <= self.app_max_side && n * n <= self.app_max_area
    }

    /// `"4096² (app max 8000²: over the app limit, engine only)"` and the like.
    pub fn size_label(&self, n: usize) -> String {
        if self.app_allows(n) {
            format!("{n}²")
        } else {
            format!("{n}² (over app limit, engine only)")
        }
    }

    pub fn log(&self, msg: &str) {
        eprintln!("[bench] {msg}");
    }
}
