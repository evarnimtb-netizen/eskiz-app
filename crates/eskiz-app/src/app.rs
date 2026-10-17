//! The egui application: state, input routing and the canvas.
//!
//! The canvas is drawn from one `TextureHandle`. A stroke refreshes only the rectangle
//! it changed (`set_partial`). Layer edits that change many pixels refresh the whole
//! texture. The bars, sidebar and popovers are in [`crate::chrome`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui::{
    self, epaint::{Mesh, Vertex}, Color32, ColorImage, Key, Modifiers, MultiTouchInfo, Order,
    PointerButton, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions, TouchPhase, Vec2,
};

use crate::brush_import::BrushStore;
use crate::brush_library::{default_brush, BrushBank, BrushId, BrushUi};
use crate::colour::ColourStore;
use crate::pour_fill::DropState;
use crate::command::Refresh;
use crate::document::Document;
use crate::eyedropper;
use crate::export::default_export_path;
use crate::gesture::{self, Gesture, Recognizer, RepeatClock};
use crate::history::PixelRect;
use crate::import::open_procreate;
use crate::input::{apply_pressure_curve, sample_from, sample_from_pen, PointerKind, RawPointer};
use crate::paint::StrokeSettings;
use crate::panels::colour::ColourUi;
use crate::popover::{Panel, Popovers};
use crate::pressure::PressureCurve;
use crate::hold_shape::{self, Kind, StillTimer};
use crate::selection_tool::{ClipAction, EditMode, SelectionTool, TransformTool};
use crate::session::Session;
use crate::tablet::{PenFrame, PenSample, PenSource, PenState};
use crate::theme::{self, Palette};
use crate::view::{self, View};

const TEX_OPTIONS: TextureOptions = TextureOptions::LINEAR;

/// The compositor's premultiplied pixels as an egui image. `Color32` is four bytes aligned to four, like
/// the `u32`s, so the pixel `Vec` is reinterpreted, not copied.
pub(crate) fn color_image(size: [usize; 2], pixels: Vec<u32>) -> ColorImage {
    ColorImage::new(size, eskiz_core::composite::cast_pixels::<Color32>(pixels))
}
/// Height of the translucent top bar, points.
pub const BAR_HEIGHT: f32 = 48.0;
/// Key for the eframe storage entry that holds [`Prefs`].
pub(crate) const PREF_KEY: &str = "eskiz-prefs";
/// Storage keys of releases before the rename; read when the new key is missing, never written.
pub(crate) const LEGACY_PREF_KEY: &str = "easel-prefs"; // legacy-easel
/// Key for the eframe storage entry that holds the colour history and palette.
pub(crate) const COLOUR_KEY: &str = "eskiz-colours";
pub(crate) const LEGACY_COLOUR_KEY: &str = "easel-colours"; // legacy-easel
/// Key for the eframe storage entry that holds the brush of each tool (see `brush_library`).
pub(crate) const BRUSH_KEY: &str = "eskiz-brushes";
pub(crate) const LEGACY_BRUSH_KEY: &str = "easel-brushes"; // legacy-easel
/// Largest brush size the sidebar slider reaches, canvas pixels.
pub const MAX_SIZE: f32 = 400.0;

type P = (f64, f64);

/// The painting tools. Each keeps its own size and opacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Smudge,
    Eraser,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Brush, Tool::Smudge, Tool::Eraser];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Size and opacity for one tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolParams {
    /// Brush diameter in canvas pixels, 1..=400.
    pub size: f32,
    /// 0..=1.
    pub opacity: f32,
}

/// Settings that survive restarts, stored with eframe's persistence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    pub light: bool,
    /// Chrome look: Classic (default) or Eskiz.
    pub identity: theme::UiIdentity,
    pub sidebar_right: bool,
    pub brush_cursor: bool,
    pub eyedrop_hold: bool,
    pub hold_shape: bool,
    /// Top edge of the sidebar in points. Negative means "centred".
    pub sidebar_y: f32,
    /// Global pressure curve, applied to touch force and pen pressure.
    pub pressure_curve: PressureCurve,
    /// Two-finger tap undoes and three-finger tap redoes.
    pub undo_gestures: bool,
    /// Holding two fingers keeps undoing.
    pub repeat_undo: bool,
    /// Two-finger pan, pinch zoom and rotate of the view.
    pub touch_navigation: bool,
    /// Timelapse recording on for new documents.
    pub timelapse: bool,
    /// Timelapse quality index: 0 Low, 1 Standard, 2 High, 3 Studio.
    pub timelapse_quality: u8,
    /// Timelapse resolution index: 0 1080p, 1 2K, 2 4K.
    pub timelapse_resolution: u8,
    /// Seconds a finger must rest before the eyedropper starts (Gesture controls).
    pub eyedrop_secs: f32,
    /// Seconds a stroke must rest before Hold to Shape snaps it.
    pub hold_shape_secs: f32,
    /// Three-finger scrub clears the layer.
    pub scrub_clear: bool,
    /// Three-finger swipe shows the copy menu.
    pub swipe_copy: bool,
    /// Four-finger tap toggles full screen.
    pub fullscreen_taps: bool,
    /// Finger touches paint. Off for pen-only use: fingers still navigate and undo.
    pub touch_painting: bool,
    /// Turn off popover and gallery zoom animations. The OS setting is also honoured.
    pub reduce_motion: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            light: false,
            identity: theme::UiIdentity::Classic,
            sidebar_right: false,
            brush_cursor: true,
            eyedrop_hold: true,
            hold_shape: true,
            sidebar_y: -1.0,
            pressure_curve: PressureCurve::default(),
            undo_gestures: true,
            repeat_undo: true,
            touch_navigation: true,
            timelapse: true,
            timelapse_quality: 1,
            timelapse_resolution: 0,
            eyedrop_secs: 0.4,
            hold_shape_secs: 0.65,
            scrub_clear: true,
            swipe_copy: true,
            fullscreen_taps: true,
            touch_painting: true,
            reduce_motion: false,
        }
    }
}

/// Version of the stored preferences string. Strings with no `ver` field are version 1.
///
/// Version 2 moved the eyedropper default from 0.2 s to 0.4 s. Version 1
/// wrote `eyed_s` for everyone, so a stored 0.2 could be the old default rather than a choice.
pub const PREFS_VERSION: u32 = 2;

/// The eyedropper delay that version 1 used as its default.
const OLD_EYEDROP_SECS: f32 = 0.2;

impl Prefs {
    /// `key=value` pairs separated by `;`.
    pub fn encode(&self) -> String {
        format!(
            "ver={PREFS_VERSION};light={};right={};cursor={};eyedrop={};hold_shape={};sidebar_y={};pcurve={};undo_gest={};repeat_undo={};touch_nav={};tl={};tlq={};tlr={};eyed_s={:.2};qs_s={:.2};scrub={};swipe={};fs4={};touch_paint={};rmotion={};identity={}",
            u8::from(self.light),
            u8::from(self.sidebar_right),
            u8::from(self.brush_cursor),
            u8::from(self.eyedrop_hold),
            u8::from(self.hold_shape),
            self.sidebar_y,
            self.pressure_curve.encode(),
            u8::from(self.undo_gestures),
            u8::from(self.repeat_undo),
            u8::from(self.touch_navigation),
            u8::from(self.timelapse),
            self.timelapse_quality,
            self.timelapse_resolution,
            self.eyedrop_secs,
            self.hold_shape_secs,
            u8::from(self.scrub_clear),
            u8::from(self.swipe_copy),
            u8::from(self.fullscreen_taps),
            u8::from(self.touch_painting),
            u8::from(self.reduce_motion),
            self.identity.key()
        )
    }

    /// Parse [`Prefs::encode`] output. Unknown or bad fields keep their defaults.
    pub fn decode(text: &str) -> Self {
        let mut p = Self::default();
        let mut version = 1;
        for pair in text.split(';') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            let flag = v == "1";
            match k {
                "ver" => {
                    if let Ok(n) = v.parse::<u32>() {
                        version = n;
                    }
                }
                "light" => p.light = flag,
                "identity" => p.identity = theme::UiIdentity::from_key(v).unwrap_or_default(),
                "right" => p.sidebar_right = flag,
                "cursor" => p.brush_cursor = flag,
                "eyedrop" => p.eyedrop_hold = flag,
                // `quickshape` is the key older builds wrote; it is still read (compat).
                "hold_shape" | "quickshape" => p.hold_shape = flag,
                "undo_gest" => p.undo_gestures = flag,
                "repeat_undo" => p.repeat_undo = flag,
                "touch_nav" => p.touch_navigation = flag,
                "tl" => p.timelapse = flag,
                "tlq" => {
                    if let Ok(q) = v.parse::<u8>() {
                        p.timelapse_quality = q.min(3);
                    }
                }
                "tlr" => {
                    if let Ok(r) = v.parse::<u8>() {
                        p.timelapse_resolution = r.min(2);
                    }
                }
                "eyed_s" => {
                    if let Ok(v) = v.parse::<f32>() {
                        p.eyedrop_secs = v.clamp(0.1, 1.5);
                    }
                }
                "qs_s" => {
                    if let Ok(v) = v.parse::<f32>() {
                        p.hold_shape_secs = v.clamp(0.2, 1.5);
                    }
                }
                "scrub" => p.scrub_clear = flag,
                "swipe" => p.swipe_copy = flag,
                "fs4" => p.fullscreen_taps = flag,
                "touch_paint" => p.touch_painting = flag,
                "rmotion" => p.reduce_motion = flag,
                "pcurve" => {
                    if let Some(c) = PressureCurve::decode(v) {
                        p.pressure_curve = c;
                    }
                }
                "sidebar_y" => {
                    if let Ok(y) = v.parse::<f32>() {
                        p.sidebar_y = y;
                    }
                }
                _ => {}
            }
        }
        // Version 1 stored the old 0.2 s default for everyone. Move exactly that value to the
        // new default. Any other delay was chosen by the user and stays.
        if version < 2 && p.eyedrop_secs == OLD_EYEDROP_SECS {
            p.eyedrop_secs = Prefs::default().eyedrop_secs;
        }
        p
    }
}

/// A transient message at the top centre.
#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub until: f64,
}

/// Touch input for one frame, in the order egui reported it.
#[derive(Clone, Copy, Debug)]
pub enum TouchEv {
    Down(u64, Pos2, f64),
    Move(u64, Pos2, f64),
    Up(u64, f64),
}

