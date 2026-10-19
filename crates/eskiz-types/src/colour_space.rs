//! sRGB and Display P3 conversion.
//!
//! Both spaces use the sRGB transfer curve (IEC 61966-2-1) and the D65 white point.
//! Conversion decodes each channel to linear light, applies a 3x3 matrix through CIE
//! XYZ, and encodes the result again:
//!
//! ```text
//! linear_to = M_to^-1 * M_from * linear_from
//! ```
//!
//! `M_space` (linear RGB to XYZ) is derived in code from each space's published
//! primary chromaticities and the D65 white point, by the standard construction
//! (Lindbloom's method, no chromatic adaptation). Deriving the matrices, rather than
//! typing them in, keeps the white point exact: RGB (1, 1, 1) maps to D65 in both spaces.
//!
//! sRGB lies inside Display P3, so sRGB to P3 never leaves `0..=1`. P3 to sRGB can
//! leave it, which is why [`ColourConverter::apply`] clamps and
//! [`ColourConverter::apply_unclamped`] does not.

use crate::colour::Rgb;

/// A colour space the converter understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ColourSpace {
    /// sRGB (IEC 61966-2-1). The default for new documents.
    #[default]
    Srgb,
    DisplayP3,
}

impl ColourSpace {
    /// Both spaces, in the order the New canvas popover lists them.
    pub const ALL: [Self; 2] = [Self::Srgb, Self::DisplayP3];

    /// The name shown in the interface.
    pub fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
        }
    }

    /// The ICC profile name stored in a `.procreate` document (`SiColorProfileArchiveICCNameKey`)
    /// and in the `desc` tag of the profile Eskiz embeds in exports.
    pub fn profile_name(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB IEC61966-2.1",
            Self::DisplayP3 => "Display P3",
        }
    }

    /// The space a stored ICC profile name stands for. A name that mentions `P3` is Display P3;
    /// anything else (including profiles Eskiz does not know, such as a generic RGB profile) is
    /// read as sRGB, which is what such documents are displayed with.
    pub fn from_profile_name(name: &str) -> Self {
        if name.to_ascii_lowercase().contains("p3") {
            Self::DisplayP3
        } else {
            Self::Srgb
        }
    }

    /// The conversion that takes a colour in `self` to what an sRGB display shows. `None` when
    /// no conversion is needed (the document is already sRGB).
    pub fn display_transform(self) -> Option<PixelConverter> {
        (self != Self::Srgb).then(|| PixelConverter::new(self, Self::Srgb))
    }
}

/// A 3x3 matrix in row-major order.
pub type Matrix3 = [[f64; 3]; 3];

/// CIE xy chromaticity.
type Xy = (f64, f64);

/// D65 white point, shared by sRGB and Display P3.
pub const D65: Xy = (0.3127, 0.3290);

/// sRGB primaries (red, green, blue), from IEC 61966-2-1.
pub const SRGB_PRIMARIES: [Xy; 3] = [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)];

/// Display P3 primaries (red, green, blue), from SMPTE EG 432-1.
pub const DISPLAY_P3_PRIMARIES: [Xy; 3] = [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)];

/// XYZ tristimulus of a chromaticity at luminance Y = 1.
fn xyz_of((x, y): Xy) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Linear RGB to XYZ for a set of primaries and a white point, with the white
/// normalised to (1, 1, 1) RGB.
pub fn rgb_to_xyz_matrix(primaries: [Xy; 3], white: Xy) -> Matrix3 {
    // Columns are the primaries' XYZ. The scale vector makes RGB (1, 1, 1) equal white.
    let prim = primaries.map(xyz_of);
    let columns: Matrix3 = std::array::from_fn(|row| std::array::from_fn(|col| prim[col][row]));
    let inv = mat_inv(&columns);
    let w = xyz_of(white);
    let scale: [f64; 3] = std::array::from_fn(|row| (0..3).map(|k| inv[row][k] * w[k]).sum());
    std::array::from_fn(|row| std::array::from_fn(|col| columns[row][col] * scale[col]))
}

/// Linear sRGB to XYZ (D65).
pub fn srgb_to_xyz() -> Matrix3 {
    rgb_to_xyz_matrix(SRGB_PRIMARIES, D65)
}

/// Linear Display P3 to XYZ (D65).
pub fn display_p3_to_xyz() -> Matrix3 {
    rgb_to_xyz_matrix(DISPLAY_P3_PRIMARIES, D65)
}

fn to_xyz(space: ColourSpace) -> Matrix3 {
    match space {
        ColourSpace::Srgb => srgb_to_xyz(),
        ColourSpace::DisplayP3 => display_p3_to_xyz(),
    }
}

/// sRGB electro-optical transfer: encoded value to linear light (odd-symmetric).
pub fn decode_transfer(encoded: f64) -> f64 {
    let a = encoded.abs();
    let linear = if a <= 0.04045 {
        a / 12.92
    } else {
        ((a + 0.055) / 1.055).powf(2.4)
    };
    linear.copysign(encoded)
}

/// Inverse of [`decode_transfer`]: linear light to encoded value (odd-symmetric).
pub fn encode_transfer(linear: f64) -> f64 {
    let a = linear.abs();
    let encoded = if a <= 0.003_130_8 {
        a * 12.92
    } else {
        1.055 * a.powf(1.0 / 2.4) - 0.055
    };
    encoded.copysign(linear)
}

/// Product of two 3x3 matrices, `a * b`.
pub fn mat_mul(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// Inverse of a 3x3 matrix, by the adjugate. The caller must pass a non-singular matrix.
pub fn mat_inv(m: &Matrix3) -> Matrix3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let inv = 1.0 / det;
    [
        [
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
        ],
        [
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
        ],
        [
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ],
    ]
}

