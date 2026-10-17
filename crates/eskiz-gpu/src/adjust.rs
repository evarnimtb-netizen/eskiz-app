//! GPU versions of the `eskiz-filters` adjustments.
//!
//! # Which adjustments
//!
//! Gaussian blur (vertical pass, then horizontal, with the exact `eskiz-filters` weights),
//! HSB, Opacity, Curves (256-entry LUTs), Gradient Map, Sharpen and Noise. Liquify, Bloom,
//! Halftone and the other filters stay on the CPU for now.
//!
//! # Data flow
//!
//! 1. The source layer's tiles are copied (GPU to GPU) into a dense canvas-sized RGBA8
//!    texture. A texel is `byte / 255`, the same input the CPU reference gets from
//!    `Image::from_rgba8_premul`.
//! 2. Compute shaders write the adjusted image into an `Rgba16Float` working texture. Half
//!    float keeps working memory at 8 bytes per pixel (128 MiB for 4096x4096) and costs well
//!    under 1/255 of error. The Gaussian uses a second working texture between its passes.
//! 3. A final pass mixes `src -> adjusted` by the mask (`mix_masked`) and stores
//!    RGBA8, which rounds to nearest like the CPU `to_rgba8_premul`.
//! 4. The result tiles are copied into a **new** [`GpuLayer`]. Only tiles that can be
//!    non-empty are allocated: the source tiles, one ring more for the Gaussian, every tile
//!    for Noise in mix mode.
//!
//! # Tolerance
//!
//! Against the CPU references the result is within 1/255 per channel for Gaussian, HSB,
//! Opacity, Curves, Gradient Map and Sharpen. Noise is within 2/255, except multichannel
//! noise (sample origins up to 4100, where an f32 ulp is 5e-4), which may differ by up to
//! about 12/255 on a few percent of bytes. See `tests/adjust.rs`.

use crate::context::{GpuContext, GpuError};
use crate::layer::{GpuLayer, GpuMask};
use crate::tile::{TileCoord, TILE};
use eskiz_filters::curves::{CurveSet, CurvesParams};
use eskiz_filters::gaussian::{self, GaussianParams};
use eskiz_filters::gradient_map::GradientMapParams;
use eskiz_filters::hsb::{self, HsbParams};
use eskiz_filters::noise::{self, CombineMode, NoiseKind, NoiseParams};
use eskiz_filters::opacity::OpacityParams;
use eskiz_filters::sharpen::SharpenParams;
use std::collections::BTreeSet;

const KIND_HSB: u32 = 1;
const KIND_OPACITY: u32 = 2;
const KIND_CURVES: u32 = 3;
const KIND_GRADIENT_MAP: u32 = 4;
const KIND_SHARPEN: u32 = 5;
const KIND_NOISE: u32 = 6;

/// Longest gradient-map table the shader accepts.
pub const MAX_TABLE_LEN: usize = 1024;

/// One adjustment with its parameters (curves are boxed: their 256-entry tables are large). The parameter types come from `eskiz-filters`, so
/// the UI drives CPU and GPU paths with the same values.
#[derive(Clone, Debug)]
pub enum Adjustment {
    Gaussian(GaussianParams),
    Hsb(HsbParams),
    Opacity(OpacityParams),
    Curves(Box<CurvesParams>),
    GradientMap(GradientMapParams),
    Sharpen(SharpenParams),
    Noise(NoiseParams),
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    dims: [u32; 4],
    iu: [u32; 4],
    f0: [f32; 4],
    f1: [f32; 4],
    f2: [f32; 4],
    f3: [f32; 4],
}

/// Everything the shaders need for one adjustment.
struct Job {
    params: Params,
    lut: Vec<[f32; 4]>,
    perm: Vec<u32>,
    gaussian: bool,
}

