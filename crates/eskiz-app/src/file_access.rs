//! Native file picking behind one trait.
//!
//! Every open, import, export, font, reference, brush and palette path in Eskiz used to be a typed
//! text field. [`FileAccess`] lets the UI ask for a path without caring how it is chosen: the
//! desktop build uses `rfd` (feature `native-dialogs`), tests use [`MockFileAccess`], and an
//! Android build can add a storage-access backend later. The typed fields stay as the fallback.
//!
//! A pick never blocks the UI thread. [`FileAccess::pick`] returns a channel; the app polls it
//! once per frame (`EskizApp::poll_picks`) and routes a finished pick to the existing action.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use crate::animation_ui::AnimFormat;
use crate::deliver::{Delivery, ExportKind};
use crate::export::ExportFormat;

/// What kind of dialog to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickKind {
    OpenFile,
    /// A save dialog with a suggested file name (with extension).
    SaveFile { suggested: String },
    Folder,
}

/// A named group of file extensions (no leading dot) for the dialog's type menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileFilter {
    pub name: &'static str,
    pub extensions: &'static [&'static str],
}

/// Which action a pick feeds. The app routes the result on this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PickPurpose {
    /// Open a `.procreate` file, replacing the drawing.
    OpenDocument,
    /// Import a picture, PSD, `.procreate` file or 3D model as layers.
    ImportImage,
    ImportFont,
    ImportBrush,
    Reference,
    PaletteImport,
    PaletteExport,
    /// The Share sheet's base name (each format adds its own extension).
    ExportBase,
    /// A folder for the layer PNGs or the 3D texture PNGs.
    ExportFolder,
    /// The 3D model as a `.glb` file.
    Model3d,
    /// The 3D model as a `.usdz` package.
    Usdz,
    GalleryImport,
    /// The folder a gallery item is exported into.
    GalleryShare,
    /// Android: where one still export is saved. The answer is a staging path; `commit` copies the
    /// finished file to the place the user chose.
    SaveExport(ExportFormat),
    /// Android: the same for an animation export.
    SaveAnim(AnimFormat),
    /// Android: choose (or change) the folder exports are saved into. The answer is its name.
    ExportTree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRequest {
    pub purpose: PickPurpose,
    pub kind: PickKind,
    pub filters: Vec<FileFilter>,
    pub start_dir: Option<PathBuf>,
}

/// The outcome of a pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    Path(PathBuf),
    /// The user cancelled.
    None,
    /// No dialog could be shown; the message says why. The typed path field still works.
    Unavailable(String),
}

/// What happened to an export after it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivered {
    /// The file is where the user asked for it already (desktop).
    InPlace,
    /// It was copied to the chosen place; `name` is the name the provider reports.
    Saved { name: String },
    Failed(String),
}

/// Message when the platform has no share sheet.
pub const NO_SHARE: &str = "Sharing is not available here. Save the file instead.";

pub trait FileAccess: Send + Sync {
    /// Ask for a path. Never blocks the UI thread; the result arrives on the channel.
    fn pick(&self, req: PickRequest) -> Receiver<Picked>;
    /// After an export wrote `written`: deliver it to where the pick pointed. Desktop: nothing to do.
    fn commit(&self, _written: &Path) -> Receiver<Delivered> {
        ready_delivered(Delivered::InPlace)
    }
    /// Hand finished files to the system share sheet. Desktop: unavailable.
    fn share(&self, _files: &[PathBuf], _mime: &str) -> Result<(), String> {
        Err(NO_SHARE.into())
    }
    /// A new path in the share cache for a file called `name` (Android only).
    fn share_path(&self, _name: &str) -> Option<PathBuf> {
        None
    }
    /// Whether picks give staging paths (SAF) and the Share tab should show the Android layout.
    fn staged(&self) -> bool {
        false
    }
    /// The name of the folder exports are saved into, while its permission is still held.
    fn export_folder(&self) -> Option<String> {
        None
    }
}

