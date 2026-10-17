//! Stroke lifecycle trace, for catching a rare vanishing stroke.
//!
//! `ESKIZ_TRACE_STROKES=1` makes the app log every stroke `begin`, `commit` and `cancel` (with the
//! reason), and every gesture or event it ignored because a stroke was open, to stderr as
//! `eskiz-stroke: ...` lines with a time since start. The check is one cached environment lookup, so
//! the trace costs nothing when the variable is not set. Tests record the lines in a per-thread
//! buffer (see [`recorded`]) whether or not the variable is set.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

fn env_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("ESKIZ_TRACE_STROKES").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

fn start() -> &'static Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now)
}

thread_local! {
    static RECORD: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Log one event. `what` is `begin`, `commit`, `cancel` or `ignored`; `detail` says why and where.
pub fn event(what: &str, detail: &str) {
    let on = env_on();
    if !on && !cfg!(test) {
        return;
    }
    let line = format!("{what} {detail}");
    if on {
        eprintln!("eskiz-stroke: [{:8.3}s] {line}", start().elapsed().as_secs_f64());
    }
    if cfg!(test) {
        RECORD.with(|r| r.borrow_mut().push(line));
    }
}

/// The lines recorded on this thread (tests only: other builds record nothing).
pub fn recorded() -> Vec<String> {
    RECORD.with(|r| r.borrow().clone())
}

/// Forget the recorded lines.
pub fn clear() {
    RECORD.with(|r| r.borrow_mut().clear());
}
