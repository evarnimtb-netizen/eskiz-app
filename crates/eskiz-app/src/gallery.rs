//! The Gallery screen and the glue between the canvas and storage.
//!
//! * [`Screen`] says what the window shows: the canvas, the gallery, or the 250 ms eased zoom
//!   from a gallery card to the canvas.
//! * [`Gallery`] holds the library, the background [`Worker`], the card thumbnails and the
//!   selection state. The app keeps one.
//! * Autosave: the document is saved when you leave the canvas, when the app closes, and every
//!   30 s while it has unsaved changes. "Unsaved" is the session edit counter plus a signature
//!   of the layer properties, compared with their values at the last successful save.
//!
//! The gallery always uses the dark palette, in the light theme too (dark backdrop.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use eframe::egui::{
    self, epaint::{RectShape, Shadow}, text::LayoutJob, Align2, Color32, CornerRadius, FontId, Id, LayerId, Margin,
    Order, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureId, TextureOptions, Vec2,
};

use crate::app::{EskizApp, FIT_TOP};
use crate::chrome::{small_label, title_row};
use crate::document::Document;
use crate::export::{default_export_path, expand_home};
use crate::icons;
use crate::journal::{self, Planner, JOURNAL_SECS, STROKE_DEBOUNCE_SECS};
use crate::popover::{self, Notice, Panel, Visible};
use crate::stacks::{LibraryIndex, SortMode};
use crate::storage::{
    meta_signature, new_id, AutosaveClock, DocEntry, Done, Job, Library, SaveRequest, Worker, AUTOSAVE_SECS,
    MAX_SIDE,
};
use crate::theme::{self, Palette};

// Card, stack and drag code lives in the `view` child module.
mod view;

/// Height of the gallery header bar, points.
/// Same height as the canvas top bar, so the title sits on the same baseline in both screens.
pub const HEADER_H: f32 = crate::app::BAR_HEIGHT;
/// Card to canvas zoom, seconds.
pub const OPEN_SECS: f64 = 0.25;
/// Leftward drag, in points, that reveals a card's actions.
pub const SWIPE_PX: f32 = 40.0;
const SIDE_MARGIN: f32 = 40.0;
const GUTTER: f32 = 28.0;
const CARD_TEXT_H: f32 = 50.0;
const THUMB_LOADS_PER_FRAME: usize = 4;

/// What the window shows.
#[derive(Debug, Default)]
pub(crate) enum Screen {
    #[default]
    Canvas,
    Gallery,
    /// Zooming from a card to the canvas.
    Opening(Box<OpenAnim>),
}

/// A card-to-canvas zoom in progress. The document is applied when the zoom ends.
#[derive(Debug)]
pub(crate) struct OpenAnim {
    id: String,
    title: String,
    size: (usize, usize),
    from: Rect,
    start: f64,
    doc: Option<Box<Document>>,
    /// A just-created blank canvas (saved as soon as it opens).
    fresh: bool,
}

/// The document on the canvas, as storage sees it.
#[derive(Clone, Debug)]
pub(crate) struct CurrentDoc {
    /// `None` until the first save assigns an ID (a file opened from outside the library).
    pub id: Option<String>,
    pub title: String,
    /// Session edit counter at the last successful save; `None` when never saved.
    pub saved_edits: Option<u64>,
    pub saved_sig: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Menu {
    NewCanvas,
    Import,
    Sort,
}

struct ThumbSlot {
    modified: SystemTime,
    tex: Option<TextureHandle>,
    /// Pixel size of the thumbnail image.
    size: Vec2,
}

/// Where the header buttons were drawn last frame (for tests and popover anchors).
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeaderRects {
    pub select: Rect,
    pub import: Rect,
    pub plus: Rect,
    pub sort: Rect,
    pub stack: Rect,
    pub back: Rect,
    pub delete_stack: Rect,
    /// The "Gallery" title text (the gallery screen's header).
    pub title: Rect,
}

impl Default for HeaderRects {
    fn default() -> Self {
        Self {
            select: Rect::NOTHING,
            title: Rect::NOTHING,
            import: Rect::NOTHING,
            plus: Rect::NOTHING,
            sort: Rect::NOTHING,
            stack: Rect::NOTHING,
            back: Rect::NOTHING,
            delete_stack: Rect::NOTHING,
        }
    }
}

/// Gallery state kept by the app.
pub(crate) struct Gallery {
    pub screen: Screen,
    pub library: Option<Library>,
    worker: Option<Worker>,
    jobs_in_flight: usize,
    saves_in_flight: usize,
    /// Set when a job could not be queued because the storage thread has stopped.
    worker_lost: bool,
    /// The ID a document got at its first save (so its saved selections stay with it).
    pub(crate) assigned_id: Option<String>,
    pub current: Option<CurrentDoc>,
    clock: AutosaveClock,
    /// Seconds between autosaves (a field so tests can shorten it).
    pub autosave_secs: f64,
    pub entries: Vec<DocEntry>,
    thumbs: HashMap<String, ThumbSlot>,
    select: bool,
    selected: HashSet<String>,
    revealed: Option<String>,
    drag: Vec2,
    pub(crate) menu: Option<(Menu, Rect)>,
    menu_shown: Option<(Menu, Rect)>,
    custom_w: String,
    /// The colour space of the next new canvas, chosen in the New canvas popover.
    pub new_space: eskiz_core::ColourSpace,
    /// Where the colour-space choices were drawn last frame (for tests).
    pub space_rects: Vec<Rect>,
    /// The bits per channel of the next new canvas, chosen in the New canvas popover.
    pub new_depth: eskiz_core::Depth,
    /// Where the 8-bit / 16-bit choices were drawn last frame (for tests).
    pub depth_rects: Vec<Rect>,
    custom_h: String,
    import_path: String,
    /// The gallery item waiting for a folder pick to export into.
    pub(crate) share_target: Option<String>,
    /// Android: the PNG being exported goes to the share sheet when it is written.
    pub(crate) share_via_sheet: bool,
    confirm: Option<Vec<String>>,
    pub header: HeaderRects,
    pub card_rects: Vec<(String, Rect)>,
    /// Where the preset rows of the "New canvas" popover were drawn last frame.
    pub preset_rects: Vec<Rect>,
    /// Root order, stacks and sort mode, as stored in the library index.
    pub index: LibraryIndex,
    /// The stack on screen, if the gallery shows one.
    pub open_stack: Option<String>,
    /// A title being edited: item ID, the draft text, and whether focus is still to be asked for.
    renaming: Option<(String, String, bool)>,
    /// A press that may become a long-press lift: item ID, where it started, and when.
    press: Option<(String, Pos2, f64)>,
    /// The card lifted by a long press, which follows the pointer.
    lift: Option<String>,
    /// Size of the lifted card, points.
    lift_size: Vec2,
    /// The frame on which a drop happened. Its click and swipe are ignored.
    drop_frame: u64,
    /// A stack delete in progress: the stack, and whether the second confirmation is showing.
    stack_confirm: Option<(String, bool)>,
    /// The grid's area last frame (drops outside it leave a stack).
    pub body: Rect,
    /// The crash-recovery journal of the document on the canvas (see `journal`).
    pub journal: Option<JournalState>,
    /// Journals found at start-up that the user has not decided on yet, oldest first.
    pub offers: Vec<journal::Pending>,
    /// The offer being restored (its ID), while the worker rebuilds it.
    pub restoring: Option<String>,
    /// A rebuilt document, waiting for the next frame to be shown on the canvas.
    pub restored: Option<(String, String, Box<Document>)>,
    /// The recovery sheet's Discard and Restore buttons, last frame (for tests).
    pub recovery_buttons: (Rect, Rect),
}

/// The journal of one document: its planner and when it was last written.
pub(crate) struct JournalState {
    pub id: String,
    pub planner: Planner,
    /// Time of the last plan, seconds.
    pub last_plan: f64,
    /// Edit counter and metadata signature the last `changed_at` was seen with.
    pub key: (u64, u64),
    /// When the document first changed since the last plan (`None` when nothing changed).
    pub changed_at: Option<f64>,
}

impl JournalState {
    pub fn new(id: String, planner: Planner, now: f64) -> Self {
        Self { id, planner, last_plan: now, key: (u64::MAX, 0), changed_at: None }
    }
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            screen: Screen::Canvas,
            library: None,
            worker: None,
            jobs_in_flight: 0,
            saves_in_flight: 0,
            worker_lost: false,
            assigned_id: None,
            current: None,
            clock: AutosaveClock::default(),
            autosave_secs: AUTOSAVE_SECS,
            entries: Vec::new(),
            thumbs: HashMap::new(),
            select: false,
            selected: HashSet::new(),
            revealed: None,
            drag: Vec2::ZERO,
            menu: None,
            menu_shown: None,
            custom_w: "1600".to_owned(),
            new_space: eskiz_core::ColourSpace::default(),
            new_depth: eskiz_core::Depth::default(),
            depth_rects: Vec::new(),
            space_rects: Vec::new(),
            custom_h: "1200".to_owned(),
            import_path: String::new(),
            share_target: None,
            share_via_sheet: false,
            confirm: None,
            header: HeaderRects::default(),
            card_rects: Vec::new(),
            preset_rects: Vec::new(),
            index: LibraryIndex::default(),
            open_stack: None,
            renaming: None,
            press: None,
            lift: None,
            lift_size: Vec2::ZERO,
            drop_frame: u64::MAX,
            stack_confirm: None,
            body: Rect::NOTHING,
            journal: None,
            offers: Vec::new(),
            restoring: None,
            restored: None,
            recovery_buttons: (Rect::NOTHING, Rect::NOTHING),
        }
    }
}

