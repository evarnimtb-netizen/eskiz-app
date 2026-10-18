//! First-run welcome tour: two cards over the Gallery, then three coach marks on the canvas.
//!
//! Design: a short, skippable welcome on a fresh install. The pieces:
//!
//! * [`Tour`] is a plain state machine with no UI in it (stored version, gating, steps).
//! * [`copy`] holds every English string in one table.
//! * The `impl EskizApp` at the end draws the cards and marks and feeds the machine.
//!
//! Rules the code keeps: every step waits for the user (there is no timer), every step can be
//! skipped (button or Esc), every gesture hint also names a button or a key, and a mark never
//! covers the thing it points at.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Id, Key, Margin, Modifiers, Order, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use crate::app::EskizApp;
use crate::gallery::Screen;
use crate::theme;

/// Version of the tour. A stored value below this shows the tour once. Bump it only on purpose.
pub(crate) const TOUR_VERSION: u32 = 1;
/// eframe storage key of the tour state (`tour=N`).
pub(crate) const KEY: &str = "eskiz-onboarding";
/// Key under the old app name; read when [`KEY`] is missing, never written.
pub(crate) const LEGACY_KEY: &str = "easel-onboarding"; // legacy-easel
/// Title of the sample document in the Gallery.
pub(crate) const SAMPLE_TITLE: &str = "Sample: first steps";

/// The stored text for tour version `n`.
pub(crate) fn encode(n: u32) -> String {
    format!("tour={n}")
}

/// The tour version in stored text, or `None` when the text has no valid `tour=N`.
pub(crate) fn decode(text: &str) -> Option<u32> {
    text.split(';').find_map(|kv| kv.trim().strip_prefix("tour=")?.trim().parse().ok())
}

// ------------------------------------------------------------------ copy

/// Every string of the tour, in one table (English only for now).
pub(crate) mod copy {
    /// The app name, in one place for the rename.
    pub const APP_NAME: &str = "Eskiz";

    pub fn welcome_title() -> String {
        format!("Welcome to {APP_NAME}")
    }
    pub const WELCOME_BODY: &str = "Paint, layer and export artwork with touch, pen or mouse. The tour takes about a minute. Skip it whenever you like.";
    pub const SHOW_AROUND: &str = "Show me around";
    pub const SKIP: &str = "Skip";
    pub const SAMPLE_TITLE: &str = "Try a sample";
    pub const SAMPLE_BODY: &str = "We made a sample canvas for you. Open it to try the tools. You can delete it from the Gallery like any other artwork.";
    pub const OPEN_SAMPLE: &str = "Open sample";
    pub const NEW_CANVAS: &str = "Start a new canvas";
    pub const SKIP_TOUR: &str = "Skip tour";
    pub const DONE_TITLE: &str = "You are set";
    pub const DONE_BODY: &str = "Find this tour again under Actions, Help, Welcome tour. Gestures and shortcuts are in the same menu.";
    pub const DONE: &str = "Done";
    pub const PAINT_TITLE: &str = "Paint";
    pub const UNDO_TITLE: &str = "Undo";
    pub const SIZE_TITLE: &str = "Brush size";
    /// Help row and Settings switch.
    pub const HELP_ROW: &str = "Welcome tour";
    pub const HELP_ROW_SUB: &str = "A one-minute walk through the basics";
    pub const AGAIN_SWITCH: &str = "Show welcome again on next launch";
    pub const AGAIN_SWITCH_SUB: &str = "The tour opens once, then switches itself off";
    /// Prefix of the two-line form shown before any input is seen.
    pub const ON_TOUCH: &str = "On touch: ";
    pub const WITH_MOUSE: &str = "With a mouse or pen: ";

    pub fn progress(n: usize, of: usize) -> String {
        format!("Step {n} of {of}")
    }

    /// Everything the hints need to know besides the input kind.
    #[derive(Clone, Copy, Debug)]
    pub struct Gestures {
        /// Finger touches paint (Settings, Gesture controls).
        pub touch_painting: bool,
        /// Two-finger tap undoes.
        pub undo_gestures: bool,
    }

    fn paint_touch(g: Gestures) -> String {
        if g.touch_painting {
            "Drag one finger to paint. The Brush button in the top bar picks the brush.".to_owned()
        } else {
            "Fingers move the view. Use the pen to paint, or press B for the brush.".to_owned()
        }
    }
    fn paint_mouse() -> String {
        "Drag to paint. Press B or click Brush in the top bar for the brush, and E for the eraser.".to_owned()
    }
    fn undo_touch(g: Gestures) -> String {
        if g.undo_gestures {
            "Changed your mind? Two-finger tap to undo, or tap Undo in the sidebar.".to_owned()
        } else {
            "Changed your mind? Tap Undo in the sidebar.".to_owned()
        }
    }
    fn undo_mouse(cmd: &str) -> String {
        format!("Changed your mind? Press {cmd}+Z, or click Undo in the sidebar.")
    }
    fn size_line() -> String {
        "Change the brush size with the Brush size slider in the sidebar, or with the [ and ] keys.".to_owned()
    }

