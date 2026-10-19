//! Colour palette files: Adobe `.swatches` bundles, Adobe Colour Swatch (`.aco`) and Adobe Swatch
//! Exchange (`.ase`) files.
//!
//! Every colour is RGBA with components in [0, 1]. Files without alpha read as alpha 1.
//! Readers return the palette together with warnings for swatches that were skipped (CMYK and
//! Lab colours, unknown colour models). A skipped swatch never fails the whole import.
//!
//! Layouts follow public descriptions of the ACO and ASE layouts. Open items: the `.swatches`
//! JSON schema beyond `name`, `color` and `title`, and the ACO version 2 name block, whose public descriptions disagree on
//! the length field (see [`read_aco`]).

use std::io::{Cursor, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::{limits, Error, Result};

/// One colour in a palette.
#[derive(Debug, Clone, PartialEq)]
pub struct Swatch {
    /// The swatch label, or `None` when the file has no name.
    pub name: Option<String>,
    /// Red, green, blue and alpha in [0, 1].
    pub rgba: [f32; 4],
}

/// A named list of swatches in on-screen order.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    pub name: String,
    pub swatches: Vec<Swatch>,
}

/// A palette read from a file, with the warnings produced while reading it.
#[derive(Debug, Clone, PartialEq)]
pub struct PaletteRead {
    pub palette: Palette,
    pub warnings: Vec<String>,
}

const SWATCHES_MEMBER: &str = "Swatches.json";
const ACO_TITLE: &str = "Imported from ACO";
const ASE_TITLE: &str = "Imported from ASE";
const UNTITLED: &str = "Untitled palette";

// ------------------------------------------------------------------ shared helpers

fn truncated() -> Error {
    Error::Format("palette file is truncated".into())
}

fn array_at<const N: usize>(b: &[u8], at: usize) -> Result<[u8; N]> {
    b.get(at..at + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(truncated)
}

fn be_u16(b: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(array_at(b, at)?))
}

fn be_u32(b: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(array_at(b, at)?))
}

fn be_f32(b: &[u8], at: usize) -> Result<f32> {
    Ok(f32::from_be_bytes(array_at(b, at)?))
}

fn be_units(b: &[u8]) -> Vec<u16> {
    b.as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect()
}

/// Decodes UTF-16 units up to the first zero unit. Empty names give `None`.
fn name_from_units(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let s = String::from_utf16_lossy(&units[..end]);
    (!s.is_empty()).then_some(s)
}

fn push_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Writes a name as a 16-bit length (units including the terminator), the UTF-16BE units, and
/// a zero terminator. Both ASE and the ACO version 2 name block use this shape.
fn push_name(out: &mut Vec<u8>, name: &str) {
    let units: Vec<u16> = name.encode_utf16().collect();
    push_u16(out, (units.len() + 1) as u16);
    for u in units {
        push_u16(out, u);
    }
    push_u16(out, 0);
}

