//! Cursor-Finder unter Windows: ein Layered-Fenster mit Alphakanal (`UpdateLayeredWindow`), das
//! Klicks durchlässt (`WS_EX_TRANSPARENT`), nie aktiviert wird (`WS_EX_NOACTIVATE`), weder in der
//! Taskleiste noch bei Alt+Tab steht (`WS_EX_TOOLWINDOW`) und über allen Fenstern liegt
//! (`WS_EX_TOPMOST`). Es gibt das Fenster nur, solange es zu sehen ist; danach braucht der Thread
//! keine Nachrichten mehr abzuholen.

use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use qross_proto::Rect;
use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, MonitorFromPoint, ReleaseDC,
    SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
    MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, PeekMessageW, RegisterClassExW,
    SetWindowPos, ShowWindow, TranslateMessage, UpdateLayeredWindow, HWND_TOPMOST, MSG, PM_REMOVE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::finder::{Display, Frame, Surface};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

/// Fensterklasse, einmal je Prozess
fn class() -> Option<&'static [u16]> {
    static CLASS: OnceLock<Option<Vec<u16>>> = OnceLock::new();
    CLASS
        .get_or_init(|| unsafe {
            let name = wide("QrossCursorFinder");
            let mut class: WNDCLASSEXW = std::mem::zeroed();
            class.cbSize = size_of::<WNDCLASSEXW>() as u32;
            class.lpfnWndProc = Some(DefWindowProcW);
            class.hInstance = GetModuleHandleW(null());
            class.lpszClassName = name.as_ptr();
            (RegisterClassExW(&class) != 0).then_some(name)
        })
        .as_deref()
}

pub(crate) struct WinSurface {
    hwnd: HWND,
    shown: bool,
}

pub(crate) fn surface() -> anyhow::Result<Box<dyn Surface>> {
    anyhow::ensure!(class().is_some(), "window class not registered");
    Ok(Box::new(WinSurface { hwnd: null_mut(), shown: false }))
}

impl Surface for WinSurface {
    fn prepare(&mut self) -> bool {
        if self.hwnd.is_null() {
            let (Some(class), title) = (class(), wide("Qross Cursor-Finder")) else { return false };
            self.hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class.as_ptr(),
                    title.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                )
            };
            if self.hwnd.is_null() {
                tracing::warn!("cursor finder: failed to create the window ({})", std::io::Error::last_os_error());
            }
        }
        !self.hwnd.is_null()
    }

    fn cursor(&mut self) -> Option<(i32, i32)> {
        let mut point = POINT { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
    }

    fn display_at(&mut self, x: i32, y: i32) -> Option<Display> {
        unsafe {
            let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            let r = info.rcMonitor;
            let (mut dpi_x, mut dpi_y) = (96, 96);
            // Physische Pixel (der Prozess ist DPI-bewusst je Monitor), Größe nach dessen Skalierung
            let scale = if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) == 0 {
                dpi_x as f32 / 96.0
            } else {
                1.0
            };
            let area = Rect::new(r.left, r.top, (r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32);
            Some(Display { area, scale: scale.clamp(1.0, 5.0), density: 1.0 })
        }
    }

    fn present(&mut self, frame: &Frame) {
        if self.hwnd.is_null() || frame.width == 0 || frame.height == 0 {
            return;
        }
        unsafe {
            let screen = GetDC(null_mut());
            let memory = CreateCompatibleDC(screen);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: frame.width as i32,
                // Negativ: Zeilen von oben nach unten, wie im Bild
                biHeight: -(frame.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits = null_mut();
            let bitmap = CreateDIBSection(screen, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if !bitmap.is_null() && !bits.is_null() {
                // BGRA mit vormultipliziertem Alpha ist genau das, was UpdateLayeredWindow erwartet.
                std::ptr::copy_nonoverlapping(frame.pixels.as_ptr(), bits as *mut u8, frame.pixels.len());
                let previous = SelectObject(memory, bitmap);
                let position = POINT { x: frame.rect.x, y: frame.rect.y };
                let size = SIZE { cx: frame.width as i32, cy: frame.height as i32 };
                let origin = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                UpdateLayeredWindow(self.hwnd, screen, &position, &size, memory, &origin, 0, &blend, ULW_ALPHA);
                SelectObject(memory, previous);
                DeleteObject(bitmap);
            }
            DeleteDC(memory);
            ReleaseDC(null_mut(), screen);
            if !self.shown {
                self.shown = true;
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                // Ganz nach oben, auch über andere Fenster, die über allem liegen wollen
                SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn hide(&mut self) {
        if !self.hwnd.is_null() {
            unsafe { DestroyWindow(self.hwnd) };
            self.hwnd = null_mut();
            self.shown = false;
        }
        self.pump();
    }

    fn pump(&mut self) {
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

impl Drop for WinSurface {
    fn drop(&mut self) {
        self.hide();
    }
}