const PICTURES: FileFilter = FileFilter { name: "Pictures", extensions: &["png", "jpg", "jpeg", "webp"] };
const IMPORTABLE: FileFilter = FileFilter {
    name: "Pictures, PSD, .procreate and 3D models",
    extensions: &["png", "jpg", "jpeg", "webp", "psd", "procreate", "glb", "gltf", "obj", "usd", "usdz", "usda", "usdc"],
};
const PROCREATE: FileFilter = FileFilter { name: "Eskiz drawings", extensions: &["procreate"] };
const FONTS: FileFilter = FileFilter { name: "Fonts", extensions: &["ttf", "otf", "ttc"] };
const BRUSHES: FileFilter = FileFilter { name: "Brushes", extensions: &["brush", "brushset", "abr"] };
const PALETTES: FileFilter = FileFilter { name: "Palettes", extensions: &["swatches", "aco", "ase"] };
const GLB: FileFilter = FileFilter { name: "glTF binary", extensions: &["glb"] };
const USDZ: FileFilter = FileFilter { name: "USDZ package", extensions: &["usdz"] };

/// The dialog filters for a purpose.
pub fn filters_for(purpose: PickPurpose) -> Vec<FileFilter> {
    match purpose {
        PickPurpose::OpenDocument | PickPurpose::GalleryImport => vec![PROCREATE],
        PickPurpose::ImportImage => vec![IMPORTABLE],
        PickPurpose::ImportFont => vec![FONTS],
        PickPurpose::ImportBrush => vec![BRUSHES],
        PickPurpose::Reference => vec![PICTURES],
        PickPurpose::PaletteImport | PickPurpose::PaletteExport => vec![PALETTES],
        PickPurpose::Model3d => vec![GLB],
        PickPurpose::Usdz => vec![USDZ],
        PickPurpose::ExportBase
        | PickPurpose::ExportFolder
        | PickPurpose::GalleryShare
        | PickPurpose::ExportTree
        | PickPurpose::SaveExport(_)
        | PickPurpose::SaveAnim(_) => Vec::new(),
    }
}

impl PickRequest {
    /// The standard request for a purpose: its kind and filters. `suggested` names the file in a
    /// save dialog and `start_dir` is where the dialog opens.
    pub fn for_purpose(purpose: PickPurpose, suggested: Option<String>, start_dir: Option<PathBuf>) -> Self {
        let kind = match purpose {
            PickPurpose::ExportFolder | PickPurpose::GalleryShare | PickPurpose::ExportTree => PickKind::Folder,
            PickPurpose::SaveExport(f) => PickKind::SaveFile {
                suggested: suggested.unwrap_or_else(|| format!("eskiz_export.{}", f.extension())),
            },
            PickPurpose::SaveAnim(f) => PickKind::SaveFile {
                suggested: suggested.unwrap_or_else(|| format!("eskiz_export.{}", f.extension())),
            },
            PickPurpose::PaletteExport => PickKind::SaveFile { suggested: suggested.unwrap_or_else(|| "palette.swatches".into()) },
            PickPurpose::ExportBase => PickKind::SaveFile { suggested: suggested.unwrap_or_else(|| "eskiz_export".into()) },
            PickPurpose::Model3d => PickKind::SaveFile { suggested: suggested.unwrap_or_else(|| "model.glb".into()) },
            PickPurpose::Usdz => PickKind::SaveFile { suggested: suggested.unwrap_or_else(|| "model.usdz".into()) },
            _ => PickKind::OpenFile,
        };
        Self { purpose, kind, filters: filters_for(purpose), start_dir }
    }
}

/// The directory a typed path points into, to start a dialog there: the path itself if it is a
/// folder, else its parent when that exists.
pub fn start_dir_of(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    path.parent().filter(|p| !p.as_os_str().is_empty() && p.is_dir()).map(Path::to_path_buf)
}

/// `path` without its extension: what the Share sheet stores, since each format adds its own.
pub fn strip_extension(path: &Path) -> PathBuf {
    path.with_extension("")
}

/// A channel that already holds `picked`.
pub(crate) fn ready(picked: Picked) -> Receiver<Picked> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(picked);
    rx
}

/// A channel that already holds `delivered`.
pub(crate) fn ready_delivered(delivered: Delivered) -> Receiver<Delivered> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(delivered);
    rx
}

/// Used when no dialog backend is compiled in (Android, `--no-default-features`) and by tests that
/// do not script a pick. Always answers [`Picked::Unavailable`].
pub struct NoFileAccess;