impl Gallery {
    /// What Back closes on the gallery screen, in the order Escape uses (dialog, menu, swipe
    /// action, select mode) and then an open stack.
    pub(crate) fn back_step(&self) -> Option<crate::back::GalleryBack> {
        use crate::back::GalleryBack as G;
        if self.confirm.is_some() {
            Some(G::Confirm)
        } else if self.menu.is_some() {
            Some(G::Menu)
        } else if self.revealed.is_some() {
            Some(G::Revealed)
        } else if self.select {
            Some(G::Select)
        } else if self.open_stack.is_some() {
            Some(G::Stack)
        } else {
            None
        }
    }

    /// Turn select mode on (tests of Back).
    #[cfg(test)]
    pub(crate) fn enter_select_for_test(&mut self) {
        self.select = true;
    }

    /// Close one thing [`Self::back_step`] named.
    pub(crate) fn back_close(&mut self, step: crate::back::GalleryBack) {
        use crate::back::GalleryBack as G;
        match step {
            G::Confirm => self.confirm = None,
            G::Menu => self.menu = None,
            G::Revealed => self.revealed = None,
            G::Select => {
                self.select = false;
                self.selected.clear();
            }
            G::Stack => self.open_stack = None,
        }
    }
}

impl Gallery {
    /// Is the New canvas popover open? (tests)
    #[cfg(test)]
    pub(crate) fn new_canvas_open(&self) -> bool {
        matches!(self.menu, Some((Menu::NewCanvas, _)))
    }

    /// Use `library` and start the background worker, which wakes the UI when a job ends.
    pub fn attach(&mut self, ctx: &egui::Context, library: Library) {
        let c = ctx.clone();
        self.worker = Some(Worker::spawn(library.clone(), move || c.request_repaint()));
        self.offers = journal::scan(library.recovery_root(), &library);
        self.library = Some(library);
        self.refresh();
    }

    /// Send a job that produces no reply (so it does not count as in flight).
    fn send_quiet(&self, job: Job) {
        if let Some(w) = &self.worker {
            w.send(job);
        }
    }

    /// True while a background job (save, load, import, export) has not finished.
    pub fn busy(&self) -> bool {
        self.jobs_in_flight > 0
    }

    /// Reload the documents, and bring the library index in line with them (documents that are
    /// new go to the front of the root list, and missing ones are dropped from it).
    pub fn refresh(&mut self) {
        if let Some(lib) = &self.library {
            let (entries, index) = lib.sync_index();
            self.entries = entries;
            let docs: Vec<String> = self.entries.iter().map(|e| e.id.clone()).collect();
            self.index = index;
            if self.open_stack.as_ref().is_some_and(|s| !self.index.is_stack(s)) {
                self.open_stack = None;
            }
            let ids: HashSet<&str> = docs.iter().map(String::as_str).collect();
            self.thumbs.retain(|id, _| ids.contains(id.as_str()));
            self.selected.retain(|id| ids.contains(id.as_str()));
        }
    }

    fn send(&mut self, job: Job) {
        if let Some(w) = &self.worker {
            let is_save = matches!(job, Job::Save(_));
            if is_save {
                self.saves_in_flight += 1;
            }
            self.jobs_in_flight += 1;
            if !w.send(job) {
                // The storage thread is gone: nothing will answer, so do not wait for it.
                self.jobs_in_flight = self.jobs_in_flight.saturating_sub(1);
                if is_save {
                    self.saves_in_flight = self.saves_in_flight.saturating_sub(1);
                }
                self.worker_lost = true;
            }
        }
    }

    /// Wait (at most `limit`) until the worker has finished every queued job. `true` when it did.
    pub(crate) fn drain_worker(&self, limit: Duration) -> bool {
        let Some(w) = &self.worker else {
            return true;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        w.send(Job::Barrier(tx));
        rx.recv_timeout(limit).is_ok()
    }

    /// Whether the document on the canvas has changes that are not on disk.
    fn is_dirty(&self, doc: &Document, edits: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|c| c.saved_edits != Some(edits) || c.saved_sig != meta_signature(doc))
    }

    /// Wait for queued jobs and stop the worker (app exit), but give up after `budget` (the close must stay short). The
    /// storage thread keeps writing; the exit deadline waits for a save that is running.
    pub fn shutdown_within(&mut self, budget: Duration) -> bool {
        self.worker.as_mut().is_none_or(|w| w.finish_within(budget))
    }
}

// ------------------------------------------------------------------ pure helpers

/// Input diagnostics for live debugging. Off unless `ESKIZ_DEBUG_INPUT=1` is set; then each line
/// goes to stderr, tagged with the egui frame number.
fn debug_input() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::compat::env_os("ESKIZ_DEBUG_INPUT").is_some_and(|v| v == "1"))
}

