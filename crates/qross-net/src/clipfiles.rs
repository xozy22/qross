//! Kopierte Dateien über die Zwischenablage (Strg+C im Explorer, Cmd+C im Finder …).
//!
//! **Senden:** Liegen beim Wechsel des Cursors Dateien in der Zwischenablage, gehen ihre Liste
//! (`Clipboard::Files`) und die Inhalte (Streams vom Typ `ClipboardFile`, niedrige Priorität) an
//! die Gegenstelle – bis zu einer Gesamtgröße (Standard 1 GB). Größeres bleibt Drag & Drop
//! vorbehalten, sonst wanderten bei jedem Wechsel Gigabytes, die vielleicht nie eingefügt werden.
//! Fügt die Gegenstelle auf Abruf ein (Windows, `Control::PastesOnDemand`), geht nur die Liste,
//! ohne Größengrenze; sie holt die Dateien erst beim Einfügen (siehe `ondemand`).
//!
//! **Empfangen:** Die Dateien landen in einem eigenen Ordner im Cache, der Neuverbindungen und
//! Programmende übersteht, denn eingefügt wird oft erst später. In die Zwischenablage kommen sie
//! erst, wenn alle geprüft angekommen sind. Ein neuerer Inhalt verwirft laufende Übertragungen,
//! damit nie Älteres über Neueres geschrieben wird.
//!
//! Beide Seiten melden den Fortschritt und können abbrechen (siehe `progress`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Result};
use qross_proto::{Clipboard, ClipboardData, FileEntry, StreamKind};
use quinn::Connection;
use tracing::{debug, info, warn};

use crate::clipboard::Content;
use crate::files::{self, human_size, sanitize, FileOutcome, LocalEntry, Staging};
use crate::ondemand::{Offers, MAX_ENTRIES};
use crate::progress::{self, Direction, Info, Kind, Phase, Transfers};
use crate::streams::{send_clipboard, send_clipboard_message};

/// Standardgrenze für die automatische Übertragung kopierter Dateien (Summe aller Dateien)
pub const DEFAULT_LIMIT: u64 = 1_000_000_000;

/// Vorherige Übertragungen bleiben so lange liegen – falls gerade noch daraus eingefügt wird.
pub(crate) const KEEP_PREVIOUS: Duration = Duration::from_secs(10 * 60);

/// Meldungen für eine Oberfläche. `names` sind die obersten kopierten Dateien und Ordner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesNotice {
    /// Kopierte Dateien dieses Rechners gehen an `peer`.
    Sending { peer: String, names: Vec<String>, bytes: u64 },
    /// Zu groß für die automatische Übertragung (größer als `limit`): besser per Drag & Drop.
    TooLarge { peer: String, names: Vec<String>, limit: u64 },
    /// Kopierte Dateien von `peer` kommen an (den Fortschritt meldet `progress`).
    Receiving { peer: String, names: Vec<String>, bytes: u64 },
    /// Angekommen und in der Zwischenablage: jetzt einfügbar.
    Ready { peer: String, names: Vec<String>, bytes: u64 },
    /// Fehlgeschlagen (ein Abbruch erscheint nur als Ende im Fortschritt)
    Failed { peer: String, reason: String },
}

pub type Notify = Arc<dyn Fn(FilesNotice) + Send + Sync>;

/// Ablageordner für empfangene kopierte Dateien
pub fn default_store() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("Qross").join("clipboard")
}

/// Oberste Namen und Gesamtgröße einer Dateiliste (für Log und Oberfläche)
pub fn summary(entries: &[FileEntry]) -> (Vec<String>, u64) {
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let top = entry.path.split('/').next().unwrap_or_default();
        if !names.iter().any(|n| n == top) {
            names.push(top.to_owned());
        }
    }
    (names, entries.iter().map(|e| e.size).sum())
}

