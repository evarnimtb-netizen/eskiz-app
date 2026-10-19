//! One atomic file write for the whole crate: the bytes go to a uniquely named temporary file in
//! the same folder, are flushed, and are renamed over the target; then the folder is flushed.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary name beside `path` that no other writer (thread or process) uses at the same time.
fn temp_beside(path: &Path) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_string_lossy();
    // Long names are cut so the temporary name stays within the usual 255-byte limit.
    let short: String = name.chars().take(80).collect();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let unique = format!("{}-{}-{nanos:x}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    Ok(path.with_file_name(format!(".{short}.{unique}.tmp")))
}

/// Write `bytes` to `path` so that readers see the old file or the whole new one, never a mix.
/// The temporary file is removed on every error.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_beside(path)?;
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        rename_over(&tmp, path)
    })();
    match result {
        Ok(()) => {
            sync_parent(path);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Rename; on Windows a sync client or virus scanner may hold the target for a moment, so a
/// refused rename is tried a few more times.
fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(40 * attempt));
            }
            other => return other,
        }
    }
}

/// Flush the folder entry of `path` (Unix only; elsewhere it does nothing).
fn sync_parent(path: &Path) {
    #[cfg(unix)]
    {
        let dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let _ = fs::File::open(dir).and_then(|d| d.sync_all());
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_writers_never_leave_a_mixed_file() {
        let dir = std::env::temp_dir().join(format!("eskiz-fsutil-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.bin");
        let threads: Vec<_> = (0..6u8)
            .map(|n| {
                let target = target.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        write_atomic(&target, &vec![n; 200_000]).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let bytes = fs::read(&target).unwrap();
        assert_eq!(bytes.len(), 200_000);
        assert!(bytes.iter().all(|b| *b == bytes[0]), "the file is one writer's whole output");
        let leftovers = fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!("eskiz-fsutil-fail-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // The target is a folder, so the rename fails.
        let target = dir.join("taken");
        fs::create_dir_all(target.join("inner")).unwrap();
        assert!(write_atomic(&target, b"x").is_err());
        let leftovers = fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
