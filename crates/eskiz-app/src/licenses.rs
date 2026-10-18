//! Help > About Eskiz > Licenses: the generated `THIRD_PARTY_NOTICES.md`, embedded at build time.
//!
//! The file starts with Eskiz's own licence and where to get the source, the MPL-2.0 source offer, the
//! Independent JPEG Group sentence and the font licences, then the full list of crates and licence
//! texts. The screen shows it as is, scrollable, with a search box that finds a package by name.

use std::sync::OnceLock;

use eframe::egui::{self, Id, RichText};

use crate::chrome::small_label;
use crate::theme::Palette;

/// The notices file made by `tools/gen_notices.sh` (cargo-about). Do not edit it by hand.
pub(crate) const NOTICES: &str = include_str!("../../../THIRD_PARTY_NOTICES.md");

/// Columns a line is wrapped to before it is drawn (the rows have a fixed height).
const WRAP_COLUMNS: usize = 84;
const ROW_HEIGHT: f32 = 14.5;

/// The notices as display rows: fences dropped, the hard-wrapped prose outside the licence texts joined
/// into paragraphs, long lines wrapped again to the sheet width.
fn rows() -> &'static [String] {
    static ROWS: OnceLock<Vec<String>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let mut out = Vec::new();
        let mut in_fence = false;
        let mut para = String::new();
        let shown = user_facing(NOTICES);
        for line in shown.lines() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                wrap_into(line, &mut out);
                continue;
            }
            let plain = !line.trim().is_empty() && !line.starts_with('#') && !line.starts_with("- ");
            if plain && !para.is_empty() {
                para.push(' ');
                para.push_str(line.trim());
                continue;
            }
            if !para.is_empty() {
                wrap_into(&std::mem::take(&mut para), &mut out);
            }
            if plain || line.starts_with("- ") {
                para = line.to_owned();
            } else {
                wrap_into(line, &mut out);
            }
        }
        if !para.is_empty() {
            wrap_into(&para, &mut out);
        }
        out
    })
}

/// The notices as the screen shows them: the build-time paragraphs (how the file is generated, per-target
/// differences) are replaced by one sentence, an unsubstituted `evarnimtb-netizen/eskiz-app` source line (private and
/// development builds; the public snapshot fills it in) becomes plain text, and the Markdown backticks outside
/// the licence texts are dropped.
pub(crate) fn user_facing(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    let mut skipping = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if skipping {
            if line.trim().is_empty() {
                skipping = false;
            }
            continue;
        }
        if line.starts_with("This file lists the third-party software") {
            out.push_str("Below are the licenses of the third-party software and fonts included in Eskiz.\n\n");
            skipping = true;
            continue;
        }
        if line.starts_with("Per-target differences:") {
            skipping = true;
            continue;
        }
        if line.starts_with("Source code:") && line.contains("evarnimtb-netizen/eskiz-app") {
            out.push_str("Source code: see the Eskiz project page.\n");
            continue;
        }
        out.push_str(&line.replace('`', ""));
        out.push('\n');
    }
    out
}

/// Splits `line` at spaces into pieces of at most [`WRAP_COLUMNS`] characters (a longer word stays whole).
fn wrap_into(line: &str, out: &mut Vec<String>) {
    let indent: String = line.chars().take_while(|c| *c == ' ').collect();
    let mut current = String::new();
    for word in line.split(' ').filter(|w| !w.is_empty()) {
        let need = if current.is_empty() { indent.chars().count() } else { current.chars().count() + 1 } + word.chars().count();
        if !current.is_empty() && need > WRAP_COLUMNS {
            out.push(std::mem::take(&mut current));
        }
        if current.is_empty() {
            current.push_str(&indent);
        } else {
            current.push(' ');
        }
        current.push_str(word);
    }
    out.push(current);
}

/// One package entry of the full list: `name version`, its upstream address, and the licence it is listed under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub name: String,
    pub version: String,
    pub licence: String,
}

/// Every `- name version (url)` row under a `### Licence` heading of the "Licence texts" part.
pub(crate) fn entries(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut in_texts = false;
    let mut licence = String::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if line.starts_with("## ") {
            in_texts = line.starts_with("## Licence texts");
        } else if in_texts && line.starts_with("### ") {
            licence = line[4..].trim().to_owned();
        } else if in_texts && licence.as_str() != "" {
            if let Some(rest) = line.strip_prefix("- ") {
                let mut parts = rest.split(' ');
                if let (Some(name), Some(version)) = (parts.next(), parts.next()) {
                    out.push(Entry { name: name.to_owned(), version: version.to_owned(), licence: licence.clone() });
                }
            }
        }
    }
    out
}

fn index() -> &'static [Entry] {
    static INDEX: OnceLock<Vec<Entry>> = OnceLock::new();
    INDEX.get_or_init(|| entries(NOTICES))
}

