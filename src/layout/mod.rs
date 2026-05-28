#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::deck::model::{Block, ImageDisplay, Inline, Slide};

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

pub fn build_layout(slide: &Slide, viewport: Viewport) -> SlideLayout {
    let wrap_width = viewport.width.max(10) as usize;
    let mut row = 0usize;
    let mut lines = Vec::new();
    let mut images = Vec::new();
    let mut searchable_text = String::new();

    for block in &slide.blocks {
        match block {
            Block::Heading(block) => {
                let style = match block.level {
                    1 => Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                    2 => Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                    3 => Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                    _ => Style::default().add_modifier(Modifier::BOLD),
                };
                push_wrapped_inline_lines(
                    &block.content,
                    wrap_width,
                    style,
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                    None,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Paragraph(block) => {
                push_wrapped_inline_lines(
                    &block.content,
                    wrap_width,
                    Style::default(),
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                    None,
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Quote(block) => {
                push_wrapped_inline_lines(
                    &block.content,
                    wrap_width.saturating_sub(2),
                    Style::default().fg(Color::Magenta),
                    &mut row,
                    &mut lines,
                    &mut searchable_text,
                    Some("▌ "),
                );
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::List(block) => {
                for item in &block.items {
                    push_wrapped_inline_lines(
                        &item.content,
                        wrap_width.saturating_sub(2),
                        Style::default(),
                        &mut row,
                        &mut lines,
                        &mut searchable_text,
                        Some("• "),
                    );
                }
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Code(block) => {
                for code_line in block.code.lines() {
                    let spans = vec![Span::styled(
                        code_line.to_string(),
                        Style::default().fg(Color::White).bg(Color::DarkGray),
                    )];
                    lines.push(LayoutLine {
                        row,
                        spans,
                        search_text: code_line.to_string(),
                        text_span_index: 0,
                    });
                    searchable_text.push_str(code_line);
                    searchable_text.push('\n');
                    row += 1;
                }
                push_blank(&mut row, &mut lines, &mut searchable_text);
            }
            Block::Table(block) => {
                for table_row in &block.rows {
                    let text = table_row
                        .iter()
                        .map(|cell| flatten_inline(cell))
                        .collect::<Vec<_>>()
                        .join(" | ");
                    lines.push(LayoutLine {
                        row,
                        spans: vec![Span::styled(text.clone(), Style::default())],
                        search_text: text.clone(),
                        text_span_index: 0,
                    });
                    searchable_text.push_str(&text);
                    searchable_text.push('\n');
                    row += 1;
                }
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
) -> Vec<Line<'static>> {
    let end = scroll.saturating_add(height);
    let mut out = Vec::new();
    for row in scroll..end {
        if let Some(line) = layout.lines.iter().find(|line| line.row == row) {
            let row_matches = matches
                .iter()
                .enumerate()
                .filter(|(_, hit)| hit.row == row)
                .collect::<Vec<_>>();
            if row_matches.is_empty() || line.search_text.is_empty() {
                out.push(Line::from(line.spans.clone()));
                continue;
            }

            let mut spans = Vec::new();
            for (index, span) in line.spans.iter().enumerate() {
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
            base_style.bg(Color::DarkGray)
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

fn push_wrapped_inline_lines(
    inlines: &[Inline],
    wrap_width: usize,
    base_style: Style,
    row: &mut usize,
    lines: &mut Vec<LayoutLine>,
    searchable_text: &mut String,
    prefix: Option<&str>,
) {
    let prefix_text = prefix.unwrap_or("");
    let text = flatten_inline(inlines);
    let wrapped = wrap_text(&text, wrap_width.max(1));
    for (index, segment) in wrapped.into_iter().enumerate() {
        let mut spans = Vec::new();
        if index == 0 && !prefix_text.is_empty() {
            spans.push(Span::styled(prefix_text.to_string(), base_style));
        } else if !prefix_text.is_empty() {
            spans.push(Span::styled(" ".repeat(prefix_text.len()), base_style));
        }
        spans.push(Span::styled(
            segment.clone(),
            style_inline(inlines, base_style),
        ));
        lines.push(LayoutLine {
            row: *row,
            spans,
            search_text: segment.clone(),
            text_span_index: if prefix_text.is_empty() { 0 } else { 1 },
        });
        searchable_text.push_str(&segment);
        searchable_text.push('\n');
        *row += 1;
    }
}

fn style_inline(inlines: &[Inline], base_style: Style) -> Style {
    if inlines
        .iter()
        .any(|inline| matches!(inline, Inline::Code(_)))
    {
        base_style.fg(Color::Cyan).bg(Color::DarkGray)
    } else {
        base_style
    }
}

fn flatten_inline(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(text)
            | Inline::Emphasis(text)
            | Inline::Strong(text)
            | Inline::Code(text) => out.push_str(text),
            Inline::Link { text, url } => {
                out.push_str(text);
                out.push_str(" (");
                out.push_str(url);
                out.push(')');
            }
        }
    }
    out
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    for raw_line in text.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                current.push_str(word);
            } else if current.len() + 1 + word.len() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current);
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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
