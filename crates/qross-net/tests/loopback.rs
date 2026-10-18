//! Ende-zu-Ende über echtes QUIC auf Loopback, mit Test-Doubles statt Betriebssystem.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qross_core::chars::{KeyChars, Produces};
use qross_core::keys::{parse_switch_keys, HotkeyEvent, HotkeyTracker, KeyConfig, KeyMode, KeyOptions};
use qross_core::sharing::{Share, Sharing, SharingRules};
use qross_core::{CaptureEvent, Inject, Layout, LinkConfig};
use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
use qross_net::progress::{Direction, Kind, Phase, Progress, Transfers};
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform, SharingHandle};
use qross_net::{clipfiles, files};
use qross_net::{Identity, TrustStore};
use qross_platform::dryrun::{CaptureCall, RecordingCapture, RecordingInjector, SharedClipboard};
use qross_platform::Clipboard;
use qross_proto::{
    read_frame, write_frame, Clipboard as ClipboardMsg, ClipboardData, Control, DragMessage, Edge, FileEntry, GameState,
    KeyCode, MouseButton, MouseMove, Platform, Rect, ScreenInfo, StreamKind, PROTOCOL_VERSION, SERVER_VERSION,
};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

const PC: Rect = Rect::new(0, 0, 1920, 1080);
const LAPTOP: Rect = Rect::new(0, 0, 1280, 800);

/// Client wie ein MacBook mit externem Monitor: links das MacBook (unten bündig), rechts ein
/// höherer Monitor, der weiter oben beginnt. Über dem MacBook liegt eine tote Fläche.
const MAC: Rect = Rect::new(0, -400, 5256, 1800);
const MAC_BUILTIN: Rect = Rect::new(0, 418, 1512, 982);
const MAC_EXTERNAL: Rect = Rect::new(1512, -400, 3744, 1800);

/// Bildschirm des Test-Clients (Gesamtfläche, Monitore); lässt sich während des Tests ändern.
type ClientScreen = Arc<Mutex<(Rect, Vec<Rect>)>>;

/// Was im Vordergrund läuft (Spielemodus); lässt sich während des Tests ändern.
type Game = Arc<Mutex<GameState>>;

fn detector(game: &Game) -> Box<dyn Fn() -> GameState + Send + Sync> {
    let game = game.clone();
    Box::new(move || *game.lock().unwrap())
}

struct Setup {
    events: UnboundedSender<CaptureEvent>,
    capture: RecordingCapture,
    server_clip: SharedClipboard,
    injected: RecordingInjector,
    client_clip: SharedClipboard,
    client_screen: ClientScreen,
    server_game: Game,
    client_game: Game,
    notices: Arc<Mutex<Vec<ServerNotice>>>,
    client_notices: Arc<Mutex<Vec<ClientNotice>>>,
    server_transfers: Transfers,
    client_transfers: Transfers,
    /// Rechte je Rechner des Servers (Standard: alles erlaubt)
    sharing: SharingHandle,
    server_task: JoinHandle<anyhow::Result<()>>,
    client_task: JoinHandle<anyhow::Result<()>>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.server_task.abort();
        self.client_task.abort();
    }
}

fn start(trust_client: bool, pin: Option<qross_net::Fingerprint>) -> Setup {
    start_with(trust_client, pin, clipfiles::DEFAULT_LIMIT)
}

/// `file_limit`: Grenze für kopierte Dateien auf beiden Seiten
fn start_with(trust_client: bool, pin: Option<qross_net::Fingerprint>, file_limit: u64) -> Setup {
    start_client_screen(trust_client, pin, file_limit, LAPTOP, Vec::new())
}

/// Sammelt alle Meldungen eines Kanals.
fn collect<N: Send + 'static>(mut rx: tokio::sync::mpsc::UnboundedReceiver<N>) -> Arc<Mutex<Vec<N>>> {
    let notices = Arc::new(Mutex::new(Vec::new()));
    let collected = notices.clone();
    tokio::spawn(async move {
        while let Some(notice) = rx.recv().await {
            collected.lock().unwrap().push(notice);
        }
    });
    notices
}

/// Wie `start_with`, mit eigener Bildschirmfläche und Monitoren des Clients (leer: keine Liste)
fn start_client_screen(
    trust_client: bool,
    pin: Option<qross_net::Fingerprint>,
    file_limit: u64,
    bounds: Rect,
    monitors: Vec<Rect>,
) -> Setup {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let server_id = Identity::generate().unwrap();
    let client_id = Identity::generate().unwrap();
    let mut trust = TrustStore::in_memory();
    if trust_client {
        trust.trust("laptop", &client_id.fingerprint()).unwrap();
    }

    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let (notice_tx, notice_rx) = tokio::sync::mpsc::unbounded_channel();
    let notices = collect(notice_rx);
    let server = Server::bind(config, &server_id, trust)
        .unwrap()
        .with_clipboard_file_limit(file_limit)
        .with_keys(switch_keys())
        .with_notices(notice_tx);
    let addr = server.local_addr().unwrap();
    let server_transfers = server.transfers();
    let sharing = server.sharing();

    let capture = RecordingCapture::default();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let server_clip = SharedClipboard::default();
    let (server_game, client_game) = (Game::default(), Game::default());
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(server_clip.clone()),
        screen: Box::new(|| PC),
        monitors: Box::new(Vec::new),
        game: detector(&server_game),
    }));

    let injected = RecordingInjector::default();
    let client_clip = SharedClipboard::default();
    let client_screen: ClientScreen = Arc::new(Mutex::new((bounds, monitors)));
    let (screen, monitors) = (client_screen.clone(), client_screen.clone());
    let client_config = ClientConfig {
        name: "laptop".into(),
        server: addr.to_string(),
        server_fingerprint: Some(pin.unwrap_or_else(|| server_id.fingerprint())),
        clipboard_file_limit: file_limit,
        share_concealed: false,
    };
    let (client_tx, client_rx) = tokio::sync::mpsc::unbounded_channel();
    let client_notices = collect(client_rx);
    let client_transfers = Transfers::new();
    let client_task = tokio::spawn(client::run_with_transfers(
        client_config,
        client_id,
        TrustStore::in_memory(),
        ClientPlatform {
            injector: Box::new(injected.clone()),
            clipboard: Box::new(client_clip.clone()),
            screen: Box::new(move || screen.lock().unwrap().0),
            monitors: Box::new(move || monitors.lock().unwrap().1.clone()),
            game: detector(&client_game),
        },
        client_tx,
        client_transfers.clone(),
    ));

    Setup {
        events,
        capture,
        server_clip,
        injected,
        client_clip,
        client_screen,
        server_game,
        client_game,
        notices,
        client_notices,
        server_transfers,
        client_transfers,
        sharing,
        server_task,
        client_task,
    }
}

async fn wait_for(what: &str, condition: impl FnMut() -> bool) {
    wait_up_to(Duration::from_secs(5), what, condition).await;
}

async fn wait_up_to(limit: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..limit.as_millis() / 25 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Zeitüberschreitung beim Warten auf: {what}");
}

fn edge_hit() -> CaptureEvent {
    CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false }
}

#[tokio::test]
async fn cursor_travels_to_client_and_back() {
    let mut s = start(true, None);
    s.server_clip.set_text("vom Server").unwrap();

    // Kantentreffer wiederholen, bis der Client verbunden ist und der Wechsel passiert.
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let injected = s.injected.clone();
    wait_for("Enter beim Client", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 399 })).await;

    for event in [
        CaptureEvent::MouseDelta { dx: 100, dy: 0 },
        CaptureEvent::Key { key: KeyCode::A, down: true },
        CaptureEvent::Key { key: KeyCode::A, down: false },
        CaptureEvent::Button { button: MouseButton::Left, down: true },
        CaptureEvent::Button { button: MouseButton::Left, down: false },
        CaptureEvent::Wheel { dx: 0, dy: -120 },
    ] {
        s.events.send(event).unwrap();
    }
    wait_for("Eingaben beim Client", || {
        injected.0.lock().unwrap().contains(&Inject::Wheel { dx: 0, dy: -120 })
    })
    .await;
    let log = injected.0.lock().unwrap().clone();
    for expected in [
        Inject::MoveTo { x: 101, y: 399 },
        Inject::Key { key: KeyCode::A, down: true },
        Inject::Key { key: KeyCode::A, down: false },
        Inject::Button { button: MouseButton::Left, down: true },
        Inject::Button { button: MouseButton::Left, down: false },
    ] {
        assert!(log.contains(&expected), "{expected:?} fehlt in {log:?}");
    }

    let client_clip = s.client_clip.clone();
    wait_for("Clipboard beim Client", || client_clip.clone().text().as_deref() == Some("vom Server")).await;

    // Zurück über die linke Kante des Laptops; dessen Clipboard kommt mit.
    s.client_clip.set_text("vom Laptop").unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    wait_for("Rückkehr zum Server", || capture.0.lock().unwrap().contains(&CaptureCall::Release { x: 1918, y: 538 }))
        .await;
    let server_clip = s.server_clip.clone();
    wait_for("Clipboard beim Server", || server_clip.clone().text().as_deref() == Some("vom Laptop")).await;
}

/// Wechsel-Kürzel aller Tests, wie in `qross.toml`
fn switch_keys() -> KeyConfig {
    let entries = HashMap::from([
        ("Ctrl+Alt+Digit1".to_owned(), "pc".to_owned()),
        ("Ctrl+Alt+Digit2".to_owned(), "laptop".to_owned()),
        ("Ctrl+Alt+ArrowLeft".to_owned(), "left".to_owned()),
    ]);
    let options = KeyOptions { switch: parse_switch_keys(&entries).unwrap(), ..KeyOptions::default() };
    KeyConfig { options, ..KeyConfig::default() }
}

const CTRL: KeyCode = KeyCode(0xE0);
const ALT: KeyCode = KeyCode(0xE2);
const DIGIT2: KeyCode = KeyCode(0x1F);
const ARROW_LEFT: KeyCode = KeyCode(0x50);

