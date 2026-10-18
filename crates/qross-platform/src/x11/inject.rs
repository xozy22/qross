//! Eingaben über die XTest-Erweiterung einspeisen.
//!
//! Zeichen (Zeichenmodus) gehen über eine Taste, die sie in der aktuellen Belegung schon ergibt.
//! Gibt es keine, oder ist gerade eine Zusatztaste gedrückt, belegt Qross einen freien Keycode
//! vorübergehend mit dem Zeichen (wie `xdotool type`). Die zuletzt benutzten Zeichen bleiben dort
//! liegen, damit nicht jeder Tastendruck die Belegung ändert; beim Beenden räumt Qross sie ab.

use anyhow::Result;
use qross_core::Inject;
use qross_proto::{KeyCode, MouseButton};
use x11rb::connection::Connection;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ConnectionExt as _, Window, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
    MOTION_NOTIFY_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use super::keymap::Keymap;
use crate::keymap_linux::{evdev_from_hid, keysym_from_char, X11_OFFSET};
use crate::Injector;

/// 120 = eine Raste (wie Windows); X11 kennt nur ganze Rasten.
const NOTCH: i32 = 120;
/// Höchstens so viele freie Keycodes belegt Qross gleichzeitig mit Zeichen
const SCRATCH: usize = 8;

pub struct X11Injector {
    conn: RustConnection,
    root: Window,
    /// Noch nicht ausgegebene Teil-Rasten (hochauflösende Räder)
    wheel_rest: (i32, i32),
    /// Belegung des X-Servers; `None`: neu lesen (nach `MappingNotify`)
    keymap: Option<Keymap>,
    xkb: bool,
    /// Vorübergehend mit einem Zeichen belegte Keycodes, zuletzt benutzte hinten
    scratch: Vec<(u8, char)>,
    /// Freie Keycodes, die noch nicht in `scratch` stehen
    free: Option<Vec<u8>>,
    /// Gedrückte Zusatztasten (Bits wie HID 0xE0–0xE7)
    modifiers: u8,
    /// Als Zeichen bzw. Kürzel gedrückte Keycodes, zum Loslassen
    typed: Vec<(char, u8)>,
}

impl X11Injector {
    pub fn new() -> Result<Self> {
        let (conn, root) = super::connect()?;
        // Für die aktive Gruppe (z. B. „us,de“) beim Suchen der Taste eines Zeichens
        let xkb = conn.xkb_use_extension(1, 0).ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.supported);
        Ok(Self {
            conn,
            root,
            wheel_rest: (0, 0),
            keymap: None,
            xkb,
            scratch: Vec::new(),
            free: None,
            modifiers: 0,
            typed: Vec::new(),
        })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
        if let Err(e) = self.conn.xtest_fake_input(kind, detail, CURRENT_TIME, self.root, x, y, 0) {
            tracing::warn!("XTest fehlgeschlagen: {e}");
        }
    }

    fn click(&self, button: u8) {
        self.fake(BUTTON_PRESS_EVENT, button, 0, 0);
        self.fake(BUTTON_RELEASE_EVENT, button, 0, 0);
    }

    fn key(&mut self, key: KeyCode, down: bool) {
        match evdev_from_hid(key) {
            Some(code) => {
                if key.is_modifier() {
                    let bit = 1 << (key.0 - 0xE0);
                    self.modifiers = if down { self.modifiers | bit } else { self.modifiers & !bit };
                }
                let keycode = (code + X11_OFFSET).min(255) as u8;
                self.fake(if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, keycode, 0, 0);
            }
            None => tracing::debug!(?key, "Taste ohne X11-Zuordnung"),
        }
    }

    /// Aktuelle Belegung; nach einem `MappingNotify` (auch dem eigenen) neu gelesen
    fn keymap(&mut self) -> Option<&mut Keymap> {
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if let Event::MappingNotify(_) = event {
                self.keymap = None;
            }
        }
        if self.keymap.is_none() {
            self.keymap = Keymap::fetch(&self.conn).map_err(|e| tracing::warn!("Tastaturbelegung nicht lesbar: {e:#}")).ok();
        }
        self.keymap.as_mut()
    }

    fn group(&self) -> u8 {
        if !self.xkb {
            return 0;
        }
        let state = self.conn.xkb_get_state(xkb::DeviceSpec::from(xkb::ID::USE_CORE_KBD)).ok().and_then(|c| c.reply().ok());
        state.map_or(0, |s| u8::from(s.group))
    }

    /// Keycode für `ch`: eine Taste, die es ohne Zusatztasten schon ergibt (`existing`), sonst ein
    /// vorübergehend belegter freier Keycode
    fn keycode_for(&mut self, ch: char, existing: bool) -> Option<u8> {
        let group = self.group();
        let found = self.keymap()?.find(ch, group);
        if let Some(code) = found.filter(|_| existing) {
            return Some(code);
        }
        if let Some(i) = self.scratch.iter().position(|&(_, c)| c == ch) {
            let slot = self.scratch.remove(i);
            self.scratch.push(slot);
            return Some(slot.0);
        }
        if self.free.is_none() {
            self.free = self.keymap.as_ref().map(Keymap::unused);
        }
        let free = if self.scratch.len() < SCRATCH { self.free.as_mut().and_then(Vec::pop) } else { None };
        let code = match free {
            Some(code) => code,
            None => {
                // Den am längsten unbenutzten Keycode neu belegen, sofern er gerade nicht gedrückt ist
                let typed = &self.typed;
                let i = self.scratch.iter().position(|(code, _)| !typed.iter().any(|(_, c)| c == code))?;
                self.scratch.remove(i).0
            }
        };
        let keysym = keysym_from_char(ch);
        let keymap = self.keymap.as_mut()?;
        let per = keymap.keysyms_per_keycode();
        let syms = vec![keysym; usize::from(per)];
        if let Err(e) = self.conn.change_keyboard_mapping(1, code, per, &syms) {
            tracing::warn!("Keycode {code} nicht belegbar: {e}");
            return None;
        }
        keymap.set(code, keysym);
        self.scratch.push((code, ch));
        Some(code)
    }

    /// Zeichen bzw. Kürzel drücken oder loslassen; `fallback`: die Taste, falls es keinen Keycode gibt
    fn type_char(&mut self, ch: char, down: bool, existing: bool, fallback: Option<KeyCode>) {
        if !down {
            if let Some(i) = self.typed.iter().position(|&(c, _)| c == ch) {
                let (_, code) = self.typed.remove(i);
                self.fake(KEY_RELEASE_EVENT, code, 0, 0);
            } else if let Some(key) = fallback {
                self.key(key, false);
            }
            return;
        }
        match self.keycode_for(ch, existing) {
            Some(code) => {
                self.fake(KEY_PRESS_EVENT, code, 0, 0);
                if !self.typed.iter().any(|&(c, _)| c == ch) {
                    self.typed.push((ch, code));
                }
            }
            None => match fallback {
                Some(key) => self.key(key, true),
                None => tracing::debug!(?ch, "Zeichen ohne freien Keycode"),
            },
        }
    }
}

