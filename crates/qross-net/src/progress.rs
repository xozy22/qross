//! Fortschritt laufender Dateiübertragungen (Drag & Drop, kopierte Dateien) und Abbrechen.
//!
//! Sender und Empfänger zählen je Übertragung die gesendeten bzw. empfangenen Bytes mit. Ein
//! Berichter meldet daraus etwa viermal pro Sekunde Stand, Rate und geschätzte Restzeit und zum
//! Schluss das Ergebnis (siehe [`Progress`]).
//!
//! Abbrechen kommt ohne Protokolländerung aus und funktioniert deshalb auch mit älteren Versionen:
//! - Der **Sender** setzt den laufenden Datei-Stream zurück (`RESET_STREAM`) und öffnet keine
//!   weiteren. Der Empfänger merkt es beim Lesen und verwirft die Übertragung wie bei jedem Fehler.
//! - Der **Empfänger** verwirft seine Staging-Dateien und hält den Stream an (`STOP_SENDING`).
//!   Der Sender merkt es beim nächsten Schreiben und hört auf.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qross_proto::StreamKind;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;
use tracing::info;

/// So oft gibt es Meldungen über laufende Übertragungen.
pub const REPORT_EVERY: Duration = Duration::from_millis(250);
/// Wird der Berichter früher geweckt (eine andere Übertragung endet), wartet eine laufende bis zu
/// ihrer nächsten Meldung mindestens so lange.
const MIN_GAP: Duration = Duration::from_millis(200);
/// Über so viele Sekunden wird die Rate gemittelt.
const RATE_WINDOW: Duration = Duration::from_secs(3);
/// Die Restzeit erst schätzen, wenn die Rate halbwegs steht.
const ESTIMATE_AFTER: Duration = Duration::from_secs(1);
/// Datei-Streams einer nie angekündigten Übertragung nicht ewig merken
const UNANNOUNCED_LIMIT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Sending,
    Receiving,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Drag & Drop
    Drag,
    /// Kopierte Dateien aus der Zwischenablage
    Clipboard,
}

impl Kind {
    pub(crate) fn of(stream: StreamKind) -> Self {
        match stream {
            StreamKind::ClipboardFile => Self::Clipboard,
            _ => Self::Drag,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Running,
    /// Vollständig gesendet bzw. vollständig und geprüft angekommen
    Done,
    /// Abgebrochen. `by_peer`: auf der anderen Seite (dort abgebrochen, verworfen oder durch neuere
    /// kopierte Dateien überholt); sonst hier, etwa über [`Transfers::cancel`].
    Cancelled { by_peer: bool },
    Failed(String),
}

impl Phase {
    pub fn is_running(&self) -> bool {
        *self == Self::Running
    }

    /// Ergebnis eines Sende- oder Empfangsvorgangs; [`Cancelled`] gilt als Abbruch, nicht als Fehler.
    pub fn of(result: &anyhow::Result<()>) -> Self {
        match result {
            Ok(()) => Self::Done,
            Err(e) => match cancelled(e) {
                Some(c) => Self::Cancelled { by_peer: c.by_peer },
                None => Self::Failed(format!("{e:#}")),
            },
        }
    }
}

/// Fehler einer abgebrochenen (statt fehlgeschlagenen) Übertragung
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled {
    pub by_peer: bool,
}

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.by_peer { "von der Gegenstelle abgebrochen" } else { "abgebrochen" })
    }
}

impl std::error::Error for Cancelled {}

/// War es ein Abbruch? Sucht auch hinter angehängtem Kontext.
pub fn cancelled(error: &anyhow::Error) -> Option<Cancelled> {
    error.chain().find_map(|e| e.downcast_ref::<Cancelled>()).copied()
}

