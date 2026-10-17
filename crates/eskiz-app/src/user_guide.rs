//! The User guide: a two-pane window (chapters on the left, rendered Markdown on the right)
//! that Actions > Help > User guide opens, and the "?" links that open one chapter from a panel.
//!
//! The chapters are the Markdown files in `docs/guide/`, embedded with `include_str!`, so the
//! guide works offline and the repository copy is the copy the app shows. The Gestures and
//! shortcuts chapter has a generated block between the `generated:shortcuts` markers. It is built
//! from [`crate::shortcuts`] and [`crate::sheets::cheat_rows`], and a test keeps it in step with
//! the code.
//!
//! The renderer covers what the chapters use: headings, paragraphs, bullet and numbered lists,
//! tables, fenced code, and **bold** and `code` inside text. HTML comments are not shown.

use eframe::egui::text::LayoutJob;
use eframe::egui::{self, Align, CornerRadius, FontId, Layout, Margin, RichText, TextFormat, Vec2};

use crate::app::EskizApp;
use crate::theme::Palette;

/// Chapter ids, which are the file names in `docs/guide/` without `.md`.
pub mod ch {
    pub const GETTING_STARTED: &str = "getting-started";
    pub const PAINTING: &str = "painting";
    pub const LAYERS: &str = "layers";
    pub const SELECTION: &str = "selection-and-transform";
    pub const ADJUSTMENTS: &str = "adjustments-and-liquify";
    pub const COLOUR: &str = "colour";
    pub const GUIDES: &str = "drawing-guides-and-reference";
    pub const TEXT: &str = "text";
    pub const ANIMATION: &str = "animation-and-timelapse";
    pub const THREE_D: &str = "3d-painting";
    pub const IMPORT_EXPORT: &str = "import-and-export";
    pub const GESTURES: &str = "gestures-and-shortcuts";
    pub const PREFERENCES: &str = "preferences";
    pub const TROUBLESHOOTING: &str = "troubleshooting";
}

/// One chapter: its id and its Markdown source.
pub struct Chapter {
    pub id: &'static str,
    pub body: &'static str,
}

impl Chapter {
    /// The title: the first level-1 heading of the chapter.
    pub fn title(&self) -> String {
        parse(self.body)
            .into_iter()
            .find_map(|b| match b {
                Block::Heading { level: 1, text } => Some(text),
                _ => None,
            })
            .unwrap_or_else(|| self.id.to_owned())
    }
}

/// The chapters, in the order of the sidebar.
pub const CHAPTERS: &[Chapter] = &[
    Chapter { id: ch::GETTING_STARTED, body: include_str!("../../../docs/guide/getting-started.md") },
    Chapter { id: ch::PAINTING, body: include_str!("../../../docs/guide/painting.md") },
    Chapter { id: ch::LAYERS, body: include_str!("../../../docs/guide/layers.md") },
    Chapter { id: ch::SELECTION, body: include_str!("../../../docs/guide/selection-and-transform.md") },
    Chapter { id: ch::ADJUSTMENTS, body: include_str!("../../../docs/guide/adjustments-and-liquify.md") },
    Chapter { id: ch::COLOUR, body: include_str!("../../../docs/guide/colour.md") },
    Chapter { id: ch::GUIDES, body: include_str!("../../../docs/guide/drawing-guides-and-reference.md") },
    Chapter { id: ch::TEXT, body: include_str!("../../../docs/guide/text.md") },
    Chapter { id: ch::ANIMATION, body: include_str!("../../../docs/guide/animation-and-timelapse.md") },
    Chapter { id: ch::THREE_D, body: include_str!("../../../docs/guide/3d-painting.md") },
    Chapter { id: ch::IMPORT_EXPORT, body: include_str!("../../../docs/guide/import-and-export.md") },
    Chapter { id: ch::GESTURES, body: include_str!("../../../docs/guide/gestures-and-shortcuts.md") },
    Chapter { id: ch::PREFERENCES, body: include_str!("../../../docs/guide/preferences.md") },
    Chapter { id: ch::TROUBLESHOOTING, body: include_str!("../../../docs/guide/troubleshooting.md") },
];

