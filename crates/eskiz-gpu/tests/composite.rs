//! Compositor behaviour against the f64 reference: opacity, hidden layers, masks, clipping,
//! groups, background, partial canvases and dirty-rectangle recompositing.
mod common;
use common::*;
use eskiz_core::BlendMode;
use eskiz_gpu::{Rect, Region, TILE};

/// Canvas that is not a multiple of the tile size and spans several tiles.
/// Modes without a threshold discontinuity, so f32 (GPU) and f64 (reference) always agree to
/// one 8-bit step. The three discontinuous modes are covered by `tests/blend.rs`.
const CONTINUOUS: [BlendMode; 12] = [
    BlendMode::Normal,
    BlendMode::Exclusion,
    BlendMode::ColorBurn,
    BlendMode::Hue,
    BlendMode::VividLight,
    BlendMode::Divide,
    BlendMode::Lighten,
    BlendMode::ColorDodge,
    BlendMode::Luminosity,
    BlendMode::SoftLight,
    BlendMode::PinLight,
    BlendMode::Add,
];

const W: u32 = 600;
const H: u32 = 530;

fn layer(ctx: &eskiz_gpu::GpuContext, doc: &mut TestDoc, rng: &mut Rng, opaque: bool) -> RefLayer {
    let _ = ctx;
    RefLayer::new(random_premul(rng, doc.w as usize, doc.h as usize, opaque))
}

fn check(name: &str, doc: &TestDoc, ctx: &eskiz_gpu::GpuContext) {
    let (_, out) = doc.render(ctx, &Region::All);
    let want = doc.expected(doc.full());
    let worst = assert_close(name, &out, &want, 1, 0.0, 1);
    eprintln!("  {name:<28} worst diff {worst}");
}

#[test]
fn background_and_empty_stack() {
    let Some(ctx) = gpu("composite::background") else { return };
    let mut doc = TestDoc::new(W, H);
    doc.bg = Some([0.25, 0.5, 0.75]);
    check("colored background", &doc, &ctx);
    doc.bg = None;
    let (_, out) = doc.render(&ctx, &Region::All);
    assert!(out.iter().all(|&b| b == 0), "hidden background gives transparent output");
    doc.bg = Some([1.0; 3]);
    check("white background", &doc, &ctx);
}

#[test]
fn opacity_hidden_and_blend_stack() {
    let Some(ctx) = gpu("composite::stack") else { return };
    let mut rng = Rng::new(20);
    let mut doc = TestDoc::new(W, H);
    let modes = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::Screen,
        BlendMode::SoftLight,
        BlendMode::Normal,
    ];
    for (i, m) in modes.iter().enumerate() {
        let mut l = layer(&ctx, &mut doc, &mut rng, i == 0);
        l.blend = *m;
        l.opacity = [1.0, 0.8, 0.55, 1.0, 0.3, 0.0][i];
        l.visible = i != 3;
        doc.push(&ctx, l);
    }
    check("blend stack", &doc, &ctx);
}

#[test]
fn layer_masks() {
    let Some(ctx) = gpu("composite::masks") else { return };
    let mut rng = Rng::new(21);
    let mut doc = TestDoc::new(W, H);
    let base = layer(&ctx, &mut doc, &mut rng, true);
    doc.push(&ctx, base);
    let mut l = layer(&ctx, &mut doc, &mut rng, false);
    l.mask = Some(random_mask(&mut rng, W as usize, H as usize));
    l.opacity = 0.9;
    l.blend = BlendMode::Multiply;
    doc.push(&ctx, l);
    // A mask that hides everything in some tiles (all zero) and is all ones in others.
    let mut l = layer(&ctx, &mut doc, &mut rng, false);
    let mut m = vec![255u8; (W * H) as usize];
    for y in 0..H as usize {
        for x in 0..W as usize {
            if x < TILE as usize {
                m[y * W as usize + x] = 0;
            } else if x < 400 {
                m[y * W as usize + x] = ((x * 7 + y * 3) % 256) as u8;
            }
        }
    }
    l.mask = Some(m);
    doc.push(&ctx, l);
    check("masks", &doc, &ctx);
}

