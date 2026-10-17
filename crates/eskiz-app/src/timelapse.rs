//! Timelapse in the app. Each open document has a recorder that takes one frame per committed change, and
//! the segments are kept in the library folder. Export and replay move the recorder onto a worker thread and
//! take it back when the job ends, so the canvas stays responsive.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use eskiz_media::{duration_for_fps, Frame, Quality, Resolution, SegmentStore, TimelapseRecorder, TimelapseSettings};

use crate::app::{EskizApp, Prefs};
use crate::document::Document;
use crate::export::expand_home;
use crate::session::Session;

/// Quality levels in the order the Canvas row cycles through them.
pub const QUALITIES: [Quality; 4] = [Quality::Low, Quality::Standard, Quality::High, Quality::Studio];
/// Resolution boxes in the order the Canvas row cycles through them.
pub const RESOLUTIONS: [Resolution; 3] = [Resolution::P1080, Resolution::K2, Resolution::K4];
/// Longest side of a replay frame, in pixels.
const REPLAY_SIDE: u32 = 480;
/// Most frames a replay keeps. The recording is sampled evenly down to this.
const REPLAY_FRAMES: usize = 120;

/// The timelapse settings from the preferences, at the documented 30 fps.
pub fn settings_from_prefs(prefs: &Prefs) -> TimelapseSettings {
    TimelapseSettings {
        fps: 30,
        quality: QUALITIES[usize::from(prefs.timelapse_quality).min(QUALITIES.len() - 1)],
        resolution: RESOLUTIONS[usize::from(prefs.timelapse_resolution).min(RESOLUTIONS.len() - 1)],
        background: [255, 255, 255],
    }
}

/// Label of a quality index.
pub fn quality_label(index: u8) -> &'static str {
    ["Low", "Standard", "High", "Studio"][usize::from(index).min(3)]
}

/// Label of a resolution index.
pub fn resolution_label(index: u8) -> &'static str {
    ["1080p", "2K", "4K"][usize::from(index).min(2)]
}

/// Records the document's current composite as one timelapse frame. A composite identical to the last one
/// captured (`last`) is skipped, so a change that does not alter the picture, such as an animation setting,
/// adds no frame. Returns whether a frame was recorded.
pub fn capture(recorder: &mut TimelapseRecorder, session: &Session, last: &mut Option<u64>) -> Result<bool, String> {
    let pixels = session.flatten_for_export();
    let mut hasher = DefaultHasher::new();
    pixels.hash(&mut hasher);
    let hash = hasher.finish();
    if *last == Some(hash) {
        return Ok(false);
    }
    *last = Some(hash);
    let frame = Frame::new(
        session.doc.width as u32,
        session.doc.height as u32,
        pixels,
        duration_for_fps(recorder.settings().fps),
    )
    .map_err(|e| e.to_string())?;
    recorder.commit(&frame).map_err(|e| e.to_string())?;
    Ok(true)
}

/// The recorder of the open document and what it has seen.
struct Active {
    key: String,
    settings: TimelapseSettings,
    /// `None` after a failure, or while a job holds it.
    recorder: Option<TimelapseRecorder>,
    /// Session edit counter at the last capture.
    last_edits: u64,
    /// Hash of the composite at the last capture.
    last_hash: Option<u64>,
}

/// A recorder job on a worker thread. The recorder comes back with the result.
enum Job {
    Export(Receiver<(TimelapseRecorder, Result<PathBuf, String>)>),
    Preview {
        rx: Receiver<(TimelapseRecorder, Result<Vec<Frame>, String>)>,
        fps: u32,
    },
}

/// The replay overlay: decoded frames as textures, and the playhead.
pub(crate) struct Replay {
    pub frames: Vec<TextureHandle>,
    pub fps: u32,
    pub index: usize,
    pub playing: bool,
    /// Frames played so far, fractional, since the last whole frame.
    accum: f64,
    /// App time of the last update, in seconds.
    clock: f64,
}

/// Timelapse state for the app.
#[derive(Default)]
pub(crate) struct TimelapseUi {
    active: Option<Active>,
    job: Option<Job>,
    /// Counts recorders, so each one writes its own folder.
    generation: u32,
    pub replay: Option<Replay>,
}

