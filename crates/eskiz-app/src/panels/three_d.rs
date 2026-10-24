//! 3D panels: the Layers panel's texture-set groups while a 3D document is open, and the Share
//! tab's GLB, USDZ and texture PNG rows. The 3D tab of the Actions panel is in [`crate::view3d`].

use eframe::egui::{self, RichText};

use crate::app::EskizApp;
use crate::chrome::small_label;
use crate::export::expand_home;
use eskiz_3d::{model_layer_groups, LayerNode, MapKind};

/// Row title for one map.
fn map_name(kind: MapKind) -> &'static str {
    match kind {
        MapKind::Colour => "Color",
        MapKind::Roughness => "Roughness",
        MapKind::Metallic => "Metallic",
        MapKind::Normal => "Normal",
        MapKind::AmbientOcclusion => "Ambient occlusion",
    }
}

/// Draw the nodes of a texture set's layer tree. Clicking a map makes it the paint target.
fn draw_nodes(ui: &mut egui::Ui, nodes: &[LayerNode], active: &mut MapKind) {
    for node in nodes {
        match node {
            LayerNode::Map(kind) => {
                if ui.selectable_label(*active == *kind, map_name(*kind)).clicked() {
                    *active = *kind;
                }
            }
            LayerNode::Group(group) => {
                ui.label(RichText::new(&group.name).strong());
                ui.indent(group.name.clone(), |ui| draw_nodes(ui, &group.children, active));
            }
            LayerNode::Composite => {
                ui.label(RichText::new("Composite (derived)").weak());
            }
            LayerNode::Selection { .. } => {
                ui.label(RichText::new("Selection").weak());
            }
        }
    }
}

impl EskizApp {
    /// The Layers panel while a 3D document is open: one group per texture set, with its
    /// visibility and lock, and its maps. The chosen map is where strokes paint.
    pub(crate) fn three_d_layers_body(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let Some(three) = self.three_d.as_mut() else { return };
        small_label(ui, &pal, "TEXTURE SETS");
        let groups = model_layer_groups(&three.doc.model);
        let order = three.doc.model.texture_set_display_order();
        let mut changed = false;
        for (group, set) in groups.iter().zip(order) {
            egui::CollapsingHeader::new(group.name.as_str())
                .id_salt(("three-d-set", set))
                .default_open(true)
                .show(ui, |ui| {
                    let ts = &mut three.doc.model.texture_sets[set];
                    ui.horizontal(|ui| {
                        changed |= crate::theme::checkbox(ui, &mut ts.visible, "Visible").changed();
                        changed |= crate::theme::checkbox(ui, &mut ts.locked, "Locked").changed();
                    });
                    draw_nodes(ui, &group.children, &mut three.doc.active_map);
                });
        }
        if changed {
            // Visible and Locked are saved with the document.
            three.doc.dirty = true;
            three.touch();
        }
    }

    /// The Share tab while a 3D document is open: the model as GLB, and the maps as PNG files.
    /// The export path field names the file (GLB) or the folder (texture PNGs).
    pub(crate) fn three_d_share_body(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let ctx = ui.ctx().clone();
        let staged = self.files.staged();
        small_label(ui, &pal, "3D EXPORT");
        if staged {
            self.share_mode_switch(ui);
        }
        let base = expand_home(&self.export_path);
        let width = ui.available_width();
        if ui.add_sized([width, 40.0], egui::Button::new("GLB")).clicked() {
            if staged {
                self.begin_staged(&ctx, crate::deliver::ExportKind::Glb);
            } else {
                self.start_glb_export(base.with_extension("glb"), crate::deliver::Delivery::Local);
            }
        }
        if !staged && ui.add_sized([width, 28.0], egui::Button::new("Choose GLB file...")).clicked() {
            let typed = self.export_path.clone();
            self.request_pick_near(crate::file_access::PickPurpose::Model3d, &typed, Some("model.glb".into()));
        }
        ui.label(RichText::new("The model with its maps, as one .glb file.").weak());
        ui.add_space(6.0);
        if ui.add_sized([width, 40.0], egui::Button::new("USDZ")).clicked() {
            if staged {
                self.begin_staged(&ctx, crate::deliver::ExportKind::Usdz);
            } else {
                self.start_usdz_export(base.with_extension("usdz"), crate::deliver::Delivery::Local);
            }
        }
        if !staged && ui.add_sized([width, 28.0], egui::Button::new("Choose USDZ file...")).clicked() {
            let typed = self.export_path.clone();
            self.request_pick_near(crate::file_access::PickPurpose::Usdz, &typed, Some("model.usdz".into()));
        }
        ui.label(RichText::new("For Apple Quick Look and AR: .usdz with the maps inside it.").weak());
        ui.add_space(6.0);
        if ui.add_sized([width, 40.0], egui::Button::new("Texture PNGs")).clicked() {
            if staged {
                self.begin_staged(&ctx, crate::deliver::ExportKind::Textures);
            } else {
                self.start_texture_export(base, crate::deliver::Delivery::Local);
            }
        }
        if !staged && ui.add_sized([width, 28.0], egui::Button::new("Choose texture folder...")).clicked() {
            let typed = self.export_path.clone();
            self.request_pick_near(crate::file_access::PickPurpose::ExportFolder, &typed, None);
        }
        ui.label(RichText::new("One PNG per map per texture set, into the folder named above.").weak());
    }
}
