use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
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
    }

    #[test]
    fn switching_defaults_apply_when_section_is_missing() {
        let file: ServerFile = toml::from_str("name = \"pc\"").unwrap();
        assert_eq!(file.switching.options(), SwitchOptions::RECOMMENDED);
    }
}
