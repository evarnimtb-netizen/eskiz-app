//! Document storage for the Gallery.
//!
//! Every gallery document is a `.procreate` package in the per-user library folder, written
//! and read with `eskiz-format`. Next to `<id>.procreate` sit two small cache files:
//!
//! * `<id>.png`: the thumbnail, the flattened art scaled to fit within 512 px;
//! * `<id>.meta`: title and pixel size, so listing the library never opens a package.
//!
//! The file name is an ID and never changes. A rename only rewrites the `.meta` file.
//! Writes go to a temporary file in the same folder and are renamed into place, so a crash
//! never leaves a half-written document.
//!
//! Saving, loading, importing and exporting run on a [`Worker`] thread, so the UI never waits
//! on disk or compression. This module has no egui dependency.
//!
//! The only code that knows how an app [`Document`] maps onto a `.procreate` package is
//! [`to_builder`] (save) and [`crate::import::open_procreate`] (load). Port those two when the
//! document model changes.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use eskiz_core::blend::BlendMode;
use eskiz_format::{DocumentBuilder, GroupSpec, LayerSpec, LayerTiles, Node, TileSource};

use crate::document::Document;
use crate::export::{home_dir, write_png};
use crate::import::open_procreate;
use crate::stacks::LibraryIndex;

/// Longest side of a stored thumbnail, pixels.
pub const THUMB_MAX: usize = 512;
/// Seconds between autosaves while a document has unsaved changes.
pub const AUTOSAVE_SECS: f64 = 30.0;
/// Environment variable that overrides the library folder.
pub const LIBRARY_ENV: &str = "ESKIZ_LIBRARY_DIR";
/// Largest canvas side the Gallery offers, pixels.
pub const MAX_SIDE: usize = 8192;

const EXT: &str = "procreate";
/// The library index: root order, stacks and sort mode (see `stacks`).
pub const INDEX_FILE: &str = "library.json";
/// Extension of a 3D document folder in the library.
pub const MODEL_EXT: &str = "eskiz3d";

/// One document in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocEntry {
    pub id: String,
    pub title: String,
    pub width: usize,
    pub height: usize,
    /// Last write time of the package. A change means the thumbnail is stale.
    pub modified: SystemTime,
}

/// A decoded thumbnail, straight RGBA8, top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// The folder of gallery documents, and the folder of crash-recovery journals beside it.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
    recovery: PathBuf,
}

