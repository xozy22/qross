//! Wächter für die Low-Level-Hooks: Systemseite zu [`crate::hook_watch`]. Ein eigener Thread prüft
//! alle 250 ms, ob Eingaben ankommen, schickt bei Verdacht eine Probe und lässt die Hooks neu setzen,
//! wenn Windows sie entfernt hat.
//!
//! Die Probe ist eine Mausbewegung um ein Pixel hin und zurück und ein Druck auf die Taste 0xE8
//! (ohne Belegung, wie bei `inject::tap_menu_mask`), markiert wie alle eigenen Eingaben: Die Hooks
//! zählen sie und lassen sie durch, Programme bemerken davon nichts. Eine Bewegung um 0 Pixel taugt
//! nicht: Ob Windows die überhaupt an die Hooks gibt, ist nicht dokumentiert.

use std::mem::{size_of, size_of_val};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS, HDESK, UOI_NAME,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    LASTINPUTINFO, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, WindowFromPoint};

use super::INJECT_MAGIC;
use crate::hook_watch::{Calls, HookWatch, Step, POLL_MS};

/// Startet den Wächter. `calls` liest die Aufrufe der Hooks bisher, `rehook` lässt sie neu setzen; er
/// endet, sobald `running` falsch wird.
pub(super) fn spawn(
    calls: impl Fn() -> Calls + Send + 'static,
    running: &'static AtomicBool,
    rehook: impl Fn() + Send + 'static,
) -> std::io::Result<()> {
    thread::Builder::new().name("qross-hook-watch".into()).spawn(move || run(calls, running, rehook)).map(drop)
}

fn run(calls: impl Fn() -> Calls, running: &AtomicBool, rehook: impl Fn()) {
    let reach = Reach::new();
    let mut watch = HookWatch::new();
    let mut lost = 0u32;
    while running.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(POLL_MS));
        let Some(input) = last_input() else { continue };
        let now = unsafe { GetTickCount64() };
        match watch.poll(now, input, calls(), || reach.now()) {
            Step::Wait => {}
            Step::Probe => {
                if !probe() {
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
    let inputs = [probe_move(1), probe_move(-1), probe_key(false), probe_key(true)];
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

/// Was eine Eingabe braucht, um die Hooks dieses Prozesses zu erreichen
struct Reach {
    /// Desktop des Prozesses (normalerweise „Default“); Hooks gelten nur auf ihm.
    desktop: Option<Vec<u16>>,
    /// Integritätsstufe des Prozesses
    integrity: Option<u32>,
}

impl Reach {
    fn new() -> Self {
        let desktop = desktop_name(unsafe { GetThreadDesktop(GetCurrentThreadId()) });
        Self { desktop, integrity: integrity_of(unsafe { GetCurrentProcessId() }) }
    }

    /// Erreichen Eingaben die Hooks gerade? Nicht auf einem anderen Desktop (Sperrbildschirm, UAC,
    /// Strg+Alt+Entf) und nicht in einem Fenster mit höherer Integritätsstufe (UIPI), weder vorne noch
    /// unter dem Cursor. Im Zweifel nein: Dann wartet der Wächter lieber.
    fn now(&self) -> bool {
        // Ein normaler Prozess darf den Desktop von Winlogon nicht öffnen.
        let input = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
        if input.is_null() {
            return false;
        }
        let name = desktop_name(input);
        unsafe { CloseDesktop(input) };
        if name.is_none() || name != self.desktop {
            return false;
        }
        let Some(own) = self.integrity else { return true };
        let mut cursor = POINT { x: 0, y: 0 };
        let under_cursor = if unsafe { GetCursorPos(&mut cursor) } != 0 { unsafe { WindowFromPoint(cursor) } } else { null_mut() };
        [unsafe { GetForegroundWindow() }, under_cursor]
            .into_iter()
            .filter(|window| !window.is_null())
            .all(|window| window_integrity(window).is_some_and(|level| level <= own))
    }
}

fn desktop_name(desktop: HDESK) -> Option<Vec<u16>> {
    if desktop.is_null() {
        return None;
    }
    let mut buf = [0u16; 256];
    let mut needed = 0;
    let ok = unsafe { GetUserObjectInformationW(desktop, UOI_NAME, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, &mut needed) };
    if ok == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(buf[..len].to_vec())
}

fn window_integrity(window: HWND) -> Option<u32> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if pid == 0 {
        return None;
    }
    integrity_of(pid)
}

/// Integritätsstufe eines Prozesses (z. B. 0x2000 normal, 0x3000 als Administrator); `None`, wenn er
/// sich nicht befragen lässt.
fn integrity_of(pid: u32) -> Option<u32> {
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let process = Handle(process);
        let mut token = null_mut();
        if OpenProcessToken(process.0, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let token = Handle(token);
        // TOKEN_MANDATORY_LABEL samt der SID dahinter; u64 für die Ausrichtung
        let mut buf = [0u64; 16];
        let mut len = 0;
        if GetTokenInformation(token.0, TokenIntegrityLevel, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, &mut len) == 0 {
            return None;
        }
        let sid = (*buf.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        (count > 0).then(|| *GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_own_process_reaches_its_own_hooks() {
        let reach = Reach::new();
        assert!(reach.desktop.as_ref().is_some_and(|name| !name.is_empty()), "Name des eigenen Desktops");
        let level = reach.integrity.expect("Integritätsstufe des eigenen Prozesses");
        assert!((0x1000..=0x4000).contains(&level), "{level:#x}");
        assert!(last_input().is_some());
    }
}

