//! Systemereignisse, nach denen eine Netzwerkverbindung sofort geprüft werden sollte: Ruhezustand,
//! Aufwachen und Netzwechsel. Ein Beobachter je Prozess meldet sie an alle Abonnenten.
//!
//! - Windows: `PowerRegisterSuspendResumeNotification` (Ruhezustand, Aufwachen; als Rückruf, also
//!   ohne Fenster), `NotifyIpInterfaceChange` und `NotifyUnicastIpAddressChange` (Netzwerk)
//! - macOS: `IORegisterForSystemPower` (Ruhezustand, Aufwachen) und SCDynamicStore (Adressen,
//!   Standardroute), beides in einer eigenen Run-Loop – geht also auch in der Kommandozeile, wo
//!   keine App-Ereignisschleife läuft
//! - Linux: logind `PrepareForSleep` über D-Bus, mit Verzögerungssperre, damit vor dem Einschlafen
//!   noch Zeit zum Trennen bleibt; Netlink (Netzwerkkarten, Adressen, Routen)
//!
//! Fehlt etwas davon (kein logind, Container), bleibt es still; Verbindungsabbrüche fallen dann wie
//! bisher erst an der Zeitüberschreitung auf.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::Duration;

#[cfg(windows)]
mod win;
#[cfg(windows)]
use self::win as imp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use self::macos as imp;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use self::linux as imp;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    pub fn start() {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemEvent {
    /// Das System geht gleich in den Ruhezustand. Bleibt aus, wenn es ohne Vorwarnung einschläft
    /// (Akku leer, erzwungen).
    Sleeping,
    /// Das System ist aus dem Ruhezustand aufgewacht. Das Netzwerk ist dann oft noch nicht wieder da.
    Woke,
    /// Netzwerkkarten, Adressen oder Routen haben sich geändert. Kommt in Schüben und auch bei
    /// Änderungen, die keine Verbindung betreffen.
    NetworkChanged,
}

/// So lange warten die Beobachter nach `Sleeping`, bevor sie das Einschlafen freigeben: Zeit genug,
/// um Verbindungen sauber zu trennen.
const BEFORE_SLEEP: Duration = Duration::from_millis(300);

type Subscriber = Arc<dyn Fn(SystemEvent) + Send + Sync>;

static SUBSCRIBERS: Mutex<Vec<(u64, Subscriber)>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
static WATCHING_POWER: AtomicBool = AtomicBool::new(false);
static WATCHING_NETWORK: AtomicBool = AtomicBool::new(false);

/// Meldet Systemereignisse an `on_event`, bis das Abonnement fallen gelassen wird. Der Beobachter
/// startet beim ersten Abonnement und läuft bis zum Prozessende. `on_event` läuft auf einem Thread
/// des Beobachters (bzw. des Systems) und sollte nur weiterreichen, etwa in einen Kanal.
pub fn subscribe(on_event: impl Fn(SystemEvent) + Send + Sync + 'static) -> Subscription {
    static START: Once = Once::new();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    subscribers().push((id, Arc::new(on_event)));
    START.call_once(imp::start);
    Subscription(id)
}

/// Abonnement aus [`subscribe`]
#[must_use = "Das Abonnement endet, sobald es fallen gelassen wird"]
pub struct Subscription(u64);

impl Drop for Subscription {
    fn drop(&mut self) {
        subscribers().retain(|(id, _)| *id != self.0);
    }
}

/// Was der Beobachter mitbekommt: (Ruhezustand und Aufwachen, Netzwechsel). Erst nach dem ersten
/// [`subscribe`] und sobald die Anmeldung beim System geklappt hat.
pub fn watching() -> (bool, bool) {
    (WATCHING_POWER.load(Ordering::Acquire), WATCHING_NETWORK.load(Ordering::Acquire))
}

fn subscribers() -> MutexGuard<'static, Vec<(u64, Subscriber)>> {
    // Ein Abonnent, der in seinem Rückruf abstürzt, soll die anderen nicht mitnehmen.
    SUBSCRIBERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Vom Beobachter: an alle Abonnenten weitergeben
fn dispatch(event: SystemEvent) {
    match event {
        SystemEvent::Sleeping => tracing::info!("System geht in den Ruhezustand"),
        SystemEvent::Woke => tracing::info!("System ist aufgewacht"),
        SystemEvent::NetworkChanged => tracing::trace!("Netzwerk geändert"),
    }
    let current: Vec<Subscriber> = subscribers().iter().map(|(_, s)| s.clone()).collect();
    for subscriber in current {
        subscriber(event);
    }
}

/// Vom Beobachter: Die Anmeldung für Ruhezustand bzw. Netzwerk hat geklappt.
fn ready(power: bool) {
    let flag = if power { &WATCHING_POWER } else { &WATCHING_NETWORK };
    flag.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subscriber_gets_every_event_until_it_unsubscribes() {
        let (a, b) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let first = {
            let a = a.clone();
            subscribe(move |e| a.lock().unwrap().push(e))
        };
        let second = {
            let b = b.clone();
            subscribe(move |e| b.lock().unwrap().push(e))
        };
        // Echte Ereignisse des Systems können dazwischenkommen; gezählt wird nur, was hier passiert.
        dispatch(SystemEvent::Woke);
        drop(first);
        dispatch(SystemEvent::Sleeping);
        assert!(a.lock().unwrap().contains(&SystemEvent::Woke));
        assert!(!a.lock().unwrap().contains(&SystemEvent::Sleeping), "nach dem Abbestellen kommt nichts mehr");
        assert!(b.lock().unwrap().contains(&SystemEvent::Woke));
        assert!(b.lock().unwrap().contains(&SystemEvent::Sleeping));
        drop(second);
    }

    /// Echt beim System angemeldet: Unter Windows liefert die Netzwerk-Anmeldung zur Bestätigung
    /// sofort einen ersten Rückruf; den Ruhezustand kann ein Test nicht auslösen, die Anmeldung
    /// aber prüfen.
    #[cfg(windows)]
    #[test]
    fn windows_accepts_the_registrations() {
        let _subscription = subscribe(|_| {});
        let started = std::time::Instant::now();
        while watching() != (true, true) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(watching(), (true, true), "(Ruhezustand, Netzwerk)");
    }
}
