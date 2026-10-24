//! The Liquify tool (with Eskiz's own decisions where the behaviour was left open).
//!
//! Adjustments > Liquify opens the mode on the active layer. Strokes drive the
//! `eskiz_filters::liquify` engine, which builds a displacement field from particles. The canvas
//! shows the layer warped through the field on every frame (a backward bilinear warp, section
//! 19.7). Done commits the warp as one undo step named "Liquify"; Cancel discards it.
//!
//! Speed: on big layers the stroke runs on a reduced grid. The grid is `1/k` of the layer in each
//! direction (`k` from [`preview_scale`]). The engine's particles, field and preview all live on
//! that grid, and the brush radius is scaled to match, so the same brush covers the same area.
//! The live preview is the warp on the grid, shown at the grid's size (each grid texel repeated
//! into the canvas-sized preview layer, which the view then scales to the screen). Commit warps the
//! layer at full resolution, once, straight from the stored RGBA8 and the grid field, a row at a
//! time, so it holds no full-resolution float buffer.
//!
//! After pen-up, momentum keeps the particles moving: [`LiquifyTool::frame`] keeps ticking while
//! any remain, and the caller requests repaints for as long as it returns `true`.
//!
//! The toolbar, the sliders and the brush circle are in [`crate::panels::liquify`].

use eskiz_filters::image::mix_px;
use eskiz_filters::liquify::{radius, Field, Liquify, Mode, Settings, StrokeSample, IDLE_DT};
use eskiz_filters::{mask_at, Image};
use eframe::egui::Pos2;
use rayon::prelude::*;

use eskiz_core::canvas::Layer;
use eskiz_core::filters::upscale_layer;

use crate::app::{over_ui, EskizApp, FrameInput};

/// The stroke modes, in toolbar order, with their labels (section 19.2).
pub const MODES: [(Mode, &str); 8] = [
    (Mode::Push, "Push"),
    (Mode::TwirlRight, "Twirl Right"),
    (Mode::TwirlLeft, "Twirl Left"),
    (Mode::Pinch, "Pinch"),
    (Mode::Expand, "Expand"),
    (Mode::Crystals, "Crystals"),
    (Mode::Edge, "Edge"),
    (Mode::Reconstruct, "Reconstruct"),
];

/// The grid holds about this many pixels or fewer; bigger layers use a coarser grid.
pub const GRID_PIXELS: usize = 1_000_000;
/// Frame times used for the simulation are kept inside this range (seconds).
const MIN_DT: f32 = 1.0 / 480.0;
const MAX_DT: f32 = 1.0 / 20.0;

/// The grid factor `k`: the smallest integer such that the grid, `1/k` of the layer in each
/// direction, holds at most [`GRID_PIXELS`] pixels. Smaller layers run at full resolution.
pub fn preview_scale(width: usize, height: usize) -> usize {
    let px = (width * height).max(1) as f64;
    ((px / GRID_PIXELS as f64).sqrt().ceil() as usize).max(1)
}

/// The brush settings for a grid of factor `k`: the radius in grid pixels is `R / k`, so the
/// brush covers the same part of the layer (`R = 1596 s^2.2 + 4`, section 19.3).
fn grid_settings(s: Settings, k: usize) -> Settings {
    if k == 1 {
        return s;
    }
    let r = radius(s.size) / k as f32;
    let size = if r <= 4.0 { 0.0 } else { ((r - 4.0) / 1596.0).powf(1.0 / 2.2) };
    Settings { size: size.clamp(0.0, 1.0), ..s }
}

/// Texel `j` of a grid of factor `k` covers the full-resolution texels from `j k` to `j k + k - 1`.
/// Its index coordinate in full resolution is `j k + (k - 1) / 2`.
fn grid_to_full(j: f32, k: f32) -> f32 {
    j * k + (k - 1.0) * 0.5
}

/// Warp `src` (the layer on the grid) through `field` (on the grid) with `magnitude` and the
/// selection `mask` at full resolution (section 19.7). Offsets are in grid pixels.
fn warp_grid(src: &Image, field: &Field, magnitude: f32, mask: Option<&[f32]>, full: (usize, usize), k: usize) -> Image {
    let (fw, fh) = full;
    let kf = k as f32;
    Image::generate(src.width, src.height, |x, y| {
        let f = field.get(x, y);
        let sampled = src.sample_texel(x as f32 + magnitude * f[0], y as f32 + magnitude * f[1]);
        let mx = (grid_to_full(x as f32, kf).round() as usize).min(fw - 1);
        let my = (grid_to_full(y as f32, kf).round() as usize).min(fh - 1);
        mix_px(src.px(x, y), sampled, mask_at(mask, my * fw + mx))
    })
}

