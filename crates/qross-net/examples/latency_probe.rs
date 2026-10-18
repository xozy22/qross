//! Misst die Verzögerung der Eingabekette auf diesem Rechner (Loopback), ohne echte Erfassung und
//! ohne echtes Einspeisen. Ein eigener Thread spielt die Rolle des Hooks: Er schickt etwa 1000
//! Mausbewegungen und 10 Tastendrücke je Sekunde an einen echten Server.
//!
//! ```text
//! cargo run -p qross-net --example latency_probe              # Client im selben Prozess, eigene Laufzeit
//! cargo run -p qross-net --example latency_probe -- server    # nur Server auf 127.0.0.1:24991
//! qross --data-dir <Ordner> client 127.0.0.1:24991 --name probe --dry-run   # dazu, zweites Fenster
//! ```
//!
//! Optional als letztes Argument die Dauer in Sekunden (Standard 20). Ausgegeben wird alle fünf
//! Sekunden und am Ende das Ergebnis beim Server. Der Client im selben Prozess merkt sich die
//! Eingaben nur (`RecordingInjector`); im zweiten Fenster schreibt `--dry-run` sie ins Log.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use qross_core::{CaptureEvent, Layout, LinkConfig};
use qross_net::client::{self, ClientConfig, ClientPlatform};
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
use qross_net::{clipfiles, latency, Identity, TrustOutcome, TrustStore, TRUSTED_CLIENTS_FILE};
use qross_platform::dryrun::{CaptureCall, RecordingCapture, RecordingInjector, SharedClipboard};
use qross_proto::{Edge, GameState, KeyCode, LatencySummary, Rect};
use tokio::sync::mpsc::UnboundedSender;

const CLIENT: &str = "probe";
const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let separate = args.first().is_some_and(|a| a == "server");
    let seconds = args.last().and_then(|a| a.parse().ok()).unwrap_or(20u64);
    let listen = if separate { "127.0.0.1:24991" } else { "127.0.0.1:0" };

    // Server und Client je mit eigener Laufzeit, wie zwei Programme
    let server_rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    // Zweites Fenster: feste Identität und Freigaben in einem eigenen Ordner, damit der Client den
    // Server nur einmal bestätigt; die Daten des Nutzers bleiben unberührt.
    let data_dir = std::env::temp_dir().join("qross-latency-probe");
    let trust_file = data_dir.join(TRUSTED_CLIENTS_FILE);
    let client_id = Identity::generate()?;
    let (server_id, trust) = if separate {
        std::fs::create_dir_all(&data_dir)?;
        (Identity::load_or_create(&data_dir)?, TrustStore::load(&trust_file)?)
    } else {
        let mut trust = TrustStore::in_memory();
        trust.trust(CLIENT, &client_id.fingerprint())?;
        (Identity::generate()?, trust)
    };
    let layout = Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: CLIENT.into() }])?;
    let config = ServerConfig { name: "pc".into(), listen: listen.parse()?, layout };
    let (notice_tx, mut notices) = tokio::sync::mpsc::unbounded_channel();
    let server = {
        let _guard = server_rt.enter();
        Server::bind(config, &server_id, trust)?.with_notices(notice_tx).with_discovery(false)
    };
    let addr = server.local_addr()?;
    let capture = RecordingCapture::default();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    server_rt.spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(SharedClipboard::default()),
        screen: Box::new(|| SCREEN),
        monitors: Box::new(Vec::new),
        game: Box::new(GameState::default),
    }));

    let _client_rt = if separate {
        println!("Server lauscht auf {addr}, Fingerprint {}", server_id.fingerprint());
        println!("Client starten: qross --data-dir <Ordner> client {addr} --name {CLIENT} --dry-run");
        None
    } else {
        let client_rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
        let config = ClientConfig {
            name: CLIENT.into(),
            server: addr.to_string(),
            server_fingerprint: Some(server_id.fingerprint()),
            clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
            share_concealed: false,
        };
        let platform = ClientPlatform {
            injector: Box::new(RecordingInjector::default()),
            clipboard: Box::new(SharedClipboard::default()),
            screen: Box::new(|| SCREEN),
            monitors: Box::new(Vec::new),
            game: Box::new(GameState::default),
        };
        client_rt.spawn(client::run(config, client_id, TrustStore::in_memory(), platform));
        Some(client_rt)
    };

    server_rt.block_on(async {
        // Verbunden (im zweiten Fenster: freigeben, sobald er anklopft) und Cursor drüben
        loop {
            match notices.recv().await {
                Some(ServerNotice::ClientConnected { name, .. }) if name == CLIENT => break,
                Some(ServerNotice::ClientRejected { name, fingerprint, trust: TrustOutcome::Unknown, .. }) if name == CLIENT => {
                    println!("„{CLIENT}“ freigegeben ({fingerprint}), er verbindet sich beim nächsten Versuch");
                    TrustStore::load(&trust_file)?.trust(&name, &fingerprint)?;
                }
                Some(_) => {}
                None => bail!("Server beendet"),
            }
        }
        while !capture.0.lock().unwrap().contains(&CaptureCall::Remote) {
            let _ = events.send(CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false });
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        println!("Cursor auf „{CLIENT}“, {seconds} s lang 1000 Bewegungen und 10 Tasten je Sekunde …");
        let hook = std::thread::spawn({
            let events = events.clone();
            move || play_hook(&events, Duration::from_secs(seconds))
        });

        let mut last: Option<LatencySummary> = None;
        let mut print = tokio::time::interval(Duration::from_secs(5));
        print.tick().await;
        loop {
            tokio::select! {
                notice = notices.recv() => match notice {
                    Some(ServerNotice::Latency { summary, .. }) => last = Some(summary),
                    Some(_) => {}
                    None => bail!("Server beendet"),
                },
                _ = print.tick() => {
                    if let Some(summary) = &last {
                        println!("{}", latency::describe(summary));
                    }
                    if hook.is_finished() {
                        break;
                    }
                }
            }
        }
        let summary = last.unwrap_or_default();
        println!("\nErgebnis (µs, Median / 95 %):");
        for (label, chain) in [("Maus", summary.mouse), ("Tasten", summary.keys)] {
            let Some(chain) = chain else { continue };
            let spread = |s: Option<qross_proto::Spread>| s.map_or("–".to_owned(), |s| format!("{} / {}", s.p50_us, s.p95_us));
            println!(
                "{label:7} gesamt {:>13}  Server {:>11}  Netz {:>11}  Client {:>11}  ({} Stichproben, {} mit Rückmeldung)",
                spread(chain.total),
                spread(Some(chain.server)),
                spread(chain.network),
                spread(chain.client),
                chain.samples,
                chain.reported
            );
        }
        if let Some(us) = summary.uncertainty_us {
            println!("Netz auf ± {us} µs genau (halbe Umlaufzeit des besten Austauschs)");
        }
        Ok(())
    })
}

/// Wie ein Hook-Thread: Bewegungen im Millisekundentakt, alle 100 ms ein Tastendruck
fn play_hook(events: &UnboundedSender<CaptureEvent>, duration: Duration) {
    let start = Instant::now();
    let mut n = 0u64;
    while start.elapsed() < duration {
        let dx = if n.is_multiple_of(2) { 2 } else { -2 };
        let _ = events.send(CaptureEvent::MouseDelta { dx, dy: 0 });
        if n % 100 == 50 {
            let _ = events.send(CaptureEvent::Key { key: KeyCode::A, down: true });
            let _ = events.send(CaptureEvent::Key { key: KeyCode::A, down: false });
        }
        n += 1;
        std::thread::sleep(Duration::from_millis(1));
    }
}
