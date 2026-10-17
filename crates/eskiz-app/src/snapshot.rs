//! Headless rendering of the whole app to an RGBA image, for tests and `--screenshot`.
//!
//! egui runs without a window. Its tessellated triangles are rasterised on the CPU
//! (barycentric coverage at pixel centres, bilinear texture sampling, premultiplied
//! "over" blending). The output is deterministic on every platform and needs no GPU,
//! which is what lets CI check the interface on Linux, macOS and Windows alike.

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{
    self, epaint::{ImageDelta, Primitive}, Color32, Event, ImageData, Modifiers, PointerButton, Pos2, RawInput,
    Rect, TextureId, Vec2,
};

use crate::app::{EskizApp, Prefs};
use crate::document::Document;

/// A premultiplied RGBA texture kept on the CPU.
struct Tex {
    w: usize,
    h: usize,
    px: Vec<Color32>,
}

impl Tex {
    fn apply(&mut self, delta: &ImageDelta) {
        let ImageData::Color(img) = &delta.image;
        let [w, h] = img.size;
        match delta.pos {
            None => {
                self.w = w;
                self.h = h;
                self.px = img.pixels.clone();
            }
            Some([x0, y0]) => {
                for y in 0..h {
                    for x in 0..w {
                        let (tx, ty) = (x0 + x, y0 + y);
                        if tx < self.w && ty < self.h {
                            self.px[ty * self.w + tx] = img.pixels[y * w + x];
                        }
                    }
                }
            }
        }
    }

    /// Bilinear sample at normalised `(u, v)`, as premultiplied floats 0..1.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if self.w == 0 || self.h == 0 {
            return [1.0; 4];
        }
        let fx = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let get = |x: usize, y: usize| {
            let c = self.px[y * self.w + x];
            [c.r(), c.g(), c.b(), c.a()].map(|b| f32::from(b) / 255.0)
        };
        let (a, b, c, d) = (get(x0, y0), get(x1, y0), get(x0, y1), get(x1, y1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            out[i] = top + (bot - top) * ty;
        }
        out
    }
}

/// A file handed to the app as if it had been dropped on the window.
#[derive(Debug)]
struct DroppedPath(std::path::PathBuf);

impl egui::DroppedFile for DroppedPath {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// An RGBA8 image (unpremultiplied, opaque after rendering).
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        crate::export::write_png(path, self.width, self.height, &self.rgba)
    }
}

/// The app running without a window.
pub struct Headless {
    pub ctx: egui::Context,
    pub app: EskizApp,
    pub size: Vec2,
    textures: HashMap<TextureId, Tex>,
    time: f64,
    events: Vec<Event>,
    pointer: Pos2,
    dropped: Vec<egui::DroppedFileHandle>,
    /// The AccessKit tree of the last frame, when `ctx.enable_accesskit()` was called.
    pub access: Option<egui::accesskit::TreeUpdate>,
    /// CPU time of the last frame's `App::update` (the UI root), not the software rasteriser.
    /// Native pixels per point reported to egui (QA capture renders at 2x).
    pub ppp: f32,
    pub last_update: std::time::Duration,
    /// Repaint delay the last frame asked for (`ZERO` = immediately, huge = idle).
    pub last_repaint: std::time::Duration,
}

impl Headless {
    /// A window of `size` points showing `doc`, with the given preferences.
    pub fn new(size: Vec2, doc: Document, prefs: Prefs) -> Self {
        Self::from_app(size, |ctx| EskizApp::new(ctx, doc, String::new(), prefs))
    }

    /// A window of `size` points running an app built by `make`.
    pub fn from_app(size: Vec2, make: impl FnOnce(&egui::Context) -> EskizApp) -> Self {
        let ctx = egui::Context::default();
        let app = make(&ctx);
        let mut h = Self {
            ctx,
            app,
            size,
            textures: HashMap::new(),
            time: 0.0,
            events: Vec::new(),
            pointer: Pos2::ZERO,
            dropped: Vec::new(),
            access: None,
            ppp: 1.0,
            last_update: std::time::Duration::ZERO,
            last_repaint: std::time::Duration::MAX,
        };
        // Two frames: the first measures and lays out, the second draws settled.
        h.step();
        h.step();
        h
    }

