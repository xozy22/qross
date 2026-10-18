//! Zuordnung Windows-Scancode (Set 1) ↔ USB-HID-Usage.
//!
//! Reine Tabellen ohne Windows-API, damit sie auf jeder Plattform getestet werden können.

use qross_proto::KeyCode;

/// Kennzeichnet in der Tabelle einen erweiterten Scancode (Präfix 0xE0).
const EXT: u16 = 0xE000;

const VK_PAUSE: u32 = 0x13;
const VK_NUMLOCK: u32 = 0x90;

/// So wird eine Taste unter Windows eingespeist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinKey {
    pub scan: u16,
    pub extended: bool,
    /// Einige Tasten lassen sich nur zuverlässig per Virtual-Key einspeisen.
    pub vk: Option<u16>,
}

#[rustfmt::skip]
const TABLE: &[(u16, u16)] = &[
    (0x01, 0x29), // Esc
    (0x02, 0x1E), (0x03, 0x1F), (0x04, 0x20), (0x05, 0x21), (0x06, 0x22), // 1–5
    (0x07, 0x23), (0x08, 0x24), (0x09, 0x25), (0x0A, 0x26), (0x0B, 0x27), // 6–0
    (0x0C, 0x2D), (0x0D, 0x2E), (0x0E, 0x2A), (0x0F, 0x2B), // - = Backspace Tab
    (0x10, 0x14), (0x11, 0x1A), (0x12, 0x08), (0x13, 0x15), (0x14, 0x17), // Q W E R T
    (0x15, 0x1C), (0x16, 0x18), (0x17, 0x0C), (0x18, 0x12), (0x19, 0x13), // Y U I O P
    (0x1A, 0x2F), (0x1B, 0x30), (0x1C, 0x28), (0x1D, 0xE0), // [ ] Enter LCtrl
    (0x1E, 0x04), (0x1F, 0x16), (0x20, 0x07), (0x21, 0x09), (0x22, 0x0A), // A S D F G
    (0x23, 0x0B), (0x24, 0x0D), (0x25, 0x0E), (0x26, 0x0F), // H J K L
    (0x27, 0x33), (0x28, 0x34), (0x29, 0x35), (0x2A, 0xE1), (0x2B, 0x31), // ; ' ` LShift \
    (0x2C, 0x1D), (0x2D, 0x1B), (0x2E, 0x06), (0x2F, 0x19), (0x30, 0x05), // Z X C V B
    (0x31, 0x11), (0x32, 0x10), (0x33, 0x36), (0x34, 0x37), (0x35, 0x38), // N M , . /
    (0x36, 0xE5), (0x37, 0x55), (0x38, 0xE2), (0x39, 0x2C), (0x3A, 0x39), // RShift KP* LAlt Space Caps
    (0x3B, 0x3A), (0x3C, 0x3B), (0x3D, 0x3C), (0x3E, 0x3D), (0x3F, 0x3E), // F1–F5
    (0x40, 0x3F), (0x41, 0x40), (0x42, 0x41), (0x43, 0x42), (0x44, 0x43), // F6–F10
    (0x46, 0x47), // ScrollLock
    (0x47, 0x5F), (0x48, 0x60), (0x49, 0x61), (0x4A, 0x56), // KP7 KP8 KP9 KP-
    (0x4B, 0x5C), (0x4C, 0x5D), (0x4D, 0x5E), (0x4E, 0x57), // KP4 KP5 KP6 KP+
    (0x4F, 0x59), (0x50, 0x5A), (0x51, 0x5B), (0x52, 0x62), (0x53, 0x63), // KP1 KP2 KP3 KP0 KP.
    (0x56, 0x64), // ISO-Taste <> (deutsche Tastatur)
    (0x57, 0x44), (0x58, 0x45), (0x59, 0x67), // F11 F12 KP=
    (0x64, 0x68), (0x65, 0x69), (0x66, 0x6A), (0x67, 0x6B), (0x68, 0x6C), (0x69, 0x6D), // F13–F18
    (0x6A, 0x6E), (0x6B, 0x6F), (0x6C, 0x70), (0x6D, 0x71), (0x6E, 0x72), (0x76, 0x73), // F19–F24
    (0x70, 0x88), (0x73, 0x87), (0x79, 0x8A), (0x7B, 0x8B), (0x7D, 0x89), // japanische Tasten
    (EXT | 0x1C, 0x58), (EXT | 0x1D, 0xE4), (EXT | 0x35, 0x54), (EXT | 0x37, 0x46), // KP-Enter RCtrl KP/ Druck
    (EXT | 0x38, 0xE6), // RAlt / AltGr
    (EXT | 0x47, 0x4A), (EXT | 0x48, 0x52), (EXT | 0x49, 0x4B), // Pos1 ↑ BildAuf
    (EXT | 0x4B, 0x50), (EXT | 0x4D, 0x4F), // ← →
    (EXT | 0x4F, 0x4D), (EXT | 0x50, 0x51), (EXT | 0x51, 0x4E), // Ende ↓ BildAb
    (EXT | 0x52, 0x49), (EXT | 0x53, 0x4C), // Einfg Entf
    (EXT | 0x5B, 0xE3), (EXT | 0x5C, 0xE7), (EXT | 0x5D, 0x65), // LWin RWin Menü
];

