//! Drag & Drop unter X11 gegen einen echten X-Server (in CI: Xvfb); ohne `DISPLAY` übersprungen.
//! Eine nachgebaute Quelle spielt das ziehende Programm so wie GTK: Sie wird Besitzer von
//! `XdndSelection`, greift Zeiger und Tastatur, liefert die Dateien als `text/uri-list` und bricht
//! bei Esc ab. Kein Fenstermanager: Dessen Rahmen samt `WM_STATE` baut der Test selbst nach.
#![cfg(target_os = "linux")]

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use qross_core::{CaptureEvent, Inject};
use qross_proto::{Edge, MouseButton};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GrabMode, GrabStatus, PropMode, SelectionNotifyEvent,
    SelectionRequestEvent, Window, WindowClass, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
    SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

/// Escape: evdev 1 + 8
const ESCAPE: u8 = 9;

enum Command {
    /// Ziehen beginnen: Besitzer werden und greifen. `Some`: diese URI-Liste anbieten, `None`: nur Text
    Start(Option<String>),
    /// Ziehen ohne Esc beenden (wie nach dem Ablegen)
    Stop,
}

/// Das ziehende Programm, in eigenem Thread mit eigener Verbindung
struct Source {
    commands: Sender<Command>,
    reports: Receiver<&'static str>,
}

impl Source {
    fn spawn() -> Self {
        let (commands, command_rx) = channel();
        let (report_tx, reports) = channel();
        thread::spawn(move || run_source(command_rx, report_tx));
        let source = Self { commands, reports };
        source.expect("bereit");
        source
    }

    fn start(&self, uris: Option<&str>) {
        self.commands.send(Command::Start(uris.map(str::to_owned))).unwrap();
        self.expect("gestartet");
    }

    fn stop(&self) {
        self.commands.send(Command::Stop).unwrap();
        self.expect("beendet");
    }

    #[track_caller]
    fn expect(&self, report: &str) {
        assert_eq!(self.reports.recv_timeout(Duration::from_secs(3)).ok(), Some(report), "Quelle");
    }
}

fn atom(conn: &impl Connection, name: &str) -> Atom {
    conn.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom
}

fn run_source(commands: Receiver<Command>, reports: Sender<&'static str>) {
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let (selection, uri_list) = (atom(&conn, "XdndSelection"), atom(&conn, "text/uri-list"));
    let window = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().event_mask(EventMask::KEY_PRESS);
    conn.create_window(COPY_DEPTH_FROM_PARENT, window, root, 700, 20, 10, 10, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.map_window(window).unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    reports.send("bereit").unwrap();
    let ungrab = |conn: &RustConnection| {
        conn.ungrab_pointer(CURRENT_TIME).unwrap();
        conn.ungrab_keyboard(CURRENT_TIME).unwrap();
        conn.flush().unwrap();
    };
    // Was das laufende Ziehen anbietet; `None`: es läuft keins
    let mut offer: Option<Option<String>> = None;
    loop {
        match commands.try_recv() {
            Ok(Command::Start(uris)) => {
                conn.set_selection_owner(window, selection, CURRENT_TIME).unwrap();
                let mask = EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
                let pointer = conn.grab_pointer(false, window, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME);
                let keyboard = conn.grab_keyboard(false, window, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC);
                let status = (pointer.unwrap().reply().unwrap().status, keyboard.unwrap().reply().unwrap().status);
                assert_eq!(status, (GrabStatus::SUCCESS, GrabStatus::SUCCESS), "Quelle greift");
                offer = Some(uris);
                reports.send("gestartet").unwrap();
            }
            Ok(Command::Stop) => {
                ungrab(&conn);
                offer = None;
                reports.send("beendet").unwrap();
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }
        match conn.poll_for_event().unwrap() {
            Some(Event::SelectionRequest(request)) => {
                let uris = offer.as_ref().and_then(|o| o.as_deref());
                answer(&conn, &request, uris, uri_list);
            }
            Some(Event::KeyPress(key)) if key.detail == ESCAPE && offer.is_some() => {
                ungrab(&conn);
                offer = None;
                reports.send("abgebrochen").unwrap();
            }
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(2)),
        }
    }
}

/// Antwort auf eine Anfrage nach den Daten: die URI-Liste oder eine Absage (Eigenschaft `NONE`)
fn answer(conn: &RustConnection, request: &SelectionRequestEvent, uris: Option<&str>, uri_list: Atom) {
    let property = if request.property == NONE { request.target } else { request.property };
    let delivered = match uris {
        Some(list) if request.target == uri_list => {
            conn.change_property8(PropMode::REPLACE, request.requestor, property, uri_list, list.as_bytes()).unwrap();
            property
        }
        _ => NONE,
    };
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: request.time,
        requestor: request.requestor,
        selection: request.selection,
        target: request.target,
        property: delivered,
    };
    conn.send_event(false, request.requestor, EventMask::NO_EVENT, notify).unwrap();
    conn.flush().unwrap();
}