fn quantize(c: f32) -> u16 {
    (c.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// HSB to RGB. `h` is a fraction of a full turn, `s` and `v` are in [0, 1].
fn hsb_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
    let s = s.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);
    let h6 = h.rem_euclid(1.0) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as u8 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Builds a ZIP archive holding the given members (deflate compressed).
pub(crate) fn zip_members(members: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in members {
        zip.start_file(*name, options)?;
        zip.write_all(data)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// Writes `bytes` to `path` through a temporary file in the same folder, then renames it into
/// place, so the destination is never left half-written.
pub fn save_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "palette".into());
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

// ------------------------------------------------------------------ .swatches

/// Reads a `.swatches` bundle: a ZIP whose `Swatches.json` member holds `{ "swatches": [...] }`.
///
/// Each swatch is `{ "name": string, "color": { "model": "RGB" | "HSB", "components": [...] } }`.
/// RGB components are in [0, 1] (alpha optional). HSB components are hue as a fraction of a
/// turn, then saturation and brightness in [0, 1]. The palette title is the top-level `title`,
/// or else the first swatch's `title`. Other models are skipped with a warning.
pub fn read_swatches(bytes: &[u8]) -> Result<PaletteRead> {
    let mut zip = limits::open_zip(bytes)?;
    let data = limits::read_entry(&mut zip, SWATCHES_MEMBER, limits::MAX_METADATA_BYTES)?
        .ok_or(zip::result::ZipError::FileNotFound)?;
    let text = String::from_utf8(data).map_err(|e| Error::Format(format!("{SWATCHES_MEMBER}: {e}")))?;
    let doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Error::Format(format!("{SWATCHES_MEMBER}: {e}")))?;
    let items = doc
        .get("swatches")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::Format(format!("{SWATCHES_MEMBER} has no swatches array")))?;

    let mut warnings = Vec::new();
    let mut swatches = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let name = item
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let color = item.get("color");
        let model = color
            .and_then(|c| c.get("model"))
            .and_then(|v| v.as_str())
            .unwrap_or("RGB")
            .to_ascii_uppercase();
        let comps: Vec<f64> = color
            .and_then(|c| c.get("components"))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
            .unwrap_or_default();
        let rgba = match (model.as_str(), comps.as_slice()) {
            ("RGB", [r, g, b, rest @ ..]) => {
                Some([*r, *g, *b, rest.first().copied().unwrap_or(1.0)])
            }
            ("HSB", [h, s, v, ..]) => {
                let [r, g, b] = hsb_to_rgb(*h, *s, *v);
                Some([r, g, b, 1.0])
            }
            _ => None,
        };
        match rgba {
            Some(c) => swatches.push(Swatch {
                name,
                rgba: c.map(|x| x as f32),
            }),
            None => warnings.push(format!(
                "swatch {}: color model {model} with {} components is not supported; skipped",
                index + 1,
                comps.len()
            )),
        }
    }

    let title = non_empty(doc.get("title").and_then(|v| v.as_str()))
        .or_else(|| {
            non_empty(
                items
                    .first()
                    .and_then(|s| s.get("title"))
                    .and_then(|v| v.as_str()),
            )
        })
        .unwrap_or_else(|| UNTITLED.into());
    Ok(PaletteRead {
        palette: Palette {
            name: title,
            swatches,
        },
        warnings,
    })
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Writes a `.swatches` bundle with RGB swatches in palette order. The title is written as the
/// top-level `title` and on the first swatch.
pub fn write_swatches(palette: &Palette) -> Result<Vec<u8>> {
    let swatches: Vec<serde_json::Value> = palette
        .swatches
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut obj = serde_json::json!({
                "name": s.name.clone().unwrap_or_default(),
                "color": { "model": "RGB", "components": s.rgba.map(f64::from) },
            });
            if i == 0 {
                obj["title"] = serde_json::Value::from(palette.name.as_str());
            }
            obj
        })
        .collect();
    let doc = serde_json::json!({ "title": palette.name, "swatches": swatches });
    let text = serde_json::to_vec(&doc).map_err(|e| Error::Format(e.to_string()))?;
    zip_members(&[(SWATCHES_MEMBER, &text)])
}

// ------------------------------------------------------------------ ACO

