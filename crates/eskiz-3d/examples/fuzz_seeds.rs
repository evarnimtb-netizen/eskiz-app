//! Writes the seed corpus for the `usd` fuzz target in `eskiz/fuzz/`: a small model saved as
//! `.usdz` and `.usda` with this crate's own exporters.
//!
//! Run from `eskiz/`: `cargo run -p eskiz-3d --example fuzz_seeds -- fuzz/seeds`.

#[path = "../tests/common/mod.rs"]
mod common;

use std::fs;
use std::path::Path;

use eskiz_3d::{write_usda, write_usdz_bytes, UsdzOptions};

fn main() {
    let out = std::env::args().nth(1).expect("usage: fuzz_seeds <out-dir>");
    let dir = Path::new(&out).join("usd");
    fs::create_dir_all(&dir).expect("create seed folder");
    let triangle = common::triangle_model();
    let cube = common::cube_model(&["Skin", "Metal"], 4);
    fs::write(dir.join("triangle.usda"), write_usda(&triangle)).expect("write");
    fs::write(dir.join("cube.usda"), write_usda(&cube)).expect("write");
    let usdz = write_usdz_bytes(&triangle, &UsdzOptions::default()).expect("usdz");
    fs::write(dir.join("triangle.usdz"), usdz).expect("write");
}
