//! Dateiübertragung für Drag & Drop und für kopierte Dateien aus der Zwischenablage
//! (siehe `clipfiles`).
//!
//! Jede Datei läuft auf einem eigenen QUIC-Stream mit niedriger Priorität: `FileHeader`,
//! Rohbytes, `FileTrailer` mit SHA-256. Der Empfänger schreibt zuerst in ein Staging-
//! Verzeichnis (`*.part`, nach erfolgreicher Prüfung umbenannt) und verschiebt alles erst
//! in den Zielordner, wenn die Dateien vollständig sind **und** abgelegt wurde.
//! Eingaben bleiben dabei flüssig, weil sie auf eigenen Streams/Datagrammen mit höherer
//! Priorität laufen und höchstens vier Datei-Streams zugleich auf ihre Bestätigung warten (sonst
//! füllen sie quinns Sendefenster, und auch die Eingaben müssen warten). Fortschritt und Abbrechen:
//! siehe `progress`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use qross_proto::{read_frame, write_frame, FileEntry, FileHeader, FileTrailer, FrameError, StreamKind};
use quinn::{Connection, ReadError, ReadExactError, RecvStream, SendStream, WriteError};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::clipfiles::summary;
use crate::progress::{self, Cancelled, Direction, Info, Kind, Outgoing, Phase, Transfers};
use crate::streams::BULK_PRIORITY;

const CHUNK: usize = 256 * 1024;

/// Eine zu sendende Datei oder ein Ordner.
#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub source: PathBuf,
    pub entry: FileEntry,
}

/// Löst die gezogenen Pfade rekursiv auf. Relative Pfade beginnen mit dem Namen des gezogenen
/// Elements; symbolische Links werden übersprungen.
pub fn collect(roots: &[PathBuf]) -> Result<Vec<LocalEntry>> {
    Ok(collect_up_to(roots, u64::MAX)?.expect("ohne Grenze immer vollständig"))
}

/// Wie [`collect`], hört aber auf, sobald die Dateien zusammen größer als `max_bytes` sind
/// (`None`). So dauert auch ein versehentlich kopiertes Laufwerk nicht ewig.
pub fn collect_up_to(roots: &[PathBuf], max_bytes: u64) -> Result<Option<Vec<LocalEntry>>> {
    let mut out = Vec::new();
    let mut total = 0;
    for root in roots {
        let name = root
            .file_name()
            .with_context(|| format!("{} hat keinen Dateinamen", root.display()))?
            .to_string_lossy()
            .into_owned();
        if !walk(root, name, &mut out, &mut total, max_bytes)? {
            return Ok(None);
        }
    }
    Ok(Some(out))
}

/// `false`, sobald `max_bytes` überschritten ist.
fn walk(path: &Path, rel: String, out: &mut Vec<LocalEntry>, total: &mut u64, max_bytes: u64) -> Result<bool> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("{} lesen", path.display()))?;
    if meta.is_dir() {
        out.push(LocalEntry { source: path.to_owned(), entry: FileEntry { path: rel.clone(), size: 0, is_dir: true } });
        let mut children: Vec<_> = fs::read_dir(path)?.collect::<Result<_, _>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            if !walk(&child.path(), format!("{rel}/{}", child.file_name().to_string_lossy()), out, total, max_bytes)? {
                return Ok(false);
            }
        }
    } else if meta.is_file() {
        *total = total.saturating_add(meta.len());
        if *total > max_bytes {
            return Ok(false);
        }
        out.push(LocalEntry { source: path.to_owned(), entry: FileEntry { path: rel, size: meta.len(), is_dir: false } });
    } else {
        debug!("überspringe {} (kein normaler Eintrag)", path.display());
    }
    Ok(true)
}

/// Höchstens so viele fertig geschriebene Datei-Streams warten auf die Bestätigung der Gegenstelle;
/// mit dem, der gerade geschrieben wird, sind es vier. Ohne Grenze legt quinn bei vielen Dateien bis
/// zu seinem Sendefenster in den Puffer (10 MB je Verbindung, alle Streams zusammen). Ist es voll,
/// wartet auch jeder Tastendruck auf dem Input-Stream, bis die Gegenstelle etwas bestätigt hat –
/// gemessen bis zu 2,4 s. So bleiben es höchstens vier Empfangsfenster eines Streams (je 1,25 MB).
const UNACKED_FILES: usize = 3;

