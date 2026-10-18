//! macOS tablet input: Wacom-style tablets and Sidecar / Apple Pencil.
//!
//! macOS delivers a tablet pen as ordinary mouse events (`NSEvent` types `LeftMouseDown`,
//! `LeftMouseDragged`, `LeftMouseUp`, `MouseMoved`) whose `subtype` is `TabletPoint`, plus
//! standalone `TabletPoint` and `TabletProximity` events. Those carry `pressure`, `tilt`,
//! `rotation` and, for proximity events, the `pointingDeviceType` (pen or eraser end).
//! winit and egui read only the position, so a pen looked like a mouse with no pressure.
//!
//! This module has two halves:
//!
//! * [`MacMapper`]: a pure function-like state machine from [`RawMacEvent`] (plain numbers
//!   copied out of an `NSEvent`) to the platform-independent [`PenEvent`]s that
//!   [`PenTracker`](crate::tablet::PenTracker) already folds into the stroke pipeline. It is
//!   compiled on every platform and unit tested with synthetic values.
//! * `native` (macOS only): an `NSEvent` local monitor that copies each event into a
//!   [`RawMacEvent`] and queues it. The unsafe/FFI surface is that one call and nothing else.
//!   The monitor returns the event unchanged, so mouse, trackpad and egui behave as before;
//!   the app gives the pen priority only while the nib is down (see `EskizApp::overlay_pen`).
//!
//! Design notes for the other platforms (not implemented here): Windows already goes through
//! Windows Ink (`octotablet`); a `WM_POINTER` reader would add barrel rotation and non-Wacom
//! pens. Linux X11 would need XInput2 valuators or libinput; Wayland uses the tablet protocol
//! through `octotablet`.

use crate::tablet::{PenEvent, PenPose};

/// What kind of `NSEvent` it was (the subset the monitor listens to).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacKind {
    MouseMoved,
    MouseDown,
    MouseDragged,
    MouseUp,
    /// Standalone `NSEventTypeTabletPoint`.
    TabletPoint,
    /// Standalone `NSEventTypeTabletProximity`.
    TabletProximity,
}

/// `NSEventSubtype` of a mouse event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacSubtype {
    /// An ordinary mouse or trackpad event.
    Mouse,
    TabletPoint,
    TabletProximity,
    /// Touch or anything else: not a pen.
    Other,
}

/// `NSPointingDeviceType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacDevice {
    Unknown,
    Pen,
    /// A puck or mouse-like tool on a tablet.
    Cursor,
    Eraser,
}

/// The numbers copied out of one `NSEvent`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawMacEvent {
    pub kind: MacKind,
    pub subtype: MacSubtype,
    /// `locationInWindow`: points, origin at the content view's bottom-left.
    pub location: [f32; 2],
    /// Height of the window's content view in points, to flip `location` to egui's y-down.
    pub view_height: f32,
    /// `pressure`, 0..1.
    pub pressure: f32,
    /// `tilt`, each axis -1..1 (+-1 is 90 degrees).
    pub tilt: [f32; 2],
    /// `rotation`: barrel rotation in degrees.
    pub rotation_deg: f32,
    /// `timestamp`: seconds since system start.
    pub timestamp: f64,
    /// Proximity events only.
    pub device: MacDevice,
    /// Proximity events only: the tool came into range.
    pub entering: bool,
}

/// A plain mouse event this long after the last tablet event ends the pen's presence
/// (a real mouse took over); shorter gaps are Sidecar sending both flavours.
const MOUSE_TAKEOVER_SECS: f64 = 0.35;

/// Maps `NSEvent` data to [`PenEvent`]s, keeping the little state it needs.
#[derive(Clone, Debug, Default)]
pub struct MacMapper {
    in_range: bool,
    down: bool,
    last_tablet_time: f64,
    eraser: bool,
}

/// True for events that carry tablet data.
fn is_tablet(ev: &RawMacEvent) -> bool {
    ev.kind == MacKind::TabletPoint
        || ev.kind == MacKind::TabletProximity
        || matches!(ev.subtype, MacSubtype::TabletPoint | MacSubtype::TabletProximity)
}

