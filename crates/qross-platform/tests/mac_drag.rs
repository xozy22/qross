//! Drag & Drop am Mac gegen das echte System (läuft auf dem macOS-Runner der CI).
//!
//! - Drag-Erkennung des Servers: Der Test legt Datei-URLs in die Drag-Zwischenablage, so wie es der
//!   Finder zu Beginn eines Drags tut. Den Event-Tap braucht das nicht.
//! - Ordner eines Finder-Fensters: braucht das Recht „Bedienungshilfen“ und öffnet ein Fenster; nur
//!   in CI oder mit `QROSS_FINDER_TEST=1`, und ohne das Recht übersprungen.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::CGRect;
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerName,
};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardNameDrag, NSPasteboardTypeFileURL, NSPasteboardTypeString,
    NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSString, NSURL};
use qross_platform::DragWatch;

fn drag_pasteboard() -> Retained<NSPasteboard> {
    NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag })
}

/// Wie der Finder zu Beginn eines Drags: je Datei ein Eintrag mit ihrer URL
fn drag_files(files: &[PathBuf]) {
    let pasteboard = drag_pasteboard();
    pasteboard.clearContents();
    let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = files
        .iter()
        .map(|file| {
            let item = NSPasteboardItem::new();
            let url = NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy())).absoluteString().unwrap();
            assert!(item.setString_forType(&url, unsafe { NSPasteboardTypeFileURL }));
            ProtocolObject::from_retained(item)
        })
        .collect();
    assert!(pasteboard.writeObjects(&NSArray::from_retained_slice(&items)));
}

fn drag_text(text: &str) {
    let pasteboard = drag_pasteboard();
    pasteboard.clearContents();
    assert!(pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString }));
}

/// macOS liefert Pfade aufgelöst zurück (`/var` → `/private/var`).
fn canonical(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())).collect()
}

#[test]
fn files_dragged_on_the_server_are_recognized() {
    let dir = std::env::temp_dir().join(format!("qross-mac-drag-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Ordner Ä")).unwrap();
    std::fs::write(dir.join("Bericht Grüße.pdf"), "Qross").unwrap();
    let files = vec![dir.join("Bericht Grüße.pdf"), dir.join("Ordner Ä")];

    let mut watch = DragWatch::new();
    drag_files(&files);
    assert!(watch.probe().is_empty(), "ohne gedrückte Taste");
    watch.press();
    assert!(watch.probe().is_empty(), "die Dateien liegen noch vom letzten Drag");
    drag_files(&files);
    assert_eq!(canonical(&watch.probe()), canonical(&files), "Drag seit dem Drücken");
    assert!(watch.probe().is_empty(), "derselbe Drag nur einmal");

    watch.release();
    watch.press();
    drag_text("Grüße");
    assert!(watch.probe().is_empty(), "gezogener Text");
    watch.release();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Das vorderste gewöhnliche Fenster des Finders (Ebene 0) samt Lage
fn front_finder_window() -> Option<CGRect> {
    let windows = copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID)?;
    windows.get_all_values().into_iter().find_map(|raw| {
        let info: CFDictionary = unsafe { CFDictionary::wrap_under_get_rule(raw as CFDictionaryRef) };
        let value = |key: CFStringRef| {
            info.find(key as *const std::ffi::c_void).map(|v| unsafe { CFType::wrap_under_get_rule(*v as CFTypeRef) })
        };
        let owner = value(unsafe { kCGWindowOwnerName })?.downcast::<CFString>()?.to_string();
        let layer = value(unsafe { kCGWindowLayer })?.downcast::<CFNumber>()?.to_i32()?;
        let bounds = value(unsafe { kCGWindowBounds })?.downcast::<CFDictionary>()?;
        (owner == "Finder" && layer == 0).then(|| CGRect::from_dict_representation(&bounds)).flatten()
    })
}

#[test]
fn a_finder_window_under_the_cursor_gives_its_folder() {
    if std::env::var_os("CI").is_none() && std::env::var_os("QROSS_FINDER_TEST").is_none() {
        eprintln!("öffnet ein Finder-Fenster – nur in CI oder mit QROSS_FINDER_TEST=1");
        return;
    }
    if !qross_platform::has_permission(qross_platform::Permission::Accessibility) {
        eprintln!("Recht „Bedienungshilfen“ fehlt – Ordner eines Finder-Fensters nicht prüfbar, übersprungen");
        return;
    }
    let dir = std::env::temp_dir().join(format!("qross-finder-{}", std::process::id())).join("Ablage Ä");
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(dir.join(name), "Qross").unwrap();
    }
    let dir = std::fs::canonicalize(&dir).unwrap();
    assert!(std::process::Command::new("open").arg(&dir).status().unwrap().success());

    // Der Finder braucht einen Moment für Fenster und Inhalt.
    let start = Instant::now();
    let mut report = Vec::new();
    while start.elapsed() < Duration::from_secs(15) {
        report = match front_finder_window() {
            Some(frame) => {
                let (x, y) = (frame.origin.x + frame.size.width / 2.0, frame.origin.y + frame.size.height / 2.0);
                let (folder, trace) = qross_platform::trace_folder_at(x as i32, y as i32);
                if folder.as_deref().and_then(|p| std::fs::canonicalize(p).ok()).as_deref() == Some(Path::new(&dir)) {
                    let _ = std::fs::remove_dir_all(dir.parent().unwrap());
                    return;
                }
                let size = frame.size;
                [format!("Finder-Fenster {}×{} bei {},{}, Mitte {x},{y}", size.width, size.height, frame.origin.x, frame.origin.y)]
                    .into_iter()
                    .chain(trace)
                    .collect()
            }
            None => vec!["kein gewöhnliches Fenster des Finders in der Fensterliste".to_owned()],
        };
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("Ordner des Finder-Fensters nicht erkannt, erwartet {}:
{}", dir.display(), report.join("
"));
}