/// Wie eine echte Erfassung: Die Tasten nacheinander drücken und in umgekehrter Reihenfolge
/// loslassen, ausgewertet mit den Kürzeln, die der Server der Erfassung gegeben hat.
fn press(s: &Setup, tracker: &mut HotkeyTracker, keys: &[KeyCode]) {
    let options = s.capture.2.lock().unwrap().clone().expect("der Server stellt die Kürzel ein");
    let strokes = keys.iter().map(|&key| (key, true)).chain(keys.iter().rev().map(|&key| (key, false)));
    for (key, down) in strokes {
        let event = match tracker.on_key(key, down, &options) {
            HotkeyEvent::Triggered { event, .. } => event,
            HotkeyEvent::Consumed { .. } => continue,
            HotkeyEvent::None => CaptureEvent::Key { key, down },
        };
        s.events.send(event).unwrap();
    }
}

#[tokio::test]
async fn hotkeys_switch_to_the_client_and_back() {
    let s = start(true, None);
    let notices = s.notices.clone();
    wait_for("Client verbunden", || {
        notices.lock().unwrap().iter().any(|n| matches!(n, ServerNotice::ClientConnected { name, .. } if name == "laptop"))
    })
    .await;
    let mut tracker = HotkeyTracker::new();

    // Strg+Alt+2: in die Mitte des Laptops
    press(&s, &mut tracker, &[CTRL, ALT, DIGIT2]);
    let (capture, injected) = (s.capture.clone(), s.injected.clone());
    wait_for("Wechsel per Kürzel", || capture.0.lock().unwrap().contains(&CaptureCall::Remote)).await;
    wait_for("Cursor in der Mitte", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 640, y: 400 })).await;

    s.events.send(CaptureEvent::MouseDelta { dx: 10, dy: 10 }).unwrap();
    wait_for("Bewegung beim Client", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 650, y: 410 })).await;

    // Strg+Alt+Pfeil links: zurück an die rechte Kante des Servers, auf Höhe des Cursors
    press(&s, &mut tracker, &[CTRL, ALT, ARROW_LEFT]);
    wait_for("Rückkehr per Kürzel", || capture.0.lock().unwrap().contains(&CaptureCall::Release { x: 1918, y: 553 })).await;
    let notices = s.notices.lock().unwrap().clone();
    let cursor: Vec<&str> =
        notices.iter().filter_map(|n| if let ServerNotice::CursorOn { screen } = n { Some(screen.as_str()) } else { None }).collect();
    assert_eq!(cursor, ["laptop", "pc"]);

    // Die Kürzel selbst erreichen den Client nie; was er an Zusatztasten bekam, ist wieder losgelassen.
    let last = |log: &[Inject], key: KeyCode| {
        log.iter().rev().find_map(|i| match *i {
            Inject::Key { key: k, down } if k == key => Some(down),
            _ => None,
        })
    };
    wait_for("Zusatztasten beim Client losgelassen", || {
        let log = injected.0.lock().unwrap();
        log.contains(&Inject::Key { key: ALT, down: true }) && [CTRL, ALT].iter().all(|&k| last(&log, k) == Some(false))
    })
    .await;
    let log = injected.0.lock().unwrap().clone();
    assert!(!log.iter().any(|i| matches!(i, Inject::Key { key, .. } if *key == DIGIT2 || *key == ARROW_LEFT)), "{log:?}");
}

#[tokio::test]
async fn files_are_dragged_to_client() {
    let s = start(true, None);
    let base = std::env::temp_dir().join(format!("qross-dnd-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (source, target) = (base.join("quelle"), base.join("ziel"));
    std::fs::create_dir_all(source.join("Projekt/sub")).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    let big: Vec<u8> = (0..8 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    std::fs::write(source.join("gross.bin"), &big).unwrap();
    std::fs::write(source.join("Projekt/sub/notiz.txt"), "Grüße").unwrap();
    *s.injected.1.lock().unwrap() = Some(target.clone());

    // Mit gedrückter Taste an der Kante: erst wird geprüft, ob Dateien gezogen werden
    let held = CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: true };
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Drag-Prüfung", || {
        let _ = events.send(held.clone());
        capture.0.lock().unwrap().contains(&CaptureCall::ProbeDrag { x: 1919, y: 540 })
    })
    .await;
    s.events
        .send(CaptureEvent::DragProbeResult { files: vec![source.join("gross.bin"), source.join("Projekt")] })
        .unwrap();
    wait_for("Wechsel mit Dateien", || {
        let calls = capture.0.lock().unwrap();
        calls.contains(&CaptureCall::CancelLocalDrag) && calls.contains(&CaptureCall::Remote)
    })
    .await;
    let injected = s.injected.clone();
    wait_for("Enter beim Client", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 399 })).await;

    s.events.send(CaptureEvent::MouseDelta { dx: 50, dy: 0 }).unwrap();
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: false }).unwrap();

    let (file, note) = (target.join("gross.bin"), target.join("Projekt/sub/notiz.txt"));
    wait_up_to(Duration::from_secs(20), "abgelegte Dateien", || file.exists() && note.exists()).await;
    assert_eq!(std::fs::read(&file).unwrap(), big);
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "Grüße");
    let log = injected.0.lock().unwrap().clone();
    assert!(
        !log.iter().any(|i| matches!(i, Inject::Button { .. })),
        "die Zieh-Taste darf auf dem Client nicht gedrückt werden: {log:?}"
    );
    assert!(log.contains(&Inject::MoveTo { x: 51, y: 399 }), "Drop an der Cursorposition");
    let _ = std::fs::remove_dir_all(&base);
}

/// Ein „Screenshot“ als PNG (Farbverlauf, damit die Kompression etwas zu tun hat)
fn screenshot_png(width: u32, height: u32) -> Vec<u8> {
    let pixels = (0..width * height)
        .flat_map(|i| {
            let (x, y) = (i % width, i / width);
            [(x * 255 / width) as u8, (y * 255 / height) as u8, ((x ^ y) & 0xFF) as u8, 255]
        })
        .collect();
    qross_platform::clipformat::rgba_to_png(&qross_platform::clipformat::Rgba { width, height, pixels }).unwrap()
}

#[tokio::test]
async fn images_and_html_travel_through_the_clipboard() {
    let mut s = start(true, None);
    let from_server = ClipboardData {
        text: Some("Grüße".into()),
        html: Some("<p><b>Grüße</b> aus Qross</p>".into()),
        png: Some(screenshot_png(800, 450)),
    };
    s.server_clip.write(&from_server).unwrap();

    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let client_clip = s.client_clip.clone();
    wait_for("Bild und HTML beim Client", || client_clip.clone().read().as_ref() == Some(&from_server)).await;

    // Ein reines Bild (z. B. Win+Shift+S) zurück zum Server
    let from_client = ClipboardData { png: Some(screenshot_png(320, 200)), ..ClipboardData::default() };
    s.client_clip.write(&from_client).unwrap();
    let injected = s.injected.clone();
    wait_for("Enter beim Client", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 399 })).await;
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let server_clip = s.server_clip.clone();
    wait_for("Bild beim Server", || server_clip.clone().read().as_ref() == Some(&from_client)).await;
}

/// Kopiert wie ein Passwortmanager: Inhalt plus Markierung
fn copy_secret(clipboard: &SharedClipboard, text: &str, marker: &'static str) {
    let mut inner = clipboard.0.lock().unwrap();
    inner.write(&ClipboardData::from_text(text)).unwrap();
    inner.conceal(marker);
}

/// Wie `enter_laptop`, auch wenn der Cursor schon einmal dort war
async fn reenter_laptop(s: &Setup) {
    s.capture.0.lock().unwrap().clear();
    s.injected.0.lock().unwrap().clear();
    enter_laptop(s).await;
}

/// Vom Laptop zurück zum Server und warten, bis der Cursor dort ist
async fn leave_laptop(s: &Setup) {
    s.capture.0.lock().unwrap().clear();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("Rückkehr zum Server", || released(&capture)).await;
}

#[tokio::test]
async fn concealed_clipboard_content_stays_on_its_computer() {
    let mut s = start(true, None);
    s.server_clip.set_text("Notiz").unwrap();
    enter_laptop(&s).await;
    let (server_clip, client_clip) = (s.server_clip.clone(), s.client_clip.clone());
    wait_for("Notiz beim Client", || client_clip.clone().text().as_deref() == Some("Notiz")).await;

    // Auf dem Server ein Passwort aus KeePassXC kopiert, dann zum Laptop
    leave_laptop(&s).await;
    copy_secret(&s.server_clip, "Passwort vom Server", "ExcludeClipboardContentFromMonitorProcessing");
    reenter_laptop(&s).await;
    let notices = s.notices.clone();
    wait_for("Hinweis beim Server", || notices.lock().unwrap().iter().any(|n| matches!(n, ServerNotice::ClipboardConcealed)))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.client_clip.text().as_deref(), Some("Notiz"), "das Passwort kommt nicht an");

    // Der Laptop schickt seinen alten Stand nicht zurück: Das Passwort bleibt auf dem Server.
    leave_laptop(&s).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.server_clip.text().as_deref(), Some("Passwort vom Server"));

    // Umgekehrt: auf dem Laptop ein Passwort kopiert (KDE-Hinweis), zurück zum Server
    reenter_laptop(&s).await;
    copy_secret(&s.client_clip, "Passwort vom Laptop", "x-kde-passwordManagerHint");
    leave_laptop(&s).await;
    let client_notices = s.client_notices.clone();
    wait_for("Hinweis beim Client", || {
        client_notices.lock().unwrap().iter().any(|n| matches!(n, ClientNotice::ClipboardConcealed))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.server_clip.text().as_deref(), Some("Passwort vom Server"), "das Passwort vom Laptop kommt nicht an");

    // Der nächste normale Inhalt wird in beide Richtungen wieder geteilt.
    reenter_laptop(&s).await;
    s.client_clip.set_text("Link vom Laptop").unwrap();
    leave_laptop(&s).await;
    wait_for("Link beim Server", || server_clip.clone().text().as_deref() == Some("Link vom Laptop")).await;
    s.server_clip.set_text("Gruß vom Server").unwrap();
    reenter_laptop(&s).await;
    wait_for("Gruß beim Client", || client_clip.clone().text().as_deref() == Some("Gruß vom Server")).await;
    let hints = s.notices.lock().unwrap().iter().filter(|n| matches!(n, ServerNotice::ClipboardConcealed)).count();
    assert_eq!(hints, 1, "ein Hinweis je zurückgehaltenem Inhalt");
}

