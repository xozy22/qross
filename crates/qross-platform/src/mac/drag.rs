//! Gezogene Dateien am Mac erkennen (Drag & Drop zum Server). Während eines Drags liegen die
//! Daten in der systemweiten Drag-Zwischenablage. Hat sie sich seit dem Drücken der Maustaste
//! geändert, läuft ein neuer Drag – die Datei-URLs darin sind die gezogenen Dateien.

use std::path::PathBuf;

use objc2_app_kit::{NSPasteboard, NSPasteboardNameDrag, NSPasteboardTypeFileURL};
use objc2_foundation::NSURL;

fn drag_pasteboard() -> objc2::rc::Retained<NSPasteboard> {
    // SAFETY: Konstante von AppKit
    NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag })
}

/// Ändert sich mit jedem neuen Drag
pub fn change_count() -> isize {
    drag_pasteboard().changeCount()
}

/// Dateien des laufenden (bzw. letzten) Drags
pub fn dragged_files() -> Vec<PathBuf> {
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
}
