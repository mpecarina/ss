use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;

#[derive(Clone, Debug, Default)]
pub struct Slide {
    pub path: PathBuf,
    pub name: String,
    pub title: String,
    pub content: String,
    pub images: Vec<ImageRef>,
}

#[derive(Clone, Debug, Default)]
pub struct ImageRef {
    pub path: String,
}

#[derive(Debug, Deserialize, Default)]
struct Frontmatter {
    layout: Option<String>,
}

pub fn load_slides(dir: &Path) -> Result<Vec<Slide>> {
    let metadata = fs::metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
    if !metadata.is_dir() {
        bail!("{} is not a directory", dir.display());
    }

    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().map(|v| v.eq_ignore_ascii_case("md")).unwrap_or(false) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        bail!("no markdown slides found");
    }
    paths.sort_by(|a, b| natural_key(a.file_name(), b.file_name()));

    let mut slides = Vec::with_capacity(paths.len());
    for path in paths {
        let absolute_path = fs::canonicalize(&path).unwrap_or(path.clone());
        let raw = fs::read_to_string(&absolute_path).with_context(|| format!("read {}", absolute_path.display()))?;
        let (content, _layout) = parse_frontmatter(&raw);
        let title = first_heading(&content).unwrap_or_else(|| fallback_title(&absolute_path));
        let images = extract_images(&content, absolute_path.parent().unwrap_or(dir));
        slides.push(Slide {
            path: absolute_path.clone(),
            name: absolute_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
            title,
            content,
            images,
        });
    }
    Ok(slides)
}

fn parse_frontmatter(raw: &str) -> (String, String) {
    if !raw.starts_with("---\n") {
        return (raw.to_string(), String::new());
    }
    let rest = &raw[4..];
    if let Some(end) = rest.find("\n---\n") {
        let head = &rest[..end];
        let body = rest[end + 5..].trim_start_matches('\n').to_string();
        let parsed = serde_yaml::from_str::<Frontmatter>(head).unwrap_or_default();
        return (body, parsed.layout.unwrap_or_default());
    }
    (raw.to_string(), String::new())
}

fn first_heading(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix('#')
            .map(|value| value.trim_start_matches('#').trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(['_', '-'], " ")
}

fn extract_images(content: &str, dir: &Path) -> Vec<ImageRef> {
    let re = Regex::new(r"!\[[^\]]*\]\(([^)]+)\)").unwrap();
    re.captures_iter(content)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().trim().trim_matches(&['<', '>'][..]).to_string()))
        .map(|path| {
            let resolved = if path.starts_with("http://") || path.starts_with("https://") || Path::new(&path).is_absolute() {
                path
            } else {
                dir.join(path).to_string_lossy().to_string()
            };
            ImageRef { path: resolved }
        })
        .collect()
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
            out.push(NaturalToken::Number(input[start..i].parse::<u64>().unwrap_or(0)));
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
        assert_eq!(layout, "image");
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
