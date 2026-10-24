//! Behaviour tests for eskiz-text.
//!
//! The crate bundles no fonts. Tests that lay out text use the system fonts. When the host has
//! none, those tests print a note and return early.

use std::fs;
use std::path::{Path, PathBuf};

use eskiz_text::storage::{apply_saved, attrs_to_saved, SavedValue, KEY_FONT_SIZE, REFERENCE_KEYS};
use eskiz_text::{
    Affine, Alignment, Attrs, Color, FontError, FontFace, FontLibrary, TextLayer, TextLayerExt,
    WritingDirection,
};

fn fonts_or_skip() -> Option<FontLibrary> {
    let fonts = FontLibrary::new();
    if fonts.default_family().is_none() {
        eprintln!("no system fonts available; skipping");
        return None;
    }
    Some(fonts)
}

fn attrs(size: f32) -> Attrs {
    Attrs {
        size_pt: size,
        ..Attrs::default()
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("eskiz-text-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn first_system_font() -> Option<&'static str> {
    [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ]
    .into_iter()
    .find(|path| Path::new(path).exists())
}

#[test]
fn layout_metrics_grow_with_size_and_tracking() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let width = |fonts: &mut FontLibrary, size: f32, tracking: f32| {
        let mut a = attrs(size);
        a.tracking = tracking;
        TextLayer::new("Hello world", a, 10_000.0)
            .layout(fonts)
            .metrics
            .width
    };
    let (s12, s24, s48) = (
        width(&mut fonts, 12.0, 0.0),
        width(&mut fonts, 24.0, 0.0),
        width(&mut fonts, 48.0, 0.0),
    );
    assert!(s12 < s24 && s24 < s48, "size: {s12} {s24} {s48}");

    let (t0, t1, t2) = (
        width(&mut fonts, 40.0, 0.0),
        width(&mut fonts, 40.0, 0.1),
        width(&mut fonts, 40.0, 0.2),
    );
    assert!(t0 < t1 && t1 < t2, "tracking: {t0} {t1} {t2}");
}

#[test]
fn letter_spacing_adds_size_times_spacing_per_character() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let plain = TextLayer::new("Hello", attrs(100.0), 0.0).layout(&mut fonts);
    let mut spaced_attrs = attrs(100.0);
    spaced_attrs.kerning = 0.1;
    spaced_attrs.tracking = 0.1;
    let spaced = TextLayer::new("Hello", spaced_attrs, 0.0).layout(&mut fonts);

    assert_eq!(plain.glyphs.len(), 5);
    assert_eq!(spaced.glyphs.len(), 5);
    // Each adjacent pair is 100 x (0.1 + 0.1) = 20 units further apart than at zero spacing.
    for i in 0..4 {
        let gap = |l: &eskiz_text::Layout| l.glyphs[i + 1].origin.0 - l.glyphs[i].origin.0;
        let extra = gap(&spaced) - gap(&plain);
        assert!(
            (extra - 20.0).abs() <= 1.0,
            "pair {i}: extra advance {extra}"
        );
    }
}

#[test]
fn alignment_offsets_follow_the_box() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let box_w = 400.0;
    let line = |fonts: &mut FontLibrary, alignment: Alignment| {
        let mut a = attrs(20.0);
        a.alignment = alignment;
        TextLayer::new("Hi", a, box_w).layout(fonts)
    };
    let left = line(&mut fonts, Alignment::Left);
    let width = left.lines[0].width;
    assert!(width > 0.0);
    assert!(left.lines[0].x.abs() < 1e-3);

    let centre = line(&mut fonts, Alignment::Centre);
    assert!((centre.lines[0].x - (box_w - width) / 2.0).abs() < 1e-2);

    let right = line(&mut fonts, Alignment::Right);
    assert!((right.lines[0].x - (box_w - width)).abs() < 1e-2);
}