    /// The lines of a coach mark. With no input seen yet there are two (touch and mouse), so
    /// nothing is guessed; the size mark reads the same for every input.
    pub fn hint(step: super::Step, input: super::InputKind, g: Gestures, cmd: &str) -> Vec<String> {
        use super::{InputKind, Step};
        let (touch, mouse) = match step {
            Step::Paint => (paint_touch(g), paint_mouse()),
            Step::Undo => (undo_touch(g), undo_mouse(cmd)),
            Step::Size => return vec![size_line()],
            _ => return Vec::new(),
        };
        match input {
            InputKind::Touch => vec![touch],
            InputKind::Mouse => vec![mouse],
            InputKind::Unknown => vec![format!("{ON_TOUCH}{touch}"), format!("{WITH_MOUSE}{mouse}")],
        }
    }
}

// ------------------------------------------------------------------ state machine

/// One step of the tour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Card 1 over the Gallery.
    Welcome,
    /// Card 2 over the Gallery.
    Sample,
    /// Coach mark: paint a stroke.
    Paint,
    /// Coach mark: undo it.
    Undo,
    /// Coach mark: change the brush size.
    Size,
    /// The closing card.
    Done,
}

impl Step {
    /// Place of a coach mark among the three, from 1.
    fn mark_number(self) -> Option<usize> {
        match self {
            Step::Paint => Some(1),
            Step::Undo => Some(2),
            Step::Size => Some(3),
            _ => None,
        }
    }
}

/// The kind of input the app saw last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum InputKind {
    #[default]
    Unknown,
    Touch,
    Mouse,
}

/// What the app looks like to the gate that starts the tour.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Gate {
    /// The Gallery is the screen (not the canvas, not the opening animation).
    pub gallery: bool,
    /// The recovery sheet is up.
    pub recovery_open: bool,
    /// The library holds no artwork; `None` while there is no library.
    pub library_empty: Option<bool>,
}

/// Counters the coach marks wait on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Obs {
    /// `Session::edits`.
    pub edits: u64,
    /// `Session::undo_count`.
    pub undos: u64,
    /// A stroke is being drawn.
    pub stroking: bool,
    /// Brush size of the current tool.
    pub size: f32,
}

/// Why the tour is waiting to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wait {
    No,
    /// Start when the gate allows. `needs_empty`: only if the library is empty (a fresh install);
    /// otherwise the tour is marked as seen without showing.
    Auto { needs_empty: bool },
}

/// The tour: stored version, gating and the current step.
#[derive(Clone, Debug)]
pub(crate) struct Tour {
    /// The version to write to storage, `None` while nothing is to be written.
    stored: Option<u32>,
    wait: Wait,
    step: Option<Step>,
    /// Counters when the current coach mark appeared.
    base: Option<Obs>,
    pub(crate) input: InputKind,
    focus_due: bool,
    /// Where the card or mark was drawn last frame (tests check it clears the target).
    pub(crate) card_rect: Rect,
    /// Where the current mark's target was last frame.
    pub(crate) target_rect: Rect,
}

impl Default for Tour {
    fn default() -> Self {
        Self::idle()
    }
}

impl Tour {
    /// No tour: tests, and apps opened on a file.
    pub(crate) fn idle() -> Self {
        Self {
            stored: None,
            wait: Wait::No,
            step: None,
            base: None,
            input: InputKind::Unknown,
            focus_due: false,
            card_rect: Rect::NOTHING,
            target_rect: Rect::NOTHING,
        }
    }

    /// The tour as it stands at launch.
    ///
    /// * `stored`: the version in storage, if any.
    /// * `has_prefs`: preferences exist in storage (not a brand-new install).
    /// * `opened_file`: the app was started on a file from the command line.
    pub(crate) fn at_startup(stored: Option<u32>, has_prefs: bool, opened_file: bool) -> Self {
        let mut t = Self::idle();
        t.stored = stored;
        if opened_file {
            return t;
        }
        match stored {
            Some(v) if v >= TOUR_VERSION => {}
            // A version below the current one (0 is the Settings switch) shows the tour once.
            Some(_) => t.wait = Wait::Auto { needs_empty: false },
            // An existing user from before the tour: mark it seen, show nothing.
            None if has_prefs => t.stored = Some(TOUR_VERSION),
            None => t.wait = Wait::Auto { needs_empty: true },
        }
        t
    }