    /// Step frames, giving background storage jobs real time to finish, until none is pending.
    pub fn settle_storage(&mut self) {
        for _ in 0..600 {
            self.step();
            if !self.app.gal.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.step();
    }

    /// Queue a click at `pos` (press and release over two frames).
    pub fn click(&mut self, pos: Pos2) {
        self.move_to(pos);
        self.press(pos, true);
        self.step();
        self.press(pos, false);
        self.step();
    }

    /// Queue a pointer move.
    pub fn move_to(&mut self, pos: Pos2) {
        self.pointer = pos;
        self.events.push(Event::PointerMoved(pos));
    }

    /// Queue a primary button press or release.
    pub fn press(&mut self, pos: Pos2, pressed: bool) {
        self.pointer = pos;
        self.events.push(Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }

    /// Scroll the content under `pos` by `dy` points (negative reveals what is further down)
    /// and let the smooth scrolling finish.
    pub fn scroll_at(&mut self, pos: Pos2, dy: f32) {
        self.move_to(pos);
        self.step();
        self.event(Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        self.advance(0.5);
    }

    /// Drop a file onto the window on the next frame.
    pub fn drop_file(&mut self, path: &Path) {
        self.dropped.push(std::sync::Arc::new(DroppedPath(path.to_path_buf())));
    }

    /// Queue any egui event.
    pub fn event(&mut self, e: Event) {
        self.events.push(e);
    }

    /// Run one frame of 1/60 s and return the clipped primitives.
    pub fn step(&mut self) -> Vec<egui::ClippedPrimitive> {
        self.time += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo { native_pixels_per_point: Some(self.ppp), ..Default::default() },
            ))
            .collect(),
            time: Some(self.time),
            predicted_dt: 1.0 / 60.0,
            events: std::mem::take(&mut self.events),
            safe_area_insets: Some(self.app.safe.insets()),
            dropped_files: std::mem::take(&mut self.dropped),
            ..RawInput::default()
        };
        let app = &mut self.app;
        let mut spent = std::time::Duration::ZERO;
        let mut out = self.ctx.run_ui(input, |ui| {
            let t0 = std::time::Instant::now();
            if app.lifecycle_on {
                let ctx = ui.ctx().clone();
                app.lifecycle_frame(&ctx);
            }
            app.ui_root(ui);
            spent = t0.elapsed();
        });
        self.last_update = spent;
        self.last_repaint = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(std::time::Duration::MAX, |v| v.repaint_delay);
        self.access = out.platform_output.accesskit_update.take();
        for (id, deltas) in &out.textures_delta.set {
            let tex = self.textures.entry(*id).or_insert(Tex { w: 0, h: 0, px: Vec::new() });
            for delta in deltas {
                tex.apply(delta);
            }
        }
        let prims = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        for id in &out.textures_delta.free {
            self.textures.remove(id);
        }
        out.textures_delta.clear();
        prims
    }

    /// Advance `secs` of time with no input (lets animations finish).
    pub fn advance(&mut self, secs: f64) {
        let frames = (secs * 60.0).ceil() as usize;
        for _ in 0..frames {
            self.step();
        }
    }

    /// Run one frame and rasterise it at `ppp` pixels per point (the window stays `size` points).
    pub fn render_at(&mut self, ppp: f32) -> Image {
        self.ppp = ppp;
        let mut prims = self.step();
        // Tessellated geometry is in points; the rasteriser works in pixels.
        for prim in &mut prims {
            prim.clip_rect = Rect::from_min_max(prim.clip_rect.min * ppp, prim.clip_rect.max * ppp);
            if let Primitive::Mesh(m) = &mut prim.primitive {
                for v in &mut m.vertices {
                    v.pos *= ppp;
                }
            }
        }
        rasterise(&prims, &self.textures, (self.size.x * ppp) as usize, (self.size.y * ppp) as usize)
    }

    /// Render one frame to an image.
    pub fn render(&mut self) -> Image {
        let prims = self.step();
        rasterise(&prims, &self.textures, self.size.x as usize, self.size.y as usize)
    }
}

/// Render a plain egui screen of `size` points, drawn by `draw`, to an image. Used for
/// component sheets such as the icon review, which are not an app window.
pub fn render_ui(size: Vec2, mut draw: impl FnMut(&mut egui::Ui)) -> Image {
    let ctx = egui::Context::default();
    let mut textures: HashMap<TextureId, Tex> = HashMap::new();
    let mut prims = Vec::new();
    // Two frames, as in `Headless::from_app`: the first loads the font atlas.
    for _ in 0..2 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            predicted_dt: 1.0 / 60.0,
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| draw(ui));
        for (id, deltas) in &out.textures_delta.set {
            let tex = textures.entry(*id).or_insert(Tex { w: 0, h: 0, px: Vec::new() });
            for delta in deltas {
                tex.apply(delta);
            }
        }
        prims = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        for id in &out.textures_delta.free {
            textures.remove(id);
        }
        out.textures_delta.clear();
    }
    rasterise(&prims, &textures, size.x as usize, size.y as usize)
}