fn curve_set_into(lut: &mut Vec<[f32; 4]>, set: &CurveSet) {
    // Each curve is 256 floats = 64 vec4. Order: master, r, g, b.
    for c in [&set.master, &set.red, &set.green, &set.blue] {
        for chunk in c.table.chunks(4) {
            lut.push([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
    }
}

fn build_job(adj: &Adjustment) -> Result<Job, GpuError> {
    let mut p = Params {
        f3: [0.0, 0.0, 0.0, 1.0],
        ..Default::default()
    };
    let mut lut: Vec<[f32; 4]> = Vec::new();
    let mut perm: Vec<u32> = Vec::new();
    let mut gaussian = false;
    match adj {
        Adjustment::Gaussian(g) => {
            let w = gaussian::weights(g.amount);
            gaussian = true;
            p.iu[0] = w.len() as u32;
            for chunk in w.chunks(4) {
                let mut v = [0.0f32; 4];
                v[..chunk.len()].copy_from_slice(chunk);
                lut.push(v);
            }
        }
        Adjustment::Hsb(h) => {
            p.dims[3] = KIND_HSB;
            let theta = std::f32::consts::PI * hsb::wrap_hue(h.hue);
            let (s, c) = theta.sin_cos();
            p.f0 = [
                c,
                s,
                1.0 + h.saturation.clamp(-1.0, 1.0),
                h.brightness.clamp(-1.0, 1.0),
            ];
        }
        Adjustment::Opacity(o) => {
            p.dims[3] = KIND_OPACITY;
            p.f0[0] = o.amount.clamp(0.0, 1.0);
        }
        Adjustment::Curves(c) => {
            p.dims[3] = KIND_CURVES;
            p.f0[0] = c.blend;
            curve_set_into(&mut lut, &c.target);
            match &c.blend_source {
                Some(src) => {
                    p.iu[1] = 1;
                    curve_set_into(&mut lut, src);
                }
                None => curve_set_into(&mut lut, &CurveSet::identity()),
            }
        }
        Adjustment::GradientMap(g) => {
            p.dims[3] = KIND_GRADIENT_MAP;
            if g.start.is_empty()
                || g.target.is_empty()
                || g.start.len() > MAX_TABLE_LEN
                || g.target.len() > MAX_TABLE_LEN
            {
                return Err(GpuError::Invalid(format!(
                    "gradient map tables need 1..={MAX_TABLE_LEN} entries"
                )));
            }
            p.iu[1] = g.start.len() as u32;
            p.iu[2] = g.target.len() as u32;
            p.f0[0] = g.blend;
            p.f3[3] = g.amount.clamp(0.0, 1.0);
            for c in g.start.iter().chain(&g.target) {
                lut.push([c[0], c[1], c[2], 0.0]);
            }
        }
        Adjustment::Sharpen(s) => {
            p.dims[3] = KIND_SHARPEN;
            p.f0[0] = s.amount;
        }
        Adjustment::Noise(n) => {
            p.dims[3] = KIND_NOISE;
            let count = noise::octave_count(n.octaves);
            let (lo, hi) = noise::normalisation_range(n.kind, count);
            p.iu = [
                count as u32,
                u32::from(n.combine == CombineMode::Mix),
                match n.kind {
                    NoiseKind::Signed => 0,
                    NoiseKind::Absolute => 1,
                    NoiseKind::Billowy => 2,
                },
                u32::from(n.multichannel),
            ];
            p.f0 = [
                noise::sample_scale(n.scale),
                n.amount.clamp(0.0, 1.0),
                n.turbulence,
                lo,
            ];
            p.f1[0] = hi;
            perm = noise::permutation(n.seed).iter().map(|&b| u32::from(b)).collect();
        }
    }
    if lut.is_empty() {
        lut.push([0.0; 4]);
    }
    if perm.is_empty() {
        perm.push(0);
    }
    Ok(Job {
        params: p,
        lut,
        perm,
        gaussian,
    })
}

/// Tiles that can hold non-zero output for `adj`, given the source tiles.
fn result_tiles(adj: &Adjustment, src: &BTreeSet<TileCoord>, tx: u32, ty: u32) -> BTreeSet<TileCoord> {
    let all = || -> BTreeSet<TileCoord> {
        (0..ty).flat_map(|y| (0..tx).map(move |x| (x, y))).collect()
    };
    match adj {
        Adjustment::Gaussian(g) => {
            // Taps reach at most 63 px, so one ring of tiles is enough.
            let r = if gaussian::tap_count(g.amount) > 1 { 1i64 } else { 0 };
            let mut out = BTreeSet::new();
            for &(x, y) in src {
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (nx, ny) = (i64::from(x) + dx, i64::from(y) + dy);
                        if nx >= 0 && ny >= 0 && nx < i64::from(tx) && ny < i64::from(ty) {
                            out.insert((nx as u32, ny as u32));
                        }
                    }
                }
            }
            out
        }
        Adjustment::Noise(n) if n.combine == CombineMode::Mix && n.amount > 0.0 => all(),
        _ => src.clone(),
    }
}

struct Work {
    w: u32,
    h: u32,
    a_view: wgpu::TextureView,
    b: Option<(wgpu::Texture, wgpu::TextureView)>,
    out: wgpu::Texture,
    out_view: wgpu::TextureView,
}

fn tex(
    ctx: &GpuContext,
    label: &str,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> (wgpu::Texture, wgpu::TextureView) {
    let t = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
    (t, v)
}

/// Runs adjustments on [`GpuLayer`]s. Keeps its pipelines and canvas-sized working textures
/// between calls.
pub struct Adjuster {
    bgl: wgpu::BindGroupLayout,
    p_gauss_v: wgpu::ComputePipeline,
    p_gauss_h: wgpu::ComputePipeline,
    p_adjust: wgpu::ComputePipeline,
    p_finish: wgpu::ComputePipeline,
    dummy_in: wgpu::TextureView,
    dummy_dst: wgpu::TextureView,
    dummy_mask: wgpu::TextureView,
    work: Option<Work>,
}

impl Adjuster {
    pub fn new(ctx: &GpuContext) -> Self {
        let dev = &ctx.device;
        let module = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("eskiz adjust"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/adjust.wgsl").into()),
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let storage_tex = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let storage_buf = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("eskiz adjust bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64),
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                storage_tex(4, wgpu::TextureFormat::Rgba16Float),
                storage_tex(5, wgpu::TextureFormat::Rgba8Unorm),
                storage_buf(6),
                storage_buf(7),
            ],
        });
        let layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("eskiz adjust layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipe = |entry: &str| {
            dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let (_, dummy_in) = tex(
            ctx,
            "adjust dummy in",
            1,
            1,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let (_, dummy_dst) = tex(
            ctx,
            "adjust dummy dst",
            1,
            1,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::STORAGE_BINDING,
        );
        let (_, dummy_mask) = tex(
            ctx,
            "adjust dummy mask",
            1,
            1,
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        Self {
            p_gauss_v: pipe("gauss_v"),
            p_gauss_h: pipe("gauss_h"),
            p_adjust: pipe("adjust"),
            p_finish: pipe("finish"),
            bgl,
            dummy_in,
            dummy_dst,
            dummy_mask,
            work: None,
        }
    }

    fn work(&mut self, ctx: &GpuContext, w: u32, h: u32, need_b: bool) -> &mut Work {
        let fresh = self.work.as_ref().is_none_or(|k| k.w != w || k.h != h);
        if fresh {
            let (_, a_view) = tex(
                ctx,
                "adjust a",
                w,
                h,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            );
            let (out, out_view) = tex(
                ctx,
                "adjust out",
                w,
                h,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            );
            self.work = Some(Work {
                w,
                h,
                a_view,
                b: None,
                out,
                out_view,
            });
        }
        let work = self.work.as_mut().expect("work exists");
        if need_b && work.b.is_none() {
            work.b = Some(tex(
                ctx,
                "adjust b",
                w,
                h,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            ));
        }
        work
    }

    /// Apply `adj` to `src`, mixing by `mask` (value `byte / 255`; `None` is 1 everywhere).
    /// The source layer is untouched; the result is a new layer of the same size.
    pub fn apply(
        &mut self,
        ctx: &GpuContext,
        src: &GpuLayer,
        mask: Option<&GpuMask>,
        adj: &Adjustment,
    ) -> Result<GpuLayer, GpuError> {
        let (w, h) = (src.width(), src.height());
        if src.depth() != eskiz_core::Depth::U8 {
            return Err(GpuError::Invalid("GPU adjustments work on 8-bit layers; 16-bit layers adjust on the CPU".into()));
        }
        if let Some(m) = mask {
            if m.store.width != w || m.store.height != h {
                return Err(GpuError::Invalid("mask size differs from the layer".into()));
            }
        }
        if w == 0 || h == 0 {
            return Ok(GpuLayer::new(w, h));
        }
        let job = build_job(adj)?;
        let src_tiles: BTreeSet<TileCoord> = src.store.map.keys().copied().collect();
        let out_tiles = result_tiles(adj, &src_tiles, w.div_ceil(TILE), h.div_ceil(TILE));
        if out_tiles.is_empty() {
            return Ok(GpuLayer::new(w, h));
        }

        let dev = &ctx.device;
        let has_mask = mask.is_some();
        // Mask default fill needs a queue write before the encoder runs.
        let gaussian = job.gaussian;
        // Split borrows: build the working set first, then take shared refs to pipelines.
        self.work(ctx, w, h, gaussian);

        let mut params = job.params;
        params.dims[0] = w;
        params.dims[1] = h;
        params.dims[2] = u32::from(has_mask);
        let params_buf = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("adjust params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&params_buf, 0, bytemuck::bytes_of(&params));
        let storage = |label, data: &[u8]| {
            let b = dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: data.len().max(16) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ctx.queue.write_buffer(&b, 0, data);
            b
        };
        let lut_buf = storage("adjust lut", bytemuck::cast_slice(&job.lut));
        let perm_buf = storage("adjust perm", bytemuck::cast_slice(&job.perm));

        let work = self.work.as_ref().expect("work exists");
        // Fresh (zero-initialised) dense inputs for every call; wgpu clears new textures lazily.
        let (src_tex, src_view) = tex(
            ctx,
            "adjust src",
            w,
            h,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let (mask_tex, mask_view_dense) = tex(
            ctx,
            "adjust mask",
            w,
            h,
            wgpu::TextureFormat::R8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );

        // Mask default value outside allocated mask tiles.
        if let Some(m) = mask {
            let byte = m.store.empty_byte;
            if byte != 0 {
                let row = vec![byte; w as usize];
                let data: Vec<u8> = (0..h).flat_map(|_| row.iter().copied()).collect();
                ctx.queue.write_texture(
                    mask_tex.as_image_copy(),
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("eskiz adjust"),
        });
        // Gather source tiles (and mask tiles) into the dense textures.
        let copy_tile = |enc: &mut wgpu::CommandEncoder,
                         from: &wgpu::Texture,
                         slice: u32,
                         to: &wgpu::Texture,
                         tc: TileCoord| {
            let (ox, oy) = (tc.0 * TILE, tc.1 * TILE);
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: from,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: slice },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: to,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: ox, y: oy, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: TILE.min(w - ox),
                    height: TILE.min(h - oy),
                    depth_or_array_layers: 1,
                },
            );
        };
        for (&tc, slot) in &src.store.map {
            copy_tile(
                &mut enc,
                &src.store.pages[slot.page as usize].texture,
                slot.slice,
                &src_tex,
                tc,
            );
        }
        if let Some(m) = mask {
            for (&tc, slot) in &m.store.map {
                copy_tile(
                    &mut enc,
                    &m.store.pages[slot.page as usize].texture,
                    slot.slice,
                    &mask_tex,
                    tc,
                );
            }
        }

        let mask_view = if has_mask { &mask_view_dense } else { &self.dummy_mask };
        let bind = |input: &wgpu::TextureView, dst: &wgpu::TextureView| {
            dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("eskiz adjust bg"),
                layout: &self.bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&src_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(mask_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(dst),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&work.out_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: lut_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: perm_buf.as_entire_binding(),
                    },
                ],
            })
        };
        let groups = (w.div_ceil(8), h.div_ceil(8), 1);
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("eskiz adjust"),
                timestamp_writes: None,
            });
            if gaussian {
                let (_, b_view) = work.b.as_ref().expect("gaussian allocates b");
                pass.set_pipeline(&self.p_gauss_v);
                pass.set_bind_group(0, Some(&bind(&self.dummy_in, b_view)), &[]);
                pass.dispatch_workgroups(groups.0, groups.1, groups.2);
                pass.set_pipeline(&self.p_gauss_h);
                pass.set_bind_group(0, Some(&bind(b_view, &work.a_view)), &[]);
                pass.dispatch_workgroups(groups.0, groups.1, groups.2);
            } else {
                pass.set_pipeline(&self.p_adjust);
                pass.set_bind_group(0, Some(&bind(&self.dummy_in, &work.a_view)), &[]);
                pass.dispatch_workgroups(groups.0, groups.1, groups.2);
            }
            pass.set_pipeline(&self.p_finish);
            pass.set_bind_group(0, Some(&bind(&work.a_view, &self.dummy_dst)), &[]);
            pass.dispatch_workgroups(groups.0, groups.1, groups.2);
        }

        // Scatter the result into a fresh layer.
        let mut result = GpuLayer::new(w, h);
        for &tc in &out_tiles {
            let slot = result.store.ensure(ctx, tc);
            let (ox, oy) = (tc.0 * TILE, tc.1 * TILE);
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &work.out,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: ox, y: oy, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &result.store.pages[slot.page as usize].texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: slot.slice,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: TILE.min(w - ox),
                    height: TILE.min(h - oy),
                    depth_or_array_layers: 1,
                },
            );
        }
        ctx.queue.submit([enc.finish()]);
        Ok(result)
    }
}