/// Number of card columns for a window `width` points wide: 3 to 5.
pub fn columns_for(width: f32) -> usize {
    if width >= 1500.0 {
        5
    } else if width >= 1100.0 {
        4
    } else {
        3
    }
}

/// Smooth start and stop for the zoom.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where the canvas sits on screen right after it opens (the same fit the canvas screen uses).
pub fn canvas_fit_rect(screen: Rect, cw: f32, ch: f32) -> Rect {
    let inner = Rect::from_min_max(screen.min + Vec2::new(16.0, FIT_TOP), screen.max - Vec2::new(16.0, 16.0));
    let zoom = ((inner.width() / cw).min(inner.height() / ch) * 0.95).clamp(0.01, 64.0);
    let size = Vec2::new(cw, ch) * zoom;
    Rect::from_min_size(inner.min + (inner.size() - size) / 2.0, size)
}

/// The part of a `tw` x `th` image shown in a 4:3 frame, cropped from the centre, as UVs.
pub fn crop_uv_4x3(tw: f32, th: f32) -> Rect {
    let aspect = tw / th.max(1.0);
    let frame = 4.0 / 3.0;
    let (u, v) = if aspect > frame { (frame / aspect, 1.0) } else { (1.0, aspect / frame) };
    Rect::from_min_max(Pos2::new((1.0 - u) / 2.0, (1.0 - v) / 2.0), Pos2::new((1.0 + u) / 2.0, (1.0 + v) / 2.0))
}

/// "Untitled", "Untitled 2", ... the first name not used by `entries`.
pub fn next_untitled(entries: &[DocEntry]) -> String {
    let taken: HashSet<&str> = entries.iter().map(|e| e.title.as_str()).collect();
    if !taken.contains("Untitled") {
        return "Untitled".to_owned();
    }
    (2..).map(|n| format!("Untitled {n}")).find(|t| !taken.contains(t.as_str())).unwrap_or_default()
}

/// The canvas sizes the "+" popover offers, as (name, width, height). Screen size comes first
/// and is filled in from the window.
pub fn size_presets(screen: (usize, usize)) -> [(&'static str, usize, usize); 4] {
    [
        ("Screen size", screen.0, screen.1),
        ("Square 2048", 2048, 2048),
        ("4K", 3840, 2160),
        ("A4 300 dpi", 2480, 3508),
    ]
}

/// Parse a custom size. Both sides must be whole numbers from 1 to [`MAX_SIDE`], and the area
/// at most 64 megapixels.
pub fn parse_custom_size(w: &str, h: &str) -> Result<(usize, usize), String> {
    let side = |s: &str| s.trim().parse::<usize>().map_err(|_| format!("'{}' is not a whole number", s.trim()));
    let (w, h) = (side(w)?, side(h)?);
    if !(1..=MAX_SIDE).contains(&w) || !(1..=MAX_SIDE).contains(&h) {
        return Err(format!("Each side must be between 1 and {MAX_SIDE} px"));
    }
    if w * h > 64_000_000 {
        return Err("That canvas is larger than 64 megapixels".to_owned());
    }
    Ok((w, h))
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min.lerp(b.min, t), a.max.lerp(b.max, t))
}

// ------------------------------------------------------------------ app glue

impl EskizApp {
    /// An app that starts on the Gallery, with documents stored in `library`.
    pub fn with_library(ctx: &egui::Context, prefs: crate::app::Prefs, library: Library) -> Self {
        let mut app = Self::new(ctx, Document::new(2048, 1536), String::new(), prefs);
        app.gal.attach(ctx, library);
        app.gal.screen = Screen::Gallery;
        app.gal.current = None;
        app
    }

    pub(crate) fn gallery_active(&self) -> bool {
        !matches!(self.gal.screen, Screen::Canvas)
    }

    /// Make sure there is a library and a worker (the default location if none was set).
    pub(crate) fn ensure_storage(&mut self, ctx: &egui::Context) {
        if self.gal.library.is_none() {
            self.gal.attach(ctx, Library::default_location());
        }
    }

    /// Register the document on the canvas as one that came from outside the library.
    pub(crate) fn set_external_document(&mut self, title: String) {
        let edits = self.session.edits();
        self.gal.current = Some(CurrentDoc {
            id: None,
            title,
            saved_edits: Some(edits),
            saved_sig: meta_signature(&self.session.doc),
        });
    }

    /// Whether the canvas has unsaved changes.
    pub(crate) fn is_dirty(&self) -> bool {
        self.gal.is_dirty(&self.session.doc, self.session.edits())
    }

    /// Queue a background save of the current document. The snapshot is taken now.
    fn queue_save(&mut self) {
        if self.session.is_stroking() {
            // The layer holds a half-drawn stroke: the autosave clock saves once the pen lifts.
            crate::stroke_trace::event("ignored", "save while a stroke is open (deferred)");
            return;
        }
        let (edits, sig) = (self.session.edits(), meta_signature(&self.session.doc));
        let Some(cur) = self.gal.current.as_mut() else {
            return;
        };
        let fresh = cur.id.is_none();
        let id = cur.id.get_or_insert_with(new_id).clone();
        if fresh {
            self.gal.assigned_id = Some(id.clone());
        }
        let req = SaveRequest { id, title: cur.title.clone(), doc: Box::new(self.session.doc.clone()), edits, sig };
        self.gal.send(Job::Save(req));
    }

    /// Tap on "Gallery": save the document if it changed, then show the gallery.
    pub(crate) fn open_gallery(&mut self, ctx: &egui::Context) {
        self.ensure_storage(ctx);
        self.close_three_d();
        self.commit_stroke("leaving the canvas for the gallery");
        self.popovers.close();
        if self.is_dirty() {
            self.queue_save();
        }
        self.gal.select = false;
        self.gal.selected.clear();
        self.gal.revealed = None;
        self.gal.menu = None;
        self.gal.open_stack = None;
        self.gal.refresh();
        self.gal.screen = Screen::Gallery;
    }

    /// The welcome tour's "Open sample": the sample document is saved in the library as a normal
    /// document (or the one saved earlier is reused) and opened with the usual zoom. Returns
    /// false when it could not be saved or read.
    pub(crate) fn open_tour_sample(&mut self, ctx: &egui::Context) -> bool {
        self.ensure_storage(ctx);
        let Some(lib) = self.gal.library.clone() else {
            return false;
        };
        let title = crate::onboarding::SAMPLE_TITLE.to_owned();
        let earlier = self.gal.entries.iter().find(|e| e.title == title).map(|e| e.id.clone());
        let loaded = earlier.and_then(|id| lib.load(&id).ok().map(|doc| (id, doc)));
        let (id, doc) = match loaded {
            Some(found) => found,
            None => {
                let doc = crate::onboarding_sample::sample_document();
                let id = new_id();
                if lib.save(&id, &title, &doc).is_err() {
                    return false;
                }
                (id, doc)
            }
        };
        self.gal.refresh();
        let from = Rect::from_center_size(self.gal.body.center(), Vec2::new(160.0, 100.0));
        self.gal.screen = Screen::Opening(Box::new(OpenAnim {
            id,
            title,
            size: (doc.width, doc.height),
            from,
            start: self.now,
            doc: Some(Box::new(doc)),
            fresh: false,
        }));
        true
    }

