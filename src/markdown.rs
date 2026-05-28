use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

#[derive(Clone, Debug, Default)]
pub struct ImageSlot {
    pub image_index: usize,
    pub start_line: usize,
    pub rows: usize,
}

#[derive(Clone, Debug, Default)]
pub struct RenderedMarkdown {
    pub lines: Vec<Line<'static>>,
    pub image_slots: Vec<ImageSlot>,
}

pub fn render_markdown(content: &str) -> RenderedMarkdown {
    let preprocessed = preprocess_images(content);
    let table_lines = detect_pipe_tables(&preprocessed.content);
    if !table_lines.is_empty() {
        return RenderedMarkdown {
            lines: table_lines,
            image_slots: preprocessed.image_slots,
        };
    }

    let parser = Parser::new_ext(&preprocessed.content, Options::all());
    let mut lines = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut list_depth = 0usize;
    let mut heading: Option<HeadingLevel> = None;
    let mut code_block = false;
    let mut emph = false;
    let mut strong = false;
    let mut blockquote = false;
    let mut link_target: Option<String> = None;
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme = default_theme();
    let mut code_language = String::new();

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                }
                Tag::Heading { level, .. } => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    heading = Some(level);
                }
                Tag::List(_) => {
                    list_depth += 1;
                }
                Tag::Item => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    spans.push(Span::raw(format!("{}• ", "  ".repeat(list_depth.saturating_sub(1)))));
                }
                Tag::Emphasis => emph = true,
                Tag::Strong => strong = true,
                Tag::CodeBlock(kind) => {
                    code_block = true;
                    code_language = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                }
                Tag::BlockQuote(_) => {
                    blockquote = true;
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    spans.push(Span::styled(
                        "▌ ",
                        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                    ));
                }
                Tag::Link { dest_url, .. } => {
                    link_target = Some(dest_url.to_string());
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    lines.push(Line::from(String::new()));
                }
                TagEnd::Heading(_) => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    lines.push(Line::from(String::new()));
                    heading = None;
                }
                TagEnd::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                }
                TagEnd::Item => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                }
                TagEnd::Emphasis => emph = false,
                TagEnd::Strong => strong = false,
                TagEnd::CodeBlock => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    lines.push(Line::from(String::new()));
                    code_block = false;
                }
                TagEnd::BlockQuote(_) => {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    lines.push(Line::from(String::new()));
                    blockquote = false;
                }
                TagEnd::Link => {
                    if let Some(dest) = link_target.take() {
                        spans.push(Span::styled(
                            format!(" ({dest})"),
                            Style::default().fg(Color::Blue).add_modifier(Modifier::UNDERLINED),
                        ));
                    }
                }
                _ => {}
            },
            Event::Text(text) => {
                if code_block {
                    if !spans.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    lines.extend(highlight_code_block(&text, &code_language, &syntax_set, &theme));
                } else {
                    spans.push(styled_span(&text, heading, code_block, blockquote, emph, strong, link_target.is_some()));
                }
            }
            Event::Code(code) => spans.push(Span::styled(
                code.to_string(),
                Style::default().fg(Color::Cyan).bg(Color::DarkGray),
            )),
            Event::SoftBreak | Event::HardBreak => {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
            Event::Rule => {
                if !spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut spans)));
                }
                lines.push(Line::from(Span::styled(
                    "─".repeat(60),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            _ => {}
        }
    }

    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(String::new()));
    }
    postprocess_image_slots(lines, preprocessed.image_slots)
}

#[derive(Default)]
struct PreprocessedMarkdown {
    content: String,
    image_slots: Vec<ImageSlot>,
}