/// Quellordner mit einer größeren Datei und einem Ordner samt Unterordner und leerem Ordner
fn copied_files(test: &str) -> (PathBuf, Vec<u8>) {
    let base = std::env::temp_dir().join(format!("qross-clip-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("Projekt/sub")).unwrap();
    std::fs::create_dir_all(base.join("Projekt/leer")).unwrap();
    let report: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
    std::fs::write(base.join("Bericht.pdf"), &report).unwrap();
    std::fs::write(base.join("Projekt/sub/notiz.txt"), "Grüße").unwrap();
    (base, report)
}

/// Die angekommenen Dateien liegen im Ablageordner für die Zwischenablage und sind vollständig.
fn assert_pasteable(paths: &[PathBuf], report: &[u8]) {
    let store = clipfiles::default_store();
    assert_eq!(paths.len(), 2, "{paths:?}");
    assert!(paths.iter().all(|p| p.starts_with(&store)), "{paths:?} liegen nicht in {}", store.display());
    assert_eq!(paths[0].file_name().unwrap(), "Bericht.pdf");
    assert_eq!(std::fs::read(&paths[0]).unwrap(), report);
    assert_eq!(std::fs::read_to_string(paths[1].join("sub/notiz.txt")).unwrap(), "Grüße");
    assert!(paths[1].join("leer").is_dir(), "leere Ordner kommen mit");
    let _ = std::fs::remove_dir_all(paths[0].parent().unwrap());
}

#[tokio::test]
async fn copied_files_reach_the_client_clipboard() {
    let mut s = start(true, None);
    let (source, report) = copied_files("zum-client");
    let names = ClipboardData::from_text("Bericht.pdf\nProjekt");
    s.server_clip.write_files(&[source.join("Bericht.pdf"), source.join("Projekt")], &names).unwrap();

    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let client_clip = s.client_clip.clone();
    wait_up_to(Duration::from_secs(20), "Dateien in der Zwischenablage des Clients", || {
        !client_clip.clone().files().is_empty()
    })
    .await;
    assert_eq!(s.client_clip.read(), Some(names), "Dateinamen als Text kommen mit");
    assert_pasteable(&s.client_clip.files(), &report);
    let _ = std::fs::remove_dir_all(&source);
}

#[tokio::test]
async fn copied_files_travel_back_to_the_server() {
    let mut s = start(true, None);
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let injected = s.injected.clone();
    wait_for("Enter beim Client", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 399 })).await;

    // Auf dem Laptop im Dateimanager kopiert, dann zurück zum Server
    let (source, report) = copied_files("zum-server");
    s.client_clip.write_files(&[source.join("Bericht.pdf"), source.join("Projekt")], &ClipboardData::default()).unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let server_clip = s.server_clip.clone();
    wait_up_to(Duration::from_secs(20), "Dateien in der Zwischenablage des Servers", || {
        !server_clip.clone().files().is_empty()
    })
    .await;
    assert_pasteable(&s.server_clip.files(), &report);
    let _ = std::fs::remove_dir_all(&source);
}

#[tokio::test]
async fn too_large_files_stay_put_but_their_names_arrive() {
    let mut s = start_with(true, None, 1024 * 1024);
    let (source, _) = copied_files("zu-gross");
    s.server_clip.write_files(&[source.join("Bericht.pdf")], &ClipboardData::from_text("Bericht.pdf")).unwrap();

    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let client_clip = s.client_clip.clone();
    wait_for("Dateiname beim Client", || client_clip.clone().text().as_deref() == Some("Bericht.pdf")).await;
    assert!(s.client_clip.files().is_empty(), "3 MB über der Grenze von 1 MB werden nicht übertragen");
    let _ = std::fs::remove_dir_all(&source);
}

/// Cursor zum Laptop bringen und warten, bis er dort angekommen ist
async fn enter_laptop(s: &Setup) {
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let injected = s.injected.clone();
    wait_for("Enter beim Client", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 399 })).await;
}

fn released(capture: &RecordingCapture) -> bool {
    capture.0.lock().unwrap().iter().any(|c| matches!(c, CaptureCall::Release { .. }))
}

#[tokio::test]
async fn files_dragged_on_the_client_are_dropped_on_the_server() {
    let s = start(true, None);
    let (source, report) = copied_files("rueckwaerts");
    let target = source.with_file_name(format!("qross-rueckwaerts-ziel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();
    *s.capture.1.lock().unwrap() = Some(target.clone());
    enter_laptop(&s).await;

    // Auf dem Laptop Dateien greifen und an die Kante zum PC ziehen
    *s.injected.2.lock().unwrap() = vec![source.join("Bericht.pdf"), source.join("Projekt")];
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: true }).unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("Rückkehr zum Server mit den Dateien", || {
        capture.0.lock().unwrap().contains(&CaptureCall::AwaitLocalDrop)
    })
    .await;
    assert!(released(&s.capture), "der Cursor ist wieder auf dem Server");

    // Über einem Ordner auf dem Server loslassen
    s.events.send(CaptureEvent::LocalDrop { x: 300, y: 200 }).unwrap();
    let (file, note) = (target.join("Bericht.pdf"), target.join("Projekt/sub/notiz.txt"));
    wait_up_to(Duration::from_secs(20), "abgelegte Dateien", || file.exists() && note.exists()).await;
    assert_eq!(std::fs::read(&file).unwrap(), report);
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "Grüße");
    assert!(target.join("Projekt/leer").is_dir());
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
}

#[tokio::test]
async fn holding_a_button_without_files_keeps_the_cursor_on_the_client() {
    let s = start(true, None);
    enter_laptop(&s).await;
    // Z. B. ein Fenster an den Rand schieben: kein Wechsel, solange die Taste gedrückt ist
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: true }).unwrap();
    for _ in 0..5 {
        s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(!released(&s.capture), "mit gedrückter Taste ohne Dateien bleibt der Cursor auf dem Client");

    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: false }).unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("Rückkehr nach dem Loslassen", || released(&capture)).await;
}

#[tokio::test]
async fn unknown_client_is_rejected() {
    let s = start(false, None);
    for _ in 0..40 {
        let _ = s.events.send(edge_hit());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(s.capture.0.lock().unwrap().is_empty(), "ein nicht freigegebener Client darf keine Eingaben bekommen");
    assert!(!s.client_task.is_finished(), "Client versucht es weiter (Freigabe kann nachträglich erfolgen)");
}

#[tokio::test]
async fn wrong_server_fingerprint_stops_client() {
    let wrong = Identity::generate().unwrap().fingerprint();
    let mut s = start(true, Some(wrong));
    let result = tokio::time::timeout(Duration::from_secs(5), &mut s.client_task).await.expect("Client hätte aufgeben müssen");
    let err = result.unwrap().unwrap_err();
    assert!(err.to_string().contains("Fingerprint"), "{err:#}");
}

/// Wartet, bis der Server diese Fläche und Monitore des Clients kennt (Meldung für die Oberfläche).
async fn wait_for_monitors(s: &Setup, bounds: Rect, monitors: &[Rect]) {
    let notices = s.notices.clone();
    wait_up_to(Duration::from_secs(10), "Monitore beim Server", || {
        notices.lock().unwrap().iter().any(|n| {
            matches!(n, ServerNotice::ClientScreenChanged { name, screen, monitors: m }
                if name == "laptop" && *screen == bounds && m == monitors)
        })
    })
    .await;
}

/// Eine Bewegung senden und warten, bis der Server sie ausgewertet hat. Das Rad-Ereignis danach
/// trennt sie von der nächsten Bewegung (wartende Bewegungen fasst der Server zusammen) und kommt
/// erst beim Client an, wenn die Bewegung verarbeitet ist.
async fn move_by(s: &Setup, dx: i32, dy: i32, marker: i16) {
    s.events.send(CaptureEvent::MouseDelta { dx, dy }).unwrap();
    s.events.send(CaptureEvent::Wheel { dx: 0, dy: marker }).unwrap();
    let injected = s.injected.clone();
    wait_for("Markierung beim Client", || injected.0.lock().unwrap().contains(&Inject::Wheel { dx: 0, dy: marker }))
        .await;
}

#[tokio::test]
async fn cursor_stays_on_the_monitors_of_a_client() {
    let s = start_client_screen(true, None, clipfiles::DEFAULT_LIMIT, MAC, vec![MAC_BUILTIN, MAC_EXTERNAL]);
    wait_for_monitors(&s, MAC, &[MAC_BUILTIN, MAC_EXTERNAL]).await;

    // Rechts oben am PC hinaus: beim Mac liegt links oben kein Monitor, also auf die Oberkante
    // des MacBooks.
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 100, buttons_down: false });
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let injected = s.injected.clone();
    wait_for("Enter auf dem MacBook", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 418 })).await;

    // Auf den externen Monitor, dann nach links in die tote Fläche über dem MacBook
    move_by(&s, 1600, -100, 1).await;
    move_by(&s, -200, 0, 2).await;
    // Kräftig weiter nach links: hier liegt kein Monitor an der äußeren Kante, also kein Wechsel
    move_by(&s, -5000, 0, 3).await;
    assert!(
        !capture.0.lock().unwrap().iter().any(|c| matches!(c, CaptureCall::Release { .. })),
        "Wechsel an einer Kante ohne Monitor"
    );
    // Hinunter aufs MacBook und über dessen linke Kante zurück zum PC
    move_by(&s, -100, 500, 4).await;
    s.events.send(CaptureEvent::MouseDelta { dx: -5000, dy: 0 }).unwrap();
    wait_for("Rückkehr zum Server", || capture.0.lock().unwrap().contains(&CaptureCall::Release { x: 1918, y: 730 }))
        .await;

    let expected = [(1, 418), (1601, 318), (1512, 318), (1412, 818)];
    wait_for("Bewegungen beim Client", || {
        let log = injected.0.lock().unwrap();
        expected.iter().all(|&(x, y)| log.contains(&Inject::MoveTo { x, y }))
    })
    .await;
    let log = injected.0.lock().unwrap().clone();
    for inject in &log {
        if let Inject::MoveTo { x, y } = *inject {
            assert!(MAC_BUILTIN.contains(x, y) || MAC_EXTERNAL.contains(x, y), "({x}, {y}) liegt auf keinem Monitor: {log:?}");
        }
    }
}

