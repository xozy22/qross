//! Tasten-Einstellungen: Namen, Tastenkombinationen (Fang-Taste, Wechsel-Kürzel), Medientasten,
//! Umbelegung und Tastenposition oder Zeichen je Rechner.
//!
//! Tasten heißen wie `KeyboardEvent.code` im Browser („KeyA“, „ScrollLock“, „AltLeft“). So kann
//! die Oberfläche eine gedrückte Taste direkt übernehmen, und `qross.toml` bleibt lesbar.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use qross_proto::{Edge, KeyCode};
use serde::{Deserialize, Serialize};

use crate::input::CaptureEvent;
use crate::lang::Lang;

/// Eine Taste in [`NAMES`]: HID-Code, Name, deutsche und englische Beschriftung
type KeyEntry = (u16, &'static str, &'static str, &'static str);

/// Name (`KeyboardEvent.code`), deutsche und englische Beschriftung je Taste
#[rustfmt::skip]
const NAMES: &[KeyEntry] = &[
    (0x04, "KeyA", "A", "A"), (0x05, "KeyB", "B", "B"), (0x06, "KeyC", "C", "C"), (0x07, "KeyD", "D", "D"),
    (0x08, "KeyE", "E", "E"), (0x09, "KeyF", "F", "F"), (0x0A, "KeyG", "G", "G"), (0x0B, "KeyH", "H", "H"),
    (0x0C, "KeyI", "I", "I"), (0x0D, "KeyJ", "J", "J"), (0x0E, "KeyK", "K", "K"), (0x0F, "KeyL", "L", "L"),
    (0x10, "KeyM", "M", "M"), (0x11, "KeyN", "N", "N"), (0x12, "KeyO", "O", "O"), (0x13, "KeyP", "P", "P"),
    (0x14, "KeyQ", "Q", "Q"), (0x15, "KeyR", "R", "R"), (0x16, "KeyS", "S", "S"), (0x17, "KeyT", "T", "T"),
    (0x18, "KeyU", "U", "U"), (0x19, "KeyV", "V", "V"), (0x1A, "KeyW", "W", "W"), (0x1B, "KeyX", "X", "X"),
    (0x1C, "KeyY", "Z (DE) / Y", "Y / Z (DE)"), (0x1D, "KeyZ", "Y (DE) / Z", "Z / Y (DE)"),
    (0x1E, "Digit1", "1", "1"), (0x1F, "Digit2", "2", "2"), (0x20, "Digit3", "3", "3"), (0x21, "Digit4", "4", "4"),
    (0x22, "Digit5", "5", "5"), (0x23, "Digit6", "6", "6"), (0x24, "Digit7", "7", "7"), (0x25, "Digit8", "8", "8"),
    (0x26, "Digit9", "9", "9"), (0x27, "Digit0", "0", "0"),
    (0x28, "Enter", "Eingabe", "Enter"), (0x29, "Escape", "Esc", "Esc"), (0x2A, "Backspace", "Rücktaste", "Backspace"),
    (0x2B, "Tab", "Tab", "Tab"), (0x2C, "Space", "Leertaste", "Space"),
    (0x2D, "Minus", "ß (DE) / -", "- / ß (DE)"), (0x2E, "Equal", "´ (DE) / =", "= / ´ (DE)"),
    (0x2F, "BracketLeft", "Ü (DE) / [", "[ / Ü (DE)"), (0x30, "BracketRight", "+ (DE) / ]", "] / + (DE)"),
    (0x31, "Backslash", "\\", "\\"), (0x32, "IntlHash", "# (DE)", "# (DE)"),
    (0x33, "Semicolon", "Ö (DE) / ;", "; / Ö (DE)"), (0x34, "Quote", "Ä (DE) / '", "' / Ä (DE)"),
    (0x35, "Backquote", "^ (DE) / `", "` / ^ (DE)"), (0x36, "Comma", ",", ","), (0x37, "Period", ".", "."),
    (0x38, "Slash", "- (DE) / /", "/ / - (DE)"),
    (0x39, "CapsLock", "Feststelltaste", "Caps Lock"),
    (0x3A, "F1", "F1", "F1"), (0x3B, "F2", "F2", "F2"), (0x3C, "F3", "F3", "F3"), (0x3D, "F4", "F4", "F4"),
    (0x3E, "F5", "F5", "F5"), (0x3F, "F6", "F6", "F6"), (0x40, "F7", "F7", "F7"), (0x41, "F8", "F8", "F8"),
    (0x42, "F9", "F9", "F9"), (0x43, "F10", "F10", "F10"), (0x44, "F11", "F11", "F11"), (0x45, "F12", "F12", "F12"),
    (0x46, "PrintScreen", "Druck", "Print Screen"), (0x47, "ScrollLock", "Rollen", "Scroll Lock"),
    (0x48, "Pause", "Pause", "Pause"),
    (0x49, "Insert", "Einfg", "Insert"), (0x4A, "Home", "Pos1", "Home"), (0x4B, "PageUp", "Bild auf", "Page Up"),
    (0x4C, "Delete", "Entf", "Delete"), (0x4D, "End", "Ende", "End"), (0x4E, "PageDown", "Bild ab", "Page Down"),
    (0x4F, "ArrowRight", "Pfeil rechts", "Right Arrow"), (0x50, "ArrowLeft", "Pfeil links", "Left Arrow"),
    (0x51, "ArrowDown", "Pfeil runter", "Down Arrow"), (0x52, "ArrowUp", "Pfeil hoch", "Up Arrow"),
    (0x53, "NumLock", "Num", "Num Lock"), (0x54, "NumpadDivide", "Num /", "Num /"),
    (0x55, "NumpadMultiply", "Num *", "Num *"), (0x56, "NumpadSubtract", "Num -", "Num -"),
    (0x57, "NumpadAdd", "Num +", "Num +"), (0x58, "NumpadEnter", "Num Eingabe", "Num Enter"),
    (0x59, "Numpad1", "Num 1", "Num 1"), (0x5A, "Numpad2", "Num 2", "Num 2"), (0x5B, "Numpad3", "Num 3", "Num 3"),
    (0x5C, "Numpad4", "Num 4", "Num 4"), (0x5D, "Numpad5", "Num 5", "Num 5"), (0x5E, "Numpad6", "Num 6", "Num 6"),
    (0x5F, "Numpad7", "Num 7", "Num 7"), (0x60, "Numpad8", "Num 8", "Num 8"), (0x61, "Numpad9", "Num 9", "Num 9"),
    (0x62, "Numpad0", "Num 0", "Num 0"), (0x63, "NumpadDecimal", "Num ,", "Num ."),
    (0x64, "IntlBackslash", "< (DE)", "< (DE)"), (0x65, "ContextMenu", "Kontextmenü", "Context Menu"),
    (0x67, "NumpadEqual", "Num =", "Num ="),
    (0x68, "F13", "F13", "F13"), (0x69, "F14", "F14", "F14"), (0x6A, "F15", "F15", "F15"), (0x6B, "F16", "F16", "F16"),
    (0x6C, "F17", "F17", "F17"), (0x6D, "F18", "F18", "F18"), (0x6E, "F19", "F19", "F19"), (0x6F, "F20", "F20", "F20"),
    (0x70, "F21", "F21", "F21"), (0x71, "F22", "F22", "F22"), (0x72, "F23", "F23", "F23"), (0x73, "F24", "F24", "F24"),
    (0x7F, "AudioVolumeMute", "Stumm", "Mute"), (0x80, "AudioVolumeUp", "Lauter", "Volume Up"),
    (0x81, "AudioVolumeDown", "Leiser", "Volume Down"),
    (0xE0, "ControlLeft", "Strg links", "Left Ctrl"), (0xE1, "ShiftLeft", "Umschalt links", "Left Shift"),
    (0xE2, "AltLeft", "Alt links", "Left Alt"), (0xE3, "MetaLeft", "Windows/Cmd links", "Left Windows/Cmd"),
    (0xE4, "ControlRight", "Strg rechts", "Right Ctrl"), (0xE5, "ShiftRight", "Umschalt rechts", "Right Shift"),
    (0xE6, "AltRight", "Alt Gr / Alt rechts", "Right Alt / Alt Gr"),
    (0xE7, "MetaRight", "Windows/Cmd rechts", "Right Windows/Cmd"),
    (KeyCode::MEDIA_PLAY_PAUSE.0, "MediaPlayPause", "Wiedergabe/Pause", "Play/Pause"),
    (KeyCode::MEDIA_STOP.0, "MediaStop", "Stopp", "Stop"),
    (KeyCode::MEDIA_NEXT.0, "MediaTrackNext", "Nächster Titel", "Next Track"),
    (KeyCode::MEDIA_PREVIOUS.0, "MediaTrackPrevious", "Vorheriger Titel", "Previous Track"),
];

fn entry(key: KeyCode) -> Option<&'static KeyEntry> {
    NAMES.iter().find(|(code, ..)| *code == key.0)
}

