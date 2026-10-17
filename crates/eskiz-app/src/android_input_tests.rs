//! A2 end to end on the headless app: Java-side `MotionEvent` data goes through the JNI queue,
//! the Android pen source and the translator into the real `EskizApp`.

use eframe::egui::Vec2;

use crate::android_input::FLAG_CANCELED;
use crate::android_queue::Queue;
use crate::android_source::AndroidSource;
use crate::app::{Prefs, Tool};
use crate::document::Document;
use crate::snapshot::Headless;
use crate::tablet::{PenSource, PenTracker};

const STYLUS: i32 = 2;
const FINGER: i32 = 1;
const ERASER: i32 = 4;
const DOWN: i32 = 0;
const UP: i32 = 1;
const MOVE: i32 = 2;
const CANCEL: i32 = 3;
const POINTER_DOWN: i32 = 5;
const POINTER_UP: i32 = 6;

struct Rig {
    h: Headless,
    q: &'static Queue,
    src: AndroidSource,
    tracker: PenTracker,
    ms: i64,
}

/// (pointer id, tool type, x, y)
type P = (i32, i32, f32, f32);

impl Rig {
    fn new() -> Self {
        let q: &'static Queue = Box::leak(Box::new(Queue::new()));
        Self {
            h: Headless::new(Vec2::new(1024.0, 700.0), Document::new(800, 600), Prefs::default()),
            q,
            src: AndroidSource::new(q),
            tracker: PenTracker::new(),
            ms: 1000,
        }
    }

    /// Send one motion event (one sample per call) and run a frame, the way the app does:
    /// poll the source, fold the pen events, hand the rest to egui.
    fn send(&mut self, action: i32, index: i32, flags: i32, pointers: &[P]) {
        self.ms += 8;
        self.q.begin(action, index, flags, 0, pointers.len() as i32, 0.0, 0.0);
        for &(id, tool, x, y) in pointers {
            self.q.sample(self.ms, id, tool, x, y, 0.6, 0.5, 1.0);
        }
        self.q.end();
        let now = self.ms as f64 / 1000.0;
        let pen = self.src.poll();
        self.h.app.pen.frame = self.tracker.frame(pen, now);
        for e in self.src.take_ui_events() {
            self.h.event(e);
        }
        self.h.step();
    }

    fn undo_steps(&self) -> usize {
        self.h.app.session.undo_steps()
    }
}

fn committed_stroke(r: &mut Rig, tool: i32) {
    r.send(DOWN, 0, 0, &[(0, tool, 300.0, 300.0)]);
    for k in 1..8 {
        r.send(MOVE, 0, 0, &[(0, tool, 300.0 + 30.0 * k as f32, 300.0)]);
    }
    r.send(UP, 0, 0, &[(0, tool, 540.0, 300.0)]);
}

#[test]
fn stylus_stroke_paints_and_commits_one_undo_step() {
    let mut r = Rig::new();
    let blank = r.h.app.session.flatten();
    committed_stroke(&mut r, STYLUS);
    assert!(!r.h.app.session.is_stroking());
    assert_ne!(r.h.app.session.flatten(), blank);
    assert_eq!(r.undo_steps(), 1);
}

#[test]
fn finger_stroke_paints_without_pen_data() {
    let mut r = Rig::new();
    let blank = r.h.app.session.flatten();
    committed_stroke(&mut r, FINGER);
    assert_ne!(r.h.app.session.flatten(), blank);
    assert_eq!(r.undo_steps(), 1);
    assert!(r.h.app.pen.stroke_reading.is_none(), "a finger stroke carries no pen reading");
}

