//! Delivering a finished export (A4).
//!
//! On desktop an export is written where the user asked and the old toast says so. On Android
//! (`FileAccess::staged`) the export is written to a staging path and then either copied to the
//! place the user chose ([`Delivery::Commit`]) or handed to the share sheet ([`Delivery::Share`]).
//! Every export site calls [`EskizApp::deliver`] after its `Ok(path)`; the desktop path is a plain
//! toast with the same words as before.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::app::EskizApp;
use crate::file_access::Delivered;

/// What happens to a file once an export wrote it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delivery {
    /// It stays where it was written (desktop). Toast: the site's own "Exported ..." text.
    #[default]
    Local,
    /// Copy it to the document or folder the user chose (Android "Save to folder").
    Commit,
    /// Open the system share sheet with it (Android "Share").
    Share,
}

/// Android Share tab: where an export goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShareMode {
    #[default]
    SaveToFolder,
    Share,
}

impl ShareMode {
    pub fn delivery(self) -> Delivery {
        match self {
            Self::SaveToFolder => Delivery::Commit,
            Self::Share => Delivery::Share,
        }
    }
}

/// A commit that has not answered yet.
pub struct PendingCommit {
    rx: Receiver<Delivered>,
}

impl EskizApp {
    /// How a finished pick is delivered: copied to the chosen place on Android, left in place on desktop.
    pub(crate) fn pick_delivery(&self) -> Delivery {
        if self.files.staged() {
            Delivery::Commit
        } else {
            Delivery::Local
        }
    }

    /// An export wrote `written`. `local_toast` is what the site said before A4.
    ///
    /// `Local` shows `local_toast` at once. `Commit` asks the backend to copy the file to the
    /// place the user chose and shows "Saved <name>" (or the failure) when that finishes; the
    /// "Exported ..." toast is held back meanwhile. `Share` opens the share sheet with the file,
    /// or with every file in it when `written` is a folder.
    pub(crate) fn deliver(&mut self, written: &Path, delivery: Delivery, local_toast: String, secs: f64) {
        match delivery {
            Delivery::Local => self.show_toast(local_toast, secs),
            Delivery::Commit => {
                let rx = self.files.commit(written);
                match rx.try_recv() {
                    Ok(done) => self.finish_commit(done, local_toast, secs),
                    Err(TryRecvError::Empty) => {
                        self.show_toast("Saving...", 30.0);
                        self.commits.push((PendingCommit { rx }, local_toast, secs));
                    }
                    Err(TryRecvError::Disconnected) => self.show_toast("Could not save the file.", 4.0),
                }
            }
            Delivery::Share => self.share_files(written),
        }
    }

