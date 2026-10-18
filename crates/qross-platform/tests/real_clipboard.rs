//! Echte Zwischenablage. Unter Linux und macOS (über `arboard`) laufen die Tests nur in CI
//! (`CI=true`) oder mit `QROSS_CLIPBOARD_TEST=1`, weil sie den Inhalt der Zwischenablage ersetzen;
//! unter Linux brauchen sie einen X-Server (`xvfb-run`). Unter Windows läuft nur die Erkennung der
//! Passwortmanager, und nur mit `QROSS_CLIPBOARD_TEST=1`; den Rest prüft
//! `cargo run -p qross-platform --example clipboard_check`.

fn enabled() -> bool {
    let ci = !cfg!(windows) && std::env::var_os("CI").is_some_and(|v| v == "true");
    let on = ci || std::env::var_os("QROSS_CLIPBOARD_TEST").is_some();
    if !on {
        eprintln!("echte Zwischenablage nur in CI oder mit QROSS_CLIPBOARD_TEST=1 – übersprungen");
    }
    on && (cfg!(any(windows, target_os = "macos")) || std::env::var_os("DISPLAY").is_some())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod arboard_systems {
    use std::path::PathBuf;

    use qross_platform::clipformat::{png_to_rgba, rgba_to_png, Rgba};
    use qross_proto::ClipboardData;

    use super::enabled;

    /// macOS liefert Pfade aufgelöst zurück (`/var` → `/private/var`).
    fn canonical(paths: &[PathBuf]) -> Vec<PathBuf> {
        paths.iter().map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone())).collect()
    }

    #[test]
    fn real_clipboard_roundtrips_formats_and_files() {
        if !enabled() {
            return;
        }
        let mut clipboard = qross_platform::clipboard();

        let before = clipboard.sequence();
        let html = ClipboardData { text: Some("Grüße".into()), html: Some("<b>Grüße</b>".into()), png: None };
        clipboard.write(&html).unwrap();
        assert_ne!(clipboard.sequence(), before, "Sequenz ändert sich mit dem Inhalt");
        let read = clipboard.read().expect("Text und HTML");
        assert_eq!(read.text.as_deref(), Some("Grüße"));
        assert!(read.html.as_deref().is_some_and(|h| h.contains("<b>Grüße</b>")), "{read:?}");

        let image =
            Rgba { width: 8, height: 4, pixels: (0..32u32).flat_map(|i| [i as u8 * 8, 255 - i as u8, 64, 255]).collect() };
        clipboard.write(&ClipboardData { png: Some(rgba_to_png(&image).unwrap()), ..Default::default() }).unwrap();
        let png = clipboard.read().and_then(|d| d.png).expect("Bild");
        assert_eq!(png_to_rgba(&png).unwrap(), image);
        assert!(clipboard.files().is_empty(), "ein Bild ist keine Dateiliste");

        let dir = std::env::temp_dir().join(format!("qross-real-clipboard-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Ordner Ä")).unwrap();
        std::fs::write(dir.join("Bericht Grüße.txt"), "Qross").unwrap();
        let files = vec![dir.join("Bericht Grüße.txt"), dir.join("Ordner Ä")];
        let seq = clipboard.sequence();
        clipboard.write_files(&files, &ClipboardData::default()).unwrap();
        assert_eq!(canonical(&clipboard.files()), canonical(&files));
        assert_ne!(clipboard.sequence(), seq, "Sequenz ändert sich mit den Dateien");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ein anderes Programm markiert wie KeePassXC: unter Linux `x-kde-passwordManagerHint: secret`,
    /// am Mac `org.nspasteboard.ConcealedType` (`arboard` setzt dasselbe mit `exclude_from_history`).
    #[test]
    fn real_clipboard_recognizes_password_managers() {
        #[cfg(target_os = "linux")]
        use arboard::SetExtLinux as _;
        #[cfg(target_os = "macos")]
        use arboard::SetExtApple as _;

        if !enabled() {
            return;
        }
        let mut clipboard = qross_platform::clipboard();
        clipboard.set_text("Notiz").unwrap();
        assert_eq!(clipboard.concealed(), None, "normaler Text");

        let mut manager = arboard::Clipboard::new().unwrap();
        manager.set().exclude_from_history().text("geheim").unwrap();
        let marker = if cfg!(target_os = "macos") { "org.nspasteboard.ConcealedType" } else { "x-kde-passwordManagerHint" };
        assert_eq!(clipboard.concealed(), Some(marker));
        assert_eq!(clipboard.text().as_deref(), Some("geheim"), "lesen darf Qross, nur nicht teilen");

        manager.set_text("wieder normal").unwrap();
        assert_eq!(clipboard.concealed(), None, "die Markierung gilt nur für den einen Inhalt");

        #[cfg(target_os = "macos")]
        {
            use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
            use objc2_foundation::NSString;

            // Text mit genau einem zusätzlichen (leeren) Typ, wie ihn Textbausteine und Generatoren setzen
            let copy_marked = |marker: &str| {
                let pasteboard = NSPasteboard::generalPasteboard();
                pasteboard.clearContents();
                // SAFETY: Konstante von AppKit
                assert!(pasteboard.setString_forType(&NSString::from_str("Baustein"), unsafe { NSPasteboardTypeString }));
                assert!(pasteboard.setString_forType(&NSString::from_str(""), &NSString::from_str(marker)));
            };
            copy_marked("org.nspasteboard.TransientType");
            assert_eq!(clipboard.concealed(), Some("org.nspasteboard.TransientType"));
            copy_marked("org.nspasteboard.AutoGeneratedType");
            assert_eq!(clipboard.concealed(), None, "automatisch erzeugt heißt nicht vertraulich");
        }
        drop(manager);
    }
}

