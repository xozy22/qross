//! Plattformunabhängiger Teil der Oberfläche: Einstellungen und Layout-Raster.
//!
//! Der Layout-Editor ordnet Bildschirme in einem Raster an. Liegen zwei Bildschirme direkt
//! nebeneinander oder übereinander, entsteht daraus ein Link wie in `qross.example.toml`.

pub mod changelog;
pub mod clipboard_files;
pub mod keys;
pub mod lifecycle;

use std::collections::HashSet;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use qross_core::server::SwitchOptions;
use qross_core::{Layout, LinkConfig};
use qross_proto::{Edge, DEFAULT_PORT};
use serde::{Deserialize, Serialize};

use crate::keys::KeySettings;

/// Einstellungen der Oberfläche, im Datenverzeichnis von Qross.
pub const SETTINGS_FILE: &str = "gui.json";
/// Aus dem Layout erzeugte Konfiguration für `qross server --config`.
pub const EXPORT_FILE: &str = "qross.toml";

/// Größe des Rasters im Layout-Editor.
pub const GRID_COLS: i32 = 5;
pub const GRID_ROWS: i32 = 3;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    Server,
    Client,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    pub role: Role,
    pub server: ServerSettings,
    pub client: ClientSettings,
    /// Kopierte Dateien beim Wechsel bis zu dieser Gesamtgröße in MB mitgeben (0 = aus).
    /// Gilt als Server wie als Client.
    pub clipboard_files_mb: u64,
    /// Lief beim Beenden ein Server oder Client, startet er beim nächsten Start wieder.
    pub resume_role: bool,
    /// Updates im Hintergrund laden und in einem ruhigen Moment installieren
    pub auto_update: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct ServerSettings {
    /// Name dieses Rechners im Layout
    pub name: String,
    pub port: u16,
    pub screens: Vec<ScreenPos>,
    /// Ecken-Totzone in Pixeln (0 = aus)
    pub corner_size: u32,
    /// So lange gegen die Kante drücken, bevor gewechselt wird (0 = sofort)
    pub switch_delay_ms: u64,
    /// Auf Suchanfragen von Clients im LAN antworten
    pub discovery: bool,
    /// Fang-Taste, Wechsel-Kürzel, Medientasten, Umbelegung je Rechner (wirkt auf dem Server)
    pub keys: KeySettings,
}

/// Ein Bildschirm im Raster des Layout-Editors.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ScreenPos {
    pub name: String,
    pub col: i32,
    pub row: i32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct ClientSettings {
    /// `host`, `host:port` oder IP-Adresse des Servers
    pub address: String,
    /// Name dieses Rechners im Layout des Servers
    pub name: String,
    /// Eingaben nur protokollieren statt ausführen
    pub dry_run: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self::for_host("dieser-pc")
    }
}

impl Default for ServerSettings {
    fn default() -> Self {
        Settings::default().server
    }
}

impl Default for ClientSettings {
    fn default() -> Self {
        Settings::default().client
    }
}

impl Settings {
    /// Startwerte: dieser Rechner als Server in der Mitte des Rasters.
    pub fn for_host(hostname: &str) -> Self {
        Self {
            role: Role::Server,
            server: ServerSettings {
                name: hostname.to_owned(),
                port: DEFAULT_PORT,
                screens: vec![ScreenPos { name: hostname.to_owned(), col: GRID_COLS / 2, row: GRID_ROWS / 2 }],
                corner_size: SwitchOptions::RECOMMENDED.corner_size,
                switch_delay_ms: SwitchOptions::RECOMMENDED.delay.as_millis() as u64,
                discovery: true,
                keys: KeySettings::default(),
            },
            client: ClientSettings { address: String::new(), name: hostname.to_owned(), dry_run: false },
            clipboard_files_mb: qross_net::clipfiles::DEFAULT_LIMIT / 1_000_000,
            resume_role: true,
            auto_update: true,
        }
    }

    /// Grenze für kopierte Dateien in Bytes (0 = nie automatisch übertragen)
    pub fn clipboard_file_limit(&self) -> u64 {
        self.clipboard_files_mb.saturating_mul(1_000_000)
    }

    /// Inhalt einer `qross.toml` für `qross server --config`.
    pub fn server_toml(&self) -> String {
        self.server.to_toml(self.clipboard_files_mb)
    }

    /// Lädt `gui.json` aus `dir`; `None`, wenn es die Datei noch nicht gibt.
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(SETTINGS_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("{} lesen", path.display()))?;
        let mut settings: Self =
            serde_json::from_str(&text).with_context(|| format!("{} ist ungültig", path.display()))?;
        settings.server.fit_grid();
        Ok(Some(settings))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir).with_context(|| format!("{} anlegen", dir.display()))?;
        let path = dir.join(SETTINGS_FILE);
        fs::write(&path, serde_json::to_string_pretty(self)?).with_context(|| format!("{} schreiben", path.display()))
    }
}

