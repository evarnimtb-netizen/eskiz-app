//! Frames in the app: the frames built from the document, playback, onion skins on the canvas,
//! background export, and the mapping between [`AnimationSettings`] and the `.procreate` animation object.
//!
//! The settings live on the document ([`eskiz_core::Document::animation`]). The timeline itself is drawn
//! by [`crate::panels::animation`].

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use eskiz_anim::{
    export_frames_with, onion_skins, render_with_onion, snapshot_from_document, AnimationSettings,
    FrameList, OnionBlend, Paper, Playback, PlaybackDirection, PlaybackMode,
};
use eskiz_format::AnimationSpec;
use eskiz_media::{
    write_apng_file, write_gif_file, write_mp4_file, ApngOptions, GifOptions, Mp4Options, Quality,
};

use crate::app::EskizApp;
use crate::export::expand_home;

/// The export job's answer when the user pressed Cancel.
const CANCELED: &str = "The export was canceled.";

/// The file's `skinBlendMode` for normal onion blending. The default.
const SKIN_BLEND_NORMAL: i64 = 2;
/// The file's `skinBlendMode` for multiply onion blending. A `[choice]`: the file value for multiply is not recovered.
const SKIN_BLEND_MULTIPLY: i64 = 1;

/// Converts the stored Frames object into settings. Colours and the export frame rate are not
/// stored in the file, so they take their defaults. Playback codes other than loop (1) are `[choice]`:
/// 0 is one-shot and 2 is ping-pong.
pub fn settings_from_spec(spec: &AnimationSpec) -> AnimationSettings {
    let playback_mode = match spec.playback_mode {
        0 => PlaybackMode::OneShot,
        2 => PlaybackMode::PingPong,
        _ => PlaybackMode::Loop,
    };
    let direction = if spec.playback_direction == 1 {
        PlaybackDirection::Reverse
    } else {
        PlaybackDirection::Forward
    };
    let onion_blend = if spec.skin_blend_mode == SKIN_BLEND_MULTIPLY {
        OnionBlend::Multiply
    } else {
        OnionBlend::Normal
    };
    let mut s = AnimationSettings {
        frames_enabled: spec.animation_mode != 0,
        playback_mode,
        direction,
        onion_enabled: spec.onion_skin_enabled,
        onion_count: spec.onion_skin_count.clamp(1, i64::from(u32::MAX)) as u32,
        onion_opacity: spec.onion_skin_opacity,
        primary_mixed: spec.primary_mixed,
        onion_blend,
        skin_blend_mode: spec.skin_blend_mode,
        ..AnimationSettings::default()
    };
    s.set_frame_rate(spec.frame_rate.round().clamp(1.0, 60.0) as u32);
    s.onion_count = s.onion_count.clamp(eskiz_anim::settings::MIN_ONION_COUNT, eskiz_anim::settings::MAX_ONION_COUNT);
    s
}

/// The stored Frames object for `s`. The background and foreground flags are document
/// state, not settings, so they are left false here and set by the caller.
pub fn spec_from_settings(s: &AnimationSettings) -> AnimationSpec {
    let playback_mode = match s.playback_mode {
        PlaybackMode::OneShot => 0,
        PlaybackMode::Loop => 1,
        PlaybackMode::PingPong => 2,
    };
    AnimationSpec {
        animation_mode: i64::from(s.frames_enabled),
        playback_mode,
        playback_direction: i64::from(s.direction == PlaybackDirection::Reverse),
        primary_mixed: s.primary_mixed,
        frame_rate: f64::from(s.fps()),
        onion_skin_count: i64::from(s.onion_count()),
        onion_skin_opacity: s.onion_opacity(),
        onion_skin_enabled: s.onion_enabled,
        skin_blend_mode: match s.onion_blend {
            OnionBlend::Normal => SKIN_BLEND_NORMAL,
            OnionBlend::Multiply => SKIN_BLEND_MULTIPLY,
        },
        last_item_is_background: false,
        first_item_is_foreground: false,
    }
}

/// The frames for one state of the document.
pub(crate) struct FrameCache {
    /// Session edit counter the frames were built at.
    pub edits: u64,
    pub frames: FrameList,
    /// Node id of each stack item, bottom first, in the same order as the frame list's items.
    pub ids: Vec<u64>,
}