    /// Show a rebuilt document on the canvas. It is saved at once, which removes its journal.
    fn show_restored(&mut self, ctx: &egui::Context, id: String, title: String, doc: Document) {
        self.cancel_stroke();
        self.session.replace_document(doc);
        self.active = self.session.valid_active(None);
        self.need_fit = true;
        self.sync_full(ctx);
        self.popovers.close();
        self.gal.current = Some(CurrentDoc {
            id: Some(id.clone()),
            title,
            saved_edits: None,
            saved_sig: meta_signature(&self.session.doc),
        });
        self.gal.journal = Some(JournalState::new(id, Planner::unsaved(), self.now));
        self.gal.clock = AutosaveClock::default();
        self.gal.screen = Screen::Canvas;
        self.queue_save();
    }

    /// Per frame: show a rebuilt document, and write the journal of a dirty document. A journal
    /// is written at most every [`JOURNAL_SECS`] seconds, and once a stroke has been quiet for
    /// [`STROKE_DEBOUNCE_SECS`]. Nothing is written while a save is in flight or a stroke is
    /// being drawn.
    pub(crate) fn journal_tick(&mut self, ctx: &egui::Context) {
        if let Some((id, title, doc)) = self.gal.restored.take() {
            self.show_restored(ctx, id, title, *doc);
        }
        if !matches!(self.gal.screen, Screen::Canvas | Screen::Gallery) || self.gal.library.is_none() || self.three_d.is_some() {
            return;
        }
        let Some(cur) = self.gal.current.clone() else {
            return;
        };
        let Some(id) = cur.id.clone() else {
            return;
        };
        let dirty = self.is_dirty();
        let (edits, sig) = (self.session.edits(), meta_signature(&self.session.doc));
        let now = self.now;
        let stroking = self.session.is_stroking();
        let saving = self.gal.saves_in_flight > 0;
        let root = self.gal.library.as_ref().map(|l| l.recovery_root().to_owned()).unwrap_or_default();
        let Some(js) = self.gal.journal.as_mut().filter(|j| j.id == id) else {
            return;
        };
        if js.key != (edits, sig) {
            js.key = (edits, sig);
            js.changed_at = Some(now);
        }
        if !dirty || stroking || saving {
            return;
        }
        let Some(changed) = js.changed_at else {
            return;
        };
        let quiet = now - changed >= STROKE_DEBOUNCE_SECS && now - js.last_plan >= STROKE_DEBOUNCE_SECS;
        let periodic = now - js.last_plan >= JOURNAL_SECS;
        if !(quiet || periodic) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        let plan = js.planner.plan(&self.session.doc, &root, &id, &cur.title);
        js.last_plan = now;
        js.changed_at = None;
        journal::set_panic_snapshot(&root, &id, &cur.title, &self.session.doc);
        self.gal.send(Job::Journal(plan));
    }

