use std::sync::Arc;

use anyhow::bail;
use qross_core::CaptureEvent;
use qross_proto::{GameState, Rect};
use tokio::sync::mpsc::UnboundedSender;

use crate::{dryrun, Capture, Clipboard, Injector};

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
    bail!("Eingabe-Erfassung ist auf dieser Plattform noch nicht implementiert")
}

pub fn injector() -> anyhow::Result<Box<dyn Injector>> {
    bail!("Eingabe-Einspeisung ist auf dieser Plattform noch nicht implementiert (Tipp: --dry-run)")
}

pub fn clipboard() -> Box<dyn Clipboard> {
    Box::new(dryrun::MemoryClipboard::default())
}
