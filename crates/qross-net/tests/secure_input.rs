//! Sichere Tastatureingabe am Server (macOS), Ende zu Ende über QUIC auf Loopback: Die Abfrage
//! ist ein Test-Double, das der Test umschaltet.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use qross_core::{CaptureEvent, Layout, LinkConfig};
use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
use qross_net::clipfiles;
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
use qross_net::{Identity, TrustStore};
use qross_platform::dryrun::{CaptureCall, RecordingCapture, RecordingInjector, SharedClipboard};
use qross_platform::SecureInput;
use qross_proto::{Edge, GameState, Rect};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

const PC: Rect = Rect::new(0, 0, 1920, 1080);
const LAPTOP: Rect = Rect::new(0, 0, 1280, 800);

struct Setup {
    events: UnboundedSender<CaptureEvent>,
    capture: RecordingCapture,
    /// Was die Abfrage des Servers gerade liefert
    secure: Arc<Mutex<Option<SecureInput>>>,
    notices: Arc<Mutex<Vec<ServerNotice>>>,
    client_notices: Arc<Mutex<Vec<ClientNotice>>>,
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

/// Server „pc“ mit „laptop“ rechts daneben, Client verbunden
fn start() -> Setup {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let (server_id, client_id) = (Identity::generate().unwrap(), Identity::generate().unwrap());
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let secure: Arc<Mutex<Option<SecureInput>>> = Arc::default();
    let detect = secure.clone();
    let (notice_tx, notice_rx) = tokio::sync::mpsc::unbounded_channel();
    let server = Server::bind(config, &server_id, trust)
        .unwrap()
        .with_notices(notice_tx)
        .with_secure_input(Box::new(move || detect.lock().unwrap().clone()));
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
    let (client_tx, client_rx) = tokio::sync::mpsc::unbounded_channel();
    let client_task = tokio::spawn(client::run_with_notices(
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
        client_tx,
    ));
    Setup {
        events,
        capture,
        secure,
        notices: collect(notice_rx),
        client_notices: collect(client_rx),
        tasks: vec![server_task, client_task],
    }
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

fn edge_hit() -> CaptureEvent {
    CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false }
}

/// Wie oft der Cursor bisher zum Client gewechselt ist
fn entered(s: &Setup) -> usize {
    s.capture.0.lock().unwrap().iter().filter(|call| **call == CaptureCall::Remote).count()
}

/// Kantentreffer wiederholen, bis der Cursor (zum `n`-ten Mal) auf dem Client ist
async fn enter_laptop(s: &Setup, n: usize) {
    wait_for("Wechsel zum Client", || {
        let _ = s.events.send(edge_hit());
        entered(s) == n
    })
    .await;
}

fn holds(s: &Setup) -> Vec<Option<String>> {
    let notices = s.notices.lock().unwrap();
    notices.iter().filter_map(|n| if let ServerNotice::SecureInputHold { from } = n { Some(from.clone()) } else { None }).collect()
}

fn holders(s: &Setup) -> Vec<Option<String>> {
    let notices = s.notices.lock().unwrap();
    notices
        .iter()
        .filter_map(|n| match n {
            ServerNotice::SecureInput { holder } => Some(holder.as_ref().and_then(|h| h.app.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn secure_input_brings_the_cursor_back_and_keeps_it_on_the_server() {
    let s = start();
    enter_laptop(&s, 1).await;

    // Terminal schaltet die sichere Tastatureingabe ein: zurück auf den Server, Client verlassen
    *s.secure.lock().unwrap() = Some(SecureInput { pid: Some(412), app: Some("Terminal".into()) });
    let capture = s.capture.clone();
    wait_for("Rückkehr zum Server", || capture.0.lock().unwrap().contains(&CaptureCall::Release { x: 960, y: 540 })).await;
    wait_for("Meldungen", || holds(&s) == [Some("laptop".to_owned())]).await;
    assert_eq!(holders(&s), [Some("Terminal".to_owned())]);
    let client_notices = s.client_notices.clone();
    wait_for("Client verlassen", || client_notices.lock().unwrap().iter().any(|n| matches!(n, ClientNotice::CursorLeft)))
        .await;

    // Solange sie an ist: kein Wechsel, der erste Versuch wird gemeldet
    for _ in 0..10 {
        s.events.send(edge_hit()).unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    wait_for("verhinderter Wechsel", || holds(&s).len() == 2).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(holds(&s), [Some("laptop".to_owned()), None], "einmal gemeldet");
    assert_eq!(entered(&s), 1, "kein Wechsel");

    // Ein anderes Programm übernimmt: nur die Meldung, kein neuer verhinderter Wechsel
    *s.secure.lock().unwrap() = Some(SecureInput { pid: Some(977), app: Some("Google Chrome".into()) });
    wait_for("neues Programm", || holders(&s).len() == 2).await;
    s.events.send(edge_hit()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(holds(&s).len(), 2);

    // Vorbei: Der Cursor wechselt wieder
    *s.secure.lock().unwrap() = None;
    wait_for("Meldung „aus“", || holders(&s).last() == Some(&None)).await;
    enter_laptop(&s, 2).await;
}
