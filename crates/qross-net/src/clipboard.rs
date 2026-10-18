//! Zwischenablage in einem eigenen Thread. Große Bilder zu lesen und als PNG zu kodieren
//! dauert spürbar; liefe das in der Ereignisschleife, würde die Maus kurz hängen.

use std::sync::mpsc;

use anyhow::{anyhow, Result};
use qross_platform::Clipboard;
use qross_proto::ClipboardData;
use tokio::sync::oneshot;

enum Job {
    /// Aktuelle Sequenznummer und – nur wenn sie sich gegenüber `known` geändert hat – der Inhalt
    Changed { known: Option<u64>, reply: oneshot::Sender<(u64, Option<ClipboardData>)> },
    /// Inhalt setzen; Antwort ist die neue Sequenznummer
    Write { data: ClipboardData, reply: oneshot::Sender<Result<u64>> },
    Sequence { reply: oneshot::Sender<u64> },
}

#[derive(Clone)]
pub struct ClipboardHub {
    jobs: mpsc::Sender<Job>,
}

impl ClipboardHub {
    /// Der Thread endet, sobald der letzte `ClipboardHub` verworfen wird.
    pub fn spawn(mut clipboard: Box<dyn Clipboard>) -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("qross-clipboard".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Changed { known, reply } => {
                            let seq = clipboard.sequence();
                            let data = if known == Some(seq) { None } else { clipboard.read() };
                            let _ = reply.send((seq, data));
                        }
                        Job::Write { data, reply } => {
                            let result = clipboard.write(&data).map(|()| clipboard.sequence());
                            let _ = reply.send(result);
                        }
                        Job::Sequence { reply } => {
                            let _ = reply.send(clipboard.sequence());
                        }
                    }
                }
            })
            .expect("Clipboard-Thread starten");
        Self { jobs }
    }

    pub async fn changed_since(&self, known: Option<u64>) -> (u64, Option<ClipboardData>) {
        let (reply, rx) = oneshot::channel();
        let _ = self.jobs.send(Job::Changed { known, reply });
        rx.await.unwrap_or((known.unwrap_or_default(), None))
    }

    pub async fn write(&self, data: ClipboardData) -> Result<u64> {
        let (reply, rx) = oneshot::channel();
        self.jobs.send(Job::Write { data, reply }).map_err(|_| anyhow!("Clipboard-Thread beendet"))?;
        rx.await.map_err(|_| anyhow!("Clipboard-Thread beendet"))?
    }

    pub async fn sequence(&self) -> u64 {
        let (reply, rx) = oneshot::channel();
        let _ = self.jobs.send(Job::Sequence { reply });
        rx.await.unwrap_or_default()
    }
}
