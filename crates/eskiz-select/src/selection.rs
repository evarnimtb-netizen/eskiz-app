//! Selection state: no selection versus an active mask, combining, inversion, the paint clip,
//! and saved selections.

use crate::error::Result;
use crate::geom::IRect;
use crate::mask::{CombineMode, Mask};

/// The document selection. Either no selection (every pixel counts as selected) or an
/// active mask (pixels without data count as 0).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Selection {
    mask: Option<Mask>,
}

impl Selection {
    /// No selection.
    pub fn none() -> Self {
        Self { mask: None }
    }

    /// An active selection holding `mask`.
    pub fn active(mask: Mask) -> Self {
        Self { mask: Some(mask) }
    }

    pub fn is_active(&self) -> bool {
        self.mask.is_some()
    }

    pub fn mask(&self) -> Option<&Mask> {
        self.mask.as_ref()
    }

    /// Selection value at a pixel: 1 with no selection, otherwise the mask value.
    pub fn value_at(&self, x: usize, y: usize) -> f32 {
        match &self.mask {
            None => 1.0,
            Some(m) => m.get(x, y),
        }
    }

    /// Effective paint coverage `c' = c * s`.
    pub fn clip_coverage(&self, x: usize, y: usize, coverage: f32) -> f32 {
        coverage * self.value_at(x, y)
    }

    /// Bounding box of the selected pixels, if any.
    pub fn bounds(&self) -> Option<IRect> {
        self.mask.as_ref()?.bounds()
    }

    /// Combines `new` with the current selection. A missing selection counts as an
    /// empty mask for Add and Subtract.
    pub fn combine(&self, new: &Mask, mode: CombineMode) -> Result<Selection> {
        match mode {
            CombineMode::Replace => Ok(Selection::active(new.clone())),
            _ => {
                let old = match &self.mask {
                    Some(m) => m.clone(),
                    None => Mask::empty(new.width, new.height),
                };
                Ok(Selection::active(old.combine(new, mode)?))
            }
        }
    }

    /// Inverts the selection. With no selection, the result selects the whole canvas.
    pub fn invert(&self, width: usize, height: usize) -> Selection {
        match &self.mask {
            None => Selection::active(Mask::full(width, height)),
            Some(m) => Selection::active(m.invert()),
        }
    }
}

/// A named saved selection with a stable identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedSelection {
    pub id: u64,
    pub name: String,
    pub mask: Mask,
}

/// The document's saved selections. Identifiers are never reused.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SavedSelections {
    items: Vec<SavedSelection>,
    next_id: u64,
}

impl SavedSelections {
    /// Stores a copy of `mask` and returns its identifier.
    pub fn save(&mut self, name: impl Into<String>, mask: &Mask) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(SavedSelection {
            id,
            name: name.into(),
            mask: mask.clone(),
        });
        id
    }

    /// A copy of the saved mask, exactly as stored.
    pub fn load(&self, id: u64) -> Option<Mask> {
        self.items
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.mask.clone())
    }

    /// Removes a saved selection and returns it.
    pub fn remove(&mut self, id: u64) -> Option<SavedSelection> {
        let pos = self.items.iter().position(|s| s.id == id)?;
        Some(self.items.remove(pos))
    }

    pub fn list(&self) -> &[SavedSelection] {
        &self.items
    }

    /// Rebuilds the store from saved items, for a document read back from disk. The items keep
    /// their identifiers; new ones are numbered above the largest.
    pub fn from_items(items: Vec<SavedSelection>) -> Self {
        let next_id = items.iter().map(|s| s.id + 1).max().unwrap_or(0);
        Self { items, next_id }
    }
}

/// Thumbnail size with the longest edge at most `max_edge` pixels, aspect kept.
/// The reference uses 138 points; the caller converts that to pixels with its display scale.
pub fn thumbnail_size(width: usize, height: usize, max_edge: usize) -> (usize, usize) {
    let longest = width.max(height);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = max_edge as f32 / longest as f32;
    let fit = |v: usize| ((v as f32 * scale).round() as usize).max(1);
    (fit(width), fit(height))
}
