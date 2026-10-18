//! Feststelltaste und Num unter X11 über XKB: Stand aus `XkbGetState` (eingerastete Modifier),
//! gesetzt mit `XkbLatchLockState` wie bei `numlockx`. Das gilt für alle Tastaturen am X-Server, und
//! die Leuchten folgen. Num liegt auf dem Modifier, dem das Keysym `Num_Lock` zugeordnet ist (meist
//! Mod2). Rollen schaltet unter X11 keinen Modifier und bleibt deshalb außen vor.
//!
//! Drückt man eine dieser Tasten, während Qross die Tastatur gegriffen hat (Cursor auf einem
//! Client), schaltet der X-Server trotzdem um: XKB wertet die Taste aus, bevor er sie zustellt.

use anyhow::Result;
use qross_proto::{Lock, LockState};
use x11rb::connection::Connection;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, ModMask};
use x11rb::rust_connection::RustConnection;

const NUM_LOCK: u32 = 0xff7f;

/// Modifier-Bits der Sperrtasten auf diesem X-Server. Setzt voraus, dass die Verbindung XKB
/// eingeschaltet hat (`xkb_use_extension`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LockMasks {
    /// Bits von Num; 0, wenn `Num_Lock` auf keinem Modifier liegt
    num: u16,
}

impl LockMasks {
    pub fn fetch(conn: &RustConnection) -> Result<Self> {
        let (min, max) = (conn.setup().min_keycode, conn.setup().max_keycode);
        let mapping = conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        let per = usize::from(mapping.keysyms_per_keycode).max(1);
        let is_num_lock = |code: u8| {
            let Some(i) = code.checked_sub(min).map(usize::from) else { return false };
            mapping.keysyms.get(i * per..(i + 1) * per).is_some_and(|syms| syms.contains(&NUM_LOCK))
        };
        let mods = conn.get_modifier_mapping()?.reply()?;
        let per_modifier = usize::from(mods.keycodes_per_modifier()).max(1);
        let mut num = 0;
        for (bit, codes) in mods.keycodes.chunks(per_modifier).enumerate().take(8) {
            if codes.iter().any(|&code| code != 0 && is_num_lock(code)) {
                num |= 1 << bit;
            }
        }
        Ok(Self { num })
    }

    fn mask(self, lock: Lock) -> u16 {
        match lock {
            Lock::Caps => u16::from(ModMask::LOCK),
            Lock::Num => self.num,
            Lock::Scroll => 0,
        }
    }

    /// Wie die Sperrtasten gerade stehen
    pub fn read(self, conn: &RustConnection) -> Result<LockState> {
        let state = conn.xkb_get_state(xkb::DeviceSpec::from(xkb::ID::USE_CORE_KBD))?.reply()?;
        let locked = u16::from(state.locked_mods);
        let mut locks = LockState::default();
        for lock in Lock::ALL {
            let mask = self.mask(lock);
            locks.set(lock, (mask != 0).then_some(locked & mask != 0));
        }
        Ok(locks)
    }

    /// Auf `target` bringen; was hier nicht geht (Rollen, Num ohne Modifier), bleibt
    pub fn set(self, conn: &RustConnection, target: LockState) -> Result<()> {
        let (mut affect, mut values) = (0u16, 0u16);
        for lock in Lock::ALL {
            let mask = self.mask(lock);
            if let (Some(on), true) = (target.get(lock), mask != 0) {
                affect |= mask;
                if on {
                    values |= mask;
                }
            }
        }
        if affect == 0 {
            return Ok(());
        }
        let none = ModMask::from(0u16);
        let device = xkb::DeviceSpec::from(xkb::ID::USE_CORE_KBD);
        conn.xkb_latch_lock_state(device, ModMask::from(affect), ModMask::from(values), false, xkb::Group::M1, none, false, 0)?;
        conn.flush()?;
        Ok(())
    }
}
