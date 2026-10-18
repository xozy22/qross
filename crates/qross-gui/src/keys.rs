//! Tastatur-Einstellungen der Oberfläche: Fang-Taste, Wechsel-Kürzel, Medientasten, Umbelegung je
//! Rechner.
//! Sie wirken auf dem Server (dort läuft die Erfassung) und werden zu `qross_core::keys::KeyConfig`.
//! Tasten heißen wie `KeyboardEvent.code` („ScrollLock“, „AltLeft“), wie in `qross.toml`.

use std::collections::{BTreeMap, HashMap};

use qross_core::keys::{key_from_name, Hotkey, KeyConfig, KeyMap, KeyOptions, MediaKeys, SwitchTarget};
use qross_proto::Edge;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct KeySettings {
    /// Fang-Taste wie „ScrollLock“ oder „Ctrl+Alt+KeyL“; leer = aus
    pub lock: String,
    pub media: MediaKeys,
    /// Rechner → Regeln in der eingegebenen Reihenfolge
    pub remap: BTreeMap<String, Vec<KeyRule>>,
    pub switch: SwitchKeys,
}

/// Wechsel-Kürzel: je Richtung und je Rechner höchstens eines, wie „Ctrl+Alt+ArrowLeft“
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(default)]
pub struct SwitchKeys {
    /// „left“, „right“, „up“, „down“ → Kürzel
    pub directions: BTreeMap<String, String>,
    /// Rechner → Kürzel
    pub screens: BTreeMap<String, String>,
}

/// Richtungen in der Reihenfolge der Oberfläche
const DIRECTIONS: [(&str, Edge); 4] = [("left", Edge::Left), ("right", Edge::Right), ("up", Edge::Top), ("down", Edge::Bottom)];

impl SwitchKeys {
    /// Alle belegten Kürzel: erst die Richtungen, dann die Rechner. Bei doppelter Belegung gilt der
    /// erste Eintrag (die Fang-Taste geht ohnehin vor); die Oberfläche zeigt solche Konflikte an.
    fn entries(&self) -> Result<Vec<(Hotkey, SwitchTarget)>, String> {
        if let Some(unknown) = self.directions.keys().find(|d| !DIRECTIONS.iter().any(|(name, _)| name == d)) {
            return Err(format!("Wechsel-Kürzel: unbekannte Richtung „{unknown}“"));
        }
        let directions = DIRECTIONS
            .iter()
            .filter_map(|(name, edge)| Some((self.directions.get(*name)?, SwitchTarget::Direction(*edge))));
        let screens = self.screens.iter().map(|(name, hotkey)| (hotkey, SwitchTarget::Screen(name.clone())));
        directions
            .chain(screens)
            .filter(|(hotkey, _)| !hotkey.trim().is_empty())
            .map(|(hotkey, target)| {
                let hotkey = hotkey.parse::<Hotkey>().map_err(|e| format!("Kürzel {}: {e}", target.label()))?;
                Ok((hotkey, target))
            })
            .collect()
    }
}

/// Gedrückte Taste `from` kommt beim Rechner als `to` an.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct KeyRule {
    pub from: String,
    pub to: String,
}

impl Default for KeySettings {
    fn default() -> Self {
        Self {
            lock: Hotkey::SCROLL_LOCK.to_string(),
            media: MediaKeys::Active,
            remap: BTreeMap::new(),
            switch: SwitchKeys::default(),
        }
    }
}

impl KeySettings {
    /// Prüft alles und baut die Konfiguration für den Server. Unbekannte Namen und doppelt
    /// umbelegte Tasten ergeben eine Meldung, keinen Abbruch. Doppelt belegte Kürzel nicht: Die
    /// Oberfläche zeigt sie an, und bis sie behoben sind, gilt die Fang-Taste bzw. der erste Eintrag.
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
        let switch = self.switch.entries()?;
        Ok(KeyConfig { options: KeyOptions { lock, media: self.media, switch }, remap })
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
        // Nur, was gilt: `qross server` lehnt doppelt belegte Kürzel ab.
        let mut used: Vec<Hotkey> = self.lock.trim().parse().into_iter().collect();
        let mut lines = String::new();
        for (hotkey, target) in self.switch.entries().unwrap_or_default() {
            if !hotkey.is_emergency_exit() && !used.contains(&hotkey) {
                used.push(hotkey);
                lines += &format!("{} = {}\n", crate::quote(&hotkey.to_string()), crate::quote(&target.to_string()));
            }
        }
        if !lines.is_empty() {
            out += &format!("\n[keys.switch]\n{lines}");
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
        let mut keys = KeySettings { lock: "Ctrl+Alt+KeyL".into(), media: MediaKeys::Local, ..KeySettings::default() };
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

    fn switch_keys(directions: &[(&str, &str)], screens: &[(&str, &str)]) -> SwitchKeys {
        let map = |pairs: &[(&str, &str)]| pairs.iter().map(|&(k, v)| (k.to_owned(), v.to_owned())).collect();
        SwitchKeys { directions: map(directions), screens: map(screens) }
    }

    #[test]
    fn switch_keys_become_options_and_toml() {
        let keys = KeySettings {
            lock: "Ctrl+Alt+KeyL".into(),
            switch: switch_keys(
                &[("up", "Ctrl+Alt+ArrowUp"), ("left", "Ctrl+Alt+ArrowLeft"), ("down", "")],
                &[("laptop", "Ctrl+Alt+Digit2"), ("pc", "Ctrl+Alt+ArrowLeft"), ("tablet", "Alt+Ctrl+KeyL")],
            ),
            ..KeySettings::default()
        };
        let options = keys.config().unwrap().options;
        let targets: Vec<String> = options.switch.iter().map(|(h, t)| format!("{h} {t}")).collect();
        assert_eq!(
            targets,
            [
                "Ctrl+Alt+ArrowLeft left",
                "Ctrl+Alt+ArrowUp up",
                "Ctrl+Alt+Digit2 laptop",
                "Ctrl+Alt+ArrowLeft pc",
                "Ctrl+Alt+KeyL tablet"
            ],
            "Richtungen in fester Reihenfolge vor den Rechnern, leere fallen weg"
        );
        assert_eq!(options.conflicts().len(), 2, "doppelt belegt und Fang-Taste – kein Abbruch");

        let toml = keys.to_toml();
        let switch = &toml[toml.find("[keys.switch]").expect("Abschnitt fehlt")..];
        assert_eq!(
            switch,
            "[keys.switch]\n\"Ctrl+Alt+ArrowLeft\" = \"left\"\n\"Ctrl+Alt+ArrowUp\" = \"up\"\n\"Ctrl+Alt+Digit2\" = \"laptop\"\n",
            "nur, was gilt"
        );
        assert!(!KeySettings::default().to_toml().contains("[keys.switch]"));
    }

    #[test]
    fn invalid_switch_keys_give_a_message() {
        let bad = KeySettings { switch: switch_keys(&[("left", "Ctrl+Nichts")], &[]), ..KeySettings::default() };
        assert!(bad.config().unwrap_err().contains("nach links"));
        let bad = KeySettings { switch: switch_keys(&[], &[("laptop", "Ctrl+")]), ..KeySettings::default() };
        assert!(bad.config().unwrap_err().contains("zu „laptop“"));
        let bad = KeySettings { switch: switch_keys(&[("links", "Ctrl+KeyA")], &[]), ..KeySettings::default() };
        assert!(bad.config().unwrap_err().contains("Richtung „links“"));
        assert!(!bad.to_toml().contains("[keys.switch]"), "Vorschau bleibt lesbar");
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
