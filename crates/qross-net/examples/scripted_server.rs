//! Server mit simulierter Eingabe, um Dateiübertragungen in der Oberfläche ohne zweiten Rechner
//! anzusehen. Er zieht einem Client eine Datei hinüber, gibt sie ihm als kopierte Datei mit oder
//! holt sich, was der Client kopiert hat:
//!
//! ```text
//! cargo run -p qross-net --example scripted_server -- drag <Client-Name> <Datei>
//! cargo run -p qross-net --example scripted_server -- copy <Client-Name> <Datei>
//! cargo run -p qross-net --example scripted_server -- receive <Client-Name>
//! ```
//!
//! Lauscht nur auf `127.0.0.1:24990` und nimmt einen Client mit diesem Namen ohne Rückfrage an.
//! In der Oberfläche als Client die Adresse `127.0.0.1:24990` eintragen, für `drag` und `copy` am
//! besten im Testmodus (sonst bewegt der Client den echten Mauszeiger).
//! - `drag`: Die Taste bleibt bis zum Ende der Übertragung gedrückt; abgelegt wird beim Client
//!   (im Testmodus in „Downloads/Qross“).
//! - `copy`: Die Datei landet in der Zwischenablage des Clients.
//! - `receive`: Cursor zum Client und zurück; dessen kopierte Dateien kommen hierher (ohne
//!   Testmodus, denn im Testmodus hat der Client eine eigene, leere Zwischenablage).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use qross_core::{CaptureEvent, Layout, LinkConfig};
use qross_net::files::human_size;
use qross_net::progress::{Direction, Progress};
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
use qross_net::{Identity, TrustOutcome, TrustStore, TRUSTED_CLIENTS_FILE};
use qross_platform::dryrun::{CaptureCall, RecordingCapture, SharedClipboard};
use qross_platform::Clipboard;
use qross_proto::{ClipboardData, Edge, MouseButton, Rect};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

const SERVER: &str = "testserver";
const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);
const EDGE: (i32, i32) = (1919, 540);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, client, file) = match args.as_slice() {
        [mode, client] if mode == "receive" => (mode.as_str(), client.clone(), None),
        [mode, client, file] if mode == "drag" || mode == "copy" => {
            let file = std::path::absolute(file)?;
            if !file.exists() {
                bail!("{} nicht gefunden", file.display());
            }
            (mode.as_str(), client.clone(), Some(file))
        }
        _ => bail!("Aufruf: scripted_server drag|copy <Client-Name> <Datei> oder scripted_server receive <Client-Name>"),
    };

    // Eigene Identität und Freigaben in einem festen Ordner: Der Client bestätigt den Fingerprint nur einmal.
    let data_dir = std::env::temp_dir().join("qross-scripted-server");
    std::fs::create_dir_all(&data_dir)?;
    let identity = Identity::load_or_create(&data_dir)?;
    let trust_file = data_dir.join(TRUSTED_CLIENTS_FILE);
    let layout = Layout::from_links(&[LinkConfig { from: SERVER.into(), edge: Edge::Right, to: client.clone() }])?;
    let config = ServerConfig { name: SERVER.into(), listen: "127.0.0.1:24990".parse()?, layout };
    let (notice_tx, mut notices) = tokio::sync::mpsc::unbounded_channel();
    let server = Server::bind(config, &identity, TrustStore::load(&trust_file)?)?
        .with_notices(notice_tx)
        .with_discovery(false);

    let capture = RecordingCapture::default();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut clipboard = SharedClipboard::default();
    if let (Some(file), "copy") = (&file, mode) {
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        clipboard.write_files(std::slice::from_ref(file), &ClipboardData::from_text(name))?;
    }
    tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(clipboard.clone()),
        screen: Box::new(|| SCREEN),
        monitors: Box::new(Vec::new),
    }));
    println!("Server „{SERVER}“ auf 127.0.0.1:24990, Fingerprint {}", identity.fingerprint());
    println!("Warte auf den Client „{client}“ …");

    loop {
        match notices.recv().await.context("Server beendet")? {
            ServerNotice::ClientRejected { name, fingerprint, trust: TrustOutcome::Unknown, .. } if name == client => {
                // Freigeben; der Client versucht es in Kürze erneut.
                TrustStore::load(&trust_file)?.trust(&name, &fingerprint)?;
                println!("„{name}“ freigegeben");
            }
            ServerNotice::ClientConnected { name, .. } if name == client => break,
            _ => {}
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;

    match (mode, file) {
        ("drag", Some(file)) => {
            // Mit gedrückter Taste an die Kante: Der Server fragt nach, ob Dateien gezogen werden.
            hit_edge(&events, &capture, true).await?;
            events.send(CaptureEvent::DragProbeResult { files: vec![file] })?;
            let end = watch(&mut notices, Direction::Sending).await?;
            // Am Ende loslassen; nach einem Abbruch legt der Client nichts ab.
            events.send(CaptureEvent::MouseDelta { dx: 40, dy: 0 })?;
            events.send(CaptureEvent::Button { button: MouseButton::Left, down: false })?;
            println!("losgelassen nach {:?}", end.phase);
        }
        ("copy", _) => {
            hit_edge(&events, &capture, false).await?;
            watch(&mut notices, Direction::Sending).await?;
        }
        _ => {
            hit_edge(&events, &capture, false).await?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            // Zurück zum Server: Der Client gibt seine Zwischenablage mit.
            events.send(CaptureEvent::MouseDelta { dx: -5000, dy: 0 })?;
            watch(&mut notices, Direction::Receiving).await?;
            tokio::time::sleep(Duration::from_secs(1)).await;
            println!("Zwischenablage des Servers: {:?}", clipboard.files());
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    Ok(())
}

/// Cursor an die rechte Kante, bis der Server zum Client wechselt (bzw. mit gedrückter Taste nachfragt).
async fn hit_edge(events: &UnboundedSender<CaptureEvent>, capture: &RecordingCapture, buttons_down: bool) -> Result<()> {
    let (x, y) = EDGE;
    for _ in 0..200 {
        events.send(CaptureEvent::EdgeHit { edge: Edge::Right, x, y, buttons_down })?;
        tokio::time::sleep(Duration::from_millis(25)).await;
        let calls = capture.0.lock().unwrap();
        if calls.iter().any(|c| matches!(c, CaptureCall::Remote | CaptureCall::ProbeDrag { .. })) {
            return Ok(());
        }
    }
    bail!("kein Wechsel zum Client")
}

/// Gibt den Fortschritt etwa jede Sekunde aus, bis eine Übertragung in `direction` endet.
async fn watch(notices: &mut UnboundedReceiver<ServerNotice>, direction: Direction) -> Result<Progress> {
    let mut printed = Instant::now();
    loop {
        let ServerNotice::Transfer(p) = notices.recv().await.context("Server beendet")? else { continue };
        if p.direction != direction {
            continue;
        }
        if !p.phase.is_running() {
            println!("Ende: {:?} nach {:.1} s ({})", p.phase, p.elapsed.as_secs_f64(), human_size(p.bytes));
            return Ok(p);
        }
        if printed.elapsed() >= Duration::from_secs(1) {
            printed = Instant::now();
            let percent = if p.total == 0 { 100.0 } else { p.bytes as f64 * 100.0 / p.total as f64 };
            println!("{percent:5.1} %  {}/s  noch {:?}", human_size(p.bytes_per_s), p.remaining.map(|r| r.as_secs()));
        }
    }
}
