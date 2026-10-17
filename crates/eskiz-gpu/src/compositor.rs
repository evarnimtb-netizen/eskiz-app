//! The tile compositor.
//!
//! The layer tree is flattened on the CPU into a short program (plain draws and clip
//! groups). For each batch of up to [`BATCH_TILES`] output tiles the GPU runs that program
//! over an f32 accumulator, then writes the 8-bit result into the output texture. Only
//! tiles in the requested [`Region`] are touched, which is the dirty-rect path.

use crate::context::{GpuContext, GpuError};
use crate::layer::{GpuLayer, GpuMask};
use crate::scene::{mode_index, Scene, SceneItem};
use crate::tile::{Rect, TileCoord, TILE};
use std::collections::BTreeMap;

/// Output tiles processed per batch (scratch memory is 2 x 64 MiB at most).
pub const BATCH_TILES: usize = 64;

const OP_INIT: u32 = 0;
const OP_DRAW: u32 = 1;
const OP_CLIP_CLEAR: u32 = 2;
const OP_CLIP_BASE: u32 = 3;
const OP_CLIP_APPLY: u32 = 4;
const OP_CLIP_END: u32 = 5;
const OP_OUTPUT: u32 = 6;

const PARAMS_SIZE: u64 = 112;
const TILE_PIXELS: u64 = (TILE as u64) * (TILE as u64);
const SCRATCH_PER_TILE: u64 = TILE_PIXELS * 16;

/// What to recomposite.
#[derive(Clone, Debug)]
pub enum Region {
    /// Every tile of the canvas.
    All,
    /// The tiles touched by this pixel rectangle.
    Rect(Rect),
    /// Exactly these tiles.
    Tiles(Vec<TileCoord>),
}

/// Work done by one [`Compositor::composite`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeStats {
    /// Output tiles recomposited.
    pub tiles: usize,
    /// Compute dispatches recorded.
    pub dispatches: usize,
}

/// Row `r` of the display matrix padded to a vec4 (identity when there is no transform).
fn row4(m: Option<[[f32; 3]; 3]>, r: usize) -> [f32; 4] {
    let m = m.unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    [m[r][0], m[r][1], m[r][2], 0.0]
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    op: u32,
    blend: u32,
    mask_mode: u32,
    list_offset: u32,
    opacity: f32,
    mask_const: f32,
    canvas_w: u32,
    canvas_h: u32,
    bg: [f32; 4],
    /// Rows of the display matrix (xyz used), applied by the output pass when `xform` is 1.
    m0: [f32; 4],
    m1: [f32; 4],
    m2: [f32; 4],
    xform: u32,
    pad: [u32; 3],
}

#[derive(Clone, Copy)]
struct LayerRef<'a> {
    layer: &'a GpuLayer,
    mask: Option<&'a GpuMask>,
    blend: u32,
    opacity: f32,
}

enum Op<'a> {
    Draw(LayerRef<'a>),
    Clip {
        base: LayerRef<'a>,
        clipped: Vec<LayerRef<'a>>,
    },
}

struct Pending<'a> {
    base: Option<LayerRef<'a>>,
    clipped: Vec<LayerRef<'a>>,
}

fn flush<'a>(pending: &mut Option<Pending<'a>>, out: &mut Vec<Op<'a>>) {
    if let Some(p) = pending.take() {
        if let Some(base) = p.base {
            if p.clipped.is_empty() {
                out.push(Op::Draw(base));
            } else {
                out.push(Op::Clip {
                    base,
                    clipped: p.clipped,
                });
            }
        }
    }
}

/// Flatten a stack (bottom to top) into ops. Groups composite in place.
fn walk<'a>(items: &'a [SceneItem<'a>], out: &mut Vec<Op<'a>>) {
    let mut pending: Option<Pending<'a>> = None;
    for item in items {
        match item {
            SceneItem::Group(g) => {
                flush(&mut pending, out);
                if g.visible {
                    walk(&g.children, out);
                }
            }
            SceneItem::Layer(l) => {
                let r = LayerRef {
                    layer: l.layer,
                    mask: l.mask,
                    blend: mode_index(l.blend),
                    opacity: l.opacity.clamp(0.0, 1.0),
                };
                if l.clipped {
                    match pending.as_mut() {
                        // Clipped to the raster base directly below (hidden base: nothing).
                        Some(p) => {
                            if l.visible && p.base.is_some() {
                                p.clipped.push(r);
                            }
                        }
                        // No base: draws as if it were not clipped.
                        None => {
                            if l.visible && r.opacity > 0.0 {
                                out.push(Op::Draw(r));
                            }
                        }
                    }
                } else {
                    flush(&mut pending, out);
                    pending = Some(Pending {
                        base: l.visible.then_some(r),
                        clipped: Vec::new(),
                    });
                }
            }
        }
    }
    flush(&mut pending, out);
}

