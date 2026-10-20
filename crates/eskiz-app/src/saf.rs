//! The pure part of the Android file bridge (A4).
//!
//! Android hands out `content://` URIs, Eskiz works on paths. The Java side copies a picked
//! document into the app cache and reports `(path, display name, MIME type)`; an export is written
//! to a staging path in the cache and copied to the chosen place afterwards (design section 3.1).
//! Everything here is plain Rust so the host tests cover it; only the JNI shims live in
//! `android_bridge.rs`.
//!
//! * [`Table`]: the request table. Every picker, commit and clipboard read has a request id; the
//!   Java side reports back with that id and the matching channel is answered. Unknown ids (the
//!   process died while a picker was open) become a [`NOTICES`] message.
//! * [`mimes_for`], [`mime_for_export`], [`clean_name`]: the MIME and file-name rules.
//! * [`staging_dir`], [`is_stale`]: the cache layout and the clean-up rule.
//! * [`parse_hook`]: the debug-only test hooks of the emulator script.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Mutex;

use crate::animation_ui::AnimFormat;
use crate::export::ExportFormat;
use crate::file_access::{Delivered, PickPurpose, Picked};

/// Result codes shared with `EskizActivity.java`.
pub const STATUS_OK: i32 = 0;
pub const STATUS_CANCELLED: i32 = 1;
pub const STATUS_ERROR: i32 = 2;

/// Shown when a result arrives for a request this process does not know (it was started by a
/// process that died while the picker was open).
pub const INTERRUPTED: &str = "The file choice was interrupted. Try again.";

/// What the Java side has queued for the frame loop: messages to toast, test hooks and Back
/// presses. The app drains its inbox once per frame ([`crate::EskizApp::poll_bridge`]).
#[derive(Default)]
pub struct Inbox {
    notices: Mutex<Vec<String>>,
    hooks: Mutex<Vec<String>>,
    backs: Mutex<u32>,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inbox {
    pub fn notice(&self, text: impl Into<String>) {
        locked(&self.notices).push(text.into());
    }

    pub fn take_notices(&self) -> Vec<String> {
        std::mem::take(&mut *locked(&self.notices))
    }

    pub fn push_hook(&self, text: impl Into<String>) {
        locked(&self.hooks).push(text.into());
    }

    pub fn take_hooks(&self) -> Vec<String> {
        std::mem::take(&mut *locked(&self.hooks))
    }

    pub fn push_back(&self) {
        *locked(&self.backs) += 1;
    }

    pub fn take_backs(&self) -> u32 {
        std::mem::take(&mut *locked(&self.backs))
    }
}

/// The process-wide inbox the JNI shims fill.
pub fn inbox() -> &'static std::sync::Arc<Inbox> {
    static INBOX: std::sync::OnceLock<std::sync::Arc<Inbox>> = std::sync::OnceLock::new();
    INBOX.get_or_init(Default::default)
}

/// One line in logcat (tag `eskiz`) on Android; nothing elsewhere. The emulator script waits for
/// these lines (`saf:`, `share:`, `clip:`, `back:`, `eskiz-test:`).
pub fn log_line(text: &str) {
    #[cfg(target_os = "android")]
    log::info!("{text}");
    #[cfg(not(target_os = "android"))]
    let _ = text;
}

/// Queue a message for the next frame.
pub fn notice(text: impl Into<String>) {
    inbox().notice(text);
}

/// Everything queued by [`notice`] so far.
pub fn take_notices() -> Vec<String> {
    inbox().take_notices()
}

// ------------------------------------------------------------------------------------------------
// Names and MIME types

const OCTET: &str = "application/octet-stream";
const ZIP: &str = "application/zip";

/// What the open picker shows for a purpose. Android has no MIME type for `.procreate`, `.brush`,
/// `.brushset`, `.abr`, the palette formats or `.usda`/`.usdc`; those purposes list the known
/// types plus `application/octet-stream` and `application/zip`, and the extension is checked after
/// the copy ([`check_extension`]).
pub fn mimes_for(purpose: PickPurpose) -> Vec<&'static str> {
    let pictures = ["image/png", "image/jpeg", "image/webp"];
    match purpose {
        PickPurpose::OpenDocument | PickPurpose::GalleryImport => vec![OCTET, ZIP],
        PickPurpose::ImportImage => {
            let mut v = pictures.to_vec();
            v.extend([
                "image/vnd.adobe.photoshop",
                "model/gltf-binary",
                "model/gltf+json",
                "model/obj",
                "model/vnd.usdz+zip",
                OCTET,
                ZIP,
            ]);
            v
        }
        PickPurpose::Reference => pictures.to_vec(),
        PickPurpose::ImportFont => vec!["font/ttf", "font/otf", "font/collection", OCTET],
        PickPurpose::ImportBrush | PickPurpose::PaletteImport => vec![OCTET, ZIP],
        PickPurpose::PaletteExport => vec![OCTET],
        PickPurpose::Model3d => vec!["model/gltf-binary"],
        PickPurpose::Usdz => vec!["model/vnd.usdz+zip"],
        PickPurpose::SaveExport(f) => vec![mime_for_export(f)],
        PickPurpose::SaveAnim(f) => vec![mime_for_anim(f)],
        PickPurpose::ExportBase | PickPurpose::ExportFolder | PickPurpose::ExportTree | PickPurpose::GalleryShare => Vec::new(),
    }
}

