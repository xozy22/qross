//! Wann die X11-Erfassung aufwacht (in der CI unter Xvfb): Verzögerung vom XTest-Ereignis bis zum
//! `CaptureEvent`, Aufwachvorgänge des Erfassungs-Threads im Leerlauf und Steuerbefehle, die ohne
//! jede Eingabe wirken müssen. Die Messwerte stehen im Log (direkt auf stderr, also auch ohne
//! `--nocapture`). Eigene Datei: Cargo führt die Test-Programme nacheinander aus, sie teilen sich
//! also nicht gleichzeitig den X-Server mit `x11.rs`.
#![cfg(target_os = "linux")]

use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use qross_core::keys::{KeyOptions, SwitchTarget};
use qross_core::CaptureEvent;
use qross_proto::Edge;
use tokio::sync::mpsc::UnboundedReceiver;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::CURRENT_TIME;

/// Messungen je Fall
const SAMPLES: u32 = 40;
/// X11-Keycodes (evdev + 8) der Standardbelegung von Xvfb
const CTRL: u8 = 37;
const ALT: u8 = 64;
const DIGIT2: u8 = 11;
const A: u8 = 38;

#[test]
fn x11_capture_wakes_on_events_only() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("kein DISPLAY – X11-Test übersprungen");
        return;
    }
    let (conn, screen) = x11rb::connect(None).unwrap();
    let root = conn.setup().roots[screen].root;
    let bounds = qross_platform::screen_bounds();
    let (cx, cy) = (bounds.width as i16 / 2, bounds.height as i16 / 2);
    let right = bounds.right() as i16;
    // XTest-Ereignis schicken; erst nach der Antwort auf GetInputFocus hat der X-Server es verarbeitet
    let fake = |kind: u8, detail: u8, x: i16, y: i16| {
        conn.xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0).unwrap();
        conn.flush().unwrap();
    };
    let sync = || {
        conn.get_input_focus().unwrap().reply().unwrap();
    };
    fake(MOTION_NOTIFY_EVENT, 0, cx, cy);
    sync();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let capture = qross_platform::start_capture(tx).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let thread = capture_thread().expect("Erfassungs-Thread nicht gefunden");
    drain(&mut rx);

    // --- Lokal --------------------------------------------------------------------------
    let idle_local = idle(&thread, Duration::from_secs(2));

    // Kante: Zeiger von der Mitte an die rechte Kante (jedes Mal eine andere Höhe)
    let edge = measure(|i| {
        fake(MOTION_NOTIFY_EVENT, 0, cx, cy);
        sync();
        pause(i);
        drain(&mut rx);
        let y = cy + i as i16;
        let start = Instant::now();
        fake(MOTION_NOTIFY_EVENT, 0, right, y);
        wait(&mut rx, start, "EdgeHit", |e| matches!(e, CaptureEvent::EdgeHit { edge: Edge::Right, y: hit, .. } if *hit == i32::from(y)))
    });
    fake(MOTION_NOTIFY_EVENT, 0, cx, cy);
    sync();

    // Wechsel-Kürzel über den passiven Griff
    let laptop = SwitchTarget::Screen("laptop".into());
    let switch = vec![("Ctrl+Alt+Digit2".parse().unwrap(), laptop.clone())];
    capture.set_key_options(KeyOptions { switch, ..KeyOptions::default() });
    let hotkey = measure(|i| {
        fake(KEY_PRESS_EVENT, CTRL, 0, 0);
        fake(KEY_PRESS_EVENT, ALT, 0, 0);
        sync();
        pause(i);
        drain(&mut rx);
        let start = Instant::now();
        fake(KEY_PRESS_EVENT, DIGIT2, 0, 0);
        let took = wait(&mut rx, start, "SwitchTo", |e| *e == CaptureEvent::SwitchTo(laptop.clone()));
        for key in [DIGIT2, ALT, CTRL] {
            fake(KEY_RELEASE_EVENT, key, 0, 0);
        }
        sync();
        took
    });

    // --- Remote -------------------------------------------------------------------------
    capture.capture_remote();
    std::thread::sleep(Duration::from_millis(100));
    drain(&mut rx);
    let idle_remote = idle(&thread, Duration::from_secs(2));

    // Relative Bewegung aus der Mitte; die Erfassung setzt den Zeiger danach zurück
    let motion = measure(|i| {
        pause(i);
        drain(&mut rx);
        let dx = 1 + (i % 7) as i16;
        let start = Instant::now();
        fake(MOTION_NOTIFY_EVENT, 1, dx, 0);
        wait(&mut rx, start, "MouseDelta", |e| *e == CaptureEvent::MouseDelta { dx: dx.into(), dy: 0 })
    });

    let key = measure(|i| {
        pause(i);
        drain(&mut rx);
        let start = Instant::now();
        fake(KEY_PRESS_EVENT, A, 0, 0);
        let took = wait(&mut rx, start, "Typed", |e| matches!(e, CaptureEvent::Typed { .. }));
        fake(KEY_RELEASE_EVENT, A, 0, 0);
        wait(&mut rx, Instant::now(), "Key", |e| matches!(e, CaptureEvent::Key { down: false, .. }));
        took
    });

    // --- Steuerbefehle ohne Eingabe -----------------------------------------------------
    // Zurück an die Kante: Ein Warp erzeugt keine Bewegung im Sinne von XInput, die Erfassung muss
    // also von sich aus nachsehen.
    let release = measure(|i| {
        capture.capture_remote();
        sync();
        pause(i);
        drain(&mut rx);
        let y = 100 + i as i32;
        let start = Instant::now();
        capture.release(right.into(), y);
        wait(&mut rx, start, "EdgeHit nach release", |e| matches!(e, CaptureEvent::EdgeHit { edge: Edge::Right, y: hit, .. } if *hit == y))
    });

    // Dateien vom Client, keine Taste gedrückt: sofort hier ablegen
    let drop_here = measure(|i| {
        fake(MOTION_NOTIFY_EVENT, 0, cx, cy);
        sync();
        pause(i);
        drain(&mut rx);
        let start = Instant::now();
        capture.await_local_drop();
        wait(&mut rx, start, "LocalDrop", |e| matches!(e, CaptureEvent::LocalDrop { .. }))
    });

    // Ende: Der Thread gibt die Verbindung (und damit die Grabs) frei
    drop(capture);
    let start = Instant::now();
    while Path::new(&format!("/proc/self/task/{thread}")).exists() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let ended = (!Path::new(&format!("/proc/self/task/{thread}")).exists()).then(|| start.elapsed());

    let row = |name: &str, samples: &Samples| format!("  {name:<44} {}\n", samples.describe());
    let mut report = format!("X11-Erfassung: Median / Maximum über {SAMPLES} Messungen\n");
    report += &row("Kante (lokal, XTest → EdgeHit)", &edge);
    report += &row("Wechsel-Kürzel (lokal, XTest → SwitchTo)", &hotkey);
    report += &row("Bewegung (remote, XTest → MouseDelta)", &motion);
    report += &row("Taste (remote, XTest → Typed)", &key);
    report += &row("release() an der Kante → EdgeHit", &release);
    report += &row("await_local_drop() → LocalDrop", &drop_here);
    report += &format!(
        "  Leerlauf lokal: {:.1} Aufwachvorgänge/s, {:.0} µs Rechenzeit/s\n",
        idle_local.wakeups, idle_local.cpu_us
    );
    report += &format!(
        "  Leerlauf remote: {:.1} Aufwachvorgänge/s, {:.0} µs Rechenzeit/s\n",
        idle_remote.wakeups, idle_remote.cpu_us
    );
    report += &match ended {
        Some(took) => format!("  Thread nach drop() beendet: nach {:.2} ms\n", ms(took)),
        None => "  Thread nach drop() beendet: nein, läuft weiter\n".to_owned(),
    };
    let _ = std::io::stderr().write_all(report.as_bytes());

    // Im Leerlauf schläft die Schleife (früher: einige hundert Mal je Sekunde wach)
    assert!(idle_local.wakeups < 5.0, "lokal {:.1}/s", idle_local.wakeups);
    assert!(idle_remote.wakeups < 5.0, "remote {:.1}/s", idle_remote.wakeups);
    // Großzügig, die CI ist ein geteilter Rechner. Mit dem früheren 4-ms-Takt lag der Median lokal
    // bei 1,5–1,8 ms, im Remote-Modus (1 ms Pause) bei 0,7 ms.
    let fast = Duration::from_millis(1);
    for (name, samples) in [("Kante", &edge), ("Kürzel", &hotkey), ("Bewegung", &motion), ("Taste", &key)] {
        assert!(samples.median() < fast, "{name}: {}", samples.describe());
    }
    // Steuerbefehle wirken, ohne dass eine Eingabe die Schleife weckt
    for (name, samples) in [("release", &release), ("await_local_drop", &drop_here)] {
        assert!(samples.median() < fast && samples.max() < Duration::from_millis(50), "{name}: {}", samples.describe());
    }
    assert!(ended.is_some(), "Erfassungs-Thread läuft nach drop() weiter");
}

