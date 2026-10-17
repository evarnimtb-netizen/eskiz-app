//! Colour palette and egui style for Eskiz's chrome.
//!
//! The canvas is never tinted: only bars, panels, text and controls use these colours.

use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Shadow, Stroke, TextStyle};

/// The accent blue: active tool glyph, selected rows, toggles and slider fills.
pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);
/// Danger red: destructive actions.
pub const DANGER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);

/// Which look the chrome uses. `Classic` is the original look (the default for 0.1.0); `Eskiz`
/// is the new identity: nib-orange accent, blueprint blue helpers, navy-graphite chrome,
/// Atkinson Hyperlegible Next, tighter radii. Switched in Actions > Settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiIdentity {
    #[default]
    Classic,
    Eskiz,
}

impl UiIdentity {
    pub const ALL: [UiIdentity; 2] = [UiIdentity::Classic, UiIdentity::Eskiz];

    /// Name shown in Settings.
    pub fn label(self) -> &'static str {
        match self {
            UiIdentity::Classic => "Classic",
            UiIdentity::Eskiz => "Eskiz",
        }
    }

    /// Stored form (`identity=` in the preferences string).
    pub fn key(self) -> &'static str {
        match self {
            UiIdentity::Classic => "classic",
            UiIdentity::Eskiz => "eskiz",
        }
    }

    /// Parse [`UiIdentity::key`]; anything else is `None`.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "classic" => Some(UiIdentity::Classic),
            "eskiz" => Some(UiIdentity::Eskiz),
            _ => None,
        }
    }
}

/// A colour from `#RRGGBB`.
pub const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Colours for one interface mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub light: bool,
    /// The identity this palette belongs to; shape and type tokens follow it.
    pub identity: UiIdentity,
    /// Canvas surroundings.
    pub backdrop: Color32,
    /// Top bar and other thin bars, translucent.
    pub bar: Color32,
    /// Floating panels and the sidebar capsule.
    pub panel: Color32,
    /// Rows inside panels, slider tracks.
    pub row: Color32,
    /// Slider rails and checkbox boxes: a step darker or lighter than the rows they sit on.
    pub track: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub danger: Color32,
    /// Ring around the colour circle.
    pub ring: Color32,
    /// Soft drop shadow colour.
    pub shadow: Color32,
    /// 1 px inner border.
    pub hairline: Color32,
    /// Row hover fill.
    pub hover: Color32,
    /// Control outlines (3:1 on panels and rows in the Eskiz identity).
    pub edge: Color32,
    /// Fill of selected rows and active controls.
    pub accent_fill: Color32,
    /// Text and glyphs on `accent_fill`.
    pub on_accent: Color32,
    pub warning: Color32,
    pub success: Color32,
    /// Guides, grids and selection helpers.
    pub blueprint: Color32,
    /// Keyboard focus ring and its opposite-colour halo.
    pub focus_ring: Color32,
    pub focus_halo: Color32,
}

/// The Classic palette (see [`palette_for`]).
pub fn palette(light: bool) -> Palette {
    palette_for(UiIdentity::Classic, light)
}

/// The palette of an identity: dark (the default) or the light variant.
pub fn palette_for(identity: UiIdentity, light: bool) -> Palette {
    if identity == UiIdentity::Eskiz {
        return eskiz_palette(light);
    }
    if light {
        Palette {
            light,
            identity: UiIdentity::Classic,
            backdrop: Color32::from_rgb(0xD9, 0xD9, 0xDE),
            bar: Color32::from_rgba_unmultiplied(0xF2, 0xF2, 0xF7, 235),
            panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            row: Color32::from_rgb(0xEF, 0xEF, 0xF4),
            track: Color32::from_rgb(0xC7, 0xC7, 0xCC),
            separator: Color32::from_rgb(0xD1, 0xD1, 0xD6),
            text: Color32::from_rgb(0x1C, 0x1C, 0x1E),
            text_dim: Color32::from_rgb(0x58, 0x58, 0x5C),
            accent: ACCENT,
            danger: DANGER,
            ring: Color32::from_rgb(0x1C, 0x1C, 0x1E),
            shadow: Color32::from_black_alpha(90),
            hairline: Color32::from_black_alpha(15),
            hover: Color32::from_rgb(0xD1, 0xD1, 0xD6),
            edge: Color32::from_rgb(0x8E, 0x8E, 0x93),
            accent_fill: ACCENT,
            on_accent: Color32::WHITE,
            warning: Color32::from_rgb(230, 90, 80),
            success: ACCENT,
            blueprint: ACCENT,
            focus_ring: ACCENT,
            focus_halo: ACCENT,
        }
    } else {
        Palette {
            light,
            identity: UiIdentity::Classic,
            backdrop: Color32::from_rgb(0x12, 0x12, 0x12),
            bar: Color32::from_rgba_unmultiplied(0x1C, 0x1C, 0x1E, 235),
            panel: Color32::from_rgb(0x2A, 0x2A, 0x2C),
            row: Color32::from_rgb(0x33, 0x33, 0x36),
            track: Color32::from_rgb(0x55, 0x55, 0x5A),
            separator: Color32::from_rgb(0x3C, 0x3C, 0x40),
            text: Color32::from_rgb(0xED, 0xED, 0xED),
            text_dim: Color32::from_rgb(0xAA, 0xAA, 0xAE),
            accent: ACCENT,
            danger: DANGER,
            ring: Color32::from_rgb(0xED, 0xED, 0xED),
            shadow: Color32::from_black_alpha(102),
            hairline: Color32::from_white_alpha(15),
            hover: Color32::from_rgb(0x3C, 0x3C, 0x40),
            edge: Color32::from_rgb(0x8A, 0x8A, 0x90),
            accent_fill: ACCENT,
            on_accent: Color32::WHITE,
            warning: Color32::from_rgb(230, 90, 80),
            success: ACCENT,
            blueprint: ACCENT,
            focus_ring: ACCENT,
            focus_halo: ACCENT,
        }
    }
}

