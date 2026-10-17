//! Draws the Eskiz app icon in code and writes the packaging icon set:
//!
//! * `png/eskiz-<N>.png` for each size in `PNG_SIZES`;
//! * `eskiz.icns`, an ICNS container with PNG-based elements;
//! * `eskiz.ico`, a multi-size ICO with PNG-based entries.
//!
//! The artwork is the Eskiz v5 icon (a navy rounded square, a blueprint corner bracket and an
//! orange-to-magenta pointer nib), ported from its design spec into `art.rs`. Every pixel is
//! computed from the design's numbers (no bitmaps are read); each size is drawn directly rather
//! than scaled down from a bitmap.
//!
//! Other modes:
//!
//! ```text
//! eskiz-icon-gen android <res-dir>      Android adaptive icon layers (mipmap-*/ic_launcher_*.png)
//! eskiz-icon-gen compare <reference.png>   difference of the render to a reference PNG
//! ```
//!
//! Run from the `eskiz/` directory:
//!
//! ```text
//! cargo run --release --manifest-path packaging/icon-gen/Cargo.toml
//! ```
//!
//! An optional first argument overrides the output directory (default `packaging/icons`).

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

/// PNG sizes written to `png/`. The 1024 master feeds the other sizes' checks.
const PNG_SIZES: [u32; 8] = [16, 32, 48, 64, 128, 256, 512, 1024];
/// Sizes embedded in the Windows `.ico` (the 256 entry is stored as PNG).
const ICO_SIZES: [u32; 4] = [16, 32, 48, 256];
/// ICNS PNG-based element types, with the pixel size each one holds.
const ICNS_ENTRIES: [(&[u8; 4], u32); 11] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
];

mod art;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        Some("android") => return android(Path::new(args.get(1).ok_or("usage: android <res-dir>")?)),
        Some("compare") => return compare(Path::new(args.get(1).ok_or("usage: compare <reference.png>")?)),
        _ => {}
    }
    let out_dir: PathBuf = match std::env::args_os().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("icons"),
    };
    let png_dir = out_dir.join("png");
    fs::create_dir_all(&png_dir)?;

    let mut rendered: Vec<(u32, Vec<u8>)> = Vec::new();
    for size in PNG_SIZES {
        let data = encode_png(size, &art::render(art::View::full(size), art::Layers::Full))?;
        fs::write(png_dir.join(format!("eskiz-{size}.png")), &data)?;
        rendered.push((size, data));
    }

    let icns_items: Vec<([u8; 4], &[u8])> = ICNS_ENTRIES
        .iter()
        .map(|(ty, size)| (**ty, png_of(&rendered, *size)))
        .collect();
    fs::write(out_dir.join("eskiz.icns"), icns(&icns_items))?;

    let ico_items: Vec<(u32, &[u8])> = ICO_SIZES
        .iter()
        .map(|&size| (size, png_of(&rendered, size)))
        .collect();
    fs::write(out_dir.join("eskiz.ico"), ico(&ico_items))?;

    println!("wrote icon set to {}", out_dir.display());
    Ok(())
}

fn png_of(rendered: &[(u32, Vec<u8>)], size: u32) -> &[u8] {
    rendered
        .iter()
        .find(|(s, _)| *s == size)
        .map(|(_, data)| data.as_slice())
        .expect("every requested size is in PNG_SIZES")
}

fn encode_png(size: u32, rgba: &[u8]) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
    }
    Ok(out)
}

