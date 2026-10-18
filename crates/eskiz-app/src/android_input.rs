//! Android input (A2), the platform-independent half.
//!
//! Android's `NativeActivity` hands its input queue to native code, so Java's
//! `dispatchTouchEvent` never fires and winit only reports position and a pressure number for
//! every tool type. The Java shim (`EskizActivity`) therefore takes the queue back, forwards
//! every `MotionEvent` and `KeyEvent` over JNI (`android_jni.rs`), and this module turns that
//! data into what the rest of the app already understands:
//!
//! * stylus and eraser contacts become [`PenEvent`]s (pressure, tilt, historical samples), so
//!   they run the same pipeline as a desktop tablet;
//! * fingers become egui touch events plus the emulated primary pointer, exactly the way
//!   egui-winit does it, with no force (so no pressure taper) and the multi-touch gestures
//!   (pinch, rotate, two-finger tap undo) keep working from the same `Touch` events;
//! * a mouse becomes egui pointer, button and wheel events.
//!
//! Policy: while a stylus is in contact, finger
//! contacts that begin are ignored, and fingers already down are cancelled (palm rejection by
//! policy, on top of what Android cancels itself). `ACTION_CANCEL` and `FLAG_CANCELED` cancel
//! the contact: a stylus gets [`PenEvent::Cancel`], a finger gets a touch `Cancel`.
//!
//! Everything here is plain data in, plain data out, so it is tested on the host with
//! structs that mirror the `MotionEvent` fields. Positions are Android pixels; the caller
//! scales them.

use std::collections::HashMap;

use eframe::egui::{Key, Modifiers};

use crate::tablet::{PenEvent, PenPose};

/// `MotionEvent.FLAG_CANCELED`: the touch was cancelled (palm), the event is not a real lift.
pub const FLAG_CANCELED: i32 = 0x20;
/// The pen is treated as out of range after this long without any stylus event.
const HOVER_IDLE_SECS: f64 = 3.0;
/// Tilt is kept below 90 degrees so `tan` stays finite.
const MAX_TILT: f32 = 1.553_343; // 89 degrees

/// `MotionEvent.TOOL_TYPE_*`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolType {
    Unknown,
    Finger,
    Stylus,
    Mouse,
    Eraser,
}

impl ToolType {
    pub fn from_raw(v: i32) -> Self {
        match v {
            1 => Self::Finger,
            2 => Self::Stylus,
            3 => Self::Mouse,
            4 => Self::Eraser,
            _ => Self::Unknown,
        }
    }

    fn is_pen(self) -> bool {
        matches!(self, Self::Stylus | Self::Eraser)
    }
}

/// `MotionEvent.getActionMasked()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    Move,
    Cancel,
    PointerDown,
    PointerUp,
    HoverMove,
    Scroll,
    HoverEnter,
    HoverExit,
    Other,
}

impl Action {
    pub fn from_masked(v: i32) -> Self {
        match v {
            0 => Self::Down,
            1 => Self::Up,
            2 => Self::Move,
            3 => Self::Cancel,
            5 => Self::PointerDown,
            6 => Self::PointerUp,
            7 => Self::HoverMove,
            8 => Self::Scroll,
            9 => Self::HoverEnter,
            10 => Self::HoverExit,
            _ => Self::Other,
        }
    }
}

/// One pointer in one sample of a `MotionEvent`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerSample {
    /// `getPointerId`, stable from down to up.
    pub id: i32,
    pub tool: ToolType,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    /// `AXIS_TILT`: radians from perpendicular, 0 = upright, pi/2 = flat on the screen.
    pub tilt: f32,
    /// `AXIS_ORIENTATION`: radians, 0 = leaning toward the top of the screen, pi/2 = right.
    pub orientation: f32,
}

/// All pointers at one instant (a historical sample, or the event's current state).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// `getHistoricalEventTime` / `getEventTime`, milliseconds on the uptime clock.
    pub time_ms: i64,
    pub pointers: Vec<PointerSample>,
}

/// The `MotionEvent` fields Eskiz uses.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionEvent {
    pub action: Action,
    /// `getActionIndex` (which pointer went down or up for `PointerDown` / `PointerUp`).
    pub action_index: usize,
    pub flags: i32,
    /// `getButtonState`.
    pub buttons: i32,
    /// Older samples batched into this event, oldest first.
    pub history: Vec<Frame>,
    /// The newest sample; its time is the event time.
    pub current: Frame,
    pub hscroll: f32,
    pub vscroll: f32,
}

/// The `KeyEvent` fields Eskiz uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub pressed: bool,
    pub repeat: bool,
    pub key_code: i32,
    /// `getUnicodeChar`, 0 when the key produces no character.
    pub unicode: u32,
    /// `getMetaState`.
    pub meta: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Start,
    Move,
    End,
    Cancel,
}

