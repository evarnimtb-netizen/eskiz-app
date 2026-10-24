//! Layout. cosmic-text shapes each paragraph, with letter spacing and word wrapping.
//! This module then places the lines: alignment, justification of all but the last line of a
//! paragraph, per-paragraph leading, underline segments, and vertical columns.
//!
//! Coordinates are in layer units (1 unit = 1 point at scale 1). The text space is y-down, with
//! the first baseline at `y = ascent` of the first line.

use cosmic_text::fontdb;
use cosmic_text::{
    Attrs as CAttrs, Buffer, Family, LayoutGlyph, Metrics, Shaping, Style as CStyle,
    Weight as CWeight, Wrap,
};

use crate::fonts::FontLibrary;
use crate::model::{Alignment, Attrs, Color, FontFace, TextLayer};

/// Axis-aligned rectangle in layer units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn outset(self, by: f32) -> Self {
        Self {
            x0: self.x0 - by,
            y0: self.y0 - by,
            x1: self.x1 + by,
            y1: self.y1 + by,
        }
    }

    pub fn is_empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

/// One glyph ready to rasterise. `origin` is the pen position on the baseline (y-down).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedGlyph {
    pub font_id: fontdb::ID,
    pub font_weight: fontdb::Weight,
    pub glyph_id: u16,
    /// Effective size in layer units, after any replacement scaling.
    pub font_size: f32,
    pub origin: (f32, f32),
    pub color: Color,
    pub outline_pt: f32,
    /// Byte offset of the glyph's cluster in the displayed text (the stored text, except that
    /// all-caps runs are shown in capitals). Lets an editor map a caret to a position.
    pub cluster: usize,
    /// Advance of the glyph, letter spacing included (layer units). Used to place carets.
    pub advance: f32,
}

/// The box of one line, or of one vertical column. `(x, y)` is its top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBox {
    pub paragraph: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// An underline segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Underline {
    pub rect: Rect,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayoutMetrics {
    /// Widest line (horizontal) or the total width of all columns (vertical). Trailing spaces are
    /// not counted.
    pub width: f32,
    /// Total height of all lines (horizontal) or the longest column (vertical).
    pub height: f32,
    pub line_count: usize,
    pub column_count: usize,
}

/// The result of laying out a text layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub vertical: bool,
    pub glyphs: Vec<PlacedGlyph>,
    pub lines: Vec<LineBox>,
    pub underlines: Vec<Underline>,
    /// Text bounds, grown by half the largest outline width on each side.
    pub bounds: Rect,
    pub metrics: LayoutMetrics,
    /// Families that are not installed, in the order they first appear.
    pub missing_fonts: Vec<String>,
}

/// A run after font resolution. It is the same index as the source run.
#[derive(Clone, Debug)]
struct Resolved {
    family: String,
    face: FontFace,
    /// Effective size in layer units.
    size: f32,
    /// Kerning plus tracking, as an em fraction. This is the unit cosmic-text expects.
    spacing_em: f32,
    /// The same spacing in layer units: size x spacing_em.
    spacing: f32,
    baseline_shift: f32,
    outline: f32,
    underline: bool,
    color: Color,
}

struct Context<'a> {
    layer: &'a TextLayer,
    resolved: Vec<Resolved>,
    /// Text shown to cosmic-text: the source with All caps applied, run by run.
    display: String,
    /// Byte offset in `display` where each run starts.
    display_starts: Vec<usize>,
    /// Index of the run that starts each paragraph.
    para_run: Vec<usize>,
}

