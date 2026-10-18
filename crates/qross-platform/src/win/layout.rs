//! Zeichenmodus unter Windows: Was ergibt eine Taste im Tastaturlayout des Vordergrundfensters?
//!
//! `ToUnicodeEx` läuft mit Flag 4 (ab Windows 10 1607): Es lässt den Zustand toter Tasten im System
//! unverändert. Ohne das Flag würde der Aufruf im Hook eine tote Taste „verbrauchen“, und ^ ´ `
//! gingen in Programmen kaputt. Das Verbinden mit dem nächsten Buchstaben übernimmt deshalb
//! `qross_core::chars::Typist`.

use std::ptr::null_mut;

use qross_core::chars::{KeyChars, Produces};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, ToUnicodeEx, HKL, VK_CAPITAL, VK_CONTROL, VK_LCONTROL, VK_LSHIFT, VK_MENU, VK_RMENU,
    VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::keymap_windows::{windows_typing, WinModifiers, WinTyping};

/// Den Zustand toter Tasten nicht verändern (Windows 10 1607 und neuer)
const KEEP_STATE: u32 = 4;

/// Tastaturlayout des Vordergrundfensters (Windows stellt es je Programm ein)
pub(crate) fn foreground_layout() -> HKL {
    unsafe { GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(), null_mut())) }
}

/// Was die Taste (Virtual-Key, Scancode) mit diesen Zusatztasten im Layout `layout` ergibt
pub(crate) fn key_chars(vk: u32, scan: u32, mods: WinModifiers, layout: HKL) -> KeyChars {
    let base = translate(vk, scan, &[0; 256], layout).first();
    match windows_typing(mods) {
        WinTyping::Shortcut => KeyChars { produces: Produces::Nothing, base, shortcut: true },
        WinTyping::Type { shift, caps, ctrl_alt } => {
            let mut state = [0u8; 256];
            let mut set = |vks: &[u16]| vks.iter().for_each(|&vk| state[usize::from(vk)] = 0x80);
            if shift {
                set(&[VK_SHIFT, VK_LSHIFT]);
            }
            if ctrl_alt {
                set(&[VK_CONTROL, VK_LCONTROL, VK_MENU, VK_RMENU]);
            }
            if caps {
                state[usize::from(VK_CAPITAL)] = 0x01;
            }
            let produces = translate(vk, scan, &state, layout);
            // Strg+Alt+Taste ohne Zeichen ist ein Kürzel
            let shortcut = ctrl_alt && produces == Produces::Nothing;
            KeyChars { produces, base, shortcut }
        }
    }
}

