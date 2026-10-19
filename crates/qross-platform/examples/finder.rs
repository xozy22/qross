//! Zeigt den Cursor-Finder dieses Rechners am Cursor, ohne Server und Client:
//! `cargo run -p qross-platform --example finder -- [Name] [ring|name|both] [Wiederholungen]`
//! (Standard: „dieser-rechner“, beides, einmal). Die Drag-Vorschau zeigt
//! `cargo run -p qross-platform --example finder -- drag [Datei …]` fünf Sekunden lang, so als würden
//! diese Dateien gerade hierher gezogen (ein `/` am Ende steht für einen Ordner). Unter macOS braucht
//! die Anzeige die AppKit-Ereignisschleife der Oberfläche; dieses Beispiel zeigt dort nichts.

use std::time::Duration;

use qross_core::finder::DragPreview;
use qross_proto::{CursorFinder, FileEntry};

fn main() -> anyhow::Result<()> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info,qross_platform=debug".into());
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new(filter)).init();
    qross_platform::init_process();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(overlay) = qross_platform::finder_overlay() else {
        anyhow::bail!("Cursor-Finder gibt es hier nicht (Wayland?)");
    };
    let game = qross_platform::game_state();
    if game.is_active() {
        println!("Im Vordergrund läuft etwas im Vollbild ({game:?}) – die Anzeige bleibt dann aus");
    }
    if args.first().map(String::as_str) == Some("drag") {
        let names = if args.len() > 1 { args[1..].to_vec() } else { vec!["Bericht.pdf".to_owned()] };
        let entries: Vec<FileEntry> = names
            .iter()
            .map(|name| FileEntry { path: name.trim_end_matches('/').to_owned(), size: 3_141_592, is_dir: name.ends_with('/') })
            .collect();
        let preview = DragPreview::of(&entries).expect("mindestens ein Name");
        println!("zeige die Drag-Vorschau „{}“ ({}) – Maus bewegen, sie folgt dem Cursor", preview.title, preview.size);
        overlay.show_drag(&preview);
        std::thread::sleep(Duration::from_secs(5));
        overlay.hide_drag();
        std::thread::sleep(Duration::from_millis(100));
        return Ok(());
    }
    let name = args.first().cloned().unwrap_or_else(|| "dieser-rechner".into());
    let style = match args.get(1).map(String::as_str) {
        Some("ring") => CursorFinder { ring: true, name: false },
        Some("name") => CursorFinder { ring: false, name: true },
        _ => CursorFinder { ring: true, name: true },
    };
    let times: u32 = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(1);
    for _ in 0..times {
        println!("zeige „{name}“");
        overlay.show(style, &name);
        std::thread::sleep(qross_platform::finder::DURATION + Duration::from_millis(300));
    }
    Ok(())
}
