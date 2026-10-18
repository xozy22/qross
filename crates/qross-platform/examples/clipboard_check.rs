//! Prüft die echte Zwischenablage dieses Rechners: Text + HTML + Bild schreiben und lesen sowie
//! ein reines DIB (wie von Win+Shift+S) als PNG lesen. Der vorherige Inhalt wird wiederhergestellt.
//! `cargo run -p qross-platform --example clipboard_check`

use qross_platform::clipformat::{png_to_rgba, rgba_to_png, Rgba};
use qross_proto::ClipboardData;

fn main() -> anyhow::Result<()> {
    let mut clipboard = qross_platform::clipboard();
    let backup = clipboard.read();

    let image = Rgba {
        width: 64,
        height: 48,
        pixels: (0..64 * 48).flat_map(|i: u32| [(i % 64 * 4) as u8, (i / 64 * 5) as u8, 128, 255]).collect(),
    };
    let data = ClipboardData {
        text: Some("Qross Prüftext".into()),
        html: Some("<p><b>Qross</b> Prüf-HTML</p>".into()),
        png: Some(rgba_to_png(&image)?),
    };
    let before = clipboard.sequence();
    clipboard.write(&data)?;
    let read = clipboard.read().expect("Zwischenablage leer");
    println!("Sequenz geändert: {}", clipboard.sequence() != before);
    println!("Text gleich: {}", read.text == data.text);
    println!("HTML gleich: {}", read.html == data.html);
    println!("Bild gleich: {}", read.png.as_deref().map(png_to_rgba).transpose()?.as_ref() == Some(&image));

    #[cfg(windows)]
    {
        dib_only(&image)?;
        let read = clipboard.read().expect("Zwischenablage leer");
        let decoded = read.png.as_deref().map(png_to_rgba).transpose()?;
        println!("Nur-DIB (Screenshot-Fall) als PNG gelesen und gleich: {}", decoded.as_ref() == Some(&image));
    }

    match backup {
        Some(previous) => clipboard.write(&previous)?,
        None => clipboard.write(&ClipboardData::default())?,
    }
    println!("vorheriger Inhalt wiederhergestellt");
    Ok(())
}

/// Legt nur ein CF_DIB ab – so wie viele Screenshot-Werkzeuge.
#[cfg(windows)]
fn dib_only(image: &Rgba) -> anyhow::Result<()> {
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    let dib = qross_platform::clipformat::rgba_to_dib(image);
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, dib.len());
        let ptr = GlobalLock(mem) as *mut u8;
        std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr, dib.len());
        GlobalUnlock(mem);
        anyhow::ensure!(OpenClipboard(std::ptr::null_mut()) != 0, "Zwischenablage belegt");
        EmptyClipboard();
        SetClipboardData(8, mem); // CF_DIB
        CloseClipboard();
    }
    Ok(())
}