/// The pose of a tablet-point event, in egui points (before the zoom factor).
pub fn pose_of(ev: &RawMacEvent) -> PenPose {
    let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
    let half_pi = std::f32::consts::FRAC_PI_2;
    PenPose {
        pos: [ev.location[0], ev.view_height - ev.location[1]],
        pressure: Some(finite(ev.pressure).clamp(0.0, 1.0)),
        // Cocoa's tilt y grows away from the user; W3C `tiltY` grows towards the user.
        tilt: Some([finite(ev.tilt[0]).clamp(-1.0, 1.0) * half_pi, -finite(ev.tilt[1]).clamp(-1.0, 1.0) * half_pi]),
        roll: Some(finite(ev.rotation_deg).to_radians()),
        time: Some(ev.timestamp),
    }
}

impl MacMapper {
    pub fn new() -> Self {
        Self::default()
    }

    fn enter(&mut self, eraser: bool, out: &mut Vec<PenEvent>) {
        if self.down {
            out.push(PenEvent::Up);
            self.down = false;
        }
        self.in_range = true;
        self.eraser = eraser;
        out.push(PenEvent::Enter { eraser });
    }

    fn leave(&mut self, out: &mut Vec<PenEvent>) {
        if self.in_range {
            out.push(PenEvent::Leave);
        }
        self.in_range = false;
        self.down = false;
        self.eraser = false;
    }

    /// Feed one event; returns what the pen did, oldest first.
    pub fn map(&mut self, ev: &RawMacEvent) -> Vec<PenEvent> {
        let mut out = Vec::new();
        if !is_tablet(ev) {
            // A mouse or trackpad event. It ends the pen's presence only when no tablet event
            // came just before it.
            if self.in_range && ev.timestamp - self.last_tablet_time > MOUSE_TAKEOVER_SECS {
                self.leave(&mut out);
            }
            return out;
        }
        self.last_tablet_time = ev.timestamp;
        if ev.kind == MacKind::TabletProximity || ev.subtype == MacSubtype::TabletProximity {
            match (ev.entering, ev.device) {
                (true, MacDevice::Pen | MacDevice::Eraser | MacDevice::Unknown) => {
                    let eraser = ev.device == MacDevice::Eraser;
                    if !self.in_range || self.down || self.eraser != eraser {
                        self.enter(eraser, &mut out);
                    }
                }
                // A puck is a pointer, not a pen.
                (true, MacDevice::Cursor) => {}
                (false, _) => self.leave(&mut out),
            }
            return out;
        }
        // Some drivers never send proximity events: the first point event brings the pen in.
        if !self.in_range {
            self.enter(false, &mut out);
        }
        let pose = pose_of(ev);
        match ev.kind {
            MacKind::MouseDown => {
                out.push(PenEvent::Pose(pose));
                if !self.down {
                    self.down = true;
                    out.push(PenEvent::Down);
                }
            }
            MacKind::MouseUp => {
                if self.down {
                    self.down = false;
                    out.push(PenEvent::Up);
                }
            }
            // Dragged, moved and standalone points. A drag without a down (the press happened
            // before the monitor saw it) still draws: treat the first one as the press.
            MacKind::MouseDragged | MacKind::MouseMoved | MacKind::TabletPoint => {
                out.push(PenEvent::Pose(pose));
                if ev.kind == MacKind::MouseDragged && !self.down {
                    self.down = true;
                    out.push(PenEvent::Down);
                }
            }
            MacKind::TabletProximity => {}
        }
        out
    }

    /// The pen is in range as far as the mapper knows.
    pub fn in_range(&self) -> bool {
        self.in_range
    }
}

#[cfg(all(target_os = "macos", feature = "tablet"))]
pub use native::MacSource;