/// Stand einer Übertragung für eine Oberfläche
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// Nummer der Übertragung; zusammen mit `direction` eindeutig (für [`Transfers::cancel`])
    pub transfer: u64,
    pub direction: Direction,
    pub kind: Kind,
    /// Name der Gegenstelle (Server- bzw. Client-Name)
    pub peer: String,
    /// Oberste Dateien und Ordner
    pub names: Vec<String>,
    /// Gesamtgröße in Bytes
    pub total: u64,
    /// Bisher gesendet bzw. empfangen
    pub bytes: u64,
    /// Läuft: über die letzten Sekunden gemittelt; danach: Durchschnitt der ganzen Übertragung
    pub bytes_per_s: u64,
    /// Geschätzte Restzeit (`None`: noch unbekannt, steht gerade still oder beendet)
    pub remaining: Option<Duration>,
    /// Seit Beginn bzw. bis zum Ende
    pub elapsed: Duration,
    /// `Running`, solange sie läuft; die letzte Meldung trägt das Ergebnis.
    pub phase: Phase,
}

/// Was übertragen wird
#[derive(Debug, Clone)]
pub(crate) struct Info {
    pub kind: Kind,
    pub peer: String,
    pub names: Vec<String>,
    pub total: u64,
}

/// Für den Sender: Bytes zählen und auf „Abbrechen“ achten
pub struct Outgoing {
    bytes: Arc<AtomicU64>,
    cancel: watch::Receiver<bool>,
}

