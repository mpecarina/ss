#![allow(dead_code)]

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct Deck {
    pub root: PathBuf,
    pub metadata: DeckMetadata,
    pub slides: Vec<Slide>,
}

#[derive(Clone, Debug, Default)]
pub struct DeckMetadata {
    pub title: String,
}

#[derive(Clone, Debug, Default)]
pub struct Slide {
    pub id: usize,
    pub path: PathBuf,
    pub name: String,
    pub title: String,
    pub frontmatter: SlideFrontmatter,
    pub blocks: Vec<Block>,
    pub assets: Vec<AssetRef>,
}

#[derive(Clone, Debug, Default)]
pub struct SlideFrontmatter {
    pub layout: Option<String>,
}

pub type BlockId = usize;
pub type AssetId = usize;

#[derive(Clone, Debug)]
pub enum Block {
    Heading(HeadingBlock),
    Paragraph(ParagraphBlock),
    List(ListBlock),
    Table(TableBlock),
    Code(CodeBlock),
    Quote(ParagraphBlock),
    Rule,
    Image(ImageBlock),
}

#[derive(Clone, Debug, Default)]
pub struct HeadingBlock {
    pub id: BlockId,
    pub level: u8,
    pub content: Vec<Inline>,
}

#[derive(Clone, Debug, Default)]
pub struct ParagraphBlock {
    pub id: BlockId,
    pub content: Vec<Inline>,
}

#[derive(Clone, Debug, Default)]
pub struct ListBlock {
    pub id: BlockId,
    pub items: Vec<ListItem>,
}

#[derive(Clone, Debug, Default)]
pub struct ListItem {
    pub content: Vec<Inline>,
}

#[derive(Clone, Debug, Default)]
pub struct TableBlock {
    pub id: BlockId,
    pub rows: Vec<Vec<Vec<Inline>>>,
}

#[derive(Clone, Debug, Default)]
pub struct CodeBlock {
    pub id: BlockId,
    pub language: String,
    pub code: String,
}

#[derive(Clone, Debug, Default)]
pub struct ImageBlock {
    pub id: BlockId,
    pub asset_id: AssetId,
    pub alt: String,
    pub title: Option<String>,
    pub display: ImageDisplay,
}

#[derive(Clone, Debug, Default)]
pub struct AssetRef {
    pub id: AssetId,
    pub path: PathBuf,
    pub size: Option<AssetSize>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AssetSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImageDisplay {
    #[default]
    Inline,
    FullWidth,
    Cover,
}

#[derive(Clone, Debug)]
pub enum Inline {
    Text(String),
    Emphasis(String),
    Strong(String),
    Code(String),
    Link { text: String, url: String },
}

impl Default for Inline {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl Block {
    pub fn id(&self) -> BlockId {
        match self {
            Block::Heading(block) => block.id,
            Block::Paragraph(block) => block.id,
            Block::List(block) => block.id,
            Block::Table(block) => block.id,
            Block::Code(block) => block.id,
            Block::Quote(block) => block.id,
            Block::Rule => usize::MAX,
            Block::Image(block) => block.id,
        }
    }
}
