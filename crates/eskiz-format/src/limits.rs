//! Input limits for files from untrusted sources.
//!
//! Every importer in this crate reads files that other people made: `.procreate` documents,
//! PSDs, brushes and palettes. A hostile or damaged file must fail with an error, never crash,
//! hang or ask for unbounded memory. The caps below are the only place those bounds are set.
//!
//! The values come from real large documents, with a wide margin:
//!
//! * The largest canvas a tablet painting app makes is 16384 pixels on a side (about 268
//!   megapixels at most, and usually far fewer). Eskiz itself makes at most 8192 x 8192. PSD allows
//!   30000 on a side.
//! * Real `.procreate` packages store LZ4 tiles uncompressed in the ZIP; only `Document.archive`
//!   is deflated, at well under 20:1. A ZIP entry that inflates by more than 200:1 is a bomb.
//! * Keyed archives are flat by design: every object sits in `$objects` and refers to others by
//!   UID, so real property lists nest three or four levels deep.
//!
//! A limit that is hit gives [`Error::Limit`], which says which cap and by how much.

use std::io::{Cursor, Read, Seek};

use zip::result::ZipError;
use zip::ZipArchive;

use crate::{Error, Result};

// ------------------------------------------------------------------------------- ZIP

/// Most entries in one ZIP package. A 16384 x 16384 canvas at 256-pixel tiles has 4096 tiles a
/// layer; 250 fully painted layers of that size is about a million entries. Real documents are
/// far sparser. The ZIP crate has already read the directory by the time this is checked; the cap
/// bounds the work done per entry afterwards (name list, tile index).
pub const MAX_ZIP_ENTRIES: usize = 1 << 20;

/// Largest single ZIP entry that is read into memory. The biggest entries a reader loads are
/// `Document.archive` (a few MiB even for large documents), thumbnails (a few MiB), brush
/// textures (a 4096 x 4096 PNG) and tiles (at most [`MAX_TILE_SIDE`]² x 16 bytes = 256 MiB).
pub const MAX_ENTRY_BYTES: u64 = 512 << 20;

/// Largest total the entries of one package may claim to expand to. Real packages store their
/// tiles and media uncompressed, so this is about the file size; 64 GiB is well above any real
/// document, time-lapse video included.
pub const MAX_TOTAL_EXPANDED_BYTES: u64 = 64 << 30;

/// Largest expansion of one deflated entry (uncompressed size over compressed size). DEFLATE
/// tops out near 1032:1; binary property lists and JSON compress well under 20:1, and tiles and
/// PNGs are already compressed.
pub const MAX_COMPRESSION_RATIO: u64 = 200;

/// Entries that expand to less than this are not ratio-checked: a small, very regular entry
/// (an empty mask, a short JSON file) can legitimately compress far better than a large one.
pub const RATIO_CHECK_MIN_BYTES: u64 = 1 << 20;

/// Largest small metadata entry read from a package (`Swatches.json`, `brushset.plist`). Real
/// ones are a few KiB; parsing JSON takes several times the text size in memory.
pub const MAX_METADATA_BYTES: u64 = 16 << 20;

// ------------------------------------------------------------------------- property lists

/// Deepest nesting of arrays and dictionaries in a property list. Keyed archives nest three or
/// four levels; the cap also keeps recursive consumers (serde, `Drop`) far from the stack limit.
pub const MAX_PLIST_DEPTH: usize = 64;

/// Most values (events) one property list may produce. Binary property lists can share an
/// object between several parents, and the reader expands each reference again, so a small
/// file could otherwise describe an exponentially large tree. A document with a thousand layers
/// has well under 100 000 values.
pub const MAX_PLIST_OBJECTS: usize = 1 << 22;

/// Most string and data bytes one property list may expand to (shared references counted each
/// time they are expanded). The `Document.archive` of a large document is a few MiB.
pub const MAX_PLIST_BYTES: u64 = 128 << 20;

/// Deepest nesting of objects in a decoded keyed archive, counting both UID references and
/// inline arrays and dictionaries. Real layer trees are under 20 deep.
pub const MAX_ARCHIVE_DEPTH: usize = 256;

/// Most values a decoded keyed archive expands to. Repeated references are expanded again, so
/// this bounds the decoded tree, not the file.
pub const MAX_ARCHIVE_NODES: usize = 1 << 22;

