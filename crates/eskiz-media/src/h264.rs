//! OpenH264 backend: library selection, encoder and decoder set-up, and RGB to YUV conversion.
//!
//! Library selection:
//! - `source` feature (default): OpenH264 compiled from source.
//! - `libloading` feature: a shared library loaded at runtime from the path in [`LIBRARY_ENV`]. Use this for
//!   Cisco's prebuilt binary (see the crate README). When [`LIBRARY_ENV`] is unset, the `source` backend is
//!   used if it is compiled in.

use openh264::decoder::{Decoder, DecoderConfig};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, Profile, RateControlMode, UsageType, VuiConfig,
};
use openh264::formats::{YUVBuffer, YUVSource};
use openh264::OpenH264API;

use crate::error::{MediaError, Result};

/// Environment variable that names a runtime OpenH264 shared library (`libloading` feature).
pub const LIBRARY_ENV: &str = "ESKIZ_OPENH264_LIB";
/// The name releases before the rename used; still honoured.
#[cfg(feature = "libloading")]
pub const LEGACY_LIBRARY_ENV: &str = "EASEL_OPENH264_LIB"; // legacy-easel

#[cfg(feature = "libloading")]
fn library_override() -> Option<Result<OpenH264API>> {
    std::env::var_os(LIBRARY_ENV).or_else(|| std::env::var_os(LEGACY_LIBRARY_ENV)).map(|path| {
        OpenH264API::from_blob_path(&path)
            .map_err(|e| MediaError::EncoderUnavailable(format!("{}: {e}", path.to_string_lossy())))
    })
}

#[cfg(not(feature = "libloading"))]
fn library_override() -> Option<Result<OpenH264API>> {
    None
}

#[cfg(feature = "source")]
fn default_api() -> Result<OpenH264API> {
    Ok(OpenH264API::from_source())
}

/// Shown wherever MP4 export is requested in a build without an encoder.
pub const MP4_UNAVAILABLE: &str =
    "MP4 export is not available in this build. Export a GIF, APNG or PNG sequence instead.";

#[cfg(not(feature = "source"))]
fn default_api() -> Result<OpenH264API> {
    Err(MediaError::EncoderUnavailable(MP4_UNAVAILABLE.to_owned()))
}

thread_local! {
    static FORCE_UNAVAILABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes the H.264 encoder and decoder unavailable on this thread, as on Android builds without
/// OpenH264. For tests of the fallback paths.
pub fn force_encoder_unavailable(on: bool) {
    FORCE_UNAVAILABLE.with(|f| f.set(on));
}

/// Whether an H.264 encoder can be created in this build and environment.
pub fn encoder_available() -> bool {
    openh264_api().is_ok()
}

/// The OpenH264 API handle for this build and environment.
pub(crate) fn openh264_api() -> Result<OpenH264API> {
    if FORCE_UNAVAILABLE.with(std::cell::Cell::get) {
        return Err(MediaError::EncoderUnavailable("forced off for a test".to_owned()));
    }
    match library_override() {
        Some(api) => api,
        None => default_api(),
    }
}

/// H.264 encoder configured for Eskiz: High profile, bitrate rate control, BT.709 tags, no frame skipping.
pub(crate) struct H264Encoder {
    inner: Encoder,
}

impl H264Encoder {
    pub(crate) fn new(bitrate_bps: u32, max_fps: f32) -> Result<Self> {
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(max_fps))
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .profile(Profile::High)
            .skip_frames(false)
            .vui(VuiConfig::bt709());
        let inner = Encoder::with_api_config(openh264_api()?, config)
            .map_err(|e| MediaError::EncoderUnavailable(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Encodes one picture. Returns Annex B bytes for the access unit, which are empty when the encoder
    /// buffers the picture.
    pub(crate) fn encode<Y: YUVSource>(&mut self, yuv: &Y) -> Result<Vec<u8>> {
        Ok(self.inner.encode(yuv)?.to_vec())
    }
}

/// H.264 decoder with the same library selection as the encoder.
pub(crate) fn new_decoder() -> Result<Decoder> {
    Ok(Decoder::with_api_config(
        openh264_api()?,
        DecoderConfig::new(),
    )?)
}

/// Converts 8-bit RGB to a BT.709 limited-range I420 buffer.
///
/// `src` is the picture size inside `rgb`. `dst` is the even-sized coded size. The extra column or row is
/// filled with `fill`. Chroma is averaged over each 2 x 2 block.
pub(crate) fn rgb_to_i420(
    rgb: &[u8],
    src: (usize, usize),
    dst: (usize, usize),
    fill: [u8; 3],
) -> YUVBuffer {
    let (sw, sh) = src;
    let (dw, dh) = dst;
    let texel = |x: usize, y: usize| -> [f32; 3] {
        let px = if x < sw && y < sh {
            let i = (y * sw + x) * 3;
            [rgb[i], rgb[i + 1], rgb[i + 2]]
        } else {
            fill
        };
        px.map(|c| f32::from(c) / 255.0)
    };
    let luma = |[r, g, b]: [f32; 3]| 0.2126 * r + 0.7152 * g + 0.0722 * b;

    let (cw, ch) = (dw / 2, dh / 2);
    let mut yuv = vec![0u8; dw * dh + 2 * cw * ch];
    let (y_plane, chroma) = yuv.split_at_mut(dw * dh);
    let (u_plane, v_plane) = chroma.split_at_mut(cw * ch);
    for y in 0..dh {
        for x in 0..dw {
            let l = luma(texel(x, y));
            y_plane[y * dw + x] = (16.0 + 219.0 * l).round() as u8;
        }
    }
    for cy in 0..ch {
        for cx in 0..cw {
            let (mut cb, mut cr) = (0.0f32, 0.0f32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let [r, g, b] = texel(cx * 2 + dx, cy * 2 + dy);
                let l = luma([r, g, b]);
                cb += (b - l) / 1.8556;
                cr += (r - l) / 1.5748;
            }
            u_plane[cy * cw + cx] = (128.0 + 224.0 * cb / 4.0).round() as u8;
            v_plane[cy * cw + cx] = (128.0 + 224.0 * cr / 4.0).round() as u8;
        }
    }
    YUVBuffer::from_vec(yuv, dw, dh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forced_off_reports_unavailable() {
        force_encoder_unavailable(true);
        assert!(!encoder_available());
        force_encoder_unavailable(false);
    }

    #[test]
    fn the_unavailable_message_names_the_alternatives() {
        assert_eq!(
            MP4_UNAVAILABLE,
            "MP4 export is not available in this build. Export a GIF, APNG or PNG sequence instead."
        );
    }
}
