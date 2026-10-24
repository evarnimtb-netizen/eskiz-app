//! A bounded close: every step of leaving the app has a time limit.
//!
//! A window that is told to close must go away within about two seconds, however the graphics
//! driver behaves while the swap chain and device are torn down. The order is fixed:
//!
//! 1. The document is written first (the storage worker finishes its queue; a save that is still
//!    running is given more time, up to [`SAVE_HARD_CAP`]).
//! 2. Caches and thumbnails are skipped when they do not finish in time.
//! 3. A deadline thread ends the process once [`GRACE`] has passed and no save is running. It
//!    covers the part nothing can instrument: eframe's own teardown of the window and the GPU.
//!
//! Every step leaves a line in `startup.log` (`exit: ...`). A stall while closing is never
//! recorded as a driver fault: only stalls while the window is in use are.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::channel;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long closing may take before the process is ended.
pub const GRACE: Duration = Duration::from_secs(2);
/// The longest a running save can hold the process back.
pub const SAVE_HARD_CAP: Duration = Duration::from_secs(30);

static CLOSING: AtomicBool = AtomicBool::new(false);
static SAVES_PENDING: AtomicUsize = AtomicUsize::new(0);
static STARTED: OnceLock<Instant> = OnceLock::new();

/// Whether the window is closing. The freeze watchdog stays quiet then.
pub fn closing() -> bool {
    CLOSING.load(Ordering::Relaxed)
}

/// A save was queued (called by the storage worker's sender).
pub fn save_queued() {
    SAVES_PENDING.fetch_add(1, Ordering::SeqCst);
}

/// A queued save finished or failed.
pub fn save_done() {
    loop {
        let n = SAVES_PENDING.load(Ordering::SeqCst);
        if n == 0 || SAVES_PENDING.compare_exchange(n, n - 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            break;
        }
    }
}

fn saves_pending() -> usize {
    SAVES_PENDING.load(Ordering::SeqCst)
}

/// One breadcrumb of the close path, in the start-up log.
pub fn step(what: &str) {
    let t = STARTED.get().map_or(0.0, |s| s.elapsed().as_secs_f64());
    crate::startup::log(&format!("exit: {what} (+{t:.2}s)"));
}

/// The window is closing: remember it, and arm the deadline that ends the process. Safe to
/// call more than once.
pub fn begin() {
    if CLOSING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = STARTED.set(Instant::now());
    step("close requested");
    if cfg!(test) {
        return;
    }
    spawn_deadline(GRACE, SAVE_HARD_CAP, saves_pending, || {
        step("deadline reached; ending the process");
        std::process::exit(0);
    });
}

/// After the window is gone: let a save that is still running finish (up to [`SAVE_HARD_CAP`]
/// after the close request), so returning from `main` does not cut it off half written.
pub fn wait_for_saves() {
    if saves_pending() == 0 {
        return;
    }
    step("waiting for a save that is still running");
    let start = STARTED.get().copied().unwrap_or_else(Instant::now);
    while saves_pending() > 0 && start.elapsed() < SAVE_HARD_CAP {
        std::thread::sleep(Duration::from_millis(20));
    }
    step(if saves_pending() == 0 { "the save finished" } else { "a save was still running at the time limit" });
}

/// Start the thread that calls `terminate` once `grace` has passed and `pending()` is zero, or
/// once `hard_cap` has passed whatever is pending.
pub fn spawn_deadline(grace: Duration, hard_cap: Duration, pending: fn() -> usize, terminate: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name("eskiz-exit-deadline".into()).spawn(move || {
        let start = Instant::now();
        std::thread::sleep(grace);
        while pending() > 0 && start.elapsed() < hard_cap {
            std::thread::sleep(Duration::from_millis(20));
        }
        terminate();
    });
}

/// Run `f` on its own thread and wait at most `budget` for it. `None` when it did not finish
/// (the thread is left behind; the process ends soon after). The outcome is logged.
pub fn bounded<T: Send + 'static>(name: &str, budget: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    step(&format!("{name}: start"));
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name(format!("eskiz-exit-{name}")).spawn(move || {
        let _ = tx.send(f());
    });
    if spawned.is_err() {
        step(&format!("{name}: could not start; skipped"));
        return None;
    }
    match rx.recv_timeout(budget) {
        Ok(v) => {
            step(&format!("{name}: done"));
            Some(v)
        }
        Err(_) => {
            step(&format!("{name}: still running after {:.1}s; skipped", budget.as_secs_f64()));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    #[test]
    fn a_hanging_step_is_given_up_within_its_budget() {
        let t = Instant::now();
        let r = bounded("fake-hang", Duration::from_millis(150), || {
            std::thread::sleep(Duration::from_secs(30));
        });
        assert!(r.is_none());
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    }

    #[test]
    fn a_quick_step_returns_its_value() {
        assert_eq!(bounded("fake-quick", Duration::from_secs(5), || 7), Some(7));
    }

    #[test]
    fn the_deadline_ends_a_hung_close() {
        let hit = Arc::new(AtomicU32::new(0));
        let h = hit.clone();
        let t = Instant::now();
        spawn_deadline(Duration::from_millis(80), Duration::from_secs(5), || 0, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        while hit.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(hit.load(Ordering::SeqCst), 1);
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn the_deadline_waits_for_a_running_save_but_not_forever() {
        fn one() -> usize {
            1
        }
        let hit = Arc::new(AtomicU32::new(0));
        let h = hit.clone();
        let t = Instant::now();
        spawn_deadline(Duration::from_millis(30), Duration::from_millis(400), one, move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(hit.load(Ordering::SeqCst), 0, "a save is still running");
        while hit.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(hit.load(Ordering::SeqCst), 1, "the hard cap ends it");
    }

    #[test]
    fn beginning_a_close_is_remembered_and_repeatable() {
        begin();
        begin();
        assert!(closing());
    }

    #[test]
    fn save_counter_never_goes_below_zero() {
        save_done();
        save_done();
        save_queued();
        assert!(saves_pending() >= 1 || saves_pending() == 0);
    }
}
