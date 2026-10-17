//! Device, queue and adapter selection.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

/// Errors from the GPU backend.
#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    /// No usable adapter (hardware or software) exists on this machine.
    #[error("no usable GPU adapter is available: {0}")]
    Unavailable(String),
    /// The adapter exists but the device could not be created.
    #[error("GPU device creation failed: {0}")]
    Device(String),
    /// A rectangle or tile lies outside the layer, or a buffer has the wrong size.
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// A GPU buffer could not be mapped for reading.
    #[error("GPU readback failed: {0}")]
    Readback(String),
}

/// Whether the selected adapter is real hardware or a CPU rasteriser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdapterKind {
    /// Discrete, integrated or virtual GPU.
    Hardware,
    /// Software adapter (llvmpipe, lavapipe, WARP, SwiftShader).
    Software,
}

/// Options for [`GpuContext::with_options`].
#[derive(Clone, Copy, Debug)]
pub struct GpuOptions {
    /// Fall back to a software adapter when no hardware adapter works. Default true.
    pub allow_software: bool,
    /// Ignore hardware adapters and take the software one. For testing. Default false.
    pub force_software: bool,
}

impl Default for GpuOptions {
    fn default() -> Self {
        Self {
            allow_software: true,
            force_software: false,
        }
    }
}

/// A wgpu device and queue with the limits Eskiz needs.
pub struct GpuContext {
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub kind: AdapterKind,
    pub info: wgpu::AdapterInfo,
}

struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Minimal single-future executor, so the crate needs no async runtime.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => thread::park(),
        }
    }
}

fn adapter_kind(info: &wgpu::AdapterInfo) -> AdapterKind {
    if info.device_type == wgpu::DeviceType::Cpu {
        AdapterKind::Software
    } else {
        AdapterKind::Hardware
    }
}

/// Minimum capabilities the shaders rely on.
fn adapter_ok(adapter: &wgpu::Adapter) -> bool {
    let l = adapter.limits();
    adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
        && l.max_storage_buffers_per_shader_stage >= 4
        && l.max_storage_textures_per_shader_stage >= 1
        && l.max_texture_array_layers >= 64
        && l.max_texture_dimension_2d >= 4096
}

fn rank(info: &wgpu::AdapterInfo) -> u32 {
    match info.device_type {
        wgpu::DeviceType::DiscreteGpu => 4,
        wgpu::DeviceType::IntegratedGpu => 3,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 1,
        wgpu::DeviceType::Cpu => 0,
    }
}

/// Sort key for adapter choice, larger is better: hardware outranks software, and Metal and Vulkan
/// outrank the rest on ties. The one place the ranking lives (`--diagnose` reports through it too).
fn preference(info: &wgpu::AdapterInfo) -> (u32, u32) {
    let backend_pref = match info.backend {
        wgpu::Backend::Metal => 3,
        wgpu::Backend::Vulkan => 2,
        wgpu::Backend::Dx12 => 1,
        _ => 0,
    };
    (rank(info), backend_pref)
}

/// What `eskiz --diagnose` reports about one adapter: its identity, the limits the compositor
/// checks, whether it passes them, and whether a device could be created on it.
#[derive(Clone, Debug)]
pub struct AdapterProbe {
    pub info: wgpu::AdapterInfo,
    pub compute_shaders: bool,
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_storage_textures_per_shader_stage: u32,
    pub max_texture_array_layers: u32,
    pub max_texture_dimension_2d: u32,
    /// Whether [`GpuContext`] would consider the adapter at all (compute shaders and the limits above).
    pub meets_requirements: bool,
    /// The sort key [`GpuContext::new`] ranks by, larger first.
    pub preference: (u32, u32),
    /// `Ok` if a device was created with the adapter's own limits, else the error.
    pub device: Result<(), String>,
}

/// Enumerate the adapters of the given backends and try to create a device on each. Does not read
/// `WGPU_BACKEND`: the caller names the backends. Used by `eskiz --diagnose`.
pub fn probe_adapters(backends: wgpu::Backends) -> Vec<AdapterProbe> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    let instance = wgpu::Instance::new(desc);
    block_on(instance.enumerate_adapters(backends))
        .into_iter()
        .map(|adapter| {
            let l = adapter.limits();
            let compute_shaders = adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
            let info = adapter.get_info();
            let desc = wgpu::DeviceDescriptor {
                label: Some("eskiz-gpu probe"),
                required_features: wgpu::Features::empty(),
                required_limits: l.clone(),
                ..Default::default()
            };
            let device = block_on(adapter.request_device(&desc)).map(|_| ()).map_err(|e| e.to_string());
            AdapterProbe {
                preference: preference(&info),
                meets_requirements: adapter_ok(&adapter),
                compute_shaders,
                max_storage_buffers_per_shader_stage: l.max_storage_buffers_per_shader_stage,
                max_storage_textures_per_shader_stage: l.max_storage_textures_per_shader_stage,
                max_texture_array_layers: l.max_texture_array_layers,
                max_texture_dimension_2d: l.max_texture_dimension_2d,
                info,
                device,
            }
        })
        .collect()
}

