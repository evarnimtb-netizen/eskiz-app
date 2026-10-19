//! The Back button, gesture and key (A4, design section 3.8).
//!
//! [`EskizApp::back_step`] says what Back would do right now, without doing it: the topmost
//! thing that can be closed, then the canvas (back to the gallery), then `None`, which means Back
//! belongs to the system and leaves the app. The Java side registers its back callback only while
//! this is `Some` (`setBackWanted`), so with nothing to close the system plays its own
//! back-to-home animation. [`EskizApp::system_back`] does the step.
//!
//! The order is the design's, with one change: the Layers panel's inner state (rename, drag,
//! menu, background picker) is closed before the panel, as Escape does, because those live inside
//! the popover.

use eframe::egui;

use crate::app::EskizApp;
use crate::gallery::Screen;
use crate::selection_tool::EditMode;

/// What Back closes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackStep {
    /// The welcome tour: skip it, as Escape does.
    Tour,
    /// Text being typed on the canvas: commit and close, as Escape does.
    TextEdit,
    /// The layer rename field, a lifted layer card, or the layer menu or colour picker.
    LayerInner,
    /// A sheet over the canvas (Gesture controls, About, shortcuts, copy menu), the Crop &
    /// Resize sheet, Canvas information or the User guide.
    Sheet,
    /// An open popover.
    Popover,
    /// Brush Studio.
    Studio,
    /// An adjustment, Liquify, a transform or the selection mode: cancel.
    Edit,
    /// The 3D view.
    ThreeD,
    /// The canvas: save and show the gallery.
    Canvas,
    /// The gallery is zooming into a drawing: Back waits for it (and is not passed to the system).
    Opening,
    /// A dialog, menu, swipe action, select mode or stack in the gallery.
    Gallery(GalleryBack),
}

/// The gallery's closable states, in the order Back closes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GalleryBack {
    Confirm,
    Menu,
    Revealed,
    Select,
    Stack,
}

impl BackStep {
    /// The word logged when Back does this step (`back: popover`, `back: gallery`, ...).
    pub fn log_name(self) -> &'static str {
        match self {
            Self::Tour => "tour",
            Self::TextEdit => "text",
            Self::LayerInner => "layers",
            Self::Sheet => "sheet",
            Self::Popover => "popover",
            Self::Studio => "studio",
            Self::Edit => "edit",
            Self::ThreeD => "3d",
            Self::Canvas => "gallery",
            Self::Opening => "opening",
            Self::Gallery(_) => "gallery-state",
        }
    }
}

/// Tells the platform whether Back has something to close (`setBackWanted` on Android).
pub trait BackBridge: Send + Sync {
    fn set_wanted(&self, wanted: bool);
}

/// Desktop: there is no system Back.
pub struct NoBack;

impl BackBridge for NoBack {
    fn set_wanted(&self, _wanted: bool) {}
}

/// Records the calls, for the host tests.
#[derive(Default)]
pub struct MockBack {
    calls: std::sync::Mutex<Vec<bool>>,
}

impl MockBack {
    pub fn calls(&self) -> Vec<bool> {
        self.calls.lock().expect("mock back").clone()
    }
}

impl BackBridge for MockBack {
    fn set_wanted(&self, wanted: bool) {
        self.calls.lock().expect("mock back").push(wanted);
    }
}

/// The bridge the app starts with: Java's back callback on Android, else [`NoBack`].
pub fn default_back_bridge() -> std::sync::Arc<dyn BackBridge> {
    #[cfg(all(target_os = "android", not(test)))]
    {
        std::sync::Arc::new(crate::android_bridge::AndroidBack)
    }
    #[cfg(not(all(target_os = "android", not(test))))]
    {
        std::sync::Arc::new(NoBack)
    }
}

impl EskizApp {
    /// What Back would close now, or `None` when Back should leave the app.
    pub fn back_step(&self) -> Option<BackStep> {
        if self.tour.step().is_some() {
            return Some(BackStep::Tour);
        }
        match &self.gal.screen {
            Screen::Opening(_) => return Some(BackStep::Opening),
            Screen::Gallery => return self.gal.back_step().map(BackStep::Gallery),
            Screen::Canvas => {}
        }
        if self.text.editing.is_some() && self.three_d.is_none() {
            return Some(BackStep::TextEdit);
        }
        if self.layers_ui.wants_escape() && self.popovers.is_open() {
            return Some(BackStep::LayerInner);
        }
        if self.sheet.is_some() || self.crop.is_some() || self.canvas_info_open || self.guide.open {
            return Some(BackStep::Sheet);
        }
        if self.popovers.is_open() {
            return Some(BackStep::Popover);
        }
        if self.studio.is_some() {
            return Some(BackStep::Studio);
        }
        if self.adjust.is_some() || self.liquify.is_some() || self.transform.is_some() || self.edit_mode != EditMode::Off {
            return Some(BackStep::Edit);
        }
        if self.three_d.is_some() {
            return Some(BackStep::ThreeD);
        }
        Some(BackStep::Canvas)
    }

    /// Do what Back does. Returns false when there was nothing to do (the system takes it).
    pub fn system_back(&mut self, ctx: &egui::Context) -> bool {
        let Some(step) = self.back_step() else { return false };
        crate::saf::log_line(&format!("back: {}", step.log_name()));
        match step {
            BackStep::Tour => self.tour.skip(),
            BackStep::TextEdit => self.end_text_edit(),
            BackStep::LayerInner => {
                let u = &mut self.layers_ui;
                if u.rename.is_some() {
                    u.rename = None;
                } else if u.drag.is_some() {
                    u.drag = None;
                } else {
                    u.menu = None;
                    u.picker = false;
                }
            }
            BackStep::Sheet => {
                if self.sheet.is_some() {
                    self.sheet = None;
                } else if self.crop.is_some() {
                    self.crop = None;
                } else if self.canvas_info_open {
                    self.canvas_info_open = false;
                } else {
                    self.guide.open = false;
                }
            }
            BackStep::Popover => self.popovers.close(),
            BackStep::Studio => self.close_studio(),
            BackStep::Edit => {
                // The same order as Escape: the adjustment first, then Liquify, then the
                // transform and selection.
                if self.adjust.is_some() {
                    self.adjust_cancel();
                } else if self.liquify.is_some() {
                    self.liquify_cancel();
                } else {
                    self.exit_edit_mode();
                }
            }
            BackStep::ThreeD => self.close_three_d(),
            BackStep::Canvas => self.open_gallery(ctx),
            BackStep::Opening => {}
            BackStep::Gallery(g) => self.gal.back_close(g),
        }
        ctx.request_repaint();
        true
    }
}
