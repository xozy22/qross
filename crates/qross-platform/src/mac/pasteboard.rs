//! Markierungen der Passwortmanager auf der allgemeinen Zwischenablage (siehe `crate::concealed`).
//! `arboard` liest nur Text, HTML, Bilder und Dateien; die Typen fragt Qross selbst ab.

use objc2_app_kit::NSPasteboard;

use crate::concealed;

/// Typen aller Einträge der allgemeinen Zwischenablage. `types` nennt die des Pasteboards,
/// manche Programme markieren aber nur einzelne Einträge.
fn general_types() -> Vec<String> {
    let pasteboard = NSPasteboard::generalPasteboard();
    let mut types: Vec<String> = pasteboard.types().map(|t| t.iter().map(|t| t.to_string()).collect()).unwrap_or_default();
    if let Some(items) = pasteboard.pasteboardItems() {
        for item in items.iter() {
            types.extend(item.types().iter().map(|t| t.to_string()));
        }
    }
    types
}

pub fn concealed() -> Option<&'static str> {
    concealed::find_in_types(concealed::MACOS, &general_types())
}
