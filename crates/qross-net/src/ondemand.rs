//! Kopierte Dateien auf Abruf: Der Sender schickt beim Wechsel nur die Liste
//! (`Clipboard::FilesOnDemand`). Der Empfänger legt sie sofort in seine Zwischenablage (Windows:
//! virtuelle Dateien) und holt eine Datei erst, wenn ein Programm sie einfügt: je Datei ein
//! bidirektionaler Stream vom Typ `Fetch` mit einer `FileRequest` ab einem Offset. So entfallen die
//! Grenze für kopierte Dateien und Übertragungen, die nie eingefügt werden.
//!
//! Nur Gegenstellen, die `Control::PastesOnDemand` gemeldet haben, bekommen Dateien so; alle anderen
//! weiter vorab (siehe `clipfiles`), ebenso Listen mit mehr als [`MAX_ENTRIES`] Einträgen.
//!
//! **Fortschritt:** Alle Abrufe eines Einfügevorgangs (Nummer `paste`, vom Empfänger vergeben) zählen auf
//! beiden Seiten als eine Übertragung ([`Pastes`]). Sie endet, wenn jede Datei einmal ganz übertragen
//! ist, beim ersten Fehler oder Abbruch, sonst [`IDLE`] nach dem letzten Abruf (das Programm hat
//! Dateien übersprungen).
//!
//! **Abbrechen** wie bei den anderen Übertragungen über die Streams (siehe `progress`): Der Empfänger hält
//! den Stream an (`STOP_SENDING`), auch wenn im Explorer „Abbrechen“ gedrückt wird; der Sender setzt ihn
//! zurück (`RESET_STREAM`). Der Explorer macht nach einer abgebrochenen Datei mit der nächsten weiter.
//! Ein abgebrochener Einfügevorgang bricht deshalb auch die folgenden Abrufe gleich ab, bis eine Weile
//! ([`LINGER`]) keiner mehr kommt.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use qross_platform::virtual_files::cancelled as cancelled_read;
use qross_platform::{VirtualFile, VirtualFiles, VirtualSource};
use qross_proto::{read_frame, write_frame, ClipboardData, FileEntry, FileReply, FileRequest, FileTrailer, StreamKind};
use quinn::{Connection, RecvStream, SendStream};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use crate::clipboard::ClipboardHub;
use crate::clipfiles::{new_transfer_id, summary, FilesNotice, Notify, KEEP_PREVIOUS};
use crate::files::{frame_read_error, frame_write_error, human_size, read_error, sanitize, write_error, LocalEntry, CHUNK};
use crate::progress::{self, Cancelled, Direction, Info, Kind, Phase, Transfers};
use crate::streams::{read_kind, BULK_PRIORITY};

/// Höchstens so viele Dateien und Ordner gehen auf Abruf; längere Listen gehen wie bisher vorab (bis zur
/// eingestellten Grenze). Der Explorer bekäme sonst eine riesige Liste (je Eintrag knapp 600 Bytes).
pub const MAX_ENTRIES: usize = 10_000;
/// So lange ohne Abruf, dann ist ein Einfügevorgang zu Ende (in Unit-Tests kürzer).
const IDLE: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(5) };
/// Ein abgebrochener Einfügevorgang bricht neue Abrufe ab, bis so lange keiner mehr kam.
const LINGER: Duration = Duration::from_secs(3);
/// Stücke zwischen Netz und lesendem Programm (je bis zu `CHUNK`): Liest es langsamer, bremst das den
/// Sender über die Flusskontrolle von QUIC.
const PIECES: usize = 8;

/// Verbindung zur Gegenstelle für Abrufe, nach einer Neuverbindung die neue; sonst warum es keine gibt
pub(crate) type Link = Arc<dyn Fn() -> Result<Connection, String> + Send + Sync>;

/// Die Datei ist beim Sender nicht (mehr) zu haben (`FileReply::Unavailable`).
#[derive(Debug)]
struct Unavailable(String);

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unavailable {}

/// Das Programm hat die Datei nicht weiter gelesen (der Empfänger räumt dann selbst auf).
#[derive(Debug)]
struct ReaderGone;

impl fmt::Display for ReaderGone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("stopped reading")
    }
}

impl std::error::Error for ReaderGone {}

// ------------------------------------------------------------------ Einfügevorgänge

/// Einfügevorgänge in einer Richtung, für Fortschritt und Abbrechen (siehe Modulbeschreibung). Klonen ist
/// billig.
#[derive(Clone)]
pub(crate) struct Pastes(Arc<PastesShared>);

struct PastesShared {
    direction: Direction,
    transfers: Transfers,
    /// Für die Leerlauf-Prüfung; Tickets enden auch in Systemthreads.
    runtime: Handle,
    state: Mutex<HashMap<u64, Paste>>,
    /// Zählt die Vorgänge in der Reihenfolge ihres Beginns
    started: AtomicU64,
}