/// The index of the chapter with `id`.
pub fn index_of(id: &str) -> Option<usize> {
    CHAPTERS.iter().position(|c| c.id == id)
}

/// A "?" link in a panel. Each site names the place it opens, so the link and the chapter it
/// opens are one entry here, and the tests check that each one lands on a chapter about it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    Liquify,
    Selection,
    Transform,
    BrushLibrary,
    Colour,
    Layers,
}

impl Link {
    pub const ALL: [Link; 6] = [Link::Liquify, Link::Selection, Link::Transform, Link::BrushLibrary, Link::Colour, Link::Layers];

    /// The chapter the link opens.
    pub fn chapter(self) -> &'static str {
        match self {
            Link::Liquify => ch::ADJUSTMENTS,
            Link::Selection | Link::Transform => ch::SELECTION,
            Link::BrushLibrary => ch::PAINTING,
            Link::Colour => ch::COLOUR,
            Link::Layers => ch::LAYERS,
        }
    }

    /// A word the opened chapter must mention, so the test can tell the right chapter.
    pub fn topic(self) -> &'static str {
        match self {
            Link::Liquify => "Liquify",
            Link::Selection => "Selection",
            Link::Transform => "Transform",
            Link::BrushLibrary => "Brush Library",
            Link::Colour => "Color",
            Link::Layers => "Layers",
        }
    }
}

// ----------------------------------------------------------------------------------------------
// Markdown

/// One block of a chapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, text: String },
    Paragraph(String),
    List { ordered: bool, items: Vec<String> },
    Table { header: Vec<String>, rows: Vec<Vec<String>> },
    Code(String),
}

/// Parse the Markdown the chapters use into blocks.
pub fn parse(md: &str) -> Vec<Block> {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = Vec::new();
    let mut para = String::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if t.starts_with("<!--") {
            // An HTML comment (the generated-block markers). Skip to the line that closes it.
            while i < lines.len() && !lines[i].contains("-->") {
                i += 1;
            }
            i += 1;
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut out);
            i += 1;
            continue;
        }
        if t.starts_with("```") {
            flush(&mut para, &mut out);
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            i += 1;
            out.push(Block::Code(code.join("\n")));
            continue;
        }
        if let Some((level, text)) = heading(t) {
            flush(&mut para, &mut out);
            out.push(Block::Heading { level, text: text.to_owned() });
            i += 1;
            continue;
        }
        if t.starts_with('|') && i + 1 < lines.len() && is_separator(lines[i + 1]) {
            flush(&mut para, &mut out);
            let header = split_row(t);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('|') {
                rows.push(split_row(lines[i].trim()));
                i += 1;
            }
            out.push(Block::Table { header, rows });
            continue;
        }
        if let Some((ordered, first)) = list_item(t) {
            flush(&mut para, &mut out);
            let mut items = vec![first.to_owned()];
            i += 1;
            // Indented lines that are not a new item continue the item before them, and the
            // next item of the same kind belongs to the same list.
            while i < lines.len() {
                let raw = lines[i];
                let s = raw.trim();
                if !raw.starts_with(' ') {
                    if let Some((o, text)) = list_item(s) {
                        if o == ordered {
                            items.push(text.to_owned());
                            i += 1;
                            continue;
                        }
                    }
                }
                let continues = raw.starts_with(' ')
                    && !s.is_empty()
                    && list_item(s).is_none()
                    && !s.starts_with('|')
                    && !s.starts_with('#')
                    && !s.starts_with("```");
                if !continues {
                    break;
                }
                let last = items.last_mut().expect("a list has an item");
                last.push(' ');
                last.push_str(s);
                i += 1;
            }
            out.push(Block::List { ordered, items });
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(t);
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

fn flush(para: &mut String, out: &mut Vec<Block>) {
    if !para.is_empty() {
        out.push(Block::Paragraph(std::mem::take(para)));
    }
}

/// `## Text` gives (2, "Text").
fn heading(t: &str) -> Option<(u8, &str)> {
    let n = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&n) {
        t[n..].strip_prefix(' ').map(|rest| (n as u8, rest.trim()))
    } else {
        None
    }
}

