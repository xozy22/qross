mod capture;
mod clipboard;
mod cursor;
mod dnd;
mod game;
mod inject;
mod layout;
mod locks;
mod shell;

use std::ptr::{null, null_mut};

use qross_proto::Rect;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

pub use capture::start_capture;
pub use clipboard::clipboard;
pub use game::game_state;
pub use inject::injector;
pub use shell::folder_of_window;

/// Markiert von Qross eingespeiste Ereignisse, damit die eigenen Hooks sie ignorieren.
pub(crate) const INJECT_MAGIC: usize = 0x5152_5353; // "QRSS"

pub fn init_process() {
    // Physische Pixel statt skalierter Koordinaten, auch bei 125/150 % Skalierung.
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn screen_bounds() -> Rect {
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32,
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32,
        )
    }
}

/// Fläche jedes Monitors in physischen Pixeln, wie `screen_bounds()`
pub fn monitors() -> Vec<Rect> {
    unsafe extern "system" fn collect(_monitor: HMONITOR, _dc: HDC, rect: *mut RECT, data: LPARAM) -> BOOL {
        let (r, list) = (&*rect, &mut *(data as *mut Vec<Rect>));
        list.push(Rect::new(r.left, r.top, (r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32));
        1
    }
    let mut list: Vec<Rect> = Vec::new();
    // Ohne Gerätekontext liefert der Rückruf je Monitor dessen Rechteck im virtuellen Desktop.
    unsafe { EnumDisplayMonitors(null_mut(), null(), Some(collect), &mut list as *mut Vec<Rect> as LPARAM) };
    list
}

/// Mitte des primären Monitors (liegt garantiert auf einem Bildschirm).
pub(crate) fn primary_center() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2) }
}
