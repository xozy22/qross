//! Echte Tests gegen einen X-Server (in CI/Docker: Xvfb). Ohne `DISPLAY` werden sie übersprungen.
//! `docker build -f docker/linux-test.Dockerfile -t qross-linux-test .` und dann `docker run …`
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use qross_core::keys::{KeyOptions, SwitchTarget};
use qross_core::{CaptureEvent, Inject};
use qross_proto::{Edge, KeyCode, MouseButton};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, WindowClass, MOTION_NOTIFY_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::CURRENT_TIME;

fn display_available() -> bool {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("kein DISPLAY – X11-Test übersprungen");
        return false;
    }
    true
}

fn pointer(conn: &impl Connection, root: u32) -> (i32, i32) {
    let p = conn.query_pointer(root).unwrap().reply().unwrap();
    (p.root_x.into(), p.root_y.into())
}

/// Ein einziger Test, weil sich die Fälle denselben X-Server (Zeiger, Fokus) teilen.
#[test]
fn x11_capture_and_injection() {
    if !display_available() {
        return;
    }
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let bounds = qross_platform::screen_bounds();
    assert!(bounds.width >= 640 && bounds.height >= 480, "{bounds:?}");
    // RandR-Monitore liegen im selben Koordinatensystem wie die Gesamtfläche (ohne RandR 1.5: keine).
    let monitors = qross_platform::monitors();
    assert!(
        monitors.iter().all(|m| bounds.contains(m.x, m.y) && bounds.contains(m.right(), m.bottom())),
        "{monitors:?} außerhalb von {bounds:?}"
    );

    // --- Einspeisen ---------------------------------------------------------------------
    let mut injector = qross_platform::injector().unwrap();
    injector.inject(&Inject::MoveTo { x: 123, y: 234 });
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(pointer(&conn, root), (123, 234));

    // Fenster mit Fokus, das Tastendrücke empfängt
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE);
    conn.create_window(0, win, root, 0, 0, 300, 300, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    conn.set_input_focus(InputFocus::POINTER_ROOT, win, CURRENT_TIME).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(100));

    injector.inject(&Inject::Key { key: KeyCode::A, down: true });
    injector.inject(&Inject::Key { key: KeyCode::A, down: false });
    let keycodes = collect_keys(&conn, Duration::from_secs(2), 2);
    assert_eq!(keycodes, vec![(38, true), (38, false)], "A = evdev 30 + 8");

    // --- Erfassen -----------------------------------------------------------------------
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = qross_platform::start_capture(tx).unwrap();
    let fake_move = |x: i16, y: i16, relative: bool| {
        conn.xtest_fake_input(MOTION_NOTIFY_EVENT, relative as u8, CURRENT_TIME, root, x, y, 0).unwrap();
        conn.flush().unwrap();
    };

    // Lokal: an die rechte Kante → EdgeHit
    fake_move(bounds.right() as i16, 300, false);
    let hit = wait_event(&mut rx, |e| matches!(e, CaptureEvent::EdgeHit { .. }));
    assert!(
        matches!(hit, CaptureEvent::EdgeHit { edge: Edge::Right, buttons_down: false, .. }),
        "{hit:?}"
    );

    // Lokal: Wechsel-Kürzel über den passiven Griff; das Fenster sieht nur Strg und Alt
    let laptop = CaptureEvent::SwitchTo(SwitchTarget::Screen("laptop".into()));
    let switch = vec![("Ctrl+Alt+Digit2".parse().unwrap(), SwitchTarget::Screen("laptop".into()))];
    capture.set_key_options(KeyOptions { switch, ..KeyOptions::default() });
    let hotkey = |injector: &mut Box<dyn qross_platform::Injector>| {
        let (ctrl, alt, digit2) = (KeyCode(0xE0), KeyCode(0xE2), KeyCode(0x1F));
        for (key, down) in [(ctrl, true), (alt, true), (digit2, true), (digit2, false), (alt, false), (ctrl, false)] {
            injector.inject(&Inject::Key { key, down });
        }
    };
    hotkey(&mut injector);
    assert_eq!(wait_event(&mut rx, |e| matches!(e, CaptureEvent::SwitchTo(_))), laptop);
    let keycodes = collect_keys(&conn, Duration::from_secs(2), 4);
    assert_eq!(keycodes, vec![(37, true), (64, true), (64, false), (37, false)], "Strg, Alt – die 2 (Keycode 11) nicht");

    // Remote: relative Bewegung, Klick und Taste werden abgefangen und gemeldet
    capture.capture_remote();
    std::thread::sleep(Duration::from_millis(100));
    let (cx, cy) = (bounds.width as i32 / 2, bounds.height as i32 / 2);
    assert_eq!(pointer(&conn, root), (cx, cy), "Zeiger steht in der Mitte");
    fake_move(cx as i16 + 15, cy as i16 - 7, false);
    let delta = wait_event(&mut rx, |e| matches!(e, CaptureEvent::MouseDelta { .. }));
    assert_eq!(delta, CaptureEvent::MouseDelta { dx: 15, dy: -7 });

    conn.xtest_fake_input(x11rb::protocol::xproto::BUTTON_PRESS_EVENT, 3, CURRENT_TIME, root, 0, 0, 0).unwrap();
    conn.flush().unwrap();
    let click = wait_event(&mut rx, |e| matches!(e, CaptureEvent::Button { .. }));
    assert_eq!(click, CaptureEvent::Button { button: MouseButton::Right, down: true });
    conn.xtest_fake_input(x11rb::protocol::xproto::BUTTON_RELEASE_EVENT, 3, CURRENT_TIME, root, 0, 0, 0).unwrap();

    injector.inject(&Inject::Key { key: KeyCode::A, down: true });
    let key = wait_event(&mut rx, |e| matches!(e, CaptureEvent::Key { .. }));
    assert_eq!(key, CaptureEvent::Key { key: KeyCode::A, down: true });
    injector.inject(&Inject::Key { key: KeyCode::A, down: false });
    assert!(collect_keys(&conn, Duration::from_millis(300), 1).is_empty(), "abgefangene Tasten erreichen das Fenster nicht");

    // Remote: Das Kürzel meldet den Wechsel, die 2 selbst geht nicht an den Client
    hotkey(&mut injector);
    assert_eq!(wait_event(&mut rx, |e| matches!(e, CaptureEvent::SwitchTo(_))), laptop);
    std::thread::sleep(Duration::from_millis(200));
    let rest: Vec<CaptureEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert!(!rest.iter().any(|e| matches!(e, CaptureEvent::Key { key: KeyCode(0x1F), .. })), "{rest:?}");

    // Zurück: Grab endet, Zeiger an der gewünschten Stelle
    capture.release(42, 43);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(pointer(&conn, root), (42, 43));
    injector.inject(&Inject::Key { key: KeyCode::A, down: true });
    injector.inject(&Inject::Key { key: KeyCode::A, down: false });
    assert_eq!(collect_keys(&conn, Duration::from_secs(2), 2).len(), 2, "nach release wieder lokal");
}

fn collect_keys(conn: &impl Connection, limit: Duration, want: usize) -> Vec<(u8, bool)> {
    let start = Instant::now();
    let mut keys = Vec::new();
    while start.elapsed() < limit && keys.len() < want {
        match conn.poll_for_event().unwrap() {
            Some(Event::KeyPress(e)) => keys.push((e.detail, true)),
            Some(Event::KeyRelease(e)) => keys.push((e.detail, false)),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    keys
}

fn wait_event(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<CaptureEvent>,
    want: impl Fn(&CaptureEvent) -> bool,
) -> CaptureEvent {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        match rx.try_recv() {
            Ok(event) if want(&event) => return event,
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("Ereignis nicht empfangen");
}
