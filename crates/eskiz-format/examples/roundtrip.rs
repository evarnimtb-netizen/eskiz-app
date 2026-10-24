//! Reads a `.procreate` file and writes it back out with `eskiz_format::document::write`.
//!
//! Usage: `cargo run -p eskiz-format --example roundtrip -- <in.procreate> <out.procreate>`
//!
//! Metadata, the layer tree, UUIDs and every layer's pixels are kept. Masks, text, video and 3D
//! data are dropped, and the composite and thumbnail are rebuilt. Layers smaller than the canvas
//! are padded with transparent pixels at the top-left.

use std::env;
use std::error::Error;
use std::process::ExitCode;

use eskiz_format::document::{self, DocumentBuilder, GroupSpec, LayerSpec, Node};
use eskiz_format::{Document, Item, Layer};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: roundtrip <in.procreate> <out.procreate>");
        return ExitCode::FAILURE;
    };
    match run(input, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &str, output: &str) -> Result<(), Box<dyn Error>> {
    let mut doc = Document::open(input)?;
    let mut builder = DocumentBuilder::new(doc.width, doc.height);
    builder.name = doc.name.clone().unwrap_or_default();
    builder.tile_size = doc.tile_size;
    builder.orientation = doc.orientation.unwrap_or(1);
    builder.background_hidden = doc.background_hidden;
    if let Some(color) = doc.background_color {
        builder.background_color = color;
    }
    builder.dpi = doc.dpi.unwrap_or(132.0);
    builder.tracked_time = doc.tracked_time.unwrap_or(0.0);
    builder.stroke_count = doc.stroke_count.unwrap_or(0);
    builder.author = doc.author.clone();
    builder.color_profile = doc.color_profile.clone();

    let tree = doc.tree.clone();
    builder.tree = convert(&mut doc, &tree)?;
    let layer_count = doc.layers.len();
    document::write(output, &builder)?;
    println!("wrote {output} with {layer_count} layers");
    Ok(())
}

fn convert(doc: &mut Document, items: &[Item]) -> Result<Vec<Node>, Box<dyn Error>> {
    let mut nodes = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Item::Layer(layer) => nodes.push(Node::Layer(layer_spec(doc, layer)?)),
            Item::Group(group) => nodes.push(Node::Group(GroupSpec {
                name: group.name.clone(),
                opacity: group.opacity,
                hidden: group.hidden,
                collapsed: group.collapsed,
                locked: group.locked,
                private: group.private,
                children: convert(doc, &group.children)?,
                animation_hold: group.animation_hold,
            })),
        }
    }
    Ok(nodes)
}

fn layer_spec(doc: &mut Document, layer: &Layer) -> Result<LayerSpec, Box<dyn Error>> {
    let pixels = doc.layer_rgba(layer)?;
    let (cw, ch) = (doc.width as usize, doc.height as usize);
    let (lw, lh) = (layer.width as usize, layer.height as usize);
    let canvas = if (lw, lh) == (cw, ch) {
        pixels
    } else {
        let mut out = vec![0u8; cw * ch * 4];
        let copy_w = lw.min(cw) * 4;
        for y in 0..lh.min(ch) {
            out[y * cw * 4..y * cw * 4 + copy_w].copy_from_slice(&pixels[y * lw * 4..y * lw * 4 + copy_w]);
        }
        out
    };
    // A canvas-sized mask keeps its grey values (the red channel).
    let mask = match doc.mask_rgba(layer)? {
        Some(rgba) if layer.mask.as_ref().is_some_and(|m| (m.width as usize, m.height as usize) == (cw, ch)) => {
            Some(rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect())
        }
        _ => None,
    };
    Ok(LayerSpec {
        text: None,
        name: layer.name.clone(),
        uuid: Some(layer.uuid.clone()),
        opacity: layer.opacity,
        blend_mode: layer.blend_mode.unwrap_or(0),
        hidden: layer.hidden,
        locked: layer.locked,
        clipped: layer.clipped,
        alpha_locked: layer.alpha_locked,
        private: layer.private,
        animation_hold: layer.animation_hold,
        pixels: Some(canvas),
        pixels16: None,
        tiles: None,
        mask,
    })
}