/// Sendet alle Dateien einer Übertragung nacheinander auf je einem eigenen Stream
/// (`kind`: `File` für Drag & Drop, `ClipboardFile` für die Zwischenablage) und kehrt zurück, wenn
/// die Gegenstelle alles bestätigt hat. Zählt die Bytes in `job`; bei „Abbrechen“ werden die
/// laufenden Streams zurückgesetzt und keiner mehr geöffnet.
pub async fn send_files(
    conn: Connection,
    kind: StreamKind,
    transfer: u64,
    entries: Vec<LocalEntry>,
    job: &Outgoing,
) -> Result<()> {
    let mut buf = vec![0u8; CHUNK];
    // Fertig geschrieben, aber noch nicht bestätigt, die ältesten vorn
    let mut unacked: VecDeque<SendStream> = VecDeque::new();
    let result: Result<()> = async {
        for (index, local) in entries.iter().enumerate() {
            if local.entry.is_dir {
                continue;
            }
            if unacked.len() == UNACKED_FILES {
                acknowledged(&unacked[0], job).await?;
                unacked.pop_front();
            }
            if job.is_cancelled() {
                bail!(Cancelled { by_peer: false });
            }
            let mut stream = conn.open_uni().await?;
            stream.set_priority(BULK_PRIORITY)?;
            stream.write_all(&[kind as u8]).await.map_err(write_error)?;
            let header = FileHeader { transfer, index: index as u32, path: local.entry.path.clone(), size: local.entry.size };
            write_frame(&mut stream, &header).await.map_err(frame_write_error)?;
            send_body(&mut stream, local, &mut buf, job).await?;
            unacked.push_back(stream);
        }
        while let Some(stream) = unacked.front() {
            acknowledged(stream, job).await?;
            unacked.pop_front();
        }
        Ok(())
    }
    .await;
    if result.is_err() {
        // Abgebrochen oder fehlgeschlagen: Was davon noch im Puffer liegt, nicht mehr senden
        for mut stream in unacked {
            let _ = stream.reset(0u32.into());
        }
    }
    result
}

/// Wartet, bis die Gegenstelle alles auf dem fertig geschriebenen `stream` bestätigt hat;
/// „Abbrechen“ wirkt dabei sofort.
async fn acknowledged(stream: &SendStream, job: &Outgoing) -> Result<()> {
    let stopped = stream.stopped();
    tokio::select! {
        biased;
        () = job.cancelled() => bail!(Cancelled { by_peer: false }),
        stopped = stopped => match stopped {
            Ok(None) => Ok(()),
            // Angehalten (`STOP_SENDING`): Der Empfänger hat abgebrochen oder die Übertragung verworfen.
            Ok(Some(_)) => Err(Cancelled { by_peer: true }.into()),
            Err(e) => Err(e.into()),
        },
    }
}

async fn send_body(stream: &mut SendStream, local: &LocalEntry, buf: &mut [u8], job: &Outgoing) -> Result<()> {
    let mut file = tokio::fs::File::open(&local.source)
        .await
        .with_context(|| format!("{} öffnen", local.source.display()))?;
    let mut hasher = Sha256::new();
    let mut remaining = local.entry.size;
    while remaining > 0 {
        let want = remaining.min(CHUNK as u64) as usize;
        let n = file.read(&mut buf[..want]).await?;
        if n == 0 {
            bail!("{} wurde während der Übertragung kürzer", local.source.display());
        }
        hasher.update(&buf[..n]);
        // Schreiben wartet auf die Gegenstelle (Flusskontrolle); „Abbrechen“ soll trotzdem sofort wirken.
        let cancelled = tokio::select! {
            biased;
            () = job.cancelled() => true,
            written = stream.write_all(&buf[..n]) => {
                written.map_err(write_error)?;
                false
            }
        };
        if cancelled {
            let _ = stream.reset(0u32.into());
            bail!(Cancelled { by_peer: false });
        }
        job.add(n as u64);
        remaining -= n as u64;
    }
    write_frame(stream, &FileTrailer { sha256: hasher.finalize().into() }).await.map_err(frame_write_error)?;
    stream.finish()?;
    Ok(())
}

