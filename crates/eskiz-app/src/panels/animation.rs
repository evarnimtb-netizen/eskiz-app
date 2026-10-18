//! The Frames timeline (bottom strip), its settings window, and the Share and Canvas rows for
//! animation exports and timelapse. The state and the work are in [`crate::animation_ui`] and
//! [`crate::timelapse`].

use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Sense, Stroke, Vec2};

use crate::animation_ui::{AnimExportOptions, AnimFormat};
use crate::app::EskizApp;
use crate::chrome::small_label;
use crate::export::expand_home;
use crate::icons::IconFn;
use crate::icons_actions as ia;
use crate::panels::rows::{list_row, Trailing};
use eskiz_anim::{Playback, PlaybackDirection, PlaybackMode};
use eskiz_core::animation::OnionBlend;
use eskiz_media::Quality;

/// Width of a timeline thumbnail, in points.
const THUMB_W: f32 = 84.0;

fn mode_name(mode: PlaybackMode) -> &'static str {
    match mode {
        PlaybackMode::Loop => "Loop",
        PlaybackMode::PingPong => "Ping-pong",
        PlaybackMode::OneShot => "One-shot",
    }
}

/// A thumbnail of an opaque RGBA image, nearest-neighbour sampled to `box_w` wide.
fn thumb_image(rgba: &[u8], w: usize, h: usize, box_w: usize) -> egui::ColorImage {
    let tw = box_w.clamp(1, w.max(1));
    let th = (h * tw / w.max(1)).max(1);
    let mut out = vec![0u8; tw * th * 4];
    for y in 0..th {
        let sy = (y * h / th).min(h.saturating_sub(1));
        for x in 0..tw {
            let sx = (x * w / tw).min(w.saturating_sub(1));
            let s = (sy * w + sx) * 4;
            let d = (y * tw + x) * 4;
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([tw, th], &out)
}

impl EskizApp {
    /// Draws the Frames timeline and settings. Called before the canvas, so the panel takes its
    /// space first. Also reports a finished animation export, whether or not the feature is on.
    pub(crate) fn animation_panel(&mut self, ui: &mut egui::Ui) {
        self.poll_anim_export();
        if self.three_d.is_some() {
            // The 3D view has no frames to play and no onion skins: stop playback and hide the strip.
            self.anim_pause();
            return;
        }
        if !self.session.doc.animation.frames_enabled {
            return;
        }
        let ctx = ui.ctx().clone();
        self.anim_sync(&ctx);
        self.anim_thumbs(&ctx);
        let pal = self.pal;
        // The tool sidebar sits on the left edge unless it is moved right, so the strip starts clear of it.
        let left = if self.prefs.sidebar_right { 12 } else { 64 };
        let margin = egui::Margin { left, right: 12, top: 8, bottom: 8 };
        egui::Panel::bottom("animation_timeline")
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.panel).inner_margin(margin))
            .show(ui, |ui| {
                self.timeline_controls(ui);
                ui.add_space(4.0);
                self.timeline_strip(ui);
            });
        self.animation_settings_window(&ctx);
    }

    fn timeline_controls(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let playing = self.anim.playback.as_ref().is_some_and(Playback::is_playing);
        let frames = self.anim.cache.as_ref().map_or(0, |c| c.frames.len());
        ui.horizontal(|ui| {
            // A fixed-width slot: Play and Pause differ in width, which used to shift Add frame and Settings.
            if ui.add(egui::Button::new(if playing { "Pause" } else { "Play" }).min_size(Vec2::new(64.0, 0.0))).clicked() {
                if playing {
                    self.anim_pause();
                } else {
                    self.anim_play();
                }
            }
            if ui.button("Add frame").on_hover_text("Adds a layer above the stack, a new frame").clicked() {
                if let Some(edit) = self.session.add_layer(None) {
                    if let Some(id) = edit.id {
                        self.active = id;
                    }
                }
            }
            if ui.button("Settings").clicked() {
                self.anim.settings_open = !self.anim.settings_open;
            }
            let s = &self.session.doc.animation;
            ui.label(
                RichText::new(format!(
                    "Frame {} of {}   {} fps   {}",
                    if frames == 0 { 0 } else { self.anim.frame + 1 },
                    frames,
                    s.fps(),
                    mode_name(s.playback_mode)
                ))
                .size(12.5)
                .color(pal.text_dim),
            );
            // The hold of the selected frame, stored on its bottom item. Always shown (same place, same label);
            // disabled while no frame is selected.
            let node = self.anim.selected.filter(|&i| i < frames).and_then(|i| self.anim_frame_node(i));
            let mut hold = node.and_then(|id| self.session.doc.node(id)).map_or(0, |n| n.props.hold);
            let before = hold;
            ui.add_space(12.0);
            ui.add_enabled(node.is_some(), egui::DragValue::new(&mut hold).range(0..=30).prefix("Hold: ").speed(0.2))
                .on_hover_text(if node.is_some() { "Extra frames the selected frame is held" } else { "Select a frame to set how long it is held" });
            if let (Some(id), true) = (node, hold != before) {
                self.session.set_props(id, "Change hold", |p| p.hold = hold);
            }
        });
    }

    fn timeline_strip(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let Some(cache) = self.anim.cache.as_ref() else {
            ui.label("No frames: make a layer visible.");
            return;
        };
        if cache.frames.is_empty() {
            ui.label(RichText::new("No frames: make a layer visible or leave the background out.").color(pal.text_dim));
            return;
        }
        let (w, h) = cache.frames.size();
        let thumb_h = THUMB_W * h as f32 / w.max(1) as f32;
        let mut clicked = None;
        egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
            ui.horizontal(|ui| {
                for i in 0..cache.frames.len() {
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(THUMB_W, thumb_h + 18.0), Sense::click());
                    let image_rect = Rect::from_min_size(rect.min, Vec2::new(THUMB_W, thumb_h));
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(image_rect, CornerRadius::same(6), pal.row);
                    if let Some(Some(tex)) = self.anim.thumbs.get(i) {
                        painter.image(
                            tex.id(),
                            image_rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    let current = i == self.anim.frame;
                    let stroke = if current {
                        Stroke::new(2.0, pal.accent)
                    } else {
                        Stroke::new(1.0, pal.hairline)
                    };
                    painter.rect_stroke(image_rect, CornerRadius::same(6), stroke, egui::StrokeKind::Inside);
                    let hold = cache.frames.hold(i);
                    painter.text(
                        egui::pos2(rect.min.x + 6.0, image_rect.max.y + 3.0),
                        egui::Align2::LEFT_TOP,
                        format!("{}", i + 1),
                        egui::FontId::proportional(12.0),
                        pal.text_dim,
                    );
                    if hold > 0 {
                        painter.text(
                            egui::pos2(image_rect.max.x - 4.0, image_rect.max.y + 3.0),
                            egui::Align2::RIGHT_TOP,
                            format!("+{hold}"),
                            egui::FontId::proportional(12.0),
                            pal.accent,
                        );
                    }
                    if response.clicked() {
                        clicked = Some(i);
                    }
                }
            });
        });
        if let Some(i) = clicked {
            self.anim_show(i);
        }
    }

    /// Draws up to [`THUMBS_PER_FRAME`] missing thumbnails, and asks for more frames until all exist.
    /// Timeline thumbnails. Each is a full composite of its frame, so they are rendered on a
    /// worker thread (one job per frame cache, in order) and only the small result image becomes a
    /// texture here. Toggling the timeline on therefore costs the frame list and the canvas view,
    /// not a composite per thumbnail on the UI thread.
    pub(crate) fn anim_thumbs(&mut self, ctx: &egui::Context) {
        let Some(cache) = self.anim.cache.as_ref() else {
            return;
        };
        let edits = cache.edits;
        if let Some(rx) = &self.anim.thumb_rx {
            while let Ok((job_edits, i, image)) = rx.try_recv() {
                if job_edits != edits {
                    continue;
                }
                let tex = ctx.load_texture(format!("anim_thumb_{i}"), image, egui::TextureOptions::LINEAR);
                if let Some(slot) = self.anim.thumbs.get_mut(i) {
                    *slot = Some(tex);
                }
            }
        }
        let missing: Vec<usize> = (0..cache.frames.len())
            .filter(|&i| self.anim.thumbs.get(i).is_some_and(Option::is_none))
            .collect();
        if missing.is_empty() || self.anim.thumb_job == Some(edits) {
            return;
        }
        let frames = cache.frames.clone();
        let (w, h) = frames.size();
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            for i in missing {
                let rgba = frames.render(i);
                let image = thumb_image(&rgba, w, h, (THUMB_W * 2.0) as usize);
                // The receiver is gone when the document changed: stop early.
                if tx.send((edits, i, image)).is_err() {
                    return;
                }
                repaint.request_repaint();
            }
        });
        // Replacing the receiver ends any older job at its next send.
        self.anim.thumb_rx = Some(rx);
        self.anim.thumb_job = Some(edits);
    }

    fn animation_settings_window(&mut self, ctx: &egui::Context) {
        if !self.anim.settings_open {
            return;
        }
        let before = self.session.doc.animation.clone();
        let mut s = before.clone();
        let bottom = self.session.doc.root.first().map(|n| (n.id, n.props.anim_background));
        let top = self.session.doc.root.last().map(|n| (n.id, n.props.anim_foreground));
        let (mut bg, mut fg) = (bottom.is_some_and(|b| b.1), top.is_some_and(|t| t.1));
        let mut fps = s.fps();
        let mut count = s.onion_count();
        let mut opacity = (s.onion_opacity() * 100.0).round();
        let mut open = true;
        egui::Window::new("Frames")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("anim_settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Playback");
                    egui::ComboBox::from_id_salt("anim_playback")
                        .selected_text(mode_name(s.playback_mode))
                        .show_ui(ui, |ui| {
                            for mode in [PlaybackMode::Loop, PlaybackMode::PingPong, PlaybackMode::OneShot] {
                                ui.selectable_value(&mut s.playback_mode, mode, mode_name(mode));
                            }
                        });
                    ui.end_row();
                    ui.label("Direction");
                    egui::ComboBox::from_id_salt("anim_direction")
                        .selected_text(if s.direction == PlaybackDirection::Reverse { "Reverse" } else { "Forward" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut s.direction, PlaybackDirection::Forward, "Forward");
                            ui.selectable_value(&mut s.direction, PlaybackDirection::Reverse, "Reverse");
                        });
                    ui.end_row();
                    ui.label("Frame rate");
                    ui.add(egui::Slider::new(&mut fps, 1..=60).suffix(" fps"));
                    ui.end_row();
                    ui.label("Onion skins");
                    crate::theme::checkbox(ui, &mut s.onion_enabled, "On");
                    ui.end_row();
                    ui.label("Onion frames");
                    ui.add(egui::Slider::new(&mut count, 1..=20).suffix(" each side"));
                    ui.end_row();
                    ui.label("Onion opacity");
                    ui.add(egui::Slider::new(&mut opacity, 0.0..=100.0).suffix(" %"));
                    ui.end_row();
                    ui.label("Previous color");
                    ui.color_edit_button_srgb(&mut s.previous_colour);
                    ui.end_row();
                    ui.label("Next color");
                    ui.color_edit_button_srgb(&mut s.next_colour);
                    ui.end_row();
                    ui.label("Primary layers");
                    crate::theme::checkbox(ui, &mut s.primary_mixed, "Mixed with onion skins");
                    ui.end_row();
                    ui.label("Onion blend");
                    ui.add_enabled_ui(s.primary_mixed, |ui| {
                        egui::ComboBox::from_id_salt("anim_onion_blend")
                            .selected_text(if s.onion_blend == OnionBlend::Multiply { "Multiply" } else { "Normal" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.onion_blend, OnionBlend::Normal, "Normal");
                                ui.selectable_value(&mut s.onion_blend, OnionBlend::Multiply, "Multiply");
                            });
                    });
                    ui.end_row();
                    ui.label("Background");
                    crate::theme::checkbox(ui, &mut bg, "Bottom layer is drawn under every frame");
                    ui.end_row();
                    ui.label("Foreground");
                    crate::theme::checkbox(ui, &mut fg, "Top layer is drawn over every frame");
                    ui.end_row();
                });
            });
        s.set_frame_rate(fps);
        s.onion_count = count;
        s.onion_opacity = opacity / 100.0;
        if s != before {
            self.session.doc.animation = s;
            self.session.mark_edited();
        }
        if let Some((id, was)) = bottom {
            if was != bg {
                self.session.set_props(id, "Animation background", |p| p.anim_background = bg);
            }
        }
        if let Some((id, was)) = top {
            if was != fg {
                self.session.set_props(id, "Animation foreground", |p| p.anim_foreground = fg);
            }
        }
        if !open {
            self.anim.settings_open = false;
        }
    }

    /// Turns Frames on or off.
    pub(crate) fn toggle_frames(&mut self) {
        let on = !self.session.doc.animation.frames_enabled;
        self.session.doc.animation.frames_enabled = on;
        self.session.mark_edited();
        self.anim.canvas_key = None;
        if !on {
            self.anim.playback = None;
        }
    }

    /// The Share sheet rows for animation and timelapse exports.
    pub(crate) fn share_animation_rows(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let busy = self.anim.export.is_some();
        let base = expand_home(&self.export_path);
        ui.add_space(6.0);
        ui.label(RichText::new("ANIMATION").size(11.0).color(pal.text_dim));
        for (format, icon, name) in [
            (AnimFormat::Gif, ia::icon_film as IconFn, "Animated GIF"),
            (AnimFormat::Apng, ia::icon_image as IconFn, "Animated PNG (APNG)"),
            (AnimFormat::Mp4, ia::icon_film as IconFn, "MP4 animation"),
        ] {
            let unavailable = format == AnimFormat::Mp4 && !eskiz_media::encoder_available();
            let file = if unavailable {
                eskiz_media::MP4_UNAVAILABLE.to_owned()
            } else {
                base.with_extension(format.extension()).display().to_string()
            };
            if list_row(ui, &pal, icon, name, Some(&file), !busy && !unavailable, Trailing::None).clicked() {
                self.start_anim_export(&ctx, format);
            }
        }
        let mut opts: AnimExportOptions = self.anim.export_opts;
        ui.add_space(4.0);
        small_label(ui, &pal, "GIF AND PNG OPTIONS");
        egui::Frame::new()
            .fill(pal.row)
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                crate::theme::checkbox(ui, &mut opts.dither, "Dither GIF");
                crate::theme::checkbox(ui, &mut opts.transparent, "Transparent background (GIF and PNG)");
                ui.add(egui::Slider::new(&mut opts.alpha_threshold, 0..=255).text("Alpha threshold"));
                // MP4 is off in this build: its quality setting is not offered.
                if eskiz_media::encoder_available() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("MP4 quality").size(12.5).color(pal.text_dim));
                        egui::ComboBox::from_id_salt("anim_mp4_quality")
                            .selected_text(format!("{:?}", opts.quality))
                            .show_ui(ui, |ui| {
                                for q in [Quality::Low, Quality::Standard, Quality::High, Quality::Studio] {
                                    ui.selectable_value(&mut opts.quality, q, format!("{q:?}"));
                                }
                            });
                    });
                }
            });
        self.anim.export_opts = opts;
        if let Some(job) = self.anim.export.as_ref() {
            let done = job.done.load(std::sync::atomic::Ordering::Relaxed);
            let total = job.total.load(std::sync::atomic::Ordering::Relaxed).max(1);
            let label = job.format.label();
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("Writing {label}... {done} of {total}")).size(12.5).color(pal.text_dim));
                if ui.button("Cancel").clicked() {
                    job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
            ui.add(egui::ProgressBar::new(done as f32 / total as f32).desired_width(f32::INFINITY));
        }
        ui.add_space(6.0);
        ui.label(RichText::new("TIMELAPSE").size(11.0).color(pal.text_dim));
        let recorded = self.timelapse.has_recording();
        let mut mp4 = base.with_extension("mp4").display().to_string();
        let can_encode = eskiz_media::encoder_available();
        if !can_encode || self.timelapse.is_unencoded() {
            mp4 = eskiz_media::MP4_UNAVAILABLE.to_owned();
        }
        let recorded = recorded && can_encode;
        if list_row(ui, &pal, ia::icon_clock, "Timelapse, full length", Some(&mp4), recorded, Trailing::None).clicked() {
            self.start_timelapse_export(&ctx, true);
        }
        if list_row(ui, &pal, ia::icon_clock, "Timelapse, 30 s summary", Some(&mp4), recorded, Trailing::None).clicked() {
            self.start_timelapse_export(&ctx, false);
        }
    }

    /// The Canvas rows for Frames and timelapse.
    pub(crate) fn canvas_animation_rows(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let on = self.session.doc.animation.frames_enabled;
        if list_row(ui, &pal, ia::icon_film, "Frames", Some("Frames, onion skins and playback"), true, Trailing::Switch(on)).clicked() {
            self.toggle_frames();
        }
        let recording = self.prefs.timelapse;
        if list_row(ui, &pal, ia::icon_clock, "Timelapse recording", Some("One frame per change"), true, Trailing::Switch(recording)).clicked() {
            self.prefs.timelapse = !recording;
        }
        let quality = crate::timelapse::quality_label(self.prefs.timelapse_quality);
        if list_row(ui, &pal, ia::icon_sun, "Timelapse quality", None, true, Trailing::Text(quality)).clicked() {
            self.prefs.timelapse_quality = (self.prefs.timelapse_quality + 1) % 4;
        }
        let resolution = crate::timelapse::resolution_label(self.prefs.timelapse_resolution);
        if list_row(ui, &pal, ia::icon_canvas, "Timelapse resolution", None, true, Trailing::Text(resolution)).clicked() {
            self.prefs.timelapse_resolution = (self.prefs.timelapse_resolution + 1) % 3;
        }
        let recorded = self.timelapse.has_recording();
        if list_row(ui, &pal, ia::icon_film, "Timelapse replay", Some("Plays the recording"), recorded, Trailing::None).clicked() {
            self.start_timelapse_replay(&ctx);
        }
    }
}
