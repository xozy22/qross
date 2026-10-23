//! Wächter für die Low-Level-Hooks: Systemseite zu [`crate::hook_watch`]. Ein eigener Thread prüft
//! alle 250 ms, ob Eingaben ankommen, schickt bei Verdacht eine Probe und lässt die Hooks neu setzen,
//! wenn Windows sie entfernt hat.
//!
//! Die Probe ist eine Mausbewegung um ein Pixel hin und zurück und ein Druck auf die Taste 0xE8
//! (ohne Belegung, wie bei `inject::tap_menu_mask`), markiert wie alle eigenen Eingaben: Die Hooks
//! zählen sie und lassen sie durch, Programme bemerken davon nichts. Eine Bewegung um 0 Pixel taugt
//! nicht: Ob Windows die überhaupt an die Hooks gibt, ist nicht dokumentiert. Für den Tastatur-Hook
//! allein reicht der Tastendruck.

use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    LASTINPUTINFO, MOUSEEVENTF_MOVE, MOUSEINPUT,
};

use super::elevated::Reach;
use super::INJECT_MAGIC;
use crate::hook_watch::{Calls, HookWatch, Step, POLL_MS};

/// Startet den Wächter. `calls` liest die Aufrufe der Hooks bisher, `keys_held`, ob ein Client gerade
/// Tasten hält, die die Hooks ihm geschickt haben; `rehook` lässt die Hooks neu setzen, `release` beim
/// Client loslassen, was er noch hält. Er endet, sobald `running` falsch wird.
pub(super) fn spawn(
    calls: impl Fn() -> Calls + Send + 'static,
    keys_held: impl Fn() -> bool + Send + 'static,
    running: &'static AtomicBool,
    rehook: impl Fn() + Send + 'static,
    release: impl Fn() + Send + 'static,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("qross-hook-watch".into())
        .spawn(move || run(calls, keys_held, running, rehook, release))
        .map(drop)
}

fn run(calls: impl Fn() -> Calls, keys_held: impl Fn() -> bool, running: &AtomicBool, rehook: impl Fn(), release: impl Fn()) {
    let reach = Reach::new();
    let mut watch = HookWatch::new();
    let mut lost = 0u32;
    while running.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(POLL_MS));
        let Some(input) = last_input() else { continue };
        let now = unsafe { GetTickCount64() };
        match watch.poll(now, input, calls(), keys_held(), || reach.now()) {
            Step::Wait => {}
            Step::Probe => {
                if !probe() {
                    watch.probe_refused();
                }
            }
            Step::ProbeKeys => {
                if !probe_keys() {
                    watch.probe_refused();
                }
            }
            Step::Rehook => {
                lost += 1;
                tracing::warn!(
                    "Windows removed the input hooks (a call took too long); reinstalling them \
                     (#{lost} since start)"
                );
                rehook();
            }
            Step::Release => release(),
        }
    }
}

/// Zeitstempel der letzten Eingabe in dieser Sitzung (Takt von `GetTickCount`)
fn last_input() -> Option<u32> {
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    (unsafe { GetLastInputInfo(&mut info) } != 0).then_some(info.dwTime)
}

/// Schickt die Probe für beide Hooks. Falsch, wenn Windows sie ablehnt (gesperrte oder getrennte
/// Sitzung, z. B. ein minimiertes RDP-Fenster).
pub(super) fn probe() -> bool {
    send(&[probe_move(1), probe_move(-1), probe_key(false), probe_key(true)])
}

/// Die Probe nur für den Tastatur-Hook. Falsch, wenn Windows sie ablehnt.
fn probe_keys() -> bool {
    send(&[probe_key(false), probe_key(true)])
}

fn send(inputs: &[INPUT]) -> bool {
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    sent as usize == inputs.len()
}

fn probe_move(dx: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: INJECT_MAGIC },
        },
    }
}

fn probe_key(up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0xE8,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: INJECT_MAGIC,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_input_is_known() {
        // Was die Hooks erreicht, prüft `elevated::tests`.
        assert!(last_input().is_some());
    }
}

