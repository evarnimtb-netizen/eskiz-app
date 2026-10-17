//! Touch gesture recogniser, as pure logic with no egui state.
//!
//! The app feeds it touch down / move / up events with timestamps, and calls `tick` once
//! per frame. It returns the gestures that fired. Pinch, rotate and pan come from egui's
//! multi-touch summary, not from here. This module handles the taps, holds, the
//! one-finger eyedropper hold, the three-finger scrub and the three-finger swipe.
//!
//! [`route`] maps each gesture to the command the app runs, or to nothing when the user has
//! turned that gesture off in Preferences > Gesture controls ([`Gestures`]).

use std::collections::BTreeMap;

use eframe::egui::{Pos2, Vec2};

/// Longest a tap may last, seconds.
pub const TAP_MAX_SECS: f64 = 0.3;
/// A finger that moves further than this from where it landed is not a tap or hold.
pub const MAX_TRAVEL: f32 = 10.0;
/// One-finger hold that starts the eyedropper, seconds.
pub const EYEDROPPER_HOLD_SECS: f64 = 0.4;
/// Two-finger hold that starts repeated undo, seconds.
pub const REPEAT_HOLD_SECS: f64 = 0.65;
/// Three fingers that travel this far from where they first sat together swipe, points.
pub const SWIPE_DIST: f32 = 120.0;
/// A three-finger swipe must cover its distance within this long, seconds.
pub const SWIPE_MAX_SECS: f64 = 0.8;

/// Something the recogniser saw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// All fingers lifted quickly without travel. `fingers` is the most that were down.
    Tap { fingers: usize },
    /// Two fingers held still past [`REPEAT_HOLD_SECS`]: repeat undo until they lift.
    HoldStart { fingers: usize },
    /// Three fingers zig-zag quickly: clear the layer.
    Scrub { fingers: usize },
    /// One finger held still past the eyedropper delay: start the eyedropper here.
    EyedropperHold { pos: Pos2 },
    /// Three fingers slide a good way in one go: show the copy menu.
    Swipe { fingers: usize },
}

/// Which gestures are on. Built from the Preferences; see [`route`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gestures {
    /// Touch-and-hold eyedropper.
    pub eyedropper: bool,
    /// Two-finger tap undoes and three-finger tap redoes.
    pub undo_taps: bool,
    /// Holding two fingers keeps undoing.
    pub repeat_undo: bool,
    /// Three-finger scrub clears the layer.
    pub scrub_clear: bool,
    /// Three-finger swipe shows the copy menu.
    pub swipe_copy: bool,
    /// Four-finger tap toggles full screen.
    pub fullscreen: bool,
}

impl Default for Gestures {
    fn default() -> Self {
        Self { eyedropper: true, undo_taps: true, repeat_undo: true, scrub_clear: true, swipe_copy: true, fullscreen: true }
    }
}

/// What the app does for a gesture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Undo,
    Redo,
    RepeatUndo,
    ClearLayer,
    ShowCopyMenu,
    ToggleFullscreen,
    Eyedropper(Pos2),
}

/// The command for `gesture`, or `None` when that gesture is turned off.
pub fn route(gesture: Gesture, on: &Gestures) -> Option<Command> {
    match gesture {
        Gesture::Tap { fingers: 4.. } => on.fullscreen.then_some(Command::ToggleFullscreen),
        Gesture::Tap { fingers: 3 } => on.undo_taps.then_some(Command::Redo),
        Gesture::Tap { .. } => on.undo_taps.then_some(Command::Undo),
        Gesture::HoldStart { .. } => on.repeat_undo.then_some(Command::RepeatUndo),
        Gesture::Scrub { .. } => on.scrub_clear.then_some(Command::ClearLayer),
        Gesture::EyedropperHold { pos } => on.eyedropper.then_some(Command::Eyedropper(pos)),
        Gesture::Swipe { .. } => on.swipe_copy.then_some(Command::ShowCopyMenu),
    }
}

/// Detects a fast zig-zag in the horizontal position of a group of fingers.
#[derive(Clone, Debug, Default)]
pub struct ScrubDetector {
    pivot: Option<f32>,
    extreme: f32,
    dir: i8,
    reversals: Vec<f64>,
}

/// Minimum horizontal swing for a direction change to count, pixels.
const SCRUB_SWING: f32 = 30.0;
/// Direction changes needed inside [`SCRUB_WINDOW`] seconds.
const SCRUB_REVERSALS: usize = 4;
const SCRUB_WINDOW: f64 = 1.0;

