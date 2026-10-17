//! Minimal stored (method 0) zip writer for USDZ packages.
//!
//! USDZ rules (OpenUSD "Usdz File Format Specification"): no compression, no encryption, and every file's data
//! begins on a 64-byte boundary measured from the start of the package. This writer pads each local header's extra
//! field so the data lands on the boundary. It writes no data descriptors and no zip64 records, so it refuses
//! packages that would need them.

use crate::export::ExportError;

/// Alignment required for every entry's data.
pub const ALIGNMENT: usize = 64;

/// Extra-field header id used for padding (the id `zipalign` uses).
const PAD_ID: u16 = 0xD935;
/// Fixed DOS date 1980-01-01 and time 00:00, so output is deterministic.
const DOS_DATE: u16 = 0x0021;
const DOS_TIME: u16 = 0;

/// CRC-32 (IEEE, as used by zip).
pub fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

fn too_large(what: &str) -> ExportError {
    ExportError::Invalid(format!("USDZ package too large: {what}"))
}

/// Write `entries` (name, data) as a stored zip, in the given order, with each entry's data 64-byte aligned.
///
/// Names must be non-empty, relative, use `/`, and contain no `..` segments or backslashes. Errors above
/// 65,535 entries or 4 GiB (no zip64).
pub fn write_stored_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, ExportError> {
    if entries.is_empty() {
        return Err(ExportError::Invalid("USDZ package has no entries".into()));
    }
    if entries.len() > usize::from(u16::MAX) {
        return Err(too_large("more than 65535 entries"));
    }
    let mut out: Vec<u8> = Vec::new();
    // (name, crc, size, local header offset)
    let mut dir: Vec<(&str, u32, u32, u32)> = Vec::new();
    for (name, data) in entries {
        if name.is_empty()
            || name.starts_with('/')
            || name.contains('\\')
            || name.split('/').any(|s| s == ".." || s.is_empty())
        {
            return Err(ExportError::Invalid(format!("bad package entry name {name:?}")));
        }
        let name_len = u16::try_from(name.len()).map_err(|_| too_large("entry name"))?;
        let size = u32::try_from(data.len()).map_err(|_| too_large("entry over 4 GiB"))?;
        let offset = u32::try_from(out.len()).map_err(|_| too_large("package over 4 GiB"))?;
        let header_end = out.len() + 30 + name.len();
        let mut pad = (ALIGNMENT - header_end % ALIGNMENT) % ALIGNMENT;
        // An extra-field record needs 4 bytes of its own header; if the gap is smaller, skip to the next boundary.
        if pad != 0 && pad < 4 {
            pad += ALIGNMENT;
        }
        let crc = crc32(data);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags: no descriptor, no UTF-8 bit (names are ASCII-safe)
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&DOS_TIME.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&(pad as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        if pad > 0 {
            out.extend_from_slice(&PAD_ID.to_le_bytes());
            out.extend_from_slice(&((pad - 4) as u16).to_le_bytes());
            out.resize(out.len() + pad - 4, 0);
        }
        debug_assert_eq!(out.len() % ALIGNMENT, 0);
        out.extend_from_slice(data);
        dir.push((name, crc, size, offset));
    }
    let cd_start = u32::try_from(out.len()).map_err(|_| too_large("package over 4 GiB"))?;
    for (name, crc, size, offset) in &dir {
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method
        out.extend_from_slice(&DOS_TIME.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    let cd_size = u32::try_from(out.len() - cd_start as usize).map_err(|_| too_large("directory"))?;
    if out.len() > u32::MAX as usize {
        return Err(too_large("package over 4 GiB"));
    }
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(dir.len() as u16).to_le_bytes());
    out.extend_from_slice(&(dir.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}