struct DispatchRec<'a> {
    param_index: u32,
    count: u32,
    layer_page: Option<&'a wgpu::TextureView>,
    mask_page: Option<&'a wgpu::TextureView>,
}

#[derive(Default)]
struct Plan<'a> {
    params: Vec<Params>,
    entries: Vec<[u32; 4]>,
    dispatches: Vec<DispatchRec<'a>>,
}

impl<'a> Plan<'a> {
    fn push(
        &mut self,
        mut p: Params,
        entries: &[[u32; 4]],
        layer_page: Option<&'a wgpu::TextureView>,
        mask_page: Option<&'a wgpu::TextureView>,
    ) {
        if entries.is_empty() {
            return;
        }
        p.list_offset = self.entries.len() as u32;
        self.entries.extend_from_slice(entries);
        self.params.push(p);
        self.dispatches.push(DispatchRec {
            param_index: (self.params.len() - 1) as u32,
            count: entries.len() as u32,
            layer_page,
            mask_page,
        });
    }
}

/// Composites a [`Scene`] into a canvas-sized RGBA8 output texture.
pub struct Compositor {
    width: u32,
    height: u32,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
    acc: wgpu::Buffer,
    clip: Option<wgpu::Buffer>,
    clip_dummy: wgpu::Buffer,
    dummy_layer: wgpu::TextureView,
    dummy_mask: wgpu::TextureView,
    params_buf: wgpu::Buffer,
    params_cap: u64,
    list_buf: wgpu::Buffer,
    list_cap: u64,
    params_stride: u64,
}