/// The layer on the grid: premultiplied RGBA8 `pixels` converted to float and averaged over
/// `k x k` blocks (the same values as `Image::from_rgba8_premul(..).downscale_box(k)`, without the
/// full-resolution float image in between).
fn small_from_rgba8(width: usize, height: usize, pixels: &[u8], k: usize) -> Image {
    let k = k.max(1);
    let (gw, gh) = (width.div_ceil(k), height.div_ceil(k));
    let mut data = vec![0.0f32; gw * gh * 4];
    data.par_chunks_mut(gw * 4).enumerate().for_each(|(y, row)| {
        for (x, out) in row.chunks_mut(4).enumerate() {
            let mut acc = [0.0f32; 4];
            let mut n = 0.0f32;
            for sy in (y * k)..((y + 1) * k).min(height) {
                for sx in (x * k)..((x + 1) * k).min(width) {
                    let i = (sy * width + sx) * 4;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += f32::from(pixels[i + c]) / 255.0;
                    }
                    n += 1.0;
                }
            }
            for (o, a) in out.iter_mut().zip(acc) {
                *o = a / n;
            }
        }
    });
    Image { width: gw, height: gh, data }
}

/// Bilinear sample of premultiplied RGBA8 at texel coordinates (clamp-to-edge): the arithmetic of
/// `Image::sample_texel` on the float image that `Image::from_rgba8_premul` would make.
fn sample_rgba8(pixels: &[u8], width: usize, height: usize, fx: f32, fy: f32) -> [f32; 4] {
    let (x0f, y0f) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0f, fy - y0f);
    let (x0, y0) = (x0f as isize, y0f as isize);
    let texel = |x: isize, y: isize| -> [f32; 4] {
        let (cx, cy) = (x.clamp(0, width as isize - 1) as usize, y.clamp(0, height as isize - 1) as usize);
        let i = (cy * width + cx) * 4;
        [0, 1, 2, 3].map(|c| f32::from(pixels[i + c]) / 255.0)
    };
    let (a, b, c, d) = (texel(x0, y0), texel(x0 + 1, y0), texel(x0, y0 + 1), texel(x0 + 1, y0 + 1));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    })
}

/// The Liquify mode's state for one layer.
#[derive(Debug)]
pub struct LiquifyTool {
    /// The layer being warped.
    pub layer: u64,
    /// `Session::edits` when the mode opened. Any other change to the document ends the mode.
    pub edits: u64,
    width: usize,
    height: usize,
    /// Grid factor `k` (see [`preview_scale`]).
    k: usize,
    /// The layer on the grid, premultiplied f32. Used by the previews.
    small: Image,
    /// Selection coverage per full-resolution pixel, `None` when nothing is selected.
    mask: Option<Vec<f32>>,
    /// The brush settings as the toolbar shows them (full-resolution radius).
    user: Settings,
    engine: Liquify,
    stroking: bool,
    /// The field from before a reset. It is shown, fading out, while the reset runs.
    fading_from: Option<Field>,
    last_time: Option<f64>,
    dirty: bool,
}

impl LiquifyTool {
    /// Start the mode on a layer of `width` x `height` with premultiplied RGBA8 `pixels`.
    pub fn new(layer: u64, width: usize, height: usize, pixels: &[u8], mask: Option<Vec<f32>>) -> Self {
        let k = preview_scale(width, height);
        let small = small_from_rgba8(width, height, pixels, k);
        let user = Settings::default();
        let mut engine = Liquify::new(small.width, small.height);
        engine.settings = grid_settings(user, k);
        Self {
            layer,
            edits: 0,
            width,
            height,
            k,
            small,
            mask,
            user,
            engine,
            stroking: false,
            fading_from: None,
            last_time: None,
            dirty: true,
        }
    }

    pub fn settings(&self) -> Settings {
        self.user
    }

    /// Change the brush settings. Used between strokes.
    pub fn set_settings(&mut self, settings: Settings) {
        self.user = settings;
        self.engine.settings = grid_settings(settings, self.k);
    }

    /// The adjust magnitude in [0, 1] (section 19.8).
    pub fn magnitude(&self) -> f32 {
        self.engine.magnitude()
    }

    /// Set the adjust magnitude. Baked into the field when the slider is released.
    pub fn set_magnitude(&mut self, m: f32) {
        self.engine.set_magnitude(m);
        self.dirty = true;
    }

    /// Bake the adjust magnitude into the field (section 19.8).
    pub fn bake(&mut self) {
        self.engine.bake();
        self.dirty = true;
    }

