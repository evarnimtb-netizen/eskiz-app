//! Shared test helpers: adapter acquisition with a clear SKIP, a deterministic RNG and an f64
//! CPU reference implementation of the compositing rules.
#![allow(dead_code)]

use eskiz_core::BlendMode;
use eskiz_gpu::GpuContext;

/// Get a GPU context, or print a SKIP line and return `None` (CI machines may have no GPU).
/// Set `ESKIZ_GPU_REQUIRE=1` to turn a missing adapter into a failure.
pub fn gpu(test: &str) -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(ctx) => {
            eprintln!("[{test}] running on {}", ctx.describe());
            Some(ctx)
        }
        Err(e) => {
            if std::env::var("ESKIZ_GPU_REQUIRE").or_else(|_| std::env::var("EASEL_GPU_REQUIRE" /* legacy-easel */)).is_ok_and(|v| v == "1") {
                panic!("[{test}] GPU required but unavailable: {e}");
            }
            eprintln!("[{test}] SKIPPED: no GPU adapter available ({e})");
            None
        }
    }
}

/// xorshift64* generator.
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }
}

/// Random premultiplied RGBA8 image. Mixes fully transparent, fully opaque and partial alpha
/// pixels and includes the extreme channel values 0 and 255 often.
pub fn random_premul(rng: &mut Rng, w: usize, h: usize, opaque: bool) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 4);
    for _ in 0..w * h {
        let a: u32 = if opaque {
            255
        } else {
            match rng.below(6) {
                0 => 0,
                1 | 2 => 255,
                _ => rng.below(256),
            }
        };
        for _ in 0..3 {
            let c = match rng.below(8) {
                0 => 0,
                1 => 255,
                2 => 128,
                _ => rng.below(256),
            };
            // Premultiply and round, keeping c <= a.
            v.push(((c * a + 127) / 255) as u8);
        }
        v.push(a as u8);
    }
    v
}