/// The MIME type of a still export. `.procreate` uses octet-stream: with `application/zip` some
/// providers append `.zip` and give `Art.procreate.zip`.
pub fn mime_for_export(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Procreate => OCTET,
        ExportFormat::Psd => "image/vnd.adobe.photoshop",
        ExportFormat::Pdf => "application/pdf",
        ExportFormat::Png | ExportFormat::Layers => "image/png",
        ExportFormat::Jpeg => "image/jpeg",
        ExportFormat::Tiff => "image/tiff",
        ExportFormat::WebP => "image/webp",
    }
}

pub fn mime_for_anim(format: AnimFormat) -> &'static str {
    match format {
        AnimFormat::Gif => "image/gif",
        AnimFormat::Apng => "image/png",
        AnimFormat::Mp4 => "video/mp4",
    }
}

/// The MIME type for a file name by its extension (share sheet and provider).
pub fn mime_for_name(name: &str) -> &'static str {
    match ext_of(name).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "psd" => "image/vnd.adobe.photoshop",
        "glb" => "model/gltf-binary",
        "usdz" => "model/vnd.usdz+zip",
        "mp4" => "video/mp4",
        _ => OCTET,
    }
}

/// The MIME type to give the share sheet for several files: the common one, else `*/*`.
pub fn common_mime(names: &[String]) -> &'static str {
    let mut it = names.iter().map(|n| mime_for_name(n));
    let Some(first) = it.next() else { return "*/*" };
    if it.all(|m| m == first) {
        first
    } else {
        "*/*"
    }
}

fn ext_of(name: &str) -> String {
    Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default()
}

/// The extension a MIME type implies, for a picked document whose display name has none.
fn ext_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/tiff" => "tif",
        "application/pdf" => "pdf",
        "image/vnd.adobe.photoshop" => "psd",
        "model/gltf-binary" => "glb",
        "model/gltf+json" => "gltf",
        "model/obj" => "obj",
        "model/vnd.usdz+zip" => "usdz",
        "font/ttf" => "ttf",
        "font/otf" => "otf",
        "font/collection" => "ttc",
        _ => return None,
    })
}

/// Longest file name kept, in bytes.
pub const MAX_NAME_BYTES: usize = 120;

/// A display name made safe for a cache file: no path separators, NUL or control characters, no
/// `..`; trimmed; at most [`MAX_NAME_BYTES`] bytes on a character boundary with the extension kept;
/// empty becomes `Untitled`; a name without an extension gets one from the MIME type when it
/// implies one. Import routing (`is_layered_file`, `is_model_file`, the brush and font loaders)
/// looks at the extension, so this matters.
pub fn clean_name(display: &str, mime: &str) -> String {
    // Keep the part after the last separator, as a provider may report a path.
    let base = display.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String = base.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
    while s.contains("..") {
        s = s.replace("..", ".");
    }
    let s = s.trim().trim_matches('.').trim().to_owned();
    let mut name = if s.is_empty() { "Untitled".to_owned() } else { s };
    if Path::new(&name).extension().is_none() {
        if let Some(ext) = ext_for_mime(&mime.to_ascii_lowercase()) {
            name = format!("{name}.{ext}");
        }
    }
    cap_bytes(&name, MAX_NAME_BYTES)
}

/// Shorten the stem of `name` so the whole is at most `max` bytes, on a character boundary,
/// keeping the extension.
fn cap_bytes(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    let room = max.saturating_sub(ext.len());
    let mut end = room.min(stem.len());
    while end > 0 && !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ext}", &stem[..end])
}