/// Beschriftung eines Eintrags in `lang`
fn entry_label(&(_, _, de, en): &KeyEntry, lang: Lang) -> &'static str {
    match lang {
        Lang::De => de,
        Lang::En => en,
    }
}

/// Name nach `KeyboardEvent.code`, z. B. „ScrollLock“
pub fn key_name(key: KeyCode) -> Option<&'static str> {
    entry(key).map(|&(_, name, ..)| name)
}

pub fn key_from_name(name: &str) -> Option<KeyCode> {
    NAMES.iter().find(|(_, n, ..)| n.eq_ignore_ascii_case(name)).map(|&(code, ..)| KeyCode(code))
}

/// Englische Beschriftung fürs Log, z. B. „Scroll Lock“ oder „Left Ctrl“
pub fn key_label(key: KeyCode) -> String {
    key_label_in(key, Lang::En)
}

/// Beschriftung für die Oberfläche, z. B. „Rollen“ oder „Strg links“ (englisch „Scroll Lock“,
/// „Left Ctrl“)
pub fn key_label_in(key: KeyCode, lang: Lang) -> String {
    match (entry(key), lang) {
        (Some(entry), _) => entry_label(entry, lang).to_owned(),
        (None, Lang::De) => format!("Taste {:#06x}", key.0),
        (None, Lang::En) => format!("Key {:#06x}", key.0),
    }
}

/// Alle benennbaren Tasten: (Taste, Name, englische Beschriftung)
pub fn all_keys() -> impl Iterator<Item = (KeyCode, &'static str, &'static str)> {
    all_keys_in(Lang::En)
}

/// Alle benennbaren Tasten: (Taste, Name, Beschriftung in `lang`)
pub fn all_keys_in(lang: Lang) -> impl Iterator<Item = (KeyCode, &'static str, &'static str)> {
    NAMES.iter().map(move |entry| (KeyCode(entry.0), entry.1, entry_label(entry, lang)))
}

/// Gedrückte Zusatztasten einer Kombination (links und rechts gelten gleich)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Platz einer Zusatztaste (HID 0xE0–0xE7) in der Liste der gedrückten Zusatztasten
    fn slot(key: KeyCode) -> Option<usize> {
        (0xE0..=0xE7).contains(&key.0).then(|| (key.0 - 0xE0) as usize)
    }

    /// Aus den einzeln gedrückten Zusatztasten (Strg, Umschalt, Alt, Meta; links, dann rechts)
    fn from_keys(keys: [bool; 8]) -> Self {
        Self { ctrl: keys[0] || keys[4], shift: keys[1] || keys[5], alt: keys[2] || keys[6], meta: keys[3] || keys[7] }
    }
}

/// Tastenkombination, z. B. „ScrollLock“ oder „Ctrl+Alt+KeyL“
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Hotkey {
    pub mods: Mods,
    pub key: KeyCode,
}

impl Hotkey {
    pub const SCROLL_LOCK: Self = Self { mods: Mods { ctrl: false, shift: false, alt: false, meta: false }, key: KeyCode::SCROLL_LOCK };

    /// Auf dem eigenen Rechner (Cursor lokal) wird nur eine Kombination verschluckt. Eine einzelne
    /// Taste wie Rollen behält dort ihre normale Wirkung (z. B. die Leuchte).
    pub fn swallow_locally(&self) -> bool {
        !self.mods.is_empty()
    }