impl TimelapseUi {
    /// True when a recorder has been started for the open document.
    pub fn has_recording(&self) -> bool {
        self.active.as_ref().is_some_and(|a| a.recorder.is_some())
    }

    /// Frames recorded for the open document (0 without a recorder).
    #[cfg(test)]
    pub fn recorded_frames(&self) -> u64 {
        self.active.as_ref().and_then(|a| a.recorder.as_ref()).map_or(0, |r| r.recorded_frames())
    }

    /// True when the recording is kept as small pictures because MP4 encoding is unavailable.
    pub fn is_unencoded(&self) -> bool {
        self.active.as_ref().and_then(|a| a.recorder.as_ref()).is_some_and(|r| r.is_unencoded())
    }
}

impl EskizApp {
    /// Per-frame timelapse work: report finished jobs, keep the replay overlay, and capture the commit.
    pub(crate) fn timelapse_frame(&mut self, ctx: &egui::Context) {
        self.timelapse_poll(ctx);
        self.replay_window(ctx);
        if self.three_d.is_some() {
            // The 3D view paints into a model, not the 2D document, so there is nothing to capture
            // here, and the recorder must not be switched to another document while it is open.
            return;
        }
        if self.timelapse.job.is_some() {
            // The recorder is on a worker. Commits made meanwhile are captured at the next commit.
            return;
        }
        if !self.prefs.timelapse {
            return;
        }
        let key = self
            .gal
            .current
            .as_ref()
            .and_then(|c| c.id.clone())
            .unwrap_or_else(|| "scratch".to_owned());
        let settings = settings_from_prefs(&self.prefs);
        let stale = match &self.timelapse.active {
            None => true,
            Some(a) => a.key != key || a.settings != settings,
        };
        if stale {
            if let Some(mut old) = self.timelapse.active.take().and_then(|a| a.recorder) {
                let _ = old.finish_segment();
            }
            self.timelapse.generation += 1;
            let folder = format!("{key}-{}", self.timelapse.generation);
            let store = match self.gal.library.as_ref() {
                Some(lib) => SegmentStore::Directory(lib.root().join("timelapse").join(folder)),
                None => SegmentStore::Memory,
            };
            let (w, h) = (self.session.doc.width as u32, self.session.doc.height as u32);
            let (recorder, failure) = match TimelapseRecorder::new(w, h, settings, store) {
                Ok(r) => (Some(r), None),
                Err(e) => (None, Some(e.to_string())),
            };
            if let Some(message) = failure {
                self.show_toast(format!("Timelapse is off: {message}"), 4.0);
            }
            self.timelapse.active = Some(Active {
                key,
                settings,
                recorder,
                last_edits: self.session.edits(),
                last_hash: None,
            });
        }
        let edits = self.session.edits();
        let stroking = self.session.is_stroking();
        let mut failure = None;
        if let Some(active) = self.timelapse.active.as_mut() {
            if edits != active.last_edits && !stroking {
                active.last_edits = edits;
                if let Some(rec) = active.recorder.as_mut() {
                    if let Err(e) = capture(rec, &self.session, &mut active.last_hash) {
                        failure = Some(e);
                        active.recorder = None;
                    }
                }
            }
        }
        if let Some(e) = failure {
            self.show_toast(format!("Timelapse stopped: {e}"), 4.0);
        }
    }

    /// Takes the recorder out of the active document for a worker job. `None` if there is none.
    fn timelapse_take(&mut self) -> Option<TimelapseRecorder> {
        self.timelapse.active.as_mut().and_then(|a| a.recorder.take())
    }

    /// Puts a recorder back after its job.
    fn timelapse_restore(&mut self, rec: TimelapseRecorder) {
        if let Some(active) = self.timelapse.active.as_mut() {
            if active.recorder.is_none() {
                active.recorder = Some(rec);
            }
        }
    }

