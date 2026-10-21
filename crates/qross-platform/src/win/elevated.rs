//! Was Eingaben von den Hooks fernhält: ein anderer Desktop oder ein Fenster mit höheren Rechten
//! (siehe [`mod@crate::elevated`]). Der Wächter der Hooks fragt das, bevor er sie für verloren hält
//! (siehe [`Reach::now`]); ein Server meldet so ein Fenster (siehe [`elevated_window`]).

use std::mem::size_of_val;
use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel, TokenUIAccess,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS, HDESK, UOI_NAME,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT,
    GA_ROOTOWNER,
};

use crate::elevated::{file_name, ElevatedWindow};

/// Integritätsstufe „hoch“ (`SECURITY_MANDATORY_HIGH_RID`): Prozesse mit Administratorrechten, bei
/// Administratoren aber auch Hilfsprogramme mit UI-Zugriff
const HIGH: u32 = 0x3000;

/// Was eine Eingabe braucht, um die Hooks dieses Prozesses zu erreichen
pub(super) struct Reach {
    /// Desktop des Prozesses (normalerweise „Default“); Hooks gelten nur auf ihm.
    pub(super) desktop: Option<Vec<u16>>,
    /// Integritätsstufe des Prozesses
    pub(super) integrity: Option<u32>,
}

/// Was Eingaben gerade von den Hooks fernhält
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Obstacle {
    /// Ein anderer Desktop ist aktiv: Sperrbildschirm, UAC-Abfrage, Strg+Alt+Entf
    Desktop,
    /// Ein Fenster mit höherer Integritätsstufe liegt vorne oder unter dem Cursor.
    Elevated { window: HWND, pid: u32, token: Token },
    /// Vorne oder unter dem Cursor liegt ein Fenster, dessen Integritätsstufe sich nicht ermitteln lässt
    Unknown,
}

/// Was das Token eines Prozesses über seine Rechte sagt
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Token {
    /// Integritätsstufe, z. B. 0x2000 normal, 0x3000 als Administrator
    pub(super) level: u32,
    /// UI-Zugriff (uiAccess im Manifest), etwa Bildschirm- und Touch-Tastatur: Solche Programme stehen
    /// ebenfalls höher, ohne Administratorrechte zu haben.
    pub(super) ui_access: bool,
}

impl Reach {
    pub(super) fn new() -> Self {
        let desktop = desktop_name(unsafe { GetThreadDesktop(GetCurrentThreadId()) });
        Self { desktop, integrity: token_of(unsafe { GetCurrentProcessId() }).map(|token| token.level) }
    }

    /// Erreichen Eingaben die Hooks gerade? Nicht auf einem anderen Desktop und nicht in einem Fenster
    /// mit höherer Integritätsstufe (UIPI), weder vorne noch unter dem Cursor. Im Zweifel nein: Dann
    /// wartet der Wächter lieber.
    pub(super) fn now(&self) -> bool {
        self.obstacle().is_none()
    }

    /// Was Eingaben gerade fernhält; `None`: nichts. Ist die eigene Integritätsstufe unbekannt, zählen
    /// Fenster nicht. Liegt ein Fenster mit höherer Stufe im Weg, geht es einem unbekannten vor; vorne
    /// geht vor unter dem Cursor.
    pub(super) fn obstacle(&self) -> Option<Obstacle> {
        // Ein normaler Prozess darf den Desktop von Winlogon nicht öffnen.
        let input = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
        if input.is_null() {
            return Some(Obstacle::Desktop);
        }
        let name = desktop_name(input);
        unsafe { CloseDesktop(input) };
        if name.is_none() || name != self.desktop {
            return Some(Obstacle::Desktop);
        }
        let own = self.integrity?;
        let mut cursor = POINT { x: 0, y: 0 };
        let under_cursor = if unsafe { GetCursorPos(&mut cursor) } != 0 { unsafe { WindowFromPoint(cursor) } } else { null_mut() };
        let mut unknown = false;
        for window in [unsafe { GetForegroundWindow() }, under_cursor] {
            if window.is_null() {
                continue;
            }
            let Some(pid) = window_pid(window) else {
                unknown = true;
                continue;
            };
            match token_of(pid) {
                Some(token) if token.level <= own => {}
                Some(token) => return Some(Obstacle::Elevated { window, pid, token }),
                None => unknown = true,
            }
        }
        unknown.then_some(Obstacle::Unknown)
    }
}

