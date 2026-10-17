//! 16-bit layers on the GPU: half-float tiles, and the composite against the CPU compositor
//! of `eskiz-core` at 16 bits. Skipped when there is no adapter.
mod common;
use common::*;
use eskiz_core::composite::{composite, CompositeOpts};
use eskiz_core::document::{Document, NodeKind, Props};
use eskiz_core::{BlendMode, Depth, Layer};
use eskiz_gpu::{Compositor, GpuLayer, Rect, Region, Scene, SceneLayer};

const W: usize = 530;
const H: usize = 300;

fn random_layer16(rng: &mut Rng, opaque: bool) -> Layer {
    let mut v = Vec::with_capacity(W * H * 4);
    for _ in 0..W * H {
        let a = if opaque {
            65_535
        } else {
            match rng.below(5) {
                0 => 0,
                1 => 65_535,
                _ => rng.below(65_536),
            }
        };
        for _ in 0..3 {
            let c = rng.below(65_536);
            v.push(((u64::from(c) * u64::from(a)) / 65_535) as u16);
        }
        v.push(a as u16);
    }
    Layer::from_rgba16(W, H, v)
}

fn document(rng: &mut Rng) -> Document {
    let mut d = Document::empty_depth(W, H, Depth::U16);
    d.background = [0.9, 0.8, 0.6];
    d.push_raster(None, Props::new("base"), None, random_layer16(rng, true));
    let mut mul = Props::new("mul");
    mul.blend = BlendMode::Multiply;
    mul.opacity = 0.7;
    d.push_raster(None, mul, None, random_layer16(rng, false));
    let mut scr = Props::new("screen");
    scr.blend = BlendMode::Screen;
    scr.opacity = 0.5;
    d.push_raster(None, scr, None, random_layer16(rng, false));
    d
}

fn gpu_scene<'a>(d: &Document, layers: &'a [GpuLayer]) -> Scene<'a> {
    let mut scene = Scene::new();
    scene.background = d.background_visible.then(|| d.background.map(|v| v as f32));
    for (n, gl) in d.root.iter().zip(layers) {
        assert!(matches!(n.kind, NodeKind::Layer));
        scene.push_layer(SceneLayer::new(gl).with_opacity(n.props.opacity as f32).with_blend(n.props.blend));
    }
    scene
}

#[test]
fn the_gpu_composite_of_a_16_bit_document_matches_the_cpu() {
    let Some(ctx) = gpu("depth16::composite") else { return };
    let mut rng = Rng::new(16);
    let d = document(&mut rng);
    let mut layers = Vec::new();
    for n in &d.root {
        let cpu = d.layer(n.id).unwrap();
        let mut gl = GpuLayer::new_depth(W as u32, H as u32, Depth::U16);
        assert_eq!(gl.depth(), Depth::U16);
        gl.upload_rect16(&ctx, Rect::new(0, 0, W as u32, H as u32), &cpu.to_rgba16()).unwrap();
        // Half floats keep at least 11 significant bits: the read back is within 33 of 65535.
        let back = gl.download16(&ctx, Rect::new(0, 0, W as u32, H as u32)).unwrap();
        let worst = back.iter().zip(cpu.to_rgba16()).map(|(a, b)| a.abs_diff(b)).max().unwrap();
        assert!(worst <= 33, "half-float storage error {worst} of 65535");
        layers.push(gl);
    }
    let mut comp = Compositor::new(&ctx, W as u32, H as u32);
    comp.composite(&ctx, &gpu_scene(&d, &layers), &Region::All).unwrap();
    let out = comp.download_output_all(&ctx).unwrap();
    let want = composite(&d, CompositeOpts::default());
    let worst = assert_close("16-bit gpu vs cpu", &out, &want, 2, 0.0, 2);
    eprintln!("  16-bit composite: worst diff {worst} of 255");
}

#[test]
fn an_8_bit_upload_into_a_16_bit_layer_widens_and_tiles_stay_sparse() {
    let Some(ctx) = gpu("depth16::sparse") else { return };
    let mut gl = GpuLayer::new_depth(600, 300, Depth::U16);
    gl.upload_rect(&ctx, Rect::new(0, 0, 600, 300), &vec![0u8; 600 * 300 * 4]).unwrap();
    assert_eq!(gl.tile_count(), 0, "transparent data allocates nothing at 16 bits either");
    let px = vec![200u8; 4 * 4 * 4];
    gl.upload_rect(&ctx, Rect::new(300, 10, 4, 4), &px).unwrap();
    assert_eq!(gl.tile_count(), 1);
    let back = gl.download16(&ctx, Rect::new(300, 10, 4, 4)).unwrap();
    assert!(back.iter().all(|&v| v.abs_diff(200 * 257) <= 33), "{:?}", &back[..4]);
    // A 16-bit tile costs twice an 8-bit one.
    let eight = {
        let mut g = GpuLayer::new(600, 300);
        g.upload_rect(&ctx, Rect::new(300, 10, 4, 4), &px).unwrap();
        g.allocated_bytes()
    };
    assert_eq!(gl.allocated_bytes(), 2 * eight);
}