    /// The tour as it stands at launch, read from eframe's storage (`None`: no storage at all).
    pub(crate) fn from_storage(storage: Option<&dyn eframe::Storage>, opened_file: bool) -> Self {
        let has_prefs = storage.is_some_and(|s| crate::compat::storage_string(s, crate::app::PREF_KEY, crate::app::LEGACY_PREF_KEY).is_some());
        let stored = storage
            .and_then(|s| crate::compat::storage_string(s, KEY, LEGACY_KEY))
            .and_then(|t| decode(&t));
        Self::at_startup(stored, has_prefs, opened_file)
    }

    /// The version to write to storage now, if any.
    pub(crate) fn stored_value(&self) -> Option<u32> {
        self.stored
    }

    /// The Settings switch "Show welcome again on next launch".
    pub(crate) fn show_again(&self) -> bool {
        self.stored == Some(0)
    }

    /// Turn the Settings switch on or off. On writes `tour=0`; off writes the current version.
    pub(crate) fn set_show_again(&mut self, on: bool) {
        self.stored = Some(if on { 0 } else { TOUR_VERSION });
    }

    /// The current step, if the tour is running.
    pub(crate) fn step(&self) -> Option<Step> {
        self.step
    }

    /// True while a card covers the Gallery, so nothing underneath may be used.
    pub(crate) fn is_modal(&self) -> bool {
        matches!(self.step, Some(Step::Welcome | Step::Sample))
    }

    /// True while the tour waits for its moment to start.
    pub(crate) fn is_waiting(&self) -> bool {
        self.wait != Wait::No
    }

    /// Per frame: start the tour when the gate allows. Returns true on the frame it starts.
    /// The version is stored the moment the tour starts, so a crash mid-tour does not replay it.
    pub(crate) fn poll(&mut self, gate: Gate) -> bool {
        let Wait::Auto { needs_empty } = self.wait else {
            return false;
        };
        if !gate.gallery || gate.recovery_open {
            return false;
        }
        let Some(empty) = gate.library_empty else {
            return false;
        };
        self.wait = Wait::No;
        if needs_empty && !empty {
            // Artwork but no tour record: an existing user. Seen, silently.
            self.stored = Some(TOUR_VERSION);
            return false;
        }
        self.stored = Some(TOUR_VERSION);
        self.enter(Step::Welcome);
        true
    }

    /// Help > Welcome tour: start from card 1. The stored version is left alone.
    pub(crate) fn replay(&mut self) {
        self.wait = Wait::No;
        self.enter(Step::Welcome);
    }

    fn enter(&mut self, step: Step) {
        self.step = Some(step);
        self.base = None;
        self.focus_due = true;
        self.card_rect = Rect::NOTHING;
        self.target_rect = Rect::NOTHING;
    }

    /// End the tour (Skip button, Esc, "Start a new canvas", Done).
    pub(crate) fn skip(&mut self) {
        self.step = None;
        self.base = None;
    }

    /// Card 1 -> card 2.
    pub(crate) fn show_around(&mut self) {
        if self.step == Some(Step::Welcome) {
            self.enter(Step::Sample);
        }
    }

    /// Card 2 -> first coach mark (the sample is opening).
    pub(crate) fn sample_opened(&mut self) {
        if self.step == Some(Step::Sample) {
            self.enter(Step::Paint);
        }
    }

    /// Whether the next frame should move keyboard focus to the card's primary button.
    pub(crate) fn take_focus(&mut self) -> bool {
        std::mem::take(&mut self.focus_due)
    }

    /// Per frame on the canvas: advance a coach mark whose action was done. Returns true when
    /// the step changed.
    pub(crate) fn observe(&mut self, obs: Obs) -> bool {
        let Some(step) = self.step else {
            return false;
        };
        if step.mark_number().is_none() {
            return false;
        }
        let base = *self.base.get_or_insert(obs);
        let done = match step {
            Step::Paint => obs.edits > base.edits && !obs.stroking,
            Step::Undo => obs.undos > base.undos,
            Step::Size => (obs.size - base.size).abs() > 1e-3,
            _ => false,
        };
        if !done {
            return false;
        }
        let next = match step {
            Step::Paint => Step::Undo,
            Step::Undo => Step::Size,
            _ => Step::Done,
        };
        self.enter(next);
        true
    }
}

// ------------------------------------------------------------------ placement

/// Width of a coach mark, points.
pub(crate) const MARK_W: f32 = 360.0;
/// Gap between a mark and its target, points.
const GAP: f32 = 18.0;

