//! The queue between the Java shim and the frame loop (A2).
//!
//! `EskizActivity.dispatchTouchEvent` runs on the Android UI thread and calls the `native*`
//! functions below (exported in [`jni_exports`]) once per event: `begin`, then one `sample` per
//! pointer per batched sample (oldest first, the event's own sample last), then `end`. The
//! frame loop drains the queue in `AndroidSource::poll`. The assembly and the queue are plain
//! Rust so they are tested on the host; only the `extern "system"` shims are Android-only.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::android_input::{Action, Frame, KeyInput, MotionEvent, PointerSample, ToolType};

/// Events kept while the frame loop is not draining (a paused app). The oldest are dropped.
const MAX_ITEMS: usize = 8192;

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Motion(MotionEvent),
    Key(KeyInput),
}

struct Pending {
    event: MotionEvent,
    pointer_count: usize,
    samples: Vec<(i64, PointerSample)>,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Item>,
    pending: Option<Pending>,
    dropped: u64,
}

/// An independent queue, so tests do not share the process-wide one.
#[derive(Default)]
pub struct Queue {
    state: Mutex<State>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start an event. `pointer_count` is the number of pointers in every sample.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &self,
        action: i32,
        action_index: i32,
        flags: i32,
        buttons: i32,
        pointer_count: i32,
        hscroll: f32,
        vscroll: f32,
    ) {
        let event = MotionEvent {
            action: Action::from_masked(action),
            action_index: action_index.max(0) as usize,
            flags,
            buttons,
            history: Vec::new(),
            current: Frame { time_ms: 0, pointers: Vec::new() },
            hscroll,
            vscroll,
        };
        lock(&self.state).pending =
            Some(Pending { event, pointer_count: pointer_count.max(0) as usize, samples: Vec::new() });
    }

    /// One pointer of one sample.
    #[allow(clippy::too_many_arguments)]
    pub fn sample(&self, time_ms: i64, id: i32, tool: i32, x: f32, y: f32, pressure: f32, tilt: f32, orientation: f32) {
        if let Some(p) = lock(&self.state).pending.as_mut() {
            let s = PointerSample { id, tool: ToolType::from_raw(tool), x, y, pressure, tilt, orientation };
            p.samples.push((time_ms, s));
        }
    }

    /// Finish the event and queue it. Returns false when there was nothing to finish.
    pub fn end(&self) -> bool {
        let mut st = lock(&self.state);
        let Some(p) = st.pending.take() else {
            return false;
        };
        let n = p.pointer_count.max(1);
        let mut frames: Vec<Frame> = p
            .samples
            .chunks(n)
            .map(|c| Frame { time_ms: c[0].0, pointers: c.iter().map(|(_, s)| *s).collect() })
            .collect();
        let Some(current) = frames.pop() else {
            return false;
        };
        let mut event = p.event;
        event.history = frames;
        event.current = current;
        Self::push(&mut st, Item::Motion(event));
        true
    }

    pub fn key(&self, pressed: bool, repeat: bool, key_code: i32, unicode: i32, meta: i32) {
        let k = KeyInput { pressed, repeat, key_code, unicode: unicode.max(0) as u32, meta };
        Self::push(&mut lock(&self.state), Item::Key(k));
    }

    fn push(st: &mut State, item: Item) {
        if st.queue.len() >= MAX_ITEMS {
            st.queue.pop_front();
            st.dropped += 1;
        }
        st.queue.push_back(item);
    }

    /// Everything queued so far, oldest first.
    pub fn drain(&self) -> Vec<Item> {
        lock(&self.state).queue.drain(..).collect()
    }

    /// Events dropped because nothing drained the queue.
    pub fn dropped(&self) -> u64 {
        lock(&self.state).dropped
    }
}

static GLOBAL: OnceLock<Queue> = OnceLock::new();
type Waker = Box<dyn Fn() + Send + Sync>;
static WAKER: Mutex<Option<Waker>> = Mutex::new(None);

/// The process-wide queue the JNI functions feed.
pub fn global() -> &'static Queue {
    GLOBAL.get_or_init(Queue::new)
}

/// Called after every event so the (otherwise idle) frame loop wakes up.
pub fn set_waker(w: impl Fn() + Send + Sync + 'static) {
    *WAKER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(w));
}

