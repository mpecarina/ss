use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine;

use crate::deck::model::Slide;
use crate::layout::LaidOutImage;
use crate::tmux::TmuxRuntime;

#[derive(Clone, Debug, Default)]
pub struct ImagePlacementSpec {
    pub block_id: usize,
    pub asset_path: String,
    pub row: u16,
    pub col: u16,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageHandle {
    pub runtime_id: String,
    pub block_id: usize,
    pub image_id: u32,
    pub placement_id: u32,
}

pub trait ImageBackend {
    fn available(&self) -> bool;
    fn name(&self) -> &'static str;
    fn draw_sequence(&self, placements: &[OwnedPlacement]) -> String;
    fn delete_sequence(&self, handles: &[ImageHandle]) -> String;
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

    fn draw_sequence(&self, _placements: &[OwnedPlacement]) -> String {
        String::new()
    }

    fn delete_sequence(&self, _handles: &[ImageHandle]) -> String {
        String::new()
    }
}

#[derive(Clone, Debug)]
pub struct KittyBackend {
    tmux: TmuxRuntime,
}

impl KittyBackend {
    pub fn new(tmux: TmuxRuntime) -> Self {
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

    fn draw_sequence(&self, placements: &[OwnedPlacement]) -> String {
        let mut out = String::new();
        for placement in placements {
            if !Path::new(&placement.spec.asset_path).exists() {
                continue;
            }
            let payload = base64::engine::general_purpose::STANDARD
                .encode(placement.spec.asset_path.as_bytes());
            let _ = write!(out, "\x1b[{};{}H", placement.spec.row, placement.spec.col);
            let _ = write!(
                out,
                "\x1b_Ga=T,f=100,t=f,i={},p={},c={},r={},C=1;{}\x1b\\",
                placement.handle.image_id,
                placement.handle.placement_id,
                placement.spec.cols,
                placement.spec.rows,
                payload
            );
        }
        self.tmux.wrap_passthrough(&out)
    }

    fn delete_sequence(&self, handles: &[ImageHandle]) -> String {
        let mut out = String::new();
        for handle in handles {
            let _ = write!(out, "\x1b_Ga=d,d=i,i={}\x1b\\", handle.image_id);
        }
        self.tmux.wrap_passthrough(&out)
    }
}

#[derive(Clone, Debug)]
pub struct OwnedPlacement {
    pub handle: ImageHandle,
    pub spec: ImagePlacementSpec,
}

#[derive(Default)]
pub struct ImageCompositor {
    runtime_id: String,
    next_image_id: AtomicU32,
    visible: BTreeMap<usize, OwnedPlacement>,
}

impl ImageCompositor {
    pub fn new(runtime_id: String) -> Self {
        Self {
            runtime_id,
            next_image_id: AtomicU32::new(1),
            visible: BTreeMap::new(),
        }
    }

    pub fn reconcile(&mut self, desired: Vec<ImagePlacementSpec>) -> ImageDiff {
        let desired_map = desired
            .into_iter()
            .map(|spec| (spec.block_id, spec))
            .collect::<BTreeMap<_, _>>();
        let mut retire = Vec::new();
        let existing_keys = self.visible.keys().copied().collect::<Vec<_>>();
        for key in existing_keys {
            let unchanged = self
                .visible
                .get(&key)
                .and_then(|current| {
                    desired_map
                        .get(&key)
                        .map(|next| same_spec(&current.spec, next))
                })
                .unwrap_or(false);
            if !unchanged {
                if let Some(owned) = self.visible.remove(&key) {
                    retire.push(owned.handle);
                }
            }
        }

        let mut draw = Vec::new();
        for (block_id, spec) in desired_map {
            if self.visible.contains_key(&block_id) {
                continue;
            }
            let owned = OwnedPlacement {
                handle: ImageHandle {
                    runtime_id: self.runtime_id.clone(),
                    block_id,
                    image_id: self.next_id(),
                    placement_id: self.next_id(),
                },
                spec,
            };
            self.visible.insert(block_id, owned.clone());
            draw.push(owned);
        }

        ImageDiff { retire, draw }
    }