    /// Strg+Alt+Umschalt+Esc ist der Notausgang und kann kein Kürzel sein.
    pub fn is_emergency_exit(&self) -> bool {
        self.key == KeyCode(0x29) && self.mods.ctrl && self.mods.alt && self.mods.shift
    }

    /// Eine Zusatztaste allein, z. B. „Strg rechts“: Sie zählt erst beim Loslassen und nur, wenn
    /// dazwischen keine andere Taste kam. Sonst gehörte schon Strg+C dazu.
    pub fn is_tap(&self) -> bool {
        self.mods.is_empty() && Mods::slot(self.key).is_some()
    }

    /// Englisch fürs Log, z. B. „Ctrl+Alt+L“
    pub fn label(&self, mac: bool) -> String {
        self.label_in(mac, Lang::En)
    }

    /// Für die Oberfläche, z. B. „Strg+Alt+L“ (englisch „Ctrl+Alt+L“)
    pub fn label_in(&self, mac: bool, lang: Lang) -> String {
        let mut parts: Vec<String> = Vec::new();
        let names = match (lang, mac) {
            (Lang::De, false) => ["Strg", "Umschalt", "Alt", "Windows"],
            (Lang::De, true) => ["Ctrl", "Umschalt", "Option", "Cmd"],
            (Lang::En, false) => ["Ctrl", "Shift", "Alt", "Windows"],
            (Lang::En, true) => ["Ctrl", "Shift", "Option", "Cmd"],
        };
        for (on, name) in [self.mods.ctrl, self.mods.shift, self.mods.alt, self.mods.meta].into_iter().zip(names) {
            if on {
                parts.push(name.to_owned());
            }
        }
        parts.push(key_label_in(self.key, lang));
        parts.join("+")
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [(self.mods.ctrl, "Ctrl"), (self.mods.shift, "Shift"), (self.mods.alt, "Alt"), (self.mods.meta, "Meta")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match key_name(self.key) {
            Some(name) => f.write_str(name),
            None => write!(f, "{:#06x}", self.key.0),
        }
    }
}

impl FromStr for Hotkey {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let mut mods = Mods::default();
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let key = parts.pop().filter(|k| !k.is_empty()).ok_or_else(|| format!("'{text}': missing key"))?;
        for part in parts {
            let slot = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "strg" => &mut mods.ctrl,
                "shift" | "umschalt" => &mut mods.shift,
                "alt" | "option" => &mut mods.alt,
                "meta" | "cmd" | "win" | "super" => &mut mods.meta,
                _ => return Err(format!("'{part}' is not a modifier key (Ctrl, Shift, Alt, Meta)")),
            };
            *slot = true;
        }
        let key = key_from_name(key).ok_or_else(|| format!("unknown key '{key}'"))?;
        Ok(Self { mods, key })
    }
}

impl TryFrom<String> for Hotkey {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        text.parse()
    }
}

impl From<Hotkey> for String {
    fn from(hotkey: Hotkey) -> Self {
        hotkey.to_string()
    }
}

/// Wohin ein Wechsel-Kürzel führt
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SwitchTarget {
    /// Zu diesem Rechner (Name im Layout); der Cursor landet in der Mitte
    Screen(String),
    /// Zum Nachbarn in dieser Richtung; der Cursor landet an der gegenüberliegenden Kante
    Direction(Edge),
}

/// Richtungen, wie sie in `qross.toml` und der Oberfläche heißen
const DIRECTIONS: [(Edge, &str, &str); 4] =
    [(Edge::Left, "left", "links"), (Edge::Right, "right", "rechts"), (Edge::Top, "up", "oben"), (Edge::Bottom, "down", "unten")];

fn direction(edge: Edge) -> (&'static str, &'static str) {
    DIRECTIONS.iter().find(|(e, ..)| *e == edge).map(|&(_, name, label)| (name, label)).expect("every direction is listed")
}

impl SwitchTarget {
    /// Englisch fürs Log, z. B. „left“ oder „to 'laptop'“
    pub fn label(&self) -> String {
        self.label_in(Lang::En)
    }

    /// Für die Oberfläche, z. B. „nach links“ oder „zu „laptop““ (englisch „left“, „to 'laptop'“)
    pub fn label_in(&self, lang: Lang) -> String {
        match (self, lang) {
            (Self::Screen(name), Lang::De) => format!("zu „{name}“"),
            (Self::Screen(name), Lang::En) => format!("to '{name}'"),
            (Self::Direction(edge), Lang::De) => format!("nach {}", direction(*edge).1),
            (Self::Direction(edge), Lang::En) => direction(*edge).0.to_owned(),
        }
    }
}

impl fmt::Display for SwitchTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Screen(name) => f.write_str(name),
            Self::Direction(edge) => f.write_str(direction(*edge).0),
        }
    }
}

impl FromStr for SwitchTarget {
    type Err = String;

    /// „left“, „right“, „up“, „down“ (auch „top“, „bottom“ wie bei den Links) sind Richtungen,
    /// alles andere ist der Name eines Rechners.
    fn from_str(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let edge = match text.to_ascii_lowercase().as_str() {
            "top" => Some(Edge::Top),
            "bottom" => Some(Edge::Bottom),
            word => DIRECTIONS.iter().find(|(_, name, _)| *name == word).map(|&(edge, ..)| edge),
        };
        match edge {
            Some(edge) => Ok(Self::Direction(edge)),
            None if text.is_empty() => Err("missing target (computer or left, right, up, down)".into()),
            None => Ok(Self::Screen(text.to_owned())),
        }
    }
}

/// Was die Erfassung mit einem Tastendruck tun soll
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    /// Kein Kürzel – normal behandeln
    None,
    /// Ein Kürzel wurde gedrückt: `event` melden
    Triggered { event: CaptureEvent, swallow_locally: bool },
    /// Wiederholung oder Loslassen der Taste eines ausgelösten Kürzels – ebenso behandeln wie das Auslösen
    Consumed { swallow_locally: bool },
    /// Eine Zusatztaste allein als Kürzel wurde getippt (siehe [`Hotkey::is_tap`]): `event` melden
    /// und das Loslassen selbst normal behandeln, wie schon das Drücken.
    Tapped { event: CaptureEvent },
}