/// `- item` gives (false, "item"); `3. item` gives (true, "item").
fn list_item(t: &str) -> Option<(bool, &str)> {
    if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        return Some((false, rest));
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        return t[digits..].strip_prefix(". ").map(|rest| (true, rest));
    }
    None
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn split_row(t: &str) -> Vec<String> {
    t.trim().trim_start_matches('|').trim_end_matches('|').split('|').map(|c| c.trim().to_owned()).collect()
}

/// The text of every block, for search. A table row is joined with " | ".
fn block_texts(block: &Block) -> Vec<String> {
    match block {
        Block::Heading { text, .. } => vec![text.clone()],
        Block::Paragraph(t) | Block::Code(t) => vec![t.clone()],
        Block::List { items, .. } => items.clone(),
        Block::Table { header, rows } => std::iter::once(header.join(" | "))
            .chain(rows.iter().map(|r| r.join(" | ")))
            .collect(),
    }
}

/// One search result: the chapter and a short piece of text around the match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub chapter: usize,
    pub snippet: String,
}

/// Most results a search shows.
const MAX_HITS: usize = 60;

/// Every place in every chapter that contains `query`, ignoring case. An empty query finds nothing.
pub fn search(query: &str) -> Vec<Hit> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (chapter, c) in CHAPTERS.iter().enumerate() {
        for block in parse(c.body) {
            for text in block_texts(&block) {
                // Show and match the words, not the Markdown marks around them.
                let text = text.replace("**", "").replace('`', "");
                // ASCII lowercasing keeps byte offsets, so the match position is valid in `text`.
                if let Some(pos) = text.to_ascii_lowercase().find(&q) {
                    hits.push(Hit { chapter, snippet: snippet(&text, pos, q.len()) });
                    if hits.len() == MAX_HITS {
                        return hits;
                    }
                }
            }
        }
    }
    hits
}

/// Up to 60 characters either side of the match, cut on character boundaries.
fn snippet(text: &str, pos: usize, len: usize) -> String {
    const CONTEXT: usize = 60;
    let mut start = pos.saturating_sub(CONTEXT);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + len + CONTEXT).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let mut s = String::new();
    if start > 0 {
        s.push_str("...");
    }
    s.push_str(&text[start..end]);
    if end < text.len() {
        s.push_str("...");
    }
    s
}

/// The generated shortcut block of the Gestures and shortcuts chapter: the same rows the
/// Gestures & shortcuts sheet shows, with every gesture on (the defaults). `cmd` names the
/// command key.
pub fn generated_shortcuts_markdown(cmd: &str) -> String {
    const BEGIN: &str = "<!-- generated:shortcuts:begin: from crates/eskiz-app/src/user_guide.rs; do not edit -->";
    const END: &str = "<!-- generated:shortcuts:end -->";
    let mut md = String::new();
    md.push_str(BEGIN);
    md.push('\n');
    for (title, rows) in crate::sheets::cheat_rows(&crate::app::Prefs::default(), cmd) {
        let heading = match title {
            "TOUCH" => "Touch",
            "KEYBOARD" => "Keyboard",
            _ => "Canvas and pen",
        };
        let column = if title == "KEYBOARD" { "Keys" } else if title == "TOUCH" { "Gesture" } else { "Input" };
        md.push_str(&format!("\n### {heading}\n\n| {column} | What it does |\n|---|---|\n"));
        for (keys, what, on) in rows {
            let off = if on { "" } else { " (off)" };
            md.push_str(&format!("| {keys} | {what}{off} |\n"));
        }
    }
    md.push('\n');
    md.push_str(END);
    md.push('\n');
    md
}

