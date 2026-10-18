//! Accessibility helpers for the controls Eskiz draws itself.
//!
//! egui feeds AccessKit (the screen-reader bridge) from the `WidgetInfo` a widget reports. A
//! control that only paints an icon has no name, so these helpers give each one a label, a role
//! and a state. They also draw the keyboard focus ring that custom painting does not get for free,
//! and read the reduced-motion request from the operating system.

use eframe::egui::{self, CornerRadius, Painter, Rect, Response, Stroke, StrokeKind, WidgetInfo, WidgetType};

use crate::theme::Palette;

/// Name a control: its accessible label and role.
pub fn named(resp: &Response, typ: WidgetType, enabled: bool, label: impl Into<String>) {
    let label: String = label.into();
    resp.widget_info(|| WidgetInfo::labeled(typ, enabled, label.as_str()));
}

/// Name a control that has an on or off state (a tab, a switch, a list choice).
pub fn toggle(resp: &Response, typ: WidgetType, enabled: bool, selected: bool, label: impl Into<String>) {
    let label: String = label.into();
    resp.widget_info(|| WidgetInfo::selected(typ, enabled, selected, label.as_str()));
}

/// Name a slider. The label carries the value as text ("Brush size 24%"), because screen readers
/// announce the label and most users hear the number, not a fraction.
pub fn slider(resp: &Response, enabled: bool, label: impl Into<String>) {
    named(resp, WidgetType::Slider, enabled, label);
}

/// Let the focused control `id` have the arrow keys. Without this egui moves keyboard focus to
/// the neighbouring widget on every arrow press, so a slider would lose focus as it was adjusted.
pub fn keep_arrows(ui: &egui::Ui, id: egui::Id) {
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() });
    });
}

/// Colours of the keyboard focus ring and of its halo outside it. Never the selected fill (Classic's accent): the
/// ring is the text colour, the halo its opposite, so focus and selection are told apart on any surface.
pub fn focus_colours(pal: &Palette) -> (egui::Color32, egui::Color32) {
    if pal.identity == crate::theme::UiIdentity::Eskiz {
        (pal.focus_ring, pal.focus_halo)
    } else if pal.light {
        (egui::Color32::from_rgb(0x10, 0x11, 0x14), egui::Color32::WHITE)
    } else {
        (egui::Color32::WHITE, egui::Color32::from_rgb(0x10, 0x11, 0x14))
    }
}

/// Draw the keyboard focus ring around `rect` when `resp` holds keyboard focus.
pub fn focus_ring(p: &Painter, rect: Rect, resp: &Response, pal: &Palette, radius: u8) {
    if resp.has_focus() {
        let (ring, halo) = focus_colours(pal);
        // A 1 px halo in the opposite colour outside the ring, so it reads on any surface.
        p.rect_stroke(rect.expand(6.0), CornerRadius::same(radius.saturating_add(6)), Stroke::new(1.0, halo), StrokeKind::Outside);
        p.rect_stroke(rect.expand(3.0), CornerRadius::same(radius.saturating_add(3)), Stroke::new(2.0, ring), StrokeKind::Outside);
    }
}

/// Whether the operating system asks for less motion. Best effort: macOS (Reduce motion) and
/// GNOME (animations switched off) are read. Other systems report false, and the Actions > Settings
/// switch still works everywhere. Read once when the app starts, not every frame.
pub fn os_prefers_reduced_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("defaults")
            .args(["read", "com.apple.universalaccess", "reduceMotion"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "false")
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// WCAG 2.x contrast ratio between two opaque colours, from 1 to 21.
pub fn contrast_ratio(a: egui::Color32, b: egui::Color32) -> f64 {
    fn lum(c: egui::Color32) -> f64 {
        let lin = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.039_28 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
    }
    let (la, lb) = (lum(a), lum(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// A plain-words name for a colour, such as "Deep blue" or "Light grey", for screen readers.
pub fn colour_name(rgb: [u8; 3]) -> String {
    let max = f32::from(rgb[0].max(rgb[1]).max(rgb[2])) / 255.0;
    let min = f32::from(rgb[0].min(rgb[1]).min(rgb[2])) / 255.0;
    let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
    if sat < 0.12 {
        return match max {
            v if v < 0.12 => "Black",
            v if v < 0.4 => "Dark gray",
            v if v < 0.75 => "Gray",
            v if v < 0.94 => "Light gray",
            _ => "White",
        }
        .to_owned();
    }
    let (r, g, b) = (f32::from(rgb[0]), f32::from(rgb[1]), f32::from(rgb[2]));
    let d = (max - min) * 255.0;
    let mx = max * 255.0;
    let mut hue = if mx == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    if hue < 0.0 {
        hue += 360.0;
    }
    let base = match hue {
        h if !(15.0..345.0).contains(&h) => "red",
        h if h < 45.0 => "orange",
        h if h < 70.0 => "yellow",
        h if h < 160.0 => "green",
        h if h < 200.0 => "cyan",
        h if h < 250.0 => "blue",
        h if h < 290.0 => "purple",
        h if h < 330.0 => "magenta",
        _ => "pink",
    };
    let shade = if max < 0.35 {
        "Dark "
    } else if max < 0.6 {
        "Deep "
    } else if sat < 0.45 && max > 0.8 {
        "Light "
    } else if sat < 0.45 {
        "Muted "
    } else {
        ""
    };
    let name = format!("{shade}{base}");
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => name,
    }
}

/// The accessible name of the current colour: its words and its hex code.
pub fn colour_label(rgb: [u8; 3]) -> String {
    format!("{}, hex {:02X}{:02X}{:02X}", colour_name(rgb), rgb[0], rgb[1], rgb[2])
}

#[cfg(test)]
mod colour_name_tests {
    use super::*;

    #[test]
    fn names_the_current_colour_in_words_with_its_hex() {
        assert_eq!(colour_label([0x1E, 0x3C, 0x8C]), "Deep blue, hex 1E3C8C");
        assert_eq!(colour_name([255, 0, 0]), "Red");
        assert_eq!(colour_name([0, 0, 0]), "Black");
        assert_eq!(colour_name([250, 250, 250]), "White");
        assert_eq!(colour_name([200, 200, 200]), "Light gray");
    }
}