/// Where to anchor a coach mark so it does not cover `target`: the anchor point and which point
/// of the card sits on it. Sidebar targets get the card on the canvas side; the canvas target
/// (a wide region) gets it under the region.
pub(crate) fn mark_anchor(target: Rect, screen: Rect, top_clear: f32, wide: bool) -> (Pos2, Align2) {
    // Keep the card's middle far enough from the edges that a tall card still fits.
    let mid_y = target.center().y.clamp(screen.top() + top_clear + 90.0, (screen.bottom() - 90.0).max(screen.top() + top_clear + 90.0));
    if wide {
        let x = target.center().x.clamp(screen.left() + MARK_W / 2.0 + 8.0, (screen.right() - MARK_W / 2.0 - 8.0).max(screen.left() + MARK_W / 2.0 + 8.0));
        // Under the region when there is room, else above it.
        let room_below = screen.bottom() - target.bottom();
        if room_below >= 150.0 {
            return (Pos2::new(x, target.bottom() + GAP), Align2::CENTER_TOP);
        }
        return (Pos2::new(x, target.top() - GAP), Align2::CENTER_BOTTOM);
    }
    if target.center().x < screen.center().x {
        (Pos2::new(target.right() + GAP, mid_y), Align2::LEFT_CENTER)
    } else {
        (Pos2::new(target.left() - GAP, mid_y), Align2::RIGHT_CENTER)
    }
}

// ------------------------------------------------------------------ UI

/// Where the sidebar controls the marks point at were drawn last frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TargetRects {
    /// The Brush size slider.
    pub size: Rect,
    /// The Undo button.
    pub undo: Rect,
}

impl Default for TargetRects {
    fn default() -> Self {
        Self { size: Rect::NOTHING, undo: Rect::NOTHING }
    }
}

/// Fill of the primary button: the accent of the look.
fn button_fill(pal: &theme::Palette) -> Color32 {
    if pal.identity == theme::UiIdentity::Eskiz {
        pal.primary
    } else {
        pal.accent
    }
}

/// Text colour on `fill`: white or black, whichever reads better (at least 4.5:1 on the accents).
pub(crate) fn ink_on(fill: Color32) -> Color32 {
    use crate::a11y::contrast_ratio;
    if contrast_ratio(Color32::WHITE, fill) >= contrast_ratio(Color32::BLACK, fill) {
        Color32::WHITE
    } else {
        Color32::BLACK
    }
}

fn cmd_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

impl EskizApp {
    /// Is the Gallery showing with nothing in front of it that the tour must wait for?
    fn tour_gate(&self) -> Gate {
        let library_empty = self.gal.library.as_ref().map(|_| self.gal.entries.is_empty());
        Gate { gallery: matches!(self.gal.screen, Screen::Gallery), recovery_open: !self.gal.offers.is_empty(), library_empty }
    }

    /// Per frame, before the Gallery draws: the tour may start now.
    pub(crate) fn tour_poll(&mut self) {
        let gate = self.tour_gate();
        self.tour.poll(gate);
    }

    /// Per frame: remember whether the user is on touch or on a mouse or pen.
    pub(crate) fn tour_watch_input(&mut self, ctx: &egui::Context) {
        let seen = ctx.input(|i| {
            if i.events.iter().any(|e| matches!(e, egui::Event::Touch { .. })) {
                Some(InputKind::Touch)
            } else if i.events.iter().any(|e| matches!(e, egui::Event::PointerButton { .. } | egui::Event::PointerMoved(_))) {
                Some(InputKind::Mouse)
            } else {
                None
            }
        });
        if let Some(kind) = seen {
            self.tour.input = kind;
        }
    }

    /// Help > Welcome tour.
    pub(crate) fn replay_tour(&mut self, ctx: &egui::Context) {
        self.popovers.close();
        self.tour.replay();
        self.open_gallery(ctx);
    }