/// Eindeutig innerhalb dieses Prozesses und mit hoher Wahrscheinlichkeit auch zwischen Rechnern
/// (der Server sammelt Dateien mehrerer Clients im selben Ordner).
pub fn new_transfer_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let seed = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    let _ = NEXT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Überträgt den Inhalt der lokalen Zwischenablage an `peer`. Dateien nur bis `limit` Bytes
/// (0 = nie); sonst geht nur mit, was außer den Dateien in der Zwischenablage liegt. Die Dateien
/// erscheinen in `transfers`. Fügt `peer` auf Abruf ein (`on_demand`), geht nur ihre Liste (ohne
/// Größengrenze, höchstens `MAX_ENTRIES` Einträge), und `peer` holt sie beim Einfügen.
pub(crate) async fn send(
    conn: Connection,
    content: Content,
    limit: u64,
    peer: String,
    notify: Notify,
    transfers: Transfers,
    on_demand: Option<Offers>,
) {
    // Neuer Inhalt: Was `peer` bisher auf Abruf hatte, ist überholt.
    if let Some(offers) = &on_demand {
        offers.supersede(&peer);
    }
    let (paths, data) = match content {
        Content::Data(data) => {
            info!("Zwischenablage → „{peer}“: {}", data.summary());
            return send_clipboard(conn, data).await;
        }
        Content::Files { paths, data } => (paths, data),
    };
    let names: Vec<String> =
        paths.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
    if let Some(offers) = on_demand.filter(|_| limit != 0) {
        let roots = paths.clone();
        match tokio::task::spawn_blocking(move || files::collect_within(&roots, u64::MAX, MAX_ENTRIES)).await {
            Ok(Ok(Some(entries))) => return offer(&conn, entries, data, peer, names, &offers, &notify).await,
            Ok(Ok(None)) => info!("Mehr als {MAX_ENTRIES} Dateien und Ordner für „{peer}“ – sie gehen vorab statt auf Abruf"),
            // Nicht lesbar: Das stellt das Vorab-Übertragen gleich noch einmal fest und meldet es.
            _ => {}
        }
    }
    let collected = if limit == 0 {
        debug!("kopierte Dateien werden nicht übertragen (ausgeschaltet)");
        None
    } else {
        match tokio::task::spawn_blocking(move || files::collect_up_to(&paths, limit)).await {
            Ok(Ok(Some(entries))) => Some(entries),
            Ok(Ok(None)) => {
                warn!(
                    "Kopierte Dateien für „{peer}“ sind größer als {} – große Dateien bitte per Drag & Drop ziehen",
                    human_size(limit)
                );
                notify(FilesNotice::TooLarge { peer: peer.clone(), names: names.clone(), limit });
                None
            }
            Ok(Err(e)) => {
                warn!("Kopierte Dateien nicht lesbar: {e:#}");
                notify(FilesNotice::Failed { peer: peer.clone(), reason: format!("{e:#}") });
                None
            }
            Err(e) => {
                warn!("Kopierte Dateien nicht lesbar: {e}");
                None
            }
        }
    };
    let Some(entries) = collected else {
        // Was außer den Dateien in der Zwischenablage liegt (z. B. Dateinamen als Text), geht trotzdem mit.
        if !data.is_empty() {
            send_clipboard(conn, data).await;
        }
        return;
    };

    let transfer = new_transfer_id();
    let bytes: u64 = entries.iter().map(|e| e.entry.size).sum();
    info!("Zwischenablage → „{peer}“: {} ({})", names.join(", "), human_size(bytes));
    notify(FilesNotice::Sending { peer: peer.clone(), names, bytes });
    let message = Clipboard::Files { transfer, entries: entries.iter().map(|e| e.entry.clone()).collect(), data };
    if let Err(e) = send_clipboard_message(&conn, &message).await {
        warn!("Kopierte Dateien an „{peer}“ nicht gesendet: {e:#}");
        return;
    }
    let started = std::time::Instant::now();
    match files::send_tracked(conn, StreamKind::ClipboardFile, transfer, entries, &peer, &transfers).await {
        Ok(()) => debug!("kopierte Dateien an „{peer}“ gesendet in {:.1} s", started.elapsed().as_secs_f64()),
        // Neuerer Inhalt beim Empfänger, Verbindung weg oder eine Qross-Version ohne Dateien in der Zwischenablage
        Err(e) => info!("Übertragung kopierter Dateien an „{peer}“ beendet: {e:#}"),
    }
}

/// Kopierte Dateien auf Abruf: Nur die Liste geht an `peer`, der sie beim Einfügen holt (siehe `ondemand`).
async fn offer(
    conn: &Connection,
    entries: Vec<LocalEntry>,
    data: ClipboardData,
    peer: String,
    names: Vec<String>,
    offers: &Offers,
    notify: &Notify,
) {
    let transfer = new_transfer_id();
    let list: Vec<FileEntry> = entries.iter().map(|e| e.entry.clone()).collect();
    let bytes = list.iter().map(|e| e.size).sum();
    offers.add(transfer, &peer, entries);
    info!("Zwischenablage → „{peer}“: {} ({}, auf Abruf)", names.join(", "), human_size(bytes));
    notify(FilesNotice::Sending { peer: peer.clone(), names, bytes });
    if let Err(e) = send_clipboard_message(conn, &Clipboard::FilesOnDemand { transfer, entries: list, data }).await {
        warn!("Kopierte Dateien an „{peer}“ nicht gesendet: {e:#}");
    }
}

/// Vollständig angekommen: kann in die Zwischenablage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub peer: String,
    /// Oberste Dateien und Ordner im Ablageordner
    pub paths: Vec<PathBuf>,
    pub data: ClipboardData,
    pub names: Vec<String>,
    pub bytes: u64,
}

/// Eine Übertragung ist gescheitert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub peer: String,
    pub reason: String,
}

