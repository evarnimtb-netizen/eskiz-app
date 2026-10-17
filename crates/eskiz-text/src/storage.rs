//! Attribute storage keys. These functions map attributes to saved key/value pairs
//! and back. They do not touch the file format, which is `eskiz-format`'s job.
//!
//! The reference keys are the five from the documented table. Attributes that the design notes do not give
//! a reference key for are stored under `eskiz.`-prefixed keys. Eskiz's own format is `[choice]`
//! in the design notes, and it must round-trip every attribute. Unknown keys are dropped on load.

use thiserror::Error;

use crate::model::{Alignment, Attrs, Color, FontFace, WritingDirection};

/// Font name, stored as `Family-Style` (for example `Helvetica-Bold`).
pub const KEY_FONT_NAME: &str = "_CFN";
/// Point size.
pub const KEY_FONT_SIZE: &str = "_CFZ";
/// Kerning, as an em fraction.
pub const KEY_KERNING: &str = "_CFK";
/// Tracking, as an em fraction.
pub const KEY_TRACKING: &str = "_CFT";
/// Outline width.
pub const KEY_OUTLINE: &str = "_CFI";

/// Prefix the first releases wrote, before the app was renamed. Read, never written.
pub const LEGACY_PREFIX: &str = "easel."; // legacy-easel
/// Prefix of the keys this app writes.
pub const PREFIX: &str = "eskiz.";

/// Maps a key written by an older release (`eskiz.*`) to its current name (`eskiz.*`).
pub fn canonical_key(key: &str) -> std::borrow::Cow<'_, str> {
    match key.strip_prefix(LEGACY_PREFIX) {
        Some(rest) => std::borrow::Cow::Owned(format!("{PREFIX}{rest}")),
        None => std::borrow::Cow::Borrowed(key),
    }
}

pub const KEY_COLOUR: &str = "eskiz.colour";
pub const KEY_UNDERLINE: &str = "eskiz.underline";
pub const KEY_BASELINE_SHIFT: &str = "eskiz.baselineShift";
pub const KEY_ALIGNMENT: &str = "eskiz.alignment";
pub const KEY_LEADING: &str = "eskiz.leading";
pub const KEY_WRITING_DIRECTION: &str = "eskiz.writingDirection";
pub const KEY_VERTICAL: &str = "eskiz.vertical";
pub const KEY_ALL_CAPS: &str = "eskiz.allCaps";

/// The five reference keys from the documented table.
pub const REFERENCE_KEYS: [&str; 5] = [
    KEY_FONT_NAME,
    KEY_FONT_SIZE,
    KEY_KERNING,
    KEY_TRACKING,
    KEY_OUTLINE,
];

/// A saved value. The documented keys need text, numbers and flags.
#[derive(Clone, Debug, PartialEq)]
pub enum SavedValue {
    Text(String),
    Number(f64),
    Flag(bool),
}

/// Errors when a saved value does not fit its key.
#[derive(Clone, Debug, PartialEq, Error)]
pub enum StorageError {
    #[error("attribute {key} has a value of the wrong type")]
    WrongType { key: String },
    #[error("attribute {key} has an invalid value {value:?}")]
    InvalidValue { key: String, value: String },
}

/// The saved font name for a family and face. The style is always appended, so the name splits
/// back unambiguously.
pub fn font_name_value(family: &str, face: FontFace) -> String {
    format!("{family}-{}", face.name())
}

/// Splits a saved font name into family and face. A name without a known style suffix is taken
/// as a Regular face of the whole name.
pub fn parse_font_name(name: &str) -> (String, FontFace) {
    if let Some((family, suffix)) = name.rsplit_once('-') {
        if let Some(face) = FontFace::from_name(suffix) {
            return (family.to_string(), face);
        }
    }
    (name.to_string(), FontFace::Regular)
}

fn alignment_name(alignment: Alignment) -> &'static str {
    match alignment {
        Alignment::Left => "left",
        Alignment::Centre => "centre",
        Alignment::Right => "right",
        Alignment::Justified => "justified",
    }
}