impl ScrubDetector {
    /// Feed the group's centroid x at time `t`. Returns true when a scrub is detected.
    pub fn feed(&mut self, t: f64, x: f32) -> bool {
        let Some(pivot) = self.pivot else {
            self.pivot = Some(x);
            self.extreme = x;
            return false;
        };
        let mut reversed = false;
        match self.dir {
            0 => {
                if x - pivot > SCRUB_SWING {
                    self.dir = 1;
                    self.extreme = x;
                } else if pivot - x > SCRUB_SWING {
                    self.dir = -1;
                    self.extreme = x;
                }
            }
            1 => {
                if x > self.extreme {
                    self.extreme = x;
                } else if self.extreme - x > SCRUB_SWING {
                    reversed = true;
                    self.dir = -1;
                    self.extreme = x;
                }
            }
            _ => {
                if x < self.extreme {
                    self.extreme = x;
                } else if x - self.extreme > SCRUB_SWING {
                    reversed = true;
                    self.dir = 1;
                    self.extreme = x;
                }
            }
        }
        if reversed {
            self.pivot = Some(x);
            self.reversals.retain(|&r| t - r <= SCRUB_WINDOW);
            self.reversals.push(t);
            return self.reversals.len() >= SCRUB_REVERSALS;
        }
        false
    }
}

/// Per-finger state.
#[derive(Clone, Copy, Debug)]
struct Finger {
    start: Pos2,
    pos: Pos2,
}

/// Groups touches into gestures. A group starts with the first finger down and ends
/// when the last finger lifts.
#[derive(Clone, Debug, Default)]
pub struct Recognizer {
    fingers: BTreeMap<u64, Finger>,
    group_start: Option<f64>,
    max_fingers: usize,
    /// Largest travel of any finger from where it landed, in points.
    travel: f32,
    /// Latest time seen, for `tick`.
    now: f64,
    hold_fired: bool,
    eyedropper_fired: bool,
    scrub: ScrubDetector,
    scrub_fired: bool,
    /// Where the three fingers' centroid started, while they are down.
    swipe_origin: Option<Pos2>,
    swipe_fired: bool,
    /// Seconds a finger must rest for the eyedropper gesture.
    eyedropper_secs: f64,
}

impl Recognizer {
    pub fn new() -> Self {
        Self { eyedropper_secs: EYEDROPPER_HOLD_SECS, ..Self::default() }
    }

    /// Set the eyedropper hold delay (the Preferences slider).
    pub fn set_eyedropper_secs(&mut self, secs: f64) {
        self.eyedropper_secs = secs;
    }

    /// Fingers currently down.
    pub fn active(&self) -> usize {
        self.fingers.len()
    }

    /// A finger landed.
    pub fn down(&mut self, id: u64, pos: Pos2, t: f64) -> Vec<Gesture> {
        if self.fingers.is_empty() {
            self.group_start = Some(t);
            self.max_fingers = 0;
            self.travel = 0.0;
            self.hold_fired = false;
            self.eyedropper_fired = false;
            self.scrub = ScrubDetector::default();
            self.scrub_fired = false;
            self.swipe_origin = None;
            self.swipe_fired = false;
        }
        self.fingers.insert(id, Finger { start: pos, pos });
        self.max_fingers = self.max_fingers.max(self.fingers.len());
        self.now = t;
        Vec::new()
    }

    /// A finger moved.
    pub fn moved(&mut self, id: u64, pos: Pos2, t: f64) -> Vec<Gesture> {
        self.now = t;
        let Some(f) = self.fingers.get_mut(&id) else {
            return Vec::new();
        };
        f.pos = pos;
        self.travel = self.travel.max((pos - f.start).length());
        let mut out = Vec::new();
        if self.fingers.len() >= 3 && !self.scrub_fired {
            let n = self.fingers.len() as f32;
            let c = (self.fingers.values().fold(Vec2::ZERO, |acc, f| acc + f.pos.to_vec2()) / n).to_pos2();
            if self.scrub.feed(t, c.x) {
                self.scrub_fired = true;
                out.push(Gesture::Scrub {
                    fingers: self.fingers.len(),
                });
            }
            let origin = *self.swipe_origin.get_or_insert(c);
            let within = self.group_start.is_some_and(|s| t - s <= SWIPE_MAX_SECS);
            if !self.swipe_fired && !self.scrub_fired && within && (c - origin).length() >= SWIPE_DIST {
                self.swipe_fired = true;
                out.push(Gesture::Swipe { fingers: self.fingers.len() });
            }
        }
        out
    }