    /// Starts writing the recording to an MP4: the full length, or the 30 s summary.
    pub(crate) fn start_timelapse_export(&mut self, ctx: &egui::Context, full: bool) {
        if self.timelapse.job.is_some() {
            self.show_toast("Timelapse is busy.", 2.0);
            return;
        }
        let Some(mut rec) = self.timelapse_take() else {
            self.show_toast("Nothing is recorded yet.", 2.5);
            return;
        };
        if rec.recorded_frames() == 0 {
            self.timelapse_restore(rec);
            self.show_toast("Nothing is recorded yet.", 2.5);
            return;
        }
        let path = expand_home(&self.export_path).with_extension("mp4");
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = if full {
                rec.export_full(&path).map(|_| ())
            } else {
                rec.export_summary(&path)
            }
            .map(|()| path.clone())
            .map_err(|e| e.to_string());
            let _ = tx.send((rec, result));
            repaint.request_repaint();
        });
        self.timelapse.job = Some(Job::Export(rx));
    }

    /// Decodes the recording for the replay overlay, on a worker thread.
    pub(crate) fn start_timelapse_replay(&mut self, ctx: &egui::Context) {
        if self.timelapse.job.is_some() {
            self.show_toast("Timelapse is busy.", 2.0);
            return;
        }
        let Some(mut rec) = self.timelapse_take() else {
            self.show_toast("Nothing is recorded yet.", 2.5);
            return;
        };
        if rec.recorded_frames() == 0 {
            self.timelapse_restore(rec);
            self.show_toast("Nothing is recorded yet.", 2.5);
            return;
        }
        let fps = rec.settings().fps;
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = rec
                .finish_segment()
                .and_then(|()| rec.preview_frames(REPLAY_FRAMES, REPLAY_SIDE))
                .map_err(|e| e.to_string());
            let _ = tx.send((rec, result));
            repaint.request_repaint();
        });
        self.timelapse.job = Some(Job::Preview { rx, fps });
    }

    fn timelapse_poll(&mut self, ctx: &egui::Context) {
        let Some(job) = self.timelapse.job.take() else {
            return;
        };
        match job {
            Job::Export(rx) => match rx.try_recv() {
                Ok((rec, result)) => {
                    self.timelapse_restore(rec);
                    match result {
                        Ok(path) => self.show_toast(format!("Exported timelapse to {}", path.display()), 3.5),
                        Err(e) => self.show_toast(format!("Timelapse export failed: {e}"), 4.0),
                    }
                }
                Err(TryRecvError::Empty) => self.timelapse.job = Some(Job::Export(rx)),
                Err(TryRecvError::Disconnected) => self.show_toast("Timelapse export failed.", 3.0),
            },
            Job::Preview { rx, fps } => match rx.try_recv() {
                Ok((rec, result)) => {
                    self.timelapse_restore(rec);
                    match result {
                        Ok(frames) if !frames.is_empty() => {
                            let textures = frames
                                .iter()
                                .enumerate()
                                .map(|(i, f)| {
                                    let image = ColorImage::from_rgba_unmultiplied(
                                        [f.width() as usize, f.height() as usize],
                                        f.pixels(),
                                    );
                                    ctx.load_texture(format!("timelapse_replay_{i}"), image, TextureOptions::LINEAR)
                                })
                                .collect();
                            self.timelapse.replay = Some(Replay {
                                frames: textures,
                                fps: fps.max(1),
                                index: 0,
                                playing: true,
                                accum: 0.0,
                                clock: self.now,
                            });
                        }
                        Ok(_) => self.show_toast("Nothing is recorded yet.", 2.5),
                        Err(e) => self.show_toast(format!("Replay failed: {e}"), 4.0),
                    }
                }
                Err(TryRecvError::Empty) => self.timelapse.job = Some(Job::Preview { rx, fps }),
                Err(TryRecvError::Disconnected) => self.show_toast("Replay failed.", 3.0),
            },
        }
    }

    /// The replay overlay: plays the decoded frames at the recording's fps.
    fn replay_window(&mut self, ctx: &egui::Context) {
        let now = self.now;
        let Some(replay) = self.timelapse.replay.as_mut() else {
            return;
        };
        if replay.playing && !replay.frames.is_empty() {
            replay.accum += (now - replay.clock).max(0.0) * f64::from(replay.fps);
            while replay.accum >= 1.0 {
                replay.accum -= 1.0;
                replay.index = (replay.index + 1) % replay.frames.len();
            }
            ctx.request_repaint();
        }
        replay.clock = now;
        let mut open = true;
        let mut toggle = false;
        let last = replay.frames.len().saturating_sub(1);
        egui::Window::new("Timelapse replay")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if let Some(tex) = replay.frames.get(replay.index) {
                    ui.add(egui::Image::new(tex).max_width(480.0));
                }
                ui.horizontal(|ui| {
                    if ui.button(if replay.playing { "Pause" } else { "Play" }).clicked() {
                        toggle = true;
                    }
                    let mut index = replay.index;
                    if ui.add(egui::Slider::new(&mut index, 0..=last).show_value(false)).changed() {
                        replay.index = index;
                        replay.playing = false;
                    }
                    ui.label(format!("{} / {}", replay.index + 1, replay.frames.len()));
                });
            });
        if toggle {
            replay.playing = !replay.playing;
        }
        if !open {
            self.timelapse.replay = None;
        }
    }
}

