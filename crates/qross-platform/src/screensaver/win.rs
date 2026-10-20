//! Windows: Bildschirmschoner über `SystemParametersInfo`, `SC_SCREENSAVE` und Mausbewegungen.
//!
//! In einer Remotedesktop-Sitzung startet Windows keinen Bildschirmschoner; Qross versucht es dort
//! nicht (ein vergeblicher Versuch sperrt dort bis zur nächsten Anmeldung die Einstellungen dazu).

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::core::{w, BOOL, PCWSTR};
use windows_sys::Win32::Graphics::Gdi::SC_SCREENSAVE;
use windows_sys::Win32::System::Power::{CallNtPowerInformation, SystemExecutionState, ES_DISPLAY_REQUIRED};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
use windows_sys::Win32::System::Shutdown::LockWorkStation;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetDesktopWindow, GetSystemMetrics, SystemParametersInfoW, SM_REMOTESESSION,
    SPI_GETSCREENSAVEACTIVE, SPI_GETSCREENSAVERRUNNING, WM_SYSCOMMAND,
};

use super::Outcome;
use crate::win::INJECT_MAGIC;

/// So lange wartet der Start darauf, dass Windows den Bildschirmschoner meldet
const START_TIMEOUT: Duration = Duration::from_millis(1500);
/// So lange wartet das Beenden darauf, dass er weg ist
const STOP_TIMEOUT: Duration = Duration::from_millis(500);
/// Bildschirmschoner von Windows enden bei einer Taste oder erst nach mehreren Mausbewegungen: die
/// mit Direct3D (Mystify, Bänder, Blasen, 3D-Text) ab der sechsten. Hin und her um wenige Pixel,
/// mit kurzen Pausen, damit Windows die Bewegungen nicht zu einer zusammenfasst; eine gerade
/// Anzahl, damit der Zeiger dort endet, wo er war.
const WIGGLES: usize = 8;
const WIGGLE_PX: i32 = 2;
const WIGGLE_GAP: Duration = Duration::from_millis(15);

pub fn running() -> bool {
    spi_flag(SPI_GETSCREENSAVERRUNNING)
}

/// Bis `done` gilt oder `timeout` um ist
fn wait_for(timeout: Duration, done: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done()
}

fn spi_flag(action: u32) -> bool {
    let mut value: BOOL = 0;
    let ok = unsafe { SystemParametersInfoW(action, 0, &mut value as *mut BOOL as *mut c_void, 0) };
    ok != 0 && value != 0
}

/// Eingerichtet: an und mit gewählter `.scr` (Richtlinie vor Nutzer-Einstellung)
fn configured() -> bool {
    let exe = registry_string(w!(r"Software\Policies\Microsoft\Windows\Control Panel\Desktop"), w!("SCRNSAVE.EXE"))
        .or_else(|| registry_string(w!(r"Control Panel\Desktop"), w!("SCRNSAVE.EXE")));
    super::windows_configured(spi_flag(SPI_GETSCREENSAVEACTIVE), exe.as_deref())
}

/// Zeichenkette aus `HKEY_CURRENT_USER`; `REG_EXPAND_SZ` kommt dabei schon aufgelöst.
fn registry_string(key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 512];
    let mut bytes = (buffer.len() * 2) as u32;
    let err = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key, value, RRF_RT_REG_SZ, null_mut(), buffer.as_mut_ptr() as *mut c_void, &mut bytes)
    };
    if err != 0 {
        return None;
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// Hält ein Programm den Bildschirm wach (`ES_DISPLAY_REQUIRED`, etwa ein Video)? Dann startet
/// Windows nach der Wartezeit auch keinen Bildschirmschoner.
fn kept_awake() -> bool {
    let mut state: u32 = 0;
    let status = unsafe {
        CallNtPowerInformation(SystemExecutionState, null(), 0, &mut state as *mut u32 as *mut c_void, size_of::<u32>() as u32)
    };
    status == 0 && state & ES_DISPLAY_REQUIRED != 0
}

pub fn start() -> Outcome {
    if running() {
        return Outcome::Unchanged;
    }
    if unsafe { GetSystemMetrics(SM_REMOTESESSION) } != 0 {
        return Outcome::Unsupported("in a Remote Desktop session");
    }
    if !configured() {
        return Outcome::NotConfigured;
    }
    if kept_awake() {
        return Outcome::KeptAwake;
    }
    // Wie nach Ablauf der Wartezeit: Windows startet den eingerichteten Bildschirmschoner samt
    // „Anmeldeseite bei Reaktivierung“, falls gewählt.
    // Laut Dokumentation von WM_SYSCOMMAND führt DefWindowProc den Befehl jederzeit aus.
    unsafe { DefWindowProcW(GetDesktopWindow(), WM_SYSCOMMAND, SC_SCREENSAVE as usize, 0) };
    if wait_for(START_TIMEOUT, running) {
        Outcome::Started
    } else {
        Outcome::Failed("Windows did not start it".into())
    }
}

/// Wie ein Nutzer, der die Maus bewegt. Ist der Bildschirmschoner mit Anmeldung gesichert, läuft er
/// auf dem Desktop der Anmeldung; dort kommt keine Eingabe von Qross an.
pub fn stop() -> Outcome {
    if !running() {
        return Outcome::Unchanged;
    }
    for i in 0..WIGGLES {
        let dx = if i % 2 == 0 { WIGGLE_PX } else { -WIGGLE_PX };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: INJECT_MAGIC },
            },
        };
        if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } == 0 {
            return Outcome::Failed("input does not get through (computer locked?)".into());
        }
        std::thread::sleep(WIGGLE_GAP);
    }
    if wait_for(STOP_TIMEOUT, || !running()) {
        Outcome::Stopped
    } else {
        Outcome::Failed("still running (password-protected?)".into())
    }
}

pub fn lock() -> Outcome {
    if unsafe { LockWorkStation() } != 0 {
        Outcome::Locked
    } else {
        Outcome::Failed(std::io::Error::last_os_error().to_string())
    }
}