impl Library {
    /// A library in `root`. The folder is created on the first write. Its journals go in
    /// `root/recovery` (the library listing ignores that folder).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let recovery = root.join("recovery");
        Self { root, recovery }
    }

    /// The default library: `$ESKIZ_LIBRARY_DIR`, else the per-user app data folder, else `~/Eskiz`.
    /// Its journals go in `recovery` in the app data folder.
    pub fn default_location() -> Self {
        let recovery = DataPaths::under(app_data_dir()).recovery();
        Self { root: default_root(), recovery }
    }

    /// The library laid out under a host-given app data folder (Android's internal data path):
    /// `base/Documents` with journals in `base/recovery`. `$ESKIZ_LIBRARY_DIR` does not apply.
    pub fn in_data_dir(base: &Path) -> Self {
        let paths = DataPaths::under(base);
        Self { root: paths.library(), recovery: paths.recovery() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder of crash-recovery journals (one subfolder per document).
    pub fn recovery_root(&self) -> &Path {
        &self.recovery
    }

    /// Use another recovery folder (tests).
    pub fn with_recovery(mut self, dir: impl Into<PathBuf>) -> Self {
        self.recovery = dir.into();
        self
    }

    fn package(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.{EXT}"))
    }

    /// The folder of a 3D document.
    pub fn model_dir(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.{MODEL_EXT}"))
    }

    /// Where a 3D document's new folder is written before it is swapped in.
    pub(crate) fn model_tmp(&self, id: &str) -> PathBuf {
        self.root.join(format!(".{id}.{MODEL_EXT}.tmp"))
    }

    /// Where a 3D document's previous folder waits while a save swaps in the new one.
    pub(crate) fn model_bak(&self, id: &str) -> PathBuf {
        self.root.join(format!(".{id}.{MODEL_EXT}.bak"))
    }

    /// Whether `id` is a 3D document (a folder) rather than a `.procreate` package.
    pub fn is_model_doc(&self, id: &str) -> bool {
        self.model_dir(id).is_dir()
    }

    /// Write the gallery thumbnail and metadata of a 3D document. Its folder is written by
    /// `view3d::save_model_doc`.
    pub(crate) fn write_model_meta(&self, id: &str, title: &str, width: usize, height: usize, rgba: &[u8]) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let _ = self.write_thumbnail(id, rgba, width, height);
        write_atomic(&self.meta_file(id), format_meta(title, width, height).as_bytes())
    }

    fn thumb_file(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.png"))
    }

    fn meta_file(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.meta"))
    }

    /// Every document, newest first. Unreadable packages are skipped.
    pub fn list(&self) -> Vec<DocEntry> {
        self.scan().map(|(entries, _)| entries).unwrap_or_default()
    }

    /// Like `list`, but `None` when the library folder cannot be read at all, and with the IDs of
    /// packages that exist but could not be opened right now (locked by a sync client or virus
    /// scanner, say). Those must not be dropped from the library index.
    pub fn scan(&self) -> Option<(Vec<DocEntry>, Vec<String>)> {
        let Ok(dir) = fs::read_dir(&self.root) else {
            return None;
        };
        let mut unreadable = Vec::new();
        let mut out = Vec::new();
        let stale = SystemTime::now() - Duration::from_secs(3600);
        let mut backups = Vec::new();
        for item in dir.flatten() {
            let path = item.path();
            // Half-written temporary files left by a crash.
            if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.') && n.ends_with(".tmp")) {
                if item.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < stale) && item.metadata().is_ok_and(|m| m.is_file()) {
                    let _ = fs::remove_file(&path);
                }
                continue;
            }
            if let Some(id) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix('.')?.strip_suffix(".bak")?.strip_suffix(&format!(".{MODEL_EXT}"))) {
                backups.push(id.to_owned());
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str());
            if ext != Some(EXT) && ext != Some(MODEL_EXT) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                continue;
            };
            if id.starts_with('.') {
                continue;
            }
            let modified = item.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
            if let Some(entry) = self.entry(&id, modified) {
                out.push(entry);
            } else {
                unreadable.push(id);
            }
        }
        // A 3D save that was cut off can leave the document only under its backup name.
        for id in backups {
            crate::view3d::recover_model_dir(self, &id);
            if !out.iter().any(|e| e.id == id) {
                out.extend(self.get(&id));
            }
        }
        out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
        unreadable.retain(|id| !out.iter().any(|e| &e.id == id));
        unreadable.sort();
        unreadable.dedup();
        Some((out, unreadable))
    }

    /// One entry, read from the `.meta` cache or (for a package copied in by hand) the package.
    fn entry(&self, id: &str, modified: SystemTime) -> Option<DocEntry> {
        if let Some((title, width, height)) = fs::read_to_string(self.meta_file(id)).ok().as_deref().and_then(parse_meta) {
            return Some(DocEntry { id: id.to_owned(), title, width, height, modified });
        }
        let doc = eskiz_format::Document::open(self.package(id)).ok()?;
        let title = doc.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Untitled".to_owned());
        let (width, height) = (doc.width as usize, doc.height as usize);
        let _ = fs::write(self.meta_file(id), format_meta(&title, width, height));
        Some(DocEntry { id: id.to_owned(), title, width, height, modified })
    }

    /// The entry for `id`, if it exists.
    pub fn get(&self, id: &str) -> Option<DocEntry> {
        let modified = fs::metadata(self.package(id))
            .or_else(|_| fs::metadata(self.model_dir(id)))
            .and_then(|m| m.modified())
            .ok()?;
        self.entry(id, modified)
    }

    /// Write `doc` as `<id>.procreate`, refresh its thumbnail and metadata, and return the entry.
    pub fn save(&self, id: &str, title: &str, doc: &Document) -> Result<DocEntry, String> {
        check_id(id)?;
        fs::create_dir_all(&self.root).map_err(|e| format!("{}: {}", self.root.display(), friendly(&e)))?;
        let bytes = to_builder(doc, title).to_bytes().map_err(|e| e.to_string())?;
        self.write_package(id, &bytes, doc.width, doc.height)?;
        let (width, height) = (doc.width, doc.height);
        // The thumbnail and metadata are caches: a failure there must not fail the save.
        let flat = flatten_doc(doc);
        let _ = self.write_thumbnail(id, &flat, width, height);
        let _ = write_atomic(&self.meta_file(id), format_meta(title, width, height).as_bytes());
        self.get(id).ok_or_else(|| "saved document is missing".to_owned())
    }

    /// Replace the package of `id` with `bytes`, so that the old package stays intact unless the
    /// new one checks out. The bytes go to a temporary file, which must open again (header and
    /// archive) before it replaces the package. The old package is kept as `.bak` until the
    /// replacement has been checked, and is put back if the check fails.
    fn write_package(&self, id: &str, bytes: &[u8], width: usize, height: usize) -> Result<(), String> {
        let path = self.package(id);
        let tmp = self.root.join(format!(".{id}.{EXT}.tmp"));
        let bak = self.root.join(format!(".{id}.{EXT}.bak"));
        let staged = write_file(&tmp).and_then(|mut f| {
            use std::io::Write;
            f.write_all(bytes)?;
            f.sync_all()
        });
        let staged = staged.map_err(|e| format!("{}: {}", tmp.display(), friendly(&e))).and_then(|()| verify_package(&tmp, width, height));
        if let Err(e) = staged {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let had_old = path.exists();
        if had_old {
            // A hard link keeps the old package under the backup name without copying it (the
            // rename below replaces the directory entry, not the old file's contents). Where
            // links are not available, copy.
            let _ = fs::remove_file(&bak);
            if let Err(e) = fs::hard_link(&path, &bak).or_else(|_| fs::copy(&path, &bak).map(|_| ())) {
                let _ = fs::remove_file(&tmp);
                return Err(format!("{}: {}", bak.display(), friendly(&e)));
            }
        }
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            let _ = fs::remove_file(&bak);
            return Err(format!("{}: {}", path.display(), friendly(&e)));
        }
        sync_dir(&self.root);
        match verify_package(&path, width, height).and_then(|()| check_fault()) {
            Ok(()) => {
                let _ = fs::remove_file(&bak);
                Ok(())
            }
            Err(e) => {
                // Put the previous version back, or remove the file that did not check out.
                if had_old {
                    let _ = fs::rename(&bak, &path);
                } else {
                    let _ = fs::remove_file(&path);
                }
                Err(e)
            }
        }
    }

    fn write_thumbnail(&self, id: &str, rgba: &[u8], width: usize, height: usize) -> Result<(), String> {
        let t = make_thumbnail(rgba, width, height, THUMB_MAX);
        let tmp = self.root.join(format!(".{id}.png.tmp"));
        write_png(&tmp, t.width, t.height, &t.rgba)?;
        fs::rename(&tmp, self.thumb_file(id)).map_err(|e| e.to_string())
    }

    /// Read a document back.
    /// If the package does not open, the `.bak` copy left by an interrupted save is tried.
    pub fn load(&self, id: &str) -> Result<Document, String> {
        check_id(id)?;
        match open_procreate(&self.package(id)) {
            Ok(doc) => Ok(doc),
            Err(e) => {
                let bak = self.root.join(format!(".{id}.{EXT}.bak"));
                open_procreate(&bak).map_err(|_| e)
            }
        }
    }

    /// The cached thumbnail of `id`, if there is one.
    pub fn thumbnail(&self, id: &str) -> Option<Thumb> {
        let file = fs::File::open(self.thumb_file(id)).ok()?;
        let mut reader = png::Decoder::new(std::io::BufReader::new(file)).read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        buf.truncate(info.buffer_size());
        Some(Thumb { width: info.width as usize, height: info.height as usize, rgba: buf })
    }

    /// Copy a document to a new ID, titled "<title> copy". Returns the new entry.
    pub fn duplicate(&self, id: &str) -> Result<DocEntry, String> {
        let src = self.get(id).ok_or("document not found")?;
        let new_id = new_id();
        if self.is_model_doc(id) {
            return self.duplicate_model(id, &src, &new_id);
        }
        let tmp = self.root.join(format!(".{new_id}.tmp"));
        fs::copy(self.package(id), &tmp).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.package(&new_id)).map_err(|e| e.to_string())?;
        let _ = fs::copy(self.thumb_file(id), self.thumb_file(&new_id));
        let title = format!("{} copy", src.title);
        write_atomic(&self.meta_file(&new_id), format_meta(&title, src.width, src.height).as_bytes())?;
        self.get(&new_id).ok_or_else(|| "copy is missing".to_owned())
    }

    /// Copy a 3D document's folder to `new_id`, titled "<title> copy", with its thumbnail and metadata.
    fn duplicate_model(&self, id: &str, src: &DocEntry, new_id: &str) -> Result<DocEntry, String> {
        let tmp = self.root.join(format!(".{new_id}.tmp"));
        let _ = fs::remove_dir_all(&tmp);
        copy_dir(&self.model_dir(id), &tmp)?;
        let title = format!("{} copy", src.title);
        set_model_title(&tmp, &title)?;
        fs::rename(&tmp, self.model_dir(new_id)).map_err(|e| e.to_string())?;
        let _ = fs::copy(self.thumb_file(id), self.thumb_file(new_id));
        write_atomic(&self.meta_file(new_id), format_meta(&title, src.width, src.height).as_bytes())?;
        self.get(new_id).ok_or_else(|| "copy is missing".to_owned())
    }

    /// Remove a document and its cache files. Deleting a missing document is fine.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        if check_id(id).is_ok() {
            for dir in [self.model_dir(id), self.model_bak(id), self.model_tmp(id)] {
                let _ = fs::remove_dir_all(dir);
            }
        }
        check_id(id)?;
        for path in [self.package(id), self.thumb_file(id), self.meta_file(id)] {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Ok(())
    }

    /// Change the title shown in the gallery. The package name (the ID) stays.
    pub fn rename(&self, id: &str, title: &str) -> Result<(), String> {
        let entry = self.get(id).ok_or("document not found")?;
        let title = clean_title(title);
        write_atomic(&self.meta_file(id), format_meta(&title, entry.width, entry.height).as_bytes())?;
        if self.is_model_doc(id) {
            set_model_title(&self.model_dir(id), &title)?;
        }
        Ok(())
    }

    /// The documents, and the library index brought in line with them. A folder that cannot be
    /// read, or a package that cannot be opened right now, never removes a document from its
    /// stack or from the manual order.
    pub fn sync_index(&self) -> (Vec<DocEntry>, LibraryIndex) {
        let mut index = self.read_index();
        let Some((entries, unreadable)) = self.scan() else {
            return (Vec::new(), index);
        };
        let mut docs: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
        docs.extend(unreadable.into_iter().filter(|id| index.contains_doc(id)));
        if index.reconcile(&docs) {
            let _ = self.write_index(&index);
        }
        (entries, index)
    }

    /// The library index. An empty one when the file is missing or unreadable.
    pub fn read_index(&self) -> LibraryIndex {
        let main = self.root.join(INDEX_FILE);
        let read = |p: &Path| fs::read_to_string(p).ok().and_then(|t| LibraryIndex::decode(&t));
        if let Some(ix) = read(&main) {
            return ix;
        }
        let backup = self.root.join(format!("{INDEX_FILE}.bak"));
        if main.exists() {
            // A damaged index: keep it aside and fall back to the last good copy.
            let _ = fs::rename(&main, self.root.join(format!("{INDEX_FILE}.damaged")));
        }
        read(&backup).unwrap_or_default()
    }

    /// Write the library index atomically.
    pub fn write_index(&self, index: &LibraryIndex) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let main = self.root.join(INDEX_FILE);
        if fs::read_to_string(&main).ok().is_some_and(|t| LibraryIndex::decode(&t).is_some()) {
            let _ = fs::copy(&main, self.root.join(format!("{INDEX_FILE}.bak")));
        }
        write_atomic(&main, index.encode().as_bytes())
    }

    /// Copy a `.procreate` file into the library under a new ID, after checking that it opens.
    pub fn import(&self, src: &Path) -> Result<DocEntry, String> {
        let doc = open_procreate(src)?;
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported");
        let title = eskiz_format::Document::open(src)
            .ok()
            .and_then(|d| d.name)
            .map(|n| clean_title(&n))
            .filter(|n| !n.is_empty() && n != "Untitled")
            .unwrap_or_else(|| clean_title(stem));
        let id = new_id();
        fs::create_dir_all(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let bytes = fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
        write_atomic(&self.package(&id), &bytes)?;
        let (width, height) = (doc.width, doc.height);
        let flat = flatten_doc(&doc);
        let _ = self.write_thumbnail(&id, &flat, width, height);
        write_atomic(&self.meta_file(&id), format_meta(&title, width, height).as_bytes())?;
        self.get(&id).ok_or_else(|| "imported document is missing".to_owned())
    }

    /// Export the flattened art of `id` as a PNG into `dir`. Returns the file written.
    pub fn export_png(&self, id: &str, dir: &Path) -> Result<PathBuf, String> {
        let entry = self.get(id).ok_or("document not found")?;
        let doc = self.load(id)?;
        let flat = flatten_doc(&doc);
        let stem = file_safe(&entry.title);
        let mut path = dir.join(format!("{stem}.png"));
        let mut n = 2;
        while path.exists() {
            path = dir.join(format!("{stem} {n}.png"));
            n += 1;
        }
        write_png(&path, doc.width, doc.height, &flat)?;
        Ok(path)
    }
}

