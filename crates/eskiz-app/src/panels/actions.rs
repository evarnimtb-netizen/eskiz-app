//! Actions panel: Add, Canvas, Share, Prefs and Help tabs. Each tab is a
//! list of rows with an icon and a label (see `rows.rs`). The tab strip stays put and the list
//! scrolls, so the panel never grows past the window.

use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText};

use eskiz_core::ColourSpace;

use crate::app::EskizApp;
use crate::chrome::{small_label, title_row};
use crate::export::{expand_home, ExportFormat};
use crate::file_access::PickPurpose;
use crate::icons;
use crate::icons_actions as ia;
use crate::panels::rows::{hint, list_row, tab_bar, Trailing};

/// The tabs of the Actions panel, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActionsTab {
    #[default]
    Add,
    Canvas,
    ThreeD,
    Share,
    Prefs,
    Help,
}

impl ActionsTab {
    pub const ALL: [ActionsTab; 6] = [Self::Add, Self::Canvas, Self::ThreeD, Self::Share, Self::Prefs, Self::Help];

    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Canvas => "Canvas",
            Self::ThreeD => "3D",
            Self::Share => "Share",
            Self::Prefs => "Settings",
            Self::Help => "Help",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// State of the Actions panel that is not a setting: the open tab and the Add tab's path field.
#[derive(Default)]
pub struct ActionsUi {
    pub tab: ActionsTab,
    pub import_path: String,
}

/// The icon and row title for one export format on the Share sheet.
fn share_row(format: ExportFormat) -> (icons::IconFn, &'static str) {
    match format {
        ExportFormat::Procreate => (icons::icon_layers, "Layered .procreate"),
        ExportFormat::Psd => (ia::icon_file_image, "Photoshop PSD, layers kept"),
        ExportFormat::Pdf => (ia::icon_file_image, "PDF, flattened"),
        ExportFormat::Png => (ia::icon_image, "PNG"),
        ExportFormat::Jpeg => (ia::icon_image, "JPEG"),
        ExportFormat::Tiff => (ia::icon_image, "TIFF"),
        ExportFormat::WebP => (ia::icon_image, "WebP"),
        ExportFormat::Layers => (ia::icon_folder, "Each layer as a PNG in a folder"),
    }
}

/// Modifier name for shortcuts on this platform.
fn cmd_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

impl EskizApp {
    pub(crate) fn actions_body(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        title_row(ui, "Actions");
        let tabs: Vec<(icons::IconFn, &str)> = ActionsTab::ALL
            .iter()
            .map(|t| {
                let icon: icons::IconFn = match t {
                    ActionsTab::Add => icons::icon_plus,
                    ActionsTab::Canvas => ia::icon_canvas,
                    ActionsTab::ThreeD => icons::icon_layers,
                    ActionsTab::Share => ia::icon_share,
                    ActionsTab::Prefs => ia::icon_prefs,
                    ActionsTab::Help => ia::icon_help,
                };
                (icon, t.label())
            })
            .collect();
        if let Some(i) = tab_bar(ui, &pal, &tabs, self.actions_ui.tab.index()) {
            self.actions_ui.tab = ActionsTab::ALL[i];
        }
        ui.add_space(8.0);
        let tab = self.actions_ui.tab;
        egui::ScrollArea::vertical()
            .id_salt(("actions-tab", tab.index()))
            .auto_shrink([false, true])
            .max_height(ui.available_height())
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                match tab {
                    ActionsTab::Add => self.actions_add(ui),
                    ActionsTab::Canvas => self.actions_canvas(ui),
                    ActionsTab::ThreeD => self.three_d_settings_body(ui),
                    ActionsTab::Share => self.actions_share(ui),
                    ActionsTab::Prefs => self.actions_prefs(ui),
                    ActionsTab::Help => self.actions_help(ui),
                }
            });
    }

    /// A rounded card holding a text field, an optional "Browse..." button and its action button.
    /// Returns `(go, browse)`: the typed path was submitted, or Browse was clicked.
    pub(crate) fn path_field(ui: &mut egui::Ui, pal: &crate::theme::Palette, text: &mut String, hint_text: &str, button: &str, enabled: bool, browse: bool) -> (bool, bool) {
        let mut go = false;
        let mut browse_clicked = false;
        egui::Frame::new()
            .fill(pal.row)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let button_w = 70.0;
                        let edit = egui::TextEdit::singleline(text)
                            .hint_text(hint_text)
                            .desired_width(ui.available_width() - button_w - 8.0 - 16.0)
                            .margin(Margin::symmetric(8, 6));
                        let resp = ui.add(edit);
                        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let click = ui
                            .add_enabled(enabled, egui::Button::new(button).min_size(egui::vec2(button_w, 32.0)))
                            .clicked();
                        go = enabled && (click || enter);
                    });
                    if browse {
                        browse_clicked = ui.add(egui::Button::new("Browse...").min_size(egui::vec2(100.0, 28.0))).clicked();
                    }
                });
            });
        (go, browse_clicked)
    }

    fn actions_add(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        list_row(
            ui,
            &pal,
            ia::icon_image,
            "Import",
            Some("PNG, JPEG, WebP, PSD or .procreate"),
            true,
            Trailing::None,
        );
        let mut path = std::mem::take(&mut self.actions_ui.import_path);
        let enabled = !path.trim().is_empty();
        let (go, browse) = Self::path_field(ui, &pal, &mut path, "Image, PSD or .procreate path", "Import", enabled, true);
        self.actions_ui.import_path = path;
        if browse {
            let typed = self.actions_ui.import_path.clone();
            self.request_pick_near(PickPurpose::ImportImage, &typed, None);
        }
        if go {
            let file = expand_home(&self.actions_ui.import_path);
            self.start_import_image(&ctx, &file);
        }
        hint(ui, &pal, "You can also drop a file onto the canvas. Pictures larger than the canvas are scaled to fit.");
        ui.add_space(4.0);
        if list_row(ui, &pal, ia::icon_text, "Add text", Some("A text layer at the center; type to edit"), self.three_d.is_none(), Trailing::None).clicked() {
            self.add_text_action();
        }
        ui.add_space(4.0);
        list_row(ui, &pal, ia::icon_photo, "Insert photo", Some("Needs a camera or photo library"), false, Trailing::Text("Soon"));
        ui.add_space(4.0);
        ui.add_space(4.0);
        list_row(ui, &pal, ia::icon_folder, "Import font", Some("A .ttf, .otf or .ttc file, kept with Eskiz"), true, Trailing::None);
        let mut font = std::mem::take(&mut self.text.font_path);
        let enabled = !font.trim().is_empty();
        let (go, browse) = Self::path_field(ui, &pal, &mut font, "Path to a .ttf, .otf or .ttc file", "Import font", enabled, true);
        self.text.font_path = font;
        if browse {
            let typed = self.text.font_path.clone();
            self.request_pick_near(PickPurpose::ImportFont, &typed, None);
        }
        if go {
            let file = expand_home(&self.text.font_path);
            self.import_font_action(&file);
        }
        ui.add_space(4.0);
        list_row(ui, &pal, ia::icon_folder, "Open a .procreate file", Some("Replaces the current drawing"), true, Trailing::None);
        let mut open = std::mem::take(&mut self.open_path);
        let enabled = !open.trim().is_empty();
        let (go, browse) = Self::path_field(ui, &pal, &mut open, "Path to a .procreate file", "Open", enabled, true);
        self.open_path = open;
        if browse {
            let typed = self.open_path.clone();
            self.request_pick_near(PickPurpose::OpenDocument, &typed, None);
        }
        if go {
            let file = expand_home(&self.open_path);
            self.load_document(&ctx, &file);
        }
    }

    /// Flip every layer, as one undo step.
    pub(crate) fn flip_canvas_action(&mut self, horizontal: bool) {
        if self.adjust.is_some() || self.session.is_stroking() {
            self.show_toast("Finish what you are doing first.", 2.0);
            return;
        }
        let name = if horizontal { "Flip Canvas Horizontal" } else { "Flip Canvas Vertical" };
        if let Some(edit) = self.session.flip_canvas(horizontal) {
            self.apply_refresh(edit.refresh);
            self.show_toast(name, 1.2);
        }
    }

    /// Convert the document to `target` (Actions > Canvas > Convert to ...), as one undo step.
    pub(crate) fn convert_colour_space_action(&mut self, target: ColourSpace) {
        if self.adjust.is_some() || self.session.is_stroking() {
            self.show_toast("Finish what you are doing first.", 2.0);
            return;
        }
        let Some(op) = crate::colour_management::plan_convert(&self.session.doc, target) else {
            self.show_toast(format!("Already {}", target.label()), 1.5);
            return;
        };
        let name = match target {
            ColourSpace::Srgb => "Convert to sRGB",
            ColourSpace::DisplayP3 => "Convert to Display P3",
        };
        if let Some(edit) = self.session.apply_doc_op(name, Box::new(op)) {
            self.apply_refresh(edit.refresh);
            self.show_toast(name, 1.5);
        }
    }

    /// Convert the document to `target` bits per channel (Actions > Canvas > Convert to 16-bit /
    /// 8-bit), as one undo step. Going down dithers, so gradients do not band.
    pub(crate) fn convert_depth_action(&mut self, target: eskiz_core::Depth) {
        if self.adjust.is_some() || self.session.is_stroking() {
            self.show_toast("Finish what you are doing first.", 2.0);
            return;
        }
        let Some(op) = crate::bit_depth::plan_convert_depth(&self.session.doc, target, true) else {
            self.show_toast(format!("Already {}", target.label()), 1.5);
            return;
        };
        let name = match target {
            eskiz_core::Depth::U8 => "Convert to 8-bit",
            eskiz_core::Depth::U16 => "Convert to 16-bit",
        };
        if let Some(edit) = self.session.apply_doc_op(name, Box::new(op)) {
            self.needs_full = true;
            self.apply_refresh(edit.refresh);
            self.show_toast(name, 1.5);
        }
    }

    fn actions_canvas(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let (w, h, layers, dpi, space, depth) = {
            let d = &self.session.doc;
            (d.width, d.height, d.layer_count(), d.dpi, d.colour_space, d.depth)
        };
        egui::Frame::new()
            .fill(pal.row)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().interact_size.y = 20.0;
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
                    ia::icon_info(&ui.painter_at(r), r, pal.text);
                    ui.label(RichText::new("Canvas information").size(14.0));
                });
                ui.add_space(2.0);
                for (label, value) in [
                    ("Size", format!("{w} \u{d7} {h} px")),
                    ("Layers", layers.to_string()),
                    ("Resolution", format!("{dpi} DPI")),
                    ("Color space", space.label().to_owned()),
                    ("Color depth", depth.label().to_owned()),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).size(12.5).color(pal.text_dim));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(value).size(12.5));
                        });
                    });
                }
            });
        let busy = self.adjust.is_some() || self.session.is_stroking();
        if list_row(ui, &pal, ia::icon_flip_h, "Flip canvas horizontally", Some("Mirrors every layer"), !busy, Trailing::None).clicked() {
            self.flip_canvas_action(true);
        }
        if list_row(ui, &pal, ia::icon_flip_v, "Flip canvas vertically", Some("Mirrors every layer"), !busy, Trailing::None).clicked() {
            self.flip_canvas_action(false);
        }
        // Convert the pixels to the other colour space. One undo step.
        let other = match space {
            ColourSpace::Srgb => ColourSpace::DisplayP3,
            ColourSpace::DisplayP3 => ColourSpace::Srgb,
        };
        let convert_label = format!("Convert to {}", other.label());
        let convert_hint = format!("Re-encode every color from {} to {}", space.label(), other.label());
        if list_row(ui, &pal, ia::icon_canvas, &convert_label, Some(convert_hint.as_str()), !busy && self.three_d.is_none(), Trailing::None).clicked() {
            self.convert_colour_space_action(other);
        }
        // 8-bit and 16-bit canvases. One undo step; going down dithers.
        let other_depth = crate::bit_depth::other(depth);
        let depth_label = format!("Convert to {}", other_depth.label());
        let depth_hint = match other_depth {
            eskiz_core::Depth::U16 => "Keep 16 bits per channel: smooth gradients, no banding",
            eskiz_core::Depth::U8 => "Back to 8 bits per channel, dithered",
        };
        if list_row(ui, &pal, ia::icon_canvas, &depth_label, Some(depth_hint), !busy && self.three_d.is_none(), Trailing::None).clicked() {
            self.convert_depth_action(other_depth);
        }
        let crop_ok = !busy && self.three_d.is_none();
        if list_row(ui, &pal, ia::icon_canvas, "Crop & Resize", Some("Crop, resize or anchor the canvas"), crop_ok, Trailing::None).clicked() {
            self.open_crop();
        }
        if list_row(ui, &pal, ia::icon_info, "Canvas information", Some("Size, DPI, layers, time spent and strokes"), true, Trailing::None).clicked() {
            self.canvas_info_open = true;
        }
        let guide_on = self.guides.settings.visible;
        if list_row(ui, &pal, ia::icon_canvas, "Drawing guide", Some("Lines over the canvas"), true, Trailing::Switch(guide_on)).clicked() {
            self.guides.settings.visible = !guide_on;
        }
        if list_row(ui, &pal, ia::icon_canvas, "Edit drawing guide", Some("Type, spacing, symmetry, Guided Layer"), true, Trailing::None).clicked() {
            self.guides.editor_open = true;
        }
        // The reference window: a floating view of the canvas or a picture.
        let ctx = ui.ctx().clone();
        let open = self.reference.open;
        if list_row(ui, &pal, ia::icon_reference, "Reference", Some("A floating view of the canvas or a picture"), true, Trailing::Switch(open)).clicked() {
            self.toggle_reference();
        }
        let mut path = std::mem::take(&mut self.reference.path);
        let enabled = !path.trim().is_empty();
        let (go, browse) = Self::path_field(ui, &pal, &mut path, "Path to a picture for the reference", "Show", enabled, true);
        self.reference.path = path;
        if browse {
            let typed = self.reference.path.clone();
            self.request_pick_near(PickPurpose::Reference, &typed, None);
        }
        if go {
            let file = expand_home(&self.reference.path);
            self.load_reference_image(&ctx, &file);
        }
        self.canvas_animation_rows(ui);
        list_row(ui, &pal, ia::icon_crop, "Crop and resize", Some("Arrives in a later update"), false, Trailing::Text("Soon"));
    }

    fn actions_share(&mut self, ui: &mut egui::Ui) {
        if self.three_d.is_some() {
            self.three_d_share_body(ui);
            return;
        }
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        small_label(ui, &pal, "SAVE TO");
        let mut choose = None;
        let mut path = std::mem::take(&mut self.export_path);
        egui::Frame::new()
            .fill(pal.row)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::TextEdit::singleline(&mut path)
                        .hint_text("~/eskiz_export")
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(8, 6)),
                );
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Choose...").min_size(egui::vec2(100.0, 28.0))).on_hover_text("Pick where to save; each format below adds its own extension").clicked() {
                        choose = Some(PickPurpose::ExportBase);
                    }
                    if ui.add(egui::Button::new("Choose folder...").min_size(egui::vec2(120.0, 28.0))).on_hover_text("Pick a folder, for layer PNGs").clicked() {
                        choose = Some(PickPurpose::ExportFolder);
                    }
                });
            });
        self.export_path = path;
        if let Some(purpose) = choose {
            let typed = self.export_path.clone();
            let name = expand_home(&typed).file_name().map(|n| n.to_string_lossy().into_owned());
            self.request_pick_near(purpose, &typed, name);
        }
        let busy = self.share_busy();
        self.share_options(ui);
        let base = expand_home(&self.export_path);
        let mut chosen = None;
        for format in ExportFormat::ALL {
            let (icon, name) = share_row(format);
            let file = if format.is_folder() {
                base.display().to_string()
            } else {
                base.with_extension(format.extension()).display().to_string()
            };
            if list_row(ui, &pal, icon, name, Some(&file), !busy, Trailing::None).clicked() {
                chosen = Some(format);
            }
        }
        self.share_animation_rows(ui);
        if let Some(format) = chosen {
            self.start_export(&ctx, format);
        }
        if let Some(format) = self.share.exporting() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("Writing {}...", format.label())).size(12.5).color(pal.text_dim));
            });
        }
        hint(ui, &pal, "Transparent background keeps alpha in PNG, TIFF, WebP and PDF. JPEG is flattened onto the paper color. .procreate and PSD keep every layer. Export runs in the background.");
    }

    /// The Share sheet's options: scale, DPI, transparency, JPEG quality and PDF pages.
    fn share_options(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let settings = &mut self.share.settings;
        egui::Frame::new()
            .fill(pal.row)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().interact_size.y = 22.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Scale").size(12.5).color(pal.text_dim));
                    ui.add(egui::DragValue::new(&mut settings.scale_percent).range(1.0..=400.0).suffix(" %").speed(1.0));
                    ui.add_space(12.0);
                    ui.label(RichText::new("DPI").size(12.5).color(pal.text_dim));
                    ui.add(egui::DragValue::new(&mut settings.dpi).range(1.0..=2400.0).speed(1.0));
                });
                crate::theme::checkbox(ui, &mut settings.transparent, "Transparent background");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("JPEG quality").size(12.5).color(pal.text_dim));
                    ui.add(egui::Slider::new(&mut settings.jpeg_quality, 0..=100));
                });
                crate::theme::checkbox(ui, &mut settings.pdf_per_layer, "PDF with one page per layer");
            });
    }

    fn actions_prefs(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let prefs = &mut self.prefs;
        for (icon, label, sub, value) in [
            (ia::icon_sun as icons::IconFn, "Light interface", None, &mut prefs.light),
            (ia::icon_sidebar_right, "Right-hand interface", Some("Sidebar on the right"), &mut prefs.sidebar_right),
            (ia::icon_cursor, "Brush cursor", Some("Outline of the brush under the pen"), &mut prefs.brush_cursor),
        ] {
            if list_row(ui, &pal, icon, label, sub, true, Trailing::Switch(*value)).clicked() {
                *value = !*value;
            }
        }
        let mut eskiz = prefs.identity == crate::theme::UiIdentity::Eskiz;
        if list_row(ui, &pal, ia::icon_eskiz_look, "Eskiz look", Some("New colors and typeface"), true, Trailing::Switch(eskiz)).clicked() {
            eskiz = !eskiz;
            prefs.identity = if eskiz { crate::theme::UiIdentity::Eskiz } else { crate::theme::UiIdentity::Classic };
        }
        let mut again = self.tour.show_again();
        if list_row(ui, &pal, ia::icon_help, crate::onboarding::copy::AGAIN_SWITCH, Some(crate::onboarding::copy::AGAIN_SWITCH_SUB), true, Trailing::Switch(again)).clicked() {
            again = !again;
            self.tour.set_show_again(again);
        }
        let prefs = &mut self.prefs;
        ui.add_space(6.0);
        small_label(ui, &pal, "GESTURES");
        for (icon, label, sub, value) in [
            (ia::icon_eyedropper as icons::IconFn, "Eyedropper on hold", Some("Hold a finger still to pick a color"), &mut prefs.eyedrop_hold),
            (ia::icon_hold_shape, "Hold to Shape", Some("Hold at the end of a stroke to snap it"), &mut prefs.hold_shape),
            (icons::icon_undo, "Undo and redo gestures", Some("Two and three finger taps"), &mut prefs.undo_gestures),
            (ia::icon_clock, "Repeat undo on hold", Some("Hold two fingers to keep undoing"), &mut prefs.repeat_undo),
            (ia::icon_fingers, "Two-finger pan, zoom, rotate", None, &mut prefs.touch_navigation),
        ] {
            if list_row(ui, &pal, icon, label, sub, true, Trailing::Switch(*value)).clicked() {
                *value = !*value;
            }
        }
        if list_row(ui, &pal, ia::icon_fingers, "Gesture controls", Some("Turn each touch gesture on or off, and set the delays"), true, Trailing::Chevron).clicked() {
            self.popovers.close();
            self.sheet = Some(crate::sheets::Sheet::Gestures);
        }
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(pal.row)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                self.pressure_curve_editor(ui);
            });
    }

    fn actions_help(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        list_row(
            ui,
            &pal,
            ia::icon_info,
            &format!("Eskiz {}", env!("CARGO_PKG_VERSION")),
            Some("A painting app, made in Rust"),
            true,
            Trailing::None,
        );
        if list_row(ui, &pal, ia::icon_help, "User guide", Some("Chapters on painting, layers, color, export and more"), true, Trailing::Chevron).clicked() {
            self.popovers.close();
            self.open_user_guide(crate::user_guide::ch::GETTING_STARTED);
        }
        if list_row(ui, &pal, ia::icon_help, crate::onboarding::copy::HELP_ROW, Some(crate::onboarding::copy::HELP_ROW_SUB), true, Trailing::Chevron).clicked() {
            let ctx = ui.ctx().clone();
            self.replay_tour(&ctx);
        }
        if list_row(ui, &pal, ia::icon_info, "About Eskiz", Some("Version, licenses"), true, Trailing::Chevron).clicked() {
            self.popovers.close();
            self.sheet = Some(crate::sheets::Sheet::About);
        }
        if list_row(ui, &pal, ia::icon_help, "Gestures & shortcuts", Some("Every gesture and key, with the off ones marked"), true, Trailing::Chevron).clicked() {
            self.popovers.close();
            self.sheet = Some(crate::sheets::Sheet::Cheatsheet);
        }
        ui.add_space(4.0);
        let cmd = cmd_key();
        let sections: [(&str, Vec<(String, &str)>); 3] = [
            (
                "SHORTCUTS",
                crate::shortcuts::KEYBOARD.iter().map(|e| (crate::shortcuts::display_keys(e, cmd), e.action)).collect(),
            ),
            (
                "CANVAS",
                vec![
                    ("Space + drag".to_owned(), "Pan"),
                    ("R + drag".to_owned(), "Rotate the view"),
                    ("Wheel".to_owned(), "Pan; with Ctrl, zoom"),
                    ("Alt + click".to_owned(), "Pick a color"),
                ],
            ),
            (
                "TOUCH",
                vec![
                    ("Two-finger tap".to_owned(), "Undo"),
                    ("Three-finger tap".to_owned(), "Redo"),
                    ("Four-finger tap".to_owned(), "Hide the interface"),
                    ("Three-finger scrub".to_owned(), "Clear layer"),
                    ("Hold still".to_owned(), "Eyedropper"),
                ],
            ),
        ];
        for (title, rows) in sections {
            small_label(ui, &pal, title);
            egui::Frame::new()
                .fill(pal.row)
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.spacing_mut().interact_size.y = 18.0;
                    for (keys, what) in rows {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(what).size(12.5));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(keys).size(12.0).color(pal.text_dim).monospace());
                            });
                        });
                    }
                });
        }
    }
}