impl ServerSettings {
    pub fn listen(&self) -> SocketAddr {
        (Ipv4Addr::UNSPECIFIED, self.port).into()
    }

    pub fn switch_options(&self) -> SwitchOptions {
        SwitchOptions { corner_size: self.corner_size, delay: Duration::from_millis(self.switch_delay_ms) }
    }

    /// Ändert sich etwas, das ein laufender Server erst nach einem Neustart übernimmt?
    pub fn needs_restart(&self, old: &ServerSettings) -> bool {
        self.links() != old.links()
            || self.name != old.name
            || self.port != old.port
            || self.switch_options() != old.switch_options()
            || self.discovery != old.discovery
            || self.keys != old.keys
    }

    pub fn contains(&self, name: &str) -> bool {
        self.screens.iter().any(|s| s.name == name)
    }

    /// Tasten-Einstellungen, wie sie gelten: Wechsel-Kürzel nur zu Rechnern im Layout
    pub fn effective_keys(&self) -> KeySettings {
        let mut keys = self.keys.clone();
        keys.switch.screens.retain(|name, _| self.contains(name));
        keys
    }

    /// Links aus dem Raster: je Paar direkt benachbarter Bildschirme ein Link nach rechts bzw. unten.
    /// Die Gegenrichtung ergänzt [`Layout::from_links`] selbst.
    pub fn links(&self) -> Vec<LinkConfig> {
        let at = |col: i32, row: i32| self.screens.iter().find(|s| s.col == col && s.row == row);
        let mut links = Vec::new();
        for screen in &self.screens {
            for (edge, dc, dr) in [(Edge::Right, 1, 0), (Edge::Bottom, 0, 1)] {
                if let Some(other) = at(screen.col + dc, screen.row + dr) {
                    links.push(LinkConfig { from: screen.name.clone(), edge, to: other.name.clone() });
                }
            }
        }
        links
    }

    /// Prüft Namen und Positionen und baut daraus das Layout für den Server.
    pub fn validate(&self) -> Result<Layout, String> {
        if self.name.trim().is_empty() {
            return Err("Der Server braucht einen Namen.".into());
        }
        if !self.contains(&self.name) {
            return Err(format!("Der Server „{}“ fehlt im Layout.", self.name));
        }
        let mut names = HashSet::new();
        let mut cells = HashSet::new();
        for screen in &self.screens {
            if screen.name.trim().is_empty() {
                return Err("Ein Bildschirm hat keinen Namen.".into());
            }
            if !names.insert(screen.name.as_str()) {
                return Err(format!("Der Name „{}“ kommt mehrfach vor.", screen.name));
            }
            if !cells.insert((screen.col, screen.row)) {
                return Err(format!("„{}“ liegt auf einem belegten Feld.", screen.name));
            }
        }
        let layout = Layout::from_links(&self.links()).map_err(|e| e.to_string())?;
        Ok(layout)
    }

    /// Bildschirme ohne Nachbarn – der Cursor kann sie nicht erreichen.
    pub fn isolated(&self) -> Vec<String> {
        let links = self.links();
        self.screens
            .iter()
            .filter(|s| self.screens.len() > 1 && !links.iter().any(|l| l.from == s.name || l.to == s.name))
            .map(|s| s.name.clone())
            .collect()
    }

    /// Fügt einen Bildschirm auf dem ersten freien Feld neben dem Server ein (sonst irgendwo).
    pub fn add_screen(&mut self, name: &str) -> bool {
        if self.contains(name) {
            return false;
        }
        let free = |col: i32, row: i32| {
            (0..GRID_COLS).contains(&col)
                && (0..GRID_ROWS).contains(&row)
                && !self.screens.iter().any(|s| s.col == col && s.row == row)
        };
        let (sc, sr) = self
            .screens
            .iter()
            .find(|s| s.name == self.name)
            .map(|s| (s.col, s.row))
            .unwrap_or((GRID_COLS / 2, GRID_ROWS / 2));
        let near = [(1, 0), (-1, 0), (0, -1), (0, 1)].into_iter().map(|(dc, dr)| (sc + dc, sr + dr));
        let anywhere = (0..GRID_ROWS).flat_map(|row| (0..GRID_COLS).map(move |col| (col, row)));
        match near.chain(anywhere).find(|&(c, r)| free(c, r)) {
            Some((col, row)) => {
                self.screens.push(ScreenPos { name: name.to_owned(), col, row });
                true
            }
            None => false,
        }
    }