/// ICNS container: `icns` header, then `type` + big-endian length + payload per element.
fn icns(items: &[([u8; 4], &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (ty, data) in items {
        body.extend_from_slice(ty);
        body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(data);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// ICO container with PNG-compressed entries (Vista and later).
fn ico(items: &[(u32, &[u8])]) -> Vec<u8> {
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(items.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * items.len() as u32;
    for (size, data) in items {
        // A directory dimension of 0 means 256.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in items {
        out.extend_from_slice(data);
    }
    out
}

/// Android adaptive icon sizes: the layer is 108 dp; densities scale it.
const DENSITIES: [&str; 5] = ["mdpi", "hdpi", "xhdpi", "xxhdpi", "xxxhdpi"];

/// Radius (in dp) of the circle the artwork must stay inside: the adaptive safe zone is 66 dp
/// across, so 33 dp.
const SAFE_RADIUS_DP: f32 = 33.0;
/// Distance from the artwork's centre to its farthest point (the top-left bracket tip), design units.
const ART_RADIUS: f32 = 438.0;
/// Centre of the artwork's bounding box, design units.
const ART_CENTRE: (f32, f32) = (511.5, 509.0);

fn android(res: &Path) -> Result<(), Box<dyn Error>> {
    // mdpi 1.0, hdpi 1.5, xhdpi 2, xxhdpi 3, xxxhdpi 4.
    let scale_of = |name: &str| match name {
        "mdpi" => 1.0f32,
        "hdpi" => 1.5,
        "xhdpi" => 2.0,
        "xxhdpi" => 3.0,
        _ => 4.0,
    };
    for name in DENSITIES {
        let density = scale_of(name);
        let px = (108.0 * density).round() as u32;
        let dir = res.join(format!("mipmap-{name}"));
        fs::create_dir_all(&dir)?;
        // Background: the whole design canvas fills the layer.
        let bg = art::View { size: px, scale: px as f32 / art::CANVAS, offset: (0.0, 0.0) };
        fs::write(dir.join("ic_launcher_background.png"), encode_png(px, &art::render(bg, art::Layers::Background))?)?;
        // Foreground and monochrome: the artwork fits the 66 dp safe circle, centred.
        let k = SAFE_RADIUS_DP / ART_RADIUS * density; // pixels per design unit
        let half = px as f32 / 2.0;
        let fg = art::View { size: px, scale: k, offset: (half - ART_CENTRE.0 * k, half - ART_CENTRE.1 * k) };
        fs::write(dir.join("ic_launcher_foreground.png"), encode_png(px, &art::render(fg, art::Layers::Foreground))?)?;
        fs::write(dir.join("ic_launcher_monochrome.png"), encode_png(px, &art::render(fg, art::Layers::Monochrome))?)?;
    }
    println!("wrote Android adaptive icon layers under {}", res.display());
    Ok(())
}

/// Difference between the render and a reference PNG (RGBA8, square).
fn compare(reference: &Path) -> Result<(), Box<dyn Error>> {
    let mut decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(reference)?));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("no buffer size")?];
    let info = reader.next_frame(&mut buf)?;
    let size = info.width;
    if info.height != size || info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("the reference must be a square RGBA8 PNG".into());
    }
    let mine = art::render(art::View::full(size), art::Layers::Full);
    let (mut sum_abs, mut sum_sq, mut max, mut over8, mut over32) = (0.0f64, 0.0f64, 0u32, 0u64, 0u64);
    let n = (size * size) as u64;
    for (a, b) in mine.chunks_exact(4).zip(buf.chunks_exact(4)) {
        // Compare premultiplied over black so fully transparent pixels do not count colour noise.
        let (wa, wb) = (f64::from(a[3]) / 255.0, f64::from(b[3]) / 255.0);
        let mut worst = 0u32;
        for c in 0..4 {
            let (x, y) = if c < 3 { (f64::from(a[c]) * wa, f64::from(b[c]) * wb) } else { (f64::from(a[3]), f64::from(b[3])) };
            let d = (x - y).abs();
            sum_abs += d;
            sum_sq += d * d;
            worst = worst.max(d.round() as u32);
        }
        max = max.max(worst);
        over8 += u64::from(worst > 8);
        over32 += u64::from(worst > 32);
    }
    let samples = (n * 4) as f64;
    let mae = sum_abs / samples;
    let rmse = (sum_sq / samples).sqrt();
    let psnr = 20.0 * (255.0 / rmse.max(1e-9)).log10();
    println!(
        "size {size}: mean abs diff {mae:.3}/255, RMSE {rmse:.3}, PSNR {psnr:.1} dB, max channel diff {max}, pixels off by >8: {:.3} %, >32: {:.3} %",
        100.0 * over8 as f64 / n as f64,
        100.0 * over32 as f64 / n as f64
    );
    Ok(())
}