/// What egui should be told, positions still in Android pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum UiOut {
    Touch { id: u64, phase: TouchPhase, pos: [f32; 2] },
    PointerMoved([f32; 2]),
    PointerButton { pos: [f32; 2], pressed: bool },
    PointerGone,
    /// Mouse wheel in lines, positive y = wheel forward (up).
    Wheel([f32; 2]),
    Key { key: Key, pressed: bool, repeat: bool, modifiers: Modifiers },
    Text(String),
}

/// The result of translating events, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub pen: Vec<PenEvent>,
    pub ui: Vec<UiOut>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    Finger,
    Stylus,
    Mouse,
    /// A contact that began while a stylus was down (palm), or one that was cancelled.
    Ignored,
}

/// Android tilt and orientation to W3C `tiltX` / `tiltY` in radians, which is what
/// [`PenPose::tilt`] carries. Orientation 0 means the pen leans toward the top of the screen
/// (tiltY negative), pi/2 toward the right (tiltX positive).
pub fn tilt_xy(tilt: f32, orientation: f32) -> Option<[f32; 2]> {
    if !tilt.is_finite() || !orientation.is_finite() {
        return None;
    }
    let t = tilt.clamp(0.0, MAX_TILT).tan();
    Some([(orientation.sin() * t).atan(), (-orientation.cos() * t).atan()])
}

/// Folds `MotionEvent`s into [`Output`], tracking pointer ids from down to up.
#[derive(Debug, Default)]
pub struct Translator {
    roles: HashMap<i32, Role>,
    last_pos: HashMap<i32, [f32; 2]>,
    /// The stylus pointer that is touching the surface.
    pen_contact: Option<i32>,
    pen_in_range: bool,
    pen_eraser: bool,
    pen_last_activity: f64,
    /// The finger that drives egui's emulated mouse pointer (the first one down).
    pointer_finger: Option<i32>,
    mouse_down: bool,
}

impl Translator {
    pub fn new() -> Self {
        Self::default()
    }

    /// A stylus is touching the surface.
    pub fn pen_down(&self) -> bool {
        self.pen_contact.is_some()
    }

    fn pose(p: &PointerSample, time_ms: i64, pressure: Option<f32>) -> PenEvent {
        PenEvent::Pose(PenPose {
            pos: [p.x, p.y],
            pressure,
            tilt: tilt_xy(p.tilt, p.orientation),
            roll: None,
            time: Some(time_ms as f64 / 1000.0),
        })
    }

    fn pen_enter(&mut self, eraser: bool, out: &mut Output) {
        if !self.pen_in_range || self.pen_eraser != eraser {
            if self.pen_in_range {
                out.pen.push(PenEvent::Leave);
            }
            out.pen.push(PenEvent::Enter { eraser });
            self.pen_in_range = true;
            self.pen_eraser = eraser;
        }
    }

    fn pen_leave(&mut self, out: &mut Output) {
        if self.pen_in_range {
            out.pen.push(PenEvent::Leave);
            self.pen_in_range = false;
            self.pen_eraser = false;
        }
    }

    /// Nothing happens for a while: a pen that never sent a hover exit goes out of range.
    pub fn tick(&mut self, now: f64, out: &mut Output) {
        if self.pen_in_range && self.pen_contact.is_none() && now - self.pen_last_activity > HOVER_IDLE_SECS {
            self.pen_leave(out);
        }
    }

    fn cancel_finger(&mut self, id: i32, out: &mut Output) {
        let pos = self.last_pos.get(&id).copied().unwrap_or([0.0, 0.0]);
        out.ui.push(UiOut::Touch { id: id as u64, phase: TouchPhase::Cancel, pos });
        if self.pointer_finger == Some(id) {
            self.pointer_finger = None;
            out.ui.push(UiOut::PointerButton { pos, pressed: false });
            out.ui.push(UiOut::PointerGone);
        }
    }

    fn cancel_all_fingers(&mut self, out: &mut Output) {
        let mut ids: Vec<i32> = self.roles.iter().filter(|(_, r)| **r == Role::Finger).map(|(i, _)| *i).collect();
        ids.sort_unstable();
        for id in ids {
            self.cancel_finger(id, out);
            self.roles.insert(id, Role::Ignored);
        }
    }

    fn cancel_pen_contact(&mut self, out: &mut Output) {
        if self.pen_contact.take().is_some() {
            out.pen.push(PenEvent::Cancel);
        }
    }