    /// Open the share sheet with `written` (a file) or the files inside it (a folder).
    pub(crate) fn share_files(&mut self, written: &Path) {
        let files = files_to_share(written);
        if files.is_empty() {
            self.show_toast("There is nothing to share.", 3.0);
            return;
        }
        let names: Vec<String> = files.iter().filter_map(|f| f.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
        let mime = crate::saf::common_mime(&names);
        if let Err(e) = self.files.share(&files, mime) {
            self.show_toast(e, 4.0);
        } else {
            self.dismiss_toast_if("Exporting");
        }
    }

    fn finish_commit(&mut self, done: Delivered, local_toast: String, secs: f64) {
        match done {
            Delivered::InPlace => self.show_toast(local_toast, secs),
            Delivered::Saved { name } => self.show_toast(format!("Saved {name}"), 3.5),
            Delivered::Failed(e) => self.show_toast(format!("Could not save: {e}"), 4.5),
        }
    }

    /// Collect finished commits. Called once per frame.
    pub(crate) fn poll_commits(&mut self) {
        if self.commits.is_empty() {
            return;
        }
        let mut done = Vec::new();
        let mut i = 0;
        while i < self.commits.len() {
            match self.commits[i].0.rx.try_recv() {
                Ok(d) => {
                    let (_, text, secs) = self.commits.remove(i);
                    done.push((d, text, secs));
                }
                Err(TryRecvError::Empty) => i += 1,
                Err(TryRecvError::Disconnected) => {
                    self.commits.remove(i);
                    done.push((Delivered::Failed("the copy did not finish".into()), String::new(), 0.0));
                }
            }
        }
        for (d, text, secs) in done {
            self.finish_commit(d, text, secs);
        }
    }

    /// Remove the current toast when it starts with `prefix` (the "Exporting..." message).
    fn dismiss_toast_if(&mut self, prefix: &str) {
        if self.toast.as_ref().is_some_and(|t| t.text.starts_with(prefix)) {
            self.toast = None;
        }
    }
}

/// The files a share sheet should carry: the file itself, or the regular files of a folder.
pub(crate) fn files_to_share(written: &Path) -> Vec<PathBuf> {
    if written.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(written)
            .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_file()).collect())
            .unwrap_or_default();
        v.sort();
        v
    } else if written.is_file() {
        vec![written.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// What the Android Share tab can write. One value per row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Still(crate::export::ExportFormat),
    Anim(crate::animation_ui::AnimFormat),
    Glb,
    Usdz,
    Textures,
}

impl ExportKind {
    /// The file (or folder) name for a drawing called `title`, and whether it is a folder.
    pub fn file_name(self, title: &str) -> (String, bool) {
        match self {
            Self::Still(f) if f.is_folder() => (format!("{} layers", crate::saf::export_file_name(title, "")), true),
            Self::Still(f) => (crate::saf::export_file_name(title, f.extension()), false),
            Self::Anim(f) => (crate::saf::export_file_name(title, f.extension()), false),
            Self::Glb => (crate::saf::export_file_name(title, "glb"), false),
            Self::Usdz => (crate::saf::export_file_name(title, "usdz"), false),
            Self::Textures => (format!("{} textures", crate::saf::export_file_name(title, "")), true),
        }
    }

    /// The pick that asks where this export is saved.
    pub fn purpose(self) -> crate::file_access::PickPurpose {
        use crate::file_access::PickPurpose as P;
        match self {
            Self::Still(f) if f.is_folder() => P::ExportFolder,
            Self::Still(f) => P::SaveExport(f),
            Self::Anim(f) => P::SaveAnim(f),
            Self::Glb => P::Model3d,
            Self::Usdz => P::Usdz,
            Self::Textures => P::ExportFolder,
        }
    }
}

impl EskizApp {
    /// Android: start the export of `kind`. In Share mode it is written to the share cache and the
    /// system sheet opens; in Save mode the file backend is asked where it goes first, and
    /// `route_pick` starts the export when it answers.
    pub(crate) fn begin_staged(&mut self, ctx: &eframe::egui::Context, kind: ExportKind) {
        let title = self.export_title();
        let (name, _) = kind.file_name(&title);
        match self.share_mode {
            ShareMode::Share => match self.files.share_path(&name) {
                Some(path) => self.run_export(ctx, kind, path, Delivery::Share),
                None => self.show_toast(crate::file_access::NO_SHARE, 3.0),
            },
            ShareMode::SaveToFolder => self.request_pick(kind.purpose(), Some(name), None),
        }
    }

    /// Start the export of `kind` into `path` (a file, or a folder for the folder kinds).
    pub(crate) fn run_export(&mut self, ctx: &eframe::egui::Context, kind: ExportKind, path: PathBuf, delivery: Delivery) {
        match kind {
            ExportKind::Still(f) => self.start_export_to(ctx, f, path, delivery),
            ExportKind::Anim(f) => self.start_anim_export_to(ctx, f, path, delivery),
            ExportKind::Glb => self.start_glb_export(path, delivery),
            ExportKind::Usdz => self.start_usdz_export(path, delivery),
            ExportKind::Textures => {
                let _ = std::fs::create_dir_all(&path);
                self.start_texture_export(path, delivery);
            }
        }
    }
}