/// What the canvas texture was drawn for. A change in any field redraws it.
#[derive(Clone, PartialEq)]
pub(crate) struct CanvasKey {
    edits: u64,
    frame: usize,
    settings: AnimationSettings,
    playing: bool,
}

/// The format of an animation export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnimFormat {
    Gif,
    Apng,
    Mp4,
}

impl AnimFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Gif => "animated GIF",
            Self::Apng => "animated PNG",
            Self::Mp4 => "MP4 animation",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Gif => "gif",
            Self::Apng => "png",
            Self::Mp4 => "mp4",
        }
    }
}

/// Options on the Share sheet for animation exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimExportOptions {
    pub dither: bool,
    /// Keep transparency in GIF and APNG (MP4 has no alpha).
    pub transparent: bool,
    /// Alpha below this becomes transparent in GIF, 0 to 255.
    pub alpha_threshold: u8,
    pub quality: Quality,
}

impl Default for AnimExportOptions {
    fn default() -> Self {
        Self {
            dither: true,
            transparent: false,
            alpha_threshold: 128,
            quality: Quality::Standard,
        }
    }
}

/// An animation export running on a worker thread.
pub(crate) struct ExportJob {
    pub rx: Receiver<Result<PathBuf, String>>,
    pub format: AnimFormat,
    /// Frames rendered so far, and the total.
    pub done: Arc<AtomicUsize>,
    pub total: Arc<AtomicUsize>,
    pub cancel: Arc<AtomicBool>,
    pub delivery: crate::deliver::Delivery,
}

/// Frames state for the open document.
#[derive(Default)]
pub(crate) struct AnimationUi {
    pub cache: Option<FrameCache>,
    pub playback: Option<Playback>,
    /// Time of the last playback update, in seconds of app time.
    pub clock: f64,
    /// The frame shown on the canvas, kept in step with the playhead.
    pub frame: usize,
    /// The frame whose hold is shown in the timeline.
    pub selected: Option<usize>,
    /// One thumbnail per frame. `None` until it is drawn.
    pub thumbs: Vec<Option<TextureHandle>>,
    /// Thumbnails rendered on a worker thread: `(frame cache edits, frame, image)`.
    pub(crate) thumb_rx: Option<Receiver<(u64, usize, egui::ColorImage)>>,
    /// The frame cache (by `edits`) the running or finished thumbnail job renders.
    pub(crate) thumb_job: Option<u64>,
    /// The first canvas view after the timeline is turned on, rendered on a worker thread.
    canvas_rx: Option<Receiver<(CanvasKey, egui::ColorImage)>>,
    /// Composite the first view on the UI thread (the headless test harness sets this, so its
    /// frames are deterministic; the window never does).
    pub(crate) sync_first_view: bool,
    canvas: Option<TextureHandle>,
    pub(crate) canvas_key: Option<CanvasKey>,
    pub settings_open: bool,
    pub export_opts: AnimExportOptions,
    pub export: Option<ExportJob>,
}

impl AnimationUi {
    /// The canvas texture for the current frame and onion skins, if one has been drawn.
    pub fn canvas(&self) -> Option<&TextureHandle> {
        self.canvas.as_ref()
    }
}

impl EskizApp {
    /// Rebuilds the frames when the document has changed. During a stroke the old frames stay, so the
    /// stroke is not re-rendered until it ends.
    pub(crate) fn ensure_frames(&mut self) {
        let edits = self.session.edits();
        let stale = match &self.anim.cache {
            None => true,
            Some(cache) => cache.edits != edits && !self.session.is_stroking(),
        };
        if !stale {
            return;
        }
        let (snapshot, ids) = snapshot_from_document(&self.session.doc);
        match FrameList::build(snapshot) {
            Ok(frames) => {
                self.anim.thumbs = vec![None; frames.len()];
                // New frames: the old thumbnail job is stale (dropping its receiver stops it).
                self.anim.thumb_rx = None;
                self.anim.thumb_job = None;
                self.anim.cache = Some(FrameCache { edits, frames, ids });
            }
            Err(_) => self.anim.cache = None,
        }
    }