/// Records three commits into a memory recorder and exports the result. Run by `--selftest`.
pub fn selftest() -> Result<String, String> {
    let mut session = Session::new(Document::new(256, 256));
    let mut rec = TimelapseRecorder::new(256, 256, TimelapseSettings::default(), SegmentStore::Memory)
        .map_err(|e| e.to_string())?;
    record_three(&mut session, &mut rec)?;
    let recorded = rec.recorded_frames();
    if recorded != 3 {
        return Err(format!("timelapse: recorded {recorded} frames for 3 commits"));
    }
    if !eskiz_media::encoder_available() {
        let kept = rec.export_full(std::path::Path::new("unused.mp4"));
        return match kept {
            Err(eskiz_media::MediaError::EncoderUnavailable(_)) if rec.is_unencoded() => {
                Ok("  timelapse: 3 commits kept as small pictures (MP4 is off in this build)".to_owned())
            }
            other => Err(format!("timelapse: expected the no-encoder mode, got {other:?}")),
        };
    }
    let path = std::env::temp_dir().join("eskiz_timelapse_selftest.mp4");
    let exported = rec.export_full(&path).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&path);
    exported?;
    Ok("  timelapse: 3 commits recorded as 3 frames, full-length export written".to_owned())
}

/// Three fills, each captured as one commit.
fn record_three(session: &mut Session, rec: &mut TimelapseRecorder) -> Result<(), String> {
    let id = session.doc.layer_ids()[0];
    let mut last = None;
    for colour in [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]] {
        session.fill_layer(id, colour).ok_or("the fill was refused")?;
        capture(rec, session, &mut last)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_commits_record_three_frames_and_export() {
        if !eskiz_media::encoder_available() {
            return;
        }
        let mut session = Session::new(Document::new(256, 256));
        let mut rec = TimelapseRecorder::new(256, 256, TimelapseSettings::default(), SegmentStore::Memory).unwrap();
        record_three(&mut session, &mut rec).unwrap();
        assert_eq!(rec.recorded_frames(), 3);
        let dir = std::env::temp_dir().join(format!("eskiz_tl_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("timelapse.mp4");
        assert_eq!(rec.export_full(&path).unwrap(), 3);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unchanged_composite_is_not_recorded_again() {
        let mut session = Session::new(Document::new(256, 256));
        let mut rec = TimelapseRecorder::new(256, 256, TimelapseSettings::default(), SegmentStore::Memory).unwrap();
        let mut last = None;
        assert!(capture(&mut rec, &session, &mut last).unwrap());
        assert!(!capture(&mut rec, &session, &mut last).unwrap());
        let id = session.doc.layer_ids()[0];
        session.fill_layer(id, [10, 20, 30]).unwrap();
        assert!(capture(&mut rec, &session, &mut last).unwrap());
        assert_eq!(rec.recorded_frames(), 2);
    }

    #[test]
    fn a_small_document_is_upscaled_for_capture() {
        // A document with a longer side of 256 px or less is multiplied by 4 before fitting.
        let rec = TimelapseRecorder::new(64, 64, TimelapseSettings::default(), SegmentStore::Memory).unwrap();
        assert_eq!(rec.capture_size(), (256, 256));
    }

    #[test]
    fn settings_come_from_the_preference_indices() {
        let prefs = Prefs { timelapse_quality: 3, timelapse_resolution: 2, ..Prefs::default() };
        let s = settings_from_prefs(&prefs);
        assert_eq!(s.quality, Quality::Studio);
        assert_eq!(s.resolution, Resolution::K4);
        assert_eq!(quality_label(0), "Low");
        assert_eq!(resolution_label(1), "2K");
    }
}
