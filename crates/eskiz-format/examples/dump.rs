//! Prints the size and layer list of a `.procreate` file, or exports decoded pixels.
//!
//! Usage:
//!   `cargo run -p eskiz-format --example dump -- <file.procreate>`
//!   `cargo run -p eskiz-format --example dump -- <file.procreate> --rgba <layer-index> <out.rgba>`
//!   `cargo run -p eskiz-format --example dump -- <file.procreate> --composite <out.rgba>`

use std::env;
use std::error::Error;
use std::fs;
use std::process::ExitCode;

use eskiz_format::{Document, Item};

type AnyResult = Result<(), Box<dyn Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result: AnyResult = match args.as_slice() {
        [path] => dump(path),
        [path, flag, index, out] if flag == "--rgba" => match index.parse::<usize>() {
            Ok(index) => export_layer(path, index, out),
            Err(_) => Err(format!("bad layer index {index:?}").into()),
        },
        [path, flag, out] if flag == "--composite" => export_composite(path, out),
        _ => {
            eprintln!("usage: dump <file.procreate> [--rgba <layer-index> <out> | --composite <out>]");
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn dump(path: &str) -> AnyResult {
    let mut doc = Document::open(path)?;
    println!("name:        {}", doc.name.as_deref().unwrap_or("(untitled)"));
    println!(
        "canvas:      {} x {} px, tile {}, version {:?}",
        doc.width, doc.height, doc.tile_size, doc.version
    );
    println!(
        "orientation: {:?}  flipH={} flipV={}",
        doc.orientation, doc.flipped_horizontally, doc.flipped_vertically
    );
    println!("layers:");
    print_items(&doc, &doc.tree, 1);
    let thumbnail = doc.thumbnail()?;
    println!("thumbnail:   {} bytes", thumbnail.map_or(0, |t| t.len()));
    Ok(())
}

fn export_layer(path: &str, index: usize, out: &str) -> AnyResult {
    let mut doc = Document::open(path)?;
    let layer = doc.layers.get(index).cloned().ok_or("layer index out of range")?;
    fs::write(out, doc.layer_rgba(&layer)?)?;
    Ok(())
}

fn export_composite(path: &str, out: &str) -> AnyResult {
    let mut doc = Document::open(path)?;
    let pixels = doc.composite_rgba()?.ok_or("no stored composite")?;
    fs::write(out, pixels)?;
    Ok(())
}

fn print_items(doc: &Document, items: &[Item], depth: usize) {
    let pad = "  ".repeat(depth);
    for item in items {
        match item {
            Item::Group(group) => {
                let hidden = if group.hidden { " hidden" } else { "" };
                println!(
                    "{pad}[group] {:?}  {:.0}%{hidden}  ({} items)",
                    group.name,
                    group.opacity * 100.0,
                    group.children.len()
                );
                print_items(doc, &group.children, depth + 1);
            }
            Item::Layer(layer) => {
                let flags: Vec<&str> = [
                    (layer.hidden, "hidden"),
                    (layer.clipped, "clip"),
                    (layer.alpha_locked, "alpha-lock"),
                    (layer.locked, "locked"),
                    (layer.mask.is_some(), "mask"),
                    (layer.is_text, "text"),
                ]
                .into_iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| name)
                .collect();
                let flags = if flags.is_empty() {
                    String::new()
                } else {
                    format!(" {}", flags.join(","))
                };
                let name = if layer.name.is_empty() {
                    "(unnamed)"
                } else {
                    layer.name.as_str()
                };
                println!(
                    "{pad}{name}  [{}, {:.0}%]{flags}  {}x{}  tiles={}  uuid={}",
                    layer.blend_name().unwrap_or_else(|| "null".into()),
                    layer.opacity * 100.0,
                    layer.width,
                    layer.height,
                    doc.tile_count(&layer.uuid),
                    layer.uuid
                );
            }
        }
    }
}
