//! Tile decompressors for `.procreate` documents.
//!
//! * [`apple_lz4_decompress`] reads Apple's framed LZ4 (`.lz4` tiles): a sequence of blocks,
//!   each introduced by a 4-byte magic. `bv41` is followed by a u32 decoded size, a u32 encoded
//!   size and a raw LZ4 block; `bv4-` by a u32 size and stored bytes; `bv4$` ends the stream.
//! * [`lzo1x_decompress`] reads LZO1X-1 streams (legacy `.chunk` tiles).
//!
//! Both are bounds-checked: malformed input
//! yields [`Error::Corrupt`], never a panic, and the output is capped at `max_output` bytes so
//! hostile input cannot force unbounded allocation.

use crate::{Error, Result};

fn corrupt<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Corrupt(msg.into()))
}

/// Fails if appending `extra` bytes to a buffer of length `out_len` (whose data starts at
/// `start`) would exceed `max_extra` bytes in total.
fn check_budget(out_len: usize, start: usize, extra: usize, max_extra: usize) -> Result<()> {
    match (out_len - start).checked_add(extra) {
        Some(total) if total <= max_extra => Ok(()),
        _ => corrupt("output larger than declared size"),
    }
}

fn read_u32(data: &[u8], at: usize) -> Result<usize> {
    let bytes = at
        .checked_add(4)
        .and_then(|end| data.get(at..end))
        .ok_or_else(|| Error::Corrupt("truncated frame header".into()))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
}

fn frame_slice<'a>(data: &'a [u8], start: usize, len: usize, what: &str) -> Result<&'a [u8]> {
    start
        .checked_add(len)
        .and_then(|end| data.get(start..end))
        .ok_or_else(|| Error::Corrupt(what.to_owned()))
}

// ----------------------------------------------------------------------------- LZ4

