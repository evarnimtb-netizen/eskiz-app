//! Sheets: the panels that open over the canvas.
//!
//! * [`Sheet::Gestures`]: Preferences > Gesture controls. Each touch gesture on or off, plus the
//!   hold delays and "Disable touch painting" for pen-only use. Changes apply at once and are
//!   saved with the other preferences.
//! * [`Sheet::About`]: Help > About Eskiz. Version (from Cargo), the app icon, and a button to
//!   [`Sheet::Licenses`], the generated third-party notices (see `licenses.rs`).
//! * [`Sheet::Cheatsheet`]: Help > Gestures & shortcuts. Every gesture and key, with the touch
//!   gestures that are switched off marked "Off".
//! * [`Sheet::CopyMenu`]: the three-finger swipe. Copy, Cut, Copy all, Paste and Duplicate.
//!
//! Each sheet is a card in the middle of a dimmed window. A tap outside it, or Escape, closes it.

use eframe::egui::{self, Align2, Color32, CornerRadius, Id, Margin, Order, RichText, Sense, TextureHandle, Vec2};

use crate::app::{EskizApp, Prefs};
use crate::chrome::small_label;
use crate::icons;
use crate::icons_actions as ia;
use crate::shortcuts;
use crate::panels::rows::{list_row, Trailing};
use crate::selection_tool::ClipAction;
use crate::theme::{self, Palette};

/// The app icon, drawn in code by `packaging/icon-gen` and shipped as a PNG.
const ICON_PNG: &[u8] = include_bytes!("../../../packaging/icons/png/eskiz-256.png");

/// A sheet over the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    Gestures,
    About,
    /// About > Licenses: `THIRD_PARTY_NOTICES.md`, searchable.
    Licenses,
    Cheatsheet,
    CopyMenu,
}

/// One row of the cheat sheet: keys, what they do, and whether the gesture is switched on.
pub type CheatRow = (String, &'static str, bool);

/// The touch and key rows of the cheat sheet, marked off where the preferences switch them off.
pub fn cheat_rows(prefs: &Prefs, cmd: &str) -> Vec<(&'static str, Vec<CheatRow>)> {
    vec![
        (
            "TOUCH",
            vec![
                ("One-finger drag".to_owned(), "Paint with the current tool", prefs.touch_painting),
                ("Two-finger tap".to_owned(), "Undo", prefs.undo_gestures),
                ("Three-finger tap".to_owned(), "Redo", prefs.undo_gestures),
                ("Two-finger hold".to_owned(), "Repeat undo", prefs.repeat_undo),
                ("Three-finger scrub".to_owned(), "Clear layer", prefs.scrub_clear),
                ("Three-finger swipe".to_owned(), "Copy menu", prefs.swipe_copy),
                ("Four-finger tap".to_owned(), "Full screen", prefs.fullscreen_taps),
                (format!("Hold {:.1} s", prefs.eyedrop_secs), "Eyedropper", prefs.eyedrop_hold),
                (format!("Hold {:.2} s at a stroke's end", prefs.hold_shape_secs), "Hold to Shape", prefs.hold_shape),
                ("Two fingers".to_owned(), "Pan, zoom and rotate the view", prefs.touch_navigation),
            ],
        ),
        (
            "KEYBOARD",
            shortcuts::KEYBOARD.iter().map(|e| (shortcuts::display_keys(e, cmd), e.action, true)).collect(),
        ),
        (
            "CANVAS AND PEN",
            vec![
                ("Space + drag".to_owned(), "Pan", true),
                ("R + drag".to_owned(), "Rotate the view", true),
                ("Wheel, Ctrl + wheel".to_owned(), "Pan; with Ctrl, zoom", true),
                ("Alt + click, pen button".to_owned(), "Pick a color", true),
                ("Hold still at a stroke's end".to_owned(), "Hold to Shape", prefs.hold_shape),
            ],
        ),
    ]
}

/// The app icon texture, decoded once and kept in egui's temp memory.
fn icon_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let key = Id::new("about-icon");
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return Some(t);
    }
    let mut reader = png::Decoder::new(std::io::Cursor::new(ICON_PNG)).read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    let image = egui::ColorImage::from_rgba_unmultiplied([info.width as usize, info.height as usize], &buf);
    let tex = ctx.load_texture("about-icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    Some(tex)
}