impl Outgoing {
    pub(crate) fn add(&self, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    /// Kehrt zurück, sobald abgebrochen wird (nie, wenn es dazu nicht mehr kommen kann).
    pub(crate) async fn cancelled(&self) {
        let mut cancel = self.cancel.clone();
        if cancel.wait_for(|c| *c).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

type CancelFn = Arc<dyn Fn() + Send + Sync>;

struct Entry {
    /// `None`: Datei-Streams sind schon da, die Ankündigung noch nicht (Reihenfolge ist beliebig)
    info: Option<Info>,
    bytes: Arc<AtomicU64>,
    started: Instant,
    ended: Option<Instant>,
    phase: Phase,
    cancel: Option<CancelFn>,
    /// (Zeitpunkt, Bytes) der letzten Sekunden für die Rate
    samples: VecDeque<(Instant, u64)>,
    reported: Option<Instant>,
}

impl Entry {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            info: None,
            bytes: Arc::default(),
            started: now,
            ended: None,
            phase: Phase::Running,
            cancel: None,
            samples: VecDeque::from([(now, 0)]),
            reported: None,
        }
    }

    fn finish(&mut self, phase: Phase) {
        if self.phase.is_running() && !phase.is_running() {
            self.phase = phase;
            self.ended = Some(Instant::now());
        }
    }

    fn bytes(&self) -> u64 {
        let total = self.info.as_ref().map_or(u64::MAX, |i| i.total);
        self.bytes.load(Ordering::Relaxed).min(total)
    }

    fn sample(&mut self, now: Instant) {
        let bytes = self.bytes();
        self.samples.push_back((now, bytes));
        // Der älteste Messpunkt bleibt knapp außerhalb des Fensters, dann gilt die Rate fürs ganze.
        while self.samples.len() > 2 && now.saturating_duration_since(self.samples[1].0) >= RATE_WINDOW {
            self.samples.pop_front();
        }
    }

    fn progress(&self, direction: Direction, transfer: u64, info: &Info, now: Instant) -> Progress {
        let bytes = self.bytes();
        let elapsed = self.ended.unwrap_or(now).saturating_duration_since(self.started);
        let (bytes_per_s, remaining) = if self.phase.is_running() {
            let (since, before) = self.samples.front().copied().unwrap_or((self.started, 0));
            let span = now.saturating_duration_since(since).as_secs_f64();
            let rate = if span > 0.0 { bytes.saturating_sub(before) as f64 / span } else { 0.0 };
            let left = info.total.saturating_sub(bytes) as f64;
            let remaining = if rate >= 1.0 && elapsed >= ESTIMATE_AFTER {
                Duration::try_from_secs_f64(left / rate).ok()
            } else {
                None
            };
            (rate as u64, remaining)
        } else {
            let secs = elapsed.as_secs_f64();
            ((if secs > 0.0 { bytes as f64 / secs } else { 0.0 }) as u64, None)
        };
        Progress {
            transfer,
            direction,
            kind: info.kind,
            peer: info.peer.clone(),
            names: info.names.clone(),
            total: info.total,
            bytes,
            bytes_per_s,
            remaining,
            elapsed,
            phase: self.phase.clone(),
        }
    }
}

#[derive(Default)]
struct Shared {
    entries: Mutex<HashMap<(Direction, u64), Entry>>,
    /// Weckt den Berichter (neue Übertragung, Ende, Abbruch)
    wake: Notify,
}

/// Alle laufenden Übertragungen eines Servers bzw. Clients, gemeinsam für Datei-Streams,
/// Hauptschleife und Oberfläche. Klonen ist billig.
#[derive(Clone, Default)]
pub struct Transfers(Arc<Shared>);

impl Transfers {
    pub fn new() -> Self {
        Self::default()
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<(Direction, u64), Entry>> {
        self.0.entries.lock().unwrap()
    }

    /// Zähler für die Bytes einer Übertragung. Legt sie an, falls die Ankündigung noch fehlt.
    pub(crate) fn counter(&self, direction: Direction, transfer: u64) -> Arc<AtomicU64> {
        self.entries().entry((direction, transfer)).or_insert_with(Entry::new).bytes.clone()
    }

    /// Eine Übertragung beginnt bzw. ist angekündigt. `cancel` bricht sie auf dieser Seite ab.
    pub(crate) fn begin(&self, direction: Direction, transfer: u64, info: Info, cancel: impl Fn() + Send + Sync + 'static) {
        {
            let mut entries = self.entries();
            let entry = entries.entry((direction, transfer)).or_insert_with(Entry::new);
            entry.info = Some(info);
            entry.cancel = Some(Arc::new(cancel));
        }
        self.0.wake.notify_one();
    }

    /// Für den Sender: anmelden und Zähler samt Abbruchsignal holen.
    pub(crate) fn begin_sending(&self, transfer: u64, info: Info) -> Outgoing {
        let (tx, cancel) = watch::channel(false);
        self.begin(Direction::Sending, transfer, info, move || {
            let _ = tx.send(true);
        });
        Outgoing { bytes: self.counter(Direction::Sending, transfer), cancel }
    }

    /// Ergebnis einer Übertragung. Zählt nur, solange sie noch läuft: Ein Abbruch bleibt ein Abbruch,
    /// auch wenn danach noch Fehler der abgebrochenen Streams eintreffen.
    pub(crate) fn end(&self, direction: Direction, transfer: u64, phase: Phase) {
        if let Some(entry) = self.entries().get_mut(&(direction, transfer)) {
            entry.finish(phase);
        }
        self.0.wake.notify_one();
    }

    /// Bricht eine laufende Übertragung auf dieser Seite ab (Knopf „Abbrechen“). Die Gegenstelle
    /// erfährt es über den abgebrochenen Stream. `false`, wenn sie nicht (mehr) läuft.
    pub fn cancel(&self, direction: Direction, transfer: u64) -> bool {
        let cancel = {
            let mut entries = self.entries();
            let Some(entry) = entries.get_mut(&(direction, transfer)) else { return false };
            if !entry.phase.is_running() || entry.info.is_none() {
                return false;
            }
            entry.finish(Phase::Cancelled { by_peer: false });
            entry.cancel.clone()
        };
        info!("Übertragung {transfer} abgebrochen");
        self.0.wake.notify_one();
        if let Some(cancel) = cancel {
            cancel();
        }
        true
    }

    /// Die Verbindung ist weg: Empfangene Übertragungen von `peer` (bzw. alle) werden nie mehr
    /// vollständig und werden verworfen.
    pub(crate) fn abort_receiving(&self, peer: Option<&str>, reason: &str) {
        let cancels: Vec<CancelFn> = {
            let mut entries = self.entries();
            entries
                .iter_mut()
                .filter(|((direction, _), entry)| {
                    *direction == Direction::Receiving
                        && entry.phase.is_running()
                        && peer.is_none_or(|p| entry.info.as_ref().is_some_and(|i| i.peer == p))
                })
                .filter_map(|(_, entry)| {
                    entry.finish(Phase::Failed(reason.to_owned()));
                    entry.cancel.clone()
                })
                .collect()
        };
        self.0.wake.notify_one();
        for cancel in cancels {
            cancel();
        }
    }

    /// Meldet über `report`, solange der zurückgegebene Wächter lebt: laufende Übertragungen etwa
    /// alle [`REPORT_EVERY`], beendete einmal mit ihrem Ergebnis. Ohne Übertragungen ruht er.
    pub(crate) fn spawn_reporter(&self, report: impl Fn(Progress) + Send + 'static) -> Reporter {
        let transfers = self.clone();
        Reporter(tokio::spawn(async move {
            loop {
                if transfers.report_once(Instant::now(), &report) {
                    transfers.0.wake.notified().await;
                } else {
                    tokio::select! {
                        () = tokio::time::sleep(REPORT_EVERY) => {}
                        () = transfers.0.wake.notified() => {}
                    }
                }
            }
        }))
    }

    /// Meldet fällige Stände und alle Enden, vergisst Beendete. `true`, wenn keine angekündigte
    /// Übertragung mehr läuft.
    fn report_once(&self, now: Instant, report: &impl Fn(Progress)) -> bool {
        let mut out = Vec::new();
        let idle = {
            let mut entries = self.entries();
            entries.retain(|&(direction, transfer), entry| {
                let Some(info) = entry.info.clone() else {
                    // Unangekündigt: nicht melden; beendet oder zu alt → vergessen
                    return entry.phase.is_running() && now.saturating_duration_since(entry.started) < UNANNOUNCED_LIMIT;
                };
                entry.sample(now);
                if !entry.phase.is_running() {
                    out.push(entry.progress(direction, transfer, &info, now));
                    return false;
                }
                if entry.reported.is_none_or(|at| now.saturating_duration_since(at) >= MIN_GAP) {
                    entry.reported = Some(now);
                    out.push(entry.progress(direction, transfer, &info, now));
                }
                true
            });
            !entries.values().any(|e| e.info.is_some())
        };
        for progress in out {
            report(progress);
        }
        idle
    }
}

/// Meldet, solange er lebt (siehe [`Transfers::spawn_reporter`]).
pub(crate) struct Reporter(JoinHandle<()>);

impl Drop for Reporter {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn info(total: u64) -> Info {
        Info { kind: Kind::Drag, peer: "laptop".into(), names: vec!["Film.mkv".into()], total }
    }

