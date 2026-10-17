//! Colour maths for the Colour panel: sRGB <-> HSV, hex text, the
//! geometry of the Disc picker, and the colour history / palette store.
//!
//! Everything here is pure (no egui), so it is unit tested and reused by `--selftest`.
//!
//! The Disc picker has three zones, from the outside in:
//! * the **hue ring**: the angle around the centre is the hue (0 at the top, clockwise);
//! * the **brightness ring**: a thin ring that sweeps 270 degrees (gap at the bottom)
//!   from black to the full-brightness colour;
//! * the **inner disc**: a saturation x brightness picker. It is the usual square
//!   (saturation across, brightness up) bent into a circle with the elliptical grid
//!   mapping, so every corner of the square (pure hue, white, black) stays reachable.

use std::f32::consts::{PI, TAU};

/// A colour as hue, saturation and brightness, each 0..=1 (hue 0 is red).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsv {
    pub h: f32,
    pub s: f32,
    pub v: f32,
}

impl Hsv {
    pub fn new(h: f32, s: f32, v: f32) -> Self {
        Self {
            h: h.rem_euclid(1.0),
            s: s.clamp(0.0, 1.0),
            v: v.clamp(0.0, 1.0),
        }
    }
}

/// sRGB bytes to HSV. Grey colours get hue 0.
pub fn rgb_to_hsv(rgb: [u8; 3]) -> Hsv {
    let [r, g, b] = rgb.map(|c| f32::from(c) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    Hsv { h, s, v: max }
}

/// HSV to sRGB bytes.
pub fn hsv_to_rgb(c: Hsv) -> [u8; 3] {
    let h = c.h.rem_euclid(1.0) * 6.0;
    let (s, v) = (c.s.clamp(0.0, 1.0), c.v.clamp(0.0, 1.0));
    let f = h - h.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match h.floor() as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b].map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// `#RRGGBB`.
pub fn format_hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Parse `#RGB`, `#RRGGBB`, with or without the `#`, in either case. Spaces around are ignored.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if !t.is_ascii() || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&t[i..=i], 16).ok();
    match t.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        6 => {
            let byte = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).ok();
            Some([byte(0)?, byte(2)?, byte(4)?])
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- disc geometry

/// Hue ring: inner and outer edge as fractions of the disc radius.
pub const HUE_RING: (f32, f32) = (0.82, 1.0);
/// Thin brightness ring, same units.
pub const BRIGHT_RING: (f32, f32) = (0.71, 0.78);
/// Radius of the inner saturation x brightness disc, same units.
pub const INNER_DISC: f32 = 0.64;
/// The brightness ring sweeps from -135 to +135 degrees about the top.
const BRIGHT_SWEEP: f32 = 0.75 * PI;

/// Unit direction for an angle measured clockwise from the top (screen y points down).
pub fn dir_from_angle(a: f32) -> (f32, f32) {
    (a.sin(), -a.cos())
}

/// Angle clockwise from the top for an offset `(dx, dy)` from the centre, in `-PI..=PI`.
pub fn angle_of(dx: f32, dy: f32) -> f32 {
    dx.atan2(-dy)
}

/// Hue (0..1) for the direction of `(dx, dy)`.
pub fn hue_at(dx: f32, dy: f32) -> f32 {
    angle_of(dx, dy).rem_euclid(TAU) / TAU
}

/// Direction of the hue `h` on the ring.
pub fn hue_dir(h: f32) -> (f32, f32) {
    dir_from_angle(h * TAU)
}

/// Brightness for the direction of `(dx, dy)` on the brightness ring. Directions in the
/// gap at the bottom snap to the nearer end.
pub fn brightness_at(dx: f32, dy: f32) -> f32 {
    ((angle_of(dx, dy) + BRIGHT_SWEEP) / (2.0 * BRIGHT_SWEEP)).clamp(0.0, 1.0)
}

/// Direction of brightness `v` on the brightness ring.
pub fn brightness_dir(v: f32) -> (f32, f32) {
    dir_from_angle(-BRIGHT_SWEEP + v.clamp(0.0, 1.0) * 2.0 * BRIGHT_SWEEP)
}