impl EskizApp {
    /// Draw the open sheet, if any, and handle its taps.
    pub(crate) fn sheet_ui(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.sheet else {
            return;
        };
        let pal = self.pal;
        let screen = ctx.content_rect();
        let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        egui::Area::new(Id::new("sheet-scrim"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(140));
                if resp.clicked() {
                    close = true;
                }
            });
        let width: f32 = match sheet {
            Sheet::Gestures => 460.0_f32,
            Sheet::CopyMenu => 300.0_f32,
            Sheet::Licenses => 680.0_f32,
            _ => 420.0_f32,
        }
        .min(screen.width() - 32.0);
        let max_h = screen.height() * 0.85;
        let mut clip: Option<ClipAction> = None;
        let mut open_licenses = false;
        egui::Area::new(Id::new("sheet"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                theme::panel_frame(&pal).inner_margin(Margin::same(20)).show(ui, |ui| {
                    ui.set_width(width);
                    if sheet == Sheet::Licenses {
                        // Scrolls by itself (virtual rows), so no outer scroll area.
                        crate::licenses::body(ui, &pal, max_h);
                    } else {
                        egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| match sheet {
                            Sheet::Gestures => self.gesture_sheet(ui, &pal),
                            Sheet::About => {
                                if about_body(ui, ctx, &pal) {
                                    open_licenses = true;
                                }
                            }
                            Sheet::Cheatsheet => self.cheatsheet_body(ui, &pal),
                            Sheet::CopyMenu => clip = copy_menu_body(ui, &pal),
                            Sheet::Licenses => {}
                        });
                    }
                    ui.add_space(12.0);
                    if ui.add_sized([ui.available_width(), 36.0], egui::Button::new("Done")).clicked() {
                        close = true;
                    }
                });
            });
        if open_licenses {
            self.sheet = Some(Sheet::Licenses);
            return;
        }
        if let Some(action) = clip {
            self.clip_action(action);
            close = true;
        }
        if close {
            self.sheet = None;
        }
    }

    /// Preferences > Gesture controls. Every row writes straight into the preferences.
    fn gesture_sheet(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let p = &mut self.prefs;
        small_label(ui, pal, "PAINTING");
        // The row says "disable", so its switch is on when finger painting is off.
        let mut disabled = !p.touch_painting;
        toggle_row(
            ui,
            pal,
            ia::icon_fingers,
            "Disable touch painting",
            Some("Pen only: fingers never paint, but still navigate and undo"),
            &mut disabled,
            true,
        );
        p.touch_painting = !disabled;
        ui.add_space(8.0);
        small_label(ui, pal, "HOLD");
        toggle_row(ui, pal, ia::icon_eyedropper, "Eyedropper on hold", Some("Hold one finger still to pick a color"), &mut p.eyedrop_hold, true);
        slider_row(ui, pal, "Eyedropper delay", &mut p.eyedrop_secs, 0.1..=1.5, p.eyedrop_hold);
        toggle_row(ui, pal, ia::icon_hold_shape, "Hold to Shape", Some("Hold still at the end of a stroke to snap it"), &mut p.hold_shape, true);
        slider_row(ui, pal, "Hold to Shape delay", &mut p.hold_shape_secs, 0.2..=1.5, p.hold_shape);
        ui.add_space(8.0);
        small_label(ui, pal, "UNDO AND REDO");
        toggle_row(ui, pal, icons::icon_undo, "Two-finger tap undo", Some("Three-finger tap redoes"), &mut p.undo_gestures, true);
        toggle_row(ui, pal, ia::icon_clock, "Repeat undo on hold", Some("Hold two fingers to keep undoing"), &mut p.repeat_undo, true);
        ui.add_space(8.0);
        small_label(ui, pal, "THREE AND FOUR FINGERS");
        toggle_row(ui, pal, icons::icon_trash, "Three-finger scrub clears the layer", None, &mut p.scrub_clear, true);
        toggle_row(ui, pal, ia::icon_share, "Three-finger swipe shows the copy menu", None, &mut p.swipe_copy, true);
        toggle_row(ui, pal, ia::icon_fingers, "Four-finger tap toggles full screen", None, &mut p.fullscreen_taps, true);
        ui.add_space(8.0);
        small_label(ui, pal, "TWO FINGERS");
        toggle_row(ui, pal, ia::icon_fingers, "Pan, zoom and rotate", None, &mut p.touch_navigation, true);
    }

    fn cheatsheet_body(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let cmd = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" };
        for (title, rows) in cheat_rows(&self.prefs, cmd) {
            small_label(ui, pal, title);
            egui::Frame::new()
                .fill(pal.row)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 5.0;
                    for (keys, what, on) in rows {
                        ui.horizontal(|ui| {
                            let colour = if on { pal.text } else { pal.text_dim };
                            ui.label(RichText::new(what).size(12.5).color(colour));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(keys).size(12.0).color(pal.text_dim).monospace());
                                if !on {
                                    ui.label(RichText::new("Off").size(11.0).color(pal.danger));
                                }
                            });
                        });
                    }
                });
            ui.add_space(6.0);
        }
    }
}