    fn down(&mut self, p: &PointerSample, time_ms: i64, now: f64, out: &mut Output) {
        let pos = [p.x, p.y];
        self.last_pos.insert(p.id, pos);
        match p.tool {
            t if t.is_pen() => {
                if self.pen_contact.is_some() {
                    self.roles.insert(p.id, Role::Ignored);
                    return;
                }
                self.cancel_all_fingers(out);
                self.roles.insert(p.id, Role::Stylus);
                self.pen_contact = Some(p.id);
                self.pen_last_activity = now;
                self.pen_enter(t == ToolType::Eraser, out);
                out.pen.push(Self::pose(p, time_ms, Some(p.pressure)));
                out.pen.push(PenEvent::Down);
            }
            ToolType::Mouse => {
                self.roles.insert(p.id, Role::Mouse);
                self.mouse_down = true;
                out.ui.push(UiOut::PointerMoved(pos));
                out.ui.push(UiOut::PointerButton { pos, pressed: true });
            }
            _ => {
                if self.pen_contact.is_some() {
                    self.roles.insert(p.id, Role::Ignored);
                    return;
                }
                self.roles.insert(p.id, Role::Finger);
                out.ui.push(UiOut::Touch { id: p.id as u64, phase: TouchPhase::Start, pos });
                if self.pointer_finger.is_none() {
                    self.pointer_finger = Some(p.id);
                    out.ui.push(UiOut::PointerMoved(pos));
                    out.ui.push(UiOut::PointerButton { pos, pressed: true });
                }
            }
        }
    }

    fn up(&mut self, p: &PointerSample, time_ms: i64, cancelled: bool, out: &mut Output) {
        let role = self.roles.remove(&p.id);
        let pos = [p.x, p.y];
        self.last_pos.remove(&p.id);
        match role {
            Some(Role::Stylus) => {
                if self.pen_contact == Some(p.id) {
                    if cancelled {
                        self.cancel_pen_contact(out);
                    } else {
                        self.pen_contact = None;
                        out.pen.push(Self::pose(p, time_ms, Some(p.pressure)));
                        out.pen.push(PenEvent::Up);
                    }
                }
            }
            Some(Role::Finger) => {
                if cancelled {
                    self.last_pos.insert(p.id, pos);
                    self.cancel_finger(p.id, out);
                    self.last_pos.remove(&p.id);
                } else {
                    out.ui.push(UiOut::Touch { id: p.id as u64, phase: TouchPhase::End, pos });
                    if self.pointer_finger == Some(p.id) {
                        self.pointer_finger = None;
                        out.ui.push(UiOut::PointerButton { pos, pressed: false });
                        out.ui.push(UiOut::PointerGone);
                    }
                }
            }
            Some(Role::Mouse) => {
                self.mouse_down = false;
                out.ui.push(UiOut::PointerButton { pos, pressed: false });
            }
            Some(Role::Ignored) | None => {}
        }
    }

    /// Translate one event. `now` is any monotonic clock in seconds, used only to time out
    /// a hovering pen.
    pub fn process(&mut self, ev: &MotionEvent, now: f64, out: &mut Output) {
        let cancelled = ev.flags & FLAG_CANCELED != 0;
        let time = ev.current.time_ms;
        match ev.action {
            Action::Down | Action::PointerDown => {
                let idx = if ev.action == Action::Down { 0 } else { ev.action_index };
                if let Some(p) = ev.current.pointers.get(idx) {
                    self.down(p, time, now, out);
                }
            }
            Action::Move => self.moves(ev, now, out),
            Action::Up | Action::PointerUp => {
                let idx = if ev.action == Action::Up { 0 } else { ev.action_index };
                // The last position may have moved since the previous MOVE.
                if let Some(p) = ev.current.pointers.get(idx).copied() {
                    self.up(&p, time, cancelled, out);
                }
            }
            Action::Cancel => {
                self.cancel_pen_contact(out);
                let mut ids: Vec<(i32, Role)> = self.roles.iter().map(|(i, r)| (*i, *r)).collect();
                ids.sort_by_key(|(i, _)| *i);
                for (id, role) in ids {
                    match role {
                        Role::Finger => self.cancel_finger(id, out),
                        Role::Mouse => {
                            let pos = self.last_pos.get(&id).copied().unwrap_or([0.0, 0.0]);
                            out.ui.push(UiOut::PointerButton { pos, pressed: false });
                        }
                        Role::Stylus | Role::Ignored => {}
                    }
                }
                self.roles.clear();
                self.last_pos.clear();
                self.pointer_finger = None;
                self.mouse_down = false;
            }
            Action::HoverEnter | Action::HoverMove => {
                for p in &ev.current.pointers {
                    if p.tool.is_pen() {
                        if self.pen_contact.is_some() {
                            continue;
                        }
                        self.pen_last_activity = now;
                        self.pen_enter(p.tool == ToolType::Eraser, out);
                        for f in &ev.history {
                            if let Some(h) = f.pointers.iter().find(|h| h.id == p.id) {
                                out.pen.push(Self::pose(h, f.time_ms, Some(0.0)));
                            }
                        }
                        out.pen.push(Self::pose(p, time, Some(0.0)));
                    } else if p.tool == ToolType::Mouse {
                        out.ui.push(UiOut::PointerMoved([p.x, p.y]));
                    }
                }
            }
            Action::HoverExit => {
                if ev.current.pointers.iter().any(|p| p.tool.is_pen()) {
                    if self.pen_contact.is_none() {
                        self.pen_leave(out);
                    }
                } else if ev.current.pointers.iter().any(|p| p.tool == ToolType::Mouse) {
                    out.ui.push(UiOut::PointerGone);
                }
            }
            Action::Scroll => {
                if let Some(p) = ev.current.pointers.first() {
                    out.ui.push(UiOut::PointerMoved([p.x, p.y]));
                }
                if ev.hscroll != 0.0 || ev.vscroll != 0.0 {
                    out.ui.push(UiOut::Wheel([ev.hscroll, ev.vscroll]));
                }
            }
            Action::Other => {}
        }
    }