#[tokio::test]
async fn changed_monitors_are_reported_again() {
    let s = start_client_screen(true, None, clipfiles::DEFAULT_LIMIT, MAC, vec![MAC_BUILTIN, MAC_EXTERNAL]);
    wait_for_monitors(&s, MAC, &[MAC_BUILTIN, MAC_EXTERNAL]).await;

    // Externen Monitor abziehen: macOS legt das MacBook nach (0, 0)
    let builtin = Rect::new(0, 0, 1512, 982);
    *s.client_screen.lock().unwrap() = (builtin, vec![builtin]);
    wait_for_monitors(&s, builtin, &[builtin]).await;

    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let injected = s.injected.clone();
    wait_for("Enter nach der Änderung", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 490 })).await;
}

/// Wartet auf die nächste Meldung, die `pick` annimmt.
async fn next_notice<N, T>(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<N>,
    what: &str,
    mut pick: impl FnMut(N) -> Option<T>,
) -> T {
    let wait = async {
        loop {
            let notice = rx.recv().await.unwrap_or_else(|| panic!("Meldungen beendet vor: {what}"));
            if let Some(found) = pick(notice) {
                return found;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .unwrap_or_else(|_| panic!("Zeitüberschreitung beim Warten auf: {what}"))
}

#[tokio::test]
async fn connection_quality_is_measured_on_both_sides() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let server_id = Identity::generate().unwrap();
    let client_id = Identity::generate().unwrap();
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let (server_tx, mut server_rx) = tokio::sync::mpsc::unbounded_channel();
    let server = Server::bind(config, &server_id, trust).unwrap().with_notices(server_tx);
    let addr = server.local_addr().unwrap();
    let (_events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(RecordingCapture::default()),
        events: events_rx,
        clipboard: Box::new(SharedClipboard::default()),
        screen: Box::new(|| PC),
        monitors: Box::new(Vec::new),
        game: Box::new(GameState::default),
    }));
    let (client_tx, mut client_rx) = tokio::sync::mpsc::unbounded_channel();
    let client_task = tokio::spawn(client::run_with_notices(
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
            clipboard: Box::new(SharedClipboard::default()),
            screen: Box::new(|| LAPTOP),
            monitors: Box::new(Vec::new),
            game: Box::new(GameState::default),
        },
        client_tx,
    ));

    let (name, at_server) = next_notice(&mut server_rx, "Messpunkt beim Server", |n| match n {
        ServerNotice::Quality { name, quality } => Some((name, quality)),
        _ => None,
    })
    .await;
    assert_eq!(name, "laptop");
    let client_quality = |n: ClientNotice| match n {
        ClientNotice::Quality(q) => Some(q),
        _ => None,
    };
    let first = next_notice(&mut client_rx, "Messpunkt beim Client", client_quality).await;
    let second = next_notice(&mut client_rx, "zweiter Messpunkt beim Client", client_quality).await;
    let third = next_notice(&mut client_rx, "dritter Messpunkt beim Client", client_quality).await;

    for q in [at_server, first, second, third] {
        assert!(q.rtt_ms > 0.0 && q.rtt_ms < 1000.0, "{q:?}");
        assert_eq!(q.loss_pct, 0.0, "auf Loopback geht nichts verloren: {q:?}");
        assert!(q.jitter_ms >= 0.0, "{q:?}");
    }
    // Ohne Eingaben laufen jede Sekunde Keep-alives in beide Richtungen; über zwei Sekunden
    // muss also Durchsatz zu sehen sein.
    assert!(second.tx_bytes_per_s + third.tx_bytes_per_s > 0, "{second:?} {third:?}");
    assert!(second.rx_bytes_per_s + third.rx_bytes_per_s > 0, "{second:?} {third:?}");
    server_task.abort();
    client_task.abort();
}

// ---------------------------------------------------------------- Spielemodus

const FULLSCREEN: GameState = GameState { fullscreen: true, captured: false };
const CAPTURED: GameState = GameState { fullscreen: false, captured: true };

/// Wartet auf die Meldung, dass der Spielemodus den Cursor auf `screen` hält (`None`: wieder frei).
async fn wait_for_game_hold(s: &Setup, screen: &str, game: Option<GameState>) {
    let notices = s.notices.clone();
    wait_for("Meldung des Spielemodus", || {
        notices.lock().unwrap().iter().any(|n| {
            matches!(n, ServerNotice::GameHold { screen: on, game: g } if on == screen && *g == game)
        })
    })
    .await;
}

async fn wait_for_client(s: &Setup) {
    let notices = s.notices.clone();
    wait_for("Client verbunden", || {
        notices.lock().unwrap().iter().any(|n| matches!(n, ServerNotice::ClientConnected { name, .. } if name == "laptop"))
    })
    .await;
}

#[tokio::test]
async fn fullscreen_on_the_server_keeps_the_cursor_there() {
    let s = start(true, None);
    *s.server_game.lock().unwrap() = FULLSCREEN;
    wait_for_game_hold(&s, "pc", Some(FULLSCREEN)).await;
    wait_for_client(&s).await;

    for _ in 0..20 {
        s.events.send(edge_hit()).unwrap();
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!s.capture.0.lock().unwrap().contains(&CaptureCall::Remote), "Vollbild auf dem Server: kein Wechsel");

    // Das Spiel endet: Der Cursor wechselt wieder
    *s.server_game.lock().unwrap() = GameState::default();
    wait_for_game_hold(&s, "pc", None).await;
    enter_laptop(&s).await;
}

#[tokio::test]
async fn a_game_on_the_client_gets_relative_moves_and_holds_the_cursor() {
    let s = start(true, None);
    enter_laptop(&s).await;
    *s.client_game.lock().unwrap() = CAPTURED;
    wait_for_game_hold(&s, "laptop", Some(CAPTURED)).await;

    // Weit über die linke Kante zum PC: Das Spiel bekommt die ganze Bewegung, gewechselt wird nicht.
    move_by(&s, -500, 7, 1).await;
    move_by(&s, -500, 0, 2).await;
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: true }).unwrap();
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: false }).unwrap();
    let injected = s.injected.clone();
    let expected = [
        Inject::MoveBy { dx: -500, dy: 7 },
        Inject::MoveBy { dx: -500, dy: 0 },
        Inject::Button { button: MouseButton::Left, down: false },
    ];
    wait_for("Bewegungen und Klick beim Client", || {
        let log = injected.0.lock().unwrap();
        expected.iter().all(|e| log.contains(e))
    })
    .await;
    assert!(!released(&s.capture), "der Spielemodus hält den Cursor auf dem Client");

    let log = injected.0.lock().unwrap().clone();
    let first = log.iter().position(|i| *i == expected[0]).unwrap();
    assert!(
        !log[first..].iter().any(|i| matches!(i, Inject::MoveTo { .. })),
        "keine absoluten Sprünge, auch nicht beim Klick: {log:?}"
    );

    // Das Spiel endet: wieder Positionen, und die linke Kante führt zurück zum PC
    s.notices.lock().unwrap().clear();
    *s.client_game.lock().unwrap() = GameState::default();
    wait_for_game_hold(&s, "laptop", None).await;
    move_by(&s, 5, 0, 3).await;
    assert!(injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 5, y: 406 }));
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("Rückkehr zum Server", || released(&capture)).await;
}

#[tokio::test]
async fn the_lock_key_releases_the_game_hold() {
    let s = start(true, None);
    enter_laptop(&s).await;
    *s.client_game.lock().unwrap() = FULLSCREEN;
    wait_for_game_hold(&s, "laptop", Some(FULLSCREEN)).await;

    s.events.send(CaptureEvent::ToggleLock).unwrap();
    wait_for_game_hold(&s, "laptop", None).await;
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("Rückkehr zum Server", || released(&capture)).await;
}

/// Baut eine QUIC-Verbindung mit der Identität `id` auf, ohne den Qross-Client.
async fn connect_raw(addr: std::net::SocketAddr, id: &Identity) -> (quinn::Endpoint, quinn::Connection) {
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(qross_net::tls::client_config(id).unwrap());
    let conn = endpoint.connect(addr, qross_net::tls::SERVER_NAME).unwrap().await.unwrap();
    (endpoint, conn)
}

/// So sah `Datagram` bis Qross 0.5 aus.
#[derive(Deserialize, Debug, Clone)]
#[allow(dead_code)]
enum DatagramV05 {
    MouseMove(MouseMove),
}

/// So sah `Control` in Qross 0.5 aus.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
enum ControlV05 {
    Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
    Welcome { version: u16, server_name: String, platform: Platform },
    Reject { reason: String },
    ScreenChanged(ScreenInfo),
    Monitors(Vec<Rect>),
}

