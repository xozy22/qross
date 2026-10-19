//! Linux mit X11 (auch XWayland als Rückfall für das Einspeisen, siehe `linux.rs`). Wayland selbst
//! läuft über die Desktop-Portale im Modul `wayland`.

mod capture;
mod desktop;
mod finder;
mod inject;
mod keymap;
mod locks;
mod xdnd;

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};
use qross_core::CaptureEvent;
use qross_proto::{GameState, Rect};
use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::NONE;

use crate::arboard_clipboard::ArboardClipboard;
use crate::{Capture, Clipboard, FinderOverlay, Injector};

pub(crate) fn connect() -> Result<(RustConnection, Window)> {
    let (conn, screen) = x11rb::connect(None)
        .context("keine Verbindung zum X-Server – ist DISPLAY gesetzt?")?;
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

/// Spielemodus: Ist das aktive Fenster im Vollbild (EWMH `_NET_WM_STATE_FULLSCREEN`)? Ob ein
/// Programm den Zeiger gegriffen hat, ließe sich nur prüfen, indem Qross selbst kurz nach ihm
/// greift; das unterbleibt.
pub fn game_state() -> GameState {
    let slot = QUERY.get_or_init(|| Mutex::new(connect().ok()));
    let guard = slot.lock().unwrap();
    let fullscreen = guard.as_ref().and_then(|(conn, root)| active_fullscreen(conn, *root).ok()).unwrap_or(false);
    GameState { fullscreen, captured: false }
}

fn active_fullscreen(conn: &RustConnection, root: Window) -> Result<bool> {
    // Ohne Fenstermanager mit EWMH gibt es die Atome nicht
    let atom = |name: &str| -> Result<Atom> { Ok(conn.intern_atom(true, name.as_bytes())?.reply()?.atom) };
    let (active, state, fullscreen) = (atom("_NET_ACTIVE_WINDOW")?, atom("_NET_WM_STATE")?, atom("_NET_WM_STATE_FULLSCREEN")?);
    if [active, state, fullscreen].contains(&NONE) {
        return Ok(false);
    }
    let reply = conn.get_property(false, root, active, AtomEnum::WINDOW, 0, 1)?.reply()?;
    let Some(window) = reply.value32().and_then(|mut values| values.next()).filter(|&w| w != NONE) else {
        return Ok(false);
    };
    let states = conn.get_property(false, window, state, AtomEnum::ATOM, 0, 64)?.reply()?;
    Ok(states.value32().is_some_and(|mut values| values.any(|a| a == fullscreen)))
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

pub fn finder_overlay() -> Arc<dyn FinderOverlay> {
    Arc::new(crate::finder::Overlay::new(finder::surface))
}