fn rasterise(prims: &[egui::ClippedPrimitive], textures: &HashMap<TextureId, Tex>, w: usize, h: usize) -> Image {
    // Premultiplied float framebuffer, starting black.
    let mut fb = vec![[0.0f32, 0.0, 0.0, 1.0]; w * h];
    let white = Tex { w: 1, h: 1, px: vec![Color32::WHITE] };
    for prim in prims {
        let Primitive::Mesh(mesh) = &prim.primitive else {
            continue;
        };
        let tex = textures.get(&mesh.texture_id).unwrap_or(&white);
        let clip = prim.clip_rect;
        let cx0 = clip.min.x.max(0.0).floor() as i64;
        let cy0 = clip.min.y.max(0.0).floor() as i64;
        let cx1 = (clip.max.x.min(w as f32).ceil() as i64).min(w as i64);
        let cy1 = (clip.max.y.min(h as f32).ceil() as i64).min(h as i64);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [
                &mesh.vertices[tri[0] as usize],
                &mesh.vertices[tri[1] as usize],
                &mesh.vertices[tri[2] as usize],
            ];
            let (p0, p1, p2) = (v[0].pos, v[1].pos, v[2].pos);
            let area = (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y);
            if area.abs() < 1e-9 {
                continue;
            }
            let x0 = (p0.x.min(p1.x).min(p2.x).floor() as i64).max(cx0);
            let x1 = (p0.x.max(p1.x).max(p2.x).ceil() as i64).min(cx1);
            let y0 = (p0.y.min(p1.y).min(p2.y).floor() as i64).max(cy0);
            let y1 = (p0.y.max(p1.y).max(p2.y).ceil() as i64).min(cy1);
            let cols = v.map(|v| {
                let c = v.color;
                [c.r(), c.g(), c.b(), c.a()].map(|b| f32::from(b) / 255.0)
            });
            for py in y0..y1 {
                for px in x0..x1 {
                    let (x, y) = (px as f32 + 0.5, py as f32 + 0.5);
                    let w0 = ((p1.x - x) * (p2.y - y) - (p2.x - x) * (p1.y - y)) / area;
                    let w1 = ((p2.x - x) * (p0.y - y) - (p0.x - x) * (p2.y - y)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-6 || w1 < -1e-6 || w2 < -1e-6 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let t = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let s = tex.sample(u, t);
                    let dst = &mut fb[py as usize * w + px as usize];
                    let mut src = [0.0f32; 4];
                    for i in 0..4 {
                        src[i] = (w0 * cols[0][i] + w1 * cols[1][i] + w2 * cols[2][i]) * s[i];
                    }
                    let inv = 1.0 - src[3];
                    for i in 0..4 {
                        dst[i] = src[i] + dst[i] * inv;
                    }
                }
            }
        }
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in fb {
        for c in &p[..3] {
            rgba.push((c.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        rgba.push(255);
    }
    Image { width: w, height: h, rgba }
}

/// Where snapshot tests write their PNGs: `eskiz/target/snapshots`.
pub fn snapshot_dir() -> std::path::PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/snapshots");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

impl Headless {
    /// Open `panel` from its top-bar button, as a click would, and let it settle.
    pub fn open_panel(&mut self, panel: crate::popover::Panel) {
        let rect = self.app.anchor_rect(panel);
        self.click(rect.center());
        self.advance(0.4);
    }

    /// The top-bar button rectangle for `panel` from the last frame.
    pub fn button_rect(&self, panel: crate::popover::Panel) -> Option<Rect> {
        self.app.button_rects.iter().find(|(p, _)| *p == panel).map(|(_, r)| *r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::COLOUR_MARGIN;
    use crate::popover::Panel;

    #[test]
    fn colour_circle_keeps_its_right_margin() {
        for width in [900.0, 1280.0, 1600.0] {
            let h = Headless::new(Vec2::new(width, 700.0), Document::new(400, 300), Prefs::default());
            let r = h.button_rect(Panel::Colour).expect("color button");
            assert!(
                (width - r.right() - COLOUR_MARGIN).abs() < 0.5,
                "width {width}: color button ends at {}",
                r.right()
            );
        }
    }

    #[test]
    fn writes_panel_snapshots() {
        let dir = snapshot_dir();
        for light in [false, true] {
            let prefs = Prefs { light, ..Prefs::default() };
            let theme = if light { "light" } else { "dark" };
            let mut h = Headless::new(Vec2::new(1280.0, 860.0), Document::new(2048, 1536), prefs);
            h.render().save_png(&dir.join(format!("main_{theme}.png"))).unwrap();
            for (panel, name) in [
                (Panel::Brush, "brush"),
                (Panel::Layers, "layers"),
                (Panel::Colour, "colour"),
                (Panel::Actions, "actions"),
                (Panel::Adjustments, "adjustments"),
            ] {
                h.open_panel(panel);
                h.render().save_png(&dir.join(format!("{name}_{theme}.png"))).unwrap();
                // Close it again (tap the same button).
                h.open_panel(panel);
            }
        }
    }

    #[test]
    fn renders_the_default_window() {
        let mut h = Headless::new(Vec2::new(1024.0, 700.0), Document::new(800, 600), Prefs::default());
        let img = h.render();
        assert_eq!((img.width, img.height), (1024, 700));
        // The canvas is white and fitted in the middle.
        assert_eq!(&img.pixel(512, 380)[..3], &[255, 255, 255]);
    }
}