impl Drop for X11Injector {
    /// Vorübergehende Belegungen wieder freigeben
    fn drop(&mut self) {
        let per = self.keymap.as_ref().map_or(1, Keymap::keysyms_per_keycode);
        for (code, _) in self.scratch.drain(..) {
            let _ = self.conn.change_keyboard_mapping(1, code, per, &vec![NONE; usize::from(per)]);
        }
        let _ = self.conn.flush();
    }
}

impl Injector for X11Injector {
    fn types_text(&self) -> bool {
        true
    }

    fn inject(&mut self, event: &Inject) {
        match *event {
            Inject::MoveTo { x, y } => {
                let clamp = |v: i32| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
                // detail 0 = absolute Position
                self.fake(MOTION_NOTIFY_EVENT, 0, clamp(x), clamp(y));
            }
            Inject::MoveBy { dx, dy } => {
                // detail 1 = relativ, wie XTestFakeRelativeMotionEvent (dort ohne Root-Fenster)
                let clamp = |v: i32| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
                if let Err(e) = self.conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 1, CURRENT_TIME, NONE, clamp(dx), clamp(dy), 0) {
                    tracing::warn!("XTest fehlgeschlagen: {e}");
                }
            }
            Inject::Button { button, down } => {
                let detail = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                    MouseButton::Back => 8,
                    MouseButton::Forward => 9,
                };
                self.fake(if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, detail, 0, 0);
            }
            Inject::Wheel { dx, dy } => {
                self.wheel_rest.0 += i32::from(dx);
                self.wheel_rest.1 += i32::from(dy);
                // X11: 4 = hoch, 5 = runter, 6 = links, 7 = rechts
                for (rest, positive, negative) in [(1, 4u8, 5u8), (0, 7, 6)] {
                    loop {
                        let value = if rest == 1 { &mut self.wheel_rest.1 } else { &mut self.wheel_rest.0 };
                        let button = if *value >= NOTCH {
                            *value -= NOTCH;
                            positive
                        } else if *value <= -NOTCH {
                            *value += NOTCH;
                            negative
                        } else {
                            break;
                        };
                        self.click(button);
                    }
                }
            }
            Inject::Key { key, down } => self.key(key, down),
            // Mit gedrückter Zusatztaste ergäbe eine vorhandene Taste womöglich etwas anderes
            Inject::Char { ch, down } => self.type_char(ch, down, self.modifiers == 0, None),
            // Kürzel prüfen Programme am Keysym: Strg+z braucht eine Taste, die z ergibt
            Inject::Shortcut { ch, key, down } => self.type_char(ch, down, true, Some(key)),
        }
        let _ = self.conn.flush();
    }
}