    /// The start-up sheet: a journal newer than its package is offered for restore. The sheet
    /// is modal: nothing underneath can be used until the user decides.
    pub(crate) fn recovery_sheet(&mut self, ctx: &egui::Context) {
        let Some(offer) = self.gal.offers.first().cloned() else {
            return;
        };
        let gp = theme::palette_for(self.prefs.identity, false);
        let busy = self.gal.restoring.as_deref() == Some(offer.id.as_str());
        let screen = ctx.content_rect();
        egui::Area::new(Id::new("recovery-scrim"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(150));
            });
        let mut choice = None;
        let mut buttons = (Rect::NOTHING, Rect::NOTHING);
        egui::Area::new(Id::new("recovery-sheet"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                theme::panel_frame(&gp).inner_margin(Margin::same(20)).show(ui, |ui| {
                    dark_widgets(ui, &gp);
                    ui.set_width(340.0);
                    let when = journal::when(offer.saved_at);
                    let text = format!("Eskiz quit unexpectedly. Restore {} from {when}?", offer.title);
                    ui.label(RichText::new(text).size(17.0).strong().color(gp.text));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Work since the last save is in a recovery copy.").size(13.0).color(gp.text_dim));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 8.0) / 2.0;
                        let discard = ui.add_enabled(!busy, egui::Button::new("Discard").min_size(Vec2::new(w, 36.0)));
                        let restore = ui.add_enabled(!busy, egui::Button::new("Restore").min_size(Vec2::new(w, 36.0)));
                        buttons = (discard.rect, restore.rect);
                        if discard.clicked() {
                            choice = Some(false);
                        }
                        if restore.clicked() {
                            choice = Some(true);
                        }
                    });
                });
            });
        self.gal.recovery_buttons = buttons;
        match choice {
            Some(true) => {
                self.gal.restoring = Some(offer.id.clone());
                self.gal.send(Job::Restore { id: offer.id, title: offer.title });
            }
            Some(false) => {
                self.gal.offers.retain(|o| o.id != offer.id);
                self.gal.send_quiet(Job::RemoveJournal(offer.id));
            }
            None => {}
        }
    }

    /// Autosave timer, once per frame while the canvas (or, after a failed save, the Gallery)
    /// is showing. A document that is still dirty is saved again every interval until it works.
    pub(crate) fn autosave_tick(&mut self, ctx: &egui::Context) {
        if !matches!(self.gal.screen, Screen::Canvas | Screen::Gallery) || self.gal.library.is_none() || self.three_d.is_some() {
            return;
        }
        let dirty = self.is_dirty();
        let busy = self.gal.saves_in_flight > 0 || self.session.is_stroking();
        let secs = self.gal.autosave_secs;
        if self.gal.clock.update(dirty, busy, self.now, secs) {
            self.queue_save();
        }
        if dirty {
            let wait = self.gal.clock.remaining(self.now, secs).unwrap_or(secs).max(0.25);
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }
    }

    /// Called when the app closes (and by eframe's periodic save): write unsaved changes.
    pub(crate) fn save_on_exit(&mut self) {
        self.save_three_d_now();
        if matches!(self.gal.screen, Screen::Canvas) && self.gal.library.is_some() && self.is_dirty() {
            self.commit_stroke("the app is saving on exit");
            self.queue_save();
        }
    }

    /// App exit: queue the final save, wait for the worker to finish everything.
    pub(crate) fn finish_storage(&mut self) {
        crate::exit::step("saving the document");
        self.save_on_exit();
        crate::exit::step("waiting for the storage worker");
        let ok = self.gal.shutdown_within(crate::exit::GRACE);
        crate::exit::step(if ok { "storage worker finished" } else { "storage worker still busy; the deadline waits for the save" });
    }

    /// Apply finished background jobs.
    pub(crate) fn poll_worker(&mut self) {
        if std::mem::take(&mut self.gal.worker_lost) {
            self.show_toast("Saving has stopped working. Please restart Eskiz; your work is kept on screen.", 6.0);
        }
        let done: Vec<Done> = match &self.gal.worker {
            Some(w) => w.rx.try_iter().collect(),
            None => return,
        };
        for d in done {
            self.gal.jobs_in_flight = self.gal.jobs_in_flight.saturating_sub(1);
            match d {
                Done::Saved { id, edits, sig, result, doc } => {
                    self.gal.saves_in_flight = self.gal.saves_in_flight.saturating_sub(1);
                    match result {
                        Ok(_) => {
                            if let Some(cur) = self.gal.current.as_mut().filter(|c| c.id.as_deref() == Some(id.as_str())) {
                                cur.saved_edits = Some(edits);
                                cur.saved_sig = sig;
                            }
                            self.gal.thumbs.remove(&id);
                            self.gal.refresh();
                            // The package now holds this snapshot, so its journal goes, and the
                            // journal starts again from it. Snapshots are not planned while a save
                            // is in flight, so no earlier snapshot can land after this removal.
                            self.gal.send_quiet(Job::RemoveJournal(id.clone()));
                            if self.gal.current.as_ref().is_some_and(|c| c.id.as_deref() == Some(id.as_str())) {
                                let now = self.now;
                                let fits = self.gal.journal.as_ref().is_some_and(|j| j.id == id);
                                if fits {
                                    if let Some(js) = self.gal.journal.as_mut() {
                                        js.planner.rebase(&doc);
                                    }
                                } else {
                                    self.gal.journal = Some(JournalState::new(id.clone(), Planner::based_on(&doc), now));
                                }
                                if self.is_dirty() {
                                    if let Some(js) = self.gal.journal.as_mut() {
                                        js.changed_at = Some(now);
                                    }
                                }
                            }
                        }
                        Err(e) => self.show_toast(format!("Could not save: {e}. Your work is kept; it will be saved again."), 4.0),
                    }
                }
                Done::Journal { id, result } => {
                    if let Err(e) = result {
                        if let Some(js) = self.gal.journal.as_mut().filter(|j| j.id == id) {
                            js.planner.mark_broken();
                        }
                        self.show_toast(format!("Recovery copy not written: {e}"), 4.0);
                    }
                }
                Done::Restored { id, title, result, skipped } => {
                    self.gal.restoring = None;
                    match result {
                        Ok(doc) => {
                            if skipped > 0 {
                                self.show_toast(format!("{skipped} part(s) of the recovery copy could not be read and were left out."), 5.0);
                            }
                            self.gal.offers.retain(|o| o.id != id);
                            self.gal.restored = Some((id, title, doc));
                        }
                        Err(e) => self.show_toast(format!("Could not restore {title}: {e}"), 4.0),
                    }
                }
                Done::Loaded { id, result } => {
                    let mut failed = None;
                    if let Screen::Opening(anim) = &mut self.gal.screen {
                        if anim.id == id {
                            match result {
                                Ok(doc) => anim.doc = Some(doc),
                                Err(e) => failed = Some(e),
                            }
                        }
                    }
                    if let Some(e) = failed {
                        self.gal.screen = Screen::Gallery;
                        self.show_toast(format!("Could not open: {e}"), 4.0);
                    }
                }
                Done::Imported(Ok(entry)) => {
                    self.gal.refresh();
                    self.show_toast(format!("Imported {}", entry.title), 2.5);
                }
                Done::Imported(Err(e)) => self.show_toast(format!("Could not import: {e}"), 4.0),
                Done::Exported(Ok(path)) => {
                    if std::mem::take(&mut self.gal.share_via_sheet) {
                        self.share_files(&path);
                    } else {
                        self.deliver(&path, crate::deliver::Delivery::Local, format!("Exported {}", path.display()), 3.0);
                    }
                }
                Done::Exported(Err(e)) => {
                    self.gal.share_via_sheet = false;
                    self.show_toast(format!("Export failed: {e}"), 4.0);
                }
            }
        }
    }

    /// Files dropped on the window while the gallery shows: import them.
    pub(crate) fn import_into_gallery(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        self.ensure_storage(ctx);
        for p in paths {
            if crate::view3d::is_model_file(&p) {
                self.start_model_import(ctx, p);
            } else {
                self.gal.send(Job::Import(p));
            }
        }
    }

    /// Export the item waiting in `gal.share_target` as a PNG into `dir`, or into the default
    /// export folder when `dir` is `None`.
    pub(crate) fn gallery_share_to(&mut self, dir: Option<PathBuf>) {
        let Some(id) = self.gal.share_target.take() else {
            return;
        };
        let dir = dir.unwrap_or_else(|| default_export_path().parent().map_or_else(|| PathBuf::from("."), PathBuf::from));
        self.gal.send(Job::Export { id, dir });
        self.show_toast("Exporting PNG...", 1.5);
    }

    /// Android: export the item waiting in `gal.share_target` as a PNG into the share cache and open the share sheet.
    pub(crate) fn gallery_share_sheet(&mut self) {
        let Some(path) = self.files.share_path("share.png") else {
            self.gallery_share_to(None);
            return;
        };
        let dir = path.parent().map_or_else(|| PathBuf::from("."), PathBuf::from);
        self.gal.share_via_sheet = true;
        self.gallery_share_to(Some(dir));
    }

    pub(crate) fn open_entry(&mut self, id: &str, from: Rect) {
        if self.gal.library.as_ref().is_some_and(|l| l.is_model_doc(id)) {
            self.start_model_open(id.to_owned());
            return;
        }
        let Some(e) = self.gal.entries.iter().find(|e| e.id == id).cloned() else {
            return;
        };
        self.gal.screen = Screen::Opening(Box::new(OpenAnim {
            id: e.id.clone(),
            title: e.title,
            size: (e.width, e.height),
            from,
            start: self.now,
            doc: None,
            fresh: false,
        }));
        self.gal.send(Job::Load(e.id));
    }

    pub(crate) fn new_canvas(&mut self, frame: u64, w: usize, h: usize, from: Rect) {
        if debug_input() {
            eprintln!("[input f{frame}] new_canvas {w}x{h} from={from:?}");
        }
        let title = next_untitled(&self.gal.entries);
        self.gal.screen = Screen::Opening(Box::new(OpenAnim {
            id: new_id(),
            title,
            size: (w, h),
            from,
            start: self.now,
            doc: Some(Box::new({
                let mut doc = Document::new_depth(w, h, self.gal.new_depth);
                doc.colour_space = self.gal.new_space;
                doc
            })),
            fresh: true,
        }));
    }

    /// The zoom has ended and the document is ready: switch to the canvas.
    fn finish_open(&mut self, ctx: &egui::Context, anim: OpenAnim) {
        let Some(doc) = anim.doc else {
            return;
        };
        self.cancel_stroke();
        self.session.replace_document(*doc);
        self.active = self.session.valid_active(None);
        self.need_fit = true;
        self.sync_full(ctx);
        self.popovers.close();
        let edits = self.session.edits();
        self.gal.current = Some(CurrentDoc {
            id: Some(anim.id),
            title: anim.title,
            saved_edits: if anim.fresh { None } else { Some(edits) },
            saved_sig: meta_signature(&self.session.doc),
        });
        self.gal.clock = AutosaveClock::default();
        self.gal.screen = Screen::Canvas;
        // The journal is relative to the package the document came from (or none, if new).
        let planner = if anim.fresh { Planner::unsaved() } else { Planner::based_on(&self.session.doc) };
        if let Some(id) = self.gal.current.as_ref().and_then(|c| c.id.clone()) {
            self.gal.journal = Some(JournalState::new(id, planner, self.now));
        }
        if anim.fresh {
            // Creating a document writes its package right away.
            self.queue_save();
        }
    }

    fn delete_entries(&mut self, ids: &[String]) {
        let Some(lib) = self.gal.library.clone() else {
            return;
        };
        let mut failed = 0;
        for id in ids {
            if lib.delete(id).is_err() {
                failed += 1;
            }
            if self.gal.current.as_ref().and_then(|c| c.id.as_deref()) == Some(id.as_str()) {
                self.gal.current = None;
            }
        }
        self.gal.selected.clear();
        self.gal.select = false;
        self.gal.refresh();
        if failed > 0 {
            self.show_toast(format!("Could not delete {failed} document(s)"), 3.0);
        }
    }

    // -------------------------------------------------------------- drawing

    /// Draw the whole gallery screen for this frame.
    pub(crate) fn gallery_root(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let gp = theme::palette_for(self.prefs.identity, false);
        let interactive = matches!(self.gal.screen, Screen::Gallery) && !self.tour.is_modal();
        if interactive && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.gal.confirm.is_some() {
                self.gal.confirm = None;
            } else if self.gal.menu.is_some() {
                self.gal.menu = None;
            } else if self.gal.revealed.is_some() {
                self.gal.revealed = None;
            } else {
                self.gal.select = false;
                self.gal.selected.clear();
            }
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(gp.backdrop))
            .show(ui, |ui| self.gallery_content(ui, &gp, interactive));
        if interactive {
            self.menu_popover(&ctx, &gp);
            self.confirm_dialog(&ctx, &gp);
            self.stack_confirm_dialog(&ctx, &gp);
        }
        self.opening_frame(&ctx);
        self.toast_ui(&ctx);
    }

    fn gallery_content(&mut self, ui: &mut egui::Ui, gp: &Palette, interactive: bool) {
        let ctx = ui.ctx().clone();
        // The panel may cover the whole window: keep the header and the grid clear of the system
        // bars, and paint the strips under them in the header colour (as the canvas screen does).
        let screen = ui.max_rect().intersect(ctx.content_rect());
        if !self.safe.is_zero() {
            let layer = egui::LayerId::new(egui::Order::Background, Id::new("gallery-safe-area"));
            let p = egui::Painter::new(ctx.clone(), layer, ctx.viewport_rect());
            for r in self.safe.strips(ctx.viewport_rect()) {
                p.rect_filled(r, egui::CornerRadius::ZERO, gp.bar);
            }
        }
        let header = Rect::from_min_size(screen.min, Vec2::new(screen.width(), HEADER_H));
        self.header_bar(ui, gp, header, interactive);

        let body = Rect::from_min_max(header.left_bottom(), screen.max);
        self.gal.body = body;
        if interactive {
            self.lift_frame(&ctx);
        }
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
        let mut actions = Vec::new();
        let scroll_id = self.gal.open_stack.clone().unwrap_or_default();
        egui::ScrollArea::vertical().id_salt(("gallery-grid", scroll_id)).auto_shrink(false).show(&mut child, |ui| {
            if self.gal.entries.is_empty() {
                let tile = self.empty_state(ui, gp, body);
                if interactive && ui.interact(body, Id::new("gallery-bg"), Sense::click()).clicked() {
                    actions.push(Action::Background);
                }
                // The blank-canvas tile does what the + button does. Registered after the
                // background so it takes the click.
                if interactive && ui.interact(tile, Id::new("gallery-empty-tile"), Sense::click()).clicked() {
                    let anchor = self.gal.header.plus;
                    self.toggle_menu(Menu::NewCanvas, anchor);
                }
                return;
            }
            self.grid(ui, &ctx, gp, body, interactive, &mut actions);
        });
        for a in actions {
            self.apply_action(&ctx, a);
        }
    }

    // -------------------------------------------------------------- popovers

    /// The "New canvas" and "Import" popovers under their header buttons.
    fn menu_popover(&mut self, ctx: &egui::Context, gp: &Palette) {
        if let Some(m) = self.gal.menu {
            self.gal.menu_shown = Some(m);
        }
        let t = ctx.animate_bool_with_time(Id::new("gallery-menu"), self.gal.menu.is_some(), popover::popover_secs(self.reduce_motion()));
        let Some((menu, anchor)) = self.gal.menu_shown else {
            return;
        };
        if self.gal.menu.is_none() && t <= 0.001 {
            self.gal.menu_shown = None;
            return;
        }
        let frame = ctx.cumulative_frame_nr();
        if debug_input() {
            ctx.input(|i| {
                for e in &i.events {
                    if matches!(e, egui::Event::PointerButton { .. } | egui::Event::PointerMoved(_) | egui::Event::Touch { .. }) {
                        eprintln!("[input f{frame}] raw {e:?}");
                    }
                }
            });
        }
        let vis = Visible { panel: Panel::Notice(Notice::Gallery), anchor, ease: popover::ease_out(t), t };
        let mut choice: Option<(usize, usize)> = None;
        let mut import: Option<PathBuf> = None;
        let mut sort: Option<SortMode> = None;
        let screen_px = {
            let s = ctx.content_rect().size() * ctx.pixels_per_point();
            ((s.x.round() as usize).clamp(256, MAX_SIDE), (s.y.round() as usize).clamp(256, MAX_SIDE))
        };
        // The gallery has no tool sidebar: forget the canvas screen's clearance and floor.
        ctx.data_mut(|d| {
            d.remove::<(bool, f32)>(Id::new("sidebar-clear"));
            d.remove::<f32>(Id::new("canvas-bottom"));
        });
        let rect = popover::draw(ctx, gp, vis, |ui| {
            dark_widgets(ui, gp);
            match menu {
                Menu::NewCanvas => choice = self.new_canvas_body(ui, gp, screen_px),
                Menu::Import => import = self.import_body(ui, gp),
                Menu::Sort => sort = self.sort_body(ui, gp),
            }
        });
        // A press outside the popover and its button closes it.
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        if pressed && self.gal.menu.is_some() {
            let pos = ctx.input(|i| i.pointer.interact_pos());
            let origin = ctx.input(|i| i.pointer.press_origin());
            let closes = pos.is_some_and(|p| !rect.contains(p) && !anchor.expand(4.0).contains(p));
            if debug_input() {
                eprintln!(
                    "[input f{frame}] popover press: interact_pos={pos:?} press_origin={origin:?} rect={rect:?} anchor={anchor:?} closes={closes}"
                );
            }
            if closes {
                self.gal.menu = None;
            }
        }
        if let Some((w, h)) = choice {
            self.gal.menu = None;
            let from = self.gal.header.plus;
            self.new_canvas(frame, w, h, from);
        }
        if let Some(path) = import {
            self.gal.menu = None;
            self.import_into_gallery(ctx, vec![path]);
        }
        if let Some(mode) = sort {
            self.gal.menu = None;
            self.set_sort(mode);
        }
    }

    fn new_canvas_body(&mut self, ui: &mut egui::Ui, gp: &Palette, screen_px: (usize, usize)) -> Option<(usize, usize)> {
        title_row(ui, "New canvas");
        let mut chosen = None;
        self.gal.preset_rects.clear();
        for (name, w, h) in size_presets(screen_px) {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
            self.gal.preset_rects.push(rect);
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(8), gp.row);
            }
            ui.painter().text(rect.left_center() + Vec2::new(10.0, 0.0), Align2::LEFT_CENTER, name, FontId::proportional(14.0), gp.text);
            ui.painter().text(
                rect.right_center() - Vec2::new(10.0, 0.0),
                Align2::RIGHT_CENTER,
                format!("{w} × {h}"),
                FontId::proportional(12.0),
                gp.text_dim,
            );
            if debug_input() {
                let (down, up) = ui.input(|i| (i.pointer.any_pressed(), i.pointer.any_released()));
                if resp.hovered() || resp.clicked() || down || up {
                    eprintln!(
                        "[input f{}] row {name}: hovered={} clicked={} any_pressed={down} any_released={up}",
                        ui.ctx().cumulative_frame_nr(),
                        resp.hovered(),
                        resp.clicked()
                    );
                }
            }
            if resp.clicked() {
                chosen = Some((w, h));
            }
        }
        ui.add_space(2.0);
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, gp.separator));
        ui.add_space(8.0);
        small_label(ui, gp, "Color space");
        self.gal.space_rects.clear();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let width = (ui.available_width() - 6.0) / eskiz_core::ColourSpace::ALL.len() as f32;
            for space in eskiz_core::ColourSpace::ALL {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
                self.gal.space_rects.push(rect);
                let on = self.gal.new_space == space;
                let fill = if on { gp.accent_fill } else if resp.hovered() { gp.track } else { gp.row };
                ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
                let ink = if on { gp.on_accent } else { gp.text };
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, space.label(), FontId::proportional(13.0), ink);
                if resp.clicked() {
                    self.gal.new_space = space;
                }
            }
        });
        ui.add_space(8.0);
        small_label(ui, gp, "Color depth");
        self.gal.depth_rects.clear();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let width = (ui.available_width() - 6.0) / 2.0;
            for depth in [eskiz_core::Depth::U8, eskiz_core::Depth::U16] {
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click());
                self.gal.depth_rects.push(rect);
                let on = self.gal.new_depth == depth;
                let fill = if on { gp.accent_fill } else if resp.hovered() { gp.track } else { gp.row };
                ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
                let ink = if on { gp.on_accent } else { gp.text };
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, depth.label(), FontId::proportional(13.0), ink);
                if resp.clicked() {
                    self.gal.new_depth = depth;
                }
            }
        });
        ui.add_space(8.0);
        small_label(ui, gp, "Custom");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add(egui::TextEdit::singleline(&mut self.gal.custom_w).desired_width(62.0).hint_text("width"));
            ui.label(RichText::new("×").color(gp.text_dim));
            ui.add(egui::TextEdit::singleline(&mut self.gal.custom_h).desired_width(62.0).hint_text("height"));
            ui.label(RichText::new("px").size(11.0).color(gp.text_dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let create = egui::Button::new(RichText::new("Create").color(gp.on_primary)).fill(gp.primary);
                let create_resp = ui.add(create);
                if debug_input() {
                    let (down, up) = ui.input(|i| (i.pointer.any_pressed(), i.pointer.any_released()));
                    eprintln!(
                        "[input f{}] Create button: hovered={} clicked={} any_pressed={down} any_released={up}",
                        ui.ctx().cumulative_frame_nr(),
                        create_resp.hovered(),
                        create_resp.clicked()
                    );
                }
                if create_resp.clicked() {
                    match parse_custom_size(&self.gal.custom_w, &self.gal.custom_h) {
                        Ok(size) => chosen = Some(size),
                        Err(e) => self.show_toast(e, 3.0),
                    }
                }
            });
        });
        chosen
    }

    fn import_body(&mut self, ui: &mut egui::Ui, gp: &Palette) -> Option<PathBuf> {
        title_row(ui, "Import");
        small_label(ui, gp, "Path to a .procreate file");
        let mut go = false;
        let mut browse = false;
        let edit = ui.add(
            egui::TextEdit::singleline(&mut self.gal.import_path)
                .desired_width(ui.available_width())
                .hint_text("~/Pictures/art.procreate"),
        );
        if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            go = true;
        }
        ui.horizontal(|ui| {
            let b = egui::Button::new(RichText::new("Import").color(gp.on_primary)).fill(gp.primary);
            if ui.add(b).clicked() {
                go = true;
            }
            if ui.button("Browse...").clicked() {
                browse = true;
            }
            small_label(ui, gp, "or drop a file on the window");
        });
        if browse {
            let typed = self.gal.import_path.clone();
            self.request_pick_near(crate::file_access::PickPurpose::GalleryImport, &typed, None);
        }
        if go && !self.gal.import_path.trim().is_empty() {
            let path = expand_home(&self.gal.import_path);
            self.gal.import_path.clear();
            return Some(path);
        }
        None
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context, gp: &Palette) {
        let Some(ids) = self.gal.confirm.clone() else {
            return;
        };
        let screen = ctx.content_rect();
        let mut decision = None;
        egui::Area::new(Id::new("gallery-confirm-scrim"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(150));
                if resp.clicked() {
                    decision = Some(false);
                }
            });
        let title = if ids.len() == 1 {
            self.gal.entries.iter().find(|e| e.id == ids[0]).map_or("this artwork".to_owned(), |e| format!("\"{}\"", e.title))
        } else {
            format!("{} artworks", ids.len())
        };
        egui::Area::new(Id::new("gallery-confirm"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                theme::panel_frame(gp).inner_margin(Margin::same(20)).show(ui, |ui| {
                    dark_widgets(ui, gp);
                    ui.set_width(300.0);
                    ui.label(RichText::new(format!("Delete {title}?")).size(17.0).strong().color(gp.text));
                    ui.add_space(4.0);
                    ui.label(RichText::new("This cannot be undone.").size(13.0).color(gp.text_dim));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let w = (ui.available_width() - 8.0) / 2.0;
                        if ui.add_sized([w, 36.0], egui::Button::new("Cancel")).clicked() {
                            decision = Some(false);
                        }
                        let del = egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(gp.danger);
                        if ui.add_sized([w, 36.0], del).clicked() {
                            decision = Some(true);
                        }
                    });
                });
            });
        match decision {
            Some(true) => {
                self.gal.confirm = None;
                self.delete_entries(&ids);
            }
            Some(false) => self.gal.confirm = None,
            None => {}
        }
    }

    // -------------------------------------------------------------- the zoom

    /// Draw the card-to-canvas zoom, and switch to the canvas when it ends.
    fn opening_frame(&mut self, ctx: &egui::Context) {
        let Screen::Opening(anim) = &self.gal.screen else {
            return;
        };
        // Under reduced motion the zoom is already at its end, so the document opens at once.
        let t = if self.reduce_motion() {
            1.0
        } else {
            ((self.now - anim.start) / OPEN_SECS).clamp(0.0, 1.0) as f32
        };
        let e = ease_in_out(t);
        let screen = ctx.content_rect();
        let target = canvas_fit_rect(screen, anim.size.0 as f32, anim.size.1 as f32);
        let rect = lerp_rect(anim.from, target, e);
        let tex = self.gal.thumbs.get(&anim.id).and_then(|s| s.tex.as_ref().map(|t| (t.id(), s.size)));
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("gallery-open")));
        // The backdrop fades to the canvas backdrop colour as the art grows.
        painter.rect_filled(screen, CornerRadius::ZERO, self.pal.backdrop.gamma_multiply(e));
        let radius = CornerRadius::same((8.0 * (1.0 - e)).round() as u8);
        painter.add(Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(100) }.as_shape(rect, radius));
        match tex {
            Some((id, size)) => {
                let a = crop_uv_4x3(size.x, size.y);
                let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.add(RectShape::filled(rect, radius, Color32::WHITE).with_texture(id, lerp_rect(a, full, e)));
            }
            None => {
                painter.rect_filled(rect, radius, Color32::WHITE);
            }
        }
        if t >= 1.0 && matches!(&self.gal.screen, Screen::Opening(a) if a.doc.is_some()) {
            if let Screen::Opening(anim) = std::mem::take(&mut self.gal.screen) {
                self.finish_open(ctx, *anim);
            }
        }
        ctx.request_repaint();
    }
}

