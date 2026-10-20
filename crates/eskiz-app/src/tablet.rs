//! Pen tablet input, the platform-independent half.
//!
//! A platform backend (`tablet_backend.rs`, behind the `tablet` cargo feature) turns OS
//! tablet events into [`PenEvent`]s. [`PenTracker`] folds one frame of those events into a
//! [`PenFrame`]: proximity, hover position, contact state and the contact samples with
//! pressure, altitude and azimuth. The app overlays that on its pointer handling, so a pen
//! drives the same stroke pipeline as the mouse. Everything here is pure, so it is tested
//! with fake events.

use eskiz_core::geometry::altitude_azimuth_from_tilt;
use eframe::egui::{Pos2, Rect};

use crate::app::Tool;
use crate::input::PenReading;

/// One pen sample as the backend reports it. Positions are window-relative egui points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenPose {
    pub pos: [f32; 2],
    /// 0..1 when the device senses pressure.
    pub pressure: Option<f32>,
    /// Tilt from perpendicular in the X and Y directions, radians (W3C `tiltX`/`tiltY`).
    pub tilt: Option<[f32; 2]>,
    /// Barrel rotation in radians (Apple Pencil Pro, Wacom art pens), when the device reports it.
    pub roll: Option<f32>,
    /// Device timestamp in seconds, on the device's own clock.
    pub time: Option<f64>,
}

/// What a backend reports, in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PenEvent {
    /// The pen came into range. `eraser` is true for the eraser end.
    Enter { eraser: bool },
    /// The nib touched the surface.
    Down,
    Pose(PenPose),
    /// The nib left the surface.
    Up,
    /// The pen left range.
    Leave,
    /// The OS cancelled the contact (Android palm rejection: `ACTION_CANCEL`, `FLAG_CANCELED`).
    /// Unlike `Up` the stroke is not finished, it is thrown away.
    Cancel,
}

/// A contact sample, ready for the stroke pipeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    /// Window position in egui points.
    pub pos: Pos2,
    /// Seconds, on the egui clock.
    pub time: f64,
    pub reading: PenReading,
}

/// The pen's state after one frame of events.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PenFrame {
    /// The pen is in range, or touched or left the surface this frame.
    pub active: bool,
    /// The eraser end is the one in range.
    pub eraser: bool,
    /// Last known position.
    pub pos: Option<Pos2>,
    /// The nib touched down this frame.
    pub pressed: bool,
    /// The nib is on the surface now.
    pub down: bool,
    /// The nib lifted this frame.
    pub released: bool,
    /// The OS cancelled the contact this frame: drop the stroke, do not commit it.
    pub cancelled: bool,
    /// Contact samples this frame, oldest first. The first one of a press is at the press point.
    pub samples: Vec<PenSample>,
    /// The pen moved (or touched) this frame.
    pub moved: bool,
}

/// Altitude and azimuth in radians from the two tilt angles, using the same conversion as
/// the engine's W3C path.
pub fn tilt_to_altitude_azimuth(tilt: [f32; 2]) -> (f64, f64) {
    // Keep the angles strictly inside (-90, 90) degrees so a driver glitch cannot give NaN.
    let limit = 89.9f64;
    let tx = f64::from(tilt[0]).to_degrees().clamp(-limit, limit);
    let ty = f64::from(tilt[1]).to_degrees().clamp(-limit, limit);
    altitude_azimuth_from_tilt(tx, ty)
}

/// Pressure, altitude and azimuth for a pose.
pub fn reading_from_pose(pose: &PenPose) -> PenReading {
    let tilt = pose.tilt.filter(|t| t.iter().all(|v| v.is_finite())).map(tilt_to_altitude_azimuth);
    PenReading {
        pressure: pose.pressure.filter(|p| p.is_finite()).map(|p| f64::from(p.clamp(0.0, 1.0))),
        altitude: tilt.map(|t| t.0),
        azimuth: tilt.map(|t| t.1),
        roll: pose.roll.filter(|r| r.is_finite()).map(f64::from),
    }
}

/// Folds backend events into per-frame [`PenFrame`]s.
#[derive(Clone, Debug, Default)]
pub struct PenTracker {
    in_range: bool,
    eraser: bool,
    down: bool,
    /// A `Down` arrived before any pose; the next pose is the press point.
    awaiting_press_pose: bool,
    last: Option<PenPose>,
    /// `egui time - device time`, fixed at the first timestamped pose after the pen enters.
    clock_offset: Option<f64>,
    last_now: f64,
    /// Most recent raw pressure while in range, for the curve editor.
    last_pressure: Option<f32>,
    /// A pen has been seen since start-up.
    seen: bool,
}