/// Input read from egui once per frame.
pub struct FrameInput {
    pub space: bool,
    pub middle: bool,
    pub alt: bool,
    pub shift: bool,
    pub r_held: bool,
    pub primary_pressed: bool,
    pub primary_down: bool,
    pub primary_released: bool,
    pub press_origin: Option<Pos2>,
    pub pointer: Option<Pos2>,
    pub pointer_delta: Vec2,
    pub hover: Option<Pos2>,
    /// Pinch or ctrl/cmd+wheel factor, 1.0 when idle.
    pub zoom: f32,
    pub scroll: Vec2,
    pub multi: Option<MultiTouchInfo>,
    pub time: f64,
    /// `Some(force)` when a touch event arrived this frame. `force` is `None` when the
    /// device did not report one.
    pub touch: Option<Option<f32>>,
    pub touches: Vec<TouchEv>,
    /// Pen contact samples this frame (empty unless a pen tablet is touching the surface).
    pub pen_samples: Vec<PenSample>,
    /// The OS cancelled a touch this frame (a palm that Android rejected).
    pub touch_cancelled: bool,
}

impl FrameInput {
    /// Make this frame look like nothing happened: no pointer, wheel, touch or pen input.
    pub(crate) fn silence(&mut self) {
        self.primary_pressed = false;
        self.primary_down = false;
        self.primary_released = false;
        self.middle = false;
        self.press_origin = None;
        self.pointer = None;
        self.hover = None;
        self.pointer_delta = Vec2::ZERO;
        self.zoom = 1.0;
        self.scroll = Vec2::ZERO;
        self.multi = None;
        self.touch = None;
        self.touches.clear();
        self.pen_samples.clear();
        self.touch_cancelled = false;
    }
}

/// The eyedropper while it is running.
#[derive(Clone, Copy, Debug)]
pub struct Eyedrop {
    /// The colour that was active before the eyedropper started.
    pub previous: [u8; 3],
    /// Where the loupe is drawn (screen points).
    pub pos: Option<Pos2>,
    /// The colour under the loupe now.
    pub preview: Option<[u8; 3]>,
}

/// A Hold to Shape that is still being dragged, scaled and rotated.
#[derive(Clone, Debug)]
pub struct ShapeEdit {
    pub base: hold_shape::Shape,
    pub current: hold_shape::Shape,
    pub kind: Kind,
    pub anchor: P,
    pub base_dist: f64,
    pub base_angle: f64,
    /// The pen that made the shape is still down (drag scales and rotates). Once it lifts the
    /// shape stays editable through its handles ("Edit Shape").
    pub live: bool,
    /// The handle being dragged (see `hold_shape::handles`).
    pub drag: Option<usize>,
    /// `Session::edits` right after the shape was painted. A different value means something
    /// else changed the document, which ends Edit Shape.
    pub edits: u64,
}

pub struct EskizApp {
    /// The welcome tour (see [`crate::onboarding`]).
    pub(crate) tour: crate::onboarding::Tour,
    /// Where the sidebar controls the tour points at were drawn last frame.
    pub(crate) tour_rects: crate::onboarding::TargetRects,
    pub(crate) session: Session,
    pub(crate) texture: Option<TextureHandle>,
    /// The GPU compositor on eframe's wgpu device, when there is one (see [`crate::gpu_display`]).
    pub(crate) gpu: Option<crate::gpu_display::GpuDisplay>,
    /// True while the canvas on screen is the GPU composite; `texture` is then unused.
    pub(crate) gpu_active: bool,
    /// Suspend and resume policy (see [`crate::lifecycle`]).
    pub(crate) lifecycle: crate::lifecycle::Lifecycle,
    /// Run the suspend/resume policy (Android; tests switch it on to drive it on the host).
    pub(crate) lifecycle_on: bool,
    /// Window area covered by system bars and cutouts, in points (zero on desktop).
    pub safe: crate::safe_area::SafeArea,
    /// Stroke-to-frame latency samples (debug builds, or `ESKIZ_LATENCY=1`).
    pub(crate) latency: crate::gpu_display::Latency,
    pub(crate) view: View,
    pub(crate) viewport: Rect,
    pub(crate) need_fit: bool,
    /// The viewport height on the previous frame: the first fit waits until it stops changing.
    pub(crate) fit_height_seen: f32,
    /// Viewport size of the previous canvas frame, and the view the last fit produced (to tell
    /// whether the user has zoomed, panned or rotated since).
    pub(crate) last_vp_size: Option<Vec2>,
    pub(crate) fitted_view: Option<View>,
    pub(crate) needs_full: bool,
    /// Id of the selected layer or group. Use `Session::valid_active` after anything that can
    /// remove it (undo, delete, load).
    pub(crate) active: u64,
    pub(crate) tool: Tool,
    pub(crate) tool_params: [ToolParams; 3],
    /// The brush of each tool: Brush, Smudge and Eraser. Kept in prefs.
    pub(crate) tool_brushes: [BrushId; 3],
    /// Which tool the Brush Library popover was opened for.
    pub(crate) library_tool: Tool,
    /// Brush Library: the set picked in the left column, per tool (None = the set of the current brush).
    pub(crate) library_set: [Option<usize>; 3],
    /// The brushes the Brush Library lists, and their engine resources.
    pub(crate) bank: BrushBank,
    /// The Brush Bench sheet, when open (see [`crate::studio`]).
    pub(crate) studio: Option<crate::studio::StudioState>,
    /// Brush Library: the import field.
    pub(crate) brush_ui: BrushUi,
    /// Brush Library: lazily rendered preview textures.
    pub(crate) previews: crate::brush_library::BrushPreviews,
    /// Brush Library: where the rows were drawn last frame.
    pub(crate) library_layout: crate::brush_library::LibraryLayout,
    pub(crate) colour: [u8; 3],
    pub(crate) prefs: Prefs,
    pub(crate) pal: Palette,
    /// The OS asks for reduced motion (read once at start-up, see `a11y`).
    pub(crate) os_reduce_motion: bool,
    applied_look: Option<(bool, theme::UiIdentity)>,
    pub(crate) popovers: Popovers,
    /// Positions of the top-bar buttons from the last frame, for anchoring popovers.
    pub(crate) button_rects: Vec<(Panel, Rect)>,
    /// The widget IDs of those buttons, so Escape can hand keyboard focus back to the opener.
    pub(crate) button_ids: Vec<(Panel, egui::Id)>,
    /// The top bar's "Gallery" button from the last frame (it moves with the macOS inset).
    pub(crate) gallery_button: Rect,
    pub(crate) toast: Option<Toast>,
    pub(crate) now: f64,
    pub(crate) fullscreen: bool,
    /// When the sidebar Pick Color button went down (app clock), while it is held.
    pub(crate) modify_press: Option<f64>,
    /// The sidebar Pick Color button from the last frame, for tests and popovers.
    pub(crate) modify_rect: Rect,
    /// Long-press tracking for the tool buttons: (tool, press start, fired).
    pub(crate) long_press: Option<(Tool, f64, bool)>,
    pub(crate) open_path: String,
    /// The file-dialog backend (`rfd` on desktop, a mock in tests).
    pub(crate) files: std::sync::Arc<dyn crate::file_access::FileAccess>,
    /// Picks asked for and not yet answered; `poll_picks` routes them each frame.
    pub(crate) pending_picks: Vec<crate::file_access::PendingPick>,
    pub(crate) export_path: String,
    pub(crate) fps: f32,
    recognizer: Recognizer,
    /// A sheet over the canvas: Gesture controls, About, the cheat sheet or the copy menu.
    pub(crate) sheet: Option<crate::sheets::Sheet>,
    /// The User guide window: open state, the chapter and the search text.
    pub(crate) guide: crate::user_guide::GuideWindow,
    repeat: Option<RepeatClock>,
    /// Two-finger pinch in progress: (start time, product of zoom deltas).
    pinch: Option<(f64, f32)>,
    eyedrop: Option<Eyedrop>,
    pub(crate) shape_edit: Option<ShapeEdit>,
    /// The floating reference window (see [`crate::reference`]).
    pub(crate) reference: crate::reference::Reference,
    still: Option<StillTimer>,
    /// Canvas-pixel points of the stroke in progress, for Hold to Shape.
    stroke_points: Vec<P>,
    stroke_is_touch: bool,
    stroke_force: Option<f32>,
    stroke_last: Option<Pos2>,
    /// R held: a drag rotates the view.
    rotating: bool,
    /// The Gallery screen, document storage and autosave (see [`crate::gallery`]).
    pub(crate) gal: crate::gallery::Gallery,
    /// Colour panel state (tab, hex field, HSV being edited).
    pub(crate) colour_ui: ColourUi,
    /// The colour before the latest change, for the "previous" half of the swatch.
    pub(crate) colour_prev: [u8; 3],
    /// Colour history and palette, saved between runs.
    pub(crate) colour_store: ColourStore,
    /// A Pour Fill being dragged.
    pub(crate) pour_fill: Option<DropState>,
    /// Pen tablet state.
    pub(crate) pen: PenState,
    /// The adjustment that is open, if any (see [`crate::adjust`]).
    pub(crate) adjust: Option<crate::adjust::AdjustState>,
    /// The Liquify mode, if it is open (see [`crate::liquify_tool`]).
    pub(crate) liquify: Option<crate::liquify_tool::LiquifyTool>,
    /// Export and import jobs running in the background (see [`crate::share`]).
    pub(crate) share: crate::share::ShareState,
    /// Actions panel state: the open tab and the Add tab's path field.
    pub(crate) actions_ui: crate::panels::actions::ActionsUi,
    /// Layers panel state: thumbnails, open controls, drag and swipe (see [`crate::panels::layers`]).
    pub(crate) layers_ui: crate::panels::layers::LayersUi,
    /// Drawing guides and Guided Layer (see [`crate::guides`]).
    pub(crate) guides: crate::guides::GuideState,
    /// Selection or Transform mode (see [`crate::selection_tool`]).
    pub(crate) edit_mode: EditMode,
    /// Selection tool state: shape, combine mode, clipboard, marching ants.
    pub(crate) sel: SelectionTool,
    /// The Crop & Resize sheet, while it is open (see [`crate::canvas_ops`]).
    pub(crate) crop: Option<crate::canvas_ops::CropState>,
    /// The Canvas information sheet is open.
    pub(crate) canvas_info_open: bool,
    /// The transform in progress, if any.
    pub(crate) transform: Option<TransformTool>,
    /// The open 3D document, if any (see [`crate::view3d`]).
    pub(crate) three_d: Option<Box<crate::view3d::ThreeD>>,
    /// The 3D import or open job in flight, and the error sheet.
    pub(crate) model_jobs: crate::view3d::ModelJobs,
    /// Text layers: the on-canvas editor and its state (see [`crate::text_tool`]).
    pub(crate) text: crate::text_tool::TextTool,
    /// Frames: frames, playback and onion skins (see [`crate::animation_ui`]).
    pub(crate) anim: crate::animation_ui::AnimationUi,
    /// Timelapse recorder and replay (see [`crate::timelapse`]).
    pub(crate) timelapse: crate::timelapse::TimelapseUi,
}

/// Top inset of the canvas so the first fit sits under the bar.
pub(crate) const FIT_TOP: f32 = BAR_HEIGHT + 8.0;

/// Is this screen point over an egui area (bars, sidebar, popovers), not the canvas?
pub fn over_ui(ctx: &egui::Context, pos: Pos2) -> bool {
    ctx.layer_id_at(pos).is_some_and(|l| l.order != Order::Background)
}

