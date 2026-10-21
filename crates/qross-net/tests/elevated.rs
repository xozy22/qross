//! Fenster mit höheren Rechten am Server (Windows), Ende zu Ende über QUIC auf Loopback: Die Abfragen
//! von Fenster und Cursor sind Test-Doubles, die der Test umstellt.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use qross_core::{CaptureEvent, Layout, LinkConfig};
use qross_net::client::{self, ClientConfig, ClientPlatform};
use qross_net::clipfiles;
use qross_net::elevated::Detector;
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
use qross_net::{Identity, TrustStore};
use qross_platform::dryrun::{CaptureCall, RecordingCapture, RecordingInjector, SharedClipboard};
use qross_platform::ElevatedWindow;
use qross_proto::{Edge, GameState, Rect};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

const PC: Rect = Rect::new(0, 0, 1920, 1080);
const LAPTOP: Rect = Rect::new(0, 0, 1280, 800);

struct Setup {
    events: UnboundedSender<CaptureEvent>,
    capture: RecordingCapture,
    /// Was die Abfragen des Servers gerade liefern
    window: Arc<Mutex<Option<ElevatedWindow>>>,
    cursor: Arc<Mutex<Option<(i32, i32)>>>,
    notices: Arc<Mutex<Vec<ServerNotice>>>,
    tasks: Vec<JoinHandle<anyhow::Result<()>>>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.tasks.iter().for_each(JoinHandle::abort);
    }
}

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

/// Server „pc“ mit „laptop“ rechts daneben; der Client verbindet sich
fn start() -> Setup {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let (server_id, client_id) = (Identity::generate().unwrap(), Identity::generate().unwrap());
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let window: Arc<Mutex<Option<ElevatedWindow>>> = Arc::default();
    let cursor: Arc<Mutex<Option<(i32, i32)>>> = Arc::new(Mutex::new(Some((960, 540))));
    let detect = Detector {
        window: Box::new({
            let window = window.clone();
            move || window.lock().unwrap().clone()
        }),
        cursor: Box::new({
            let cursor = cursor.clone();
            move || *cursor.lock().unwrap()
        }),
    };
    let (notice_tx, notice_rx) = tokio::sync::mpsc::unbounded_channel();
    let server =
        Server::bind(config, &server_id, trust).unwrap().with_notices(notice_tx).with_elevated_windows(detect);
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
        finder: None,
    }));
    let client_config = ClientConfig {
        name: "laptop".into(),
        server: addr.to_string(),
        server_fingerprint: Some(server_id.fingerprint()),
        clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
        share_concealed: false,
    };
    let client_task = tokio::spawn(client::run(
        client_config,
        client_id,
        TrustStore::in_memory(),
        ClientPlatform {
            injector: Box::new(RecordingInjector::default()),
            clipboard: Box::new(SharedClipboard::default()),
            screen: Box::new(|| LAPTOP),
            monitors: Box::new(Vec::new),
            game: Box::new(GameState::default),
            finder: None,
        },
    ));
    Setup { events, capture, window, cursor, notices: collect(notice_rx), tasks: vec![server_task, client_task] }
}

async fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Zeitüberschreitung beim Warten auf: {what}");
}

fn taskmgr() -> ElevatedWindow {
    ElevatedWindow { pid: 4711, exe: Some("Taskmgr.exe".into()), title: Some("Task-Manager".into()), admin: true }
}

fn edge_hit() -> CaptureEvent {
    CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false }
}

/// Wie oft der Cursor bisher zum Client gewechselt ist
fn entered(s: &Setup) -> usize {
    s.capture.0.lock().unwrap().iter().filter(|call| **call == CaptureCall::Remote).count()
}

async fn connected(s: &Setup) {
    let notices = s.notices.clone();
    wait_for("Client verbunden", || {
        notices.lock().unwrap().iter().any(|n| matches!(n, ServerNotice::ClientConnected { name, .. } if name == "laptop"))
    })
    .await;
}

/// Gemeldete Fenster bisher (nach Programm)
fn windows(s: &Setup) -> Vec<Option<String>> {
    let notices = s.notices.lock().unwrap();
    notices
        .iter()
        .filter_map(|n| match n {
            ServerNotice::ElevatedWindow { window } => Some(window.as_ref().and_then(|w| w.exe.clone())),
            _ => None,
        })
        .collect()
}

/// Gemeldete Wechselversuche bisher
fn holds(s: &Setup) -> usize {
    s.notices.lock().unwrap().iter().filter(|n| matches!(n, ServerNotice::ElevatedWindowHold)).count()
}

async fn block(s: &Setup, window: Option<ElevatedWindow>) {
    let expected = window.as_ref().and_then(|w| w.exe.clone());
    let reported = windows(s).len();
    *s.window.lock().unwrap() = window;
    wait_for("Meldung des Fensters", || windows(s).len() > reported).await;
    assert_eq!(windows(s).last(), Some(&expected));
}

#[tokio::test]
async fn an_elevated_window_keeps_the_cursor_on_the_server() {
    let s = start();
    connected(&s).await;
    block(&s, Some(taskmgr())).await;

    // Die Erfassung sieht die Kante (das Fenster liegt nur vorne): kein Wechsel, einmal gemeldet
    for _ in 0..10 {
        s.events.send(edge_hit()).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    wait_for("Wechselversuch", || holds(&s) == 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!((holds(&s), entered(&s)), (1, 0));

    // Wieder frei: Der Cursor wechselt
    block(&s, None).await;
    wait_for("Wechsel zum Client", || {
        let _ = s.events.send(edge_hit());
        entered(&s) == 1
    })
    .await;
    assert_eq!(holds(&s), 1);
}

#[tokio::test]
async fn the_cursor_at_an_edge_is_an_attempt_to_switch() {
    let s = start();
    connected(&s).await;
    // Mitten auf dem Bildschirm will niemand wechseln
    block(&s, Some(taskmgr())).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(holds(&s), 0);

    // An der Kante zum Laptop, wo die Erfassung nichts sieht: gemeldet, aber nicht gewechselt
    *s.cursor.lock().unwrap() = Some((1919, 540));
    wait_for("Wechselversuch", || holds(&s) == 1).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!((holds(&s), entered(&s)), (1, 0), "einmal je Blockade, kein Wechsel");

    // Nächste Blockade, nächste Meldung
    block(&s, None).await;
    block(&s, Some(taskmgr())).await;
    wait_for("zweiter Wechselversuch", || holds(&s) == 2).await;
    assert_eq!(entered(&s), 0);
}

#[tokio::test]
async fn an_elevated_window_leaves_the_cursor_on_the_client() {
    let s = start();
    connected(&s).await;
    wait_for("Wechsel zum Client", || {
        let _ = s.events.send(edge_hit());
        entered(&s) == 1
    })
    .await;
    // Der geparkte Cursor steht nicht an einer Kante; auch dort zählte er nicht
    *s.cursor.lock().unwrap() = Some((1919, 540));
    block(&s, Some(taskmgr())).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    let calls = s.capture.0.lock().unwrap().clone();
    assert!(!calls.iter().any(|call| matches!(call, CaptureCall::Release { .. })), "nicht zurückgeholt: {calls:?}");
    assert_eq!(holds(&s), 0);
}