impl FileAccess for NoFileAccess {
    fn pick(&self, _req: PickRequest) -> Receiver<Picked> {
        ready(Picked::Unavailable("File dialogs are not available here. Type the path instead.".into()))
    }
}

/// The platform dialogs through `rfd`.
///
/// `rfd` builds the dialog on the calling thread (the UI thread, which macOS needs for its open
/// panel) and the returned future is awaited on a short-lived worker thread, so the UI never
/// blocks. On Linux the xdg-desktop-portal is used; `rfd` 0.17 drives it through `zbus` with its
/// own small executor, so no `tokio` or `async-std` feature is needed. With no portal running no
/// dialog opens and the pick answers [`Picked::None`], as if it were cancelled (`rfd` does not
/// report why).
#[cfg(feature = "native-dialogs")]
pub struct RfdFileAccess;

#[cfg(feature = "native-dialogs")]
impl RfdFileAccess {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "native-dialogs")]
impl Default for RfdFileAccess {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "native-dialogs")]
impl FileAccess for RfdFileAccess {
    fn pick(&self, req: PickRequest) -> Receiver<Picked> {
        use rfd::AsyncFileDialog;
        let mut dialog = AsyncFileDialog::new();
        for f in &req.filters {
            dialog = dialog.add_filter(f.name, f.extensions);
        }
        if let Some(dir) = &req.start_dir {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let finish = move |handle: Option<rfd::FileHandle>| {
            let _ = tx.send(handle.map_or(Picked::None, |h| Picked::Path(h.path().to_path_buf())));
        };
        match req.kind {
            PickKind::OpenFile => spawn_wait(dialog.pick_file(), finish),
            PickKind::SaveFile { suggested } => spawn_wait(dialog.set_file_name(suggested).save_file(), finish),
            PickKind::Folder => spawn_wait(dialog.pick_folder(), finish),
        }
        rx
    }
}

/// Await `fut` on a worker thread and hand its output to `finish`.
#[cfg(feature = "native-dialogs")]
fn spawn_wait<F>(fut: F, finish: impl FnOnce(Option<rfd::FileHandle>) + Send + 'static)
where
    F: std::future::Future<Output = Option<rfd::FileHandle>> + Send + 'static,
{
    let _ = std::thread::Builder::new().name("eskiz-file-dialog".into()).spawn(move || finish(block_on(fut)));
}

/// A minimal `block_on`: park the thread until the future's waker fires.
#[cfg(feature = "native-dialogs")]
fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// The backend the app starts with: `rfd` where compiled in, else [`NoFileAccess`]. Unit tests
/// always get [`NoFileAccess`] so that no test can open a real dialog.
pub fn default_file_access() -> std::sync::Arc<dyn FileAccess> {
    #[cfg(all(target_os = "android", not(test)))]
    {
        std::sync::Arc::new(crate::android_bridge::SafFileAccess::new())
    }
    #[cfg(all(feature = "native-dialogs", not(test), not(target_os = "android")))]
    {
        std::sync::Arc::new(RfdFileAccess::new())
    }
    #[cfg(not(any(
        all(target_os = "android", not(test)),
        all(feature = "native-dialogs", not(test), not(target_os = "android"))
    )))]
    {
        std::sync::Arc::new(NoFileAccess)
    }
}

/// Scripted picks for tests and the self-test: each `pick` pops the next scripted answer (or
/// answers `Picked::None` when the script is empty) and records the request. It also records
/// `commit` and `share` calls and scripts their answers, so the Android flows run on the host.
#[derive(Default)]
pub struct MockFileAccess {
    script: std::sync::Mutex<std::collections::VecDeque<Picked>>,
    requests: std::sync::Mutex<Vec<PickRequest>>,
    staged: std::sync::atomic::AtomicBool,
    folder: std::sync::Mutex<Option<String>>,
    commits: std::sync::Mutex<Vec<PathBuf>>,
    commit_script: std::sync::Mutex<std::collections::VecDeque<Delivered>>,
    shares: std::sync::Mutex<Vec<(Vec<PathBuf>, String)>>,
    share_error: std::sync::Mutex<Option<String>>,
    share_root: std::sync::Mutex<Option<PathBuf>>,
    share_count: std::sync::atomic::AtomicU64,
}

impl MockFileAccess {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(answers: impl IntoIterator<Item = Picked>) -> Self {
        let m = Self::new();
        m.script.lock().expect("mock script").extend(answers);
        m
    }