impl HotkeyEvent {
    /// Soll die Erfassung die Taste verschlucken? `remote`: Der Cursor ist auf einem anderen Rechner.
    pub fn swallow(&self, remote: bool) -> bool {
        match self {
            Self::None | Self::Tapped { .. } => false,
            Self::Triggered { swallow_locally, .. } | Self::Consumed { swallow_locally } => remote || *swallow_locally,
        }
    }
}

/// Erkennt die Kürzel aus [`KeyOptions`] im Strom der Tastenereignisse (je Plattform im Hook benutzt).
#[derive(Debug, Default)]
pub struct HotkeyTracker {
    /// Gedrückte Zusatztasten einzeln, damit links und rechts sauber zusammenspielen
    mod_keys: [bool; 8],
    /// Gedrückte Tasten, die zu einem Kürzel gehören, und – falls ihr Drücken ausgelöst hat –
    /// ob sie auch lokal verschluckt werden
    held: Vec<(KeyCode, Option<bool>)>,
    /// Allein gedrückte Zusatztaste, die beim Loslassen ein Kürzel auslöst, wenn bis dahin keine
    /// andere Taste kommt
    tap: Option<KeyCode>,
}

impl HotkeyTracker {
    /// Für `const`-Initialisierung in Hook-Threads
    pub const fn new() -> Self {
        Self { mod_keys: [false; 8], held: Vec::new(), tap: None }
    }

    pub fn on_key(&mut self, key: KeyCode, down: bool, options: &KeyOptions) -> HotkeyEvent {
        let slot = Mods::slot(key);
        let alone = !self.mod_keys.iter().enumerate().any(|(i, &on)| on && Some(i) != slot);
        if let Some(slot) = slot {
            self.mod_keys[slot] = down;
        }
        // Zusatztaste allein als Kürzel: erst beim Loslassen, und jede andere Taste dazwischen
        // macht eine Kombination daraus (Strg+C bleibt Strg+C)
        if down {
            if self.tap != Some(key) {
                self.tap = (alone && options.tapped(key).is_some()).then_some(key);
            }
        } else if self.tap == Some(key) {
            self.tap = None;
            if let Some(event) = options.tapped(key) {
                self.held.retain(|(k, _)| *k != key);
                return HotkeyEvent::Tapped { event };
            }
        }
        let consumed = |fired: Option<bool>| {
            fired.map_or(HotkeyEvent::None, |swallow_locally| HotkeyEvent::Consumed { swallow_locally })
        };
        if !down {
            let Some(i) = self.held.iter().position(|(k, _)| *k == key) else { return HotkeyEvent::None };
            return consumed(self.held.swap_remove(i).1);
        }
        if let Some(&(_, fired)) = self.held.iter().find(|(k, _)| *k == key) {
            // Auto-Repeat
            return consumed(fired);
        }
        if !options.hotkeys().any(|h| h.key == key) {
            return HotkeyEvent::None;
        }
        // Ist die Taste selbst eine Zusatztaste (z. B. „Strg rechts“), zählt nur sie selbst nicht mit.
        let mut others = self.mod_keys;
        if let Some(slot) = slot {
            others[slot] = false;
        }
        let hit = options.find(key, Mods::from_keys(others));
        self.held.push((key, hit.as_ref().map(|&(_, swallow_locally)| swallow_locally)));
        match hit {
            Some((event, swallow_locally)) => HotkeyEvent::Triggered { event, swallow_locally },
            None => HotkeyEvent::None,
        }
    }

    /// Gerade gedrückte Zusatztasten übernehmen – für Erfassungen, die nicht jeden Tastendruck
    /// sehen (X11 im lokalen Modus).
    pub fn sync_modifiers(&mut self, mut pressed: impl FnMut(KeyCode) -> bool) {
        for (slot, down) in self.mod_keys.iter_mut().enumerate() {
            *down = pressed(KeyCode(0xE0 + slot as u16));
        }
    }

    /// Gedrückte Kürzel-Tasten vergessen, wenn die Erfassung ihr Loslassen nicht mehr sieht (X11
    /// nach dem Wechsel zurück auf den Server). So löst der nächste Druck sicher wieder aus.
    pub fn forget_keys(&mut self) {
        self.held.clear();
    }
}

/// Wohin Medientasten (Lautstärke, Wiedergabe) gehen
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKeys {
    /// An den Rechner, auf dem gerade der Cursor ist (wie alle anderen Tasten)
    #[default]
    Active,
    /// Immer auf dem Server, z. B. weil dort die Lautsprecher hängen
    Local,
}

/// Was die Eingabe-Erfassung des Servers über Tasten wissen muss
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyOptions {
    /// Hält den Cursor auf dem aktuellen Bildschirm fest (`None` = aus)
    pub lock: Option<Hotkey>,
    pub media: MediaKeys,
    /// Wechsel-Kürzel. Ist ein Kürzel mehrfach belegt, gilt die Fang-Taste, sonst der erste Eintrag.
    pub switch: Vec<(Hotkey, SwitchTarget)>,
}

impl Default for KeyOptions {
    fn default() -> Self {
        Self { lock: Some(Hotkey::SCROLL_LOCK), media: MediaKeys::Active, switch: Vec::new() }
    }
}

impl KeyOptions {
    /// Alle Kürzel, die Fang-Taste zuerst
    pub fn hotkeys(&self) -> impl Iterator<Item = &Hotkey> {
        self.lock.iter().chain(self.switch.iter().map(|(hotkey, _)| hotkey))
    }

    /// Kürzel mit dieser Taste und genau diesen Zusatztasten: das zu meldende Ereignis und ob die
    /// Taste auch lokal verschluckt wird. Wechsel-Kürzel erreichen nie ein Programm, denn ihr
    /// Loslassen geschieht schon auf dem anderen Rechner. Zusatztasten allein zählen hier nicht
    /// (siehe [`Self::tapped`]).
    fn find(&self, key: KeyCode, mods: Mods) -> Option<(CaptureEvent, bool)> {
        let hit = |h: &Hotkey| h.key == key && h.mods == mods && !h.is_emergency_exit() && !h.is_tap();
        if let Some(lock) = self.lock.as_ref().filter(|h| hit(h)) {
            return Some((CaptureEvent::ToggleLock, lock.swallow_locally()));
        }
        let (_, target) = self.switch.iter().find(|(h, _)| hit(h))?;
        Some((CaptureEvent::SwitchTo(target.clone()), true))
    }