impl PenTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// A pen has reported anything yet.
    pub fn seen(&self) -> bool {
        self.seen
    }

    /// The latest pose, while the pen is in range (for the Settings readout).
    pub fn last_pose(&self) -> Option<&PenPose> {
        self.last.as_ref()
    }

    /// The latest raw pressure, while the pen is in range.
    pub fn last_pressure(&self) -> Option<f32> {
        self.last_pressure
    }

    fn sample(&self, pose: &PenPose, time: f64) -> PenSample {
        PenSample { pos: Pos2::new(pose.pos[0], pose.pos[1]), time, reading: reading_from_pose(pose) }
    }

    /// Process one frame of events. `now` is the egui time at the start of the frame.
    pub fn frame(&mut self, events: impl IntoIterator<Item = PenEvent>, now: f64) -> PenFrame {
        let mut f = PenFrame::default();
        // Samples without a device timestamp are spread over the time since the last frame.
        let mut pending: Vec<(PenPose, Option<f64>)> = Vec::new();
        for ev in events {
            match ev {
                PenEvent::Enter { eraser } => {
                    self.seen = true;
                    self.in_range = true;
                    self.eraser = eraser;
                    self.clock_offset = None;
                }
                PenEvent::Pose(p) => {
                    f.moved = true;
                    self.seen = true;
                    self.in_range = true;
                    if let (Some(t), None) = (p.time, self.clock_offset) {
                        self.clock_offset = Some(now - t);
                    }
                    self.last = Some(p);
                    self.last_pressure = p.pressure;
                    let mut contact = self.down;
                    if self.awaiting_press_pose {
                        self.awaiting_press_pose = false;
                        f.pressed = true;
                        contact = true;
                    }
                    if contact {
                        pending.push((p, p.time));
                    }
                }
                PenEvent::Down => {
                    if !self.in_range || self.down {
                        continue;
                    }
                    self.down = true;
                    match self.last {
                        Some(p) => {
                            f.pressed = true;
                            pending.push((p, p.time));
                        }
                        None => self.awaiting_press_pose = true,
                    }
                }
                PenEvent::Up => {
                    self.awaiting_press_pose = false;
                    if self.down {
                        self.down = false;
                        f.released = true;
                    }
                }
                PenEvent::Cancel => {
                    self.awaiting_press_pose = false;
                    if self.down {
                        self.down = false;
                        f.cancelled = true;
                        // Samples gathered before the cancel belong to the dead stroke.
                        pending.clear();
                        f.pressed = false;
                    }
                }
                PenEvent::Leave => {
                    self.awaiting_press_pose = false;
                    if self.down {
                        self.down = false;
                        f.released = true;
                    }
                    self.in_range = false;
                    self.last = None;
                    self.last_pressure = None;
                    self.eraser = false;
                }
            }
        }
        let n = pending.len();
        let span = (now - self.last_now).clamp(0.0, 0.1);
        for (i, (pose, dev_time)) in pending.iter().enumerate() {
            let time = match (dev_time, self.clock_offset) {
                (Some(t), Some(off)) => t + off,
                _ => now - span + span * (i + 1) as f64 / n as f64,
            };
            f.samples.push(self.sample(pose, time));
        }
        self.last_now = now;
        f.active = self.in_range || f.released;
        f.eraser = self.eraser;
        f.down = self.down;
        f.pos = self.last.map(|p| Pos2::new(p.pos[0], p.pos[1]));
        // A contact that begins and ends in one frame with no sample still needs a press point.
        if f.pressed && f.samples.is_empty() {
            if let Some(p) = self.last {
                f.samples.push(self.sample(&p, now));
            }
        }
        f
    }
}

/// Pen state held by the app: the event source, the tracker and this frame's result.
#[derive(Default)]
pub struct PenState {
    pub(crate) source: Option<Box<dyn PenSource>>,
    pub(crate) tracker: PenTracker,
    pub(crate) frame: PenFrame,
    /// Why there is no pen source.
    pub(crate) status: String,
    /// The tool to go back to when the eraser end leaves range.
    pub(crate) saved_tool: Option<Tool>,
    /// Pen data of the stroke in progress, `None` for mouse and touch strokes.
    pub(crate) stroke_reading: Option<PenReading>,
    /// Where the pressure curve editor drew last frame.
    pub(crate) editor_rect: Option<Rect>,
    /// The pen backend connects after the first frames, not in the middle of start-up.
    #[cfg_attr(not(all(feature = "tablet", windows)), allow(dead_code))]
    pub(crate) later: PenLater,
    /// The "pen input was switched off after a freeze" notice is showing.
    pub(crate) banner: bool,
}

/// Frames to draw before the pen backend is connected (Windows).
pub const CONNECT_AFTER_FRAMES: u32 = 8;