/// The Eskiz identity. The values are checked against `packaging/identity/eskiz-tokens.json` by a test.
fn eskiz_palette(light: bool) -> Palette {
    if light {
        Palette {
            light,
            identity: UiIdentity::Eskiz,
            backdrop: hex(0xD4D4D4),
            bar: Color32::from_rgba_unmultiplied(0xF3, 0xF4, 0xF8, 235),
            panel: hex(0xFAFBFD),
            row: hex(0xECEEF4),
            track: hex(0x9AA3BC),
            separator: hex(0xD5D9E4),
            text: hex(0x141A2B),
            text_dim: hex(0x454F68),
            accent: hex(0xA33008),
            danger: hex(0xA5211A),
            ring: hex(0x141A2B),
            shadow: Color32::from_black_alpha(89),
            hairline: Color32::from_black_alpha(20),
            hover: hex(0xDBDFEA),
            edge: hex(0x7580A0),
            accent_fill: hex(0xFF5A1F),
            on_accent: hex(0x0B1020),
            warning: hex(0x7A4F00),
            success: hex(0x0B5E3D),
            blueprint: hex(0x1D5195),
            focus_ring: hex(0x1E4F96),
            focus_halo: hex(0xFFFFFF),
        }
    } else {
        Palette {
            light,
            identity: UiIdentity::Eskiz,
            backdrop: hex(0x242424),
            bar: Color32::from_rgba_unmultiplied(0x12, 0x18, 0x29, 235),
            panel: hex(0x1A2133),
            row: hex(0x242D45),
            track: hex(0x6A779A),
            separator: hex(0x2B3552),
            text: hex(0xEEF2FA),
            text_dim: hex(0xA9B5CE),
            accent: hex(0xFF8A60),
            danger: hex(0xFF8F8F),
            ring: hex(0xEEF2FA),
            shadow: Color32::from_black_alpha(89),
            hairline: Color32::from_white_alpha(20),
            hover: hex(0x303B59),
            edge: hex(0x7A88AD),
            accent_fill: hex(0xFF5A1F),
            on_accent: hex(0x0B1020),
            warning: hex(0xFFC247),
            success: hex(0x5FD6A0),
            blueprint: hex(0x8FB9F2),
            focus_ring: hex(0x8FB9F2),
            focus_halo: hex(0x0B1020),
        }
    }
}

impl Palette {
    /// Colour of a disabled control's label or glyph. It is the secondary-text token, which
    /// meets 4.5:1 on every surface, so disabled controls stay readable. They look different from
    /// enabled ones by colour (dimmer than `text`) and by not responding.
    pub fn disabled(&self) -> Color32 {
        self.text_dim
    }
}

/// Shape tokens of an identity (radii in points). Classic returns today's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    /// Panels, popovers and windows.
    pub panel_radius: u8,
    /// Popup menus.
    pub menu_radius: u8,
    /// Buttons, fields and other widgets.
    pub widget_radius: u8,
    pub checkbox_radius: u8,
    /// Warning banners.
    pub banner_radius: u8,
    /// Value bubbles beside sliders.
    pub bubble_radius: u8,
    /// Toasts.
    pub toast_radius: u8,
    /// Shadow offset (y), blur, and alpha out of 255.
    pub shadow_dy: i8,
    pub shadow_blur: u8,
    pub shadow_alpha: u8,
    /// Slider rail width in points.
    pub rail_width: f32,
}

