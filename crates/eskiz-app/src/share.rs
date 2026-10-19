//! Background jobs behind the Actions panel: exporting the document (Share tab) and importing
//! pictures or layered files (Add tab and drag-and-drop). Both run on a worker thread so the
//! canvas stays responsive; the result is polled once per frame and reported with a toast.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use eframe::egui;

use crate::app::EskizApp;
use crate::deliver::Delivery;
use crate::export::{export_with, expand_home, ExportFormat, ExportSettings};
use crate::import::{decode_image, fit_premul, fit_to_canvas, is_layered_file, read_layers};

/// A running export.
struct ExportJob {
    rx: Receiver<Result<PathBuf, String>>,
    format: ExportFormat,
    delivery: Delivery,
}

/// Layers to add above the active one, bottom to top: each name with canvas-sized premultiplied
/// pixels.
type Layers = Vec<(String, Vec<u8>)>;

/// A running import: the name used in toasts, and the layers when they are ready.
struct ImportJob {
    rx: Receiver<Result<Layers, String>>,
    label: String,
}

/// Jobs in flight (at most one of each kind), and the Share sheet's options.
#[derive(Default)]
pub struct ShareState {
    export: Option<ExportJob>,
    import: Option<ImportJob>,
    /// The options on the Share sheet: scale, transparency, DPI, JPEG quality, PDF pages.
    pub settings: ExportSettings,
}

impl ShareState {
    pub fn busy(&self) -> bool {
        self.export.is_some() || self.import.is_some()
    }

    /// The format being written, while an export runs.
    pub fn exporting(&self) -> Option<ExportFormat> {
        self.export.as_ref().map(|job| job.format)
    }
}

impl EskizApp {
    /// Is an export or an import still running?
    pub fn share_busy(&self) -> bool {
        self.share.busy()
    }

    /// Export the document to `export_path` in `format` with the Share sheet's settings, off the
    /// UI thread. A toast reports the file written, or the error. On Android the export goes to a
    /// staging path and is saved or shared (see [`crate::deliver`]).
    pub(crate) fn start_export(&mut self, ctx: &egui::Context, format: ExportFormat) {
        if self.share.export.is_some() {
            self.show_toast("An export is already running.", 2.0);
            return;
        }
        if self.files.staged() {
            self.start_export_staged(ctx, format);
            return;
        }
        if self.export_path.trim().is_empty() {
            self.show_toast("Type a file path to export to.", 2.0);
            return;
        }
        let path = expand_home(&self.export_path);
        self.start_export_to(ctx, format, path, Delivery::Local);
    }

    /// Android: write the export to the share cache, or ask where to save it first.
    fn start_export_staged(&mut self, ctx: &egui::Context, format: ExportFormat) {
        if let Err(e) = self.share.settings.validate() {
            self.show_toast(e, 3.0);
            return;
        }
        self.begin_staged(ctx, crate::deliver::ExportKind::Still(format));
    }

    /// Write `format` to `path` in the background, then deliver it as `delivery` says.
    pub(crate) fn start_export_to(&mut self, ctx: &egui::Context, format: ExportFormat, path: PathBuf, delivery: Delivery) {
        if self.share.export.is_some() {
            self.show_toast("An export is already running.", 2.0);
            return;
        }
        if let Err(e) = self.share.settings.validate() {
            self.show_toast(e, 3.0);
            return;
        }
        // A snapshot, so painting can carry on while the file is written.
        let doc = self.session.doc.clone();
        let settings = self.share.settings.clone();
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new().name("eskiz-export".into()).spawn(move || {
            let _ = tx.send(export_with(&doc, format, &path, &settings));
            repaint.request_repaint();
        });
        if spawned.is_err() {
            self.show_toast("Could not start the export.", 2.5);
            return;
        }
        self.share.export = Some(ExportJob { rx, format, delivery });
        self.show_toast(format!("Exporting {}...", format.label()), 30.0);
    }

    /// The title the open drawing is saved under, for file names.
    pub(crate) fn export_title(&self) -> String {
        self.gal.current.as_ref().map_or_else(|| "Eskiz".to_owned(), |c| c.title.clone())
    }