/// What a frame of the grid asks for. Applied after drawing, so drawing never mutates the list
/// it is iterating.
enum Action {
    Background,
    Reveal(Option<String>),
    Tap(String, Rect),
    Share(String),
    Duplicate(String),
    AskDelete(Vec<String>),
}

/// A right-aligned header text button. `x` is the right edge on entry and the left edge on exit.
#[allow(clippy::too_many_arguments)]
fn header_text(ui: &mut egui::Ui, gp: &Palette, x: &mut f32, cy: f32, label: &str, colour: Color32, id: &str, sense: Sense) -> (Rect, bool) {
    let font = FontId::proportional(15.0);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, colour);
    let size = Vec2::new(galley.size().x + 24.0, 36.0);
    let rect = Rect::from_min_size(Pos2::new(*x - size.x, cy - size.y / 2.0), size);
    let resp = ui.interact(rect, Id::new(id), sense);
    crate::a11y::named(&resp, egui::WidgetType::Button, sense != Sense::hover(), label);
    if resp.hovered() && sense != Sense::hover() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), gp.row);
    }
    ui.painter().galley(Pos2::new(rect.left() + 12.0, cy - galley.size().y / 2.0), galley, colour);
    *x = rect.left();
    (rect, resp.clicked())
}

/// The gallery is dark in the light theme too, so widgets inside its popovers and dialogs take
/// the dark palette instead of egui's light visuals.
pub(crate) fn dark_widgets(ui: &mut egui::Ui, gp: &Palette) {
    let v = ui.visuals_mut();
    v.dark_mode = true;
    v.override_text_color = Some(gp.text);
    v.extreme_bg_color = gp.row;
    v.faint_bg_color = gp.row;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.fg_stroke.color = gp.text;
    }
    v.widgets.inactive.bg_fill = gp.row;
    v.widgets.inactive.weak_bg_fill = gp.row;
    v.widgets.hovered.bg_fill = gp.separator;
    v.widgets.hovered.weak_bg_fill = gp.separator;
}

