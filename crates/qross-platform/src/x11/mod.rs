//! Linux mit X11. Wayland erlaubt kein globales Abfangen/Einspeisen ohne Portale
//! (libei/InputCapture) – das ist ein eigener, späterer Schritt.

mod capture;
mod inject;

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use qross_core::CaptureEvent;
use qross_proto::Rect;
use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
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

/// RandR-Monitore (ab RandR 1.5) in Root-Koordinaten wie `screen_bounds()`; ohne RandR leer.
pub fn monitors() -> Vec<Rect> {
    let slot = QUERY.get_or_init(|| Mutex::new(connect().ok()));
    let guard = slot.lock().unwrap();
    let Some((conn, root)) = guard.as_ref() else { return Vec::new() };
    randr_monitors(conn, *root).unwrap_or_default()
}

fn randr_monitors(conn: &RustConnection, root: Window) -> Result<Vec<Rect>> {
    let version = conn.randr_query_version(1, 5)?.reply()?;
    if (version.major_version, version.minor_version) < (1, 5) {
        return Ok(Vec::new());
    }
    let reply = conn.randr_get_monitors(root, true)?.reply()?;
    Ok(reply.monitors.iter().map(|m| Rect::new(m.x.into(), m.y.into(), m.width.into(), m.height.into())).collect())
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
