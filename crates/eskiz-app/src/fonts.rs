//! Fonts. Classic keeps egui's default set. The Eskiz look puts Atkinson Hyperlegible Next
//! (SIL OFL 1.1, bundled unmodified, see `packaging/identity/fonts/README.md`) first in the proportional
//! family, with the egui fonts behind it for glyphs it lacks.
//!
//! egui 0.36 cannot switch on OpenType features, and the font's default digits are proportional,
//! so value readouts draw digits in fixed-width cells ([`paint_numeric`]).

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2, Rect, Vec2};

use crate::theme::UiIdentity;

const REGULAR: &[u8] = include_bytes!("../../../packaging/identity/fonts/AtkinsonHyperlegibleNext-Regular.ttf");
const SEMIBOLD: &[u8] = include_bytes!("../../../packaging/identity/fonts/AtkinsonHyperlegibleNext-SemiBold.ttf");

const REGULAR_NAME: &str = "AtkinsonHyperlegibleNext-Regular";
const SEMIBOLD_NAME: &str = "AtkinsonHyperlegibleNext-SemiBold";

/// The family used for headings in the Eskiz look.
pub const HEADING_FAMILY: &str = "eskiz-heading";

/// Font set of an identity.
pub fn definitions(identity: UiIdentity) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    if identity == UiIdentity::Eskiz {
        fonts.font_data.insert(REGULAR_NAME.to_owned(), Arc::new(FontData::from_static(REGULAR)));
        fonts.font_data.insert(SEMIBOLD_NAME.to_owned(), Arc::new(FontData::from_static(SEMIBOLD)));
        let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
        proportional.insert(0, REGULAR_NAME.to_owned());
        let mut heading = vec![SEMIBOLD_NAME.to_owned()];
        heading.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
        fonts.families.insert(FontFamily::Name(HEADING_FAMILY.into()), heading);
    }
    fonts
}

/// Install the fonts of `identity`.
pub fn install(ctx: &egui::Context, identity: UiIdentity) {
    ctx.set_fonts(definitions(identity));
}

/// The heading font for `size`, or `None` while the heading family is not yet bound in this pass.
///
/// `Context::set_fonts` only takes effect at the start of the next pass, but the palette flag
/// (`theme::is_eskiz`) flips at once. Text that names [`HEADING_FAMILY`] before the fonts have
/// arrived would panic ("family not bound"), so callers fall back to the proportional font.
pub fn heading_font(ctx: &egui::Context, size: f32) -> Option<FontId> {
    let family = FontFamily::Name(HEADING_FAMILY.into());
    ctx.fonts(|f| f.families().contains(&family)).then(|| FontId::new(size, family))
}

/// Width of one digit cell: the width of "0" in `font`.
pub fn digit_cell(painter: &Painter, font: &FontId) -> f32 {
    painter.layout_no_wrap("0".to_owned(), font.clone(), Color32::WHITE).size().x
}

/// Horizontal advances of the characters of `text`: digits take `cell`, others their own width.
pub fn advances(painter: &Painter, text: &str, font: &FontId, cell: f32) -> Vec<f32> {
    text.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                cell
            } else {
                painter.layout_no_wrap(c.to_string(), font.clone(), Color32::WHITE).size().x
            }
        })
        .collect()
}

/// Total width of `text` with fixed digit cells.
pub fn numeric_width(painter: &Painter, text: &str, font: &FontId) -> f32 {
    let cell = digit_cell(painter, font);
    advances(painter, text, font, cell).iter().sum()
}

/// Draw `text` centred on `center` with every digit centred in a cell of equal width, so the
/// readout does not jitter as the value changes.
pub fn paint_numeric(painter: &Painter, center: Pos2, text: &str, font: FontId, color: Color32) {
    let cell = digit_cell(painter, &font);
    let adv = advances(painter, text, &font, cell);
    let total: f32 = adv.iter().sum();
    let mut x = center.x - total / 2.0;
    for (c, a) in text.chars().zip(adv) {
        painter.text(Pos2::new(x + a / 2.0, center.y), Align2::CENTER_CENTER, c.to_string(), font.clone(), color);
        x += a;
    }
}

/// The rect a numeric readout covers (for tests and layout).
pub fn numeric_rect(painter: &Painter, center: Pos2, text: &str, font: &FontId) -> Rect {
    Rect::from_center_size(center, Vec2::new(numeric_width(painter, text, font), font.size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_painter<R>(identity: UiIdentity, f: impl FnOnce(&Painter) -> R) -> R {
        let ctx = egui::Context::default();
        install(&ctx, identity);
        let mut out = None;
        let mut f = Some(f);
        let mut full = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                out = Some(f(ui.painter()));
            }
        });
        full.textures_delta.clear();
        out.expect("ran")
    }

    #[test]
    fn classic_fonts_are_the_egui_defaults() {
        assert_eq!(definitions(UiIdentity::Classic).font_data.len(), FontDefinitions::default().font_data.len());
        let eskiz = definitions(UiIdentity::Eskiz);
        assert_eq!(eskiz.families[&FontFamily::Proportional][0], REGULAR_NAME);
        // The egui fonts stay behind it as fallback.
        assert!(eskiz.families[&FontFamily::Proportional].len() > 1);
        assert!(eskiz.families.contains_key(&FontFamily::Name(HEADING_FAMILY.into())));
    }

    #[test]
    fn bundled_font_files_match_their_recorded_checksums() {
        use sha2::{Digest, Sha256};
        let sums = include_str!("../../../packaging/identity/fonts/SHA256SUMS");
        for (name, bytes) in [
            ("AtkinsonHyperlegibleNext-Regular.ttf", REGULAR),
            ("AtkinsonHyperlegibleNext-SemiBold.ttf", SEMIBOLD),
            ("AtkinsonHyperlegibleNext-OFL.txt", include_bytes!("../../../packaging/identity/fonts/AtkinsonHyperlegibleNext-OFL.txt")),
        ] {
            let hash: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
            assert!(sums.lines().any(|l| l.starts_with(&hash) && l.ends_with(name)), "{name} has hash {hash}");
        }
    }

    /// The font's own digits have different widths; the numeric readout does not.
    #[test]
    fn numeric_readouts_have_fixed_width_digits() {
        with_painter(UiIdentity::Eskiz, |p| {
            let font = FontId::proportional(13.0);
            let raw = |t: &str| p.layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE).size().x;
            assert!((raw("111") - raw("808")).abs() > 1.0, "proportional digits differ in width");
            let w = |t: &str| numeric_width(p, t, &font);
            assert_eq!(w("111%"), w("808%"));
            assert_eq!(w("1%"), w("9%"));
            assert!(w("100%") > w("1%"));
        });
    }
}
