use std::fmt::Write as _;
use std::path::Path;

use crate::slides::ImageRef;
use crate::tmux::TmuxContext;

#[derive(Clone, Debug, Default)]
pub struct ImagePlacement {
    pub path: String,
    pub row: u16,
    pub col: u16,
    pub cols: u16,
    pub rows: u16,
    pub image_id: u32,
    pub placement_id: u32,
}

pub trait ImageBackend {
    fn available(&self) -> bool;
    fn name(&self) -> &'static str;
    fn clear_sequence(&self) -> String;
    fn draw_sequence(&self, placements: &[ImagePlacement]) -> String;
}

#[derive(Clone, Debug, Default)]
pub struct NoopBackend;

impl ImageBackend for NoopBackend {
    fn available(&self) -> bool {
        false
    }

    fn name(&self) -> &'static str {
        "disabled"
    }

    fn clear_sequence(&self) -> String {
        String::new()
    }

    fn draw_sequence(&self, _placements: &[ImagePlacement]) -> String {
        String::new()
    }
}

#[derive(Clone, Debug)]
pub struct KittyBackend {
    tmux: TmuxContext,
}

impl KittyBackend {
    pub fn new(tmux: TmuxContext) -> Self {
        Self { tmux }
    }
}

impl ImageBackend for KittyBackend {
    fn available(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "kitty"
    }

    fn clear_sequence(&self) -> String {
        wrap_for_tmux("\x1b_Ga=d,d=A\x1b\\", &self.tmux)
    }

    fn draw_sequence(&self, placements: &[ImagePlacement]) -> String {
        let mut out = String::new();
        for placement in placements {
            if !Path::new(&placement.path).exists() {
                continue;
            }
            let payload = base64_path(&placement.path);
            let _ = write!(out, "\x1b[{};{}H", placement.row, placement.col);
            let _ = write!(
                out,
                "\x1b_Ga=T,f=100,t=f,i={},p={},c={},r={},C=1;{}\x1b\\",
                placement.image_id,
                placement.placement_id,
                placement.cols,
                placement.rows,
                payload
            );
        }
        wrap_for_tmux(&out, &self.tmux)
    }
}

pub fn detect_backend(tmux: TmuxContext) -> Box<dyn ImageBackend> {
    let forced = std::env::var("SS_IMAGE_BACKEND")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    match forced.as_str() {
        "none" | "off" | "disabled" => return Box::new(NoopBackend),
        "kitty" | "ghostty" => return Box::new(KittyBackend::new(tmux)),
        _ => {}
    }

    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    if term_program == "ghostty"
        || term.contains("ghostty")
        || term.contains("kitty")
        || std::env::var("KITTY_WINDOW_ID").is_ok()
        || std::env::var("GHOSTTY_BIN_DIR").is_ok()
    {
        return Box::new(KittyBackend::new(tmux));
    }

    Box::new(NoopBackend)
}

pub fn build_placement(image: &ImageRef, width: u16, height: u16) -> Option<ImagePlacement> {
    build_placement_at(image, 2, 2, width, height)
}

pub fn build_placement_at(image: &ImageRef, row: u16, col: u16, width: u16, height: u16) -> Option<ImagePlacement> {
    if image.path.is_empty() {
        return None;
    }
    Some(ImagePlacement {
        path: image.path.clone(),
        row,
        col,
        cols: width.max(10),
        rows: height.max(6),
        image_id: 1,
        placement_id: 1,
    })
}

pub fn build_grid_placements(images: &[ImageRef], row: u16, col: u16, width: u16, height: u16) -> Vec<ImagePlacement> {
    if images.is_empty() || width < 4 || height < 4 {
        return Vec::new();
    }

    let count = images.len().min(4);
    let mut placements = Vec::new();
    match count {
        1 => {
            if let Some(mut placement) = build_placement_at(&images[0], row, col, width, height) {
                placement.image_id = 1;
                placement.placement_id = 1;
                placements.push(placement);
            }
        }
        2 => {
            let half = height / 2;
            for (index, image) in images.iter().take(2).enumerate() {
                let top = if index == 0 { row } else { row + half };
                let rows = if index == 0 { half.saturating_sub(1) } else { height.saturating_sub(half).saturating_sub(1) };
                if let Some(mut placement) = build_placement_at(image, top, col, width, rows.max(6)) {
                    placement.image_id = (index + 1) as u32;
                    placement.placement_id = (index + 1) as u32;
                    placements.push(placement);
                }
            }
        }
        _ => {
            let half_w = width / 2;
            let half_h = height / 2;
            for (index, image) in images.iter().take(count).enumerate() {
                let r = index / 2;
                let c = index % 2;
                let top = row + (r as u16 * half_h);
                let left = col + (c as u16 * half_w);
                let cols = if c == 0 { half_w.saturating_sub(1) } else { width.saturating_sub(half_w).saturating_sub(1) };
                let rows = if r == 0 { half_h.saturating_sub(1) } else { height.saturating_sub(half_h).saturating_sub(1) };
                if let Some(mut placement) = build_placement_at(image, top, left, cols.max(10), rows.max(6)) {
                    placement.image_id = (index + 1) as u32;
                    placement.placement_id = (index + 1) as u32;
                    placements.push(placement);
                }
            }
        }
    }
    placements
}

fn wrap_for_tmux(seq: &str, tmux: &TmuxContext) -> String {
    if !tmux.in_tmux() || seq.is_empty() {
        return seq.to_string();
    }
    let escaped = seq.replace('\x1b', "\x1b\x1b");
    format!("\x1bPtmux;{}\x1b\\", escaped)
}

fn base64_path(path: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = path.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = if i + 1 < bytes.len() { bytes[i + 1] } else { 0 };
        let b2 = if i + 2 < bytes.len() { bytes[i + 2] } else { 0 };
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(TABLE[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < bytes.len() {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}
