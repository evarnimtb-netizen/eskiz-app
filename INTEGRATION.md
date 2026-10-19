# Integrating the engine crates into `eskiz-app`

For the engineer wiring the new crates into the eframe/egui app. Everything here is checked against the code. Where a note and the code disagree, the code is quoted.

Contents:
1. [Recommended order](#1-recommended-order)
2. [Conventions every crate shares, and the exceptions](#2-conventions)
3. [Duplicate types and the conversions between them](#3-duplicate-types-and-conversions)
4. [Per-crate guide](#4-per-crate-guide)
5. [Error types](#5-error-types)
6. [Threading and performance](#6-threading-and-performance)
7. [What the review changed](#7-what-the-review-changed)
8. [Open decisions and gaps](#8-open-decisions-and-gaps)

---

## 1. Recommended order

The order follows dependencies. The document model decides what every later step consumes, so it comes first. The renderer switch is the one step that touches everything visible, so it comes once the model is settled and while the app is still small.

| Step | Work | Crates | Notes |
|---|---|---|---|
| 0 | Add `eskiz-app/src/convert.rs` with the conversions in section 3 (blend mode, pixel formats, rectangles, samples). Test them once, use them everywhere. | core, format, select, filters, gpu, guides | |
| 1 | **Document model v2**: layer tree (groups, masks, clipping, lock transparency, hidden/locked/private, background colour). The app's `Document` is flat today (`Canvas` plus `LayerInfo`). | core, format | |
| 2 | **`.procreate` open and save** on the new model, replacing `import.rs`. Gallery import and the native save format. | format | |
| 3 | **Brush Library panel and painting with library brushes.** Cheap, visible, no renderer change. | brushlib, core, format (`.brush`) | |
| 4 | **Colour panel**: disc, classic, palettes, history, harmony. | guides, format (palettes) | |
| 5 | **Selection and canvas operations**, then transform and clipboard. Painting is clipped through `Selection::clip_coverage`. | select | |
| 6 | **Adjustments panel on the CPU first** (Opacity, Gaussian, HSB), using the selection mask. This builds the preview/apply/undo flow without GPU risk. | filters | |
| 7 | **Renderer switch to wgpu and the GPU compositor.** Display path, then dirty-rect upload, then GPU adjustments. See 4.8 for the loop. | gpu | |
| 8 | **Drawing guides and Guided Layer.** | guides | |
| 9 | **Text layers**, then rasterise to a layer. | text, format | |
| 10 | **Frames, GIF/APNG/MP4 export, timelapse.** | anim, media | |
| 11 | **Export dialog**: PNG, JPEG, TIFF, WebP, PDF, PSD. | format (`export`, `psd`) | |
| 12 | **3D painting.** | 3d, gpu (later, for the viewport) | |

Why 7 is not first: the painting engine is CPU. Moving the display to the GPU does not make strokes faster until the dirty-rect upload loop (4.8) exists, and that loop wants the document model from step 1. Steps 3 to 6 need no renderer change and each shows up in the UI.

Why 6 before 7: the CPU filters are the ground truth the GPU shaders were tested against. Wiring the UI flow to them first means the GPU swap later changes one function (`apply`), not the interaction.

---

## 2. Conventions

### What is shared

| Topic | Convention (all crates unless noted below) |
|---|---|
| Pixel origin | Top-left, `y` grows down. Pixel `(x, y)` covers `[x, x+1) x [y, y+1)`, centre `(x+0.5, y+0.5)`. Rows are stored top row first. |
| Layer pixels | **Premultiplied** RGBA, 4 channels, row-major, no padding. `eskiz-core` `Layer`, `eskiz-text` `Raster`, `eskiz-media` `Frame`, `eskiz-format` `LayerSpec.pixels`, `PsdLayer.pixels`, `export::Raster`, `eskiz-gpu` layer tiles and the compositor output, `eskiz-3d` texture maps. |
| Bytes to float | `v = byte / 255`. Float to byte is `round(clamp(v, 0, 1) * 255)`. `eskiz-core::compose::to_u8`, `eskiz-filters::Image::to_rgba8_premul` and the GPU stores all follow this, so a CPU-GPU round trip is bit-stable for 8-bit data. |
| Colour space | sRGB-encoded values throughout; blending is done on the encoded values (as usual for this format). `eskiz-types::colour_space` (re-exported as `eskiz-guides::colour_space`) converts to Display P3. `eskiz-3d` shades in linear light and converts at the edges. |
| Layer order | Stacks are **bottom to top** in `eskiz-core::Canvas`, `eskiz-gpu::Scene`, `eskiz-anim::StackSnapshot` and `PsdDocument.root`. |
| Opacity | 0 to 1. `f64` in core, format and anim; `f32` in gpu, psd and 3d. |
| Clipping | "Clipped" means clipped to the nearest non-clipped item directly below, in the same parent. Same meaning in format, psd, gpu and anim. |

### The exceptions you will trip over

| Where | Difference | What to do |
|---|---|---|
| `eskiz-format` `.procreate` tree | `Document.tree`, `Document.layers`, `DocumentBuilder.tree` and `GroupSpec.children` are **top-most first**, as stored. | Reverse when converting to or from a bottom-to-top stack. `Document.layers` is depth-first in the same top-most-first order. |
| `eskiz-select::Image` | **Straight** (non-premultiplied) alpha, `f32`, one `[f32; 4]` per pixel. | Use `Image::from_premul_rgba8` / `to_premul_rgba8` (added in this review). |
| `eskiz-filters::Image` | Premultiplied `f32`, one flat `Vec<f32>` (`width * height * 4`). | `from_rgba8_premul` / `to_rgba8_premul`. |
| Palettes (`eskiz-format::palettes`) and `DocumentBuilder.background_color` | Straight RGBA `f32`. | Convert at the edge. |
| `eskiz-text::Color` | Straight RGBA `u8`. | Converted internally; the `Raster` it produces is premultiplied. |
| `eskiz-guides::Rgb` (defined in `eskiz-types`) | Straight sRGB `f64`. `Rgb8` is `u8`. | `eskiz-core` stroke colours are `[f64; 3]` straight sRGB, so they map 1:1. |
| Angles | `eskiz-core` (tilt, azimuth, roll) and `eskiz-guides` are radians. `eskiz-select` (`rotate_deg`, `snap_angle`) is degrees. `eskiz-3d` camera is radians. | Convert at the call. |
| Time | `eskiz-core::Sample.t` is seconds (`f64`). `eskiz-media` uses `std::time::Duration`. `eskiz-anim::Playback::advance` takes `dt_seconds: f64`. | |
| Integer widths | `usize` in core, select, filters, text layout; `u32` in format, media, gpu, text `Raster`. | Plain `as` casts; sizes are bounded by 16k anyway. |
| Text units | `eskiz-text` lays out in "layer units": 1 unit = 1 pt at scale 1; the `Affine` places the layer in canvas pixels. | |
| UV space | `eskiz-3d`: `(0,0)` top-left texel corner, `v` grows down. glTF matches; the OBJ loader flips `v`. | |

---

## 3. Duplicate types and conversions

Several concepts exist more than once. They are not all worth unifying today. The table says which to leave and how to convert.

| Concept | Where it appears | Verdict |
|---|---|---|
| Blend mode | `eskiz_core::BlendMode` (enum, 26), `eskiz_format::psd::PsdBlend` (same 26 variants, same order), `eskiz_format::Layer.blend_mode: Option<i64>` and `LayerSpec.blend_mode: i64` (file index), `eskiz_gpu` re-exports `BlendMode`. | Keep. Convert with the helpers below. |
| Raster image | `eskiz_core::Layer`, `eskiz_text::Raster`, `eskiz_media::Frame`, `eskiz_format::export::Raster`, `eskiz_filters::Image`, `eskiz_select::Image` | Keep; they differ on purpose (see section 2). Convert at crate boundaries. |
| Integer rectangle | `eskiz_gpu::Rect` (`x, y, w, h: u32`), `eskiz_select::IRect` (`x, y, w, h: usize`), `eskiz-app` `PixelRect` | Convert with the snippet below. |
| Float rectangle | `eskiz_select::Rect` (`x0,y0,x1,y1: f32`), `eskiz_text::Rect` (same, layer units), `eskiz_guides::Rect` (`min`, `max: Vec2`, `f64`) | Different spaces and precisions; leave. |
| Vector | `eskiz_guides::Vec2` (`f64`), `eskiz_select::Point` (`f32`), `glam::Vec2` in `eskiz-3d` | Leave. |
| Input sample | `eskiz_core::Sample` (x, y, t, pressure, tilt, azimuth, roll, pointer), `eskiz_guides::Sample` (`pos: Vec2`, `pressure`) | Convert; see 4.9. |
| Combine mode | `eskiz_select::CombineMode` (Replace/Add/Subtract), `eskiz_filters::noise::CombineMode` (Additive/Mix) | Unrelated. Import with a path or alias. |
| Curve set | `eskiz_core::CurveSet` (brush dynamics), `eskiz_filters::curves::CurveSet` (tone curves) | Unrelated. |
| Brush set | `eskiz_format::brushes::BrushSet` (imported `.brushset`), `eskiz_brushlib::BrushSet` (built-in library) | Unrelated; the Brush Library panel will list both. |
| Frame rate limits | `MIN_FPS`, `MAX_FPS` were defined in both `eskiz-media` and `eskiz-anim` | Fixed: anim now re-exports media's. |

### Conversions to put in `convert.rs`

```rust
// Blend mode: file index <-> enum.
let mode = BlendMode::from_index(layer.blend_mode.unwrap_or(0)).unwrap_or_default(); // .procreate read
let index: i64 = mode.to_index();                                                    // .procreate write (new helper)

// PSD blend: PsdBlend and BlendMode::ALL have the same variants in the same order.
fn to_psd(m: BlendMode) -> PsdBlend { PSD_ALL[BlendMode::ALL.iter().position(|&x| x == m).unwrap()] }
// Build PSD_ALL once from PsdBlend::from_key / key; assert in a unit test that
// PsdBlend::from_key(*b"mul ") corresponds to BlendMode::Multiply, etc.

// Premultiplied RGBA8 <-> the other pixel types.
let sel_img  = eskiz_select::Image::from_premul_rgba8(w, h, &layer.pixels).unwrap();   // straight f32
let back     = sel_img.to_premul_rgba8();
let f_img    = eskiz_filters::Image::from_rgba8_premul(w, h, &layer.pixels);           // premul f32
let frame    = eskiz_media::Frame::new(w as u32, h as u32, layer.pixels.clone(), dur)?;

// Masks: select::Mask (f32 0..1) <-> GPU / file bytes (u8) <-> filters (&[f32]).
let bytes = mask.to_u8();                                     // for GpuMask::upload_rect
let mask  = eskiz_select::Mask::from_u8(w, h, &bytes).unwrap();
let mask_for_filters: Option<&[f32]> = Some(&mask.data);      // filters take the mask as a slice

// Rectangles.
let g = eskiz_gpu::Rect::new(r.x as u32, r.y as u32, r.w as u32, r.h as u32);   // from select::IRect

// Colours.
let rgb: [f64; 3] = [c.r, c.g, c.b];                          // guides::Rgb -> core stroke colour
```

When `.procreate` layers are decoded, `Document::layer_rgba` returns premultiplied RGBA8 top row first, which is what `eskiz_core::Layer.pixels` wants directly.

---

## 4. Per-crate guide

Each section: what it provides, entry points, conversions, known gaps.

### 4.1 eskiz-core (context, changed in this review)

The painting engine and the app's current document types. Not part of the request, but you will touch it constantly.

- `Layer::paint_stroke_with(&Stroke, PaintOptions)` paints with the built-in tips.
- **New:** `Layer::paint_stroke_with_raster(&Stroke, PaintOptions, &DabRaster)` and `DabRaster::with_textures(&Brush, ShapeTexture, Option<GrainTexture>)`, with `ShapeTexture::from_gray(n, &[u8])` and `GrainTexture::from_gray(n, &[u8])`. This is how library and imported brushes paint with their own tip and grain. Tips are square `n x n` 8-bit masks, white is paint. Grain size is taken from the texture, not a constant.
- **Fixed:** Soft Light is the Pegtop form, the non-separable modes use the standard luminance weights (0.30005, 0.58984, 0.10998), Lighter/Darker Color use the luma weights (0.29907, 0.58691, 0.11401) with ties going to the source, and Color Dodge, Color Burn and Divide use the 2^-14 divisor floor (the known W3C correction). The GPU compositor and the CPU reference now agree.
- **New:** `BlendMode::to_index()`, the inverse of `from_index`. Note `BlendMode::ALL` order is not the file index (Darken is 18th in `ALL` and 19 in files).
- `flatten` and `flatten_rect` assume an opaque white backdrop and no masks, clipping or groups. They remain the app's CPU fallback and the oracle for `eskiz-gpu` on opaque stacks.

### 4.2 eskiz-format

**Provides.** `.procreate` read and write, PSD read and write, `.brush` / `.brushset` / `.abr` import and `.brush` export, `.swatches` / `.aco` / `.ase` read and write, raster and PDF export.

**Entry points.**
- Read: `Document::open(path)` or `from_reader`. Fields: `width`, `height`, `tree: Vec<Item>` (top-most first), `layers`, `background_color: Option<[f32;4]>`, `background_hidden`, `dpi`. Pixels: `layer_rgba(&Layer)`, `composite_rgba()`, `thumbnail()`. `layer.mask` holds a mask layer; decode it with **`mask_rgba(&parent_layer) -> Option<Vec<u8>>`** (`None` when the layer has no mask); missing mask tiles read `0xFF` per the format spec.
- Write: build a `DocumentBuilder` (tree of `Node::Layer(LayerSpec)` / `Node::Group(GroupSpec)`, top-most first), then `document::write(path, &builder)` or `to_bytes()`. `LayerSpec.blend_mode` is the file index (`BlendMode::to_index()`); indices 18, 27 to 33 and above 34 are rejected.
- PSD: `psd::read(path)` / `psd::write(path, &doc)`. `PsdDocument.root` is **bottom to top** (unlike `.procreate`). `PsdLayer.pixels` is canvas-sized premultiplied RGBA8.
- Brushes: `brushes::read_brush(name_hint, bytes)`, `read_brushset`, `read_abr`, `write_brush`, `write_brushset`; `engine_params(&ImportedBrush)` extracts size, spacing, Smoothing and tapers. `shape_png` / `grain_png` are PNG bytes.
- Palettes: `palettes::read_swatches` / `read_aco` / `read_ase` (return `PaletteRead` with warnings) and the matching `write_*`. Colours are straight RGBA `f32`.
- Export: `export::encode_raster` / `export_raster` (PNG, JPEG, TIFF, WebP), `export_layers_png`, `encode_pdf` / `export_pdf` / `export_layers_pdf`. They take `export::Raster` (premultiplied RGBA8) and write atomically (temp file then rename). *`export.rs` was still being edited by another worker when this was written; check its docs for final names.*

**Buffer conversions.** See section 3. Reversal of the tree order is the main trap. A mask layer is an RGBA8 layer; which channel carries the mask value is not confirmed on a real file.


**Known gaps.**
- No `.brush` to `eskiz_core::Brush` converter. `engine_params` covers only size, spacing, Smoothing and tapers. A real mapping (shape, grain, dynamics curves) is still to write, and the `shape_png` / `grain_png` must be decoded to grey and passed to `ShapeTexture::from_gray` / `GrainTexture::from_gray`.
- The reader decodes 8-bit RGBA only (16-bit and 32-bit layers return `Error::Unsupported`).
- Text layers are detected (`Layer::is_text`) but their attributed string is not parsed; `eskiz-text::storage` has the key mapping for the write side.
- Tile row order (top-left vs bottom-left origin) is not documented anywhere official. The reader assumes top-left. Verify with a real file early, because a flipped layer is the first thing a user would see.
- Saved selections (`userSavedSelectionMasks`) are not read, and the writer emits an empty list.
- Writer version numbers are guessed (documented in `document.rs`).

### 4.3 eskiz-media

**Provides.** Animated GIF, APNG, MP4 (H.264 via openh264) export, and the timelapse recorder (segments, full export, 30-second summary).

**Entry points.** `Frame::new(w, h, premul_rgba, Duration)`; `encode_gif` / `write_gif_file(path, &[Frame], &GifOptions)`; `encode_apng` / `write_apng_file`; `encode_mp4` / `write_mp4_file`; `TimelapseRecorder::new(doc_w, doc_h, settings, store)` then `commit(&Frame)` per committed change, `finish_segment()`, `export_full(path)`, `export_summary(path)`. `spec.rs` helpers: `duration_for_fps`, `capture_size`, `oriented_box`.

**Conversions.** `Frame` takes premultiplied RGBA8, top row first. Feed it from `eskiz_core::compose::flatten` output (alpha 255) or from a GPU download.


**Known gaps.**
- MP4 uses openh264. The default `source` feature compiles it from source; for distributed builds the plan is Cisco's runtime-downloaded binary, selected with the `libloading` feature and `ESKIZ_OPENH264_LIB`. That runtime download is not implemented in the app.
- The timelapse recorder takes finished frames; deciding *when* to commit (one per committed change) and downscaling to the capture size are the caller's job (`capture_size` gives the size, `Frame::resize_box` resizes).
- Segment storage on disk only; no in-app playback viewer.

### 4.4 eskiz-text

**Provides.** Attributed text layers: fonts, shaping (cosmic-text), the documented line, leading, alignment, justification and vertical rules, outline and underline, rasterisation (swash), and the saved-attribute key mapping.

**Entry points.** `FontLibrary::new()` (system fonts), `import_file` / `load_dir` for user fonts, `resolve(family)` for the missing-font fallback. `TextLayer::new(text, attrs, box_width)`, `set_attrs(range, |a| ..)`, `layout(&mut fonts)`, `rasterise(&mut fonts)` or `rasterise_at_scale(&mut fonts, s)`. The data types are defined in `eskiz-types` (`eskiz_types::text`, so `eskiz-core` can hold them without fonts) and re-exported from `eskiz_text::model`; the three font methods are on the trait `eskiz_text::TextLayerExt`, which callers import. `storage::attrs_to_saved` / `apply_saved` for persistence.

**Conversions.** `Raster { origin: (i32, i32), width, height, pixels }` is premultiplied RGBA8 with the document position of its top-left pixel. To rasterise onto a layer: blit with source-over at `origin`, clipped to the layer. There is no `blit` helper; add one to `convert.rs`.


**Known gaps.**
- "Editable until you rasterize" needs the app to keep the `TextLayer` and re-rasterise on edit; there is no layer-model slot for it yet (step 1 should reserve one).
- No caret, hit-testing or selection geometry for the on-canvas editor: `Layout` exposes `LineBox` and `PlacedGlyph` positions from which these can be built, but nothing does it.
- Grey-scale antialiasing only; no hinting.

### 4.5 eskiz-filters

**Provides.** CPU reference implementations of every adjustment and Liquify: Gaussian, pyramid blur, motion blur, perspective blur, sharpen, noise, HSB, colour balance, curves, gradient map, recolor, bloom, chromatic aberration, glitch, halftone, clone, opacity, liquify. Parallel with rayon.

**Entry points.** Each module has `apply(&Image, Option<&[f32]>, &Params) -> Image`. The mask is optional and is mixed in with the shared rule (`out = mix(src, adjusted, m)`). `blur_for_mode` selects Gaussian (Layer mode) or pyramid (Pencil mode). `gaussian::preview` is the live-preview variant (reduced-scale blur). `drag_to_value` implements the drag-on-canvas value mapping. `liquify` is separate (see its module docs).

**Conversions.** `Image::from_rgba8_premul(w, h, &layer.pixels)` and `to_rgba8_premul()`. Selection masks pass straight through as `&selection_mask.data`. The constructors panic on a wrong buffer size.


**Known gaps.**
- Everything allocates whole-canvas `f32` images (16 bytes per pixel). At 4096 x 4096 that is 256 MiB per image. Use it for previews at reduced size and for tests; use the GPU path for full-size interactive work.
- Undo integration and the preview/commit lifecycle are the app's job (`gaussian::UNDO_NAME` is the only hook).
- Several parameters carry "unverified" in their doc comments; read those before exposing the controls.

### 4.6 eskiz-select

**Provides.** Selection masks and tools (rectangle, ellipse, polygon, freehand with smoothing, flood-fill "Automatic" with drag-to-threshold), combine modes, invert, feather, outline for marching ants, saved selections; transform sessions (free, uniform, distort, warp mesh, snapping, numeric input, flip, rotate 45, fit); clipboard (copy, copy all, cut, paste); canvas operations (crop, resize canvas with anchor, resize image, flip).

**Entry points.** `raster::{rectangle, ellipse, polygon}` and `raster::FreehandStroke` (`push`, `finish(w, h)`) give a `Mask`. `Selection::none()`, `Selection::active(mask)`, `combine(&Mask, CombineMode)`, `invert(w, h)`, `value_at`, `clip_coverage(x, y, coverage)` (multiply stroke coverage by the selection), `bounds()`. `wand::flood_select(&Image, seed, threshold)`. `feather::feather(&Mask, amount)`. `outline::outline(&Mask, level)` returns polylines for the ants. `TransformSession::new(source, footprint, initial, interp)`, `apply_gesture`, `render()` for the preview, `commit(&layer)` for the result. `clipboard::*`, `canvas::*`.

**Conversions.** Uses **straight** `f32` images: convert with `Image::from_premul_rgba8` / `to_premul_rgba8` (new). The brush engine takes the selection as a coverage multiplier per pixel; wire `Selection::clip_coverage` into the coverage buffer stage in `eskiz-core` (the app owns that seam).


**Known gaps.**
- Whole-canvas `f32` buffers again (a `Mask` is 4 bytes per pixel, an `Image` 16). Fine to 4k x 4k for occasional operations, heavy for per-stroke work.
- `outline` returns the iso-contour for display; it is not animated, and "marching ants" timing is the UI's job.
- Selection-aware painting is not hooked into `eskiz-core` yet.
- GPU preview of transforms is not provided (the roadmap lists it). `TransformSession::render` is CPU.

### 4.7 eskiz-anim

**Provides.** Frames logic: settings with defaults, frame list built from a stack snapshot (holds, background and foreground, private and hidden handling, clipped items joining the base's frame), playback (loop, ping-pong, one-shot, direction, fps), onion skins and their tinted rendering, and export sequencing for `eskiz-media`.

**Entry points.** Build a `StackSnapshot { width, height, items: Vec<StackItem> }` (bottom first; a group is one item flattened by the caller), then `FrameList::build(snapshot)?`. `Playback::new(&settings, frame)`, `advance(dt_seconds, &frames)` returns frames stepped. `render_with_onion(&frames, index, &skins, blend)`. `onion_skins(...)`. `export_frames(&settings, &frames)` returns `Vec<eskiz_media::Frame>`.

**Conversions.** Items carry `eskiz_core::Layer` (premultiplied RGBA8) plus `BlendMode`, flags and `hold`. Rendering is CPU and flattens over white.


**Known gaps.**
- No layer masks on `StackItem`; a masked layer must be baked first. Groups must be flattened by the caller before the snapshot.
- Snapshotting clones every layer. For large documents, build the snapshot from the layers' `Arc`s once you have copy-on-write tiles.
- The frame size is bound by CPU flattening speed; playback at full canvas size on a 4k document will not hold 24 fps. Render onion/frames at preview size.

### 4.8 eskiz-gpu

**Provides.** A headless wgpu 30 backend (the version egui-wgpu 0.36 uses).
- `GpuContext`: prefers a hardware adapter (Metal, then Vulkan, then DX12; discrete over integrated), falls back to software (llvmpipe, lavapipe, WARP), else `GpuError::Unavailable`. `GpuContext::from_parts(adapter, device, queue)` wraps the device eframe already created.
- `GpuLayer` and `GpuMask`: sparse 256 x 256 tiles. Layers are `Rgba8Unorm` premultiplied, masks `R8Unorm`. Tiles are packed into texture-array pages (4, 8, 16, ... 64 slices), so an empty region costs nothing and unallocated mask tiles read as the mask's default value.
- `Compositor`: composites a `Scene` (layers, groups, clipping, masks, opacity, hidden, background) of all 26 blend modes into an `Rgba8Unorm` output texture, whole canvas or dirty tiles only. The accumulator is `f32`, so there is no 8-bit rounding between layers.
- `Adjuster`: Gaussian (exact weights, vertical then horizontal), HSB, Opacity, Curves (LUTs), Gradient Map, Sharpen, Noise, each with an optional mask. Takes the same parameter structs as `eskiz-filters`.

**Entry points.**

```rust
let ctx = GpuContext::new()?;                           // or from_parts(...) with eframe's device
let mut layer = GpuLayer::new(w, h);
layer.upload_rect(&ctx, rect, &crop_of_premul_rgba8)?;  // returns the Rect to mark dirty
let mut scene = Scene::new();                           // background Some([1,1,1]); None = hidden
scene.push_layer(SceneLayer::new(&layer).with_opacity(0.8).with_blend(BlendMode::Multiply));
// groups: SceneItem::Group(SceneGroup { children, .. }); masks: .with_mask(&gpu_mask); clipping: .clipped()
let mut comp = Compositor::new(&ctx, w, h);
comp.composite(&ctx, &scene, &Region::Rect(dirty))?;    // Region::All, Rect(..) or Tiles(..)
let tex: &wgpu::Texture = comp.output_texture();        // premultiplied RGBA8, TEXTURE_BINDING | COPY_SRC
let png_bytes_source = comp.download_output_all(&ctx)?; // blocking readback for export
let mut adj = Adjuster::new(&ctx);
let new_layer = adj.apply(&ctx, &layer, Some(&mask), &Adjustment::Gaussian(GaussianParams { amount: 0.6 }))?;
```

`composite` only submits work; it does not wait. Readbacks (`download*`) block.

**The painting loop (step 7).** The brush engine stays on the CPU and `eskiz_core::Layer` stays the editing buffer. After each dab batch, take the stroke's `PixelRect`, copy that rectangle out of the CPU layer, `upload_rect` it, then `composite(.., Region::Rect(rect))` and let egui draw the output texture. Measured on an Apple M3: one dirty 256 x 256 tile costs about 1.4 ms and a 1024 x 1024 region about 13 ms with 20 layers (numbers in section 6). Build the `Scene` per composite from the app's layer tree; it only borrows.

**Showing the output in egui.** Done in step 7; see 4.8.1 for how the app does it. The CPU `flatten` path stays as the fallback for `GpuError::Unavailable` and for headless tests.

**Conversions.** `GpuLayer` bytes in and out are exactly `eskiz_core::Layer.pixels` layout. `GpuMask` bytes are `eskiz_select::Mask::to_u8()`. For adjustments, build the `Adjustment` from the same `*Params` structs the CPU filter takes.


**Tests.** 28 tests in 5 files compare the GPU with CPU references on random images: every blend mode (opaque, partial alpha, opacity, transparent backdrop), clipping (including hidden bases and stacked clips), masks, groups, dirty-rect recompositing, multi-page tile stores, and every adjustment. They run on Metal here and **skip with a message** when no adapter exists. Set `ESKIZ_GPU_REQUIRE=1` to make a missing adapter fail instead. Tolerance is 1/255 per byte (a handful of pixels in the largest stress test may be 2 or 3 off at near-grey Hue/Colour edges; Noise has the documented bounds in `tests/adjust.rs`).

**Known gaps.**
- Group opacity, group blend mode and group masks are stored in `SceneGroup` but **not applied**: children composite in place, for now. Isolated groups need an offscreen pass.
- The clip model is Eskiz's own (several clipped layers per base, the base's opacity applied once).
- Adjustments run on whole canvas-sized working textures (8 bytes per pixel for half-float intermediates), then copy tiles back. Pencil-mode pyramid blur, motion blur, perspective blur, bloom, halftone, glitch, chromatic aberration, clone, recolor, colour balance and Liquify are CPU only.
- 8-bit layers only. 16-bit support (roadmap 9.2) would change the tile format and the shader loads.
- No copy-on-write or tile-delta undo yet; `free_tile` and `upload_rect` are the primitives that undo would use. Undo today stores CPU rectangles in `history.rs`.
- Several `GpuLayer`s of the same size allocate independent pages, so a 20-layer 4k document holds about 1.6 GB of tiles. Fine on unified-memory machines; discrete cards with 4 GB need lazy tile eviction.
- Linux CI uses a software Vulkan adapter (`mesa-vulkan-drivers` was added to the apt line in `.github/workflows/ci.yml`). The tests are written to pass on it but that path has not been run from here.

### 4.8.1 The GPU display in the app (step 7, done)

**Renderer.** `eskiz-app` uses eframe's `wgpu` renderer (wgpu 30, the same as `eskiz-gpu`): Metal on macOS, DX12 or Vulkan on Windows, Vulkan or GL on Linux. The eframe `glow` renderer is kept behind the cargo feature `gl-fallback` (on by default; `--no-default-features --features tablet` builds wgpu only). `src/renderer.rs` opens the window; if wgpu cannot start (no adapter, no surface) and the application has not been created yet, it retries once with glow and the CPU compositor, otherwise it returns an error that says what to try. eframe creates the device with the adapter's own limits, so canvases above 8192 px work where the GPU allows.

**`ESKIZ_RENDERER`.** unset or `gpu`: wgpu window, GPU compositor. `cpu`: wgpu window, the old CPU compositor and `TextureHandle` (the ground-truth path). `gl`: glow window, CPU compositor (needs `gl-fallback`). The GPU compositor also steps aside, per document, in two cases: the device lacks compute shaders or the limits (`GpuContext::supports_compositing`), or the document has a group with its own opacity, blend mode or mask, which `eskiz-gpu` does not apply yet (4.8 known gaps). It switches back as soon as the group is plain again.

**Modules.**
- `gpu_canvas.rs`: `GpuCanvas` mirrors the document as one `GpuLayer` (and `GpuMask`) per layer and owns the `Compositor`. No egui in it. `flush(&Session)` uploads what changed and recomposites only the dirty tiles. A layer is re-uploaded when its `Session::layer_epoch` changed (rectangle only if one is pending, else the whole layer); the stroke layer is uploaded inside the stroke's dirty rectangle every frame (its epoch only changes at commit). New layers upload in full, removed layers are dropped, a canvas size change rebuilds everything. Empty tiles are never allocated on either side.
- `gpu_display.rs`: `GpuDisplay` wraps eframe's device with `GpuContext::from_parts`, registers the compositor output with `egui_wgpu::Renderer::register_native_texture` (re-bound with `update_egui_texture_from_wgpu_texture` on resize, so the `TextureId` is stable), runs the first resize and flush in wgpu error scopes (a bad device becomes an error and the CPU display, not a panic), and holds the latency report. `EskizApp::refresh` collects dirty rectangles; `gpu_flush` runs once per frame at the end of `ui_root`, before egui's render pass on the same queue, so a pointer sample shows in the frame that follows it.
- The layer preview (adjustments, transform) is mirrored from `Session::preview_layer` and replaces its layer in the scene.

**Live adjustment preview on the GPU (step 7, item 5).** Opacity, Gaussian Blur and HSB previews run in `eskiz_gpu::Adjuster` straight from the layer's GPU mirror, at full quality (the CPU preview used a downscaled proxy for the blur). The final apply is still `Adjustment::render(.., Quality::Full)` on the CPU, so stored pixels never depend on the GPU. If the GPU path fails or the kind has no shader, the old CPU preview runs. A test checks the GPU preview against the CPU render within 3/255.

**Latency.** In debug builds, or with `ESKIZ_LATENCY=1`, each stroke prints `[eskiz latency] gpu|cpu: N frames; ms mean/max: update .., frame .., gpu-done ..`: the display update on the UI thread, the time from the first pointer sample of a frame until `ui()` returned, and for the GPU path until the GPU finished the composite. `cargo test --release -p eskiz-app gpu_tests::latency_report -- --ignored --nocapture` times the two paths per pointer sample (Apple M3, Metal; "CPU" includes the upload of the texture delta that egui-wgpu would do, "GPU" waits for the GPU to finish):

| Document, brush | CPU compositor + upload | GPU compositor |
|---|---|---|
| 2048 x 1536, 6 layers, 24 px | 0.31 ms median | 0.33 ms median |
| 2048 x 1536, 6 layers, 250 px | 1.67 ms | 0.63 ms |
| 4096 x 4096, 12 layers, 24 px | 0.33 ms | 0.51 ms |
| 4096 x 4096, 12 layers, 250 px | 2.97 ms | 0.91 ms |
| Full recomposite (opacity change), 2048 x 1536, 6 layers | 73 ms | 5.5 ms |
| Full recomposite (opacity change), 4096 x 4096, 12 layers | 690 ms | 58 ms |

Small dabs are about equal (the CPU path flattens only a 30 px square); the GPU path wins as the dirty area and the layer count grow, and by 12 to 13 times on anything that recomposites the whole canvas (opacity or blend sliders, reorder, hide, undo of such steps). The brush engine itself (0.05 to 1 ms here) is now the larger part of a stroke sample.

**Tests.** `gpu_tests.rs` runs the headless app with a real wgpu device (`RenderState` built by hand) and checks the GPU composite against `Session::flatten_display` after strokes, undo, redo, opacity, blend, hide, masks, clipping, reorder, fill, duplicate, flip, background, merge, delete, new document sizes, group fallback and adjustments. They skip with a message when no adapter exists (`ESKIZ_GPU_REQUIRE=1` makes that a failure). `--selftest` also compares the two compositors when an adapter exists and prints `skipped` otherwise.

**Known limits.** Group opacity, blend and masks fall back to the CPU display (above). The Frames onion-skin view and the 3D view still draw CPU textures. `Refresh::Rect` is trusted: an edit that changes a layer outside its reported rectangle without a full refresh would show stale pixels until the next full one. Text layers upload their re-rendered raster like any other layer change.

### 4.9 eskiz-guides

**Provides.** Drawing guides (2D grid, isometric, perspective with 1, 2 or 3 vanishing points, symmetry: vertical, horizontal, quadrant, radial), Guided Layer (snapping and mirrored strokes), and colour maths: colour disc geometry and hit-testing, HSB/RGB, hex, colour history (10), harmony, palette extraction by k-means, sRGB to Display P3.

**Entry points.** `GuideRender::lines(viewport)` / `render(viewport)` return canvas-space `Segment`s to paint (`Grid`, `Isometric`, `Perspective`, `SnapGuide`). `DrawingAssist { snap, symmetry, lock_distance }` with `begin_stroke()` then `StrokeAssist::push(sample)` returns one sample per output stroke. `ColourDisc::new(radius)`, `hit_test`, `colour_at`, `point_for`. `harmony(base, Harmony::..)`. `extract_palette(&[[u8;4]], n)`. `ColourHistory`.

**Conversions.** Positions are canvas pixels in `f64`.
```rust
let s = eskiz_guides::Sample::new(Vec2::new(x, y), pressure);   // from eskiz_core::Sample
// Assist output keeps only pos and pressure. For each output stroke, rebuild the core sample
// from the matching input (same index) and replace x, y: tilt, azimuth, roll and t come from the input.
```
`extract_palette` wants straight RGBA bytes. Layer pixels are premultiplied, so un-premultiply first (for example through `eskiz_select::Image::from_premul_rgba8` and a straight-byte export), or the colours at partial alpha come out dark.


**Known gaps.**
- Guides produce geometry only; drawing the lines, handles for moving vanishing points, and the per-layer "Guided Layer" toggle are UI work.
- `DrawingAssist` mirrors in canvas space but does not know about the view; symmetry centre handles need view transforms.
- Per-layer Assist state and its persistence in the document is not defined.

### 4.10 eskiz-brushlib

**Provides.** Forty original presets in eight sets. Each pairs an `eskiz_core::Brush` with a generated 128 x 128 grey tip and an optional 256 x 256 grey grain; all procedural and seeded. Previews for the Brush Library panel.

**Entry points.** `library_ref() -> &'static [BrushSet]` (textures built once, cached). `LibraryBrush { id, name, brush, tip: GrayImage, grain: Option<GrayImage> }`. `preview_stroke(&LibraryBrush, w, h) -> RgbaImage` (opaque, ink over paper) and `stroke_mask`. `tips::tip` and `grains::grain` generate textures directly.

**To paint with a preset** (uses the new core hooks):
```rust
let raster = DabRaster::with_textures(
    &lb.brush,
    ShapeTexture::from_gray(lb.tip.width() as usize, lb.tip.as_raw()).unwrap(),
    lb.grain.as_ref().and_then(|g| GrainTexture::from_gray(g.width() as usize, g.as_raw())),
);
layer.paint_stroke_with_raster(&stroke, PaintOptions::new(colour), &raster);
```
Cache the `DabRaster` per selected brush rather than rebuilding per stroke.


**Known gaps.**
- `preview.rs` re-implements the dab rasteriser because core had no custom-texture hook. It can now be replaced by `DabRaster::with_textures` plus `paint_stroke_with_raster` on a scratch layer, which removes the duplicated dab maths in that file. Left alone because that crate was being edited.
- Previews are RGBA for a fixed ink and paper; the panel wants off-white on the panel colour so tint at draw time from `stroke_mask`.
- The engine's wet-mix settings are set on some presets, but `eskiz-core` does not yet implement the mixing itself, so those presets paint as plain brushes for now.

### 4.11 eskiz-3d

**Provides.** Headless CPU 3D painting core: glTF/GLB and OBJ load with model checks, orbit camera, a screen to UV lookup buffer, painting a stroke's dabs into texture-space layers with local UV density and gutter dilation, a CPU GGX shading reference, layer-group description of texture sets, and GLB plus per-map PNG export.

**Entry points.** `load_gltf(path, &ImportOptions)` / `load_obj`; `OrbitCamera::{view, projection, orbit, pan, zoom_by, frame}`; `render_lookup(&model, &camera, width, height, RenderOptions) -> LookupBuffer`; `PaintSession::new(brush, PaintOptions, PaintSettings)`, `add_dabs(&model, &lookup, &dabs)`, `apply(&mut model)`, `finish(&mut model)`; `export_glb(&model, path)`, `export_map_pngs`; `model_layer_groups(&model)` for the Layers panel.

**Conversions.** Texture maps are `eskiz_core::Layer` (premultiplied RGBA8). The colour map is sRGB, the rest are linear bytes (`MapKind::is_srgb`). Dabs are `eskiz_core::Dab` in screen space, so the stroke pipeline is unchanged.


**Known gaps.**
- No viewport renderer. `shade` is a CPU reference; a wgpu pipeline for the mesh view (reusing `eskiz-gpu`'s context) is unwritten, and it needs the shader to match `shade` (GGX, tangent frame).
- USDZ deferred by decision.
- The lookup buffer is rendered on the CPU per camera move; large meshes will want caching or a GPU lookup pass.

---

## 5. Error types

| Crate | Type | Notes |
|---|---|---|
| eskiz-format | `Error` (thiserror) with `Result<T>` alias | `Io`, `Zip`, `Plist`, `NotProcreate`, `Format`, `Corrupt`, `Unsupported`, `InvalidInput`, `Encode` |
| eskiz-media | `MediaError` (thiserror) | `InvalidFrame`, `InvalidSettings`, `NoFrames`, `EncoderUnavailable`, `DocumentTooSmall`, `Unsupported`, plus wrapped codec errors |
| eskiz-gpu | `GpuError` (thiserror) | `Unavailable`, `Device`, `Invalid`, `Readback` |
| eskiz-select | `SelectError` (`Copy`, hand-written `Display`) | `SizeMismatch`, `EmptyRect`, `SizeOutOfRange`, `NoContent`, `Singular`, `BadData`, `WrongMode` |
| eskiz-anim | `AnimError` (thiserror) | `EmptyCanvas`, `ItemSize` |
| eskiz-text | `FontError`, `StorageError` (thiserror) | |
| eskiz-3d | `ImportError`, `ExportError` (hand-written) | |
| eskiz-guides | `ParseHexError` | |
| eskiz-filters, eskiz-brushlib | none | infallible except documented panics on wrong buffer sizes |

All implement `std::error::Error + Display`, so one app-level enum with `#[from]` for each, or `Box<dyn Error>`, works. The app's current `Result<_, String>` style (`import.rs`) can keep using `.map_err(|e| e.to_string())` until a real `AppError` is worth it. No crate panics on bad file input; the documented panics are programmer errors (wrong buffer length passed to `eskiz_filters::Image::from_rgba8_premul`).

---

## 6. Threading and performance

- UI budget: never block the UI thread more than 8 ms. `Compositor::composite` and `GpuLayer::upload_rect` only record and submit; `download*`, `GpuContext::new` and `Adjuster::apply`'s first call (pipeline creation) can take longer. Create the context and `Adjuster`/`Compositor` at startup or on a worker.
- CPU filters and selection operations allocate whole-canvas `f32` buffers; run them off the UI thread, and downscale for previews.
- `eskiz-media` encodes on the calling thread. Run exports and timelapse segment finishing on a worker.
- wgpu handles are `Send + Sync` on native; the GPU structs are not `Clone` and mutate through `&mut self` (`GpuLayer::upload_rect`, `Compositor::composite`, `Adjuster::apply`). Keep them behind the one owner.

Benchmark (`cargo run --release -p eskiz-gpu --example bench`, Apple M3, Metal): canvas 4096 x 4096, 20 layers.

| Operation | Time |
|---|---|
| Upload of 20 layers (1.58 GiB of tiles) | 1.3 s |
| Full recomposite, all Normal | about 148 ms/frame |
| Full recomposite, mixed modes, masks and clips | about 211 ms/frame |
| Dirty rect, 1 tile (256 x 256) | about 1.4 ms |
| Dirty rect, 16 tiles (1024 x 1024) | about 12.7 ms |
| Readback of the 4096 x 4096 output | about 40 ms |
| Adjustment HSB, one 4096 x 4096 layer | about 16 ms |
| Adjustment Gaussian amount 0.6 / 1.0 | about 79 ms / 100 ms |

The dirty-rect numbers are the ones that matter for painting. A full recomposite is for zoom, layer property changes and export.

---

### File access

`eskiz-app` asks for paths through `file_access::FileAccess` (`pick(PickRequest) -> Receiver<Picked>`), never by blocking. `EskizApp::request_pick` queues a pick for a `PickPurpose`; `poll_picks` runs every frame and routes a finished pick to the existing action (`start_import_image`, `load_document`, `import_font_action`, `import_brush_file`, `load_reference_image`, `start_export`, palette import and export, gallery import and share). `RfdFileAccess` (cargo feature `native-dialogs`, on by default) uses `rfd`; Android builds use `--no-default-features` and a later storage-access backend behind the same trait. Tests use `MockFileAccess`. Every path text field stays as the fallback and for headless runs.

## 7. What the review changed

Small, safe changes only. Every crate still passes its own tests.

- **eskiz-core:** Soft Light, luminance weights, Lighter/Darker Color luma and tie rule, and the Color Dodge / Color Burn / Divide divisor floor now follow the W3C definitions. Tests updated. Added `BlendMode::to_index`, `ShapeTexture::from_gray`, `GrainTexture::from_gray`, `DabRaster::with_textures`, `Layer::paint_stroke_with_raster`; grain size now comes from the texture.
- **eskiz-format:** added `Document::mask_rgba` (missing mask tiles read `0xFF`).
- **eskiz-select:** added `Image::from_premul_rgba8`, `Image::to_premul_rgba8`, `Mask::to_u8`, `Mask::from_u8` and a test.
- **eskiz-anim:** `MIN_FPS` / `MAX_FPS` are now re-exported from `eskiz-media` instead of duplicated.
- **eskiz-gpu:** completed (adjustment shaders, benchmark, tests, `from_parts`), dropped unused dependencies, and added `mesa-vulkan-drivers` to the Linux CI apt line.

Not changed on purpose (do not "tidy" these without agreement): the different pixel formats (section 2), the top-most-first order in `.procreate` structures, the separate `Rect`/`Vec2` families.

---

## 8. Open decisions and gaps

Needing a decision:
1. **Renderer. Decided and done (step 7, see 4.8.1).** eframe runs on `wgpu`, with `glow` kept as the `gl-fallback` feature. On Android the window uses wgpu on Vulkan only; `gl-fallback` is a `compile_error!` there. The `android` feature builds with `cargo ndk -t arm64-v8a build -p eskiz-app --lib --no-default-features --features android` (A5). Android work resumed on 2026-10-18: A0 to A5 are done, including A4 (system file picker, share sheet, clipboard, Back). The APK stays experimental and is not part of 0.1. Still open: web builds, and whether the shipped Linux build should require a Vulkan or GL driver at install time.
2. **Group isolation.** Decide before the Layers panel exposes group opacity and blend controls, because `eskiz-gpu` ignores them today.
3. **Mask channel and tile origin** in `.procreate` files need one real file to settle (section 4.2).
4. **Where `.brush` becomes `Brush`.** The converter has no home yet; `eskiz-format` has no dependency on `eskiz-core`, and adding one is cheap now but a cycle risk later. A small adapter module in `eskiz-app` avoids the question.

Gaps with no owner yet (also listed per crate): `.brush` to `Brush` converter; selection-aware painting hook in core; text-layer slot in the document model; GPU 3D viewport; tile-delta undo; 16-bit layers.
