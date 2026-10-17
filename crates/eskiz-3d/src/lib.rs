//! eskiz-3d: CPU, headless 3D painting core.
//!
//! Pipeline:
//!
//! 1. [`load`] reads glTF/GLB or OBJ into a [`Model`] and runs the model checks
//!    ([`check`]), returning a [`CheckReport`].
//! 2. [`OrbitCamera`] gives view and projection matrices.
//! 3. [`render_lookup`] rasterises the model into a [`LookupBuffer`]: per pixel a
//!    16-bit normalised (u, v) pair, depth and triangle id.
//! 4. [`PaintSession`] maps eskiz-core screen-space [`eskiz_core::Dab`]s through the
//!    lookup, scales them by local UV density, stamps them into the texture-space
//!    layer with eskiz-core's raster, and dilates painted texels into the gutter.
//! 5. [`shade`] is a CPU reference of the GGX preview shader.
//! 6. [`export`] writes a GLB with embedded PNG textures, a USDZ package ([`usd`]), plus per-map PNGs.
//!
//! UV convention: `(0, 0)` is the top-left texel corner and `v` grows downward, so
//! texel position is `(u * width, v * height)` with no flip. The OBJ loader flips
//! `v`; glTF already uses this convention.

#![allow(unknown_lints, clippy::chunks_exact_to_as_chunks)]

pub mod camera;
pub mod check;
pub mod export;
pub mod layers;
pub mod load;
pub mod lookup;
pub mod model;
pub mod paint;
pub mod shade;
pub mod usd;

pub use camera::OrbitCamera;
pub use check::{CheckReport, FatalIssue, Warning};
pub use export::{
    export_glb, export_map_pngs, export_usdz, write_glb_bytes, write_usdz_bytes, ExportError, UsdzOptions,
};
pub use usd::{write_usda, RootFormat};
#[cfg(feature = "usd")]
pub use usd::import::{load_usd_file, load_usd_slice, load_usdz_slice};
pub use layers::{model_layer_groups, texture_set_layers, LayerGroup, LayerNode, BASE_GROUP_NAME};
pub use load::{
    load_gltf, load_gltf_slice, load_obj, load_obj_str, ImportError, ImportOptions, ImportedModel,
};
pub use lookup::{render_lookup, Hit, LookupBuffer, RenderOptions, Side, SurfaceBuffer};
pub use model::{MapKind, Model, SubMesh, TextureSet, UvMask};
pub use paint::{restore_texels, TexelChange,
    dilate_gutter, map_dabs, MappedDab, PaintSession, PaintSettings, DEFAULT_DILATION,
};
pub use shade::{
    apply_normal_map, encode_output, shade, tangent_frame, Light, SurfaceInput, MAX_LIGHTS,
    ROUGHNESS_FLOOR,
};
