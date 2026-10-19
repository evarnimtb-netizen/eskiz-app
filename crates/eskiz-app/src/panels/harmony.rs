//! Colour panel additions: the Harmony tab (companion swatches on the disc), and the palette
//! import, export and "from canvas" actions under the Palettes tab.

use std::fs;
use std::path::Path;

use crate::file_access::PickPurpose;

use eskiz_format::palettes::{self, Palette as FilePalette, Swatch};
use eskiz_guides::colour::{hsb_to_rgb, rgb_to_hsb, Rgb8};
use eskiz_guides::harmony::{harmony, Harmony};
use eskiz_guides::palette::{extract_palette, MAX_PALETTE_COLOURS};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, UiBuilder, Vec2};

use crate::app::EskizApp;
use crate::colour::{format_hex, hue_dir, rgb_to_hsv};
use crate::theme::Palette;

/// The harmony modes in the Harmony tab, in order.
pub const HARMONIES: [(Harmony, &str); 5] = [
    (Harmony::Complementary, "Complementary"),
    (Harmony::SplitComplementary, "Split"),
    (Harmony::Analogous, "Analogous"),
    (Harmony::Triadic, "Triadic"),
    (Harmony::Tetradic, "Tetradic"),
];

/// Companion colours of `colour` for `mode`, as RGB bytes. The base colour is not included.
pub fn companions(colour: [u8; 3], mode: Harmony) -> Vec<[u8; 3]> {
    let base = rgb_to_hsb(Rgb8::new(colour[0], colour[1], colour[2]).to_rgb());
    harmony(base, mode)
        .into_iter()
        .map(|hsb| {
            let c = hsb_to_rgb(hsb).to_rgb8();
            [c.r, c.g, c.b]
        })
        .collect()
}

#[derive(Clone, Copy)]
enum PaletteAction {
    Import,
    Export,
    FromCanvas,
}

fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn ext_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