fn dist(a: P, b: P) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

impl EskizApp {
    pub fn new(ctx: &egui::Context, doc: Document, status: String, prefs: Prefs) -> Self {
        let active = doc.layer_ids().last().copied().unwrap_or(0);
        let mut app = Self {
            tour: crate::onboarding::Tour::idle(),
            tour_rects: crate::onboarding::TargetRects::default(),
            session: Session::new(doc),
            texture: None,
            gpu: None,
            gpu_active: false,
            lifecycle: crate::lifecycle::Lifecycle::default(),
            lifecycle_on: cfg!(target_os = "android"),
            safe: crate::safe_area::SafeArea::ZERO,
            latency: crate::gpu_display::Latency::default(),
            view: View::default(),
            viewport: Rect::NOTHING,
            need_fit: true,
            fit_height_seen: 0.0,
            last_vp_size: None,
            fitted_view: None,
            needs_full: false,
            active,
            tool: Tool::Brush,
            tool_params: [
                ToolParams { size: 24.0, opacity: 1.0 },
                ToolParams { size: 60.0, opacity: 0.5 },
                ToolParams { size: 40.0, opacity: 1.0 },
            ],
            tool_brushes: [default_brush(0), default_brush(1), default_brush(2)],
            library_tool: Tool::Brush,
            library_set: [None; 3],
            studio: None,
            bank: BrushBank::with_stores(BrushStore::default_location(), BrushStore::user_location()),
            brush_ui: BrushUi::default(),
            previews: crate::brush_library::BrushPreviews::default(),
            library_layout: crate::brush_library::LibraryLayout::default(),
            colour: [30, 60, 140],
            pal: theme::palette_for(prefs.identity, prefs.light),
            os_reduce_motion: false,
            prefs,
            applied_look: None,
            popovers: Popovers::default(),
            button_rects: Vec::new(),
            button_ids: Vec::new(),
            gallery_button: Rect::NOTHING,
            toast: None,
            now: 0.0,
            fullscreen: false,
            modify_press: None,
            modify_rect: Rect::NOTHING,
            long_press: None,
            open_path: String::new(),
            files: crate::file_access::default_file_access(),
            pending_picks: Vec::new(),
            // The Share sheet's path is a base name: each format adds its own extension.
            export_path: default_export_path().with_extension("").display().to_string(),
            fps: 0.0,
            recognizer: Recognizer::new(),
            sheet: None,
            guide: crate::user_guide::GuideWindow::default(),
            repeat: None,
            pinch: None,
            eyedrop: None,
            shape_edit: None,
            reference: crate::reference::Reference::default(),
            still: None,
            stroke_points: Vec::new(),
            stroke_is_touch: false,
            stroke_force: None,
            stroke_last: None,
            rotating: false,
            gal: crate::gallery::Gallery::default(),
            colour_ui: ColourUi::default(),
            colour_prev: [30, 60, 140],
            colour_store: ColourStore::default(),
            pour_fill: None,
            pen: PenState::default(),
            adjust: None,
            liquify: None,
            share: crate::share::ShareState::default(),
            actions_ui: crate::panels::actions::ActionsUi::default(),
            layers_ui: crate::panels::layers::LayersUi::default(),
            guides: crate::guides::GuideState::default(),
            edit_mode: EditMode::Off,
            sel: SelectionTool::default(),
            crop: None,
            canvas_info_open: false,
            transform: None,
            three_d: None,
            model_jobs: crate::view3d::ModelJobs::default(),
            text: crate::text_tool::TextTool::default(),
            anim: Default::default(),
            timelapse: Default::default(),
        };
        app.sync_full(ctx);
        if !status.is_empty() {
            app.show_toast(status, 2.5);
        }
        app
    }

    /// Load a saved colour history and palette, and make its current colour active.
    pub fn set_colour_store(&mut self, store: ColourStore) {
        self.colour = store.current;
        self.colour_prev = store.current;
        self.colour_store = store;
    }

    /// A colour counts as used when a stroke or fill is committed with it, or it is picked.
    pub(crate) fn note_colour_used(&mut self, c: [u8; 3]) {
        self.colour_store.push_history(c);
    }

    pub(crate) fn canvas_size(&self) -> (f32, f32) {
        (self.session.doc.width as f32, self.session.doc.height as f32)
    }

    /// Show a transient message for `secs` seconds (the newest replaces the older one).
    pub(crate) fn show_toast(&mut self, text: impl Into<String>, secs: f64) {
        self.toast = Some(Toast {
            text: text.into(),
            until: self.now + secs,
        });
    }

    /// Re-upload the whole flattened canvas. On the GPU path this marks the whole canvas for
    /// recompositing at the end of the frame instead.
    pub(crate) fn sync_full(&mut self, ctx: &egui::Context) {
        if self.gpu_sync_full(ctx) {
            return;
        }
        let doc = &self.session.doc;
        let size = [doc.width, doc.height];
        let image = color_image(size, self.session.display_pixels());
        let reuse = self.texture.as_ref().is_some_and(|t| t.size() == size);
        if let (true, Some(tex)) = (reuse, self.texture.as_mut()) {
            tex.set(image, TEX_OPTIONS);
        } else {
            self.texture = Some(ctx.load_texture("canvas", image, TEX_OPTIONS));
        }
    }

    /// Re-upload one rectangle of the canvas.
    pub(crate) fn refresh(&mut self, rect: PixelRect) {
        if self.gpu_active {
            // Collected here, uploaded and composited once per frame by `gpu_flush`.
            self.gpu_mark_rect(rect);
            return;
        }
        if let Some(tex) = self.texture.as_mut() {
            self.latency.note_input();
            let t = std::time::Instant::now();
            let image = color_image([rect.w, rect.h], self.session.display_pixels_rect(rect));
            tex.set_partial([rect.x, rect.y], image, TEX_OPTIONS);
            self.latency.record_update(t.elapsed());
        }
    }

    pub(crate) fn load_document(&mut self, ctx: &egui::Context, path: &Path) {
        self.cancel_stroke();
        self.shape_edit = None;
        self.adjust_cancel();
        self.exit_edit_mode();
        match open_procreate(path) {
            Ok(doc) => {
                self.session.replace_document(doc);
                self.active = self.session.valid_active(None);
                self.need_fit = true;
                self.sync_full(ctx);
                let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_owned();
                self.set_external_document(title);
                self.show_toast(format!("Opened {}", path.display()), 2.5);
            }
            Err(e) => self.show_toast(format!("Could not open: {e}"), 3.0),
        }
    }

    pub(crate) fn undo_action(&mut self) {
        if self.three_d.is_some() {
            self.step_three_d(false);
            return;
        }
        if self.adjust.is_some() || self.liquify.is_some() {
            return;
        }
        if self.transform.is_some() {
            return;
        }
        // Undo while Edit Shape is open takes the shape back.
        self.shape_edit = None;
        let name = self.session.undo_name().unwrap_or("Stroke");
        let size = (self.session.doc.width, self.session.doc.height);
        if let Some(refresh) = self.session.undo() {
            self.apply_history_refresh(size, refresh);
            self.show_toast(format!("Undo {name}"), 1.0);
        }
    }

    pub(crate) fn redo_action(&mut self) {
        if self.three_d.is_some() {
            self.step_three_d(true);
            return;
        }
        if self.adjust.is_some() || self.liquify.is_some() {
            return;
        }
        if self.transform.is_some() {
            return;
        }
        self.shape_edit = None;
        let name = self.session.redo_name().unwrap_or("Stroke");
        let size = (self.session.doc.width, self.session.doc.height);
        if let Some(refresh) = self.session.redo() {
            self.apply_history_refresh(size, refresh);
            self.show_toast(format!("Redo {name}"), 1.0);
        }
    }

    /// Redraw what an edit changed, and keep the active layer valid (undo or redo may have
    /// removed it).
    pub(crate) fn apply_refresh(&mut self, refresh: Refresh) {
        self.active = self.session.valid_active(Some(self.active));
        match refresh {
            Refresh::None => {}
            Refresh::Rect(rect) => self.refresh(rect),
            Refresh::Everything => self.needs_full = true,
        }
    }

    /// Empty the active layer. One undo step.
    pub(crate) fn clear_active_layer(&mut self) {
        if self.adjust.is_some() {
            return;
        }
        if let Some(edit) = self.session.clear_layer(self.active) {
            self.apply_refresh(edit.refresh);
            self.show_toast("Clear Layer", 1.0);
        } else if self.session.doc.node(self.active).is_some_and(|n| n.props.locked) {
            self.show_toast("The layer is locked.", 2.0);
        }
    }

    pub(crate) fn fit(&mut self) {
        self.need_fit = true;
    }

