//! Zeigt den Cursor-Finder dieses Rechners am Cursor, ohne Server und Client:
//! `cargo run -p qross-platform --example finder -- [Name] [ring|name|both] [Wiederholungen]`
//! (Standard: „dieser-rechner“, beides, einmal). Unter macOS braucht der Cursor-Finder die
//! AppKit-Ereignisschleife der Oberfläche; dieses Beispiel zeigt dort nichts.

use std::time::Duration;

use qross_proto::CursorFinder;

fn main() -> anyhow::Result<()> {
    qross_platform::init_process();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().cloned().unwrap_or_else(|| "dieser-rechner".into());
    let style = match args.get(1).map(String::as_str) {
        Some("ring") => CursorFinder { ring: true, name: false },
        Some("name") => CursorFinder { ring: false, name: true },
        _ => CursorFinder { ring: true, name: true },
    };
    let times: u32 = args.get(2).and_then(|n| n.parse().ok()).unwrap_or(1);
    let Some(overlay) = qross_platform::finder_overlay() else {
        anyhow::bail!("Cursor-Finder gibt es hier nicht (Wayland?)");
    };
    for _ in 0..times {
        println!("zeige „{name}“");
        overlay.show(style, &name);
        std::thread::sleep(qross_platform::finder::DURATION + Duration::from_millis(300));
    }
    Ok(())
}
