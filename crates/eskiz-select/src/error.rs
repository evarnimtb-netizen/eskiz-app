//! Error type shared by the crate.

use std::fmt;

/// Reasons an operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectError {
    /// Two buffers that must match in size do not.
    SizeMismatch,
    /// A rectangle or selection has no area (for example a zero-width crop).
    EmptyRect,
    /// A canvas or image side is outside the allowed range.
    SizeOutOfRange,
    /// The operation needs pixels or selected area and found none.
    NoContent,
    /// A matrix or quad is degenerate and cannot be inverted or solved.
    Singular,
    /// Serialised data is malformed.
    BadData,
    /// The operation is not valid in the session's current mode (for example a quad edit
    /// while the session is in warp mode).
    WrongMode,
}

/// Result alias used across the crate.
pub type Result<T> = std::result::Result<T, SelectError>;

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            SelectError::SizeMismatch => "buffers differ in size",
            SelectError::EmptyRect => "rectangle has no area",
            SelectError::SizeOutOfRange => "size is outside the allowed range",
            SelectError::NoContent => "nothing to operate on",
            SelectError::Singular => "matrix or quad is degenerate",
            SelectError::BadData => "serialized data is malformed",
            SelectError::WrongMode => "operation is not valid in the current session mode",
        };
        f.write_str(text)
    }
}

impl std::error::Error for SelectError {}
