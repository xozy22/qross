//! Linux: Auswahl zur Laufzeit zwischen Wayland (Desktop-Portale und libei) und X11.
//!
//! - In einer Wayland-Sitzung (`WAYLAND_DISPLAY`) speist Qross über das Portal RemoteDesktop ein.
//!   Fehlt es oder kann es noch kein libei, geht es per XTest an XWayland, sofern `DISPLAY` gesetzt
//!   ist. Das erreicht je nach Compositor nur X11-Programme.
//! - Erfasst wird in einer Wayland-Sitzung nur über das Portal InputCapture. X11-Grabs unter
//!   XWayland sehen den Cursor nur über X11-Fenstern und taugen dafür nicht.
//! - Die Zwischenablage nimmt `arboard`: unter Wayland über data-control (KDE, wlroots), sonst
//!   über XWayland.
//! - `QROSS_BACKEND=x11` bzw. `wayland` erzwingt eine Seite.

use std::sync::{Arc, OnceLock};

use anyhow::Result;
use qross_core::CaptureEvent;
use qross_proto::Rect;
use tokio::sync::mpsc::UnboundedSender;

use crate::wayland_logic::{hull, linux_session, LinuxSession};
use crate::{wayland, x11, Capture, Clipboard, FinderOverlay, Injector};

pub fn session() -> LinuxSession {
    static SESSION: OnceLock<LinuxSession> = OnceLock::new();
    *SESSION.get_or_init(|| {
        let var = |name: &str| std::env::var(name).ok();
        linux_session(
            var("QROSS_BACKEND").as_deref(),
            var("WAYLAND_DISPLAY").as_deref(),
            var("XDG_SESSION_TYPE").as_deref(),
        )
    })
}

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

pub fn init_process() {
    x11::init_process();
    match session() {
        LinuxSession::Wayland => tracing::info!("Wayland-Sitzung: Eingaben über die Desktop-Portale (libei)"),
        LinuxSession::X11 => tracing::debug!("X11-Sitzung"),
    }
}

pub fn screen_bounds() -> Rect {
    if session() == LinuxSession::Wayland {
        if let Some(rect) = wayland::geometry().as_deref().and_then(hull) {
            return rect;
        }
    }
    x11::screen_bounds()
}

pub fn monitors() -> Vec<Rect> {
    if session() == LinuxSession::Wayland {
        if let Some(monitors) = wayland::geometry() {
            return monitors;
        }
    }
    x11::monitors()
}

/// Spielemodus: Vollbild erkennt Qross unter X11 (EWMH). In einer Wayland-Sitzung ist das aktive
/// Fenster für andere Programme nicht abfragbar; dort hält der Spielemodus deshalb nicht fest.
pub fn game_state() -> qross_proto::GameState {
    match session() {
        LinuxSession::X11 => x11::game_state(),
        LinuxSession::Wayland => qross_proto::GameState::default(),
    }
}

pub fn start_capture(events: UnboundedSender<CaptureEvent>) -> Result<Arc<dyn Capture>> {
    match session() {
        LinuxSession::Wayland => wayland::start_capture(events),
        LinuxSession::X11 => x11::start_capture(events),
    }
}

pub fn injector() -> Result<Box<dyn Injector>> {
    if session() == LinuxSession::X11 {
        return x11::injector();
    }
    let portal = env_set("LIBEI_SOCKET") || wayland::portals().remote_desktop.is_some_and(|v| v >= 2);
    if !portal && env_set("DISPLAY") {
        tracing::warn!(
            "Kein Desktop-Portal mit RemoteDesktop und libei – Qross speist über XWayland (XTest) ein. \
             Das erreicht je nach Compositor nur X11-Programme (siehe docs/wayland.md)."
        );
        return x11::injector();
    }
    wayland::injector()
}

pub fn clipboard() -> Box<dyn Clipboard> {
    x11::clipboard()
}

/// Cursor-Finder nur unter X11: Wayland lässt keine Fenster über anderen Programmen zu, und unter
/// XWayland kennt Qross die Cursorposition nur über X11-Fenstern.
pub fn finder_overlay() -> Option<Arc<dyn FinderOverlay>> {
    match session() {
        LinuxSession::X11 => Some(x11::finder_overlay()),
        LinuxSession::Wayland => None,
    }
}
