use std::io;

/// Errors from reading a `.procreate` document.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file could not be read.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// The ZIP container is damaged or uses an unsupported feature.
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// A property list could not be parsed.
    #[error("property list error: {0}")]
    Plist(#[from] plist::Error),
    /// The ZIP does not contain a `Document.archive`.
    #[error("not a valid .procreate file: {0}")]
    NotProcreate(String),
    /// The NSKeyedArchiver structure is malformed or unsupported.
    #[error("malformed archive: {0}")]
    Format(String),
    /// Compressed data is damaged.
    #[error("corrupt data: {0}")]
    Corrupt(String),
    /// The document uses a feature this reader does not decode (for example 16-bit pixels).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A document or archive to be written is inconsistent (wrong pixel count, bad blend mode).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The PNG encoder failed.
    #[error("encoding error: {0}")]
    Encode(String),
}

/// Convenience alias for results in this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_procreate_message_is_user_facing() {
        let e = Error::NotProcreate("no Document.archive".into());
        assert_eq!(e.to_string(), "not a valid .procreate file: no Document.archive");
    }
}