#[tokio::test]
async fn an_older_client_stays_connected_and_gets_positions() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let server_id = Identity::generate().unwrap();
    let client_id = Identity::generate().unwrap();
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let server = Server::bind(config, &server_id, trust).unwrap();
    let addr = server.local_addr().unwrap();
    let capture = RecordingCapture::default();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(SharedClipboard::default()),
        screen: Box::new(|| PC),
        monitors: Box::new(Vec::new),
        game: Box::new(GameState::default),
    }));

    // Ein Client wie Qross 0.5: Hello, Welcome, danach meldet er nie einen Spielemodus.
    let (_endpoint, conn) = connect_raw(addr, &client_id).await;
    let (mut control, mut control_rx) = conn.open_bi().await.unwrap();
    let screen = ScreenInfo { bounds: LAPTOP };
    let hello = Control::Hello { version: PROTOCOL_VERSION, name: "laptop".into(), screen, platform: Platform::Windows };
    write_frame(&mut control, &hello).await.unwrap();
    let welcome = read_frame::<_, ControlV05>(&mut control_rx).await.unwrap();
    assert!(matches!(welcome, Some(ControlV05::Welcome { version: SERVER_VERSION, .. })), "{welcome:?}");

    let datagrams = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let (conn, datagrams) = (conn.clone(), datagrams.clone());
        tokio::spawn(async move {
            while let Ok(bytes) = conn.read_datagram().await {
                let decoded = postcard::from_bytes::<DatagramV05>(&bytes).map_err(|e| e.to_string());
                datagrams.lock().unwrap().push(decoded);
            }
        })
    };
    let calls = capture.clone();
    wait_for("Wechsel zum alten Client", || {
        let _ = events.send(edge_hit());
        calls.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    for _ in 0..10 {
        events.send(CaptureEvent::MouseDelta { dx: 20, dy: 1 }).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let received = datagrams.clone();
    wait_for("Positionen beim alten Client", || received.lock().unwrap().len() >= 5).await;
    let log = datagrams.lock().unwrap().clone();
    assert!(log.iter().all(Result::is_ok), "ein Client aus 0.5 muss jedes Datagramm lesen können: {log:?}");
    assert!(conn.close_reason().is_none(), "der alte Client bleibt verbunden");
    reader.abort();
    server_task.abort();
}

/// Ein Server wie Qross 0.5 bzw. 0.6 (`welcome_version`). Ein Client mit Vollbildprogramm meldet
/// sich an; geliefert wird alles, was er danach auf dem Control-Stream schickt (als Rohdaten).
async fn client_reports_to(welcome_version: u16) -> Vec<Vec<u8>> {
    client_reports_with(welcome_version, RecordingInjector::default()).await
}

/// Wie `client_reports_to`, mit eigenem Einspeisen (z. B. einem, das Zeichen eingeben kann)
async fn client_reports_with(welcome_version: u16, injector: RecordingInjector) -> Vec<Vec<u8>> {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let server_id = Identity::generate().unwrap();
    let endpoint =
        quinn::Endpoint::server(qross_net::tls::server_config(&server_id).unwrap(), "127.0.0.1:0".parse().unwrap())
            .unwrap();
    let addr = endpoint.local_addr().unwrap();
    let client_task = tokio::spawn(client::run(
        ClientConfig {
            name: "laptop".into(),
            server: addr.to_string(),
            server_fingerprint: Some(server_id.fingerprint()),
            clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
            share_concealed: false,
        },
        Identity::generate().unwrap(),
        TrustStore::in_memory(),
        ClientPlatform {
            injector: Box::new(injector),
            clipboard: Box::new(SharedClipboard::default()),
            screen: Box::new(|| LAPTOP),
            monitors: Box::new(Vec::new),
            game: Box::new(|| FULLSCREEN),
        },
    ));

    let conn = endpoint.accept().await.unwrap().await.unwrap();
    let (mut control, mut control_rx) = conn.accept_bi().await.unwrap();
    let hello = read_frame::<_, Control>(&mut control_rx).await.unwrap();
    assert!(matches!(hello, Some(Control::Hello { version: PROTOCOL_VERSION, .. })), "{hello:?}");
    let welcome = Control::Welcome { version: welcome_version, server_name: "pc".into(), platform: Platform::Windows };
    write_frame(&mut control, &welcome).await.unwrap();

    // Eine Weile mitlesen; frühestens nach einer Abfragerunde wüsste der Client vom Vollbild.
    let mut frames = Vec::new();
    let _ = tokio::time::timeout(Duration::from_millis(1200), async {
        loop {
            let mut len = [0u8; 4];
            if control_rx.read_exact(&mut len).await.is_err() {
                return;
            }
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            if control_rx.read_exact(&mut body).await.is_err() {
                return;
            }
            frames.push(body);
        }
    })
    .await;
    assert!(conn.close_reason().is_none(), "der Client bleibt verbunden");
    assert!(!client_task.is_finished());
    client_task.abort();
    frames
}

#[tokio::test]
async fn the_client_reports_fullscreen_only_to_servers_that_know_it() {
    // Qross 0.5 trennt bei unbekannten Control-Nachrichten: Es bekommt keine.
    for frame in client_reports_to(4).await {
        let msg = postcard::from_bytes::<ControlV05>(&frame);
        assert!(msg.is_ok(), "ein Server aus 0.5 kann {msg:?} nicht lesen");
    }
    // Ein neuer Server erfährt vom Vollbild
    let frames = client_reports_to(SERVER_VERSION).await;
    let messages: Vec<Control> = frames.iter().map(|f| postcard::from_bytes(f).unwrap()).collect();
    assert_eq!(messages, vec![Control::Game(FULLSCREEN)]);
}

/// So groß, dass die Übertragung auch im Release-Build lange genug läuft, um sie abzubrechen
const BIG: u64 = 256 * 1024 * 1024;

/// Große Datei aus Nullen (sofort angelegt) mit eindeutigem Namen
fn big_file(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::File::create(&path).unwrap().set_len(BIG).unwrap();
    path
}

fn server_progress(s: &Setup) -> Vec<Progress> {
    let notices = s.notices.lock().unwrap();
    notices.iter().filter_map(|n| if let ServerNotice::Transfer(p) = n { Some(p.clone()) } else { None }).collect()
}

fn client_progress(s: &Setup) -> Vec<Progress> {
    let notices = s.client_notices.lock().unwrap();
    notices.iter().filter_map(|n| if let ClientNotice::Transfer(p) = n { Some(p.clone()) } else { None }).collect()
}

/// Wartet auf eine Fortschrittsmeldung, die `pick` erfüllt.
async fn wait_for_progress(
    what: &str,
    progress: impl Fn() -> Vec<Progress>,
    pick: impl Fn(&Progress) -> bool,
) -> Progress {
    for _ in 0..20_000 / 25 {
        if let Some(found) = progress().into_iter().find(&pick) {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Zeitüberschreitung beim Warten auf: {what}");
}

fn running(direction: Direction) -> impl Fn(&Progress) -> bool {
    move |p| p.direction == direction && p.phase == Phase::Running && p.bytes > 0
}

fn ended(transfer: u64, phase: Phase) -> impl Fn(&Progress) -> bool {
    move |p| p.transfer == transfer && p.phase == phase
}

/// Teile von `name` im Drag-Staging irgendeines Laufs (jeder Server und Client hat dort einen eigenen
/// Ordner; die Übertragungsnummern beginnen je Server bei 1)
fn drag_leftovers(transfer: u64, name: &str) -> Vec<PathBuf> {
    let Ok(runs) = std::fs::read_dir(files::drag_staging_base()) else { return Vec::new() };
    runs.flatten()
        .flat_map(|run| {
            let dir = run.path().join(transfer.to_string());
            [dir.join(name), dir.join(format!("{name}.part"))]
        })
        .filter(|p| p.exists())
        .collect()
}

#[tokio::test]
async fn dragged_files_report_progress_and_the_receiver_can_cancel() {
    let s = start(true, None);
    let base = std::env::temp_dir().join(format!("qross-abbruch-drag-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let name = "abbrechen-gezogen.bin";
    let source = big_file(&base.join("quelle"), name);
    let target = base.join("ziel");
    std::fs::create_dir_all(&target).unwrap();
    *s.injected.1.lock().unwrap() = Some(target.clone());

    let held = CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: true };
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Drag-Prüfung", || {
        let _ = events.send(held.clone());
        capture.0.lock().unwrap().contains(&CaptureCall::ProbeDrag { x: 1919, y: 540 })
    })
    .await;
    s.events.send(CaptureEvent::DragProbeResult { files: vec![source] }).unwrap();

    // Beide Seiten melden Stand, Gesamtgröße und Namen.
    let sending = wait_for_progress("Fortschritt beim Server", || server_progress(&s), running(Direction::Sending)).await;
    assert_eq!((sending.kind, sending.peer.as_str(), sending.total), (Kind::Drag, "laptop", BIG));
    assert_eq!(sending.names, [name]);
    assert!(sending.bytes < BIG, "läuft noch: {sending:?}");
    let id = sending.transfer;
    let receiving = wait_for_progress("Fortschritt beim Client", || client_progress(&s), |p| {
        p.transfer == id && running(Direction::Receiving)(p)
    })
    .await;
    assert_eq!((receiving.kind, receiving.peer.as_str(), receiving.total), (Kind::Drag, "pc", BIG));

    // Der Empfänger bricht ab, der Sender erfährt es über den angehaltenen Stream.
    assert!(s.client_transfers.cancel(Direction::Receiving, id));
    let cancelled = Phase::Cancelled { by_peer: false };
    wait_for_progress("Abbruch beim Client", || client_progress(&s), ended(id, cancelled)).await;
    let by_peer = Phase::Cancelled { by_peer: true };
    wait_for_progress("Abbruch beim Server", || server_progress(&s), ended(id, by_peer)).await;
    assert!(!s.client_transfers.cancel(Direction::Receiving, id), "läuft nicht mehr");

    // Loslassen legt nichts mehr ab, und im Staging bleibt nichts liegen.
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: false }).unwrap();
    wait_for("aufgeräumtes Staging", || drag_leftovers(id, name).is_empty()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0, "Zielordner bleibt leer");
    assert!(drag_leftovers(id, name).is_empty(), "{:?}", drag_leftovers(id, name));
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn copied_files_report_progress_and_the_sender_can_cancel() {
    let mut s = start(true, None);
    let base = std::env::temp_dir().join(format!("qross-abbruch-kopiert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let name = "abbrechen-kopiert.bin";
    let source = big_file(&base, name);
    s.server_clip.write_files(&[source], &ClipboardData::from_text(name)).unwrap();

    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    let sending = wait_for_progress("Fortschritt beim Server", || server_progress(&s), running(Direction::Sending)).await;
    assert_eq!((sending.kind, sending.peer.as_str(), sending.total), (Kind::Clipboard, "laptop", BIG));
    let id = sending.transfer;
    let receiving = wait_for_progress("Fortschritt beim Client", || client_progress(&s), |p| {
        p.transfer == id && running(Direction::Receiving)(p)
    })
    .await;
    assert_eq!((receiving.kind, receiving.peer.as_str(), receiving.names.clone()), (Kind::Clipboard, "pc", vec![name.to_owned()]));

    // Der Sender bricht ab, der Empfänger erfährt es über den zurückgesetzten Stream.
    assert!(s.server_transfers.cancel(Direction::Sending, id));
    wait_for_progress("Abbruch beim Server", || server_progress(&s), ended(id, Phase::Cancelled { by_peer: false })).await;
    wait_for_progress("Abbruch beim Client", || client_progress(&s), ended(id, Phase::Cancelled { by_peer: true })).await;

    let store = clipfiles::default_store().join(id.to_string());
    wait_for("aufgeräumte Ablage", || !store.exists()).await;
    assert!(s.client_clip.files().is_empty(), "nichts in der Zwischenablage");
    let failed = s.client_notices.lock().unwrap().iter().any(|n| {
        matches!(n, ClientNotice::ClipboardFiles(clipfiles::FilesNotice::Failed { .. }))
    });
    assert!(!failed, "ein Abbruch ist kein Fehler");
    let _ = std::fs::remove_dir_all(&base);
}

// ------------------------------------------------------------------ Rechte je Rechner

/// Nur „laptop“ eingeschränkt, alle anderen dürfen alles
fn laptop_rules(laptop: Sharing) -> SharingRules {
    SharingRules { default: Sharing::ALL, clients: [("laptop".to_owned(), laptop)].into() }
}

/// Zuletzt beim Client gemeldete Rechte
fn client_sharing(s: &Setup) -> Option<Sharing> {
    s.client_notices.lock().unwrap().iter().rev().find_map(|n| match n {
        ClientNotice::Sharing(sharing) => Some(*sharing),
        _ => None,
    })
}

/// Setzt die Rechte von „laptop“ und wartet, bis der Client sie kennt. Dann gelten sie auch beim
/// Server, denn der schickt sie erst, wenn er sie übernommen hat.
async fn share_with_laptop(s: &Setup, laptop: Sharing) {
    s.sharing.set(laptop_rules(laptop));
    wait_for("Rechte beim Client", || client_sharing(s) == Some(laptop)).await;
}

/// Vergisst die bisherigen Aufrufe, damit `enter_laptop` und `released` erneut warten können.
fn forget_calls(s: &Setup) {
    s.capture.0.lock().unwrap().clear();
    s.injected.0.lock().unwrap().clear();
}

fn sent_by_client(s: &Setup) -> bool {
    client_progress(s).iter().any(|p| p.direction == Direction::Sending)
}

#[tokio::test]
async fn rights_reach_the_client_at_once_without_reconnecting() {
    let s = start(true, None);
    wait_for("Rechte nach dem Verbinden", || client_sharing(&s) == Some(Sharing::ALL)).await;
    let laptop = Sharing { clipboard: Share::ToClient, files: Share::Off, drag: Share::FromClient };
    share_with_laptop(&s, laptop).await;
    // Eine Vorgabe für unbekannte Rechner betrifft „laptop“ mit eigenem Eintrag nicht.
    let mut rules = laptop_rules(laptop);
    rules.default = Sharing { clipboard: Share::Off, files: Share::Off, drag: Share::Off };
    s.sharing.set(rules);
    s.sharing.set(laptop_rules(Sharing::ALL));
    wait_for("wieder alles erlaubt", || client_sharing(&s) == Some(Sharing::ALL)).await;

    let notices = s.client_notices.lock().unwrap();
    assert_eq!(notices.iter().filter(|n| matches!(n, ClientNotice::Connected { .. })).count(), 1);
    assert!(!notices.iter().any(|n| matches!(n, ClientNotice::Disconnected { .. })), "die Verbindung bleibt bestehen");
    let seen: Vec<Sharing> =
        notices.iter().filter_map(|n| if let ClientNotice::Sharing(x) = n { Some(*x) } else { None }).collect();
    assert_eq!(seen, [Sharing::ALL, laptop, Sharing::ALL], "nur Änderungen, die den Rechner betreffen");
}

#[tokio::test]
async fn the_clipboard_travels_only_where_it_may() {
    let mut s = start(true, None);
    share_with_laptop(&s, Sharing { clipboard: Share::FromClient, ..Sharing::ALL }).await;
    s.server_clip.set_text("vom Server").unwrap();
    enter_laptop(&s).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(s.client_clip.text(), None, "zum Laptop nicht erlaubt");

    s.client_clip.set_text("vom Laptop").unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let server_clip = s.server_clip.clone();
    wait_for("Text vom Laptop beim Server", || server_clip.clone().text().as_deref() == Some("vom Laptop")).await;

    // Umgekehrt, ohne neu zu verbinden
    share_with_laptop(&s, Sharing { clipboard: Share::ToClient, ..Sharing::ALL }).await;
    s.server_clip.set_text("zweiter Text").unwrap();
    forget_calls(&s);
    enter_laptop(&s).await;
    let client_clip = s.client_clip.clone();
    wait_for("Text vom Server beim Laptop", || client_clip.clone().text().as_deref() == Some("zweiter Text")).await;
    s.client_clip.set_text("bleibt auf dem Laptop").unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("zurück beim Server", || released(&capture)).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(s.server_clip.text().as_deref(), Some("zweiter Text"), "vom Laptop nicht erlaubt");
}

#[tokio::test]
async fn copied_files_travel_only_where_they_may() {
    let mut s = start(true, None);
    // Dateien nur zum Laptop; die Namen als Text gehören zur Zwischenablage und dürfen überallhin.
    share_with_laptop(&s, Sharing { files: Share::ToClient, ..Sharing::ALL }).await;
    let (source, report) = copied_files("rechte-hin");
    let names = ClipboardData::from_text("Bericht.pdf\nProjekt");
    s.server_clip.write_files(&[source.join("Bericht.pdf"), source.join("Projekt")], &names).unwrap();
    enter_laptop(&s).await;
    let client_clip = s.client_clip.clone();
    wait_up_to(Duration::from_secs(20), "Dateien beim Laptop", || !client_clip.clone().files().is_empty()).await;
    assert_pasteable(&s.client_clip.files(), &report);

    // Zurück nur die Namen; der Laptop versucht die Dateien gar nicht erst.
    let (back, _) = copied_files("rechte-her");
    s.client_clip.write_files(&[back.join("Bericht.pdf")], &ClipboardData::from_text("Bericht.pdf")).unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let server_clip = s.server_clip.clone();
    wait_for("Name beim Server", || server_clip.clone().text().as_deref() == Some("Bericht.pdf")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(s.server_clip.files().is_empty(), "keine Dateien vom Laptop");
    assert!(!sent_by_client(&s), "der Laptop überträgt nichts");

    // Andersherum: Dateien nur vom Laptop. Zum Laptop gehen nur noch die Namen.
    share_with_laptop(&s, Sharing { files: Share::FromClient, ..Sharing::ALL }).await;
    let sent_before = server_progress(&s).len();
    s.server_clip.write_files(&[source.join("Bericht.pdf"), source.join("Projekt")], &names).unwrap();
    forget_calls(&s);
    enter_laptop(&s).await;
    let client_clip = s.client_clip.clone();
    wait_for("Namen beim Laptop", || client_clip.clone().read().as_ref() == Some(&names)).await;
    assert!(s.client_clip.files().is_empty(), "keine Dateien zum Laptop");
    assert_eq!(server_progress(&s).len(), sent_before, "der Server überträgt nichts");
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&back);
}

#[tokio::test]
async fn drag_and_drop_only_from_the_client() {
    let s = start(true, None);
    share_with_laptop(&s, Sharing { drag: Share::FromClient, ..Sharing::ALL }).await;
    // Zum Laptop nicht: Der Server sieht nicht einmal nach, ob Dateien gezogen werden.
    let held = CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: true };
    for _ in 0..8 {
        s.events.send(held.clone()).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let calls = s.capture.0.lock().unwrap().clone();
    assert!(!calls.iter().any(|c| matches!(c, CaptureCall::ProbeDrag { .. } | CaptureCall::Remote)), "{calls:?}");

    // Vom Laptop zum Server schon
    let (source, report) = copied_files("rechte-drag-her");
    let target = source.with_file_name(format!("qross-rechte-drag-her-ziel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();
    *s.capture.1.lock().unwrap() = Some(target.clone());
    enter_laptop(&s).await;
    *s.injected.2.lock().unwrap() = vec![source.join("Bericht.pdf")];
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: true }).unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("Dateien vom Laptop", || capture.0.lock().unwrap().contains(&CaptureCall::AwaitLocalDrop)).await;
    s.events.send(CaptureEvent::LocalDrop { x: 300, y: 200 }).unwrap();
    let file = target.join("Bericht.pdf");
    wait_up_to(Duration::from_secs(20), "abgelegte Datei", || file.exists()).await;
    assert_eq!(std::fs::read(&file).unwrap(), report);
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
}

#[tokio::test]
async fn drag_and_drop_only_to_the_client() {
    let s = start(true, None);
    share_with_laptop(&s, Sharing { drag: Share::ToClient, ..Sharing::ALL }).await;
    // Zum Laptop wie gewohnt
    let (source, report) = copied_files("rechte-drag-hin");
    let target = source.with_file_name(format!("qross-rechte-drag-hin-ziel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();
    *s.injected.1.lock().unwrap() = Some(target.clone());
    let held = CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: true };
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Drag-Prüfung", || {
        let _ = events.send(held.clone());
        capture.0.lock().unwrap().contains(&CaptureCall::ProbeDrag { x: 1919, y: 540 })
    })
    .await;
    s.events.send(CaptureEvent::DragProbeResult { files: vec![source.join("Bericht.pdf")] }).unwrap();
    let injected = s.injected.clone();
    wait_for("Enter beim Laptop", || injected.0.lock().unwrap().contains(&Inject::MoveTo { x: 1, y: 399 })).await;
    s.events.send(CaptureEvent::MouseDelta { dx: 50, dy: 0 }).unwrap();
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: false }).unwrap();
    let file = target.join("Bericht.pdf");
    wait_up_to(Duration::from_secs(20), "abgelegte Datei", || file.exists()).await;
    assert_eq!(std::fs::read(&file).unwrap(), report);

    // Vom Laptop zurück nicht: Der Server fragt nicht nach, die Dateien bleiben drüben und der
    // Cursor auch, solange die Taste gedrückt ist.
    *s.injected.2.lock().unwrap() = vec![source.join("Bericht.pdf")];
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: true }).unwrap();
    for _ in 0..5 {
        s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(!released(&s.capture), "mit gedrückter Taste bleibt der Cursor auf dem Laptop");
    assert_eq!(s.injected.2.lock().unwrap().len(), 1, "der Laptop gibt seinen Drag nicht ab");
    assert!(!sent_by_client(&s));
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: false }).unwrap();
    s.events.send(CaptureEvent::MouseDelta { dx: -500, dy: 0 }).unwrap();
    let capture = s.capture.clone();
    wait_for("Rückkehr nach dem Loslassen", || released(&capture)).await;
    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
}