#[cfg(not(test))]
static BACKGROUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
thread_local! {
    static BACKGROUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The activity is stopped (`true`: `onStop`) or visible again (`false`: `onStart`). Set by
/// the Java shim before the system tears the surface down, so the app can tell a real trip to
/// the background from eframe's periodic `save`.
pub fn set_backgrounded(on: bool) {
    #[cfg(not(test))]
    BACKGROUND.store(on, std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    BACKGROUND.with(|b| b.set(on));
}

/// True while the activity is stopped (see [`set_backgrounded`]).
pub fn backgrounded() -> bool {
    #[cfg(not(test))]
    return BACKGROUND.load(std::sync::atomic::Ordering::SeqCst);
    #[cfg(test)]
    return BACKGROUND.with(std::cell::Cell::get);
}

#[cfg(target_os = "android")]
fn wake() {
    if let Some(w) = WAKER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        w();
    }
}

/// The `Java_app_eskiz_EskizActivity_native*` functions. The method names and signatures must
/// match `EskizActivity.java`.
#[cfg(target_os = "android")]
#[allow(unsafe_code, non_snake_case, clippy::too_many_arguments)]
mod jni_exports {
    use std::ffi::c_void;

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeBegin(
        _env: *mut c_void,
        _class: *mut c_void,
        action: i32,
        action_index: i32,
        flags: i32,
        buttons: i32,
        pointer_count: i32,
        hscroll: f32,
        vscroll: f32,
    ) {
        super::global().begin(action, action_index, flags, buttons, pointer_count, hscroll, vscroll);
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeSample(
        _env: *mut c_void,
        _class: *mut c_void,
        time_ms: i64,
        id: i32,
        tool: i32,
        x: f32,
        y: f32,
        pressure: f32,
        tilt: f32,
        orientation: f32,
    ) {
        super::global().sample(time_ms, id, tool, x, y, pressure, tilt, orientation);
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeEnd(_env: *mut c_void, _class: *mut c_void) {
        if super::global().end() {
            super::wake();
        }
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeBackground(
        _env: *mut c_void,
        _class: *mut c_void,
        stopped: u8,
    ) {
        super::set_backgrounded(stopped != 0);
        super::wake();
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeInsets(
        _env: *mut c_void,
        _class: *mut c_void,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    ) {
        crate::safe_area::set_pixels(left, top, right, bottom);
        super::wake();
    }

    #[no_mangle]
    pub extern "system" fn Java_app_eskiz_EskizActivity_nativeKey(
        _env: *mut c_void,
        _class: *mut c_void,
        pressed: u8,
        repeat: u8,
        key_code: i32,
        unicode: i32,
        meta: i32,
    ) {
        super::global().key(pressed != 0, repeat != 0, key_code, unicode, meta);
        super::wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_history_then_current_per_pointer() {
        let q = Queue::new();
        // A MOVE with two pointers and two batched samples plus the current one.
        q.begin(2, 0, 0, 0, 2, 0.0, 0.0);
        for (t, x) in [(10, 1.0), (20, 2.0), (30, 3.0)] {
            q.sample(t, 5, 2, x, 0.0, 0.5, 0.1, 0.2);
            q.sample(t, 9, 1, x + 100.0, 0.0, 0.0, 0.0, 0.0);
        }
        assert!(q.end());
        let items = q.drain();
        let Item::Motion(m) = &items[0] else { panic!("motion expected") };
        assert_eq!(m.action, Action::Move);
        assert_eq!(m.history.len(), 2);
        assert_eq!(m.history[0].time_ms, 10);
        assert_eq!(m.history[1].pointers[0].x, 2.0);
        assert_eq!(m.current.time_ms, 30);
        assert_eq!(m.current.pointers.len(), 2);
        assert_eq!(m.current.pointers[0].id, 5);
        assert_eq!(m.current.pointers[0].tool, ToolType::Stylus);
        assert_eq!(m.current.pointers[1].tool, ToolType::Finger);
        assert!(q.drain().is_empty());
    }

    #[test]
    fn end_without_begin_and_keys_and_overflow() {
        let q = Queue::new();
        assert!(!q.end());
        q.key(true, false, 29, 97, 0);
        assert!(matches!(q.drain()[0], Item::Key(k) if k.key_code == 29 && k.unicode == 97));
        for _ in 0..MAX_ITEMS + 5 {
            q.key(true, false, 29, 0, 0);
        }
        assert_eq!(q.drain().len(), MAX_ITEMS);
        assert_eq!(q.dropped(), 5);
    }

    #[test]
    fn a_queue_survives_a_poisoned_lock() {
        let q = std::sync::Arc::new(Queue::new());
        let q2 = q.clone();
        let _ = std::thread::spawn(move || {
            let _g = q2.state.lock().unwrap();
            panic!("poison");
        })
        .join();
        q.key(true, false, 1, 0, 0);
        assert_eq!(q.drain().len(), 1);
    }
}