#[cfg(all(target_os = "macos", feature = "tablet"))]
mod native {
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventSubtype, NSEventType, NSPointingDeviceType};

    use super::{MacDevice, MacKind, MacMapper, MacSubtype, RawMacEvent};
    use crate::tablet::{PenEvent, PenSource};

    type Queue = Arc<Mutex<Vec<RawMacEvent>>>;

    /// Reads tablet events with an `NSEvent` local monitor (main thread only).
    pub struct MacSource {
        monitor: Option<Retained<AnyObject>>,
        queue: Queue,
        mapper: MacMapper,
    }

    fn kind_of(t: NSEventType) -> Option<MacKind> {
        Some(match t {
            NSEventType::MouseMoved => MacKind::MouseMoved,
            NSEventType::LeftMouseDown => MacKind::MouseDown,
            NSEventType::LeftMouseDragged => MacKind::MouseDragged,
            NSEventType::LeftMouseUp => MacKind::MouseUp,
            NSEventType::TabletPoint => MacKind::TabletPoint,
            NSEventType::TabletProximity => MacKind::TabletProximity,
            _ => return None,
        })
    }

    /// Copy the numbers out of an event. Only reads properties valid for these event types.
    fn read(event: &NSEvent, mtm: MainThreadMarker) -> Option<RawMacEvent> {
        let kind = kind_of(event.r#type())?;
        let mouse_like = !matches!(kind, MacKind::TabletPoint | MacKind::TabletProximity);
        let subtype = if mouse_like {
            match event.subtype() {
                NSEventSubtype::MouseEvent => MacSubtype::Mouse,
                NSEventSubtype::TabletPoint => MacSubtype::TabletPoint,
                NSEventSubtype::TabletProximity => MacSubtype::TabletProximity,
                _ => MacSubtype::Other,
            }
        } else {
            MacSubtype::Other
        };
        let tablet = !mouse_like || subtype != MacSubtype::Mouse;
        let proximity = kind == MacKind::TabletProximity || subtype == MacSubtype::TabletProximity;
        let loc = event.locationInWindow();
        let view_height = event
            .window(mtm)
            .and_then(|w| w.contentView())
            .map_or(0.0, |v| v.bounds().size.height);
        let (pressure, tilt, rotation) = if tablet && !proximity {
            let t = event.tilt();
            (event.pressure(), [t.x as f32, t.y as f32], event.rotation())
        } else {
            (0.0, [0.0, 0.0], 0.0)
        };
        let (device, entering) = if proximity {
            let d = event.pointingDeviceType();
            let device = if d == NSPointingDeviceType::Pen {
                MacDevice::Pen
            } else if d == NSPointingDeviceType::Eraser {
                MacDevice::Eraser
            } else if d == NSPointingDeviceType::Cursor {
                MacDevice::Cursor
            } else {
                MacDevice::Unknown
            };
            (device, event.isEnteringProximity())
        } else {
            (MacDevice::Unknown, false)
        };
        Some(RawMacEvent {
            kind,
            subtype,
            location: [loc.x as f32, loc.y as f32],
            view_height: view_height as f32,
            pressure,
            tilt,
            rotation_deg: rotation,
            timestamp: event.timestamp(),
            device,
            entering,
        })
    }

    impl MacSource {
        /// Install the monitor. Fails off the main thread.
        pub fn new() -> Result<Self, String> {
            let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
            let queue: Queue = Arc::new(Mutex::new(Vec::new()));
            let q = Arc::clone(&queue);
            let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
                // SAFETY: AppKit passes a valid event for the duration of the call.
                let ev = unsafe { event.as_ref() };
                if let Some(raw) = read(ev, mtm) {
                    if let Ok(mut q) = q.lock() {
                        // A pen at 240 Hz for a second is small; cap against a stalled app.
                        if q.len() < 8192 {
                            q.push(raw);
                        }
                    }
                }
                event.as_ptr()
            });
            let mask = NSEventMask::LeftMouseDown
                | NSEventMask::LeftMouseUp
                | NSEventMask::LeftMouseDragged
                | NSEventMask::MouseMoved
                | NSEventMask::TabletPoint
                | NSEventMask::TabletProximity;
            // SAFETY: the block returns the event it was given, which is a valid pointer.
            let monitor = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
            if monitor.is_none() {
                return Err("macOS refused the event monitor".to_owned());
            }
            Ok(Self { monitor, queue, mapper: MacMapper::new() })
        }
    }

    impl Drop for MacSource {
        fn drop(&mut self) {
            if let Some(m) = self.monitor.take() {
                // SAFETY: `m` came from `addLocalMonitor...`.
                unsafe { NSEvent::removeMonitor(&m) };
            }
        }
    }

    impl PenSource for MacSource {
        fn poll(&mut self) -> Vec<PenEvent> {
            let raws: Vec<RawMacEvent> = self.queue.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
            raws.iter().flat_map(|r| self.mapper.map(r)).collect()
        }

        fn name(&self) -> &'static str {
            "macOS tablet events"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tablet::PenTracker;

    fn ev(kind: MacKind, subtype: MacSubtype, x: f32, y: f32, pressure: f32, t: f64) -> RawMacEvent {
        RawMacEvent {
            kind,
            subtype,
            location: [x, y],
            view_height: 600.0,
            pressure,
            tilt: [0.0, 0.0],
            rotation_deg: 0.0,
            timestamp: t,
            device: MacDevice::Unknown,
            entering: false,
        }
    }

    fn prox(entering: bool, device: MacDevice, t: f64) -> RawMacEvent {
        RawMacEvent { entering, device, ..ev(MacKind::TabletProximity, MacSubtype::Other, 0.0, 0.0, 0.0, t) }
    }

    #[test]
    fn pose_flips_y_scales_tilt_and_converts_rotation() {
        let mut e = ev(MacKind::TabletPoint, MacSubtype::Other, 100.0, 50.0, 0.75, 2.0);
        e.tilt = [0.5, -1.0];
        e.rotation_deg = 90.0;
        let p = pose_of(&e);
        assert_eq!(p.pos, [100.0, 550.0]);
        assert_eq!(p.pressure, Some(0.75));
        let t = p.tilt.unwrap();
        assert!((t[0] - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert!((t[1] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((p.roll.unwrap() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(p.time, Some(2.0));
    }

    #[test]
    fn garbage_values_stay_in_range() {
        let mut e = ev(MacKind::TabletPoint, MacSubtype::Other, 1.0, 1.0, f32::NAN, 0.0);
        e.tilt = [f32::INFINITY, 7.0];
        let p = pose_of(&e);
        assert_eq!(p.pressure, Some(0.0));
        assert!(p.tilt.unwrap().iter().all(|v| v.is_finite() && v.abs() <= std::f32::consts::FRAC_PI_2 + 1e-6));
        let e = ev(MacKind::TabletPoint, MacSubtype::Other, 1.0, 1.0, 5.0, 0.0);
        assert_eq!(pose_of(&e).pressure, Some(1.0));
    }

    #[test]
    fn plain_mouse_events_make_no_pen_events() {
        let mut m = MacMapper::new();
        for k in [MacKind::MouseDown, MacKind::MouseDragged, MacKind::MouseUp, MacKind::MouseMoved] {
            // A trackpad force-click reports pressure too, but its subtype is Mouse.
            assert!(m.map(&ev(k, MacSubtype::Mouse, 10.0, 10.0, 0.8, 1.0)).is_empty());
        }
        assert!(!m.in_range());
    }

    #[test]
    fn proximity_brings_pen_or_eraser_in_and_out() {
        let mut m = MacMapper::new();
        assert_eq!(m.map(&prox(true, MacDevice::Eraser, 1.0)), vec![PenEvent::Enter { eraser: true }]);
        assert_eq!(m.map(&prox(false, MacDevice::Eraser, 2.0)), vec![PenEvent::Leave]);
        assert_eq!(m.map(&prox(true, MacDevice::Pen, 3.0)), vec![PenEvent::Enter { eraser: false }]);
        // A puck is not a pen.
        let mut m = MacMapper::new();
        assert!(m.map(&prox(true, MacDevice::Cursor, 1.0)).is_empty());
        assert!(!m.in_range());
    }

    #[test]
    fn stroke_with_proximity_down_drag_up() {
        let mut m = MacMapper::new();
        let mut all = m.map(&prox(true, MacDevice::Pen, 0.0));
        all.extend(m.map(&ev(MacKind::MouseMoved, MacSubtype::TabletPoint, 5.0, 5.0, 0.0, 0.1)));
        all.extend(m.map(&ev(MacKind::MouseDown, MacSubtype::TabletPoint, 6.0, 6.0, 0.3, 0.2)));
        all.extend(m.map(&ev(MacKind::MouseDragged, MacSubtype::TabletPoint, 9.0, 6.0, 0.6, 0.3)));
        all.extend(m.map(&ev(MacKind::MouseUp, MacSubtype::TabletPoint, 9.0, 6.0, 0.0, 0.4)));
        let kinds: Vec<&str> = all
            .iter()
            .map(|e| match e {
                PenEvent::Enter { .. } => "enter",
                PenEvent::Pose(_) => "pose",
                PenEvent::Down => "down",
                PenEvent::Up => "up",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["enter", "pose", "pose", "down", "pose", "up"]);
    }

    #[test]
    fn tracker_gets_pressure_from_mapped_events() {
        let mut m = MacMapper::new();
        let mut events = Vec::new();
        events.extend(m.map(&ev(MacKind::MouseDown, MacSubtype::TabletPoint, 6.0, 6.0, 0.3, 0.2)));
        events.extend(m.map(&ev(MacKind::MouseDragged, MacSubtype::TabletPoint, 9.0, 6.0, 0.6, 0.3)));
        let mut tr = PenTracker::new();
        let f = tr.frame(events, 1.0);
        assert!(f.pressed && f.down && f.active);
        let pressures: Vec<f64> = f.samples.iter().filter_map(|s| s.reading.pressure).collect();
        assert_eq!(pressures.len(), 2);
        assert!((pressures[0] - 0.3).abs() < 1e-6 && (pressures[1] - 0.6).abs() < 1e-6);
        assert!(f.samples.iter().all(|s| s.reading.roll.is_some() && s.reading.altitude.is_some()));
        assert_eq!(tr.last_pressure(), Some(0.6));
    }

    #[test]
    fn missing_proximity_events_still_work_and_drag_without_down_draws() {
        let mut m = MacMapper::new();
        let out = m.map(&ev(MacKind::MouseDragged, MacSubtype::TabletPoint, 1.0, 1.0, 0.5, 0.0));
        assert!(matches!(out[0], PenEvent::Enter { eraser: false }));
        assert!(out.contains(&PenEvent::Down));
    }

    #[test]
    fn a_real_mouse_takes_over_only_after_a_gap() {
        let mut m = MacMapper::new();
        m.map(&ev(MacKind::TabletPoint, MacSubtype::Other, 1.0, 1.0, 0.0, 10.0));
        assert!(m.in_range());
        // Sidecar sends the plain flavour right after: ignored.
        assert!(m.map(&ev(MacKind::MouseMoved, MacSubtype::Mouse, 1.0, 1.0, 0.0, 10.05)).is_empty());
        assert!(m.in_range());
        // A mouse moving a second later: the pen is gone.
        assert_eq!(m.map(&ev(MacKind::MouseMoved, MacSubtype::Mouse, 3.0, 3.0, 0.0, 11.5)), vec![PenEvent::Leave]);
        assert!(!m.in_range());
    }

    #[test]
    fn pen_down_then_leave_releases() {
        let mut m = MacMapper::new();
        m.map(&ev(MacKind::MouseDown, MacSubtype::TabletPoint, 1.0, 1.0, 0.5, 0.0));
        let out = m.map(&prox(false, MacDevice::Pen, 0.1));
        assert_eq!(out, vec![PenEvent::Leave]);
        let mut tr = PenTracker::new();
        let mut events = m.map(&ev(MacKind::MouseDown, MacSubtype::TabletPoint, 1.0, 1.0, 0.5, 1.0));
        events.extend(out);
        let f = tr.frame(events, 1.0);
        assert!(f.released && !f.down);
    }
}
