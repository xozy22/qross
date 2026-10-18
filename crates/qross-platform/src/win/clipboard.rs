//! Zwischenablage unter Windows: Text (CF_UNICODETEXT), HTML („HTML Format“) und Bilder
//! („PNG“ von Browsern/Office, sonst CF_DIBV5/CF_DIB, z. B. von Win+Shift+S).
//!
//! Die Zwischenablage ist systemweit gesperrt, solange sie offen ist. Deshalb werden nur die
//! Rohdaten kopiert; Umwandlungen (DIB → PNG usw.) passieren danach.

use std::ptr::{copy_nonoverlapping, null_mut};
use std::sync::OnceLock;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{bail, Context};
use qross_proto::ClipboardData;
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

use crate::clipformat::{build_cf_html, cf_html_fragment, dib_to_rgba, png_to_rgba, rgba_to_dib, rgba_to_png};
use crate::Clipboard;

const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

pub fn clipboard() -> Box<dyn Clipboard> {
    Box::new(WinClipboard)
}

struct WinClipboard;

/// Formate, die Windows nicht fest vergibt, sondern zur Laufzeit registriert
fn registered(name: &'static str, slot: &'static OnceLock<u32>) -> u32 {
    *slot.get_or_init(|| {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
    })
}

fn html_format() -> u32 {
    static SLOT: OnceLock<u32> = OnceLock::new();
    registered("HTML Format", &SLOT)
}

fn png_format() -> u32 {
    static SLOT: OnceLock<u32> = OnceLock::new();
    registered("PNG", &SLOT)
}

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

/// Rohbytes eines Formats (nur bei offenem Clipboard aufrufen)
unsafe fn raw(format: u32) -> Option<Vec<u8>> {
    if format == 0 || IsClipboardFormatAvailable(format) == 0 {
        return None;
    }
    let handle = GetClipboardData(format);
    if handle.is_null() {
        return None;
    }
    let ptr = GlobalLock(handle) as *const u8;
    if ptr.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(ptr, GlobalSize(handle)).to_vec();
    GlobalUnlock(handle);
    Some(bytes)
}

/// Kopiert Bytes in einen beweglichen globalen Speicherblock für SetClipboardData.
unsafe fn global(bytes: &[u8]) -> anyhow::Result<*mut core::ffi::c_void> {
    let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
    if mem.is_null() {
        bail!("GlobalAlloc fehlgeschlagen");
    }
    let ptr = GlobalLock(mem) as *mut u8;
    if ptr.is_null() {
        GlobalFree(mem);
        bail!("GlobalLock fehlgeschlagen");
    }
    copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
    GlobalUnlock(mem);
    Ok(mem)
}

fn utf16_text(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

impl Clipboard for WinClipboard {
    fn read(&mut self) -> Option<ClipboardData> {
        let (text, html, png, dib) = {
            let _open = Open::acquire()?;
            unsafe {
                let png = raw(png_format()).filter(|b| b.starts_with(PNG_SIGNATURE));
                let dib = if png.is_none() { raw(CF_DIBV5).or_else(|| raw(CF_DIB)) } else { None };
                (raw(CF_UNICODETEXT), raw(html_format()), png, dib)
            }
        };
        let png = png.or_else(|| {
            let image = dib.as_deref().map(dib_to_rgba)?;
            match image.and_then(|img| rgba_to_png(&img)) {
                Ok(png) => Some(png),
                Err(e) => {
                    tracing::debug!("Bild in der Zwischenablage nicht lesbar: {e:#}");
                    None
                }
            }
        });
        let data = ClipboardData {
            text: text.map(|b| utf16_text(&b)).filter(|t| !t.is_empty()),
            html: html.and_then(|b| cf_html_fragment(b.split(|&c| c == 0).next().unwrap_or(&b))),
            png,
        };
        (!data.is_empty()).then_some(data)
    }

    fn write(&mut self, data: &ClipboardData) -> anyhow::Result<()> {
        // Alles vorbereiten, bevor das Clipboard gesperrt wird
        let text = data.text.as_ref().map(|t| {
            t.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect::<Vec<u8>>()
        });
        let html = data.html.as_ref().map(|h| {
            let mut block = build_cf_html(h);
            block.push(0);
            block
        });
        // Zusätzlich als DIB, damit auch ältere Programme (Paint, Office) das Bild einfügen können
        let dib = match &data.png {
            Some(png) => Some(rgba_to_dib(&png_to_rgba(png).context("Bild aus der Gegenstelle ist kein gültiges PNG")?)),
            None => None,
        };

        let formats = [
            (CF_UNICODETEXT, text),
            (html_format(), html),
            (png_format(), data.png.clone()),
            (CF_DIB, dib),
        ];
        let _open = Open::acquire().context("Clipboard ist von einem anderen Programm belegt")?;
        unsafe {
            EmptyClipboard();
            for (format, bytes) in formats {
                let Some(bytes) = bytes else { continue };
                if format == 0 {
                    continue;
                }
                let mem = global(&bytes)?;
                // Bei Erfolg gehört der Speicher dem System.
                if SetClipboardData(format, mem).is_null() {
                    GlobalFree(mem);
                    tracing::debug!("Format {format} nicht gesetzt");
                }
            }
        }
        Ok(())
    }

    fn sequence(&mut self) -> u64 {
        unsafe { GetClipboardSequenceNumber() as u64 }
    }
}