    pub fn clear(&mut self) -> Vec<ImageHandle> {
        self.visible
            .values()
            .map(|owned| owned.handle.clone())
            .collect::<Vec<_>>()
            .tap(|_| self.visible.clear())
    }

    fn next_id(&self) -> u32 {
        self.next_image_id.fetch_add(1, Ordering::Relaxed)
    }
}

pub struct ImageDiff {
    pub retire: Vec<ImageHandle>,
    pub draw: Vec<OwnedPlacement>,
}

pub fn detect_backend(tmux: &TmuxRuntime) -> Box<dyn ImageBackend> {
    let forced = std::env::var("SS_IMAGE_BACKEND")
        .unwrap_or_default()
        .trim()
        .to_lowercase();
    match forced.as_str() {
        "none" | "off" | "disabled" => return Box::new(NoopBackend),
        "kitty" | "ghostty" => return Box::new(KittyBackend::new(tmux.clone())),
        _ => {}
    }

    let term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    let explicit_kitty = std::env::var("KITTY_WINDOW_ID").is_ok();
    let explicit_ghostty = term_program == "ghostty" || std::env::var("GHOSTTY_BIN_DIR").is_ok();
    if term_program == "ghostty"
        || term.contains("ghostty")
        || term.contains("kitty")
        || explicit_kitty
        || explicit_ghostty
    {
        if tmux.in_tmux() && !explicit_kitty && !explicit_ghostty {
            return Box::new(NoopBackend);
        }
        return Box::new(KittyBackend::new(tmux.clone()));
    }
    Box::new(NoopBackend)
}

pub fn placements_for_view(
    slide: &Slide,
    images: &[LaidOutImage],
    scroll: usize,
    body_y: u16,
    body_x: u16,
    body_height: u16,
) -> Vec<ImagePlacementSpec> {
    images
        .iter()
        .filter(|image| {
            image.start_row + image.rows > scroll && image.start_row < scroll + body_height as usize
        })
        .filter_map(|image| {
            let asset = slide
                .assets
                .iter()
                .find(|asset| asset.id == image.asset_id)?;
            let local_row = image.start_row.saturating_sub(scroll) as u16;
            Some(ImagePlacementSpec {
                block_id: image.block_id,
                asset_path: asset.path.to_string_lossy().to_string(),
                row: body_y.saturating_add(local_row).saturating_add(1),
                col: body_x.saturating_add(1),
                cols: image.cols,
                rows: image.rows.min(body_height as usize) as u16,
            })
        })
        .collect()
}

fn same_spec(a: &ImagePlacementSpec, b: &ImagePlacementSpec) -> bool {
    a.asset_path == b.asset_path
        && a.row == b.row
        && a.col == b.col
        && a.cols == b.cols
        && a.rows == b.rows
}

trait Tap: Sized {
    fn tap<F: FnOnce(&Self)>(self, f: F) -> Self {
        f(&self);
        self
    }
}

impl<T> Tap for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositor_retires_changed_block() {
        let mut compositor = ImageCompositor::new("runtime".to_string());
        let first = compositor.reconcile(vec![ImagePlacementSpec {
            block_id: 1,
            asset_path: "a.png".to_string(),
            row: 1,
            col: 1,
            cols: 10,
            rows: 5,
        }]);
        assert_eq!(first.draw.len(), 1);
        let second = compositor.reconcile(vec![ImagePlacementSpec {
            block_id: 1,
            asset_path: "a.png".to_string(),
            row: 2,
            col: 1,
            cols: 10,
            rows: 5,
        }]);
        assert_eq!(second.retire.len(), 1);
        assert_eq!(second.draw.len(), 1);
    }
}