    /// Import a picture (PNG, JPEG or WebP) as a new layer above the active one, or a PSD or
    /// `.procreate` file as one new layer per layer in it. Runs off the UI thread.
    pub(crate) fn start_import_image(&mut self, ctx: &egui::Context, path: &Path) {
        if crate::view3d::is_model_file(path) {
            self.start_model_import(ctx, path.to_path_buf());
            return;
        }
        if self.three_d.is_some() {
            self.show_toast("Leave the 3D view to add pictures to a 2D canvas.", 3.0);
            return;
        }
        if self.adjust.is_some() {
            self.show_toast("Finish the adjustment first.", 2.0);
            return;
        }
        if self.share.import.is_some() {
            self.show_toast("An import is already running.", 2.0);
            return;
        }
        let path = path.to_path_buf();
        let (cw, ch) = (self.session.doc.width, self.session.doc.height);
        let label = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Image").to_owned();
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new().name("eskiz-import".into()).spawn(move || {
            let _ = tx.send(import_layers(&path, cw, ch));
            repaint.request_repaint();
        });
        if spawned.is_err() {
            self.show_toast("Could not start the import.", 2.5);
            return;
        }
        self.share.import = Some(ImportJob { rx, label });
        self.show_toast("Importing...", 30.0);
    }

    /// Collect finished jobs. Called once per frame.
    pub(crate) fn poll_share(&mut self) {
        if let Some(job) = self.share.export.as_ref() {
            match job.rx.try_recv() {
                Ok(result) => {
                    let label = job.format.label();
                    let delivery = job.delivery;
                    self.share.export = None;
                    match result {
                        Ok(path) => {
                            let text = format!("Exported {label} to {}", path.display());
                            self.deliver(&path, delivery, text, 3.5);
                        }
                        Err(e) => self.show_toast(format!("Export failed: {e}"), 4.0),
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.share.export = None;
                    self.show_toast("Export failed.", 3.0);
                }
            }
        }
        if let Some(job) = self.share.import.as_ref() {
            match job.rx.try_recv() {
                Ok(result) => {
                    let label = job.label.clone();
                    self.share.import = None;
                    match result {
                        Ok(layers) => self.finish_import(&label, layers),
                        Err(e) => self.show_toast(format!("Could not import: {e}"), 4.0),
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.share.import = None;
                    self.show_toast("Import failed.", 3.0);
                }
            }
        }
    }

    /// Add each layer above the active one, in order. Each is one undo step.
    fn finish_import(&mut self, label: &str, layers: Layers) {
        let mut added = 0;
        for (name, pixels) in layers {
            let Some(edit) = self.session.import_layer(Some(self.active), &name, pixels) else {
                self.show_toast("Could not add the layer right now.", 2.5);
                break;
            };
            if let Some(id) = edit.id {
                self.active = id;
            }
            self.apply_refresh(edit.refresh);
            added += 1;
        }
        match added {
            0 => {}
            1 => self.show_toast(format!("Imported {label} as a new layer"), 2.5),
            n => self.show_toast(format!("Imported {n} layers from {label}"), 2.5),
        }
    }
}

/// Read a picture or layered file for the document of `cw x ch` (worker thread).
fn import_layers(path: &Path, cw: usize, ch: usize) -> Result<Layers, String> {
    if is_layered_file(path) {
        let layers = read_layers(path)?;
        if layers.is_empty() {
            return Err(format!("{} has no layers", path.display()));
        }
        Ok(layers
            .into_iter()
            .enumerate()
            .map(|(i, layer)| {
                let name = if layer.name.trim().is_empty() { format!("Layer {}", i + 1) } else { layer.name };
                (name, fit_premul(&layer.pixels, layer.width, layer.height, cw, ch))
            })
            .collect())
    } else {
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Image").to_owned();
        let (w, h, rgba) = decode_image(path)?;
        Ok(vec![(name, fit_to_canvas(&rgba, w, h, cw, ch))])
    }
}
