//! Spielemodus: Läuft im Vordergrund ein Vollbildprogramm, und hat ein Programm den Zeiger
//! eingefangen?

use std::mem::size_of;

use qross_proto::{GameState, Rect};
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_BUSY, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetClipCursor, GetForegroundWindow, GetShellWindow, GetWindowLongW, GetWindowRect, GWL_STYLE,
    WS_CAPTION,
};

use super::screen_bounds;

pub fn game_state() -> GameState {
    GameState { fullscreen: fullscreen(), captured: pointer_captured() }
}

/// Vollbild nach Auskunft der Shell (exklusives Direct3D-Vollbild oder ein Programm, das den
/// ganzen Monitor füllt) oder weil das Vordergrundfenster ohne Titelleiste seinen Monitor bedeckt
/// (randlose Spiele, Videos). Der Desktop und die Taskleiste zählen nicht.
fn fullscreen() -> bool {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() || is_shell(hwnd) {
        // Kein Vordergrundfenster (z. B. Sperrbildschirm) oder der Desktop
        return false;
    }
    let shell = unsafe { SHQueryUserNotificationState() };
    matches!(shell, Ok(state) if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_BUSY) || covers_monitor(hwnd)
}

fn covers_monitor(hwnd: HWND) -> bool {
    unsafe {
        // Maximierte Fenster bedecken mit ihrem unsichtbaren Rahmen ebenfalls den Monitor, wenn die
        // Taskleiste automatisch ausgeblendet wird; sie behalten aber ihre Titelleiste.
        if GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION == WS_CAPTION {
            return false;
        }
        let mut window = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetWindowRect(hwnd, &mut window) == 0 {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        if monitor.is_null() {
            return false;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return false;
        }
        let m = info.rcMonitor;
        window.left <= m.left && window.top <= m.top && window.right >= m.right && window.bottom >= m.bottom
    }
}

fn is_shell(hwnd: HWND) -> bool {
    if hwnd == unsafe { GetShellWindow() } {
        return true;
    }
    let mut buf = [0u16; 64];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) }.max(0) as usize;
    let class = String::from_utf16_lossy(&buf[..len]);
    matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// Hat ein Programm den Zeiger per `ClipCursor` auf weniger als den Bildschirm eingesperrt?
fn pointer_captured() -> bool {
    let mut clip = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if unsafe { GetClipCursor(&mut clip) } == 0 {
        return false;
    }
    let clip = Rect::new(clip.left, clip.top, (clip.right - clip.left).max(0) as u32, (clip.bottom - clip.top).max(0) as u32);
    is_confined(clip, screen_bounds())
}

/// Ohne Einschränkung meldet Windows den ganzen virtuellen Bildschirm.
fn is_confined(clip: Rect, screen: Rect) -> bool {
    clip.width < screen.width || clip.height < screen.height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_smaller_clip_counts_as_captured() {
        let screen = Rect::new(-1920, 0, 3840, 1080);
        assert!(!is_confined(screen, screen), "ohne ClipCursor");
        assert!(is_confined(Rect::new(0, 0, 1920, 1080), screen), "auf einen Monitor eingesperrt");
        assert!(is_confined(Rect::new(500, 400, 1, 1), screen), "auf einen Punkt (Mausblick)");
    }

    #[test]
    fn game_state_can_be_queried() {
        // Was gerade läuft, hängt vom Rechner ab; die Abfrage darf nur nicht scheitern.
        let _ = game_state();
        let desktop = unsafe { GetShellWindow() };
        assert!(desktop.is_null() || is_shell(desktop), "der Desktop zählt nie als Vollbild");
    }
}
