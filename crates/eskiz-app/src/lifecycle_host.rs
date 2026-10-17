//! `EskizApp` as a lifecycle [`Host`]: the glue between the policy in `lifecycle` and the
//! document, the storage worker, the GPU canvas and eframe's storage.

use std::time::Duration;

use eframe::egui;

use crate::app::{EskizApp, BRUSH_KEY, COLOUR_KEY, PREF_KEY};
use crate::gallery::Screen;
use crate::lifecycle::{journal_sync, Host, Phase};

/// How long a suspend waits for queued storage jobs before it writes the journal anyway.
const DRAIN_LIMIT: Duration = Duration::from_secs(3);

#[cfg(test)]
thread_local! {
    /// Seconds the tests have "idled" (added to the wall clock the policy reads).
    pub(crate) static CLOCK_SKEW: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

fn wall_secs() -> f64 {
    let t = crate::journal::now_ms() as f64 / 1000.0;
    #[cfg(test)]
    let t = t + CLOCK_SKEW.with(std::cell::Cell::get);
    t
}

struct AppHost<'a> {
    app: &'a mut EskizApp,
    storage: Option<&'a mut dyn eframe::Storage>,
    ctx: Option<egui::Context>,
}

impl Host for AppHost<'_> {
    fn stop_stroke(&mut self) {
        self.app.commit_stroke("the app is going to the background");
    }

    fn write_journal(&mut self) -> Result<(), String> {
        let app = &mut *self.app;
        let on_canvas = matches!(app.gal.screen, Screen::Canvas | Screen::Gallery) && app.three_d.is_none();
        if !on_canvas || !app.is_dirty() {
            return Ok(());
        }
        let (Some(lib), Some(cur)) = (&app.gal.library, app.gal.current.clone()) else {
            return Ok(());
        };
        let Some(id) = cur.id else {
            return Ok(());
        };
        let root = lib.recovery_root().to_owned();
        // Queued journal writes finish first, so two writers never share the folder.
        app.gal.drain_worker(DRAIN_LIMIT);
        let now = app.now;
        let Some(js) = app.gal.journal.as_mut().filter(|j| j.id == id) else {
            return Ok(());
        };
        let result = journal_sync(&mut js.planner, &app.session.doc, &root, &id, &cur.title);
        js.last_plan = now;
        js.changed_at = None;
        result
    }

    fn persist_prefs(&mut self) {
        if let Some(storage) = self.storage.as_deref_mut() {
            self.app.persist_prefs(storage);
        }
    }

    fn recreate_surface(&mut self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }

    fn reupload_layers(&mut self) {
        let Some(ctx) = self.ctx.clone() else {
            return;
        };
        if let Some(g) = self.app.gpu.as_mut() {
            g.canvas_mut().invalidate_mirrors();
        }
        self.app.sync_full(&ctx);
        ctx.request_repaint();
    }
}

impl EskizApp {
    /// Prefs, colours and brush choice into eframe's storage.
    pub(crate) fn persist_prefs(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(PREF_KEY, self.prefs.encode());
        self.colour_store.current = self.colour;
        storage.set_string(COLOUR_KEY, self.colour_store.encode());
        storage.set_string(BRUSH_KEY, self.brush_choice_text());
        if let Some(v) = self.tour.stored_value() {
            storage.set_string(crate::onboarding::KEY, crate::onboarding::encode(v));
        }
    }

    /// The activity is going to the background (eframe's `save`): see [`crate::lifecycle`].
    pub(crate) fn lifecycle_suspend(&mut self, storage: &mut dyn eframe::Storage) {
        let mut lc = std::mem::take(&mut self.lifecycle);
        lc.suspend(&mut AppHost { app: self, storage: Some(storage), ctx: None }, wall_secs());
        if let Some(e) = &lc.last_error {
            eprintln!("eskiz: journal write on suspend failed: {e}");
        }
        self.lifecycle = lc;
    }

    /// Back in the foreground without waiting for the gap heuristic (tests).
    #[cfg(test)]
    pub(crate) fn lifecycle_resume(&mut self, ctx: &egui::Context) {
        let mut lc = std::mem::take(&mut self.lifecycle);
        lc.resume(&mut AppHost { app: self, storage: None, ctx: Some(ctx.clone()) });
        self.lifecycle = lc;
    }

    /// Once per frame: the first frame after a trip to the background re-uploads the layers.
    pub(crate) fn lifecycle_frame(&mut self, ctx: &egui::Context) {
        let mut lc = std::mem::take(&mut self.lifecycle);
        let mut host = AppHost { app: self, storage: None, ctx: Some(ctx.clone()) };
        if lc.phase() == Phase::Suspended && !crate::android_queue::backgrounded() {
            // The activity is started again: this is the resume, however short the trip was.
            // It only marks layers for re-upload; the input of this frame is handled as usual.
            lc.resume(&mut host);
        } else if !crate::android_queue::backgrounded() {
            lc.frame(&mut host, wall_secs());
        }
        self.lifecycle = lc;
    }
}