/// Shape tokens for an identity.
pub fn tokens(identity: UiIdentity) -> Tokens {
    match identity {
        UiIdentity::Classic => Tokens {
            panel_radius: 12,
            menu_radius: 12,
            widget_radius: 8,
            checkbox_radius: 4,
            banner_radius: 8,
            bubble_radius: 8,
            toast_radius: 10,
            shadow_dy: 6,
            shadow_blur: 24,
            shadow_alpha: 0, // unused: Classic uses `Palette::shadow` as is
            rail_width: 4.0,
        },
        UiIdentity::Eskiz => Tokens {
            panel_radius: 8,
            menu_radius: 8,
            widget_radius: 4,
            checkbox_radius: 2,
            banner_radius: 4,
            bubble_radius: 4,
            toast_radius: 6,
            shadow_dy: 4,
            shadow_blur: 16,
            shadow_alpha: 89,
            rail_width: 3.0,
        },
    }
}

/// Panel shadow: Classic 24 px blur at 40% black; Eskiz 16 px at 35%.
pub fn shadow(pal: &Palette) -> Shadow {
    let t = tokens(pal.identity);
    Shadow {
        offset: [0, t.shadow_dy],
        blur: t.shadow_blur,
        spread: 0,
        color: pal.shadow,
    }
}

/// Frame for floating panels: 12 px radius (Eskiz 8), 1 px hairline, soft shadow.
pub fn panel_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.panel)
        .corner_radius(CornerRadius::same(tokens(pal.identity).panel_radius))
        .stroke(Stroke::new(1.0, pal.hairline))
        .shadow(shadow(pal))
        .inner_margin(Margin::same(12))
}

/// Frame for the translucent bars (top bar, sidebar capsule uses `panel_frame`).
pub fn bar_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new().fill(pal.bar).inner_margin(Margin::symmetric(8, 0))
}

/// Install the palette and the spacing and type sizes into egui.
pub fn apply(ctx: &egui::Context, pal: &Palette) {
    let mut visuals = if pal.light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.dark_mode = !pal.light;
    visuals.override_text_color = Some(pal.text);
    visuals.panel_fill = pal.backdrop;
    visuals.window_fill = pal.panel;
    visuals.window_stroke = Stroke::new(1.0, pal.hairline);
    let tk = tokens(pal.identity);
    visuals.window_corner_radius = CornerRadius::same(tk.panel_radius);
    visuals.menu_corner_radius = CornerRadius::same(tk.menu_radius);
    visuals.window_shadow = shadow(pal);
    visuals.popup_shadow = shadow(pal);
    visuals.extreme_bg_color = pal.row;
    visuals.faint_bg_color = pal.row;
    visuals.hyperlink_color = pal.accent;
    visuals.selection.bg_fill = pal.accent_fill;
    visuals.selection.stroke = Stroke::new(1.0, pal.accent_fill);
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(tk.widget_radius);
    }
    visuals.widgets.inactive.bg_fill = pal.track;
    visuals.widgets.inactive.weak_bg_fill = pal.row;
    visuals.widgets.hovered.bg_fill = pal.hover;
    visuals.widgets.hovered.weak_bg_fill = pal.hover;
    visuals.widgets.active.bg_fill = pal.accent_fill;
    visuals.widgets.active.weak_bg_fill = pal.accent_fill;
    ctx.set_visuals(visuals);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("eskiz_identity"), pal.identity == UiIdentity::Eskiz));
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("eskiz_checkbox_tokens"), (tk.checkbox_radius, pal.on_accent)));

    ctx.global_style_mut(|style| {
        style.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        style.text_styles.insert(TextStyle::Heading, FontId::proportional(if pal.identity == UiIdentity::Eskiz { 16.0 } else { 17.0 }));
        style.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.0));
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.interact_size = egui::vec2(36.0, 36.0);
    });
}


/// Whether the Eskiz look is installed in this context (set by [`apply`]).
pub fn is_eskiz(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::new("eskiz_identity"))).unwrap_or(false)
}

