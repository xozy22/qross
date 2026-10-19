//! Bildschirmschoner abgleichen: regelmäßig nachsehen, ob er auf dem Server läuft. Die Abfrage
//! läuft in einem eigenen Thread, denn unter Linux geht sie über den D-Bus.

use std::time::Duration;

use qross_platform::screensaver::Outcome;
use tokio::sync::watch;
use tracing::{debug, info, warn};

/// So oft wird nachgesehen
pub const POLL: Duration = Duration::from_millis(500);

/// Läuft der Bildschirmschoner? (üblich: `Capture::screen_saver_active`)
pub type Detector = Box<dyn Fn() -> bool + Send + Sync>;

/// Startet die Abfrage. Sie endet, sobald alle Empfänger verworfen sind.
pub fn watch(detect: Detector) -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    let spawned = std::thread::Builder::new().name("qross-screensaver".into()).spawn(move || {
        while !tx.is_closed() {
            let now = detect();
            tx.send_if_modified(|running| std::mem::replace(running, now) != now);
            std::thread::sleep(POLL);
        }
    });
    if let Err(e) = spawned {
        warn!("Bildschirmschoner: Abfrage nicht gestartet: {e}");
    }
    rx
}

/// Client: ins Protokoll, was der Bildschirmschoner des Servers hier bewirkt hat
pub fn report(active: bool, outcome: &Outcome) {
    let what = if active { "gestartet" } else { "beendet" };
    match outcome {
        Outcome::Unchanged => debug!("Bildschirmschoner des Servers {what}; hier: {outcome}"),
        Outcome::Failed(_) => warn!("Bildschirmschoner des Servers {what}; hier: {outcome}"),
        _ => info!("Bildschirmschoner des Servers {what}; hier: {outcome}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn changes_are_reported_until_nobody_listens() {
        let running = Arc::new(AtomicBool::new(false));
        let detect = running.clone();
        let mut rx = watch(Box::new(move || detect.load(Ordering::Relaxed)));
        running.store(true, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(3), rx.changed()).await.unwrap().unwrap();
        assert!(*rx.borrow_and_update());
        // Unverändert: keine weitere Meldung
        assert!(tokio::time::timeout(POLL * 3, rx.changed()).await.is_err());
        running.store(false, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(3), rx.changed()).await.unwrap().unwrap();
        assert!(!*rx.borrow_and_update());
        drop(rx);
    }
}