#[test]
fn clipping_masks() {
    let Some(ctx) = gpu("composite::clip") else { return };
    let mut rng = Rng::new(22);
    let mut doc = TestDoc::new(W, H);
    let base = layer(&ctx, &mut doc, &mut rng, true);
    doc.push(&ctx, base);
    // Base with partial alpha, a mask, opacity and a blend mode, plus three clipped layers.
    let mut b = layer(&ctx, &mut doc, &mut rng, false);
    b.opacity = 0.6;
    b.blend = BlendMode::Multiply;
    b.mask = Some(random_mask(&mut rng, W as usize, H as usize));
    doc.push(&ctx, b);
    for (i, m) in [BlendMode::Normal, BlendMode::Overlay, BlendMode::Color].iter().enumerate() {
        let mut c = layer(&ctx, &mut doc, &mut rng, false);
        c.clipped = true;
        c.blend = *m;
        c.opacity = 0.9 - 0.2 * i as f32;
        if i == 1 {
            c.mask = Some(random_mask(&mut rng, W as usize, H as usize));
        }
        doc.push(&ctx, c);
    }
    // A hidden clipped layer is skipped.
    let mut c = layer(&ctx, &mut doc, &mut rng, false);
    c.clipped = true;
    c.visible = false;
    doc.push(&ctx, c);
    // A normal layer above ends the clip group.
    let top = layer(&ctx, &mut doc, &mut rng, false);
    doc.push(&ctx, top);
    check("clip group", &doc, &ctx);
}

#[test]
fn clip_edge_cases() {
    let Some(ctx) = gpu("composite::clip_edges") else { return };
    let mut rng = Rng::new(23);
    // Clipped layer with nothing below draws as if it were not clipped.
    let mut doc = TestDoc::new(W, H);
    let mut c = layer(&ctx, &mut doc, &mut rng, false);
    c.clipped = true;
    c.opacity = 0.7;
    doc.push(&ctx, c);
    check("clipped without base", &doc, &ctx);

    // Hidden base hides its clipped layers.
    let mut doc = TestDoc::new(W, H);
    let mut b = layer(&ctx, &mut doc, &mut rng, true);
    b.visible = false;
    doc.push(&ctx, b);
    let mut c = layer(&ctx, &mut doc, &mut rng, false);
    c.clipped = true;
    doc.push(&ctx, c);
    check("hidden base", &doc, &ctx);

    // A clipped layer directly above a group has no base inside the group's parent: it draws
    // unclipped.
    let mut doc = TestDoc::new(W, H);
    let inner = layer(&ctx, &mut doc, &mut rng, false);
    let i = doc.make(&ctx, inner);
    doc.nodes.push(Node::Group {
        visible: true,
        children: vec![Node::Layer(i)],
    });
    let mut c = layer(&ctx, &mut doc, &mut rng, false);
    c.clipped = true;
    doc.push(&ctx, c);
    check("clipped over a group", &doc, &ctx);
}

#[test]
fn groups_composite_in_place() {
    let Some(ctx) = gpu("composite::groups") else { return };
    let mut rng = Rng::new(24);
    let mut doc = TestDoc::new(W, H);
    let base = layer(&ctx, &mut doc, &mut rng, true);
    doc.push(&ctx, base);
    // Group with a Multiply child, nested group, clip inside the group, and a hidden group.
    let mut a = layer(&ctx, &mut doc, &mut rng, false);
    a.blend = BlendMode::Multiply;
    let ia = doc.make(&ctx, a);
    let b = layer(&ctx, &mut doc, &mut rng, false);
    let ib = doc.make(&ctx, b);
    let mut c = layer(&ctx, &mut doc, &mut rng, false);
    c.clipped = true;
    c.blend = BlendMode::Screen;
    let ic = doc.make(&ctx, c);
    let hidden = layer(&ctx, &mut doc, &mut rng, true);
    let ih = doc.make(&ctx, hidden);
    doc.nodes.push(Node::Group {
        visible: true,
        children: vec![
            Node::Layer(ia),
            Node::Group {
                visible: true,
                children: vec![Node::Layer(ib), Node::Layer(ic)],
            },
        ],
    });
    doc.nodes.push(Node::Group {
        visible: false,
        children: vec![Node::Layer(ih)],
    });
    check("nested groups", &doc, &ctx);

    // The documented example: a white Multiply child in a group over red shows red.
    let mut doc = TestDoc::new(8, 8);
    doc.bg = None;
    doc.push(&ctx, RefLayer::new([255, 0, 0, 255].repeat(64)));
    let mut white = RefLayer::new([255u8; 4].repeat(64));
    white.blend = BlendMode::Multiply;
    let i = doc.make(&ctx, white);
    doc.nodes.push(Node::Group {
        visible: true,
        children: vec![Node::Layer(i)],
    });
    let (_, out) = doc.render(&ctx, &Region::All);
    assert_eq!(&out[..4], &[255, 0, 0, 255]);
}

