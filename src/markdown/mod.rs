use std::path::Path;

use anyhow::Result;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::deck::loader::resolve_asset;
use crate::deck::model::{
    Block, CodeBlock, HeadingBlock, ImageBlock, ImageDisplay, Inline, ListBlock, ListItem,
    ParagraphBlock, TableBlock,
};

#[derive(Default)]
pub struct SlideDocument {
    pub blocks: Vec<Block>,
    pub assets: Vec<crate::deck::model::AssetRef>,
}

pub fn parse_slide(content: &str, dir: &Path, _slide_id: usize) -> Result<SlideDocument> {
    let parser = Parser::new_ext(content, Options::all());
    let mut state = ParseState::default();

    for event in parser {
        state.handle_event(event, dir);
    }
    state.finish();
    Ok(SlideDocument {
        blocks: state.blocks,
        assets: state.assets,
    })
}

pub fn slide_title(document: &SlideDocument) -> Option<String> {
    document.blocks.iter().find_map(|block| {
        if let Block::Heading(heading) = block {
            Some(flatten_inline(&heading.content))
        } else {
            None
        }
    })
}

#[derive(Default)]
struct ParseState {
    blocks: Vec<Block>,
    assets: Vec<crate::deck::model::AssetRef>,
    next_block_id: usize,
    heading_level: Option<HeadingLevel>,
    paragraph_inlines: Vec<Inline>,
    quote_inlines: Vec<Inline>,
    list_items: Vec<ListItem>,
    list_item_inlines: Vec<Inline>,
    table_rows: Vec<Vec<Vec<Inline>>>,
    table_row: Vec<Vec<Inline>>,
    table_cell: Vec<Inline>,
    code_language: String,
    code_text: String,
    in_code_block: bool,
    in_quote: bool,
    in_list: bool,
    in_table: bool,
    in_table_cell: bool,
    active_link: Option<String>,
    active_image: Option<(usize, String, Option<String>)>,
}

impl ParseState {
    fn handle_event(&mut self, event: Event<'_>, dir: &Path) {
        match event {
            Event::Start(tag) => self.start_tag(tag, dir),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(text.to_string()),
            Event::Code(code) => self.push_inline(Inline::Code(code.to_string())),
            Event::SoftBreak | Event::HardBreak => self.push_text("\n".to_string()),
            Event::Rule => self.blocks.push(Block::Rule),
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>, dir: &Path) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush_paragraph();
                self.heading_level = Some(level);
            }
            Tag::Paragraph => {
                self.flush_paragraph();
            }
            Tag::List(_) => {
                self.flush_paragraph();
                self.in_list = true;
                self.list_items.clear();
            }
            Tag::Item => {
                self.list_item_inlines.clear();
            }
            Tag::BlockQuote(_) => {
                self.flush_paragraph();
                self.in_quote = true;
            }
            Tag::CodeBlock(kind) => {
                self.flush_paragraph();
                self.in_code_block = true;
                self.code_language = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                self.code_text.clear();
            }
            Tag::Link { dest_url, .. } => {
                self.active_link = Some(dest_url.to_string());
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                let asset_id = self.assets.len();
                self.assets
                    .push(resolve_asset(dest_url.as_ref(), dir, asset_id));
                self.active_image = Some((
                    asset_id,
                    String::new(),
                    if title.is_empty() {
                        None
                    } else {
                        Some(title.to_string())
                    },
                ));
            }
            Tag::Table(_) => {
                self.flush_paragraph();
                self.in_table = true;
                self.table_rows.clear();
            }
            Tag::TableRow => {
                self.table_row.clear();
            }
            Tag::TableCell => {
                self.in_table_cell = true;
                self.table_cell.clear();
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                let level = self.heading_level.take().unwrap_or(HeadingLevel::H1);
                let content = std::mem::take(&mut self.paragraph_inlines);
                let id = self.next_id();
                self.blocks.push(Block::Heading(HeadingBlock {
                    id,
                    level: heading_to_u8(level),
                    content,
                }));
            }
            TagEnd::Paragraph => self.flush_paragraph(),
            TagEnd::List(_) => {
                self.in_list = false;
                let id = self.next_id();
                self.blocks.push(Block::List(ListBlock {
                    id,
                    items: std::mem::take(&mut self.list_items),
                }));
            }
            TagEnd::Item => {
                self.list_items.push(ListItem {
                    content: std::mem::take(&mut self.list_item_inlines),
                });
            }
            TagEnd::BlockQuote(_) => {
                self.in_quote = false;
                let id = self.next_id();
                self.blocks.push(Block::Quote(ParagraphBlock {
                    id,
                    content: std::mem::take(&mut self.quote_inlines),
                }));
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                let id = self.next_id();
                self.blocks.push(Block::Code(CodeBlock {
                    id,
                    language: std::mem::take(&mut self.code_language),
                    code: std::mem::take(&mut self.code_text),
                }));
            }
            TagEnd::Link => {
                self.active_link = None;
            }
            TagEnd::Image => {
                if let Some((asset_id, alt, title)) = self.active_image.take() {
                    self.flush_paragraph();
                    let id = self.next_id();
                    self.blocks.push(Block::Image(ImageBlock {
                        id,
                        asset_id,
                        alt,
                        title,
                        display: ImageDisplay::Inline,
                    }));
                }
            }
            TagEnd::Table => {
                self.in_table = false;
                let id = self.next_id();
                self.blocks.push(Block::Table(TableBlock {
                    id,
                    rows: std::mem::take(&mut self.table_rows),
                }));
            }
            TagEnd::TableRow => {
                if self.in_table {
                    self.table_rows.push(std::mem::take(&mut self.table_row));
                }
            }
            TagEnd::TableCell => {
                self.in_table_cell = false;
                self.table_row.push(std::mem::take(&mut self.table_cell));
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: String) {
        if self.active_image.is_some() {
            if let Some((_, alt, _)) = &mut self.active_image {
                alt.push_str(&text);
            }
            return;
        }
        if self.in_code_block {
            self.code_text.push_str(&text);
            return;
        }
        self.push_inline(if let Some(url) = &self.active_link {
            Inline::Link {
                text,
                url: url.clone(),
            }
        } else {
            Inline::Text(text)
        });
    }

    fn push_inline(&mut self, inline: Inline) {
        if self.in_table_cell {
            self.table_cell.push(inline);
        } else if self.in_list {
            self.list_item_inlines.push(inline);
        } else if self.in_quote {
            self.quote_inlines.push(inline);
        } else {
            self.paragraph_inlines.push(inline);
        }
    }

    fn flush_paragraph(&mut self) {
        if !self.paragraph_inlines.is_empty() {
            let id = self.next_id();
            self.blocks.push(Block::Paragraph(ParagraphBlock {
                id,
                content: std::mem::take(&mut self.paragraph_inlines),
            }));
        }
    }

    fn finish(&mut self) {
        self.flush_paragraph();
    }

    fn next_id(&mut self) -> usize {
        let id = self.next_block_id;
        self.next_block_id += 1;
        id
    }
}

fn heading_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
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
            Inline::Link { text, .. } => out.push_str(text),
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn parses_heading_and_image_block() {
        let doc = parse_slide("# Title\n\n![x](./img.png)", Path::new("."), 0).unwrap();
        assert!(matches!(doc.blocks[0], Block::Heading(_)));
        assert!(
            doc.blocks
                .iter()
                .any(|block| matches!(block, Block::Image(_)))
        );
    }
}
