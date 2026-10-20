//! Tastatur-Einstellungen der Oberfläche: Fang-Taste, Wechsel-Kürzel, Medientasten, Umbelegung und
//! Tastenposition oder Zeichen je Rechner.
//! Sie wirken auf dem Server (dort läuft die Erfassung) und werden zu `qross_core::keys::KeyConfig`.
//! Tasten heißen wie `KeyboardEvent.code` („ScrollLock“, „AltLeft“), wie in `qross.toml`.

use std::collections::{BTreeMap, HashMap};

use qross_core::keys::{key_from_name, Hotkey, KeyConfig, KeyMap, KeyMode, KeyOptions, MediaKeys, SwitchTarget};
use qross_proto::Edge;
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct KeySettings {
    /// Fang-Taste wie „ScrollLock“ oder „Ctrl+Alt+KeyL“; leer = aus
    pub lock: String,
    pub media: MediaKeys,
    /// Rechner → Regeln in der eingegebenen Reihenfolge
    pub remap: BTreeMap<String, Vec<KeyRule>>,
    pub switch: SwitchKeys,
    /// Rechner → Tastenposition (Standard, fehlt dann meist) oder Zeichen
    pub modes: BTreeMap<String, KeyMode>,
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
    /// Fehler in `lang`.
    fn entries(&self, lang: Lang) -> Result<Vec<(Hotkey, SwitchTarget)>, String> {
        if let Some(unknown) = self.directions.keys().find(|d| !DIRECTIONS.iter().any(|(name, _)| name == d)) {
            return Err(match lang {
                Lang::De => format!("Wechsel-Kürzel: unbekannte Richtung „{unknown}“"),
                Lang::En => format!("Switch shortcut: unknown direction “{unknown}”"),
            });
        }
        let directions = DIRECTIONS
            .iter()
            .filter_map(|(name, edge)| Some((self.directions.get(*name)?, SwitchTarget::Direction(*edge))));
        let screens = self.screens.iter().map(|(name, hotkey)| (hotkey, SwitchTarget::Screen(name.clone())));
        directions
            .chain(screens)
            .filter(|(hotkey, _)| !hotkey.trim().is_empty())
            .map(|(text, target)| {
                let hotkey = text.parse::<Hotkey>().map_err(|_| {
                    let problem = hotkey_problem(text, lang);
                    match (lang, &target) {
                        (Lang::De, _) => format!("Kürzel {}: {problem}", target.label_in(lang)),
                        (Lang::En, SwitchTarget::Screen(name)) => format!("Shortcut for switching to “{name}”: {problem}"),
                        (Lang::En, SwitchTarget::Direction(_)) => format!("Shortcut for switching {target}: {problem}"),
                    }
                })?;
                Ok((hotkey, target))
            })
            .collect()
    }
}

/// Warum sich `text` nicht als Kürzel lesen lässt, in `lang` – dieselben Prüfungen wie
/// `Hotkey::from_str`, dessen Meldung englisch fürs Log ist
fn hotkey_problem(text: &str, lang: Lang) -> String {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let key = parts.pop().unwrap_or_default();
    // Eine Zusatztaste ist gültig, wenn sie vor einer bekannten Taste gelesen wird
    let not_modifier = parts.into_iter().find(|part| format!("{part}+KeyA").parse::<Hotkey>().is_err());
    match (key.is_empty(), not_modifier, lang) {
        (true, _, Lang::De) => format!("„{text}“: Taste fehlt"),
        (true, _, Lang::En) => format!("“{text}”: the key is missing"),
        (false, Some(part), Lang::De) => format!("„{part}“ ist keine Zusatztaste (Ctrl, Shift, Alt, Meta)"),
        (false, Some(part), Lang::En) => format!("“{part}” isn't a modifier key (Ctrl, Shift, Alt, Meta)"),
        (false, None, Lang::De) => format!("unbekannte Taste „{key}“"),
        (false, None, Lang::En) => format!("unknown key “{key}”"),
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
            modes: BTreeMap::new(),
        }
    }
}

