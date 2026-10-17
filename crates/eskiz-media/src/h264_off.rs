//! Stand-in for the OpenH264 backend in builds without `source` or `libloading`: no encoder is linked.

/// Environment variable that names a runtime OpenH264 shared library (`libloading` feature).
pub const LIBRARY_ENV: &str = "ESKIZ_OPENH264_LIB";

/// Shown wherever MP4 export is requested in a build without an encoder.
pub const MP4_UNAVAILABLE: &str =
    "MP4 export is not available in this build. Export a GIF, APNG or PNG sequence instead.";

/// Nothing to switch off: this build has no encoder.
pub fn force_encoder_unavailable(_on: bool) {}

/// Always false: no H.264 encoder is compiled into this build.
pub fn encoder_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_encoder_in_this_build() {
        assert!(!encoder_available());
        assert!(MP4_UNAVAILABLE.contains("GIF, APNG or PNG"));
    }

    #[test]
    fn mp4_paths_report_unavailable() {
        let frame = crate::Frame::new(4, 4, vec![255; 64], std::time::Duration::from_millis(40)).unwrap();
        let err = crate::encode_mp4(&[frame], &crate::Mp4Options::default()).unwrap_err();
        assert!(matches!(err, crate::MediaError::EncoderUnavailable(m) if m == MP4_UNAVAILABLE));
    }
}
