//! Misst die Verzögerung der Eingabekette auf diesem Rechner (Loopback), ohne echte Erfassung und
//! ohne echtes Einspeisen. Ein eigener Thread spielt die Rolle des Hooks: Er schickt im
//! Millisekundentakt eine Mausbewegung (unter Windows schafft `sleep` eher 600 je Sekunde) und mit jeder
//! 20. einen Tastendruck an einen echten Server.
//!
//! ```text
//! cargo run --release -p qross-net --example latency_probe            # Client im selben Prozess, eigene Laufzeit
//! cargo run --release -p qross-net --example latency_probe -- files   # dabei kopierte Dateien über eine Leitung wie WLAN
//! cargo run --release -p qross-net --example latency_probe -- hang    # die Leitung hängt zweimal 2 s lang
//! cargo run -p qross-net --example latency_probe -- server            # nur Server auf 127.0.0.1:24991
//! qross --data-dir <Ordner> client 127.0.0.1:24991 --name probe --dry-run   # dazu, zweites Fenster
//! ```
//!
//! Optional als letztes Argument die Dauer in Sekunden (Standard 20; mit `files` mindestens bis kurz
//! nach dem Ende der Übertragung). Ausgegeben wird alle fünf Sekunden und am Ende das Ergebnis beim
//! Server (Median und 95 % je Anteil). Der Client im selben Prozess merkt sich, wann er was eingespeist
//! hat; daraus misst die Probe zusätzlich von Ende zu Ende, vom Hook bis die Bewegung (oder eine neuere)
//! bzw. der Tastendruck eingespeist ist, mit Median, 95. und 99. Perzentil und Maximum. Im zweiten
//! Fenster schreibt `--dry-run` die Eingaben nur ins Log.
//!
//! **`files`:** Zwischen Client und Server liegt ein Vermittler mit 50 Mbit/s, einer Warteschlange von
//! 64 KB am Engpass (gut 10 ms) und 1 ms Laufzeit je Richtung. In der Zwischenablage des Servers liegen
//! 320 Dateien zu 256 KB; sie gehen beim Wechsel zum Client hinüber. Ausgewertet wird getrennt nach
//! „während der Übertragung“ (vom Senden bis alles beim Client liegt) und „ohne Übertragung“.
//!
//! **`hang`:** Der Vermittler lässt nach 5 s und nach 12 s je 2 s lang nichts durch (Funkloch im WLAN).
//! Gezählt wird, wie viele veraltete Positionen danach noch eingespeist werden und wann der Cursor
//! wieder aktuell ist.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use qross_core::{CaptureEvent, Inject, Layout, LinkConfig};
use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
use qross_net::clipfiles::FilesNotice;
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
use qross_net::{clipfiles, latency, Identity, TrustOutcome, TrustStore, TRUSTED_CLIENTS_FILE};
use qross_platform::dryrun::{CaptureCall, RecordingCapture, SharedClipboard};
use qross_platform::{Clipboard, Injector};
use qross_proto::{ClipboardData, Edge, GameState, KeyCode, LatencySummary, Rect};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{self, UnboundedSender};

