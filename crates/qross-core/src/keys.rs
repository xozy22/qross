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

/// Name (`KeyboardEvent.code`) und deutsche Beschriftung je Taste
#[rustfmt::skip]
const NAMES: &[(u16, &str, &str)] = &[
    (0x04, "KeyA", "A"), (0x05, "KeyB", "B"), (0x06, "KeyC", "C"), (0x07, "KeyD", "D"), (0x08, "KeyE", "E"),
    (0x09, "KeyF", "F"), (0x0A, "KeyG", "G"), (0x0B, "KeyH", "H"), (0x0C, "KeyI", "I"), (0x0D, "KeyJ", "J"),
    (0x0E, "KeyK", "K"), (0x0F, "KeyL", "L"), (0x10, "KeyM", "M"), (0x11, "KeyN", "N"), (0x12, "KeyO", "O"),
    (0x13, "KeyP", "P"), (0x14, "KeyQ", "Q"), (0x15, "KeyR", "R"), (0x16, "KeyS", "S"), (0x17, "KeyT", "T"),
    (0x18, "KeyU", "U"), (0x19, "KeyV", "V"), (0x1A, "KeyW", "W"), (0x1B, "KeyX", "X"), (0x1C, "KeyY", "Z (DE) / Y"),
    (0x1D, "KeyZ", "Y (DE) / Z"),
    (0x1E, "Digit1", "1"), (0x1F, "Digit2", "2"), (0x20, "Digit3", "3"), (0x21, "Digit4", "4"), (0x22, "Digit5", "5"),
    (0x23, "Digit6", "6"), (0x24, "Digit7", "7"), (0x25, "Digit8", "8"), (0x26, "Digit9", "9"), (0x27, "Digit0", "0"),
    (0x28, "Enter", "Eingabe"), (0x29, "Escape", "Esc"), (0x2A, "Backspace", "Rücktaste"), (0x2B, "Tab", "Tab"),
    (0x2C, "Space", "Leertaste"), (0x2D, "Minus", "ß (DE) / -"), (0x2E, "Equal", "´ (DE) / ="),
    (0x2F, "BracketLeft", "Ü (DE) / ["), (0x30, "BracketRight", "+ (DE) / ]"), (0x31, "Backslash", "\\"),
    (0x32, "IntlHash", "# (DE)"), (0x33, "Semicolon", "Ö (DE) / ;"), (0x34, "Quote", "Ä (DE) / '"),
    (0x35, "Backquote", "^ (DE) / `"), (0x36, "Comma", ","), (0x37, "Period", "."), (0x38, "Slash", "- (DE) / /"),
    (0x39, "CapsLock", "Feststelltaste"),
    (0x3A, "F1", "F1"), (0x3B, "F2", "F2"), (0x3C, "F3", "F3"), (0x3D, "F4", "F4"), (0x3E, "F5", "F5"),
    (0x3F, "F6", "F6"), (0x40, "F7", "F7"), (0x41, "F8", "F8"), (0x42, "F9", "F9"), (0x43, "F10", "F10"),
    (0x44, "F11", "F11"), (0x45, "F12", "F12"),
    (0x46, "PrintScreen", "Druck"), (0x47, "ScrollLock", "Rollen"), (0x48, "Pause", "Pause"),
    (0x49, "Insert", "Einfg"), (0x4A, "Home", "Pos1"), (0x4B, "PageUp", "Bild auf"), (0x4C, "Delete", "Entf"),
    (0x4D, "End", "Ende"), (0x4E, "PageDown", "Bild ab"), (0x4F, "ArrowRight", "Pfeil rechts"),
    (0x50, "ArrowLeft", "Pfeil links"), (0x51, "ArrowDown", "Pfeil runter"), (0x52, "ArrowUp", "Pfeil hoch"),
    (0x53, "NumLock", "Num"), (0x54, "NumpadDivide", "Num /"), (0x55, "NumpadMultiply", "Num *"),
    (0x56, "NumpadSubtract", "Num -"), (0x57, "NumpadAdd", "Num +"), (0x58, "NumpadEnter", "Num Eingabe"),
    (0x59, "Numpad1", "Num 1"), (0x5A, "Numpad2", "Num 2"), (0x5B, "Numpad3", "Num 3"), (0x5C, "Numpad4", "Num 4"),
    (0x5D, "Numpad5", "Num 5"), (0x5E, "Numpad6", "Num 6"), (0x5F, "Numpad7", "Num 7"), (0x60, "Numpad8", "Num 8"),
    (0x61, "Numpad9", "Num 9"), (0x62, "Numpad0", "Num 0"), (0x63, "NumpadDecimal", "Num ,"),
    (0x64, "IntlBackslash", "< (DE)"), (0x65, "ContextMenu", "Kontextmenü"), (0x67, "NumpadEqual", "Num ="),
    (0x68, "F13", "F13"), (0x69, "F14", "F14"), (0x6A, "F15", "F15"), (0x6B, "F16", "F16"), (0x6C, "F17", "F17"),
    (0x6D, "F18", "F18"), (0x6E, "F19", "F19"), (0x6F, "F20", "F20"), (0x70, "F21", "F21"), (0x71, "F22", "F22"),
    (0x72, "F23", "F23"), (0x73, "F24", "F24"),
    (0x7F, "AudioVolumeMute", "Stumm"), (0x80, "AudioVolumeUp", "Lauter"), (0x81, "AudioVolumeDown", "Leiser"),
    (0xE0, "ControlLeft", "Strg links"), (0xE1, "ShiftLeft", "Umschalt links"), (0xE2, "AltLeft", "Alt links"),
    (0xE3, "MetaLeft", "Windows/Cmd links"), (0xE4, "ControlRight", "Strg rechts"),
    (0xE5, "ShiftRight", "Umschalt rechts"), (0xE6, "AltRight", "Alt Gr / Alt rechts"),
    (0xE7, "MetaRight", "Windows/Cmd rechts"),
    (KeyCode::MEDIA_PLAY_PAUSE.0, "MediaPlayPause", "Wiedergabe/Pause"),
    (KeyCode::MEDIA_STOP.0, "MediaStop", "Stopp"),
    (KeyCode::MEDIA_NEXT.0, "MediaTrackNext", "Nächster Titel"),
    (KeyCode::MEDIA_PREVIOUS.0, "MediaTrackPrevious", "Vorheriger Titel"),
];