/// `file://`-URI mit Prozentkodierung, wie Dateimanager sie anbieten
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[track_caller]
fn wait_event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<CaptureEvent>,
    want: impl Fn(&CaptureEvent) -> bool,
) -> CaptureEvent {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        match rx.try_recv() {
            Ok(event) if want(&event) => return event,
            Ok(_) => {}
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("Ereignis nicht empfangen");
}

#[track_caller]
fn probe(capture: &dyn qross_platform::Capture, rx: &mut tokio::sync::mpsc::UnboundedReceiver<CaptureEvent>) -> Vec<PathBuf> {
    capture.probe_drag(1279, 300);
    match wait_event(rx, |e| matches!(e, CaptureEvent::DragProbeResult { .. })) {
        CaptureEvent::DragProbeResult { files } => files,
        _ => unreachable!(),
    }
}

/// Ein einziger Test, weil sich die Fälle denselben X-Server (Zeiger, Auswahl, Grabs) teilen.
#[test]
fn x11_drag_and_drop() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("kein DISPLAY – X11-Test übersprungen");
        return;
    }
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let bounds = qross_platform::screen_bounds();
    let dir = std::env::temp_dir().join(format!("qross-xdnd-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Ordner Ä")).unwrap();
    std::fs::write(dir.join("Bericht Grüße.pdf"), "Qross").unwrap();
    let files = vec![dir.join("Bericht Grüße.pdf"), dir.join("Ordner Ä")];
    let uris = format!("# von Qross\r\n{}\r\n{}\r\n", file_uri(&files[0]), file_uri(&files[1]));

    let source = Source::spawn();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = qross_platform::start_capture(tx).unwrap();
    let fake = |kind: u8, detail: u8, x: i32, y: i32| {
        conn.xtest_fake_input(kind, detail, CURRENT_TIME, root, x as i16, y as i16, 0).unwrap();
        conn.flush().unwrap();
    };
    let move_to = |x: i32, y: i32| fake(MOTION_NOTIFY_EVENT, 0, x, y);
    let left = |down: bool| fake(if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, 1, 0, 0);
    // Die Erfassung fragt den Zeiger alle 4 ms ab
    let settle = || thread::sleep(Duration::from_millis(100));

    // --- Server: Dateien an die Kante ziehen -------------------------------------------------
    move_to(400, 300);
    settle();
    left(true);
    settle();
    source.start(Some(&uris));
    move_to(bounds.right(), 300);
    let hit = wait_event(&mut rx, |e| matches!(e, CaptureEvent::EdgeHit { .. }));
    assert!(matches!(hit, CaptureEvent::EdgeHit { edge: Edge::Right, buttons_down: true, .. }), "{hit:?}");
    assert_eq!(probe(&*capture, &mut rx), files);
    assert!(probe(&*capture, &mut rx).is_empty(), "dieselben Dateien nicht ein zweites Mal");

    // Abbrechen: Esc erreicht die Quelle, sie gibt ihre Grabs frei, dann greift die Erfassung.
    capture.cancel_local_drag();
    capture.capture_remote();
    source.expect("abgebrochen");
    thread::sleep(Duration::from_millis(100));
    let pending: Vec<CaptureEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(!pending.contains(&CaptureEvent::PanicReturn), "Wechsel trotz des Grabs der Quelle: {pending:?}");
    let (cx, cy) = (bounds.width as i32 / 2, bounds.height as i32 / 2);
    move_to(cx + 15, cy - 7);
    assert_eq!(wait_event(&mut rx, |e| matches!(e, CaptureEvent::MouseDelta { .. })), CaptureEvent::MouseDelta { dx: 15, dy: -7 });
    left(false);
    let release = wait_event(&mut rx, |e| matches!(e, CaptureEvent::Button { .. }));
    assert_eq!(release, CaptureEvent::Button { button: MouseButton::Left, down: false }, "Loslassen legt beim Client ab");
    capture.release(400, 300);
    settle();

    // --- Server: ohne neues Ziehen bzw. ohne Dateien keine Dateien -------------------------
    left(true);
    settle();
    assert!(probe(&*capture, &mut rx).is_empty(), "XdndSelection gehört noch dem letzten Ziehen");
    source.start(None);
    assert!(probe(&*capture, &mut rx).is_empty(), "gezogener Text");
    assert!(probe(&*capture, &mut rx).is_empty(), "auch beim zweiten Mal");
    source.stop();
    left(false);
    settle();
    assert!(probe(&*capture, &mut rx).is_empty(), "ohne gedrückte Taste");

    // --- Client: Dateien zum Server ziehen ------------------------------------------------
    let mut injector = qross_platform::injector().unwrap();
    injector.inject(&Inject::MoveTo { x: 400, y: 300 });
    injector.inject(&Inject::Button { button: MouseButton::Left, down: true });
    assert!(injector.take_dragged_files(400, 300).is_empty(), "noch kein Ziehen");
    source.start(Some(&uris));
    injector.inject(&Inject::MoveTo { x: 0, y: 300 });
    assert_eq!(injector.take_dragged_files(0, 300), files);
    source.expect("abgebrochen");
    assert!(injector.take_dragged_files(0, 300).is_empty(), "nur einmal");
    injector.inject(&Inject::Button { button: MouseButton::Left, down: false });

    // --- Ablegen: Schreibtisch unter dem Cursor -------------------------------------------
    let desktop = dirs::desktop_dir().or_else(|| dirs::home_dir().map(|home| home.join("Desktop"))).unwrap();
    std::fs::create_dir_all(&desktop).unwrap();
    assert_eq!(qross_platform::folder_at(500, 500), None, "nur das Root-Fenster");
    // Rahmen eines Fenstermanagers, darin das Fenster des Schreibtisch-Programms
    let (width, height) = (bounds.width as u16, bounds.height as u16);
    let window = |parent: Window, x: i16, y: i16, w: u16, h: u16| {
        let id = conn.generate_id().unwrap();
        conn.create_window(COPY_DEPTH_FROM_PARENT, id, parent, x, y, w, h, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new())
            .unwrap();
        id
    };
    let frame = window(root, 0, 0, width, height);
    let client = window(frame, 0, 0, width, height);
    let wm_state = atom(&conn, "WM_STATE");
    conn.change_property32(PropMode::REPLACE, client, wm_state, wm_state, &[1, 0]).unwrap();
    let (window_type, desktop_type) = (atom(&conn, "_NET_WM_WINDOW_TYPE"), atom(&conn, "_NET_WM_WINDOW_TYPE_DESKTOP"));
    conn.change_property32(PropMode::REPLACE, client, window_type, AtomEnum::ATOM, &[desktop_type]).unwrap();
    conn.map_window(client).unwrap();
    conn.map_window(frame).unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    assert_eq!(qross_platform::folder_at(500, 500), Some(desktop.clone()), "Client: Schreibtisch");
    assert_eq!(capture.drop_folder(500, 500), Some(desktop.clone()), "Server: Schreibtisch");
    // Ein gewöhnliches Fenster darüber
    let normal = window(root, 100, 100, 200, 200);
    conn.map_window(normal).unwrap();
    conn.get_input_focus().unwrap().reply().unwrap();
    assert_eq!(qross_platform::folder_at(150, 150), None, "über einem Programmfenster");
    assert_eq!(qross_platform::folder_at(500, 500), Some(desktop), "daneben weiter der Schreibtisch");
    let _ = std::fs::remove_dir_all(&dir);
}
