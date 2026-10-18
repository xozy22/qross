//! Zuordnung USB-HID-Usage ↔ Linux-evdev-Keycode (`KEY_*` aus `input-event-codes.h`).
//! X11 verwendet mit dem evdev-Treiber `evdev + 8` als Keycode.
//!
//! Reine Tabellen ohne Systemaufrufe, damit sie auf jeder Plattform getestet werden können.

use qross_core::chars::{KeyChars, Produces};
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

// --- Zeichen unter X11 (Zeichenmodus) ---------------------------------------------------------

/// Keysyms von Latin-3 und Latin-4, die von Latin-1 abweichen (aus `keysymdef.h`)
#[rustfmt::skip]
const LATIN_3_4: &[(u32, char)] = &[
    (0x2a1, 'Ħ'), (0x2a6, 'Ĥ'), (0x2a9, 'İ'), (0x2ab, 'Ğ'), (0x2ac, 'Ĵ'), (0x2b1, 'ħ'), (0x2b6, 'ĥ'),
    (0x2b9, 'ı'), (0x2bb, 'ğ'), (0x2bc, 'ĵ'), (0x2c5, 'Ċ'), (0x2c6, 'Ĉ'), (0x2d5, 'Ġ'), (0x2d8, 'Ĝ'),
    (0x2dd, 'Ŭ'), (0x2de, 'Ŝ'), (0x2e5, 'ċ'), (0x2e6, 'ĉ'), (0x2f5, 'ġ'), (0x2f8, 'ĝ'), (0x2fd, 'ŭ'),
    (0x2fe, 'ŝ'),
    (0x3a2, 'ĸ'), (0x3a3, 'Ŗ'), (0x3a5, 'Ĩ'), (0x3a6, 'Ļ'), (0x3aa, 'Ē'), (0x3ab, 'Ģ'), (0x3ac, 'Ŧ'),
    (0x3b3, 'ŗ'), (0x3b5, 'ĩ'), (0x3b6, 'ļ'), (0x3ba, 'ē'), (0x3bb, 'ģ'), (0x3bc, 'ŧ'), (0x3bd, 'Ŋ'),
    (0x3bf, 'ŋ'), (0x3c0, 'Ā'), (0x3c7, 'Į'), (0x3cc, 'Ė'), (0x3cf, 'Ī'), (0x3d1, 'Ņ'), (0x3d2, 'Ō'),
    (0x3d3, 'Ķ'), (0x3d9, 'Ų'), (0x3dd, 'Ũ'), (0x3de, 'Ū'), (0x3e0, 'ā'), (0x3e7, 'į'), (0x3ec, 'ė'),
    (0x3ef, 'ī'), (0x3f1, 'ņ'), (0x3f2, 'ō'), (0x3f3, 'ķ'), (0x3f9, 'ų'), (0x3fd, 'ũ'), (0x3fe, 'ū'),
];

/// Satz- und Technikzeichen, die Layouts auf der AltGr-Ebene haben, sowie Latin-9 und Euro
#[rustfmt::skip]
const SYMBOLS: &[(u32, char)] = &[
    (0x13bc, 'Œ'), (0x13bd, 'œ'), (0x13be, 'Ÿ'), (0x20ac, '€'),
    (0x8fb, '←'), (0x8fc, '↑'), (0x8fd, '→'), (0x8fe, '↓'),
    (0xaa9, '—'), (0xaaa, '–'), (0xaae, '…'), (0xac9, '™'), (0xad0, '‘'), (0xad1, '’'), (0xad2, '“'),
    (0xad3, '”'), (0xad5, '‰'), (0xad6, '′'), (0xad7, '″'), (0xae6, '•'), (0xaf1, '†'), (0xaf2, '‡'),
    (0xafd, '‚'), (0xafe, '„'),
];