    /// The CPU texture the canvas draws: the Frames view while the feature is on.
    /// (The app draws through `canvas_texture_id`, which also knows the GPU composite.)
    #[cfg(test)]
    pub(crate) fn canvas_texture(&self) -> Option<&TextureHandle> {
        if self.session.doc.animation.frames_enabled && self.three_d.is_none() {
            if let Some(tex) = self.anim.canvas() {
                return Some(tex);
            }
        }
        self.texture.as_ref()
    }

    /// Advances playback, and redraws the canvas view when the frame, settings or document changed.
    pub(crate) fn anim_sync(&mut self, ctx: &egui::Context) {
        self.ensure_frames();
        let settings = self.session.doc.animation.clone();
        let now = self.now;
        if let (Some(p), Some(cache)) = (self.anim.playback.as_mut(), self.anim.cache.as_ref()) {
            p.set_fps(settings.fps());
            if p.is_playing() {
                p.advance(now - self.anim.clock, &cache.frames);
                ctx.request_repaint();
            }
            self.anim.frame = p.frame();
        }
        self.anim.clock = now;
        self.refresh_canvas(ctx, &settings);
    }

    fn refresh_canvas(&mut self, ctx: &egui::Context, settings: &AnimationSettings) {
        let Some(cache) = self.anim.cache.as_ref() else {
            return;
        };
        let n = cache.frames.len();
        if n == 0 {
            self.anim.canvas = None;
            self.anim.canvas_key = None;
            return;
        }
        let frame = self.anim.frame.min(n - 1);
        let playing = self.anim.playback.as_ref().is_some_and(Playback::is_playing);
        let key = CanvasKey {
            edits: cache.edits,
            frame,
            settings: settings.clone(),
            playing,
        };
        if self.anim.canvas_key.as_ref() == Some(&key) {
            return;
        }
        if self.session.is_stroking() && self.anim.canvas.is_some() {
            return;
        }
        // The very first view (the timeline was just turned on) is composited on a worker thread:
        // until it arrives the ordinary canvas stays on screen, so the toggle does not hitch. Later
        // updates (an edit, a new frame while playing) replace a view that exists, so they stay in step.
        if self.anim.canvas.is_none() && !playing && !self.anim.sync_first_view {
            if let Some(rx) = &self.anim.canvas_rx {
                match rx.try_recv() {
                    Ok((done, image)) => {
                        self.anim.canvas_rx = None;
                        self.anim.canvas = Some(ctx.load_texture("animation_canvas", image, TextureOptions::LINEAR));
                        // A key that no longer matches is redrawn by the next call.
                        self.anim.canvas_key = Some(done);
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => self.anim.canvas_rx = None,
                }
                if self.anim.canvas_rx.is_some() || self.anim.canvas.is_some() {
                    return;
                }
            } else {
                let frames = cache.frames.clone();
                let (settings, space, job_key) = (settings.clone(), self.session.doc.colour_space, key);
                let (tx, rx) = channel();
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let skins = onion_skins(&settings, &frames, frame, false);
                    let mut rgba = render_with_onion(&frames, frame, &skins, settings.onion_blend_mode());
                    crate::colour_management::apply_display_rgba8(space, &mut rgba);
                    let (w, h) = frames.size();
                    let _ = tx.send((job_key, ColorImage::from_rgba_unmultiplied([w, h], &rgba)));
                    repaint.request_repaint();
                });
                self.anim.canvas_rx = Some(rx);
                return;
            }
        }
        let skins = onion_skins(settings, &cache.frames, frame, playing);
        let mut rgba = render_with_onion(&cache.frames, frame, &skins, settings.onion_blend_mode());
        crate::colour_management::apply_display_rgba8(self.session.doc.colour_space, &mut rgba);
        let (w, h) = cache.frames.size();
        let image = ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        match self.anim.canvas.as_mut() {
            Some(tex) => tex.set(image, TextureOptions::LINEAR),
            None => {
                self.anim.canvas = Some(ctx.load_texture("animation_canvas", image, TextureOptions::LINEAR));
            }
        }
        self.anim.canvas_key = Some(key);
    }

    /// Starts playback from the current frame.
    pub(crate) fn anim_play(&mut self) {
        if self.three_d.is_some() {
            return;
        }
        self.ensure_frames();
        let Some(cache) = self.anim.cache.as_ref() else {
            return;
        };
        if cache.frames.is_empty() {
            return;
        }
        let settings = &self.session.doc.animation;
        let mut p = Playback::new(settings, self.anim.frame.min(cache.frames.len() - 1));
        p.play(&cache.frames);
        self.anim.clock = self.now;
        self.anim.playback = Some(p);
    }

    /// Pauses playback, keeping the frame.
    pub(crate) fn anim_pause(&mut self) {
        if let Some(p) = self.anim.playback.as_mut() {
            p.pause();
        }
    }

    /// Shows frame `index` and makes its stack item the active layer.
    pub(crate) fn anim_show(&mut self, index: usize) {
        self.ensure_frames();
        let Some(cache) = self.anim.cache.as_ref() else {
            return;
        };
        if index >= cache.frames.len() {
            return;
        }
        let item = cache.frames.frame(index).and_then(|f| f.items.first().copied());
        let target = item.and_then(|i| cache.ids.get(i).copied());
        let frames = &cache.frames;
        let settings = &self.session.doc.animation;
        let p = self
            .anim
            .playback
            .get_or_insert_with(|| Playback::new(settings, index));
        p.scrub(index, frames);
        self.anim.frame = index;
        self.anim.selected = Some(index);
        if let Some(id) = target {
            self.active = id;
        }
    }

    /// The node id of the bottom item of frame `index`, which holds the frame's hold value.
    pub(crate) fn anim_frame_node(&self, index: usize) -> Option<u64> {
        let cache = self.anim.cache.as_ref()?;
        let item = cache.frames.frame(index)?.items.first().copied()?;
        cache.ids.get(item).copied()
    }

    /// Starts an animation export on a worker thread. The frames are copied, so editing goes on meanwhile.
    pub(crate) fn start_anim_export(&mut self, ctx: &egui::Context, format: AnimFormat) {
        if self.anim.export.is_some() {
            self.show_toast("An animation export is already running.", 2.0);
            return;
        }
        if self.files.staged() {
            self.begin_staged(ctx, crate::deliver::ExportKind::Anim(format));
            return;
        }
        // "Study v1.2" keeps its ".2": only a known extension is replaced.
        let path = crate::export::with_export_extension(&expand_home(&self.export_path), format.extension());
        self.start_anim_export_to(ctx, format, path, crate::deliver::Delivery::Local);
    }

    /// Like [`Self::start_anim_export`], writing to `path` and delivering the file as `delivery` says.
    pub(crate) fn start_anim_export_to(&mut self, ctx: &egui::Context, format: AnimFormat, path: PathBuf, delivery: crate::deliver::Delivery) {
        if self.anim.export.is_some() {
            self.show_toast("An animation export is already running.", 2.0);
            return;
        }
        self.ensure_frames();
        let Some(cache) = self.anim.cache.as_ref() else {
            self.show_toast("There are no frames to export.", 2.5);
            return;
        };
        if cache.frames.is_empty() {
            self.show_toast("There are no frames to export.", 2.5);
            return;
        }
        let frames = cache.frames.clone();
        let settings = self.session.doc.animation.clone();
        let opts = self.anim.export_opts;
        let (tx, rx) = channel();
        let done = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (d, t, c) = (done.clone(), total.clone(), cancel.clone());
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = run_anim_export(format, &frames, &settings, opts, &path, &mut |k, n| {
                d.store(k, Ordering::Relaxed);
                t.store(n, Ordering::Relaxed);
                !c.load(Ordering::Relaxed)
            });
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.anim.export = Some(ExportJob {
            rx,
            format,
            done,
            total,
            cancel,
            delivery,
        });
    }

    /// Reports the finished export, if there is one.
    pub(crate) fn poll_anim_export(&mut self) {
        let Some(job) = self.anim.export.as_ref() else {
            return;
        };
        match job.rx.try_recv() {
            Ok(result) => {
                let label = job.format.label();
                let delivery = job.delivery;
                self.anim.export = None;
                match result {
                    Ok(path) => {
                        let text = format!("Exported {label} to {}", path.display());
                        self.deliver(&path, delivery, text, 3.5);
                    }
                    // A cancel the user asked for is not a failure.
                    Err(e) if e == CANCELED => self.show_toast(CANCELED, 2.0),
                    Err(e) => self.show_toast(format!("Export failed: {e}"), 4.0),
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.anim.export = None;
                self.show_toast("Export failed.", 3.0);
            }
        }
    }
}