/// Medientasten: (Virtual-Key, Scancode, HID). Windows meldet sie je nach Tastatur mit oder ohne
/// Scancode, und Programme reagieren auf den Virtual-Key – daher in beide Richtungen über ihn.
#[rustfmt::skip]
const MEDIA: &[(u32, u16, u16)] = &[
    (0xAD, EXT | 0x20, KeyCode::MUTE.0), (0xAE, EXT | 0x2E, KeyCode::VOLUME_DOWN.0),
    (0xAF, EXT | 0x30, KeyCode::VOLUME_UP.0),
    (0xB0, EXT | 0x19, KeyCode::MEDIA_NEXT.0), (0xB1, EXT | 0x10, KeyCode::MEDIA_PREVIOUS.0),
    (0xB2, EXT | 0x24, KeyCode::MEDIA_STOP.0), (0xB3, EXT | 0x22, KeyCode::MEDIA_PLAY_PAUSE.0),
];

/// Zusätzliche Scancodes, die Windows liefert, die aber auf eine schon belegte HID-Usage zeigen.
const FORWARD_ONLY: &[(u16, u16)] = &[
    (0x54, 0x46), // Alt+Druck meldet SysRq
];

/// Zusätzliche HID-Usages ohne eigenen Windows-Scancode.
const REVERSE_ONLY: &[(u16, u16)] = &[
    (0x32, 0x2B), // Non-US # → Windows meldet dafür 0x2B
];

/// Übersetzt ein Ereignis aus dem Low-Level-Tastatur-Hook.
pub fn hid_from_windows(scan: u32, extended: bool, vk: u32) -> Option<KeyCode> {
    match vk {
        VK_PAUSE => return Some(KeyCode(0x48)),
        VK_NUMLOCK => return Some(KeyCode(0x53)),
        _ => {}
    }
    if let Some(&(_, _, hid)) = MEDIA.iter().find(|(media_vk, ..)| *media_vk == vk) {
        return Some(KeyCode(hid));
    }
    let scan = (scan & 0xFF) as u16;
    let lookup = |code: u16| {
        TABLE
            .iter()
            .chain(FORWARD_ONLY)
            .find(|(s, _)| *s == code)
            .map(|&(_, hid)| KeyCode(hid))
    };
    if extended {
        // Manche Tasten (z. B. RShift) kommen je nach Treiber mit gesetztem Extended-Flag.
        lookup(EXT | scan).or_else(|| lookup(scan))
    } else {
        lookup(scan)
    }
}