#[test]
fn justified_lines_stretch_except_the_last() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let mut a = attrs(20.0);
    a.alignment = Alignment::Justified;
    let probe = TextLayer::new("aaa bbb", attrs(20.0), 0.0).layout(&mut fonts);
    // Just wide enough for "aaa bbb", so "ccc" wraps to the next line.
    let box_w = probe.metrics.width + 2.0;
    let text = "aaa bbb ccc ddd";

    let justified = TextLayer::new(text, a.clone(), box_w).layout(&mut fonts);
    let mut left_attrs = a;
    left_attrs.alignment = Alignment::Left;
    let left = TextLayer::new(text, left_attrs, box_w).layout(&mut fonts);
    assert!(left.lines.len() >= 2, "text should wrap");

    // Glyph 4 is the "b" after the single inner space of the first line. Its shift is the slack.
    let slack = box_w - left.lines[0].width;
    let shift = justified.glyphs[4].origin.0 - left.glyphs[4].origin.0;
    assert!(
        (shift - slack).abs() < 0.5,
        "shift {shift} vs slack {slack}"
    );

    // The last line of the paragraph is not stretched.
    let last = justified.lines.len() - 1;
    assert!(justified.lines[last].x.abs() < 1e-3);
    assert!((justified.lines[last].width - left.lines[last].width).abs() < 1e-3);
}

#[test]
fn black_text_on_transparent_has_no_colour_fringes() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let raster = TextLayer::new("Hello, World", attrs(40.0), 0.0).rasterise(&mut fonts);
    assert!(raster.alpha_sum() > 0, "text must produce ink");
    for y in 0..raster.height {
        for x in 0..raster.width {
            let [r, g, b, a] = raster.pixel(x, y);
            assert!(r == g && g == b, "fringe at ({x}, {y}): {r} {g} {b} {a}");
            assert!(r <= a, "premultiplied color exceeds alpha at ({x}, {y})");
        }
    }
}

#[test]
fn vertical_text_is_taller_than_wide() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let mut vertical = attrs(40.0);
    vertical.vertical = true;
    let tall = TextLayer::new("Hello World", vertical, 0.0).rasterise(&mut fonts);
    let (x0, y0, x1, y1) = tall.ink_bounds().expect("vertical text has ink");
    assert!(
        y1 - y0 > x1 - x0,
        "vertical box {}x{} is not taller than wide",
        x1 - x0,
        y1 - y0
    );

    let wide = TextLayer::new("Hello World", attrs(40.0), 0.0).rasterise(&mut fonts);
    let (x0, y0, x1, y1) = wide.ink_bounds().expect("horizontal text has ink");
    assert!(x1 - x0 > y1 - y0);
}

#[test]
fn glyphs_sit_above_the_baseline() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let mut layer = TextLayer::new("l", attrs(100.0), 0.0);
    layer.transform = Affine::translation(50.0, 200.0);
    // The first baseline sits one ascent below the layer's top edge, so measure from it.
    let baseline = 200.0 + layer.layout(&mut fonts).glyphs[0].origin.1;
    let raster = layer.rasterise(&mut fonts);
    let (_, y0, _, y1) = raster.ink_bounds().expect("an 'l' has ink");
    // An 'l' has no descender, so its ink ends at the baseline and rises above it.
    assert!(
        f64::from(y1) <= f64::from(baseline) + 1.5,
        "ink extends below the baseline: bottom {y1}, baseline {baseline}"
    );
    assert!(
        f64::from(y0) < f64::from(baseline) - 50.0,
        "ink does not rise above the baseline: top {y0}, baseline {baseline}"
    );
}

#[test]
fn outline_increases_coverage() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let plain = TextLayer::new("Oi", attrs(40.0), 0.0).rasterise(&mut fonts);
    let mut outlined_attrs = attrs(40.0);
    outlined_attrs.outline_pt = 4.0;
    let outlined = TextLayer::new("Oi", outlined_attrs, 0.0).rasterise(&mut fonts);
    assert!(
        outlined.alpha_sum() > plain.alpha_sum() + plain.alpha_sum() / 10,
        "outline {} vs plain {}",
        outlined.alpha_sum(),
        plain.alpha_sum()
    );
    assert!(
        outlined.width > plain.width,
        "the gutter must widen the raster"
    );
}

#[test]
fn raster_at_scale_is_redrawn_not_enlarged() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let layer = TextLayer::new("Hello", attrs(30.0), 0.0);
    let one = layer.rasterise(&mut fonts);
    let three = layer.rasterise_at_scale(&mut fonts, 3.0);
    let ratio = f64::from(three.width) / f64::from(one.width);
    assert!((ratio - 3.0).abs() < 0.3, "width ratio {ratio}");
    assert!(
        three.alpha_sum() > one.alpha_sum() * 5,
        "3x raster should carry about 9x the ink"
    );
}