impl GpuContext {
    /// Hardware adapter if there is one, otherwise a software adapter.
    pub fn new() -> Result<Self, GpuError> {
        Self::with_options(GpuOptions::default())
    }

    /// Create a context with explicit options. Blocks while the device is created.
    pub fn with_options(opts: GpuOptions) -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        // `WGPU_BACKEND` (for example `dx12` or `vulkan,gl`) narrows the choice, as it does for the
        // eframe window, so one setting steers both. Unset means every backend.
        let mut adapters = block_on(instance.enumerate_adapters(wgpu::Backends::all().with_env()));
        adapters.retain(|a| {
            let kind = adapter_kind(&a.get_info());
            adapter_ok(a)
                && match kind {
                    AdapterKind::Hardware => !opts.force_software,
                    AdapterKind::Software => opts.allow_software || opts.force_software,
                }
        });
        // Best first. Hardware outranks software; Metal and Vulkan outrank the rest on ties.
        adapters.sort_by_key(|a| std::cmp::Reverse(preference(&a.get_info())));
        if adapters.is_empty() {
            return Err(GpuError::Unavailable(
                "no adapter with compute shader support was found".into(),
            ));
        }
        let mut last_err = String::new();
        for adapter in adapters {
            let info = adapter.get_info();
            let desc = wgpu::DeviceDescriptor {
                label: Some("eskiz-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                ..Default::default()
            };
            match block_on(adapter.request_device(&desc)) {
                Ok((device, queue)) => {
                    return Ok(Self {
                        kind: adapter_kind(&info),
                        info,
                        adapter,
                        device,
                        queue,
                    });
                }
                Err(e) => last_err = format!("{}: {e}", info.name),
            }
        }
        Err(GpuError::Device(last_err))
    }

    /// Wrap a device that already exists, e.g. the one eframe creates when its `wgpu`
    /// feature is on (`egui_wgpu::RenderState` holds `adapter`, `device` and `queue`; clone the
    /// handles, they are reference counted). The device may use wgpu's default limits: the
    /// shaders need 4 storage buffers, 2 storage textures and 4096 px textures per stage.
    pub fn from_parts(
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Self {
        let info = adapter.get_info();
        Self {
            kind: adapter_kind(&info),
            info,
            adapter,
            device,
            queue,
        }
    }

    /// Largest canvas edge this device can composite: its 2D texture limit.
    pub fn max_canvas_edge(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Whether the device can run the compositor and adjustments: compute shaders and the
    /// limits the shaders rely on. A device that eframe created with default limits passes;
    /// a GLES 3.0 device without compute shaders does not.
    pub fn supports_compositing(&self) -> bool {
        let l = self.device.limits();
        self.adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
            && l.max_storage_buffers_per_shader_stage >= 4
            && l.max_storage_textures_per_shader_stage >= 1
            && l.max_texture_array_layers >= 64
            && l.max_texture_dimension_2d >= 4096
    }

    /// Human-readable adapter description, e.g. `Apple M2 (Metal, hardware)`.
    pub fn describe(&self) -> String {
        format!(
            "{} ({:?}, {})",
            self.info.name,
            self.info.backend,
            match self.kind {
                AdapterKind::Hardware => "hardware",
                AdapterKind::Software => "software",
            }
        )
    }

    /// Hand the uploads queued by `Queue::write_texture` to the GPU now and let the driver
    /// retire the ones that finished, without waiting. wgpu keeps a staging copy of every
    /// queued write until the next submission completes, so a long run of tile uploads calls
    /// this now and then to keep the staging memory bounded.
    pub fn flush_uploads(&self) {
        self.queue.submit(std::iter::empty());
        let _ = self.device.poll(wgpu::PollType::Poll);
    }

    /// Block until all submitted work has finished.
    pub fn wait_idle(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Read a buffer that was copied into with `MAP_READ` usage. Blocks.
    pub(crate) fn read_buffer(&self, buf: &wgpu::Buffer, size: u64) -> Result<Vec<u8>, GpuError> {
        let slice = buf.slice(..size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .to_vec();
        buf.unmap();
        Ok(data)
    }
}