    pub(crate) fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        self.popovers.close();
    }

    /// Switch tool. Popovers stay as they are.
    pub(crate) fn set_tool(&mut self, tool: Tool) {
        self.end_text_edit();
        self.tool = tool;
    }

    /// Size of the active tool's brush, canvas pixels.
    pub(crate) fn tool_size(&self) -> f32 {
        self.tool_params[self.tool.index()].size
    }

    /// The settings of a stroke with the active tool: its brush, with the tool's size and opacity,
    /// and the brush's tip and grain textures.
    pub(crate) fn stroke_settings(&mut self) -> StrokeSettings {
        let tool = self.tool.index();
        let params = self.tool_params[tool];
        let id = self.tool_brushes[tool].clone();
        let engine = self.bank.engine_or_default(&id, tool);
        let mut brush = engine.brush.clone();
        brush.size = f64::from(params.size);
        brush.opacity = f64::from(params.opacity);
        let [r, g, b] = self.colour;
        StrokeSettings {
            brush,
            colour: [f64::from(r) / 255.0, f64::from(g) / 255.0, f64::from(b) / 255.0],
            erase: self.tool == Tool::Eraser,
            raster: Some(engine.raster.clone()),
        }
    }

    pub(crate) fn cancel_stroke(&mut self) {
        if let Some(rect) = self.session.cancel_stroke() {
            self.refresh(rect);
        }
        self.finish_stroke_state();
    }

    /// Palm rejection: when the OS cancels a contact (Android `ACTION_CANCEL` / `FLAG_CANCELED`)
    /// the stroke it drew is thrown away, not committed: the layer goes back to its tiles from
    /// before the stroke and no undo step is made. A cancelled finger cannot kill a pen stroke
    /// and the other way round.
    pub(crate) fn palm_cancel(&mut self, input: &mut FrameInput) {
        let pen_stroke = self.pen.stroke_reading.is_some();
        let hit = (self.pen.frame.cancelled && pen_stroke) || (input.touch_cancelled && !pen_stroke);
        if input.touch_cancelled && !self.pen.frame.pressed {
            // A finger that began and was cancelled in the same frame never starts a stroke.
            input.primary_pressed = false;
        }
        if hit && self.session.is_stroking() {
            self.cancel_stroke();
        }
    }

    fn finish_stroke_state(&mut self) {
        self.stroke_last = None;
        self.still = None;
        self.stroke_points.clear();
        self.pen.stroke_reading = None;
    }

    /// Start a stroke at a screen point, with the active tool.
    fn start_stroke(&mut self, pos: Pos2, time: f64, touch: bool, force: Option<f32>) {
        if !self.session.can_paint(self.active) {
            let doc = &self.session.doc;
            let msg = if doc.node(self.active).is_none_or(|n| n.is_group()) {
                "Select a layer to paint on."
            } else if doc.effective_locked(self.active) {
                "The active layer is locked."
            } else {
                "The active layer is hidden. Show it to paint."
            };
            self.show_toast(msg, 2.0);
            return;
        }
        self.popovers.close();
        let started = if self.tool == Tool::Smudge {
            let params = self.tool_params[Tool::Smudge.index()];
            let id = self.tool_brushes[Tool::Smudge.index()].clone();
            let tip = self.bank.engine_or_default(&id, Tool::Smudge.index()).tip.clone();
            self.session.begin_smudge(
                self.active,
                f64::from(params.size) / 2.0,
                f64::from(params.opacity),
                Some(tip),
            )
        } else {
            let settings = self.stroke_settings();
            self.session.set_canvas_rotation(-f64::from(self.view.rotation));
            match self.assist_for_stroke() {
                Some(assist) => self.session.begin_assisted_stroke(self.active, &settings, &assist),
                None => self.session.begin_stroke(self.active, &settings),
            }
        };
        if started {
            self.stroke_is_touch = touch;
            self.stroke_force = force;
            self.stroke_points.clear();
            let (x, y) = self.view.screen_to_canvas(self.viewport, pos);
            self.still = Some(StillTimer::with_secs((x, y), time, f64::from(self.prefs.hold_shape_secs)));
            self.stroke_point(pos, time);
        }
    }

    /// Send one pointer position to the stroke in progress.
    fn stroke_point(&mut self, pos: Pos2, time: f64) {
        let (x, y) = self.view.screen_to_canvas(self.viewport, pos);
        let kind = if self.stroke_is_touch {
            PointerKind::Touch
        } else {
            PointerKind::Mouse
        };
        let raw = RawPointer {
            x,
            y,
            time,
            kind,
            force: self.stroke_force.map(f64::from),
        };
        self.stroke_last = Some(pos);
        if self.stroke_points.len() < 50_000 {
            self.stroke_points.push((x, y));
        }
        let sample = match self.pen.stroke_reading {
            Some(reading) => sample_from_pen(x, y, time, reading),
            None => sample_from(raw),
        };
        // The global pressure curve covers touch force and pen pressure.
        let sample = apply_pressure_curve(sample, &self.prefs.pressure_curve);
        if let Some(rect) = self.session.push(sample) {
            self.refresh(rect);
        }
    }

    fn end_stroke(&mut self) {
        if self.session.is_stroking() {
            let paints_colour = self.tool == Tool::Brush;
            if let Some(rect) = self.session.end_stroke() {
                self.refresh(rect);
                if paints_colour {
                    self.note_colour_used(self.colour);
                }
            }
        }
        self.finish_stroke_state();
    }

    /// Hold to Shape check, once per frame while a stroke is held.
    fn check_hold_shape(&mut self, pointer: Pos2, time: f64) {
        if !self.prefs.hold_shape || self.tool == Tool::Smudge || self.stroke_points.len() < 3 {
            return;
        }
        let p = self.view.screen_to_canvas(self.viewport, pointer);
        let Some(timer) = self.still.as_mut() else {
            return;
        };
        if timer.update(p, time) {
            self.try_hold_shape(time);
        }
    }

    /// Replace the stroke in progress with the best-fit shape, if it has one.
    fn try_hold_shape(&mut self, time: f64) {
        let points = std::mem::take(&mut self.stroke_points);
        let Some(fit) = hold_shape::classify(&points) else {
            self.stroke_points = points;
            return;
        };
        self.cancel_stroke();
        let hold = points[points.len() - 1];
        let anchor = hold_shape::anchor(&fit.shape);
        self.shape_edit = Some(ShapeEdit {
            base: fit.shape.clone(),
            current: fit.shape.clone(),
            kind: fit.kind,
            anchor,
            base_dist: dist(hold, anchor).max(8.0),
            base_angle: (hold.1 - anchor.1).atan2(hold.0 - anchor.0),
            live: true,
            drag: None,
            edits: 0,
        });
        self.paint_shape(&fit.shape, time);
        let edits = self.session.edits();
        if let Some(e) = self.shape_edit.as_mut() {
            e.edits = edits;
        }
        self.show_toast(format!("Shape created: {} \u{b7} Edit Shape", fit.kind.name()), 2.4);
    }

    /// Paint a shape as one stroke with the current brush. Refreshes the canvas.
    pub(crate) fn paint_shape(&mut self, shape: &hold_shape::Shape, time: f64) -> bool {
        let settings = self.stroke_settings();
        if !self.session.begin_stroke(self.active, &settings) {
            return false;
        }
        let mut dirty: Option<PixelRect> = None;
        let mut outline = hold_shape::outline(shape, 2.0);
        // A closed shape runs on past its start (a sixth of its length), so the end taper lies
        // over solid ink and the join leaves no gap.
        if outline.len() > 16 && dist(outline[0], outline[outline.len() - 1]) < 1e-6 {
            let extra = (outline.len() / 6).max(4);
            let more: Vec<P> = outline[1..=extra].to_vec();
            outline.extend(more);
        }
        for (i, (x, y)) in outline.into_iter().enumerate() {
            let raw = RawPointer {
                x,
                y,
                time: time + i as f64 / 120.0,
                kind: PointerKind::Mouse,
                force: None,
            };
            if let Some(r) = self.session.push(sample_from(raw)) {
                dirty = Some(dirty.map_or(r, |d| d.union(r)));
            }
        }
        if let Some(r) = self.session.end_stroke() {
            dirty = Some(dirty.map_or(r, |d| d.union(r)));
            if self.tool == Tool::Brush {
                self.note_colour_used(self.colour);
            }
        }
        match dirty {
            Some(r) => {
                self.refresh(r);
                true
            }
            None => false,
        }
    }

    /// Drag after a Hold to Shape: scale and rotate about the anchor, then constrain.
    fn shape_edit_frame(&mut self, input: &FrameInput) {
        let Some(edit) = self.shape_edit.clone() else {
            return;
        };
        let Some(pointer) = input.pointer else {
            return;
        };
        let p = self.view.screen_to_canvas(self.viewport, pointer);
        let constrained = input.shift || input.multi.is_some_and(|m| m.num_touches >= 2);
        let scale = (dist(p, edit.anchor) / edit.base_dist).clamp(0.05, 20.0);
        let rot = (p.1 - edit.anchor.1).atan2(p.0 - edit.anchor.0) - edit.base_angle;
        let mut next = hold_shape::transform(&edit.base, edit.anchor, scale, rot);
        if constrained {
            next = hold_shape::constrain(&next);
        }
        if next == edit.current {
            return;
        }
        if let Some(refresh) = self.session.undo() {
            self.apply_refresh(refresh);
        }
        self.paint_shape(&next, input.time);
        let edits = self.session.edits();
        if let Some(e) = self.shape_edit.as_mut() {
            e.current = next;
            e.edits = edits;
        }
    }

    /// Colour under a screen point: the average of a 5 x 5 patch of the flattened canvas.
    pub(crate) fn sample_at(&self, pos: Pos2) -> Option<[u8; 3]> {
        let (fx, fy) = self.view.screen_to_canvas(self.viewport, pos);
        let (x, y) = (fx.floor() as i64, fy.floor() as i64);
        let (cw, ch) = (self.session.doc.width, self.session.doc.height);
        if x < 0 || y < 0 || x >= cw as i64 || y >= ch as i64 {
            return None;
        }
        let r = 2i64;
        let rect = PixelRect::from_inclusive(
            x - r,
            y - r,
            x + r,
            y + r,
            cw,
            ch,
        )?;
        let buf = self.session.flatten_rect(rect);
        eyedropper::sample_region(
            &buf,
            rect.w,
            rect.h,
            x - rect.x as i64,
            y - rect.y as i64,
            r,
        )
    }

    pub(crate) fn begin_eyedrop(&mut self, pos: Pos2) {
        self.cancel_stroke();
        self.eyedrop = Some(Eyedrop {
            previous: self.colour,
            pos: Some(pos),
            preview: self.sample_at(pos),
        });
    }

    fn commit_eyedrop(&mut self) {
        if let Some(e) = self.eyedrop.take() {
            if let Some(c) = e.preview {
                self.colour_prev = e.previous;
                self.colour = c;
                self.note_colour_used(c);
                self.show_toast(format!("Picked {}", eyedropper::hex(c)), 1.2);
            }
        }
    }

    /// Holding the sidebar Pick Color button starts the eyedropper once the hold reaches the
    /// Preferences delay (0.4 s by default). The loupe then follows the pointer onto the canvas
    /// and commits on release. A tap that lifts sooner starts nothing. Off with the
    /// "Eyedropper on hold" switch, like the one-finger hold.
    fn modify_hold_frame(&mut self, ctx: &egui::Context, input: &FrameInput) {
        let Some(start) = self.modify_press else {
            return;
        };
        if !self.prefs.eyedrop_hold || !input.primary_down || self.eyedrop.is_some() || self.shape_edit.is_some() {
            return;
        }
        let delay = f64::from(self.prefs.eyedrop_secs);
        let held = input.time - start;
        if held < delay {
            ctx.request_repaint_after(Duration::from_secs_f64(delay - held));
            return;
        }
        if let Some(p) = input.pointer.or(input.press_origin) {
            self.begin_eyedrop(p);
        }
    }

    fn update_fps(&mut self, ctx: &egui::Context) {
        let dt = ctx.input(|i| i.stable_dt);
        if dt > 0.0 {
            let instant = 1.0 / dt;
            self.fps = if self.fps == 0.0 {
                instant
            } else {
                self.fps * 0.9 + instant * 0.1
            };
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        // Brush files go to the Brush Library, whatever screen is showing.
        let (brushes, dropped): (Vec<PathBuf>, Vec<PathBuf>) =
            dropped.into_iter().partition(|p| crate::brush_import::is_brush_file(p));
        for path in &brushes {
            self.import_brush_file(path);
        }
        if self.gallery_active() {
            // On the gallery, dropped files are imported into the library.
            if !dropped.is_empty() {
                self.import_into_gallery(ctx, dropped);
            }
        } else if let Some(path) = dropped.first() {
            if crate::import::is_import_file(path) || crate::view3d::is_model_file(path) {
                // A picture or a PSD / .procreate file becomes new layers above the active one.
                self.start_import_image(ctx, path);
            } else if self.three_d.is_some() {
                self.show_toast("Leave the 3D view to open a 2D document.", 3.0);
            } else {
                self.load_document(ctx, path);
            }
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        self.pal = theme::palette_for(self.prefs.identity, self.prefs.light);
        if self.applied_look != Some((self.prefs.light, self.prefs.identity)) {
            // Fonts change only when the identity does (Classic keeps egui's defaults untouched).
            let before = self.applied_look.map_or(theme::UiIdentity::Classic, |(_, id)| id);
            if before != self.prefs.identity {
                crate::fonts::install(ctx, self.prefs.identity);
            }
            theme::apply(ctx, &self.pal);
            // Ask the OS to draw window chrome (macOS title bar, Windows frame) in the same appearance.
            let system = if self.prefs.light {
                egui::SystemTheme::Light
            } else {
                egui::SystemTheme::Dark
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(system));
            self.applied_look = Some((self.prefs.light, self.prefs.identity));
        }
    }

    /// Whether animations should be skipped: the Prefs switch, or the OS request.
    pub(crate) fn reduce_motion(&self) -> bool {
        self.prefs.reduce_motion || self.os_reduce_motion
    }

    /// Keyboard shortcuts, when no text field has focus.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        self.text_tick(ctx);
        if self.studio.is_some() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) && !ctx.egui_wants_keyboard_input() {
                self.close_studio();
            }
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // The bindings are the constants in `shortcuts`, which the shortcut table also lists.
        use crate::shortcuts as sc;
        if ctx.input_mut(|i| i.consume_shortcut(&sc::REDO)) {
            self.redo_action();
        } else if ctx.input_mut(|i| i.consume_shortcut(&sc::UNDO)) {
            self.undo_action();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc::FIT)) {
            self.fit();
            self.show_toast("Fit to screen", 1.0);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc::CLEAR_LAYER)) {
            self.clear_active_layer();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&sc::COPY)) {
            self.clip_action(ClipAction::Copy);
        } else if ctx.input_mut(|i| i.consume_shortcut(&sc::CUT)) {
            self.clip_action(ClipAction::Cut);
        } else if ctx.input_mut(|i| i.consume_shortcut(&sc::PASTE)) {
            self.clip_action(ClipAction::Paste);
        }
        if self.text.editing.is_some() && self.three_d.is_none() {
            // While typing, the keys go to the text, not to the tool shortcuts below.
            self.text_keys(ctx);
            return;
        }
        if self.edit_mode != EditMode::Off && ctx.input(|i| i.key_pressed(sc::ESCAPE)) {
            self.exit_edit_mode();
        }
        if self.transform.is_some() && ctx.input(|i| i.key_pressed(sc::ENTER)) {
            self.apply_transform();
        }
        // Tab is keyboard focus navigation, so full screen is F only.
        if ctx.input(|i| i.key_pressed(sc::FULLSCREEN) && i.modifiers == Modifiers::NONE) {
            self.toggle_fullscreen();
        }
        if ctx.input(|i| i.key_pressed(sc::ESCAPE)) && self.shape_edit.as_ref().is_some_and(|e| !e.live) {
            self.shape_edit = None;
        }
        if ctx.input(|i| i.key_pressed(sc::ESCAPE)) && !(self.popovers.open_panel() == Some(Panel::Layers) && self.layers_ui.wants_escape()) {
            // Escape returns keyboard focus to the button that opened the popover.
            if let Some(panel) = self.popovers.open_panel() {
                if let Some((_, id)) = self.button_ids.iter().find(|(p, _)| *p == panel) {
                    ctx.memory_mut(|m| m.request_focus(*id));
                }
            }
            self.popovers.close();
        }
        let plain = |key: Key| ctx.input(|i| i.key_pressed(key) && i.modifiers == Modifiers::NONE);
        if plain(sc::BRUSH) {
            self.set_tool(Tool::Brush);
        }
        if plain(sc::SMUDGE) {
            self.set_tool(Tool::Smudge);
        }
        if plain(sc::ERASER) {
            self.set_tool(Tool::Eraser);
        }
        if plain(sc::LAYERS) {
            let rect = self.anchor_rect(Panel::Layers);
            self.popovers.toggle(Panel::Layers, rect);
        }
        let shrink = ctx.input(|i| i.key_pressed(sc::SMALLER_BRUSH));
        let grow = ctx.input(|i| i.key_pressed(sc::LARGER_BRUSH));
        if shrink || grow {
            let p = &mut self.tool_params[self.tool.index()];
            p.size = (p.size * if grow { 1.05 } else { 0.95 }).clamp(1.0, MAX_SIZE);
            let size = p.size;
            self.show_toast(format!("Size {size:.0} px"), 1.0);
        }
    }

    /// Where a popover for `panel` should hang from: its button, else the top-right corner.
    pub(crate) fn anchor_rect(&self, panel: Panel) -> Rect {
        self.button_rects
            .iter()
            .find(|(p, _)| *p == panel)
            .map(|(_, r)| *r)
            .unwrap_or_else(|| Rect::from_min_size(self.viewport.right_top(), Vec2::new(0.0, 0.0)))
    }

    pub(crate) fn read_input(&self, ui: &egui::Ui) -> FrameInput {
        let mut input = self.read_egui_input(ui);
        self.overlay_pen(&mut input);
        input
    }

    /// Let a pen tablet drive the pointer fields. While the pen touches the surface it is
    /// the only source of press, drag and release (the OS may also send them as mouse input).
    /// Hovering only moves the cursor, so a mouse keeps working when the pen is idle in range.
    fn overlay_pen(&self, input: &mut FrameInput) {
        let f: &PenFrame = &self.pen.frame;
        if !f.active {
            return;
        }
        if let Some(pos) = f.pos {
            if f.moved || f.down {
                input.hover = Some(pos);
                input.pointer = Some(pos);
            }
        }
        if f.pressed || f.down || f.released {
            input.primary_pressed = f.pressed;
            input.primary_down = f.down;
            input.primary_released = f.released;
            input.press_origin = f.samples.first().map(|s| s.pos).or(f.pos).or(input.press_origin);
            input.pen_samples = f.samples.clone();
        }
    }

    fn read_egui_input(&self, ui: &egui::Ui) -> FrameInput {
        ui.input(|i| {
            let mut touch = None;
            let mut touches = Vec::new();
            let mut touch_cancelled = false;
            for e in &i.events {
                if let egui::Event::Touch { id, phase, pos, force, .. } = e {
                    let id = id.0;
                    match phase {
                        TouchPhase::Start => {
                            touch = Some(*force);
                            touches.push(TouchEv::Down(id, *pos, i.time));
                        }
                        TouchPhase::Move => {
                            touch = Some(*force);
                            touches.push(TouchEv::Move(id, *pos, i.time));
                        }
                        TouchPhase::End => touches.push(TouchEv::Up(id, i.time)),
                        TouchPhase::Cancel => {
                            touch_cancelled = true;
                            touches.push(TouchEv::Up(id, i.time));
                        }
                    }
                }
            }
            FrameInput {
                space: i.key_down(Key::Space),
                middle: i.pointer.button_down(PointerButton::Middle),
                alt: i.modifiers.alt,
                shift: i.modifiers.shift,
                r_held: i.key_down(Key::R),
                primary_pressed: i.pointer.primary_pressed(),
                primary_down: i.pointer.primary_down(),
                primary_released: i.pointer.primary_released(),
                press_origin: i.pointer.press_origin(),
                pointer: i.pointer.latest_pos(),
                hover: i.pointer.hover_pos(),
                pointer_delta: i.pointer.delta(),
                zoom: i.zoom_delta(),
                scroll: i.smooth_scroll_delta,
                multi: i.multi_touch(),
                time: i.time,
                touch,
                touches,
                pen_samples: Vec::new(),
                touch_cancelled,
            }
        })
    }

    /// Feed touch events to the recogniser and act on what it reports.
    fn process_touches(&mut self, ctx: &egui::Context, input: &FrameInput) {
        self.recognizer.set_eyedropper_secs(f64::from(self.prefs.eyedrop_secs));
        let mut gestures = Vec::new();
        for ev in &input.touches {
            gestures.extend(match *ev {
                TouchEv::Down(id, p, t) => self.recognizer.down(id, p, t),
                TouchEv::Move(id, p, t) => self.recognizer.moved(id, p, t),
                TouchEv::Up(id, t) => self.recognizer.up(id, t),
            });
        }
        gestures.extend(self.recognizer.tick(input.time));
        if self.recognizer.active() == 0 {
            self.repeat = None;
        }
        for g in gestures {
            self.on_gesture(ctx, g, input.time);
        }
    }

    /// The gesture switches from Preferences.
    fn gesture_switches(&self) -> gesture::Gestures {
        gesture::Gestures {
            eyedropper: self.prefs.eyedrop_hold,
            undo_taps: self.prefs.undo_gestures,
            repeat_undo: self.prefs.repeat_undo,
            scrub_clear: self.prefs.scrub_clear,
            swipe_copy: self.prefs.swipe_copy,
            fullscreen: self.prefs.fullscreen_taps,
        }
    }

    /// Run what a recognised gesture asks for, if its switch is on.
    fn on_gesture(&mut self, ctx: &egui::Context, g: Gesture, time: f64) {
        let Some(cmd) = gesture::route(g, &self.gesture_switches()) else {
            return;
        };
        match cmd {
            gesture::Command::Undo => self.undo_action(),
            gesture::Command::Redo => self.redo_action(),
            gesture::Command::RepeatUndo => {
                let mut clock = RepeatClock::default();
                clock.start(time);
                self.repeat = Some(clock);
            }
            gesture::Command::ClearLayer => self.clear_active_layer(),
            gesture::Command::ShowCopyMenu => self.sheet = Some(crate::sheets::Sheet::CopyMenu),
            gesture::Command::ToggleFullscreen => self.toggle_fullscreen(),
            gesture::Command::Eyedropper(pos) => {
                if self.adjust.is_none() && self.eyedrop.is_none() {
                    self.begin_eyedrop(pos);
                }
            }
        }
        let _ = ctx;
    }

    /// Two or more fingers: pan, pinch-zoom and rotate, with snapping and a quick-pinch fit.
    fn multi_touch_frame(&mut self, mt: MultiTouchInfo) {
        if self.session.is_stroking() {
            self.cancel_stroke();
        }
        let (zoom_before, rot_before) = (self.view.zoom, self.view.rotation);
        if self.shape_edit.is_none() && self.prefs.touch_navigation {
            self.view.pan(mt.translation_delta);
            let next_rot = rot_before + mt.rotation_delta;
            let (rot_next, rot_snapped) = view::snap_rotation(rot_before, next_rot);
            if rot_next != rot_before {
                let viewport = self.viewport;
                self.view.rotate_at(viewport, mt.center_pos, rot_next - rot_before);
            }
            let (zoom_next, zoom_snapped) = view::snap_zoom(zoom_before, zoom_before * mt.zoom_delta);
            if (zoom_next / zoom_before - 1.0).abs() > 1e-6 {
                let viewport = self.viewport;
                self.view.zoom_at(viewport, mt.center_pos, zoom_next / zoom_before);
            }
            if zoom_snapped {
                self.show_toast("Zoom 100%", 1.0);
            } else if self.view.zoom != zoom_before {
                let pct = self.view.zoom * 100.0;
                self.show_toast(format!("Zoom {pct:.0}%"), 1.0);
            }
            if rot_snapped {
                self.show_toast("Rotation 0°", 1.0);
            } else if self.view.rotation != rot_before {
                let deg = self.view.rotation.to_degrees().rem_euclid(360.0);
                self.show_toast(format!("Rotation {deg:.0}°"), 1.0);
            }
        }
        let pinch = self.pinch.get_or_insert((self.now, 1.0));
        pinch.1 *= mt.zoom_delta;
    }

    /// The window changed shape by a rotation (landscape <-> portrait): refit the canvas the way
    /// opening a document does, unless the user has zoomed, panned or rotated since the last
    /// fit; then keep the canvas point at the centre of the viewport at the centre. Other size
    /// changes (a desktop window being dragged larger) leave the view alone, as before.
    fn on_viewport_resize(&mut self, vp: Rect) {
        let size = vp.size();
        let prev = self.last_vp_size.replace(size);
        let Some(prev) = prev.filter(|p| p.x > 1.0 && p.y > 1.0 && size.x > 1.0 && size.y > 1.0) else {
            return;
        };
        if (prev.x > prev.y) == (size.x > size.y) || self.need_fit {
            return;
        }
        if self.fitted_view == Some(self.view) {
            self.need_fit = true;
        } else {
            let old = Rect::from_min_size(vp.min, prev);
            let (cx, cy) = self.view.screen_to_canvas(old, old.center());
            let now = self.view.canvas_to_screen(vp, cx, cy);
            self.view.offset += vp.center() - now;
        }
    }

    fn canvas_frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let vp = ui.max_rect();
        self.viewport = vp;
        let (cw, ch) = self.canvas_size();
        // Panels such as the timeline strip are measured a frame late, so fit once the height has settled.
        let settled = (vp.height() - self.fit_height_seen).abs() < 0.5;
        self.fit_height_seen = vp.height();
        self.on_viewport_resize(vp);
        if self.need_fit && vp.width() > 1.0 && settled {
            let inner = Rect::from_min_max(
                vp.min + Vec2::new(16.0 + self.safe.left, FIT_TOP + self.safe.top),
                vp.max - Vec2::new(16.0 + self.safe.right, 16.0 + self.safe.bottom),
            );
            let mut v = View::fit(inner, cw, ch);
            v.offset += inner.min - vp.min;
            self.view = v;
            self.fitted_view = Some(v);
            self.need_fit = false;
        }
        let mut input = self.read_input(ui);
        if self.studio.is_some() {
            // The Brush Bench sheet is modal: the canvas underneath hears nothing.
            input.silence();
        }
        self.palm_cancel(&mut input);
        let time = input.time;

        self.process_touches(&ctx, &input);
        if let Some(n) = self.repeat.as_mut().map(|c| c.advance(time)) {
            for _ in 0..n {
                self.undo_action();
            }
        }

        // Pinch and rotate, or a single pointer.
        match input.multi.filter(|m| m.num_touches >= 2) {
            Some(mt) => self.multi_touch_frame(mt),
            None => {
                if let Some((start, scale)) = self.pinch.take() {
                    if gesture::is_quick_pinch(time - start, scale) {
                        self.fit();
                        self.show_toast("Fit to screen", 1.0);
                    }
                }
                self.single_pointer_frame(&ctx, &input);
            }
        }

        self.draw_canvas(ui, vp, cw, ch, &input);
        self.draw_text_overlay(ui);
        self.text_style_window(&ctx);
    }

    /// Pan, zoom and rotate with the wheel and keys, and paint with the pointer.
    fn single_pointer_frame(&mut self, ctx: &egui::Context, input: &FrameInput) {
        let viewport = self.viewport;
        if input.primary_pressed {
            if let Some(p) = input.press_origin {
                if !self.popovers.contains(p) {
                    self.popovers.close();
                }
            }
        }
        if input.zoom != 1.0 {
            let anchor = input.hover.unwrap_or(viewport.center());
            self.view.zoom_at(viewport, anchor, input.zoom);
        } else if input.hover.is_some_and(|p| viewport.contains(p)) && input.scroll != Vec2::ZERO {
            self.view.pan(input.scroll);
        }
        let panning = input.space || input.middle;
        if panning && input.primary_down && input.pointer_delta != Vec2::ZERO {
            self.view.pan(input.pointer_delta);
        }
        if self.liquify.is_some() {
            // Liquify mode: a drag on the canvas warps the layer; nothing paints.
            if !panning {
                self.liquify_pointer_frame(ctx, input);
            }
            return;
        }
        if self.crop.is_some() {
            // Crop mode: the handles are the only thing a drag changes, so nothing paints.
            return;
        }
        if self.adjust.is_some() {
            // Adjustment mode: a drag on the canvas changes the value; nothing paints.
            if !panning {
                self.adjust_pointer_frame(ctx, input);
            }
            return;
        }
        let press_on_ui = input
            .press_origin
            .is_some_and(|p| over_ui(ctx, p));
        if self.text.editing.is_some() {
            if !panning {
                self.text_pointer_frame(input, press_on_ui);
            }
            return;
        }
        if self.edit_mode != EditMode::Off {
            if !panning {
                self.edit_pointer(input, press_on_ui);
            }
            return;
        }
        // Pen-only use: a finger never starts a stroke when finger painting is off.
        let finger_paints = input.touch.is_none() || self.prefs.touch_painting;
        if input.primary_pressed && !press_on_ui && !panning && finger_paints {
            self.rotating = input.r_held;
            if let Some(pos) = input.press_origin {
                if self.shape_edit.is_none() && self.eyedrop.is_none() {
                    // Alt/Option starts the eyedropper at once. The Pick Color button waits for its hold (see
                    // `modify_hold_frame`).
                    if input.alt {
                        self.begin_eyedrop(pos);
                    } else if !self.rotating {
                        let first = input.pen_samples.first();
                        self.pen.stroke_reading = first.map(|s| s.reading);
                        let time = first.map_or_else(|| time_of(input), |s| s.time);
                        self.start_stroke(pos, time, input.touch.is_some(), input.touch.flatten());
                        if !self.session.is_stroking() {
                            self.pen.stroke_reading = None;
                        }
                    }
                }
            }
        }
        if self.rotating {
            if input.primary_down && input.pointer_delta.x != 0.0 {
                if let Some(p) = input.pointer {
                    self.view.rotate_at(viewport, p, input.pointer_delta.x * 0.01);
                }
            }
            if input.primary_released || !input.primary_down {
                self.rotating = false;
            }
            return;
        }
        if panning {
            return;
        }
        self.modify_hold_frame(ctx, input);
        if let Some(e) = self.eyedrop.as_mut() {
            if let Some(p) = input.pointer {
                e.pos = Some(p);
            }
        }
        if self.eyedrop.is_some() {
            if let Some(p) = input.pointer {
                let preview = self.sample_at(p);
                if let Some(e) = self.eyedrop.as_mut() {
                    e.preview = preview;
                }
            }
            if input.primary_released || !input.primary_down {
                self.commit_eyedrop();
            }
            return;
        }
        if let Some(live) = self.shape_edit.as_ref().map(|e| e.live) {
            if live && input.primary_down {
                self.shape_edit_frame(input);
                return;
            }
            if live {
                // The pen lifted: the shape stays open for Edit Shape.
                if let Some(e) = self.shape_edit.as_mut() {
                    e.live = false;
                }
                return;
            }
            self.shape_handles_frame(input, press_on_ui);
            return;
        }
        if self.session.is_stroking() {
            if self.pen.stroke_reading.is_some() {
                // A pen stroke: every sample the tablet reported this frame, not just the
                // last pointer position. The press sample was already sent by `start_stroke`.
                for s in input.pen_samples.iter().skip(usize::from(input.primary_pressed)) {
                    self.pen.stroke_reading = Some(s.reading);
                    self.stroke_point(s.pos, s.time);
                }
                if input.primary_down {
                    if let Some(p) = input.pointer {
                        self.check_hold_shape(p, input.time);
                    }
                }
            } else if input.primary_down {
                if let Some(p) = input.pointer {
                    if Some(p) != self.stroke_last {
                        if let Some(force) = input.touch {
                            self.stroke_force = force;
                        }
                        self.stroke_point(p, input.time);
                    }
                    self.check_hold_shape(p, input.time);
                }
            }
            if (!input.primary_down || input.primary_released) && self.session.is_stroking() {
                self.end_stroke();
            }
        }
    }

    /// Canvas shadow, the canvas itself (rotated mesh) and the cursor overlays.
    fn draw_canvas(&mut self, ui: &mut egui::Ui, vp: Rect, cw: f32, ch: f32, input: &FrameInput) {
        let painter = ui.painter_at(vp);
        let corners = self.view.corners(vp, cw, ch);
        painter.add(Shape::mesh(canvas_shadow_mesh(corners)));
        if let Some(tex_id) = self.canvas_texture_id() {
            let mut mesh = Mesh::with_texture(tex_id);
            let uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
            for (p, (u, v)) in corners.iter().zip(uvs) {
                mesh.vertices.push(Vertex {
                    pos: *p,
                    uv: Pos2::new(u, v),
                    color: Color32::WHITE,
                });
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(Shape::mesh(mesh));
        }

        self.draw_guides(&painter);
        match self.edit_mode {
            EditMode::Select => self.selection_overlay(&painter),
            EditMode::Transform => self.transform_overlay(&painter),
            EditMode::Off => {}
        }

        // Edit Shape handles go over the canvas, under the cursor.
        self.draw_shape_handles(&painter);
        let pal = self.pal;
        if self.prefs.brush_cursor && self.eyedrop.is_none() && self.edit_mode == EditMode::Off && !self.editing_shape() {
            if let Some(p) = input.hover {
                if vp.contains(p) && !over_ui(ui.ctx(), p) {
                    let r = (self.tool_size() / 2.0 * self.view.zoom).max(1.0);
                    painter.circle_stroke(p, r, Stroke::new(2.0, Color32::from_black_alpha(90)));
                    painter.circle_stroke(p, r, Stroke::new(1.0, Color32::from_white_alpha(230)));
                }
            }
        }
        if let Some(e) = self.eyedrop {
            if let Some(p) = e.pos {
                let new = e.preview.unwrap_or(e.previous);
                eyedropper::draw_loupe(&painter, p, Color32::from_rgb(new[0], new[1], new[2]), Color32::from_rgb(e.previous[0], e.previous[1], e.previous[2]), pal.text);
            }
        }
        if self.session.is_stroking() || self.eyedrop.is_some() || self.shape_edit.is_some() {
            ui.ctx().request_repaint();
        }
    }

    /// One frame: the whole UI, then the GPU display update (strokes that arrived this frame are
    /// uploaded and composited before egui renders).
    pub(crate) fn ui_root(&mut self, ui: &mut egui::Ui) {
        // Colours the interface paints (swatches, the colour disc) are in the document's space.
        crate::colour_management::set_ui_space(self.session.doc.colour_space);
        self.ui_body(ui);
        let ctx = ui.ctx().clone();
        self.gpu_flush(&ctx);
    }

    fn ui_body(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        self.apply_pen_eraser();
        self.update_fps(&ctx);
        self.apply_theme(&ctx);
        self.poll_worker();
        self.recovery_sheet(&ctx);
        self.journal_tick(&ctx);
        self.handle_drops(&ctx);
        self.poll_picks(&ctx);
        self.model_frame(&ctx);
        if self.gallery_active() {
            // A document that is still dirty here (its save failed) is retried.
            self.autosave_tick(&ctx);
            self.tour_poll();
            self.gallery_root(ui);
            self.tour_cards(&ctx);
            self.tour_watch_input(&ctx);
            return;
        }
        self.handle_keys(&ctx);
        self.poll_share();
        self.timelapse_frame(&ctx);
        self.adjust_frame(&ctx);
        if self.needs_full {
            self.needs_full = false;
            self.sync_full(&ctx);
        }
        self.animation_panel(ui);
        self.liquify_bar(ui);
        let backdrop = self.pal.backdrop;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(backdrop))
            .show(ui, |ui| {
                if self.three_d.is_some() {
                    self.three_d_frame(ui);
                } else {
                    self.canvas_frame(ui);
                }
            });
        self.chrome(&ctx);
        self.tour_marks(&ctx);
        self.autosave_tick(&ctx);
        self.pour_fill_frame(&ctx);
        self.guides_frame(&ctx);
        self.saved_selections_frame();
        if !self.gallery_active() {
            self.crop_frame(&ctx);
            self.canvas_info_window(&ctx);
        }
        if self.toast.as_ref().is_some_and(|t| t.until > self.now - 0.3) {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

/// Time of the input frame (kept as a helper so the call site stays readable).
fn time_of(input: &FrameInput) -> f64 {
    input.time
}

impl eframe::App for EskizApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if !self.gallery_active() && self.three_d.is_none() {
            let dt = f64::from(ui.ctx().input(|i| i.stable_dt)).min(0.25);
            self.session.add_time_spent(dt);
        }
        if self.lifecycle_on {
            let ctx = ui.ctx().clone();
            self.lifecycle_frame(&ctx);
        }
        self.ui_root(ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.lifecycle_on {
            // eframe calls `save` when the activity is suspended, but also every 30 s while
            // it runs. Only a real trip to the background (the Java shim saw `onStop`) is a
            // suspend checkpoint: the journal and the preferences go to disk before it
            // returns (the process may be killed next). A periodic save only stores the
            // preferences: it must not cancel a stroke or arm a resume.
            if crate::android_queue::backgrounded() {
                self.lifecycle_suspend(storage);
            } else {
                self.persist_prefs(storage);
            }
        } else {
            self.persist_prefs(storage);
        }
        self.save_on_exit();
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if cfg!(target_os = "android") {
            self.safe = crate::safe_area::SafeArea::from_pixels(crate::safe_area::pixels(), ctx.pixels_per_point());
        }
        if cfg!(target_os = "android") || !self.safe.is_zero() {
            raw_input.safe_area_insets = Some(self.safe.insets());
        }
        self.pump_pen(ctx, raw_input);
    }

    #[cfg(feature = "gl-fallback")]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shut_down();
    }

    #[cfg(not(feature = "gl-fallback"))]
    fn on_exit(&mut self) {
        self.shut_down();
    }
}

