//! Restore-Token der Portale: Damit fragt das System beim nächsten Start nicht erneut nach der
//! Freigabe. Jedes Token gilt nur einmal; nach jedem Start liefert das Portal ein neues.
//! Abgelegt im Datenverzeichnis von Qross, nur für den Nutzer lesbar.

use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    RemoteDesktop,
    InputCapture,
}

impl Kind {
    fn file(self) -> &'static str {
        match self {
            Kind::RemoteDesktop => "wayland-remote-desktop.token",
            Kind::InputCapture => "wayland-input-capture.token",
        }
    }
}

static DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

pub(crate) fn set_data_dir(dir: &Path) {
    *DATA_DIR.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.to_owned());
}

/// Wie die Kommandozeile und die Oberfläche: `~/.config/Qross`, sofern nichts anderes gesetzt ist
fn dir() -> Option<PathBuf> {
    let set = DATA_DIR.lock().unwrap_or_else(|e| e.into_inner()).clone();
    set.or_else(|| dirs::config_dir().map(|d| d.join("Qross")))
}

fn path(kind: Kind) -> Option<PathBuf> {
    dir().map(|d| d.join(kind.file()))
}

pub(crate) fn load(kind: Kind) -> Option<String> {
    let text = fs::read_to_string(path(kind)?).ok()?;
    let token = text.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

/// Neues Token merken; `None` löscht ein altes (das Portal merkt sich diese Freigabe nicht).
pub(crate) fn save(kind: Kind, token: Option<&str>) {
    let Some(path) = path(kind) else { return };
    let result = match token {
        Some(token) => write(&path, token),
        None => match fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    };
    if let Err(e) = result {
        tracing::warn!("failed to save the portal permission ({}): {e}", path.display());
    }
}

fn write(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    file.write_all(token.as_bytes())
}

pub(crate) fn exists(kind: Kind) -> bool {
    load(kind).is_some()
}

/// Gemerkte Freigaben vergessen: Beim nächsten Start fragt das System wieder.
pub(crate) fn forget() -> anyhow::Result<()> {
    for kind in [Kind::RemoteDesktop, Kind::InputCapture] {
        if let Some(path) = path(kind) {
            match fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(anyhow::anyhow!("failed to delete {}: {e}", path.display()))
                }
                _ => {}
            }
        }
    }
    Ok(())
}