    pub fn push(&self, answer: Picked) {
        self.script.lock().expect("mock script").push_back(answer);
    }

    /// Every request seen so far.
    pub fn requests(&self) -> Vec<PickRequest> {
        self.requests.lock().expect("mock requests").clone()
    }

    /// Behave like the Android backend: picks give staging paths, the Share tab shows the Android layout.
    pub fn set_staged(&self, on: bool) {
        self.staged.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// The export folder name to report (`None`: no folder chosen).
    pub fn set_folder(&self, name: Option<&str>) {
        *self.folder.lock().expect("mock folder") = name.map(str::to_owned);
    }

    /// The next `commit` answers `answer` (the default is `Saved` with the file's name).
    pub fn push_commit(&self, answer: Delivered) {
        self.commit_script.lock().expect("mock commits").push_back(answer);
    }

    /// Every path handed to `commit`.
    pub fn commits(&self) -> Vec<PathBuf> {
        self.commits.lock().expect("mock commits").clone()
    }

    /// Every `share` call: the files and the MIME type.
    pub fn shares(&self) -> Vec<(Vec<PathBuf>, String)> {
        self.shares.lock().expect("mock shares").clone()
    }

    /// Make `share` fail with `error`.
    pub fn fail_share(&self, error: &str) {
        *self.share_error.lock().expect("mock share error") = Some(error.to_owned());
    }

    /// Where `share_path` hands out files (a test folder).
    pub fn set_share_root(&self, root: PathBuf) {
        *self.share_root.lock().expect("mock share root") = Some(root);
    }
}

impl FileAccess for MockFileAccess {
    fn pick(&self, req: PickRequest) -> Receiver<Picked> {
        self.requests.lock().expect("mock requests").push(req);
        ready(self.script.lock().expect("mock script").pop_front().unwrap_or(Picked::None))
    }

    fn commit(&self, written: &Path) -> Receiver<Delivered> {
        self.commits.lock().expect("mock commits").push(written.to_path_buf());
        let answer = self.commit_script.lock().expect("mock commits").pop_front().unwrap_or_else(|| Delivered::Saved {
            name: written.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        });
        ready_delivered(answer)
    }