    /// The two cards over the Gallery (no-op for any other step).
    pub(crate) fn tour_cards(&mut self, ctx: &egui::Context) {
        let Some(step @ (Step::Welcome | Step::Sample)) = self.tour.step() else {
            return;
        };
        if !matches!(self.gal.screen, Screen::Gallery) {
            return;
        }
        let gp = theme::palette_for(self.prefs.identity, false);
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.tour.skip();
            return;
        }
        let screen = ctx.content_rect();
        egui::Area::new(Id::new("tour-scrim")).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
            ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(150));
        });
        #[derive(PartialEq)]
        enum Pick {
            None,
            Around,
            Open,
            NewCanvas,
            Skip,
        }
        let mut pick = Pick::None;
        let focus = self.tour.take_focus();
        let mut rect = Rect::NOTHING;
        egui::Area::new(Id::new("tour-card")).order(Order::Tooltip).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            let frame = theme::panel_frame(&gp).inner_margin(Margin::same(20)).show(ui, |ui| {
                crate::gallery::dark_widgets(ui, &gp);
                ui.set_width(360.0);
                let (title, body) = match step {
                    Step::Welcome => (copy::welcome_title(), copy::WELCOME_BODY),
                    _ => (copy::SAMPLE_TITLE.to_owned(), copy::SAMPLE_BODY),
                };
                ui.add(egui::Label::new(RichText::new(title).size(20.0).strong().color(gp.text)).wrap());
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(body).size(14.0).color(gp.text_dim)).wrap());
                ui.add_space(16.0);
                let primary = |label: &str| egui::Button::new(RichText::new(label).color(ink_on(button_fill(&gp)))).fill(button_fill(&gp)).min_size(Vec2::new(ui_w(ui), 38.0));
                match step {
                    Step::Welcome => {
                        let go = ui.add(primary(copy::SHOW_AROUND));
                        if focus {
                            go.request_focus();
                        }
                        if go.clicked() {
                            pick = Pick::Around;
                        }
                    }
                    _ => {
                        let go = ui.add(primary(copy::OPEN_SAMPLE));
                        if focus {
                            go.request_focus();
                        }
                        if go.clicked() {
                            pick = Pick::Open;
                        }
                        ui.add_space(6.0);
                        if ui.add(egui::Button::new(copy::NEW_CANVAS).min_size(Vec2::new(ui_w(ui), 36.0))).clicked() {
                            pick = Pick::NewCanvas;
                        }
                    }
                }
                ui.add_space(6.0);
                if ui.add(egui::Button::new(copy::SKIP).min_size(Vec2::new(ui_w(ui), 36.0))).clicked() {
                    pick = Pick::Skip;
                }
            });
            rect = frame.response.rect;
        });
        self.tour.card_rect = rect;
        match pick {
            Pick::None => {}
            Pick::Around => self.tour.show_around(),
            Pick::Skip => self.tour.skip(),
            Pick::Open => {
                if self.open_tour_sample(ctx) {
                    self.tour.sample_opened();
                } else {
                    self.show_toast("Could not open the sample".to_owned(), 3.0);
                    self.tour.skip();
                }
            }
            Pick::NewCanvas => {
                self.tour.skip();
                let anchor = self.gal.header.plus;
                self.toggle_menu(crate::gallery::Menu::NewCanvas, anchor);
            }
        }
        // Repaint only when the card changed something (a click already repaints; nothing here animates).
        if pick != Pick::None || focus {
            ctx.request_repaint();
        }
    }

    /// Per frame on the canvas: the coach marks and the closing card.
    pub(crate) fn tour_marks(&mut self, ctx: &egui::Context) {
        self.tour_watch_input(ctx);
        let Some(step) = self.tour.step() else {
            return;
        };
        if matches!(step, Step::Welcome | Step::Sample) || self.gallery_active() {
            return;
        }
        let obs = Obs {
            edits: self.session.edits(),
            undos: self.session.undo_count(),
            stroking: self.session.is_stroking(),
            size: self.tool_size(),
        };
        if self.tour.observe(obs) {
            ctx.request_repaint();
        }
        let Some(step) = self.tour.step() else {
            return;
        };
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.tour.skip();
            return;
        }
        let pal = self.pal;
        let screen = ctx.content_rect();
        let cmd = cmd_key();
        let g = copy::Gestures { touch_painting: self.prefs.touch_painting, undo_gestures: self.prefs.undo_gestures };
        let focus = self.tour.take_focus();
        // The mark fades in over 0.15 s from the frame its step first shows (no fade under reduced motion).
        let since = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(Id::new(format!("tour-mark-since-{step:?}")), || self.now));
        let fade = if self.reduce_motion() { 1.0 } else { ((self.now - since) / 0.15).clamp(0.0, 1.0) as f32 };
        if fade < 1.0 {
            ctx.request_repaint();
        }
        let mut finished = false;
        let mut skipped = false;
        if step == Step::Done {
            let mut rect = Rect::NOTHING;
            egui::Area::new(Id::new("tour-mark")).order(Order::Tooltip).anchor(Align2::CENTER_BOTTOM, Vec2::new(0.0, -32.0)).show(ctx, |ui| {
                ui.set_opacity(fade);
                let frame = theme::panel_frame(&pal).inner_margin(Margin::same(16)).show(ui, |ui| {
                    ui.set_width(MARK_W);
                    ui.add(egui::Label::new(RichText::new(copy::DONE_TITLE).size(18.0).strong().color(pal.text)).wrap());
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(RichText::new(copy::DONE_BODY).size(14.0).color(pal.text_dim)).wrap());
                    ui.add_space(12.0);
                    let b = ui.add(egui::Button::new(RichText::new(copy::DONE).color(ink_on(button_fill(&pal)))).fill(button_fill(&pal)).min_size(Vec2::new(ui_w(ui), 38.0)));
                    if focus {
                        b.request_focus();
                    }
                    finished = b.clicked();
                });
                rect = frame.response.rect;
            });
            self.tour.card_rect = rect;
            self.tour.target_rect = Rect::NOTHING;
            if finished {
                self.tour.skip();
            }
            return;
        }
        // Target of the mark: the middle of the canvas, the Undo button or the size slider.
        let top_clear = crate::app::BAR_HEIGHT;
        let (target, title, wide) = match step {
            Step::Paint => {
                let vp = self.viewport;
                (vp.shrink2(Vec2::new(vp.width() * 0.2, vp.height() * 0.34)), copy::PAINT_TITLE, true)
            }
            Step::Undo => (self.tour_rects.undo, copy::UNDO_TITLE, false),
            _ => (self.tour_rects.size, copy::SIZE_TITLE, false),
        };
        if !target.is_positive() {
            // The target was not drawn yet (the UI is hidden): wait a frame.
            ctx.request_repaint();
            return;
        }
        let (anchor, pivot) = mark_anchor(target, screen, top_clear, wide);
        let lines = copy::hint(step, self.tour.input, g, cmd);
        let n = step.mark_number().unwrap_or(1);
        let mut rect = Rect::NOTHING;
        egui::Area::new(Id::new("tour-mark")).order(Order::Tooltip).pivot(pivot).fixed_pos(anchor).show(ctx, |ui| {
            ui.set_opacity(fade);
            let frame = theme::panel_frame(&pal).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_width(MARK_W);
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(title).size(16.0).strong().color(pal.text)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(copy::progress(n, 3)).size(12.0).color(pal.text_dim)));
                    });
                });
                ui.add_space(4.0);
                for line in &lines {
                    ui.add(egui::Label::new(RichText::new(line).size(14.0).color(pal.text)).wrap());
                    ui.add_space(2.0);
                }
                ui.add_space(8.0);
                if ui.add(egui::Button::new(copy::SKIP_TOUR).min_size(Vec2::new(0.0, 32.0))).clicked() {
                    skipped = true;
                }
            });
            rect = frame.response.rect;
        });
        // A ring around the target; static, so reduced motion needs no special case.
        let ring = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("tour-ring")));
        let ring_col = if pal.identity == theme::UiIdentity::Eskiz { pal.focus_ring } else { pal.accent };
        if !wide {
            ring.rect_stroke(target.expand(4.0), CornerRadius::same(12), Stroke::new(2.0, ring_col), StrokeKind::Outside);
        }
        self.tour.card_rect = rect;
        self.tour.target_rect = target;
        if skipped {
            self.tour.skip();
        }
    }
}

