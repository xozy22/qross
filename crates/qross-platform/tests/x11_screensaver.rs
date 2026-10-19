//! Bildschirmschoner gegen einen echten X-Server (in CI/Docker: Xvfb): starten, erkennen und beenden
//! über den Bildschirmschoner des X-Servers. Übersprungen ohne `DISPLAY` und auf einem echten Desktop
//! (`XDG_CURRENT_DESKTOP`), damit der Test dort nicht den Bildschirm abschaltet. Eigene Datei, damit er
//! nicht neben `x11.rs` läuft: Dessen Eingaben würden den Bildschirmschoner beenden.
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use qross_platform::screensaver::{self, Outcome};
use x11rb::protocol::xproto::{Blanking, ConnectionExt as _, Exposures};

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < Duration::from_secs(5), "Zeitüberschreitung: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn x11_screen_saver_starts_and_stops() {
    if std::env::var_os("DISPLAY").is_none() {
        return eprintln!("kein DISPLAY – X11-Test übersprungen");
    }
    if std::env::var_os("XDG_CURRENT_DESKTOP").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return eprintln!("echter Desktop – Bildschirmschoner-Test übersprungen");
    }
    let (conn, _) = x11rb::connect(None).unwrap();
    let saved = conn.get_screen_saver().unwrap().reply().unwrap();
    // Wie nach `xset s 600`: eingerichtet
    let set = |timeout: i16| {
        conn.set_screen_saver(timeout, 600, Blanking::PREFERRED, Exposures::ALLOWED).unwrap().check().unwrap();
    };
    set(600);

    assert!(!screensaver::running());
    assert_eq!(screensaver::start(false), Outcome::Started);
    wait_for("Bildschirmschoner läuft", screensaver::running);
    assert_eq!(screensaver::start(false), Outcome::Unchanged);
    assert_eq!(screensaver::stop(), Outcome::Stopped);
    wait_for("Bildschirmschoner beendet", || !screensaver::running());
    assert_eq!(screensaver::stop(), Outcome::Unchanged);

    // Wie nach `xset s off`: keiner eingerichtet, also startet Qross auch keinen
    set(0);
    assert_eq!(screensaver::start(false), Outcome::NotConfigured);
    assert!(!screensaver::running());

    conn.set_screen_saver(saved.timeout as i16, saved.interval as i16, saved.prefer_blanking, saved.allow_exposures)
        .unwrap()
        .check()
        .unwrap();
}