/// Unit square `(u, w)` in `-1..=1` to the unit disc (elliptical grid mapping).
pub fn square_to_disc(u: f32, w: f32) -> (f32, f32) {
    (u * (1.0 - w * w / 2.0).max(0.0).sqrt(), w * (1.0 - u * u / 2.0).max(0.0).sqrt())
}

/// Unit disc to the unit square: the inverse of [`square_to_disc`]. Points outside the
/// disc are pulled to its rim first.
pub fn disc_to_square(x: f32, y: f32) -> (f32, f32) {
    let r = x.hypot(y);
    let (x, y) = if r > 1.0 { (x / r, y / r) } else { (x, y) };
    let k = 2.0 * std::f32::consts::SQRT_2;
    let (x2, y2) = (x * x, y * y);
    let root = |t: f32| t.max(0.0).sqrt();
    let u = 0.5 * root(2.0 + x2 - y2 + k * x) - 0.5 * root(2.0 + x2 - y2 - k * x);
    let w = 0.5 * root(2.0 - x2 + y2 + k * y) - 0.5 * root(2.0 - x2 + y2 - k * y);
    (u.clamp(-1.0, 1.0), w.clamp(-1.0, 1.0))
}

/// Saturation and brightness at a point of the unit disc: saturation grows to the right,
/// brightness upwards.
pub fn disc_to_sv(x: f32, y: f32) -> (f32, f32) {
    let (u, w) = disc_to_square(x, y);
    ((u + 1.0) / 2.0, (1.0 - w) / 2.0)
}

/// Where saturation `s` and brightness `v` sit on the unit disc.
pub fn sv_to_disc(s: f32, v: f32) -> (f32, f32) {
    square_to_disc(s.clamp(0.0, 1.0) * 2.0 - 1.0, 1.0 - v.clamp(0.0, 1.0) * 2.0)
}

/// Which part of the disc a press landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Hue,
    Brightness,
    Inner,
}

/// Which zone lies at `r` (a fraction of the disc radius). A little slack on each ring
/// makes the thin brightness ring easy to hit; `None` outside everything.
pub fn zone_at(r: f32) -> Option<Zone> {
    if r > HUE_RING.1 + 0.04 {
        None
    } else if r >= (HUE_RING.0 + BRIGHT_RING.1) / 2.0 {
        Some(Zone::Hue)
    } else if r >= (BRIGHT_RING.0 + INNER_DISC) / 2.0 {
        Some(Zone::Brightness)
    } else {
        Some(Zone::Inner)
    }
}

/// New colour after dragging `zone` to the offset `(dx, dy)` from the centre, in
/// fractions of the disc radius.
pub fn pick(zone: Zone, dx: f32, dy: f32, old: Hsv) -> Hsv {
    match zone {
        Zone::Hue => Hsv::new(hue_at(dx, dy), old.s, old.v),
        Zone::Brightness => Hsv::new(old.h, old.s, brightness_at(dx, dy)),
        Zone::Inner => {
            let (s, v) = disc_to_sv(dx / INNER_DISC, dy / INNER_DISC);
            Hsv::new(old.h, s, v)
        }
    }
}

// ---------------------------------------------------------------- palette and history

/// Number of colours in the History row.
pub const HISTORY_LEN: usize = 10;

