use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use qross_core::keys::{parse_key_map, parse_switch_keys, Hotkey, KeyConfig, KeyOptions, MediaKeys};
use qross_core::server::SwitchOptions;
use qross_core::LinkConfig;
use qross_proto::DEFAULT_PORT;
use serde::Deserialize;

/// Inhalt von `qross.toml` auf dem Server.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ServerFile {
    /// Name dieses Rechners im Layout (Standard: Rechnername in Kleinbuchstaben)
    pub name: Option<String>,
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub links: Vec<LinkConfig>,
    #[serde(default)]
    pub switching: Switching,
    /// Auf Suchanfragen im LAN antworten (`qross client` ohne Adresse)
    #[serde(default = "yes")]
    pub discovery: bool,
    #[serde(default)]
    pub clipboard: ClipboardSection,
    #[serde(default)]
    pub keys: KeysSection,
}

/// Abschnitt `[keys]`: Fang-Taste, Wechsel-Kürzel, Medientasten, Umbelegung je Rechner.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct KeysSection {
    /// Taste oder Kombination wie „ScrollLock“ oder „Ctrl+Alt+KeyL“; leer = aus
    #[serde(default = "default_lock")]
    pub lock: String,
    #[serde(default)]
    pub media: MediaKeys,
    /// Name des Rechners → { gedrückte Taste = gesendete Taste }
    #[serde(default)]
    pub remap: HashMap<String, HashMap<String, String>>,
    /// Kürzel → Rechner oder Richtung („left“, „right“, „up“, „down“)
    #[serde(default)]
    pub switch: HashMap<String, String>,
}

impl Default for KeysSection {
    fn default() -> Self {
        Self { lock: default_lock(), media: MediaKeys::default(), remap: HashMap::new(), switch: HashMap::new() }
    }
}

fn default_lock() -> String {
    Hotkey::SCROLL_LOCK.to_string()
}

impl KeysSection {
    pub fn config(&self) -> Result<KeyConfig> {
        let lock = match self.lock.trim() {
            "" => None,
            text => Some(text.parse::<Hotkey>().map_err(|e| anyhow::anyhow!("[keys] lock: {e}"))?),
        };
        let remap = self
            .remap
            .iter()
            .map(|(name, entries)| {
                let map = parse_key_map(entries).map_err(|e| anyhow::anyhow!("[keys.remap.{name}]: {e}"))?;
                Ok((name.clone(), map))
            })
            .collect::<Result<_>>()?;
        let switch = parse_switch_keys(&self.switch).map_err(|e| anyhow::anyhow!("[keys.switch]: {e}"))?;
        let options = KeyOptions { lock, media: self.media, switch };
        if let Some(conflict) = options.conflicts().into_iter().next() {
            anyhow::bail!("[keys]: {conflict}");
        }
        Ok(KeyConfig { options, remap })
    }
}

/// Abschnitt `[clipboard]`: kopierte Dateien.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ClipboardSection {
    /// Kopierte Dateien bis zu dieser Gesamtgröße in MB beim Wechsel mitgeben (0 = nie)
    #[serde(default = "default_max_file_mb")]
    pub max_file_mb: u64,
}

impl Default for ClipboardSection {
    fn default() -> Self {
        Self { max_file_mb: default_max_file_mb() }
    }
}

impl ClipboardSection {
    pub fn file_limit(&self) -> u64 {
        mb_to_bytes(self.max_file_mb)
    }
}

pub fn default_max_file_mb() -> u64 {
    qross_net::clipfiles::DEFAULT_LIMIT / 1_000_000
}

pub fn mb_to_bytes(mb: u64) -> u64 {
    mb.saturating_mul(1_000_000)
}

/// Abschnitt `[switching]`: wann der Cursor den Bildschirm wechselt.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Switching {
    /// Ecken-Totzone in Pixeln (0 = aus)
    #[serde(default = "default_corner")]
    pub corner_size: u32,
    /// So lange gegen die Kante drücken, bevor gewechselt wird (0 = sofort)
    #[serde(default)]
    pub delay_ms: u64,
}

impl Default for Switching {
    fn default() -> Self {
        Self { corner_size: default_corner(), delay_ms: 0 }
    }
}

impl Switching {
    pub fn options(&self) -> SwitchOptions {
        SwitchOptions { corner_size: self.corner_size, delay: Duration::from_millis(self.delay_ms) }
    }
}

fn default_corner() -> u32 {
    SwitchOptions::RECOMMENDED.corner_size
}

fn yes() -> bool {
    true
}