    fn share(&self, files: &[PathBuf], mime: &str) -> Result<(), String> {
        self.shares.lock().expect("mock shares").push((files.to_vec(), mime.to_owned()));
        match self.share_error.lock().expect("mock share error").clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn share_path(&self, name: &str) -> Option<PathBuf> {
        let root = self.share_root.lock().expect("mock share root").clone()?;
        let n = self.share_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = root.join(n.to_string());
        let _ = std::fs::create_dir_all(&dir);
        Some(dir.join(name))
    }

    fn staged(&self) -> bool {
        self.staged.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn export_folder(&self) -> Option<String> {
        self.folder.lock().expect("mock folder").clone()
    }
}

/// A pick that was asked for and has not answered yet.
pub struct PendingPick {
    pub purpose: PickPurpose,
    rx: Receiver<Picked>,
}

impl PendingPick {
    #[cfg(test)]
    pub(crate) fn for_test(purpose: PickPurpose, rx: Receiver<Picked>) -> Self {
        Self { purpose, rx }
    }
}

impl crate::app::EskizApp {
    /// Ask the file backend for a path; the answer is routed by `poll_picks`. A second request for
    /// the same purpose replaces the first, so a stale answer is never acted on.
    pub(crate) fn request_pick(&mut self, purpose: PickPurpose, suggested: Option<String>, start_dir: Option<PathBuf>) {
        let req = PickRequest::for_purpose(purpose, suggested, start_dir);
        let rx = self.files.pick(req);
        self.pending_picks.retain(|p| p.purpose != purpose);
        self.pending_picks.push(PendingPick { purpose, rx });
    }

    /// Request a pick that starts in the folder the typed path field points at.
    pub(crate) fn request_pick_near(&mut self, purpose: PickPurpose, typed: &str, suggested: Option<String>) {
        let start = start_dir_of(&crate::export::expand_home(typed));
        self.request_pick(purpose, suggested, start);
    }

    /// Route finished picks to the existing actions. Called every frame. Cancelled picks do
    /// nothing, unavailable ones show a toast, and picks whose sender is gone are dropped.
    pub(crate) fn poll_picks(&mut self, ctx: &egui::Context) {
        if self.pending_picks.is_empty() {
            return;
        }
        let mut done = Vec::new();
        self.pending_picks.retain(|p| match p.rx.try_recv() {
            Ok(picked) => {
                done.push((p.purpose, picked));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        if !self.pending_picks.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        for (purpose, picked) in done {
            self.route_pick(ctx, purpose, picked);
        }
    }

    fn route_pick(&mut self, ctx: &egui::Context, purpose: PickPurpose, picked: Picked) {
        let path = match picked {
            Picked::Path(p) => p,
            Picked::None => {
                if purpose == PickPurpose::GalleryShare {
                    self.gal.share_target = None;
                }
                return;
            }
            Picked::Unavailable(msg) => {
                if purpose == PickPurpose::GalleryShare {
                    // No dialog: keep the old behaviour and export into the default folder.
                    self.gallery_share_to(None);
                }
                self.show_toast(msg, 4.0);
                return;
            }
        };
        let shown = path.display().to_string();
        match purpose {
            PickPurpose::OpenDocument => {
                self.open_path = shown;
                self.load_document(ctx, &path);
            }
            PickPurpose::ImportImage => {
                self.actions_ui.import_path = shown;
                self.start_import_image(ctx, &path);
            }
            PickPurpose::ImportFont => {
                self.text.font_path = shown;
                self.import_font_action(&path);
            }
            PickPurpose::ImportBrush => {
                self.brush_ui.path = shown;
                self.import_brush_file(&path);
            }
            PickPurpose::Reference => {
                self.reference.path = shown;
                self.load_reference_image(ctx, &path);
            }
            PickPurpose::PaletteImport => {
                self.guides.palette_path = shown;
                self.palette_import();
            }
            PickPurpose::PaletteExport => {
                self.guides.palette_path = shown;
                self.palette_export_with(self.pick_delivery());
            }
            PickPurpose::ExportBase => {
                self.export_path = strip_extension(&path).display().to_string();
                self.show_toast("Export location set. Choose a format below.", 3.0);
            }
            PickPurpose::ExportFolder => {
                if self.files.staged() {
                    // Android: `path` is a staging folder; the export goes into a subfolder of it.
                    let kind = if self.three_d.is_some() { ExportKind::Textures } else { ExportKind::Still(ExportFormat::Layers) };
                    let (name, _) = kind.file_name(&self.export_title());
                    self.run_export(ctx, kind, path.join(name), Delivery::Commit);
                    return;
                }
                self.export_path = shown;
                if self.three_d.is_some() {
                    self.start_texture_export(path, Delivery::Local);
                } else {
                    self.show_toast("Export folder set. Choose a format below.", 3.0);
                }
            }
            PickPurpose::Model3d => {
                if !self.files.staged() {
                    self.export_path = strip_extension(&path).display().to_string();
                }
                if self.three_d.is_some() {
                    self.start_glb_export(path.with_extension("glb"), self.pick_delivery());
                }
            }
            PickPurpose::Usdz => {
                if !self.files.staged() {
                    self.export_path = strip_extension(&path).display().to_string();
                }
                if self.three_d.is_some() {
                    self.start_usdz_export(path.with_extension("usdz"), self.pick_delivery());
                }
            }
            PickPurpose::SaveExport(format) => self.start_export_to(ctx, format, path, Delivery::Commit),
            PickPurpose::SaveAnim(format) => self.start_anim_export_to(ctx, format, path, Delivery::Commit),
            PickPurpose::ExportTree => self.show_toast(format!("Exports are saved to {shown}"), 3.0),
            PickPurpose::GalleryImport => self.import_into_gallery(ctx, vec![path]),
            PickPurpose::GalleryShare => self.gallery_share_to(Some(path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [PickPurpose; 14] = [
        PickPurpose::OpenDocument,
        PickPurpose::ImportImage,
        PickPurpose::ImportFont,
        PickPurpose::ImportBrush,
        PickPurpose::Reference,
        PickPurpose::PaletteImport,
        PickPurpose::PaletteExport,
        PickPurpose::ExportBase,
        PickPurpose::ExportFolder,
        PickPurpose::Model3d,
        PickPurpose::Usdz,
        PickPurpose::GalleryImport,
        PickPurpose::GalleryShare,
        PickPurpose::ExportTree,
    ];

    fn exts(p: PickPurpose) -> Vec<&'static str> {
        filters_for(p).iter().flat_map(|f| f.extensions.iter().copied()).collect()
    }

    #[test]
    fn filters_match_what_each_site_accepts() {
        assert_eq!(exts(PickPurpose::OpenDocument), ["procreate"]);
        assert_eq!(exts(PickPurpose::GalleryImport), ["procreate"]);
        assert_eq!(exts(PickPurpose::ImportFont), ["ttf", "otf", "ttc"]);
        assert_eq!(exts(PickPurpose::ImportBrush), ["brush", "brushset", "abr"]);
        assert_eq!(exts(PickPurpose::PaletteImport), ["swatches", "aco", "ase"]);
        assert_eq!(exts(PickPurpose::PaletteExport), ["swatches", "aco", "ase"]);
        assert_eq!(exts(PickPurpose::Model3d), ["glb"]);
        assert_eq!(exts(PickPurpose::Usdz), ["usdz"]);
        assert_eq!(exts(PickPurpose::Reference), ["png", "jpg", "jpeg", "webp"]);
        let import = exts(PickPurpose::ImportImage);
        for e in ["png", "jpg", "webp", "psd", "procreate", "glb", "gltf", "obj", "usd", "usdz", "usda", "usdc"] {
            assert!(import.contains(&e), "import filter lacks {e}");
        }
        for p in [PickPurpose::ExportBase, PickPurpose::ExportFolder, PickPurpose::GalleryShare, PickPurpose::ExportTree] {
            assert!(filters_for(p).is_empty());
        }
    }

    #[test]
    fn extensions_have_no_dot_and_are_lowercase() {
        for p in ALL {
            for e in exts(p) {
                assert!(!e.starts_with('.') && e == e.to_lowercase(), "{e}");
            }
        }
    }

    #[test]
    fn kinds_per_purpose() {
        for p in ALL {
            let kind = PickRequest::for_purpose(p, None, None).kind;
            let want_save = matches!(p, PickPurpose::PaletteExport | PickPurpose::ExportBase | PickPurpose::Model3d | PickPurpose::Usdz);
            let want_folder = matches!(p, PickPurpose::ExportFolder | PickPurpose::GalleryShare | PickPurpose::ExportTree);
            match kind {
                PickKind::SaveFile { .. } => assert!(want_save, "{p:?}"),
                PickKind::Folder => assert!(want_folder, "{p:?}"),
                PickKind::OpenFile => assert!(!want_save && !want_folder, "{p:?}"),
            }
        }
    }

    #[test]
    fn export_base_strips_the_extension() {
        assert_eq!(strip_extension(Path::new("/a/b/art.png")), PathBuf::from("/a/b/art"));
        assert_eq!(strip_extension(Path::new("/a/b/art")), PathBuf::from("/a/b/art"));
    }

    #[test]
    fn mock_records_and_scripts() {
        let m = MockFileAccess::with([Picked::Path("/x/y.png".into()), Picked::None]);
        let req = PickRequest::for_purpose(PickPurpose::Reference, None, None);
        assert_eq!(m.pick(req.clone()).try_recv().unwrap(), Picked::Path("/x/y.png".into()));
        assert_eq!(m.pick(req.clone()).try_recv().unwrap(), Picked::None);
        assert_eq!(m.pick(req.clone()).try_recv().unwrap(), Picked::None, "an empty script cancels");
        assert_eq!(m.requests().len(), 3);
        assert_eq!(m.requests()[0], req);
    }

    #[test]
    fn no_file_access_is_unavailable() {
        let rx = NoFileAccess.pick(PickRequest::for_purpose(PickPurpose::ImportFont, None, None));
        assert!(matches!(rx.try_recv().unwrap(), Picked::Unavailable(_)));
    }

    #[test]
    fn start_dir_prefers_an_existing_folder() {
        let dir = std::env::temp_dir();
        assert_eq!(start_dir_of(&dir), Some(dir.clone()));
        assert_eq!(start_dir_of(&dir.join("nope-file.png")), Some(dir));
        assert_eq!(start_dir_of(Path::new("nope-file.png")), None);
    }
}
