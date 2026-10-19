//! The per-frame work of the Android bridge (A4): messages from the Java side, the debug test
//! hooks, Back presses and the Back registration. On desktop the inbox stays empty and the Back
//! bridge ignores the calls, so `poll_bridge` does nothing there.

use eframe::egui;

use crate::app::EskizApp;
use crate::deliver::ShareMode;
use crate::file_access::PickPurpose;
use crate::saf::{self, Hook, HookFormat};

impl EskizApp {
    /// Drain the inbox, run hooks and Back presses, and tell Java whether Back is wanted. Once per frame.
    pub(crate) fn poll_bridge(&mut self, ctx: &egui::Context) {
        for text in self.inbox.take_notices() {
            self.show_toast(text, 4.0);
        }
        for text in self.inbox.take_hooks() {
            self.run_hook(ctx, &text);
        }
        for _ in 0..self.inbox.take_backs() {
            // A Back that finds nothing to close (it arrived a frame late) is ignored.
            self.system_back(ctx);
        }
        let wanted = self.back_step().is_some();
        if self.back_sent != Some(wanted) {
            self.back_sent = Some(wanted);
            self.back_bridge.set_wanted(wanted);
        }
    }

    /// Run one debug test hook (`--es eskiz_test <hook>`). Hooks reach here only from debuggable APKs.
    pub(crate) fn run_hook(&mut self, ctx: &egui::Context, text: &str) {
        let Some(hook) = saf::parse_hook(text) else {
            saf::log_line(&format!("eskiz-test: unknown hook {text}"));
            return;
        };
        saf::log_line(&format!("eskiz-test: run {text}"));
        match hook {
            Hook::OpenDocument => self.request_pick(PickPurpose::OpenDocument, None, None),
            Hook::ImportImage => self.request_pick(PickPurpose::ImportImage, None, None),
            Hook::ImportBrush => self.request_pick(PickPurpose::ImportBrush, None, None),
            Hook::GalleryImport => self.request_pick(PickPurpose::GalleryImport, None, None),
            Hook::ChooseFolder => self.request_pick(PickPurpose::ExportTree, None, None),
            Hook::Export { format, share } => {
                self.share_mode = if share { ShareMode::Share } else { ShareMode::SaveToFolder };
                match format {
                    HookFormat::Still(f) => self.start_export(ctx, f),
                    HookFormat::Anim(f) => self.start_anim_export(ctx, f),
                }
            }
            Hook::GalleryShareFirst => {
                let first = self.gal.entries.first().map(|e| e.id.clone());
                match first {
                    Some(id) => {
                        self.gal.share_target = Some(id);
                        self.gallery_share_sheet();
                    }
                    None => saf::log_line("eskiz-test: the gallery is empty"),
                }
            }
            Hook::CopyAll => self.clip_action(crate::selection_tool::ClipAction::CopyAll),
            Hook::Paste => self.clip_action(crate::selection_tool::ClipAction::Paste),
            Hook::BackState => {
                let state = self.back_step().map_or("none", |s| s.log_name());
                saf::log_line(&format!("back: state={state}"));
            }
            Hook::NewCanvas => {
                let screen = ctx.content_rect();
                self.new_canvas(0, 1024, 1024, egui::Rect::from_center_size(screen.center(), egui::vec2(120.0, 120.0)));
            }
        }
    }
}