pub fn random_mask(rng: &mut Rng, w: usize, h: usize) -> Vec<u8> {
    (0..w * h)
        .map(|_| match rng.below(5) {
            0 => 0,
            1 => 255,
            _ => rng.below(256) as u8,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// f64 reference of the design notes formulas.
// ---------------------------------------------------------------------------

const LUM_W: [f64; 3] = [0.30005, 0.58984, 0.10998];
const LUMA_W: [f64; 3] = [0.29907, 0.58691, 0.11401];

fn lum(c: [f64; 3]) -> f64 {
    LUM_W[0] * c[0] + LUM_W[1] * c[1] + LUM_W[2] * c[2]
}
pub fn luma(c: [f64; 3]) -> f64 {
    LUMA_W[0] * c[0] + LUMA_W[1] * c[1] + LUMA_W[2] * c[2]
}

fn clip_color(c: [f64; 3]) -> [f64; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut o = c;
    if n < 0.0 && l - n > 1e-12 {
        for v in &mut o {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 && x - l > 1e-12 {
        for v in &mut o {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    o
}

fn set_lum(c: [f64; 3], l: f64) -> [f64; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f64; 3]) -> f64 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f64; 3], s: f64) -> [f64; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx > mn {
        c.map(|v| (v - mn) * s / (mx - mn))
    } else {
        [0.0; 3]
    }
}

fn screen(cb: f64, cs: f64) -> f64 {
    cb + cs - cb * cs
}
fn hard_light(cb: f64, cs: f64) -> f64 {
    if cs <= 0.5 {
        cb * 2.0 * cs
    } else {
        screen(cb, 2.0 * cs - 1.0)
    }
}
const DIV_FLOOR: f64 = 1.0 / 16384.0;
fn dodge(cb: f64, cs: f64) -> f64 {
    (cb / (1.0 - cs).max(DIV_FLOOR)).min(1.0)
}
fn burn(cb: f64, cs: f64) -> f64 {
    1.0 - ((1.0 - cb) / cs.max(DIV_FLOOR)).min(1.0)
}
/// Pegtop soft light.
pub fn soft_light(cb: f64, cs: f64) -> f64 {
    if cs <= 0.5 {
        cb * cb * (1.0 - 2.0 * cs) + 2.0 * cb * cs
    } else {
        2.0 * cb * (1.0 - cs) + cb.sqrt() * (2.0 * cs - 1.0)
    }
}

fn sep(mode: BlendMode, cb: f64, cs: f64) -> f64 {
    use BlendMode::*;
    match mode {
        Normal => cs,
        Multiply => cb * cs,
        Screen => screen(cb, cs),
        Add => (cb + cs).min(1.0),
        Lighten => cb.max(cs),
        Exclusion => cb + cs - 2.0 * cb * cs,
        Difference => (cb - cs).abs(),
        Subtract => (cb - cs).max(0.0),
        LinearBurn => (cb + cs - 1.0).max(0.0),
        ColorDodge => dodge(cb, cs),
        ColorBurn => burn(cb, cs),
        Overlay => hard_light(cs, cb),
        HardLight => hard_light(cb, cs),
        SoftLight => soft_light(cb, cs),
        Darken => cb.min(cs),
        HardMix => {
            if cb + cs >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        VividLight => {
            if cs <= 0.5 {
                burn(cb, 2.0 * cs)
            } else {
                dodge(cb, 2.0 * cs - 1.0)
            }
        }
        LinearLight => (cb + 2.0 * cs - 1.0).clamp(0.0, 1.0),
        PinLight => {
            if cs <= 0.5 {
                cb.min(2.0 * cs)
            } else {
                cb.max(2.0 * cs - 1.0)
            }
        }
        Divide => (cb / cs.max(DIV_FLOOR)).min(1.0),
        _ => cs,
    }
}

/// B(Cb, Cs) on straight colours.
pub fn blend_fn(mode: BlendMode, cb: [f64; 3], cs: [f64; 3]) -> [f64; 3] {
    use BlendMode::*;
    match mode {
        Color => set_lum(cs, lum(cb)),
        Luminosity => set_lum(cb, lum(cs)),
        Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        // Ties go to the source colour.
        LighterColor => {
            if luma(cs) >= luma(cb) {
                cs
            } else {
                cb
            }
        }
        DarkerColor => {
            if luma(cs) <= luma(cb) {
                cs
            } else {
                cb
            }
        }
        _ => [
            sep(mode, cb[0], cs[0]),
            sep(mode, cb[1], cs[1]),
            sep(mode, cb[2], cs[2]),
        ],
    }
}

/// True when this (backdrop, source) pair sits within 1e-4 of a discontinuity of the mode,
/// where f32 and f64 may legitimately pick different branches.
pub fn near_discontinuity(mode: BlendMode, cb: [f64; 3], cs: [f64; 3]) -> bool {
    match mode {
        BlendMode::HardMix => (0..3).any(|c| (cb[c] + cs[c] - 1.0).abs() < 1e-4),
        BlendMode::LighterColor | BlendMode::DarkerColor => (luma(cs) - luma(cb)).abs() < 1e-4,
        _ => false,
    }
}

/// For one pixel. `acc` is the premultiplied backdrop.
pub fn composite_px(acc: [f64; 4], cs: [f64; 3], a_s: f64, o: f64, mode: BlendMode) -> [f64; 4] {
    let d = [acc[0], acc[1], acc[2]];
    let ab = acc[3];
    let cb = if ab > 0.0 {
        d.map(|v| (v / ab).clamp(0.0, 1.0))
    } else {
        [0.0; 3]
    };
    let b = if mode == BlendMode::Normal {
        cs
    } else {
        blend_fn(mode, cb, cs)
    };
    let mut out = [0.0; 4];
    for c in 0..3 {
        let co = a_s * ((1.0 - ab) * cs[c] + ab * b[c]) + (1.0 - a_s) * d[c];
        out[c] = (d[c] + o * (co - d[c])).clamp(0.0, 1.0);
    }
    let ao = a_s + (1.0 - a_s) * ab;
    out[3] = (ab + o * (ao - ab)).clamp(0.0, 1.0);
    out
}

#[derive(Clone)]
pub struct RefLayer {
    /// Premultiplied RGBA8, `w * h * 4`.
    pub pixels: Vec<u8>,
    /// Optional mask bytes, `w * h`. `None` = no mask.
    pub mask: Option<Vec<u8>>,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub clipped: bool,
}

impl RefLayer {
    pub fn new(pixels: Vec<u8>) -> Self {
        Self {
            pixels,
            mask: None,
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            clipped: false,
        }
    }
}

#[derive(Clone)]
pub enum RefItem {
    Layer(RefLayer),
    Group { visible: bool, children: Vec<RefItem> },
}

enum RefOp<'a> {
    Draw(&'a RefLayer),
    Clip(&'a RefLayer, Vec<&'a RefLayer>),
}

fn ref_walk<'a>(items: &'a [RefItem], out: &mut Vec<RefOp<'a>>) {
    // (base, hidden?) and the clipped layers attached to it.
    let mut pending: Option<(Option<&'a RefLayer>, Vec<&'a RefLayer>)> = None;
    fn flush<'a>(p: &mut Option<(Option<&'a RefLayer>, Vec<&'a RefLayer>)>, out: &mut Vec<RefOp<'a>>) {
        if let Some((Some(b), c)) = p.take() {
            if c.is_empty() {
                out.push(RefOp::Draw(b));
            } else {
                out.push(RefOp::Clip(b, c));
            }
        }
        *p = None;
    }
    for it in items {
        match it {
            RefItem::Group { visible, children } => {
                flush(&mut pending, out);
                if *visible {
                    ref_walk(children, out);
                }
            }
            RefItem::Layer(l) => {
                if l.clipped {
                    if let Some((b, c)) = pending.as_mut() {
                        if l.visible && b.is_some() {
                            c.push(l);
                        }
                    } else if l.visible {
                        out.push(RefOp::Draw(l));
                    }
                } else {
                    flush(&mut pending, out);
                    pending = Some((l.visible.then_some(l), Vec::new()));
                }
            }
        }
    }
    flush(&mut pending, out);
}

fn px(l: &RefLayer, i: usize) -> ([f64; 3], f64) {
    let p = &l.pixels[i * 4..i * 4 + 4];
    let a = f64::from(p[3]) / 255.0;
    if a <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    (
        [
            (f64::from(p[0]) / 255.0 / a).clamp(0.0, 1.0),
            (f64::from(p[1]) / 255.0 / a).clamp(0.0, 1.0),
            (f64::from(p[2]) / 255.0 / a).clamp(0.0, 1.0),
        ],
        a,
    )
}

fn mask_at(l: &RefLayer, i: usize) -> f64 {
    l.mask.as_ref().map_or(1.0, |m| f64::from(m[i]) / 255.0)
}

/// Reference composite of the rectangle `(x0, y0, w, h)` of a `cw`-wide canvas. Returns
/// premultiplied RGBA8.
pub fn reference_composite(
    cw: usize,
    bg: Option<[f64; 3]>,
    items: &[RefItem],
    rect: (usize, usize, usize, usize),
) -> Vec<u8> {
    let mut ops = Vec::new();
    ref_walk(items, &mut ops);
    let (x0, y0, w, h) = rect;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let i = y * cw + x;
            let mut acc = match bg {
                Some(c) => [c[0], c[1], c[2], 1.0],
                None => [0.0; 4],
            };
            for op in &ops {
                match op {
                    RefOp::Draw(l) => {
                        let (cs, a) = px(l, i);
                        if a <= 0.0 {
                            continue;
                        }
                        let o = f64::from(l.opacity).clamp(0.0, 1.0) * mask_at(l, i);
                        acc = composite_px(acc, cs, a, o, l.blend);
                    }
                    RefOp::Clip(b, cl) => {
                        let (mut cg, a) = px(b, i);
                        let big_a = a * mask_at(b, i);
                        if a <= 0.0 || big_a <= 0.0 {
                            continue;
                        }
                        for c in cl {
                            let (cs, ca) = px(c, i);
                            if ca <= 0.0 {
                                continue;
                            }
                            let w = f64::from(c.opacity).clamp(0.0, 1.0) * mask_at(c, i) * ca;
                            let bl = blend_fn(c.blend, cg, cs);
                            for k in 0..3 {
                                cg[k] = (1.0 - w) * cg[k] + w * bl[k];
                            }
                        }
                        acc = composite_px(acc, cg, big_a, f64::from(b.opacity).clamp(0.0, 1.0), b.blend);
                    }
                }
            }
            for v in acc {
                out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    out
}

/// Largest per-byte difference.
pub fn max_diff(a: &[u8], b: &[u8]) -> (u8, usize) {
    assert_eq!(a.len(), b.len(), "length mismatch");
    let mut worst = 0u8;
    let mut at = 0;
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let d = x.abs_diff(*y);
        if d > worst {
            worst = d;
            at = i;
        }
    }
    (worst, at)
}

// ---------------------------------------------------------------------------
// A test document: the same stack described for the GPU and for the CPU reference.
// ---------------------------------------------------------------------------

use eskiz_gpu::{Compositor, GpuLayer, GpuMask, Rect, Region, Scene, SceneGroup, SceneItem, SceneLayer};

/// Node of the stack description. Layers refer to `TestDoc::layers` by index.
#[derive(Clone)]
pub enum Node {
    Layer(usize),
    Group { visible: bool, children: Vec<Node> },
}

pub struct TestDoc {
    pub w: u32,
    pub h: u32,
    pub bg: Option<[f64; 3]>,
    pub layers: Vec<RefLayer>,
    pub gpu: Vec<(GpuLayer, Option<GpuMask>)>,
    pub nodes: Vec<Node>,
}

impl TestDoc {
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            w,
            h,
            bg: Some([1.0; 3]),
            layers: Vec::new(),
            gpu: Vec::new(),
            nodes: Vec::new(),
        }
    }

    /// Upload a layer (and its mask) and return its index. Does not add it to the stack.
    pub fn make(&mut self, ctx: &GpuContext, l: RefLayer) -> usize {
        let mut gl = GpuLayer::new(self.w, self.h);
        gl.upload_all(ctx, &l.pixels).unwrap();
        let gm = l.mask.as_ref().map(|m| {
            let mut gm = GpuMask::new(self.w, self.h, 1.0);
            gm.upload_rect(ctx, Rect::new(0, 0, self.w, self.h), m).unwrap();
            gm
        });
        self.layers.push(l);
        self.gpu.push((gl, gm));
        self.layers.len() - 1
    }

    /// Upload a layer and put it on top of the root stack.
    pub fn push(&mut self, ctx: &GpuContext, l: RefLayer) -> usize {
        let i = self.make(ctx, l);
        self.nodes.push(Node::Layer(i));
        i
    }

    fn scene_items<'a>(&'a self, nodes: &[Node]) -> Vec<SceneItem<'a>> {
        nodes
            .iter()
            .map(|n| match n {
                Node::Layer(i) => {
                    let r = &self.layers[*i];
                    let (gl, gm) = &self.gpu[*i];
                    let mut s = SceneLayer::new(gl);
                    s.mask = gm.as_ref();
                    s.visible = r.visible;
                    s.opacity = r.opacity;
                    s.blend = r.blend;
                    s.clipped = r.clipped;
                    SceneItem::Layer(s)
                }
                Node::Group { visible, children } => {
                    let mut g = SceneGroup::new(self.scene_items(children));
                    g.visible = *visible;
                    SceneItem::Group(g)
                }
            })
            .collect()
    }

    pub fn scene(&self) -> Scene<'_> {
        let mut s = Scene::new();
        s.background = self.bg.map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]);
        s.items = self.scene_items(&self.nodes);
        s
    }

    fn ref_items(&self, nodes: &[Node]) -> Vec<RefItem> {
        nodes
            .iter()
            .map(|n| match n {
                Node::Layer(i) => RefItem::Layer(self.layers[*i].clone()),
                Node::Group { visible, children } => RefItem::Group {
                    visible: *visible,
                    children: self.ref_items(children),
                },
            })
            .collect()
    }

    /// Reference composite of a rectangle, premultiplied RGBA8.
    pub fn expected(&self, r: Rect) -> Vec<u8> {
        reference_composite(
            self.w as usize,
            self.bg,
            &self.ref_items(&self.nodes),
            (r.x as usize, r.y as usize, r.w as usize, r.h as usize),
        )
    }

    pub fn full(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    /// Composite `region` with a fresh compositor and return the whole output.
    pub fn render(&self, ctx: &GpuContext, region: &Region) -> (Compositor, Vec<u8>) {
        let mut comp = Compositor::new(ctx, self.w, self.h);
        comp.composite(ctx, &self.scene(), region).unwrap();
        let out = comp.download_output_all(ctx).unwrap();
        (comp, out)
    }
}

/// Assert two RGBA8 images agree within `tol` per byte, allowing `max_outlier_frac` of the
/// pixels to exceed it (never by more than `hard`). Returns the worst difference seen.
pub fn assert_close(name: &str, got: &[u8], want: &[u8], tol: u8, max_outlier_frac: f64, hard: u8) -> u8 {
    assert_eq!(got.len(), want.len(), "{name}: length mismatch");
    let mut worst = 0u8;
    let mut outliers = 0usize;
    let mut first = None;
    for (i, (a, b)) in got.iter().zip(want).enumerate() {
        let d = a.abs_diff(*b);
        worst = worst.max(d);
        if d > tol {
            outliers += 1;
            first.get_or_insert((i, *a, *b));
        }
    }
    let frac = outliers as f64 / got.len() as f64;
    assert!(
        frac <= max_outlier_frac && worst <= hard.max(tol),
        "{name}: {outliers} bytes ({:.4}%) differ by more than {tol}, worst {worst}, first (index, gpu, cpu) = {:?}",
        frac * 100.0,
        first
    );
    worst
}