/// The original 30-colour default palette: ten hue families, each in a light, a mid and a
/// dark tone (three rows of ten).
pub const DEFAULT_PALETTE: [[u8; 3]; 30] = [
    // light
    [0xF2, 0xEF, 0xE9], [0xF4, 0xA9, 0xA0], [0xF9, 0xC0, 0x8F], [0xFB, 0xE7, 0xA1], [0xB9, 0xE3, 0xA4],
    [0xA3, 0xE3, 0xDB], [0xA9, 0xCC, 0xF5], [0xBD, 0xB6, 0xF2], [0xDD, 0xB7, 0xEE], [0xF7, 0xB5, 0xD3],
    // mid
    [0x8C, 0x88, 0x82], [0xD8, 0x45, 0x3A], [0xEE, 0x7B, 0x2B], [0xF2, 0xC2, 0x30], [0x4F, 0xA8, 0x5A],
    [0x25, 0xA5, 0x9B], [0x3F, 0x7B, 0xE0], [0x60, 0x55, 0xD6], [0xA0, 0x4F, 0xC8], [0xE0, 0x55, 0x8F],
    // dark
    [0x2B, 0x29, 0x27], [0x7E, 0x1F, 0x1B], [0x9A, 0x43, 0x10], [0xA8, 0x7D, 0x0E], [0x24, 0x60, 0x3A],
    [0x11, 0x60, 0x5F], [0x1D, 0x3F, 0x8C], [0x30, 0x29, 0x7E], [0x5A, 0x25, 0x78], [0x8E, 0x24, 0x59],
];

/// Colour history and the palette, saved between runs.
#[derive(Clone, Debug, PartialEq)]
pub struct ColourStore {
    /// Used colours, newest first, without duplicates, at most [`HISTORY_LEN`].
    pub history: Vec<[u8; 3]>,
    /// The Palettes tab: starts as [`DEFAULT_PALETTE`]; "+" appends.
    pub palette: Vec<[u8; 3]>,
    /// The current colour when the app last ran.
    pub current: [u8; 3],
}

impl Default for ColourStore {
    fn default() -> Self {
        Self {
            history: Vec::new(),
            palette: DEFAULT_PALETTE.to_vec(),
            current: [30, 60, 140],
        }
    }
}

impl ColourStore {
    /// Record `c` as used: it moves to the front, any older copy goes, the row stays capped.
    pub fn push_history(&mut self, c: [u8; 3]) {
        self.history.retain(|h| *h != c);
        self.history.insert(0, c);
        self.history.truncate(HISTORY_LEN);
    }

    /// Add `c` to the palette unless it is already there. Returns whether it was added.
    pub fn add_to_palette(&mut self, c: [u8; 3]) -> bool {
        if self.palette.contains(&c) {
            return false;
        }
        self.palette.push(c);
        true
    }

    /// Remove the palette entry at `i`.
    pub fn remove_from_palette(&mut self, i: usize) {
        if i < self.palette.len() {
            self.palette.remove(i);
        }
    }

    /// `cur=RRGGBB;hist=RRGGBB,...;pal=RRGGBB,...`
    pub fn encode(&self) -> String {
        let list = |v: &[[u8; 3]]| v.iter().map(|c| format_hex(*c)[1..].to_owned()).collect::<Vec<_>>().join(",");
        format!("cur={};hist={};pal={}", &format_hex(self.current)[1..], list(&self.history), list(&self.palette))
    }

