//! Systemcursor ausblenden, solange der Cursor auf einem anderen Rechner ist.
//!
//! Windows kennt kein globales „Cursor verstecken“. Deshalb werden alle Standardcursor durch
//! einen transparenten ersetzt, und `SPI_SETCURSORS` lädt danach das Schema des Benutzers
//! neu. Nach einem Absturz im Remote-Modus stellt der nächste Start den Cursor wieder her.
//!
//! Beides läuft in einem eigenen Thread, in der Reihenfolge der Aufrufe: `SPI_SETCURSORS` liest das
//! Schema von der Platte, und 14 Cursor zu ersetzen dauert auch. Der Aufrufer ist die Ereignisschleife
//! des Servers; solange sie wartete, stünde der Cursor auf dem Client still.

use std::ptr::null_mut;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::OnceLock;
use std::time::Duration;

use windows_sys::Win32::UI::WindowsAndMessaging::{CreateCursor, SetSystemCursor, SystemParametersInfoW, SPI_SETCURSORS};

/// OCR_NORMAL, IBEAM, WAIT, CROSS, UP, SIZENWSE, SIZENESW, SIZEWE, SIZENS, SIZEALL, NO, HAND, APPSTARTING, HELP
const SYSTEM_CURSORS: [u32; 14] =
    [32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650, 32651];

enum Job {
    Hide,
    /// Mit Rückmeldung, wenn der Aufrufer darauf wartet
    Restore(Option<SyncSender<()>>),
}

/// Der Thread für beides; `None`, wenn er nicht startet (dann gleich im Aufrufer)
fn worker() -> Option<&'static Sender<Job>> {
    static WORKER: OnceLock<Option<Sender<Job>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new().name("qross-cursor".into()).spawn(move || run(rx)) {
                Ok(_) => Some(tx),
                Err(e) => {
                    tracing::warn!("Thread für den Cursor startet nicht ({e}) – Ausblenden hält die Eingabe kurz auf");
                    None
                }
            }
        })
        .as_ref()
}

fn run(jobs: Receiver<Job>) {
    for job in jobs {
        match job {
            Job::Hide => hide_now(),
            Job::Restore(done) => {
                restore_now();
                if let Some(done) = done {
                    let _ = done.send(());
                }
            }
        }
    }
}

pub fn hide() {
    if worker().is_none_or(|w| w.send(Job::Hide).is_err()) {
        hide_now();
    }
}

pub fn restore() {
    if worker().is_none_or(|w| w.send(Job::Restore(None)).is_err()) {
        restore_now();
    }
}

/// Wie [`restore`], wartet aber, bis der Cursor wieder da ist (auch hinter einem noch wartenden
/// Ausblenden). Beim Beenden: Danach läuft der Thread womöglich nicht mehr.
pub fn restore_and_wait() {
    let (done, finished) = mpsc::sync_channel(1);
    let queued = worker().is_some_and(|w| w.send(Job::Restore(Some(done))).is_ok());
    if !queued || finished.recv_timeout(Duration::from_secs(2)).is_err() {
        restore_now();
    }
}

fn hide_now() {
    // 32×32 monochrom: AND-Maske 1 (Hintergrund bleibt), XOR-Maske 0 (nichts darüber)
    let and_mask = [0xFFu8; 32 * 4];
    let xor_mask = [0u8; 32 * 4];
    for id in SYSTEM_CURSORS {
        unsafe {
            let blank = CreateCursor(null_mut(), 0, 0, 32, 32, and_mask.as_ptr().cast(), xor_mask.as_ptr().cast());
            if !blank.is_null() {
                // SetSystemCursor übernimmt (und zerstört) das Handle.
                SetSystemCursor(blank, id);
            }
        }
    }
}

fn restore_now() {
    unsafe {
        SystemParametersInfoW(SPI_SETCURSORS, 0, null_mut(), 0);
    }
}