struct Announced {
    peer: String,
    entries: Vec<FileEntry>,
    data: ClipboardData,
}

#[derive(Default)]
struct Pending {
    announced: Option<Announced>,
    done: HashSet<u32>,
}

/// Buchführung des Empfängers. Ankündigung und Datei-Streams dürfen in beliebiger Reihenfolge eintreffen.
pub struct Incoming {
    staging: Arc<Staging>,
    pending: HashMap<u64, Pending>,
    /// Fortschritt für die Oberfläche; „Abbrechen“ verwirft über das Staging.
    progress: Transfers,
}

impl Incoming {
    pub fn new(staging: Arc<Staging>, progress: Transfers) -> Self {
        Self { staging, pending: HashMap::new(), progress }
    }

    pub fn staging(&self) -> Arc<Staging> {
        self.staging.clone()
    }

    /// Dateien sind angekündigt. Ältere angekündigte Übertragungen sind damit überholt.
    pub fn announce(&mut self, peer: String, transfer: u64, entries: Vec<FileEntry>, data: ClipboardData) -> Result<Option<Ready>> {
        let older: Vec<u64> =
            self.pending.iter().filter(|(id, p)| **id != transfer && p.announced.is_some()).map(|(id, _)| *id).collect();
        for id in older {
            self.drop_transfer(id, "überholt", Phase::Cancelled { by_peer: true });
        }
        if let Err(e) = entries.iter().try_for_each(|e| sanitize(&e.path).map(drop)) {
            self.drop_transfer(transfer, "ungültige Liste", Phase::Failed(format!("{e:#}")));
            bail!("{e:#}");
        }
        let (names, bytes) = summary(&entries);
        info!("kopierte Dateien von „{peer}“: {} ({})", names.join(", "), human_size(bytes));
        let staging = self.staging.clone();
        let info = Info { kind: Kind::Clipboard, peer: peer.clone(), names, total: bytes };
        self.progress.begin(Direction::Receiving, transfer, info, move || staging.discard(transfer));
        self.pending.entry(transfer).or_default().announced = Some(Announced { peer, entries, data });
        Ok(self.try_finish(transfer))
    }

    pub fn on_file(&mut self, outcome: FileOutcome) -> Result<Option<Ready>, Failed> {
        let id = outcome.transfer;
        if self.staging.is_cancelled(id) {
            // Kam nach dem Verwerfen (oder Abbrechen) noch an: wieder wegräumen
            self.drop_transfer(id, "abgebrochen", Phase::Cancelled { by_peer: false });
            return Ok(None);
        }
        match outcome.result {
            Ok(()) => {
                self.pending.entry(id).or_default().done.insert(outcome.index);
                Ok(self.try_finish(id))
            }
            Err(e) => {
                if let Some(cancelled) = progress::cancelled(&e) {
                    self.drop_transfer(id, "vom Sender abgebrochen", Phase::Cancelled { by_peer: cancelled.by_peer });
                    return Ok(None);
                }
                let peer = self.pending.get(&id).and_then(|p| p.announced.as_ref()).map(|a| a.peer.clone());
                self.drop_transfer(id, "fehlgeschlagen", Phase::Failed(format!("{e:#}")));
                warn!("kopierte Datei nicht übernommen: {e:#}");
                Err(Failed { peer: peer.unwrap_or_default(), reason: format!("{e:#}") })
            }
        }
    }

    /// Ein neuerer Inhalt ohne Dateien ist angekommen: laufende Übertragungen sind überholt.
    pub fn supersede(&mut self) {
        let ids: Vec<u64> = self.pending.keys().copied().collect();
        for id in ids {
            self.drop_transfer(id, "überholt", Phase::Cancelled { by_peer: true });
        }
    }

    fn drop_transfer(&mut self, id: u64, why: &str, phase: Phase) {
        debug!("kopierte Dateien {id} verworfen ({why})");
        self.pending.remove(&id);
        self.staging.discard(id);
        self.progress.end(Direction::Receiving, id, phase);
    }

