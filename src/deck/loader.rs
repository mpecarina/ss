use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use imagesize::size;
use serde::Deserialize;

use crate::deck::model::{AssetRef, AssetSize, Deck, DeckMetadata, Slide, SlideFrontmatter};
use crate::markdown;

#[derive(Debug, Deserialize, Default)]
struct RawFrontmatter {
    layout: Option<String>,
}

pub fn load_deck(dir: &Path) -> Result<Deck> {
    let metadata = fs::metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
    if !metadata.is_dir() {
        bail!("{} is not a directory", dir.display());
    }

    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .map(|v| v.eq_ignore_ascii_case("md"))
                .unwrap_or(false)
        {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        bail!("no markdown slides found");
    }
    paths.sort_by(|a, b| natural_key(a.file_name(), b.file_name()));

    let mut slides = Vec::with_capacity(paths.len());
    for (slide_id, path) in paths.into_iter().enumerate() {
        let absolute_path = fs::canonicalize(&path).unwrap_or(path.clone());
        let raw = fs::read_to_string(&absolute_path)
            .with_context(|| format!("read {}", absolute_path.display()))?;
        let (body, frontmatter) = parse_frontmatter(&raw);
        let parent = absolute_path.parent().unwrap_or(dir);
        let document = markdown::parse_slide(&body, parent, slide_id)?;
        let title =
            markdown::slide_title(&document).unwrap_or_else(|| fallback_title(&absolute_path));
        slides.push(Slide {
            id: slide_id,
            path: absolute_path.clone(),
            name: absolute_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            title,
            frontmatter,
            blocks: document.blocks,
            assets: document.assets,
        });
    }

    let title = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "ss".to_string());
    Ok(Deck {
        root: dir.to_path_buf(),
        metadata: DeckMetadata { title },
        slides,
    })
}

fn parse_frontmatter(raw: &str) -> (String, SlideFrontmatter) {
    if !raw.starts_with("---\n") {
        return (raw.to_string(), SlideFrontmatter::default());
    }
    let rest = &raw[4..];
    if let Some(end) = rest.find("\n---\n") {
        let head = &rest[..end];
        let body = rest[end + 5..].trim_start_matches('\n').to_string();
        let parsed = serde_yaml::from_str::<RawFrontmatter>(head).unwrap_or_default();
        return (
            body,
            SlideFrontmatter {
                layout: parsed.layout,
            },
        );
    }
    (raw.to_string(), SlideFrontmatter::default())
}

pub(crate) fn resolve_asset(path: &str, dir: &Path, asset_id: usize) -> AssetRef {
    let resolved = if path.starts_with("http://")
        || path.starts_with("https://")
        || Path::new(path).is_absolute()
    {
        PathBuf::from(path)
    } else {
        dir.join(path)
    };
    let size = size(&resolved).ok().map(|value| AssetSize {
        width: value.width as u32,
        height: value.height as u32,
    });
    AssetRef {
        id: asset_id,
        path: resolved,
        size,
    }
}

fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(['_', '-'], " ")
}

fn natural_key(a: Option<&std::ffi::OsStr>, b: Option<&std::ffi::OsStr>) -> std::cmp::Ordering {
    let a = a.unwrap_or_default().to_string_lossy();
    let b = b.unwrap_or_default().to_string_lossy();
    tokenize_natural(&a).cmp(&tokenize_natural(&b))
}

fn tokenize_natural(input: &str) -> Vec<NaturalToken> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            out.push(NaturalToken::Number(
                input[start..i].parse::<u64>().unwrap_or(0),
            ));
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_digit() {
                i += 1;
            }
            out.push(NaturalToken::Text(input[start..i].to_lowercase()));
        }
    }
    out
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum NaturalToken {
    Text(String),
    Number(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_heading_and_layout() {
        let (body, layout) = parse_frontmatter("---\nlayout: image\n---\n# Title\n");
        assert_eq!(layout.layout.as_deref(), Some("image"));
        assert!(body.contains("# Title"));
    }

    #[test]
    fn natural_sort_orders_numeric_names() {
        let mut items = [
            PathBuf::from("10_last.md"),
            PathBuf::from("02_middle.md"),
            PathBuf::from("01_first.md"),
        ];
        items.sort_by(|a, b| natural_key(a.file_name(), b.file_name()));
        assert_eq!(items[0].to_string_lossy(), "01_first.md");
    }
}