/// Wie [`send_files`], meldet die Übertragung dabei in `transfers` an (`peer`: Empfänger), sodass sie
/// dort gemeldet und abgebrochen werden kann.
pub async fn send_tracked(
    conn: Connection,
    kind: StreamKind,
    transfer: u64,
    entries: Vec<LocalEntry>,
    peer: &str,
    transfers: &Transfers,
) -> Result<()> {
    let list: Vec<FileEntry> = entries.iter().map(|e| e.entry.clone()).collect();
    let (names, total) = summary(&list);
    let job = transfers.begin_sending(transfer, Info { kind: Kind::of(kind), peer: peer.to_owned(), names, total });
    let result = send_files(conn, kind, transfer, entries, &job).await;
    transfers.end(Direction::Sending, transfer, Phase::of(&result));
    result
}

/// Hält der Empfänger den Stream an (`STOP_SENDING`), hat er die Übertragung abgebrochen oder verworfen.
fn write_error(e: WriteError) -> anyhow::Error {
    match e {
        WriteError::Stopped(_) => Cancelled { by_peer: true }.into(),
        e => e.into(),
    }
}

fn frame_write_error(e: FrameError) -> anyhow::Error {
    if let FrameError::Io(io) = &e {
        if let Some(WriteError::Stopped(_)) = io.get_ref().and_then(|inner| inner.downcast_ref::<WriteError>()) {
            return Cancelled { by_peer: true }.into();
        }
    }
    e.into()
}

/// Setzt der Sender den Stream zurück (`RESET_STREAM`), hat er die Übertragung abgebrochen.
fn read_error(e: ReadExactError) -> anyhow::Error {
    match e {
        ReadExactError::ReadError(ReadError::Reset(_)) => Cancelled { by_peer: true }.into(),
        e => e.into(),
    }
}

fn frame_read_error(e: FrameError) -> anyhow::Error {
    if let FrameError::Io(io) = &e {
        if let Some(ReadError::Reset(_)) = io.get_ref().and_then(|inner| inner.downcast_ref::<ReadError>()) {
            return Cancelled { by_peer: true }.into();
        }
    }
    e.into()
}

/// Macht aus einem relativen Protokollpfad einen sicheren lokalen Pfad: keine absoluten Pfade,
/// kein `..`, unter Windows unzulässige Zeichen und reservierte Namen werden ersetzt.
pub fn sanitize(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            bail!("unzulässiger Pfad „{path}“");
        }
        let mut clean: String = part
            .chars()
            .map(|c| if c < ' ' || matches!(c, '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') { '_' } else { c })
            .collect();
        // Windows erlaubt keine Namen mit Punkt oder Leerzeichen am Ende
        while clean.ends_with(['.', ' ']) {
            clean.pop();
        }
        if clean.is_empty() {
            clean.push('_');
        }
        let stem = clean.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        if reserved {
            clean.insert(0, '_');
        }
        out.push(clean);
    }
    Ok(out)
}

/// Ablageort für ankommende Dateien, gemeinsam für die Stream-Leser und die Hauptschleife.
pub struct Staging {
    root: PathBuf,
    cancelled: Mutex<HashSet<u64>>,
    /// Bleibt beim Beenden erhalten (Zwischenablage: eingefügt wird oft erst später).
    keep: bool,
}

const DAY: Duration = Duration::from_secs(24 * 3600);

impl Staging {
    /// Eigenes Verzeichnis pro Lauf, wird beim Beenden gelöscht; Reste älterer Läufe (> 1 Tag)
    /// werden aufgeräumt.
    pub fn create(base: &Path) -> Result<Arc<Self>> {
        remove_older_than(base, DAY, None);
        // Zähler dazu: Mehrere Instanzen im selben Prozess (Server und Client, parallele Tests) dürfen
        // nie denselben Ordner bekommen – sonst löscht die erste beim Beenden die Dateien der anderen.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = base.join(format!("{}-{nanos}-{n}", std::process::id()));
        fs::create_dir_all(&root).with_context(|| format!("{} anlegen", root.display()))?;
        Ok(Arc::new(Self { root, cancelled: Mutex::new(HashSet::new()), keep: false }))
    }