#[cfg(windows)]
mod windows {
    use std::ptr::{copy_nonoverlapping, null_mut};
    use std::thread::sleep;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    use super::enabled;

    const CF_UNICODETEXT: u32 = 13;

    fn register(name: &str) -> u32 {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
    }

    /// Schreibt wie ein Passwortmanager: Text und dazu Formate mit diesem Inhalt
    fn copy(text: &str, markers: &[(&str, &[u8])]) {
        let text: Vec<u8> = text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
        let formats: Vec<(u32, &[u8])> =
            std::iter::once((CF_UNICODETEXT, &text[..])).chain(markers.iter().map(|(name, data)| (register(name), *data))).collect();
        unsafe {
            let mut tries = 0;
            while OpenClipboard(null_mut()) == 0 {
                tries += 1;
                assert!(tries < 100, "Zwischenablage belegt");
                sleep(Duration::from_millis(10));
            }
            EmptyClipboard();
            for (format, data) in formats {
                let mem = GlobalAlloc(GMEM_MOVEABLE, data.len().max(1));
                let ptr = GlobalLock(mem) as *mut u8;
                copy_nonoverlapping(data.as_ptr(), ptr, data.len());
                GlobalUnlock(mem);
                if SetClipboardData(format, mem).is_null() {
                    GlobalFree(mem);
                    panic!("Format {format} nicht gesetzt");
                }
            }
            CloseClipboard();
        }
    }

    #[test]
    fn real_clipboard_recognizes_password_managers() {
        if !enabled() {
            return;
        }
        let mut clipboard = qross_platform::clipboard();
        let backup = clipboard.read();

        // So legt KeePassXC ein Passwort ab
        let keepassxc: &[(&str, &[u8])] = &[
            ("ExcludeClipboardContentFromMonitorProcessing", b"1"),
            ("CanIncludeInClipboardHistory", &[0; 4]),
            ("CanUploadToCloudClipboard", &[0; 4]),
        ];
        copy("geheim", keepassxc);
        assert_eq!(clipboard.concealed(), Some("ExcludeClipboardContentFromMonitorProcessing"));
        assert_eq!(clipboard.text().as_deref(), Some("geheim"), "lesen darf Qross, nur nicht teilen");

        for (name, data) in [
            ("CanIncludeInClipboardHistory", &[0u8; 4][..]),
            ("CanUploadToCloudClipboard", &[0; 4][..]),
            ("Clipboard Viewer Ignore", &[][..]),
        ] {
            copy("geheim", &[(name, data)]);
            assert_eq!(clipboard.concealed(), Some(name), "{name} allein");
        }

        copy("erlaubt", &[("CanIncludeInClipboardHistory", &1u32.to_le_bytes()), ("CanUploadToCloudClipboard", &1u32.to_le_bytes())]);
        assert_eq!(clipboard.concealed(), None, "ausdrücklich erlaubt");
        clipboard.set_text("Notiz").unwrap();
        assert_eq!(clipboard.concealed(), None, "Qross selbst markiert nichts");

        match backup {
            Some(data) => clipboard.write(&data).unwrap(),
            None => clipboard.set_text("").unwrap(),
        }
    }
}