    pub fn is_stroking(&self) -> bool {
        self.stroking
    }

    /// Is a reset fading out?
    pub fn is_fading(&self) -> bool {
        self.fading_from.is_some()
    }

    /// The accurate field, on the grid (section 19.6). During a reset this is already zero.
    pub fn field(&self) -> &Field {
        self.engine.field()
    }

    /// Mutable access to the field, for tests and for restoring a stored field.
    pub fn field_mut(&mut self) -> &mut Field {
        self.dirty = true;
        self.engine.field_mut()
    }

    /// Start a stroke. Ignored while one is running.
    pub fn begin_stroke(&mut self) {
        if self.stroking {
            return;
        }
        self.engine.begin_stroke();
        self.stroking = true;
        self.dirty = true;
    }

    /// Stop the stroke. Momentum may keep particles moving after this, in [`frame`](Self::frame).
    pub fn end_stroke(&mut self) {
        if !self.stroking {
            return;
        }
        self.stroking = false;
        if self.engine.particle_count() == 0 {
            self.engine.end_stroke();
        }
    }

    /// One frame at time `now` (seconds). While stroking, `pen` is the pen's position in layer
    /// pixels, and the stroke grows. The particles run each frame, and any reset fade advances.
    /// Returns `true` while the picture is still changing, so the caller keeps repainting.
    pub fn frame(&mut self, now: f64, pen: Option<StrokeSample>) -> bool {
        let dt = match self.last_time.replace(now) {
            Some(prev) => ((now - prev) as f32).clamp(MIN_DT, MAX_DT),
            None => IDLE_DT,
        };
        if self.stroking {
            if let Some(s) = pen {
                let k = self.k as f32;
                let on_grid = StrokeSample { x: s.x / k, y: s.y / k, pressure: s.pressure };
                self.engine.add_sample(on_grid, dt);
                self.dirty = true;
            }
        }
        if self.engine.particle_count() > 0 {
            self.engine.tick(dt);
            self.dirty = true;
            if !self.stroking && self.engine.particle_count() == 0 {
                self.engine.end_stroke();
            }
        }
        if self.fading_from.is_some() {
            self.engine.advance_fade(dt);
            if !self.engine.is_fading() {
                self.fading_from = None;
            }
            self.dirty = true;
        }
        self.stroking || self.engine.particle_count() > 0 || self.fading_from.is_some()
    }

    /// Reset: the field fades out over [`RESET_SECONDS`](eskiz_filters::liquify::RESET_SECONDS)
    /// (section 19.9). The mode stays open.
    pub fn reset(&mut self) {
        if self.stroking {
            return;
        }
        let before = self.engine.field().clone();
        self.engine.reset();
        self.fading_from = Some(before);
        self.dirty = true;
    }

    /// Is the preview out of date? Clears the flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The field being shown: the pre-reset field while it fades, otherwise the live field.
    fn shown_field(&self) -> &Field {
        self.fading_from.as_ref().unwrap_or_else(|| self.engine.field())
    }

    /// True when the view equals the layer: no magnitude, or a field that is zero everywhere.
    pub fn is_identity(&self) -> bool {
        self.engine.preview_magnitude() == 0.0 || self.shown_field().data.par_iter().all(|v| v[0] == 0.0 && v[1] == 0.0)
    }

    /// The layer as shown on the canvas, or `None` when it equals the layer (the caller then
    /// shows the stored pixels). The warped grid is shown at the grid's size: each grid texel
    /// fills a `k x k` block of the canvas-sized layer, no more than a screen pixel at the zoom the
    /// grid is for, and the view scales the picture to the screen.
    pub fn view_layer(&self) -> Option<Layer> {
        if self.is_identity() {
            return None;
        }
        let grid = warp_grid(
            &self.small,
            self.shown_field(),
            self.engine.preview_magnitude(),
            self.mask.as_deref(),
            (self.width, self.height),
            self.k,
        );
        Some(upscale_layer(&grid.to_rgba8_premul(), grid.width, grid.height, self.width, self.height, self.k))
    }

    /// [`Self::view_layer`] as premultiplied RGBA8.
    pub fn view(&self) -> Option<Vec<u8>> {
        self.view_layer().map(|l| l.to_rgba())
    }

