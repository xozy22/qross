//! Aufwachen aus dem Ruhezustand und Netzwechsel: Systemereignisse für Client und Server, Wartezeit
//! bis zum nächsten Verbindungsversuch, Lebendprüfung einer Verbindung.
//!
//! Ohne das fiele eine Verbindung, die im Ruhezustand gestorben ist, erst nach der
//! Zeitüberschreitung auf (siehe `tls`): unter macOS und Linux steht die Uhr im Ruhezustand still,
//! nach dem Aufwachen bleiben also noch bis zu 5 s. Mit den Ereignissen prüft der Client sofort, zieht
//! bei einer neuen Adresse per QUIC-Migration um und baut sonst ohne Wartezeit neu auf; der Server
//! trennt vor seinem Ruhezustand sauber und wirft nach dem Aufwachen stumme Clients hinaus.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use quinn::{Connection, Endpoint, VarInt};
use tokio::sync::mpsc;
use tracing::debug;

pub use qross_platform::sysevents::SystemEvent;

/// Fehlercode beim Schließen: Der Rechner geht in den Ruhezustand. Ältere Gegenstellen werten
/// Fehlercodes nicht aus; für sie ist es eine gewöhnliche Trennung.
pub const CLOSE_SLEEPING: VarInt = VarInt::from_u32(1);
/// Fehlercode beim Schließen: Derselbe Client (Name und Schlüssel) hat sich neu angemeldet.
pub const CLOSE_REPLACED: VarInt = VarInt::from_u32(2);

/// So lange darf ein Verbindungsaufbau dauern. Bis dahin wiederholt quinn das erste Paket nach
/// 0,3 s und 0,9 s (siehe `initial_rtt` in `tls`); danach beginnt lieber ein neuer Versuch mit
/// frischem Socket, statt auf die nächste, immer spätere Wiederholung zu warten.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Lebendprüfung nach dem Aufwachen oder einem Netzwechsel: Kommt so lange nichts vom Server, gilt
/// die Verbindung als tot. Er antwortet sonst auf das PING nach dem Umzug innerhalb einer Laufzeit.
pub const CLIENT_PROBE: Duration = Duration::from_millis(1500);
/// Dasselbe beim Server. Er stößt nichts an, sondern wartet auf die Keep-alives: Beide Seiten senden
/// spätestens nach 1 s Ruhe etwas, die Gegenseite bestätigt es sofort.
pub const SERVER_PROBE: Duration = Duration::from_secs(2);

/// Nach `Sleeping` höchstens so lange warten, bevor der Client wieder versucht, sich zu verbinden –
/// falls der Rechner doch nicht einschläft oder das Aufwachen nicht gemeldet wird.
pub const SLEEP_PAUSE: Duration = Duration::from_secs(30);

/// Abstand der Abfragen in [`answers`]
const ANSWER_POLL: Duration = Duration::from_millis(20);

/// Wartezeit vor dem nächsten Verbindungsversuch: schnell beginnen (ein Server-Neustart dauert oft
/// nur Sekundenbruchteile), dann verdoppeln. In der ersten Minute höchstens 1 s, danach bis 3 s, damit
/// ein Laptop fern vom Server nicht pausenlos sendet. Nach einer stabilen Sitzung, dem Aufwachen und
/// einem Netzwechsel ([`Backoff::reset`]) beginnt es von vorn.
#[derive(Debug)]
pub(crate) struct Backoff {
    next: Duration,
    /// Seit wann es nicht klappt
    failing_since: Option<Instant>,
}

impl Backoff {
    pub const MIN: Duration = Duration::from_millis(250);
    /// Obergrenze in der ersten Minute
    pub const FAST_MAX: Duration = Duration::from_secs(1);
    /// Obergrenze danach
    pub const SLOW_MAX: Duration = Duration::from_secs(3);
    const FAST_FOR: Duration = Duration::from_secs(60);
    /// Eine Sitzung, die so lange lief, gilt als erfolgreich.
    pub const STABLE: Duration = Duration::from_secs(10);

    pub fn new() -> Self {
        Self { next: Self::MIN, failing_since: None }
    }

