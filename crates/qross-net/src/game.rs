//! Spielemodus: regelmäßig nachsehen, was im Vordergrund läuft. Die Abfrage läuft in einem
//! eigenen Thread, denn unter macOS fragt sie das fokussierte Programm und kann dauern.

use std::time::Duration;

use qross_proto::GameState;
use tokio::sync::watch;
use tracing::warn;

/// So oft wird nachgesehen
pub const POLL: Duration = Duration::from_millis(250);

/// Liefert, was gerade im Vordergrund läuft (siehe `qross_platform::game_state`).
pub type Detector = Box<dyn Fn() -> GameState + Send + Sync>;

/// Startet die Abfrage. Sie endet, sobald alle Empfänger verworfen sind.
pub fn watch(detect: Detector) -> watch::Receiver<GameState> {
    let (tx, rx) = watch::channel(GameState::default());
    let spawned = std::thread::Builder::new().name("qross-game".into()).spawn(move || {
        while !tx.is_closed() {
            let now = detect();
            tx.send_if_modified(|state| std::mem::replace(state, now) != now);
            std::thread::sleep(POLL);
        }
    });
    if let Err(e) = spawned {
        warn!("Spielemodus: Abfrage nicht gestartet: {e}");
    }
    rx
}

/// Fürs Log, z. B. „Vollbildprogramm“
pub fn describe(game: GameState) -> &'static str {
    match (game.fullscreen, game.captured) {
        (true, true) => "Vollbildprogramm mit eingefangenem Zeiger",
        (true, false) => "Vollbildprogramm",
        (false, true) => "Programm mit eingefangenem Zeiger",
        (false, false) => "nichts im Vollbild",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn changes_are_reported_until_nobody_listens() {
        let current = Arc::new(Mutex::new(GameState::default()));
        let detect = current.clone();
        let mut rx = watch(Box::new(move || *detect.lock().unwrap()));
        let fullscreen = GameState { fullscreen: true, captured: false };
        *current.lock().unwrap() = fullscreen;
        tokio::time::timeout(Duration::from_secs(2), rx.changed()).await.unwrap().unwrap();
        assert_eq!(*rx.borrow_and_update(), fullscreen);
        // Unverändert: keine weitere Meldung
        assert!(tokio::time::timeout(POLL * 3, rx.changed()).await.is_err());
        drop(rx);
    }
}