/// Imported fonts: `$ESKIZ_FONT_DIR`, else `<user data dir>/Eskiz/Fonts`, else `~/Eskiz/Fonts`.
/// Every `.ttf`, `.otf` and `.ttc` in it is loaded at start-up (see `text_tool`).
pub fn fonts_dir() -> PathBuf {
    if let Some(dir) = crate::compat::env_os("ESKIZ_FONT_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(base) = data_root() {
        return DataPaths::under(base).fonts();
    }
    home_fallback().join("Fonts")
}

/// The user's own brushes, saved by the Brush Bench: the `My Brushes` folder inside
/// [`brushes_dir`]. The importer reads only the files directly in `brushes_dir`, so the two sets
/// never mix.
pub fn user_brushes_dir() -> PathBuf {
    brushes_dir().join("My Brushes")
}

/// Imported brushes: `$ESKIZ_BRUSH_DIR`, else `<user data dir>/Eskiz/Brushes`, else `~/Eskiz/Brushes`.
pub fn brushes_dir() -> PathBuf {
    if let Some(dir) = crate::compat::env_os("ESKIZ_BRUSH_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(base) = data_root() {
        return DataPaths::under(base).brushes();
    }
    home_fallback().join("Brushes")
}

/// The app data folder given by the host (Android's internal data path), set once at start-up.
static APP_DATA_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Make `dir` the app data folder (the parent of the library, recovery, fonts and brushes).
/// Android calls this with `AndroidApp::internal_data_path()` because `directories` finds no
/// home folder there. The first call wins; later calls are ignored.
pub fn set_app_data_dir(dir: impl Into<PathBuf>) {
    let _ = APP_DATA_DIR.set(dir.into());
}

/// The per-user data folder: the host override, else the platform folder from `directories`.
fn data_root() -> Option<PathBuf> {
    if let Some(dir) = APP_DATA_DIR.get() {
        return Some(dir.clone());
    }
    static ROOT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let new = directories::ProjectDirs::from("", "", "Eskiz").map(|d| d.data_dir().to_owned())?;
        // Data written by releases under the old name moves over on first use.
        Some(match crate::compat::legacy_data_dir() {
            Some(old) => crate::compat::migrate_dir(new, &old),
            None => new,
        })
    })
    .clone()
}

/// The per-user app data folder (after any migration from the old name).
pub fn app_data_root() -> Option<PathBuf> {
    data_root()
}

/// Moves a data folder left by an older release to the current name, before anything reads it.
/// Call once at start-up, ahead of eframe's own storage (which lives in the same folder).
pub fn migrate_legacy_data() {
    let _ = data_root();
}

/// `~/Eskiz`, the folder used when the platform has no data folder; the old `~/Eskiz` moves there.
fn home_fallback() -> PathBuf {
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    crate::compat::migrate_dir(home.join("Eskiz"), &home.join(crate::compat::LEGACY_HOME_DIR))
}

/// The folders under one app data folder. Desktop and Android use the same layout; Android's
/// base is the internal data path, which survives updates and is private to the app.
pub struct DataPaths {
    base: PathBuf,
}

impl DataPaths {
    pub fn under(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// Where saved documents live.
    pub fn library(&self) -> PathBuf {
        self.base.join("Documents")
    }

    /// Crash-recovery journals.
    pub fn recovery(&self) -> PathBuf {
        self.base.join("recovery")
    }

    /// Imported fonts.
    pub fn fonts(&self) -> PathBuf {
        self.base.join("Fonts")
    }

    /// Imported and saved brushes.
    pub fn brushes(&self) -> PathBuf {
        self.base.join("Brushes")
    }

    /// eframe's preferences file (panel layout, colours, settings). Android has no default
    /// location for it, so the shell passes this as `persistence_path`.
    pub fn prefs_file(&self) -> PathBuf {
        self.base.join("eframe-storage.ron")
    }
}

/// `$ESKIZ_LIBRARY_DIR`, else `<user data dir>/Eskiz/Documents`, else `~/Eskiz`.
fn default_root() -> PathBuf {
    if let Some(dir) = crate::compat::env_os(LIBRARY_ENV).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(base) = data_root() {
        return DataPaths::under(base).library();
    }
    home_fallback()
}

/// The per-user app data folder (the parent of the default library and of `recovery/`).
fn app_data_dir() -> PathBuf {
    if let Some(base) = data_root() {
        return base;
    }
    home_fallback()
}

// ---------------------------------------------------------------- Document <-> .procreate

/// The one place that turns an app [`Document`] into a `.procreate` description.
///
/// The app stores layers bottom to top and the package stores its tree top-most first, so the
/// order is reversed. Pixels are premultiplied RGBA8 in both. Hidden layers stay in the file.
/// Set the title in a 3D document's `document.json`, so the folder agrees with the gallery.
fn set_model_title(dir: &Path, title: &str) -> Result<(), String> {
    let path = dir.join("document.json");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    json["title"] = serde_json::Value::String(title.to_owned());
    write_atomic(&path, json.to_string().as_bytes())
}

/// Copy a folder tree, file by file.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

pub fn to_builder(doc: &Document, title: &str) -> DocumentBuilder {
    let mut b = DocumentBuilder::new(doc.width as u32, doc.height as u32);
    b.name = clean_title(title);
    let [r, g, bl] = doc.background.map(|v| v as f32);
    b.background_color = [r, g, bl, 1.0];
    b.background_hidden = !doc.background_visible;
    b.dpi = doc.dpi;
    b.color_profile = Some(doc.colour_space.profile_name().to_owned());
    b.bits_per_channel = doc.depth.bits();
    b.tree = builder_nodes(doc, &doc.root);
    let mut animation = crate::animation_ui::spec_from_settings(&doc.animation);
    animation.last_item_is_background = doc.root.first().is_some_and(|n| n.props.anim_background);
    animation.first_item_is_foreground = doc.root.last().is_some_and(|n| n.props.anim_foreground);
    b.animation = animation;
    b
}

/// A layer's tiles for the package writer: the allocated tiles are read in place.
struct LayerTileSource(eskiz_core::Layer);

impl TileSource for LayerTileSource {
    fn edge(&self) -> usize {
        eskiz_core::canvas::TILE
    }

    fn bits(&self) -> u32 {
        self.0.depth().bits()
    }

    fn tile8(&self, col: usize, row: usize) -> Option<&[u8]> {
        match &**self.0.tile(col, row)? {
            eskiz_core::canvas::Tile::U8(d) => Some(d),
            eskiz_core::canvas::Tile::U16(_) => None,
        }
    }