    /// The warped layer at full resolution, premultiplied RGBA8 (section 19.7). `pixels` is the
    /// layer's stored pixels, which do not change while the mode is open.
    ///
    /// Row by row and on several threads: each output pixel reads four stored texels and the grid
    /// field (interpolated to the pixel on the spot), so nothing but the output is allocated.
    pub fn commit(&self, pixels: &[u8]) -> Vec<u8> {
        let (w, h) = (self.width, self.height);
        let m = self.engine.magnitude();
        let mask = self.mask.as_deref();
        let grid = self.engine.field();
        let (kf, half) = (self.k as f32, (self.k as f32 - 1.0) * 0.5);
        let mut out = vec![0u8; w * h * 4];
        out.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
            for (x, px) in row.chunks_mut(4).enumerate() {
                // The field at this pixel, in full-resolution offsets (they scale by `k`).
                let f = if self.k == 1 {
                    grid.get(x, y)
                } else {
                    let v = grid.sample_texel((x as f32 - half) / kf, (y as f32 - half) / kf);
                    [v[0] * kf, v[1] * kf]
                };
                let sampled = sample_rgba8(pixels, w, h, x as f32 + m * f[0], y as f32 + m * f[1]);
                let i = (y * w + x) * 4;
                let own = [0, 1, 2, 3].map(|c| f32::from(pixels[i + c]) / 255.0);
                let mixed = mix_px(own, sampled, mask_at(mask, y * w + x));
                for (o, v) in px.iter_mut().zip(mixed) {
                    *o = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        });
        out
    }
}

// ----- the mode in the app -----

impl EskizApp {
    /// Is the Liquify mode open? Painting, undo and layer edits wait until it closes.
    pub fn liquifying(&self) -> bool {
        self.liquify.is_some()
    }

    /// Open Liquify on the active layer. Refused for groups, locked layers and during a stroke.
    pub fn enter_liquify(&mut self) -> bool {
        if self.refuse_in_3d() {
            return false;
        }
        if self.liquify.is_some() {
            return true;
        }
        if self.session.is_stroking() || self.adjust.is_some() {
            return false;
        }
        let id = self.active;
        if !self.session.can_edit_pixels(id) {
            let msg = if self.session.doc.node(id).is_none_or(|n| n.is_group()) {
                "Select a layer to liquify."
            } else {
                "The layer is locked."
            };
            self.show_toast(msg, 2.0);
            return false;
        }
        self.adjust_cancel();
        self.commit_stroke("liquify opened");
        self.popovers.close();
        let (w, h) = (self.session.doc.width, self.session.doc.height);
        let Some(pixels) = self.session.doc.layer(id).map(|l| l.to_rgba()) else {
            return false;
        };
        let sel = self.session.selection();
        let mask = sel.is_active().then(|| (0..w * h).map(|i| sel.value_at(i % w, i / w)).collect());
        let mut tool = LiquifyTool::new(id, w, h, &pixels, mask);
        tool.edits = self.session.edits();
        self.liquify = Some(tool);
        self.needs_full = true;
        true
    }

    /// Discard the warp and leave the mode.
    pub(crate) fn liquify_cancel(&mut self) {
        if self.liquify.take().is_some() {
            self.session.clear_preview();
            self.gpu_clear_preview();
            self.needs_full = true;
        }
    }

    /// Commit the warp to the layer as one undo step named "Liquify" and leave the mode.
    pub(crate) fn liquify_done(&mut self) {
        let Some(st) = self.liquify.as_ref().filter(|s| !s.is_stroking()) else {
            return;
        };
        let layer = st.layer;
        if st.is_identity() {
            self.liquify_cancel();
            self.show_toast("No change to apply.", 1.5);
            return;
        }
        let Some(before) = self.session.doc.layer(layer).map(|l| l.to_rgba()) else {
            self.liquify_cancel();
            return;
        };
        let mut out = st.commit(&before);
        if self.session.doc.node(layer).is_some_and(|n| n.props.alpha_lock) {
            eskiz_core::filters::keep_alpha(&before, &mut out);
        }
        self.liquify = None;
        self.session.clear_preview();
        self.gpu_clear_preview();
        self.needs_full = true;
        match self.session.replace_layer_pixels(layer, out, "Liquify") {
            Some(edit) => {
                self.apply_refresh(edit.refresh);
                self.show_toast("Liquify", 1.5);
            }
            None => self.show_toast("The liquify could not be applied.", 2.5),
        }
    }

    /// Reset the field (animated, section 19.9).
    pub(crate) fn liquify_reset(&mut self) {
        if let Some(st) = self.liquify.as_mut() {
            st.reset();
        }
    }

