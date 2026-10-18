//! Einspeisen über libei gegen eine eigene EIS-Gegenstelle (reis), ohne Compositor und ohne
//! Desktop-Portal: `LIBEI_SOCKET` zeigt auf einen Socket dieses Tests. Geprüft werden Handshake,
//! Binden der Geräte, Regionen als Monitore und die Ereignisse, die beim Compositor ankämen.
//! Eigenes Testprogramm, weil es Umgebungsvariablen setzt.
#![cfg(target_os = "linux")]

use std::sync::mpsc;
use std::time::{Duration, Instant};

use qross_core::Inject;
use qross_proto::{KeyCode, MouseButton, Rect};
use reis::eis;
use reis::handshake::EisHandshaker;
use reis::request::{DeviceCapability, EisRequest, EisRequestConverter};
use reis::PendingRequestResult;

#[derive(Debug, PartialEq)]
enum Got {
    Absolute(f32, f32),
    Button(u32, bool),
    Discrete(i32, i32),
    Key(u32, bool),
}

/// Eine EIS-Gegenstelle wie ein Compositor: ein Sitz, ein Absolut-Gerät mit einer Region
/// 1280×800 und eine Tastatur. Meldet die empfangenen Eingaben.
fn serve(listener: eis::Listener, tx: mpsc::Sender<Got>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let context = loop {
        if let Some(context) = listener.accept().expect("accept") {
            break context;
        }
        assert!(Instant::now() < deadline, "kein Client verbunden");
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut handshaker = EisHandshaker::new(&context, 1);
    let mut converter: Option<EisRequestConverter> = None;
    // Der Sitz lebt so lange wie die Verbindung
    let mut _seat = None;
    loop {
        match context.read() {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return,
            Err(e) => panic!("EIS lesen: {e}"),
            Ok(_) => {}
        }
        while let Some(result) = context.pending_request() {
            let request = match result {
                PendingRequestResult::Request(request) => request,
                PendingRequestResult::ParseError(e) => panic!("Protokollfehler des Clients: {e:?}"),
                PendingRequestResult::InvalidObject(id) => panic!("unbekanntes Objekt {id}"),
            };
            match &mut converter {
                None => {
                    if let Some(resp) = handshaker.handle_request(request).expect("Handshake") {
                        assert_eq!(resp.context_type, reis::ei::handshake::ContextType::Sender);
                        let conv = EisRequestConverter::new(&context, resp, 1);
                        _seat = Some(conv.handle().add_seat(
                            Some("default"),
                            DeviceCapability::PointerAbsolute
                                | DeviceCapability::Pointer
                                | DeviceCapability::Button
                                | DeviceCapability::Scroll
                                | DeviceCapability::Keyboard,
                        ));
                        converter = Some(conv);
                    }
                }
                Some(conv) => conv.handle_request(request).expect("Anfrage des Clients"),
            }
        }
        if let Some(conv) = &mut converter {
            while let Some(request) = conv.next_request() {
                let got = match request {
                    EisRequest::Bind(bind) => {
                        assert!(bind.capabilities.contains(DeviceCapability::PointerAbsolute), "{bind:?}");
                        let absolute = bind.seat.add_device(
                            Some("absolut"),
                            eis::device::DeviceType::Virtual,
                            DeviceCapability::PointerAbsolute | DeviceCapability::Button | DeviceCapability::Scroll,
                            |device| device.device().region(0, 0, 1280, 800, 1.0),
                        );
                        absolute.resumed();
                        let keyboard = bind.seat.add_device(
                            Some("tastatur"),
                            eis::device::DeviceType::Virtual,
                            DeviceCapability::Keyboard.into(),
                            |_| {},
                        );
                        keyboard.resumed();
                        None
                    }
                    EisRequest::PointerMotionAbsolute(e) => Some(Got::Absolute(e.dx_absolute, e.dy_absolute)),
                    EisRequest::Button(e) => Some(Got::Button(e.button, e.state == eis::button::ButtonState::Press)),
                    EisRequest::ScrollDiscrete(e) => Some(Got::Discrete(e.discrete_dx, e.discrete_dy)),
                    EisRequest::KeyboardKey(e) => Some(Got::Key(e.key, e.state == eis::keyboard::KeyState::Press)),
                    EisRequest::Disconnect => return,
                    _ => None,
                };
                if let Some(got) = got {
                    let _ = tx.send(got);
                }
            }
            let _ = conv.handle().flush();
        }
        let _ = context.flush();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn injects_over_libei() {
    let dir = std::env::temp_dir().join(format!("qross-ei-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("eis-0");
    let listener = eis::Listener::bind(&path).expect("EIS-Socket");
    std::env::set_var("LIBEI_SOCKET", &path);
    std::env::set_var("QROSS_BACKEND", "wayland");
    // Kein echtes Datenverzeichnis anfassen
    qross_platform::set_data_dir(&dir);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || serve(listener, tx));

    let mut injector = qross_platform::injector().expect("Einspeisung über libei");
    assert_eq!(qross_platform::desktop(), qross_platform::Desktop::Wayland);
    // Die Regionen der Gegenstelle sind die Monitore
    assert_eq!(qross_platform::monitors(), vec![Rect::new(0, 0, 1280, 800)]);
    assert_eq!(qross_platform::screen_bounds(), Rect::new(0, 0, 1280, 800));

    let a = KeyCode(0x04);
    for event in [
        Inject::MoveTo { x: 100, y: 200 },
        // Außerhalb der Region: auf den nächsten Punkt der Region gelegt
        Inject::MoveTo { x: 5000, y: -3 },
        Inject::Button { button: MouseButton::Left, down: true },
        Inject::Button { button: MouseButton::Left, down: false },
        Inject::Button { button: MouseButton::Back, down: true },
        // Eine Raste nach oben
        Inject::Wheel { dx: 0, dy: 120 },
        Inject::Key { key: a, down: true },
        Inject::Key { key: a, down: false },
    ] {
        injector.inject(&event);
    }
    let expected = [
        Got::Absolute(100.0, 200.0),
        Got::Absolute(1279.0, 0.0),
        Got::Button(0x110, true),
        Got::Button(0x110, false),
        Got::Button(0x113, true),
        Got::Discrete(0, -120),
        Got::Key(30, true),
        Got::Key(30, false),
    ];
    for want in expected {
        let got = rx.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| panic!("{want:?} kam nicht an"));
        assert_eq!(got, want);
    }
    drop(injector);
    let _ = std::fs::remove_dir_all(&dir);
}
