//! Einspeisen über libei gegen einen echten Compositor: mutter ohne Bildschirm
//! (`mutter --headless --wayland --no-x11 --virtual-monitor 1280x800`). Statt des Desktop-Portals,
//! dessen Freigabe-Dialog sich nicht automatisch bestätigen lässt, öffnet der Test die Sitzung über
//! mutters eigene D-Bus-Schnittstelle `org.gnome.Mutter.RemoteDesktop` und reicht die
//! libei-Verbindung an Qross weiter.
//!
//! Geprüft wird, dass mutter ein Gerät für absolute Bewegung mit dem virtuellen Monitor als Region
//! anbietet und die Eingaben ohne Protokollfehler annimmt. Ob der Cursor wirklich wandert, sieht
//! der Test nicht (dafür bräuchte es einen Wayland-Client unter dem Cursor).
//!
//! Läuft nur auf Anfrage: `cargo test -p qross-platform --test wayland_mutter -- --ignored`
//! innerhalb von `dbus-run-session` mit laufendem mutter (siehe `.github/workflows/ci.yml`).
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ashpd::zbus;
use ashpd::zvariant::{OwnedFd, OwnedObjectPath, Value};
use qross_core::Inject;
use qross_proto::{KeyCode, MouseButton, Rect};

const SERVICE: &str = "org.gnome.Mutter.RemoteDesktop";

/// Sitzung bei mutter öffnen und starten; liefert die Verbindung (hält die Sitzung) und das EIS-fd.
async fn mutter_eis() -> zbus::Result<(zbus::Connection, std::os::fd::OwnedFd)> {
    let conn = zbus::Connection::session().await?;
    let reply =
        conn.call_method(Some(SERVICE), "/org/gnome/Mutter/RemoteDesktop", Some(SERVICE), "CreateSession", &()).await?;
    let session: OwnedObjectPath = reply.body().deserialize()?;
    let interface = "org.gnome.Mutter.RemoteDesktop.Session";
    conn.call_method(Some(SERVICE), session.as_str(), Some(interface), "Start", &()).await?;
    let options: HashMap<&str, Value<'_>> = HashMap::new();
    let reply = conn.call_method(Some(SERVICE), session.as_str(), Some(interface), "ConnectToEIS", &(options,)).await?;
    let fd: OwnedFd = reply.body().deserialize()?;
    Ok((conn, fd.into()))
}

#[test]
#[ignore = "braucht einen laufenden mutter --headless (CI-Job „Wayland (mutter)“)"]
fn injects_into_headless_mutter() {
    // Der Test läuft nicht als Kind von mutter, also ohne WAYLAND_DISPLAY
    std::env::set_var("QROSS_BACKEND", "wayland");
    let dir = std::env::temp_dir().join(format!("qross-mutter-{}", std::process::id()));
    qross_platform::set_data_dir(&dir);
    let rt = tokio::runtime::Runtime::new().unwrap();
    // mutter braucht nach dem Start einen Moment, bis die Schnittstelle bereitsteht
    let deadline = Instant::now() + Duration::from_secs(30);
    let (_conn, fd) = loop {
        match rt.block_on(mutter_eis()) {
            Ok(found) => break found,
            Err(e) if Instant::now() < deadline => {
                eprintln!("mutter noch nicht bereit: {e}");
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => panic!("org.gnome.Mutter.RemoteDesktop nicht erreichbar: {e}"),
        }
    };

    let mut injector = qross_platform::injector_from_ei_fd(fd).expect("libei-Verbindung zu mutter");
    // Ohne Bildschirmübertragung bietet mutter ein gemeinsames Absolut-Gerät mit einer Region je
    // logischem Monitor an.
    let monitors = qross_platform::monitors();
    assert_eq!(monitors, vec![Rect::new(0, 0, 1280, 800)], "Regionen von mutter");

    let a = KeyCode(0x04);
    for event in [
        Inject::MoveTo { x: 100, y: 200 },
        Inject::MoveTo { x: 1279, y: 799 },
        Inject::Button { button: MouseButton::Left, down: true },
        Inject::Button { button: MouseButton::Left, down: false },
        Inject::Wheel { dx: 0, dy: -120 },
        Inject::Key { key: a, down: true },
        Inject::Key { key: a, down: false },
    ] {
        injector.inject(&event);
        std::thread::sleep(Duration::from_millis(20));
    }
    // Ein Protokollfehler würde die Verbindung trennen; Qross meldet das sofort als Problem.
    std::thread::sleep(Duration::from_millis(500));
    let state = qross_platform::portal_state().expect("Wayland");
    assert_eq!(state.injector, None, "mutter hat die Verbindung getrennt");
}
