//! The Android [`PenSource`] (A2): drains the JNI queue ([`crate::android_queue`]), runs the
//! events through the pure [`Translator`] and hands the frame loop pen events (stylus,
//! eraser) and egui events (fingers, mouse, keys).
//!
//! Only `tablet_backend::connect` on Android builds one for the app; the type itself has no
//! Android dependency, so the tests run on the host with a private queue.

use std::time::Instant;

use eframe::egui::{self, Event, TouchDeviceId, TouchId};

use crate::android_input::{Output, TouchPhase, Translator, UiOut};
use crate::android_queue::{Item, Queue};
use crate::tablet::{PenEvent, PenSource};

pub struct AndroidSource {
    queue: &'static Queue,
    translator: Translator,
    start: Instant,
    native_ppp: f32,
    zoom: f32,
    ui: Vec<Event>,
}

#[cfg(target_os = "android")]
fn log_input(what: &str) {
    log::info!("input: {what}");
}

#[cfg(not(target_os = "android"))]
fn log_input(_what: &str) {}

impl AndroidSource {
    pub fn new(queue: &'static Queue) -> Self {
        Self { queue, translator: Translator::new(), start: Instant::now(), native_ppp: 1.0, zoom: 1.0, ui: Vec::new() }
    }

    fn to_event(&self, u: UiOut) -> Event {
        let k = 1.0 / (self.native_ppp * self.zoom).max(0.01);
        let pt = |p: [f32; 2]| egui::pos2(p[0] * k, p[1] * k);
        match u {
            UiOut::Touch { id, phase, pos } => Event::Touch {
                device_id: TouchDeviceId(0),
                id: TouchId(id),
                phase: match phase {
                    TouchPhase::Start => egui::TouchPhase::Start,
                    TouchPhase::Move => egui::TouchPhase::Move,
                    TouchPhase::End => egui::TouchPhase::End,
                    TouchPhase::Cancel => egui::TouchPhase::Cancel,
                },
                pos: pt(pos),
                // A finger has no pressure: no force means no taper.
                force: None,
            },
            UiOut::PointerMoved(p) => Event::PointerMoved(pt(p)),
            UiOut::PointerButton { pos, pressed } => Event::PointerButton {
                pos: pt(pos),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            },
            UiOut::PointerGone => Event::PointerGone,
            UiOut::Wheel(d) => Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(d[0], d[1]),
                modifiers: egui::Modifiers::NONE,
            },
            UiOut::Key { key, pressed, repeat, modifiers } => {
                Event::Key { key, physical_key: Some(key), pressed, repeat, modifiers }
            }
            UiOut::Text(t) => Event::Text(t),
        }
    }
}

impl PenSource for AndroidSource {
    fn poll(&mut self) -> Vec<PenEvent> {
        let now = self.start.elapsed().as_secs_f64();
        let mut out = Output::default();
        for item in self.queue.drain() {
            match item {
                Item::Motion(m) => self.translator.process(&m, now, &mut out),
                Item::Key(k) => self.translator.key(k, &mut out),
            }
        }
        self.translator.tick(now, &mut out);
        // Pen positions are logical pixels (the app divides by the zoom factor itself).
        let k = 1.0 / self.native_ppp.max(0.01);
        let mut pen = out.pen;
        // One logcat line per contact start, end and cancel (input_smoke.sh looks for them).
        for e in &pen {
            let what = match e {
                PenEvent::Down => "pen down",
                PenEvent::Up => "pen up",
                PenEvent::Cancel => "pen cancel",
                _ => continue,
            };
            log_input(what);
        }
        for u in &out.ui {
            if let UiOut::Touch { phase: TouchPhase::Start | TouchPhase::Cancel, .. } = u {
                log_input("finger start or cancel");
            }
        }
        for e in &mut pen {
            if let PenEvent::Pose(p) = e {
                p.pos = [p.pos[0] * k, p.pos[1] * k];
            }
        }
        let ui: Vec<Event> = out.ui.into_iter().map(|u| self.to_event(u)).collect();
        self.ui.extend(ui);
        pen
    }

    fn name(&self) -> &'static str {
        "Android MotionEvent"
    }

    // A stylus is not delivered to egui at all (winit never sees it), so the app feeds egui
    // pointer events for it, as it does for the Wayland tablet protocol.
    fn needs_pointer_events(&self) -> bool {
        true
    }

    fn set_scale(&mut self, native_pixels_per_point: f32, zoom: f32) {
        self.native_ppp = native_pixels_per_point;
        self.zoom = zoom;
    }

    fn take_ui_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.ui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leak() -> &'static Queue {
        Box::leak(Box::new(Queue::new()))
    }

    fn push(q: &Queue, action: i32, idx: i32, t: i64, tool: i32, x: f32, pressure: f32) {
        q.begin(action, idx, 0, 0, 1, 0.0, 0.0);
        q.sample(t, 0, tool, x, 40.0, pressure, 0.0, 0.0);
        q.end();
    }

    #[test]
    fn stylus_and_finger_end_up_in_different_places() {
        let q = leak();
        let mut s = AndroidSource::new(q);
        s.set_scale(2.0, 1.0);
        push(q, 0, 0, 100, 2, 200.0, 0.6);
        let pen = s.poll();
        assert!(matches!(pen[1], PenEvent::Pose(p) if p.pos == [100.0, 20.0] && p.pressure == Some(0.6)));
        assert!(s.take_ui_events().is_empty());
        push(q, 1, 0, 120, 2, 200.0, 0.6);
        s.poll();
        push(q, 0, 0, 200, 1, 200.0, 0.9);
        assert!(s.poll().is_empty());
        let ui = s.take_ui_events();
        assert!(matches!(ui[0], Event::Touch { force: None, pos, .. } if pos == egui::pos2(100.0, 20.0)));
        assert!(s.take_ui_events().is_empty(), "taken once");
    }

    #[test]
    fn zoom_factor_scales_touch_but_not_pen_positions() {
        let q = leak();
        let mut s = AndroidSource::new(q);
        s.set_scale(2.0, 2.0);
        push(q, 0, 0, 100, 1, 400.0, 1.0);
        s.poll();
        assert!(matches!(s.take_ui_events()[0], Event::Touch { pos, .. } if pos == egui::pos2(100.0, 10.0)));
    }
}