impl EskizApp {
    fn shut_down(&mut self) {
        // The tablet connection borrows the window's display; close it first.
        self.pen.source = None;
        self.finish_storage();
    }

    /// Attach the result of `tablet_backend::connect`. On failure the reason is shown in
    /// Prefs and the app carries on with mouse and touch.
    pub fn set_pen_source(&mut self, source: Result<Box<dyn PenSource>, String>) {
        match source {
            Ok(s) => {
                self.pen.source = Some(s);
                self.pen.status.clear();
            }
            Err(e) => {
                self.pen.source = None;
                self.pen.status = format!("Tablet: {e}");
            }
        }
    }

    /// Once per frame, before egui reads its input: poll the tablet, fold its events into
    /// this frame's [`PenFrame`], and (when the OS does not also send the pen as a mouse)
    /// give egui pointer events so the pen can operate the interface.
    fn pump_pen(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let Some(source) = self.pen.source.as_mut() else {
            return;
        };
        let now = raw_input.time.unwrap_or(self.now);
        // Backends report logical pixels; egui points differ only by the zoom factor.
        let zoom = ctx.zoom_factor().max(0.1);
        source.set_scale(ctx.native_pixels_per_point().unwrap_or(1.0), zoom);
        let mut events = source.poll();
        for e in &mut events {
            if let crate::tablet::PenEvent::Pose(p) = e {
                p.pos = [p.pos[0] / zoom, p.pos[1] / zoom];
            }
        }
        let inject = source.needs_pointer_events();
        let f = self.pen.tracker.frame(events, now);
        let has_pointer_events = raw_input.events.iter().any(|e| {
            matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. })
        });
        if inject && !has_pointer_events {
            if let Some(pos) = f.pos.filter(|_| f.active) {
                let mods = egui::Modifiers::NONE;
                if f.moved {
                    raw_input.events.push(egui::Event::PointerMoved(pos));
                }
                if f.pressed {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: mods,
                    });
                }
                if f.released {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: mods,
                    });
                }
            }
        }
        if f.cancelled && inject {
            // The contact was thrown away: let go of the button egui thinks is down.
            let pos = f.pos.unwrap_or(Pos2::ZERO);
            raw_input.events.push(egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        // Finger, mouse and key events the source owns (Android): the system's own input
        // queue is not used there.
        raw_input.events.extend(source.take_ui_events());
        // Tablet events do not wake the window loop on their own: keep polling, quickly
        // while a pen is in range and slowly to notice one arriving.
        if f.active {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        self.pen.frame = f;
    }

    /// The pen's eraser end acts as the Eraser tool while it is in range.
    pub(crate) fn apply_pen_eraser(&mut self) {
        let want = self.pen.frame.active && self.pen.frame.eraser;
        if self.session.is_stroking() {
            return;
        }
        if want && self.pen.saved_tool.is_none() {
            self.pen.saved_tool = Some(self.tool);
            self.tool = Tool::Eraser;
        } else if !want {
            if let Some(t) = self.pen.saved_tool.take() {
                self.tool = t;
            }
        }
    }
}

/// Open the desktop window, optionally with a document loaded from `file`.
pub fn run_desktop(file: Option<PathBuf>) -> Result<(), String> {
    run_shared(file, |_| {})
}

/// The window setup shared by desktop and Android (renderer selection, library, prefs, GPU
/// compositor). `tweak` adjusts the [`eframe::NativeOptions`] built for every attempt; Android
/// uses it for `android_app` and `persistence_path`.
pub fn run_shared(file: Option<PathBuf>, tweak: impl Fn(&mut eframe::NativeOptions)) -> Result<(), String> {
    // Move data left under the old app name before eframe opens its storage file.
    crate::storage::migrate_legacy_data();
    #[allow(unused_mut)]
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Eskiz")
        .with_app_id("eskiz")
        .with_inner_size([1280.0, 860.0])
        .with_min_inner_size([900.0, 600.0]);
    // macOS: the content runs under a transparent title bar. The traffic-light buttons stay,
    // and the top bar draws its own dark background behind them.
    #[cfg(target_os = "macos")]
    {
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false)
            .with_titlebar_buttons_shown(true);
    }
    crate::renderer::run_native(
        "Eskiz",
        || {
            let mut options = eframe::NativeOptions {
                viewport: viewport.clone(),
                ..eframe::NativeOptions::default()
            };
            tweak(&mut options);
            options
        },
        |cc, mode| {
            let (doc, status) = match file.as_deref() {
                Some(path) => match open_procreate(path) {
                    Ok(doc) => (doc, format!("Opened {}", path.display())),
                    Err(e) => (Document::new(2048, 1536), format!("Could not open: {e}")),
                },
                None => (Document::new(2048, 1536), String::new()),
            };
            let start_in_gallery = file.is_none();
            let prefs = cc
                .storage
                .and_then(|s| crate::compat::storage_string(s, PREF_KEY, LEGACY_PREF_KEY))
                .map(|t| Prefs::decode(&t))
                .unwrap_or_default();
            let mut app = EskizApp::new(&cc.egui_ctx, doc, status, prefs);
            app.tour = crate::onboarding::Tour::from_storage(cc.storage, file.is_some());
            app.os_reduce_motion = crate::a11y::os_prefers_reduced_motion();
            app.gal.attach(&cc.egui_ctx, crate::storage::Library::default_location());
            if start_in_gallery {
                app.gal.screen = crate::gallery::Screen::Gallery;
            } else if let Some(path) = file.as_deref() {
                // A file from the command line is a document outside the library until it is edited.
                let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_owned();
                app.set_external_document(title);
            }
            if let Some(text) = cc.storage.and_then(|s| crate::compat::storage_string(s, COLOUR_KEY, LEGACY_COLOUR_KEY)) {
                app.set_colour_store(ColourStore::decode(&text));
            }
            if let Some(text) = cc.storage.and_then(|s| crate::compat::storage_string(s, BRUSH_KEY, LEGACY_BRUSH_KEY)) {
                app.set_brush_choice(&text);
            }
            app.set_pen_source(crate::tablet_backend::connect(cc));
            if mode.gpu_compositing() {
                if let Some(state) = cc.wgpu_render_state.as_ref() {
                    app.attach_gpu(&cc.egui_ctx, state);
                }
            }
            let app: Box<dyn eframe::App> = Box::new(app);
            Ok(app)
        },
    )
}