impl Context<'_> {
    fn run_at(&self, display_byte: usize) -> usize {
        self.display_starts
            .partition_point(|&start| start <= display_byte)
            .saturating_sub(1)
    }

    fn is_space(&self, glyph: &LayoutGlyph) -> bool {
        matches!(self.display.as_bytes().get(glyph.start), Some(b' ' | b'\t'))
    }

    /// Ascent, descent and leading ratios for a line. An empty line uses its run's face.
    fn ratios(
        &self,
        fonts: &mut FontLibrary,
        first: Option<&LayoutGlyph>,
        res: &Resolved,
    ) -> (f32, f32, f32) {
        match first {
            Some(glyph) => fonts.ratios_by_id(glyph.font_id, glyph.font_weight),
            None => fonts.ratios_for_face(&res.family, res.face),
        }
    }
}

/// Lays out a text layer. Used by [`TextLayer::layout`].
pub(crate) fn layout(layer: &TextLayer, fonts: &mut FontLibrary) -> Layout {
    let consistent = layer.runs.iter().map(|run| run.len).sum::<usize>() == layer.text.len();
    if layer.runs.is_empty() || !consistent {
        return Layout::default();
    }
    let vertical = layer.runs[0].attrs.vertical;

    let mut missing_fonts: Vec<String> = Vec::new();
    let mut resolved = Vec::with_capacity(layer.runs.len());
    for run in &layer.runs {
        let attrs: &Attrs = &run.attrs;
        let res = fonts.resolve(&attrs.font_family);
        if res.missing && !missing_fonts.contains(&attrs.font_family) {
            missing_fonts.push(attrs.font_family.clone());
        }
        let size = attrs.size_pt * res.size_scale;
        let spacing_em = attrs.kerning + attrs.tracking;
        resolved.push(Resolved {
            family: res.family,
            face: attrs.font_face,
            size,
            spacing_em,
            spacing: size * spacing_em,
            baseline_shift: attrs.baseline_shift_pt,
            outline: attrs.outline_pt,
            underline: attrs.underline,
            color: attrs.color,
        });
    }

    let mut display = String::with_capacity(layer.text.len());
    let mut display_starts = Vec::with_capacity(layer.runs.len());
    let mut display_texts: Vec<String> = Vec::with_capacity(layer.runs.len());
    let mut src = 0usize;
    for run in &layer.runs {
        let text = layer.text.get(src..src + run.len).unwrap_or("");
        src += run.len;
        let shown = if run.attrs.all_caps {
            text.to_uppercase()
        } else {
            text.to_string()
        };
        let shown = no_extra_paragraphs(shown);
        display_starts.push(display.len());
        display.push_str(&shown);
        display_texts.push(shown);
    }

    let mut para_starts = vec![0usize];
    para_starts.extend(layer.text.match_indices('\n').map(|(i, _)| i + 1));
    let run_of_source = |byte: usize| {
        let mut pos = 0usize;
        for (i, run) in layer.runs.iter().enumerate() {
            if byte < pos + run.len {
                return i;
            }
            pos += run.len;
        }
        layer.runs.len() - 1
    };
    let para_run: Vec<usize> = para_starts
        .iter()
        .map(|&byte| run_of_source(byte))
        .collect();
    let para_count = para_starts.len();

    let cx = Context {
        layer,
        resolved,
        display,
        display_starts,
        para_run,
    };
    let cattrs: Vec<CAttrs<'_>> = cx.resolved.iter().map(cosmic_attrs).collect();
    let spans: Vec<(&str, CAttrs<'_>)> = display_texts
        .iter()
        .map(String::as_str)
        .zip(cattrs.iter().cloned())
        .collect();
    let default_attrs = CAttrs::new()
        .family(Family::SansSerif)
        .metrics(Metrics::new(12.0, 12.0));

    let mut buffer = Buffer::new(fonts.system_mut(), Metrics::new(12.0, 12.0));
    let wrap_width = if vertical || layer.box_width <= 0.0 {
        None
    } else {
        Some(layer.box_width)
    };
    buffer.set_size(wrap_width, None);
    buffer.set_wrap(if vertical {
        Wrap::None
    } else {
        Wrap::WordOrGlyph
    });
    buffer.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fonts.system_mut(), false);

    // cosmic-text numbers a glyph's bytes from the start of its own paragraph; the rest of the
    // layout (runs, spaces, carets) works in bytes of the whole displayed text.
    let display_para_starts: Vec<usize> = std::iter::once(0)
        .chain(cx.display.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let mut grouped: Vec<Vec<Vec<LayoutGlyph>>> = vec![Vec::new(); para_count];
    for run in buffer.layout_runs() {
        if let Some(lines) = grouped.get_mut(run.line_i) {
            let base = display_para_starts.get(run.line_i).copied().unwrap_or(0);
            lines.push(
                run.glyphs
                    .iter()
                    .map(|g| {
                        let mut g = g.clone();
                        g.start += base;
                        g.end += base;
                        g
                    })
                    .collect(),
            );
        }
    }

    let mut out = Layout {
        vertical,
        missing_fonts,
        ..Layout::default()
    };
    if vertical {
        place_columns(&cx, fonts, &grouped, &mut out);
    } else {
        place_lines(&cx, fonts, &grouped, &mut out);
    }
    finish(&mut out);
    out
}

fn cosmic_attrs(res: &Resolved) -> CAttrs<'_> {
    let family = if res.family.is_empty() {
        Family::SansSerif
    } else {
        Family::Name(res.family.as_str())
    };
    let style = if res.face.is_italic() {
        CStyle::Italic
    } else {
        CStyle::Normal
    };
    let weight = if res.face.is_bold() {
        CWeight::BOLD
    } else {
        CWeight::NORMAL
    };
    let size = res.size.max(0.01);
    CAttrs::new()
        .family(family)
        .weight(weight)
        .style(style)
        .metrics(Metrics::new(size, size))
        .letter_spacing(res.spacing_em)
}