fn translate(vk: u32, scan: u32, state: &[u8; 256], layout: HKL) -> Produces {
    let mut buf = [0u16; 8];
    let n = unsafe { ToUnicodeEx(vk, scan, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, KEEP_STATE, layout) };
    match n {
        // Tote Taste: Windows schreibt die alleinstehende Form des Akzents
        n if n < 0 => char::from_u32(u32::from(buf[0])).map_or(Produces::Nothing, Produces::Dead),
        0 => Produces::Nothing,
        n => Produces::from_text(String::from_utf16_lossy(&buf[..(n as usize).min(buf.len())])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutList;

    /// Installierte Tastaturlayouts (ohne eines zu laden: Das änderte die Liste des Nutzers)
    fn installed() -> Vec<HKL> {
        let mut list = vec![null_mut(); 64];
        let n = unsafe { GetKeyboardLayoutList(list.len() as i32, list.as_mut_ptr()) };
        list.truncate(n.max(0) as usize);
        list
    }

    /// Layout nach seiner Kennung im oberen Wort des HKL (0x0407: Deutsch), egal für welche Sprache
    /// es eingerichtet ist (z. B. Englisch (Deutschland) mit deutscher Tastatur)
    fn find(layout: usize) -> Option<HKL> {
        installed().into_iter().find(|&hkl| (hkl as usize >> 16) & 0xFFFF == layout)
    }

    const NONE: WinModifiers =
        WinModifiers { shift: false, ctrl: false, alt: false, altgr: false, win: false, caps: false };
    const SHIFT: WinModifiers = WinModifiers { shift: true, ..NONE };
    const ALTGR: WinModifiers = WinModifiers { alt: true, altgr: true, ..NONE };
    const CTRL: WinModifiers = WinModifiers { ctrl: true, ..NONE };

    fn text(s: &str) -> Produces {
        Produces::Text(s.into())
    }

    #[test]
    fn german_layout_letters_altgr_and_dead_keys() {
        let Some(de) = find(0x0407) else {
            return eprintln!("deutsches Layout nicht installiert – übersprungen");
        };
        // Die Taste rechts neben T: VK_Z, Scancode 0x15 (US: Y)
        assert_eq!(key_chars(0x5A, 0x15, NONE, de), KeyChars::text("z", 'z'));
        assert_eq!(key_chars(0x5A, 0x15, SHIFT, de).produces, text("Z"));
        assert_eq!(key_chars(0x5A, 0x15, WinModifiers { caps: true, ..NONE }, de).produces, text("Z"));
        assert_eq!(key_chars(0x51, 0x10, ALTGR, de).produces, text("@"), "AltGr+Q");
        assert_eq!(key_chars(0x45, 0x12, ALTGR, de).produces, text("€"), "AltGr+E");
        assert_eq!(key_chars(0x37, 0x08, ALTGR, de).produces, text("{"), "AltGr+7");
        assert_eq!(key_chars(0xDB, 0x0C, ALTGR, de).produces, text("\\"), "AltGr+ß");
        let ctrl_alt = WinModifiers { ctrl: true, alt: true, ..NONE };
        assert_eq!(key_chars(0x51, 0x10, ctrl_alt, de).produces, text("@"), "Strg+Alt+Q wie AltGr");
        assert_eq!(key_chars(0xC0, 0x27, NONE, de).produces, text("ö"));
        // Tote Tasten: ^ links neben der 1, ´ und ` links neben der Rücktaste
        assert_eq!(key_chars(0xDC, 0x29, NONE, de).produces, Produces::Dead('^'));
        assert_eq!(key_chars(0xDD, 0x0D, NONE, de).produces, Produces::Dead('´'));
        assert_eq!(key_chars(0xDD, 0x0D, SHIFT, de).produces, Produces::Dead('`'));
        // Der Zustand bleibt: Ein zweiter Aufruf meldet wieder die tote Taste, kein „^^“
        assert_eq!(key_chars(0xDC, 0x29, NONE, de).produces, Produces::Dead('^'));
        assert_eq!(key_chars(0x45, 0x12, NONE, de).produces, text("e"), "kein ê: nichts gespeichert");
        // Kürzel: Strg+Z meint z
        assert_eq!(key_chars(0x5A, 0x15, CTRL, de), KeyChars { produces: Produces::Nothing, base: Some('z'), shortcut: true });
        // Strg+Alt+Taste ohne Zeichen ist ein Kürzel
        assert!(key_chars(0x46, 0x21, ctrl_alt, de).shortcut, "Strg+Alt+F");
    }

    #[test]
    fn us_layout_has_no_dead_keys() {
        let Some(us) = find(0x0409) else {
            return eprintln!("US-Layout nicht installiert – übersprungen");
        };
        assert_eq!(key_chars(0x59, 0x15, NONE, us), KeyChars::text("y", 'y'));
        assert_eq!(key_chars(0x32, 0x03, SHIFT, us).produces, text("@"));
        assert_eq!(key_chars(0xC0, 0x29, NONE, us).produces, text("`"));
        assert_eq!(key_chars(0xDE, 0x28, SHIFT, us).produces, text("\""));
        // Rechtes Alt ist hier kein AltGr: der Hook meldet dann kein künstliches Strg
        let alt = WinModifiers { alt: true, ..NONE };
        assert!(key_chars(0x51, 0x10, alt, us).shortcut);
    }

    #[test]
    fn the_foreground_layout_is_installed() {
        let layout = foreground_layout();
        if layout.is_null() {
            return eprintln!("kein Vordergrundfenster – übersprungen");
        }
        assert!(installed().contains(&layout), "{layout:?}");
    }
}
