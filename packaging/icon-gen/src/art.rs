//! The Eskiz v5 app icon, ported from the design spec (`spec-v5.json`: canvas 1024, corner
//! radius 230, a dark navy rounded square with a vignette, a blueprint corner bracket and an
//! orange-to-magenta pointer nib with a soft red glow). Everything is drawn from the numbers
//! below: no bitmaps are read. Coordinates are in the 1024-unit design space.

/// Straight-alpha colour, components 0..=1.
pub type Rgba = [f32; 4];

pub const CANVAS: f32 = 1024.0;
pub const CORNER_RADIUS: f32 = 230.0;

/// Background: vertical linear gradient from y = 0 to y = 1024.
pub const BG_STOPS: [(f32, u32); 2] = [(0.0, 0x26324A), (1.0, 0x0B1020)];
/// Vignette: radial, centre and radius, colour, alpha 0.45 at the centre fading to 0.
pub const VIGNETTE_CENTRE: (f32, f32) = (360.0, 340.0);
pub const VIGNETTE_RADIUS: f32 = 720.0;
pub const VIGNETTE_COLOUR: u32 = 0x3A4A6E;
pub const VIGNETTE_ALPHA: f32 = 0.45;

/// Blueprint ink: linear gradient (176,176) to (706,706).
pub const INK_FROM: (f32, f32) = (176.0, 176.0);
pub const INK_TO: (f32, f32) = (706.0, 706.0);
pub const INK_STOPS: [(f32, u32); 2] = [(0.0, 0xEAF4FF), (1.0, 0x8FB9F2)];
/// Blueprint bars (x, y, w, h), square ends with radius 3.
pub const BARS: [(f32, f32, f32, f32); 8] = [
    (176.0, 226.0, 210.0, 20.0),
    (226.0, 176.0, 20.0, 210.0),
    (414.0, 226.0, 22.0, 20.0),
    (226.0, 414.0, 20.0, 22.0),
    (464.0, 226.0, 242.0, 20.0),
    (226.0, 464.0, 20.0, 242.0),
    (696.0, 204.0, 20.0, 64.0),
    (204.0, 696.0, 64.0, 20.0),
];
pub const BAR_RADIUS: f32 = 3.0;

/// Nib fill: linear gradient (302,302) to (726.26,726.26).
pub const NIB_FROM: (f32, f32) = (302.0, 302.0);
pub const NIB_TO: (f32, f32) = (726.26, 726.26);
pub const NIB_STOPS: [(f32, u32); 4] = [(0.0, 0xFFB224), (0.36, 0xFF5A1F), (0.7, 0xF0124B), (1.0, 0xE0399E)];

/// Glow behind the nib: colour, opacity, and the design's blur radius (a Gaussian whose
/// standard deviation is `GLOW_RADIUS * GLOW_SIGMA_PER_RADIUS`; 0.35 is fitted to the reference render).
pub const GLOW_COLOUR: u32 = 0xFF3D3D;
pub const GLOW_OPACITY: f32 = 0.3;
pub const GLOW_RADIUS: f32 = 45.0;
pub const GLOW_SIGMA_PER_RADIUS: f32 = 0.35;

/// Nib outline: 22 cubic Bezier segments (from, control 1, control 2, to). Straight edges with
/// small rounded corners (radius 10, tip radius 16).
pub const NIB: [(f32, f32, f32, f32, f32, f32, f32, f32); 22] = [
    (308.12, 316.78, 304.76, 308.65, 308.65, 304.76, 316.78, 308.12),
    (316.78, 308.12, 468.91, 371.14, 621.05, 434.15, 773.18, 497.17),
    (773.18, 497.17, 778.26, 499.27, 784.14, 505.15, 786.24, 510.23),
    (786.24, 510.23, 805.38, 556.43, 824.51, 602.62, 843.65, 648.82),
    (843.65, 648.82, 845.75, 653.9, 843.32, 656.33, 838.23, 654.23),
    (838.23, 654.23, 792.04, 635.09, 745.85, 615.96, 699.65, 596.83),
    (699.65, 596.83, 694.57, 594.72, 686.26, 591.28, 681.18, 589.17),
    (681.18, 589.17, 665.78, 582.79, 650.38, 576.42, 634.98, 570.04),
    (634.98, 570.04, 629.9, 567.93, 627.46, 570.37, 629.57, 575.45),
    (629.57, 575.45, 643.92, 610.1, 658.27, 644.75, 672.63, 679.4),
    (672.63, 679.4, 674.73, 684.48, 672.29, 686.91, 667.21, 684.81),
    (667.21, 684.81, 647.96, 676.83, 628.71, 668.86, 609.46, 660.89),
    (609.46, 660.89, 604.38, 658.78, 601.94, 661.22, 604.05, 666.3),
    (604.05, 666.3, 609.15, 678.62, 614.25, 690.94, 619.36, 703.25),
    (619.36, 703.25, 621.46, 708.33, 624.9, 716.65, 627.01, 721.73),
    (627.01, 721.73, 642.32, 758.69, 657.62, 795.64, 672.93, 832.6),
    (672.93, 832.6, 675.04, 837.68, 672.6, 840.11, 667.52, 838.01),
    (667.52, 838.01, 630.56, 822.7, 593.61, 807.39, 556.65, 792.09),
    (556.65, 792.09, 551.57, 789.98, 544.23, 791.44, 540.34, 795.33),
    (540.34, 795.33, 534.69, 800.99, 529.03, 806.64, 523.37, 812.3),
    (523.37, 812.3, 519.48, 816.19, 514.58, 815.22, 512.48, 810.13),
    (512.48, 810.13, 444.36, 645.68, 376.24, 481.23, 308.12, 316.78),
];