    /// Parse [`ColourStore::encode`] output. Bad fields keep their defaults.
    pub fn decode(text: &str) -> Self {
        let mut s = Self::default();
        let list = |v: &str| v.split(',').filter_map(parse_hex).collect::<Vec<_>>();
        for pair in text.split(';') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            match k {
                "cur" => {
                    if let Some(c) = parse_hex(v) {
                        s.current = c;
                    }
                }
                "hist" => {
                    s.history = list(v);
                    s.history.truncate(HISTORY_LEN);
                }
                "pal" => {
                    let p = list(v);
                    if !p.is_empty() {
                        s.palette = p;
                    }
                }
                _ => {}
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_colours_in_hsv() {
        let red = rgb_to_hsv([255, 0, 0]);
        assert_eq!((red.h, red.s, red.v), (0.0, 1.0, 1.0));
        let green = rgb_to_hsv([0, 255, 0]);
        assert!((green.h - 1.0 / 3.0).abs() < 1e-6);
        let blue = rgb_to_hsv([0, 0, 255]);
        assert!((blue.h - 2.0 / 3.0).abs() < 1e-6);
        let grey = rgb_to_hsv([128, 128, 128]);
        assert_eq!((grey.h, grey.s), (0.0, 0.0));
        assert_eq!(rgb_to_hsv([0, 0, 0]).v, 0.0);
    }

    #[test]
    fn rgb_hsv_round_trip_is_exact() {
        for r in (0..=255u16).step_by(15) {
            for g in (0..=255u16).step_by(15) {
                for b in (0..=255u16).step_by(15) {
                    let c = [r as u8, g as u8, b as u8];
                    assert_eq!(hsv_to_rgb(rgb_to_hsv(c)), c);
                }
            }
        }
        // Odd values too.
        for c in [[1, 2, 3], [254, 253, 1], [17, 200, 99], [255, 254, 254]] {
            assert_eq!(hsv_to_rgb(rgb_to_hsv(c)), c);
        }
    }

    #[test]
    fn hsv_to_rgb_known_values() {
        assert_eq!(hsv_to_rgb(Hsv::new(0.0, 1.0, 1.0)), [255, 0, 0]);
        assert_eq!(hsv_to_rgb(Hsv::new(0.5, 1.0, 1.0)), [0, 255, 255]);
        assert_eq!(hsv_to_rgb(Hsv::new(0.25, 0.0, 0.5)), [128, 128, 128]);
        assert_eq!(hsv_to_rgb(Hsv::new(1.0, 1.0, 1.0)), [255, 0, 0]);
    }

    #[test]
    fn hex_parse_and_format() {
        assert_eq!(format_hex([0x3B, 0x82, 0xF6]), "#3B82F6");
        assert_eq!(parse_hex("#3b82f6"), Some([0x3B, 0x82, 0xF6]));
        assert_eq!(parse_hex("  3B82F6 "), Some([0x3B, 0x82, 0xF6]));
        assert_eq!(parse_hex("#f80"), Some([0xFF, 0x88, 0x00]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#GG0000"), None);
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#é12345"), None);
        for c in [[0, 0, 0], [255, 255, 255], [12, 200, 7]] {
            assert_eq!(parse_hex(&format_hex(c)), Some(c));
        }
    }

    #[test]
    fn hue_ring_points_round_trip() {
        for i in 0..24 {
            let h = i as f32 / 24.0;
            let (dx, dy) = hue_dir(h);
            assert!((hue_at(dx * 0.9, dy * 0.9) - h).abs() < 1e-4, "hue {h}");
        }
        // Red at the top, green a third of the way clockwise.
        let (x, y) = hue_dir(0.0);
        assert!(x.abs() < 1e-6 && y < -0.99);
        let (x, y) = hue_dir(0.25);
        assert!(x > 0.99 && y.abs() < 1e-6);
    }

    #[test]
    fn brightness_ring_runs_black_to_white_with_a_gap() {
        for i in 0..=10 {
            let v = i as f32 / 10.0;
            let (dx, dy) = brightness_dir(v);
            assert!((brightness_at(dx, dy) - v).abs() < 1e-4);
        }
        // The gap at the bottom snaps to the nearer end.
        assert_eq!(brightness_at(-0.1, 1.0), 0.0);
        assert_eq!(brightness_at(0.1, 1.0), 1.0);
        // The top is mid brightness.
        assert!((brightness_at(0.0, -1.0) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn disc_square_mapping_round_trips_and_reaches_the_corners() {
        for iu in -10..=10 {
            for iw in -10..=10 {
                let (u, w) = (iu as f32 / 10.0, iw as f32 / 10.0);
                let (x, y) = square_to_disc(u, w);
                assert!(x.hypot(y) <= 1.0 + 1e-5);
                let (u2, w2) = disc_to_square(x, y);
                assert!((u - u2).abs() < 2e-3 && (w - w2).abs() < 2e-3, "({u},{w}) -> ({u2},{w2})");
            }
        }
        // The disc centre is the middle of the square.
        let (s, v) = disc_to_sv(0.0, 0.0);
        assert!((s - 0.5).abs() < 1e-6 && (v - 0.5).abs() < 1e-6);
    }

    #[test]
    fn disc_sv_round_trip_and_extremes() {
        for is in 0..=10 {
            for iv in 0..=10 {
                let (s, v) = (is as f32 / 10.0, iv as f32 / 10.0);
                let (x, y) = sv_to_disc(s, v);
                let (s2, v2) = disc_to_sv(x, y);
                assert!((s - s2).abs() < 2e-3 && (v - v2).abs() < 2e-3, "({s},{v}) -> ({s2},{v2})");
            }
        }
        // Pure hue (s=1, v=1) is up and to the right, white (s=0, v=1) up and to the left,
        // black along the bottom.
        let (x, y) = sv_to_disc(1.0, 1.0);
        assert!(x > 0.5 && y < -0.5);
        let (x, y) = sv_to_disc(0.0, 1.0);
        assert!(x < -0.5 && y < -0.5);
        let (_, y) = sv_to_disc(0.5, 0.0);
        assert!(y > 0.9);
    }

    #[test]
    fn zones_and_picking() {
        assert_eq!(zone_at(0.95), Some(Zone::Hue));
        assert_eq!(zone_at(0.75), Some(Zone::Brightness));
        assert_eq!(zone_at(0.3), Some(Zone::Inner));
        assert_eq!(zone_at(0.0), Some(Zone::Inner));
        assert_eq!(zone_at(1.3), None);

        let old = Hsv::new(0.1, 0.5, 0.5);
        // Pointing right on the ring sets hue a quarter turn round; s and v stay.
        let c = pick(Zone::Hue, 0.9, 0.0, old);
        assert!((c.h - 0.25).abs() < 1e-5 && c.s == 0.5 && c.v == 0.5);
        // Brightness ring: straight up is half brightness.
        let c = pick(Zone::Brightness, 0.0, -0.75, old);
        assert!((c.v - 0.5).abs() < 1e-5 && c.h == old.h);
        // The top-right of the inner disc is vivid; the bottom is black.
        let c = pick(Zone::Inner, 0.45, -0.45, old);
        assert!(c.s > 0.9 && c.v > 0.9);
        assert!(pick(Zone::Inner, 0.0, 0.64, old).v < 0.02);
        // Dragging far outside the disc still gives valid values.
        let c = pick(Zone::Inner, 9.0, -9.0, old);
        assert!(c.s <= 1.0 && c.v <= 1.0);
    }

    #[test]
    fn history_is_newest_first_unique_and_capped() {
        let mut s = ColourStore::default();
        for i in 0..15u8 {
            s.push_history([i, 0, 0]);
        }
        assert_eq!(s.history.len(), HISTORY_LEN);
        assert_eq!(s.history[0], [14, 0, 0]);
        s.push_history([10, 0, 0]);
        assert_eq!(s.history[0], [10, 0, 0]);
        assert_eq!(s.history.iter().filter(|c| **c == [10, 0, 0]).count(), 1);
        assert_eq!(s.history.len(), HISTORY_LEN);
    }

    #[test]
    fn default_palette_has_30_distinct_colours() {
        let s = ColourStore::default();
        assert_eq!(s.palette.len(), 30);
        let mut seen = s.palette.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 30);
    }

    #[test]
    fn palette_add_and_remove() {
        let mut s = ColourStore::default();
        assert!(s.add_to_palette([1, 2, 3]));
        assert!(!s.add_to_palette([1, 2, 3]));
        assert_eq!(s.palette.len(), 31);
        s.remove_from_palette(30);
        assert_eq!(s.palette.len(), 30);
        s.remove_from_palette(99);
        assert_eq!(s.palette.len(), 30);
    }

    #[test]
    fn store_round_trips_and_survives_garbage() {
        let mut s = ColourStore::default();
        s.push_history([1, 2, 3]);
        s.push_history([200, 100, 50]);
        s.add_to_palette([9, 9, 9]);
        s.current = [10, 20, 30];
        assert_eq!(ColourStore::decode(&s.encode()), s);
        assert_eq!(ColourStore::decode("garbage;pal=zz,;hist=12"), ColourStore::default());
    }
}
