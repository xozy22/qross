//! Linux mit X11. Wayland erlaubt kein globales Abfangen/Einspeisen ohne Portale
//! (libei/InputCapture) – das ist ein eigener, späterer Schritt.

mod capture;
mod inject;

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use glide_core::CaptureEvent;
use glide_proto::Rect;
use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use crate::arboard_clipboard::ArboardClipboard;
use crate::{Capture, Clipboard, Injector};

pub(crate) fn connect() -> Result<(RustConnection, Window)> {
    let (conn, screen) = x11rb::connect(None)
        .context("keine Verbindung zum X-Server – ist DISPLAY gesetzt? (Wayland wird noch nicht unterstützt)")?;
    let root = conn.setup().roots[screen].root;
    Ok((conn, root))
}

pub(crate) fn root_size(conn: &RustConnection, root: Window) -> Result<(u16, u16)> {
    let geometry = conn.get_geometry(root)?.reply()?;
    Ok((geometry.width, geometry.height))
}

pub fn init_process() {}

/// Eine gemeinsame Verbindung für die regelmäßige Größenabfrage
static QUERY: OnceLock<Mutex<Option<(RustConnection, Window)>>> = OnceLock::new();

pub fn screen_bounds() -> Rect {
    let slot = QUERY.get_or_init(|| Mutex::new(connect().ok()));
    let guard = slot.lock().unwrap();
    match guard.as_ref().and_then(|(conn, root)| root_size(conn, *root).ok()) {
        Some((width, height)) => Rect::new(0, 0, width.into(), height.into()),
        None => Rect::new(0, 0, 1920, 1080),
    }
}

pub fn start_capture(events: UnboundedSender<CaptureEvent>) -> Result<Arc<dyn Capture>> {
    capture::start(events)
}

pub fn injector() -> Result<Box<dyn Injector>> {
    Ok(Box::new(inject::X11Injector::new()?))
}

pub fn clipboard() -> Box<dyn Clipboard> {
    Box::new(ArboardClipboard::default())
}