/// Name nach `KeyboardEvent.code`, z. B. „ScrollLock“
pub fn key_name(key: KeyCode) -> Option<&'static str> {
    NAMES.iter().find(|(code, ..)| *code == key.0).map(|&(_, name, _)| name)
}

pub fn key_from_name(name: &str) -> Option<KeyCode> {
    NAMES.iter().find(|(_, n, _)| n.eq_ignore_ascii_case(name)).map(|&(code, ..)| KeyCode(code))
}

/// Beschriftung für die Oberfläche, z. B. „Rollen“ oder „Strg links“
pub fn key_label(key: KeyCode) -> String {
    NAMES
        .iter()
        .find(|(code, ..)| *code == key.0)
        .map_or_else(|| format!("Taste {:#06x}", key.0), |&(.., label)| label.to_owned())
}

/// Alle benennbaren Tasten: (Taste, Name, Beschriftung)
pub fn all_keys() -> impl Iterator<Item = (KeyCode, &'static str, &'static str)> {
    NAMES.iter().map(|&(code, name, label)| (KeyCode(code), name, label))
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

    /// Für die Oberfläche, z. B. „Strg+Alt+L“
    pub fn label(&self, mac: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        let names = if mac { ["Ctrl", "Umschalt", "Option", "Cmd"] } else { ["Strg", "Umschalt", "Alt", "Windows"] };
        for (on, name) in [self.mods.ctrl, self.mods.shift, self.mods.alt, self.mods.meta].into_iter().zip(names) {
            if on {
                parts.push(name.to_owned());
            }
        }
        parts.push(key_label(self.key));
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
        let key = parts.pop().filter(|k| !k.is_empty()).ok_or_else(|| format!("„{text}“: Taste fehlt"))?;
        for part in parts {
            let slot = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "strg" => &mut mods.ctrl,
                "shift" | "umschalt" => &mut mods.shift,
                "alt" | "option" => &mut mods.alt,
                "meta" | "cmd" | "win" | "super" => &mut mods.meta,
                _ => return Err(format!("„{part}“ ist keine Zusatztaste (Ctrl, Shift, Alt, Meta)")),
            };
            *slot = true;
        }
        let key = key_from_name(key).ok_or_else(|| format!("unbekannte Taste „{key}“"))?;
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
    DIRECTIONS.iter().find(|(e, ..)| *e == edge).map(|&(_, name, label)| (name, label)).expect("alle Richtungen")
}