    fn moves(&mut self, ev: &MotionEvent, now: f64, out: &mut Output) {
        for p in &ev.current.pointers {
            match self.roles.get(&p.id).copied() {
                Some(Role::Stylus) if self.pen_contact == Some(p.id) => {
                    self.pen_last_activity = now;
                    // Batched samples first, oldest to newest, then the event's own sample.
                    for f in &ev.history {
                        if let Some(h) = f.pointers.iter().find(|h| h.id == p.id) {
                            out.pen.push(Self::pose(h, f.time_ms, Some(h.pressure)));
                        }
                    }
                    out.pen.push(Self::pose(p, ev.current.time_ms, Some(p.pressure)));
                    self.last_pos.insert(p.id, [p.x, p.y]);
                }
                Some(Role::Finger) => {
                    let pos = [p.x, p.y];
                    if self.last_pos.get(&p.id) != Some(&pos) {
                        self.last_pos.insert(p.id, pos);
                        out.ui.push(UiOut::Touch { id: p.id as u64, phase: TouchPhase::Move, pos });
                        if self.pointer_finger == Some(p.id) {
                            out.ui.push(UiOut::PointerMoved(pos));
                        }
                    }
                }
                Some(Role::Mouse) if self.mouse_down => {
                    out.ui.push(UiOut::PointerMoved([p.x, p.y]));
                    self.last_pos.insert(p.id, [p.x, p.y]);
                }
                _ => {}
            }
        }
    }

    /// Translate a key event: the key itself, and the character it types.
    pub fn key(&mut self, k: KeyInput, out: &mut Output) {
        let modifiers = modifiers_from_meta(k.meta);
        if let Some(key) = key_from_android(k.key_code) {
            out.ui.push(UiOut::Key { key, pressed: k.pressed, repeat: k.repeat, modifiers });
        }
        if k.pressed && !modifiers.ctrl && !modifiers.alt && !modifiers.command {
            if let Some(c) = char::from_u32(k.unicode).filter(|c| !c.is_control()) {
                out.ui.push(UiOut::Text(c.to_string()));
            }
        }
    }
}

/// `KeyEvent.META_*` to egui modifiers.
pub fn modifiers_from_meta(meta: i32) -> Modifiers {
    let ctrl = meta & 0x1000 != 0;
    Modifiers { alt: meta & 0x2 != 0, ctrl, shift: meta & 0x1 != 0, mac_cmd: false, command: ctrl }
}