/// Size of a line: the largest glyph, or the run's size for an empty line.
fn line_size(glyphs: &[LayoutGlyph], fallback: f32) -> f32 {
    if glyphs.is_empty() {
        fallback
    } else {
        glyphs.iter().map(|g| g.font_size).fold(0.0, f32::max)
    }
}

struct OpenUnderline {
    x0: f32,
    x1: f32,
    y: f32,
    thickness: f32,
    color: Color,
}

fn flush_underline(out: &mut Layout, open: Option<OpenUnderline>) {
    if let Some(u) = open {
        out.underlines.push(Underline {
            rect: Rect {
                x0: u.x0,
                y0: u.y,
                x1: u.x1,
                y1: u.y + u.thickness,
            },
            color: u.color,
        });
    }
}

fn place_lines(
    cx: &Context<'_>,
    fonts: &mut FontLibrary,
    grouped: &[Vec<Vec<LayoutGlyph>>],
    out: &mut Layout,
) {
    let mut y = 0.0f32;
    let mut widest = 0.0f32;
    let mut line_count = 0usize;
    for (p, paragraph) in grouped.iter().enumerate() {
        let style = &cx.layer.runs[cx.para_run[p]].attrs;
        let res = &cx.resolved[cx.para_run[p]];
        let empty: Vec<LayoutGlyph> = Vec::new();
        let lines: Vec<&[LayoutGlyph]> = if paragraph.is_empty() {
            vec![empty.as_slice()]
        } else {
            paragraph.iter().map(Vec::as_slice).collect()
        };
        let last_line = lines.len() - 1;

        for (li, glyphs) in lines.iter().copied().enumerate() {
            let (asc, desc, lead) = cx.ratios(fonts, glyphs.first(), res);
            let size = line_size(glyphs, res.size);
            let content = (asc + desc) * size;
            let line_h = (asc + desc + lead) * size * style.leading;
            let baseline = y + (line_h - content) * 0.5 + asc * size;

            // Trailing spaces do not count towards width or alignment.
            let end = glyphs.iter().rposition(|g| !cx.is_space(g));
            let width = end.map_or(0.0, |i| glyphs[i].x + glyphs[i].w);
            let visible = end.map_or(0, |i| i + 1);
            let box_w = cx.layer.box_width.max(width);
            let spaces = glyphs[..visible].iter().filter(|g| cx.is_space(g)).count();

            let justify = style.alignment == Alignment::Justified
                && li != last_line
                && spaces > 0
                && box_w > width;
            let x0 = match style.alignment {
                Alignment::Left | Alignment::Justified => 0.0,
                Alignment::Centre => (box_w - width) * 0.5,
                Alignment::Right => box_w - width,
            };
            let per_space = if justify {
                (box_w - width) / spaces as f32
            } else {
                0.0
            };

            let mut shift = 0.0f32;
            let mut open: Option<OpenUnderline> = None;
            for (gi, glyph) in glyphs.iter().enumerate() {
                let run = &cx.resolved[cx.run_at(glyph.start)];
                let x = x0 + glyph.x + shift;
                let oy = baseline - run.baseline_shift + glyph.y;
                out.glyphs.push(PlacedGlyph {
                    font_id: glyph.font_id,
                    font_weight: glyph.font_weight,
                    glyph_id: glyph.glyph_id,
                    font_size: glyph.font_size,
                    origin: (x, oy),
                    color: run.color,
                    outline_pt: run.outline,
                    cluster: glyph.start,
                    advance: glyph.w,
                });

                if run.underline {
                    let thickness = run.size / 16.0;
                    open = match open.take() {
                        Some(mut u) if u.color == run.color => {
                            u.x1 = x + glyph.w;
                            Some(u)
                        }
                        previous => {
                            flush_underline(out, previous);
                            Some(OpenUnderline {
                                x0: x,
                                x1: x + glyph.w,
                                y: oy + thickness,
                                thickness,
                                color: run.color,
                            })
                        }
                    };
                } else {
                    flush_underline(out, open.take());
                }

                if justify && gi < visible && cx.is_space(glyph) {
                    shift += per_space;
                }
            }
            flush_underline(out, open.take());

            out.lines.push(LineBox {
                paragraph: p,
                x: x0,
                y,
                width,
                height: line_h,
            });
            widest = widest.max(width);
            line_count += 1;
            y += line_h;
        }
    }
    out.metrics.width = widest;
    out.metrics.height = y;
    out.metrics.line_count = line_count;
}