impl Compositor {
    /// Create a compositor for a `width` x `height` canvas.
    pub fn new(ctx: &GpuContext, width: u32, height: u32) -> Self {
        let dev = &ctx.device;
        let shader = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("eskiz composite"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/composite.wgsl").into()),
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let buf_entry = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("eskiz composite bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_SIZE),
                    },
                    count: None,
                },
                buf_entry(1, true),
                buf_entry(2, false),
                buf_entry(3, false),
                tex_entry(4),
                tex_entry(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let layout = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("eskiz composite layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("eskiz composite"),
            layout: Some(&layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let output = dev.create_texture(&wgpu::TextureDescriptor {
            label: Some("eskiz composite output"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());

        let dummy = |format| {
            dev.create_texture(&wgpu::TextureDescriptor {
                label: Some("eskiz dummy tile"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let storage = |label, size| {
            dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let align = u64::from(dev.limits().min_uniform_buffer_offset_alignment).max(16);
        let params_stride = PARAMS_SIZE.div_ceil(align) * align;
        let params_cap = params_stride * 64;
        let list_cap = 16 * 1024;
        let params_buf = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("eskiz composite params"),
            size: params_cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            width,
            height,
            output,
            output_view,
            pipeline,
            bgl,
            acc: storage("eskiz accumulator", BATCH_TILES as u64 * SCRATCH_PER_TILE),
            clip: None,
            clip_dummy: storage("eskiz clip dummy", 16),
            dummy_layer: dummy(wgpu::TextureFormat::Rgba8Unorm),
            dummy_mask: dummy(wgpu::TextureFormat::R8Unorm),
            params_buf,
            params_cap,
            list_buf: storage("eskiz composite list", list_cap),
            list_cap,
            params_stride,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The composited canvas, premultiplied RGBA8 (`Rgba8Unorm`). Usable as a sampled texture.
    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output
    }

    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    fn tiles_for(&self, region: &Region) -> Vec<TileCoord> {
        let tx = self.width.div_ceil(TILE);
        let ty = self.height.div_ceil(TILE);
        let mut v = Vec::new();
        match region {
            Region::All => {
                for y in 0..ty {
                    for x in 0..tx {
                        v.push((x, y));
                    }
                }
            }
            Region::Rect(r) => {
                let r = Rect::new(
                    r.x.min(self.width),
                    r.y.min(self.height),
                    r.w.min(self.width.saturating_sub(r.x)),
                    r.h.min(self.height.saturating_sub(r.y)),
                );
                if let Some(((x0, y0), (x1, y1))) = r.tile_range() {
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            v.push((x, y));
                        }
                    }
                }
            }
            Region::Tiles(t) => {
                v.extend(t.iter().copied().filter(|&(x, y)| x < tx && y < ty));
                v.sort_unstable();
                v.dedup();
            }
        }
        v
    }

    /// Recomposite the tiles of `region` from `scene` into the output texture.
    pub fn composite(
        &mut self,
        ctx: &GpuContext,
        scene: &Scene<'_>,
        region: &Region,
    ) -> Result<CompositeStats, GpuError> {
        let tiles = self.tiles_for(region);
        if tiles.is_empty() {
            return Ok(CompositeStats::default());
        }
        let mut ops = Vec::new();
        walk(&scene.items, &mut ops);
        let has_clip = ops.iter().any(|o| matches!(o, Op::Clip { .. }));
        if has_clip && self.clip.is_none() {
            self.clip = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("eskiz clip buffer"),
                size: BATCH_TILES as u64 * SCRATCH_PER_TILE,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));
        }

        let bg = match scene.background {
            Some(c) => [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), 1.0],
            None => [0.0; 4],
        };
        let base = Params {
            op: 0,
            blend: 0,
            mask_mode: 0,
            list_offset: 0,
            opacity: 1.0,
            mask_const: 1.0,
            canvas_w: self.width,
            canvas_h: self.height,
            bg,
            m0: row4(scene.display, 0),
            m1: row4(scene.display, 1),
            m2: row4(scene.display, 2),
            xform: u32::from(scene.display.is_some()),
            pad: [0; 3],
        };

        let mut plan = Plan::default();
        for batch in tiles.chunks(BATCH_TILES) {
            let slots: Vec<(u32, TileCoord)> = batch
                .iter()
                .enumerate()
                .map(|(i, &t)| (i as u32, t))
                .collect();
            let all_entries: Vec<[u32; 4]> = slots
                .iter()
                .map(|&(s, (tx, ty))| [s, 0, 0, tx | (ty << 16)])
                .collect();
            plan.push(Params { op: OP_INIT, ..base }, &all_entries, None, None);
            for op in &ops {
                match op {
                    Op::Draw(l) => {
                        Self::plan_layer(&mut plan, base, OP_DRAW, l, 1.0, &slots);
                    }
                    Op::Clip { base: b, clipped } => {
                        // Tiles where the base has pixels (and is not masked out entirely).
                        let base_tiles: Vec<(u32, TileCoord)> = slots
                            .iter()
                            .copied()
                            .filter(|&(_, t)| b.layer.store.get(t).is_some())
                            .collect();
                        if base_tiles.is_empty() {
                            continue;
                        }
                        let ents: Vec<[u32; 4]> = base_tiles
                            .iter()
                            .map(|&(s, (tx, ty))| [s, 0, 0, tx | (ty << 16)])
                            .collect();
                        plan.push(Params { op: OP_CLIP_CLEAR, ..base }, &ents, None, None);
                        Self::plan_layer(&mut plan, base, OP_CLIP_BASE, b, 1.0, &base_tiles);
                        for c in clipped {
                            Self::plan_layer(&mut plan, base, OP_CLIP_APPLY, c, 1.0, &base_tiles);
                        }
                        plan.push(
                            Params {
                                op: OP_CLIP_END,
                                blend: b.blend,
                                opacity: b.opacity,
                                ..base
                            },
                            &ents,
                            None,
                            None,
                        );
                    }
                }
            }
            plan.push(Params { op: OP_OUTPUT, ..base }, &all_entries, None, None);
        }

        self.execute(ctx, &plan);
        Ok(CompositeStats {
            tiles: tiles.len(),
            dispatches: plan.dispatches.len(),
        })
    }

    /// Queue draw-style dispatches for one layer over `tiles`, grouped by page pair.
    fn plan_layer<'a>(
        plan: &mut Plan<'a>,
        base: Params,
        op: u32,
        l: &LayerRef<'a>,
        _unused: f32,
        tiles: &[(u32, TileCoord)],
    ) {
        if l.opacity <= 0.0 && op != OP_CLIP_BASE {
            return;
        }
        // (layer page, mask mode, mask page) -> entries
        let mut groups: BTreeMap<(u32, u32, u32), Vec<[u32; 4]>> = BTreeMap::new();
        for &(slot, tc) in tiles {
            let Some(ls) = l.layer.store.get(tc) else {
                continue;
            };
            let (mode, mpage, mslice) = match l.mask {
                None => (0, 0, 0),
                Some(m) => match m.store.get(tc) {
                    Some(ms) => (2, ms.page, ms.slice),
                    None if m.default_value() > 0.0 => (1, 0, 0),
                    None => continue,
                },
            };
            groups
                .entry((ls.page, mode, mpage))
                .or_default()
                .push([slot, ls.slice, mslice, tc.0 | (tc.1 << 16)]);
        }
        for ((lpage, mode, mpage), entries) in groups {
            let mask_const = l.mask.map_or(1.0, |m| m.default_value());
            let p = Params {
                op,
                blend: l.blend,
                mask_mode: mode,
                opacity: l.opacity,
                mask_const,
                ..base
            };
            let lv = &l.layer.store.pages[lpage as usize].view;
            let mv = if mode == 2 {
                l.mask.map(|m| &m.store.pages[mpage as usize].view)
            } else {
                None
            };
            plan.push(p, &entries, Some(lv), mv);
        }
    }

    fn execute(&mut self, ctx: &GpuContext, plan: &Plan<'_>) {
        let dev = &ctx.device;
        // Params, 1 entry per dispatch at `params_stride`.
        let need_params = plan.params.len() as u64 * self.params_stride;
        if need_params > self.params_cap {
            self.params_cap = need_params.next_power_of_two();
            self.params_buf = dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some("eskiz composite params"),
                size: self.params_cap,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let mut pbytes = vec![0u8; need_params as usize];
        for (i, p) in plan.params.iter().enumerate() {
            let o = i * self.params_stride as usize;
            pbytes[o..o + PARAMS_SIZE as usize].copy_from_slice(bytemuck::bytes_of(p));
        }
        ctx.queue.write_buffer(&self.params_buf, 0, &pbytes);

        let need_list = (plan.entries.len() as u64 * 16).max(16);
        if need_list > self.list_cap {
            self.list_cap = need_list.next_power_of_two();
            self.list_buf = dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some("eskiz composite list"),
                size: self.list_cap,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        ctx.queue
            .write_buffer(&self.list_buf, 0, bytemuck::cast_slice(&plan.entries));

        let clip_buf = self.clip.as_ref().unwrap_or(&self.clip_dummy);
        let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("eskiz composite"),
        });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("eskiz composite"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            for d in &plan.dispatches {
                let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("eskiz composite bg"),
                    layout: &self.bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.params_buf,
                                offset: 0,
                                size: wgpu::BufferSize::new(PARAMS_SIZE),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: self.list_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: self.acc.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: clip_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(
                                d.layer_page.unwrap_or(&self.dummy_layer),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(
                                d.mask_page.unwrap_or(&self.dummy_mask),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 6,
                            resource: wgpu::BindingResource::TextureView(&self.output_view),
                        },
                    ],
                });
                pass.set_bind_group(
                    0,
                    Some(&bg),
                    &[(u64::from(d.param_index) * self.params_stride) as u32],
                );
                pass.dispatch_workgroups(TILE / 8, TILE / 8, d.count);
            }
        }
        ctx.queue.submit([enc.finish()]);
    }

    /// Read back a rectangle of the output as premultiplied RGBA8.
    pub fn download_output(&self, ctx: &GpuContext, rect: Rect) -> Result<Vec<u8>, GpuError> {
        if rect.x + rect.w > self.width || rect.y + rect.h > self.height {
            return Err(GpuError::Invalid("rect outside the canvas".into()));
        }
        read_rgba8_texture(ctx, &self.output, rect)
    }

    /// Read back the whole output.
    pub fn download_output_all(&self, ctx: &GpuContext) -> Result<Vec<u8>, GpuError> {
        self.download_output(ctx, Rect::new(0, 0, self.width, self.height))
    }
}

/// Copy a rectangle of an RGBA8 texture to the CPU.
pub(crate) fn read_rgba8_texture(
    ctx: &GpuContext,
    tex: &wgpu::Texture,
    rect: Rect,
) -> Result<Vec<u8>, GpuError> {
    if rect.is_empty() {
        return Ok(Vec::new());
    }
    let row = rect.w * 4;
    let padded = row.div_ceil(256) * 256;
    let size = u64::from(padded) * u64::from(rect.h);
    let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("eskiz texture readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: rect.x,
                y: rect.y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(rect.h),
            },
        },
        wgpu::Extent3d {
            width: rect.w,
            height: rect.h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([enc.finish()]);
    let raw = ctx.read_buffer(&staging, size)?;
    let mut out = Vec::with_capacity((row * rect.h) as usize);
    for y in 0..rect.h as usize {
        let o = y * padded as usize;
        out.extend_from_slice(&raw[o..o + row as usize]);
    }
    Ok(out)
}