    /// Run once per frame while the mode is open, from the canvas pointer handling. It ends the
    /// mode when the document moved on under it, starts and ends strokes from the primary button,
    /// advances the particles (live and after pen-up), and refreshes the preview.
    pub(crate) fn liquify_pointer_frame(&mut self, ctx: &eframe::egui::Context, input: &FrameInput) {
        let stale = match self.liquify.as_ref() {
            Some(st) => {
                !st.is_stroking()
                    && (self.session.edits() != st.edits || self.active != st.layer || self.session.doc.layer(st.layer).is_none())
            }
            None => return,
        };
        if stale {
            self.liquify_cancel();
            return;
        }
        let viewport = self.viewport;
        let on_canvas = |p: Pos2| viewport.contains(p) && !over_ui(ctx, p);
        let press = input.primary_pressed && input.press_origin.is_some_and(on_canvas);
        let pressure = input.pen_samples.last().and_then(|s| s.reading.pressure).unwrap_or(1.0).clamp(0.0, 1.0) as f32;
        let pen = if input.primary_down {
            input.pointer.or(input.press_origin).map(|p| {
                let (x, y) = self.view.screen_to_canvas(viewport, p);
                StrokeSample { x: x as f32, y: y as f32, pressure }
            })
        } else {
            None
        };
        let now = self.now;
        let Some(st) = self.liquify.as_mut() else {
            return;
        };
        if press {
            st.begin_stroke();
        }
        if !input.primary_down {
            st.end_stroke();
        }
        let pen = if st.is_stroking() { pen } else { None };
        let more = st.frame(now, pen);
        let layer = st.layer;
        let shown = if st.take_dirty() { Some(st.view_layer()) } else { None };
        if let Some(view) = shown {
            self.gpu_clear_preview();
            match view {
                Some(px) => self.session.set_preview_layer(layer, px),
                None => {
                    self.session.clear_preview();
                }
            }
            self.needs_full = true;
        }
        if more {
            ctx.request_repaint();
        }
    }
}

// ----- headless checks, shared by the unit tests and `--selftest` -----

/// Push a dot along a stroke, commit, check the dot moved along the stroke, then undo and check
/// the layer is back. Returns how far the dot's centre moved, in pixels.
pub fn check_push_commit_undo() -> Result<f32, String> {
    use crate::document::Document;
    use crate::session::Session;

    let (w, h) = (96usize, 64usize);
    let mut s = Session::new(Document::new(w, h));
    let id = s.doc.layer_id(0).ok_or("no layer")?;
    {
        let layer = s.doc.layer_mut(id).ok_or("no layer")?;
        for y in 0..h {
            for x in 0..w {
                if (x as f32 - 30.0).hypot(y as f32 - 32.0) <= 3.0 {
                    layer.set_pixel(x, y, [40, 90, 200, 255]);
                }
            }
        }
    }
    let before = s.doc.layer(id).ok_or("no layer")?.to_rgba();
    let mut tool = LiquifyTool::new(id, w, h, &before, None);
    tool.set_settings(Settings { mode: Mode::Push, size: TEST_SIZE, ..Settings::default() });
    run_stroke(&mut tool, |i| (26.0 + 34.0 * i as f32 / 39.0, 32.0), 40);
    let out = tool.commit(&before);
    s.replace_layer_pixels(id, out, "Liquify").ok_or("the layer was not replaced")?;
    if s.undo_name() != Some("Liquify") {
        return Err(format!("the commit is named {:?}, not Liquify", s.undo_name()));
    }
    let after = s.doc.layer(id).ok_or("no layer")?.to_rgba();
    let moved = centroid(&after, w).0 - centroid(&before, w).0;
    if moved < 0.5 {
        return Err(format!("the dot did not move along the stroke ({moved:.2} px)"));
    }
    s.undo().ok_or("nothing to undo")?;
    if s.doc.layer(id).ok_or("no layer")?.to_rgba() != before {
        return Err("undo did not restore the layer".into());
    }
    Ok(moved)
}

/// Selftest line for the Liquify tool.
pub fn selftest() -> Result<String, String> {
    let moved = check_push_commit_undo()?;
    Ok(format!(
        "  liquify: push moves a dot {moved:.1} px along the stroke, commit is one step named Liquify, undo restores the layer: ok"
    ))
}

/// Brush size that gives radius R = 12 px (section 19.3, `R = 1596 s^2.2 + 4`).
const TEST_SIZE: f32 = 0.09;

/// Alpha-weighted centre of the painted pixels: (x, y, total alpha).
fn centroid(px: &[u8], w: usize) -> (f32, f32, f32) {
    let (mut sx, mut sy, mut sa) = (0.0f32, 0.0f32, 0.0f32);
    for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
        let a = f32::from(p[3]);
        sx += a * (i % w) as f32;
        sy += a * (i / w) as f32;
        sa += a;
    }
    if sa > 0.0 {
        (sx / sa, sy / sa, sa)
    } else {
        (0.0, 0.0, 0.0)
    }
}