/// One glyph in a vertical column, positioned once the column pitch is known.
struct ColumnItem {
    font_id: fontdb::ID,
    font_weight: fontdb::Weight,
    glyph_id: u16,
    font_size: f32,
    color: Color,
    outline_pt: f32,
    /// Horizontal advance of the glyph without letter spacing, used to centre it in its column.
    base_width: f32,
    /// Baseline position within the column, measured from the column's top.
    origin_y: f32,
    cluster: usize,
}

/// Vertical layout: characters stack top to bottom. Columns run right to left and are spaced by
/// the line height. Each paragraph starts a new column. Columns also break when they reach the
/// wrap width. Each character advances by its em size plus its letter spacing.
fn place_columns(
    cx: &Context<'_>,
    fonts: &mut FontLibrary,
    grouped: &[Vec<Vec<LayoutGlyph>>],
    out: &mut Layout,
) {
    let limit = cx.layer.box_width;
    let mut columns: Vec<Vec<ColumnItem>> = Vec::new();
    let mut lengths: Vec<f32> = Vec::new();
    let mut paragraph_of: Vec<usize> = Vec::new();
    let mut pitch = 0.0f32;

    for (p, paragraph) in grouped.iter().enumerate() {
        let style = &cx.layer.runs[cx.para_run[p]].attrs;
        let res = &cx.resolved[cx.para_run[p]];
        let glyphs: Vec<&LayoutGlyph> = paragraph.iter().flatten().collect();
        let (asc, desc, lead) = cx.ratios(fonts, glyphs.first().copied(), res);
        let size = if glyphs.is_empty() {
            res.size
        } else {
            glyphs.iter().map(|g| g.font_size).fold(0.0, f32::max)
        };
        pitch = pitch.max((asc + desc + lead) * size * style.leading);

        columns.push(Vec::new());
        lengths.push(0.0);
        paragraph_of.push(p);
        for glyph in glyphs {
            let run = &cx.resolved[cx.run_at(glyph.start)];
            let advance = run.size + run.spacing;
            let current = lengths[lengths.len() - 1];
            if limit > 0.0 && current > 0.0 && current + advance > limit {
                columns.push(Vec::new());
                lengths.push(0.0);
                paragraph_of.push(p);
            }
            let top = lengths[lengths.len() - 1];
            if let Some(column) = columns.last_mut() {
                column.push(ColumnItem {
                    font_id: glyph.font_id,
                    font_weight: glyph.font_weight,
                    glyph_id: glyph.glyph_id,
                    font_size: glyph.font_size,
                    color: run.color,
                    outline_pt: run.outline,
                    base_width: (glyph.w - run.spacing).max(0.0),
                    origin_y: top + asc * glyph.font_size,
                    cluster: glyph.start,
                });
            }
            if let Some(length) = lengths.last_mut() {
                *length += advance;
            }
        }
    }

    let count = columns.len();
    for (index, column) in columns.into_iter().enumerate() {
        let x_left = (count - 1 - index) as f32 * pitch;
        let centre = x_left + pitch * 0.5;
        for item in column {
            out.glyphs.push(PlacedGlyph {
                font_id: item.font_id,
                font_weight: item.font_weight,
                glyph_id: item.glyph_id,
                font_size: item.font_size,
                origin: (centre - item.base_width * 0.5, item.origin_y),
                color: item.color,
                outline_pt: item.outline_pt,
                cluster: item.cluster,
                advance: item.base_width,
            });
        }
        out.lines.push(LineBox {
            paragraph: paragraph_of[index],
            x: x_left,
            y: 0.0,
            width: pitch,
            height: lengths[index],
        });
    }
    out.metrics.width = count as f32 * pitch;
    out.metrics.height = lengths.iter().copied().fold(0.0, f32::max);
    out.metrics.line_count = count;
    out.metrics.column_count = count;
}

