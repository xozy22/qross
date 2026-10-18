//! Feststelltaste, Num und Rollen unter Windows: Stand lesen (`GetKeyState`, Bit 0) und umschalten
//! (Taste per `SendInput` drücken und loslassen). `GetKeyState` liefert den neuen Stand auch in
//! Threads ohne Nachrichtenschleife gleich nach dem `SendInput`; das Einspeisen kann deshalb vor
//! jedem Umschalten nachsehen und schaltet nie doppelt.

use std::mem::size_of;

use qross_proto::{Lock, LockState};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    VK_CAPITAL, VK_NUMLOCK, VK_SCROLL,
};

use super::INJECT_MAGIC;

/// Virtual-Key, Scancode und ob er „extended“ ist (Num wie im Beispiel zu `keybd_event`)
const fn key(lock: Lock) -> (u16, u16, bool) {
    match lock {
        Lock::Caps => (VK_CAPITAL, 0x3A, false),
        Lock::Num => (VK_NUMLOCK, 0x45, true),
        Lock::Scroll => (VK_SCROLL, 0x46, false),
    }
}

/// Welchen Zustand der Virtual-Key `vk` umschaltet
pub(crate) fn lock_of_vk(vk: u32) -> Option<Lock> {
    Lock::ALL.into_iter().find(|&lock| u32::from(key(lock).0) == vk)
}

pub(crate) fn is_on(lock: Lock) -> bool {
    let state = unsafe { GetKeyState(i32::from(key(lock).0)) };
    state & 1 != 0
}

/// Alle drei; Windows kennt sie auch ohne die Tasten (z. B. Rollen am Laptop)
pub fn current() -> LockState {
    let mut state = LockState::default();
    for lock in Lock::ALL {
        state.set(lock, Some(is_on(lock)));
    }
    state
}

/// Auf `target` bringen: nur umschalten, was abweicht
pub fn set(target: LockState) {
    for lock in Lock::ALL {
        if target.get(lock).is_some_and(|want| want != is_on(lock)) {
            tap(lock);
        }
    }
}

/// Drücken und loslassen, markiert für den eigenen Hook
fn tap(lock: Lock) {
    let (vk, scan, extended) = key(lock);
    let flags = if extended { KEYEVENTF_EXTENDEDKEY } else { 0 };
    let event = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: INJECT_MAGIC,
            },
        },
    };
    let inputs = [event(false), event(true)];
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        tracing::warn!("{} nicht umgeschaltet: SendInput hat {sent} von 2 Ereignissen angenommen", lock.label());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_keys_name_their_lock() {
        assert_eq!(lock_of_vk(0x14), Some(Lock::Caps));
        assert_eq!(lock_of_vk(0x90), Some(Lock::Num));
        assert_eq!(lock_of_vk(0x91), Some(Lock::Scroll));
        assert_eq!(lock_of_vk(0x41), None);
        assert!(current().is_known());
    }

    /// Echt umschalten und wieder zurück: Feststelltaste und Num (Rollen ist oft die Fang-Taste
    /// eines laufenden Synergy oder Qross). Von Hand: `cargo test -p qross-platform -- --ignored locks`
    #[test]
    #[ignore = "schaltet Feststelltaste und Num dieses Rechners kurz um (und wieder zurück)"]
    fn locks_are_set_and_restored() {
        let before = current();
        let flipped = LockState { caps: before.caps.map(|on| !on), num: before.num.map(|on| !on), scroll: None };
        set(flipped);
        let during = current();
        // Erst zurückstellen, dann prüfen: Der Rechner soll danach wieder so stehen wie vorher.
        set(before);
        let after = current();
        assert_eq!(after, before, "nicht wiederhergestellt");
        assert_ne!(
            during, before,
            "nichts umgeschaltet: Bildschirm gesperrt, Sandbox ohne Zugriff auf die Eingabe oder ein Fenster mit \
             Administratorrechten im Vordergrund"
        );
        assert_eq!((during.caps, during.num), (flipped.caps, flipped.num), "nicht umgeschaltet: {during:?}");
        assert_eq!(during.scroll, before.scroll, "Rollen bleibt, wie es war");
        // Schon richtig: nichts zu tun
        set(before);
        assert_eq!(current(), before);
    }
}