impl KeySettings {
    /// Prüft alles und baut die Konfiguration für den Server. Unbekannte Namen und doppelt
    /// umbelegte Tasten ergeben eine Meldung, keinen Abbruch. Doppelt belegte Kürzel nicht: Die
    /// Oberfläche zeigt sie an, und bis sie behoben sind, gilt die Fang-Taste bzw. der erste Eintrag.
    /// Die Meldung kann im Fenster stehen, deshalb in `lang`.
    pub fn config(&self, lang: Lang) -> Result<KeyConfig, String> {
        let lock = match self.lock.trim() {
            "" => None,
            text => Some(text.parse::<Hotkey>().map_err(|_| match lang {
                Lang::De => format!("Fang-Taste: {}", hotkey_problem(text, lang)),
                Lang::En => format!("Cursor lock key: {}", hotkey_problem(text, lang)),
            })?),
        };
        let mut remap: HashMap<String, KeyMap> = HashMap::new();
        for (machine, rules) in &self.remap {
            let mut map = KeyMap::new();
            for rule in rules {
                let key = |name: &str| {
                    key_from_name(name).ok_or_else(|| match lang {
                        Lang::De => format!("Umbelegung für „{machine}“: unbekannte Taste „{name}“"),
                        Lang::En => format!("Remapping for “{machine}”: unknown key “{name}”"),
                    })
                };
                let (from, to) = (key(&rule.from)?, key(&rule.to)?);
                if map.insert(from, to).is_some() {
                    return Err(match lang {
                        Lang::De => format!("Umbelegung für „{machine}“: „{}“ ist doppelt belegt", rule.from),
                        Lang::En => format!("Remapping for “{machine}”: “{}” is remapped twice", rule.from),
                    });
                }
            }
            if !map.is_empty() {
                remap.insert(machine.clone(), map);
            }
        }
        let switch = self.switch.entries(lang)?;
        let modes = self.modes.iter().map(|(machine, mode)| (machine.clone(), *mode)).collect();
        Ok(KeyConfig { options: KeyOptions { lock, media: self.media, switch }, remap, modes })
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
        for (hotkey, target) in self.switch.entries(Lang::En).unwrap_or_default() {
            if !hotkey.is_emergency_exit() && !used.contains(&hotkey) {
                used.push(hotkey);
                lines += &format!("{} = {}\n", crate::quote(&hotkey.to_string()), crate::quote(&target.to_string()));
            }
        }
        if !lines.is_empty() {
            out += &format!("\n[keys.switch]\n{lines}");
        }
        // Nur Abweichungen vom Standard (Tastenposition)
        let chars: Vec<&String> = self.modes.iter().filter(|(_, mode)| **mode == KeyMode::Chars).map(|(m, _)| m).collect();
        if !chars.is_empty() {
            out += "\n[keys.mode]\n";
            for machine in chars {
                out += &format!("{} = \"chars\"\n", crate::quote(machine));
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
        assert_eq!(KeySettings::default().config(Lang::En).unwrap(), KeyConfig::default());
    }

    #[test]
    fn settings_become_a_key_config() {
        let mut keys = KeySettings { lock: "Ctrl+Alt+KeyL".into(), media: MediaKeys::Local, ..KeySettings::default() };
        keys.remap.insert("laptop".into(), vec![rule("AltLeft", "MetaLeft"), rule("MetaLeft", "AltLeft")]);
        keys.remap.insert("leer".into(), vec![]);
        let config = keys.config(Lang::En).unwrap();
        assert_eq!(config.options.lock, Some("Ctrl+Alt+KeyL".parse().unwrap()));
        assert_eq!(config.options.media, MediaKeys::Local);
        assert_eq!(config.remap["laptop"][&KeyCode(0xE2)], KeyCode(0xE3), "Alt links → Windows/Cmd links");
        assert_eq!(config.remap["laptop"].len(), 2);
        assert!(!config.remap.contains_key("leer"), "Rechner ohne Regeln fallen weg");

        keys.lock = "   ".into();
        assert_eq!(keys.config(Lang::En).unwrap().options.lock, None, "leer = aus");
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
        let options = keys.config(Lang::En).unwrap().options;
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
        assert_eq!(bad.config(Lang::De).unwrap_err(), "Kürzel nach links: unbekannte Taste „Nichts“");
        assert_eq!(bad.config(Lang::En).unwrap_err(), "Shortcut for switching left: unknown key “Nichts”");
        let bad = KeySettings { switch: switch_keys(&[], &[("laptop", "Ctrl+")]), ..KeySettings::default() };
        assert_eq!(bad.config(Lang::De).unwrap_err(), "Kürzel zu „laptop“: „Ctrl+“: Taste fehlt");
        assert_eq!(bad.config(Lang::En).unwrap_err(), "Shortcut for switching to “laptop”: “Ctrl+”: the key is missing");
        let bad = KeySettings { switch: switch_keys(&[("links", "Ctrl+KeyA")], &[]), ..KeySettings::default() };
        assert_eq!(bad.config(Lang::De).unwrap_err(), "Wechsel-Kürzel: unbekannte Richtung „links“");
        assert_eq!(bad.config(Lang::En).unwrap_err(), "Switch shortcut: unknown direction “links”");
        assert!(!bad.to_toml().contains("[keys.switch]"), "Vorschau bleibt lesbar");
    }

    #[test]
    fn key_modes_become_config_and_toml() {
        let mut keys = KeySettings::default();
        assert!(!keys.to_toml().contains("[keys.mode]"), "Standard: nichts exportieren");
        keys.modes.insert("mac".into(), KeyMode::Chars);
        keys.modes.insert("pc 2".into(), KeyMode::Position);
        let modes = keys.config(Lang::En).unwrap().modes;
        assert_eq!((modes["mac"], modes["pc 2"]), (KeyMode::Chars, KeyMode::Position));
        let toml = keys.to_toml();
        assert!(toml.ends_with("\n[keys.mode]\n\"mac\" = \"chars\"\n"), "{toml}");
        // gui.json ohne Feld (ältere Version): alles Tastenposition
        let old: KeySettings = serde_json::from_str(r#"{"lock":"ScrollLock"}"#).unwrap();
        assert!(old.modes.is_empty());
        let saved: KeySettings = serde_json::from_str(r#"{"modes":{"mac":"chars"}}"#).unwrap();
        assert_eq!(saved.modes["mac"], KeyMode::Chars);
    }

    #[test]
    fn invalid_settings_give_a_message() {
        let bad_lock = KeySettings { lock: "Ctrl+Gibtsnicht".into(), ..KeySettings::default() };
        assert_eq!(bad_lock.config(Lang::De).unwrap_err(), "Fang-Taste: unbekannte Taste „Gibtsnicht“");
        assert_eq!(bad_lock.config(Lang::En).unwrap_err(), "Cursor lock key: unknown key “Gibtsnicht”");
        let bad_mod = KeySettings { lock: "Ctrl+Hyper+KeyL".into(), ..KeySettings::default() };
        assert_eq!(bad_mod.config(Lang::De).unwrap_err(), "Fang-Taste: „Hyper“ ist keine Zusatztaste (Ctrl, Shift, Alt, Meta)");
        assert_eq!(bad_mod.config(Lang::En).unwrap_err(), "Cursor lock key: “Hyper” isn't a modifier key (Ctrl, Shift, Alt, Meta)");
        // Dieselben Gründe wie beim Lesen in `qross_core`, nur in der Sprache der Oberfläche
        assert_eq!(hotkey_problem("Alt+", Lang::De), "„Alt+“: Taste fehlt");
        assert_eq!(hotkey_problem("+KeyL", Lang::De), "„“ ist keine Zusatztaste (Ctrl, Shift, Alt, Meta)");
        assert_eq!(hotkey_problem("Strg+Hyper+Nichts", Lang::En), "“Hyper” isn't a modifier key (Ctrl, Shift, Alt, Meta)");

        let mut bad_key = KeySettings::default();
        bad_key.remap.insert("laptop".into(), vec![rule("AltLeft", "Falsch")]);
        let err = bad_key.config(Lang::De).unwrap_err();
        assert!(err.contains("laptop") && err.contains("Falsch"), "{err}");
        assert_eq!(err, "Umbelegung für „laptop“: unbekannte Taste „Falsch“");
        assert_eq!(bad_key.config(Lang::En).unwrap_err(), "Remapping for “laptop”: unknown key “Falsch”");

        let mut twice = KeySettings::default();
        twice.remap.insert("laptop".into(), vec![rule("CapsLock", "ControlLeft"), rule("CapsLock", "Escape")]);
        assert!(twice.config(Lang::De).unwrap_err().contains("doppelt"));
        assert_eq!(twice.config(Lang::En).unwrap_err(), "Remapping for “laptop”: “CapsLock” is remapped twice");
    }
}