#[tokio::test]
async fn revoking_rights_cancels_a_running_drag() {
    let s = start(true, None);
    let base = std::env::temp_dir().join(format!("qross-rechte-entzogen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let name = "rechte-entzogen.bin";
    let source = big_file(&base.join("quelle"), name);
    let target = base.join("ziel");
    std::fs::create_dir_all(&target).unwrap();
    *s.injected.1.lock().unwrap() = Some(target.clone());

    let held = CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: true };
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Drag-Prüfung", || {
        let _ = events.send(held.clone());
        capture.0.lock().unwrap().contains(&CaptureCall::ProbeDrag { x: 1919, y: 540 })
    })
    .await;
    s.events.send(CaptureEvent::DragProbeResult { files: vec![source] }).unwrap();
    let id = wait_for_progress("Fortschritt beim Server", || server_progress(&s), running(Direction::Sending)).await.transfer;
    wait_for_progress("Fortschritt beim Laptop", || client_progress(&s), |p| p.transfer == id && running(Direction::Receiving)(p))
        .await;

    // Drag & Drop zum Laptop entzogen: Die laufende Übertragung bricht sofort ab.
    s.sharing.set(laptop_rules(Sharing { drag: Share::FromClient, ..Sharing::ALL }));
    wait_for_progress("Abbruch beim Server", || server_progress(&s), ended(id, Phase::Cancelled { by_peer: false })).await;
    wait_for_progress("Abbruch beim Laptop", || client_progress(&s), ended(id, Phase::Cancelled { by_peer: true })).await;
    wait_for("aufgeräumtes Staging", || drag_leftovers(id, name).is_empty()).await;
    s.events.send(CaptureEvent::Button { button: MouseButton::Left, down: false }).unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0, "nichts abgelegt");
    assert!(drag_leftovers(id, name).is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

/// Ein Stream wie von Qross 0.6: Typ-Byte, eine Nachricht
async fn send_raw(conn: &quinn::Connection, kind: StreamKind, message: &impl serde::Serialize) {
    let mut stream = conn.open_uni().await.unwrap();
    stream.write_all(&[kind as u8]).await.unwrap();
    write_frame(&mut stream, message).await.unwrap();
    stream.finish().unwrap();
}

#[tokio::test]
async fn an_older_client_that_tries_anyway_is_refused_cleanly() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let server_id = Identity::generate().unwrap();
    let client_id = Identity::generate().unwrap();
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let nothing = Sharing { clipboard: Share::Off, files: Share::Off, drag: Share::Off };
    let (notice_tx, notice_rx) = tokio::sync::mpsc::unbounded_channel();
    let notices = collect(notice_rx);
    let server =
        Server::bind(config, &server_id, trust).unwrap().with_sharing(laptop_rules(nothing)).with_notices(notice_tx);
    let (addr, sharing) = (server.local_addr().unwrap(), server.sharing());
    let capture = RecordingCapture::default();
    let server_clip = SharedClipboard::default();
    let (_events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(server_clip.clone()),
        screen: Box::new(|| PC),
        monitors: Box::new(Vec::new),
        game: Box::new(GameState::default),
    }));

    // Ein Client wie Qross 0.6: Nach dem Welcome liest er den Control-Stream nicht mehr und kennt
    // seine Rechte deshalb nicht.
    let (_endpoint, conn) = connect_raw(addr, &client_id).await;
    let (mut control, mut control_rx) = conn.open_bi().await.unwrap();
    let hello = Control::Hello {
        version: PROTOCOL_VERSION,
        name: "laptop".into(),
        screen: ScreenInfo { bounds: LAPTOP },
        platform: Platform::Windows,
    };
    write_frame(&mut control, &hello).await.unwrap();
    let welcome = read_frame::<_, ControlV05>(&mut control_rx).await.unwrap();
    assert!(matches!(welcome, Some(ControlV05::Welcome { version: SERVER_VERSION, .. })), "{welcome:?}");

    let (source, _) = copied_files("rechte-alt");
    let entries = files::collect(&[source.join("Bericht.pdf")]).unwrap();
    let list: Vec<FileEntry> = entries.iter().map(|e| e.entry.clone()).collect();
    let sender = Transfers::new();
    // Text, kopierte Dateien samt Inhalt und gezogene Dateien, ohne dass der Server gefragt hätte
    send_raw(&conn, StreamKind::Clipboard, &ClipboardMsg::Data(ClipboardData::from_text("geheim"))).await;
    let copied = clipfiles::new_transfer_id();
    let files_msg = ClipboardMsg::Files { transfer: copied, entries: list.clone(), data: ClipboardData::from_text("Bericht.pdf") };
    send_raw(&conn, StreamKind::Clipboard, &files_msg).await;
    let _ = files::send_tracked(conn.clone(), StreamKind::ClipboardFile, copied, entries.clone(), "pc", &sender).await;
    let dragged = clipfiles::new_transfer_id();
    send_raw(&conn, StreamKind::Drag, &DragMessage::Offer { probe: 1, transfer: dragged, entries: list }).await;
    let _ = files::send_tracked(conn.clone(), StreamKind::File, dragged, entries, "pc", &sender).await;

    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(server_clip.clone().read(), None, "nichts in der Zwischenablage des Servers");
    assert!(server_clip.clone().files().is_empty());
    assert!(!clipfiles::default_store().join(copied.to_string()).exists(), "keine Reste in der Ablage");
    assert!(drag_leftovers(dragged, "Bericht.pdf").is_empty(), "keine Reste im Drag-Staging");
    assert!(!capture.0.lock().unwrap().contains(&CaptureCall::AwaitLocalDrop), "kein Ablegen");
    let transfers = notices.lock().unwrap().iter().filter(|n| matches!(n, ServerNotice::Transfer(_))).count();
    assert_eq!(transfers, 0, "keine Übertragung angenommen");
    assert!(conn.close_reason().is_none(), "der alte Client bleibt verbunden");

    // Gegenprobe ohne neue Verbindung: Mit Rechten kommt derselbe Weg an, als Text auch bei Dateien.
    sharing.set(laptop_rules(Sharing { clipboard: Share::Both, ..nothing }));
    let mut arrived = false;
    for _ in 0..100 {
        send_raw(&conn, StreamKind::Clipboard, &files_msg).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        if server_clip.clone().text().as_deref() == Some("Bericht.pdf") {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "mit Rechten kommt der Text an");
    assert!(server_clip.clone().files().is_empty(), "Dateien weiterhin nicht");
    assert!(!clipfiles::default_store().join(copied.to_string()).exists());
    server_task.abort();
    let _ = std::fs::remove_dir_all(&source);
}