/// Every saved key/value pair for one attribute set.
pub fn attrs_to_saved(attrs: &Attrs) -> Vec<(&'static str, SavedValue)> {
    vec![
        (
            KEY_FONT_NAME,
            SavedValue::Text(font_name_value(&attrs.font_family, attrs.font_face)),
        ),
        (KEY_FONT_SIZE, SavedValue::Number(f64::from(attrs.size_pt))),
        (KEY_KERNING, SavedValue::Number(f64::from(attrs.kerning))),
        (KEY_TRACKING, SavedValue::Number(f64::from(attrs.tracking))),
        (KEY_OUTLINE, SavedValue::Number(f64::from(attrs.outline_pt))),
        (KEY_COLOUR, SavedValue::Text(attrs.color.to_hex())),
        (KEY_UNDERLINE, SavedValue::Flag(attrs.underline)),
        (
            KEY_BASELINE_SHIFT,
            SavedValue::Number(f64::from(attrs.baseline_shift_pt)),
        ),
        (
            KEY_ALIGNMENT,
            SavedValue::Text(alignment_name(attrs.alignment).to_string()),
        ),
        (KEY_LEADING, SavedValue::Number(f64::from(attrs.leading))),
        (KEY_WRITING_DIRECTION, SavedValue::Text("ltr".to_string())),
        (KEY_VERTICAL, SavedValue::Flag(attrs.vertical)),
        (KEY_ALL_CAPS, SavedValue::Flag(attrs.all_caps)),
    ]
}

fn number(key: &str, value: &SavedValue) -> Result<f32, StorageError> {
    match value {
        SavedValue::Number(n) if n.is_finite() => Ok(*n as f32),
        SavedValue::Number(n) => Err(StorageError::InvalidValue {
            key: key.to_string(),
            value: n.to_string(),
        }),
        _ => Err(StorageError::WrongType {
            key: key.to_string(),
        }),
    }
}

fn text<'a>(key: &str, value: &'a SavedValue) -> Result<&'a str, StorageError> {
    match value {
        SavedValue::Text(s) => Ok(s.as_str()),
        _ => Err(StorageError::WrongType {
            key: key.to_string(),
        }),
    }
}

fn flag(key: &str, value: &SavedValue) -> Result<bool, StorageError> {
    match value {
        SavedValue::Flag(b) => Ok(*b),
        _ => Err(StorageError::WrongType {
            key: key.to_string(),
        }),
    }
}

fn invalid(key: &str, value: &str) -> StorageError {
    StorageError::InvalidValue {
        key: key.to_string(),
        value: value.to_string(),
    }
}

/// Applies one saved pair to `attrs`. Returns `Ok(true)` when the key was used, and `Ok(false)`
/// for a key this module does not own, which is dropped. Numbers are clamped to their ranges.
pub fn apply_saved(attrs: &mut Attrs, key: &str, value: &SavedValue) -> Result<bool, StorageError> {
    let key = canonical_key(key);
    let key = key.as_ref();
    match key {
        KEY_FONT_NAME => {
            let (family, face) = parse_font_name(text(key, value)?);
            attrs.font_family = family;
            attrs.font_face = face;
        }
        KEY_FONT_SIZE => attrs.size_pt = number(key, value)?,
        KEY_KERNING => attrs.kerning = number(key, value)?,
        KEY_TRACKING => attrs.tracking = number(key, value)?,
        KEY_OUTLINE => attrs.outline_pt = number(key, value)?,
        KEY_COLOUR => {
            let raw = text(key, value)?;
            attrs.color = Color::from_hex(raw).ok_or_else(|| invalid(key, raw))?;
        }
        KEY_UNDERLINE => attrs.underline = flag(key, value)?,
        KEY_BASELINE_SHIFT => attrs.baseline_shift_pt = number(key, value)?,
        KEY_ALIGNMENT => {
            let raw = text(key, value)?;
            attrs.alignment = match raw {
                "left" => Alignment::Left,
                "centre" => Alignment::Centre,
                "right" => Alignment::Right,
                "justified" => Alignment::Justified,
                other => return Err(invalid(key, other)),
            };
        }
        KEY_LEADING => attrs.leading = number(key, value)?,
        KEY_WRITING_DIRECTION => {
            let raw = text(key, value)?;
            attrs.writing_direction = match raw {
                "ltr" => WritingDirection::LeftToRight,
                other => return Err(invalid(key, other)),
            };
        }
        KEY_VERTICAL => attrs.vertical = flag(key, value)?,
        KEY_ALL_CAPS => attrs.all_caps = flag(key, value)?,
        _ => return Ok(false),
    }
    *attrs = std::mem::take(attrs).clamped();
    Ok(true)
}
