//! Betriebssystem-Anbindung: Eingaben abfangen (Server), Eingaben einspeisen (Client),
//! Clipboard und Bildschirmgröße. Windows, macOS und Linux (X11 und Wayland); andere Plattformen
//! liefern einen klaren Fehler.

pub mod clipformat;
pub mod concealed;
pub mod dnd_logic;
pub mod dryrun;
pub mod keymap_linux;
pub mod keymap_mac;
pub mod keymap_windows;
pub mod network;
pub mod wayland_logic;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as imp;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod arboard_clipboard;
#[cfg(target_os = "linux")]
mod clipboard_hint;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
use unsupported as imp;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use qross_core::keys::KeyOptions;
use qross_core::{CaptureEvent, Inject};
use qross_proto::{ClipboardData, GameState, Rect};
use tokio::sync::mpsc::UnboundedSender;

/// Steuerung der Eingabe-Erfassung auf dem Server.
pub trait Capture: Send + Sync {
    /// Ab jetzt alle lokalen Eingaben abfangen und als Deltas/Ereignisse melden.
    fn capture_remote(&self);
    /// Abfangen beenden und den lokalen Cursor auf (x, y) setzen.
    fn release(&self, x: i32, y: i32);
    /// Prüfen, ob an (x, y) gerade Dateien gezogen werden. Muss mit
    /// `CaptureEvent::DragProbeResult` antworten; ohne Drag-&-Drop-Unterstützung passiert nichts.
    fn probe_drag(&self, _x: i32, _y: i32) {}
    /// Den lokalen Drag abbrechen (die Dateien gehen stattdessen an einen Client).
    fn cancel_local_drag(&self) {}
    /// Fang-Taste und Medientasten einstellen (Standard: Rollen, Medientasten an den aktiven Rechner).
    fn set_key_options(&self, _options: KeyOptions) {}
    /// Dateien vom Client sind unterwegs: Das nächste Loslassen einer Maustaste hier als
    /// `CaptureEvent::LocalDrop` melden. Ohne Unterstützung bleiben sie liegen, bis der Cursor wechselt.
    fn await_local_drop(&self) {}
    /// Ordner zum Ablegen an (x, y), z. B. das Explorer- bzw. Finder-Fenster oder der Schreibtisch
    /// darunter; `None`: „Downloads“.
    fn drop_folder(&self, _x: i32, _y: i32) -> Option<PathBuf> {
        None
    }
}

pub trait Injector: Send {
    fn inject(&mut self, event: &Inject);
    /// Kann dieser Rechner Zeichen eingeben, egal welches Tastaturlayout er hat (`Inject::Char`,
    /// `Inject::Shortcut`)? Nur dann meldet der Client das dem Server, und der schickt ihm im
    /// Zeichenmodus Zeichen statt Tastenpositionen. Unter Wayland (libei) geht das nicht.
    fn types_text(&self) -> bool {
        false
    }
    /// Ordner, in dem an (x, y) abgelegte Dateien landen sollen – z. B. das Explorer- bzw.
    /// Finder-Fenster oder der Schreibtisch unter dem Cursor. `None`: keine passende Ablage, die
    /// Laufzeit nimmt „Downloads“.
    fn drop_folder(&mut self, _x: i32, _y: i32) -> Option<PathBuf> {
        None
    }
    /// Zieht der Nutzer hier gerade Dateien (Cursor bei x, y, Taste gedrückt)? Dann wird der lokale
    /// Drag abgebrochen und die Pfade geliefert – die Dateien wandern zum Server. Leer: kein Datei-Drag.
    fn take_dragged_files(&mut self, _x: i32, _y: i32) -> Vec<PathBuf> {
        Vec::new()
    }
}

pub trait Clipboard: Send {
    /// Alle verfügbaren Darstellungen: Text, HTML, Bild (als PNG). `None`, wenn leer.
    fn read(&mut self) -> Option<ClipboardData>;
    /// Ersetzt den Inhalt; alle vorhandenen Darstellungen werden gleichzeitig bereitgestellt.
    fn write(&mut self, data: &ClipboardData) -> anyhow::Result<()>;
    /// Ändert sich bei jeder Änderung des Clipboards (auch durch andere Programme).
    fn sequence(&mut self) -> u64;