// --- Zeichen statt Tastenposition ---------------------------------------------------------------

/// Server, auf dem „laptop“ auf `mode` steht, und ein Client mit dem Einspeisen `injected`
/// (`RecordingInjector::typing()` kann Zeichen eingeben, wie Windows, macOS und X11)
fn start_typing(mode: KeyMode, injected: RecordingInjector) -> Setup {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let (server_id, client_id) = (Identity::generate().unwrap(), Identity::generate().unwrap());
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let keys = KeyConfig { modes: HashMap::from([("laptop".to_owned(), mode)]), ..KeyConfig::default() };
    let (notice_tx, notice_rx) = tokio::sync::mpsc::unbounded_channel();
    let server = Server::bind(config, &server_id, trust).unwrap().with_keys(keys).with_notices(notice_tx);
    let (addr, server_transfers, sharing) = (server.local_addr().unwrap(), server.transfers(), server.sharing());

    let capture = RecordingCapture::default();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let (server_clip, client_clip) = (SharedClipboard::default(), SharedClipboard::default());
    let (server_game, client_game) = (Game::default(), Game::default());
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(server_clip.clone()),
        screen: Box::new(|| PC),
        monitors: Box::new(Vec::new),
        game: detector(&server_game),
    }));
    let client_config = ClientConfig {
        name: "laptop".into(),
        server: addr.to_string(),
        server_fingerprint: Some(server_id.fingerprint()),
        clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
        share_concealed: false,
    };
    let (client_tx, client_rx) = tokio::sync::mpsc::unbounded_channel();
    let client_transfers = Transfers::new();
    let client_screen: ClientScreen = Arc::new(Mutex::new((LAPTOP, Vec::new())));
    let client_task = tokio::spawn(client::run_with_transfers(
        client_config,
        client_id,
        TrustStore::in_memory(),
        ClientPlatform {
            injector: Box::new(injected.clone()),
            clipboard: Box::new(client_clip.clone()),
            screen: Box::new(|| LAPTOP),
            monitors: Box::new(Vec::new),
            game: detector(&client_game),
        },
        client_tx,
        client_transfers.clone(),
    ));
    Setup {
        events,
        capture,
        server_clip,
        injected,
        client_clip,
        client_screen,
        server_game,
        client_game,
        notices: collect(notice_rx),
        client_notices: collect(client_rx),
        server_transfers,
        client_transfers,
        sharing,
        server_task,
        client_task,
    }
}

