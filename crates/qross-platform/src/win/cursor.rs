//! Systemcursor ausblenden, solange der Cursor auf einem anderen Rechner ist.
//!
//! Windows kennt kein globales „Cursor verstecken“. Deshalb werden alle Standardcursor durch
//! einen transparenten ersetzt, und `SPI_SETCURSORS` lädt danach das Schema des Benutzers
//! neu. Nach einem Absturz im Remote-Modus stellt der nächste Start den Cursor wieder her.

use std::ptr::null_mut;

use windows_sys::Win32::UI::WindowsAndMessaging::{CreateCursor, SetSystemCursor, SystemParametersInfoW, SPI_SETCURSORS};

/// OCR_NORMAL, IBEAM, WAIT, CROSS, UP, SIZENWSE, SIZENESW, SIZEWE, SIZENS, SIZEALL, NO, HAND, APPSTARTING, HELP
const SYSTEM_CURSORS: [u32; 14] =
    [32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650, 32651];

pub fn hide() {
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

pub fn restore() {
    unsafe {
        SystemParametersInfoW(SPI_SETCURSORS, 0, null_mut(), 0);
    }
}