    /// Setzt Bildschirme außerhalb des Rasters (z. B. aus einer älteren Version) auf freie Felder.
    pub fn fit_grid(&mut self) {
        let inside = |s: &ScreenPos| (0..GRID_COLS).contains(&s.col) && (0..GRID_ROWS).contains(&s.row);
        let (keep, outside): (Vec<_>, Vec<_>) = self.screens.drain(..).partition(inside);
        self.screens = keep;
        for screen in outside {
            self.add_screen(&screen.name);
        }
    }

    /// Inhalt einer `qross.toml` für `qross server --config` (siehe auch [`Settings::server_toml`]).
    pub fn to_toml(&self, clipboard_files_mb: u64) -> String {
        let mut out = String::from("# Erzeugt von der Qross-Oberfläche\n");
        out += &format!("name = {}\n", quote(&self.name));
        out += &format!("listen = \"{}\"\n", self.listen());
        out += &format!("discovery = {}\n", self.discovery);
        out += &format!("\n[switching]\ncorner_size = {}\ndelay_ms = {}\n", self.corner_size, self.switch_delay_ms);
        out += &format!("\n[clipboard]\nmax_file_mb = {clipboard_files_mb}\n");
        out += &self.effective_keys().to_toml();
        for link in self.links() {
            out += &format!(
                "\n[[links]]\nfrom = {}\nedge = \"{}\"\nto = {}\n",
                quote(&link.from),
                edge_name(link.edge),
                quote(&link.to)
            );
        }
        out
    }
}

fn edge_name(edge: Edge) -> &'static str {
    match edge {
        Edge::Left => "left",
        Edge::Right => "right",
        Edge::Top => "top",
        Edge::Bottom => "bottom",
    }
}