/// Bounds of all lines, grown by half the largest outline width.
fn finish(out: &mut Layout) {
    let mut bounds = Rect {
        x0: f32::MAX,
        y0: f32::MAX,
        x1: f32::MIN,
        y1: f32::MIN,
    };
    for line in &out.lines {
        bounds.x0 = bounds.x0.min(line.x);
        bounds.y0 = bounds.y0.min(line.y);
        bounds.x1 = bounds.x1.max(line.x + line.width);
        bounds.y1 = bounds.y1.max(line.y + line.height);
    }
    if out.lines.is_empty() {
        bounds = Rect::default();
    }
    let gutter = out.glyphs.iter().map(|g| g.outline_pt).fold(0.0, f32::max) * 0.5;
    out.bounds = bounds.outset(gutter);
}

/// The text with every paragraph separator other than `'\n'` turned into spaces of the same
/// byte length. cosmic-text also ends a paragraph at `'\r'`, U+001C..U+001E, U+0085 and U+2029
/// (Unicode bidi class B), but the layout counts paragraphs by `'\n'` only; an extra paragraph
/// would shift the glyph offsets of every later line and drop the last ones.
fn no_extra_paragraphs(text: String) -> String {
    if !text.contains(['\r', '\u{1C}', '\u{1D}', '\u{1E}', '\u{85}', '\u{2029}']) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\r' | '\u{1C}'..='\u{1E}' | '\u{85}' | '\u{2029}' => {
                out.extend(std::iter::repeat_n(' ', c.len_utf8()));
            }
            c => out.push(c),
        }
    }
    out
}