/// A file name for an export: the document title cleaned for a file system, plus the extension.
pub fn export_file_name(title: &str, extension: &str) -> String {
    // A slash in a title is dropped, not treated as a folder boundary.
    let title: String = title.chars().filter(|c| !matches!(c, '/' | '\\')).collect();
    let stem = clean_name(&title, "");
    let stem = Path::new(&stem).file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_owned();
    let stem = if stem.is_empty() { "Untitled".to_owned() } else { stem };
    if extension.is_empty() {
        stem
    } else {
        cap_bytes(&format!("{stem}.{extension}"), MAX_NAME_BYTES)
    }
}

/// Does the picked file's name fit the purpose? `Err` carries the message to toast. Purposes
/// whose content is sniffed by the loaders (pictures, fonts) are not checked here.
pub fn check_extension(purpose: PickPurpose, name: &str) -> Result<(), String> {
    let ext = ext_of(name);
    let (ok, what): (&[&str], &str) = match purpose {
        PickPurpose::OpenDocument | PickPurpose::GalleryImport => (&["procreate"], "That file is not a .procreate drawing."),
        PickPurpose::ImportBrush => (&["brush", "brushset", "abr"], "That file is not a brush (.brush, .brushset or .abr)."),
        PickPurpose::PaletteImport => (&["swatches", "aco", "ase"], "That file is not a palette (.swatches, .aco or .ase)."),
        PickPurpose::ImportFont => (&["ttf", "otf", "ttc"], "That file is not a font (.ttf, .otf or .ttc)."),
        _ => return Ok(()),
    };
    if ok.contains(&ext.as_str()) {
        Ok(())
    } else {
        Err(what.to_owned())
    }
}

// ------------------------------------------------------------------------------------------------
// Staging layout and clean-up

/// `<cache>/saf/<req>`: where a request's files live.
pub fn staging_dir(cache: &Path, req: i32) -> PathBuf {
    cache.join("saf").join(req.to_string())
}

/// `<cache>/share/<n>`: where a file for the share sheet is written.
pub fn share_dir(cache: &Path, n: u64) -> PathBuf {
    cache.join("share").join(n.to_string())
}

/// `<cache>/share/clip`: the PNGs put on the system clipboard.
pub fn clip_dir(cache: &Path) -> PathBuf {
    cache.join("share").join("clip")
}

/// Cache entries older than this are deleted at start.
pub const MAX_AGE_SECS: u64 = 24 * 3600;

/// Should the cache entry be deleted at start? Old ones go, except the current clipboard file
/// (another app may read it later).
pub fn is_stale(age_secs: u64, is_current_clip: bool) -> bool {
    age_secs > MAX_AGE_SECS && !is_current_clip
}

/// The request-independent rule of the provider: is `candidate` (already canonical) inside `root`
/// (canonical)? The Java provider applies the same rule with `File.getCanonicalPath`.
pub fn inside_root(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root) && candidate != root
}

// ------------------------------------------------------------------------------------------------
// The request table

/// The process-wide table the JNI shims answer into.
pub fn global() -> std::sync::MutexGuard<'static, Table> {
    static TABLE: std::sync::OnceLock<Mutex<Table>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(Table::new())).lock().unwrap_or_else(|e| e.into_inner())
}

/// Staging folders (or files' parent folders) and where their contents go once written. Filled by
/// the pick answers, read by `commit`.
pub static STAGED: Mutex<Vec<(PathBuf, Dest)>> = Mutex::new(Vec::new());

/// Remember where the files under `dir` go.
pub fn remember_stage(dir: PathBuf, dest: Dest) {
    let mut v = STAGED.lock().unwrap_or_else(|e| e.into_inner());
    v.retain(|(d, _)| *d != dir);
    v.push((dir, dest));
    // Only the latest few matter; an export is committed right after it is written.
    if v.len() > 64 {
        v.remove(0);
    }
}

/// The destination for a written file or folder: the nearest remembered staging folder above it.
pub fn dest_for(written: &Path) -> Option<Dest> {
    let v = STAGED.lock().unwrap_or_else(|e| e.into_inner());
    written.ancestors().find_map(|a| v.iter().find(|(d, _)| d == a).map(|(_, dest)| dest.clone()))
}

/// What a request is waiting for.
pub enum Pending {
    /// An open picker: the copy lands in `dir`, and the pick is checked against `purpose`.
    Open { purpose: PickPurpose, dir: PathBuf, tx: Sender<Picked> },
    /// A create-document picker: the file will be written to `dir/<name>` and committed.
    Create { dir: PathBuf, tx: Sender<Picked> },
    /// A tree picker. The answer carries the folder's display name.
    Tree { tx: Sender<Picked> },
    /// A tree picker that must end with a staging folder (layer PNGs, texture PNGs).
    TreeStage { dir: PathBuf, tx: Sender<Picked> },
    /// A copy to the chosen place.
    Commit { tx: Sender<Delivered> },
    /// A read of a foreign clipboard image.
    Clip { tx: Sender<Result<PathBuf, String>> },
}