    fn tile16(&self, col: usize, row: usize) -> Option<&[u16]> {
        match &**self.0.tile(col, row)? {
            eskiz_core::canvas::Tile::U16(d) => Some(d),
            eskiz_core::canvas::Tile::U8(_) => None,
        }
    }
}

/// Convert items (bottom to top) to package nodes (top-most first). A group's blend mode and
/// mask have no place in the package, so they are not saved.
fn builder_nodes(doc: &Document, items: &[eskiz_core::Node]) -> Vec<Node> {
    items
        .iter()
        .rev()
        .map(|n| match &n.kind {
            eskiz_core::NodeKind::Layer => Node::Layer(LayerSpec {
                name: n.props.name.clone(),
                opacity: n.props.opacity,
                blend_mode: blend_index(n.props.blend),
                hidden: !n.props.visible,
                locked: n.props.locked,
                clipped: n.props.clipped,
                alpha_locked: n.props.alpha_lock,
                private: n.props.private,
                animation_hold: n.props.hold,
                // The package stores tiles, and a blank layer has none: pass nothing rather than a
                // canvas-sized buffer of zeros. Other layers hand over their allocated tiles (a
                // cheap clone that shares them), which the writer reads in place.
                tiles: doc
                    .layer(n.id)
                    .filter(|l| !l.is_blank() && (l.width, l.height) == (doc.width, doc.height))
                    .map(|l| LayerTiles::new(LayerTileSource(l.clone()))),
                text: doc.texts.get(&n.id).map(crate::text_tool::to_spec),
                mask: n.mask.as_ref().filter(|m| m.data.len() == doc.width * doc.height).map(|m| m.data.clone()),
                ..LayerSpec::default()
            }),
            eskiz_core::NodeKind::Group { children, collapsed } => Node::Group(GroupSpec {
                name: n.props.name.clone(),
                opacity: n.props.opacity,
                hidden: !n.props.visible,
                collapsed: *collapsed,
                locked: n.props.locked,
                private: n.props.private,
                animation_hold: n.props.hold,
                children: builder_nodes(doc, children),
            }),
        })
        .collect()
}

/// The document composited for thumbnails and PNG export: private items left out.
/// The flattened picture as the gallery shows it: mapped from the document's colour space to the
/// screen's, since thumbnails are drawn like any other interface image.
fn flatten_doc(doc: &Document) -> Vec<u8> {
    let mut px = eskiz_core::composite(
        doc,
        eskiz_core::CompositeOpts {
            skip_private: true,
            background: true,
        },
    );
    crate::colour_management::apply_display_rgba8(doc.colour_space, &mut px);
    px
}

/// The `.procreate` blend index for a mode (the inverse of `BlendMode::from_index`).
pub(crate) fn blend_index(mode: BlendMode) -> i64 {
    (0..=34).find(|&i| BlendMode::from_index(i) == Some(mode)).unwrap_or(0)
}

/// A hash of everything about a document that is not pixels: layer count, names, blend modes,
/// visibility and opacity. A change means unsaved edits even when no history step was made.
pub fn meta_signature(doc: &Document) -> u64 {
    fn walk(items: &[eskiz_core::Node], h: &mut DefaultHasher) {
        for n in items {
            let p = &n.props;
            (n.id, &p.name, p.blend, p.visible, p.opacity.to_bits()).hash(h);
            (p.clipped, p.alpha_lock, p.locked, p.private, n.mask.is_some(), n.is_group()).hash(h);
            walk(n.children(), h);
        }
    }
    let mut h = DefaultHasher::new();
    (doc.width, doc.height, doc.layer_count()).hash(&mut h);
    (doc.background.map(f64::to_bits), doc.background_visible, doc.colour_space).hash(&mut h);
    walk(&doc.root, &mut h);
    h.finish()
}

// ---------------------------------------------------------------- thumbnails

/// Scale `rgba` (`width * height`, straight RGBA8) to fit within `max` pixels on the long
/// side, with a box filter. Never enlarges. Each side is at least 2 px (or the source side).
pub fn make_thumbnail(rgba: &[u8], width: usize, height: usize, max: usize) -> Thumb {
    let scale = (max as f64 / width.max(height).max(1) as f64).min(1.0);
    let tw = ((width as f64 * scale).round() as usize).clamp(2.min(width), width.max(1));
    let th = ((height as f64 * scale).round() as usize).clamp(2.min(height), height.max(1));
    let mut out = vec![0u8; tw * th * 4];
    if tw == 0 || th == 0 {
        return Thumb { width: tw, height: th, rgba: out };
    }
    // Output rows are independent: each is filled from its own band of source rows.
    use rayon::prelude::*;
    out.par_chunks_mut(tw * 4).enumerate().for_each(|(ty, out_row)| {
        let y0 = ty * height / th;
        let y1 = ((ty + 1) * height / th).max(y0 + 1).min(height);
        for tx in 0..tw {
            let x0 = tx * width / tw;
            let x1 = ((tx + 1) * width / tw).max(x0 + 1).min(width);
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for px in rgba[(y * width + x0) * 4..(y * width + x1) * 4].as_chunks::<4>().0 {
                    for (s, v) in sum.iter_mut().zip(px) {
                        *s += u32::from(*v);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = tx * 4;
            for c in 0..4 {
                out_row[o + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    });
    Thumb { width: tw, height: th, rgba: out }
}

// ---------------------------------------------------------------- small helpers

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh document ID: time plus a process-wide counter, so IDs never repeat.
pub fn new_id() -> String {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or(Duration::ZERO).as_nanos();
    format!("{:x}-{:03x}", nanos, COUNTER.fetch_add(1, Ordering::Relaxed) & 0xfff)
}

fn check_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!("bad document id '{id}'"))
    }
}

/// Titles are one line, trimmed and at most 80 characters. Empty becomes "Untitled".
pub fn clean_title(title: &str) -> String {
    let one: String = title.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let t: String = one.trim().chars().take(80).collect();
    if t.is_empty() {
        "Untitled".to_owned()
    } else {
        t
    }
}

fn file_safe(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let s = s.trim().trim_matches('.').to_owned();
    if s.is_empty() {
        "Eskiz".to_owned()
    } else {
        avoid_device_name(&s)
    }
}

/// Windows cannot create files named after its devices (`CON`, `NUL`, `COM1`, ..., with or
/// without an extension): such a name gets a leading underscore.
pub(crate) fn avoid_device_name(name: &str) -> String {
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| stem.strip_prefix(p).is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")));
    if reserved {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

fn format_meta(title: &str, width: usize, height: usize) -> String {
    format!("{width}\n{height}\n{}\n", clean_title(title))
}

fn parse_meta(text: &str) -> Option<(String, usize, usize)> {
    let mut lines = text.lines();
    let width = lines.next()?.trim().parse().ok()?;
    let height = lines.next()?.trim().parse().ok()?;
    let title = lines.next()?.to_owned();
    (width > 0 && height > 0).then(|| (clean_title(&title), width, height))
}

/// Write to a temporary file in the same folder, then rename over the target.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic_nodir(path, bytes)?;
    if let Some(parent) = path.parent() {
        sync_dir(parent);
    }
    Ok(())
}

/// [`write_atomic`] without flushing the folder (for batches that flush it once afterwards).
pub(crate) fn write_atomic_nodir(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", COUNTER.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        use std::io::Write;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("{}: {}", path.display(), friendly(&e))
    })
}

/// A short reason for a file error, for toasts: disk full and permission errors get plain words.
pub(crate) fn friendly(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::StorageFull => "the disk is full".to_owned(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
        _ => e.to_string(),
    }
}

/// Flush a folder's entries (new or renamed files in it) to disk. Only Unix-like systems can open
/// a folder for this; elsewhere, and where the file system refuses, it does nothing.
pub(crate) fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    let _ = fs::File::open(dir).and_then(|f| f.sync_all());
    #[cfg(not(unix))]
    let _ = dir;
}

/// Create a file for writing. Test builds can make the next create fail.
pub(crate) fn write_file(path: &Path) -> std::io::Result<fs::File> {
    #[cfg(test)]
    if let Some(kind) = FAIL_NEXT_WRITE.with(|c| c.take()) {
        return Err(std::io::Error::new(kind, "injected write failure"));
    }
    #[cfg(test)]
    if FAIL_WRITES_UNDER.lock().is_ok_and(|v| v.iter().any(|d| path.starts_with(d))) {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected write failure"));
    }
    fs::File::create(path)
}

/// Open a saved package again and check its header and archive. The size must match the document.
fn verify_package(path: &Path, width: usize, height: usize) -> Result<(), String> {
    let doc = eskiz_format::Document::open(path).map_err(|e| format!("{}: the saved file does not open: {e}", path.display()))?;
    if (doc.width as usize, doc.height as usize) != (width, height) {
        return Err(format!("{}: the saved file has the wrong size", path.display()));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn check_fault() -> Result<(), String> {
    if FAIL_NEXT_CHECK.with(|c| c.replace(false)) {
        return Err("injected check failure".to_owned());
    }
    Ok(())
}

#[cfg(not(test))]
pub(crate) fn check_fault() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Test hook: the next package (or 3D document) file create fails with this kind.
    pub(crate) static FAIL_NEXT_WRITE: std::cell::Cell<Option<std::io::ErrorKind>> = const { std::cell::Cell::new(None) };
    /// Test hook: the check of the next replaced package (or 3D document folder) fails.
    pub(crate) static FAIL_NEXT_CHECK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
/// Test hook for app-level tests, where saves run on the worker thread: every package file
/// created under one of these folders fails until the folder is removed from the list.
pub(crate) static FAIL_WRITES_UNDER: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

// ---------------------------------------------------------------- autosave timing

/// Decides when the autosave timer fires. Pure: feed it the clock, it says "save now".
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AutosaveClock {
    /// When the current countdown started: the document became dirty, or the last attempt.
    since: Option<f64>,
}

impl AutosaveClock {
    /// Call once per frame. Returns true when a save should start now.
    ///
    /// While the document is clean nothing runs. When it turns dirty the countdown starts, and
    /// `interval` seconds later (once no save is in flight) it fires and restarts, so a failed
    /// write is retried after another interval.
    pub fn update(&mut self, dirty: bool, busy: bool, now: f64, interval: f64) -> bool {
        if !dirty {
            self.since = None;
            return false;
        }
        let start = *self.since.get_or_insert(now);
        if now - start >= interval && !busy {
            self.since = Some(now);
            return true;
        }
        false
    }

    /// Seconds until the next possible fire, or `None` while clean.
    pub fn remaining(&self, now: f64, interval: f64) -> Option<f64> {
        self.since.map(|s| (interval - (now - s)).max(0.0))
    }
}

// ---------------------------------------------------------------- background worker

/// A document snapshot to write.
pub struct SaveRequest {
    pub id: String,
    pub title: String,
    pub doc: Box<Document>,
    /// The session edit counter and metadata signature the snapshot was taken at.
    pub edits: u64,
    pub sig: u64,
}

pub enum Job {
    Save(SaveRequest),
    Load(String),
    Import(PathBuf),
    Export { id: String, dir: PathBuf },
    /// Write a planned crash-recovery snapshot (see `journal`).
    Journal(crate::journal::Plan),
    /// Remove a document's journal (its package has been saved, or the user discarded it).
    RemoveJournal(String),
    /// Rebuild a document from its journal and package.
    Restore { id: String, title: String },
    /// Answered when every job queued before it has finished (the lifecycle suspend waits
    /// on it before writing the journal itself).
    Barrier(std::sync::mpsc::Sender<()>),
}

/// Run one job body; a panic inside it becomes an error for that job, so the worker thread
/// (and with it saving and crash recovery) keeps running.
fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let what = p
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(format!("internal error{}{what}", if what.is_empty() { "" } else { ": " }))
        }
    }
}