// ----------------------------------------------------------------------------------------------
// Drawing

/// The text of a paragraph, with **bold** and `code` styled. Bold has no heavier face in the
/// bundled fonts, so it is set in the text colour and code in a monospace face on a panel tint.
fn inline_job(text: &str, size: f32, pal: &Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut code = false;
    let mut buf = String::new();
    let mut rest = text;
    let flush_buf = |job: &mut LayoutJob, buf: &mut String, code: bool| {
        if buf.is_empty() {
            return;
        }
        let format = if code {
            TextFormat { font_id: FontId::monospace(size - 1.0), color: pal.text, background: pal.row, ..Default::default() }
        } else {
            TextFormat { font_id: FontId::proportional(size), color: pal.text, ..Default::default() }
        };
        job.append(buf, 0.0, format);
        buf.clear();
    };
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("**") {
            // Bold has no heavier face in the bundled fonts: the markers are dropped.
            flush_buf(&mut job, &mut buf, code);
            rest = r;
        } else if let Some(r) = rest.strip_prefix('`') {
            flush_buf(&mut job, &mut buf, code);
            code = !code;
            rest = r;
        } else {
            let c = rest.chars().next().expect("not empty");
            buf.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    flush_buf(&mut job, &mut buf, code);
    job
}

/// Draw parsed blocks.
pub fn render_blocks(ui: &mut egui::Ui, pal: &Palette, blocks: &[Block]) {
    ui.spacing_mut().item_spacing.y = 6.0;
    for (n, block) in blocks.iter().enumerate() {
        match block {
            Block::Heading { level, text } => {
                let size = match level {
                    1 => 24.0,
                    2 => 18.0,
                    _ => 15.0,
                };
                if n > 0 {
                    ui.add_space(8.0);
                }
                ui.label(RichText::new(text).size(size).strong().color(pal.text));
            }
            Block::Paragraph(t) => {
                ui.label(inline_job(t, 13.5, pal));
            }
            Block::List { ordered, items } => {
                for (k, item) in items.iter().enumerate() {
                    // A horizontal layout does not wrap, so the text gets its own wrapping column.
                    let text_w = (ui.available_width() - 26.0).max(40.0);
                    ui.horizontal_top(|ui| {
                        let marker = if *ordered { format!("{}.", k + 1) } else { "\u{2022}".to_owned() };
                        ui.add_sized([18.0, 16.0], egui::Label::new(RichText::new(marker).size(13.5).color(pal.text_dim)));
                        ui.allocate_ui_with_layout(Vec2::new(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
                            ui.set_max_width(text_w);
                            ui.label(inline_job(item, 13.5, pal));
                        });
                    });
                }
            }
            Block::Table { header, rows } => table(ui, pal, n, header, rows),
            Block::Code(code) => {
                egui::Frame::new()
                    .fill(pal.row)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(code).monospace().size(12.5).color(pal.text));
                    });
            }
        }
    }
}

fn table(ui: &mut egui::Ui, pal: &Palette, id: usize, header: &[String], rows: &[Vec<String>]) {
    let cols = header.len().max(1);
    egui::Frame::new()
        .fill(pal.row)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new(("guide-table", id)).num_columns(cols).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in header {
                    ui.label(RichText::new(h).strong().size(12.5).color(pal.text_dim));
                }
                ui.end_row();
                for row in rows {
                    for c in 0..cols {
                        let cell = row.get(c).map(String::as_str).unwrap_or("");
                        ui.label(inline_job(cell, 12.5, pal));
                    }
                    ui.end_row();
                }
            });
        });
}