/// Request ids and their channels.
#[derive(Default)]
pub struct Table {
    pub(crate) next: i32,
    map: HashMap<i32, Pending>,
}

/// Where a staged export goes once it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dest {
    /// The document made by the create picker with this request id.
    Doc(i32),
    /// The export folder (a persisted tree).
    Tree,
}

impl Table {
    pub fn new() -> Self {
        Self::default()
    }

    /// An unused id: positive, below 2^16 and not in use.
    pub fn fresh_id(&mut self) -> i32 {
        loop {
            self.next = if self.next >= 0xFFFE { 1 } else { self.next + 1 };
            if !self.map.contains_key(&self.next) {
                return self.next;
            }
        }
    }

    /// Register a request.
    pub fn alloc(&mut self, pending: Pending) -> i32 {
        self.alloc_with(|_| pending)
    }

    /// Register a request that needs its own id to build itself (its staging folder).
    pub fn alloc_with(&mut self, make: impl FnOnce(i32) -> Pending) -> i32 {
        let id = self.fresh_id();
        self.map.insert(id, make(id));
        id
    }

    pub fn take(&mut self, req: i32) -> Option<Pending> {
        self.map.remove(&req)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// A picker finished. Returns the staging destination to remember for a create picker.
    ///
    /// `path` is the copy Java made (open pickers) and `name` the display name; for errors `name`
    /// is the message. Unknown ids are reported through [`notice`].
    pub fn resolve_pick(&mut self, req: i32, status: i32, path: &str, name: &str, mime: &str) -> Option<(PathBuf, Dest)> {
        let Some(pending) = self.take(req) else {
            log_line(&format!("saf: unknown request {req}"));
            notice(INTERRUPTED);
            return None;
        };
        match pending {
            Pending::Open { purpose, dir, tx } => {
                let answer = match status {
                    STATUS_OK => open_answer(purpose, &dir, path, name, mime),
                    STATUS_CANCELLED => Picked::None,
                    _ => Picked::Unavailable(error_text(name)),
                };
                let _ = tx.send(answer);
                None
            }
            Pending::Create { dir, tx } => match status {
                STATUS_OK => {
                    let file = dir.join(clean_name(name, mime));
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = tx.send(Picked::Path(file.clone()));
                    Some((dir, Dest::Doc(req)))
                }
                STATUS_CANCELLED => {
                    let _ = tx.send(Picked::None);
                    None
                }
                _ => {
                    let _ = tx.send(Picked::Unavailable(error_text(name)));
                    None
                }
            },
            Pending::Tree { tx } => {
                let _ = tx.send(match status {
                    STATUS_OK => Picked::Path(PathBuf::from(name)),
                    STATUS_CANCELLED => Picked::None,
                    _ => Picked::Unavailable(error_text(name)),
                });
                None
            }
            Pending::TreeStage { dir, tx } => match status {
                STATUS_OK => {
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = tx.send(Picked::Path(dir.clone()));
                    Some((dir, Dest::Tree))
                }
                STATUS_CANCELLED => {
                    let _ = tx.send(Picked::None);
                    None
                }
                _ => {
                    let _ = tx.send(Picked::Unavailable(error_text(name)));
                    None
                }
            },
            Pending::Commit { tx } => {
                let _ = tx.send(delivered_from(status, name));
                None
            }
            Pending::Clip { tx } => {
                let _ = tx.send(match status {
                    STATUS_OK => Ok(PathBuf::from(path)),
                    _ => Err(error_text(name)),
                });
                None
            }
        }
    }

    /// A commit finished.
    pub fn resolve_commit(&mut self, req: i32, status: i32, text: &str) {
        match self.take(req) {
            Some(Pending::Commit { tx }) => {
                let _ = tx.send(delivered_from(status, text));
            }
            Some(_) | None => {
                log_line(&format!("saf: unknown request {req}"));
                notice(INTERRUPTED)
            }
        }
    }

    /// A foreign clipboard image was copied (or not).
    pub fn resolve_clip(&mut self, req: i32, status: i32, path: &str, text: &str) {
        if let Some(Pending::Clip { tx }) = self.take(req) {
            let _ = tx.send(if status == STATUS_OK { Ok(PathBuf::from(path)) } else { Err(error_text(text)) });
        }
    }
}

fn error_text(text: &str) -> String {
    if text.trim().is_empty() {
        "The file could not be read.".to_owned()
    } else {
        text.to_owned()
    }
}

fn delivered_from(status: i32, text: &str) -> Delivered {
    match status {
        STATUS_OK => Delivered::Saved { name: text.to_owned() },
        STATUS_CANCELLED => Delivered::Failed("Saving was canceled.".to_owned()),
        _ => Delivered::Failed(error_text(text)),
    }
}

/// Rename the copy Java made to its cleaned display name and check the name fits the purpose.
fn open_answer(purpose: PickPurpose, dir: &Path, path: &str, name: &str, mime: &str) -> Picked {
    let clean = clean_name(name, mime);
    if let Err(msg) = check_extension(purpose, &clean) {
        let _ = std::fs::remove_file(path);
        return Picked::Unavailable(msg);
    }
    let from = PathBuf::from(path);
    let to = dir.join(&clean);
    if from == to {
        return Picked::Path(to);
    }
    match std::fs::rename(&from, &to).or_else(|_| std::fs::copy(&from, &to).map(|_| ()).and_then(|()| std::fs::remove_file(&from))) {
        Ok(()) => Picked::Path(to),
        Err(e) => Picked::Unavailable(format!("The file could not be prepared: {e}")),
    }
}

// ------------------------------------------------------------------------------------------------
// Test hooks (debug APK only)

/// A command from `adb shell am start ... --es eskiz_test <hook>` (design section 3.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hook {
    OpenDocument,
    ImportImage,
    ImportBrush,
    GalleryImport,
    ChooseFolder,
    Export { format: HookFormat, share: bool },
    GalleryShareFirst,
    CopyAll,
    Paste,
    BackState,
    NewCanvas,
    /// `skip_tour`: dismiss the welcome tour and never start it.
    SkipTour,
    /// `open_path:<file>`: open a `.procreate` file by path, without a picker.
    OpenPath(String),
    /// `open_first`: open the first document of the library.
    OpenFirst,
}

