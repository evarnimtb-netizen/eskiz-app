//! Saved selections persist with their document. They are kept in
//! `<library>/<id>.selections` beside the `.procreate` package and the `.guides` file, so they
//! survive a save and a reload. The `.procreate` writer has no field for them.
//!
//! The file is binary: a magic line, a count, then each selection's id, name and mask (the mask
//! in `eskiz-select`'s own exact encoding).

use std::fs;
use std::path::{Path, PathBuf};

use eskiz_select::mask::Mask;
use eskiz_select::selection::{SavedSelection, SavedSelections};

use crate::app::EskizApp;

const MAGIC: &[u8; 8] = b"ESELSEL1";

/// Where the saved selections of document `id` live in the library folder `dir`.
pub fn file_for(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.selections"))
}

/// The file's bytes for `store`.
pub fn encode(store: &SavedSelections) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(store.list().len() as u32).to_le_bytes());
    for s in store.list() {
        let name = s.name.as_bytes();
        let mask = s.mask.to_bytes();
        out.extend_from_slice(&s.id.to_le_bytes());
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(&(mask.len() as u64).to_le_bytes());
        out.extend_from_slice(&mask);
    }
    out
}

/// Read a file written by [`encode`]. `None` when it is not one, or is damaged.
pub fn decode(bytes: &[u8]) -> Option<SavedSelections> {
    let mut r = Reader { bytes, pos: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return None;
    }
    let count = r.u32()? as usize;
    let mut items = Vec::new();
    for _ in 0..count {
        let id = r.u64()?;
        let name_len = r.u32()? as usize;
        let name = String::from_utf8(r.take(name_len)?.to_vec()).ok()?;
        let mask_len = r.u64()? as usize;
        let mask = Mask::from_bytes(r.take(mask_len)?).ok()?;
        items.push(SavedSelection { id, name, mask });
    }
    Some(SavedSelections::from_items(items))
}

/// A cursor over the file bytes. Every read is bounds-checked.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.bytes.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

/// What the sync last saw: the saved selections' ids and names, to tell when to write.
pub type Signature = Vec<(u64, String)>;

fn signature(store: &SavedSelections) -> Signature {
    store.list().iter().map(|s| (s.id, s.name.clone())).collect()
}

impl EskizApp {
    /// Once a frame: follow the open document. Its saved selections load with it, and they are
    /// written whenever the set changes. A document that has just received its first save keeps
    /// the selections it had.
    pub(crate) fn saved_selections_frame(&mut self) {
        let key = self.gal.current.as_ref().and_then(|c| c.id.clone());
        let dir = self.gal.library.as_ref().map(|l| l.root().to_path_buf());
        let assigned = self.gal.assigned_id.clone();
        let tool = &mut self.sel;
        if key != tool.store_key {
            // Only a document that has just been given its ID by its own first save keeps the
            // selections it had; any other document starts from its own file.
            let was_unsaved = tool.store_key.is_none() && key.is_some() && key == assigned;
            tool.store_key = key.clone();
            match (&key, &dir) {
                (Some(id), Some(d)) => {
                    let loaded = fs::read(file_for(d, id)).ok().and_then(|b| decode(&b));
                    if let Some(store) = loaded {
                        tool.saved = store;
                        tool.store_sig = signature(&tool.saved);
                    } else if !was_unsaved {
                        tool.saved = SavedSelections::default();
                        tool.store_sig = Vec::new();
                    }
                }
                _ => {
                    tool.saved = SavedSelections::default();
                    tool.store_sig = Vec::new();
                }
            }
        }
        let now = signature(&tool.saved);
        let mut failed = false;
        if now != tool.store_sig {
            if let (Some(id), Some(d)) = (key, dir) {
                // A failed write is not tried again every frame (it re-encodes every mask): the
                // next change to the set tries again.
                failed = crate::storage::write_atomic(&file_for(&d, &id), &encode(&tool.saved)).is_err();
                tool.store_sig = now;
            }
        }
        if failed {
            self.show_toast("Could not save the saved selections; the next change tries again", 3.0);
        }
    }
}

/// Headless check (`--selftest`): saved selections written to a library folder come back intact.
pub fn selftest() -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("eskiz-selections-selftest-{}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut store = SavedSelections::default();
    store.save("Sky", &Mask { width: 4, height: 3, data: vec![1.0; 12] });
    crate::storage::write_atomic(&file_for(&dir, "doc"), &encode(&store))?;
    let back = fs::read(file_for(&dir, "doc")).ok().and_then(|b| decode(&b)).ok_or("saved selections did not reload")?;
    let _ = fs::remove_dir_all(&dir);
    if back != store {
        return Err("saved selections changed on the way to disk".to_owned());
    }
    Ok("  saved selections: written beside the document and reloaded intact".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(w: usize, h: usize, v: f32) -> Mask {
        Mask {
            width: w,
            height: h,
            data: vec![v; w * h],
        }
    }

    #[test]
    fn saved_selections_round_trip_with_ids_and_names() {
        let mut store = SavedSelections::default();
        let a = store.save("Sky", &mask(3, 2, 1.0));
        let b = store.save("Leaf \u{1f343}", &mask(3, 2, 0.25));
        store.remove(a);
        let back = decode(&encode(&store)).expect("decodes");
        assert_eq!(back, store);
        assert_eq!(back.list()[0].id, b);
        assert_eq!(back.load(b), store.load(b));
        // New saves after a reload get fresh ids.
        let mut back = back;
        assert!(back.save("More", &mask(1, 1, 0.0)) > b);
    }

    #[test]
    fn damaged_files_are_refused() {
        let mut store = SavedSelections::default();
        store.save("A", &mask(2, 2, 1.0));
        let bytes = encode(&store);
        assert!(decode(&bytes[..bytes.len() - 3]).is_none());
        assert!(decode(b"not a selection file").is_none());
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(decode(&bad).is_none());
    }
}