impl SwitchTarget {
    /// Für Log und Oberfläche, z. B. „nach links“ oder „zu „laptop““
    pub fn label(&self) -> String {
        match self {
            Self::Screen(name) => format!("zu „{name}“"),
            Self::Direction(edge) => format!("nach {}", direction(*edge).1),
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
            None if text.is_empty() => Err("Ziel fehlt (Rechner oder left, right, up, down)".into()),
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
}

impl HotkeyEvent {
    /// Soll die Erfassung die Taste verschlucken? `remote`: Der Cursor ist auf einem anderen Rechner.
    pub fn swallow(&self, remote: bool) -> bool {
        match self {
            Self::None => false,
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
}

impl HotkeyTracker {
    /// Für `const`-Initialisierung in Hook-Threads
    pub const fn new() -> Self {
        Self { mod_keys: [false; 8], held: Vec::new() }
    }

    pub fn on_key(&mut self, key: KeyCode, down: bool, options: &KeyOptions) -> HotkeyEvent {
        let slot = Mods::slot(key);
        if let Some(slot) = slot {
            self.mod_keys[slot] = down;
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
    /// Loslassen geschieht schon auf dem anderen Rechner.
    fn find(&self, key: KeyCode, mods: Mods) -> Option<(CaptureEvent, bool)> {
        let hit = |h: &Hotkey| h.key == key && h.mods == mods && !h.is_emergency_exit();
        if let Some(lock) = self.lock.as_ref().filter(|h| hit(h)) {
            return Some((CaptureEvent::ToggleLock, lock.swallow_locally()));
        }
        let (_, target) = self.switch.iter().find(|(h, _)| hit(h))?;
        Some((CaptureEvent::SwitchTo(target.clone()), true))
    }

    /// Doppelt belegte oder reservierte Kürzel, je eine Meldung
    pub fn conflicts(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(lock) = self.lock.filter(Hotkey::is_emergency_exit) {
            out.push(format!("Fang-Taste „{lock}“ ist der Notausgang"));
        }
        for (i, (hotkey, target)) in self.switch.iter().enumerate() {
            let what = format!("Kürzel „{hotkey}“ ({})", target.label());
            if hotkey.is_emergency_exit() {
                out.push(format!("{what} ist der Notausgang"));
            } else if self.lock == Some(*hotkey) {
                out.push(format!("{what} ist schon die Fang-Taste"));
            } else if let Some((_, first)) = self.switch[..i].iter().find(|(h, _)| h == hotkey) {
                out.push(format!("{what} ist schon belegt ({})", first.label()));
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
    /// Für Log und Oberfläche
    pub fn label(self) -> &'static str {
        match self {
            Self::Position => "Tastenposition",
            Self::Chars => "Zeichen",
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
            let key = |name: &str| key_from_name(name).ok_or_else(|| format!("unbekannte Taste „{name}“"));
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
            let target = target.parse::<SwitchTarget>().map_err(|e| format!("„{hotkey}“: {e}"))?;
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
        assert_eq!(key_label(KeyCode(0xE3)), "Windows/Cmd links");
        assert_eq!(key_label(KeyCode(0x99)), "Taste 0x0099");
    }

    #[test]
    fn hotkeys_parse_and_print() {
        let hotkey: Hotkey = "Ctrl+Alt+KeyL".parse().unwrap();
        assert_eq!(hotkey, Hotkey { mods: Mods { ctrl: true, alt: true, ..Mods::default() }, key: L });
        assert_eq!(hotkey.to_string(), "Ctrl+Alt+KeyL");
        assert_eq!(hotkey.label(false), "Strg+Alt+L");
        assert_eq!(hotkey.label(true), "Ctrl+Option+L");
        assert_eq!("ScrollLock".parse::<Hotkey>().unwrap(), Hotkey::SCROLL_LOCK);
        assert_eq!("strg + umschalt + F12".parse::<Hotkey>().unwrap().to_string(), "Ctrl+Shift+F12");
        assert!("Ctrl+".parse::<Hotkey>().is_err());
        assert!("Hyper+KeyA".parse::<Hotkey>().is_err());
        assert!("Ctrl+Nichts".parse::<Hotkey>().is_err());
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
    fn a_modifier_alone_can_be_the_hotkey() {
        let options = lock_only("ControlRight");
        let mut t = HotkeyTracker::default();
        assert_eq!(t.on_key(RCTRL, true, &options), toggled(false), "einzelne Taste: lokal durchlassen");
        assert_eq!(t.on_key(RCTRL, false, &options), HotkeyEvent::Consumed { swallow_locally: false });
        t.on_key(CTRL, true, &options);
        assert_eq!(t.on_key(RCTRL, true, &options), HotkeyEvent::None, "mit linkem Strg ist es Strg+Strg");
        let off = KeyOptions { lock: None, ..KeyOptions::default() };
        assert_eq!(t.on_key(L, true, &off), HotkeyEvent::None);
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

        let conflicts = options.conflicts();
        assert_eq!(conflicts.len(), 2, "{conflicts:?}");
        assert!(conflicts[0].contains("„Ctrl+Alt+KeyL“ (zu „tablet“) ist schon die Fang-Taste"), "{conflicts:?}");
        assert!(conflicts[1].contains("Notausgang"), "{conflicts:?}");
        options.switch.push(("Alt+Ctrl+Digit1".parse().unwrap(), SwitchTarget::Direction(Edge::Right)));
        assert!(options.conflicts()[2].contains("ist schon belegt (zu „laptop“)"), "{:?}", options.conflicts());
        assert!(switching().conflicts().is_empty());
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
        assert_eq!(target("right").label(), "nach rechts");
        assert_eq!(target("laptop").label(), "zu „laptop“");

        let bad = HashMap::from([("Ctrl+Alt+Digit1".to_owned(), String::new())]);
        assert!(parse_switch_keys(&bad).unwrap_err().contains("„Ctrl+Alt+Digit1“"));
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
        assert!(parse_key_map(&HashMap::from([("Gibtsnicht".to_owned(), "KeyA".to_owned())])).is_err());
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
        assert_eq!(KeyMode::Chars.label(), "Zeichen");
        assert!(KeyConfig::default().modes.is_empty());
    }

    /// Ohne toml-Abhängigkeit im Kern: dieselben serde-Pfade über den String-Deserializer
    fn toml_like<T: for<'de> Deserialize<'de>>(media: &str, lock: &str) -> T {
        use serde::de::value::{Error, MapDeserializer};
        let map = MapDeserializer::<_, Error>::new([("media", media), ("lock", lock)].into_iter());
        T::deserialize(map).unwrap()
    }
}