/// ISO 8859-2, obere Hälfte (0xA0–0xFF): Latin-2-Keysyms sind 0x100 + dieses Byte
const ISO_8859_2: &str = "\u{a0}Ą˘Ł¤ĽŚ§¨ŠŞŤŹ\u{ad}ŽŻ°ą˛ł´ľśˇ¸šşťź˝žżŔÁÂĂÄĹĆÇČÉĘËĚÍÎĎĐŃŇÓÔŐÖ×ŘŮÚŰÜÝŢßŕáâăäĺćçčéęëěíîďđńňóôőö÷řůúűüýţ˙";

/// Kyrillische Keysyms 0x6A1–0x6FF (der Hauptteil in der Reihenfolge von KOI8-R)
const CYRILLIC: &str = "ђѓёєѕіїјљњћќґўџ№ЂЃЁЄЅІЇЈЉЊЋЌҐЎЏюабцдефгхийклмнопярстужвьызшэщчъЮАБЦДЕФГХИЙКЛМНОПЯРСТУЖВЬЫЗШЭЩЧЪ";

/// Griechische Keysyms mit Akzent (0x7A1–0x7BB)
#[rustfmt::skip]
const GREEK_ACCENTED: &[(u32, char)] = &[
    (0x7a1, 'Ά'), (0x7a2, 'Έ'), (0x7a3, 'Ή'), (0x7a4, 'Ί'), (0x7a5, 'Ϊ'), (0x7a7, 'Ό'), (0x7a8, 'Ύ'),
    (0x7a9, 'Ϋ'), (0x7ab, 'Ώ'), (0x7ae, '΅'), (0x7af, '―'), (0x7b1, 'ά'), (0x7b2, 'έ'), (0x7b3, 'ή'),
    (0x7b4, 'ί'), (0x7b5, 'ϊ'), (0x7b6, 'ΐ'), (0x7b7, 'ό'), (0x7b8, 'ύ'), (0x7b9, 'ϋ'), (0x7ba, 'ΰ'),
    (0x7bb, 'ώ'),
];

/// Das Zeichen eines X11-Keysyms; `None` für Funktions-, Zusatz- und tote Tasten und für Schriften,
/// die Qross nicht kennt (dann bleibt es bei der Tastenposition).
pub fn char_from_keysym(keysym: u32) -> Option<char> {
    let table = |table: &[(u32, char)]| table.iter().find(|(k, _)| *k == keysym).map(|&(_, c)| c);
    let nth = |text: &str, i: u32| text.chars().nth(i as usize);
    match keysym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(keysym),
        0x0100_00a0..=0x0110_ffff => char::from_u32(keysym - 0x0100_0000),
        0x1a1..=0x1ff => nth(ISO_8859_2, keysym - 0x1a0),
        0x6a1..=0x6ff => nth(CYRILLIC, keysym - 0x6a1),
        0x7a1..=0x7bb => table(GREEK_ACCENTED),
        0x7c1..=0x7d1 => char::from_u32(0x391 + keysym - 0x7c1),
        0x7d2 => Some('Σ'),
        0x7d4..=0x7d9 => char::from_u32(0x3a4 + keysym - 0x7d4),
        0x7e1..=0x7f1 => char::from_u32(0x3b1 + keysym - 0x7e1),
        0x7f2 => Some('σ'),
        0x7f3 => Some('ς'),
        0x7f4..=0x7f9 => char::from_u32(0x3c4 + keysym - 0x7f4),
        _ => table(LATIN_3_4).or_else(|| table(SYMBOLS)),
    }
}

/// Keysym für ein Zeichen: Latin-1 direkt, sonst der Unicode-Keysym (0x0100_0000 + Codepunkt)
pub fn keysym_from_char(ch: char) -> u32 {
    match ch as u32 {
        c @ (0x20..=0x7e | 0xa0..=0xff) => c,
        c => 0x0100_0000 + c,
    }
}

/// Tote Tasten (`dead_grave` … `dead_ogonek`) → Akzentzeichen
pub fn dead_accent(keysym: u32) -> Option<char> {
    const ACCENTS: [char; 13] = ['`', '´', '^', '~', '¯', '˘', '˙', '¨', '°', '˝', 'ˇ', '¸', '˛'];
    (0xfe50..=0xfe5c).contains(&keysym).then(|| ACCENTS[(keysym - 0xfe50) as usize])
}