/// A pen backend that is waiting to be connected, and what is needed to do it later.
#[derive(Default)]
#[cfg_attr(not(all(feature = "tablet", windows)), allow(dead_code))]
pub struct PenLater {
    #[cfg(all(feature = "tablet", windows))]
    pub(crate) handles: Option<crate::tablet_backend::Handles>,
    /// Frames drawn while waiting.
    pub(crate) frames: u32,
    pub(crate) pending: bool,
    /// The setting as of the last frame, to notice a change.
    pub(crate) setting_seen: Option<bool>,
}

impl PenState {
    /// One line for the preferences panel.
    pub fn status_line(&self) -> String {
        match (&self.source, self.tracker.seen()) {
            (Some(s), true) => format!("Pen detected ({})", s.name()),
            (Some(s), false) => format!("Tablet: {}, no pen seen yet", s.name()),
            (None, _) if self.status.is_empty() => "Tablet: not available".to_owned(),
            (None, _) => self.status.clone(),
        }
    }
}

/// A source of pen events. The real one wraps the OS tablet API; tests use fakes.
pub trait PenSource {
    /// Everything that happened since the last call.
    fn poll(&mut self) -> Vec<PenEvent>;
    /// Short backend name for the preferences panel.
    fn name(&self) -> &'static str;
    /// True when the OS does not also deliver the pen as mouse input, so the app must feed
    /// pointer events to egui itself (the Wayland tablet protocol).
    fn needs_pointer_events(&self) -> bool {
        false
    }
    /// The display scale, set before every `poll`: egui's native pixels per point and the
    /// zoom factor. Sources that report device pixels (Android) divide by them.
    fn set_scale(&mut self, _native_pixels_per_point: f32, _zoom: f32) {}
    /// Non-pen egui events the source produced during the last `poll` (Android fingers, mouse
    /// and keys, which it receives instead of winit). Empty for desktop tablet backends.
    /// True once the backend panicked while polling: the app drops the source and carries on
    /// with the mouse and touch.
    fn failed(&self) -> bool {
        false
    }
    fn take_ui_events(&mut self) -> Vec<eframe::egui::Event> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32, y: f32, pressure: f32, time: Option<f64>) -> PenPose {
        PenPose { pos: [x, y], pressure: Some(pressure), tilt: None, roll: None, time }
    }

    #[test]
    fn pose_maps_pressure_and_tilt() {
        let p = PenPose { pos: [1.0, 2.0], pressure: Some(0.4), tilt: Some([0.0, 0.0]), roll: None, time: None };
        let r = reading_from_pose(&p);
        assert!((r.pressure.unwrap() - 0.4).abs() < 1e-6);
        assert!((r.altitude.unwrap() - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert!(r.azimuth.unwrap().abs() < 1e-9);
    }

    #[test]
    fn tilt_lowers_altitude_and_sets_azimuth() {
        // Leaning towards +X by 45 degrees: altitude drops below 90, azimuth 0.
        let (alt, az) = tilt_to_altitude_azimuth([std::f32::consts::FRAC_PI_4, 0.0]);
        assert!(alt < std::f64::consts::FRAC_PI_2 - 0.5 && alt > 0.5, "{alt}");
        assert!(az.abs() < 1e-9);
        // Leaning towards +Y (toward the user): azimuth +90 degrees.
        let (_, az) = tilt_to_altitude_azimuth([0.0, 0.5]);
        assert!((az - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        // Glitchy values never give NaN.
        let (alt, az) = tilt_to_altitude_azimuth([4.0, -9.0]);
        assert!(alt.is_finite() && az.is_finite());
    }

    #[test]
    fn missing_axes_stay_none() {
        let p = PenPose { pos: [0.0, 0.0], pressure: None, tilt: None, roll: None, time: None };
        assert_eq!(reading_from_pose(&p), PenReading::default());
        let p = PenPose { pos: [0.0, 0.0], pressure: Some(f32::NAN), tilt: Some([f32::NAN, 0.0]), roll: None, time: None };
        assert_eq!(reading_from_pose(&p), PenReading::default());
    }

    #[test]
    fn hover_has_no_contact_samples() {
        let mut t = PenTracker::new();
        let f = t.frame([PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(10.0, 20.0, 0.0, None))], 1.0);
        assert!(f.active && !f.down && !f.pressed && f.samples.is_empty());
        assert_eq!(f.pos, Some(Pos2::new(10.0, 20.0)));
        assert!(t.seen());
    }

    #[test]
    fn press_move_release_across_frames() {
        let mut t = PenTracker::new();
        t.frame([PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(10.0, 20.0, 0.0, None))], 1.0);
        let f = t.frame([PenEvent::Down, PenEvent::Pose(pose(11.0, 20.0, 0.3, None))], 1.016);
        assert!(f.pressed && f.down && !f.released);
        // The press sample is at the pre-press position, then the new pose follows.
        assert_eq!(f.samples.len(), 2);
        assert_eq!(f.samples[0].pos, Pos2::new(10.0, 20.0));
        assert_eq!(f.samples[1].pos, Pos2::new(11.0, 20.0));
        assert!((f.samples[1].reading.pressure.unwrap() - 0.3).abs() < 1e-6);
        let f = t.frame([PenEvent::Pose(pose(12.0, 21.0, 0.6, None)), PenEvent::Pose(pose(13.0, 22.0, 0.7, None))], 1.032);
        assert!(f.down && !f.pressed);
        assert_eq!(f.samples.len(), 2);
        let f = t.frame([PenEvent::Pose(pose(14.0, 22.0, 0.2, None)), PenEvent::Up], 1.048);
        assert!(f.released && !f.down && f.active);
        assert_eq!(f.samples.len(), 1);
        let f = t.frame([PenEvent::Pose(pose(15.0, 22.0, 0.0, None))], 1.064);
        assert!(f.samples.is_empty() && f.active);
        let f = t.frame([PenEvent::Leave], 1.08);
        assert!(!f.active && f.pos.is_none());
    }

    #[test]
    fn a_tap_inside_one_frame_still_gives_a_press_sample() {
        let mut t = PenTracker::new();
        t.frame([PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(5.0, 5.0, 0.0, None))], 0.0);
        let f = t.frame([PenEvent::Down, PenEvent::Up], 0.016);
        assert!(f.pressed && f.released && !f.down);
        assert_eq!(f.samples.len(), 1);
    }

    #[test]
    fn down_before_first_pose_waits_for_it() {
        let mut t = PenTracker::new();
        let f = t.frame([PenEvent::Enter { eraser: false }, PenEvent::Down, PenEvent::Pose(pose(7.0, 8.0, 0.5, None))], 0.0);
        assert!(f.pressed && f.down);
        assert_eq!(f.samples.len(), 1);
        assert_eq!(f.samples[0].pos, Pos2::new(7.0, 8.0));
    }

    #[test]
    fn leaving_range_while_down_releases() {
        let mut t = PenTracker::new();
        t.frame([PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(1.0, 1.0, 0.0, None)), PenEvent::Down], 0.0);
        let f = t.frame([PenEvent::Leave], 0.016);
        assert!(f.released && !f.down && f.active);
        assert!(!t.frame([], 0.032).active);
    }

    #[test]
    fn eraser_end_is_reported_until_the_pen_leaves() {
        let mut t = PenTracker::new();
        let f = t.frame([PenEvent::Enter { eraser: true }, PenEvent::Pose(pose(1.0, 1.0, 0.0, None))], 0.0);
        assert!(f.eraser);
        assert!(t.frame([], 0.016).eraser);
        t.frame([PenEvent::Leave], 0.032);
        assert!(!t.frame([], 0.048).eraser);
        let f = t.frame([PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(1.0, 1.0, 0.0, None))], 0.064);
        assert!(!f.eraser);
    }

    #[test]
    fn device_timestamps_are_shifted_onto_the_egui_clock() {
        let mut t = PenTracker::new();
        // Device clock is at 500 s when egui's is at 2 s.
        t.frame([PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(0.0, 0.0, 0.0, Some(500.0)))], 2.0);
        let f = t.frame(
            [PenEvent::Down, PenEvent::Pose(pose(1.0, 0.0, 0.5, Some(500.004))), PenEvent::Pose(pose(2.0, 0.0, 0.5, Some(500.012)))],
            2.016,
        );
        let times: Vec<f64> = f.samples.iter().map(|s| s.time).collect();
        assert_eq!(times.len(), 3);
        assert!((times[1] - 2.004).abs() < 1e-9 && (times[2] - 2.012).abs() < 1e-9, "{times:?}");
        assert!(times.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn untimestamped_samples_spread_over_the_frame() {
        let mut t = PenTracker::new();
        t.frame([PenEvent::Enter { eraser: false }, PenEvent::Pose(pose(0.0, 0.0, 0.0, None))], 1.0);
        let f = t.frame(
            [PenEvent::Down, PenEvent::Pose(pose(1.0, 0.0, 0.5, None)), PenEvent::Pose(pose(2.0, 0.0, 0.5, None))],
            1.03,
        );
        let times: Vec<f64> = f.samples.iter().map(|s| s.time).collect();
        assert!(times.windows(2).all(|w| w[1] > w[0]), "{times:?}");
        assert!(times.iter().all(|t| (1.0..=1.03 + 1e-9).contains(t)));
    }

    #[test]
    fn pen_events_without_enter_still_work() {
        // Some backends send poses without a proximity event.
        let mut t = PenTracker::new();
        let f = t.frame([PenEvent::Pose(pose(1.0, 1.0, 0.0, None)), PenEvent::Down, PenEvent::Pose(pose(2.0, 1.0, 0.5, None))], 0.0);
        assert!(f.pressed && f.down);
    }
}