    /// Kürzel aus dieser Zusatztaste allein: das zu meldende Ereignis, die Fang-Taste zuerst
    fn tapped(&self, key: KeyCode) -> Option<CaptureEvent> {
        let hit = |h: &Hotkey| h.is_tap() && h.key == key;
        if self.lock.as_ref().is_some_and(hit) {
            return Some(CaptureEvent::ToggleLock);
        }
        let (_, target) = self.switch.iter().find(|(h, _)| hit(h))?;
        Some(CaptureEvent::SwitchTo(target.clone()))
    }

    /// Doppelt belegte oder reservierte Kürzel, je eine Meldung, englisch für Log und Fehler
    pub fn conflicts(&self) -> Vec<String> {
        self.conflicts_in(Lang::En)
    }

    /// Doppelt belegte oder reservierte Kürzel, je eine Meldung in `lang`
    pub fn conflicts_in(&self, lang: Lang) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(lock) = self.lock.filter(Hotkey::is_emergency_exit) {
            out.push(match lang {
                Lang::De => format!("Fang-Taste „{lock}“ ist der Notausgang"),
                Lang::En => format!("lock key '{lock}' is the emergency exit"),
            });
        }
        for (i, (hotkey, target)) in self.switch.iter().enumerate() {
            let what = match lang {
                Lang::De => format!("Kürzel „{hotkey}“ ({})", target.label_in(lang)),
                Lang::En => format!("shortcut '{hotkey}' ({})", target.label_in(lang)),
            };
            if hotkey.is_emergency_exit() {
                out.push(match lang {
                    Lang::De => format!("{what} ist der Notausgang"),
                    Lang::En => format!("{what} is the emergency exit"),
                });
            } else if self.lock == Some(*hotkey) {
                out.push(match lang {
                    Lang::De => format!("{what} ist schon die Fang-Taste"),
                    Lang::En => format!("{what} is already the lock key"),
                });
            } else if let Some((_, first)) = self.switch[..i].iter().find(|(h, _)| h == hotkey) {
                out.push(match lang {
                    Lang::De => format!("{what} ist schon belegt ({})", first.label_in(lang)),
                    Lang::En => format!("{what} is already taken ({})", first.label_in(lang)),
                });
            }
        }
        out
    }
}

/// Umbelegung für einen Rechner: gedrückte Taste → gesendete Taste
pub type KeyMap = HashMap<KeyCode, KeyCode>;

/// Was bei einem Rechner ankommt, wenn auf dem Server eine Taste gedrückt wird
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyMode {
    /// Die Taste an derselben Stelle; das Tastaturlayout des Rechners bestimmt das Zeichen
    /// (wie eine dort angeschlossene Tastatur, nötig für Spiele)
    #[default]
    Position,
    /// Das Zeichen, das die Taste auf dem Server ergibt, egal welches Layout der Rechner hat.
    /// Kürzel wie Strg+Z gehen nach dem Zeichen, Pfeile, F-Tasten und Co. bleiben Tasten.
    Chars,
}

impl KeyMode {
    /// Englisch fürs Log
    pub fn label(self) -> &'static str {
        self.label_in(Lang::En)
    }

    /// Für die Oberfläche: „Tastenposition“ oder „Zeichen“ (englisch „Key position“, „Characters“)
    pub fn label_in(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Self::Position, Lang::De) => "Tastenposition",
            (Self::Chars, Lang::De) => "Zeichen",
            (Self::Position, Lang::En) => "Key position",
            (Self::Chars, Lang::En) => "Characters",
        }
    }
}

/// Alle Tasten-Einstellungen des Servers
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyConfig {
    pub options: KeyOptions,
    /// Name des Rechners → Umbelegung
    pub remap: HashMap<String, KeyMap>,
    /// Name des Rechners → Tastenposition oder Zeichen (fehlt: Tastenposition)
    pub modes: HashMap<String, KeyMode>,
}

/// Liest eine Umbelegung wie in `qross.toml` (`AltLeft = "MetaLeft"`).
pub fn parse_key_map(entries: &HashMap<String, String>) -> Result<KeyMap, String> {
    entries
        .iter()
        .map(|(from, to)| {
            let key = |name: &str| key_from_name(name).ok_or_else(|| format!("unknown key '{name}'"));
            Ok((key(from)?, key(to)?))
        })
        .collect()
}