/// Spalte im Kern-Tastaturmapping (GetKeyboardMapping) für XKB-Gruppe und Ebene 0–3, wie der
/// X-Server sie aus XKB ableitet: G1L1 G1L2 G2L1 G2L2 G1L3 G1L4 G2L3 G2L4, dann G3, G4.
pub fn x11_column(group: u8, level: u8) -> usize {
    let (group, level) = (usize::from(group.min(3)), usize::from(level.min(3)));
    match group {
        0 | 1 => group * 2 + level % 2 + if level >= 2 { 4 } else { 0 },
        _ => 8 + (group - 2) * 4 + level,
    }
}

/// Was eine Taste unter X11 ergibt. `syms`: ihre Keysyms aus dem Kern-Mapping, `state`: der
/// Zustand aus dem KeyPress (Umschalt 1, Feststell 2, Strg 4, Mod1/Alt 8, Mod4/Super 0x40, Gruppe in
/// Bit 13–14), `level3`: die Modifier-Maske von AltGr (ISO_Level3_Shift, meist Mod5).
pub fn x11_key_chars(syms: &[u32], state: u16, level3: u16) -> KeyChars {
    let (shift, lock, ctrl, alt, meta) = (state & 1 != 0, state & 2 != 0, state & 4 != 0, state & 8 != 0, state & 0x40 != 0);
    let altgr = level3 != 0 && state & level3 != 0;
    let at = |group: u8, level: u8| syms.get(x11_column(group, level)).copied().filter(|&sym| sym != 0);
    // Gruppen ohne eigene Belegung (z. B. Leertaste mit „us,de“) nehmen die erste
    let group = Some(((state >> 13) & 3) as u8).filter(|&g| at(g, 0).is_some()).unwrap_or(0);
    let char_of = |sym: u32| dead_accent(sym).map(Produces::Dead).unwrap_or_else(|| {
        char_from_keysym(sym).map_or(Produces::Nothing, |c| Produces::from_text(c.to_string()))
    });
    let base = at(group, 0).map(char_of).and_then(|p| p.first());
    if ctrl || meta || (alt && !altgr) {
        return KeyChars { produces: Produces::Nothing, base, shortcut: true };
    }
    // Ebene 2 fehlt: Ebene 1, bei Buchstaben großgeschrieben; AltGr ohne eigene Ebene: wie ohne
    let two_level = |shift: bool| match (shift, at(group, 1)) {
        (true, Some(sym)) => Some((sym, false)),
        (true, None) => at(group, 0).map(|sym| (sym, true)),
        (false, _) => at(group, 0).map(|sym| (sym, false)),
    };
    let picked = if altgr { at(group, 2 + u8::from(shift)).or(at(group, 2)).map(|sym| (sym, false)) } else { None };
    let Some((sym, upper)) = picked.or_else(|| two_level(shift)) else {
        return KeyChars { produces: Produces::Nothing, base, shortcut: false };
    };
    let produces = match char_of(sym) {
        // Feststelltaste und fehlende Ebene 2 betreffen nur Buchstaben; Umschalt hebt die
        // Feststelltaste auf (wie der XKB-Typ ALPHABETIC).
        Produces::Text(text) => match text.chars().next() {
            Some(c) if c.is_alphabetic() && lock => Produces::Text(case(c, !shift).to_string()),
            Some(c) if c.is_alphabetic() && upper => Produces::Text(case(c, true).to_string()),
            _ => Produces::Text(text),
        },
        other => other,
    };
    KeyChars { produces, base, shortcut: false }
}

