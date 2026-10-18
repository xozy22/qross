//! Zeigt den Cursor-Finder dieses Rechners am Cursor, ohne Server und Client:
//! `cargo run -p qross-platform --example finder -- [Name] [ring|name|both] [Wiederholungen]`
//! (Standard: „dieser-rechner“, beides, einmal). Unter macOS braucht der Cursor-Finder die
//! AppKit-Ereignisschleife der Oberfläche; dieses Beispiel zeigt dort nichts.

use std::time::Duration;

use qross_proto::CursorFinder;

fn main() -> anyhow::Result<()> {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info,qross_platform=debug".into());
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new(filter)).init();
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
    let game = qross_platform::game_state();
    if game.is_active() {
        println!("Im Vordergrund läuft etwas im Vollbild ({game:?}) – der Cursor-Finder bleibt dann aus");
    }
    for _ in 0..times {
        println!("zeige „{name}“");
        overlay.show(style, &name);
        std::thread::sleep(qross_platform::finder::DURATION + Duration::from_millis(300));
    }
    Ok(())
}