/// Decodes one raw LZ4 block (no frame header), appending to `out`.
/// At most `max_extra` bytes may be appended.
pub fn lz4_block_decompress(src: &[u8], out: &mut Vec<u8>, max_extra: usize) -> Result<()> {
    let start = out.len();
    let n = src.len();
    let mut i = 0usize;
    while i < n {
        let token = src[i];
        i += 1;
        let mut lit = usize::from(token >> 4);
        if lit == 15 {
            loop {
                let Some(&b) = src.get(i) else {
                    return corrupt("truncated literal length");
                };
                i += 1;
                lit += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }
        let lit_end = match i.checked_add(lit) {
            Some(end) if end <= n => end,
            _ => return corrupt("literal run past end of input"),
        };
        check_budget(out.len(), start, lit, max_extra)?;
        out.extend_from_slice(&src[i..lit_end]);
        i = lit_end;
        if i >= n {
            break; // the last sequence has no match
        }
        if i + 2 > n {
            return corrupt("truncated match offset");
        }
        let offset = usize::from(src[i]) | (usize::from(src[i + 1]) << 8);
        i += 2;
        // the offset may reach into earlier blocks' output, so check against all of `out`
        if offset == 0 || offset > out.len() {
            return corrupt("invalid match offset");
        }
        let mut mlen = usize::from(token & 15);
        if mlen == 15 {
            loop {
                let Some(&b) = src.get(i) else {
                    return corrupt("truncated match length");
                };
                i += 1;
                mlen += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }
        mlen += 4;
        check_budget(out.len(), start, mlen, max_extra)?;
        let pos = out.len() - offset;
        if offset >= mlen {
            out.extend_from_within(pos..pos + mlen);
        } else if offset == 1 {
            // a run of one byte
            let b = out[pos];
            out.resize(out.len() + mlen, b);
        } else {
            // Overlapping copy (the match repeats its own output with period `offset`): copy
            // `offset` bytes, then everything produced since `pos`, which doubles each round.
            let mut remaining = mlen;
            let mut chunk = offset;
            while remaining > 0 {
                let n = remaining.min(chunk);
                out.extend_from_within(pos..pos + n);
                remaining -= n;
                chunk = out.len() - pos;
            }
        }
    }
    Ok(())
}

/// The total decoded size the block headers (`bv41` / `bv4-` size prefixes) of a framed stream
/// declare, so a caller can compare it with the size it expects before anything is allocated.
/// Stops at the end marker or the first header that does not parse; a lying header only makes
/// the estimate wrong, never the decoding, which counts its output against its own cap.
pub fn apple_lz4_declared_size(data: &[u8]) -> usize {
    let mut total = 0usize;
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let size = |at: usize| read_u32(data, at).ok();
        match &data[i..i + 4] {
            b"bv41" => {
                let (Some(d), Some(e)) = (size(i + 4), size(i + 8)) else { break };
                total = total.saturating_add(d);
                i = i.saturating_add(12).saturating_add(e);
            }
            b"bv4-" => {
                let Some(n) = size(i + 4) else { break };
                total = total.saturating_add(n);
                i = i.saturating_add(8).saturating_add(n);
            }
            _ => break,
        }
    }
    total
}

/// Decodes Apple framed LZ4 (`bv41` / `bv4-` / `bv4$` blocks).
///
/// Fails if the output would exceed `max_output` bytes. A caller that knows the exact size
/// should compare the returned length against it.
pub fn apple_lz4_decompress(data: &[u8], max_output: usize) -> Result<Vec<u8>> {
    // pre-size from the headers, but never trust them with a large allocation
    let mut out = Vec::with_capacity(apple_lz4_declared_size(data).min(max_output).min(16 << 20));
    let mut i = 0usize;
    while i + 4 <= data.len() {
        match &data[i..i + 4] {
            b"bv4$" => break,
            b"bv41" => {
                let dsize = read_u32(data, i + 4)?;
                let esize = read_u32(data, i + 8)?;
                let start = i + 12;
                let block = frame_slice(data, start, esize, "truncated bv41 block")?;
                check_budget(out.len(), 0, dsize, max_output)?;
                let before = out.len();
                lz4_block_decompress(block, &mut out, dsize)?;
                if out.len() - before != dsize {
                    return corrupt("bv41 block size mismatch");
                }
                i = start + esize;
            }
            b"bv4-" => {
                let size = read_u32(data, i + 4)?;
                let start = i + 8;
                let bytes = frame_slice(data, start, size, "truncated bv4- block")?;
                check_budget(out.len(), 0, size, max_output)?;
                out.extend_from_slice(bytes);
                i = start + size;
            }
            other => {
                return corrupt(format!("unknown LZ4 frame magic {other:?} at offset {i}"));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------- LZO1X

struct Lzo<'a> {
    src: &'a [u8],
    ip: usize,
    out: Vec<u8>,
    max_output: usize,
}

impl Lzo<'_> {
    fn rd(&mut self) -> Result<usize> {
        let b = *self
            .src
            .get(self.ip)
            .ok_or_else(|| Error::Corrupt("LZO input overrun".into()))?;
        self.ip += 1;
        Ok(usize::from(b))
    }

    fn reserve(&self, extra: usize) -> Result<()> {
        check_budget(self.out.len(), 0, extra, self.max_output)
    }

    fn lit_copy(&mut self, count: usize) -> Result<()> {
        let end = match self.ip.checked_add(count) {
            Some(end) if end <= self.src.len() => end,
            _ => return corrupt("LZO literal overrun"),
        };
        self.reserve(count)?;
        self.out.extend_from_slice(&self.src[self.ip..end]);
        self.ip = end;
        Ok(())
    }

    fn copy_match(&mut self, dist: usize, len: usize) -> Result<()> {
        if dist == 0 || dist > self.out.len() {
            return corrupt("LZO lookbehind overrun");
        }
        self.reserve(len)?;
        let pos = self.out.len() - dist;
        if dist >= len {
            self.out.extend_from_within(pos..pos + len);
        } else {
            for k in 0..len {
                let b = self.out[pos + k];
                self.out.push(b);
            }
        }
        Ok(())
    }

    /// Zero-run length extension: each 0 byte adds 255, the first non-zero byte is added as is.
    fn ext_len(&mut self, base: usize) -> Result<usize> {
        let mut t = 0usize;
        let mut b = self.rd()?;
        while b == 0 {
            t = t.saturating_add(255);
            b = self.rd()?;
        }
        Ok(t.saturating_add(base).saturating_add(b))
    }

    fn run(&mut self) -> Result<()> {
        let mut state = 0usize; // literals copied by the previous instruction (0..3, or 4 for more)
        let mut t = self.rd()?;
        if t > 17 {
            t -= 17;
            self.lit_copy(t)?;
            state = if t > 3 { 4 } else { t };
            t = self.rd()?;
        }
        loop {
            let s = if t < 16 {
                if state == 0 {
                    // literal run
                    let count = if t == 0 { self.ext_len(15)? } else { t };
                    self.lit_copy(count + 3)?;
                    state = 4;
                    t = self.rd()?;
                    continue;
                }
                if state == 4 {
                    // M1 after a long literal run: 3-byte match, distance 2049..3072
                    let dist = 1 + 0x0800 + (t >> 2) + (self.rd()? << 2);
                    self.copy_match(dist, 3)?;
                } else {
                    // M1: 2-byte match, distance 1..1024
                    let dist = 1 + (t >> 2) + (self.rd()? << 2);
                    self.copy_match(dist, 2)?;
                }
                t & 3
            } else if t >= 64 {
                // M2: 3..8 byte match, distance 1..2048
                let dist = 1 + ((t >> 2) & 7) + (self.rd()? << 3);
                self.copy_match(dist, (t >> 5) + 1)?;
                t & 3
            } else if t >= 32 {
                // M3: match of length 3.., distance 1..16384
                let mut len = t & 31;
                if len == 0 {
                    len = self.ext_len(31)?;
                }
                let b0 = self.rd()?;
                let b1 = self.rd()?;
                self.copy_match(1 + (b0 >> 2) + (b1 << 6), len + 2)?;
                b0 & 3
            } else {
                // M4: match of length 3.., distance 16384+; distance 0 is the end-of-stream marker
                let mut len = t & 7;
                if len == 0 {
                    len = self.ext_len(7)?;
                }
                let b0 = self.rd()?;
                let b1 = self.rd()?;
                let dist = ((t & 8) << 11) + (b0 >> 2) + (b1 << 6);
                if dist == 0 {
                    break;
                }
                self.copy_match(dist + 0x4000, len + 2)?;
                b0 & 3
            };
            if s > 0 {
                self.lit_copy(s)?;
            }
            state = s;
            t = self.rd()?;
        }
        Ok(())
    }
}

/// Decodes an LZO1X-1 stream (as produced by `lzo1x_1_compress`).
///
/// Fails if the output would exceed `max_output` bytes. A caller that knows the exact size
/// should compare the returned length against it.
pub fn lzo1x_decompress(src: &[u8], max_output: usize) -> Result<Vec<u8>> {
    let mut d = Lzo {
        src,
        ip: 0,
        out: Vec::new(),
        max_output,
    };
    d.run()?;
    Ok(d.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic byte generator for the round-trip and no-panic tests.
    struct Lcg(u64);

    impl Lcg {
        fn next_u8(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u8
        }
    }

    /// One LZ4 block made only of literals (valid, uncompressed).
    fn lz4_literal_block(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let lit = data.len();
        if lit < 15 {
            out.push((lit as u8) << 4);
        } else {
            out.push(0xF0);
            let mut rem = lit - 15;
            while rem >= 255 {
                out.push(255);
                rem -= 255;
            }
            out.push(rem as u8);
        }
        out.extend_from_slice(data);
        out
    }

    /// Wraps one raw block as an Apple `bv41` frame, followed by the `bv4$` end marker.
    fn bv41_frame(raw_len: usize, block: &[u8]) -> Vec<u8> {
        let mut f = b"bv41".to_vec();
        f.extend_from_slice(&(raw_len as u32).to_le_bytes());
        f.extend_from_slice(&(block.len() as u32).to_le_bytes());
        f.extend_from_slice(block);
        f.extend_from_slice(b"bv4$");
        f
    }

    /// An LZO1X stream of one literal run followed by the end-of-stream marker.
    fn lzo_literal_stream(data: &[u8]) -> Vec<u8> {
        let n = data.len();
        let mut s = Vec::new();
        if (1..=238).contains(&n) {
            s.push((17 + n) as u8); // first byte > 17: n literals
        } else if n > 238 {
            // t = 15 + 255*zeros + last, literal count = t + 3
            let rem = n - 18;
            let last = (rem - 1) % 255 + 1;
            let zeros = (rem - last) / 255;
            s.push(0);
            s.extend(std::iter::repeat_n(0, zeros));
            s.push(last as u8);
        }
        s.extend_from_slice(data);
        s.extend_from_slice(&[0x11, 0x00, 0x00]);
        s
    }

    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    mod apple_reference {
        use std::ffi::c_void;

        #[link(name = "compression")]
        extern "C" {
            fn compression_encode_buffer(
                dst: *mut u8,
                dst_size: usize,
                src: *const u8,
                src_size: usize,
                scratch: *mut c_void,
                algorithm: i32,
            ) -> usize;
        }

        const COMPRESSION_LZ4: i32 = 0x100;

        /// Compresses with Apple's Compression framework (framed LZ4), as the Python tests do.
        pub fn encode_lz4(data: &[u8]) -> Vec<u8> {
            let mut dst = vec![0u8; data.len() + 4096];
            // SAFETY: both buffers are valid for the lengths passed, and a null scratch pointer
            // is accepted by compression_encode_buffer.
            let n = unsafe {
                compression_encode_buffer(
                    dst.as_mut_ptr(),
                    dst.len(),
                    data.as_ptr(),
                    data.len(),
                    std::ptr::null_mut(),
                    COMPRESSION_LZ4,
                )
            };
            assert!(n > 0, "compression_encode_buffer failed");
            dst.truncate(n);
            dst
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_lz4_roundtrip_against_compression_framework() {
        let mut rng = Lcg(1);
        let random: Vec<u8> = (0..5000).map(|_| rng.next_u8()).collect();
        let samples: Vec<Vec<u8>> = vec![
            b"a".to_vec(),
            b"abcabcabcabcabcabc".repeat(50),
            random,
            vec![0u8; 300_000],
            (0..=255u8).cycle().take(230_400).collect(),
        ];
        for s in samples {
            let frame = apple_reference::encode_lz4(&s);
            assert_eq!(apple_lz4_decompress(&frame, s.len()).unwrap(), s);
        }
    }

    #[test]
    fn apple_lz4_literal_frames_roundtrip() {
        let mut rng = Lcg(7);
        let mut samples: Vec<Vec<u8>> = vec![vec![], vec![b'a'], vec![9; 14], vec![9; 15], vec![9; 16]];
        samples.push((0..5000).map(|_| rng.next_u8()).collect());
        samples.push(vec![0u8; 300_000]);
        for s in samples {
            let frame = bv41_frame(s.len(), &lz4_literal_block(&s));
            assert_eq!(apple_lz4_decompress(&frame, s.len()).unwrap(), s);
        }
    }

    #[test]
    fn apple_lz4_mixed_stored_and_compressed_blocks() {
        let mut f = b"bv4-".to_vec();
        f.extend_from_slice(&3u32.to_le_bytes());
        f.extend_from_slice(b"xyz");
        let block = lz4_literal_block(b"hello");
        f.extend_from_slice(b"bv41");
        f.extend_from_slice(&5u32.to_le_bytes());
        f.extend_from_slice(&(block.len() as u32).to_le_bytes());
        f.extend_from_slice(&block);
        f.extend_from_slice(b"bv4$");
        assert_eq!(apple_lz4_decompress(&f, 100).unwrap(), b"xyzhello");
    }

    #[test]
    fn apple_lz4_wrong_size_detected() {
        let data = vec![b'x'; 100];
        let frame = bv41_frame(100, &lz4_literal_block(&data));
        // output limit below the real size
        assert!(matches!(apple_lz4_decompress(&frame, 99), Err(Error::Corrupt(_))));
        // header claims 10 bytes but the block holds 100
        let lying = bv41_frame(10, &lz4_literal_block(&data));
        assert!(matches!(apple_lz4_decompress(&lying, 1000), Err(Error::Corrupt(_))));
    }

    #[test]
    fn apple_lz4_rejects_damaged_frames() {
        assert!(apple_lz4_decompress(b"bv41\x01\x00", 100).is_err()); // truncated header
        assert!(apple_lz4_decompress(b"bv41\x04\x00\x00\x00\x50\x00\x00\x00ab", 100).is_err()); // short block
        assert!(apple_lz4_decompress(b"bv4-\x09\x00\x00\x00ab", 100).is_err()); // short stored block
        assert!(apple_lz4_decompress(b"XXXX\x00\x00\x00\x00", 100).is_err()); // unknown magic
    }

    #[test]
    fn lz4_overlapping_match() {
        // token: 1 literal 'a', match length 4+6=10 at offset 1; then final literal 'b'
        let blk = [0x16, b'a', 1, 0, 0x10, b'b'];
        let mut out = Vec::new();
        lz4_block_decompress(&blk, &mut out, 64).unwrap();
        let mut expected = vec![b'a'; 11];
        expected.push(b'b');
        assert_eq!(out, expected);
    }

    #[test]
    fn lz4_non_overlapping_match() {
        // literals "abcd", then a match at offset 4 of length 4, then an empty final sequence
        let blk = [0x40, b'a', b'b', b'c', b'd', 4, 0, 0x00];
        let mut out = Vec::new();
        lz4_block_decompress(&blk, &mut out, 64).unwrap();
        assert_eq!(out, b"abcdabcd");
    }

    #[test]
    fn lz4_rejects_bad_blocks() {
        let mut out = Vec::new();
        assert!(lz4_block_decompress(&[0x10, b'a', 0, 0], &mut out, 100).is_err()); // offset 0
        assert!(lz4_block_decompress(&[0x10, b'a', 9, 0, 0x00], &mut out, 100).is_err()); // past start
        assert!(lz4_block_decompress(&[0xF0, 1, 2], &mut out, 100).is_err()); // literals past end
        assert!(lz4_block_decompress(&[0x10, b'a', 1], &mut out, 100).is_err()); // truncated offset
        let blk = lz4_literal_block(&[7; 32]);
        assert!(lz4_block_decompress(&blk, &mut Vec::new(), 31).is_err()); // output limit
    }

    #[test]
    fn lzo_literal_and_m2() {
        // first byte 17+3: three literals; M2 length 8 distance 3; end marker
        let stream = [20, b'a', b'b', b'c', 0xE8, 0x00, 0x11, 0x00, 0x00];
        assert_eq!(lzo1x_decompress(&stream, 64).unwrap(), b"abcabcabcab");
    }

    #[test]
    fn lzo_long_literal_run() {
        let data: Vec<u8> = (0..40).collect();
        // literal run: t = 0, then extension byte 22 -> t = 15 + 22 = 37, copy t + 3 = 40
        let mut stream = vec![0x00, 40 - 3 - 15];
        stream.extend_from_slice(&data);
        stream.extend_from_slice(&[0x11, 0x00, 0x00]);
        assert_eq!(lzo1x_decompress(&stream, 64).unwrap(), data);
    }

    #[test]
    fn lzo_m3_match() {
        // "abc", then M3 with length 2+2=4 and distance 1 + (8>>2) = 3 -> "abca"; end marker
        let stream = [20, b'a', b'b', b'c', 0x22, 8, 0, 0x11, 0x00, 0x00];
        assert_eq!(lzo1x_decompress(&stream, 64).unwrap(), b"abcabca");
    }

    #[test]
    fn lzo_m1_after_literals() {
        // "abc" leaves state 3, so t = 0 is M1: distance 1, length 2 -> "cc"; end marker
        let stream = [20, b'a', b'b', b'c', 0x00, 0x00, 0x11, 0x00, 0x00];
        assert_eq!(lzo1x_decompress(&stream, 64).unwrap(), b"abccc");
    }

    #[test]
    fn lzo_literal_roundtrip_various_lengths() {
        for n in [0usize, 1, 3, 4, 17, 18, 19, 100, 238, 239, 255, 256, 1000, 70_000] {
            let data: Vec<u8> = (0..n).map(|i| (i * 31 % 251) as u8).collect();
            let stream = lzo_literal_stream(&data);
            assert_eq!(lzo1x_decompress(&stream, n).unwrap(), data, "length {n}");
        }
    }

    #[test]
    fn lzo_rejects_bad_streams() {
        // no end-of-stream marker
        assert!(lzo1x_decompress(&[20, b'a', b'b', b'c'], 64).is_err());
        // M2 distance 129 with only one byte of output
        assert!(lzo1x_decompress(&[18, b'a', 0x80, 0x10, 0x11, 0x00, 0x00], 64).is_err());
        // output limit
        assert!(lzo1x_decompress(&lzo_literal_stream(&[1; 100]), 50).is_err());
    }

    #[test]
    fn random_input_never_panics() {
        let mut rng = Lcg(42);
        for len in 0..300 {
            let buf: Vec<u8> = (0..len).map(|_| rng.next_u8()).collect();
            let _ = lzo1x_decompress(&buf, 1 << 16);
            let _ = lz4_block_decompress(&buf, &mut Vec::new(), 1 << 16);
            let _ = apple_lz4_decompress(&buf, 1 << 16);
            let mut framed = b"bv41".to_vec();
            framed.extend_from_slice(&buf);
            let _ = apple_lz4_decompress(&framed, 1 << 16);
        }
    }
}

// ------------------------------------------------------------------------- LZ4 encoder
//
// A hand-written greedy LZ4 block compressor and Apple framing. The output follows the LZ4
// block rules that other decoders rely on: no match starts within the last 12 bytes of a block,
// and the final 5 bytes are literals.

/// Input bytes given to one framed block. Keeps match offsets within 16 bits.
const ENC_BLOCK_INPUT: usize = 64 * 1024;
const ENC_MIN_MATCH: usize = 4;
/// No match may start within this many bytes of the end of a block.
const ENC_MF_LIMIT: usize = 12;
/// The last bytes of a block are always literals.
const ENC_LAST_LITERALS: usize = 5;
const ENC_MAX_OFFSET: usize = 0xFFFF;

fn enc_push_length(out: &mut Vec<u8>, mut len: usize) {
    while len >= 255 {
        out.push(255);
        len -= 255;
    }
    out.push(len as u8);
}

/// Appends one LZ4 sequence: its literals, then the match `(offset, length)` if there is one.
fn enc_sequence(out: &mut Vec<u8>, literals: &[u8], found: Option<(usize, usize)>) {
    let match_code = found.map_or(0, |(_, len)| len - ENC_MIN_MATCH);
    out.push(((literals.len().min(15) as u8) << 4) | (match_code.min(15) as u8));
    if literals.len() >= 15 {
        enc_push_length(out, literals.len() - 15);
    }
    out.extend_from_slice(literals);
    if let Some((offset, _)) = found {
        out.extend_from_slice(&(offset as u16).to_le_bytes());
        if match_code >= 15 {
            enc_push_length(out, match_code - 15);
        }
    }
}

#[inline(always)]
fn enc_hash_word(word: u32) -> usize {
    (word.wrapping_mul(2_654_435_761) >> 16) as usize
}

#[inline(always)]
fn read_word(src: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([src[at], src[at + 1], src[at + 2], src[at + 3]])
}

#[inline(always)]
fn read_u64(src: &[u8], at: usize) -> u64 {
    let b: [u8; 8] = src[at..at + 8].try_into().expect("8 bytes");
    u64::from_le_bytes(b)
}

/// Length of the common run of `src[c..]` and `src[i..]`, given that the first `ENC_MIN_MATCH`
/// bytes are equal, stopping at `end_limit` for the later position. Compares eight bytes at a
/// time.
#[inline(always)]
fn match_length(src: &[u8], c: usize, i: usize, end_limit: usize) -> usize {
    let mut len = ENC_MIN_MATCH;
    while i + len + 8 <= end_limit {
        let diff = read_u64(src, c + len) ^ read_u64(src, i + len);
        if diff != 0 {
            return len + (diff.trailing_zeros() / 8) as usize;
        }
        len += 8;
    }
    while i + len < end_limit && src[c + len] == src[i + len] {
        len += 1;
    }
    len
}

/// The per-thread match table of the encoder: 65536 `u32` slots (256 KiB), allocated once per
/// thread and never cleared between blocks. A slot holds `base + position + 1` for the block
/// being compressed, and `base` advances by the block length after each block, so every slot
/// written by an earlier block is `<= base` and reads as empty.
struct EncTable {
    slots: Vec<u32>,
    base: u32,
}

thread_local! {
    static ENC_TABLE: std::cell::RefCell<EncTable> =
        const { std::cell::RefCell::new(EncTable { slots: Vec::new(), base: 0 }) };
}

/// Inputs this long or longer use the wide-table encoder (their positions might overflow a
/// `u32` slot).
const ENC_NARROW_LIMIT: usize = 1 << 30;

/// Compresses one raw LZ4 block (no frame header) with a greedy hash-table matcher.
///
/// Any input is accepted. Matches reach back at most 65535 bytes within the block, so the output
/// is valid for [`lz4_block_decompress`] and for standard LZ4 decoders.
pub fn lz4_block_compress(src: &[u8]) -> Vec<u8> {
    if src.len() >= ENC_NARROW_LIMIT {
        return lz4_block_compress_wide(src);
    }
    ENC_TABLE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut table) => compress_narrow(src, &mut table),
        // re-entered (cannot happen from this module): use a private table
        Err(_) => compress_narrow(src, &mut EncTable { slots: Vec::new(), base: 0 }),
    })
}

fn compress_narrow(src: &[u8], table: &mut EncTable) -> Vec<u8> {
    let n = src.len();
    let mut out = Vec::with_capacity(n / 2 + 16);
    if table.slots.is_empty() {
        table.slots = vec![0u32; 1 << 16];
    }
    // `n < 2^30`, so the arithmetic below fits in u32 unless `base` is already high
    if u64::from(table.base) + n as u64 + 1 > u64::from(u32::MAX) {
        table.slots.fill(0);
        table.base = 0;
    }
    let base = table.base;
    let slots = &mut table.slots[..];
    let mut anchor = 0usize;
    let mut i = 0usize;
    if n > ENC_MF_LIMIT {
        let start_limit = n - ENC_MF_LIMIT;
        let end_limit = n - ENC_LAST_LITERALS;
        while i < start_limit {
            let word = read_word(src, i);
            let h = enc_hash_word(word);
            let candidate = slots[h];
            slots[h] = base + i as u32 + 1;
            if candidate > base {
                let c = (candidate - base - 1) as usize;
                if i - c <= ENC_MAX_OFFSET && read_word(src, c) == word {
                    let len = match_length(src, c, i, end_limit);
                    enc_sequence(&mut out, &src[anchor..i], Some((i - c, len)));
                    i += len;
                    anchor = i;
                    continue;
                }
            }
            i += 1;
        }
    }
    enc_sequence(&mut out, &src[anchor..], None);
    table.base = base + n as u32;
    out
}

/// The original encoder: a fresh `usize` table per call. Kept for inputs too long for the
/// narrow table, and as the baseline the tests compare the fast one against.
fn lz4_block_compress_wide(src: &[u8]) -> Vec<u8> {
    let n = src.len();
    let mut out = Vec::with_capacity(n / 2 + 16);
    // hash of four bytes -> last position + 1 (0 = empty)
    let mut table = vec![0usize; 1 << 16];
    let mut anchor = 0usize;
    let mut i = 0usize;
    if n > ENC_MF_LIMIT {
        let start_limit = n - ENC_MF_LIMIT;
        let end_limit = n - ENC_LAST_LITERALS;
        while i < start_limit {
            let h = enc_hash_word(read_word(src, i));
            let candidate = table[h];
            table[h] = i + 1;
            if candidate != 0 {
                let c = candidate - 1;
                if i - c <= ENC_MAX_OFFSET && src[c..c + ENC_MIN_MATCH] == src[i..i + ENC_MIN_MATCH] {
                    let mut len = ENC_MIN_MATCH;
                    while i + len < end_limit && src[c + len] == src[i + len] {
                        len += 1;
                    }
                    enc_sequence(&mut out, &src[anchor..i], Some((i - c, len)));
                    i += len;
                    anchor = i;
                    continue;
                }
            }
            i += 1;
        }
    }
    enc_sequence(&mut out, &src[anchor..], None);
    out
}

/// Compresses `data` as Apple framed LZ4: `bv41` blocks, `bv4-` (stored) for blocks that do not
/// shrink, and the `bv4$` end marker. Readable by [`apple_lz4_decompress`] and by Apple's
/// `compression_decode_buffer` with `COMPRESSION_LZ4`.
pub fn apple_lz4_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    for chunk in data.chunks(ENC_BLOCK_INPUT) {
        let block = lz4_block_compress(chunk);
        if block.len() < chunk.len() {
            out.extend_from_slice(b"bv41");
            out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
            out.extend_from_slice(&(block.len() as u32).to_le_bytes());
            out.extend_from_slice(&block);
        } else {
            out.extend_from_slice(b"bv4-");
            out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
            out.extend_from_slice(chunk);
        }
    }
    out.extend_from_slice(b"bv4$");
    out
}

#[cfg(test)]
mod encoder_tests {
    use super::*;

    struct Rng(u64);

    impl Rng {
        fn next_u8(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u8
        }
    }

    fn samples() -> Vec<Vec<u8>> {
        let mut rng = Rng(99);
        let random: Vec<u8> = (0..70_000).map(|_| rng.next_u8()).collect();
        vec![
            vec![],
            vec![b'a'],
            vec![7; 4],
            vec![7; 12],
            vec![7; 13],
            vec![7; 17],
            (0u8..=255).cycle().take(5_000).collect(),
            vec![0; 256 * 256 * 4],
            vec![0xAB; 300_000],
            b"abcabcabcabcabcabc".repeat(400),
            random[..1000].to_vec(),
            random,
        ]
    }

    #[test]
    fn apple_framed_roundtrip() {
        for s in samples() {
            let frame = apple_lz4_compress(&s);
            assert_eq!(apple_lz4_decompress(&frame, s.len()).unwrap(), s, "len {}", s.len());
        }
    }

    #[test]
    fn raw_block_roundtrip() {
        for s in samples() {
            // one raw block holds at most 64 KiB
            let s = &s[..s.len().min(ENC_BLOCK_INPUT)];
            let block = lz4_block_compress(s);
            let mut out = Vec::new();
            lz4_block_decompress(&block, &mut out, s.len()).unwrap();
            assert_eq!(out, s);
        }
    }

    #[test]
    fn empty_input_is_just_the_end_marker() {
        assert_eq!(apple_lz4_compress(&[]), b"bv4$");
    }

    #[test]
    fn structured_data_shrinks_and_noise_is_stored() {
        let zeros = apple_lz4_compress(&vec![0u8; 256 * 256 * 4]);
        assert!(zeros.len() < 2_000, "zero tile became {} bytes", zeros.len());

        let mut rng = Rng(3);
        let noise: Vec<u8> = (0..65_536).map(|_| rng.next_u8()).collect();
        let frame = apple_lz4_compress(&noise);
        assert_eq!(&frame[..4], b"bv4-");
        assert_eq!(frame.len(), 65_536 + 8 + 4);
    }

    /// The framing of [`apple_lz4_compress`] over the original (wide table) block encoder.
    fn apple_frame_reference(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in data.chunks(ENC_BLOCK_INPUT) {
            let block = lz4_block_compress_wide(chunk);
            if block.len() < chunk.len() {
                out.extend_from_slice(b"bv41");
                out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                out.extend_from_slice(&(block.len() as u32).to_le_bytes());
                out.extend_from_slice(&block);
            } else {
                out.extend_from_slice(b"bv4-");
                out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
                out.extend_from_slice(chunk);
            }
        }
        out.extend_from_slice(b"bv4$");
        out
    }

    /// A random input of a random kind: noise, repeats, runs, zeros, or a mix.
    fn random_input(rng: &mut Rng, len: usize) -> Vec<u8> {
        let kind = rng.next_u8() % 6;
        let mut v = Vec::with_capacity(len);
        match kind {
            0 => v.extend((0..len).map(|_| rng.next_u8())),
            1 => v.resize(len, 0),
            2 => {
                let period = 1 + usize::from(rng.next_u8() % 40);
                let unit: Vec<u8> = (0..period).map(|_| rng.next_u8()).collect();
                v.extend(unit.iter().cycle().take(len));
            }
            3 => {
                // runs of random lengths of random bytes
                while v.len() < len {
                    let run = 1 + usize::from(rng.next_u8()) * 3;
                    let b = rng.next_u8();
                    v.extend(std::iter::repeat_n(b, run.min(len - v.len())));
                }
            }
            4 => {
                // few distinct values (small alphabet) so matches are common but irregular
                v.extend((0..len).map(|_| rng.next_u8() % 3));
            }
            _ => {
                // paint-like: zero runs with pixel-sized noisy patches
                while v.len() < len {
                    if rng.next_u8().is_multiple_of(3) {
                        let n = 4 * (1 + usize::from(rng.next_u8() % 30));
                        v.extend((0..n.min(len - v.len())).map(|_| rng.next_u8()));
                    } else {
                        let n = 4 * (1 + usize::from(rng.next_u8()));
                        v.extend(std::iter::repeat_n(0u8, n.min(len - v.len())));
                    }
                }
            }
        }
        v.truncate(len);
        v
    }

    #[test]
    fn fast_encoder_output_is_byte_identical_to_the_original() {
        let mut rng = Rng(2024);
        // Lengths around the block-size and tile boundaries, small lengths, and random ones.
        let mut lens: Vec<usize> = (0..70).collect();
        for edge in [ENC_BLOCK_INPUT, 2 * ENC_BLOCK_INPUT, 256 * 256 * 4] {
            lens.extend((edge - 20)..(edge + 20));
        }
        for _ in 0..150 {
            let a = usize::from(rng.next_u8());
            let b = usize::from(rng.next_u8());
            lens.push(a * 600 + b * 3 + usize::from(rng.next_u8() % 4));
        }
        for len in lens {
            let data = random_input(&mut rng, len);
            // the block encoder reuses its table across calls on this thread: call it twice
            let once = lz4_block_compress(&data);
            let again = lz4_block_compress(&data);
            let reference = lz4_block_compress_wide(&data);
            assert_eq!(once, reference, "block, len {len}");
            assert_eq!(again, reference, "second call, len {len}");
            assert_eq!(apple_lz4_compress(&data), apple_frame_reference(&data), "frame, len {len}");
        }
    }

    #[test]
    fn table_epoch_wraps_without_changing_output() {
        let mut rng = Rng(5);
        let data = random_input(&mut rng, 40_000);
        let reference = lz4_block_compress_wide(&data);
        // Put the thread's table right below the u32 limit, so the next blocks must reset it.
        for base in [u32::MAX - 100_000, u32::MAX - 40_000, u32::MAX - 39_999, u32::MAX] {
            ENC_TABLE.with(|t| {
                let mut t = t.borrow_mut();
                if t.slots.is_empty() {
                    t.slots = vec![0u32; 1 << 16];
                }
                // dirty every slot with values that would look valid if the base were wrong
                t.slots.fill(base);
                t.base = base;
            });
            assert_eq!(lz4_block_compress(&data), reference, "base {base}");
            assert_eq!(lz4_block_compress(&data), reference, "base {base}, next block");
        }
    }

    #[test]
    fn stale_table_entries_from_earlier_blocks_are_ignored() {
        // Block A fills the table; block B (different data) must see an empty table.
        let mut rng = Rng(8);
        for _ in 0..30 {
            let (la, lb) = (3_000 + usize::from(rng.next_u8()) * 50, 100 + usize::from(rng.next_u8()) * 40);
            let a = random_input(&mut rng, la);
            let b = random_input(&mut rng, lb);
            let _ = lz4_block_compress(&a);
            assert_eq!(lz4_block_compress(&b), lz4_block_compress_wide(&b));
        }
    }

    #[test]
    fn original_encoder_output_decodes_with_the_new_decoder() {
        let mut rng = Rng(77);
        for _ in 0..120 {
            let len = usize::from(rng.next_u8()) * 1500 + usize::from(rng.next_u8());
            let data = random_input(&mut rng, len);
            let frame = apple_frame_reference(&data);
            assert_eq!(apple_lz4_decompress(&frame, data.len()).unwrap(), data, "len {len}");
        }
        for s in samples() {
            let frame = apple_frame_reference(&s);
            assert_eq!(apple_lz4_decompress(&frame, s.len()).unwrap(), s);
        }
    }

    /// The decoder as it was before the overlap-copy change: byte-at-a-time.
    fn lz4_block_decompress_reference(src: &[u8], out: &mut Vec<u8>, max_extra: usize) -> Result<()> {
        let start = out.len();
        let n = src.len();
        let mut i = 0usize;
        while i < n {
            let token = src[i];
            i += 1;
            let mut lit = usize::from(token >> 4);
            if lit == 15 {
                loop {
                    let Some(&b) = src.get(i) else {
                        return corrupt("truncated literal length");
                    };
                    i += 1;
                    lit += usize::from(b);
                    if b != 255 {
                        break;
                    }
                }
            }
            let lit_end = match i.checked_add(lit) {
                Some(end) if end <= n => end,
                _ => return corrupt("literal run past end of input"),
            };
            check_budget(out.len(), start, lit, max_extra)?;
            out.extend_from_slice(&src[i..lit_end]);
            i = lit_end;
            if i >= n {
                break;
            }
            if i + 2 > n {
                return corrupt("truncated match offset");
            }
            let offset = usize::from(src[i]) | (usize::from(src[i + 1]) << 8);
            i += 2;
            if offset == 0 || offset > out.len() {
                return corrupt("invalid match offset");
            }
            let mut mlen = usize::from(token & 15);
            if mlen == 15 {
                loop {
                    let Some(&b) = src.get(i) else {
                        return corrupt("truncated match length");
                    };
                    i += 1;
                    mlen += usize::from(b);
                    if b != 255 {
                        break;
                    }
                }
            }
            mlen += 4;
            check_budget(out.len(), start, mlen, max_extra)?;
            let pos = out.len() - offset;
            for k in 0..mlen {
                let b = out[pos + k];
                out.push(b);
            }
        }
        Ok(())
    }

    #[test]
    fn decoder_matches_the_byte_by_byte_reference() {
        let mut rng = Rng(31);
        // valid blocks from the encoder
        for _ in 0..100 {
            let len = usize::from(rng.next_u8()) * 200;
            let data = random_input(&mut rng, len);
            let block = lz4_block_compress(&data);
            let (mut a, mut b) = (Vec::new(), Vec::new());
            lz4_block_decompress(&block, &mut a, data.len()).unwrap();
            lz4_block_decompress_reference(&block, &mut b, data.len()).unwrap();
            assert_eq!(a, data);
            assert_eq!(b, data);
        }
        // hand-built overlapping matches of every small offset and many lengths
        for offset in 1..=20usize {
            for mlen in 4..=90usize {
                let seed: Vec<u8> = (0..offset).map(|k| (k * 7 + 3) as u8).collect();
                let mut block = vec![((offset.min(15) as u8) << 4) | ((mlen - 4).min(15) as u8)];
                if offset >= 15 {
                    let mut rest = offset - 15;
                    while rest >= 255 {
                        block.push(255);
                        rest -= 255;
                    }
                    block.push(rest as u8);
                }
                block.extend_from_slice(&seed);
                block.extend_from_slice(&(offset as u16).to_le_bytes());
                if mlen - 4 >= 15 {
                    let mut rest = mlen - 4 - 15;
                    while rest >= 255 {
                        block.push(255);
                        rest -= 255;
                    }
                    block.push(rest as u8);
                }
                block.push(0); // final token: no literals, no match
                let (mut a, mut b) = (Vec::new(), Vec::new());
                let ra = lz4_block_decompress(&block, &mut a, 1 << 20);
                let rb = lz4_block_decompress_reference(&block, &mut b, 1 << 20);
                assert_eq!(ra.is_ok(), rb.is_ok(), "offset {offset} mlen {mlen}");
                assert_eq!(a, b, "offset {offset} mlen {mlen}");
                assert_eq!(a.len(), offset + mlen);
            }
        }
        // garbage: same result and same error class as the baseline
        for _ in 0..3000 {
            let len = usize::from(rng.next_u8() % 80);
            let junk: Vec<u8> = (0..len).map(|_| rng.next_u8()).collect();
            let (mut a, mut b) = (Vec::new(), Vec::new());
            let ra = lz4_block_decompress(&junk, &mut a, 4096);
            let rb = lz4_block_decompress_reference(&junk, &mut b, 4096);
            assert_eq!(ra.is_ok(), rb.is_ok());
            if ra.is_ok() {
                assert_eq!(a, b);
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[allow(unsafe_code)]
    mod apple_decode {
        use std::ffi::c_void;

        #[link(name = "compression")]
        extern "C" {
            fn compression_decode_buffer(
                dst: *mut u8,
                dst_size: usize,
                src: *const u8,
                src_size: usize,
                scratch: *mut c_void,
                algorithm: i32,
            ) -> usize;
        }

        const COMPRESSION_LZ4: i32 = 0x100;

        /// Decodes a framed stream with Apple's Compression framework.
        pub fn decode_lz4(frame: &[u8], expected_len: usize) -> Vec<u8> {
            let mut dst = vec![0u8; expected_len];
            // SAFETY: both buffers are valid for the lengths passed, and a null scratch pointer
            // is accepted by compression_decode_buffer.
            let n = unsafe {
                compression_decode_buffer(
                    dst.as_mut_ptr(),
                    dst.len(),
                    frame.as_ptr(),
                    frame.len(),
                    std::ptr::null_mut(),
                    COMPRESSION_LZ4,
                )
            };
            dst.truncate(n);
            dst
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apple_compression_framework_decodes_our_frames() {
        for s in samples().into_iter().filter(|s| !s.is_empty()) {
            let frame = apple_lz4_compress(&s);
            assert_eq!(apple_decode::decode_lz4(&frame, s.len()), s, "len {}", s.len());
        }
    }
}