/// A switch row. A tap flips `value`.
fn toggle_row(ui: &mut egui::Ui, pal: &Palette, icon: icons::IconFn, label: &str, sub: Option<&str>, value: &mut bool, enabled: bool) {
    if list_row(ui, pal, icon, label, sub, enabled, Trailing::Switch(*value)).clicked() {
        *value = !*value;
    }
}

/// A labelled slider, in seconds, for a delay.
fn slider_row(ui: &mut egui::Ui, pal: &Palette, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, enabled: bool) {
    ui.add_enabled_ui(enabled, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(label).size(12.5).color(pal.text_dim));
        });
        ui.add(egui::Slider::new(value, range).suffix(" s").fixed_decimals(2).step_by(0.05));
    });
}

/// Help > About Eskiz. Returns true when "Licenses and source" was pressed.
fn about_body(ui: &mut egui::Ui, ctx: &egui::Context, pal: &Palette) -> bool {
    ui.vertical_centered(|ui| {
        if let Some(tex) = icon_texture(ctx) {
            ui.add(egui::Image::new((tex.id(), Vec2::splat(96.0))));
        }
        ui.add_space(6.0);
        ui.label(RichText::new("Eskiz").size(26.0).strong().color(pal.text));
        ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(13.0).color(pal.text_dim));
        ui.add_space(4.0);
        ui.label(RichText::new("A painting app for touch, pen and mouse, made in Rust.").size(13.0).color(pal.text));
    });
    ui.add_space(12.0);
    small_label(ui, pal, "LICENSES");
    ui.label(
        RichText::new(
            "Eskiz's own code is MIT OR Apache-2.0. It includes third-party software and fonts under their own licenses.",
        )
        .size(12.0)
        .color(pal.text_dim),
    );
    ui.add_space(6.0);
    ui.add_sized([ui.available_width(), 34.0], egui::Button::new("Licenses and source")).clicked()
}

/// The three-finger swipe menu. Returns the action picked.
fn copy_menu_body(ui: &mut egui::Ui, pal: &Palette) -> Option<ClipAction> {
    let _ = pal;
    let mut picked = None;
    for (label, action) in [
        ("Copy", ClipAction::Copy),
        ("Cut", ClipAction::Cut),
        ("Copy all", ClipAction::CopyAll),
        ("Paste", ClipAction::Paste),
        ("Duplicate", ClipAction::Duplicate),
    ] {
        if ui.add_sized([ui.available_width(), 38.0], egui::Button::new(label)).clicked() {
            picked = Some(action);
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::snapshot::{snapshot_dir, Headless};
    use eframe::egui::Vec2;

    #[test]
    fn writes_sheet_snapshots() {
        let mut h = Headless::new(Vec2::new(900.0, 900.0), Document::new(400, 300), Prefs::default());
        for (name, sheet) in [
            ("sheet_gestures.png", Sheet::Gestures),
            ("sheet_cheatsheet.png", Sheet::Cheatsheet),
            ("sheet_about.png", Sheet::About),
            ("sheet_licenses.png", Sheet::Licenses),
        ] {
            h.app.sheet = Some(sheet);
            h.advance(0.3);
            h.render().save_png(&snapshot_dir().join(name)).unwrap();
        }
    }

    #[test]
    fn cheat_sheet_marks_switched_off_gestures() {
        let off = Prefs { scrub_clear: false, undo_gestures: false, ..Prefs::default() };
        let rows = cheat_rows(&off, "Ctrl");
        let touch = &rows[0].1;
        let scrub = touch.iter().find(|r| r.0 == "Three-finger scrub").unwrap();
        assert!(!scrub.2, "a disabled gesture is marked off");
        let undo = touch.iter().find(|r| r.0 == "Two-finger tap").unwrap();
        assert!(!undo.2);
        assert!(touch.iter().any(|r| r.0 == "Four-finger tap" && r.2), "others stay on");
    }

    #[test]
    fn about_has_the_app_icon() {
        assert!(!ICON_PNG.is_empty());
    }
}
