//! App lifecycle policy for platforms that suspend the process (Android).
//!
//! When the OS sends the app to the background it may kill it without another call, so the
//! work that cannot be lost is written *before* the suspend returns:
//!
//! * [`Lifecycle::suspend`]: finish the stroke in flight, write the crash-recovery journal
//!   **synchronously** (see [`journal_sync`]) and persist the preferences;
//! * [`Lifecycle::resume`]: recreate the surface and upload every layer to the GPU again,
//!   because the window and, on some devices, the device were lost while hidden.
//!
//! The policy is a small state machine over the [`Host`] trait, so the host tests can drive
//! it with a recording host (and the real journal) without a window. eframe offers no
//! suspend callback, only `App::save`, which it calls on `Suspended`; the app therefore
//! treats every save as a suspend checkpoint (cheap: the journal is incremental) and
//! recognises a real resume as the first frame after a gap of [`RESUME_GAP_SECS`] or more.
//! A periodic save followed by a frame right away is not a resume and re-uploads nothing.

use std::path::Path;

use crate::document::Document;
use crate::journal::{self, Planner};

/// A frame this long after a save counts as coming back from the background.
pub const RESUME_GAP_SECS: f64 = 1.0;

/// What the policy needs from the app.
pub trait Host {
    /// Cancel or finish the stroke in progress (nothing half-drawn may reach the journal).
    fn stop_stroke(&mut self);
    /// Write the recovery journal of the unsaved document now, on this thread. Returns once
    /// the files are on disk. A clean document has nothing to write.
    fn write_journal(&mut self) -> Result<(), String>;
    /// Persist preferences, colours and brush choice.
    fn persist_prefs(&mut self);
    /// The window has a surface again: size it and ask for a frame.
    fn recreate_surface(&mut self);
    /// Upload every layer to the GPU again and recomposite.
    fn reupload_layers(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Active,
    Suspended,
}

/// The state machine. Cheap to create; owned by the app.
#[derive(Debug)]
pub struct Lifecycle {
    phase: Phase,
    suspended_at: f64,
    /// Number of suspends and of resumes that re-uploaded layers (for tests and the log).
    pub suspends: u64,
    pub resumes: u64,
    /// The last journal error, if the latest suspend could not write it.
    pub last_error: Option<String>,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self { phase: Phase::Active, suspended_at: 0.0, suspends: 0, resumes: 0, last_error: None }
    }
}

impl Lifecycle {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The app is going to the background (or a periodic save): write what must survive.
    /// Safe to call again while suspended; it writes again (the journal is incremental).
    pub fn suspend(&mut self, host: &mut dyn Host, now: f64) {
        host.stop_stroke();
        self.last_error = host.write_journal().err();
        host.persist_prefs();
        if self.phase == Phase::Active {
            self.phase = Phase::Suspended;
            self.suspended_at = now;
            self.suspends += 1;
        }
    }

    /// Called once per frame. A frame after a long enough gap is a resume.
    pub fn frame(&mut self, host: &mut dyn Host, now: f64) {
        if self.phase == Phase::Suspended {
            if now - self.suspended_at >= RESUME_GAP_SECS {
                self.resume(host);
            } else {
                // A periodic save, not a trip to the background.
                self.phase = Phase::Active;
            }
        }
    }

    /// Back in the foreground: new surface, layers uploaded again. No effect when active.
    pub fn resume(&mut self, host: &mut dyn Host) {
        if self.phase != Phase::Suspended {
            return;
        }
        self.phase = Phase::Active;
        self.resumes += 1;
        host.recreate_surface();
        host.reupload_layers();
    }
}

/// Write the journal of `doc` on this thread with the document's own planner, so the next
/// periodic plan stays incremental. A failed write marks the planner broken (the next plan
/// starts from the base again).
pub fn journal_sync(planner: &mut Planner, doc: &Document, root: &Path, id: &str, title: &str) -> Result<(), String> {
    let plan = planner.plan(doc, root, id, title);
    let result = journal::execute(&plan);
    if result.is_err() {
        planner.mark_broken();
    }
    result
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