/// Entries whose package name contains `query` (case-insensitive, ignoring surrounding spaces).
pub(crate) fn search<'a>(index: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    index.iter().filter(|e| e.name.to_lowercase().contains(&q)).collect()
}

/// The licences sheet body: search box, then the notices (or the search results).
pub(crate) fn body(ui: &mut egui::Ui, pal: &Palette, max_height: f32) {
    let id = Id::new("licenses-query");
    let mut query = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
    ui.label(RichText::new("Licenses").size(20.0).strong().color(pal.text));
    ui.label(
        RichText::new("Eskiz is MIT OR Apache-2.0. These are the licenses of the software and fonts it includes.")
            .size(12.0)
            .color(pal.text_dim),
    );
    ui.add_space(8.0);
    let edit = ui.add(egui::TextEdit::singleline(&mut query).hint_text("Search packages by name").desired_width(ui.available_width()));
    if edit.changed() {
        ui.data_mut(|d| d.insert_temp(id, query.clone()));
    }
    ui.add_space(6.0);
    let height = (max_height - 150.0).max(120.0);
    let hits = search(index(), &query);
    if query.trim().is_empty() {
        let lines = rows();
        egui::ScrollArea::vertical().max_height(height).auto_shrink([false, false]).show_rows(ui, ROW_HEIGHT, lines.len(), |ui, range| {
            for line in &lines[range] {
                notice_row(ui, pal, line);
            }
        });
    } else {
        small_label(ui, pal, &format!("{} MATCHING PACKAGES", hits.len()));
        egui::ScrollArea::vertical().max_height(height).auto_shrink([false, false]).show_rows(ui, ROW_HEIGHT, hits.len(), |ui, range| {
            for e in &hits[range] {
                ui.label(RichText::new(format!("{} {}   {}", e.name, e.version, e.licence)).monospace().size(11.0).color(pal.text));
            }
        });
    }
}

