//! Betriebssystem-Anbindung: Eingaben abfangen (Server), Eingaben einspeisen (Client),
//! Clipboard und Bildschirmgröße. Aktuell ist Windows implementiert; andere Plattformen
//! liefern einen klaren Fehler, bis sie nachgezogen werden.

pub mod clipformat;
pub mod dryrun;
pub mod keymap_linux;
pub mod keymap_mac;
pub mod keymap_windows;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as imp;

#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
use x11 as imp;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as imp;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod arboard_clipboard;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
use unsupported as imp;

use std::path::PathBuf;
use std::sync::Arc;

use qross_core::{CaptureEvent, Inject};
use qross_proto::{ClipboardData, Rect};
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
}

pub trait Injector: Send {
    fn inject(&mut self, event: &Inject);
    /// Ordner, in dem an (x, y) abgelegte Dateien landen sollen – z. B. das Explorer-Fenster oder
    /// der Desktop unter dem Cursor. `None`: keine passende Ablage, die Laufzeit nimmt „Downloads“.
    fn drop_folder(&mut self, _x: i32, _y: i32) -> Option<PathBuf> {
        None
    }
}

pub trait Clipboard: Send {
    /// Alle verfügbaren Darstellungen: Text, HTML, Bild (als PNG). `None`, wenn leer.
    fn read(&mut self) -> Option<ClipboardData>;
    /// Ersetzt den Inhalt; alle vorhandenen Darstellungen werden gleichzeitig bereitgestellt.
    fn write(&mut self, data: &ClipboardData) -> anyhow::Result<()>;
    /// Ändert sich bei jeder Änderung des Clipboards (auch durch andere Programme).
    fn sequence(&mut self) -> u64;

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

/// Einmal beim Programmstart aufrufen (z. B. DPI-Awareness unter Windows).
pub fn init_process() {
    imp::init_process();
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

/// Startet die Erfassung. Ereignisse landen in `events`.
pub fn start_capture(events: UnboundedSender<CaptureEvent>) -> anyhow::Result<Arc<dyn Capture>> {
    imp::start_capture(events)
}

pub fn injector() -> anyhow::Result<Box<dyn Injector>> {
    imp::injector()
}

pub fn clipboard() -> Box<dyn Clipboard> {
    imp::clipboard()
}

/// Ablageordner an einer Bildschirmposition (Diagnose; derselbe Weg wie beim Drop).
pub fn folder_at(x: i32, y: i32) -> Option<PathBuf> {
    injector().ok()?.drop_folder(x, y)
}

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