    /// Wartezeit nach einer Sitzung (bzw. einem Versuch), die `lasted` dauerte und `now` endete
    pub fn after(&mut self, lasted: Duration, now: Instant) -> Duration {
        if lasted >= Self::STABLE {
            self.reset();
        }
        let since = *self.failing_since.get_or_insert(now);
        let max = if now.duration_since(since) < Self::FAST_FOR { Self::FAST_MAX } else { Self::SLOW_MAX };
        let wait = self.next.min(max);
        self.next = (wait * 2).min(max);
        wait
    }

    /// Wieder von vorn: Aufgewacht, Netz gewechselt oder eine Sitzung lief stabil.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// Neues UDP-Socket für `endpoint`, passend zur Adressfamilie von `peer`. Hat der Rechner eine neue
/// Adresse, zieht die Verbindung damit um (QUIC-Migration: der Server prüft den neuen Weg und macht
/// weiter); sonst ändert sich nur der Port. quinn schickt dabei sofort ein PING – kennt der Server
/// die Verbindung nicht mehr, antwortet er mit einem Stateless Reset, und sie endet gleich.
pub(crate) fn rebind(endpoint: &Endpoint, peer: SocketAddr) -> std::io::Result<()> {
    endpoint.rebind(std::net::UdpSocket::bind(unspecified(peer))?)
}

/// Lokale Adresse „beliebig“ in der Familie von `peer`
pub(crate) fn unspecified(peer: SocketAddr) -> SocketAddr {
    match peer.ip() {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    }
}

/// Kommt innerhalb von `within` irgendetwas von der Gegenseite an (Antwort auf ein PING,
/// Keep-alive, Eingaben)? `false` auch, wenn die Verbindung inzwischen geschlossen ist.
pub(crate) async fn answers(conn: Connection, within: Duration) -> bool {
    let before = conn.stats().udp_rx.datagrams;
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if conn.stats().udp_rx.datagrams > before {
            return true;
        }
        if conn.close_reason().is_some() || tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(ANSWER_POLL).await;
    }
}

/// Eigene Adressen und ob die Karte läuft, sortiert. Ändert sich das, hat das Netz gewechselt.
type Addresses = Vec<(String, IpAddr, bool)>;

fn local_addresses() -> Addresses {
    let mut addresses: Addresses = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .map(|i| {
            let up = i.oper_status == if_addrs::IfOperStatus::Up;
            (i.name.clone(), i.ip(), up)
        })
        .collect();
    addresses.sort();
    addresses
}

/// Systemereignisse des Betriebssystems für eine Verbindung. Ruhezustand und Aufwachen kommen sofort,
/// Netzwerkmeldungen nur, wenn sich die eigenen Adressen tatsächlich geändert haben (das System
/// meldet auch vieles, was keine Verbindung betrifft). Endet mit dem Empfänger.
pub(crate) fn system_events() -> mpsc::UnboundedReceiver<SystemEvent> {
    let (raw_tx, raw) = mpsc::unbounded_channel();
    let subscription = qross_platform::sysevents::subscribe(move |event| {
        let _ = raw_tx.send(event);
    });
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _subscription = subscription;
        let addresses = || async { tokio::task::spawn_blocking(local_addresses).await.unwrap_or_default() };
        tokio::select! {
            _ = filter(raw, &tx, addresses) => {}
            _ = tx.closed() => {}
        }
    });
    rx
}

