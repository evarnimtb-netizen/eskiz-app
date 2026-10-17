//! The streamed PSD writer gives the same bytes as the dense one, on random documents.

use eskiz_format::psd::{self, Compression16, LayerPixels, PsdBlend, PsdDocument, PsdGroup, PsdLayer, PsdNode};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const BLENDS: [PsdBlend; 9] = [
    PsdBlend::Normal,
    PsdBlend::Multiply,
    PsdBlend::Screen,
    PsdBlend::Add,
    PsdBlend::Overlay,
    PsdBlend::Color,
    PsdBlend::Hue,
    PsdBlend::SoftLight,
    PsdBlend::Divide,
];

/// Dense buffers served by rows.
struct Dense {
    w: usize,
    l8: Vec<Vec<u8>>,
    l16: Vec<Vec<u16>>,
}

impl LayerPixels for Dense {
    fn rows8(&self, index: usize, y0: usize, y1: usize) -> Vec<u8> {
        self.l8[index][y0 * self.w * 4..y1 * self.w * 4].to_vec()
    }
    fn rows16(&self, index: usize, y0: usize, y1: usize) -> Vec<u16> {
        self.l16[index][y0 * self.w * 4..y1 * self.w * 4].to_vec()
    }
    fn rows_blank(&self, index: usize, y0: usize, y1: usize) -> bool {
        // exact for the 8-bit pixels (the 16-bit ones are zero exactly where these are)
        self.l8[index][y0 * self.w * 4..y1 * self.w * 4].iter().all(|&b| b == 0)
    }
}

/// The same pixels, never claiming a band is blank.
struct NoHints<'a>(&'a Dense);

impl LayerPixels for NoHints<'_> {
    fn rows8(&self, index: usize, y0: usize, y1: usize) -> Vec<u8> {
        self.0.rows8(index, y0, y1)
    }
    fn rows16(&self, index: usize, y0: usize, y1: usize) -> Vec<u16> {
        self.0.rows16(index, y0, y1)
    }
}

/// Premultiplied pixels with patches; 16-bit ones derive from the 8-bit pattern plus noise.
fn pixels(rng: &mut Rng, w: usize, h: usize) -> (Vec<u8>, Vec<u16>) {
    let mut p8 = vec![0u8; w * h * 4];
    let mut p16 = vec![0u16; w * h * 4];
    for _ in 0..rng.below(4) {
        let (px, py) = (rng.below(w), rng.below(h));
        let (pw, ph) = (1 + rng.below(w - px), 1 + rng.below(h - py));
        for y in py..py + ph {
            for x in px..px + pw {
                let a = (rng.next() % 256) as u8;
                let o = (y * w + x) * 4;
                for c in 0..3 {
                    let v = (rng.next() % (u64::from(a) + 1)) as u8;
                    p8[o + c] = v;
                    p16[o + c] = (u16::from(v) * 257).saturating_add((rng.next() % 3) as u16);
                }
                p8[o + 3] = a;
                p16[o + 3] = u16::from(a) * 257;
                for c in 0..3 {
                    p16[o + c] = p16[o + c].min(p16[o + 3]);
                }
            }
        }
    }
    (p8, p16)
}

fn node(rng: &mut Rng, w: usize, h: usize, depth: usize, dense: &mut Dense, with_pixels: &mut bool) -> PsdNode {
    if depth > 0 && rng.below(4) == 0 {
        let n = 1 + rng.below(3);
        let children = (0..n).map(|_| node(rng, w, h, depth - 1, dense, with_pixels)).collect();
        return PsdNode::Group(PsdGroup {
            name: format!("group {}", rng.below(100)),
            opacity: [0.0, 1.0, 0.5, 0.77][rng.below(4)],
            blend: BLENDS[rng.below(BLENDS.len())],
            hidden: rng.below(6) == 0,
            collapsed: rng.below(2) == 0,
            children,
        });
    }
    let (p8, p16) = pixels(rng, w, h);
    let layer = PsdLayer {
        name: format!("layer {}", rng.below(1000)),
        opacity: [0.0, 1.0, 0.5, 0.3][rng.below(4)],
        blend: BLENDS[rng.below(BLENDS.len())],
        hidden: rng.below(8) == 0,
        clipped: rng.below(3) == 0,
        alpha_locked: rng.below(5) == 0,
        locked: rng.below(5) == 0,
        pixels: if *with_pixels { p8.clone() } else { Vec::new() },
    };
    dense.l8.push(p8);
    dense.l16.push(p16);
    PsdNode::Layer(layer)
}

fn build(rng: &mut Rng, w: usize, h: usize, with_pixels: bool, seed: u64) -> (PsdDocument, Dense) {
    // the same random choices twice: once with pixels in the tree, once without
    let _ = seed;
    let mut dense = Dense { w, l8: Vec::new(), l16: Vec::new() };
    let mut wp = with_pixels;
    let n = 1 + rng.below(5);
    let root = (0..n).map(|_| node(rng, w, h, 2, &mut dense, &mut wp)).collect();
    (
        PsdDocument { width: w as u32, height: h as u32, dpi: 144.0, colour_space: None, root },
        dense,
    )
}

#[test]
fn streamed_psd_matches_the_dense_writer() {
    for case in 0..60u64 {
        let mut rng = Rng(0x5EED + case);
        let (w, h) = match case % 5 {
            0 => (1, 1),
            1 => (63, 130), // more than two bands of rows
            _ => (1 + rng.below(150), 1 + rng.below(200)),
        };
        let seed = rng.0;
        let (full, dense) = build(&mut Rng(seed), w, h, true, seed);
        let (bare, dense2) = build(&mut Rng(seed), w, h, false, seed);
        assert_eq!(dense.l8, dense2.l8);
        assert_eq!(full.width, bare.width);

        let reference = psd::to_bytes(&full).unwrap();
        let streamed = psd::to_bytes_streamed(&bare, &dense2, None).unwrap();
        assert!(reference == streamed, "8-bit, case {case} ({w}x{h})");
        let unhinted = psd::to_bytes_streamed(&bare, &NoHints(&dense2), None).unwrap();
        assert!(reference == unhinted, "8-bit without blank hints, case {case}");

        for compression in [Compression16::Rle, Compression16::Raw] {
            let reference = psd::to_bytes16(&bare, &dense.l16, compression).unwrap();
            let streamed = psd::to_bytes_streamed(&bare, &dense2, Some(compression)).unwrap();
            assert!(reference == streamed, "16-bit {compression:?}, case {case} ({w}x{h})");
        }
    }
}

#[test]
fn streamed_psd_checks_the_canvas_size() {
    let doc = PsdDocument { width: 0, height: 5, dpi: 72.0, colour_space: None, root: Vec::new() };
    let dense = Dense { w: 0, l8: Vec::new(), l16: Vec::new() };
    assert!(psd::to_bytes_streamed(&doc, &dense, None).is_err());
}