#[test]
fn dirty_rect_recomposite_matches_full() {
    let Some(ctx) = gpu("composite::dirty") else { return };
    let mut rng = Rng::new(25);
    let mut doc = TestDoc::new(W, H);
    for i in 0..4 {
        let mut l = layer(&ctx, &mut doc, &mut rng, i == 0);
        l.opacity = 0.7;
        l.blend = [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Normal][i];
        doc.push(&ctx, l);
    }
    let mut comp = eskiz_gpu::Compositor::new(&ctx, W, H);
    let first = comp.composite(&ctx, &doc.scene(), &Region::All).unwrap();
    assert_eq!(first.tiles, 3 * 3);
    assert_close("first full", &comp.download_output_all(&ctx).unwrap(), &doc.expected(doc.full()), 1, 0.0, 1);

    // Edit the top layer inside tile (2, 1) only and recomposite just that rectangle.
    let top = doc.layers.len() - 1;
    let r = Rect::new(520, 270, 60, 40);
    let patch = random_premul(&mut rng, 60, 40, false);
    doc.gpu[top].0.upload_rect(&ctx, r, &patch).unwrap();
    for y in 0..40usize {
        let i = ((r.y as usize + y) * W as usize + r.x as usize) * 4;
        doc.layers[top].pixels[i..i + 240].copy_from_slice(&patch[y * 240..y * 240 + 240]);
    }
    let stats = comp.composite(&ctx, &doc.scene(), &Region::Rect(r)).unwrap();
    assert_eq!(stats.tiles, 1);
    let out = comp.download_output_all(&ctx).unwrap();
    assert_close("dirty rect", &out, &doc.expected(doc.full()), 1, 0.0, 1);

    // Tiles outside the dirty region were left untouched: change the scene without telling
    // the compositor and the stale tiles must stay as they were.
    let before = comp.download_output_all(&ctx).unwrap();
    doc.layers[0].opacity = 0.2;
    let mut scene = doc.scene();
    scene.background = Some([0.0, 0.0, 0.0]);
    comp.composite(&ctx, &scene, &Region::Tiles(vec![(0, 0)])).unwrap();
    let after = comp.download_output_all(&ctx).unwrap();
    let w = W as usize;
    for y in 0..H as usize {
        for x in 0..w {
            let i = (y * w + x) * 4;
            if x >= TILE as usize || y >= TILE as usize {
                assert_eq!(&before[i..i + 4], &after[i..i + 4], "pixel {x},{y} changed");
            }
        }
    }
    assert_ne!(&before[..4], &after[..4]);
}

#[test]
fn many_layers_across_pages_and_batches() {
    let Some(ctx) = gpu("composite::large") else { return };
    // 2816 x 1536 = 11 x 6 = 66 tiles: more than one 64-tile batch, and 12 layers push the
    // tile store past its first pages.
    let (w, h) = (2816u32, 1536u32);
    let mut rng = Rng::new(26);
    let mut doc = TestDoc::new(w, h);
    for i in 0..12 {
        let mut l = RefLayer::new(random_premul(&mut rng, w as usize, h as usize, i == 0));
        l.opacity = 0.5 + 0.04 * i as f32;
        l.blend = CONTINUOUS[i % CONTINUOUS.len()];
        if i % 4 == 3 {
            l.clipped = true;
        }
        doc.push(&ctx, l);
    }
    let (comp, out) = doc.render(&ctx, &Region::All);
    let _ = comp;
    // ClipColor divides by tiny differences on near-grey colours, so a few thousandths of a
    // percent of bytes may be 2 or 3 steps off.
    assert_close("large", &out, &doc.expected(doc.full()), 1, 0.00005, 3);
}
