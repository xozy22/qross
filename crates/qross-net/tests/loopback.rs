//! Ende-zu-Ende über echtes QUIC auf Loopback, mit Test-Doubles statt Betriebssystem.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qross_core::keys::{parse_switch_keys, HotkeyEvent, HotkeyTracker, KeyConfig, KeyOptions};
use qross_core::{CaptureEvent, Inject, Layout, LinkConfig};
use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
use qross_net::clipfiles;
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
use qross_net::{Identity, TrustStore};
use qross_platform::dryrun::{CaptureCall, RecordingCapture, RecordingInjector, SharedClipboard};
use qross_platform::Clipboard;
use qross_proto::{ClipboardData, Edge, KeyCode, MouseButton, Rect};
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

struct Setup {
    events: UnboundedSender<CaptureEvent>,
    capture: RecordingCapture,
    server_clip: SharedClipboard,
    injected: RecordingInjector,
    client_clip: SharedClipboard,
    client_screen: ClientScreen,
    notices: Arc<Mutex<Vec<ServerNotice>>>,
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
    let (notice_tx, mut notice_rx) = tokio::sync::mpsc::unbounded_channel();
    let notices = Arc::new(Mutex::new(Vec::new()));
    let collected = notices.clone();
    tokio::spawn(async move {
        while let Some(notice) = notice_rx.recv().await {
            collected.lock().unwrap().push(notice);
        }
    });
    let server = Server::bind(config, &server_id, trust)
        .unwrap()
        .with_clipboard_file_limit(file_limit)
        .with_keys(switch_keys())
        .with_notices(notice_tx);
    let addr = server.local_addr().unwrap();

    let capture = RecordingCapture::default();
    let (events, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let server_clip = SharedClipboard::default();
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(capture.clone()),
        events: events_rx,
        clipboard: Box::new(server_clip.clone()),
        screen: Box::new(|| PC),
        monitors: Box::new(Vec::new),
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
    };
    let client_task = tokio::spawn(client::run(
        client_config,
        client_id,
        TrustStore::in_memory(),
        ClientPlatform {
            injector: Box::new(injected.clone()),
            clipboard: Box::new(client_clip.clone()),
            screen: Box::new(move || screen.lock().unwrap().0),
            monitors: Box::new(move || monitors.lock().unwrap().1.clone()),
        },
    ));

    Setup { events, capture, server_clip, injected, client_clip, client_screen, notices, server_task, client_task }
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
    }));
    let (client_tx, mut client_rx) = tokio::sync::mpsc::unbounded_channel();
    let client_task = tokio::spawn(client::run_with_notices(
        ClientConfig {
            name: "laptop".into(),
            server: addr.to_string(),
            server_fingerprint: Some(server_id.fingerprint()),
            clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
        },
        client_id,
        TrustStore::in_memory(),
        ClientPlatform {
            injector: Box::new(RecordingInjector::default()),
            clipboard: Box::new(SharedClipboard::default()),
            screen: Box::new(|| LAPTOP),
            monitors: Box::new(Vec::new),
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