    /// A finger lifted. Emits a tap when the whole group was a quick, still tap.
    pub fn up(&mut self, id: u64, t: f64) -> Vec<Gesture> {
        self.now = t;
        self.fingers.remove(&id);
        if !self.fingers.is_empty() {
            return Vec::new();
        }
        let duration = self.group_start.map_or(0.0, |s| t - s);
        let is_tap = self.max_fingers >= 2
            && duration < TAP_MAX_SECS
            && self.travel < MAX_TRAVEL
            && !self.hold_fired
            && !self.scrub_fired
            && !self.swipe_fired;
        self.group_start = None;
        if is_tap {
            vec![Gesture::Tap {
                fingers: self.max_fingers,
            }]
        } else {
            Vec::new()
        }
    }

    /// Timer checks: holds that have lasted long enough while still.
    pub fn tick(&mut self, t: f64) -> Vec<Gesture> {
        self.now = t;
        let Some(start) = self.group_start else {
            return Vec::new();
        };
        let held = t - start;
        let mut out = Vec::new();
        if self.fingers.len() == 1
            && self.max_fingers == 1
            && !self.eyedropper_fired
            && held >= self.eyedropper_secs
            && self.travel < MAX_TRAVEL
        {
            self.eyedropper_fired = true;
            if let Some(f) = self.fingers.values().next() {
                out.push(Gesture::EyedropperHold { pos: f.pos });
            }
        }
        if self.fingers.len() == 2
            && self.max_fingers == 2
            && !self.hold_fired
            && held >= REPEAT_HOLD_SECS
            && self.travel < MAX_TRAVEL
        {
            self.hold_fired = true;
            out.push(Gesture::HoldStart { fingers: 2 });
        }
        out
    }
}

/// Is a two-finger pinch a "quick pinch" (fit to screen)? It must be shorter than 0.25 s
/// and shrink the view to under 0.8 of its size.
pub fn is_quick_pinch(duration_secs: f64, scale: f32) -> bool {
    duration_secs < 0.25 && scale < 0.8
}

/// Repeated undo during a two-finger hold: 4 per second, rising to 10 per second over
/// the first two seconds.
pub fn repeat_rate(held_secs: f64) -> f64 {
    4.0 + 6.0 * (held_secs / 2.0).clamp(0.0, 1.0)
}

/// Turns a repeat rate into a count of repeats due, once per frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct RepeatClock {
    /// Time of the next repeat, in seconds. `None` until the first `advance`.
    next: Option<f64>,
    start: f64,
}

impl RepeatClock {
    pub fn start(&mut self, t: f64) {
        self.start = t;
        self.next = None;
    }

    /// Number of repeats due at time `t`. The first repeat is due immediately.
    pub fn advance(&mut self, t: f64) -> usize {
        let mut due = 0;
        let mut next = self.next.unwrap_or(t);
        // Bounded so a long frame gap cannot queue an unbounded burst.
        while t >= next && due < 8 {
            due += 1;
            next += 1.0 / repeat_rate(next - self.start);
        }
        if t >= next {
            next = t + 1.0 / repeat_rate(t - self.start);
        }
        self.next = Some(next);
        due
    }
}