    fn try_finish(&mut self, id: u64) -> Option<Ready> {
        let pending = self.pending.get(&id)?;
        let announced = pending.announced.as_ref()?;
        let complete =
            announced.entries.iter().enumerate().all(|(i, e)| e.is_dir || pending.done.contains(&(i as u32)));
        if !complete {
            return None;
        }
        let Announced { peer, entries, data } = self.pending.remove(&id)?.announced?;
        let dir = self.staging.transfer_dir(id);
        let mut paths = Vec::new();
        for entry in &entries {
            let local = dir.join(sanitize(&entry.path).ok()?);
            if entry.is_dir {
                if let Err(e) = std::fs::create_dir_all(&local) {
                    warn!("Ordner {} nicht angelegt: {e}", local.display());
                }
            }
            let top = dir.join(sanitize(entry.path.split('/').next().unwrap_or_default()).ok()?);
            if !paths.contains(&top) {
                paths.push(top);
            }
        }
        self.staging.prune(KEEP_PREVIOUS, id);
        self.progress.end(Direction::Receiving, id, Phase::Done);
        let (names, bytes) = summary(&entries);
        Some(Ready { peer, paths, data, names, bytes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64, is_dir: bool) -> FileEntry {
        FileEntry { path: path.into(), size, is_dir }
    }

    fn ok(transfer: u64, index: u32) -> FileOutcome {
        FileOutcome { transfer, index, result: Ok(()) }
    }

    fn incoming(name: &str) -> (Incoming, PathBuf) {
        let base = std::env::temp_dir().join(format!("qross-clipfiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        (Incoming::new(Staging::persistent(&base).unwrap(), Transfers::new()), base)
    }

    #[test]
    fn ready_once_announced_and_all_files_arrived_in_any_order() {
        let (mut rx, base) = incoming("order");
        let entries = vec![entry("Ordner", 0, true), entry("Ordner/a.txt", 5, false), entry("Ordner/leer", 0, true), entry("b.png", 9, false)];
        assert_eq!(rx.on_file(ok(7, 3)), Ok(None), "Datei vor der Ankündigung");
        let data = ClipboardData::from_text("Ordner\nb.png");
        assert_eq!(rx.announce("pc".into(), 7, entries, data.clone()).unwrap(), None);
        let ready = rx.on_file(ok(7, 1)).unwrap().expect("vollständig");
        let dir = base.join("7");
        assert_eq!(ready.paths, vec![dir.join("Ordner"), dir.join("b.png")]);
        assert_eq!((ready.peer.as_str(), ready.bytes, ready.data), ("pc", 14, data));
        assert_eq!(ready.names, ["Ordner", "b.png"]);
        assert!(dir.join("Ordner").join("leer").is_dir(), "leere Ordner werden angelegt");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn only_folders_are_ready_immediately() {
        let (mut rx, base) = incoming("dirs");
        let ready = rx.announce("pc".into(), 1, vec![entry("Leer", 0, true)], ClipboardData::default()).unwrap();
        assert_eq!(ready.map(|r| r.paths), Some(vec![base.join("1").join("Leer")]));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn newer_content_discards_running_transfers() {
        let (mut rx, base) = incoming("newer");
        rx.announce("pc".into(), 1, vec![entry("alt.txt", 3, false)], ClipboardData::default()).unwrap();
        std::fs::create_dir_all(base.join("1")).unwrap();
        rx.announce("pc".into(), 2, vec![entry("neu.txt", 3, false)], ClipboardData::default()).unwrap();
        assert!(!base.join("1").exists(), "ältere Übertragung ist weg");
        assert_eq!(rx.on_file(ok(1, 0)), Ok(None), "verspätete Datei der alten Übertragung zählt nicht");
        assert!(rx.on_file(ok(2, 0)).unwrap().is_some());

        rx.announce("pc".into(), 3, vec![entry("x.txt", 3, false)], ClipboardData::default()).unwrap();
        rx.supersede();
        assert_eq!(rx.on_file(ok(3, 0)), Ok(None), "Text kam danach – die Dateien sind überholt");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn failures_and_bad_paths_are_reported() {
        let (mut rx, base) = incoming("fail");
        rx.announce("laptop".into(), 4, vec![entry("a.txt", 3, false)], ClipboardData::default()).unwrap();
        let failed = rx.on_file(FileOutcome { transfer: 4, index: 0, result: Err(anyhow::anyhow!("Prüfsumme stimmt nicht")) });
        assert_eq!(failed, Err(Failed { peer: "laptop".into(), reason: "Prüfsumme stimmt nicht".into() }));
        assert!(rx.announce("laptop".into(), 5, vec![entry("../boot.ini", 1, false)], ClipboardData::default()).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_cancel_is_cleaned_up_but_not_reported_as_failure() {
        let (mut rx, base) = incoming("abbruch");
        rx.announce("pc".into(), 6, vec![entry("a.txt", 3, false)], ClipboardData::default()).unwrap();
        std::fs::create_dir_all(base.join("6")).unwrap();
        let reset = FileOutcome { transfer: 6, index: 0, result: Err(progress::Cancelled { by_peer: true }.into()) };
        assert_eq!(rx.on_file(reset), Ok(None), "kein Fehlerhinweis");
        assert!(!base.join("6").exists(), "Angekommenes ist weg");
        assert_eq!(rx.on_file(ok(6, 0)), Ok(None), "Nachzügler zählen nicht");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn transfer_ids_are_unique() {
        let ids: HashSet<u64> = (0..1000).map(|_| new_transfer_id()).collect();
        assert_eq!(ids.len(), 1000);
    }
}