impl EskizApp {
    /// The Harmony tab: mode chips, the disc, and the companion swatches on the hue ring.
    /// Clicking a companion picks it.
    pub(crate) fn harmony_tab(&mut self, ui: &mut egui::Ui, area: Rect, pal: &Palette) {
        // The mode names wrap onto two rows in a narrow popover. The disc starts below whatever they use.
        let bar = Rect::from_min_size(area.min, Vec2::new(area.width(), 60.0));
        let used = ui
            .scope_builder(UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (i, (_, name)) in HARMONIES.iter().enumerate() {
                        if ui.selectable_label(self.guides.harmony == i, *name).clicked() {
                            self.guides.harmony = i;
                        }
                    }
                });
                ui.min_rect()
            })
            .inner;
        let disc_area = Rect::from_min_max(Pos2::new(area.left(), used.bottom() + 6.0), area.max);
        self.disc_tab(ui, disc_area, pal);

        let Some((centre, radius)) = self.colour_ui.disc else {
            return;
        };
        let mode = HARMONIES[self.guides.harmony.min(HARMONIES.len() - 1)].0;
        let ring = radius * 0.86;
        let marker = radius * 0.11;
        let mut chosen = None;
        for (i, c) in companions(self.colour, mode).into_iter().enumerate() {
            let (dx, dy) = hue_dir(rgb_to_hsv(c).h);
            let at = centre + Vec2::new(dx, dy) * ring;
            let rect = Rect::from_center_size(at, Vec2::splat(marker * 2.0));
            ui.painter()
                .circle_filled(at, marker, {
                    let s = crate::colour_management::show(c);
                    Color32::from_rgb(s[0], s[1], s[2])
                });
            ui.painter()
                .circle_stroke(at, marker, Stroke::new(1.5, pal.text));
            let resp = ui.interact(rect, ui.id().with(("harmony", i)), Sense::click());
            if resp.clicked() {
                chosen = Some(c);
            }
            resp.on_hover_text(format_hex(c));
        }
        if let Some(c) = chosen {
            self.choose_colour(c);
        }
    }

    /// The "Palette files" window, opened from the Palettes tab: import, export, and a palette
    /// from the canvas. It floats, so the swatch grid keeps its full height.
    pub(crate) fn palette_files_window(&mut self, ctx: &egui::Context) {
        if !self.guides.palette_window_open {
            return;
        }
        let mut open = true;
        let mut action = None;
        let mut browse = None;
        egui::Window::new("Palette files")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 120.0))
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("File (.swatches, .aco or .ase)").size(12.0));
                ui.add(
                    egui::TextEdit::singleline(&mut self.guides.palette_path)
                        .hint_text("/path/to/palette.aco")
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    if ui.button("Import...").on_hover_text("Choose a palette file").clicked() {
                        browse = Some(PickPurpose::PaletteImport);
                    }
                    if ui.button("Export...").on_hover_text("Choose where to save the palette").clicked() {
                        browse = Some(PickPurpose::PaletteExport);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Import").clicked() {
                        action = Some(PaletteAction::Import);
                    }
                    if ui.button("Export").clicked() {
                        action = Some(PaletteAction::Export);
                    }
                    if ui.button("New from canvas").clicked() {
                        action = Some(PaletteAction::FromCanvas);
                    }
                });
            });
        self.guides.palette_window_open = open;
        if let Some(purpose) = browse {
            let typed = self.guides.palette_path.clone();
            let name = (purpose == PickPurpose::PaletteExport).then(|| "palette.swatches".to_owned());
            self.request_pick_near(purpose, &typed, name);
        }
        match action {
            // Android has no typed path: the buttons open the system picker.
            Some(PaletteAction::Import) if self.files.staged() => self.request_pick(PickPurpose::PaletteImport, None, None),
            Some(PaletteAction::Export) if self.files.staged() => {
                self.request_pick(PickPurpose::PaletteExport, Some("palette.swatches".into()), None);
            }
            Some(PaletteAction::Import) => self.palette_import(),
            Some(PaletteAction::Export) => self.palette_export(),
            Some(PaletteAction::FromCanvas) => self.palette_from_canvas(),
            None => {}
        }
    }

    /// Appends the colours of a `.swatches`, `.aco` or `.ase` file to the palette.
    pub(crate) fn palette_import(&mut self) {
        let path = self.guides.palette_path.trim().to_owned();
        let read = match ext_of(&path).as_str() {
            "swatches" | "aco" | "ase" => fs::read(&path).map_err(|e| e.to_string()),
            _ => {
                self.show_toast("Use a .swatches, .aco or .ase file", 3.0);
                return;
            }
        };
        let bytes = match read {
            Ok(bytes) => bytes,
            Err(e) => {
                self.show_toast(format!("Could not read {path}: {e}"), 3.0);
                return;
            }
        };
        let parsed = match ext_of(&path).as_str() {
            "swatches" => palettes::read_swatches(&bytes),
            "aco" => palettes::read_aco(&bytes),
            _ => palettes::read_ase(&bytes),
        };
        match parsed {
            Ok(read) => {
                let added = read.palette.swatches.len();
                for swatch in read.palette.swatches {
                    let [r, g, b, _] = swatch.rgba;
                    self.colour_store
                        .palette
                        .push([to_byte(r), to_byte(g), to_byte(b)]);
                }
                self.show_toast(format!("Imported {added} colors"), 2.5);
            }
            Err(e) => self.show_toast(format!("Could not read the palette: {e}"), 3.0),
        }
    }

    /// Writes the palette in the format the file name asks for.
    pub(crate) fn palette_export(&mut self) {
        self.palette_export_with(crate::deliver::Delivery::Local);
    }

    /// Like [`Self::palette_export`], delivering the file as `delivery` says.
    pub(crate) fn palette_export_with(&mut self, delivery: crate::deliver::Delivery) {
        let path = self.guides.palette_path.trim().to_owned();
        let ext = ext_of(&path);
        let palette = FilePalette {
            name: "Eskiz".to_owned(),
            swatches: self
                .colour_store
                .palette
                .iter()
                .map(|c| Swatch {
                    name: None,
                    rgba: [
                        f32::from(c[0]) / 255.0,
                        f32::from(c[1]) / 255.0,
                        f32::from(c[2]) / 255.0,
                        1.0,
                    ],
                })
                .collect(),
        };
        let encoded = match ext.as_str() {
            "swatches" => palettes::write_swatches(&palette),
            "aco" => palettes::write_aco(&palette),
            "ase" => palettes::write_ase(&palette),
            _ => {
                self.show_toast("Use a .swatches, .aco or .ase file name", 3.0);
                return;
            }
        };
        let result = encoded.map_err(|e| e.to_string()).and_then(|bytes| {
            palettes::save_atomic(Path::new(&path), &bytes).map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => {
                let text = format!("Exported {} colors", palette.swatches.len());
                self.deliver(Path::new(&path), delivery, text, 2.5);
            }
            Err(e) => self.show_toast(format!("Could not write {path}: {e}"), 3.0),
        }
    }

    /// Replaces the palette with the dominant colours of the current canvas.
    fn palette_from_canvas(&mut self) {
        // The flattened canvas is opaque, so it needs no un-premultiplying.
        let flat = self.session.flatten();
        let pixels: Vec<[u8; 4]> = flat.as_chunks::<4>().0.to_vec();
        let colours = extract_palette(&pixels, MAX_PALETTE_COLOURS);
        if colours.is_empty() {
            self.show_toast("The canvas has no colors to take", 2.5);
            return;
        }
        self.colour_store.palette = colours.iter().map(|c| [c.r, c.g, c.b]).collect();
        self.show_toast(
            format!(
                "Palette replaced with {} colors from the canvas",
                colours.len()
            ),
            2.5,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complementary_companion_is_the_opposite_hue() {
        let red = [220, 30, 30];
        let comp = companions(red, Harmony::Complementary);
        assert_eq!(comp.len(), 1);
        // Opposite of red is cyan-ish: green and blue both above red.
        assert!(comp[0][1] > 150 && comp[0][2] > 150, "got {:?}", comp[0]);
    }

    #[test]
    fn each_mode_gives_its_number_of_companions() {
        for (mode, n) in HARMONIES.iter().map(|(m, _)| *m).zip([1usize, 2, 2, 2, 3]) {
            assert_eq!(companions([40, 90, 200], mode).len(), n);
        }
    }
}
