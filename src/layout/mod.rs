#![allow(dead_code)]

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::deck::model::{Block, ImageDisplay, Inline, Slide};

const CODE_PANEL_BG: Color = Color::Rgb(28, 30, 36);
const CODE_PANEL_EDGE: Color = Color::Rgb(72, 78, 92);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
}

#[derive(Clone, Debug, Default)]
pub struct SlideLayout {
    pub total_rows: usize,
    pub lines: Vec<LayoutLine>,
    pub images: Vec<LaidOutImage>,
    pub searchable_text: String,
}

#[derive(Clone, Debug, Default)]
pub struct LayoutLine {
    pub row: usize,
    pub spans: Vec<Span<'static>>,
    pub search_text: String,
    pub text_span_index: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub row: usize,
    pub start: usize,
    pub len: usize,
}

#[derive(Clone, Debug, Default)]
pub struct LaidOutImage {
    pub block_id: usize,
    pub asset_id: usize,
    pub start_row: usize,
    pub rows: usize,
    pub cols: u16,
    pub display: ImageDisplay,
}

#[derive(Clone, Debug)]
struct StyledChunk {
    text: String,
    style: Style,
}

impl StyledChunk {
    fn width(&self) -> usize {
        self.text.chars().count()
    }
}

pub fn build_layout(slide: &Slide, viewport: Viewport) -> SlideLayout {
    let wrap_width = viewport.width.max(10) as usize;
    let mut row = 0usize;
    let mut lines = Vec::new();
    let mut images = Vec::new();
    let mut searchable_text = String::new();

    for block in &slide.blocks {
        match block {
            Block::Heading(block) => {
                push_heading_block(
                    block.level,
                    &block.content,
                    wrap_width,
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Paragraph(block) => {
                push_rich_inline_lines(
                    &block.content,
                    wrap_width,
                    Style::default().fg(Color::Gray),
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                    None,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Quote(block) => {
                push_quote_block(
                    &block.content,
                    wrap_width,
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::List(block) => {
                push_list_block(
                    &block.items,
                    wrap_width,
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Code(block) => {
                push_code_block(
                    &block.language,
                    &block.code,
                    wrap_width,
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Table(block) => {
                push_table_block(
                    &block.rows,
                    wrap_width,
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Rule => {
                lines.push(LayoutLine {
                    row,
                    spans: vec![Span::styled(
                        "─".repeat(wrap_width.min(80)),
                        Style::default().fg(Color::DarkGray),
                    )],
                    search_text: String::new(),
                    text_span_index: 0,
                });
                searchable_text.push('\n');
                row += 1;
            }
            Block::Image(block) => {
                let rows = image_rows(slide, block.asset_id, viewport.width, block.display);
                images.push(LaidOutImage {
                    block_id: block.id,
                    asset_id: block.asset_id,
                    start_row: row,
                    rows,
                    cols: viewport.width.saturating_sub(2).max(10),
                    display: block.display,
                });
                searchable_text.push_str(&format!("[image:{}]\n", block.alt));
                row += rows;
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
        }
    }

    SlideLayout {
        total_rows: row,
        lines,
        images,
        searchable_text,
    }
}

pub fn viewport_lines(
    layout: &SlideLayout,
    scroll: usize,
    height: usize,
    matches: &[SearchMatch],
    selected_match: Option<usize>,
    active_row: Option<usize>,
    selection: Option<(usize, usize)>,
) -> Vec<Line<'static>> {
    let end = scroll.saturating_add(height);
    let mut out = Vec::new();
    for row in scroll..end {
        if let Some(line) = layout.lines.iter().find(|line| line.row == row) {
            let is_active_row = active_row == Some(row);
            let has_visible_text = !line.search_text.trim().is_empty();
            let in_selection = selection
                .map(|(start, finish)| row >= start && row <= finish && has_visible_text)
                .unwrap_or(false);
            let base_spans = line
                .spans
                .iter()
                .cloned()
                .map(|span| {
                    let span_style = span.style;
                    if in_selection {
                        span.style(span_style.bg(Color::Rgb(44, 47, 56)))
                    } else if is_active_row && has_visible_text {
                        span.style(span_style.bg(Color::Rgb(34, 37, 44)))
                    } else {
                        span
                    }
                })
                .collect::<Vec<_>>();
            let mut prefixed_spans = vec![Span::styled(
                if is_active_row && in_selection {
                    "▌ "
                } else if in_selection {
                    "▎ "
                } else if is_active_row {
                    "▶ "
                } else {
                    "  "
                },
                if is_active_row && in_selection {
                    Style::default().fg(Color::Yellow)
                } else if in_selection {
                    Style::default().fg(Color::DarkGray)
                } else if is_active_row {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            )];
            let row_matches = matches
                .iter()
                .enumerate()
                .filter(|(_, hit)| hit.row == row)
                .collect::<Vec<_>>();
            if row_matches.is_empty() || line.search_text.is_empty() {
                prefixed_spans.extend(base_spans);
                out.push(Line::from(prefixed_spans));
                continue;
            }

            let mut spans = Vec::new();
            spans.extend(prefixed_spans);
            for (index, span) in base_spans.iter().enumerate() {
                if index == line.text_span_index {
                    spans.extend(highlight_search_matches(
                        span.content.as_ref(),
                        span.style,
                        &row_matches,
                        selected_match,
                    ));
                } else {
                    spans.push(span.clone());
                }
            }
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(String::new()));
        }
    }
    out
}

fn push_heading_block(
    level: u8,
    inlines: &[Inline],
    wrap_width: usize,
    row: &mut usize,
    lines: &mut Vec<LayoutLine>,
    searchable_text: &mut String,
) {
    let style = match level {
        1 => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        3 => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    };
    if level == 1 {
        let title = flatten_inline(inlines).trim().to_string();
        let accent = "═".repeat(title.chars().count().min(wrap_width.max(10)));
        lines.push(LayoutLine {
            row: *row,
            spans: vec![Span::styled(accent, Style::default().fg(Color::DarkGray))],
            search_text: String::new(),
            text_span_index: 0,
        });
        searchable_text.push('\n');
        *row += 1;
    }
    push_rich_inline_lines(
        inlines,
        wrap_width,
        style,
        row,
        lines,
        searchable_text,
        None,
    );
    if level <= 2 {
        let accent = if level == 1 { '═' } else { '─' };
        let title = flatten_inline(inlines);
        lines.push(LayoutLine {
            row: *row,
            spans: vec![Span::styled(
                accent
                    .to_string()
                    .repeat(title.chars().count().min(wrap_width.max(10))),
                Style::default().fg(Color::DarkGray),
            )],
            search_text: String::new(),
            text_span_index: 0,
        });
        searchable_text.push('\n');
        *row += 1;
    }
}

fn push_quote_block(
    inlines: &[Inline],
    wrap_width: usize,
    row: &mut usize,
    lines: &mut Vec<LayoutLine>,
    searchable_text: &mut String,
) {
    let quote_text = flatten_inline(inlines);
    let trimmed = quote_text.trim();
    let (label, body, color) = if trimmed.starts_with("[!NOTE]") {
        (
            " NOTE ",
            trimmed.trim_start_matches("[!NOTE]").trim(),
            Color::Blue,
        )
    } else if trimmed.starts_with("[!TIP]") {
        (
            " TIP ",
            trimmed.trim_start_matches("[!TIP]").trim(),
            Color::Green,
        )
    } else if trimmed.starts_with("[!WARN]") {
        (
            " WARN ",
            trimmed.trim_start_matches("[!WARN]").trim(),
            Color::Yellow,
        )
    } else {
        (" QUOTE ", trimmed, Color::Magenta)
    };

    lines.push(LayoutLine {
        row: *row,
        spans: vec![Span::styled(
            label.to_string(),
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        )],
        search_text: String::new(),
        text_span_index: 0,
    });
    searchable_text.push('\n');
    *row += 1;

    lines.push(LayoutLine {
        row: *row,
        spans: vec![Span::styled(
            "─".repeat(wrap_width.saturating_sub(4).max(8)),
            Style::default().fg(color),
        )],
        search_text: String::new(),
        text_span_index: 0,
    });
    searchable_text.push('\n');
    *row += 1;

    push_rich_inline_lines(
        &[Inline::Text(body.to_string())],
        wrap_width.saturating_sub(4),
        Style::default().fg(color),
        row,
        lines,
        searchable_text,
        Some("▎ "),
    );
}

fn push_list_block(
    items: &[crate::deck::model::ListItem],
    wrap_width: usize,
    row: &mut usize,
    lines: &mut Vec<LayoutLine>,
    searchable_text: &mut String,
) {
    for item in items {
        push_rich_inline_lines(
            &item.content,
            wrap_width.saturating_sub(6),
            Style::default().fg(Color::Gray),
            row,
            lines,
            searchable_text,
            Some("◆ "),
        );
    }
}

fn push_code_block(
    language: &str,
    code: &str,
    wrap_width: usize,
    row: &mut usize,
    lines: &mut Vec<LayoutLine>,
    searchable_text: &mut String,
) {
    let title = if language.trim().is_empty() {
        " code ".to_string()
    } else {
        format!(" {} ", language.trim())
    };
    lines.push(LayoutLine {
        row: *row,
        spans: vec![Span::styled(
            format!("╭{}", title),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )],
        search_text: String::new(),
        text_span_index: 0,
    });
    searchable_text.push('\n');
    *row += 1;

    let highlighted_lines = highlight_code_lines(language, code);
    for (index, line_spans) in highlighted_lines.into_iter().enumerate() {
        let raw_line = code.lines().nth(index).unwrap_or_default();
        let mut spans = vec![Span::styled(
            "│ ",
            Style::default().fg(CODE_PANEL_EDGE).bg(CODE_PANEL_BG),
        )];
        let visible_width = raw_line.chars().count();
        spans.extend(line_spans);
        let fill = wrap_width.saturating_sub(visible_width.saturating_add(2));
        if fill > 0 {
            spans.push(Span::styled(
                " ".repeat(fill),
                Style::default().bg(CODE_PANEL_BG),
            ));
        }
        lines.push(LayoutLine {
            row: *row,
            spans,
            search_text: raw_line.to_string(),
            text_span_index: 1,
        });
        searchable_text.push_str(raw_line);
        searchable_text.push('\n');
        *row += 1;
    }

    lines.push(LayoutLine {
        row: *row,
        spans: vec![Span::styled(
            format!("╰{}", "─".repeat(wrap_width.saturating_sub(2).max(4))),
            Style::default().fg(CODE_PANEL_EDGE),
        )],
        search_text: String::new(),
        text_span_index: 0,
    });
    searchable_text.push('\n');
    *row += 1;
}

fn push_table_block(
    rows: &[Vec<Vec<Inline>>],
    wrap_width: usize,
    row: &mut usize,
    lines: &mut Vec<LayoutLine>,
    searchable_text: &mut String,
) {
    if rows.is_empty() {
        return;
    }
    let mut widths = vec![0usize; rows.iter().map(|r| r.len()).max().unwrap_or(0)];
    let flattened = rows
        .iter()
        .map(|table_row| {
            table_row
                .iter()
                .enumerate()
                .map(|(index, cell)| {
                    let text = flatten_inline(cell);
                    widths[index] = widths[index].max(text.chars().count());
                    text
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let border = build_table_border('┌', '┬', '┐', &widths, wrap_width);
    lines.push(LayoutLine {
        row: *row,
        spans: vec![Span::styled(border, Style::default().fg(Color::DarkGray))],
        search_text: String::new(),
        text_span_index: 0,
    });
    searchable_text.push('\n');
    *row += 1;

    for (index, table_row) in flattened.iter().enumerate() {
        let line = format_table_row(table_row, &widths, wrap_width);
        lines.push(LayoutLine {
            row: *row,
            spans: vec![Span::styled(
                line.clone(),
                if index == 0 {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            )],
            search_text: table_row.join(" "),
            text_span_index: 0,
        });
        searchable_text.push_str(&table_row.join(" "));
        searchable_text.push('\n');
        *row += 1;
        if index == 0 && flattened.len() > 1 {
            lines.push(LayoutLine {
                row: *row,
                spans: vec![Span::styled(
                    build_table_border('├', '┼', '┤', &widths, wrap_width),
                    Style::default().fg(Color::DarkGray),
                )],
                search_text: String::new(),
                text_span_index: 0,
            });
            searchable_text.push('\n');
            *row += 1;
        }
    }

    lines.push(LayoutLine {
        row: *row,
        spans: vec![Span::styled(
            build_table_border('└', '┴', '┘', &widths, wrap_width),
            Style::default().fg(Color::DarkGray),
        )],
        search_text: String::new(),
        text_span_index: 0,
    });
    searchable_text.push('\n');
    *row += 1;
}

fn build_table_border(
    left: char,
    join: char,
    right: char,
    widths: &[usize],
    wrap_width: usize,
) -> String {
    let mut out = String::new();
    out.push(left);
    for (index, width) in widths.iter().enumerate() {
        out.push_str(&"─".repeat((*width).min(wrap_width / widths.len().max(1)).max(1) + 2));
        if index + 1 < widths.len() {
            out.push(join);
        }
    }
    out.push(right);
    out
}

fn format_table_row(cells: &[String], widths: &[usize], wrap_width: usize) -> String {
    let mut out = String::new();
    out.push('│');
    for (index, cell) in cells.iter().enumerate() {
        let width = widths[index].min(wrap_width / widths.len().max(1)).max(1);
        let truncated = truncate_chars(cell, width);
        out.push(' ');
        out.push_str(&format!("{:width$}", truncated, width = width));
        out.push(' ');
        out.push('│');
    }
    out
}

fn truncate_chars(input: &str, width: usize) -> String {
    let chars = input.chars().collect::<Vec<_>>();
    if chars.len() <= width {
        return input.to_string();
    }
    chars[..width.saturating_sub(1)].iter().collect::<String>() + "…"
}

fn highlight_code_lines(language: &str, code: &str) -> Vec<Vec<Span<'static>>> {
    let syntax_set = syntax_set();
    let theme = code_theme();
    let syntax = syntax_set
        .find_syntax_by_token(language.trim())
        .or_else(|| syntax_set.find_syntax_by_extension(language.trim()))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let clean = line.strip_suffix('\n').unwrap_or(line);
        let spans = highlighter
            .highlight_line(line, syntax_set)
            .ok()
            .map(|ranges| {
                ranges
                    .into_iter()
                    .filter(|(_, text)| !text.is_empty() && *text != "\n")
                    .map(|(style, text)| {
                        Span::styled(text.to_string(), syntect_style_to_ratatui(style.foreground))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![Span::styled(clean.to_string(), code_base_style())]);
        lines.push(if spans.is_empty() {
            vec![Span::styled(String::new(), code_base_style())]
        } else {
            spans
        });
    }

    if lines.is_empty() {
        lines.push(vec![Span::styled(String::new(), code_base_style())]);
    }
    lines
}

fn syntect_style_to_ratatui(color: SyntectColor) -> Style {
    Style::default().fg(Color::Rgb(color.r, color.g, color.b))
}

fn code_base_style() -> Style {
    Style::default().fg(Color::White).bg(CODE_PANEL_BG)
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn code_theme() -> &'static Theme {
    static CODE_THEME: OnceLock<Theme> = OnceLock::new();
    CODE_THEME.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        themes
            .themes
            .get("base16-ocean.dark")
            .cloned()
            .or_else(|| themes.themes.values().next().cloned())
            .unwrap_or_default()
    })
}

fn push_rich_inline_lines(
    inlines: &[Inline],
    wrap_width: usize,
    base_style: Style,
    row: &mut usize,
    lines: &mut Vec<LayoutLine>,
    searchable_text: &mut String,
    prefix: Option<&str>,
) {
    let prefix_text = prefix.unwrap_or("");
    let chunks = styled_chunks(inlines, base_style);
    let wrapped = wrap_chunks(&chunks, wrap_width.max(1));
    for (index, wrapped_line) in wrapped.into_iter().enumerate() {
        let mut spans = Vec::new();
        if index == 0 && !prefix_text.is_empty() {
            spans.push(Span::styled(
                prefix_text.to_string(),
                base_style.fg(Color::DarkGray),
            ));
        } else if !prefix_text.is_empty() {
            spans.push(Span::styled(
                " ".repeat(prefix_text.chars().count()),
                base_style,
            ));
        }
        let text_span_index = if prefix_text.is_empty() { 0 } else { 1 };
        let search_text = wrapped_line
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<String>();
        for chunk in wrapped_line {
            spans.push(Span::styled(chunk.text, chunk.style));
        }
        lines.push(LayoutLine {
            row: *row,
            spans,
            search_text: search_text.clone(),
            text_span_index,
        });
        searchable_text.push_str(&search_text);
        searchable_text.push('\n');
        *row += 1;
    }
}

fn styled_chunks(inlines: &[Inline], base_style: Style) -> Vec<StyledChunk> {
    let mut chunks = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text(text) => chunks.push(StyledChunk {
                text: text.clone(),
                style: base_style,
            }),
            Inline::Emphasis(text) => chunks.push(StyledChunk {
                text: text.clone(),
                style: base_style
                    .fg(Color::LightYellow)
                    .add_modifier(Modifier::ITALIC),
            }),
            Inline::Strong(text) => chunks.push(StyledChunk {
                text: text.clone(),
                style: base_style.fg(Color::White).add_modifier(Modifier::BOLD),
            }),
            Inline::Code(text) => chunks.push(StyledChunk {
                text: format!(" {} ", text),
                style: base_style.fg(Color::Cyan).bg(Color::DarkGray),
            }),
            Inline::Link { text, .. } => {
                chunks.push(StyledChunk {
                    text: text.clone(),
                    style: base_style
                        .fg(Color::Blue)
                        .add_modifier(Modifier::UNDERLINED),
                });
                chunks.push(StyledChunk {
                    text: " ↗".to_string(),
                    style: base_style.fg(Color::Blue),
                });
            }
        }
    }
    if chunks.is_empty() {
        chunks.push(StyledChunk {
            text: String::new(),
            style: base_style,
        });
    }
    chunks
}

fn wrap_chunks(chunks: &[StyledChunk], width: usize) -> Vec<Vec<StyledChunk>> {
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0usize;

    for chunk in chunks {
        for piece in split_chunk_for_wrap(chunk) {
            if piece.text == "\n" {
                lines.push(finish_wrapped_line(&mut current));
                current_width = 0;
                continue;
            }
            let piece_width = piece.width();
            if current_width > 0 && current_width + piece_width > width {
                lines.push(finish_wrapped_line(&mut current));
                current_width = 0;
            }
            current_width += piece_width;
            current.push(piece);
        }
    }

    if current.is_empty() {
        lines.push(vec![StyledChunk {
            text: String::new(),
            style: Style::default(),
        }]);
    } else {
        lines.push(finish_wrapped_line(&mut current));
    }
    lines
}

fn split_chunk_for_wrap(chunk: &StyledChunk) -> Vec<StyledChunk> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in chunk.text.chars() {
        if ch == '\n' {
            if !current.is_empty() {
                out.push(StyledChunk {
                    text: std::mem::take(&mut current),
                    style: chunk.style,
                });
            }
            out.push(StyledChunk {
                text: "\n".to_string(),
                style: chunk.style,
            });
        } else if ch.is_whitespace() {
            current.push(ch);
            out.push(StyledChunk {
                text: std::mem::take(&mut current),
                style: chunk.style,
            });
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push(StyledChunk {
            text: current,
            style: chunk.style,
        });
    }
    out
}

fn finish_wrapped_line(current: &mut Vec<StyledChunk>) -> Vec<StyledChunk> {
    if current.is_empty() {
        return vec![StyledChunk {
            text: String::new(),
            style: Style::default(),
        }];
    }
    std::mem::take(current)
}

fn highlight_search_matches(
    text: &str,
    base_style: Style,
    matches: &[(usize, &SearchMatch)],
    selected_match: Option<usize>,
) -> Vec<Span<'static>> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut spans = Vec::new();
    let mut cursor = 0usize;

    for (match_index, hit) in matches {
        let start = hit.start.min(chars.len());
        let end = hit.start.saturating_add(hit.len).min(chars.len());
        if cursor < start {
            spans.push(Span::styled(
                chars[cursor..start].iter().collect::<String>(),
                base_style,
            ));
        }

        let style = if Some(*match_index) == selected_match {
            base_style.fg(Color::Black).bg(Color::Yellow)
        } else {
            base_style.bg(Color::Rgb(58, 60, 72))
        };
        spans.push(Span::styled(
            chars[start..end].iter().collect::<String>(),
            style,
        ));
        cursor = end;
    }

    if cursor < chars.len() {
        spans.push(Span::styled(
            chars[cursor..].iter().collect::<String>(),
            base_style,
        ));
    }

    spans
}

fn flatten_inline(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text)
            | Inline::Emphasis(text)
            | Inline::Strong(text)
            | Inline::Code(text) => out.push_str(text),
            Inline::Link { text, .. } => out.push_str(text),
        }
    }
    out
}

fn image_rows(slide: &Slide, asset_id: usize, width: u16, display: ImageDisplay) -> usize {
    let asset = slide.assets.iter().find(|asset| asset.id == asset_id);
    let cols = width.saturating_sub(2).max(10) as usize;
    let hinted = match display {
        ImageDisplay::Inline => 12,
        ImageDisplay::FullWidth => 20,
        ImageDisplay::Cover => 24,
    };
    if let Some(size) = asset.and_then(|asset| asset.size) {
        let ratio = size.height as f32 / size.width.max(1) as f32;
        ((cols as f32 * ratio) / 2.0).round() as usize
    } else {
        hinted
    }
    .max(6)
}

fn push_blank(row: &mut usize, lines: &mut Vec<LayoutLine>, searchable_text: &mut String) {
    lines.push(LayoutLine {
        row: *row,
        spans: vec![Span::raw(String::new())],
        search_text: String::new(),
        text_span_index: 0,
    });
    searchable_text.push('\n');
    *row += 1;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::deck::model::{AssetRef, Block, ParagraphBlock, Slide};

    use super::*;

    #[test]
    fn builds_layout_lines() {
        let slide = Slide {
            blocks: vec![Block::Paragraph(ParagraphBlock {
                id: 0,
                content: vec![Inline::Text("hello world".to_string())],
            })],
            assets: vec![AssetRef {
                id: 0,
                path: PathBuf::from("a.png"),
                size: None,
            }],
            ..Slide::default()
        };
        let layout = build_layout(
            &slide,
            Viewport {
                width: 40,
                height: 10,
            },
        );
        assert!(!layout.lines.is_empty());
    }
}