#[test]
fn fallback_picks_the_default_font_for_a_missing_family() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let missing = "No Such Family Qzx";
    let resolution = fonts.resolve(missing);
    assert!(resolution.missing);
    assert_eq!(Some(resolution.family.as_str()), fonts.default_family());
    assert!((resolution.size_scale - 1.0).abs() < f32::EPSILON);

    let mut missing_attrs = attrs(40.0);
    missing_attrs.font_family = missing.to_string();
    let layout = TextLayer::new("Hi", missing_attrs, 0.0).layout(&mut fonts);
    assert_eq!(layout.missing_fonts, vec![missing.to_string()]);

    // Same glyphs as the default family at the same size.
    let default_layout = TextLayer::new("Hi", attrs(40.0), 0.0).layout(&mut fonts);
    let ids = |l: &eskiz_text::Layout| {
        l.glyphs
            .iter()
            .map(|g| (g.font_id, g.glyph_id))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&layout), ids(&default_layout));
    assert!(layout
        .glyphs
        .iter()
        .all(|g| (g.font_size - 40.0).abs() < 1e-3));
}

#[test]
fn replacement_scales_the_size_of_the_run() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let default = fonts.default_family().expect("checked").to_string();
    let missing = "No Such Family Qzx";
    fonts
        .set_replacement(missing, &default, 0.9)
        .expect("default family is installed");

    let mut a = attrs(40.0);
    a.font_family = missing.to_string();
    let layout = TextLayer::new("Hi", a, 0.0).layout(&mut fonts);
    assert!(
        layout
            .glyphs
            .iter()
            .all(|g| (g.font_size - 36.0).abs() < 1e-3),
        "replaced run should be 0.9 x 40"
    );
    assert_eq!(layout.missing_fonts, vec![missing.to_string()]);
}

#[test]
fn attribute_keys_round_trip() {
    let original = Attrs {
        font_family: "Ab-Cd Sans".to_string(),
        font_face: FontFace::BoldItalic,
        size_pt: 37.5,
        kerning: 0.125,
        tracking: -0.25,
        leading: 1.5,
        baseline_shift_pt: 10.0,
        alignment: Alignment::Justified,
        outline_pt: 4.0,
        underline: true,
        color: Color::rgba(12, 34, 56, 78),
        all_caps: true,
        vertical: true,
        writing_direction: WritingDirection::LeftToRight,
    };
    let saved = attrs_to_saved(&original);
    for key in REFERENCE_KEYS {
        assert!(
            saved.iter().any(|(k, _)| *k == key),
            "missing reference key {key}"
        );
    }

    let mut restored = Attrs::default();
    for (key, value) in &saved {
        assert_eq!(
            apply_saved(&mut restored, key, value),
            Ok(true),
            "key {key}"
        );
    }
    assert_eq!(restored, original);

    // Unknown keys are dropped, not errors.
    assert_eq!(
        apply_saved(&mut restored, "_CFX", &SavedValue::Number(1.0)),
        Ok(false)
    );
    // A value of the wrong type is an error.
    assert!(apply_saved(&mut restored, KEY_FONT_SIZE, &SavedValue::Text("12".into())).is_err());
}

#[test]
fn set_attrs_changes_only_the_selected_characters() {
    let mut layer = TextLayer::new("abcdef", attrs(12.0), 0.0);
    layer.set_attrs(2..4, |a| a.size_pt = 30.0);
    let size_at = |layer: &TextLayer, byte: usize| layer.attrs_at(byte).expect("has runs").size_pt;
    assert_eq!(size_at(&layer, 1), 12.0);
    assert_eq!(size_at(&layer, 2), 30.0);
    assert_eq!(size_at(&layer, 3), 30.0);
    assert_eq!(size_at(&layer, 4), 12.0);
}

#[test]
fn import_rejects_an_invalid_file_and_names_it() {
    let mut fonts = FontLibrary::new();
    let dir = temp_dir("import-invalid");
    let path = dir.join("bogus.ttf");
    fs::write(&path, b"this is not a font").expect("write fixture");
    let error = fonts
        .import_file(&path)
        .expect_err("garbage must be rejected");
    assert!(matches!(error, FontError::Invalid { .. }), "{error:?}");
    assert!(
        error.to_string().contains("bogus.ttf"),
        "message should name the file: {error}"
    );
}

