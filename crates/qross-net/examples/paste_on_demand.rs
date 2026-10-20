//! Kopierte Dateien auf Abruf von Ende zu Ende auf einem Windows-Rechner: ein Server mit der echten
//! Zwischenablage (wie der Windows-PC) und ein Client im selben Prozess (wie der Mac), verbunden über QUIC
//! auf Loopback. Am Client liegt eine große Datei in der Zwischenablage (Standard 1,2 GB); der Cursor geht
//! hinüber und zurück, und ein zweiter Prozess fügt sie wie Strg+V im Explorer ein. Danach dasselbe mit
//! einem Abbruch nach etwa einem Drittel. Die Zwischenablage wird kurz ersetzt; ihr Text kommt danach
//! wieder hinein.
//!
//! ```text
//! cargo run --release -p qross-net --example paste_on_demand [MB]
//! ```

#[cfg(not(windows))]
fn main() {
    println!("nur unter Windows");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    if let (Some("einfuegen"), Some(folder)) = (std::env::args().nth(1).as_deref(), std::env::args().nth(2)) {
        return qross_platform::paste_clipboard_into(std::path::Path::new(&folder));
    }
    tokio::runtime::Runtime::new()?.block_on(windows_check::run())
}

#[cfg(windows)]
mod windows_check {
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use anyhow::{bail, Context, Result};
    use qross_core::{CaptureEvent, Layout, LinkConfig};
    use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
    use qross_net::clipfiles::{self, FilesNotice};
    use qross_net::files::human_size;
    use qross_net::progress::{Direction, Phase, Progress, Transfers};
    use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
    use qross_net::{Identity, TrustStore};
    use qross_platform::dryrun::{CaptureCall, RecordingCapture, RecordingInjector, SharedClipboard};
    use qross_platform::Clipboard;
    use qross_proto::{ClipboardData, Edge, GameState, Rect};
    use sha2::{Digest, Sha256};
    use tokio::sync::mpsc::UnboundedReceiver;

    const NAME: &str = "Qross Film Ä.bin";