/// A single line of text that ends in an ellipsis when it does not fit `width`.
fn one_line(text: &str, size: f32, colour: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), colour);
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

/// Share: a tray with an arrow leaving it. Original glyph, same 24-unit grid as the icon set.
fn icon_share(p: &Painter, r: Rect, c: Color32) {
    let s = r.width().min(r.height()) / 24.0;
    let o = r.center() - Vec2::splat(12.0 * s);
    let pt = |x: f32, y: f32| o + Vec2::new(x, y) * s;
    let stroke = Stroke::new(1.75 * s, c);
    let line = |pts: &[(f32, f32)]| {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
        for w in v.windows(2) {
            p.line_segment([w[0], w[1]], stroke);
        }
        for q in [v[0], v[v.len() - 1]] {
            p.circle_filled(q, stroke.width * 0.5, c);
        }
    };
    line(&[(6.0, 11.0), (6.0, 20.0), (18.0, 20.0), (18.0, 11.0)]);
    line(&[(12.0, 15.0), (12.0, 3.5)]);
    line(&[(7.5, 8.0), (12.0, 3.5), (16.5, 8.0)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str) -> DocEntry {
        DocEntry { id: title.to_owned(), title: title.to_owned(), width: 1, height: 1, modified: SystemTime::UNIX_EPOCH }
    }

    #[test]
    fn three_to_five_columns_by_width() {
        assert_eq!(columns_for(900.0), 3);
        assert_eq!(columns_for(1280.0), 4);
        assert_eq!(columns_for(1920.0), 5);
        assert_eq!(columns_for(4000.0), 5);
        assert_eq!(columns_for(300.0), 3);
    }

    #[test]
    fn ease_is_smooth_and_bounded() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert_eq!(ease_in_out(0.5), 0.5);
        assert!(ease_in_out(0.1) < 0.1 && ease_in_out(0.9) > 0.9);
        assert_eq!(ease_in_out(3.0), 1.0);
    }

    #[test]
    fn canvas_fit_rect_matches_the_canvas_screen_fit() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 860.0));
        let r = canvas_fit_rect(screen, 2048.0, 1536.0);
        let inner = Rect::from_min_max(Pos2::new(16.0, FIT_TOP), Pos2::new(1264.0, 844.0));
        let mut v = crate::view::View::fit(inner, 2048.0, 1536.0);
        v.offset += inner.min - screen.min;
        let corners = v.corners(screen, 2048.0, 1536.0);
        assert!((corners[0].x - r.left()).abs() < 0.01 && (corners[0].y - r.top()).abs() < 0.01);
        assert!((corners[2].x - r.right()).abs() < 0.01 && (corners[2].y - r.bottom()).abs() < 0.01);
    }

    #[test]
    fn crop_is_centred_and_four_by_three() {
        let uv = crop_uv_4x3(1024.0, 512.0);
        assert!((uv.width() - (4.0 / 3.0) / 2.0).abs() < 1e-6 && uv.height() == 1.0);
        assert!((uv.center().x - 0.5).abs() < 1e-6);
        let uv = crop_uv_4x3(512.0, 1024.0);
        assert!(uv.width() == 1.0 && (uv.height() - (0.5 / (4.0 / 3.0))).abs() < 1e-6);
        let uv = crop_uv_4x3(400.0, 300.0);
        assert_eq!((uv.width(), uv.height()), (1.0, 1.0));
    }

    #[test]
    fn untitled_names_do_not_repeat() {
        assert_eq!(next_untitled(&[]), "Untitled");
        assert_eq!(next_untitled(&[entry("Untitled")]), "Untitled 2");
        assert_eq!(next_untitled(&[entry("Untitled"), entry("Untitled 2"), entry("x")]), "Untitled 3");
    }

    #[test]
    fn presets_and_custom_sizes() {
        let p = size_presets((1920, 1080));
        assert_eq!(p[0], ("Screen size", 1920, 1080));
        assert_eq!((p[1].1, p[1].2), (2048, 2048));
        assert_eq!((p[2].1, p[2].2), (3840, 2160));
        assert_eq!((p[3].1, p[3].2), (2480, 3508));
        assert_eq!(parse_custom_size(" 800 ", "600"), Ok((800, 600)));
        assert!(parse_custom_size("0", "10").is_err());
        assert!(parse_custom_size("abc", "10").is_err());
        assert!(parse_custom_size("9000", "10").is_err());
        assert!(parse_custom_size("8192", "8192").is_err());
        assert!(parse_custom_size("-5", "5").is_err());
    }
}

#[cfg(test)]
#[path = "gallery_tests.rs"]
mod ui_tests;
