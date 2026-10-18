//! Dateiübertragung für Drag & Drop.
//!
//! Jede Datei läuft auf einem eigenen QUIC-Stream mit niedriger Priorität: `FileHeader`,
//! Rohbytes, `FileTrailer` mit SHA-256. Der Empfänger schreibt zuerst in ein Staging-
//! Verzeichnis (`*.part`, nach erfolgreicher Prüfung umbenannt) und verschiebt alles erst
//! in den Zielordner, wenn die Dateien vollständig sind **und** abgelegt wurde.
//! Eingaben bleiben dabei flüssig, weil sie auf eigenen Streams/Datagrammen mit höherer
//! Priorität laufen.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use qross_proto::{read_frame, write_frame, FileEntry, FileHeader, FileTrailer, StreamKind};
use quinn::{Connection, RecvStream};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, info, warn};

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
    let mut out = Vec::new();
    for root in roots {
        let name = root
            .file_name()
            .with_context(|| format!("{} hat keinen Dateinamen", root.display()))?
            .to_string_lossy()
            .into_owned();
        walk(root, name, &mut out)?;
    }
    Ok(out)
}

fn walk(path: &Path, rel: String, out: &mut Vec<LocalEntry>) -> Result<()> {
    let meta = fs::symlink_metadata(path).with_context(|| format!("{} lesen", path.display()))?;
    if meta.is_dir() {
        out.push(LocalEntry { source: path.to_owned(), entry: FileEntry { path: rel.clone(), size: 0, is_dir: true } });
        let mut children: Vec<_> = fs::read_dir(path)?.collect::<Result<_, _>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            walk(&child.path(), format!("{rel}/{}", child.file_name().to_string_lossy()), out)?;
        }
    } else if meta.is_file() {
        out.push(LocalEntry { source: path.to_owned(), entry: FileEntry { path: rel, size: meta.len(), is_dir: false } });
    } else {
        debug!("überspringe {} (kein normaler Eintrag)", path.display());
    }
    Ok(())
}

/// Sendet alle Dateien einer Übertragung nacheinander auf je einem eigenen Stream.
pub async fn send_files(conn: Connection, transfer: u64, entries: Vec<LocalEntry>) -> Result<()> {
    let mut buf = vec![0u8; CHUNK];
    for (index, local) in entries.iter().enumerate() {
        if local.entry.is_dir {
            continue;
        }
        let mut stream = conn.open_uni().await?;
        stream.set_priority(BULK_PRIORITY)?;
        stream.write_all(&[StreamKind::File as u8]).await?;
        let header = FileHeader { transfer, index: index as u32, path: local.entry.path.clone(), size: local.entry.size };
        write_frame(&mut stream, &header).await?;

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
            stream.write_all(&buf[..n]).await?;
            remaining -= n as u64;
        }
        write_frame(&mut stream, &FileTrailer { sha256: hasher.finalize().into() }).await?;
        stream.finish()?;
    }
    Ok(())
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

/// Gemeinsamer Zustand zwischen den Stream-Lesern und der Client-Hauptschleife.
pub struct Staging {
    root: PathBuf,
    cancelled: Mutex<HashSet<u64>>,
}