    pub async fn run() -> Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
            .init();
        let mb: u64 = std::env::args().nth(1).map(|a| a.parse()).transpose()?.unwrap_or(1200);
        let base = std::env::temp_dir().join(format!("qross-auf-abruf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base)?;
        let source = base.join(NAME);
        let started = Instant::now();
        let expected = write_source(&source, mb * 1_000_000)?;
        println!("Quelle: {} ({}) in {:.1} s angelegt", source.display(), human_size(mb * 1_000_000), started.elapsed().as_secs_f64());

        let mut real = qross_platform::clipboard();
        let backup = real.read();
        anyhow::ensure!(real.pastes_on_demand(), "diese Zwischenablage fügt nicht auf Abruf ein (QROSS_PASTE_ON_DEMAND=0?)");

        // Server „pc“ mit der echten Zwischenablage, Client „laptop“ mit der Datei in seiner
        let (server_id, client_id) = (Identity::generate()?, Identity::generate()?);
        let mut trust = TrustStore::in_memory();
        trust.trust("laptop", &client_id.fingerprint())?;
        let layout = Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }])?;
        let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse()?, layout };
        let (notice_tx, mut notices) = tokio::sync::mpsc::unbounded_channel();
        let server = Server::bind(config, &server_id, trust)?.with_notices(notice_tx).with_discovery(false);
        let (addr, server_transfers) = (server.local_addr()?, server.transfers());
        let capture = RecordingCapture::default();
        let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let server_task = tokio::spawn(server.run(ServerPlatform {
            capture: Arc::new(capture.clone()),
            events: events_rx,
            clipboard: qross_platform::clipboard(),
            screen: Box::new(|| Rect::new(0, 0, 1920, 1080)),
            monitors: Box::new(Vec::new),
            game: Box::new(GameState::default),
            finder: None,
        }));
        let mut laptop_clip = SharedClipboard::default();
        let (client_tx, mut client_notices) = tokio::sync::mpsc::unbounded_channel();
        let client_task = tokio::spawn(client::run_with_transfers(
            ClientConfig {
                name: "laptop".into(),
                server: addr.to_string(),
                server_fingerprint: Some(server_id.fingerprint()),
                clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
                share_concealed: false,
            },
            client_id,
            TrustStore::in_memory(),
            ClientPlatform {
                injector: Box::new(RecordingInjector::default()),
                clipboard: Box::new(laptop_clip.clone()),
                screen: Box::new(|| Rect::new(0, 0, 1512, 982)),
                monitors: Box::new(Vec::new),
                game: Box::new(GameState::default),
                finder: None,
            },
            client_tx,
            Transfers::new(),
        ));
        loop {
            if let ServerNotice::ClientConnected { .. } = notices.recv().await.context("Server beendet")? {
                break;
            }
        }
        // Beide Seiten melden sich gleich nach dem Welcome, wer auf Abruf einfügt.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Cursor zum Laptop, dort kopieren und zurück: Er gibt seine Zwischenablage mit.
        for _ in 0..200 {
            events.send(CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false })?;
            tokio::time::sleep(Duration::from_millis(25)).await;
            if capture.0.lock().unwrap().contains(&CaptureCall::Remote) {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        laptop_clip.write_files(std::slice::from_ref(&source), &ClipboardData::from_text(NAME))?;
        let switched = Instant::now();
        events.send(CaptureEvent::MouseDelta { dx: -5000, dy: 0 })?;
        loop {
            let notice = tokio::time::timeout(Duration::from_secs(30), notices.recv()).await.context("nichts zum Einfügen")?;
            match notice.context("Server beendet")? {
                ServerNotice::ClipboardFiles(FilesNotice::Ready { names, bytes, .. }) => {
                    println!(
                        "bereit zum Einfügen nach {:.0} ms: {names:?} ({})",
                        switched.elapsed().as_secs_f64() * 1000.0,
                        human_size(bytes)
                    );
                    break;
                }
                ServerNotice::ClipboardFiles(FilesNotice::Failed { reason, .. }) => bail!("nicht abgelegt: {reason}"),
                ServerNotice::Transfer(p) => bail!("vor dem Einfügen übertragen: {p:?}"),
                _ => {}
            }
        }

        // Einfügen wie Strg+V im Explorer, in einem zweiten Prozess
        let target = base.join("eingefügt");
        std::fs::create_dir_all(&target)?;
        let paste = paste_in_background(&target)?;
        let end = watch(&mut notices, None).await?;
        let status = tokio::task::spawn_blocking(move || paste.wait_with_output()).await??;
        println!("Einfügen beendet ({}), Fortschritt am Ende: {:?}", status.status, end.phase);
        let pasted = target.join(NAME);
        let got = hash(&pasted)?;
        println!("eingefügt: {} – Inhalt {}", human_size(std::fs::metadata(&pasted)?.len()), if got == expected { "gleich" } else { "VERSCHIEDEN" });
        let sent = last_sending(&mut client_notices).await;
        println!("Client meldet: {:?}, {} gesendet", sent.as_ref().map(|p| p.phase.clone()), human_size(sent.map_or(0, |p| p.bytes)));

        // Noch einmal einfügen und nach etwa einem Drittel abbrechen
        let target = base.join("abgebrochen");
        std::fs::create_dir_all(&target)?;
        let paste = paste_in_background(&target)?;
        let end = watch(&mut notices, Some(&server_transfers)).await?;
        let status = tokio::task::spawn_blocking(move || paste.wait_with_output()).await??;
        println!("Einfügen beendet ({}), Fortschritt am Ende: {:?}", status.status, end.phase);
        println!("abgebrochene Datei im Ziel: {}", if target.join(NAME).exists() { "noch da" } else { "weg" });
        let sent = last_sending(&mut client_notices).await;
        println!("Client meldet: {:?}", sent.map(|p| p.phase));

        client_task.abort();
        server_task.abort();
        match backup {
            Some(previous) => real.write(&previous)?,
            None => real.write(&ClipboardData::default())?,
        }
        println!("vorheriger Inhalt der Zwischenablage wiederhergestellt");
        let _ = std::fs::remove_dir_all(&base);
        Ok(())
    }

    /// Schreibt `size` Bytes mit wechselndem Inhalt und liefert ihren SHA-256.
    fn write_source(path: &Path, size: u64) -> Result<[u8; 32]> {
        let mut file = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
        let mut hasher = Sha256::new();
        let mut block = vec![0u8; 1 << 20];
        let mut written = 0u64;
        while written < size {
            let n = (size - written).min(block.len() as u64) as usize;
            for (i, chunk) in block[..n].chunks_mut(8).enumerate() {
                let value = (written / 8 + i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes();
                chunk.copy_from_slice(&value[..chunk.len()]);
            }
            hasher.update(&block[..n]);
            file.write_all(&block[..n])?;
            written += n as u64;
        }
        file.flush()?;
        Ok(hasher.finalize().into())
    }

    fn hash(path: &Path) -> Result<[u8; 32]> {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                return Ok(hasher.finalize().into());
            }
            hasher.update(&buf[..n]);
        }
    }

    fn paste_in_background(target: &PathBuf) -> Result<std::process::Child> {
        Ok(std::process::Command::new(std::env::current_exe()?).arg("einfuegen").arg(target).spawn()?)
    }

    /// Gibt den Fortschritt des Einfügens am Server etwa jede Sekunde aus, bis es endet. Mit `cancel`
    /// bricht es nach einem Drittel ab.
    async fn watch(notices: &mut UnboundedReceiver<ServerNotice>, cancel: Option<&Transfers>) -> Result<Progress> {
        let mut printed = Instant::now();
        loop {
            let ServerNotice::Transfer(p) = notices.recv().await.context("Server beendet")? else { continue };
            if p.direction != Direction::Receiving {
                continue;
            }
            if !p.phase.is_running() {
                println!(
                    "  Ende: {:?} nach {:.1} s, {} ({}/s)",
                    p.phase,
                    p.elapsed.as_secs_f64(),
                    human_size(p.bytes),
                    human_size(p.bytes_per_s)
                );
                return Ok(p);
            }
            if let Some(transfers) = cancel.filter(|_| p.bytes * 3 >= p.total) {
                println!("  breche nach {} ab", human_size(p.bytes));
                transfers.cancel(Direction::Receiving, p.transfer);
            }
            if printed.elapsed() >= Duration::from_secs(1) {
                printed = Instant::now();
                let left = p.remaining.map(|r| format!(", noch {} s", r.as_secs())).unwrap_or_default();
                println!("  {:5.1} %  {}/s{left}", p.bytes as f64 * 100.0 / p.total.max(1) as f64, human_size(p.bytes_per_s));
            }
        }
    }

    /// Die letzte Meldung des Clients über ein beendetes Senden
    async fn last_sending(notices: &mut UnboundedReceiver<ClientNotice>) -> Option<Progress> {
        tokio::time::sleep(Duration::from_millis(600)).await;
        let mut last = None;
        while let Ok(notice) = notices.try_recv() {
            if let ClientNotice::Transfer(p) = notice {
                if p.direction == Direction::Sending && p.phase != Phase::Running {
                    last = Some(p);
                }
            }
        }
        last
    }
}