/// The "?" button that opens `link`'s chapter. A tap is stored in the egui context, and
/// [`EskizApp::guide_window`] opens the guide with it, so panels need no access to the app.
pub fn help_link(ui: &mut egui::Ui, pal: &Palette, link: Link) {
    let title = index_of(link.chapter()).map(|i| CHAPTERS[i].title()).unwrap_or_default();
    let name = format!("Open the guide: {title}");
    let resp = ui.add(
        egui::Button::new(RichText::new("?").size(12.5).strong().color(pal.text_dim))
            .min_size(Vec2::splat(22.0))
            .corner_radius(CornerRadius::same(11)),
    );
    // A screen reader hears the destination, not "question mark".
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name.as_str()));
    let tapped = resp.on_hover_text(name.as_str()).clicked();
    if tapped {
        let ctx = ui.ctx().clone();
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(PENDING_LINK), link));
    }
}

/// Egui memory key for a "?" tap waiting to be opened.
const PENDING_LINK: &str = "user-guide-pending-link";

/// State of the User guide window.
#[derive(Default)]
pub struct GuideWindow {
    pub open: bool,
    /// The chapter shown (an index into [`CHAPTERS`]).
    pub chapter: usize,
    pub query: String,
}

impl GuideWindow {
    /// Open the window at `chapter` (an id from [`ch`]).
    pub fn show(&mut self, chapter: &str) {
        self.open = true;
        self.chapter = index_of(chapter).unwrap_or(0);
    }
}

impl EskizApp {
    /// Open the User guide at `chapter`.
    pub(crate) fn open_user_guide(&mut self, chapter: &str) {
        self.guide.show(chapter);
    }

    /// Open the User guide at the chapter a "?" link names.
    pub(crate) fn open_user_guide_for(&mut self, link: Link) {
        self.guide.show(link.chapter());
    }

    /// The User guide window, when it is open. Drawn once a frame, above the canvas.
    pub(crate) fn guide_window(&mut self, ctx: &egui::Context) {
        if let Some(link) = ctx.data_mut(|d| d.remove_temp::<Link>(egui::Id::new(PENDING_LINK))) {
            self.open_user_guide_for(link);
        }
        if !self.guide.open {
            return;
        }
        let pal = self.pal;
        let mut open = true;
        // Most of the window, up to a comfortable reading size.
        let size = (ctx.content_rect().size() * 0.85).min(Vec2::new(860.0, 620.0)).max(Vec2::new(420.0, 300.0));
        egui::Window::new("User guide")
            .id(egui::Id::new("user-guide"))
            .open(&mut open)
            .order(egui::Order::Tooltip)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .fixed_size(size)
            .show(ctx, |ui| guide_body(ui, &pal, &mut self.guide, size));
        if !open {
            self.guide.open = false;
        }
    }
}