/// Most string and data bytes a decoded keyed archive expands to, repeated references included.
/// The bytes are counted before they are copied, so this is also the most the decoder allocates
/// for them.
pub const MAX_ARCHIVE_BYTES: u64 = 128 << 20;

// ------------------------------------------------------------------------------ images

/// Longest canvas or layer side accepted from a file. Above the 16384 that tablet apps make
/// and the 30000 PSD allows.
pub const MAX_CANVAS_SIDE: u32 = 32_768;

/// Most pixels in one canvas or layer: 16384 x 16384 (268 megapixels). One RGBA8 buffer of
/// that size is 1 GiB and RGBA16 is 2 GiB.
pub const MAX_CANVAS_PIXELS: u64 = 1 << 28;

/// Most layers and groups in one document. Large animations have a few hundred layers.
pub const MAX_LAYERS: usize = 10_000;

/// Largest tile edge of a `.procreate` package. Real documents use 256; Eskiz writes 256 and its
/// tests go up to 512. At this edge a tile payload is at most 4096² x 16 bytes = 256 MiB.
pub const MAX_TILE_SIDE: u32 = 4096;

/// Most bytes of decoded pixels one PSD import may hold (every layer is a full canvas). Past
/// this the import cannot fit in memory on any machine Eskiz targets, so it fails early with a
/// clear message instead of being killed by the OS.
pub const MAX_DECODED_BYTES: u64 = 16 << 30;

// ----------------------------------------------------------------------------- brushes

/// Longest side of a sampled ABR brush tip. Photoshop's largest tip is 5000 pixels.
pub const MAX_BRUSH_TIP_SIDE: u32 = 8192;

/// Deepest nesting of Photoshop action descriptors in an ABR file. Real presets nest about four
/// levels.
pub const MAX_DESCRIPTOR_DEPTH: usize = 64;

/// Most brushes read from one `.brushset` or ABR file. Large commercial sets have a few hundred.
pub const MAX_BRUSHES: usize = 4096;

/// The error for a limit that was hit.
pub(crate) fn exceeded(what: impl std::fmt::Display) -> Error {
    Error::Limit(what.to_string())
}

/// Checks a canvas or layer size against [`MAX_CANVAS_SIDE`] and [`MAX_CANVAS_PIXELS`]. A zero
/// side is allowed (callers decide whether an empty image makes sense).
pub fn check_canvas(width: u32, height: u32, what: &str) -> Result<()> {
    if width > MAX_CANVAS_SIDE || height > MAX_CANVAS_SIDE {
        return Err(exceeded(format!(
            "{what} is {width}x{height}; the largest side Eskiz opens is {MAX_CANVAS_SIDE} pixels"
        )));
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_CANVAS_PIXELS {
        return Err(exceeded(format!(
            "{what} is {width}x{height} ({} megapixels); the most Eskiz opens is {} megapixels",
            pixels / 1_000_000,
            MAX_CANVAS_PIXELS / 1_000_000
        )));
    }
    Ok(())
}

/// Checks a ZIP package's directory: the entry count, each deflated entry's expansion ratio, the
/// total size the entries claim, and that the entries' compressed data fits in the file
/// (`file_len` bytes), which rules out the overlapping-entry bomb where many entries share one
/// compressed stream. Nothing is decompressed.
pub fn check_zip<R: Read + Seek>(zip: &ZipArchive<R>, file_len: u64) -> Result<()> {
    if zip.len() > MAX_ZIP_ENTRIES {
        return Err(exceeded(format!(
            "the package has {} entries; the most Eskiz reads is {MAX_ZIP_ENTRIES}",
            zip.len()
        )));
    }
    let mut expanded = 0u64;
    let mut compressed = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index_data(i)?;
        let (size, packed) = (entry.size(), entry.compressed_size());
        if size >= RATIO_CHECK_MIN_BYTES && size / packed.max(1) > MAX_COMPRESSION_RATIO {
            return Err(exceeded(format!(
                "entry {} expands {packed} bytes to {size} (more than {MAX_COMPRESSION_RATIO}:1)",
                String::from_utf8_lossy(entry.name_raw())
            )));
        }
        expanded = expanded.saturating_add(size);
        compressed = compressed.saturating_add(packed);
    }
    if expanded > MAX_TOTAL_EXPANDED_BYTES {
        return Err(exceeded(format!(
            "the package expands to {expanded} bytes; the most Eskiz reads is {MAX_TOTAL_EXPANDED_BYTES}"
        )));
    }
    if compressed > file_len {
        return Err(exceeded(format!(
            "the package's entries hold {compressed} bytes but the file has {file_len}: entries overlap"
        )));
    }
    Ok(())
}

