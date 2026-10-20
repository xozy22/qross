//! Zwischenablage in einem eigenen Thread. Große Bilder zu lesen und als PNG zu kodieren
//! dauert spürbar; liefe das in der Ereignisschleife, würde die Maus kurz hängen.
//!
//! Inhalte, die ein Passwortmanager als vertraulich markiert (`qross_platform::concealed`),
//! verlassen den Rechner nicht: Für Gegenstellen sieht es aus, als hätte sich nichts geändert.
//! Ihr Stand bleibt, wie er war, und sie schicken ihn auch nicht zurück; der nächste normale
//! Inhalt wird wieder geteilt.

use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::{anyhow, Result};
use qross_platform::{Clipboard, VirtualFiles};
use qross_proto::ClipboardData;
use tokio::sync::oneshot;
use tracing::info;

/// Inhalt der lokalen Zwischenablage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Data(ClipboardData),
    /// Kopierte Dateien und Ordner (lokale Pfade) und was sonst noch dabeiliegt
    Files { paths: Vec<PathBuf>, data: ClipboardData },
}

enum Job {
    /// Aktuelle Sequenznummer und – nur wenn sie sich gegenüber `known` geändert hat – der Inhalt
    Changed { known: Option<u64>, reply: oneshot::Sender<(u64, Option<Content>)> },
    /// Inhalt setzen; Antwort ist die neue Sequenznummer
    Write { data: ClipboardData, reply: oneshot::Sender<Result<u64>> },
    WriteFiles { files: Vec<PathBuf>, data: ClipboardData, reply: oneshot::Sender<Result<u64>> },
    WriteVirtual { files: VirtualFiles, data: ClipboardData, reply: oneshot::Sender<Result<u64>> },
    Sequence { reply: oneshot::Sender<u64> },
}

#[derive(Clone)]
pub struct ClipboardHub {
    jobs: mpsc::Sender<Job>,
    /// Kopierte Dateien kommen hier erst beim Einfügen (siehe `Clipboard::pastes_on_demand`).
    pastes_on_demand: bool,
}

impl ClipboardHub {
    /// Der Thread endet, sobald der letzte `ClipboardHub` verworfen wird. `share_concealed`:
    /// vertraulich markierte Inhalte trotzdem teilen; sonst ruft der Thread `on_withheld` einmal
    /// je zurückgehaltenem Inhalt (für einen Hinweis in der Oberfläche).
    pub fn spawn(
        mut clipboard: Box<dyn Clipboard>,
        share_concealed: bool,
        on_withheld: impl Fn() + Send + 'static,
    ) -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        let pastes_on_demand = clipboard.pastes_on_demand();
        let mut guard = Guard { share: share_concealed, withheld: None, on_withheld: Box::new(on_withheld) };
        std::thread::Builder::new()
            .name("qross-clipboard".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Changed { known, reply } => {
                            let seq = clipboard.sequence();
                            let content = if known == Some(seq) { None } else { guard.read(&mut *clipboard, seq) };
                            let _ = reply.send((seq, content));
                        }
                        Job::Write { data, reply } => {
                            let result = clipboard.write(&data).map(|()| clipboard.sequence());
                            let _ = reply.send(result);
                        }
                        Job::WriteFiles { files, data, reply } => {
                            let result = clipboard.write_files(&files, &data).map(|()| clipboard.sequence());
                            let _ = reply.send(result);
                        }
                        Job::WriteVirtual { files, data, reply } => {
                            let result = clipboard.write_virtual_files(files, &data).map(|()| clipboard.sequence());
                            let _ = reply.send(result);
                        }
                        Job::Sequence { reply } => {
                            let _ = reply.send(clipboard.sequence());
                        }
                    }
                }
            })
            .expect("failed to spawn clipboard thread");
        Self { jobs, pastes_on_demand }
    }

    /// Kopierte Dateien kommen hier erst beim Einfügen (siehe `Clipboard::pastes_on_demand`).
    pub fn pastes_on_demand(&self) -> bool {
        self.pastes_on_demand
    }

    pub async fn changed_since(&self, known: Option<u64>) -> (u64, Option<Content>) {
        let (reply, rx) = oneshot::channel();
        let _ = self.jobs.send(Job::Changed { known, reply });
        rx.await.unwrap_or((known.unwrap_or_default(), None))
    }

    pub async fn write(&self, data: ClipboardData) -> Result<u64> {
        let (reply, rx) = oneshot::channel();
        self.jobs.send(Job::Write { data, reply }).map_err(|_| anyhow!("clipboard thread ended"))?;
        rx.await.map_err(|_| anyhow!("clipboard thread ended"))?
    }

    /// Legt Dateien (und `data`) in die Zwischenablage; Antwort ist die neue Sequenznummer.
    pub async fn write_files(&self, files: Vec<PathBuf>, data: ClipboardData) -> Result<u64> {
        let (reply, rx) = oneshot::channel();
        self.jobs.send(Job::WriteFiles { files, data, reply }).map_err(|_| anyhow!("clipboard thread ended"))?;
        rx.await.map_err(|_| anyhow!("clipboard thread ended"))?
    }

    /// Legt Dateien auf Abruf (und `data`) in die Zwischenablage; Antwort ist die neue Sequenznummer.
    pub async fn write_virtual_files(&self, files: VirtualFiles, data: ClipboardData) -> Result<u64> {
        let (reply, rx) = oneshot::channel();
        self.jobs.send(Job::WriteVirtual { files, data, reply }).map_err(|_| anyhow!("clipboard thread ended"))?;
        rx.await.map_err(|_| anyhow!("clipboard thread ended"))?
    }

    pub async fn sequence(&self) -> u64 {
        let (reply, rx) = oneshot::channel();
        let _ = self.jobs.send(Job::Sequence { reply });
        rx.await.unwrap_or_default()
    }
}