#[test]
fn folder_import_loads_fonts_and_reports_bad_ones() {
    let Some(source) = first_system_font() else {
        eprintln!("no system font file found; skipping");
        return;
    };
    let dir = temp_dir("folder");
    fs::copy(source, dir.join("copied.ttf")).expect("copy font fixture");
    fs::write(dir.join("notes.txt"), "not a font, ignored").expect("write fixture");
    fs::write(dir.join("broken.otf"), b"junk").expect("write fixture");

    let mut fonts = FontLibrary::new();
    let report = fonts.load_dir(&dir).expect("folder readable");
    assert!(!report.families.is_empty(), "the copied font should load");
    assert_eq!(report.rejected.len(), 1, "only broken.otf is rejected");
    assert!(fonts.has_family(&report.families[0]));
}

#[test]
fn import_accepts_a_valid_font_file() {
    let Some(source) = first_system_font() else {
        eprintln!("no system font file found; skipping");
        return;
    };
    let dir = temp_dir("import-valid");
    let path = dir.join("imported.ttf");
    fs::copy(source, &path).expect("copy font fixture");
    let mut fonts = FontLibrary::new();
    let families = fonts.import_file(&path).expect("valid font imports");
    assert!(!families.is_empty());
    assert!(fonts.has_family(&families[0]));
}

#[test]
fn empty_layer_still_has_one_line() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let layout = TextLayer::new("", attrs(20.0), 0.0).layout(&mut fonts);
    assert_eq!(layout.metrics.line_count, 1);
    assert!(layout.metrics.height > 0.0);
    assert!(layout.glyphs.is_empty());
    let trailing = TextLayer::new("a\n", attrs(20.0), 0.0).layout(&mut fonts);
    assert_eq!(
        trailing.metrics.line_count, 2,
        "a trailing line break adds an empty final line"
    );
}

#[test]
fn writing_direction_default_is_left_to_right() {
    assert_eq!(
        Attrs::default().writing_direction,
        WritingDirection::LeftToRight
    );
    assert_eq!(Attrs::default().size_pt, 12.0);
}

#[test]
fn android_style_font_folder_gives_a_face_without_any_bundled_font() {
    // Stand-in for /system/fonts: one copied face in an otherwise empty library.
    let Some(source) = first_system_font() else { return };
    let dir = std::env::temp_dir().join(format!("eskiz-sysfonts-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create folder");
    fs::copy(source, dir.join("Roboto-Regular.ttf")).expect("copy font fixture");
    let missing = dir.join("no-such-folder");
    let library = FontLibrary::with_font_dirs([missing.as_path(), dir.as_path()]);
    assert!(!library.families().is_empty(), "the folder's face is listed");
    assert!(library.default_family().is_some(), "a default family is chosen from it");
    let empty = FontLibrary::with_font_dirs([missing.as_path()]);
    assert!(empty.families().is_empty());
    assert!(empty.default_family().is_none());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn absurd_scale_gives_an_empty_raster() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let layer = TextLayer::new("Hello", attrs(1000.0), 10_000.0);
    let r = layer.rasterise_at_scale(&mut fonts, 1.0e6);
    assert_eq!((r.width, r.height), (0, 0));
}

#[test]
fn outline_width_is_limited_by_the_text_size() {
    let a = Attrs {
        size_pt: 20.0,
        outline_pt: 1.0e6,
        ..Attrs::default()
    }
    .clamped();
    assert!(a.outline_pt <= 10.0);
}

#[test]
fn later_lines_keep_their_own_run_and_text_offsets() {
    let Some(mut fonts) = fonts_or_skip() else {
        return;
    };
    let mut layer = TextLayer::new("Hello\nWorld", attrs(20.0), 0.0);
    let red = Color { r: 255, g: 0, b: 0, a: 255 };
    layer.set_attrs(6..11, |a| a.color = red);
    let layout = layer.layout(&mut fonts);
    let second: Vec<_> = layout.glyphs.iter().filter(|g| g.cluster >= 6).collect();
    assert!(!second.is_empty(), "glyphs of line 2 are numbered from the start of the text");
    assert!(second.iter().all(|g| g.color == red), "line 2 takes its own run's colour");
    assert!(layout.glyphs.iter().filter(|g| g.cluster < 5).all(|g| g.color != red));
}
