//! „Sichere Tastatureingabe“ am Server (macOS): regelmäßig nachsehen, ob ein Programm sie
//! eingeschaltet hat (siehe `qross_platform::secure_input`). Solange sie an ist, erreichen Tasten
//! die Erfassung nicht; der Kern hält den Cursor deshalb auf dem Server.

use std::time::Duration;

use qross_platform::SecureInput;
use tokio::sync::watch;
use tracing::warn;

/// So oft wird nachgesehen. Kurz, denn bis dahin gehen Tasten am Client vorbei.
pub const POLL: Duration = Duration::from_millis(250);

/// Liefert, ob und von wem die sichere Tastatureingabe gerade gehalten wird.
pub type Detector = Box<dyn Fn() -> Option<SecureInput> + Send + Sync>;

/// Startet die Abfrage in einem eigenen Thread. Sie endet, sobald alle Empfänger verworfen sind.
pub fn watch(detect: Detector) -> watch::Receiver<Option<SecureInput>> {
    let (tx, rx) = watch::channel(None);
    let spawned = std::thread::Builder::new().name("qross-secure-input".into()).spawn(move || {
        while !tx.is_closed() {
            let now = detect();
            tx.send_if_modified(|state| {
                let changed = *state != now;
                *state = now;
                changed
            });
            std::thread::sleep(POLL);
        }
    });
    if let Err(e) = spawned {
        warn!("Secure Keyboard Entry: failed to start polling: {e}");
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn changes_and_other_holders_are_reported() {
        let current = Arc::new(Mutex::new(None));
        let detect = current.clone();
        let mut rx = watch(Box::new(move || detect.lock().unwrap().clone()));
        let terminal = SecureInput { pid: Some(412), app: Some("Terminal".into()) };
        *current.lock().unwrap() = Some(terminal.clone());
        tokio::time::timeout(Duration::from_secs(2), rx.changed()).await.unwrap().unwrap();
        assert_eq!(*rx.borrow_and_update(), Some(terminal));
        // Unverändert: keine weitere Meldung
        assert!(tokio::time::timeout(POLL * 3, rx.changed()).await.is_err());
        // Ein anderes Programm hält sie jetzt
        let chrome = SecureInput { pid: Some(977), app: Some("Google Chrome".into()) };
        *current.lock().unwrap() = Some(chrome.clone());
        tokio::time::timeout(Duration::from_secs(2), rx.changed()).await.unwrap().unwrap();
        assert_eq!(*rx.borrow_and_update(), Some(chrome));
        *current.lock().unwrap() = None;
        tokio::time::timeout(Duration::from_secs(2), rx.changed()).await.unwrap().unwrap();
        assert_eq!(*rx.borrow_and_update(), None);
    }
}