    /// Markierung, mit der ein Passwortmanager den aktuellen Inhalt als vertraulich gekennzeichnet
    /// hat (siehe [`concealed`]); `None`, wenn keine vorliegt. Liest nur die Markierungen, nie den
    /// Inhalt selbst.
    fn concealed(&mut self) -> Option<&'static str> {
        None
    }

    /// Kopierte Dateien und Ordner (Explorer, Finder, Dateimanager) als absolute Pfade; leer, wenn keine.
    fn files(&mut self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Legt Dateien zum Einfügen im Dateimanager ab und dazu die Darstellungen aus `data`, soweit
    /// das System beides zugleich erlaubt (Windows: ja; über `arboard` gewinnen die Dateien).
    fn write_files(&mut self, files: &[PathBuf], data: &ClipboardData) -> anyhow::Result<()> {
        let _ = (files, data);
        anyhow::bail!("Dateien in der Zwischenablage werden auf diesem System nicht unterstützt")
    }

    fn text(&mut self) -> Option<String> {
        self.read().and_then(|data| data.text)
    }

    fn set_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.write(&ClipboardData::from_text(text))
    }
}

/// Recht, das macOS für Eingaben verlangt (Systemeinstellungen → Datenschutz & Sicherheit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// „Bedienungshilfen“: Eingaben einspeisen (Client) und abfangen (Server)
    Accessibility,
    /// „Eingabeüberwachung“: Maus und Tastatur mitlesen (Server)
    InputMonitoring,
}

/// Hat Qross dieses Recht? Windows und Linux kennen solche Rechte nicht und liefern immer `true`
/// (die Freigaben der Wayland-Portale stehen in [`portal_state`]).
pub fn has_permission(permission: Permission) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::has_permission(permission)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = permission;
        true
    }
}

/// Bittet das System um das Recht. macOS trägt Qross dafür in die Liste ein und fragt beim ersten
/// Mal selbst nach; anderswo passiert nichts.
pub fn request_permission(permission: Permission) {
    #[cfg(target_os = "macos")]
    mac::request_permission(permission);
    #[cfg(not(target_os = "macos"))]
    let _ = permission;
}

/// Einmal beim Programmstart aufrufen (z. B. DPI-Awareness unter Windows).
pub fn init_process() {
    imp::init_process();
}

/// Datenverzeichnis von Qross (z. B. `--data-dir`). Dort merkt sich Qross unter Wayland die
/// Freigaben der Desktop-Portale; ohne Aufruf gilt `~/.config/Qross`.
pub fn set_data_dir(dir: &Path) {
    #[cfg(target_os = "linux")]
    wayland::set_data_dir(dir);
    #[cfg(not(target_os = "linux"))]
    let _ = dir;
}

/// Grafische Sitzung, über die Qross Eingaben erfasst und einspeist
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Windows,
    MacOs,
    X11,
    /// Wayland: über die Desktop-Portale (xdg-desktop-portal) und libei
    Wayland,
    Other,
}

pub fn desktop() -> Desktop {
    #[cfg(windows)]
    return Desktop::Windows;
    #[cfg(target_os = "macos")]
    return Desktop::MacOs;
    #[cfg(target_os = "linux")]
    return match linux::session() {
        wayland_logic::LinuxSession::X11 => Desktop::X11,
        wayland_logic::LinuxSession::Wayland => Desktop::Wayland,
    };
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    return Desktop::Other;
}

/// Warum das Desktop-Portal unter Wayland keine Sitzung geöffnet hat
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortalProblem {
    /// Das Portal fehlt oder ist zu alt (was genau)
    Missing(String),
    /// Der Nutzer hat die Freigabe abgelehnt
    Denied,
    /// Sonstiger Fehler
    Failed(String),
}

impl std::fmt::Display for PortalProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PortalProblem::Missing(what) => f.write_str(what),
            PortalProblem::Denied => f.write_str("Freigabe abgelehnt"),
            PortalProblem::Failed(error) => f.write_str(error),
        }
    }
}

/// Stand der Desktop-Portale unter Wayland, für Hinweise in der Oberfläche
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortalState {
    /// Version der Portal-Schnittstelle RemoteDesktop (Einspeisen; libei ab 2), `None`: fehlt
    pub remote_desktop: Option<u32>,
    /// Version der Portal-Schnittstelle InputCapture (Erfassen), `None`: fehlt
    pub input_capture: Option<u32>,
    /// Letztes Problem beim Einspeisen bzw. Erfassen; `None`, wenn es geklappt hat oder nie versucht wurde
    pub injector: Option<PortalProblem>,
    pub capture: Option<PortalProblem>,
    /// Eine Freigabe ist gemerkt (Restore-Token), beim nächsten Start fragt das System nicht erneut
    pub remembered_injector: bool,
    pub remembered_capture: bool,
}