    fn collect(transfers: &Transfers, now: Instant) -> (Vec<Progress>, bool) {
        let out = Mutex::new(Vec::new());
        let idle = transfers.report_once(now, &|p| out.lock().unwrap().push(p));
        (out.into_inner().unwrap(), idle)
    }

    #[test]
    fn running_transfers_report_rate_and_remaining_time() {
        let transfers = Transfers::new();
        let job = transfers.begin_sending(7, info(10_000_000));
        let start = Instant::now();
        job.add(1_000_000);
        let (first, idle) = collect(&transfers, start + Duration::from_millis(250));
        assert!(!idle);
        assert_eq!((first[0].bytes, first[0].total, first[0].phase.clone()), (1_000_000, 10_000_000, Phase::Running));
        assert_eq!(first[0].remaining, None, "in der ersten Sekunde noch keine Schätzung");

        job.add(1_000_000);
        let (second, _) = collect(&transfers, start + Duration::from_secs(2));
        let p = &second[0];
        // 2 MB in 2 s → etwa 1 MB/s, noch 8 MB → etwa 8 s
        assert!((900_000..=1_100_000).contains(&p.bytes_per_s), "{p:?}");
        let left = p.remaining.expect("Restzeit").as_secs_f64();
        assert!((7.0..=9.0).contains(&left), "{p:?}");

        // Zu früh für die nächste Meldung: nichts
        let (none, _) = collect(&transfers, start + Duration::from_millis(2050));
        assert!(none.is_empty());
    }

