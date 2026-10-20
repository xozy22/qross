use std::sync::Arc;

use anyhow::bail;
use qross_core::CaptureEvent;
use qross_proto::{GameState, Rect};
use tokio::sync::mpsc::UnboundedSender;

use crate::{dryrun, Capture, Clipboard, FinderOverlay, Injector};

pub fn init_process() {}

pub fn screen_bounds() -> Rect {
    Rect::new(0, 0, 1920, 1080)
}

pub fn monitors() -> Vec<Rect> {
    Vec::new()
}

pub fn game_state() -> GameState {
    GameState::default()
}

pub fn start_capture(_events: UnboundedSender<CaptureEvent>) -> anyhow::Result<Arc<dyn Capture>> {
    bail!("input capture is not implemented on this platform yet")
}

pub fn injector() -> anyhow::Result<Box<dyn Injector>> {
    bail!("input injection is not implemented on this platform yet (hint: --dry-run)")
}

pub fn clipboard() -> Box<dyn Clipboard> {
    Box::new(dryrun::MemoryClipboard::default())
}

/// Ohne Fenster kein Cursor-Finder
pub fn finder_overlay() -> Option<Arc<dyn FinderOverlay>> {
    None
}