/// The canvas drop shadow as one gradient mesh. Rings run from the canvas edge out to 12%
/// larger, sinking by up to 6 px, and the alpha falls off with the square of the distance. The
/// innermost ring sits exactly on the canvas edge, so the mesh leaves no gap under it. The GPU
/// interpolates the vertex colours, so there are no visible steps.
fn canvas_shadow_mesh(corners: [Pos2; 4]) -> Mesh {
    const RINGS: usize = 32;
    const GROW: f32 = 0.12;
    const DROP: f32 = 6.0;
    /// Alpha at the canvas edge, out of 255.
    const PEAK: f32 = 28.0;
    let centre = corners.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2()) / 4.0;
    let centre = centre.to_pos2();
    let mut mesh = Mesh::default();
    for k in 0..=RINGS {
        let t = k as f32 / RINGS as f32;
        let alpha = (PEAK * (1.0 - t) * (1.0 - t)).round() as u8;
        let colour = Color32::from_black_alpha(alpha);
        let grow = 1.0 + GROW * t;
        for c in corners {
            mesh.colored_vertex(centre + (c - centre) * grow + Vec2::new(0.0, DROP * t), colour);
        }
    }
    for k in 0..RINGS {
        for i in 0..4u32 {
            let j = (i + 1) % 4;
            let (inner_i, inner_j) = (k as u32 * 4 + i, k as u32 * 4 + j);
            let (outer_i, outer_j) = ((k as u32 + 1) * 4 + i, (k as u32 + 1) * 4 + j);
            mesh.add_triangle(inner_i, inner_j, outer_j);
            mesh.add_triangle(inner_i, outer_j, outer_i);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_shadow_fades_smoothly_outward() {
        let corners = [Pos2::new(0.0, 0.0), Pos2::new(400.0, 0.0), Pos2::new(400.0, 300.0), Pos2::new(0.0, 300.0)];
        let mesh = canvas_shadow_mesh(corners);
        // 33 rings of 4 corners, with 8 triangles per ring step.
        assert_eq!(mesh.vertices.len(), 33 * 4);
        assert_eq!(mesh.indices.len(), 32 * 4 * 6);
        let alphas: Vec<u8> = mesh.vertices.chunks(4).map(|ring| ring[0].color.a()).collect();
        assert!(alphas.windows(2).all(|w| w[0] >= w[1]), "alpha never rises outward");
        assert_eq!(*alphas.last().unwrap(), 0, "the outer edge is clear");
        assert!(alphas[0] >= 25 && alphas[0] <= 28, "the edge is the darkest part");
        // Adjacent rings differ by at most one level, so the falloff has no visible steps.
        assert!(alphas.windows(2).all(|w| w[0] - w[1] <= 2));
    }

    #[test]
    fn prefs_round_trip() {
        let p = Prefs {
            light: true,
            identity: theme::UiIdentity::Eskiz,
            reduce_motion: true,
            sidebar_right: true,
            brush_cursor: false,
            eyedrop_hold: true,
            hold_shape: false,
            sidebar_y: 120.5,
            pressure_curve: PressureCurve::from_mid([[0.123, 0.28], [0.575, 0.88]]),
            undo_gestures: false,
            repeat_undo: false,
            touch_navigation: false,
            timelapse: false,
            timelapse_quality: 3,
            timelapse_resolution: 2,
            eyedrop_secs: 0.45,
            hold_shape_secs: 1.1,
            scrub_clear: false,
            swipe_copy: false,
            fullscreen_taps: false,
            touch_painting: false,
        };
        assert_eq!(Prefs::decode(&p.encode()), p);
    }

    #[test]
    fn gesture_controls_survive_a_restart_and_bad_values_fall_back() {
        let p = Prefs::decode("eyed_s=9;qs_s=-3;scrub=0;swipe=0;fs4=0;touch_paint=0");
        assert_eq!((p.eyedrop_secs, p.hold_shape_secs), (1.5, 0.2), "delays are clamped to their sliders");
        assert!(!p.scrub_clear && !p.swipe_copy && !p.fullscreen_taps && !p.touch_painting);
        let d = Prefs::decode("light=1");
        assert!(d.scrub_clear && d.touch_painting && d.eyedrop_secs == 0.4, "old strings keep the defaults");
    }

    #[test]
    fn prefs_from_before_the_pressure_curve_still_decode() {
        // `quickshape=` is the pre-rename key (compat read).
        let old = "light=1;right=0;cursor=1;eyedrop=1;quickshape=0;sidebar_y=33";
        let p = Prefs::decode(old);
        assert!(p.light && !p.hold_shape);
        assert_eq!(p.sidebar_y, 33.0);
        assert!(p.pressure_curve.is_identity());
        // The new key round-trips.
        assert!(!Prefs::decode(&Prefs { hold_shape: false, ..Prefs::default() }.encode()).hold_shape);
        // A damaged curve falls back to the identity and keeps the other fields.
        let bad = Prefs::decode("light=1;pcurve=oops");
        assert!(bad.light && bad.pressure_curve.is_identity());
    }

    #[test]
    fn old_eyedropper_default_migrates_to_the_new_default() {
        // Version 1 wrote the old 0.2 s default for everyone, so 0.2 with no version moves to 0.4.
        assert_eq!(Prefs::decode("light=1;eyed_s=0.20").eyedrop_secs, 0.4);
        assert_eq!(Prefs::decode("ver=1;eyed_s=0.20").eyedrop_secs, 0.4);
        // A delay the user chose is kept, whatever the version.
        assert_eq!(Prefs::decode("eyed_s=0.30").eyedrop_secs, 0.3);
        assert_eq!(Prefs::decode("ver=1;eyed_s=0.30").eyedrop_secs, 0.3);
        // No stored delay means the default, which is already 0.4 s.
        assert_eq!(Prefs::decode("light=1").eyedrop_secs, 0.4);
    }

    #[test]
    fn identity_pref_round_trips_and_old_strings_load_as_classic() {
        use crate::theme::UiIdentity;
        assert_eq!(Prefs::default().identity, UiIdentity::Classic);
        assert_eq!(Prefs::decode("ver=2;light=1;right=0").identity, UiIdentity::Classic);
        assert_eq!(Prefs::decode("identity=bogus").identity, UiIdentity::Classic);
        let p = Prefs { identity: UiIdentity::Eskiz, light: true, ..Prefs::default() };
        assert_eq!(Prefs::decode(&p.encode()), p);
    }

    #[test]
    fn current_version_keeps_a_chosen_0_2_second_delay() {
        let chosen = Prefs { eyedrop_secs: 0.2, ..Prefs::default() };
        let text = chosen.encode();
        assert!(text.starts_with("ver=2;"), "saved prefs carry the version: {text}");
        assert_eq!(Prefs::decode(&text), chosen);
        assert_eq!(Prefs::decode("ver=2;eyed_s=0.20").eyedrop_secs, 0.2);
        assert_eq!(Prefs::decode(&Prefs::default().encode()), Prefs::default());
    }

    #[test]
    fn bad_prefs_fall_back_to_defaults() {
        assert_eq!(Prefs::decode("garbage;sidebar_y=nope"), Prefs::default());
    }

    /// A pen stroke from fake tablet events reaches the canvas, and the eraser end
    /// switches the tool while it is in range.
    #[test]
    fn pen_events_paint_and_the_eraser_end_switches_tool() {
        use crate::snapshot::Headless;
        use crate::tablet::{PenEvent, PenPose, PenTracker};

        let mut h = Headless::new(Vec2::new(1024.0, 700.0), Document::new(800, 600), Prefs::default());
        let before = h.app.session.flatten();
        let mut tracker = PenTracker::new();
        let pose = |x: f32, p: f32| PenEvent::Pose(PenPose {
            pos: [x, 350.0],
            pressure: Some(p),
            tilt: Some([0.4, 0.0]),
            time: None,
        });
        let mut now = 1.0;
        let mut frame = |events: Vec<PenEvent>, h: &mut Headless| {
            now += 1.0 / 60.0;
            h.app.pen.frame = tracker.frame(events, now);
            h.step();
        };
        frame(vec![PenEvent::Enter { eraser: false }, pose(400.0, 0.0)], &mut h);
        assert_eq!(h.app.tool, Tool::Brush);
        frame(vec![PenEvent::Down, pose(402.0, 0.3)], &mut h);
        assert!(h.app.session.is_stroking(), "pen press starts a stroke");
        for k in 0..6 {
            let x = 410.0 + k as f32 * 20.0;
            frame(vec![pose(x, 0.3 + k as f32 * 0.1), pose(x + 10.0, 0.4 + k as f32 * 0.1)], &mut h);
        }
        frame(vec![PenEvent::Up], &mut h);
        assert!(!h.app.session.is_stroking(), "pen lift ends the stroke");
        assert_ne!(h.app.session.flatten(), before, "the stroke painted something");
        frame(vec![PenEvent::Leave], &mut h);
        // Eraser end.
        frame(vec![PenEvent::Enter { eraser: true }, pose(500.0, 0.0)], &mut h);
        assert_eq!(h.app.tool, Tool::Eraser);
        frame(vec![PenEvent::Leave], &mut h);
        assert_eq!(h.app.tool, Tool::Brush);
    }

    /// The sidebar Pick Color button: a tap starts nothing, a hold past the delay starts the
    /// eyedropper, and release over the canvas commits the colour under the loupe.
    #[test]
    fn modify_button_hold_starts_the_eyedropper_after_the_delay() {
        use crate::snapshot::Headless;

        let mut h = Headless::new(Vec2::new(1024.0, 700.0), Document::new(800, 600), Prefs::default());
        let button = h.app.modify_rect.center();
        let before = h.app.colour;

        // A quick tap, shorter than the 0.4 s delay: no eyedropper, and no colour change.
        h.press(button, true);
        h.step();
        h.advance(0.25);
        h.press(button, false);
        h.step();
        assert!(h.app.eyedrop.is_none(), "a tap does not start the eyedropper");
        assert_eq!(h.app.colour, before);

        // Held just under the delay: still nothing.
        h.press(button, true);
        h.step();
        h.advance(0.3);
        assert!(h.app.eyedrop.is_none(), "inside the delay the eyedropper has not started");
        h.press(button, false);
        h.step();
        assert!(h.app.eyedrop.is_none());
        assert_eq!(h.app.colour, before);

        // Held past the delay: the eyedropper starts, and the loupe follows the pointer onto the canvas.
        h.press(button, true);
        h.step();
        h.advance(0.5);
        assert!(h.app.eyedrop.is_some(), "a hold past 0.4 s starts the eyedropper");
        let on_canvas = h.app.viewport.center();
        h.move_to(on_canvas);
        h.step();
        let expected = h.app.sample_at(on_canvas).expect("the viewport center is on the canvas");
        assert_eq!(h.app.eyedrop.and_then(|e| e.pos), Some(on_canvas), "the loupe follows the pointer");
        // Release commits the colour under the loupe.
        h.press(on_canvas, false);
        h.step();
        assert!(h.app.eyedrop.is_none(), "release ends the eyedropper");
        assert_eq!(h.app.colour, expected);
    }

    /// With "Eyedropper on hold" switched off, holding the Pick Color button picks nothing.
    #[test]
    fn modify_button_hold_respects_the_eyedropper_switch() {
        use crate::snapshot::Headless;

        let prefs = Prefs { eyedrop_hold: false, ..Prefs::default() };
        let mut h = Headless::new(Vec2::new(1024.0, 700.0), Document::new(800, 600), prefs);
        let button = h.app.modify_rect.center();
        h.press(button, true);
        h.step();
        h.advance(0.6);
        assert!(h.app.eyedrop.is_none());
        h.press(button, false);
        h.step();
    }
}