/// Opens a ZIP package held in memory and checks it with [`check_zip`].
pub fn open_zip(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>> {
    let zip = ZipArchive::new(Cursor::new(bytes))?;
    check_zip(&zip, bytes.len() as u64)?;
    Ok(zip)
}

/// Reads one ZIP entry into memory, or `None` when there is no entry called `name`. Fails when
/// the entry claims more than `cap` bytes, and stops reading (with an error) as soon as the
/// decompressed data runs past the size the entry declares, so a lying header cannot make the
/// read grow without bound.
pub fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str, cap: u64) -> Result<Option<Vec<u8>>> {
    let file = match zip.by_name(name) {
        Ok(file) => file,
        Err(ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let declared = file.size();
    if declared > cap {
        return Err(exceeded(format!("entry {name} is {declared} bytes; the most Eskiz reads is {cap}")));
    }
    let mut buf = Vec::new();
    // the declared size is only a hint until the data is read
    buf.try_reserve(declared.min(16 << 20) as usize)
        .map_err(|_| exceeded(format!("cannot allocate {declared} bytes for entry {name}")))?;
    file.take(declared + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > declared {
        return Err(Error::Corrupt(format!("entry {name} holds more data than its header says")));
    }
    Ok(Some(buf))
}

/// Checks a property list (binary, XML or text) before it is built into a tree: its nesting
/// depth, its number of values and its string and data bytes.
///
/// A binary property list is scanned without building anything: the objects form a graph in
/// which one object can be shared by several parents, and the `plist` crate expands every
/// reference again, so the size of the tree it would build is computed per object, once, with
/// shared objects counted each time they are reached. Cycles are rejected. A text property list
/// (XML or OpenStep) cannot share objects, so only its nesting is checked.
pub fn check_plist(data: &[u8]) -> Result<()> {
    if data.starts_with(b"bplist") {
        return check_binary_plist(data);
    }
    let depth = text_plist_depth(data);
    if depth > MAX_PLIST_DEPTH {
        return Err(exceeded(format!("the property list nests more than {MAX_PLIST_DEPTH} levels deep")));
    }
    Ok(())
}

fn damaged(what: &str) -> Error {
    Error::Format(format!("damaged binary property list: {what}"))
}

/// Big-endian unsigned integer of `n` (1 to 8) bytes at `at`.
fn be_uint(data: &[u8], at: usize, n: usize) -> Result<u64> {
    if !(1..=8).contains(&n) {
        return Err(damaged("bad integer size"));
    }
    let bytes = at.checked_add(n).and_then(|end| data.get(at..end)).ok_or_else(|| damaged("truncated"))?;
    Ok(bytes.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

/// The objects of a binary property list (`bplist00`), read from its trailer.
struct BinaryPlist<'a> {
    data: &'a [u8],
    offset_size: usize,
    ref_size: usize,
    count: usize,
    top: usize,
    table: usize,
}

/// One object of a binary property list: its own string or data bytes and its children.
struct PlistObject {
    bytes: u64,
    container: bool,
    children: Vec<usize>,
}

impl<'a> BinaryPlist<'a> {
    fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 8 + 32 {
            return Err(damaged("too short"));
        }
        let trailer = data.len() - 32;
        let offset_size = usize::from(data[trailer + 6]);
        let ref_size = usize::from(data[trailer + 7]);
        let count = be_uint(data, trailer + 8, 8)?;
        let top = be_uint(data, trailer + 16, 8)?;
        let table = be_uint(data, trailer + 24, 8)?;
        // every object takes at least one byte and one offset entry
        if count == 0 || count > data.len() as u64 || top >= count || table >= data.len() as u64 {
            return Err(damaged("bad trailer"));
        }
        if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) {
            return Err(damaged("bad trailer"));
        }
        Ok(Self { data, offset_size, ref_size, count: count as usize, top: top as usize, table: table as usize })
    }

    /// The length that follows a marker whose low nibble is `info`, and where the payload starts.
    fn length(&self, info: u8, at: usize) -> Result<(u64, usize)> {
        if info != 0x0F {
            return Ok((u64::from(info), at));
        }
        let marker = *self.data.get(at).ok_or_else(|| damaged("truncated"))?;
        if marker >> 4 != 0x1 || marker & 0x0F > 3 {
            return Err(damaged("bad length"));
        }
        let n = 1usize << (marker & 0x0F);
        Ok((be_uint(self.data, at + 1, n)?, at + 1 + n))
    }

    fn object(&self, index: usize) -> Result<PlistObject> {
        let entry = index
            .checked_mul(self.offset_size)
            .and_then(|o| o.checked_add(self.table))
            .ok_or_else(|| damaged("bad offset"))?;
        let at = usize::try_from(be_uint(self.data, entry, self.offset_size)?).map_err(|_| damaged("bad offset"))?;
        let marker = *self.data.get(at).ok_or_else(|| damaged("offset past the end"))?;
        let (kind, info) = (marker >> 4, marker & 0x0F);
        let payload = |len: u64, unit: u64, start: usize| -> Result<u64> {
            let bytes = len.checked_mul(unit).ok_or_else(|| damaged("bad length"))?;
            if (start as u64).checked_add(bytes).is_none_or(|end| end > self.data.len() as u64) {
                return Err(damaged("object runs past the end"));
            }
            Ok(bytes)
        };
        let leaf = |bytes| Ok(PlistObject { bytes, container: false, children: Vec::new() });
        match kind {
            0x4 | 0x5 => {
                let (len, start) = self.length(info, at + 1)?;
                leaf(payload(len, 1, start)?)
            }
            0x6 => {
                let (len, start) = self.length(info, at + 1)?;
                leaf(payload(len, 2, start)?)
            }
            0xA | 0xC | 0xD => {
                let (len, start) = self.length(info, at + 1)?;
                let refs = if kind == 0xD { len.checked_mul(2).ok_or_else(|| damaged("bad length"))? } else { len };
                payload(refs, self.ref_size as u64, start)?;
                let children = (0..refs as usize)
                    .map(|k| {
                        let r = be_uint(self.data, start + k * self.ref_size, self.ref_size)?;
                        if r >= self.count as u64 {
                            return Err(damaged("reference out of range"));
                        }
                        Ok(r as usize)
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(PlistObject { bytes: 0, container: true, children })
            }
            _ => leaf(0),
        }
    }
}

/// The tree a binary property list expands to, per object: values, string and data bytes, and
/// container nesting.
#[derive(Clone, Copy, Default)]
struct Expanded {
    values: u64,
    bytes: u64,
    depth: usize,
}

fn check_binary_plist(data: &[u8]) -> Result<()> {
    if !data.starts_with(b"bplist00") {
        return Err(damaged("unknown version"));
    }
    let plist = BinaryPlist::new(data)?;
    // 0: not seen, 1: being expanded (on the stack), 2: done
    let mut state = vec![0u8; plist.count];
    let mut done = vec![Expanded::default(); plist.count];
    // (object, its children, the next child to visit, the sum so far)
    let mut stack: Vec<(usize, Vec<usize>, usize, Expanded)> = Vec::new();
    let enter = |index: usize, state: &mut [u8], stack: &mut Vec<(usize, Vec<usize>, usize, Expanded)>| -> Result<()> {
        let obj = plist.object(index)?;
        state[index] = 1;
        let own = Expanded { values: 1, bytes: obj.bytes, depth: usize::from(obj.container) };
        stack.push((index, obj.children, 0, own));
        if stack.len() > MAX_PLIST_DEPTH + 1 {
            return Err(exceeded(format!("the property list nests more than {MAX_PLIST_DEPTH} levels deep")));
        }
        Ok(())
    };
    enter(plist.top, &mut state, &mut stack)?;
    while let Some((index, children, next, sum)) = stack.last_mut() {
        if let Some(&child) = children.get(*next) {
            *next += 1;
            match state[child] {
                2 => {
                    let c = done[child];
                    sum.values = sum.values.saturating_add(c.values);
                    sum.bytes = sum.bytes.saturating_add(c.bytes);
                    sum.depth = sum.depth.max(c.depth + 1);
                }
                1 => return Err(damaged("an object contains itself")),
                _ => enter(child, &mut state, &mut stack)?,
            }
            continue;
        }
        let (index, sum) = (*index, *sum);
        if sum.values > MAX_PLIST_OBJECTS as u64 {
            return Err(exceeded(format!("the property list expands to more than {MAX_PLIST_OBJECTS} values")));
        }
        if sum.bytes > MAX_PLIST_BYTES {
            return Err(exceeded(format!("the property list expands to more than {MAX_PLIST_BYTES} bytes")));
        }
        state[index] = 2;
        done[index] = sum;
        stack.pop();
        if let Some((_, _, _, parent)) = stack.last_mut() {
            parent.values = parent.values.saturating_add(sum.values);
            parent.bytes = parent.bytes.saturating_add(sum.bytes);
            parent.depth = parent.depth.max(sum.depth + 1);
        }
    }
    Ok(())
}

/// The deepest nesting of arrays and dictionaries in an XML or OpenStep property list, found by
/// a scan of the text. A collection opened inside a string or comment can only overcount.
fn text_plist_depth(data: &[u8]) -> usize {
    let (mut depth, mut max) = (0usize, 0usize);
    let mut open = |depth: &mut usize| {
        *depth += 1;
        max = max.max(*depth);
    };
    let first = data.iter().position(|b| !b.is_ascii_whitespace() && !matches!(b, 0xEF | 0xBB | 0xBF));
    if first.is_some_and(|i| data[i] == b'<') {
        let mut i = 0;
        while let Some(lt) = data[i..].iter().position(|&b| b == b'<') {
            let tag_start = i + lt + 1;
            let Some(gt) = data[tag_start..].iter().position(|&b| b == b'>') else { break };
            let tag = &data[tag_start..tag_start + gt];
            let name_end = tag.iter().position(|b| b.is_ascii_whitespace() || *b == b'/').unwrap_or(tag.len());
            let closing = tag.starts_with(b"/");
            let name = if closing { &tag[1..] } else { &tag[..name_end] };
            if matches!(name, b"array" | b"dict") {
                if closing {
                    depth = depth.saturating_sub(1);
                } else if !tag.ends_with(b"/") {
                    open(&mut depth);
                }
            }
            i = tag_start + gt + 1;
        }
    } else {
        let mut quoted = false;
        let mut escaped = false;
        for &b in data {
            if quoted {
                match b {
                    _ if escaped => escaped = false,
                    b'\\' => escaped = true,
                    b'"' => quoted = false,
                    _ => {}
                }
                continue;
            }
            match b {
                b'"' => quoted = true,
                b'(' | b'{' => open(&mut depth),
                b')' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    max
}

/// [`check_plist`], then the property list parsed into a [`plist::Value`].
pub fn parse_plist(data: &[u8]) -> Result<plist::Value> {
    check_plist(data)?;
    Ok(plist::from_bytes(data)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_limits() {
        assert!(check_canvas(16384, 16384, "canvas").is_ok());
        assert!(check_canvas(0, 0, "canvas").is_ok());
        assert!(matches!(check_canvas(MAX_CANVAS_SIDE + 1, 1, "canvas"), Err(Error::Limit(_))));
        let err = check_canvas(32768, 32768, "canvas").unwrap_err();
        assert!(err.to_string().contains("megapixels"), "{err}");
    }

    #[test]
    fn plist_depth_and_size_are_checked() {
        let mut v = plist::Value::Integer(1.into());
        for _ in 0..MAX_PLIST_DEPTH {
            v = plist::Value::Array(vec![v]);
        }
        let mut ok = Vec::new();
        v.to_writer_binary(&mut ok).unwrap();
        assert!(check_plist(&ok).is_ok());
        let v = plist::Value::Array(vec![v]);
        let mut deep = Vec::new();
        v.to_writer_binary(&mut deep).unwrap();
        assert!(matches!(check_plist(&deep), Err(Error::Limit(_))));
    }
}
