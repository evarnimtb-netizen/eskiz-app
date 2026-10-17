//! Atomic file writes: write a temporary file next to the target, then rename it over the target.

use std::fs::{self, File};
use std::path::Path;

use crate::error::{MediaError, Result};

/// Writes `path` through a temporary file in the same folder. On failure the temporary file is removed,
/// so the target is never left half-written.
pub(crate) fn write_atomic(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("export");
    let temp = path.with_file_name(format!(".{name}.{}.partial", std::process::id()));
    let outcome =
        write_temp(&temp, write).and_then(|()| fs::rename(&temp, path).map_err(MediaError::from));
    if outcome.is_err() {
        let _ = fs::remove_file(&temp);
    }
    outcome
}

fn write_temp(temp: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let mut file = File::create(temp)?;
    write(&mut file)?;
    file.sync_all()?;
    Ok(())
}
