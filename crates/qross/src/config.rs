use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use qross_core::keys::{parse_key_map, parse_switch_keys, Hotkey, KeyConfig, KeyMode, KeyOptions, MediaKeys};
use qross_core::server::SwitchOptions;
use qross_core::sharing::{Share, Sharing, SharingRules};
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
    #[serde(default)]
    pub game: GameSection,
    #[serde(default)]
    pub sharing: SharingSection,
    #[serde(default)]
    pub screensaver: ScreenSaverSection,
}

/// Abschnitt `[screensaver]`: den Bildschirmschoner mit den Clients abgleichen.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ScreenSaverSection {
    /// Beginnt er hier, starten die Clients ihren; endet er, beenden sie ihn.
    #[serde(default = "yes")]
    pub sync: bool,
    /// Beim Beginn sperren sich die Clients stattdessen (entsperrt wird an ihrer eigenen Tastatur).
    #[serde(default)]
    pub lock_clients: bool,
}

impl Default for ScreenSaverSection {
    fn default() -> Self {
        Self { sync: true, lock_clients: false }
    }
}

/// Abschnitt `[sharing]`: Rechte je Rechner. Die Werte direkt darunter gelten für jeden Rechner,
/// soweit `[sharing.clients.<name>]` nichts anderes sagt; ohne Angabe ist alles erlaubt.
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct SharingSection {
    pub clipboard: Option<Share>,
    pub files: Option<Share>,
    pub drag: Option<Share>,
    /// Name des Rechners → eigene Rechte
    #[serde(default)]
    pub clients: HashMap<String, ShareEntries>,
}

/// Was ein Rechner teilen darf: `"both"`, `"to-client"`, `"from-client"` oder `"off"`
#[derive(Deserialize, Debug, Default, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct ShareEntries {
    /// Zwischenablage: Text, HTML, Bild
    pub clipboard: Option<Share>,
    /// Kopierte Dateien
    pub files: Option<Share>,
    /// Drag & Drop
    pub drag: Option<Share>,
}

impl ShareEntries {
    fn over(self, base: Sharing) -> Sharing {
        Sharing {
            clipboard: self.clipboard.unwrap_or(base.clipboard),
            files: self.files.unwrap_or(base.files),
            drag: self.drag.unwrap_or(base.drag),
        }
    }
}

impl SharingSection {
    pub fn rules(&self) -> SharingRules {
        let section = ShareEntries { clipboard: self.clipboard, files: self.files, drag: self.drag };
        let default = section.over(Sharing::ALL);
        let clients = self.clients.iter().map(|(name, entries)| (name.clone(), entries.over(default))).collect();
        SharingRules { default, clients }
    }
}

/// Abschnitt `[game]`: der Spielemodus.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct GameSection {
    /// Bei Vollbild bzw. eingefangenem Zeiger auf dem Rechner mit dem Cursor festhalten und
    /// Clients dort relative Bewegungen schicken
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl Default for GameSection {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Abschnitt `[keys]`: Fang-Taste, Wechsel-Kürzel, Medientasten, Umbelegung und Tastenposition
/// oder Zeichen je Rechner.
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
    /// Name des Rechners → „position“ (Standard) oder „chars“
    #[serde(default)]
    pub mode: HashMap<String, KeyMode>,
}

impl Default for KeysSection {
    fn default() -> Self {
        Self {
            lock: default_lock(),
            media: MediaKeys::default(),
            remap: HashMap::new(),
            switch: HashMap::new(),
            mode: HashMap::new(),
        }
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
        Ok(KeyConfig { options, remap, modes: self.mode.clone() })
    }
}

/// Abschnitt `[clipboard]`: kopierte Dateien und vertrauliche Inhalte.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ClipboardSection {
    /// Kopierte Dateien bis zu dieser Gesamtgröße in MB beim Wechsel mitgeben (0 = nie)
    #[serde(default = "default_max_file_mb")]
    pub max_file_mb: u64,
    /// Inhalte, die ein Passwortmanager als vertraulich markiert, trotzdem teilen
    #[serde(default)]
    pub share_concealed: bool,
}

impl Default for ClipboardSection {
    fn default() -> Self {
        Self { max_file_mb: default_max_file_mb(), share_concealed: false }
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
        assert!(!file.clipboard.share_concealed);
        let keys = file.keys.config().unwrap();
        assert_eq!(keys.options.lock, Some(Hotkey::SCROLL_LOCK));
        assert_eq!(keys.options.media, MediaKeys::Active);
        assert_eq!(keys.remap["laptop"].len(), 1, "Beispiel: Alt und Windows-Taste am Mac");
        assert!(file.game.enabled);
    }

    #[test]
    fn screen_saver_is_synced_without_locking_unless_configured() {
        let file: ServerFile = toml::from_str("").unwrap();
        assert!(file.screensaver.sync && !file.screensaver.lock_clients);
        let file: ServerFile = toml::from_str("[screensaver]\nlock_clients = true").unwrap();
        assert!(file.screensaver.sync && file.screensaver.lock_clients);
        let file: ServerFile = toml::from_str("[screensaver]\nsync = false").unwrap();
        assert!(!file.screensaver.sync);
        assert!(toml::from_str::<ServerFile>("[screensaver]\nlock = true").is_err(), "Tippfehler fallen auf");
    }