    /// Dauerhafter Ablageort direkt in `base`, der Programmende und Neuverbindungen übersteht.
    /// Übertragungen, die älter als ein Tag sind, werden aufgeräumt.
    pub fn persistent(base: &Path) -> Result<Arc<Self>> {
        remove_older_than(base, DAY, None);
        fs::create_dir_all(base).with_context(|| format!("{} anlegen", base.display()))?;
        Ok(Arc::new(Self { root: base.to_owned(), cancelled: Mutex::new(HashSet::new()), keep: true }))
    }

    pub fn transfer_dir(&self, transfer: u64) -> PathBuf {
        self.root.join(transfer.to_string())
    }

    /// Bricht laufende Streams dieser Übertragung ab und löscht, was schon angekommen ist.
    pub fn discard(&self, transfer: u64) {
        self.cancel(transfer);
        let _ = fs::remove_dir_all(self.transfer_dir(transfer));
    }

    /// Löscht alle anderen Übertragungen, die seit `age` nicht mehr verändert wurden.
    pub fn prune(&self, age: Duration, except: u64) {
        remove_older_than(&self.root, age, Some(&self.transfer_dir(except)));
    }

    fn cancel(&self, transfer: u64) {
        self.cancelled.lock().unwrap().insert(transfer);
    }