pub fn windows_from_hid(key: KeyCode) -> Option<WinKey> {
    match key.0 {
        0x48 => return Some(WinKey { scan: 0x45, extended: false, vk: Some(VK_PAUSE as u16) }),
        0x53 => return Some(WinKey { scan: 0x45, extended: true, vk: Some(VK_NUMLOCK as u16) }),
        _ => {}
    }
    if let Some(&(vk, scan, _)) = MEDIA.iter().find(|(.., hid)| *hid == key.0) {
        return Some(WinKey { scan: scan & 0xFF, extended: true, vk: Some(vk as u16) });
    }
    let code = TABLE
        .iter()
        .find(|(_, hid)| *hid == key.0)
        .or_else(|| REVERSE_ONLY.iter().find(|(hid, _)| *hid == key.0).map(|(_, scan)| {
            TABLE.iter().find(|(s, _)| s == scan).expect("REVERSE_ONLY zeigt auf TABLE")
        }))
        .map(|&(scan, _)| scan)?;
    Some(WinKey { scan: code & 0xFF, extended: code & EXT != 0, vk: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_is_unique_in_both_directions() {
        let scans: HashSet<_> = TABLE.iter().map(|(s, _)| s).collect();
        let hids: HashSet<_> = TABLE.iter().map(|(_, h)| h).collect();
        assert_eq!(scans.len(), TABLE.len());
        assert_eq!(hids.len(), TABLE.len());
    }

    #[test]
    fn every_table_entry_roundtrips() {
        for &(code, hid) in TABLE {
            let key = hid_from_windows(code as u32 & 0xFF, code & EXT != 0, 0).unwrap();
            assert_eq!(key, KeyCode(hid));
            let win = windows_from_hid(key).unwrap();
            assert_eq!((win.scan, win.extended), (code & 0xFF, code & EXT != 0), "HID {hid:#x}");
        }
    }

    #[test]
    fn special_keys() {
        // Pause und NumLock teilen sich Scancode 0x45 und werden über den Virtual-Key unterschieden.
        assert_eq!(hid_from_windows(0x45, false, VK_PAUSE), Some(KeyCode(0x48)));
        assert_eq!(hid_from_windows(0x45, true, VK_NUMLOCK), Some(KeyCode(0x53)));
        assert_eq!(windows_from_hid(KeyCode(0x48)).unwrap().vk, Some(VK_PAUSE as u16));
        // RShift mit Extended-Flag
        assert_eq!(hid_from_windows(0x36, true, 0xA1), Some(KeyCode(0xE5)));
        // Deutsche <>-Taste und AltGr
        assert_eq!(hid_from_windows(0x56, false, 0xE2), Some(KeyCode(0x64)));
        assert_eq!(windows_from_hid(KeyCode::RIGHT_ALT), Some(WinKey { scan: 0x38, extended: true, vk: None }));
        assert_eq!(windows_from_hid(KeyCode(0x32)).unwrap().scan, 0x2B);
        assert_eq!(hid_from_windows(0x00, false, 0xFF), None);
    }

    #[test]
    fn media_keys_go_by_virtual_key_in_both_directions() {
        for &(vk, scan, hid) in MEDIA {
            // Mit und ohne Scancode (manche Tastaturen melden 0)
            assert_eq!(hid_from_windows(0, false, vk), Some(KeyCode(hid)));
            assert_eq!(hid_from_windows((scan & 0xFF) as u32, true, vk), Some(KeyCode(hid)));
            let win = windows_from_hid(KeyCode(hid)).unwrap();
            assert_eq!(win, WinKey { scan: scan & 0xFF, extended: true, vk: Some(vk as u16) });
            assert!(KeyCode(hid).is_media());
        }
        assert_eq!(hid_from_windows(0x22, true, 0xB3), Some(KeyCode::MEDIA_PLAY_PAUSE));
    }
}