/// Unter Wayland der Stand der Portale (fragt beim ersten Mal den Portal-Dienst, bis zu 5 s);
/// sonst `None`.
pub fn portal_state() -> Option<PortalState> {
    #[cfg(target_os = "linux")]
    if desktop() == Desktop::Wayland {
        return Some(wayland::state());
    }
    None
}

/// Gemerkte Freigaben der Portale vergessen: Beim nächsten Start fragt das System wieder.
pub fn forget_portal_permissions() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    return wayland::forget_tokens();
    #[cfg(not(target_os = "linux"))]
    Ok(())
}

/// Bounding-Box aller Monitore in physischen Pixeln.
pub fn screen_bounds() -> Rect {
    imp::screen_bounds()
}

/// Die einzelnen Monitore in denselben Koordinaten wie `screen_bounds()`. Leer, wenn das System sie
/// nicht nennt; dann gilt das umschließende Rechteck als ein einziger Monitor.
pub fn monitors() -> Vec<Rect> {
    let mut monitors = Vec::new();
    for m in imp::monitors() {
        // Gespiegelte Bildschirme melden dieselbe Fläche mehrfach.
        if m.width > 0 && m.height > 0 && !monitors.contains(&m) {
            monitors.push(m);
        }
    }
    monitors
}

/// Spielemodus: Läuft im Vordergrund ein Vollbildprogramm, bzw. hat ein Programm den Zeiger
/// eingefangen? Wird regelmäßig abgefragt und darf dafür ein paar Millisekunden brauchen.
/// - Windows: Auskunft der Shell (`SHQueryUserNotificationState`) oder Vordergrundfenster ohne
///   Titelleiste, das seinen Monitor bedeckt; eingefangen per `GetClipCursor`
/// - macOS: das fokussierte Fenster ist im Vollbild (`AXFullScreen`) oder bedeckt einen Bildschirm
///   samt Menüleiste; Einfangen lässt sich nicht abfragen
/// - X11: das aktive Fenster hat `_NET_WM_STATE_FULLSCREEN`; Einfangen lässt sich nicht abfragen,
///   ohne selbst nach dem Zeiger zu greifen
pub fn game_state() -> GameState {
    imp::game_state()
}

/// Startet die Erfassung. Ereignisse landen in `events`.
pub fn start_capture(events: UnboundedSender<CaptureEvent>) -> anyhow::Result<Arc<dyn Capture>> {
    imp::start_capture(events)
}

pub fn injector() -> anyhow::Result<Box<dyn Injector>> {
    imp::injector()
}

/// Nur für Tests: Wayland-Einspeisung über eine schon geöffnete libei-Verbindung, z. B. von
/// `org.gnome.Mutter.RemoteDesktop.Session.ConnectToEIS` eines mutter ohne Portal.
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub fn injector_from_ei_fd(fd: std::os::fd::OwnedFd) -> anyhow::Result<Box<dyn Injector>> {
    wayland::injector_from_fd(fd)
}

pub fn clipboard() -> Box<dyn Clipboard> {
    imp::clipboard()
}

/// Ablageordner an einer Bildschirmposition (Diagnose; derselbe Weg wie beim Drop).
pub fn folder_at(x: i32, y: i32) -> Option<PathBuf> {
    injector().ok()?.drop_folder(x, y)
}

/// Diagnose: wie [`folder_at`], dazu jeder Schritt der Suche als Text (Fensterliste, Elemente des
/// Finder-Fensters). Nur am Mac.
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub fn trace_folder_at(x: i32, y: i32) -> (Option<PathBuf>, Vec<String>) {
    mac::trace_folder_at(x, y)
}

/// Nur für Tests: die Drag-Erkennung des Mac-Servers, ohne Event-Tap (der braucht Rechte).
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use mac::DragWatch;

/// Diagnose: aktueller Ordner eines Explorer-Fensters (Windows-Fensterhandle).
#[cfg(windows)]
pub fn explorer_folder(hwnd: isize) -> Option<PathBuf> {
    win::folder_of_window(hwnd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitors_span_the_screen_bounds() {
        init_process();
        let monitors = monitors();
        if monitors.is_empty() {
            eprintln!("System nennt keine Monitore – übersprungen");
            return;
        }
        let left = monitors.iter().map(|m| m.x).min().unwrap();
        let top = monitors.iter().map(|m| m.y).min().unwrap();
        let right = monitors.iter().map(|m| m.right()).max().unwrap();
        let bottom = monitors.iter().map(|m| m.bottom()).max().unwrap();
        let hull = Rect::new(left, top, (right - left + 1) as u32, (bottom - top + 1) as u32);
        assert_eq!(hull, screen_bounds(), "Monitore: {monitors:?}");
    }
}