#[test]
fn eraser_tool_type_switches_to_the_eraser_tool_while_in_range() {
    let mut r = Rig::new();
    assert_eq!(r.h.app.tool, Tool::Brush);
    r.send(DOWN, 0, 0, &[(0, ERASER, 300.0, 300.0)]);
    assert_eq!(r.h.app.tool, Tool::Eraser);
    r.send(UP, 0, 0, &[(0, ERASER, 300.0, 300.0)]);
    // The eraser end leaves hover range: the tool goes back.
    r.send(10, 0, 0, &[(0, ERASER, 300.0, 300.0)]);
    assert_eq!(r.h.app.tool, Tool::Brush);
}

/// Palm rejection with a stylus-type contact: Android cancels it mid-stroke.
#[test]
fn action_cancel_mid_stroke_leaves_the_layer_and_the_undo_stack_untouched() {
    for (cancel_action, flags) in [(CANCEL, 0), (UP, FLAG_CANCELED)] {
        let mut r = Rig::new();
        committed_stroke(&mut r, STYLUS);
        let layer = r.h.app.session.flatten();
        let steps = r.undo_steps();
        r.send(DOWN, 0, 0, &[(0, STYLUS, 300.0, 400.0)]);
        for k in 1..6 {
            r.send(MOVE, 0, 0, &[(0, STYLUS, 300.0 + 40.0 * k as f32, 400.0)]);
        }
        assert!(r.h.app.session.is_stroking(), "the stroke was under way");
        r.send(cancel_action, 0, flags, &[(0, STYLUS, 500.0, 400.0)]);
        assert!(!r.h.app.session.is_stroking());
        assert_eq!(r.h.app.session.flatten(), layer, "nothing of the canceled stroke is left ({cancel_action})");
        assert_eq!(r.undo_steps(), steps, "no undo step for the canceled stroke");
        assert!(r.h.app.session.can_undo());
        // And the pen works again straight away.
        committed_stroke(&mut r, STYLUS);
        assert_eq!(r.undo_steps(), steps + 1);
    }
}

#[test]
fn cancel_in_the_frame_of_the_press_never_starts_a_stroke() {
    let mut r = Rig::new();
    let blank = r.h.app.session.flatten();
    // Both events arrive before the frame loop runs.
    r.q.begin(DOWN, 0, 0, 0, 1, 0.0, 0.0);
    r.q.sample(1000, 0, STYLUS, 300.0, 300.0, 0.6, 0.0, 0.0);
    r.q.end();
    r.q.begin(CANCEL, 0, 0, 0, 1, 0.0, 0.0);
    r.q.sample(1010, 0, STYLUS, 300.0, 300.0, 0.6, 0.0, 0.0);
    r.q.end();
    let pen = r.src.poll();
    r.h.app.pen.frame = r.tracker.frame(pen, 1.0);
    r.h.step();
    assert!(!r.h.app.session.is_stroking());
    assert_eq!(r.h.app.session.flatten(), blank);
    assert_eq!(r.undo_steps(), 0);
}

/// Palm rejection with a finger the system cancels (touch painting is on by default).
#[test]
fn a_cancelled_finger_stroke_leaves_nothing_behind() {
    for (action, flags) in [(CANCEL, 0), (UP, FLAG_CANCELED)] {
        let mut r = Rig::new();
        committed_stroke(&mut r, STYLUS);
        let layer = r.h.app.session.flatten();
        let steps = r.undo_steps();
        r.send(DOWN, 0, 0, &[(0, FINGER, 300.0, 450.0)]);
        for k in 1..6 {
            r.send(MOVE, 0, 0, &[(0, FINGER, 300.0 + 40.0 * k as f32, 450.0)]);
        }
        assert!(r.h.app.session.is_stroking());
        r.send(action, 0, flags, &[(0, FINGER, 500.0, 450.0)]);
        assert!(!r.h.app.session.is_stroking());
        assert_eq!(r.h.app.session.flatten(), layer);
        assert_eq!(r.undo_steps(), steps);
    }
}

