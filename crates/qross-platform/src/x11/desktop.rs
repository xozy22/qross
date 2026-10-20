//! Drag & Drop: Liegt unter dem Cursor der Schreibtisch? Erkannt am Fenstertyp
//! `_NET_WM_WINDOW_TYPE_DESKTOP`, den xfdesktop, Plasma, Nemo, Caja, PCManFM und die Schreibtisch-
//! Symbole unter GNOME setzen. Dann landen die Dateien im Schreibtisch-Ordner (`XDG_DESKTOP_DIR`).
//!
//! Dateimanager verraten unter X11 nicht verlässlich, welchen Ordner ein Fenster zeigt: Der Titel
//! nennt nur den Namen; Dolphin, Nemo und Thunar sagen es gar nicht, Nautilus über D-Bus nur alle
//! Orte eines Fensters samt Tabs zusammen. Dort bleibt es bei „Downloads/Qross“.

use std::path::PathBuf;

use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::NONE;

use crate::wayland_logic::LinuxSession;

pub fn folder_at(conn: &RustConnection, root: Window, x: i32, y: i32) -> Option<PathBuf> {
    // Unter XWayland kennt der X-Server nur X11-Fenster, nicht den Schreibtisch des Compositors.
    if crate::linux::session() != LinuxSession::X11 {
        return None;
    }
    let desktop = match on_desktop(conn, root, x, y) {
        Ok(desktop) => desktop,
        Err(e) => {
            tracing::debug!("cannot read the window under the cursor: {e:#}");
            false
        }
    };
    let folder = desktop.then(desktop_dir).flatten();
    tracing::debug!(?folder, "drop folder at {x},{y}");
    folder
}

/// Der Schreibtisch-Ordner, sofern es ihn gibt (ohne Eintrag in `user-dirs.dirs`: `~/Desktop`)
pub fn desktop_dir() -> Option<PathBuf> {
    dirs::desktop_dir().or_else(|| dirs::home_dir().map(|home| home.join("Desktop"))).filter(|dir| dir.is_dir())
}

fn on_desktop(conn: &RustConnection, root: Window, x: i32, y: i32) -> anyhow::Result<bool> {
    let atom = |name: &str| -> anyhow::Result<Atom> { Ok(conn.intern_atom(true, name.as_bytes())?.reply()?.atom) };
    let (wm_state, window_type, desktop) =
        (atom("WM_STATE")?, atom("_NET_WM_WINDOW_TYPE")?, atom("_NET_WM_WINDOW_TYPE_DESKTOP")?);
    if desktop == NONE {
        // Kein Programm hat den Typ je benutzt
        return Ok(false);
    }
    let (Ok(x), Ok(y)) = (i16::try_from(x), i16::try_from(y)) else { return Ok(false) };
    // Vom obersten Fenster an dieser Stelle abwärts bis zum Programmfenster (das mit WM_STATE); der
    // Fenstermanager setzt es oft in einen eigenen Rahmen.
    let top = conn.translate_coordinates(root, root, x, y)?.reply()?.child;
    if top == NONE {
        return Ok(false);
    }
    let mut window = top;
    let mut client = None;
    for _ in 0..8 {
        if wm_state != NONE && conn.get_property(false, window, wm_state, AtomEnum::ANY, 0, 0)?.reply()?.type_ != NONE {
            client = Some(window);
            break;
        }
        let child = conn.translate_coordinates(root, window, x, y)?.reply()?.child;
        if child == NONE {
            break;
        }
        window = child;
    }
    // Ohne Fenstermanager (oder ohne WM_STATE) trägt das oberste Fenster den Typ selbst.
    let typed = client.unwrap_or(top);
    let types = conn.get_property(false, typed, window_type, AtomEnum::ATOM, 0, 32)?.reply()?;
    Ok(types.value32().is_some_and(|mut types| types.any(|t| t == desktop)))
}