/// Gibt Ruhezustand und Aufwachen weiter, Netzwerkmeldungen nur bei geänderten Adressen. Schübe
/// werden zusammengefasst: Was schon wartet, zählt mit zur selben Prüfung.
async fn filter<F, Fut>(mut raw: mpsc::UnboundedReceiver<SystemEvent>, tx: &mpsc::UnboundedSender<SystemEvent>, addresses: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = Addresses>,
{
    let mut known = addresses().await;
    while let Some(event) = raw.recv().await {
        let mut network = event == SystemEvent::NetworkChanged;
        if !network {
            let _ = tx.send(event);
        }
        while let Ok(event) = raw.try_recv() {
            if event == SystemEvent::NetworkChanged {
                network = true;
            } else {
                let _ = tx.send(event);
            }
        }
        if network {
            let now = addresses().await;
            if now != known {
                debug!("local addresses changed: {now:?}");
                known = now;
                let _ = tx.send(SystemEvent::NetworkChanged);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn backoff_starts_fast_doubles_and_stays_short_in_the_first_minute() {
        let start = Instant::now();
        let mut backoff = Backoff::new();
        let short = Duration::from_millis(50);
        let waits: Vec<u128> = (0..5u64)
            .map(|i| backoff.after(short, start + Duration::from_secs(i)).as_millis())
            .collect();
        assert_eq!(waits, [250, 500, 1000, 1000, 1000]);
        // Nach einer Minute ohne Erfolg seltener, höchstens 3 s
        let later: Vec<u128> = (0..4u64)
            .map(|i| backoff.after(short, start + Duration::from_secs(61 + i)).as_millis())
            .collect();
        assert_eq!(later, [1000, 2000, 3000, 3000]);
    }

    #[test]
    fn backoff_starts_over_after_a_stable_session_or_an_event() {
        let start = Instant::now();
        let mut backoff = Backoff::new();
        for i in 0..80 {
            backoff.after(Duration::ZERO, start + Duration::from_secs(i));
        }
        assert_eq!(backoff.after(Duration::ZERO, start + Duration::from_secs(80)), Backoff::SLOW_MAX);
        // Eine Sitzung lief stabil: wieder schnell
        assert_eq!(backoff.after(Backoff::STABLE, start + Duration::from_secs(100)), Backoff::MIN);
        assert_eq!(backoff.after(Duration::ZERO, start + Duration::from_secs(101)), Duration::from_millis(500));
        // Aufgewacht oder Netz gewechselt: ebenso
        backoff.reset();
        assert_eq!(backoff.after(Duration::ZERO, start + Duration::from_secs(200)), Backoff::MIN);
    }

    fn address(ip: &str, up: bool) -> Addresses {
        vec![("WLAN".to_owned(), ip.parse().unwrap(), up)]
    }

    async fn filtered(raw: Vec<SystemEvent>, states: Vec<Addresses>) -> (Vec<SystemEvent>, usize) {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        for event in raw {
            raw_tx.send(event).unwrap();
        }
        drop(raw_tx);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let states = Arc::new(Mutex::new(states.into_iter()));
        let asked = Arc::new(Mutex::new(0));
        let source = {
            let (states, asked) = (states.clone(), asked.clone());
            move || {
                *asked.lock().unwrap() += 1;
                let next = states.lock().unwrap().next().expect("nicht öfter fragen als nötig");
                async move { next }
            }
        };
        filter(raw_rx, &tx, source).await;
        drop(tx);
        let mut out = Vec::new();
        while let Some(event) = rx.recv().await {
            out.push(event);
        }
        let asked = *asked.lock().unwrap();
        (out, asked)
    }

    #[tokio::test]
    async fn network_events_only_pass_when_the_addresses_changed() {
        use SystemEvent::*;
        // Ein Schub aus drei Meldungen ist eine Prüfung; erst die zweite ändert etwas.
        let (out, asked) = filtered(
            vec![NetworkChanged, NetworkChanged, NetworkChanged],
            vec![address("192.168.1.20", true), address("192.168.1.20", true)],
        )
        .await;
        assert_eq!(out, []);
        assert_eq!(asked, 2, "einmal zu Beginn, einmal für den ganzen Schub");

        let (out, _) = filtered(
            vec![NetworkChanged],
            vec![address("192.168.1.20", true), address("10.0.0.7", true)],
        )
        .await;
        assert_eq!(out, [NetworkChanged]);

        // Kabel ab: die Adresse bleibt noch, die Karte ist aber unten
        let (out, _) = filtered(
            vec![NetworkChanged],
            vec![address("192.168.1.20", true), address("192.168.1.20", false)],
        )
        .await;
        assert_eq!(out, [NetworkChanged]);
    }

    #[tokio::test]
    async fn sleeping_and_waking_always_pass_in_order() {
        use SystemEvent::*;
        let (out, _) = filtered(
            vec![Sleeping, NetworkChanged, Woke, NetworkChanged],
            vec![address("192.168.1.20", true), address("192.168.1.20", true)],
        )
        .await;
        assert_eq!(out, [Sleeping, Woke]);
    }
}