impl Staging {
    /// Eigenes Verzeichnis pro Lauf; Reste älterer Läufe (> 1 Tag) werden aufgeräumt.
    pub fn create(base: &Path) -> Result<Arc<Self>> {
        if let Ok(entries) = fs::read_dir(base) {
            let day_ago = SystemTime::now() - Duration::from_secs(24 * 3600);
            for entry in entries.flatten() {
                let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < day_ago);
                if old {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let root = base.join(format!("{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).with_context(|| format!("{} anlegen", root.display()))?;
        Ok(Arc::new(Self { root, cancelled: Mutex::new(HashSet::new()) }))
    }

    pub fn transfer_dir(&self, transfer: u64) -> PathBuf {
        self.root.join(transfer.to_string())
    }

    fn cancel(&self, transfer: u64) {
        self.cancelled.lock().unwrap().insert(transfer);
    }

    fn is_cancelled(&self, transfer: u64) -> bool {
        self.cancelled.lock().unwrap().contains(&transfer)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Ergebnis eines Datei-Streams.
#[derive(Debug)]
pub struct FileOutcome {
    pub transfer: u64,
    pub index: u32,
    pub result: Result<()>,
}

/// Empfängt eine Datei (das `StreamKind`-Byte ist bereits gelesen).
pub async fn receive_file(stream: &mut RecvStream, staging: &Staging) -> Result<FileOutcome> {
    let header: FileHeader = read_frame(stream).await?.context("Datei-Stream ohne Header")?;
    let result = receive_body(stream, staging, &header).await;
    Ok(FileOutcome { transfer: header.transfer, index: header.index, result })
}

async fn receive_body(stream: &mut RecvStream, staging: &Staging, header: &FileHeader) -> Result<()> {
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
                let _ = stream.stop(0u32.into());
                bail!("abgebrochen");
            }
            let n = remaining.min(CHUNK as u64) as usize;
            stream.read_exact(&mut buf[..n]).await?;
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).await?;
            remaining -= n as u64;
        }
        file.flush().await?;
        drop(file);
        let trailer: FileTrailer = read_frame(stream).await?.context("Prüfsumme fehlt")?;
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
    target: Option<PathBuf>,
}

impl Transfer {
    fn complete(&self) -> bool {
        self.entries.as_ref().is_some_and(|entries| {
            entries.iter().enumerate().all(|(i, e)| e.is_dir || self.done.contains(&(i as u32)))
        })
    }
}

/// Buchführung des Clients über laufende Übertragungen. `DragStart`, `Drop` und die Datei-
/// Streams dürfen in beliebiger Reihenfolge eintreffen.
pub struct Receiver {
    staging: Arc<Staging>,
    transfers: HashMap<u64, Transfer>,
}

impl Receiver {
    pub fn new(staging: Arc<Staging>) -> Self {
        Self { staging, transfers: HashMap::new() }
    }

    pub fn staging(&self) -> Arc<Staging> {
        self.staging.clone()
    }

    pub fn on_drag_start(&mut self, transfer: u64, entries: Vec<FileEntry>) {
        let total: u64 = entries.iter().map(|e| e.size).sum();
        info!("empfange {} Einträge ({})", entries.len(), human_size(total));
        self.transfers.entry(transfer).or_default().entries = Some(entries);
        self.try_finish(transfer);
    }

    pub fn on_drop(&mut self, transfer: u64, target: PathBuf) {
        info!("abgelegt in {}", target.display());
        self.transfers.entry(transfer).or_default().target = Some(target);
        self.try_finish(transfer);
    }

    pub fn on_file(&mut self, outcome: FileOutcome) {
        let transfer = self.transfers.entry(outcome.transfer).or_default();
        match outcome.result {
            Ok(()) => {
                transfer.done.insert(outcome.index);
            }
            Err(e) => transfer.failed.push(format!("Datei {}: {e:#}", outcome.index)),
        }
        self.try_finish(outcome.transfer);
    }

    /// Der Cursor hat den Rechner verlassen: Übertragungen ohne Drop verwerfen.
    pub fn cancel_undropped(&mut self) {
        let undropped: Vec<u64> = self.transfers.iter().filter(|(_, t)| t.target.is_none()).map(|(id, _)| *id).collect();
        for id in undropped {
            debug!("Übertragung {id} verworfen (nicht abgelegt)");
            self.staging.cancel(id);
            self.transfers.remove(&id);
            let _ = fs::remove_dir_all(self.staging.transfer_dir(id));
        }
    }

    fn try_finish(&mut self, id: u64) {
        let Some(transfer) = self.transfers.get(&id) else { return };
        if !transfer.failed.is_empty() {
            warn!("Übertragung fehlgeschlagen: {}", transfer.failed.join("; "));
            self.staging.cancel(id);
            self.transfers.remove(&id);
            let _ = fs::remove_dir_all(self.staging.transfer_dir(id));
            return;
        }
        if transfer.target.is_none() || !transfer.complete() {
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
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1_500_000), "1.5 MB");
        assert_eq!(human_size(4_700_000_000), "4.7 GB");
    }
}
