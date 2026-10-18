//! Text-Clipboard (CF_UNICODETEXT).

use std::ptr::{copy_nonoverlapping, null_mut};
use std::thread::sleep;
use std::time::Duration;

use anyhow::bail;
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

use crate::Clipboard;

const CF_UNICODETEXT: u32 = 13;

pub fn clipboard() -> Box<dyn Clipboard> {
    Box::new(WinClipboard)
}

struct WinClipboard;

/// Offenes Clipboard; wird beim Drop geschlossen.
struct Open;

impl Open {
    /// Andere Programme halten das Clipboard oft kurz offen – daher ein paar Versuche.
    fn acquire() -> Option<Self> {
        for _ in 0..20 {
            if unsafe { OpenClipboard(null_mut()) } != 0 {
                return Some(Open);
            }
            sleep(Duration::from_millis(5));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe { CloseClipboard() };
    }
}

impl Clipboard for WinClipboard {
    fn text(&mut self) -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT) == 0 {
                return None;
            }
            let _open = Open::acquire()?;
            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                return None;
            }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let max = GlobalSize(handle) / 2;
            let mut len = 0;
            while len < max && *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            GlobalUnlock(handle);
            Some(text)
        }
    }

    fn set_text(&mut self, text: &str) -> anyhow::Result<()> {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            if mem.is_null() {
                bail!("GlobalAlloc fehlgeschlagen");
            }
            let ptr = GlobalLock(mem) as *mut u16;
            if ptr.is_null() {
                GlobalFree(mem);
                bail!("GlobalLock fehlgeschlagen");
            }
            copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(mem);

            let Some(_open) = Open::acquire() else {
                GlobalFree(mem);
                bail!("Clipboard ist von einem anderen Programm belegt");
            };
            EmptyClipboard();
            // Bei Erfolg gehört der Speicher dem System.
            if SetClipboardData(CF_UNICODETEXT, mem).is_null() {
                GlobalFree(mem);
                bail!("SetClipboardData fehlgeschlagen");
            }
        }
        Ok(())
    }

    fn sequence(&mut self) -> u64 {
        unsafe { GetClipboardSequenceNumber() as u64 }
    }
}