const KEY_E: KeyCode = KeyCode(0x08);
const KEY_Q: KeyCode = KeyCode(0x14);
/// Deutsche Tastatur: Z (US: Y)
const KEY_Y: KeyCode = KeyCode(0x1C);
/// Deutsche Tastatur: ^ als tote Taste
const BACKQUOTE: KeyCode = KeyCode(0x35);
const ALTGR: KeyCode = KeyCode::RIGHT_ALT;

/// Was der Client an Tasten und Zeichen eingespeist hat (ohne Mausbewegungen)
fn typed(injected: &RecordingInjector) -> Vec<Inject> {
    let log = injected.0.lock().unwrap();
    log.iter().filter(|i| matches!(i, Inject::Key { .. } | Inject::Char { .. } | Inject::Shortcut { .. })).copied().collect()
}

#[tokio::test]
async fn characters_reach_a_client_set_to_them() {
    let s = start_typing(KeyMode::Chars, RecordingInjector::typing());
    let notices = s.notices.clone();
    wait_for("Client kann Zeichen eingeben", || {
        notices.lock().unwrap().iter().any(|n| matches!(n, ServerNotice::ClientTypesText { name } if name == "laptop"))
    })
    .await;
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;

    // Deutscher Server: AltGr+Q = @, Strg+Z (Taste Y), ^ e = ê – egal, welches Layout der Client hat
    for event in [
        CaptureEvent::Key { key: ALTGR, down: true },
        CaptureEvent::Typed { key: KEY_Q, chars: KeyChars::text("@", 'q') },
        CaptureEvent::Key { key: KEY_Q, down: false },
        CaptureEvent::Key { key: ALTGR, down: false },
        CaptureEvent::Key { key: CTRL, down: true },
        CaptureEvent::Typed { key: KEY_Y, chars: KeyChars { produces: Produces::Nothing, base: Some('z'), shortcut: true } },
        CaptureEvent::Key { key: KEY_Y, down: false },
        CaptureEvent::Key { key: CTRL, down: false },
        CaptureEvent::Typed { key: BACKQUOTE, chars: KeyChars { produces: Produces::Dead('^'), base: Some('^'), shortcut: false } },
        CaptureEvent::Key { key: BACKQUOTE, down: false },
        CaptureEvent::Typed { key: KEY_E, chars: KeyChars::text("e", 'e') },
        CaptureEvent::Key { key: KEY_E, down: false },
        // Pfeile bleiben Tasten
        CaptureEvent::Key { key: ARROW_LEFT, down: true },
        CaptureEvent::Key { key: ARROW_LEFT, down: false },
    ] {
        s.events.send(event).unwrap();
    }
    let injected = s.injected.clone();
    wait_for("Pfeil beim Client", || typed(&injected).contains(&Inject::Key { key: ARROW_LEFT, down: false })).await;
    assert_eq!(
        typed(&s.injected),
        vec![
            Inject::Char { ch: '@', down: true },
            Inject::Char { ch: '@', down: false },
            Inject::Key { key: CTRL, down: true },
            Inject::Shortcut { ch: 'z', key: KEY_Y, down: true },
            Inject::Shortcut { ch: 'z', key: KEY_Y, down: false },
            Inject::Key { key: CTRL, down: false },
            Inject::Char { ch: 'ê', down: true },
            Inject::Char { ch: 'ê', down: false },
            Inject::Key { key: ARROW_LEFT, down: true },
            Inject::Key { key: ARROW_LEFT, down: false },
        ],
        "AltGr erreicht den Client nie, die tote Taste wartet auf das e"
    );
}

#[tokio::test]
async fn clients_that_cannot_type_characters_get_positions() {
    // Kann der Client keine Zeichen eingeben (Wayland), meldet er es nicht – und bekommt Tasten.
    let s = start_typing(KeyMode::Chars, RecordingInjector::default());
    let (events, capture) = (s.events.clone(), s.capture.clone());
    wait_for("Wechsel zum Client", || {
        let _ = events.send(edge_hit());
        capture.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    for event in [
        CaptureEvent::Key { key: ALTGR, down: true },
        CaptureEvent::Typed { key: KEY_Q, chars: KeyChars::text("@", 'q') },
        CaptureEvent::Key { key: KEY_Q, down: false },
        CaptureEvent::Key { key: ALTGR, down: false },
    ] {
        s.events.send(event).unwrap();
    }
    let injected = s.injected.clone();
    wait_for("Tasten beim Client", || typed(&injected).len() == 4).await;
    assert_eq!(
        typed(&s.injected),
        vec![
            Inject::Key { key: ALTGR, down: true },
            Inject::Key { key: KEY_Q, down: true },
            Inject::Key { key: KEY_Q, down: false },
            Inject::Key { key: ALTGR, down: false },
        ]
    );
    let notices = s.notices.lock().unwrap();
    assert!(!notices.iter().any(|n| matches!(n, ServerNotice::ClientTypesText { .. })), "{notices:?}");
}

/// So sah `Control` in Qross 0.6 aus.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
enum ControlV06 {
    Hello { version: u16, name: String, screen: ScreenInfo, platform: Platform },
    Welcome { version: u16, server_name: String, platform: Platform },
    Reject { reason: String },
    ScreenChanged(ScreenInfo),
    Monitors(Vec<Rect>),
    Game(GameState),
}

/// So sah `Input` in Qross 0.6 aus.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[allow(dead_code)]
enum InputV06 {
    Enter { epoch: u16, x: i32, y: i32 },
    Leave,
    Key { key: KeyCode, down: bool },
    Button { button: MouseButton, down: bool, x: i32, y: i32 },
    Wheel { dx: i16, dy: i16 },
    DragStart { transfer: u64, entries: Vec<qross_proto::FileEntry> },
    Drop { transfer: u64, x: i32, y: i32 },
}

#[tokio::test]
async fn the_client_announces_typing_only_to_servers_that_know_it() {
    // Qross 0.6 meldet Version 5 und trennte bei `TypesText`: Er bekommt nur, was er lesen kann.
    for frame in client_reports_with(5, RecordingInjector::typing()).await {
        let msg = postcard::from_bytes::<ControlV06>(&frame);
        assert!(msg.is_ok(), "ein Server aus 0.6 kann {msg:?} nicht lesen");
    }
    let frames = client_reports_with(SERVER_VERSION, RecordingInjector::typing()).await;
    let messages: Vec<Control> = frames.iter().map(|f| postcard::from_bytes(f).unwrap()).collect();
    assert_eq!(messages, vec![Control::TypesText, Control::Game(FULLSCREEN)]);
}

#[tokio::test]
async fn an_older_client_set_to_characters_gets_keys_it_can_read() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let server_id = Identity::generate().unwrap();
    let client_id = Identity::generate().unwrap();
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let keys = KeyConfig { modes: HashMap::from([("laptop".to_owned(), KeyMode::Chars)]), ..KeyConfig::default() };
    let server = Server::bind(config, &server_id, trust).unwrap().with_keys(keys);
    let addr = server.local_addr().unwrap();
    let capture = RecordingCapture::default();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(SharedClipboard::default()),
        screen: Box::new(|| PC),
        monitors: Box::new(Vec::new),
        game: Box::new(GameState::default),
    }));

    // Ein Client wie Qross 0.6: Hello, Welcome, kein `TypesText`
    let (_endpoint, conn) = connect_raw(addr, &client_id).await;
    let (mut control, mut control_rx) = conn.open_bi().await.unwrap();
    let hello = Control::Hello { version: PROTOCOL_VERSION, name: "laptop".into(), screen: ScreenInfo { bounds: LAPTOP }, platform: Platform::MacOs };
    write_frame(&mut control, &hello).await.unwrap();
    let welcome = read_frame::<_, ControlV06>(&mut control_rx).await.unwrap();
    assert!(matches!(welcome, Some(ControlV06::Welcome { version: SERVER_VERSION, .. })), "{welcome:?}");

    let inputs = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let (conn, inputs) = (conn.clone(), inputs.clone());
        tokio::spawn(async move {
            let mut stream = conn.accept_uni().await.unwrap();
            let mut kind = [0u8; 1];
            stream.read_exact(&mut kind).await.unwrap();
            assert_eq!(kind[0], 1, "Input-Stream");
            loop {
                let input = read_frame::<_, InputV06>(&mut stream).await.map_err(|e| e.to_string());
                let end = !matches!(input, Ok(Some(_)));
                inputs.lock().unwrap().push(input);
                if end {
                    break;
                }
            }
        })
    };
    let calls = capture.clone();
    wait_for("Wechsel zum alten Client", || {
        let _ = events.send(edge_hit());
        calls.0.lock().unwrap().contains(&CaptureCall::Remote)
    })
    .await;
    events.send(CaptureEvent::Typed { key: KEY_Q, chars: KeyChars::text("@", 'q') }).unwrap();
    events.send(CaptureEvent::Key { key: KEY_Q, down: false }).unwrap();
    let received = inputs.clone();
    wait_for("Tasten beim alten Client", || received.lock().unwrap().len() >= 3).await;
    let log = inputs.lock().unwrap().clone();
    assert_eq!(
        log[1..3],
        [Ok(Some(InputV06::Key { key: KEY_Q, down: true })), Ok(Some(InputV06::Key { key: KEY_Q, down: false }))],
        "{log:?}"
    );
    assert!(matches!(log[0], Ok(Some(InputV06::Enter { .. }))), "{log:?}");
    assert!(conn.close_reason().is_none(), "der alte Client bleibt verbunden");
    reader.abort();
    server_task.abort();
}