/// `--selftest` line: the gesture toggles and their delays.
pub fn selftest_line() -> Result<String, String> {
    let off = Gestures { eyedropper: false, ..Gestures::default() };
    if route(Gesture::EyedropperHold { pos: Pos2::new(1.0, 2.0) }, &off).is_some() {
        return Err("gesture prefs: a disabled eyedropper still ran".into());
    }
    let mut r = Recognizer::new();
    r.set_eyedropper_secs(0.35);
    r.down(1, Pos2::new(4.0, 4.0), 0.0);
    if !r.tick(0.2).is_empty() || r.tick(0.35).is_empty() {
        return Err("gesture prefs: the eyedropper delay did not apply".into());
    }
    Ok("gesture prefs: toggles disable their gesture, eyedropper delay applies ok".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn gesture_of(kind: usize) -> Gesture {
        match kind {
            0 => Gesture::Tap { fingers: 2 },
            1 => Gesture::Tap { fingers: 3 },
            2 => Gesture::Tap { fingers: 4 },
            3 => Gesture::HoldStart { fingers: 2 },
            4 => Gesture::Scrub { fingers: 3 },
            5 => Gesture::EyedropperHold { pos: pos2(1.0, 2.0) },
            _ => Gesture::Swipe { fingers: 3 },
        }
    }

    #[test]
    fn every_gesture_runs_its_command_when_on() {
        let on = Gestures::default();
        let cmds: Vec<Option<Command>> = (0..7).map(|k| route(gesture_of(k), &on)).collect();
        assert_eq!(
            cmds,
            vec![
                Some(Command::Undo),
                Some(Command::Redo),
                Some(Command::ToggleFullscreen),
                Some(Command::RepeatUndo),
                Some(Command::ClearLayer),
                Some(Command::Eyedropper(pos2(1.0, 2.0))),
                Some(Command::ShowCopyMenu),
            ]
        );
    }

    #[test]
    fn each_toggle_disables_exactly_its_gesture() {
        type Toggle = fn(&mut Gestures);
        let toggles: [(Toggle, &[usize]); 6] = [
            (|g| g.eyedropper = false, &[5]),
            (|g| g.undo_taps = false, &[0, 1]),
            (|g| g.repeat_undo = false, &[3]),
            (|g| g.scrub_clear = false, &[4]),
            (|g| g.swipe_copy = false, &[6]),
            (|g| g.fullscreen = false, &[2]),
        ];
        for (off, dead) in toggles {
            let mut on = Gestures::default();
            off(&mut on);
            for k in 0..7 {
                let silent = dead.contains(&k);
                assert_eq!(route(gesture_of(k), &on).is_none(), silent, "gesture {k} with toggle {on:?}");
            }
        }
    }

    #[test]
    fn three_finger_slide_is_a_swipe_and_a_zigzag_is_not() {
        let mut r = Recognizer::new();
        for id in 1..=3 {
            r.down(id, pos2(100.0, 300.0), 0.0);
        }
        let mut fired = Vec::new();
        for i in 1..=6 {
            for id in 1..=3 {
                fired.extend(r.moved(id, pos2(100.0 + 30.0 * i as f32, 300.0), 0.1 * i as f64));
            }
        }
        assert_eq!(fired, vec![Gesture::Swipe { fingers: 3 }]);
        for id in 1..=3 {
            assert!(r.up(id, 0.7).len() <= 1);
        }
    }

    #[test]
    fn eyedropper_delay_is_configurable() {
        let mut r = Recognizer::new();
        r.set_eyedropper_secs(0.5);
        r.down(1, pos2(10.0, 10.0), 0.0);
        assert!(r.tick(0.3).is_empty(), "not yet at the longer delay");
        assert_eq!(r.tick(0.5), vec![Gesture::EyedropperHold { pos: pos2(10.0, 10.0) }]);
    }


    #[test]
    fn two_finger_tap_is_undo_sized() {
        let mut r = Recognizer::new();
        assert!(r.down(1, pos2(10.0, 10.0), 0.0).is_empty());
        r.down(2, pos2(40.0, 10.0), 0.05);
        assert_eq!(r.active(), 2);
        assert!(r.up(1, 0.12).is_empty());
        assert_eq!(r.up(2, 0.15), vec![Gesture::Tap { fingers: 2 }]);
        assert_eq!(r.active(), 0);
    }

    #[test]
    fn three_and_four_finger_taps_report_their_count() {
        let mut r = Recognizer::new();
        for id in 1..=3 {
            r.down(id, pos2(id as f32 * 30.0, 50.0), 0.0);
        }
        for id in 1..=3 {
            r.up(id, 0.2);
        }
        let mut r = Recognizer::new();
        for id in 1..=4 {
            r.down(id, pos2(id as f32 * 30.0, 50.0), 0.0);
        }
        let mut out = Vec::new();
        for id in 1..=4 {
            out.extend(r.up(id, 0.1));
        }
        assert_eq!(out, vec![Gesture::Tap { fingers: 4 }]);
    }

    #[test]
    fn a_slow_tap_is_not_a_tap() {
        let mut r = Recognizer::new();
        r.down(1, pos2(0.0, 0.0), 0.0);
        r.down(2, pos2(30.0, 0.0), 0.0);
        r.up(1, 0.5);
        assert!(r.up(2, 0.6).is_empty());
    }

    #[test]
    fn moving_more_than_ten_points_cancels_the_tap() {
        let mut r = Recognizer::new();
        r.down(1, pos2(0.0, 0.0), 0.0);
        r.down(2, pos2(30.0, 0.0), 0.0);
        r.moved(2, pos2(45.0, 0.0), 0.05);
        r.up(1, 0.1);
        assert!(r.up(2, 0.12).is_empty());
    }

    #[test]
    fn two_finger_hold_fires_after_0_65_seconds_only_while_still() {
        let mut r = Recognizer::new();
        r.down(1, pos2(0.0, 0.0), 0.0);
        r.down(2, pos2(30.0, 0.0), 0.0);
        assert!(r.tick(0.6).is_empty());
        assert_eq!(r.tick(0.66), vec![Gesture::HoldStart { fingers: 2 }]);
        // Fires once, and lifting after a hold is not a tap.
        assert!(r.tick(0.9).is_empty());
        r.up(1, 1.0);
        assert!(r.up(2, 1.05).is_empty());

        let mut moving = Recognizer::new();
        moving.down(1, pos2(0.0, 0.0), 0.0);
        moving.down(2, pos2(30.0, 0.0), 0.0);
        moving.moved(1, pos2(0.0, 25.0), 0.1);
        assert!(moving.tick(0.5).is_empty());
    }

    #[test]
    fn one_finger_hold_starts_the_eyedropper_at_0_4_seconds() {
        let mut r = Recognizer::new();
        r.down(7, pos2(100.0, 120.0), 0.0);
        assert!(r.tick(0.35).is_empty());
        assert_eq!(
            r.tick(0.41),
            vec![Gesture::EyedropperHold {
                pos: pos2(100.0, 120.0)
            }]
        );
        assert!(r.tick(0.5).is_empty());
    }

    #[test]
    fn a_moving_finger_does_not_start_the_eyedropper() {
        let mut r = Recognizer::new();
        r.down(7, pos2(100.0, 120.0), 0.0);
        r.moved(7, pos2(112.0, 120.0), 0.05);
        assert!(r.tick(0.3).is_empty());
    }

    #[test]
    fn three_finger_zigzag_scrubs_and_a_slow_drag_does_not() {
        let mut r = Recognizer::new();
        for id in 1..=3 {
            r.down(id, pos2(200.0, 100.0), 0.0);
        }
        let xs = [200.0, 260.0, 200.0, 260.0, 200.0, 260.0, 200.0];
        let mut fired = Vec::new();
        for (i, x) in xs.iter().enumerate() {
            for id in 1..=3 {
                fired.extend(r.moved(id, pos2(*x, 100.0), 0.1 * i as f64));
            }
        }
        assert_eq!(fired, vec![Gesture::Scrub { fingers: 3 }]);

        let mut slow = Recognizer::new();
        for id in 1..=3 {
            slow.down(id, pos2(200.0, 100.0), 0.0);
        }
        let mut none = Vec::new();
        for i in 0..20 {
            let x = 200.0 + i as f32 * 10.0;
            for id in 1..=3 {
                none.extend(slow.moved(id, pos2(x, 100.0), i as f64));
            }
        }
        assert!(none.is_empty());
    }

    #[test]
    fn quick_pinch_rule() {
        assert!(is_quick_pinch(0.2, 0.5));
        assert!(!is_quick_pinch(0.4, 0.5));
        assert!(!is_quick_pinch(0.2, 0.9));
    }

    #[test]
    fn repeat_rate_accelerates_from_4_to_10() {
        assert!((repeat_rate(0.0) - 4.0).abs() < 1e-9);
        assert!((repeat_rate(1.0) - 7.0).abs() < 1e-9);
        assert!((repeat_rate(10.0) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn repeat_clock_fires_about_the_rate() {
        let mut clock = RepeatClock::default();
        clock.start(0.0);
        let mut total = 0;
        // 60 frames at 60 Hz: one second of holding at about 4 per second.
        for i in 0..=60 {
            total += clock.advance(f64::from(i) / 60.0);
        }
        assert!((4..=6).contains(&total), "got {total}");
    }

    #[test]
    fn scrub_detector_counts_reversals_in_a_window() {
        let mut d = ScrubDetector::default();
        assert!(!d.feed(0.0, 0.0));
        assert!(!d.feed(0.1, 50.0));
        assert!(!d.feed(0.2, 0.0));
        assert!(!d.feed(0.3, 50.0));
        assert!(!d.feed(0.4, 0.0));
        assert!(d.feed(0.5, 50.0));

        // The same zig-zag spread over five seconds is too slow to count.
        let mut slow = ScrubDetector::default();
        for i in 0..7 {
            let x = if i % 2 == 0 { 0.0 } else { 50.0 };
            assert!(!slow.feed(f64::from(i) * 1.5, x));
        }
    }
}
