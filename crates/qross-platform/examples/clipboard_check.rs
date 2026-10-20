//! Prüft die echte Zwischenablage dieses Rechners: Text + HTML + Bild schreiben und lesen sowie
//! ein reines DIB (wie von Win+Shift+S) als PNG lesen sowie kopierte Dateien ablegen und lesen.
//! Der vorherige Inhalt wird wiederhergestellt.
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
    let read = clipboard.read().expect("clipboard is empty");
    println!("Sequence changed: {}", clipboard.sequence() != before);
    println!("Text matches: {}", read.text == data.text);
    println!("HTML matches: {}", read.html == data.html);
    println!("Image matches: {}", read.png.as_deref().map(png_to_rgba).transpose()?.as_ref() == Some(&image));

    #[cfg(windows)]
    {
        dib_only(&image)?;
        let read = clipboard.read().expect("clipboard is empty");
        let decoded = read.png.as_deref().map(png_to_rgba).transpose()?;
        println!("DIB only (screenshot case) read as PNG and matches: {}", decoded.as_ref() == Some(&image));
    }

    // Kopierte Dateien: Qross legt ab und liest zurück. Unter Windows prüft zusätzlich Windows
    // selbst (.NET über OLE, wie andere Programme), was es sieht, und der Explorer fügt sie ein.
    let dir = std::env::temp_dir().join(format!("qross-clipboard-check-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("Ordner Ä"))?;
    std::fs::write(dir.join("Ordner Ä").join("drin.txt"), "im Ordner")?;
    std::fs::write(dir.join("Bericht Grüße.txt"), "Qross")?;
    let files = vec![dir.join("Bericht Grüße.txt"), dir.join("Ordner Ä")];
    clipboard.write_files(&files, &ClipboardData::from_text("Bericht Grüße.txt"))?;
    println!("Files match: {}", clipboard.files() == files);
    println!("File names included as text: {}", clipboard.text().as_deref() == Some("Bericht Grüße.txt"));
    #[cfg(windows)]
    {
        let shell = powershell("Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Clipboard]::GetFileDropList()")?;
        let expected: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        println!("Windows sees the same files: {}", shell == expected);
        if shell != expected {
            println!("  Windows: {shell:?}, expected: {expected:?}");
        }
        // Wie Strg+V im Explorer: das Shell-Verb „Einfügen“ auf einen Zielordner
        let target = dir.join("Ziel");
        std::fs::create_dir_all(&target)?;
        powershell(&format!(
            "(New-Object -ComObject Shell.Application).NameSpace('{}').Self.InvokeVerb('Paste')",
            target.display()
        ))?;
        let pasted = target.join("Ordner Ä").join("drin.txt");
        for _ in 0..50 {
            if pasted.exists() && target.join("Bericht Grüße.txt").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!(
            "Explorer pasted: {}",
            std::fs::read_to_string(target.join("Bericht Grüße.txt")).is_ok_and(|t| t == "Qross")
                && std::fs::read_to_string(&pasted).is_ok_and(|t| t == "im Ordner")
        );
        let single = dir.join("Bericht Grüße.txt");
        powershell(&format!("Set-Clipboard -LiteralPath '{}'", single.display()))?;
        println!("File copied in Explorer format read: {}", clipboard.files() == vec![single]);
    }
    let _ = std::fs::remove_dir_all(&dir);

    match backup {
        Some(previous) => clipboard.write(&previous)?,
        None => clipboard.write(&ClipboardData::default())?,
    }
    println!("Previous content restored");
    Ok(())
}

/// Zeilen der Ausgabe eines PowerShell-Befehls (UTF-8)
#[cfg(windows)]
fn powershell(command: &str) -> anyhow::Result<Vec<String>> {
    let script = format!("[Console]::OutputEncoding = [Text.Encoding]::UTF8; {command}");
    let out = std::process::Command::new("powershell").args(["-NoProfile", "-Sta", "-Command", &script]).output()?;
    anyhow::ensure!(out.status.success(), "PowerShell: {}", String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(str::to_owned).filter(|l| !l.is_empty()).collect())
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
        anyhow::ensure!(OpenClipboard(std::ptr::null_mut()) != 0, "clipboard in use");
        EmptyClipboard();
        SetClipboardData(8, mem); // CF_DIB
        CloseClipboard();
    }
    Ok(())
}