#[test]
fn a_palm_cancelled_by_the_stylus_does_not_stop_the_stylus_stroke() {
    let mut r = Rig::new();
    // Palm lands first (a finger), then the pen touches: the palm is dropped, the pen paints.
    r.send(DOWN, 0, 0, &[(0, FINGER, 700.0, 500.0)]);
    r.send(POINTER_DOWN, 1, 0, &[(0, FINGER, 700.0, 500.0), (1, STYLUS, 300.0, 300.0)]);
    for k in 1..6 {
        r.send(MOVE, 0, 0, &[(0, FINGER, 700.0, 500.0 + k as f32), (1, STYLUS, 300.0 + 40.0 * k as f32, 300.0)]);
    }
    assert!(r.h.app.session.is_stroking());
    assert!(r.h.app.pen.stroke_reading.is_some(), "it is the pen stroke that is running");
    r.send(POINTER_UP, 1, 0, &[(0, FINGER, 700.0, 505.0), (1, STYLUS, 500.0, 300.0)]);
    r.send(UP, 0, 0, &[(0, FINGER, 700.0, 505.0)]);
    assert!(!r.h.app.session.is_stroking());
    assert_eq!(r.undo_steps(), 1, "one stroke, the pen's");
}

#[test]
fn finger_resting_on_the_screen_while_the_stylus_draws_is_ignored() {
    let mut r = Rig::new();
    r.send(DOWN, 0, 0, &[(1, STYLUS, 300.0, 300.0)]);
    r.send(POINTER_DOWN, 1, 0, &[(1, STYLUS, 300.0, 300.0), (2, FINGER, 700.0, 500.0)]);
    for k in 1..6 {
        r.send(MOVE, 0, 0, &[(1, STYLUS, 300.0 + 40.0 * k as f32, 300.0), (2, FINGER, 700.0 + k as f32, 500.0)]);
    }
    r.send(POINTER_UP, 1, 0, &[(1, STYLUS, 500.0, 300.0), (2, FINGER, 705.0, 500.0)]);
    r.send(UP, 0, 0, &[(1, STYLUS, 500.0, 300.0)]);
    assert_eq!(r.undo_steps(), 1);
    assert!(r.h.app.session.can_undo());
}

/// Multi-touch keeps working: the finger events are the same `Touch` events egui-winit made.
#[test]
fn two_finger_tap_still_undoes() {
    let mut r = Rig::new();
    committed_stroke(&mut r, STYLUS);
    assert_eq!(r.undo_steps(), 1);
    let a = (0, FINGER, 400.0, 350.0);
    let b = (1, FINGER, 460.0, 350.0);
    r.send(DOWN, 0, 0, &[a]);
    r.send(POINTER_DOWN, 1, 0, &[a, b]);
    r.send(POINTER_UP, 0, 0, &[a, b]);
    r.send(UP, 0, 0, &[b]);
    assert!(!r.h.app.session.can_undo(), "the two-finger tap undid the stroke");
}

#[test]
fn two_finger_pinch_zooms_the_view_and_never_paints() {
    let mut r = Rig::new();
    let zoom = r.h.app.view.zoom;
    r.send(DOWN, 0, 0, &[(0, FINGER, 400.0, 350.0)]);
    r.send(POINTER_DOWN, 1, 0, &[(0, FINGER, 400.0, 350.0), (1, FINGER, 500.0, 350.0)]);
    for k in 1..=10 {
        let d = 10.0 * k as f32;
        r.send(MOVE, 0, 0, &[(0, FINGER, 400.0 - d, 350.0), (1, FINGER, 500.0 + d, 350.0)]);
    }
    assert!(r.h.app.view.zoom > zoom * 1.3, "pinch out zoomed in: {} -> {}", zoom, r.h.app.view.zoom);
    r.send(POINTER_UP, 0, 0, &[(0, FINGER, 300.0, 350.0), (1, FINGER, 600.0, 350.0)]);
    r.send(UP, 0, 0, &[(1, FINGER, 600.0, 350.0)]);
    assert_eq!(r.undo_steps(), 0);
}