/// A checkbox that stays visible on rows and panels: its box has an outline, and it fills with
/// the accent when on.
pub fn checkbox(ui: &mut egui::Ui, value: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    // Eskiz passes its own radius and ink through the style: see `apply`.
    let (checkbox_radius, on_accent) = ui.data(|d| d.get_temp::<(u8, Color32)>(egui::Id::new("eskiz_checkbox_tokens"))).unwrap_or((4, Color32::WHITE));
    let accent = ui.visuals().selection.bg_fill;
    let outline = ui.visuals().weak_text_color();
    ui.scope(|ui| {
        let inactive = &mut ui.visuals_mut().widgets.inactive;
        inactive.corner_radius = CornerRadius::same(checkbox_radius);
        inactive.bg_stroke = Stroke::new(1.0, outline);
        if *value {
            inactive.bg_fill = accent;
            inactive.fg_stroke = Stroke::new(1.8, on_accent);
        }
        ui.checkbox(value, text)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_is_the_default_look() {
        let dark = palette(false);
        assert!(!dark.light);
        assert_eq!(dark.backdrop, Color32::from_rgb(0x12, 0x12, 0x12));
        assert_eq!(dark.panel, Color32::from_rgb(0x2A, 0x2A, 0x2C));
        assert_eq!(dark.accent, ACCENT);
    }

    #[test]
    fn light_variant_inverts_text_and_bar() {
        let light = palette(true);
        assert!(light.light);
        assert_eq!(light.text, Color32::from_rgb(0x1C, 0x1C, 0x1E));
        assert_ne!(light.bar, palette(false).bar);
    }

    const TOKENS_JSON: &str = include_str!("../../../packaging/identity/eskiz-tokens.json");

    fn parse_hex(v: &serde_json::Value) -> Color32 {
        let s = v.as_str().expect("hex string").trim_start_matches('#');
        hex(u32::from_str_radix(s, 16).expect("hex digits"))
    }

    /// The Eskiz palettes in `theme.rs` are the values of the checked-in token file.
    #[test]
    fn identity_tokens_match_the_checked_in_json() {
        let json: serde_json::Value = serde_json::from_str(TOKENS_JSON).unwrap();
        for light in [false, true] {
            let pal = palette_for(UiIdentity::Eskiz, light);
            let j = &json["palette"][if light { "light" } else { "dark" }];
            let alpha = j["bar_alpha"].as_u64().unwrap() as u8;
            let want = |k: &str| parse_hex(&j[k]);
            assert_eq!(pal.backdrop, want("backdrop"), "backdrop light={light}");
            let bar = want("bar");
            assert_eq!(pal.bar, Color32::from_rgba_unmultiplied(bar.r(), bar.g(), bar.b(), alpha), "bar");
            for (k, got) in [
                ("panel", pal.panel),
                ("row", pal.row),
                ("hover", pal.hover),
                ("separator", pal.separator),
                ("track", pal.track),
                ("edge", pal.edge),
                ("text", pal.text),
                ("text_dim", pal.text_dim),
                ("accent", pal.accent),
                ("accent_fill", pal.accent_fill),
                ("on_accent", pal.on_accent),
                ("danger", pal.danger),
                ("warning", pal.warning),
                ("success", pal.success),
                ("blueprint", pal.blueprint),
                ("focus_ring", pal.focus_ring),
                ("focus_halo", pal.focus_halo),
            ] {
                assert_eq!(got, want(k), "{k} light={light}");
            }
        }
        // Shape tokens.
        let r = &json["shape"]["eskiz_radius"];
        let t = tokens(UiIdentity::Eskiz);
        assert_eq!(u64::from(t.panel_radius), r["panel"].as_u64().unwrap());
        assert_eq!(u64::from(t.widget_radius), r["widget"].as_u64().unwrap());
        assert_eq!(u64::from(t.checkbox_radius), r["checkbox"].as_u64().unwrap());
        assert_eq!(u64::from(t.toast_radius), r["toast"].as_u64().unwrap());
        let sh = &json["shape"]["eskiz_shadow"];
        assert_eq!(i64::from(t.shadow_dy), sh["offset"][1].as_i64().unwrap());
        assert_eq!(u64::from(t.shadow_blur), sh["blur"].as_u64().unwrap());
        assert_eq!(f64::from(t.shadow_alpha), (sh["alpha"].as_f64().unwrap() * 255.0).round());
        assert_eq!(f64::from(t.rail_width), json["shape"]["stroke"]["slider_rail"].as_f64().unwrap());
    }

    /// The icon's accent colors are the Eskiz accent source.
    #[test]
    fn eskiz_accent_comes_from_the_icon() {
        assert_eq!(palette_for(UiIdentity::Eskiz, false).accent_fill, hex(0xFF5A1F));
        assert_eq!(palette_for(UiIdentity::Eskiz, false).blueprint, hex(0x8FB9F2));
    }

    #[test]
    fn classic_is_unchanged_by_the_identity_work() {
        let c = palette(false);
        assert_eq!(c.identity, UiIdentity::Classic);
        assert_eq!(c.hover, c.separator);
        assert_eq!(c.accent_fill, ACCENT);
        assert_eq!(c.on_accent, Color32::WHITE);
        let t = tokens(UiIdentity::Classic);
        assert_eq!((t.panel_radius, t.widget_radius, t.shadow_dy, t.shadow_blur), (12, 8, 6, 24));
        assert_eq!(shadow(&c).blur, 24);
        assert_eq!(UiIdentity::default(), UiIdentity::Classic);
        for id in UiIdentity::ALL {
            assert_eq!(UiIdentity::from_key(id.key()), Some(id));
        }
    }
}
