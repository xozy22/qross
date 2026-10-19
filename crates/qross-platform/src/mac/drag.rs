//! Gezogene Dateien am Mac erkennen, am Server (Dateien an der Kante zum Client) wie am Client
//! (Drag & Drop zum Server). Während eines Drags liegen die Daten in der systemweiten
//! Drag-Zwischenablage. Hat sie sich seit dem Drücken der Maustaste geändert, läuft ein neuer Drag –
//! die Datei-URLs darin sind die gezogenen Dateien.

use std::path::PathBuf;

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL};
use objc2_foundation::NSURL;

use crate::dnd_logic::DragTracker;

fn drag_pasteboard() -> objc2::rc::Retained<NSPasteboard> {
    // SAFETY: Konstante von AppKit
    NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag })
}

/// Ändert sich mit jedem neuen Drag
pub fn change_count() -> isize {
    // Eigener Pool: Aufgerufen wird aus Threads ohne Run-Loop von AppKit.
    autoreleasepool(|_| drag_pasteboard().changeCount())
}

/// Dateien des laufenden (bzw. letzten) Drags
pub fn dragged_files() -> Vec<PathBuf> {
    autoreleasepool(|_| {
        let Some(items) = drag_pasteboard().pasteboardItems() else { return Vec::new() };
        items
            .iter()
            .filter_map(|item| {
                // SAFETY: Konstante von AppKit
                let url = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
                let path = NSURL::URLWithString(&url)?.path()?;
                Some(PathBuf::from(path.to_string()))
            })
            .collect()
    })
}

/// Erkennt einen Drag von Dateien, der nach dem Drücken der linken Maustaste begonnen hat:
/// `press` beim Drücken, `release` beim Loslassen, `probe` an der Kante. Benutzt von der Erfassung
/// des Servers und vom Einspeisen des Clients; öffentlich (versteckt) nur für Tests.
#[derive(Debug, Default)]
pub struct DragWatch(DragTracker);

impl DragWatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn press(&mut self) {
        self.0.press(change_count() as i64);
    }

    pub fn release(&mut self) {
        self.0.release();
    }

    /// Die gezogenen Dateien, wenn seit dem Drücken ein Drag mit Dateien begonnen hat. Jeder Drag
    /// liefert sie nur einmal; leer auch bei gezogenem Text oder ohne gedrückte Taste.
    pub fn probe(&mut self) -> Vec<PathBuf> {
        if !self.0.is_pressed() || !self.0.take(change_count() as i64) {
            return Vec::new();
        }
        dragged_files()
    }
}