pub enum Done {
    /// `doc` is the snapshot that was saved, handed back to the UI (for the journal).
    Saved { id: String, edits: u64, sig: u64, result: Result<DocEntry, String>, doc: Box<Document> },
    Loaded { id: String, result: Result<Box<Document>, String> },
    Imported(Result<DocEntry, String>),
    Exported(Result<PathBuf, String>),
    Journal { id: String, result: Result<(), String> },
    Restored { id: String, title: String, result: Result<Box<Document>, String>, skipped: usize },
}

/// One background thread that runs jobs in order, so two saves of a document can never
/// overtake each other.
pub struct Worker {
    tx: Option<Sender<Job>>,
    pub rx: Receiver<Done>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// Start the thread. `notify` is called after every finished job (to wake the UI).
    pub fn spawn(library: Library, notify: impl Fn() + Send + 'static) -> Self {
        let (tx, jobs) = channel::<Job>();
        let (done_tx, rx) = channel::<Done>();
        let handle = std::thread::Builder::new()
            .name("eskiz-storage".into())
            .spawn(move || {
                // Set when a journal write failed: later snapshots would build on it, so they
                // fail too until a snapshot that starts from the base succeeds.
                let mut journal_failed = false;
                for job in jobs {
                    let done = match job {
                        Job::Save(r) => {
                            let result = guarded(|| library.save(&r.id, &r.title, &r.doc));
                            crate::exit::save_done();
                            Done::Saved { result, id: r.id, edits: r.edits, sig: r.sig, doc: r.doc }
                        }
                        Job::Load(id) => Done::Loaded { result: guarded(|| library.load(&id)).map(Box::new), id },
                        Job::Import(path) => Done::Imported(guarded(|| library.import(&path))),
                        Job::Export { id, dir } => Done::Exported(guarded(|| library.export_png(&id, &dir))),
                        Job::Journal(plan) => {
                            let result = if journal_failed && !plan.reset {
                                Err("an earlier recovery write failed".to_owned())
                            } else {
                                guarded(|| crate::journal::execute(&plan))
                            };
                            journal_failed = result.is_err();
                            Done::Journal { id: plan.id, result }
                        }
                        Job::RemoveJournal(id) => {
                            journal_failed = false;
                            let _ = guarded(|| crate::journal::remove(library.recovery_root(), &id));
                            continue;
                        }
                        Job::Restore { id, title } => {
                            let (result, skipped) = match guarded(|| crate::journal::restore_counted(library.recovery_root(), &library, &id)) {
                                Ok((doc, n)) => (Ok(Box::new(doc)), n),
                                Err(e) => (Err(e), 0),
                            };
                            Done::Restored { id, title, result, skipped }
                        }
                        Job::Barrier(reply) => {
                            let _ = reply.send(());
                            continue;
                        }
                    };
                    if done_tx.send(done).is_err() {
                        break;
                    }
                    notify();
                }
            })
            .ok();
        Self { tx: Some(tx), rx, handle }
    }

    /// Queue a job. Returns false when the worker thread is gone (the job was not queued).
    pub fn send(&self, job: Job) -> bool {
        if let Some(tx) = &self.tx {
            let is_save = matches!(job, Job::Save(_));
            if is_save {
                crate::exit::save_queued();
            }
            if tx.send(job).is_err() {
                if is_save {
                    crate::exit::save_done();
                }
                return false;
            }
        }
        true
    }

    /// Finish every queued job, waiting at most `budget`. Jobs that are still running after that
    /// are left to the thread (it stops with the process); returns whether everything finished.
    pub fn finish_within(&mut self, budget: std::time::Duration) -> bool {
        let Some(tx) = self.tx.take() else {
            return true;
        };
        let (reply, answer) = channel::<()>();
        if tx.send(Job::Barrier(reply)).is_err() {
            return true;
        }
        drop(tx);
        if answer.recv_timeout(budget).is_ok() {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            true
        } else {
            // Detach: dropping the handle does not wait.
            self.handle = None;
            false
        }
    }

    /// Finish every queued job, then stop the thread.
    pub fn finish(&mut self) {
        self.tx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.finish();
    }
}

// ---------------------------------------------------------------- selftest

