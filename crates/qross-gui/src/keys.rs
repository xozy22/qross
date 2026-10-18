//! Tastatur-Einstellungen der Oberfläche: Fang-Taste, Medientasten, Umbelegung je Rechner.
//! Sie wirken auf dem Server (dort läuft die Erfassung) und werden zu `qross_core::keys::KeyConfig`.
//! Tasten heißen wie `KeyboardEvent.code` („ScrollLock“, „AltLeft“), wie in `qross.toml`.

use std::collections::{BTreeMap, HashMap};

use qross_core::keys::{key_from_name, Hotkey, KeyConfig, KeyMap, KeyOptions, MediaKeys};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct KeySettings {
    /// Fang-Taste wie „ScrollLock“ oder „Ctrl+Alt+KeyL“; leer = aus
    pub lock: String,
    pub media: MediaKeys,
    /// Rechner → Regeln in der eingegebenen Reihenfolge
    pub remap: BTreeMap<String, Vec<KeyRule>>,
}

/// Gedrückte Taste `from` kommt beim Rechner als `to` an.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct KeyRule {
    pub from: String,
    pub to: String,
}

impl Default for KeySettings {
    fn default() -> Self {
        Self { lock: Hotkey::SCROLL_LOCK.to_string(), media: MediaKeys::Active, remap: BTreeMap::new() }
    }
}

impl KeySettings {
    /// Prüft alles und baut die Konfiguration für den Server. Unbekannte Namen und doppelt
    /// belegte Tasten ergeben eine Meldung, keinen Abbruch.
    pub fn config(&self) -> Result<KeyConfig, String> {
        let lock = match self.lock.trim() {
            "" => None,
            text => Some(text.parse::<Hotkey>().map_err(|e| format!("Fang-Taste: {e}"))?),
        };
        let mut remap: HashMap<String, KeyMap> = HashMap::new();
        for (machine, rules) in &self.remap {
            let mut map = KeyMap::new();
            for rule in rules {
                let key = |name: &str| {
                    key_from_name(name).ok_or_else(|| format!("Umbelegung für „{machine}“: unbekannte Taste „{name}“"))
                };
                let (from, to) = (key(&rule.from)?, key(&rule.to)?);
                if map.insert(from, to).is_some() {
                    return Err(format!("Umbelegung für „{machine}“: „{}“ ist doppelt belegt", rule.from));
                }
            }
            if !map.is_empty() {
                remap.insert(machine.clone(), map);
            }
        }
        Ok(KeyConfig { options: KeyOptions { lock, media: self.media }, remap })
    }

    /// Abschnitt `[keys]` für `qross.toml` (gleiches Format wie `qross.example.toml`)
    pub fn to_toml(&self) -> String {
        let media = match self.media {
            MediaKeys::Active => "active",
            MediaKeys::Local => "local",
        };
        let mut out = format!("\n[keys]\nlock = {}\nmedia = \"{media}\"\n", crate::quote(self.lock.trim()));
        for (machine, rules) in self.remap.iter().filter(|(_, rules)| !rules.is_empty()) {
            out += &format!("\n[keys.remap.{}]\n", crate::quote(machine));
            for rule in rules {
                out += &format!("{} = {}\n", crate::quote(&rule.from), crate::quote(&rule.to));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qross_proto::KeyCode;

    fn rule(from: &str, to: &str) -> KeyRule {
        KeyRule { from: from.into(), to: to.into() }
    }

    #[test]
    fn defaults_match_the_server_defaults() {
        assert_eq!(KeySettings::default().config().unwrap(), KeyConfig::default());
    }

    #[test]
    fn settings_become_a_key_config() {
        let mut keys = KeySettings { lock: "Ctrl+Alt+KeyL".into(), media: MediaKeys::Local, remap: BTreeMap::new() };
        keys.remap.insert("laptop".into(), vec![rule("AltLeft", "MetaLeft"), rule("MetaLeft", "AltLeft")]);
        keys.remap.insert("leer".into(), vec![]);
        let config = keys.config().unwrap();
        assert_eq!(config.options.lock, Some("Ctrl+Alt+KeyL".parse().unwrap()));
        assert_eq!(config.options.media, MediaKeys::Local);
        assert_eq!(config.remap["laptop"][&KeyCode(0xE2)], KeyCode(0xE3), "Alt links → Windows/Cmd links");
        assert_eq!(config.remap["laptop"].len(), 2);
        assert!(!config.remap.contains_key("leer"), "Rechner ohne Regeln fallen weg");

        keys.lock = "   ".into();
        assert_eq!(keys.config().unwrap().options.lock, None, "leer = aus");
    }

    #[test]
    fn invalid_settings_give_a_message() {
        let bad_lock = KeySettings { lock: "Ctrl+Gibtsnicht".into(), ..KeySettings::default() };
        assert!(bad_lock.config().unwrap_err().starts_with("Fang-Taste"));

        let mut bad_key = KeySettings::default();
        bad_key.remap.insert("laptop".into(), vec![rule("AltLeft", "Falsch")]);
        let err = bad_key.config().unwrap_err();
        assert!(err.contains("laptop") && err.contains("Falsch"), "{err}");

        let mut twice = KeySettings::default();
        twice.remap.insert("laptop".into(), vec![rule("CapsLock", "ControlLeft"), rule("CapsLock", "Escape")]);
        assert!(twice.config().unwrap_err().contains("doppelt"));
    }
}