/// Hält vertraulich markierte Inhalte zurück.
struct Guard {
    share: bool,
    /// Sequenznummer des zuletzt zurückgehaltenen Inhalts: ein Hinweis je Inhalt, auch wenn ihn
    /// mehrere Clients nacheinander abfragen
    withheld: Option<u64>,
    on_withheld: Box<dyn Fn() + Send>,
}

impl Guard {
    /// Inhalt zur Sequenznummer `seq`; `None` auch, wenn er vertraulich ist.
    fn read(&mut self, clipboard: &mut dyn Clipboard, seq: u64) -> Option<Content> {
        if self.share {
            return read(clipboard);
        }
        // Vorher prüfen erspart das Lesen großer Inhalte; danach noch einmal, falls der
        // Passwortmanager genau dazwischen kopiert hat.
        let before = clipboard.concealed();
        let content = if before.is_none() { read(clipboard) } else { None };
        match before.or_else(|| clipboard.concealed()) {
            Some(marker) => {
                self.withhold(seq, marker);
                None
            }
            None => content,
        }
    }

    fn withhold(&mut self, seq: u64, marker: &str) {
        if self.withheld != Some(seq) {
            self.withheld = Some(seq);
            info!("clipboard: confidential content (marked with '{marker}') is not shared");
            (self.on_withheld)();
        }
    }
}

/// Dateien zuerst: Wer im Dateimanager kopiert, will Dateien einfügen.
fn read(clipboard: &mut dyn Clipboard) -> Option<Content> {
    let paths = clipboard.files();
    let data = clipboard.read();
    if paths.is_empty() {
        data.map(Content::Data)
    } else {
        Some(Content::Files { paths, data: data.unwrap_or_default() })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use qross_platform::dryrun::SharedClipboard;

    use super::*;

    /// Hub über einer Test-Zwischenablage; zählt die Hinweise auf zurückgehaltene Inhalte.
    fn hub(share_concealed: bool) -> (ClipboardHub, SharedClipboard, Arc<AtomicUsize>) {
        let (clipboard, withheld) = (SharedClipboard::default(), Arc::new(AtomicUsize::new(0)));
        let count = withheld.clone();
        let hub = ClipboardHub::spawn(Box::new(clipboard.clone()), share_concealed, move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        (hub, clipboard, withheld)
    }

    /// Schreibt wie ein Passwortmanager: Inhalt plus Markierung
    fn copy_secret(clipboard: &SharedClipboard, data: &ClipboardData, marker: &'static str) {
        let mut inner = clipboard.0.lock().unwrap();
        inner.write(data).unwrap();
        inner.conceal(marker);
    }

    #[tokio::test]
    async fn concealed_content_is_withheld_until_the_next_normal_one() {
        let (hub, clipboard, withheld) = hub(false);
        clipboard.clone().set_text("vorher").unwrap();
        let (seq, content) = hub.changed_since(None).await;
        assert_eq!(content, Some(Content::Data(ClipboardData::from_text("vorher"))));

        copy_secret(&clipboard, &ClipboardData::from_text("Passwort"), "ExcludeClipboardContentFromMonitorProcessing");
        let (secret, content) = hub.changed_since(Some(seq)).await;
        assert_ne!(secret, seq, "die neue Sequenznummer gilt als abgeglichen");
        assert_eq!(content, None);
        // Ein zweiter Client fragt nach: wieder nichts, aber kein zweiter Hinweis
        assert_eq!(hub.changed_since(Some(seq)).await, (secret, None));
        assert_eq!(hub.changed_since(Some(secret)).await, (secret, None));
        assert_eq!(withheld.load(Ordering::SeqCst), 1);

        clipboard.clone().set_text("danach").unwrap();
        let (_, content) = hub.changed_since(Some(secret)).await;
        assert_eq!(content, Some(Content::Data(ClipboardData::from_text("danach"))));
        assert_eq!(withheld.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concealed_files_are_withheld_too() {
        let (hub, clipboard, withheld) = hub(false);
        {
            let mut inner = clipboard.0.lock().unwrap();
            inner.write_files(&[PathBuf::from("/tmp/Tresor.kdbx")], &ClipboardData::default()).unwrap();
            inner.conceal("org.nspasteboard.ConcealedType");
        }
        assert_eq!(hub.changed_since(None).await.1, None);
        assert_eq!(withheld.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concealed_content_is_shared_when_allowed() {
        let (hub, clipboard, withheld) = hub(true);
        copy_secret(&clipboard, &ClipboardData::from_text("Passwort"), "x-kde-passwordManagerHint");
        assert_eq!(hub.changed_since(None).await.1, Some(Content::Data(ClipboardData::from_text("Passwort"))));
        assert_eq!(withheld.load(Ordering::SeqCst), 0);
    }
}