/// Liest Wechsel-Kürzel wie in `qross.toml` (`"Ctrl+Alt+ArrowLeft" = "left"`), nach Kürzel sortiert.
pub fn parse_switch_keys(entries: &HashMap<String, String>) -> Result<Vec<(Hotkey, SwitchTarget)>, String> {
    let mut keys = entries
        .iter()
        .map(|(hotkey, target)| {
            let hotkey = hotkey.parse::<Hotkey>()?;
            let target = target.parse::<SwitchTarget>().map_err(|e| format!("'{hotkey}': {e}"))?;
            Ok((hotkey, target))
        })
        .collect::<Result<Vec<_>, String>>()?;
    keys.sort_by_key(|(hotkey, _)| hotkey.to_string());
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: KeyCode = KeyCode(0xE0);
    const ALT: KeyCode = KeyCode(0xE2);
    const RCTRL: KeyCode = KeyCode(0xE4);
    const L: KeyCode = KeyCode(0x0F);

    #[test]
    fn names_are_unique_and_roundtrip() {
        let mut codes = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        for (key, name, _) in all_keys() {
            assert!(codes.insert(key), "doppelt: {key:?}");
            assert!(names.insert(name), "doppelt: {name}");
            assert_eq!(key_from_name(name), Some(key));
            assert_eq!(key_name(key), Some(name));
        }
        assert_eq!(key_from_name("scrolllock"), Some(KeyCode::SCROLL_LOCK), "Groß/klein egal");
        assert_eq!(key_label_in(KeyCode(0xE3), Lang::De), "Windows/Cmd links");
        assert_eq!(key_label_in(KeyCode(0x99), Lang::De), "Taste 0x0099");
        assert_eq!(key_label_in(KeyCode::SCROLL_LOCK, Lang::De), "Rollen");
        assert_eq!(key_label_in(KeyCode::MEDIA_NEXT, Lang::De), "Nächster Titel");
    }

    #[test]
    fn keys_are_labelled_in_both_languages() {
        let german: Vec<_> = all_keys_in(Lang::De).collect();
        let english: Vec<_> = all_keys_in(Lang::En).collect();
        assert_eq!(german.len(), english.len());
        for ((key, name, de), (_, _, en)) in german.iter().zip(&english) {
            assert!(!de.is_empty() && !en.is_empty(), "{name}");
            assert_eq!(key_label_in(*key, Lang::De), *de);
            assert_eq!(key_label_in(*key, Lang::En), *en);
        }
        assert!(all_keys().eq(all_keys_in(Lang::En)), "ohne Sprache: englisch");

        assert_eq!(key_label(KeyCode(0xE3)), "Left Windows/Cmd");
        assert_eq!(key_label(KeyCode(0x99)), "Key 0x0099");
        assert_eq!(key_label(KeyCode::SCROLL_LOCK), "Scroll Lock");
        assert_eq!(key_label_in(KeyCode::CAPS_LOCK, Lang::En), "Caps Lock");
        assert_eq!(key_label_in(KeyCode::CAPS_LOCK, Lang::De), "Feststelltaste");
        assert_eq!(key_label_in(KeyCode(0x2D), Lang::En), "- / ß (DE)");
        assert_eq!(key_label_in(KeyCode(0x2D), Lang::De), "ß (DE) / -");
        assert_eq!(key_label_in(KeyCode::MEDIA_NEXT, Lang::En), "Next Track");
    }

    #[test]
    fn hotkeys_parse_and_print() {
        let hotkey: Hotkey = "Ctrl+Alt+KeyL".parse().unwrap();
        assert_eq!(hotkey, Hotkey { mods: Mods { ctrl: true, alt: true, ..Mods::default() }, key: L });
        assert_eq!(hotkey.to_string(), "Ctrl+Alt+KeyL");
        assert_eq!(hotkey.label_in(false, Lang::De), "Strg+Alt+L");
        assert_eq!(hotkey.label_in(true, Lang::De), "Ctrl+Option+L");
        assert_eq!(hotkey.label(false), "Ctrl+Alt+L");
        assert_eq!(hotkey.label(true), "Ctrl+Option+L");
        assert_eq!("ScrollLock".parse::<Hotkey>().unwrap(), Hotkey::SCROLL_LOCK);
        let all: Hotkey = "strg + umschalt + F12".parse().unwrap();
        assert_eq!(all.to_string(), "Ctrl+Shift+F12");
        let all: Hotkey = "Ctrl+Shift+Alt+Meta+Delete".parse().unwrap();
        assert_eq!(all.label_in(false, Lang::De), "Strg+Umschalt+Alt+Windows+Entf");
        assert_eq!(all.label_in(true, Lang::De), "Ctrl+Umschalt+Option+Cmd+Entf");
        assert_eq!(all.label_in(false, Lang::En), "Ctrl+Shift+Alt+Windows+Delete");
        assert_eq!(all.label_in(true, Lang::En), "Ctrl+Shift+Option+Cmd+Delete");
        assert_eq!(all.label(true), all.label_in(true, Lang::En));
        assert_eq!("Ctrl+".parse::<Hotkey>().unwrap_err(), "'Ctrl+': missing key");
        assert_eq!("Hyper+KeyA".parse::<Hotkey>().unwrap_err(), "'Hyper' is not a modifier key (Ctrl, Shift, Alt, Meta)");
        assert_eq!("Ctrl+Nichts".parse::<Hotkey>().unwrap_err(), "unknown key 'Nichts'");
        assert!(!Hotkey::SCROLL_LOCK.swallow_locally() && hotkey.swallow_locally());
    }

    fn lock_only(lock: &str) -> KeyOptions {
        KeyOptions { lock: Some(lock.parse().unwrap()), ..KeyOptions::default() }
    }

    const CONSUMED: HotkeyEvent = HotkeyEvent::Consumed { swallow_locally: true };

    fn toggled(swallow_locally: bool) -> HotkeyEvent {
        HotkeyEvent::Triggered { event: CaptureEvent::ToggleLock, swallow_locally }
    }

    #[test]
    fn hotkey_fires_once_with_exact_modifiers_and_consumes_repeat_and_release() {
        let options = lock_only("Ctrl+Alt+KeyL");
        let mut t = HotkeyTracker::default();
        let mut press = |key, down| t.on_key(key, down, &options);

        assert_eq!(press(L, true), HotkeyEvent::None, "ohne Zusatztasten");
        assert_eq!(press(L, false), HotkeyEvent::None);

        press(RCTRL, true);
        press(ALT, true);
        assert_eq!(press(L, true), toggled(true), "rechtes Strg zählt wie linkes");
        assert_eq!(press(L, true), CONSUMED, "Auto-Repeat");
        assert_eq!(press(L, false), CONSUMED, "Loslassen");

        let shift = KeyCode(0xE1);
        press(shift, true);
        assert_eq!(press(L, true), HotkeyEvent::None, "zusätzliches Umschalt: keine Übereinstimmung");
        assert_eq!(press(L, true), HotkeyEvent::None, "auch nicht bei der Wiederholung");
        assert_eq!(press(L, false), HotkeyEvent::None);
    }

    #[test]
    fn a_modifier_alone_counts_when_tapped() {
        let options = lock_only("ControlRight");
        let tapped = HotkeyEvent::Tapped { event: CaptureEvent::ToggleLock };
        let mut t = HotkeyTracker::default();
        assert_eq!(t.on_key(RCTRL, true, &options), HotkeyEvent::None, "Drücken geht normal durch");
        assert_eq!(t.on_key(RCTRL, true, &options), HotkeyEvent::None, "Auto-Repeat ebenso");
        assert_eq!(t.on_key(RCTRL, false, &options), tapped, "erst das Loslassen löst aus");
        assert!(!tapped.swallow(true), "auch das Loslassen geht normal weiter, wohin das Drücken ging");

        // Zweimal hintereinander: jedes Tippen zählt
        t.on_key(RCTRL, true, &options);
        assert_eq!(t.on_key(RCTRL, false, &options), tapped);

        t.on_key(CTRL, true, &options);
        t.on_key(RCTRL, true, &options);
        assert_eq!(t.on_key(RCTRL, false, &options), HotkeyEvent::None, "mit linkem Strg ist es Strg+Strg");
        t.on_key(CTRL, false, &options);
        let off = KeyOptions { lock: None, ..KeyOptions::default() };
        assert_eq!(t.on_key(L, true, &off), HotkeyEvent::None);
    }

    #[test]
    fn ctrl_c_stays_ctrl_c_with_ctrl_alone_as_hotkey() {
        // So stand es beim Nutzer: Strg allein wechselte zum Mac, und Strg+C wechselte schon beim
        // Drücken von Strg, statt zu kopieren
        const C: KeyCode = KeyCode(0x06);
        let entries = HashMap::from([("ControlLeft".to_owned(), "macbook".to_owned())]);
        let options = KeyOptions { switch: parse_switch_keys(&entries).unwrap(), ..KeyOptions::default() };
        let mut t = HotkeyTracker::new();
        assert_eq!(t.on_key(CTRL, true, &options), HotkeyEvent::None);
        assert_eq!(t.on_key(C, true, &options), HotkeyEvent::None, "C geht an das Programm");
        assert_eq!(t.on_key(C, false, &options), HotkeyEvent::None);
        assert_eq!(t.on_key(CTRL, false, &options), HotkeyEvent::None, "kein Wechsel nach Strg+C");

        t.on_key(CTRL, true, &options);
        let switched = HotkeyEvent::Tapped { event: CaptureEvent::SwitchTo(SwitchTarget::Screen("macbook".into())) };
        assert_eq!(t.on_key(CTRL, false, &options), switched, "Strg allein getippt wechselt");
    }

    fn switching() -> KeyOptions {
        let entries = HashMap::from([
            ("Ctrl+Alt+ArrowLeft".to_owned(), "left".to_owned()),
            ("Ctrl+Alt+Digit1".to_owned(), "laptop".to_owned()),
            ("Ctrl+Shift+ArrowLeft".to_owned(), "Bottom".to_owned()),
        ]);
        KeyOptions { lock: Some("Ctrl+Alt+KeyL".parse().unwrap()), switch: parse_switch_keys(&entries).unwrap(), ..KeyOptions::default() }
    }

    fn switched(target: SwitchTarget) -> HotkeyEvent {
        HotkeyEvent::Triggered { event: CaptureEvent::SwitchTo(target), swallow_locally: true }
    }

    #[test]
    fn several_hotkeys_are_told_apart_by_key_and_modifiers() {
        const LEFT: KeyCode = KeyCode(0x50);
        const DIGIT1: KeyCode = KeyCode(0x1E);
        const SHIFT: KeyCode = KeyCode(0xE1);
        let options = switching();
        let mut t = HotkeyTracker::new();
        let mut press = |key, down| t.on_key(key, down, &options);

        press(CTRL, true);
        press(ALT, true);
        assert_eq!(press(LEFT, true), switched(SwitchTarget::Direction(Edge::Left)));
        assert_eq!(press(LEFT, true), CONSUMED, "Auto-Repeat wechselt nicht noch einmal");
        assert_eq!(press(DIGIT1, true), switched(SwitchTarget::Screen("laptop".into())), "zweites Kürzel, erstes noch gedrückt");
        assert_eq!(press(LEFT, false), CONSUMED);
        assert_eq!(press(DIGIT1, false), CONSUMED);
        assert_eq!(press(L, true), toggled(true), "die Fang-Taste wirkt weiter");
        press(L, false);
        press(ALT, false);
        press(SHIFT, true);
        assert_eq!(press(LEFT, true), switched(SwitchTarget::Direction(Edge::Bottom)), "gleiche Taste, andere Zusatztasten");
        press(LEFT, false);
        press(SHIFT, false);
        assert_eq!(press(LEFT, true), HotkeyEvent::None, "nur Strg: Pfeil links geht normal durch");
        assert!(!press(LEFT, false).swallow(true));
    }

    #[test]
    fn the_lock_key_wins_and_the_emergency_exit_stays_free() {
        let mut options = switching();
        options.switch.insert(0, (options.lock.unwrap(), SwitchTarget::Screen("tablet".into())));
        let emergency: Hotkey = "Ctrl+Alt+Shift+Escape".parse().unwrap();
        options.switch.push((emergency, SwitchTarget::Screen("pc".into())));
        let mut t = HotkeyTracker::new();
        for key in [CTRL, ALT] {
            t.on_key(key, true, &options);
        }
        assert_eq!(t.on_key(L, true, &options), toggled(true));
        t.on_key(KeyCode(0xE1), true, &options);
        assert_eq!(t.on_key(KeyCode(0x29), true, &options), HotkeyEvent::None, "Notausgang bleibt Notausgang");

        let conflicts = options.conflicts_in(Lang::De);
        assert_eq!(conflicts.len(), 2, "{conflicts:?}");
        assert!(conflicts[0].contains("„Ctrl+Alt+KeyL“ (zu „tablet“) ist schon die Fang-Taste"), "{conflicts:?}");
        assert!(conflicts[1].contains("Notausgang"), "{conflicts:?}");
        let english = options.conflicts();
        assert_eq!(english, options.conflicts_in(Lang::En));
        assert_eq!(english[0], "shortcut 'Ctrl+Alt+KeyL' (to 'tablet') is already the lock key");
        assert_eq!(english[1], "shortcut 'Ctrl+Shift+Alt+Escape' (to 'pc') is the emergency exit");
        options.switch.push(("Alt+Ctrl+Digit1".parse().unwrap(), SwitchTarget::Direction(Edge::Right)));
        let conflicts = options.conflicts_in(Lang::De);
        assert!(conflicts[2].contains("ist schon belegt (zu „laptop“)"), "{conflicts:?}");
        assert_eq!(options.conflicts()[2], "shortcut 'Ctrl+Alt+Digit1' (right) is already taken (to 'laptop')");
        assert!(switching().conflicts().is_empty());

        let exit = KeyOptions { lock: Some(emergency), ..KeyOptions::default() };
        assert_eq!(exit.conflicts_in(Lang::De), ["Fang-Taste „Ctrl+Shift+Alt+Escape“ ist der Notausgang"]);
        assert_eq!(exit.conflicts(), ["lock key 'Ctrl+Shift+Alt+Escape' is the emergency exit"]);
    }

    #[test]
    fn modifiers_can_be_synced_and_held_keys_forgotten() {
        const RIGHT: KeyCode = KeyCode(0x4F);
        let options = KeyOptions { switch: vec![("Ctrl+Alt+ArrowRight".parse().unwrap(), SwitchTarget::Direction(Edge::Right))], ..switching() };
        let mut t = HotkeyTracker::new();
        // Die Erfassung hat das Drücken von Strg und Alt nicht gesehen (X11 lokal)
        t.sync_modifiers(|key| key == RCTRL || key == ALT);
        assert_eq!(t.on_key(RIGHT, true, &options), switched(SwitchTarget::Direction(Edge::Right)));
        // Das Loslassen ging an ein anderes Programm: vergessen, sonst gälte der nächste Druck als Wiederholung
        t.forget_keys();
        assert_eq!(t.on_key(RIGHT, true, &options), switched(SwitchTarget::Direction(Edge::Right)));
        t.sync_modifiers(|_| false);
        t.forget_keys();
        assert_eq!(t.on_key(RIGHT, true, &options), HotkeyEvent::None);
    }

    #[test]
    fn switch_targets_parse_and_print() {
        let target = |text: &str| text.parse::<SwitchTarget>().unwrap();
        assert_eq!(target("left"), SwitchTarget::Direction(Edge::Left));
        assert_eq!(target(" UP "), SwitchTarget::Direction(Edge::Top));
        assert_eq!(target("top"), SwitchTarget::Direction(Edge::Top), "wie bei den Links");
        assert_eq!(target("down").to_string(), "down");
        assert_eq!(target(" laptop "), SwitchTarget::Screen("laptop".into()));
        assert_eq!(target("Mac Mini").to_string(), "Mac Mini");
        assert!("  ".parse::<SwitchTarget>().is_err());
        assert_eq!(target("right").label_in(Lang::De), "nach rechts");
        assert_eq!(target("up").label_in(Lang::De), "nach oben");
        assert_eq!(target("laptop").label_in(Lang::De), "zu „laptop“");
        assert_eq!(target("right").label(), "right");
        assert_eq!(target("bottom").label_in(Lang::En), "down");
        assert_eq!(target("laptop").label(), "to 'laptop'");

        let bad = HashMap::from([("Ctrl+Alt+Digit1".to_owned(), String::new())]);
        assert_eq!(parse_switch_keys(&bad).unwrap_err(), "'Ctrl+Alt+Digit1': missing target (computer or left, right, up, down)");
        let bad = HashMap::from([("Ctrl+Nichts".to_owned(), "laptop".to_owned())]);
        assert!(parse_switch_keys(&bad).unwrap_err().contains("Nichts"));
        let keys: Vec<String> = switching().switch.iter().map(|(h, t)| format!("{h}={t}")).collect();
        assert_eq!(keys, ["Ctrl+Alt+ArrowLeft=left", "Ctrl+Alt+Digit1=laptop", "Ctrl+Shift+ArrowLeft=down"], "sortiert");
    }

    #[test]
    fn key_maps_and_options_read_like_the_config() {
        let entries = HashMap::from([("AltLeft".to_owned(), "MetaLeft".to_owned()), ("CapsLock".into(), "ControlLeft".into())]);
        let map = parse_key_map(&entries).unwrap();
        assert_eq!(map.get(&ALT), Some(&KeyCode(0xE3)));
        assert_eq!(map.get(&KeyCode(0x39)), Some(&CTRL));
        let unknown = parse_key_map(&HashMap::from([("Gibtsnicht".to_owned(), "KeyA".to_owned())]));
        assert_eq!(unknown.unwrap_err(), "unknown key 'Gibtsnicht'");
        assert_eq!(KeyOptions::default().lock, Some(Hotkey::SCROLL_LOCK));
        #[derive(Deserialize)]
        struct Wrap {
            media: MediaKeys,
            lock: Hotkey,
        }
        let parsed: Wrap = toml_like("local", "Ctrl+KeyL");
        assert_eq!((parsed.media, parsed.lock.to_string()), (MediaKeys::Local, "Ctrl+KeyL".to_owned()));
    }

    #[test]
    fn key_modes_read_like_the_config() {
        use serde::de::value::Error;
        use serde::de::IntoDeserializer;
        let mode = |text: &str| KeyMode::deserialize(IntoDeserializer::<Error>::into_deserializer(text)).ok();
        assert_eq!(mode("chars"), Some(KeyMode::Chars));
        assert_eq!(mode("position"), Some(KeyMode::Position));
        assert_eq!(mode("zeichen"), None);
        assert_eq!(KeyMode::default(), KeyMode::Position, "wie bisher");
        assert_eq!(KeyMode::Chars.label_in(Lang::De), "Zeichen");
        assert_eq!(KeyMode::Position.label_in(Lang::De), "Tastenposition");
        assert_eq!(KeyMode::Chars.label(), "Characters");
        assert_eq!(KeyMode::Position.label_in(Lang::En), "Key position");
        assert!(KeyConfig::default().modes.is_empty());
    }

    /// Ohne toml-Abhängigkeit im Kern: dieselben serde-Pfade über den String-Deserializer
    fn toml_like<T: for<'de> Deserialize<'de>>(media: &str, lock: &str) -> T {
        use serde::de::value::{Error, MapDeserializer};
        let map = MapDeserializer::<_, Error>::new([("media", media), ("lock", lock)].into_iter());
        T::deserialize(map).unwrap()
    }
}