/// `--selftest` check: save a two-layer document into a temporary library, list it, load it
/// back and compare pixels, rename, duplicate, delete.
pub fn selftest() -> Result<Vec<String>, String> {
    let dir = std::env::temp_dir().join(format!("eskiz-selftest-library-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let result = selftest_in(&dir);
    let _ = fs::remove_dir_all(&dir);
    result
}

fn selftest_in(dir: &Path) -> Result<Vec<String>, String> {
    let lib = Library::at(dir);
    let doc = sample_document(300, 200);
    let id = new_id();
    let entry = lib.save(&id, "Selftest", &doc)?;
    if (entry.width, entry.height, entry.title.as_str()) != (300, 200, "Selftest") {
        return Err(format!("gallery: bad entry {entry:?}"));
    }
    if lib.list().len() != 1 {
        return Err("gallery: saved document is not listed".into());
    }
    let back = lib.load(&id)?;
    let same_pixels = back.layer_count() == doc.layer_count()
        && (0..doc.layer_count()).all(|i| back.layer_at(i) == doc.layer_at(i));
    if !same_pixels || meta_signature(&back) != meta_signature(&doc) {
        return Err("gallery: document changed in the save/load round trip".into());
    }
    let thumb = lib.thumbnail(&id).ok_or("gallery: no thumbnail")?;
    if thumb.width != 300 || thumb.height != 200 {
        return Err(format!("gallery: thumbnail is {}x{}", thumb.width, thumb.height));
    }
    lib.rename(&id, "Renamed")?;
    let copy = lib.duplicate(&id)?;
    if lib.list().len() != 2 || lib.get(&copy.id).map(|e| e.title) != Some("Renamed copy".to_owned()) {
        return Err("gallery: duplicate failed".into());
    }
    lib.delete(&id)?;
    lib.delete(&copy.id)?;
    if !lib.list().is_empty() {
        return Err("gallery: delete left documents behind".into());
    }
    Ok(vec!["  gallery: save -> list -> load round trip (pixels, blend modes, thumbnail), rename, duplicate, delete: ok".to_owned()])
}

/// A small two-layer document with a gradient and a translucent Multiply layer.
pub fn sample_document(w: usize, h: usize) -> Document {
    let mut doc = Document::new(w, h);
    doc.add_layer_above(None);
    for i in 0..doc.layer_count() {
        let layer = doc.layer_at_mut(i).unwrap();
        for y in 0..h {
            for x in 0..w {
                let on = if i == 0 { x % 40 < 20 } else { (x + y) % 50 < 25 };
                if on {
                    let a = 200u8;
                    let c = [(x * 255 / w) as u8, (y * 255 / h) as u8, 90 + 60 * i as u8];
                    let p = |v: u8| (u16::from(v) * u16::from(a) / 255) as u8;
                    layer.set_pixel(x, y, [p(c[0]), p(c[1]), p(c[2]), a]);
                }
            }
        }
    }
    let p = doc.props_at_mut(1).unwrap();
    p.opacity = 0.75;
    p.blend = BlendMode::Multiply;
    p.name = "Shade".to_owned();
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("eskiz-storage-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Android: everything lives under the internal data path the shell hands over. Here a temp
    /// folder stands in for `/data/user/0/app.eskiz/files`.
    #[test]
    fn data_dir_override_holds_library_recovery_fonts_brushes_and_prefs() {
        let base = temp("android-data");
        let paths = DataPaths::under(&base);
        for p in [paths.library(), paths.recovery(), paths.fonts(), paths.brushes(), paths.prefs_file()] {
            assert!(p.starts_with(&base), "{} is under the data folder", p.display());
        }
        assert_eq!(paths.prefs_file(), base.join("eframe-storage.ron"));

        let lib = Library::in_data_dir(&base);
        assert_eq!(lib.root(), base.join("Documents"));
        assert_eq!(lib.recovery_root(), base.join("recovery"));
        let doc = sample_document(40, 30);
        let id = new_id();
        lib.save(&id, "Phone sketch", &doc).unwrap();
        assert!(base.join("Documents").is_dir());
        assert_eq!(lib.list().len(), 1);

        // A recovery journal written for the document is found again by a fresh Library value
        // built from the same folder (a restart after the process was killed).
        crate::journal::write_now(lib.recovery_root(), &id, "Phone sketch", &doc).unwrap();
        let again = Library::in_data_dir(&base);
        let pending = crate::journal::scan(again.recovery_root(), &again);
        assert_eq!(pending.len(), 1, "the journal under the data folder is offered for recovery");
        assert!(base.join("recovery").is_dir());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn save_list_load_gives_the_same_pixels() {
        let dir = temp("roundtrip");
        let lib = Library::at(&dir);
        let doc = sample_document(120, 90);
        let id = new_id();
        lib.save(&id, "Sunset", &doc).unwrap();

        let list = lib.list();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].title.as_str(), list[0].width, list[0].height), ("Sunset", 120, 90));
        assert_eq!(list[0].id, id);

        let back = lib.load(&id).unwrap();
        assert_eq!(back.layer_count(), 2);
        for i in 0..2 {
            assert_eq!(back.layer_at(i).unwrap().to_rgba(), doc.layer_at(i).unwrap().to_rgba());
            let (a, b) = (&back.node(back.layer_id(i).unwrap()).unwrap().props, &doc.node(doc.layer_id(i).unwrap()).unwrap().props);
            assert!((a.opacity - b.opacity).abs() < 1e-6);
            assert_eq!((a.visible, a.blend, &a.name), (b.visible, b.blend, &b.name));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// The package of a random document, written from its tiles, holds exactly the pixels and
    /// the tiles of the document, and the tile members agree with a dense `to_rgba` writer.
    #[test]
    fn tile_save_round_trips_random_documents() {
        use eskiz_core::{Depth, Layer};
        let dir = temp("tile-roundtrip");
        let lib = Library::at(&dir);
        let mut state = 0xC0FFEEu64;
        let mut next = move || {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        for case in 0..24u64 {
            let (w, h) = match case % 4 {
                0 => (1 + (next() % 20) as usize, 1 + (next() % 20) as usize),
                1 => (256, 257),
                _ => (1 + (next() % 700) as usize, 1 + (next() % 600) as usize),
            };
            let depth = if case % 3 == 2 { Depth::U16 } else { Depth::U8 };
            let mut doc = Document::empty_depth(w, h, depth);
            let mut parent = None;
            for i in 0..(1 + next() % 4) {
                let mut layer = Layer::new_depth(w, h, depth);
                for _ in 0..(next() % 4) {
                    let (x, y) = ((next() as usize) % w, (next() as usize) % h);
                    let (pw, ph) = (1 + (next() as usize) % (w - x).min(300), 1 + (next() as usize) % (h - y).min(300));
                    let rect: Vec<u8> = (0..pw * ph * 4).map(|_| next() as u8).collect();
                    layer.write_rect(x, y, pw, ph, &rect);
                }
                if next() % 4 == 0 {
                    layer.tile_mut(0, 0); // an allocated tile that holds only zeros: not stored
                }
                if next() % 5 == 0 && parent.is_none() {
                    parent = Some(doc.push_group(None, eskiz_core::Props::new("Folder"), None));
                }
                doc.push_raster(parent, eskiz_core::Props::new(format!("L{i}")), None, layer);
                if next() % 3 == 0 {
                    parent = None;
                }
            }
            let id = format!("case-{case}");
            lib.save(&id, "T", &doc).unwrap();
            let back = lib.load(&id).unwrap();
            assert_eq!((back.width, back.height, back.depth), (doc.width, doc.height, doc.depth), "case {case}");
            assert_eq!(back.layer_count(), doc.layer_count(), "case {case}");
            for i in 0..doc.layer_count() {
                let (a, b) = (back.layer_at(i).unwrap(), doc.layer_at(i).unwrap());
                assert!(a == b, "case {case} layer {i}: pixels");
                // allocated tiles: the stored ones are the non-zero tiles of the original
                for ty in 0..b.tiles_y() {
                    for tx in 0..b.tiles_x() {
                        let stored = b.tile(tx, ty).is_some_and(|t| !t.is_blank());
                        assert_eq!(a.tile(tx, ty).is_some(), stored, "case {case} layer {i} tile {tx},{ty}");
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// Timing of the phases of `Library::save` on a 4096 x 4096 document of 20 sparse layers.
    /// Run with `cargo test --release -p eskiz-app --lib save_phase_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing probe, not a check"]
    fn save_phase_timing() {
        use std::time::Instant;
        let n = 4096usize;
        let mut doc = Document::new(n, n);
        for i in 0..20usize {
            let id = if i == 0 { doc.layer_id(0).unwrap() } else { doc.add_layer_above(doc.layer_id(i - 1)) };
            let layer = doc.layer_mut(id).unwrap();
            let rect: Vec<u8> = (0..600 * 600 * 4).map(|k| ((k * 31 + i * 7) % 253) as u8).collect();
            layer.write_rect((i * 150) % 3000, (i * 97) % 3000, 600, 600, &rect);
        }
        let dir = temp("save-timing");
        let lib = Library::at(&dir);
        let t = Instant::now();
        let bytes = to_builder(&doc, "T").to_bytes().unwrap();
        println!("to_bytes {:?} ({} bytes)", t.elapsed(), bytes.len());
        let t = Instant::now();
        fs::create_dir_all(&dir).unwrap();
        lib.write_package("a", &bytes, n, n).unwrap();
        println!("write_package (new) {:?}", t.elapsed());
        let t = Instant::now();
        lib.write_package("a", &bytes, n, n).unwrap();
        println!("write_package (replace) {:?}", t.elapsed());
        let t = Instant::now();
        let flat = flatten_doc(&doc);
        println!("flatten_doc {:?}", t.elapsed());
        let t = Instant::now();
        lib.write_thumbnail("a", &flat, n, n).unwrap();
        println!("write_thumbnail {:?}", t.elapsed());
        let t = Instant::now();
        verify_package(&lib.package("a"), n, n).unwrap();
        println!("verify_package {:?}", t.elapsed());
        let t = Instant::now();
        lib.save("b", "T", &doc).unwrap();
        println!("Library::save {:?}", t.elapsed());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Time and peak memory of save, load and PSD export of 20 sparse layers at 8000 x 8000
    /// (what the report's 29 s autosave was). Run with
    /// `cargo test --release -p eskiz-app --lib big_document_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing probe, not a check"]
    fn big_document_timing() {
        use std::time::Instant;
        fn peak_mib() -> u64 {
            let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
            status
                .lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
                .map_or(0, |kb| kb / 1024)
        }
        let n = 8000usize;
        let mut doc = Document::new(n, n);
        for i in 0..20usize {
            let id = if i == 0 { doc.layer_id(0).unwrap() } else { doc.add_layer_above(doc.layer_id(i - 1)) };
            let layer = doc.layer_mut(id).unwrap();
            let rect: Vec<u8> = (0..1500 * 600 * 4).map(|k| ((k * 31 + i * 7) % 253) as u8).collect();
            layer.write_rect((i * 300) % 6000, (i * 197) % 7000, 1500, 600, &rect);
        }
        println!("built: peak {} MiB", peak_mib());
        let dir = temp("big-timing");
        let lib = Library::at(&dir);
        let t = Instant::now();
        lib.save("a", "T", &doc).unwrap();
        println!("Library::save {:?}, peak {} MiB", t.elapsed(), peak_mib());
        let t = Instant::now();
        let path = dir.join("x.procreate");
        crate::export::export_document(&doc, crate::export::ExportFormat::Procreate, &path).unwrap();
        println!("export .procreate {:?}, peak {} MiB", t.elapsed(), peak_mib());
        let t = Instant::now();
        let back = lib.load("a").unwrap();
        println!("load {:?}, peak {} MiB", t.elapsed(), peak_mib());
        drop(back);
        let t = Instant::now();
        crate::export::export_document(&doc, crate::export::ExportFormat::Psd, &dir.join("x.psd")).unwrap();
        println!("export PSD {:?}, peak {} MiB", t.elapsed(), peak_mib());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_layer_tree_round_trips() {
        let dir = temp("tree");
        let lib = Library::at(&dir);
        let mut doc = sample_document(24, 16);
        let (a, b) = (doc.layer_id(0).unwrap(), doc.layer_id(1).unwrap());
        let g = doc.push_group(None, eskiz_core::Props::new("Folder"), None);
        let inner = doc.push_layer(Some(g), eskiz_core::Props::new("Inner"), None, vec![9; 24 * 16 * 4]);
        {
            let p = &mut doc.node_mut(inner).unwrap().props;
            (p.clipped, p.alpha_lock, p.locked, p.private, p.opacity) = (true, true, true, true, 0.5);
        }
        {
            let n = doc.node_mut(g).unwrap();
            n.props.opacity = 0.25;
            n.props.visible = false;
            if let eskiz_core::NodeKind::Group { collapsed, .. } = &mut n.kind {
                *collapsed = true;
            }
        }
        doc.background = [0.5, 0.25, 0.75];
        doc.background_visible = false;
        let id = new_id();
        lib.save(&id, "Tree", &doc).unwrap();
        let back = lib.load(&id).unwrap();
        assert_eq!(back.root.len(), 3);
        assert_eq!(back.layer_ids().len(), 3);
        let bg = &back.root[2];
        assert_eq!((bg.props.name.as_str(), bg.props.opacity, bg.props.visible), ("Folder", 0.25, false));
        assert!(matches!(bg.kind, eskiz_core::NodeKind::Group { collapsed: true, .. }));
        let inner_back = &bg.children()[0];
        let p = &inner_back.props;
        assert_eq!((p.clipped, p.alpha_lock, p.locked, p.private, p.opacity), (true, true, true, true, 0.5));
        assert_eq!(back.layer(inner_back.id).unwrap().to_rgba(), vec![9; 24 * 16 * 4]);
        assert!(!back.background_visible);
        assert!((back.background[0] - 0.5).abs() < 1e-6 && (back.background[2] - 0.75).abs() < 1e-6);
        assert_eq!(back.root[0].props.name, doc.node(a).unwrap().props.name);
        assert_eq!(back.root[1].props.name, doc.node(b).unwrap().props.name);
        assert_eq!(meta_signature(&back), meta_signature(&back.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_layers_and_all_blend_modes_survive() {
        let dir = temp("blend");
        let lib = Library::at(&dir);
        let mut doc = sample_document(16, 16);
        doc.props_at_mut(0).unwrap().visible = false;
        for i in 0..=34 {
            if let Some(mode) = BlendMode::from_index(i) {
                doc.props_at_mut(1).unwrap().blend = mode;
                let id = new_id();
                lib.save(&id, "b", &doc).unwrap();
                let back = lib.load(&id).unwrap();
                assert_eq!(back.node(back.layer_id(1).unwrap()).unwrap().props.blend, mode, "index {i}");
                assert!(!back.node(back.layer_id(0).unwrap()).unwrap().props.visible);
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumbnail_fits_within_512_and_matches_the_art() {
        let dir = temp("thumb");
        let lib = Library::at(&dir);
        let mut doc = Document::new(2000, 1000);
        // Left half solid red.
        doc.layer_at_mut(0).unwrap().write_rect(0, 0, 1000, 1000, &[255, 0, 0, 255].repeat(1000 * 1000));
        let id = new_id();
        lib.save(&id, "Big", &doc).unwrap();
        let t = lib.thumbnail(&id).unwrap();
        assert_eq!((t.width, t.height), (512, 256));
        assert_eq!(&t.rgba[(128 * 512 + 100) * 4..][..4], &[255, 0, 0, 255]);
        assert_eq!(&t.rgba[(128 * 512 + 400) * 4..][..4], &[255, 255, 255, 255]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// `make_thumbnail` as it was before it ran on several threads.
    fn make_thumbnail_serial(rgba: &[u8], width: usize, height: usize, max: usize) -> Thumb {
        let scale = (max as f64 / width.max(height).max(1) as f64).min(1.0);
        let tw = ((width as f64 * scale).round() as usize).clamp(2.min(width), width.max(1));
        let th = ((height as f64 * scale).round() as usize).clamp(2.min(height), height.max(1));
        let mut out = vec![0u8; tw * th * 4];
        for ty in 0..th {
            let y0 = ty * height / th;
            let y1 = ((ty + 1) * height / th).max(y0 + 1).min(height);
            for tx in 0..tw {
                let x0 = tx * width / tw;
                let x1 = ((tx + 1) * width / tw).max(x0 + 1).min(width);
                let mut sum = [0u32; 4];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * width + x) * 4;
                        for (s, v) in sum.iter_mut().zip(&rgba[i..i + 4]) {
                            *s += u32::from(*v);
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                let o = (ty * tw + tx) * 4;
                for c in 0..4 {
                    out[o + c] = ((sum[c] + n / 2) / n) as u8;
                }
            }
        }
        Thumb { width: tw, height: th, rgba: out }
    }

    #[test]
    fn parallel_thumbnail_equals_the_serial_one() {
        let mut state = 0x7B1Fu64;
        let mut next = move || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as u8
        };
        for (w, h, max) in [(1, 1, 512), (2, 3, 512), (100, 50, 512), (999, 777, 512), (4000, 3, 512), (3, 4000, 512), (1300, 1301, 100), (1031, 640, 512)] {
            let px: Vec<u8> = (0..w * h * 4).map(|_| next()).collect();
            assert_eq!(make_thumbnail(&px, w, h, max), make_thumbnail_serial(&px, w, h, max), "{w}x{h} to {max}");
        }
        assert_eq!(make_thumbnail(&[], 0, 0, 512).rgba.len(), 0);
    }

    #[test]
    fn make_thumbnail_sizes() {
        let px = |w: usize, h: usize| vec![200u8; w * h * 4];
        let t = make_thumbnail(&px(100, 50), 100, 50, 512);
        assert_eq!((t.width, t.height), (100, 50), "never enlarges");
        let t = make_thumbnail(&px(1024, 768), 1024, 768, 512);
        assert_eq!((t.width, t.height), (512, 384));
        let t = make_thumbnail(&px(4000, 3), 4000, 3, 512);
        assert_eq!((t.width, t.height), (512, 2), "minimum side is 2");
        assert!(t.rgba.iter().all(|&v| v == 200));
        let t = make_thumbnail(&px(1, 1), 1, 1, 512);
        assert_eq!((t.width, t.height), (1, 1));
    }

    #[test]
    fn duplicate_rename_delete() {
        let dir = temp("ops");
        let lib = Library::at(&dir);
        let id = new_id();
        lib.save(&id, "Alpha", &sample_document(40, 30)).unwrap();
        let copy = lib.duplicate(&id).unwrap();
        assert_ne!(copy.id, id);
        assert_eq!(copy.title, "Alpha copy");
        assert_eq!(lib.load(&copy.id).unwrap().layer_at(0).unwrap().to_rgba(), lib.load(&id).unwrap().layer_at(0).unwrap().to_rgba());
        assert!(lib.thumbnail(&copy.id).is_some());

        lib.rename(&id, "  Beta\nline ").unwrap();
        assert_eq!(lib.get(&id).unwrap().title, "Beta line");
        lib.rename(&id, "   ").unwrap();
        assert_eq!(lib.get(&id).unwrap().title, "Untitled");

        lib.delete(&id).unwrap();
        assert!(lib.get(&id).is_none());
        assert!(lib.thumbnail(&id).is_none());
        assert_eq!(lib.list().len(), 1);
        lib.delete(&id).unwrap();
        assert!(lib.delete("../x").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_copies_a_procreate_file_and_export_writes_a_png() {
        let dir = temp("import");
        let lib = Library::at(dir.join("lib"));
        fs::create_dir_all(&dir).unwrap();
        let src = dir.join("outside.procreate");
        let doc = sample_document(64, 48);
        eskiz_format::document::write(&src, &to_builder(&doc, "Outside")).unwrap();
        let entry = lib.import(&src).unwrap();
        assert_eq!((entry.title.as_str(), entry.width, entry.height), ("Outside", 64, 48));
        assert!(lib.thumbnail(&entry.id).is_some());
        assert!(src.exists(), "the original stays");
        let png = lib.export_png(&entry.id, &dir).unwrap();
        assert!(png.exists());
        let again = lib.export_png(&entry.id, &dir).unwrap();
        assert_ne!(png, again);
        assert!(lib.import(&dir.join("missing.procreate")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_recovers_a_package_without_metadata() {
        let dir = temp("nometa");
        let lib = Library::at(&dir);
        let id = new_id();
        lib.save(&id, "Lost meta", &sample_document(20, 10)).unwrap();
        fs::remove_file(lib.meta_file(&id)).unwrap();
        let list = lib.list();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].title.as_str(), list[0].width), ("Lost meta", 20));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_are_unique_and_safe() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(check_id(&a).is_ok());
        assert!(check_id("a/b").is_err());
        assert!(check_id("").is_err());
    }

    #[test]
    fn meta_signature_tracks_layer_properties() {
        let mut doc = sample_document(8, 8);
        let s0 = meta_signature(&doc);
        assert_eq!(s0, meta_signature(&doc.clone()));
        doc.props_at_mut(0).unwrap().visible = false;
        let s1 = meta_signature(&doc);
        assert_ne!(s0, s1);
        doc.props_at_mut(0).unwrap().name = "x".into();
        assert_ne!(s1, meta_signature(&doc));
    }

    #[test]
    fn autosave_fires_every_interval_while_dirty() {
        let mut c = AutosaveClock::default();
        assert!(!c.update(false, false, 0.0, 30.0), "clean never fires");
        assert!(!c.update(true, false, 10.0, 30.0), "countdown starts when dirty");
        assert!(!c.update(true, false, 39.9, 30.0));
        assert!(c.update(true, false, 40.0, 30.0));
        assert!(!c.update(true, false, 50.0, 30.0), "restarts after firing");
        assert!(c.update(true, false, 70.0, 30.0), "retries while still dirty (failed write)");
        assert!(!c.update(false, false, 71.0, 30.0));
        assert!(!c.update(true, false, 72.0, 30.0), "a new edit starts a fresh countdown");
        assert!(!c.update(true, false, 101.9, 30.0));
        assert!(c.update(true, false, 102.0, 30.0));
    }

    #[test]
    fn autosave_waits_for_a_save_in_flight() {
        let mut c = AutosaveClock::default();
        c.update(true, false, 0.0, 30.0);
        assert!(!c.update(true, true, 45.0, 30.0));
        assert!(c.update(true, false, 46.0, 30.0));
        assert_eq!(c.remaining(46.0, 30.0), Some(30.0));
        assert_eq!(AutosaveClock::default().remaining(0.0, 30.0), None);
    }

    #[test]
    fn worker_saves_loads_and_finishes_queued_jobs() {
        let dir = temp("worker");
        let lib = Library::at(&dir);
        let mut w = Worker::spawn(lib.clone(), || {});
        let id = new_id();
        w.send(Job::Save(SaveRequest {
            id: id.clone(),
            title: "W".into(),
            doc: Box::new(sample_document(30, 20)),
            edits: 7,
            sig: 9,
        }));
        w.send(Job::Load(id.clone()));
        w.finish();
        let done: Vec<Done> = w.rx.try_iter().collect();
        assert_eq!(done.len(), 2);
        assert!(matches!(&done[0], Done::Saved { edits: 7, sig: 9, result: Ok(_), .. }));
        assert!(matches!(&done[1], Done::Loaded { result: Ok(d), .. } if d.width == 30));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rename_persists_across_a_reopen() {
        let dir = temp("rename-persist");
        let lib = Library::at(&dir);
        let id = new_id();
        lib.save(&id, "First", &sample_document(32, 24)).unwrap();
        lib.rename(&id, "  Second\tname  ").unwrap();
        assert_eq!(Library::at(&dir).get(&id).unwrap().title, "Second name");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn library_index_is_written_and_read_back() {
        let dir = temp("index-persist");
        let lib = Library::at(&dir);
        assert_eq!(lib.read_index(), LibraryIndex::default(), "no file: an empty index");
        let mut ix = LibraryIndex::default();
        ix.order.push(crate::stacks::Item::Doc("a".into()));
        lib.write_index(&ix).unwrap();
        assert_eq!(Library::at(&dir).read_index(), ix);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_panicking_job_becomes_an_error() {
        let r: Result<u8, String> = guarded(|| panic!("boom"));
        assert!(r.unwrap_err().contains("boom"));
        assert_eq!(guarded(|| Ok::<_, String>(3)).unwrap(), 3);
    }

    #[test]
    fn a_damaged_index_falls_back_to_the_backup() {
        let dir = temp("index-bak");
        let lib = Library::at(&dir);
        let mut ix = LibraryIndex::default();
        ix.order.push(crate::stacks::Item::Doc("a".into()));
        lib.write_index(&ix).unwrap();
        lib.write_index(&ix).unwrap(); // the second write leaves the first as the backup
        fs::write(dir.join(INDEX_FILE), "{ truncated").unwrap();
        assert_eq!(lib.read_index(), ix);
        assert!(dir.join(format!("{INDEX_FILE}.damaged")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_package_or_folder_keeps_its_place_in_the_index() {
        let dir = temp("sync-keeps");
        let lib = Library::at(&dir);
        let (a, b) = (new_id(), new_id());
        lib.save(&a, "A", &sample_document(32, 24)).unwrap();
        lib.save(&b, "B", &sample_document(32, 24)).unwrap();
        let mut ix = lib.sync_index().1;
        ix.stack_docs("s1".into(), "Stack".into(), &[a.clone(), b.clone()], &a);
        lib.write_index(&ix).unwrap();
        let grouped = lib.read_index();
        assert_eq!(grouped.stacks.len(), 1);
        // One package cannot be opened for a moment (its cache file is gone and it is damaged).
        let _ = fs::remove_file(dir.join(format!("{a}.meta")));
        let good = fs::read(dir.join(format!("{a}.{EXT}"))).unwrap();
        fs::write(dir.join(format!("{a}.{EXT}")), b"locked").unwrap();
        assert_eq!(lib.sync_index().1, grouped, "the stack survives");
        fs::write(dir.join(format!("{a}.{EXT}")), good).unwrap();
        let _ = fs::remove_dir_all(&dir);
        // A library folder that cannot be listed changes nothing.
        let gone = Library::at(dir.join("gone"));
        assert!(gone.sync_index().0.is_empty());
    }

    #[test]
    fn scan_reports_unreadable_packages_and_a_missing_folder() {
        let dir = temp("scan-unreadable");
        let lib = Library::at(&dir);
        assert!(Library::at(dir.join("nope")).scan().is_none());
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("broken.procreate"), b"not a package").unwrap();
        let (entries, unreadable) = lib.scan().unwrap();
        assert!(entries.is_empty());
        assert_eq!(unreadable, vec!["broken".to_owned()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_names_keep_dotted_titles_and_avoid_device_names() {
        use crate::export::with_export_extension as w;
        let p = |s: &str| w(Path::new(s), "png");
        assert_eq!(p("/x/Sketch v1.2"), PathBuf::from("/x/Sketch v1.2.png"));
        assert_ne!(p("/x/Sketch v1.2"), p("/x/Sketch v1.3"));
        assert_eq!(p("/x/a.PNG"), PathBuf::from("/x/a.png"));
        assert_eq!(w(Path::new("/x/a.png"), "pdf"), PathBuf::from("/x/a.pdf"));
        assert_eq!(p("/x/Con"), PathBuf::from("/x/_Con.png"));
        assert_eq!(p("/x/nul.txt"), PathBuf::from("/x/_nul.txt.png"));
        assert_eq!(p("/x/COM1"), PathBuf::from("/x/_COM1.png"));
        assert_eq!(p("/x/Console"), PathBuf::from("/x/Console.png"));
        assert_eq!(p("/x/name. "), PathBuf::from("/x/name.png"));
        assert_eq!(file_safe("LPT3"), "_LPT3");
    }

    #[test]
    fn selftest_passes() {
        let lines = selftest().unwrap();
        assert!(lines[0].contains("gallery"));
    }
}
