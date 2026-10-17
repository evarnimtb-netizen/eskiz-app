//! Error type shared by every export and recording path.

/// Errors from Eskiz media export and timelapse recording.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    /// A frame's pixel buffer does not match its size, or frames in one export disagree on size.
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    /// An option is out of range or inconsistent.
    #[error("invalid settings: {0}")]
    InvalidSettings(String),
    /// The export has no frames to write.
    #[error("no frames to export")]
    NoFrames,
    /// The H.264 encoder cannot be initialised on this platform or build.
    #[error("H.264 encoder unavailable: {0}")]
    EncoderUnavailable(String),
    /// The document is too small for a timelapse capture (either side under 128 px).
    #[error("Document too small for a video capture ({width} x {height} px)")]
    DocumentTooSmall { width: u32, height: u32 },
    /// The requested operation is not implemented.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// An OpenH264 encode or decode call failed.
    #[cfg(feature = "h264")]
    #[error("H.264 error: {0}")]
    H264(#[from] openh264::Error),
    /// The GIF encoder failed.
    #[error("GIF error: {0}")]
    Gif(#[from] gif::EncodingError),
    /// The PNG (APNG) encoder failed.
    #[error("PNG error: {0}")]
    Png(#[from] png::EncodingError),
    /// The MP4 muxer or reader failed.
    #[cfg(feature = "h264")]
    #[error("MP4 error: {0}")]
    Mp4(#[from] mp4::Error),
    /// Reading or writing a file failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, MediaError>;