const CLIENT: &str = "probe";
const SCREEN: Rect = Rect::new(0, 0, 1920, 1080);
/// Alle so viele Bewegungen ein Tastendruck (50 je Sekunde)
const KEY_EVERY: u64 = 20;
/// Kopierte Dateien für `files`
const FILES: usize = 320;
const FILE_SIZE: usize = 256 * 1024;
/// Hänger für `hang`: Beginn nach dem Start des Hooks und Dauer
const HANGS: [(Duration, Duration); 2] =
    [(Duration::from_secs(5), Duration::from_secs(2)), (Duration::from_secs(12), Duration::from_secs(2))];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// Nur Eingaben, direkt über Loopback
    Plain,
    /// Nur der Server; der Client läuft im zweiten Fenster
    Server,
    /// Eingaben, während kopierte Dateien über eine gedrosselte Leitung gehen
    Files,
    /// Eingaben über eine Leitung, die zwischendurch hängt
    Hang,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scenario = match args.first().map(String::as_str) {
        Some("server") => Scenario::Server,
        Some("files") => Scenario::Files,
        Some("hang") => Scenario::Hang,
        _ => Scenario::Plain,
    };
    let seconds = args.last().and_then(|a| a.parse().ok()).unwrap_or(20u64);
    let separate = scenario == Scenario::Server;
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
    let (notice_tx, mut notices) = mpsc::unbounded_channel();
    let server = {
        let _guard = server_rt.enter();
        Server::bind(config, &server_id, trust)?.with_notices(notice_tx).with_discovery(false)
    };
    let addr = server.local_addr()?;
    let capture = RecordingCapture::default();
    let (events, events_rx) = mpsc::unbounded_channel();
    let server_clip = SharedClipboard::default();
    if scenario == Scenario::Files {
        let folder = probe_files(&data_dir.join("dateien"))?;
        server_clip.clone().write_files(&[folder], &ClipboardData::from_text("dateien"))?;
    }
    server_rt.spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(server_clip.clone()),
        screen: Box::new(|| SCREEN),
        monitors: Box::new(Vec::new),
        game: Box::new(GameState::default),
        finder: None,
    }));

    // Zwischen Client und Server: eine Leitung wie WLAN bzw. eine, die hängt
    let relay = match scenario {
        Scenario::Files => Some(Relay::start(addr, Link::WLAN)?),
        Scenario::Hang => Some(Relay::start(addr, Link::FAST)?),
        _ => None,
    };
    let injected = Stamped::default();
    let client_clip = SharedClipboard::default();
    let (client_tx, mut client_notices) = mpsc::unbounded_channel();
    let _client_rt = if separate {
        println!("Server listening on {addr}, fingerprint {}", server_id.fingerprint());
        println!("Start the client: qross --data-dir <folder> client {addr} --name {CLIENT} --dry-run");
        None
    } else {
        let client_rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
        let config = ClientConfig {
            name: CLIENT.into(),
            server: relay.as_ref().map_or(addr, |r| r.addr).to_string(),
            server_fingerprint: Some(server_id.fingerprint()),
            clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
            share_concealed: false,
        };
        let platform = ClientPlatform {
            injector: Box::new(injected.clone()),
            clipboard: Box::new(client_clip.clone()),
            screen: Box::new(|| SCREEN),
            monitors: Box::new(Vec::new),
            game: Box::new(GameState::default),
            finder: None,
        };
        client_rt.spawn(client::run_with_notices(config, client_id, TrustStore::in_memory(), platform, client_tx));
        Some(client_rt)
    };

    server_rt.block_on(async {
        // Verbunden (im zweiten Fenster: freigeben, sobald er anklopft) und Cursor drüben
        loop {
            match notices.recv().await {
                Some(ServerNotice::ClientConnected { name, .. }) if name == CLIENT => break,
                Some(ServerNotice::ClientRejected { name, fingerprint, trust: TrustOutcome::Unknown, .. }) if name == CLIENT => {
                    println!("'{CLIENT}' approved ({fingerprint}); it connects on its next attempt");
                    TrustStore::load(&trust_file)?.trust(&name, &fingerprint)?;
                }
                Some(_) => {}
                None => bail!("server stopped"),
            }
        }
        while !capture.0.lock().unwrap().contains(&CaptureCall::Remote) {
            let _ = events.send(CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false });
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let entered_at = Instant::now();
        // Im selben Prozess an den Anfang der Bahn, damit jede Position genau einer Bewegung gehört
        if !separate {
            let entered = loop {
                if let Some(at) = injected.last_position() {
                    break at;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            };
            let start = position(0);
            let _ = events.send(CaptureEvent::MouseDelta { dx: start.0 - entered.0, dy: start.1 - entered.1 });
            while injected.last_position() != Some(start) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        let what = match scenario {
            Scenario::Files => format!("{FILES} copied files of {} KB over 50 Mbit/s", FILE_SIZE / 1024),
            Scenario::Hang => format!("link hangs for 2 s each after {} s and {} s", HANGS[0].0.as_secs(), HANGS[1].0.as_secs()),
            _ => format!("{seconds} s"),
        };
        println!("Cursor on '{CLIENT}', a move every millisecond, every 20th with a key press; {what} …");
        let stop = Arc::new(AtomicBool::new(false));
        let hook = std::thread::spawn({
            let (events, stop) = (events.clone(), stop.clone());
            let duration = Duration::from_secs(if scenario == Scenario::Files { 300 } else { seconds });
            move || play_hook(&events, duration, &stop)
        });
        let started = Instant::now();
        let hangs = match &relay {
            Some(relay) if scenario == Scenario::Hang => Some(tokio::spawn(play_hangs(relay.hang.clone()))),
            _ => None,
        };

        let mut last: Option<LatencySummary> = None;
        let mut print = tokio::time::interval(Duration::from_secs(5));
        print.tick().await;
        // Übertragung: Beginn beim Server, Ende beim Client (alles angekommen)
        let (mut sending, mut ready): (Option<Instant>, Option<Instant>) = (None, None);
        let minimum = started + Duration::from_secs(seconds);
        loop {
            tokio::select! {
                notice = notices.recv() => match notice {
                    Some(ServerNotice::Latency { summary, .. }) => last = Some(summary),
                    Some(ServerNotice::ClipboardFiles(FilesNotice::Sending { .. })) => sending = Some(Instant::now()),
                    Some(ServerNotice::ClipboardFiles(FilesNotice::Failed { reason, .. })) => bail!("transfer failed: {reason}"),
                    Some(_) => {}
                    None => bail!("server stopped"),
                },
                Some(notice) = client_notices.recv() => match notice {
                    ClientNotice::ClipboardFiles(FilesNotice::Ready { .. }) => ready = Some(Instant::now()),
                    ClientNotice::ClipboardFiles(FilesNotice::Failed { reason, .. }) => bail!("transfer failed: {reason}"),
                    _ => {}
                },
                _ = print.tick() => {
                    if let Some(summary) = &last {
                        println!("{}", latency::describe(summary));
                    }
                    // Mit Dateien: bis kurz nach dem Ende der Übertragung
                    let now = Instant::now();
                    if scenario == Scenario::Files && ready.is_some_and(|at| now >= at + Duration::from_secs(3)) && now >= minimum {
                        stop.store(true, Ordering::Relaxed);
                    }
                    if hook.is_finished() {
                        break;
                    }
                }
            }
        }
        let sent = hook.join().expect("hook thread panicked");
        let hangs = match hangs {
            Some(task) => task.await?,
            None => Vec::new(),
        };
        // Nachzügler abwarten
        tokio::time::sleep(Duration::from_millis(500)).await;

        let summary = last.unwrap_or_default();
        println!("\nResult at the server (µs, median / 95%):");
        for (label, chain) in [("Mouse", summary.mouse), ("Keys", summary.keys)] {
            let Some(chain) = chain else { continue };
            let spread = |s: Option<qross_proto::Spread>| s.map_or("–".to_owned(), |s| format!("{} / {}", s.p50_us, s.p95_us));
            println!(
                "{label:7} total {:>13}  server {:>11}  network {:>11}  client {:>11}  ({} samples, {} reported back)",
                spread(chain.total),
                spread(Some(chain.server)),
                spread(chain.network),
                spread(chain.client),
                chain.samples,
                chain.reported
            );
        }
        if let Some(us) = summary.uncertainty_us {
            println!("Network accurate to ± {us} µs (half the round trip of the best exchange)");
        }
        if separate {
            return Ok(());
        }

        let injected = injected.0.lock().unwrap().clone();
        let mouse = mouse_latencies(&sent.moves, &injected);
        let keys = key_latencies(&sent.keys, &injected);
        println!("\nEnd to end, from the hook until injected:");
        header();
        // Bewegung 0 ist der Anfang der Bahn, kein Ereignis des Hooks
        let (moves, mouse) = (&sent.moves[1..], &mouse[1..]);
        report("Mouse", moves, mouse, |_| true);
        report("Keys", &sent.keys, &keys, |_| true);
        if scenario == Scenario::Files {
            let (Some(from), Some(to)) = (sending, ready) else { bail!("the transfer did not finish") };
            let during = |at: Instant| at >= from && at <= to;
            let bytes = (FILES * FILE_SIZE) as f64;
            let secs = (to - from).as_secs_f64();
            println!(
                "\nTransfer: {:.0} MB in {} s ({} Mbit/s), started {} s after the switch",
                bytes / 1e6,
                decimal(secs),
                decimal(bytes * 8.0 / secs / 1e6),
                decimal(from.saturating_duration_since(entered_at).as_secs_f64())
            );
            header();
            report("Mouse, during transfer", moves, mouse, during);
            report("Mouse, otherwise", moves, mouse, |at| !during(at));
            report("Keys, during transfer", &sent.keys, &keys, during);
            report("Keys, otherwise", &sent.keys, &keys, |at| !during(at));
            // Angekommene Dateien wieder wegräumen (sie liegen im Ablageordner der Zwischenablage)
            if let Some(dir) = client_clip.clone().files().first().and_then(|p| p.parent()) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        for (i, (from, to)) in hangs.iter().enumerate() {
            let after = after_hang(&sent.moves, &injected, *to);
            println!(
                "\nHang {} ({} s): first move {} ms after it, {} stale positions replayed (over 100 ms old, \
                 the oldest {} ms), current again after {} ms",
                i + 1,
                decimal((*to - *from).as_secs_f64()),
                after.first.map_or("–".into(), ms),
                after.stale,
                ms(after.oldest),
                after.current.map_or("–".into(), ms),
            );
        }
        Ok(())
    })
}

/// Bahn der Bewegungen auf dem Client: Zeile für Zeile hin und her, jede Position nur einmal
const LEFT: i32 = 100;
const RIGHT: i32 = 1800;
const TOP: i32 = 100;
const ROW: u64 = (RIGHT - LEFT + 1) as u64;

/// Position nach `i` Bewegungen
fn position(i: u64) -> (i32, i32) {
    let (row, col) = ((i / ROW) as i32, (i % ROW) as i32);
    let x = if row % 2 == 0 { LEFT + col } else { RIGHT - col };
    (x, TOP + row)
}

/// Nach wie vielen Bewegungen der Cursor hier steht (`None`: nicht auf der Bahn)
fn index(x: i32, y: i32) -> Option<u64> {
    let row = y - TOP;
    if row < 0 || !(LEFT..=RIGHT).contains(&x) {
        return None;
    }
    let col = if row % 2 == 0 { x - LEFT } else { RIGHT - x };
    Some(row as u64 * ROW + col as u64)
}

/// Was der Hook wann geschickt hat: Bewegung `i` führt zu `position(i)` (Index 0 ist der Anfang)
struct Sent {
    moves: Vec<Instant>,
    keys: Vec<Instant>,
}

/// Wie ein Hook-Thread: Bewegungen im Millisekundentakt, alle 20 ms ein Tastendruck
fn play_hook(events: &UnboundedSender<CaptureEvent>, duration: Duration, stop: &AtomicBool) -> Sent {
    let start = Instant::now();
    let mut sent = Sent { moves: vec![start], keys: Vec::new() };
    let mut n = 0u64;
    while start.elapsed() < duration && !stop.load(Ordering::Relaxed) {
        n += 1;
        let ((x0, y0), (x1, y1)) = (position(n - 1), position(n));
        sent.moves.push(Instant::now());
        let _ = events.send(CaptureEvent::MouseDelta { dx: x1 - x0, dy: y1 - y0 });
        if n % KEY_EVERY == KEY_EVERY / 2 {
            sent.keys.push(Instant::now());
            let _ = events.send(CaptureEvent::Key { key: KeyCode::A, down: true });
            let _ = events.send(CaptureEvent::Key { key: KeyCode::A, down: false });
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    sent
}

/// Lässt den Vermittler zweimal hängen; liefert, wann genau
async fn play_hangs(hang: Arc<AtomicBool>) -> Vec<(Instant, Instant)> {
    let start = tokio::time::Instant::now();
    let mut out = Vec::new();
    for (after, length) in HANGS {
        tokio::time::sleep_until(start + after).await;
        hang.store(true, Ordering::SeqCst);
        let from = Instant::now();
        tokio::time::sleep(length).await;
        hang.store(false, Ordering::SeqCst);
        out.push((from, Instant::now()));
    }
    out
}

/// Einspeisen nur mit Zeitstempel merken
#[derive(Clone, Default)]
struct Stamped(Arc<Mutex<Vec<(Instant, Inject)>>>);

impl Stamped {
    fn last_position(&self) -> Option<(i32, i32)> {
        self.0.lock().unwrap().iter().rev().find_map(|(_, event)| match event {
            Inject::MoveTo { x, y } => Some((*x, *y)),
            _ => None,
        })
    }
}

impl Injector for Stamped {
    fn inject(&mut self, event: &Inject) {
        self.0.lock().unwrap().push((Instant::now(), *event));
    }
}

/// Je Bewegung: wie lange, bis sie oder eine spätere eingespeist war (`None`: nie)
fn mouse_latencies(sent: &[Instant], injected: &[(Instant, Inject)]) -> Vec<Option<Duration>> {
    let mut out = vec![None; sent.len()];
    let mut next = 1;
    for (at, event) in injected {
        let Inject::MoveTo { x, y } = event else { continue };
        let Some(i) = index(*x, *y) else { continue };
        while next <= i as usize && next < sent.len() {
            out[next] = Some(at.saturating_duration_since(sent[next]));
            next += 1;
        }
    }
    out
}

/// Je Tastendruck: wie lange, bis er eingespeist war
fn key_latencies(sent: &[Instant], injected: &[(Instant, Inject)]) -> Vec<Option<Duration>> {
    let mut downs = injected.iter().filter(|(_, e)| matches!(e, Inject::Key { down: true, .. })).map(|(at, _)| *at);
    sent.iter().map(|s| downs.next().map(|at| at.saturating_duration_since(*s))).collect()
}

/// Nach einem Hänger, der um `end` vorbei war
struct AfterHang {
    /// Erste eingespeiste Bewegung danach
    first: Option<Duration>,
    /// So viele Positionen waren beim Einspeisen über 100 ms alt, bevor wieder eine aktuelle kam
    stale: usize,
    oldest: Duration,
    /// Wann wieder eine aktuelle Position (höchstens 20 ms alt) eingespeist wurde
    current: Option<Duration>,
}

fn after_hang(sent: &[Instant], injected: &[(Instant, Inject)], end: Instant) -> AfterHang {
    let mut after = AfterHang { first: None, stale: 0, oldest: Duration::ZERO, current: None };
    for (at, event) in injected.iter().filter(|(at, _)| *at > end) {
        let Inject::MoveTo { x, y } = event else { continue };
        let Some(&sent_at) = index(*x, *y).and_then(|i| sent.get(i as usize)) else { continue };
        after.first.get_or_insert(at.saturating_duration_since(end));
        let age = at.saturating_duration_since(sent_at);
        if age <= Duration::from_millis(20) {
            after.current = Some(at.saturating_duration_since(end));
            break;
        }
        if age > Duration::from_millis(100) {
            after.stale += 1;
            after.oldest = after.oldest.max(age);
        }
    }
    after
}

fn ms(d: Duration) -> String {
    format!("{:.2}", d.as_secs_f64() * 1000.0)
}

/// Eine Nachkommastelle, mit Punkt
fn decimal(value: f64) -> String {
    format!("{value:.1}")
}

fn header() {
    println!("{:24} {:>8} {:>8} {:>8} {:>8} {:>7} {:>7} {:>5}", "ms", "p50", "p95", "p99", "max", ">50 ms", "count", "never");
}

/// Median, 95. und 99. Perzentil, Maximum und Ruckler über 50 ms der Ereignisse, deren Versand `pick` wählt
fn report(label: &str, sent: &[Instant], latencies: &[Option<Duration>], pick: impl Fn(Instant) -> bool) {
    let chosen: Vec<Option<Duration>> = sent.iter().zip(latencies).filter(|(at, _)| pick(**at)).map(|(_, l)| *l).collect();
    let never = chosen.iter().filter(|l| l.is_none()).count();
    let mut samples: Vec<Duration> = chosen.into_iter().flatten().collect();
    if samples.is_empty() {
        return println!("{label:24} –");
    }
    samples.sort();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    let over = samples.iter().filter(|d| **d > Duration::from_millis(50)).count();
    println!(
        "{label:24} {:>8} {:>8} {:>8} {:>8} {:>7} {:>7} {:>5}",
        ms(at(0.5)),
        ms(at(0.95)),
        ms(at(0.99)),
        ms(samples[samples.len() - 1]),
        over,
        samples.len(),
        never
    );
}

/// Ordner mit den kopierten Dateien für `files` (bleibt für den nächsten Lauf liegen)
fn probe_files(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    for i in 0..FILES {
        let path = dir.join(format!("datei-{i:03}.bin"));
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == FILE_SIZE as u64) {
            continue;
        }
        let bytes: Vec<u8> = (0..FILE_SIZE)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        std::fs::write(&path, bytes)?;
    }
    Ok(dir.to_owned())
}

/// Eigenschaften der Leitung zwischen Client und Server, je Richtung
#[derive(Clone, Copy)]
struct Link {
    /// 0: unbegrenzt
    mbit: f64,
    /// Warteschlange am Engpass; was nicht mehr hineinpasst, fällt weg
    queue: usize,
    delay: Duration,
}

impl Link {
    /// Wie ein ordentliches WLAN
    const WLAN: Self = Self { mbit: 50.0, queue: 64 * 1024, delay: Duration::from_millis(1) };
    /// Ohne Engpass
    const FAST: Self = Self { mbit: 0.0, queue: 0, delay: Duration::from_millis(1) };
}

/// Engpass einer Richtung: Ein Paket wartet, bis die Leitung frei ist; ist die Warteschlange voll,
/// fällt es weg (wie in einem WLAN-Router).
struct Bottleneck {
    link: Link,
    free_at: tokio::time::Instant,
}

impl Bottleneck {
    fn new(link: Link) -> Self {
        Self { link, free_at: tokio::time::Instant::now() }
    }

    /// Wann das Paket drüben ankommt; `None`: verloren (Hänger oder volle Warteschlange)
    fn admit(&mut self, len: usize, hang: &AtomicBool) -> Option<tokio::time::Instant> {
        if hang.load(Ordering::Relaxed) {
            return None;
        }
        let now = tokio::time::Instant::now();
        if self.link.mbit <= 0.0 {
            return Some(now + self.link.delay);
        }
        let rate = self.link.mbit * 1e6 / 8.0;
        let start = self.free_at.max(now);
        let queued = (start - now).as_secs_f64() * rate;
        if queued + len as f64 > self.link.queue as f64 {
            return None;
        }
        self.free_at = start + Duration::from_secs_f64(len as f64 / rate);
        Some(self.free_at + self.link.delay)
    }
}

/// UDP-Vermittler zwischen Client und Server in einem eigenen Thread
struct Relay {
    addr: SocketAddr,
    /// Solange gesetzt, kommt nichts durch
    hang: Arc<AtomicBool>,
}

impl Relay {
    fn start(server: SocketAddr, link: Link) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let (front, up) = rt.block_on(async {
            let up = UdpSocket::bind("127.0.0.1:0").await?;
            up.connect(server).await?;
            Ok::<_, std::io::Error>((UdpSocket::bind("127.0.0.1:0").await?, up))
        })?;
        let addr = front.local_addr()?;
        let hang = Arc::new(AtomicBool::new(false));
        let stalled = hang.clone();
        std::thread::Builder::new().name("relay".into()).spawn(move || rt.block_on(relay(front, up, link, stalled)))?;
        Ok(Self { addr, hang })
    }
}

type Delivery = mpsc::UnboundedSender<(tokio::time::Instant, Vec<u8>)>;

async fn relay(front: UdpSocket, up: UdpSocket, link: Link, hang: Arc<AtomicBool>) {
    let (front, up) = (Arc::new(front), Arc::new(up));
    let client: Arc<Mutex<Option<SocketAddr>>> = Arc::default();
    // Je Richtung der Reihe nach zustellen
    let (to_server, mut for_server) = mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
    let (to_client, mut for_client) = mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
    tokio::spawn({
        let up = up.clone();
        async move {
            while let Some((at, data)) = for_server.recv().await {
                tokio::time::sleep_until(at).await;
                let _ = up.send(&data).await;
            }
        }
    });
    tokio::spawn({
        let (front, client) = (front.clone(), client.clone());
        async move {
            while let Some((at, data)) = for_client.recv().await {
                tokio::time::sleep_until(at).await;
                let addr = *client.lock().unwrap();
                if let Some(addr) = addr {
                    let _ = front.send_to(&data, addr).await;
                }
            }
        }
    });
    let (mut upward, mut downward) = (Bottleneck::new(link), Bottleneck::new(link));
    let (mut a, mut b) = (vec![0u8; 65536], vec![0u8; 65536]);
    let pass = |bottleneck: &mut Bottleneck, data: &[u8], to: &Delivery| {
        if let Some(at) = bottleneck.admit(data.len(), &hang) {
            let _ = to.send((at, data.to_vec()));
        }
    };
    loop {
        // Unter Windows meldet `recv` nach einem ICMP „Port nicht erreichbar“ einen Fehler; weiter geht es trotzdem.
        tokio::select! {
            received = front.recv_from(&mut a) => if let Ok((n, from)) = received {
                *client.lock().unwrap() = Some(from);
                pass(&mut upward, &a[..n], &to_server);
            },
            received = up.recv(&mut b) => if let Ok(n) = received {
                pass(&mut downward, &b[..n], &to_client);
            },
        }
    }
}