/// Gemessene Zeiten eines Falls
struct Samples(Vec<Duration>);

impl Samples {
    fn median(&self) -> Duration {
        self.0[self.0.len() / 2]
    }

    fn max(&self) -> Duration {
        self.0[self.0.len() - 1]
    }

    fn describe(&self) -> String {
        format!("{:>6.2} / {:>6.2} ms", ms(self.median()), ms(self.max()))
    }
}

fn measure(mut sample: impl FnMut(u32) -> Duration) -> Samples {
    let mut samples: Vec<Duration> = (0..SAMPLES).map(&mut sample).collect();
    samples.sort();
    Samples(samples)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Unterschiedlich lange Pausen, damit die Messungen nicht immer an derselben Stelle eines festen
/// Abfragetakts landen
fn pause(i: u32) {
    std::thread::sleep(Duration::from_micros(3000 + u64::from(i) * 1700 % 5000));
}

fn drain(rx: &mut UnboundedReceiver<CaptureEvent>) {
    while rx.try_recv().is_ok() {}
}

/// Wartet ohne zu schlafen (für genaue Zeiten) auf ein Ereignis und gibt die Zeit seit `start` zurück
#[track_caller]
fn wait(
    rx: &mut UnboundedReceiver<CaptureEvent>,
    start: Instant,
    what: &str,
    want: impl Fn(&CaptureEvent) -> bool,
) -> Duration {
    while start.elapsed() < Duration::from_secs(3) {
        match rx.try_recv() {
            Ok(event) if want(&event) => return start.elapsed(),
            Ok(_) => {}
            Err(_) => std::thread::yield_now(),
        }
    }
    panic!("{what} nicht empfangen");
}

/// Kennung des Erfassungs-Threads („qross-x11-capture“, vom Kernel auf 15 Zeichen gekürzt)
fn capture_thread() -> Option<String> {
    std::fs::read_dir("/proc/self/task").ok()?.flatten().find_map(|entry| {
        let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
        comm.starts_with("qross-x11-cap").then(|| entry.file_name().to_string_lossy().into_owned())
    })
}

struct Idle {
    wakeups: f64,
    cpu_us: f64,
}

/// Aufwachvorgänge und Rechenzeit des Threads je Sekunde, während nichts passiert
fn idle(thread: &str, span: Duration) -> Idle {
    let (switches, cpu) = activity(thread);
    std::thread::sleep(span);
    let (switches_after, cpu_after) = activity(thread);
    let secs = span.as_secs_f64();
    Idle { wakeups: (switches_after - switches) as f64 / secs, cpu_us: (cpu_after - cpu) as f64 / 1000.0 / secs }
}

/// Kontextwechsel (jedes Blockieren in poll, sleep oder Futex ist einer) und Rechenzeit in ns
fn activity(thread: &str) -> (u64, u64) {
    let status = std::fs::read_to_string(format!("/proc/self/task/{thread}/status")).unwrap();
    let switches = status
        .lines()
        .filter(|line| line.contains("ctxt_switches"))
        .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
        .sum();
    let cpu = std::fs::read_to_string(format!("/proc/self/task/{thread}/schedstat"))
        .ok()
        .and_then(|stat| stat.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    (switches, cpu)
}