fn preprocess_images(content: &str) -> PreprocessedMarkdown {
    let image_line = regex::Regex::new(r"^\s*!\[[^\]]*\]\(([^)]+)\)\s*$").unwrap();
    let mut out = String::new();
    let mut image_index = 0usize;
    let mut image_slots = Vec::new();

    for line in content.lines() {
        if image_line.is_match(line) {
            let marker = format!("SS_IMAGE_SLOT_{}", image_index);
            out.push_str(&marker);
            out.push('\n');
            image_slots.push(ImageSlot {
                image_index,
                start_line: 0,
                rows: 10,
            });
            image_index += 1;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    PreprocessedMarkdown { content: out, image_slots }
}

fn postprocess_image_slots(lines: Vec<Line<'static>>, mut image_slots: Vec<ImageSlot>) -> RenderedMarkdown {
    let mut out = Vec::new();
    let mut slot_cursor = 0usize;
    let mut line_index = 0usize;

    for line in lines {
        let plain = line.spans.iter().map(|span| span.content.as_ref()).collect::<String>();
        if let Some(index) = parse_image_slot_marker(&plain) {
            if let Some(slot) = image_slots.get_mut(index) {
                slot.start_line = line_index;
                for _ in 0..slot.rows {
                    out.push(Line::from(String::new()));
                    line_index += 1;
                }
                slot_cursor = slot_cursor.max(index + 1);
                continue;
            }
        }
        out.push(line);
        line_index += 1;
    }

    image_slots.truncate(slot_cursor.max(image_slots.len()));
    RenderedMarkdown { lines: out, image_slots }
}

fn parse_image_slot_marker(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    let prefix = "SS_IMAGE_SLOT_";
    if !trimmed.starts_with(prefix) {
        return None;
    }
    trimmed[prefix.len()..].parse::<usize>().ok()
}

fn detect_pipe_tables(content: &str) -> Vec<Line<'static>> {
    let raw_lines = content.lines().collect::<Vec<_>>();
    if raw_lines.len() < 2 {
        return Vec::new();
    }
    if !looks_like_pipe_table_row(raw_lines[0]) || !looks_like_pipe_separator(raw_lines[1]) {
        return Vec::new();
    }

    let mut rows = Vec::new();
    rows.push(parse_pipe_row(raw_lines[0]));
    for line in raw_lines.into_iter().skip(2) {
        if !looks_like_pipe_table_row(line) {
            break;
        }
        rows.push(parse_pipe_row(line));
    }
    render_table_rows(rows)
}

fn render_table_rows(rows: Vec<Vec<String>>) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }
    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; column_count];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.trim().len());
        }
    }

    let mut lines = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        let mut spans = Vec::new();
        spans.push(Span::styled("│ ", Style::default().fg(Color::DarkGray)));
        for (index, width) in widths.iter().enumerate() {
            let value = row.get(index).map(|v| v.trim()).unwrap_or("");
            let padded = format!("{value:<width$}", width = *width);
            let style = if row_index == 0 {
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            spans.push(Span::styled(padded, style));
            spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
        lines.push(Line::from(spans));
        if row_index == 0 {
            lines.push(table_separator(&widths));
        }
    }
    lines
}

fn looks_like_pipe_table_row(line: &str) -> bool {
    line.contains('|') && line.trim().starts_with('|') && line.trim().ends_with('|')
}

fn looks_like_pipe_separator(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && trimmed.chars().all(|ch| ch == '|' || ch == '-' || ch == ':' || ch.is_whitespace())
}

fn parse_pipe_row(line: &str) -> Vec<String> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn table_separator(widths: &[usize]) -> Line<'static> {
    let mut spans = Vec::new();
    spans.push(Span::styled("├", Style::default().fg(Color::DarkGray)));
    for (index, width) in widths.iter().enumerate() {
        spans.push(Span::styled("─".repeat(*width + 2), Style::default().fg(Color::DarkGray)));
        if index + 1 == widths.len() {
            spans.push(Span::styled("┤", Style::default().fg(Color::DarkGray)));
        } else {
            spans.push(Span::styled("┼", Style::default().fg(Color::DarkGray)));
        }
    }
    Line::from(spans)
}

fn highlight_code_block(text: &str, language: &str, syntax_set: &SyntaxSet, theme: &Theme) -> Vec<Line<'static>> {
    let syntax = syntax_set
        .find_syntax_by_token(language)
        .or_else(|| syntax_set.find_syntax_by_extension(language))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();

    for line in text.lines() {
        let ranges = highlighter.highlight_line(line, syntax_set).unwrap_or_default();
        let spans = ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_to_ratatui(style)))
            .collect::<Vec<_>>();
        out.push(Line::from(spans));
    }

    if out.is_empty() {
        out.push(Line::from(String::new()));
    }
    out
}

fn syntect_to_ratatui(style: SyntectStyle) -> Style {
    let mut ratatui_style = Style::default()
        .fg(rgb_to_color(style.foreground.r, style.foreground.g, style.foreground.b))
        .bg(rgb_to_color(style.background.r, style.background.g, style.background.b));
    if style.font_style.contains(syntect::highlighting::FontStyle::BOLD) {
        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(syntect::highlighting::FontStyle::ITALIC) {
        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(syntect::highlighting::FontStyle::UNDERLINE) {
        ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
    }
    ratatui_style
}

fn rgb_to_color(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

fn default_theme() -> Theme {
    ThemeSet::load_defaults().themes["InspiredGitHub"].clone()
}

fn styled_span(
    text: &str,
    heading: Option<HeadingLevel>,
    code_block: bool,
    blockquote: bool,
    emph: bool,
    strong: bool,
    link: bool,
) -> Span<'static> {
    let mut style = Style::default();
    if let Some(level) = heading {
        style = match level {
            HeadingLevel::H1 => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            HeadingLevel::H2 => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            HeadingLevel::H3 => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            _ => Style::default().add_modifier(Modifier::BOLD),
        };
    }
    if code_block {
        style = style.bg(Color::DarkGray).fg(Color::White);
    }
    if blockquote {
        style = style.fg(Color::Magenta);
    }
    if emph {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if strong {
        style = style.add_modifier(Modifier::BOLD);
    }
    if link {
        style = style.fg(Color::Blue).add_modifier(Modifier::UNDERLINED);
    }
    Span::styled(text.to_string(), style)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headings_and_lists() {
        let lines = render_markdown("# Title\n\n- one\n- two\n");
        assert!(!lines.lines.is_empty());
    }

    #[test]
    fn renders_tables() {
        let lines = render_markdown("| Name | Value |\n| --- | --- |\n| one | 1 |\n| two | 22 |\n");
        let joined = lines
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Name"), "{joined}");
    }
}