    pub(crate) fn is_cancelled(&self, transfer: u64) -> bool {
        self.cancelled.lock().unwrap().contains(&transfer)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn remove_older_than(dir: &Path, age: Duration, except: Option<&Path>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let Some(limit) = SystemTime::now().checked_sub(age) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t <= limit);
        if old && except != Some(entry.path().as_path()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Ergebnis eines Datei-Streams.
#[derive(Debug)]
pub struct FileOutcome {
    pub transfer: u64,
    pub index: u32,
    pub result: Result<()>,
}

/// Empfängt eine Datei (das `StreamKind`-Byte ist bereits gelesen) und zählt die Bytes in `transfers`.
pub async fn receive_file(stream: &mut RecvStream, staging: &Staging, transfers: &Transfers) -> Result<FileOutcome> {
    let header: FileHeader = read_frame(stream).await?.context("Datei-Stream ohne Header")?;
    let bytes = transfers.counter(Direction::Receiving, header.transfer);
    let result = receive_body(stream, staging, &header, &bytes).await;
    Ok(FileOutcome { transfer: header.transfer, index: header.index, result })
}

/// Bricht den Stream ab, weil die Übertragung hier verworfen wurde.
fn stop(stream: &mut RecvStream) -> anyhow::Error {
    let _ = stream.stop(0u32.into());
    Cancelled { by_peer: false }.into()
}

async fn receive_body(stream: &mut RecvStream, staging: &Staging, header: &FileHeader, bytes: &AtomicU64) -> Result<()> {
    // Schon verworfen: nichts mehr anlegen
    if staging.is_cancelled(header.transfer) {
        return Err(stop(stream));
    }
    let dest = staging.transfer_dir(header.transfer).join(sanitize(&header.path)?);
    let mut part = OsString::from(dest.as_os_str());
    part.push(".part");
    let part = PathBuf::from(part);
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }

    let result: Result<()> = async {
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        let mut remaining = header.size;
        while remaining > 0 {
            if staging.is_cancelled(header.transfer) {
                return Err(stop(stream));
            }
            let n = remaining.min(CHUNK as u64) as usize;
            stream.read_exact(&mut buf[..n]).await.map_err(read_error)?;
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).await?;
            bytes.fetch_add(n as u64, Ordering::Relaxed);
            remaining -= n as u64;
        }
        file.flush().await?;
        drop(file);
        let trailer: FileTrailer = read_frame(stream).await.map_err(frame_read_error)?.context("Prüfsumme fehlt")?;
        if trailer.sha256 != <[u8; 32]>::from(hasher.finalize()) {
            bail!("Prüfsumme stimmt nicht");
        }
        tokio::fs::rename(&part, &dest).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
}

#[derive(Default)]
struct Transfer {
    entries: Option<Vec<FileEntry>>,
    done: HashSet<u32>,
    failed: Vec<String>,
    /// Abgebrochen statt fehlgeschlagen (`true`: vom Sender)
    cancelled_by_peer: Option<bool>,
    target: Option<PathBuf>,
    /// Alle Dateien sind da und als fertig gemeldet; es fehlt höchstens noch das Ablegen.
    arrived: bool,
}

impl Transfer {
    fn complete(&self) -> bool {
        self.entries.as_ref().is_some_and(|entries| {
            entries.iter().enumerate().all(|(i, e)| e.is_dir || self.done.contains(&(i as u32)))
        })
    }
}

/// Buchführung des Empfängers über gezogene Dateien. `DragStart`, `Drop` und die Datei-
/// Streams dürfen in beliebiger Reihenfolge eintreffen.
pub struct Receiver {
    staging: Arc<Staging>,
    transfers: HashMap<u64, Transfer>,
    /// Fortschritt für die Oberfläche; „Abbrechen“ verwirft über das Staging.
    progress: Transfers,
}

impl Receiver {
    pub fn new(staging: Arc<Staging>, progress: Transfers) -> Self {
        Self { staging, transfers: HashMap::new(), progress }
    }

    pub fn staging(&self) -> Arc<Staging> {
        self.staging.clone()
    }

    /// Gezogene Dateien von `peer` sind angekündigt.
    pub fn on_drag_start(&mut self, peer: &str, transfer: u64, entries: Vec<FileEntry>) {
        let (names, total) = summary(&entries);
        info!("empfange {} Einträge ({})", entries.len(), human_size(total));
        let staging = self.staging.clone();
        let info = Info { kind: Kind::Drag, peer: peer.to_owned(), names, total };
        self.progress.begin(Direction::Receiving, transfer, info, move || staging.discard(transfer));
        self.transfers.entry(transfer).or_default().entries = Some(entries);
        self.try_finish(transfer);
    }

    pub fn on_drop(&mut self, transfer: u64, target: PathBuf) {
        if self.staging.is_cancelled(transfer) {
            return info!("Übertragung {transfer} ist abgebrochen – nichts abzulegen");
        }
        info!("abgelegt in {}", target.display());
        self.transfers.entry(transfer).or_default().target = Some(target);
        self.try_finish(transfer);
    }

    pub fn on_file(&mut self, outcome: FileOutcome) {
        let id = outcome.transfer;
        if self.staging.is_cancelled(id) {
            // Abgebrochen oder schon verworfen: Was danach noch ankam, wieder wegräumen.
            self.discard(id, Phase::Cancelled { by_peer: false });
            return;
        }
        let transfer = self.transfers.entry(id).or_default();
        match outcome.result {
            Ok(()) => {
                transfer.done.insert(outcome.index);
            }
            Err(e) => match progress::cancelled(&e) {
                Some(c) => transfer.cancelled_by_peer = Some(c.by_peer),
                None => transfer.failed.push(format!("Datei {}: {e:#}", outcome.index)),
            },
        }
        self.try_finish(id);
    }

    /// Einträge einer angekündigten Übertragung, die noch aufs Ablegen wartet; `None`, wenn sie schon
    /// abgelegt, abgebrochen oder fehlgeschlagen ist (bzw. noch nicht angekündigt)
    pub fn awaiting_drop(&self, transfer: u64) -> Option<&[FileEntry]> {
        if self.staging.is_cancelled(transfer) {
            return None;
        }
        self.transfers.get(&transfer).filter(|t| t.target.is_none())?.entries.as_deref()
    }

    /// Der Cursor hat den Rechner verlassen: Übertragungen ohne Drop verwerfen.
    pub fn cancel_undropped(&mut self) {
        let undropped: Vec<u64> = self.transfers.iter().filter(|(_, t)| t.target.is_none()).map(|(id, _)| *id).collect();
        for id in undropped {
            debug!("Übertragung {id} verworfen (nicht abgelegt)");
            self.discard(id, Phase::Cancelled { by_peer: false });
        }
    }

    fn discard(&mut self, id: u64, phase: Phase) {
        self.staging.discard(id);
        self.transfers.remove(&id);
        self.progress.end(Direction::Receiving, id, phase);
    }

    fn try_finish(&mut self, id: u64) {
        let Some(transfer) = self.transfers.get_mut(&id) else { return };
        if let Some(by_peer) = transfer.cancelled_by_peer {
            info!("Übertragung {id} wurde {}", if by_peer { "vom Sender abgebrochen" } else { "abgebrochen" });
            return self.discard(id, Phase::Cancelled { by_peer });
        }
        if !transfer.failed.is_empty() {
            let reason = transfer.failed.join("; ");
            warn!("Übertragung fehlgeschlagen: {reason}");
            return self.discard(id, Phase::Failed(reason));
        }
        if !transfer.complete() {
            return;
        }
        if !transfer.arrived {
            transfer.arrived = true;
            self.progress.end(Direction::Receiving, id, Phase::Done);
        }
        if transfer.target.is_none() {
            return;
        }
        let transfer = self.transfers.remove(&id).expect("eben geprüft");
        let entries = transfer.entries.expect("vollständig");
        let target = transfer.target.expect("abgelegt");
        let source = self.staging.transfer_dir(id);
        // Dateisystemarbeit nicht im Netzwerk-Task
        tokio::task::spawn_blocking(move || match finish(&source, &target, &entries) {
            Ok(placed) => {
                for path in placed {
                    info!("abgelegt: {}", path.display());
                }
            }
            Err(e) => warn!("Ablegen in {} fehlgeschlagen: {e:#}", target.display()),
        });
    }
}

/// Verschiebt die obersten Einträge aus dem Staging in den Zielordner; vorhandene Namen werden
/// nicht überschrieben, sondern durchnummeriert („Bericht (2).pdf“).
pub fn finish(source: &Path, target: &Path, entries: &[FileEntry]) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(target).with_context(|| format!("{} anlegen", target.display()))?;
    let mut tops: Vec<&str> = Vec::new();
    for entry in entries {
        let top = entry.path.split('/').next().unwrap_or_default();
        if !tops.contains(&top) {
            tops.push(top);
        }
        if entry.is_dir {
            fs::create_dir_all(source.join(sanitize(&entry.path)?))?;
        }
    }
    let mut placed = Vec::new();
    for top in tops {
        let name = sanitize(top)?;
        let dest = unique_path(&target.join(&name));
        move_path(&source.join(&name), &dest)?;
        placed.push(dest);
    }
    let _ = fs::remove_dir_all(source);
    Ok(placed)
}

fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_owned();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unendlich viele Kandidaten")
}