/// `KeyEvent.KEYCODE_*` to an egui key, for the keys the app has shortcuts or text fields for.
pub fn key_from_android(code: i32) -> Option<Key> {
    const LETTERS: [Key; 26] = [
        Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H, Key::I, Key::J, Key::K, Key::L, Key::M,
        Key::N, Key::O, Key::P, Key::Q, Key::R, Key::S, Key::T, Key::U, Key::V, Key::W, Key::X, Key::Y, Key::Z,
    ];
    const DIGITS: [Key; 10] = [
        Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
    ];
    Some(match code {
        29..=54 => LETTERS[(code - 29) as usize],
        7..=16 => DIGITS[(code - 7) as usize],
        66 | 160 => Key::Enter,
        67 => Key::Backspace,
        112 => Key::Delete,
        61 => Key::Tab,
        62 => Key::Space,
        111 => Key::Escape,
        19 => Key::ArrowUp,
        20 => Key::ArrowDown,
        21 => Key::ArrowLeft,
        22 => Key::ArrowRight,
        122 => Key::Home,
        123 => Key::End,
        92 => Key::PageUp,
        93 => Key::PageDown,
        69 => Key::Minus,
        70 => Key::Equals,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{sample_from, sample_from_pen, PointerKind, RawPointer};
    use crate::tablet::{reading_from_pose, PenTracker};
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    fn ptr(id: i32, tool: ToolType, x: f32, y: f32) -> PointerSample {
        PointerSample { id, tool, x, y, pressure: 0.5, tilt: 0.0, orientation: 0.0 }
    }

    fn ev(action: Action, idx: usize, t: i64, pointers: Vec<PointerSample>) -> MotionEvent {
        MotionEvent {
            action,
            action_index: idx,
            flags: 0,
            buttons: 0,
            history: vec![],
            current: Frame { time_ms: t, pointers },
            hscroll: 0.0,
            vscroll: 0.0,
        }
    }

    fn run(tr: &mut Translator, events: &[MotionEvent]) -> Output {
        let mut out = Output::default();
        for (i, e) in events.iter().enumerate() {
            tr.process(e, i as f64 * 0.01, &mut out);
        }
        out
    }

    fn poses(out: &Output) -> Vec<PenPose> {
        out.pen.iter().filter_map(|e| if let PenEvent::Pose(p) = e { Some(*p) } else { None }).collect()
    }

    #[test]
    fn finger_goes_to_egui_without_force_and_stylus_to_the_pen() {
        let mut tr = Translator::new();
        let finger = run(&mut tr, &[ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Finger, 10.0, 20.0)])]);
        assert!(finger.pen.is_empty());
        assert_eq!(
            finger.ui,
            vec![
                UiOut::Touch { id: 0, phase: TouchPhase::Start, pos: [10.0, 20.0] },
                UiOut::PointerMoved([10.0, 20.0]),
                UiOut::PointerButton { pos: [10.0, 20.0], pressed: true },
            ]
        );
        let mut tr = Translator::new();
        let pen = run(&mut tr, &[ev(Action::Down, 0, 0, vec![ptr(3, ToolType::Stylus, 10.0, 20.0)])]);
        assert!(pen.ui.is_empty(), "the stylus never reaches egui as a touch");
        assert_eq!(pen.pen.len(), 3);
        assert_eq!(pen.pen[0], PenEvent::Enter { eraser: false });
        assert_eq!(pen.pen[2], PenEvent::Down);
    }

    #[test]
    fn finger_has_no_pressure_taper_and_stylus_has() {
        // Finger: the app reads Touch with force None -> sample_from gives has_pressure false.
        let s = sample_from(RawPointer { x: 1.0, y: 1.0, time: 0.0, kind: PointerKind::Touch, force: None });
        assert_eq!(s.has_pressure, Some(false));
        // Stylus: pressure reaches the sample through the pen tracker.
        let mut tr = Translator::new();
        let mut p = ptr(0, ToolType::Stylus, 5.0, 5.0);
        p.pressure = 0.7;
        let out = run(&mut tr, &[ev(Action::Down, 0, 100, vec![p])]);
        let mut tracker = PenTracker::new();
        let f = tracker.frame(out.pen, 1.0);
        assert!(f.pressed && f.down);
        let sample = sample_from_pen(5.0, 5.0, f.samples[0].time, f.samples[0].reading);
        assert_eq!(sample.has_pressure, Some(true));
        assert!((sample.pressure.unwrap() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn tilt_and_orientation_become_w3c_tilt() {
        // Upright pen: no tilt.
        let t = tilt_xy(0.0, 0.0).unwrap();
        assert!(t[0].abs() < 1e-6 && t[1].abs() < 1e-6);
        // Leaning toward the top of the screen: tiltY negative, tiltX zero.
        let t = tilt_xy(FRAC_PI_4, 0.0).unwrap();
        assert!(t[0].abs() < 1e-6);
        assert!((t[1] + FRAC_PI_4).abs() < 1e-5);
        // Leaning right: tiltX positive.
        let t = tilt_xy(FRAC_PI_4, FRAC_PI_2).unwrap();
        assert!((t[0] - FRAC_PI_4).abs() < 1e-5 && t[1].abs() < 1e-5);
        // Leaning toward the bottom (orientation pi): tiltY positive.
        let t = tilt_xy(FRAC_PI_4, std::f32::consts::PI).unwrap();
        assert!((t[1] - FRAC_PI_4).abs() < 1e-5);
        // Flat on the screen stays finite, junk gives none.
        assert!(tilt_xy(FRAC_PI_2, 1.0).unwrap().iter().all(|v| v.is_finite()));
        assert!(tilt_xy(f32::NAN, 0.0).is_none());
        // And it reaches the engine's altitude and azimuth: a 45 degree tilt is altitude 45.
        let mut p = ptr(0, ToolType::Stylus, 0.0, 0.0);
        p.tilt = FRAC_PI_4;
        p.orientation = FRAC_PI_2;
        let out = run(&mut Translator::new(), &[ev(Action::Down, 0, 0, vec![p])]);
        let reading = reading_from_pose(&poses(&out)[0]);
        assert!((reading.altitude.unwrap().to_degrees() - 45.0).abs() < 0.1);
        // A perpendicular pen has altitude 90.
        let out = run(&mut Translator::new(), &[ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Stylus, 0.0, 0.0)])]);
        assert!((reading_from_pose(&poses(&out)[0]).altitude.unwrap().to_degrees() - 90.0).abs() < 0.1);
    }

    #[test]
    fn historical_samples_are_kept_in_order_before_the_current_one() {
        let mut tr = Translator::new();
        let mut m = ev(Action::Move, 0, 130, vec![ptr(0, ToolType::Stylus, 4.0, 0.0)]);
        m.history = vec![
            Frame { time_ms: 110, pointers: vec![ptr(0, ToolType::Stylus, 1.0, 0.0)] },
            Frame { time_ms: 120, pointers: vec![ptr(0, ToolType::Stylus, 2.0, 0.0)] },
            Frame { time_ms: 125, pointers: vec![ptr(0, ToolType::Stylus, 3.0, 0.0)] },
        ];
        let out = run(&mut tr, &[ev(Action::Down, 0, 100, vec![ptr(0, ToolType::Stylus, 0.0, 0.0)]), m]);
        let xs: Vec<f32> = poses(&out).iter().map(|p| p.pos[0]).collect();
        assert_eq!(xs, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        let ts: Vec<f64> = poses(&out).iter().map(|p| p.time.unwrap()).collect();
        assert!(ts.windows(2).all(|w| w[0] < w[1]), "times strictly increase: {ts:?}");
        assert_eq!(ts[4], 0.130);
        // The tracker hands them to the stroke pipeline in the same order.
        let f = PenTracker::new().frame(out.pen, 1.0);
        let sx: Vec<f32> = f.samples.iter().map(|s| s.pos.x).collect();
        assert_eq!(sx, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn stylus_lifecycle_down_move_up() {
        let mut tr = Translator::new();
        let out = run(
            &mut tr,
            &[
                ev(Action::Down, 0, 0, vec![ptr(2, ToolType::Stylus, 1.0, 1.0)]),
                ev(Action::Move, 0, 10, vec![ptr(2, ToolType::Stylus, 2.0, 1.0)]),
                ev(Action::Up, 0, 20, vec![ptr(2, ToolType::Stylus, 3.0, 1.0)]),
            ],
        );
        let mut tracker = PenTracker::new();
        let f = tracker.frame(out.pen, 1.0);
        assert!(f.pressed && f.released && !f.down);
        assert_eq!(f.samples.len(), 3);
        assert!(!tr.pen_down());
    }

    #[test]
    fn eraser_tool_type_selects_the_eraser_end() {
        let out = run(&mut Translator::new(), &[ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Eraser, 1.0, 1.0)])]);
        assert_eq!(out.pen[0], PenEvent::Enter { eraser: true });
        let f = PenTracker::new().frame(out.pen, 1.0);
        assert!(f.eraser && f.down);
    }

    #[test]
    fn hover_enters_and_exit_leaves() {
        let mut tr = Translator::new();
        let out = run(
            &mut tr,
            &[
                ev(Action::HoverEnter, 0, 0, vec![ptr(0, ToolType::Stylus, 1.0, 1.0)]),
                ev(Action::HoverMove, 0, 5, vec![ptr(0, ToolType::Stylus, 2.0, 1.0)]),
            ],
        );
        let mut tracker = PenTracker::new();
        let f = tracker.frame(out.pen, 1.0);
        assert!(f.active && f.moved && !f.down && f.samples.is_empty());
        let out = run(&mut tr, &[ev(Action::HoverExit, 0, 9, vec![ptr(0, ToolType::Stylus, 2.0, 1.0)])]);
        assert_eq!(out.pen, vec![PenEvent::Leave]);
        // A pen that never says goodbye goes out of range by itself.
        let mut tr = Translator::new();
        run(&mut tr, &[ev(Action::HoverMove, 0, 0, vec![ptr(0, ToolType::Stylus, 1.0, 1.0)])]);
        let mut out = Output::default();
        tr.tick(1.0, &mut out);
        assert!(out.pen.is_empty());
        tr.tick(10.0, &mut out);
        assert_eq!(out.pen, vec![PenEvent::Leave]);
    }

    #[test]
    fn action_cancel_cancels_the_stylus_contact() {
        let mut tr = Translator::new();
        let out = run(
            &mut tr,
            &[
                ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Stylus, 1.0, 1.0)]),
                ev(Action::Move, 0, 10, vec![ptr(0, ToolType::Stylus, 5.0, 1.0)]),
                ev(Action::Cancel, 0, 20, vec![ptr(0, ToolType::Stylus, 5.0, 1.0)]),
            ],
        );
        assert_eq!(out.pen.last(), Some(&PenEvent::Cancel));
        assert!(!out.pen.contains(&PenEvent::Up));
        let f = PenTracker::new().frame(out.pen, 1.0);
        assert!(f.cancelled && !f.released && !f.down);
        assert!(!tr.pen_down());
    }

    #[test]
    fn flag_canceled_on_up_cancels_instead_of_lifting() {
        let mut tr = Translator::new();
        let mut up = ev(Action::Up, 0, 20, vec![ptr(0, ToolType::Stylus, 5.0, 1.0)]);
        up.flags = FLAG_CANCELED;
        let out = run(&mut tr, &[ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Stylus, 1.0, 1.0)]), up]);
        assert_eq!(out.pen.last(), Some(&PenEvent::Cancel));
        assert!(!out.pen.contains(&PenEvent::Up));
        // A finger with the flag gets a touch cancel and the emulated pointer lets go.
        let mut tr = Translator::new();
        let mut up = ev(Action::Up, 0, 20, vec![ptr(0, ToolType::Finger, 5.0, 1.0)]);
        up.flags = FLAG_CANCELED;
        let out = run(&mut tr, &[ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Finger, 1.0, 1.0)]), up]);
        assert!(out.ui.iter().any(|u| matches!(u, UiOut::Touch { phase: TouchPhase::Cancel, .. })));
        assert!(!out.ui.iter().any(|u| matches!(u, UiOut::Touch { phase: TouchPhase::End, .. })));
        assert!(out.ui.contains(&UiOut::PointerButton { pos: [5.0, 1.0], pressed: false }));
    }

    #[test]
    fn pointer_ids_are_tracked_through_a_two_finger_gesture() {
        let mut tr = Translator::new();
        let a = ptr(4, ToolType::Finger, 10.0, 10.0);
        let b = ptr(7, ToolType::Finger, 50.0, 10.0);
        let out = run(
            &mut tr,
            &[
                ev(Action::Down, 0, 0, vec![a]),
                ev(Action::PointerDown, 1, 5, vec![a, b]),
                ev(Action::Move, 0, 10, vec![ptr(4, ToolType::Finger, 12.0, 10.0), ptr(7, ToolType::Finger, 60.0, 10.0)]),
                ev(Action::PointerUp, 0, 15, vec![ptr(4, ToolType::Finger, 12.0, 10.0), ptr(7, ToolType::Finger, 60.0, 10.0)]),
                ev(Action::Up, 0, 20, vec![ptr(7, ToolType::Finger, 61.0, 10.0)]),
            ],
        );
        let touches: Vec<(u64, TouchPhase)> = out
            .ui
            .iter()
            .filter_map(|u| if let UiOut::Touch { id, phase, .. } = u { Some((*id, *phase)) } else { None })
            .collect();
        assert_eq!(
            touches,
            vec![
                (4, TouchPhase::Start),
                (7, TouchPhase::Start),
                (4, TouchPhase::Move),
                (7, TouchPhase::Move),
                (4, TouchPhase::End),
                (7, TouchPhase::End),
            ]
        );
        // Only the first finger drives the emulated pointer.
        let moves = out.ui.iter().filter(|u| matches!(u, UiOut::PointerMoved(_))).count();
        assert_eq!(moves, 2, "start and one move of finger 4");
        assert!(tr.roles.is_empty());
    }

    #[test]
    fn stylus_with_a_resting_palm_and_a_finger_before_the_stylus() {
        // Palm (finger) first, then the stylus: the palm is cancelled, the stylus draws.
        let mut tr = Translator::new();
        let out = run(
            &mut tr,
            &[
                ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Finger, 100.0, 100.0)]),
                ev(Action::PointerDown, 1, 5, vec![ptr(0, ToolType::Finger, 100.0, 100.0), ptr(1, ToolType::Stylus, 5.0, 5.0)]),
            ],
        );
        assert!(out.ui.iter().any(|u| matches!(u, UiOut::Touch { id: 0, phase: TouchPhase::Cancel, .. })));
        assert!(out.pen.contains(&PenEvent::Down));
        // The cancelled palm's later movement and lift do nothing.
        let more = run(
            &mut tr,
            &[
                ev(Action::Move, 0, 10, vec![ptr(0, ToolType::Finger, 110.0, 100.0), ptr(1, ToolType::Stylus, 6.0, 5.0)]),
                ev(Action::PointerUp, 0, 15, vec![ptr(0, ToolType::Finger, 110.0, 100.0), ptr(1, ToolType::Stylus, 6.0, 5.0)]),
            ],
        );
        assert!(more.ui.is_empty());
        assert_eq!(poses(&more).len(), 1);
        // Stylus first, then a finger: the finger is ignored for its whole life.
        let mut tr = Translator::new();
        let out = run(
            &mut tr,
            &[
                ev(Action::Down, 0, 0, vec![ptr(1, ToolType::Stylus, 5.0, 5.0)]),
                ev(Action::PointerDown, 1, 5, vec![ptr(1, ToolType::Stylus, 5.0, 5.0), ptr(2, ToolType::Finger, 90.0, 90.0)]),
                ev(Action::Move, 0, 10, vec![ptr(1, ToolType::Stylus, 6.0, 5.0), ptr(2, ToolType::Finger, 95.0, 90.0)]),
                ev(Action::PointerUp, 1, 15, vec![ptr(1, ToolType::Stylus, 6.0, 5.0), ptr(2, ToolType::Finger, 95.0, 90.0)]),
                ev(Action::Up, 0, 20, vec![ptr(1, ToolType::Stylus, 7.0, 5.0)]),
            ],
        );
        assert!(out.ui.is_empty(), "no finger event leaks through: {:?}", out.ui);
        assert_eq!(out.pen.last(), Some(&PenEvent::Up));
    }

    #[test]
    fn finger_then_stylus_after_the_finger_lifted_both_work() {
        let mut tr = Translator::new();
        let out = run(
            &mut tr,
            &[
                ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Finger, 1.0, 1.0)]),
                ev(Action::Up, 0, 5, vec![ptr(0, ToolType::Finger, 1.0, 1.0)]),
                ev(Action::Down, 0, 10, vec![ptr(0, ToolType::Stylus, 2.0, 2.0)]),
                ev(Action::Up, 0, 15, vec![ptr(0, ToolType::Stylus, 2.0, 2.0)]),
            ],
        );
        assert_eq!(out.ui.iter().filter(|u| matches!(u, UiOut::Touch { .. })).count(), 2);
        assert!(out.pen.contains(&PenEvent::Up));
    }

    #[test]
    fn mouse_and_wheel_become_pointer_events() {
        let mut tr = Translator::new();
        let mut scroll = ev(Action::Scroll, 0, 5, vec![ptr(0, ToolType::Mouse, 3.0, 4.0)]);
        scroll.vscroll = 1.0;
        let out = run(
            &mut tr,
            &[
                ev(Action::Down, 0, 0, vec![ptr(0, ToolType::Mouse, 1.0, 1.0)]),
                ev(Action::Move, 0, 2, vec![ptr(0, ToolType::Mouse, 2.0, 1.0)]),
                ev(Action::Up, 0, 4, vec![ptr(0, ToolType::Mouse, 2.0, 1.0)]),
                scroll,
            ],
        );
        assert!(out.pen.is_empty());
        assert!(!out.ui.iter().any(|u| matches!(u, UiOut::Touch { .. })));
        assert!(out.ui.contains(&UiOut::PointerButton { pos: [2.0, 1.0], pressed: false }));
        assert_eq!(out.ui.last(), Some(&UiOut::Wheel([0.0, 1.0])));
    }

    #[test]
    fn keys_map_to_egui_keys_and_text() {
        let mut out = Output::default();
        let mut tr = Translator::new();
        tr.key(KeyInput { pressed: true, repeat: false, key_code: 29, unicode: 'a' as u32, meta: 0 }, &mut out);
        assert!(matches!(out.ui[0], UiOut::Key { key: Key::A, pressed: true, .. }));
        assert_eq!(out.ui[1], UiOut::Text("a".into()));
        let mut out = Output::default();
        tr.key(KeyInput { pressed: true, repeat: false, key_code: 54, unicode: 'z' as u32, meta: 0x1000 }, &mut out);
        assert_eq!(out.ui.len(), 1, "ctrl+z is a shortcut, not text");
        assert!(matches!(&out.ui[0], UiOut::Key { key: Key::Z, modifiers, .. } if modifiers.ctrl && modifiers.command));
        assert_eq!(key_from_android(4), None, "Back is left to the system");
    }
}