/// The matrix taking linear `from` RGB to linear `to` RGB.
pub fn linear_matrix(from: ColourSpace, to: ColourSpace) -> Matrix3 {
    mat_mul(&mat_inv(&to_xyz(to)), &to_xyz(from))
}

/// Applies the transfer curves and the matrix to a colour. Build one with
/// [`ColourConverter::new`] when converting many pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColourConverter {
    matrix: Matrix3,
}

impl ColourConverter {
    pub fn new(from: ColourSpace, to: ColourSpace) -> Self {
        Self {
            matrix: linear_matrix(from, to),
        }
    }

    /// The linear-light matrix this converter applies.
    pub fn matrix(&self) -> Matrix3 {
        self.matrix
    }

    /// Converts without clamping. Out-of-gamut results can fall outside `0..=1`.
    pub fn apply_unclamped(&self, c: Rgb) -> Rgb {
        let lin = [
            decode_transfer(c.r),
            decode_transfer(c.g),
            decode_transfer(c.b),
        ];
        let out: [f64; 3] =
            std::array::from_fn(|row| (0..3).map(|k| self.matrix[row][k] * lin[k]).sum::<f64>());
        Rgb::new(
            encode_transfer(out[0]),
            encode_transfer(out[1]),
            encode_transfer(out[2]),
        )
    }

    /// Converts and clamps each channel to `0..=1`, ready for display.
    pub fn apply(&self, c: Rgb) -> Rgb {
        let u = self.apply_unclamped(c);
        Rgb::new(
            u.r.clamp(0.0, 1.0),
            u.g.clamp(0.0, 1.0),
            u.b.clamp(0.0, 1.0),
        )
    }
}

/// One-off conversion. For many pixels, use [`ColourConverter`].
pub fn convert(c: Rgb, from: ColourSpace, to: ColourSpace) -> Rgb {
    ColourConverter::new(from, to).apply(c)
}

/// Converts 8-bit pixels between spaces. The decode step is a 256-entry table and the encode step
/// is a table over linear light that is interpolated, so a full canvas converts without a
/// `powf` per channel. Out-of-gamut results are clamped per channel (a P3 colour outside sRGB
/// loses its excess; P3 `(1, 0, 0)` becomes sRGB `(255, 0, 0)`).
///
/// The same 3x3 matrix ([`PixelConverter::matrix_f32`]) and sRGB transfer curve are what the GPU
/// compositor's display pass applies, so the two paths agree to within a level or two.
#[derive(Clone, Debug)]
pub struct PixelConverter {
    matrix: [[f32; 3]; 3],
    decode: [f32; 256],
    encode: Vec<f32>,
}

/// Entries in the encode table (the table has one more, for interpolation at 1.0).
const ENCODE_STEPS: usize = 4096;

impl PixelConverter {
    pub fn new(from: ColourSpace, to: ColourSpace) -> Self {
        let m = linear_matrix(from, to);
        let matrix = std::array::from_fn(|r| std::array::from_fn(|c| m[r][c] as f32));
        let decode = std::array::from_fn(|i| decode_transfer(i as f64 / 255.0) as f32);
        let encode = (0..=ENCODE_STEPS)
            .map(|i| encode_transfer(i as f64 / ENCODE_STEPS as f64) as f32)
            .collect();
        Self { matrix, decode, encode }
    }

    /// The linear-light matrix as `f32`, row-major (for a shader uniform).
    pub fn matrix_f32(&self) -> [[f32; 3]; 3] {
        self.matrix
    }

    /// Encoded value for linear light, by table lookup with linear interpolation.
    fn encode_lut(&self, linear: f32) -> u8 {
        let x = linear.clamp(0.0, 1.0) * ENCODE_STEPS as f32;
        let i = (x as usize).min(ENCODE_STEPS - 1);
        let f = x - i as f32;
        let v = self.encode[i] + (self.encode[i + 1] - self.encode[i]) * f;
        (v * 255.0 + 0.5) as u8
    }

    /// Converts one opaque colour.
    pub fn convert(&self, rgb: [u8; 3]) -> [u8; 3] {
        let lin = rgb.map(|c| self.decode[usize::from(c)]);
        std::array::from_fn(|r| {
            let m = &self.matrix[r];
            self.encode_lut(m[0] * lin[0] + m[1] * lin[1] + m[2] * lin[2])
        })
    }

    /// Converts straight RGBA8 in place. Alpha is untouched.
    pub fn convert_straight_rgba8(&self, rgba: &mut [u8]) {
        for px in rgba.as_chunks_mut::<4>().0 {
            let out = self.convert([px[0], px[1], px[2]]);
            px[..3].copy_from_slice(&out);
        }
    }

    /// Converts one premultiplied RGBA8 pixel: it is un-premultiplied, converted and
    /// premultiplied again. A fully transparent pixel is returned as it is.
    pub fn convert_premultiplied(&self, px: [u8; 4]) -> [u8; 4] {
        let a = u32::from(px[3]);
        match a {
            0 => px,
            255 => {
                let c = self.convert([px[0], px[1], px[2]]);
                [c[0], c[1], c[2], 255]
            }
            _ => {
                let straight = [px[0], px[1], px[2]].map(|c| ((u32::from(c) * 255 + a / 2) / a).min(255) as u8);
                let c = self.convert(straight);
                let pre = |v: u8| ((u32::from(v) * a + 127) / 255) as u8;
                [pre(c[0]), pre(c[1]), pre(c[2]), px[3]]
            }
        }
    }
}