pub fn rgb(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 255) as f32 / 255.0, ((hex >> 8) & 255) as f32 / 255.0, (hex & 255) as f32 / 255.0]
}

/// Colour on a gradient with `t` already projected to 0..=1.
pub fn ramp(stops: &[(f32, u32)], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    for w in stops.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let k = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            let (a, b) = (rgb(c0), rgb(c1));
            return [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k];
        }
    }
    rgb(stops[stops.len() - 1].1)
}

/// Projection of `p` on the gradient axis `from -> to`, as 0..=1.
pub fn project(p: (f32, f32), from: (f32, f32), to: (f32, f32)) -> f32 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    ((p.0 - from.0) * dx + (p.1 - from.1) * dy) / (dx * dx + dy * dy)
}

/// The nib outline flattened to a polygon in design units.
pub fn nib_polygon() -> Vec<(f32, f32)> {
    const STEPS: usize = 24;
    let mut pts = Vec::new();
    for &(x0, y0, x1, y1, x2, y2, x3, y3) in NIB.iter() {
        for i in 0..STEPS {
            let t = i as f32 / STEPS as f32;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            pts.push((a * x0 + b * x1 + c * x2 + d * x3, a * y0 + b * y1 + c * y2 + d * y3));
        }
    }
    pts
}

/// Signed distance to a rounded rectangle (negative inside).
pub fn rrect_sdf(p: (f32, f32), x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let qx = (p.0 - cx).abs() - (w / 2.0 - r);
    let qy = (p.1 - cy).abs() - (h / 2.0 - r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Which layers to draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layers {
    /// The whole icon on its rounded square.
    Full,
    /// Android adaptive background: gradient and vignette over the whole canvas (no rounding).
    Background,
    /// Android adaptive foreground: bars, glow and nib on transparency.
    Foreground,
    /// Android themed icon: bars and nib as one white silhouette.
    Monochrome,
}

/// Maps design units to pixels: `px = unit * scale + offset`.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub size: u32,
    pub scale: f32,
    pub offset: (f32, f32),
}

impl View {
    /// The whole 1024 canvas on a `size` square.
    pub fn full(size: u32) -> Self {
        Self { size, scale: size as f32 / CANVAS, offset: (0.0, 0.0) }
    }
    fn to_unit(&self, px: f32, py: f32) -> (f32, f32) {
        ((px - self.offset.0) / self.scale, (py - self.offset.1) / self.scale)
    }
}

/// Coverage of a polygon on a `size` x `size` pixel grid: 8 sub-rows per pixel row with exact
/// horizontal coverage (even-odd rule).
fn polygon_coverage(points: &[(f32, f32)], size: usize) -> Vec<f32> {
    const SUB: usize = 8;
    let mut cov = vec![0.0f32; size * size];
    let n = points.len();
    let mut xs: Vec<f32> = Vec::new();
    for row in 0..size {
        for s in 0..SUB {
            let y = row as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            for i in 0..n {
                let (a, b) = (points[i], points[(i + 1) % n]);
                if (a.1 <= y) != (b.1 <= y) {
                    xs.push(a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            for span in xs.chunks_exact(2) {
                let (x0, x1) = (span[0].max(0.0), span[1].min(size as f32));
                if x1 <= x0 {
                    continue;
                }
                let (p0, p1) = (x0.floor() as usize, (x1.ceil() as usize).min(size));
                for px in p0..p1 {
                    let l = x0.max(px as f32);
                    let r = x1.min(px as f32 + 1.0);
                    if r > l {
                        cov[row * size + px] += (r - l) / SUB as f32;
                    }
                }
            }
        }
    }
    cov
}

/// Separable Gaussian blur of a square mask.
fn blur(mask: &[f32], size: usize, sigma: f32) -> Vec<f32> {
    if sigma < 0.3 {
        return mask.to_vec();
    }
    let radius = (sigma * 3.0).ceil() as isize;
    let mut k: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= sum);
    let pass = |src: &[f32], horizontal: bool| {
        let mut out = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                let mut acc = 0.0;
                for (i, w) in k.iter().enumerate() {
                    let d = i as isize - radius;
                    let (xx, yy) = if horizontal { (x as isize + d, y as isize) } else { (x as isize, y as isize + d) };
                    if xx >= 0 && yy >= 0 && (xx as usize) < size && (yy as usize) < size {
                        acc += src[yy as usize * size + xx as usize] * w;
                    }
                }
                out[y * size + x] = acc;
            }
        }
        out
    };
    pass(&pass(mask, true), false)
}

/// `over` for straight-alpha colours.
fn over(dst: Rgba, src: Rgba) -> Rgba {
    let a = src[3] + dst[3] * (1.0 - src[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0, 0.0, 0.0, a];
    for c in 0..3 {
        out[c] = (src[c] * src[3] + dst[c] * dst[3] * (1.0 - src[3])) / a;
    }
    out
}

/// Render `layers` into straight-alpha RGBA8.
pub fn render(view: View, layers: Layers) -> Vec<u8> {
    let size = view.size as usize;
    let s = view.scale;
    let nib: Vec<(f32, f32)> = nib_polygon().into_iter().map(|(x, y)| (x * s + view.offset.0, y * s + view.offset.1)).collect();
    let nib_cov = polygon_coverage(&nib, size);
    let glow = if layers == Layers::Full || layers == Layers::Foreground {
        blur(&nib_cov, size, GLOW_RADIUS * GLOW_SIGMA_PER_RADIUS * s)
    } else {
        Vec::new()
    };
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let px = (x as f32 + 0.5, y as f32 + 0.5);
            let p = view.to_unit(px.0, px.1);
            let aa = |sd: f32| (0.5 - sd * s).clamp(0.0, 1.0);
            let mut px_col: Rgba = [0.0; 4];
            let shape = match layers {
                Layers::Full => aa(rrect_sdf(p, 0.0, 0.0, CANVAS, CANVAS, CORNER_RADIUS)),
                Layers::Background => 1.0,
                _ => 0.0,
            };
            if matches!(layers, Layers::Full | Layers::Background) {
                let bg = ramp(&BG_STOPS, p.1 / CANVAS);
                px_col = [bg[0], bg[1], bg[2], 1.0];
                let d = ((p.0 - VIGNETTE_CENTRE.0).powi(2) + (p.1 - VIGNETTE_CENTRE.1).powi(2)).sqrt();
                let va = VIGNETTE_ALPHA * (1.0 - d / VIGNETTE_RADIUS).clamp(0.0, 1.0);
                let v = rgb(VIGNETTE_COLOUR);
                px_col = over(px_col, [v[0], v[1], v[2], va]);
                px_col[3] = shape;
            }
            if layers != Layers::Background {
                let i = y * size + x;
                if layers != Layers::Monochrome {
                    // Glow first, then bars, then the nib.
                    let g = rgb(GLOW_COLOUR);
                    px_col = over(px_col, [g[0], g[1], g[2], GLOW_OPACITY * glow[i].clamp(0.0, 1.0)]);
                }
                let mut bar_cov = 0.0f32;
                for &(bx, by, bw, bh) in BARS.iter() {
                    bar_cov = bar_cov.max(aa(rrect_sdf(p, bx, by, bw, bh, BAR_RADIUS)));
                }
                if layers == Layers::Monochrome {
                    let a = bar_cov.max(nib_cov[i].min(1.0));
                    px_col = over(px_col, [1.0, 1.0, 1.0, a]);
                } else {
                    let ink = ramp(&INK_STOPS, project(p, INK_FROM, INK_TO));
                    px_col = over(px_col, [ink[0], ink[1], ink[2], bar_cov]);
                    let n = ramp(&NIB_STOPS, project(p, NIB_FROM, NIB_TO));
                    px_col = over(px_col, [n[0], n[1], n[2], nib_cov[i].min(1.0)]);
                }
                // The full icon is clipped to its rounded square (the glow cannot leave it).
                if layers == Layers::Full {
                    px_col[3] = px_col[3].min(shape);
                }
            }
            for c in 0..3 {
                out.push((px_col[c] * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            out.push((px_col[3] * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    out
}

/// Apple's icon grid: the rounded-rectangle body is 824 of the 1024 canvas, centred, with the
/// rest transparent margin that holds the soft shadow.
pub const MACOS_BODY: f32 = 824.0;
/// Shadow: blur sigma and downward offset (1024 canvas units), and peak opacity.
const MACOS_SHADOW_SIGMA: f32 = 14.0;
const MACOS_SHADOW_DY: f32 = 12.0;
const MACOS_SHADOW_ALPHA: f32 = 0.32;

/// The macOS flavour of the full icon: the artwork scaled to the 824/1024 body, centred, with a
/// soft shadow below it in the margin. Dock and Cmd-Tab show the image as is, so without the
/// margin the icon looks larger than every other app's.
pub fn render_macos(size: u32) -> Vec<u8> {
    let n = size as usize;
    let k = size as f32 / CANVAS;
    let scale = k * MACOS_BODY / CANVAS;
    let margin = (CANVAS - MACOS_BODY) / 2.0 * k;
    let body = render(View { size, scale, offset: (margin, margin) }, Layers::Full);
    let alpha: Vec<f32> = body.chunks_exact(4).map(|p| f32::from(p[3]) / 255.0).collect();
    let mut shadow = blur(&alpha, n, MACOS_SHADOW_SIGMA * k);
    let dy = (MACOS_SHADOW_DY * k).round() as usize;
    let mut out = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let sa = if y >= dy { shadow[(y - dy) * n + x] } else { 0.0 } * MACOS_SHADOW_ALPHA;
            let i = (y * n + x) * 4;
            let b = [f32::from(body[i]) / 255.0, f32::from(body[i + 1]) / 255.0, f32::from(body[i + 2]) / 255.0, f32::from(body[i + 3]) / 255.0];
            let c = over([0.0, 0.0, 0.0, sa], b);
            for v in &c[..3] {
                out.push((v * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            out.push((c[3] * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    shadow.clear();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nib_outline_is_closed_and_inside_the_canvas() {
        let first = NIB[0];
        let last = NIB[NIB.len() - 1];
        assert!((first.0 - last.6).abs() < 0.01 && (first.1 - last.7).abs() < 0.01);
        for (x, y) in nib_polygon() {
            assert!((0.0..CANVAS).contains(&x) && (0.0..CANVAS).contains(&y));
        }
    }

    #[test]
    fn full_icon_has_transparent_corners_and_a_nib() {
        let img = render(View::full(128), Layers::Full);
        let px = |x: usize, y: usize| &img[(y * 128 + x) * 4..(y * 128 + x) * 4 + 4];
        assert_eq!(px(0, 0)[3], 0);
        assert_eq!(px(64, 2)[3], 255);
        // Inside the nib (near its middle) the red channel dominates.
        let c = px(80, 62);
        assert!(c[0] > 200 && c[2] < 160, "{c:?}");
    }

    #[test]
    fn adaptive_layers_are_transparent_where_expected() {
        let v = View::full(64);
        let bg = render(v, Layers::Background);
        assert!(bg.chunks_exact(4).all(|p| p[3] == 255));
        let fg = render(v, Layers::Foreground);
        assert_eq!(fg[3], 0);
        let mono = render(v, Layers::Monochrome);
        assert!(mono.chunks_exact(4).all(|p| p[3] == 0 || (p[0], p[1], p[2]) == (255, 255, 255)));
        assert!(mono.chunks_exact(4).any(|p| p[3] == 255));
    }

    #[test]
    fn macos_icon_body_is_80_percent_centred_with_a_soft_shadow() {
        let size = 256usize;
        let img = render_macos(size as u32);
        let a = |x: usize, y: usize| img[(y * size + x) * 4 + 3];
        let (mut x0, mut y0, mut x1, mut y1) = (size, size, 0, 0);
        for y in 0..size {
            for x in 0..size {
                if a(x, y) >= 250 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        let ratio = (x1 - x0) as f32 / size as f32;
        assert!((ratio - 824.0 / 1024.0).abs() < 0.01, "{ratio}");
        assert_eq!(x0 + x1, size, "centred horizontally");
        assert_eq!(y0 + y1, size, "centred vertically");
        // Shadow below the body, none at the very edge.
        assert!(a(size / 2, y1 + 2) > 0 && a(size / 2, y1 + 2) < 120);
        assert_eq!(a(0, 0), 0);
        assert_eq!(a(size / 2, size - 1), 0);
    }
}