/// Drive a stroke at 60 frames a second through `pos`, then let the particles drain.
fn run_stroke(tool: &mut LiquifyTool, pos: impl Fn(usize) -> (f32, f32), frames: usize) {
    tool.begin_stroke();
    let mut t = 0.0;
    for i in 0..frames {
        let (x, y) = pos(i);
        tool.frame(t, Some(StrokeSample { x, y, pressure: 1.0 }));
        t += 1.0 / 60.0;
    }
    tool.end_stroke();
    drain(tool, &mut t);
}

fn drain(tool: &mut LiquifyTool, t: &mut f64) {
    for _ in 0..5000 {
        *t += 1.0 / 60.0;
        if !tool.frame(*t, None) {
            break;
        }
    }
}

/// A premultiplied RGBA8 canvas with an opaque disc of radius 3 at (`cx`, `cy`).
#[cfg(test)]
fn disc_canvas(w: usize, h: usize, cx: f32, cy: f32) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            if (x as f32 - cx).hypot(y as f32 - cy) <= 3.0 {
                px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[40, 90, 200, 255]);
            }
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dot at (43, 40) on an 80 x 80 canvas, with the stroke centred on (40, 40).
    fn dot_tool(mode: Mode) -> (LiquifyTool, Vec<u8>) {
        let (w, h) = (80, 80);
        let px = disc_canvas(w, h, 43.0, 40.0);
        let mut tool = LiquifyTool::new(1, w, h, &px, None);
        tool.set_settings(Settings { mode, size: TEST_SIZE, ..Settings::default() });
        (tool, px)
    }

    /// Alternate between two points either side of the centre, so every frame has a segment.
    fn jitter_about_centre(i: usize) -> (f32, f32) {
        (if i.is_multiple_of(2) { 39.5 } else { 40.5 }, 40.0)
    }

    #[test]
    fn push_moves_a_dot_along_the_stroke() {
        let (w, h) = (96, 64);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                if (x as f32 - 30.0).hypot(y as f32 - 32.0) <= 3.0 {
                    px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[40, 90, 200, 255]);
                }
            }
        }
        let mut tool = LiquifyTool::new(1, w, h, &px, None);
        tool.set_settings(Settings { mode: Mode::Push, size: TEST_SIZE, ..Settings::default() });
        run_stroke(&mut tool, |i| (26.0 + 34.0 * i as f32 / 39.0, 32.0), 40);
        let out = tool.commit(&px);
        let (c0, c1) = (centroid(&px, w), centroid(&out, w));
        assert!(c1.0 - c0.0 > 0.5, "the dot should move right along the stroke: {} -> {}", c0.0, c1.0);
        assert!((c1.1 - c0.1).abs() < 0.5, "a horizontal push should not move the dot vertically");
    }

    #[test]
    fn twirl_right_rotates_clockwise_on_screen() {
        let (mut tool, px) = dot_tool(Mode::TwirlRight);
        run_stroke(&mut tool, jitter_about_centre, 60);
        let out = tool.commit(&px);
        // A dot to the right of the centre turns clockwise (y down): it moves down the screen.
        let (c0, c1) = (centroid(&px, 80), centroid(&out, 80));
        assert!(c1.1 - c0.1 > 0.5, "clockwise: the dot right of center should move down: {} -> {}", c0.1, c1.1);
    }

    #[test]
    fn twirl_left_rotates_counter_clockwise_on_screen() {
        let (mut tool, px) = dot_tool(Mode::TwirlLeft);
        run_stroke(&mut tool, jitter_about_centre, 60);
        let out = tool.commit(&px);
        let (c0, c1) = (centroid(&px, 80), centroid(&out, 80));
        assert!(c0.1 - c1.1 > 0.5, "counter-clockwise: the dot right of center should move up: {} -> {}", c0.1, c1.1);
    }

    #[test]
    fn pinch_moves_content_inward() {
        let (mut tool, px) = dot_tool(Mode::Pinch);
        run_stroke(&mut tool, jitter_about_centre, 60);
        let out = tool.commit(&px);
        let (c0, c1) = (centroid(&px, 80), centroid(&out, 80));
        assert!(c0.0 - c1.0 > 0.5, "pinch: the dot should move toward the center (x 43 -> 40): {} -> {}", c0.0, c1.0);
    }

    #[test]
    fn expand_moves_content_outward() {
        let (mut tool, px) = dot_tool(Mode::Expand);
        run_stroke(&mut tool, jitter_about_centre, 60);
        let out = tool.commit(&px);
        let (c0, c1) = (centroid(&px, 80), centroid(&out, 80));
        assert!(c1.0 - c0.0 > 0.5, "expand: the dot should move away from the center: {} -> {}", c0.0, c1.0);
    }

    #[test]
    fn reconstruct_reduces_the_field() {
        let (mut tool, px) = dot_tool(Mode::Reconstruct);
        for v in tool.field_mut().data.iter_mut() {
            *v = [4.0, 0.0];
        }
        run_stroke(&mut tool, jitter_about_centre, 60);
        let under = tool.field().get(40, 40);
        let far = tool.field().get(5, 5);
        assert!(under[0].hypot(under[1]) < 0.5, "the brush center should be reconstructed to near zero: {under:?}");
        assert_eq!(far, [4.0, 0.0], "texels away from the brush keep their field");
        assert_eq!(tool.commit(&px).len(), px.len());
    }

    #[test]
    fn reset_returns_to_identity() {
        let (mut tool, px) = dot_tool(Mode::Push);
        run_stroke(&mut tool, |i| (34.0 + 12.0 * i as f32 / 29.0, 40.0), 30);
        assert!(!tool.is_identity(), "the stroke should have warped the view");
        tool.reset();
        // Mid-fade the old field is still shown, weaker.
        let mut t = 100.0;
        for _ in 0..5 {
            t += 1.0 / 60.0;
            tool.frame(t, None);
        }
        assert!(tool.is_fading());
        assert!(tool.view().is_some(), "the warp should still show while it fades");
        for _ in 0..40 {
            t += 1.0 / 60.0;
            tool.frame(t, None);
        }
        assert!(!tool.is_fading());
        assert!(tool.field().data.iter().all(|v| *v == [0.0, 0.0]), "the field should be zero after the reset");
        assert_eq!(tool.magnitude(), 1.0);
        assert!(tool.view().is_none(), "after the reset the view is the layer itself");
        assert_eq!(tool.commit(&px), px, "after the reset the commit is the layer itself");
    }

    #[test]
    fn commit_and_undo_restore_the_layer() {
        let moved = check_push_commit_undo().expect("push, commit and undo");
        assert!(moved > 0.5);
    }

    #[test]
    fn big_layers_stroke_on_a_grid_and_commit_at_full_resolution() {
        let (w, h) = (1600, 1400);
        assert_eq!(preview_scale(w, h), 2, "a 2.2 Mpx layer strokes on a half-size grid");
        let px = disc_canvas(w, h, 800.0, 700.0);
        let mut tool = LiquifyTool::new(1, w, h, &px, None);
        tool.set_settings(Settings { mode: Mode::Push, size: TEST_SIZE, ..Settings::default() });
        run_stroke(&mut tool, |i| (780.0 + 40.0 * i as f32 / 39.0, 700.0), 40);
        let view = tool.view().expect("the warp is visible");
        assert_eq!(view.len(), w * h * 4);
        let out = tool.commit(&px);
        let shift = centroid(&out, w).0 - centroid(&px, w).0;
        assert!(shift > 0.5, "the full-resolution commit moves the dot: {shift}");
        let preview_shift = centroid(&view, w).0 - centroid(&px, w).0;
        assert!((preview_shift - shift).abs() < 1.5, "the preview should agree with the commit: {preview_shift} vs {shift}");
    }

    /// Seeded random premultiplied pixels with transparent, opaque and translucent areas.
    fn noisy_canvas(seed: u64, w: usize, h: usize) -> Vec<u8> {
        let mut state = seed | 1;
        let mut next = move || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 24
        };
        let mut px = vec![0u8; w * h * 4];
        for p in px.as_chunks_mut::<4>().0 {
            let a = [255u64, 255, 0, next() % 256][(next() % 4) as usize] as u8;
            for c in p.iter_mut().take(3) {
                *c = if a == 0 { 0 } else { (next() % (u64::from(a) + 1)) as u8 };
            }
            p[3] = a;
        }
        px
    }

    /// The commit as it was: a full-resolution float source, the field upsampled to full
    /// resolution, a float output.
    fn reference_commit(tool: &LiquifyTool, pixels: &[u8]) -> Vec<u8> {
        let (w, h) = (tool.width, tool.height);
        let source = Image::from_rgba8_premul(w, h, pixels);
        let grid = tool.engine.field();
        let field = if tool.k == 1 {
            grid.clone()
        } else {
            let (kf, half) = (tool.k as f32, (tool.k as f32 - 1.0) * 0.5);
            let mut data = vec![[0.0f32; 2]; w * h];
            for (i, out) in data.iter_mut().enumerate() {
                let (x, y) = (i % w, i / w);
                let v = grid.sample_texel((x as f32 - half) / kf, (y as f32 - half) / kf);
                *out = [v[0] * kf, v[1] * kf];
            }
            Field { width: w, height: h, data }
        };
        let m = tool.engine.magnitude();
        let mask = tool.mask.as_deref();
        Image::generate(w, h, |x, y| {
            let f = field.get(x, y);
            let sampled = source.sample_texel(x as f32 + m * f[0], y as f32 + m * f[1]);
            mix_px(source.px(x, y), sampled, mask_at(mask, y * w + x))
        })
        .to_rgba8_premul()
    }

    /// The grid-wise commit gives the bytes of the full-resolution float one, at full-size grids
    /// and coarse ones, with and without a selection.
    #[test]
    fn commit_equals_the_full_resolution_float_commit() {
        for (w, h) in [(120usize, 90usize), (1301, 1003), (1703, 1201)] {
            for selected in [false, true] {
                let px = noisy_canvas(w as u64, w, h);
                let mask = selected.then(|| (0..w * h).map(|i| if (i % w) < w / 3 { 0.0 } else { ((i % 11) as f32 / 10.0).min(1.0) }).collect());
                let mut tool = LiquifyTool::new(1, w, h, &px, mask);
                tool.set_settings(Settings { mode: Mode::TwirlRight, size: TEST_SIZE * 1.6, ..Settings::default() });
                run_stroke(&mut tool, |i| (w as f32 * 0.5 + 8.0 * (i as f32 * 0.3).cos(), h as f32 * 0.5 + 8.0 * (i as f32 * 0.3).sin()), 50);
                assert!(!tool.is_identity());
                assert_eq!(tool.k, preview_scale(w, h));
                assert!(tool.commit(&px) == reference_commit(&tool, &px), "{w}x{h} selected {selected} k{}", tool.k);
            }
        }
    }

    #[test]
    fn the_grid_is_built_straight_from_the_bytes() {
        for (w, h, k) in [(37usize, 23usize, 1usize), (100, 80, 3), (257, 130, 4)] {
            let px = noisy_canvas(7, w, h);
            let want = Image::from_rgba8_premul(w, h, &px).downscale_box(k);
            let got = small_from_rgba8(w, h, &px, k);
            assert!(got == want, "{w}x{h} k{k}");
        }
    }

    /// The preview layer is the warped grid repeated in `k x k` blocks.
    #[test]
    fn the_preview_layer_is_the_grid_in_blocks() {
        let (w, h) = (1301usize, 1003usize);
        let px = noisy_canvas(3, w, h);
        let mut tool = LiquifyTool::new(1, w, h, &px, None);
        tool.set_settings(Settings { mode: Mode::Push, size: TEST_SIZE * 2.0, ..Settings::default() });
        run_stroke(&mut tool, |i| (600.0 + 3.0 * i as f32, 500.0), 40);
        let layer = tool.view_layer().expect("warped");
        assert_eq!((layer.width, layer.height), (w, h));
        let k = tool.k;
        assert!(k > 1);
        let grid = warp_grid(&tool.small, tool.shown_field(), tool.engine.preview_magnitude(), None, (w, h), k).to_rgba8_premul();
        let gw = w.div_ceil(k);
        let got = layer.to_rgba();
        for &(x, y) in &[(0usize, 0usize), (600, 500), (700, 520), (1300, 1002), (w / 2, h / 3)] {
            let g = ((y / k) * gw + x / k) * 4;
            assert_eq!(got[(y * w + x) * 4..][..4], grid[g..g + 4], "at {x},{y}");
        }
    }

    #[test]
    fn momentum_keeps_particles_moving_after_pen_up() {
        let (mut tool, _) = dot_tool(Mode::Push);
        tool.set_settings(Settings { mode: Mode::Push, size: TEST_SIZE, momentum: 1.0, ..Settings::default() });
        tool.begin_stroke();
        let mut t = 0.0;
        for i in 0..10 {
            tool.frame(t, Some(StrokeSample { x: 30.0 + i as f32, y: 40.0, pressure: 1.0 }));
            t += 1.0 / 60.0;
        }
        tool.end_stroke();
        assert!(tool.frame(t, None), "with momentum the stroke keeps drawing after pen-up");
        let mut frames = 0;
        while tool.frame({ t += 1.0 / 60.0; t }, None) && frames < 5000 {
            frames += 1;
        }
        assert!(frames < 5000, "the momentum drain should finish");
    }
}