fn notice_row(ui: &mut egui::Ui, pal: &Palette, line: &str) {
    let (text, size, strong) = if let Some(h) = line.strip_prefix("### ") {
        (h, 12.5, true)
    } else if let Some(h) = line.strip_prefix("## ") {
        (h, 14.0, true)
    } else if let Some(h) = line.strip_prefix("# ") {
        (h, 16.0, true)
    } else {
        (line, 11.0, false)
    };
    let mut rt = RichText::new(text).size(size).color(if strong { pal.text } else { pal.text_dim });
    if !strong {
        rt = rt.monospace();
    } else {
        rt = rt.strong();
    }
    ui.add(egui::Label::new(rt).wrap_mode(egui::TextWrapMode::Extend));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::process::Command;

    #[test]
    fn notices_are_ordered_and_hold_the_required_text() {
        let at = |needle: &str| NOTICES.find(needle).unwrap_or_else(|| panic!("notices lack {needle:?}"));
        let ijg = "This software is based in part on the work of the Independent JPEG Group.";
        let order = [
            at("## Eskiz"),
            at("MIT OR Apache-2.0"),
            at("Source code: <https://github.com/"),
            at("## MPL-2.0 source offer"),
            at(ijg),
            at("## Bundled fonts"),
            at("## Licence summary"),
            at("## Licence texts and the crates that use them"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "sections out of order: {order:?}");
        for needle in [
            "UBUNTU FONT LICENCE Version 1.0",
            "SIL OPEN FONT LICENSE Version 1.1",
            "Copyright 2013 Google Inc.",
            "Copyright 2011 Canonical Ltd",
            "Copyright 2018 Source Foundry Authors",
            "BITSTREAM VERA LICENSE",
            "The work in the DejaVu project was committed to the public domain.",
            "Copyright (c) 2014 John Slegers",
            "Copyright 2020-2024 The Atkinson Hyperlegible Next Project Authors",
            "Mozilla Public License Version 2.0",
            "https://crates.io/crates/option-ext/0.2.0",
            "Unicode License v3",
        ] {
            at(needle);
        }
    }

    /// Package names and versions linked into `eskiz-app` (default features, normal dependencies) on one
    /// target, from `cargo tree` (which, unlike `cargo metadata`, resolves features for this package alone).
    fn linked(target: &str) -> BTreeSet<(String, String)> {
        let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["tree", "--locked", "-p", "eskiz-app", "-e", "normal", "--prefix", "none", "--target", target, "-f", "{p}"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("run cargo tree");
        assert!(out.status.success(), "cargo tree: {}", String::from_utf8_lossy(&out.stderr));
        let mut result = BTreeSet::new();
        for line in String::from_utf8(out.stdout).unwrap().lines() {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(version)) = (parts.next(), parts.next()) else { continue };
            // Workspace crates show a path, e.g. `eskiz-core v0.1.0 (/src/eskiz/crates/eskiz-core)`.
            if parts.next().is_some_and(|p| p.starts_with("(/") || p.starts_with("(\\") || p.chars().nth(2) == Some(':')) {
                continue;
            }
            result.insert((name.to_owned(), version.trim_start_matches('v').to_owned()));
        }
        result
    }

    #[test]
    #[cfg_attr(windows, ignore = "spawns `cargo tree`, which a cross-built Windows test binary cannot run (Wine CI); covered on macOS and Linux")]
    fn every_linked_package_is_in_the_notices() {
        let listed: BTreeSet<(String, String)> = entries(NOTICES).into_iter().map(|e| (e.name, e.version)).collect();
        assert!(listed.len() > 200, "the entries index looks empty: {}", listed.len());
        let mut missing = Vec::new();
        let mut total = 0;
        for target in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "aarch64-apple-darwin"] {
            for pkg in linked(target) {
                total += 1;
                if !listed.contains(&pkg) {
                    missing.push(format!("{} {} ({target})", pkg.0, pkg.1));
                }
            }
        }
        assert!(total > 250, "metadata walk found only {total} packages");
        assert!(missing.is_empty(), "packages linked into eskiz-app but missing from THIRD_PARTY_NOTICES.md:\n{}", missing.join("\n"));
    }

    #[test]
    #[cfg_attr(windows, ignore = "spawns `cargo tree`, which a cross-built Windows test binary cannot run (Wine CI); covered on macOS and Linux")]
    fn openh264_is_not_in_the_default_build_notices() {
        for target in ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"] {
            let names: Vec<String> = linked(target).into_iter().map(|p| p.0).collect();
            assert!(!names.iter().any(|n| n.starts_with("openh264")), "openh264 is linked by default on {target}");
        }
    }

    #[test]
    fn search_finds_packages_by_name_ignoring_case() {
        let idx = index();
        let hits = search(idx, "  OPTION-ext ");
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].licence.contains("Mozilla Public License"), "{:?}", hits[0]);
        assert!(search(idx, "").is_empty());
        assert!(search(idx, "no-such-crate-xyz").is_empty());
        assert!(search(idx, "jpeg").iter().any(|e| e.name == "jpeg-encoder"));
    }

    #[test]
    fn the_screen_shows_user_text_not_build_text() {
        let shown = rows().join("\n");
        for bad in ["evarnimtb-netizen/eskiz-app", "cargo about", "do not edit by hand", "Per-target differences", "APK", '`'.to_string().as_str()] {
            let head: String = shown.lines().take(30).collect::<Vec<_>>().join("\n");
            assert!(!head.contains(bad), "the notices header still shows {bad:?}:\n{head}");
        }
        assert!(shown.contains("Source code:"));
        assert!(shown.contains("Below are the licenses"));
        let sample = "Source code: <https://github.com/evarnimtb-netizen/eskiz-app>\n\nPer-target differences: x\ny\n\n## Next\n";
        let out = user_facing(sample);
        assert!(out.contains("see the Eskiz project page") && !out.contains("Per-target") && out.contains("## Next"));
        assert!(user_facing("Source code: <https://github.com/acme/eskiz>").contains("github.com/acme/eskiz"));
    }

    #[test]
    fn long_lines_are_wrapped_and_nothing_is_lost() {
        let mut out = Vec::new();
        let text = "word ".repeat(60);
        wrap_into(text.trim_end(), &mut out);
        assert!(out.len() > 1 && out.iter().all(|l| l.chars().count() <= WRAP_COLUMNS));
        assert_eq!(out.join(" ").split(' ').count(), 60);
        assert!(rows().len() > 1000);
    }

    /// Headless snapshots of the screen: the notices from the top, and a search for "icu".
    #[test]
    fn writes_licenses_snapshots() {
        use crate::document::Document;
        use crate::sheets::Sheet;
        use crate::snapshot::{snapshot_dir, Headless};
        use crate::app::Prefs;
        let mut h = Headless::new(egui::Vec2::new(900.0, 900.0), Document::new(400, 300), Prefs::default());
        h.app.sheet = Some(Sheet::Licenses);
        h.advance(0.3);
        let plain = h.render();
        plain.save_png(&snapshot_dir().join("sheet_licenses.png")).unwrap();
        h.ctx.data_mut(|d| d.insert_temp(Id::new("licenses-query"), "icu".to_owned()));
        h.advance(0.3);
        let found = h.render();
        found.save_png(&snapshot_dir().join("sheet_licenses_search.png")).unwrap();
        let differing = (0..900).step_by(3).flat_map(|y| (0..900).step_by(3).map(move |x| (x, y))).filter(|&(x, y)| plain.pixel(x, y) != found.pixel(x, y)).count();
        assert!(differing > 200, "search results should change the card ({differing} sampled pixels differ)");
    }
}