fn default_listen() -> SocketAddr {
    (Ipv4Addr::UNSPECIFIED, DEFAULT_PORT).into()
}

impl ServerFile {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!("Konfiguration {} nicht gefunden (Vorlage: qross.example.toml)", path.display())
        })?;
        toml::from_str(&text).with_context(|| format!("{} ist ungültig", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qross_core::keys::SwitchTarget;
    use qross_proto::Edge;

    #[test]
    fn example_config_parses() {
        let text = include_str!("../../../qross.example.toml");
        let file: ServerFile = toml::from_str(text).unwrap();
        assert_eq!(file.listen.port(), DEFAULT_PORT);
        assert_eq!(file.links[0].edge, Edge::Right);
        qross_core::Layout::from_links(&file.links).unwrap();
        assert_eq!(file.switching.options(), SwitchOptions { corner_size: 8, delay: Duration::from_millis(0) });
        assert!(file.discovery);
        assert_eq!(file.clipboard.file_limit(), 1_000_000_000);
        let keys = file.keys.config().unwrap();
        assert_eq!(keys.options.lock, Some(Hotkey::SCROLL_LOCK));
        assert_eq!(keys.options.media, MediaKeys::Active);
        assert_eq!(keys.remap["laptop"].len(), 1, "Beispiel: Alt und Windows-Taste am Mac");
        let switch: Vec<String> = keys.options.switch.iter().map(|(h, t)| format!("{h} = {t}")).collect();
        assert_eq!(switch, ["Ctrl+Alt+ArrowLeft = left", "Ctrl+Alt+ArrowRight = right", "Ctrl+Alt+Digit1 = pc", "Ctrl+Alt+Digit2 = laptop"]);
    }

    #[test]
    fn switch_keys_are_checked() {
        let file: ServerFile = toml::from_str("[keys.switch]
\"Ctrl+Alt+Digit1\" = \"laptop\"
\"Ctrl+Alt+ArrowUp\" = \"up\"").unwrap();
        let switch = file.keys.config().unwrap().options.switch;
        assert_eq!(switch[0], ("Ctrl+Alt+ArrowUp".parse().unwrap(), SwitchTarget::Direction(Edge::Top)));
        assert_eq!(switch[1].1, SwitchTarget::Screen("laptop".into()));

        let error = |text: &str| toml::from_str::<ServerFile>(text).unwrap().keys.config().unwrap_err().to_string();
        assert!(error("[keys.switch]
\"Ctrl+Gibtsnicht\" = \"left\"").contains("[keys.switch]"));
        assert!(error("[keys.switch]
\"Ctrl+Alt+KeyL\" = \"\"").contains("Ziel fehlt"));
        let lock = error("[keys]
lock = \"Ctrl+Alt+KeyL\"
[keys.switch]
\"Alt+Ctrl+KeyL\" = \"laptop\"");
        assert!(lock.contains("schon die Fang-Taste"), "{lock}");
        let twice = error("[keys.switch]
\"Ctrl+Alt+KeyL\" = \"laptop\"
\"Alt+Ctrl+KeyL\" = \"left\"");
        assert!(twice.contains("schon belegt"), "{twice}");
        assert!(error("[keys.switch]
\"Ctrl+Alt+Shift+Escape\" = \"pc\"").contains("Notausgang"));
    }

    #[test]
    fn keys_section_is_checked() {
        let file: ServerFile = toml::from_str("[keys]\nlock = \"\"\nmedia = \"local\"").unwrap();
        let keys = file.keys.config().unwrap();
        assert_eq!((keys.options.lock, keys.options.media), (None, MediaKeys::Local));
        let file: ServerFile = toml::from_str("[keys]\nlock = \"Ctrl+Gibtsnicht\"").unwrap();
        assert!(file.keys.config().is_err());
        let file: ServerFile = toml::from_str("[keys.remap.laptop]\nAltLeft = \"Falsch\"").unwrap();
        assert!(file.keys.config().unwrap_err().to_string().contains("laptop"));
    }

    #[test]
    fn switching_defaults_apply_when_section_is_missing() {
        let file: ServerFile = toml::from_str("name = \"pc\"").unwrap();
        assert_eq!(file.switching.options(), SwitchOptions::RECOMMENDED);
        assert_eq!(file.clipboard.file_limit(), qross_net::clipfiles::DEFAULT_LIMIT);
    }

    #[test]
    fn copied_files_can_be_switched_off() {
        let file: ServerFile = toml::from_str("[clipboard]\nmax_file_mb = 0").unwrap();
        assert_eq!(file.clipboard.file_limit(), 0);
    }
}