    #[test]
    fn game_mode_is_on_unless_switched_off() {
        let file: ServerFile = toml::from_str("name = \"pc\"").unwrap();
        assert!(file.game.enabled);
        let file: ServerFile = toml::from_str("[game]\nenabled = false").unwrap();
        assert!(!file.game.enabled);
        assert!(toml::from_str::<ServerFile>("[game]\nan = true").is_err(), "Tippfehler fallen auf");
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
    fn switch_keys_are_checked() {
        let example: ServerFile = toml::from_str(include_str!("../../../qross.example.toml")).unwrap();
        let switch: Vec<String> = example.keys.config().unwrap().options.switch.iter().map(|(h, t)| format!("{h} = {t}")).collect();
        assert_eq!(switch, ["Ctrl+Alt+ArrowLeft = left", "Ctrl+Alt+ArrowRight = right", "Ctrl+Alt+Digit1 = pc", "Ctrl+Alt+Digit2 = laptop"]);

        let file: ServerFile = toml::from_str("[keys.switch]\n\"Ctrl+Alt+Digit1\" = \"laptop\"\n\"Ctrl+Alt+ArrowUp\" = \"up\"").unwrap();
        let switch = file.keys.config().unwrap().options.switch;
        assert_eq!(switch[0], ("Ctrl+Alt+ArrowUp".parse().unwrap(), SwitchTarget::Direction(Edge::Top)));
        assert_eq!(switch[1].1, SwitchTarget::Screen("laptop".into()));

        let error = |text: &str| toml::from_str::<ServerFile>(text).unwrap().keys.config().unwrap_err().to_string();
        assert!(error("[keys.switch]\n\"Ctrl+Gibtsnicht\" = \"left\"").contains("[keys.switch]"));
        assert!(error("[keys.switch]\n\"Ctrl+Alt+KeyL\" = \"\"").contains("Ziel fehlt"));
        let lock = error("[keys]\nlock = \"Ctrl+Alt+KeyL\"\n[keys.switch]\n\"Alt+Ctrl+KeyL\" = \"laptop\"");
        assert!(lock.contains("schon die Fang-Taste"), "{lock}");
        let twice = error("[keys.switch]\n\"Ctrl+Alt+KeyL\" = \"laptop\"\n\"Alt+Ctrl+KeyL\" = \"left\"");
        assert!(twice.contains("schon belegt"), "{twice}");
        assert!(error("[keys.switch]\n\"Ctrl+Alt+Shift+Escape\" = \"pc\"").contains("Notausgang"));
    }

    #[test]
    fn key_modes_are_read_per_machine() {
        let example: ServerFile = toml::from_str(include_str!("../../../qross.example.toml")).unwrap();
        assert!(example.keys.config().unwrap().modes.is_empty(), "Standard: Tastenposition");
        let file: ServerFile = toml::from_str("[keys.mode]\nlaptop = \"chars\"\nmac = \"position\"").unwrap();
        let modes = file.keys.config().unwrap().modes;
        assert_eq!((modes["laptop"], modes["mac"]), (KeyMode::Chars, KeyMode::Position));
        let wrong = toml::from_str::<ServerFile>("[keys.mode]\nlaptop = \"zeichen\"").unwrap_err().to_string();
        assert!(wrong.contains("chars"), "{wrong}");
    }

    #[test]
    fn switching_defaults_apply_when_section_is_missing() {
        let file: ServerFile = toml::from_str("name = \"pc\"").unwrap();
        assert_eq!(file.switching.options(), SwitchOptions::RECOMMENDED);
        assert_eq!(file.clipboard.file_limit(), qross_net::clipfiles::DEFAULT_LIMIT);
    }

    #[test]
    fn sharing_falls_back_to_the_section_and_then_to_everything() {
        let example: ServerFile = toml::from_str(include_str!("../../../qross.example.toml")).unwrap();
        assert!(!example.sharing.rules().restricts(), "das Beispiel schränkt nichts ein");
        let file: ServerFile = toml::from_str("name = \"pc\"").unwrap();
        assert_eq!(file.sharing.rules(), SharingRules::default());

        let text = "[sharing]\ndrag = \"off\"\n\n[sharing.clients.laptop]\nclipboard = \"to-client\"\nfiles = \"from-client\"\n";
        let rules = toml::from_str::<ServerFile>(text).unwrap().sharing.rules();
        assert_eq!(rules.default, Sharing { drag: Share::Off, ..Sharing::ALL });
        let laptop = Sharing { clipboard: Share::ToClient, files: Share::FromClient, drag: Share::Off };
        assert_eq!(rules.for_client("laptop"), laptop, "fehlende Angaben kommen aus [sharing]");
        assert_eq!(rules.for_client("tablet"), rules.default);

        assert!(toml::from_str::<ServerFile>("[sharing]\nclipboard = \"beide\"").is_err(), "nur bekannte Werte");
        assert!(toml::from_str::<ServerFile>("[sharing]\naudio = \"off\"").is_err(), "Tippfehler fallen auf");
        assert!(toml::from_str::<ServerFile>("[sharing.clients.laptop]\naudio = \"off\"").is_err());
    }

    #[test]
    fn copied_files_can_be_switched_off() {
        let file: ServerFile = toml::from_str("[clipboard]\nmax_file_mb = 0").unwrap();
        assert_eq!(file.clipboard.file_limit(), 0);
    }

    #[test]
    fn concealed_content_stays_unless_shared_explicitly() {
        let file: ServerFile = toml::from_str("name = \"pc\"").unwrap();
        assert!(!file.clipboard.share_concealed);
        let file: ServerFile = toml::from_str("[clipboard]\nshare_concealed = true").unwrap();
        assert!(file.clipboard.share_concealed);
        assert_eq!(file.clipboard.file_limit(), qross_net::clipfiles::DEFAULT_LIMIT);
    }
}