/// Reads an Adobe Colour Swatch file.
///
/// The file holds a version 1 section (colours only), optionally followed by a version 2
/// section (the same colours with names). Each record is a colour-space ID and four 16-bit
/// words, all big-endian. Spaces: 0 RGB (0 to 65535), 1 HSB (hue 0 to 65535 is a full turn),
/// 8 grayscale (0 to 10000). CMYK, Lab and other spaces are skipped with a warning. When a
/// version 2 section exists it is used, since it carries the names.
///
/// Version 2 name block, after the four words: a zero word, a length word counting the UTF-16
/// units that follow (name plus terminator), the units, and the terminating zero. The public
/// descriptions disagree on this length (one says a 32-bit count of characters, another a
/// 16-bit count of units plus one). This reader follows the 16-bit form.
pub fn read_aco(bytes: &[u8]) -> Result<PaletteRead> {
    let mut sections: Vec<(u16, Vec<Swatch>, Vec<String>)> = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let version = be_u16(bytes, pos)?;
        let count = be_u16(bytes, pos + 2)?;
        pos += 4;
        if version > 2 {
            return Err(Error::Format(format!("unsupported ACO version {version}")));
        }
        let mut swatches = Vec::new();
        let mut warnings = Vec::new();
        for index in 0..count {
            let space = be_u16(bytes, pos)?;
            let words = [
                be_u16(bytes, pos + 2)?,
                be_u16(bytes, pos + 4)?,
                be_u16(bytes, pos + 6)?,
                be_u16(bytes, pos + 8)?,
            ];
            pos += 10;
            let mut name = None;
            if version == 2 {
                let (n, next) = read_aco_name(bytes, pos)?;
                name = n;
                pos = next;
            }
            match aco_colour(space, words) {
                Some(rgba) => swatches.push(Swatch { name, rgba }),
                None => warnings.push(format!(
                    "swatch {}: color space {} is not supported; skipped",
                    index + 1,
                    aco_space_name(space)
                )),
            }
        }
        sections.push((version, swatches, warnings));
    }

    let chosen = sections
        .iter()
        .rev()
        .find(|s| s.0 == 2)
        .or_else(|| sections.first())
        .ok_or_else(|| Error::Format("empty ACO file".into()))?;
    Ok(PaletteRead {
        palette: Palette {
            name: ACO_TITLE.into(),
            swatches: chosen.1.clone(),
        },
        warnings: chosen.2.clone(),
    })
}

fn aco_space_name(space: u16) -> String {
    match space {
        2 => "CMYK".into(),
        7 => "Lab".into(),
        9 => "wide CMYK".into(),
        other => other.to_string(),
    }
}

fn aco_colour(space: u16, w: [u16; 4]) -> Option<[f32; 4]> {
    match space {
        0 => Some([
            f32::from(w[0]) / 65535.0,
            f32::from(w[1]) / 65535.0,
            f32::from(w[2]) / 65535.0,
            1.0,
        ]),
        1 => {
            let [r, g, b] = hsb_to_rgb(
                f64::from(w[0]) / 65535.0,
                f64::from(w[1]) / 65535.0,
                f64::from(w[2]) / 65535.0,
            );
            Some([r as f32, g as f32, b as f32, 1.0])
        }
        8 => {
            let g = (f32::from(w[0]) / 10000.0).clamp(0.0, 1.0);
            Some([g, g, g, 1.0])
        }
        _ => None,
    }
}

/// Reads the version 2 name block at `pos`. Returns the name and the position after it.
fn read_aco_name(bytes: &[u8], pos: usize) -> Result<(Option<String>, usize)> {
    // The first word is the zero word and is ignored; the second counts units.
    let units = be_u16(bytes, pos + 2)? as usize;
    let start = pos + 4;
    let body = bytes.get(start..start + 2 * units).ok_or_else(truncated)?;
    Ok((name_from_units(&be_units(body)), start + 2 * units))
}