    #[test]
    fn the_end_is_reported_once_and_cancel_wins_over_later_errors() {
        let transfers = Transfers::new();
        let called = Arc::new(AtomicBool::new(false));
        let flag = called.clone();
        transfers.begin(Direction::Receiving, 3, info(100), move || flag.store(true, Ordering::SeqCst));
        assert!(transfers.cancel(Direction::Receiving, 3));
        assert!(called.load(Ordering::SeqCst), "Abbruch auf dieser Seite ausgeführt");
        assert!(!transfers.cancel(Direction::Receiving, 3), "läuft nicht mehr");
        transfers.end(Direction::Receiving, 3, Phase::Failed("Stream zurückgesetzt".into()));

        let (out, idle) = collect(&transfers, Instant::now());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].phase, Phase::Cancelled { by_peer: false });
        assert!(idle);
        assert!(collect(&transfers, Instant::now()).0.is_empty(), "danach vergessen");
    }

    #[test]
    fn streams_may_arrive_before_the_announcement() {
        let transfers = Transfers::new();
        transfers.counter(Direction::Receiving, 9).fetch_add(500, Ordering::Relaxed);
        let (out, idle) = collect(&transfers, Instant::now());
        assert!(out.is_empty() && idle, "Unangekündigtes wird nicht gemeldet");
        assert!(!transfers.cancel(Direction::Receiving, 9));

        transfers.begin(Direction::Receiving, 9, info(1000), || {});
        let (out, _) = collect(&transfers, Instant::now());
        assert_eq!(out[0].bytes, 500, "schon Gezähltes bleibt");
    }

    #[test]
    fn lost_connections_abort_only_the_receiving_transfers_of_that_peer() {
        let transfers = Transfers::new();
        let discarded = Arc::new(AtomicBool::new(false));
        let flag = discarded.clone();
        transfers.begin(Direction::Receiving, 1, info(10), move || flag.store(true, Ordering::SeqCst));
        transfers.begin(Direction::Receiving, 2, Info { peer: "mac".into(), ..info(10) }, || {});
        let _job = transfers.begin_sending(3, info(10));
        transfers.abort_receiving(Some("laptop"), "Verbindung getrennt");
        assert!(discarded.load(Ordering::SeqCst), "Staging wird verworfen");

        let (out, _) = collect(&transfers, Instant::now());
        let phase = |id| out.iter().find(|p| p.transfer == id).map(|p| p.phase.clone());
        assert_eq!(phase(1), Some(Phase::Failed("Verbindung getrennt".into())));
        assert_eq!(phase(2), Some(Phase::Running));
        assert_eq!(phase(3), Some(Phase::Running));
    }

    #[test]
    fn results_map_to_phases() {
        assert_eq!(Phase::of(&Ok(())), Phase::Done);
        let peer: anyhow::Result<()> = Err(anyhow::Error::new(Cancelled { by_peer: true }).context("Datei 2"));
        assert_eq!(Phase::of(&peer), Phase::Cancelled { by_peer: true });
        assert_eq!(Phase::of(&Err(anyhow::anyhow!("Verbindung weg"))), Phase::Failed("Verbindung weg".into()));
    }

    #[tokio::test]
    async fn the_sender_notices_a_cancel() {
        let transfers = Transfers::new();
        let job = transfers.begin_sending(5, info(10));
        assert!(!job.is_cancelled());
        transfers.cancel(Direction::Sending, 5);
        tokio::time::timeout(Duration::from_secs(1), job.cancelled()).await.expect("Abbruch kommt an");
        assert!(job.is_cancelled());
    }
}