/// Siehe [`crate::elevated_window`]
pub fn elevated_window() -> Option<ElevatedWindow> {
    static REACH: OnceLock<Reach> = OnceLock::new();
    match REACH.get_or_init(Reach::new).obstacle()? {
        Obstacle::Elevated { window, pid, token } => Some(ElevatedWindow {
            pid,
            exe: exe_name(pid),
            title: window_title(window),
            admin: token.level >= HIGH && !token.ui_access,
        }),
        Obstacle::Desktop | Obstacle::Unknown => None,
    }
}

/// Siehe [`crate::cursor_position`]. `GetCursorPos` geht auch, wenn UIPI die Hooks blind macht.
pub fn cursor_position() -> Option<(i32, i32)> {
    let mut cursor = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut cursor) } != 0).then_some((cursor.x, cursor.y))
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

fn window_pid(window: HWND) -> Option<u32> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    (pid != 0).then_some(pid)
}

/// Schließt ein Handle von `OpenProcess` bzw. `OpenProcessToken`
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Zum Befragen öffnen; das erlaubt Windows auch bei Prozessen mit höheren Rechten desselben Nutzers.
fn open_process(pid: u32) -> Option<Handle> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    (!process.is_null()).then_some(Handle(process))
}

/// Integritätsstufe und UI-Zugriff eines Prozesses; `None`, wenn er sich nicht befragen lässt. Prozesse
/// desselben Nutzers mit Administratorrechten lassen das zu.
fn token_of(pid: u32) -> Option<Token> {
    let process = open_process(pid)?;
    unsafe {
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
        if count == 0 {
            return None;
        }
        let level = *GetSidSubAuthority(sid, u32::from(count) - 1);
        let mut ui_access = 0u32;
        let ok = GetTokenInformation(token.0, TokenUIAccess, (&raw mut ui_access).cast(), size_of_val(&ui_access) as u32, &mut len);
        Some(Token { level, ui_access: ok != 0 && ui_access != 0 })
    }
}

/// Name der ausführbaren Datei eines Prozesses, z. B. „Taskmgr.exe“
fn exe_name(pid: u32) -> Option<String> {
    let process = open_process(pid)?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
    file_name(&path).map(str::to_owned)
}

/// Titel des Fensters, zu dem `window` gehört (unter dem Cursor oft nur ein Teil davon); ohne eigenen
/// den seines Besitzers. `GetWindowTextW` schickt anderen Prozessen keine Nachricht, hängt also nicht
/// an einem Programm, das nicht reagiert, und UIPI hält es nicht auf.
fn window_title(window: HWND) -> Option<String> {
    [GA_ROOT, GA_ROOTOWNER].into_iter().find_map(|which| {
        let top = unsafe { GetAncestor(window, which) };
        if top.is_null() {
            return None;
        }
        let mut buf = [0u16; 256];
        let len = unsafe { GetWindowTextW(top, buf.as_mut_ptr(), buf.len() as i32) };
        let title = String::from_utf16_lossy(&buf[..len.clamp(0, buf.len() as i32) as usize]);
        let title = title.trim();
        (!title.is_empty()).then(|| title.to_owned())
    })
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
    }

    #[test]
    fn the_own_token_tells_its_rights() {
        let token = token_of(unsafe { GetCurrentProcessId() }).expect("eigenes Token");
        assert_eq!(Some(token.level), Reach::new().integrity);
        assert!(!token.ui_access, "Tests laufen ohne UI-Zugriff");
        assert_eq!(token_of(0), None, "Leerlaufprozess");
    }

    #[test]
    fn the_own_process_names_its_file() {
        let own = unsafe { GetCurrentProcessId() };
        let exe = exe_name(own).expect("eigene ausführbare Datei");
        assert!(exe.to_ascii_lowercase().ends_with(".exe"), "{exe}");
        assert!(!exe.contains('\\'), "nur der Dateiname: {exe}");
        assert_eq!(exe_name(0), None, "Leerlaufprozess");
    }

    #[test]
    fn the_query_never_names_the_own_process() {
        // Was vorne liegt, hängt vom Rechner ab, auf dem der Test läuft; das eigene Fenster hat nie
        // höhere Rechte als der eigene Prozess.
        if let Some(window) = elevated_window() {
            assert_ne!(window.pid, unsafe { GetCurrentProcessId() });
        }
        assert!(cursor_position().is_some() || Reach::new().obstacle() == Some(Obstacle::Desktop));
    }
}
