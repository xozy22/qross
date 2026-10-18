mod capture;
mod clipboard;
mod cursor;
mod dnd;
mod inject;
mod shell;

use glide_proto::Rect;
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

pub use capture::start_capture;
pub use clipboard::clipboard;
pub use inject::injector;
pub use shell::folder_of_window;

/// Markiert von Glide eingespeiste Ereignisse, damit die eigenen Hooks sie ignorieren.
pub(crate) const INJECT_MAGIC: usize = 0x474C_4944; // "GLID"

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

/// Mitte des primären Monitors (liegt garantiert auf einem Bildschirm).
pub(crate) fn primary_center() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN) / 2, GetSystemMetrics(SM_CYSCREEN) / 2) }
}