/// Renders the frames and writes the file. Runs on a worker thread.
fn run_anim_export(
    format: AnimFormat,
    frames: &FrameList,
    settings: &AnimationSettings,
    opts: AnimExportOptions,
    path: &std::path::Path,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<PathBuf, String> {
    let transparent = opts.transparent && format != AnimFormat::Mp4;
    let paper = if transparent { Paper::Transparent } else { Paper::White };
    let out = export_frames_with(settings, frames, paper, progress)
        .ok_or_else(|| CANCELED.to_owned())?;
    if out.is_empty() {
        return Err("There are no frames to export.".to_owned());
    }
    let written = match format {
        AnimFormat::Gif => {
            let gif = GifOptions {
                dither: opts.dither,
                transparent_background: transparent,
                alpha_threshold: opts.alpha_threshold,
                ..GifOptions::default()
            };
            write_gif_file(path, &out, &gif)
        }
        AnimFormat::Apng => write_apng_file(path, &out, &ApngOptions::default()),
        AnimFormat::Mp4 => {
            let (w, h) = frames.size();
            let mp4 = Mp4Options {
                bitrate_bps: eskiz_media::bitrate_bps(w as u32, h as u32, opts.quality),
                background: [255, 255, 255],
            };
            write_mp4_file(path, &out, &mp4)
        }
    };
    written.map_err(|e| match e {
        eskiz_media::MediaError::EncoderUnavailable(msg) => msg,
        e => e.to_string(),
    })?;
    Ok(path.to_path_buf())
}

/// A short self-check of Frames, run by `--selftest`: three frames, two ticks of playback,
/// the onion count, and a GIF export with three frames.
pub fn selftest() -> Result<Vec<String>, String> {
    let frames = three_frame_list()?;
    if frames.len() != 3 {
        return Err(format!("animation: expected 3 frames, got {}", frames.len()));
    }
    let settings = AnimationSettings::default();
    let mut p = Playback::new(&settings, 0);
    p.play(&frames);
    let ticks = p.advance(2.0 / f64::from(settings.fps()) + 1e-6, &frames);
    if ticks != 2 || p.frame() != 2 {
        return Err(format!("animation: two ticks moved {ticks} ticks to frame {}", p.frame()));
    }
    let skins = onion_skins(&settings, &frames, 1, false);
    if skins.len() != 2 {
        return Err(format!("animation: expected 2 onion skins at frame 1, got {}", skins.len()));
    }
    let exported = eskiz_anim::export_frames(&settings, &frames);
    let gif = gif_frame_count(&exported)?;
    if gif != 3 {
        return Err(format!("animation: GIF of 3 frames decoded to {gif}"));
    }
    Ok(vec![
        "  animation: 3 frames, 2 playback ticks to frame 2, 2 onion skins, GIF decodes to 3 frames".to_owned(),
    ])
}

/// Encodes `frames` as a GIF and counts the frames that decode back.
fn gif_frame_count(frames: &[eskiz_media::Frame]) -> Result<usize, String> {
    let mut bytes = Vec::new();
    eskiz_media::encode_gif(&mut bytes, frames, &GifOptions::default()).map_err(|e| e.to_string())?;
    let mut decoder = gif::DecodeOptions::new()
        .read_info(bytes.as_slice())
        .map_err(|e| e.to_string())?;
    let mut count = 0;
    while decoder.read_next_frame().map_err(|e| e.to_string())?.is_some() {
        count += 1;
    }
    Ok(count)
}

/// Three layers on a 64 by 48 canvas, one per frame. Each paints a 24 by 24 square, moved along by 16
/// pixels, so the neighbouring frames show outside it and the onion skins are visible. Used by the
/// self-check and the tests.
pub(crate) fn three_frame_list() -> Result<FrameList, String> {
    use eskiz_anim::{StackItem, StackSnapshot};
    use eskiz_core::Layer;
    let (w, h) = (64usize, 48usize);
    let items = [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]]
        .iter()
        .enumerate()
        .map(|(i, rgb)| {
            let mut px = vec![0u8; w * h * 4];
            for y in 8..32 {
                for x in (16 * i)..(16 * i + 24) {
                    let p = (y * w + x) * 4;
                    px[p..p + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
            }
            StackItem::new(Layer::from_rgba(w, h, px))
        })
        .collect();
    FrameList::build(StackSnapshot { width: w, height: h, items }).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eskiz_core::BlendMode;

    #[test]
    fn three_frames_and_two_ticks() {
        let frames = three_frame_list().unwrap();
        assert_eq!(frames.len(), 3);
        let settings = AnimationSettings::default();
        let mut p = Playback::new(&settings, 0);
        p.play(&frames);
        // 15 fps: two ticks take 2/15 s.
        let ticks = p.advance(2.0 / 15.0 + 1e-6, &frames);
        assert_eq!(ticks, 2);
        assert_eq!(p.frame(), 2);
    }

    #[test]
    fn onion_skins_count_and_order() {
        let frames = three_frame_list().unwrap();
        let mut s = AnimationSettings { onion_count: 1, ..AnimationSettings::default() };
        s.onion_enabled = true;
        let skins = onion_skins(&s, &frames, 1, false);
        assert_eq!(skins.len(), 2);
        assert_eq!(skins[0].frame, 0);
        assert_eq!(skins[1].frame, 2);
        assert!(onion_skins(&s, &frames, 1, true).is_empty(), "no onion skins while playing");
    }

    #[test]
    fn onion_hook_changes_the_composite() {
        let frames = three_frame_list().unwrap();
        let s = AnimationSettings::default();
        let plain = render_with_onion(&frames, 1, &[], BlendMode::Normal);
        let skins = onion_skins(&s, &frames, 1, false);
        let with = render_with_onion(&frames, 1, &skins, BlendMode::Normal);
        assert_eq!(plain.len(), with.len());
        assert_ne!(plain, with, "onion skins must change the rendered canvas");
        assert_eq!(plain, frames.render(1), "no skins is the plain frame");
    }

    #[test]
    fn settings_round_trip_through_the_file_object() {
        let settings = AnimationSettings {
            frames_enabled: true,
            frame_rate: 24,
            playback_mode: PlaybackMode::PingPong,
            direction: PlaybackDirection::Reverse,
            onion_enabled: false,
            onion_count: 5,
            onion_opacity: 0.35,
            primary_mixed: true,
            onion_blend: OnionBlend::Multiply,
            ..AnimationSettings::default()
        };
        let back = settings_from_spec(&spec_from_settings(&settings));
        assert_eq!(back.frames_enabled, settings.frames_enabled);
        assert_eq!(back.fps(), settings.fps());
        assert_eq!(back.playback_mode, settings.playback_mode);
        assert_eq!(back.direction, settings.direction);
        assert_eq!(back.onion_enabled, settings.onion_enabled);
        assert_eq!(back.onion_count(), settings.onion_count());
        assert!((back.onion_opacity() - settings.onion_opacity()).abs() < 1e-12);
        assert_eq!(back.primary_mixed, settings.primary_mixed);
        assert_eq!(back.onion_blend, settings.onion_blend);
    }

    #[test]
    fn animation_settings_and_holds_survive_a_procreate_round_trip() {
        use eskiz_core::Props;
        let mut doc = eskiz_core::Document::empty(64, 64);
        doc.push_layer(None, Props { anim_background: true, hold: 1, ..Props::new("Paper") }, None, vec![255; 64 * 64 * 4]);
        let mut ink = vec![0u8; 64 * 64 * 4];
        ink[..4].copy_from_slice(&[10, 20, 30, 255]);
        doc.push_layer(None, Props { hold: 3, ..Props::new("Ink") }, None, ink);
        doc.push_layer(None, Props { anim_foreground: true, ..Props::new("Top") }, None, vec![0; 64 * 64 * 4]);
        doc.animation = AnimationSettings {
            frames_enabled: true,
            frame_rate: 24,
            playback_mode: PlaybackMode::PingPong,
            direction: PlaybackDirection::Reverse,
            onion_enabled: true,
            onion_count: 5,
            onion_opacity: 0.35,
            primary_mixed: true,
            onion_blend: OnionBlend::Multiply,
            skin_blend_mode: 1,
            ..AnimationSettings::default()
        };
        let path = std::env::temp_dir().join(format!("eskiz_anim_roundtrip_{}.procreate", std::process::id()));
        let builder = crate::storage::to_builder(&doc, "Animation round trip");
        eskiz_format::document::write(&path, &builder).unwrap();
        let back = crate::import::open_procreate(&path);
        let _ = std::fs::remove_file(&path);
        let back = back.unwrap();
        assert_eq!(back.animation, doc.animation);
        assert_eq!(back.root.len(), 3);
        assert_eq!(back.root[0].props.hold, 1);
        assert!(back.root[0].props.anim_background, "bottom item is the background");
        assert_eq!(back.root[1].props.hold, 3);
        assert!(back.root[2].props.anim_foreground, "top item is the foreground");
        assert!(!back.root[2].props.anim_background);
    }

    #[test]
    fn the_timeline_renders_headless_for_a_visual_check() {
        use crate::snapshot::{snapshot_dir, Headless};
        use eframe::egui::Vec2;
        let mut doc = eskiz_core::Document::empty(160, 120);
        for (i, rgb) in [[230u8, 60, 60], [60, 200, 90], [60, 110, 230]].iter().enumerate() {
            let mut px = vec![0u8; 160 * 120 * 4];
            for y in 20..90 {
                for x in (20 + 30 * i)..(80 + 30 * i) {
                    let p = (y * 160 + x) * 4;
                    px[p..p + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
            }
            doc.push_layer(None, eskiz_core::Props::new(format!("Frame {}", i + 1)), None, px);
        }
        doc.animation.frames_enabled = true;
        let mut h = Headless::new(Vec2::new(900.0, 520.0), doc, crate::app::Prefs::default());
        let mut image = h.render();
        for _ in 0..3 {
            image = h.render();
        }
        let dir = snapshot_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("animation_timeline.png");
        image.save_png(&out).unwrap();
        assert!(out.exists());
    }

    /// B9: turning the Frames timeline on used to cost one frame of 19 ms or more because two
    /// full-canvas thumbnails were composited on the UI thread per frame. They are rendered on a
    /// worker thread now: the toggle frame creates no thumbnail, and they arrive later.
    #[test]
    fn b9_toggling_the_timeline_renders_no_thumbnail_on_the_ui_thread() {
        use crate::snapshot::Headless;
        use eframe::egui::Vec2;
        let (w, h) = (1536usize, 1024usize);
        let mut doc = eskiz_core::Document::empty(w, h);
        for i in 0..4u8 {
            let mut px = vec![0u8; w * h * 4];
            for y in 100..900 {
                for x in (100 + 90 * usize::from(i))..(700 + 90 * usize::from(i)) {
                    px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[40 + 50 * i, 90, 200 - 40 * i, 255]);
                }
            }
            doc.push_layer(None, eskiz_core::Props::new(format!("Frame {}", i + 1)), None, px);
        }
        let mut hl = Headless::new(Vec2::new(1000.0, 640.0), doc, crate::app::Prefs::default());
        hl.app.anim.sync_first_view = false;
        for _ in 0..3 {
            hl.render();
        }
        hl.app.session.doc.animation.frames_enabled = true;
        let t0 = std::time::Instant::now();
        hl.render();
        let toggle_ms = t0.elapsed().as_secs_f64() * 1e3;
        assert!(hl.app.anim.cache.is_some(), "the toggle frame builds the frame list");
        assert_eq!(hl.app.anim.thumbs.len(), 4);
        assert!(hl.app.anim.thumbs.iter().all(Option::is_none), "no thumbnail is composited on the toggle frame");
        assert!(hl.app.anim.canvas().is_none(), "the first Frames view is not composited on the toggle frame either");
        let start = std::time::Instant::now();
        while !hl.app.anim.thumbs.iter().all(Option::is_some) {
            hl.render();
            assert!(start.elapsed().as_secs() < 30, "thumbnails never arrived");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // The UI-thread parts of the toggle, measured on their own (the headless frame also paints the whole window in software).
        let mut doc2 = hl.app.session.doc.clone();
        doc2.animation.frames_enabled = false;
        let mut fresh = Headless::new(Vec2::new(1000.0, 640.0), doc2, crate::app::Prefs::default());
        fresh.app.anim.sync_first_view = false;
        fresh.render();
        fresh.app.session.doc.animation.frames_enabled = true;
        let ctx = egui::Context::default();
        let t1 = std::time::Instant::now();
        fresh.app.ensure_frames();
        let build_ms = t1.elapsed().as_secs_f64() * 1e3;
        let t2 = std::time::Instant::now();
        fresh.app.anim_sync(&ctx);
        let sync_ms = t2.elapsed().as_secs_f64() * 1e3;
        let t3 = std::time::Instant::now();
        fresh.app.anim_thumbs(&ctx);
        let thumbs_ms = t3.elapsed().as_secs_f64() * 1e3;
        let t4 = std::time::Instant::now();
        while fresh.app.anim.canvas().is_none() {
            fresh.app.anim_sync(&ctx);
            assert!(t4.elapsed().as_secs() < 30, "the first Frames view never arrived");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        eprintln!("B9: UI thread on toggle: frame list {build_ms:.2} ms, canvas view {sync_ms:.2} ms, thumbnails (start worker) {thumbs_ms:.3} ms");
        eprintln!("B9: timeline toggle frame {toggle_ms:.1} ms (1536x1024, 4 frames); thumbnails ready after {:.0} ms", start.elapsed().as_secs_f64() * 1e3);
    }

    #[test]
    fn the_default_spec_matches_format_4a() {
        let spec = spec_from_settings(&AnimationSettings::default());
        assert_eq!(spec.animation_mode, 0);
        assert_eq!(spec.playback_mode, 1);
        assert_eq!(spec.frame_rate, 15.0);
        assert_eq!(spec.onion_skin_count, 12);
        assert_eq!(spec.onion_skin_opacity, 0.6);
        assert!(spec.onion_skin_enabled);
        assert_eq!(spec.skin_blend_mode, 2);
        assert_eq!(spec, AnimationSpec { last_item_is_background: false, first_item_is_foreground: false, ..AnimationSpec::default() });
    }

    #[test]
    fn gif_export_of_three_frames_decodes_to_three() {
        let frames = three_frame_list().unwrap();
        let exported = eskiz_anim::export_frames(&AnimationSettings::default(), &frames);
        assert_eq!(exported.len(), 3);
        assert_eq!(gif_frame_count(&exported).unwrap(), 3);
    }

    #[test]
    fn holds_lengthen_the_frame_in_export() {
        // A held item is one exported frame shown for hold + 1 ticks.
        let mut doc = eskiz_core::Document::empty(8, 8);
        doc.push_layer(None, eskiz_core::Props { hold: 2, ..eskiz_core::Props::new("a") }, None, vec![255; 8 * 8 * 4]);
        let (snapshot, _) = snapshot_from_document(&doc);
        let built = FrameList::build(snapshot).unwrap();
        assert_eq!(built.hold(0), 2);
        let out = eskiz_anim::export_frames(&AnimationSettings::default(), &built);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].duration(), eskiz_media::duration_for_fps(AnimationSettings::default().export_fps()) * 3);
    }

    #[test]
    fn text_layers_count_as_normal_frames() {
        use eskiz_text::{Attrs, TextLayer};
        let mut doc = eskiz_core::Document::empty(8, 8);
        doc.push_layer(None, eskiz_core::Props::new("sketch"), None, vec![255; 8 * 8 * 4]);
        let id = doc.push_layer(None, eskiz_core::Props { hold: 1, ..eskiz_core::Props::new("caption") }, None, vec![200; 8 * 8 * 4]);
        doc.texts.insert(id, TextLayer::new("hi", Attrs::default(), 0.0));
        let (snapshot, ids) = snapshot_from_document(&doc);
        let built = FrameList::build(snapshot).unwrap();
        assert_eq!(built.len(), 2, "the text layer is a frame like any other layer");
        assert!(ids.contains(&id));
        assert_eq!(built.hold(1), 1, "and it keeps its hold");
    }
}
