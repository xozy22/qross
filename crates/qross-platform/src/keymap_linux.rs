//! Zuordnung USB-HID-Usage ↔ Linux-evdev-Keycode (`KEY_*` aus `input-event-codes.h`).
//! X11 verwendet mit dem evdev-Treiber `evdev + 8` als Keycode.
//!
//! Reine Tabellen ohne Systemaufrufe, damit sie auf jeder Plattform getestet werden können.

use qross_proto::KeyCode;

/// Abstand zwischen evdev-Code und X11-Keycode
pub const X11_OFFSET: u16 = 8;

#[rustfmt::skip]
const TABLE: &[(u16, u16)] = &[
    // Buchstaben A–Z
    (0x04, 30), (0x05, 48), (0x06, 46), (0x07, 32), (0x08, 18), (0x09, 33), (0x0A, 34), (0x0B, 35),
    (0x0C, 23), (0x0D, 36), (0x0E, 37), (0x0F, 38), (0x10, 50), (0x11, 49), (0x12, 24), (0x13, 25),
    (0x14, 16), (0x15, 19), (0x16, 31), (0x17, 20), (0x18, 22), (0x19, 47), (0x1A, 17), (0x1B, 45),
    (0x1C, 21), (0x1D, 44),
    // 1–9, 0
    (0x1E, 2), (0x1F, 3), (0x20, 4), (0x21, 5), (0x22, 6), (0x23, 7), (0x24, 8), (0x25, 9), (0x26, 10), (0x27, 11),
    (0x28, 28), (0x29, 1), (0x2A, 14), (0x2B, 15), (0x2C, 57), (0x2D, 12), (0x2E, 13), // Enter Esc ⌫ Tab Space - =
    (0x2F, 26), (0x30, 27), (0x31, 43), (0x33, 39), (0x34, 40), (0x35, 41), // [ ] \ ; ' `
    (0x36, 51), (0x37, 52), (0x38, 53), (0x39, 58), // , . / Caps
    // F1–F12
    (0x3A, 59), (0x3B, 60), (0x3C, 61), (0x3D, 62), (0x3E, 63), (0x3F, 64), (0x40, 65), (0x41, 66),
    (0x42, 67), (0x43, 68), (0x44, 87), (0x45, 88),
    (0x46, 99), (0x47, 70), (0x48, 119), // Druck Rollen Pause
    (0x49, 110), (0x4A, 102), (0x4B, 104), (0x4C, 111), (0x4D, 107), (0x4E, 109), // Einfg Pos1 BildAuf Entf Ende BildAb
    (0x4F, 106), (0x50, 105), (0x51, 108), (0x52, 103), // → ← ↓ ↑
    (0x53, 69), (0x54, 98), (0x55, 55), (0x56, 74), (0x57, 78), (0x58, 96), // Num KP/ KP* KP- KP+ KP-Enter
    (0x59, 79), (0x5A, 80), (0x5B, 81), (0x5C, 75), (0x5D, 76), (0x5E, 77), // KP1–KP6
    (0x5F, 71), (0x60, 72), (0x61, 73), (0x62, 82), (0x63, 83), // KP7–KP9 KP0 KP.
    (0x64, 86), (0x65, 127), (0x67, 117), // <> Menü KP=
    // F13–F24
    (0x68, 183), (0x69, 184), (0x6A, 185), (0x6B, 186), (0x6C, 187), (0x6D, 188),
    (0x6E, 189), (0x6F, 190), (0x70, 191), (0x71, 192), (0x72, 193), (0x73, 194),
    (0x7F, 113), (0x80, 115), (0x81, 114), // Stumm Lauter Leiser
    // Medientasten (HID-Consumer-Page): Weiter, Zurück, Stopp, Wiedergabe/Pause
    (0xC0B5, 163), (0xC0B6, 165), (0xC0B7, 166), (0xC0CD, 164),
    (0x87, 89), (0x88, 93), (0x89, 124), (0x8A, 92), (0x8B, 94), // japanische Tasten
    // Modifier
    (0xE0, 29), (0xE1, 42), (0xE2, 56), (0xE3, 125), (0xE4, 97), (0xE5, 54), (0xE6, 100), (0xE7, 126),
];

/// HID-Usages ohne eigenen evdev-Code
const REVERSE_ONLY: &[(u16, u16)] = &[
    (0x32, 43), // Non-US # → KEY_BACKSLASH (so macht es auch der Kernel)
];

pub fn evdev_from_hid(key: KeyCode) -> Option<u16> {
    TABLE.iter().chain(REVERSE_ONLY).find(|(hid, _)| *hid == key.0).map(|&(_, code)| code)
}

pub fn hid_from_evdev(code: u16) -> Option<KeyCode> {
    TABLE.iter().find(|(_, c)| *c == code).map(|&(hid, _)| KeyCode(hid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_is_unique_and_roundtrips() {
        let codes: HashSet<_> = TABLE.iter().map(|(_, c)| c).collect();
        let hids: HashSet<_> = TABLE.iter().map(|(h, _)| h).collect();
        assert_eq!(codes.len(), TABLE.len());
        assert_eq!(hids.len(), TABLE.len());
        for &(hid, code) in TABLE {
            assert_eq!(evdev_from_hid(KeyCode(hid)), Some(code));
            assert_eq!(hid_from_evdev(code), Some(KeyCode(hid)));
        }
    }

    #[test]
    fn covers_the_same_keys_as_windows() {
        // Jede Taste, die Windows erfassen kann, muss Linux einspeisen können (und umgekehrt).
        let media = [KeyCode::MEDIA_NEXT, KeyCode::MEDIA_PREVIOUS, KeyCode::MEDIA_STOP, KeyCode::MEDIA_PLAY_PAUSE];
        for key in (0..=0xFFu16).map(KeyCode).chain(media) {
            let hid = key.0;
            assert_eq!(
                crate::keymap_windows::windows_from_hid(key).is_some(),
                evdev_from_hid(key).is_some(),
                "HID {hid:#04x}"
            );
        }
    }

    #[test]
    fn evdev_matches_set1_scancodes_for_basic_keys() {
        // evdev stammt historisch von Scancode-Set 1 ab – Stichproben gegen die Windows-Tabelle
        for hid in [0x04u16, 0x1E, 0x28, 0x2C, 0x3A, 0x64, 0xE0, 0xE1] {
            let key = KeyCode(hid);
            assert_eq!(evdev_from_hid(key), crate::keymap_windows::windows_from_hid(key).map(|w| w.scan));
        }
    }
}