/// Width of the space left in `ui`, for full-width buttons.
fn ui_w(ui: &egui::Ui) -> f32 {
    ui.available_width()
}

/// `--selftest`: the tour's gating logic runs, and the sample document is built, saved and read
/// back (one layer, 1600 x 1000, the right title).
pub(crate) fn selftest_check() -> Result<String, String> {
    let mut t = Tour::at_startup(None, false, false);
    let gate = Gate { gallery: true, recovery_open: false, library_empty: Some(true) };
    if !t.poll(gate) || t.step() != Some(Step::Welcome) {
        return Err("onboarding: a fresh install did not show the tour".to_owned());
    }
    if Tour::at_startup(None, true, false).is_waiting() {
        return Err("onboarding: an upgrade would show the tour".to_owned());
    }
    let doc = crate::onboarding_sample::sample_document();
    if doc.layer_ids().is_empty() {
        return Err("onboarding: the sample has no layer".to_owned());
    }
    let dir = std::env::temp_dir().join(format!("eskiz-selftest-tour-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = crate::storage::Library::at(&dir);
    let saved = lib.save("sample", SAMPLE_TITLE, &doc);
    let back = saved.and_then(|e| lib.load(&e.id));
    let _ = std::fs::remove_dir_all(&dir);
    let back = back?;
    if (back.width, back.height) != (crate::onboarding_sample::SAMPLE_W, crate::onboarding_sample::SAMPLE_H) {
        return Err("onboarding: the sample changed size on the way through storage".to_owned());
    }
    Ok(format!("  onboarding: tour gating ok, sample saved and reopened ({} layer)", back.layer_ids().len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate_ok() -> Gate {
        Gate { gallery: true, recovery_open: false, library_empty: Some(true) }
    }
    fn obs() -> Obs {
        Obs { edits: 5, undos: 0, stroking: false, size: 24.0 }
    }

    #[test]
    fn stored_text_round_trips() {
        assert_eq!(decode(&encode(1)), Some(1));
        assert_eq!(decode("tour=0"), Some(0));
        assert_eq!(decode("junk"), None);
        assert_eq!(decode("tour=x"), None);
    }

    #[test]
    fn fresh_install_shows_on_an_empty_library_and_stores_at_start() {
        let mut t = Tour::at_startup(None, false, false);
        assert!(t.is_waiting() && t.step().is_none());
        assert_eq!(t.stored_value(), None, "nothing is stored until the tour starts");
        assert!(t.poll(gate_ok()));
        assert_eq!(t.step(), Some(Step::Welcome));
        assert_eq!(t.stored_value(), Some(TOUR_VERSION), "stored at start, so a crash does not replay it");
    }

    #[test]
    fn upgrade_is_migrated_silently() {
        let mut t = Tour::at_startup(None, true, false);
        assert!(!t.is_waiting());
        assert_eq!(t.stored_value(), Some(TOUR_VERSION));
        assert!(!t.poll(gate_ok()));
        assert_eq!(t.step(), None);
    }

    #[test]
    fn artwork_without_a_record_never_shows() {
        let mut t = Tour::at_startup(None, false, false);
        assert!(!t.poll(Gate { library_empty: Some(false), ..gate_ok() }));
        assert_eq!(t.step(), None);
        assert_eq!(t.stored_value(), Some(TOUR_VERSION));
        assert!(!t.is_waiting());
    }

    #[test]
    fn a_seen_tour_and_a_launch_on_a_file_show_nothing() {
        let mut seen = Tour::at_startup(Some(TOUR_VERSION), true, false);
        assert!(!seen.poll(gate_ok()) && seen.step().is_none());
        let mut file = Tour::at_startup(None, false, true);
        assert!(!file.poll(gate_ok()) && file.step().is_none());
        assert_eq!(file.stored_value(), None);
    }

    #[test]
    fn recovery_sheet_and_other_screens_make_it_wait() {
        let mut t = Tour::at_startup(None, false, false);
        assert!(!t.poll(Gate { recovery_open: true, ..gate_ok() }));
        assert!(!t.poll(Gate { gallery: false, ..gate_ok() }));
        assert!(!t.poll(Gate { library_empty: None, ..gate_ok() }));
        assert!(t.is_waiting() && t.stored_value().is_none());
        assert!(t.poll(gate_ok()), "starts once the sheet is closed");
    }

    #[test]
    fn skip_at_every_step_ends_the_tour_and_keeps_the_version() {
        for steps in 0..6 {
            let mut t = Tour::at_startup(None, false, false);
            t.poll(gate_ok());
            if steps >= 1 {
                t.show_around();
            }
            if steps >= 2 {
                t.sample_opened();
            }
            let mut o = obs();
            if steps >= 3 {
                t.observe(o);
                o.edits += 1;
                t.observe(o);
            }
            if steps >= 4 {
                t.observe(o);
                o.undos += 1;
                t.observe(o);
            }
            if steps >= 5 {
                t.observe(o);
                o.size = 30.0;
                t.observe(o);
                assert_eq!(t.step(), Some(Step::Done));
            }
            assert!(t.step().is_some(), "step {steps} is running");
            t.skip();
            assert_eq!(t.step(), None);
            assert_eq!(t.stored_value(), Some(TOUR_VERSION));
        }
    }

    #[test]
    fn marks_wait_for_the_action_and_move_on() {
        let mut t = Tour::at_startup(None, false, false);
        t.poll(gate_ok());
        t.show_around();
        t.sample_opened();
        assert_eq!(t.step(), Some(Step::Paint));
        let mut o = obs();
        assert!(!t.observe(o), "baseline taken, nothing done");
        o.stroking = true;
        o.edits += 1;
        assert!(!t.observe(o), "a stroke still being drawn does not count");
        o.stroking = false;
        assert!(t.observe(o));
        assert_eq!(t.step(), Some(Step::Undo));
        assert!(!t.observe(o));
        o.undos += 1;
        assert!(t.observe(o));
        assert_eq!(t.step(), Some(Step::Size));
        assert!(!t.observe(o));
        o.size += 5.0;
        assert!(t.observe(o));
        assert_eq!(t.step(), Some(Step::Done));
        t.skip();
        assert_eq!(t.step(), None);
    }

    #[test]
    fn replay_starts_at_card_one_and_leaves_the_stored_version() {
        let mut t = Tour::at_startup(Some(TOUR_VERSION), true, false);
        t.replay();
        assert_eq!(t.step(), Some(Step::Welcome));
        assert_eq!(t.stored_value(), Some(TOUR_VERSION));
        t.show_around();
        t.skip();
        t.replay();
        assert_eq!(t.step(), Some(Step::Welcome));
    }

    #[test]
    fn show_again_switch_replays_once_on_the_next_launch() {
        let mut t = Tour::at_startup(Some(TOUR_VERSION), true, false);
        t.set_show_again(true);
        assert!(t.show_again());
        assert_eq!(t.stored_value(), Some(0));
        // Next launch: prefs exist and the library has artwork, and it still shows.
        let mut next = Tour::at_startup(Some(0), true, false);
        assert!(next.poll(Gate { library_empty: Some(false), ..gate_ok() }));
        assert_eq!(next.step(), Some(Step::Welcome));
        assert!(!next.show_again(), "the switch turned itself off");
        t.set_show_again(false);
        assert_eq!(t.stored_value(), Some(TOUR_VERSION));
    }

    #[test]
    fn a_bumped_version_shows_once_to_everyone() {
        // Today's version is 1; a stored 0 stands in for "an older tour version".
        let mut t = Tour::at_startup(Some(TOUR_VERSION - 1), true, false);
        assert!(t.poll(gate_ok()));
        let again = Tour::at_startup(t.stored_value(), true, false);
        assert!(!again.is_waiting());
    }

    #[test]
    fn modal_only_for_the_gallery_cards() {
        let mut t = Tour::at_startup(None, false, false);
        t.poll(gate_ok());
        assert!(t.is_modal());
        t.show_around();
        assert!(t.is_modal());
        t.sample_opened();
        assert!(!t.is_modal());
    }

    // ------------------------------------------------------------ copy

    fn words(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn copy_follows_the_rules() {
        let g_on = copy::Gestures { touch_painting: true, undo_gestures: true };
        let g_off = copy::Gestures { touch_painting: false, undo_gestures: false };
        let mut all: Vec<String> = vec![
            copy::welcome_title(),
            copy::WELCOME_BODY.into(),
            copy::SAMPLE_BODY.into(),
            copy::DONE_BODY.into(),
            copy::HELP_ROW_SUB.into(),
            copy::AGAIN_SWITCH_SUB.into(),
        ];
        for step in [Step::Paint, Step::Undo, Step::Size] {
            for input in [InputKind::Unknown, InputKind::Touch, InputKind::Mouse] {
                for g in [g_on, g_off] {
                    let lines = copy::hint(step, input, g, "Ctrl");
                    assert!(!lines.is_empty());
                    for l in &lines {
                        // Every hint names a button or a key (WCAG 2.5.1), gesture or not.
                        let names_control = ["Brush", "Undo", "slider", "press", "Press", "[ and ]", "B for"].iter().any(|k| l.contains(k));
                        assert!(names_control, "no button or key in: {l}");
                    }
                    all.extend(lines);
                }
            }
        }
        for s in &all {
            assert!(!s.contains('!'), "no exclamation marks: {s}");
            assert!(s.is_ascii(), "plain text: {s}");
            let limit = if s.contains(copy::ON_TOUCH) || s.contains(copy::WITH_MOUSE) { 32 } else { 27 };
            assert!(words(s) <= limit, "too long ({} words): {s}", words(s));
        }
        // Touch gestures are marked off when the switches are off.
        assert!(!copy::hint(Step::Undo, InputKind::Touch, g_off, "Ctrl")[0].contains("Two-finger"));
        assert!(copy::hint(Step::Undo, InputKind::Touch, g_on, "Ctrl")[0].contains("Two-finger"));
        assert!(copy::hint(Step::Paint, InputKind::Touch, g_off, "Ctrl")[0].contains("pen"));
        assert!(copy::hint(Step::Undo, InputKind::Mouse, g_on, "Cmd")[0].contains("Cmd+Z"));
        assert_eq!(copy::hint(Step::Paint, InputKind::Unknown, g_on, "Ctrl").len(), 2, "no guess before any input");
    }

    // ------------------------------------------------------------ placement

    #[test]
    fn marks_never_cover_their_target() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
        let card_h = 170.0;
        for sidebar_x in [8.0, 1280.0 - 56.0 - 8.0] {
            let target = Rect::from_min_size(Pos2::new(sidebar_x, 300.0), Vec2::new(56.0, 56.0));
            let (p, pivot) = mark_anchor(target, screen, 48.0, false);
            let card = Rect::from_min_size(Pos2::ZERO, Vec2::new(MARK_W, card_h));
            let (ax, ay) = (pivot.x().to_factor(), pivot.y().to_factor());
            let r = Rect::from_min_size(Pos2::new(p.x - ax * card.width(), p.y - ay * card.height()), card.size());
            assert!(!r.intersects(target), "{r:?} covers {target:?}");
            assert!(screen.contains_rect(r), "{r:?} leaves the screen");
        }
        let vp = Rect::from_min_max(Pos2::new(0.0, 48.0), Pos2::new(1280.0, 800.0));
        let region = vp.shrink2(Vec2::new(vp.width() * 0.2, vp.height() * 0.34));
        let (p, pivot) = mark_anchor(region, screen, 48.0, true);
        let (ax, ay) = (pivot.x().to_factor(), pivot.y().to_factor());
        let r = Rect::from_min_size(Pos2::new(p.x - ax * MARK_W, p.y - ay * card_h), Vec2::new(MARK_W, card_h));
        assert!(!r.intersects(region), "{r:?} covers {region:?}");
        assert!(screen.contains_rect(r));
    }
}