/// TOML-Zeichenkette mit Escapes (Namen stammen aus Benutzereingaben).
pub(crate) fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            c if c.is_control() => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(screens: &[(&str, i32, i32)]) -> ServerSettings {
        ServerSettings {
            name: screens[0].0.to_owned(),
            port: DEFAULT_PORT,
            screens: screens.iter().map(|&(n, c, r)| ScreenPos { name: n.into(), col: c, row: r }).collect(),
            ..Settings::for_host("pc").server
        }
    }

    #[test]
    fn neighbours_in_the_grid_become_links() {
        // tablet
        // pc  laptop
        let s = server(&[("pc", 2, 1), ("laptop", 3, 1), ("tablet", 2, 0)]);
        let layout = s.validate().unwrap();
        assert_eq!(layout.neighbor("pc", Edge::Right), Some("laptop"));
        assert_eq!(layout.neighbor("laptop", Edge::Left), Some("pc"));
        assert_eq!(layout.neighbor("pc", Edge::Top), Some("tablet"));
        assert_eq!(layout.neighbor("tablet", Edge::Bottom), Some("pc"));
        assert_eq!(layout.neighbor("laptop", Edge::Top), None);
        assert!(s.isolated().is_empty());
    }

    #[test]
    fn diagonal_screens_are_not_linked() {
        let s = server(&[("pc", 3, 2), ("laptop", 4, 1)]);
        assert!(s.links().is_empty());
        assert_eq!(s.isolated(), vec!["pc".to_string(), "laptop".to_string()]);
    }

    #[test]
    fn invalid_layouts_are_rejected() {
        assert!(server(&[("pc", 3, 2), ("pc", 4, 2)]).validate().is_err());
        assert!(server(&[("pc", 3, 2), ("laptop", 3, 2)]).validate().is_err());
        let mut missing = server(&[("pc", 3, 2)]);
        missing.name = "anders".into();
        assert!(missing.validate().is_err());
    }

    #[test]
    fn switch_keys_only_apply_to_screens_in_the_layout() {
        let mut s = server(&[("pc", 3, 2), ("laptop", 4, 2)]);
        let switch = &mut s.keys.switch;
        switch.directions.insert("right".into(), "Ctrl+Alt+ArrowRight".into());
        switch.screens.insert("laptop".into(), "Ctrl+Alt+Digit2".into());
        switch.screens.insert("entfernt".into(), "Ctrl+Alt+Digit3".into());
        let targets: Vec<String> =
            s.effective_keys().config().unwrap().options.switch.iter().map(|(_, t)| t.to_string()).collect();
        assert_eq!(targets, ["right", "laptop"], "Rechner, die nicht mehr im Layout stehen, fallen weg");

        let file: toml::Table = toml::from_str(&s.to_toml(0)).unwrap();
        let entries: std::collections::HashMap<String, String> =
            file["keys"]["switch"].clone().try_into().expect("[keys.switch] als Tabelle");
        let parsed = qross_core::keys::parse_switch_keys(&entries).unwrap();
        assert_eq!(parsed.len(), 2, "die CLI liest dieselben Kürzel: {parsed:?}");
    }

    #[test]
    fn exported_toml_matches_the_cli_format() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ServerFile {
            name: String,
            listen: SocketAddr,
            discovery: bool,
            switching: Switching,
            clipboard: ClipboardSection,
            keys: KeysSection,
            links: Vec<LinkConfig>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct KeysSection {
            lock: String,
            media: qross_core::keys::MediaKeys,
            #[serde(default)]
            remap: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Switching {
            corner_size: u32,
            delay_ms: u64,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ClipboardSection {
            max_file_mb: u64,
        }
        let mut s = server(&[("pc", 3, 2), ("lap\"top", 4, 2)]);
        s.corner_size = 12;
        s.switch_delay_ms = 150;
        s.discovery = false;
        s.keys.lock = "Ctrl+Alt+KeyL".into();
        s.keys.media = qross_core::keys::MediaKeys::Local;
        s.keys.remap.insert(
            "lap\"top".into(),
            vec![crate::keys::KeyRule { from: "AltLeft".into(), to: "MetaLeft".into() }],
        );
        let file: ServerFile = toml::from_str(&s.to_toml(250)).unwrap();
        assert_eq!(file.keys.lock, "Ctrl+Alt+KeyL");
        assert_eq!(file.keys.media, qross_core::keys::MediaKeys::Local);
        let remap = qross_core::keys::parse_key_map(&file.keys.remap["lap\"top"]).unwrap();
        assert_eq!(remap.len(), 1, "die CLI liest die Umbelegung");
        assert_eq!(file.clipboard.max_file_mb, 250);
        assert_eq!(file.name, "pc");
        assert!(!file.discovery);
        assert_eq!((file.switching.corner_size, file.switching.delay_ms), (12, 150));
        assert_eq!(file.listen.port(), DEFAULT_PORT);
        assert_eq!(file.links, s.links());
        assert_eq!(file.links[0].to, "lap\"top");
    }

    #[test]
    fn new_screens_go_next_to_the_server() {
        let mut s = Settings::for_host("pc").server;
        assert!(s.add_screen("laptop"));
        assert!(!s.add_screen("laptop"));
        assert!(s.add_screen("tablet"));
        let layout = s.validate().unwrap();
        assert_eq!(layout.neighbor("pc", Edge::Right), Some("laptop"));
        assert_eq!(layout.neighbor("pc", Edge::Left), Some("tablet"));
    }

    #[test]
    fn screens_outside_the_grid_are_moved_inside() {
        let mut s = server(&[("pc", 2, 1), ("laptop", 6, 4)]);
        s.fit_grid();
        assert_eq!(s.screens[1], ScreenPos { name: "laptop".into(), col: 3, row: 1 });
    }

    #[test]
    fn settings_roundtrip_and_tolerate_missing_fields() {
        let dir = std::env::temp_dir().join(format!("qross-gui-test-{}", std::process::id()));
        assert!(Settings::load(&dir).unwrap().is_none());
        let mut settings = Settings::for_host("pc");
        settings.role = Role::Client;
        settings.client.address = "192.168.1.10".into();
        settings.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir).unwrap(), Some(settings));
        let _ = fs::remove_dir_all(&dir);

        let partial: Settings = serde_json::from_str(r#"{"role":"client"}"#).unwrap();
        assert_eq!(partial.role, Role::Client);
        assert_eq!(partial.server.port, DEFAULT_PORT);

        // Ältere gui.json ohne Wechsel-Optionen bekommt die Empfehlung
        let old: Settings = serde_json::from_str(r#"{"server":{"name":"pc","port":24900,"screens":[]}}"#).unwrap();
        assert_eq!(old.server.switch_options(), SwitchOptions::RECOMMENDED);
        assert!(old.server.discovery);
        // … und die Standardgrenze für kopierte Dateien (1 GB)
        assert_eq!(old.clipboard_files_mb, 1000);
        // … und Fortsetzen sowie automatische Updates sind an
        assert!(old.resume_role && old.auto_update);
        // … und die Tasten-Standards des Servers (Rollen, Medientasten an den aktiven Rechner)
        assert_eq!(old.server.keys, crate::keys::KeySettings::default());
        assert_eq!(old.server.keys.config().unwrap(), qross_core::keys::KeyConfig::default());
        assert_eq!(old.clipboard_file_limit(), qross_net::clipfiles::DEFAULT_LIMIT);
    }
}