fn guide_body(ui: &mut egui::Ui, pal: &Palette, g: &mut GuideWindow, size: Vec2) {
    // The window has a fixed size: the sidebar is 190 points, the rest is the page.
    ui.set_max_size(size);
    let height = size.y - 4.0;
    let body_w = size.x - 190.0 - 2.0 * ui.spacing().item_spacing.x - 8.0;
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(190.0, height), Layout::top_down(Align::Min), |ui| {
            ui.add(egui::TextEdit::singleline(&mut g.query).hint_text("Search all chapters").desired_width(ui.available_width()));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("guide-chapters").auto_shrink([false, false]).show(ui, |ui| {
                for (i, c) in CHAPTERS.iter().enumerate() {
                    if ui.add_sized([ui.available_width(), 28.0], egui::Button::selectable(i == g.chapter, c.title())).clicked() {
                        g.chapter = i;
                        g.query.clear();
                    }
                }
            });
        });
        ui.separator();
        ui.allocate_ui_with_layout(Vec2::new(body_w, height), Layout::top_down(Align::Min), |ui| {
            ui.set_max_width(body_w);
            if g.query.trim().is_empty() {
                let blocks = parse(CHAPTERS[g.chapter].body);
                egui::ScrollArea::vertical()
                    .id_salt(("guide-body", g.chapter))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        render_blocks(ui, pal, &blocks);
                    });
            } else {
                let hits = search(&g.query);
                ui.label(RichText::new(format!("{} matches for \"{}\"", hits.len(), g.query.trim())).size(12.5).color(pal.text_dim));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("guide-hits").auto_shrink([false, false]).show(ui, |ui| {
                    for hit in &hits {
                        let title = CHAPTERS[hit.chapter].title();
                        let text = format!("{title}: {}", hit.snippet);
                        if ui.add(egui::Button::selectable(false, text).wrap()).clicked() {
                            g.chapter = hit.chapter;
                            g.query.clear();
                            break;
                        }
                    }
                });
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Prefs;
    use crate::document::Document;
    use crate::snapshot::{snapshot_dir, Headless};

    const SHORTCUTS_DOC: &str = include_str!("../../../docs/guide/gestures-and-shortcuts.md");

    /// The generated block as it stands in the chapter file.
    fn block_in_file(doc: &str) -> &str {
        let begin = doc.find("<!-- generated:shortcuts:begin").expect("begin marker");
        let end_marker = "<!-- generated:shortcuts:end -->";
        let end = doc.find(end_marker).expect("end marker") + end_marker.len();
        let block = &doc[begin..end];
        // The file has a newline after the end marker; compare up to it.
        block
    }

    fn headless() -> Headless {
        Headless::new(Vec2::new(1000.0, 720.0), Document::new(400, 300), Prefs::default())
    }

    #[test]
    fn every_chapter_parses_and_has_a_title() {
        for c in CHAPTERS {
            let blocks = parse(c.body);
            assert!(!blocks.is_empty(), "{} is empty", c.id);
            assert!(matches!(blocks.first(), Some(Block::Heading { level: 1, .. })), "{} starts with a title", c.id);
            assert!(!c.title().is_empty());
        }
        assert_eq!(CHAPTERS.len(), 14);
    }

    #[test]
    fn every_chapter_renders() {
        let mut h = headless();
        for (i, c) in CHAPTERS.iter().enumerate() {
            h.app.guide.show(c.id);
            assert_eq!(h.app.guide.chapter, i);
            h.advance(0.1);
            h.render();
        }
    }

    #[test]
    fn search_finds_hold_to_shape_in_painting() {
        let hits = search("Hold to Shape");
        assert!(!hits.is_empty());
        let painting = index_of(ch::PAINTING).unwrap();
        assert!(hits.iter().any(|h| h.chapter == painting), "Hold to Shape is in the Painting chapter");
        assert!(hits.iter().all(|h| h.snippet.to_ascii_lowercase().contains("hold to shape")));
        assert!(search("   ").is_empty());
        assert!(search("zzz-no-such-word-zzz").is_empty());
    }

    #[test]
    fn search_is_case_insensitive() {
        assert_eq!(search("hold to shape").len(), search("HOLD TO SHAPE").len());
    }

    #[test]
    fn every_contextual_link_opens_its_chapter() {
        for link in Link::ALL {
            let mut h = headless();
            h.app.open_user_guide_for(link);
            assert!(h.app.guide.open, "{link:?} opens the guide");
            let chapter = &CHAPTERS[h.app.guide.chapter];
            assert_eq!(chapter.id, link.chapter(), "{link:?} opens {}", link.chapter());
            let body = chapter.body.to_ascii_lowercase();
            assert!(body.contains(&link.topic().to_ascii_lowercase()), "{} covers {}", chapter.id, link.topic());
            h.advance(0.1);
            h.render();
        }
    }

    #[test]
    fn a_tapped_question_mark_opens_the_guide_at_its_chapter() {
        let mut h = headless();
        assert!(!h.app.guide.open);
        h.ctx.data_mut(|d| d.insert_temp(egui::Id::new(PENDING_LINK), Link::Colour));
        h.advance(0.1);
        assert!(h.app.guide.open);
        assert_eq!(CHAPTERS[h.app.guide.chapter].id, ch::COLOUR);
    }

    #[test]
    fn search_ignores_markdown_marks() {
        let hits = search("Hold to Shape delay");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| !h.snippet.contains("**") && !h.snippet.contains('`')));
    }

    #[test]
    fn shortcut_table_matches_docs() {
        let expected = generated_shortcuts_markdown("Cmd");
        assert_eq!(block_in_file(SHORTCUTS_DOC), expected.trim_end(), "regenerate with ESKIZ_UPDATE_GUIDE=1 cargo test -p eskiz-app update_generated_guide");
    }

    #[test]
    fn every_documented_shortcut_is_bound() {
        let block = generated_shortcuts_markdown("Cmd");
        for entry in crate::shortcuts::KEYBOARD {
            assert!(!entry.bindings.is_empty(), "{} has a binding", entry.action);
            let keys = crate::shortcuts::display_keys(entry, "Cmd");
            assert!(block.contains(&format!("| {keys} |")), "{keys} is documented");
        }
    }

    /// Rewrites the generated block of the Gestures and shortcuts chapter. Does nothing unless
    /// `ESKIZ_UPDATE_GUIDE` is set.
    #[test]
    fn update_generated_guide() {
        if crate::compat::env_os("ESKIZ_UPDATE_GUIDE").is_none() {
            return;
        }
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/guide/gestures-and-shortcuts.md");
        let doc = std::fs::read_to_string(path).unwrap();
        let old = block_in_file(&doc).to_owned();
        let new = generated_shortcuts_markdown("Cmd");
        std::fs::write(path, doc.replacen(&old, new.trim_end(), 1)).unwrap();
    }

    #[test]
    fn writes_guide_snapshots() {
        let mut h = headless();
        h.app.guide.show(ch::PAINTING);
        h.advance(0.3);
        h.render().save_png(&snapshot_dir().join("user_guide_window.png")).unwrap();
        h.app.guide.show(ch::GESTURES);
        h.advance(0.3);
        h.render().save_png(&snapshot_dir().join("user_guide_gestures.png")).unwrap();
        h.app.guide.show(ch::PAINTING);
        h.app.guide.query = "Hold to Shape".into();
        h.advance(0.3);
        h.render().save_png(&snapshot_dir().join("user_guide_search.png")).unwrap();
    }

    #[test]
    fn writes_a_panel_with_its_question_mark_link() {
        let mut h = headless();
        for (panel, name) in [(crate::popover::Panel::Layers, "layers"), (crate::popover::Panel::Colour, "colour")] {
            h.open_panel(panel);
            h.advance(0.4);
            h.render().save_png(&snapshot_dir().join(format!("user_guide_link_{name}.png"))).unwrap();
        }
    }

    #[test]
    fn parser_handles_the_markdown_the_chapters_use() {
        let blocks = parse("# Title\n\nSome **bold** text\nand more.\n\n- one\n- two\n  continued\n\n1. first\n\n| A | B |\n|---|---|\n| x | `y` |\n\n```\ncode\n```\n<!-- hidden -->\n");
        assert_eq!(blocks[0], Block::Heading { level: 1, text: "Title".into() });
        assert_eq!(blocks[1], Block::Paragraph("Some **bold** text and more.".into()));
        assert_eq!(blocks[2], Block::List { ordered: false, items: vec!["one".into(), "two continued".into()] });
        assert_eq!(blocks[3], Block::List { ordered: true, items: vec!["first".into()] });
        assert_eq!(blocks[4], Block::Table { header: vec!["A".into(), "B".into()], rows: vec![vec!["x".into(), "`y`".into()]] });
        assert_eq!(blocks[5], Block::Code("code".into()));
        assert_eq!(blocks.len(), 6, "the comment is not a block");
    }
}
