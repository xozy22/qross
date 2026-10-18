//! Tastaturbelegung des X-Servers für den Zeichenmodus: Was ergibt eine Taste (Erfassung), und
//! welche Taste ergibt ein Zeichen (Einspeisen)? Gelesen wird das Kern-Mapping
//! (GetKeyboardMapping), das der X-Server aus XKB ableitet; nach jedem `MappingNotify` neu.

use anyhow::Result;
use qross_core::chars::KeyChars;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::keymap_linux::{char_from_keysym, x11_column, x11_key_chars};

const ISO_LEVEL3_SHIFT: u32 = 0xfe03;
const MODE_SWITCH: u32 = 0xff7e;

/// Kern-Tastaturmapping samt der Modifier-Maske von AltGr
pub(crate) struct Keymap {
    min: u8,
    per: u8,
    syms: Vec<u32>,
    /// Modifier-Bits, auf denen ISO_Level3_Shift bzw. Mode_switch liegt (meist Mod5)
    level3: u16,
}

impl Keymap {
    pub fn fetch(conn: &RustConnection) -> Result<Self> {
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let reply = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        let mut keymap = Self { min, per: reply.keysyms_per_keycode, syms: reply.keysyms, level3: 0 };
        let mods = conn.get_modifier_mapping()?.reply()?;
        let per_modifier = usize::from(mods.keycodes_per_modifier()).max(1);
        for (bit, codes) in mods.keycodes.chunks(per_modifier).enumerate().take(8) {
            let altgr = |&code: &u8| keymap.syms(code).iter().any(|&sym| sym == ISO_LEVEL3_SHIFT || sym == MODE_SWITCH);
            if codes.iter().any(altgr) {
                keymap.level3 |= 1 << bit;
            }
        }
        Ok(keymap)
    }

    pub fn keysyms_per_keycode(&self) -> u8 {
        self.per
    }

    fn keycodes(&self) -> impl Iterator<Item = u8> + '_ {
        let count = self.syms.len() / usize::from(self.per.max(1));
        (0..count).filter_map(|i| u8::try_from(usize::from(self.min) + i).ok())
    }

    /// Keysyms eines Keycodes (leer, wenn es ihn nicht gibt)
    pub fn syms(&self, keycode: u8) -> &[u32] {
        let per = usize::from(self.per);
        let Some(i) = keycode.checked_sub(self.min).map(usize::from) else { return &[] };
        self.syms.get(i * per..(i + 1) * per).unwrap_or(&[])
    }

    /// Was die Taste mit dem Zustand `state` eines KeyPress ergibt
    pub fn chars(&self, keycode: u8, state: u16) -> KeyChars {
        x11_key_chars(self.syms(keycode), state, self.level3)
    }

    /// Ein Keycode, der in Gruppe `group` ohne Zusatztasten `ch` ergibt
    pub fn find(&self, ch: char, group: u8) -> Option<u8> {
        self.keycodes().find(|&code| {
            let syms = self.syms(code);
            let sym = syms.get(x11_column(group, 0)).copied().filter(|&sym| sym != 0).or(syms.first().copied());
            sym.and_then(char_from_keysym) == Some(ch)
        })
    }

    /// Keycodes ohne jede Belegung, für vorübergehende Zuordnungen beim Einspeisen
    pub fn unused(&self) -> Vec<u8> {
        self.keycodes().filter(|&code| self.syms(code).iter().all(|&sym| sym == 0)).collect()
    }

    /// Eine Zuordnung, die Qross selbst gesetzt hat, hier nachtragen (das `MappingNotify` dazu
    /// kommt erst später)
    pub fn set(&mut self, keycode: u8, keysym: u32) {
        let per = usize::from(self.per);
        if let Some(i) = keycode.checked_sub(self.min).map(usize::from) {
            if let Some(slots) = self.syms.get_mut(i * per..(i + 1) * per) {
                slots.fill(keysym);
            }
        }
    }
}