/// The formats the `export:<format>:<mode>` hook names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookFormat {
    Still(ExportFormat),
    Anim(AnimFormat),
}

/// Parse a hook string. Unknown hooks give `None`.
pub fn parse_hook(text: &str) -> Option<Hook> {
    let text = text.trim();
    Some(match text {
        "open_document" => Hook::OpenDocument,
        "import_image" => Hook::ImportImage,
        "import_brush" => Hook::ImportBrush,
        "gallery_import" => Hook::GalleryImport,
        "choose_folder" => Hook::ChooseFolder,
        "gallery_share_first" => Hook::GalleryShareFirst,
        "copy_all" => Hook::CopyAll,
        "paste" => Hook::Paste,
        "back_state" => Hook::BackState,
        "new_canvas" => Hook::NewCanvas,
        "skip_tour" => Hook::SkipTour,
        "open_first" => Hook::OpenFirst,
        _ if text.starts_with("open_path:") && text.len() > "open_path:".len() => Hook::OpenPath(text["open_path:".len()..].to_owned()),
        _ => {
            let mut parts = text.split(':');
            if parts.next()? != "export" {
                return None;
            }
            let format = match parts.next()? {
                "procreate" => HookFormat::Still(ExportFormat::Procreate),
                "psd" => HookFormat::Still(ExportFormat::Psd),
                "pdf" => HookFormat::Still(ExportFormat::Pdf),
                "png" => HookFormat::Still(ExportFormat::Png),
                "jpeg" | "jpg" => HookFormat::Still(ExportFormat::Jpeg),
                "tiff" => HookFormat::Still(ExportFormat::Tiff),
                "webp" => HookFormat::Still(ExportFormat::WebP),
                "layers" => HookFormat::Still(ExportFormat::Layers),
                "gif" => HookFormat::Anim(AnimFormat::Gif),
                "apng" => HookFormat::Anim(AnimFormat::Apng),
                _ => return None,
            };
            let share = match parts.next()? {
                "save" => false,
                "share" => true,
                _ => return None,
            };
            if parts.next().is_some() {
                return None;
            }
            Hook::Export { format, share }
        }
    })
}

/// Hooks only run when the APK is debuggable; a release APK ignores them.
pub fn hooks_allowed(debuggable: bool) -> bool {
    debuggable
}

pub fn push_hook(text: impl Into<String>) {
    inbox().push_hook(text);
}

pub fn take_hooks() -> Vec<String> {
    inbox().take_hooks()
}

pub fn push_back() {
    inbox().push_back();
}

pub fn take_backs() -> u32 {
    inbox().take_backs()
}
