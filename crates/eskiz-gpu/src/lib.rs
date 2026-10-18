//! eskiz-gpu: headless wgpu backend for Eskiz.
//!
//! * [`GpuContext`]: adapter selection (hardware first, software fallback) and device.
//! * [`GpuLayer`] / [`GpuMask`]: sparse 256x256 tiled textures.
//! * [`Compositor`]: composites a [`Scene`] (layers, groups, clipping, masks, 26 blend modes)
//!   into an output texture, optionally only for dirty tiles.
//! * [`adjust`]: GPU versions of the `eskiz-filters` adjustments.

mod context;
mod layer;
mod scene;
mod tile;
pub mod adjust;
pub mod compositor;

pub use adjust::{Adjuster, Adjustment};
pub use compositor::{CompositeStats, Compositor, Region, BATCH_TILES};
pub use context::{block_on, bounded_wait, GPU_WAIT_BOUND, probe_adapters, AdapterKind, AdapterProbe, GpuContext, GpuError, GpuOptions};
pub use eskiz_core::Depth;
pub use layer::{f16_to_unorm16, unorm16_to_f16, GpuLayer, GpuMask};
pub use scene::{mode_index, BlendMode, Scene, SceneGroup, SceneItem, SceneLayer};
pub use tile::{Rect, TileCoord, TILE};
// Parameter types of the adjustments, so callers need not depend on `eskiz-filters` directly.
pub use eskiz_filters::gaussian::GaussianParams;
pub use eskiz_filters::hsb::HsbParams;
pub use eskiz_filters::opacity::OpacityParams;