struct Paste {
    /// Laufende Nummer: Bei zwei gleichzeitigen bekommt der ältere zuerst die nächste Datei.
    order: u64,
    transfer: u64,
    /// Dateien (ohne Ordner) der Übertragung
    files: usize,
    opened: HashSet<u32>,
    complete: HashSet<u32>,
    /// Laufende Abrufe
    active: usize,
    /// Hält laufende Abrufe an (abgebrochen)
    stop: watch::Sender<bool>,
    /// Zu Ende mit diesem Ergebnis; Zeitpunkt des letzten Abrufs danach (für `LINGER`)
    ended: Option<(Instant, Phase)>,
    /// Zählt jeden Abruf, damit eine ältere Leerlauf-Prüfung nichts beendet
    generation: u64,
    bytes: Arc<AtomicU64>,
}

impl Pastes {
    /// In einer Tokio-Laufzeit aufrufen.
    pub fn new(direction: Direction, transfers: Transfers) -> Self {
        Self(Arc::new(PastesShared {
            direction,
            transfers,
            runtime: Handle::current(),
            state: Mutex::new(HashMap::new()),
            started: AtomicU64::new(0),
        }))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Paste>> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Empfänger: Datei `index` der Übertragung `transfer` wird gelesen. Sie gehört zum Einfügevorgang
    /// dieser Übertragung, der sie noch nicht gelesen hat – zuerst zu einem eben abgebrochenen, der sie
    /// gleich ablehnt –, sonst zu einem neuen.
    pub fn join(&self, transfer: u64, index: u32, info: &Info, files: usize) -> Ticket {
        let now = Instant::now();
        let mut state = self.state();
        prune(&mut state, now);
        let fits = |p: &Paste| p.transfer == transfer && !p.opened.contains(&index);
        let oldest = |cancelled: bool| {
            state
                .iter()
                .filter(|(_, p)| fits(p))
                .filter(|(_, p)| match &p.ended {
                    None => !cancelled,
                    Some((_, ended)) => cancelled && matches!(ended, Phase::Cancelled { .. }),
                })
                .min_by_key(|(_, p)| p.order)
                .map(|(id, _)| *id)
        };
        let cancelled = oldest(true);
        let running = || oldest(false);
        let id = match cancelled.or_else(running) {
            Some(id) => id,
            None => self.start(&mut state, transfer, info, files, new_transfer_id()),
        };
        self.ticket(&mut state, id, index, now)
    }

    /// Sender: Abruf für den Einfügevorgang `paste` der Gegenstelle. `None`, wenn der hier abgebrochen
    /// wurde.
    pub fn enter(&self, paste: u64, transfer: u64, index: u32, info: &Info, files: usize) -> Option<Ticket> {
        let now = Instant::now();
        let mut state = self.state();
        prune(&mut state, now);
        match state.get_mut(&paste).map(|p| &mut p.ended) {
            Some(Some((at, Phase::Cancelled { .. }))) => {
                *at = now;
                return None;
            }
            // Fertig oder fehlgeschlagen und jetzt doch weiter (etwa nach einer Pause beim Empfänger)
            Some(Some(_)) => {
                state.remove(&paste);
                self.start(&mut state, transfer, info, files, paste);
            }
            Some(None) => {}
            None => {
                self.start(&mut state, transfer, info, files, paste);
            }
        }
        Some(self.ticket(&mut state, paste, index, now))
    }

    fn start(&self, state: &mut HashMap<u64, Paste>, transfer: u64, info: &Info, files: usize, id: u64) -> u64 {
        let (stop, _) = watch::channel(false);
        let order = self.0.started.fetch_add(1, Ordering::Relaxed);
        let weak: Weak<PastesShared> = Arc::downgrade(&self.0);
        self.0.transfers.begin(self.0.direction, id, info.clone(), move || {
            if let Some(shared) = weak.upgrade() {
                Pastes(shared).stopped_here(id, order);
            }
        });
        let bytes = self.0.transfers.counter(self.0.direction, id);
        let paste = Paste {
            order,
            transfer,
            files,
            opened: HashSet::new(),
            complete: HashSet::new(),
            active: 0,
            stop,
            ended: None,
            generation: 0,
            bytes,
        };
        state.insert(id, paste);
        id
    }

    fn ticket(&self, state: &mut HashMap<u64, Paste>, id: u64, index: u32, now: Instant) -> Ticket {
        let paste = state.get_mut(&id).expect("just created");
        paste.opened.insert(index);
        if let Some((at, _)) = &mut paste.ended {
            *at = now;
        }
        paste.active += 1;
        paste.generation += 1;
        let order = paste.order;
        Ticket { pastes: self.clone(), paste: id, order, index, bytes: paste.bytes.clone(), stop: paste.stop.subscribe() }
    }

    /// Der Vorgang `id`, sofern es noch derselbe ist (beim Sender kann dieselbe Nummer neu beginnen)
    fn find(state: &mut HashMap<u64, Paste>, id: u64, order: u64) -> Option<&mut Paste> {
        state.get_mut(&id).filter(|p| p.order == order)
    }

    /// Abgebrochen über `Transfers` (Knopf „Abbrechen“, Rechte entzogen) oder die Verbindung ist weg
    /// (dann fehlgeschlagen): laufende Abrufe anhalten.
    fn stopped_here(&self, id: u64, order: u64) {
        let phase = self.0.transfers.phase(self.0.direction, id).unwrap_or(Phase::Cancelled { by_peer: false });
        let mut state = self.state();
        if let Some(paste) = Self::find(&mut state, id, order) {
            if paste.ended.is_none() {
                paste.ended = Some((Instant::now(), phase));
            }
            let _ = paste.stop.send(true);
        }
    }

    /// Beendet den Einfügevorgang, falls er noch läuft. Ein Abbruch hält auch die anderen Abrufe an.
    fn end(&self, id: u64, order: u64, phase: Phase) {
        let mut state = self.state();
        let Some(paste) = Self::find(&mut state, id, order).filter(|p| p.ended.is_none()) else { return };
        if matches!(phase, Phase::Cancelled { .. }) {
            let _ = paste.stop.send(true);
        }
        paste.ended = Some((Instant::now(), phase.clone()));
        self.0.transfers.end(self.0.direction, id, phase);
    }

    fn complete(&self, id: u64, order: u64, index: u32) {
        let mut state = self.state();
        let Some(paste) = Self::find(&mut state, id, order) else { return };
        paste.complete.insert(index);
        if paste.ended.is_none() && paste.complete.len() >= paste.files {
            paste.ended = Some((Instant::now(), Phase::Done));
            self.0.transfers.end(self.0.direction, id, Phase::Done);
        }
    }

    /// Ein Abruf ist vorbei. War es der letzte, endet der Vorgang nach `IDLE`, wenn keiner mehr kommt.
    fn release(&self, id: u64, order: u64) {
        let generation = {
            let mut state = self.state();
            let Some(paste) = Self::find(&mut state, id, order) else { return };
            paste.active -= 1;
            if paste.active > 0 || paste.ended.is_some() {
                return;
            }
            paste.generation += 1;
            paste.generation
        };
        let pastes = self.clone();
        self.0.runtime.spawn(async move {
            tokio::time::sleep(IDLE).await;
            let mut state = pastes.state();
            let Some(paste) = Self::find(&mut state, id, order) else { return };
            if paste.generation == generation && paste.active == 0 && paste.ended.is_none() {
                // Das Programm hat nicht alles gelesen (übersprungen): fertig mit dem, was kam.
                paste.ended = Some((Instant::now(), Phase::Done));
                pastes.0.transfers.settle(pastes.0.direction, id);
            }
        });
    }

    fn ended(&self, id: u64, order: u64) -> Option<Phase> {
        let mut state = self.state();
        Self::find(&mut state, id, order).and_then(|p| p.ended.as_ref().map(|(_, phase)| phase.clone()))
    }
}

/// Vergisst beendete Vorgänge, nach denen eine Weile nichts mehr kam.
fn prune(state: &mut HashMap<u64, Paste>, now: Instant) {
    state.retain(|_, p| p.active > 0 || p.ended.as_ref().is_none_or(|(at, _)| now.saturating_duration_since(*at) < LINGER));
}

/// Ein Abruf in einem Einfügevorgang; zählt die Bytes und erfährt vom Abbrechen.
pub(crate) struct Ticket {
    pastes: Pastes,
    paste: u64,
    order: u64,
    index: u32,
    bytes: Arc<AtomicU64>,
    stop: watch::Receiver<bool>,
}

impl Ticket {
    pub fn add(&self, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Die Datei ist ganz übertragen.
    pub fn complete(&self) {
        self.pastes.complete(self.paste, self.order, self.index);
    }

    /// Fehlgeschlagen bzw. abgebrochen: beendet den ganzen Einfügevorgang.
    pub fn fail(&self, phase: Phase) {
        self.pastes.end(self.paste, self.order, phase);
    }

    /// Wurde der Einfügevorgang abgebrochen?
    fn is_cancelled(&self) -> bool {
        matches!(self.pastes.ended(self.paste, self.order), Some(Phase::Cancelled { .. }))
    }

    /// Kehrt zurück, sobald der Einfügevorgang angehalten wird (nie, wenn das nicht mehr geht).
    async fn stopped(&self) {
        let mut stop = self.stop.clone();
        if stop.wait_for(|s| *s).await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// Fehler für einen angehaltenen Abruf: abgebrochen oder (Verbindung weg) fehlgeschlagen
    fn stop_error(&self) -> anyhow::Error {
        match self.pastes.ended(self.paste, self.order) {
            Some(Phase::Failed(reason)) => anyhow::anyhow!(reason),
            Some(Phase::Cancelled { by_peer }) => Cancelled { by_peer }.into(),
            _ => Cancelled { by_peer: false }.into(),
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.pastes.release(self.paste, self.order);
    }
}

// ------------------------------------------------------------------ Sender

/// Kopierte Dateien, die Gegenstellen beim Einfügen abrufen dürfen (Sender). Klonen ist billig.
#[derive(Clone, Default)]
pub(crate) struct Offers(Arc<Mutex<HashMap<u64, Offer>>>);

struct Offer {
    peer: String,
    entries: Arc<Vec<LocalEntry>>,
    info: Info,
    files: usize,
    /// Seit wann eine neuere Zwischenablage an dieselbe Gegenstelle ging
    superseded: Option<Instant>,
    /// Letzter Abruf
    used: Instant,
}

impl Offers {
    fn offers(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Offer>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Neue Zwischenablage an `peer`: Ältere Angebote an ihn sind überholt. Sie bleiben noch
    /// `KEEP_PREVIOUS` nach dem letzten Abruf abrufbar, falls dort gerade daraus eingefügt wird.
    pub fn supersede(&self, peer: &str) {
        let now = Instant::now();
        let mut offers = self.offers();
        for offer in offers.values_mut().filter(|o| o.peer == peer && o.superseded.is_none()) {
            offer.superseded = Some(now);
        }
        offers.retain(|_, o| o.superseded.is_none_or(|at| now.saturating_duration_since(at.max(o.used)) < KEEP_PREVIOUS));
    }

    /// `peer` darf die Dateien `entries` als Übertragung `transfer` abrufen.
    pub fn add(&self, transfer: u64, peer: &str, entries: Vec<LocalEntry>) {
        let list: Vec<FileEntry> = entries.iter().map(|e| e.entry.clone()).collect();
        let (names, total) = summary(&list);
        let info = Info { kind: Kind::Clipboard, peer: peer.to_owned(), names, total };
        let files = list.iter().filter(|e| !e.is_dir).count();
        let offer =
            Offer { peer: peer.to_owned(), entries: Arc::new(entries), info, files, superseded: None, used: Instant::now() };
        self.offers().insert(transfer, offer);
    }

    fn get(&self, peer: &str, transfer: u64) -> Option<(Arc<Vec<LocalEntry>>, Info, usize)> {
        let mut offers = self.offers();
        let offer = offers.get_mut(&transfer).filter(|o| o.peer == peer)?;
        offer.used = Instant::now();
        Some((offer.entries.clone(), offer.info.clone(), offer.files))
    }
}

/// Nimmt Abrufe von `peer` an (bidirektionale Streams nach dem Control-Stream) und beantwortet sie,
/// solange die Verbindung besteht. `allowed`: Darf `peer` gerade kopierte Dateien bekommen?
pub(crate) async fn accept_fetches(
    conn: Connection,
    peer: String,
    offers: Offers,
    pastes: Pastes,
    allowed: impl Fn() -> bool + Clone + Send + Sync + 'static,
) {
    while let Ok((send, mut recv)) = conn.accept_bi().await {
        let (peer, offers, pastes, allowed) = (peer.clone(), offers.clone(), pastes.clone(), allowed.clone());
        tokio::spawn(async move {
            let result = match read_kind(&mut recv).await {
                Ok(StreamKind::Fetch) => serve(send, recv, &peer, &offers, &pastes, allowed()).await,
                Ok(other) => Err(anyhow::anyhow!("unexpected stream of type {other:?}")),
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                debug!("fetch from '{peer}': {e:#}");
            }
        });
    }
}

/// Antwortet auf einen Abruf (das erste Byte ist gelesen).
async fn serve(mut send: SendStream, mut recv: RecvStream, peer: &str, offers: &Offers, pastes: &Pastes, allowed: bool) -> Result<()> {
    let request: FileRequest = read_frame(&mut recv).await?.context("fetch without a request")?;
    send.set_priority(BULK_PRIORITY)?;
    if !allowed {
        return refuse(send, "sharing copied files is not allowed").await;
    }
    let Some((entries, info, files)) = offers.get(peer, request.transfer) else {
        return refuse(send, "no longer in the clipboard").await;
    };
    let Some(local) = entries.get(request.index as usize).filter(|e| !e.entry.is_dir) else {
        return refuse(send, "unknown file").await;
    };
    let Some(ticket) = pastes.enter(request.paste, request.transfer, request.index, &info, files) else {
        // Hier abgebrochen: auch die folgenden Dateien nicht mehr
        let _ = send.reset(0u32.into());
        bail!(Cancelled { by_peer: false });
    };
    let result = send_file(&mut send, local, request.offset, &ticket).await;
    match &result {
        Ok(()) => {
            debug!("'{}' delivered to '{peer}' (from byte {})", local.entry.path, request.offset);
            ticket.complete();
        }
        Err(e) => {
            if progress::cancelled(e).is_none() {
                warn!("failed to deliver '{}' to '{peer}': {e:#}", local.entry.path);
            }
            ticket.fail(phase_of(e));
        }
    }
    result
}

async fn refuse(mut send: SendStream, reason: &str) -> Result<()> {
    write_frame(&mut send, &FileReply::Unavailable { reason: reason.to_owned() }).await?;
    send.finish()?;
    bail!("refused: {reason}")
}

/// Liefert `local` ab `offset`: Antwort, Rohbytes, Prüfsumme.
async fn send_file(send: &mut SendStream, local: &LocalEntry, offset: u64, ticket: &Ticket) -> Result<()> {
    let name = &local.entry.path;
    let mut file = match tokio::fs::File::open(&local.source).await {
        Ok(file) => file,
        Err(e) => return unavailable(send, format!("'{name}' is not readable: {e}")).await,
    };
    let size = file.metadata().await?.len();
    if size != local.entry.size {
        return unavailable(send, format!("'{name}' has changed since it was copied")).await;
    }
    if offset > size {
        return unavailable(send, format!("'{name}' has only {size} bytes")).await;
    }
    file.seek(SeekFrom::Start(offset)).await?;
    write_frame(send, &FileReply::Data { size: size - offset }).await.map_err(frame_write_error)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut remaining = size - offset;
    while remaining > 0 {
        let want = remaining.min(CHUNK as u64) as usize;
        let n = file.read(&mut buf[..want]).await?;
        if n == 0 {
            bail!("'{name}' got shorter during the transfer");
        }
        hasher.update(&buf[..n]);
        // Schreiben wartet auf die Gegenstelle; „Abbrechen“ soll trotzdem sofort wirken.
        let stopped = tokio::select! {
            biased;
            () = ticket.stopped() => true,
            written = send.write_all(&buf[..n]) => {
                written.map_err(write_error)?;
                false
            }
        };
        if stopped {
            let _ = send.reset(0u32.into());
            return Err(ticket.stop_error());
        }
        ticket.add(n as u64);
        remaining -= n as u64;
    }
    write_frame(send, &FileTrailer { sha256: hasher.finalize().into() }).await.map_err(frame_write_error)?;
    send.finish()?;
    Ok(())
}

async fn unavailable(send: &mut SendStream, reason: String) -> Result<()> {
    write_frame(send, &FileReply::Unavailable { reason: reason.clone() }).await.map_err(frame_write_error)?;
    send.finish()?;
    Err(Unavailable(reason).into())
}

// ------------------------------------------------------------------ Empfänger

/// Angekündigte Dateien auf Abruf von `peer`, für die Zwischenablage des Empfängers
pub(crate) struct Offered {
    pub peer: String,
    /// Wie angekündigt (Pfade der Gegenstelle)
    pub entries: Vec<FileEntry>,
    /// Oberste Namen und Gesamtgröße (für Meldungen)
    pub names: Vec<String>,
    pub bytes: u64,
    source: Arc<SourceShared>,
}

struct SourceShared {
    runtime: Handle,
    link: Link,
    peer: String,
    transfer: u64,
    entries: Vec<FileEntry>,
    info: Info,
    files: usize,
    pastes: Pastes,
}

impl Offered {
    /// In einer Tokio-Laufzeit aufrufen. `link` liefert die Verbindung zu `peer`. Fehler bei unzulässigen
    /// Pfaden.
    pub fn new(peer: String, transfer: u64, entries: Vec<FileEntry>, link: Link, pastes: Pastes) -> Result<Self> {
        entries.iter().try_for_each(|e| sanitize(&e.path).map(drop))?;
        let (names, bytes) = summary(&entries);
        let info = Info { kind: Kind::Clipboard, peer: peer.clone(), names: names.clone(), total: bytes };
        let files = entries.iter().filter(|e| !e.is_dir).count();
        let source = Arc::new(SourceShared {
            runtime: Handle::current(),
            link,
            peer: peer.clone(),
            transfer,
            entries: entries.clone(),
            info,
            files,
            pastes,
        });
        Ok(Self { peer, entries, names, bytes, source })
    }

    /// Für `Clipboard::write_virtual_files`: bereinigte Pfade wie beim Vorab-Übertragen, die Inhalte
    /// über Abrufe. Fehler bei unzulässigen Pfaden.
    pub fn virtual_files(&self) -> Result<VirtualFiles> {
        let entries = self
            .entries
            .iter()
            .map(|e| {
                let path = sanitize(&e.path)?.iter().map(|part| part.to_string_lossy()).collect::<Vec<_>>().join("/");
                Ok(FileEntry { path, size: e.size, is_dir: e.is_dir })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(VirtualFiles { entries, source: Arc::new(Source(self.source.clone())) })
    }
}

/// Legt angekündigte Dateien auf Abruf in die Zwischenablage und liefert ihre neue Sequenznummer.
pub(crate) async fn deliver(offered: Offered, data: ClipboardData, hub: &ClipboardHub, notify: &Notify) -> Option<u64> {
    let placed = match offered.virtual_files() {
        Ok(files) => hub.write_virtual_files(files, data).await,
        Err(e) => Err(e),
    };
    let Offered { peer, names, bytes, .. } = offered;
    match placed {
        Ok(seq) => {
            info!("copied files from '{peer}' are in the clipboard for paste on demand ({})", human_size(bytes));
            notify(FilesNotice::Ready { peer, names, bytes });
            Some(seq)
        }
        Err(e) => {
            warn!("failed to put copied files from '{peer}' into the clipboard: {e:#}");
            notify(FilesNotice::Failed { peer, reason: format!("{e:#}") });
            None
        }
    }
}

/// Fragt Datei `index` ab `offset` an. Liefert den Stream mit der Antwort gelesen und die Zahl der
/// folgenden Bytes.
async fn request(source: &SourceShared, index: usize, offset: u64, paste: u64) -> Result<(RecvStream, u64)> {
    let conn = (source.link)().map_err(anyhow::Error::msg)?;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(&[StreamKind::Fetch as u8]).await?;
    write_frame(&mut send, &FileRequest { transfer: source.transfer, index: index as u32, offset, paste }).await?;
    send.finish()?;
    match read_frame::<_, FileReply>(&mut recv).await.map_err(frame_read_error)? {
        Some(FileReply::Data { size }) => {
            let expected = source.entries[index].size.saturating_sub(offset);
            ensure!(size == expected, "'{}' now has {size} instead of {expected} bytes", source.entries[index].path);
            Ok((recv, size))
        }
        Some(FileReply::Unavailable { reason }) => Err(Unavailable(reason).into()),
        None => bail!("no response from '{}'", source.peer),
    }
}

/// Liest `size` Bytes und die Prüfsumme für ein lesendes Programm (siehe [`Fetching`]).
async fn pump(recv: &mut RecvStream, size: u64, ticket: &Ticket, tx: &mpsc::Sender<Piece>) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut remaining = size;
    while remaining > 0 {
        // Ganze Stücke wie beim Vorab-Empfang: Je Paket ein Stück wäre für das lesende Programm zu klein.
        let mut piece = vec![0u8; remaining.min(CHUNK as u64) as usize];
        tokio::select! {
            biased;
            () = ticket.stopped() => {
                let _ = recv.stop(0u32.into());
                return Err(ticket.stop_error());
            }
            () = tx.closed() => {
                let _ = recv.stop(0u32.into());
                return Err(ReaderGone.into());
            }
            read = recv.read_exact(&mut piece) => read.map_err(read_error)?,
        }
        hasher.update(&piece);
        ticket.add(piece.len() as u64);
        remaining -= piece.len() as u64;
        let sent = tokio::select! {
            biased;
            () = ticket.stopped() => {
                let _ = recv.stop(0u32.into());
                return Err(ticket.stop_error());
            }
            sent = tx.send(Piece::Data(piece)) => sent,
        };
        if sent.is_err() {
            let _ = recv.stop(0u32.into());
            return Err(ReaderGone.into());
        }
    }
    // Alle Bytes sind da: die Prüfsumme auch dann noch lesen, wenn das Programm schon aufgehört hat.
    let trailer: FileTrailer = read_frame(recv).await.map_err(frame_read_error)?.context("checksum missing")?;
    ensure!(trailer.sha256 == <[u8; 32]>::from(hasher.finalize()), "checksum mismatch");
    Ok(())
}

/// Ergebnis eines Abrufs für den Einfügevorgang: ein Abbruch als solcher, sonst fehlgeschlagen
fn phase_of(error: &anyhow::Error) -> Phase {
    match progress::cancelled(error) {
        Some(c) => Phase::Cancelled { by_peer: c.by_peer },
        None => Phase::Failed(format!("{error:#}")),
    }
}

/// Fehler beim Lesen für die Plattform: ein Abbruch als solcher, sonst mit Text
fn io_error(error: &anyhow::Error) -> io::Error {
    if progress::cancelled(error).is_some() {
        return cancelled_read();
    }
    if let Some(Unavailable(reason)) = error.downcast_ref() {
        return io::Error::new(io::ErrorKind::NotFound, reason.clone());
    }
    io::Error::other(format!("{error:#}"))
}

/// Was beim lesenden Programm ankommt
enum Piece {
    Data(Vec<u8>),
    /// Ende, Prüfsumme stimmt
    End,
    Failed(io::Error),
}

/// Die Plattform holt die Dateien darüber (siehe `qross_platform::VirtualSource`).
struct Source(Arc<SourceShared>);

impl VirtualSource for Source {
    fn open(&self, index: usize) -> Box<dyn VirtualFile> {
        Box::new(Fetching { source: self.0.clone(), index, ticket: None, download: None })
    }
}

/// Eine Datei, die ein Programm gerade liest
struct Fetching {
    source: Arc<SourceShared>,
    index: usize,
    /// Ab dem ersten Lesen
    ticket: Option<Arc<Ticket>>,
    download: Option<Download>,
}

/// Ein laufender Abruf ab `position`
struct Download {
    /// Nächstes Byte für das Programm
    position: u64,
    pieces: mpsc::Receiver<Piece>,
    chunk: Vec<u8>,
    at: usize,
    task: tokio::task::AbortHandle,
    finished: bool,
}

impl VirtualFile for Fetching {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let source = self.source.clone();
        let index = self.index;
        let size = source.entries[index].size;
        let ticket = self
            .ticket
            .get_or_insert_with(|| Arc::new(source.pastes.join(source.transfer, index as u32, &source.info, source.files)))
            .clone();
        if ticket.is_cancelled() {
            return Err(cancelled_read());
        }
        if size == 0 {
            ticket.complete();
            return Ok(0);
        }
        if !self.download.as_ref().is_some_and(|d| d.position == offset) {
            if offset >= size {
                return Ok(0);
            }
            if let Some(old) = self.download.take() {
                old.task.abort();
            }
            let (tx, pieces) = mpsc::channel(PIECES);
            let task = source.runtime.spawn(download(source.clone(), index, offset, ticket, tx)).abort_handle();
            self.download = Some(Download { position: offset, pieces, chunk: Vec::new(), at: 0, task, finished: false });
        }
        let download = self.download.as_mut().expect("just started");
        loop {
            if download.at < download.chunk.len() {
                let n = (download.chunk.len() - download.at).min(buf.len());
                buf[..n].copy_from_slice(&download.chunk[download.at..download.at + n]);
                download.at += n;
                download.position += n as u64;
                return Ok(n);
            }
            if download.finished {
                return Ok(0);
            }
            match download.pieces.blocking_recv() {
                Some(Piece::Data(bytes)) => (download.chunk, download.at) = (bytes, 0),
                Some(Piece::End) => download.finished = true,
                Some(Piece::Failed(e)) => return Err(e),
                None => return Err(io::Error::other("fetch ended")),
            }
        }
    }
}

impl Drop for Fetching {
    fn drop(&mut self) {
        let (Some(ticket), Some(download)) = (&self.ticket, &self.download) else { return };
        if download.position < self.source.entries[self.index].size && !download.finished {
            // Das Programm liest nicht weiter, etwa nach „Abbrechen“ im Explorer: Der Abruf hält den
            // Stream an, und der ganze Einfügevorgang gilt als hier abgebrochen.
            download.task.abort();
            ticket.fail(Phase::Cancelled { by_peer: false });
        }
    }
}

/// Holt Datei `index` ab `offset` für ein lesendes Programm.
async fn download(source: Arc<SourceShared>, index: usize, offset: u64, ticket: Arc<Ticket>, tx: mpsc::Sender<Piece>) {
    let result = async {
        let (mut recv, size) = request(&source, index, offset, ticket.paste).await?;
        pump(&mut recv, size, &ticket, &tx).await
    }
    .await;
    let name = &source.entries[index].path;
    match result {
        Ok(()) => {
            debug!("'{name}' from '{}' pasted (from byte {offset})", source.peer);
            ticket.complete();
            let _ = tx.send(Piece::End).await;
        }
        // Darum kümmert sich `Fetching::drop`.
        Err(e) if e.is::<ReaderGone>() => {}
        Err(e) => {
            match progress::cancelled(&e) {
                Some(c) => info!("pasting '{name}' {}", if c.by_peer { "cancelled by the sender" } else { "cancelled" }),
                None => warn!("failed to paste '{name}' from '{}': {e:#}", source.peer),
            }
            ticket.fail(phase_of(&e));
            let io = io_error(&e);
            let _ = tx.send(Piece::Failed(io)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(total: u64) -> Info {
        Info { kind: Kind::Clipboard, peer: "mac".into(), names: vec!["Urlaub".into()], total }
    }

    fn phase(transfers: &Transfers, direction: Direction, paste: u64) -> Option<Phase> {
        transfers.phase(direction, paste)
    }

    #[tokio::test]
    async fn reads_of_one_paste_count_as_one_transfer_until_every_file_is_complete() {
        let transfers = Transfers::new();
        let pastes = Pastes::new(Direction::Receiving, transfers.clone());
        let first = pastes.join(7, 1, &info(30), 2);
        first.add(10);
        let second = pastes.join(7, 3, &info(30), 2);
        assert_eq!(first.paste, second.paste, "andere Datei derselben Übertragung: derselbe Vorgang");
        first.complete();
        assert_eq!(phase(&transfers, Direction::Receiving, first.paste), Some(Phase::Running));
        second.add(20);
        second.complete();
        assert_eq!(phase(&transfers, Direction::Receiving, first.paste), Some(Phase::Done));
        assert_eq!(transfers.counter(Direction::Receiving, first.paste).load(Ordering::Relaxed), 30);
    }

    #[tokio::test]
    async fn pasting_twice_at_once_gives_two_transfers() {
        let transfers = Transfers::new();
        let pastes = Pastes::new(Direction::Receiving, transfers.clone());
        let here = pastes.join(7, 1, &info(30), 2);
        let there = pastes.join(7, 1, &info(30), 2);
        assert_ne!(here.paste, there.paste, "dieselbe Datei noch einmal: ein zweites Einfügen");
        assert_eq!(pastes.join(7, 3, &info(30), 2).paste, here.paste, "die nächste Datei gehört zum ersten");
        assert_eq!(pastes.join(7, 3, &info(30), 2).paste, there.paste, "und dann zum zweiten");
        assert_ne!(pastes.join(8, 1, &info(30), 2).paste, here.paste, "andere Übertragung");
    }

    #[tokio::test]
    async fn a_cancelled_paste_refuses_the_next_files_for_a_while() {
        let transfers = Transfers::new();
        let pastes = Pastes::new(Direction::Receiving, transfers.clone());
        let first = pastes.join(7, 1, &info(30), 3);
        // „Abbrechen“ in der Oberfläche
        assert!(transfers.cancel(Direction::Receiving, first.paste));
        assert!(first.is_cancelled());
        tokio::time::timeout(Duration::from_secs(1), first.stopped()).await.expect("laufende Abrufe halten an");
        let next = pastes.join(7, 2, &info(30), 3);
        assert_eq!(next.paste, first.paste, "der Explorer macht mit der nächsten Datei weiter …");
        assert!(next.is_cancelled(), "… die gleich abgebrochen wird");
        assert!(matches!(next.stop_error().downcast_ref::<Cancelled>(), Some(Cancelled { by_peer: false })));

        // Lange nichts mehr: vergessen, ein neues Einfügen beginnt von vorn.
        drop((first, next));
        let mut state = pastes.state();
        prune(&mut state, Instant::now() + LINGER);
        assert!(state.is_empty());
    }

    #[tokio::test]
    async fn a_lost_connection_fails_the_paste_without_making_it_sticky() {
        let transfers = Transfers::new();
        let pastes = Pastes::new(Direction::Receiving, transfers.clone());
        let reading = pastes.join(7, 1, &info(30), 3);
        transfers.abort_receiving(Some("mac"), "Verbindung getrennt");
        assert_eq!(phase(&transfers, Direction::Receiving, reading.paste), Some(Phase::Failed("Verbindung getrennt".into())));
        assert_eq!(reading.stop_error().to_string(), "Verbindung getrennt");
        let next = pastes.join(7, 2, &info(30), 3);
        assert_ne!(next.paste, reading.paste, "die nächste Datei versucht es neu");
    }

    #[tokio::test]
    async fn skipped_files_end_the_paste_after_a_pause_with_what_arrived() {
        let transfers = Transfers::new();
        let pastes = Pastes::new(Direction::Receiving, transfers.clone());
        let ticket = pastes.join(7, 1, &info(100), 3);
        let paste = ticket.paste;
        ticket.add(40);
        ticket.complete();
        drop(ticket);
        tokio::time::sleep(IDLE / 2).await;
        // Kommt rechtzeitig der nächste Abruf, läuft der Vorgang weiter.
        let again = pastes.join(7, 2, &info(100), 3);
        assert_eq!(again.paste, paste);
        drop(again);
        tokio::time::sleep(IDLE * 2).await;
        assert_eq!(phase(&transfers, Direction::Receiving, paste), Some(Phase::Done), "übersprungen: fertig");
    }

    #[tokio::test]
    async fn the_sender_follows_the_receivers_numbers_and_refuses_cancelled_pastes() {
        let transfers = Transfers::new();
        let pastes = Pastes::new(Direction::Sending, transfers.clone());
        let first = pastes.enter(42, 7, 1, &info(30), 2).expect("neu");
        let second = pastes.enter(42, 7, 3, &info(30), 2).expect("derselbe Vorgang");
        assert_eq!((first.paste, second.paste), (42, 42));
        assert!(transfers.cancel(Direction::Sending, 42));
        assert!(pastes.enter(42, 7, 4, &info(30), 2).is_none(), "abgebrochen: keine weiteren Dateien");

        // Fertig und dann doch weiter (der Empfänger hat gewartet): läuft wieder
        let other = pastes.enter(43, 7, 1, &info(30), 1).expect("neu");
        other.complete();
        assert_eq!(phase(&transfers, Direction::Sending, 43), Some(Phase::Done));
        let later = pastes.enter(43, 7, 1, &info(30), 1).expect("wieder");
        assert_eq!(phase(&transfers, Direction::Sending, later.paste), Some(Phase::Running));
    }

    fn local(path: &str) -> LocalEntry {
        LocalEntry { source: path.into(), entry: FileEntry { path: path.into(), size: 1, is_dir: false } }
    }

    #[test]
    fn offers_belong_to_their_peer_and_stay_a_while_after_being_superseded() {
        let offers = Offers::default();
        offers.add(1, "mac", vec![local("a.txt")]);
        assert!(offers.get("mac", 1).is_some());
        assert!(offers.get("pc", 1).is_none(), "nur für die Gegenstelle, an die es ging");
        offers.supersede("mac");
        offers.add(2, "mac", vec![local("b.txt")]);
        assert!(offers.get("mac", 1).is_some(), "überholt, aber ein Einfügen kann noch laufen");
        assert!(offers.get("mac", 2).is_some());
        let (_, info, files) = offers.get("mac", 2).unwrap();
        assert_eq!((info.names, info.total, files), (vec!["b.txt".to_owned()], 1, 1));

        // Lange überholt und nicht mehr abgerufen: weg
        offers.offers().get_mut(&1).unwrap().superseded = Some(Instant::now() - KEEP_PREVIOUS);
        offers.offers().get_mut(&1).unwrap().used = Instant::now() - KEEP_PREVIOUS;
        offers.supersede("pc");
        assert!(offers.get("mac", 1).is_none());
        assert!(offers.get("mac", 2).is_some());
    }

    #[test]
    fn errors_reach_the_platform_as_cancelled_missing_or_failed() {
        use qross_platform::virtual_files::is_cancelled;
        assert!(is_cancelled(&io_error(&Cancelled { by_peer: true }.into())));
        assert!(is_cancelled(&io_error(&anyhow::Error::new(Cancelled { by_peer: false }).context("Datei 2"))));
        let missing = io_error(&Unavailable("„a.txt“ hat sich seit dem Kopieren geändert".into()).into());
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
        assert!(missing.to_string().contains("geändert"));
        assert_eq!(io_error(&anyhow::anyhow!("Prüfsumme stimmt nicht")).kind(), io::ErrorKind::Other);
        assert_eq!(phase_of(&Cancelled { by_peer: true }.into()), Phase::Cancelled { by_peer: true });
        assert_eq!(phase_of(&anyhow::anyhow!("weg")), Phase::Failed("weg".into()));
    }
}