/// Groß- bzw. Kleinbuchstabe, wenn es dafür genau ein Zeichen gibt (ß bleibt ß)
fn case(c: char, upper: bool) -> char {
    let mut converted: Vec<char> = if upper { c.to_uppercase().collect() } else { c.to_lowercase().collect() };
    match converted.len() {
        1 => converted.remove(0),
        _ => c,
    }
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

    // Keysyms aus keysymdef.h
    const XK_A_LOWER: u32 = 0x61;
    const XK_A_UPPER: u32 = 0x41;
    const XK_Q_LOWER: u32 = 0x71;
    const XK_Q_UPPER: u32 = 0x51;
    const XK_AT: u32 = 0x40;
    const XK_DEAD_CIRCUMFLEX: u32 = 0xfe52;
    const XK_DEGREE: u32 = 0xb0;
    const XK_ODIAERESIS: u32 = 0xf6;
    const XK_ODIAERESIS_UPPER: u32 = 0xd6;
    const XK_DEAD_DOUBLEACUTE: u32 = 0xfe59;
    const XK_SEVEN: u32 = 0x37;
    const XK_SLASH: u32 = 0x2f;
    const XK_BRACELEFT: u32 = 0x7b;
    const XK_CYRILLIC_IE: u32 = 0x6c5;
    const XK_CYRILLIC_IE_UPPER: u32 = 0x6e5;
    const SHIFT: u16 = 1;
    const LOCK: u16 = 2;
    const CTRL: u16 = 4;
    const ALT: u16 = 8;
    const MOD5: u16 = 0x80;
    const GROUP2: u16 = 1 << 13;

    #[test]
    fn keysyms_become_characters() {
        assert_eq!(ISO_8859_2.chars().count(), 96);
        assert_eq!(CYRILLIC.chars().count(), 0x6ff - 0x6a1 + 1);
        for (keysym, c) in [
            (0x61, 'a'), (0xe4, 'ä'), (0xdf, 'ß'), (0x20, ' '), (0x20ac, '€'), (0x1a3, 'Ł'), (0x1b1, 'ą'),
            (0x1e8, 'č'), (0x1ff, '˙'), (0x2b9, 'ı'), (0x3bf, 'ŋ'), (0x13bd, 'œ'), (0x6a3, 'ё'), (0x6b0, '№'),
            (0x6c1, 'а'), (0x6e1, 'А'), (0x6ff, 'Ъ'), (0x7c1, 'Α'), (0x7d1, 'Ρ'), (0x7d2, 'Σ'), (0x7d9, 'Ω'),
            (0x7e1, 'α'), (0x7f3, 'ς'), (0x7f9, 'ω'), (0x7b1, 'ά'), (0xafe, '„'), (0x8fd, '→'),
            (0x0100_20bd, '₽'), (0x0101_f600, '😀'),
        ] {
            assert_eq!(char_from_keysym(keysym), Some(c), "{keysym:#x}");
        }
        // Funktionstasten, Zusatztasten und tote Tasten sind keine Zeichen
        for keysym in [0xff08, 0xff0d, 0xffe1, 0xfe03, XK_DEAD_CIRCUMFLEX, 0x7d3, 0, 0x1a0] {
            assert_eq!(char_from_keysym(keysym), None, "{keysym:#x}");
        }
        assert_eq!(keysym_from_char('a'), 0x61);
        assert_eq!(keysym_from_char('ä'), 0xe4);
        assert_eq!(keysym_from_char('€'), 0x0100_20ac);
        assert_eq!(char_from_keysym(keysym_from_char('€')), Some('€'));
        assert_eq!(dead_accent(XK_DEAD_CIRCUMFLEX), Some('^'));
        assert_eq!(dead_accent(0xfe50), Some('`'));
        assert_eq!(dead_accent(0xfe51), Some('´'));
        assert_eq!(dead_accent(0xfe5c), Some('˛'));
        assert_eq!(dead_accent(0xfe5d), None);
    }

    #[test]
    fn columns_follow_the_xkb_core_mapping() {
        assert_eq!([0, 1, 2, 3].map(|level| x11_column(0, level)), [0, 1, 4, 5]);
        assert_eq!([0, 1, 2, 3].map(|level| x11_column(1, level)), [2, 3, 6, 7]);
        assert_eq!(x11_column(2, 0), 8);
        assert_eq!(x11_column(3, 3), 15);
    }

    /// Wie die Tasten im deutschen Layout im Kern-Mapping stehen (je G1L1 G1L2 G2L1 G2L2 G1L3 G1L4)
    fn german(level1: u32, level2: u32, level3: u32, level4: u32) -> Vec<u32> {
        vec![level1, level2, level1, level2, level3, level4]
    }

    #[test]
    fn german_x11_layout_with_altgr_dead_keys_and_caps_lock() {
        let q = german(XK_Q_LOWER, XK_Q_UPPER, XK_AT, 0x7d9);
        let text = |s: &str| Produces::Text(s.into());
        assert_eq!(x11_key_chars(&q, 0, MOD5), KeyChars::text("q", 'q'));
        assert_eq!(x11_key_chars(&q, SHIFT, MOD5).produces, text("Q"));
        assert_eq!(x11_key_chars(&q, MOD5, MOD5).produces, text("@"), "AltGr+Q");
        assert_eq!(x11_key_chars(&q, LOCK, MOD5).produces, text("Q"), "Feststelltaste");
        assert_eq!(x11_key_chars(&q, LOCK | SHIFT, MOD5).produces, text("q"), "Umschalt hebt sie auf");
        assert_eq!(x11_key_chars(&q, LOCK | MOD5, MOD5).produces, text("@"), "nur Buchstaben");

        let seven = german(XK_SEVEN, XK_SLASH, XK_BRACELEFT, 0);
        assert_eq!(x11_key_chars(&seven, MOD5, MOD5).produces, text("{"));
        assert_eq!(x11_key_chars(&seven, SHIFT | MOD5, MOD5).produces, text("{"), "Ebene 4 fehlt: Ebene 3");
        assert_eq!(x11_key_chars(&seven, LOCK, MOD5).produces, text("7"));

        let circumflex = german(XK_DEAD_CIRCUMFLEX, XK_DEGREE, 0, 0);
        assert_eq!(x11_key_chars(&circumflex, 0, MOD5).produces, Produces::Dead('^'));
        assert_eq!(x11_key_chars(&circumflex, 0, MOD5).base, Some('^'));
        assert_eq!(x11_key_chars(&circumflex, SHIFT, MOD5).produces, text("°"));
        let o = german(XK_ODIAERESIS, XK_ODIAERESIS_UPPER, XK_DEAD_DOUBLEACUTE, 0);
        assert_eq!(x11_key_chars(&o, MOD5, MOD5).produces, Produces::Dead('˝'));

        // Kürzel: Strg und Alt; AltGr nicht
        for state in [CTRL, ALT, CTRL | SHIFT, 0x40] {
            assert_eq!(x11_key_chars(&q, state, MOD5), KeyChars { produces: Produces::Nothing, base: Some('q'), shortcut: true });
        }
        // Ein Layout ohne AltGr-Modifier: Mod5 ist dann nichts Besonderes
        assert_eq!(x11_key_chars(&q, MOD5, 0).produces, text("q"));
    }

    #[test]
    fn x11_groups_and_missing_levels() {
        // „us,ru“: Gruppe 2 ist Russisch
        let key = vec![XK_A_LOWER, XK_A_UPPER, XK_CYRILLIC_IE, XK_CYRILLIC_IE_UPPER];
        assert_eq!(x11_key_chars(&key, GROUP2, 0).produces, Produces::Text("е".into()));
        assert_eq!(x11_key_chars(&key, GROUP2 | SHIFT, 0).produces, Produces::Text("Е".into()));
        assert_eq!(x11_key_chars(&key, GROUP2 | CTRL, 0).base, Some('е'));
        // Nur eine Gruppe belegt (Leertaste) und keine Ebene 2: Ebene 1, Buchstaben groß
        let space = vec![0x20];
        assert_eq!(x11_key_chars(&space, GROUP2 | SHIFT, 0).produces, Produces::Text(" ".into()));
        let single = vec![XK_A_LOWER];
        assert_eq!(x11_key_chars(&single, SHIFT, 0).produces, Produces::Text("A".into()));
        assert_eq!(x11_key_chars(&[], 0, 0), KeyChars { produces: Produces::Nothing, base: None, shortcut: false });
        assert_eq!(case('ß', true), 'ß');
    }
}