/// Umbenennen, bei anderem Laufwerk kopieren und löschen.
fn move_path(src: &Path, dst: &Path) -> Result<()> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_recursive(src, dst)?;
    if src.is_dir() {
        fs::remove_dir_all(src)?;
    } else {
        fs::remove_file(src)?;
    }
    Ok(())
}

fn copy_recursive(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        for child in fs::read_dir(src)? {
            let child = child?;
            copy_recursive(&child.path(), &dst.join(child.file_name()))?;
        }
    } else {
        fs::copy(src, dst).with_context(|| format!("{} → {}", src.display(), dst.display()))?;
    }
    Ok(())
}

/// Ablage, wenn unter dem Cursor kein Ordner erkennbar ist: „Downloads/Qross“.
pub fn fallback_drop_folder() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(std::env::temp_dir).join("Qross")
}

/// Zwischenordner für gezogene Dateien, bis sie abgelegt werden
pub fn drag_staging_base() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("Qross").join("incoming")
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_escapes_and_fixes_names() {
        assert!(sanitize("../boot.ini").is_err());
        assert!(sanitize("a//b").is_err());
        assert!(sanitize("/etc/passwd").is_err());
        assert_eq!(sanitize("Ordner/Datei.txt").unwrap(), Path::new("Ordner").join("Datei.txt"));
        assert_eq!(sanitize("C:evil").unwrap(), PathBuf::from("C_evil"));
        assert_eq!(sanitize("a\\..\\b").unwrap(), PathBuf::from("a_.._b"));
        assert_eq!(sanitize("con.txt").unwrap(), PathBuf::from("_con.txt"));
        assert_eq!(sanitize("COM1").unwrap(), PathBuf::from("_COM1"));
        assert_eq!(sanitize("COMMENT.md").unwrap(), PathBuf::from("COMMENT.md"));
        assert_eq!(sanitize("name. ").unwrap(), PathBuf::from("name"));
    }

    #[test]
    fn collect_and_finish_keep_structure_and_avoid_overwrites() {
        let base = std::env::temp_dir().join(format!("qross-files-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let src = base.join("quelle");
        fs::create_dir_all(src.join("Ordner/leer")).unwrap();
        fs::write(src.join("Ordner/a.txt"), b"hallo").unwrap();
        fs::write(src.join("bild.png"), vec![7u8; 1000]).unwrap();

        let entries = collect(&[src.join("Ordner"), src.join("bild.png")]).unwrap();
        let paths: Vec<_> = entries.iter().map(|e| (e.entry.path.as_str(), e.entry.is_dir)).collect();
        assert_eq!(paths, vec![("Ordner", true), ("Ordner/a.txt", false), ("Ordner/leer", true), ("bild.png", false)]);

        // Staging so befüllen, wie es der Empfänger tun würde
        let staging = base.join("staging");
        for e in entries.iter().filter(|e| !e.entry.is_dir) {
            let dest = staging.join(sanitize(&e.entry.path).unwrap());
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::copy(&e.source, dest).unwrap();
        }
        let target = base.join("ziel");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("bild.png"), b"schon da").unwrap();

        let list: Vec<FileEntry> = entries.into_iter().map(|e| e.entry).collect();
        let placed = finish(&staging, &target, &list).unwrap();
        assert_eq!(placed, vec![target.join("Ordner"), target.join("bild (2).png")]);
        assert_eq!(fs::read(target.join("Ordner/a.txt")).unwrap(), b"hallo");
        assert!(target.join("Ordner/leer").is_dir());
        assert_eq!(fs::read(target.join("bild.png")).unwrap(), b"schon da");
        assert!(!staging.exists());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn collect_up_to_stops_at_the_limit() {
        let base = std::env::temp_dir().join(format!("qross-files-limit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("Ordner")).unwrap();
        fs::write(base.join("Ordner/a.bin"), vec![0u8; 600]).unwrap();
        fs::write(base.join("Ordner/b.bin"), vec![0u8; 600]).unwrap();
        let roots = [base.join("Ordner")];
        assert_eq!(collect_up_to(&roots, 1200).unwrap().map(|e| e.len()), Some(3));
        assert!(collect_up_to(&roots, 1199).unwrap().is_none());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn every_staging_gets_its_own_folder() {
        let base = std::env::temp_dir().join(format!("qross-staging-unique-{}", std::process::id()));
        let stagings: Vec<_> = (0..50).map(|_| Staging::create(&base).unwrap()).collect();
        let roots: HashSet<_> = stagings.iter().map(|s| s.root.clone()).collect();
        assert_eq!(roots.len(), stagings.len(), "auch bei gleicher Uhrzeit verschiedene Ordner");
        drop(stagings);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn persistent_staging_survives_and_prunes() {
        let base = std::env::temp_dir().join(format!("qross-staging-keep-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let staging = Staging::persistent(&base).unwrap();
        for id in [1, 2] {
            fs::create_dir_all(staging.transfer_dir(id)).unwrap();
        }
        staging.prune(Duration::ZERO, 2);
        assert!(!staging.transfer_dir(1).exists() && staging.transfer_dir(2).exists());
        staging.discard(2);
        assert!(!staging.transfer_dir(2).exists());
        fs::create_dir_all(staging.transfer_dir(3)).unwrap();
        drop(staging);
        assert!(base.join("3").exists(), "bleibt nach dem Beenden erhalten");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_drag_awaits_its_drop_until_dropped_or_cancelled() {
        let base = std::env::temp_dir().join(format!("qross-awaiting-drop-{}", std::process::id()));
        let transfers = Transfers::new();
        let mut receiver = Receiver::new(Staging::create(&base).unwrap(), transfers.clone());
        let entries = vec![FileEntry { path: "Bericht.pdf".into(), size: 10, is_dir: false }];
        assert!(receiver.awaiting_drop(1).is_none(), "noch nicht angekündigt");
        receiver.on_drag_start("pc", 1, entries.clone());
        assert_eq!(receiver.awaiting_drop(1), Some(entries.as_slice()));
        receiver.on_drop(1, base.join("ziel"));
        assert!(receiver.awaiting_drop(1).is_none(), "abgelegt");

        // Abgebrochen (Knopf „Abbrechen“, Rechte entzogen, Gegenstelle fort)
        receiver.on_drag_start("pc", 2, entries.clone());
        assert!(transfers.cancel(Direction::Receiving, 2));
        assert!(receiver.awaiting_drop(2).is_none());
        // `Drop` kam vor `DragStart`
        receiver.on_drop(3, base.join("ziel"));
        receiver.on_drag_start("pc", 3, entries);
        assert!(receiver.awaiting_drop(3).is_none());
        drop(receiver);
        let _ = fs::remove_dir_all(&base);
    }

    /// So lange steht der Empfänger in `many_files_leave_room_for_keystrokes` still
    const STALL: Duration = Duration::from_millis(400);

    /// Empfänger in einem eigenen Thread: liest alle Streams und zählt die Bytes. Sobald der dritte
    /// Stream da ist, steht er `STALL` lang ganz still, wie ein Laptop im Funkloch: Nichts wird
    /// bestätigt, der Sender muss mit seinem Puffer auskommen.
    fn stalling_receiver(config: quinn::ServerConfig) -> (std::net::SocketAddr, Arc<AtomicU64>) {
        let received = Arc::new(AtomicU64::new(0));
        let (addr_tx, addr) = std::sync::mpsc::channel();
        let counted = received.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
                addr_tx.send(endpoint.local_addr().unwrap()).unwrap();
                let conn = endpoint.accept().await.unwrap().await.unwrap();
                let mut streams = 0;
                while let Ok(mut stream) = conn.accept_uni().await {
                    streams += 1;
                    if streams == 3 {
                        std::thread::sleep(STALL);
                    }
                    let counted = counted.clone();
                    tokio::spawn(async move {
                        if let Ok(data) = stream.read_to_end(usize::MAX).await {
                            counted.fetch_add(data.len() as u64, Ordering::Relaxed);
                        }
                    });
                }
            });
        });
        (addr.recv().unwrap(), received)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn many_files_leave_room_for_keystrokes() {
        // Sendefenster auf 256 KB verkleinert: 200 Dateien zu 32 KB füllten es ohne Grenze, sobald
        // nichts mehr bestätigt wird; so liegen höchstens vier davon im Puffer.
        let base = std::env::temp_dir().join(format!("qross-files-room-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("Fotos")).unwrap();
        for i in 0..200u32 {
            fs::write(base.join(format!("Fotos/{i:03}.jpg")), vec![i as u8; 32 * 1024]).unwrap();
        }
        let entries = collect(&[base.join("Fotos")]).unwrap();
        let (server_id, client_id) = (crate::Identity::generate().unwrap(), crate::Identity::generate().unwrap());
        let (addr, received) = stalling_receiver(crate::tls::server_config(&server_id).unwrap());
        let mut config = crate::tls::client_config(&client_id).unwrap();
        let mut transport = crate::tls::transport();
        transport.send_window(256 * 1024);
        config.transport_config(Arc::new(transport));
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(config);
        let sender = endpoint.connect(addr, crate::tls::SERVER_NAME).unwrap().await.unwrap();

        let transfers = Transfers::new();
        let info = Info { kind: Kind::Drag, peer: "laptop".into(), names: vec!["Fotos".into()], total: 200 * 32 * 1024 };
        let job = transfers.begin_sending(1, info);
        let mut keys = sender.open_uni().await.unwrap();
        keys.set_priority(10).unwrap();
        // Wie im Server: Dateien und Eingaben schreiben in eigenen Tasks.
        let send = tokio::spawn({
            let sender = sender.clone();
            async move { send_files(sender, StreamKind::File, 1, entries, &job).await }
        });
        // Währenddessen jede Millisekunde ein „Tastendruck“: Er muss sofort in den Puffer passen.
        let (mut presses, mut slowest) = (0, Duration::ZERO);
        while !send.is_finished() {
            let started = std::time::Instant::now();
            keys.write_all(&[0x04; 16]).await.unwrap();
            slowest = slowest.max(started.elapsed());
            presses += 1;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        send.await.unwrap().unwrap();
        assert!(presses > 0);
        assert!(slowest < STALL / 4, "ein Tastendruck wartete {} ms auf Platz im Puffer", slowest.as_millis());
        // Bestätigt heißt angekommen: Der Empfänger kann alles lesen.
        for _ in 0..500 {
            if received.load(Ordering::Relaxed) >= 200 * 32 * 1024 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(received.load(Ordering::Relaxed) >= 200 * 32 * 1024, "nicht alles angekommen");
        sender.close(0u32.into(), b"");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1_500_000), "1.5 MB");
        assert_eq!(human_size(4_700_000_000), "4.7 GB");
    }
}
