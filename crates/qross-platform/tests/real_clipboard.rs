//! Echte Zwischenablage unter Linux und macOS (über `arboard`). Läuft nur in CI (`CI=true`) oder mit
//! `QROSS_CLIPBOARD_TEST=1`, weil der Test den Inhalt der Zwischenablage ersetzt. Unter Linux braucht er
//! einen X-Server (`xvfb-run`). Windows prüft `cargo run -p qross-platform --example clipboard_check`.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::path::PathBuf;

use qross_platform::clipformat::{png_to_rgba, rgba_to_png, Rgba};
use qross_proto::ClipboardData;

fn enabled() -> bool {
    let on = std::env::var_os("CI").is_some_and(|v| v == "true") || std::env::var_os("QROSS_CLIPBOARD_TEST").is_some();
    if !on {
        eprintln!("echte Zwischenablage nur in CI oder mit QROSS_CLIPBOARD_TEST=1 – übersprungen");
    }
    on && (cfg!(target_os = "macos") || std::env::var_os("DISPLAY").is_some())
}

/// macOS liefert Pfade aufgelöst zurück (`/var` → `/private/var`).
fn canonical(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())).collect()
}

#[test]
fn real_clipboard_roundtrips_formats_and_files() {
    if !enabled() {
        return;
    }
    let mut clipboard = qross_platform::clipboard();

    let before = clipboard.sequence();
    let html = ClipboardData { text: Some("Grüße".into()), html: Some("<b>Grüße</b>".into()), png: None };
    clipboard.write(&html).unwrap();
    assert_ne!(clipboard.sequence(), before, "Sequenz ändert sich mit dem Inhalt");
    let read = clipboard.read().expect("Text und HTML");
    assert_eq!(read.text.as_deref(), Some("Grüße"));
    assert!(read.html.as_deref().is_some_and(|h| h.contains("<b>Grüße</b>")), "{read:?}");

    let image = Rgba { width: 8, height: 4, pixels: (0..32u32).flat_map(|i| [i as u8 * 8, 255 - i as u8, 64, 255]).collect() };
    clipboard.write(&ClipboardData { png: Some(rgba_to_png(&image).unwrap()), ..Default::default() }).unwrap();
    let png = clipboard.read().and_then(|d| d.png).expect("Bild");
    assert_eq!(png_to_rgba(&png).unwrap(), image);
    assert!(clipboard.files().is_empty(), "ein Bild ist keine Dateiliste");

    let dir = std::env::temp_dir().join(format!("qross-real-clipboard-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Ordner Ä")).unwrap();
    std::fs::write(dir.join("Bericht Grüße.txt"), "Qross").unwrap();
    let files = vec![dir.join("Bericht Grüße.txt"), dir.join("Ordner Ä")];
    let seq = clipboard.sequence();
    clipboard.write_files(&files, &ClipboardData::default()).unwrap();
    assert_eq!(canonical(&clipboard.files()), canonical(&files));
    assert_ne!(clipboard.sequence(), seq, "Sequenz ändert sich mit den Dateien");
    let _ = std::fs::remove_dir_all(&dir);
}