/// Writes an ACO file with a version 1 section and a version 2 section (the layout Photoshop
/// writes). Colours are RGB, quantised to 16 bits.
pub fn write_aco(palette: &Palette) -> Result<Vec<u8>> {
    let count = u16::try_from(palette.swatches.len())
        .map_err(|_| Error::InvalidInput("too many swatches for an ACO file".into()))?;
    let mut out = Vec::new();
    for version in [1u16, 2] {
        push_u16(&mut out, version);
        push_u16(&mut out, count);
        for s in &palette.swatches {
            push_u16(&mut out, 0); // RGB
            for c in &s.rgba[..3] {
                push_u16(&mut out, quantize(*c));
            }
            push_u16(&mut out, 0);
            if version == 2 {
                push_u16(&mut out, 0);
                push_name(&mut out, s.name.as_deref().unwrap_or(""));
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ ASE

enum AseColour {
    Swatch(Swatch),
    Skipped(String),
}

/// Reads an Adobe Swatch Exchange file.
///
/// Header: `ASEF`, a 16-bit major and minor version, and a 32-bit block count. Each block is a
/// 16-bit type, a 32-bit payload length, and the payload, all big-endian. Colour entries are
/// type `0x0001`: a name (16-bit length counting the terminator, then UTF-16BE units), a model
/// (`RGB `, `Gray`, `LAB ` or `CMYK`), and float components. Group start (`0xC001`) and group
/// end (`0xC002`) blocks bracket a group. Groups are flattened, and the first named group
/// titles the palette. Unnamed palettes are titled "Imported from ASE". RGB and grayscale
/// swatches are read. Lab and CMYK swatches, and unknown models, are skipped with a warning.
pub fn read_ase(bytes: &[u8]) -> Result<PaletteRead> {
    if !bytes.starts_with(b"ASEF") {
        return Err(Error::Format("not an Adobe Swatch Exchange file".into()));
    }
    let count = be_u32(bytes, 8)?;
    let mut pos = 12;
    let mut title = None;
    let mut swatches = Vec::new();
    let mut warnings = Vec::new();
    for block in 0..count {
        let kind = be_u16(bytes, pos)?;
        let len = be_u32(bytes, pos + 2)? as usize;
        let payload = bytes.get(pos + 6..pos + 6 + len).ok_or_else(truncated)?;
        pos += 6 + len;
        match kind {
            0x0001 => match ase_colour(payload)? {
                AseColour::Swatch(s) => swatches.push(s),
                AseColour::Skipped(model) => warnings.push(format!(
                    "block {}: color model {model:?} is not supported; skipped",
                    block + 1
                )),
            },
            0xC001 => {
                if title.is_none() {
                    let n = be_u16(payload, 0)? as usize;
                    let units = be_units(payload.get(2..2 + 2 * n).ok_or_else(truncated)?);
                    title = name_from_units(&units);
                }
            }
            0xC002 => {}
            other => warnings.push(format!("block {}: type {other:#06x} skipped", block + 1)),
        }
    }
    Ok(PaletteRead {
        palette: Palette {
            name: title.unwrap_or_else(|| ASE_TITLE.into()),
            swatches,
        },
        warnings,
    })
}

fn ase_colour(payload: &[u8]) -> Result<AseColour> {
    let name_len = be_u16(payload, 0)? as usize;
    let units = be_units(payload.get(2..2 + 2 * name_len).ok_or_else(truncated)?);
    let name = name_from_units(&units);
    let at = 2 + 2 * name_len;
    let model = String::from_utf8_lossy(payload.get(at..at + 4).ok_or_else(truncated)?)
        .trim_end()
        .to_string();
    let floats = match model.as_str() {
        "RGB" => 3,
        "Gray" => 1,
        _ => return Ok(AseColour::Skipped(model)),
    };
    let value = |i: usize| be_f32(payload, at + 4 + 4 * i);
    let rgba = if floats == 3 {
        [value(0)?, value(1)?, value(2)?, 1.0]
    } else {
        let g = value(0)?;
        [g, g, g, 1.0]
    };
    Ok(AseColour::Swatch(Swatch { name, rgba }))
}

/// Writes an ASE file of RGB colour entries. A non-empty palette name is written as a group
/// around the swatches, so the name survives a round trip.
pub fn write_ase(palette: &Palette) -> Result<Vec<u8>> {
    let grouped = !palette.name.is_empty();
    let blocks = palette.swatches.len() + if grouped { 2 } else { 0 };
    let mut out = b"ASEF".to_vec();
    push_u16(&mut out, 1);
    push_u16(&mut out, 0);
    push_u32(&mut out, blocks as u32);
    if grouped {
        let mut payload = Vec::new();
        push_name(&mut payload, &palette.name);
        push_ase_block(&mut out, 0xC001, &payload);
    }
    for s in &palette.swatches {
        let mut payload = Vec::new();
        push_name(&mut payload, s.name.as_deref().unwrap_or(""));
        payload.extend_from_slice(b"RGB ");
        for c in &s.rgba[..3] {
            payload.extend_from_slice(&c.to_be_bytes());
        }
        push_u16(&mut payload, 2); // normal colour
        push_ase_block(&mut out, 0x0001, &payload);
    }
    if grouped {
        push_ase_block(&mut out, 0xC002, &[]);
    }
    Ok(out)
}

fn push_ase_block(out: &mut Vec<u8>, kind: u16, payload: &[u8]) {
    push_u16(out, kind);
    push_u32(out, payload.len() as u32);
    out.extend_from_slice(payload);
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    fn sample_palette() -> Palette {
        Palette {
            name: "Dusk".into(),
            swatches: vec![
                Swatch {
                    name: Some("Ember".into()),
                    rgba: [1.0, 0.25, 0.0, 1.0],
                },
                Swatch {
                    name: None,
                    rgba: [0.1, 0.2, 0.3, 1.0],
                },
                Swatch {
                    name: Some("Ash \u{e9}".into()),
                    rgba: [0.5, 0.5, 0.5, 1.0],
                },
            ],
        }
    }

    fn assert_close(a: [f32; 4], b: [f32; 4]) {
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x - y).abs() < 2e-5, "{a:?} != {b:?}");
        }
    }

    #[test]
    fn aco_round_trip_keeps_names_and_colours() {
        let palette = sample_palette();
        let bytes = write_aco(&palette).unwrap();
        let read = read_aco(&bytes).unwrap();
        assert_eq!(read.palette.name, "Imported from ACO");
        assert!(read.warnings.is_empty());
        assert_eq!(read.palette.swatches.len(), 3);
        for (got, want) in read.palette.swatches.iter().zip(&palette.swatches) {
            assert_eq!(got.name, want.name);
            assert_close(got.rgba, want.rgba);
        }
    }

    #[test]
    fn aco_v1_only_mixed_spaces_skip_cmyk_with_warning() {
        let mut b = Vec::new();
        push_u16(&mut b, 1);
        push_u16(&mut b, 4);
        // RGB pure red.
        push_u16(&mut b, 0);
        for w in [65535u16, 0, 0, 0] {
            push_u16(&mut b, w);
        }
        // HSB: hue 0, full saturation and brightness, which is red.
        push_u16(&mut b, 1);
        for w in [0u16, 65535, 65535, 0] {
            push_u16(&mut b, w);
        }
        // Grayscale 5000 of 10000.
        push_u16(&mut b, 8);
        for w in [5000u16, 0, 0, 0] {
            push_u16(&mut b, w);
        }
        // CMYK, skipped.
        push_u16(&mut b, 2);
        for w in [1u16, 2, 3, 4] {
            push_u16(&mut b, w);
        }
        let read = read_aco(&b).unwrap();
        assert_eq!(read.palette.swatches.len(), 3);
        assert_eq!(read.warnings.len(), 1);
        assert!(read.warnings[0].contains("CMYK"));
        assert_close(read.palette.swatches[0].rgba, [1.0, 0.0, 0.0, 1.0]);
        assert_close(read.palette.swatches[1].rgba, [1.0, 0.0, 0.0, 1.0]);
        assert_close(read.palette.swatches[2].rgba, [0.5, 0.5, 0.5, 1.0]);
        assert!(read.palette.swatches.iter().all(|s| s.name.is_none()));
    }

    #[test]
    fn aco_v2_only_file_reads_names() {
        let mut b = Vec::new();
        push_u16(&mut b, 2);
        push_u16(&mut b, 1);
        push_u16(&mut b, 0);
        for w in [0u16, 65535, 0, 0] {
            push_u16(&mut b, w);
        }
        push_u16(&mut b, 0); // zero word
        push_name(&mut b, "Sky");
        let read = read_aco(&b).unwrap();
        assert_eq!(read.palette.swatches[0].name.as_deref(), Some("Sky"));
        assert_close(read.palette.swatches[0].rgba, [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn ase_round_trip_keeps_title_names_and_colours() {
        let palette = sample_palette();
        let bytes = write_ase(&palette).unwrap();
        let read = read_ase(&bytes).unwrap();
        assert!(read.warnings.is_empty());
        assert_eq!(read.palette.name, "Dusk");
        assert_eq!(read.palette.swatches.len(), 3);
        for (got, want) in read.palette.swatches.iter().zip(&palette.swatches) {
            assert_eq!(got.name, want.name);
            assert_close(got.rgba, want.rgba);
        }
    }

    #[test]
    fn ase_groups_gray_and_skipped_models() {
        let mut out = b"ASEF".to_vec();
        push_u16(&mut out, 1);
        push_u16(&mut out, 0);
        push_u32(&mut out, 5);
        let mut group = Vec::new();
        push_name(&mut group, "Reds");
        push_ase_block(&mut out, 0xC001, &group);
        let mut rgb = Vec::new();
        push_name(&mut rgb, "Red");
        rgb.extend_from_slice(b"RGB ");
        for c in [1.0f32, 0.0, 0.0] {
            rgb.extend_from_slice(&c.to_be_bytes());
        }
        push_u16(&mut rgb, 2);
        push_ase_block(&mut out, 0x0001, &rgb);
        let mut gray = Vec::new();
        push_name(&mut gray, "");
        gray.extend_from_slice(b"Gray");
        gray.extend_from_slice(&0.25f32.to_be_bytes());
        push_u16(&mut gray, 2);
        push_ase_block(&mut out, 0x0001, &gray);
        let mut lab = Vec::new();
        push_name(&mut lab, "Lab");
        lab.extend_from_slice(b"LAB ");
        lab.extend_from_slice(&[0u8; 12]);
        push_u16(&mut lab, 2);
        push_ase_block(&mut out, 0x0001, &lab);
        push_ase_block(&mut out, 0xC002, &[]);

        let read = read_ase(&out).unwrap();
        assert_eq!(read.palette.name, "Reds");
        assert_eq!(read.palette.swatches.len(), 2);
        assert_eq!(read.palette.swatches[0].name.as_deref(), Some("Red"));
        assert_eq!(read.palette.swatches[1].name, None);
        assert_close(read.palette.swatches[1].rgba, [0.25, 0.25, 0.25, 1.0]);
        assert_eq!(read.warnings.len(), 1);
    }

    #[test]
    fn ase_rejects_bad_signature() {
        assert!(matches!(read_ase(b"NOPE1234"), Err(Error::Format(_))));
    }

    #[test]
    fn swatches_round_trip_and_member_layout() {
        let palette = sample_palette();
        let bytes = write_swatches(&palette).unwrap();

        let mut zip = ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let mut json = String::new();
        zip.by_name("Swatches.json")
            .unwrap()
            .read_to_string(&mut json)
            .unwrap();
        let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(doc["swatches"].as_array().unwrap().len(), 3);

        let read = read_swatches(&bytes).unwrap();
        assert!(read.warnings.is_empty());
        assert_eq!(read.palette, palette);
    }

    #[test]
    fn swatches_reads_hsb_and_skips_cmyk() {
        let json = br#"{"title":"Mixed","swatches":[
            {"name":"Red","color":{"model":"HSB","components":[0.0,1.0,1.0]}},
            {"name":"Ink","color":{"model":"CMYK","components":[0.0,0.0,0.0,1.0]}},
            {"color":{"model":"RGB","components":[0.0,0.5,1.0]}}]}"#;
        let bytes = zip_members(&[("Swatches.json", json)]).unwrap();
        let read = read_swatches(&bytes).unwrap();
        assert_eq!(read.palette.name, "Mixed");
        assert_eq!(read.palette.swatches.len(), 2);
        assert_close(read.palette.swatches[0].rgba, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(read.palette.swatches[1].name, None);
        assert_close(read.palette.swatches[1].rgba, [0.0, 0.5, 1.0, 1.0]);
        assert_eq!(read.warnings.len(), 1);
    }
}
