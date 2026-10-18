//! Profile je Ort, z. B. „Büro“ und „Zuhause“: Rolle, Anordnung und Server-Adresse wechseln
//! gemeinsam, von Hand oder automatisch nach Netzwerk. Reine Logik ohne Tauri; die Abfrage des
//! Netzwerks steckt in [`qross_platform::network`].
//!
//! Die Werte des aktiven Profils stehen wie bisher in `gui.json` (siehe [`Settings`]), alle Profile in
//! `profiles.json`. So lädt eine ältere Version `gui.json` unverändert und lässt die Profile in Ruhe.
//! Beim Laden gelten die Werte aus `gui.json` als die des aktiven Profils, auch wenn eine ältere
//! Version sie zwischendurch geändert hat.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{Role, ScreenPos, Settings};

/// Alle Profile, im Datenverzeichnis neben `gui.json`
pub const PROFILES_FILE: &str = "profiles.json";
/// In dieses Profil wandern die bestehenden Einstellungen beim ersten Start mit Profilen.
pub const FIRST_PROFILE: &str = "Standard";
/// Längster erlaubter Profilname (Tray-Menü, Kopfzeile)
pub const MAX_NAME: usize = 40;

/// Was je Ort gilt: alles, was im Hauptfenster eingestellt wird. Alles unter „Einstellungen“
/// (Autostart, Updates, Tastatur, Spielemodus, Zwischenablage) gilt für alle Profile.
///
/// Eine neue Einstellung je Ort hier eintragen; [`Place::capture`] und [`Place::apply`] lassen
/// sich erst wieder übersetzen, wenn sie dort ebenfalls steht.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct Place {
    pub role: Role,
    pub server: ServerPlace,
    pub client: ClientPlace,
}

/// Server je Ort: Name im Layout, Port, Anordnung und Wechsel (ohne Tastatur, die gilt überall)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct ServerPlace {
    pub name: String,
    pub port: u16,
    pub screens: Vec<ScreenPos>,
    pub corner_size: u32,
    pub switch_delay_ms: u64,
    pub discovery: bool,
}

/// Client je Ort: Server-Adresse und der Name, unter dem dieser Rechner dort im Layout steht
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct ClientPlace {
    pub address: String,
    pub name: String,
    pub dry_run: bool,
}

impl Default for Place {
    fn default() -> Self {
        Self::capture(&Settings::default())
    }
}

impl Default for ServerPlace {
    fn default() -> Self {
        Place::default().server
    }
}

impl Default for ClientPlace {
    fn default() -> Self {
        Place::default().client
    }
}

impl Place {
    /// Die Werte je Ort aus den Einstellungen
    pub fn capture(s: &Settings) -> Self {
        Self {
            role: s.role,
            server: ServerPlace {
                name: s.server.name.clone(),
                port: s.server.port,
                screens: s.server.screens.clone(),
                corner_size: s.server.corner_size,
                switch_delay_ms: s.server.switch_delay_ms,
                discovery: s.server.discovery,
            },
            client: ClientPlace { address: s.client.address.clone(), name: s.client.name.clone(), dry_run: s.client.dry_run },
        }
    }

    /// Setzt die Werte je Ort in die Einstellungen; alles andere bleibt.
    pub fn apply(&self, s: &mut Settings) {
        let Place {
            role,
            server: ServerPlace { name, port, screens, corner_size, switch_delay_ms, discovery },
            client: ClientPlace { address, name: client_name, dry_run },
        } = self.clone();
        s.role = role;
        (s.server.name, s.server.port, s.server.screens) = (name, port, screens);
        (s.server.corner_size, s.server.switch_delay_ms, s.server.discovery) = (corner_size, switch_delay_ms, discovery);
        (s.client.address, s.client.name, s.client.dry_run) = (address, client_name, dry_run);
        s.server.fit_grid();
    }

    /// Startwerte für ein neues Profil: dieser Rechner allein als Server, unter den bisherigen Namen.
    pub fn fresh(current: &Settings) -> Self {
        let name = current.server.name.trim();
        let mut fresh = Settings::for_host(if name.is_empty() { "dieser-pc" } else { name });
        fresh.client.name = current.client.name.clone();
        Self::capture(&fresh)
    }
}

/// Die Werte je Ort gehören zu dem Profil, für das das Fenster sie bearbeitet hat. Hat inzwischen
/// ein anderes Profil übernommen (das Fenster speichert verzögert, ein automatischer Wechsel kann
/// dazwischenkommen), gelten aus `incoming` nur die übrigen Einstellungen.
pub fn rebase(mut incoming: Settings, current: &Settings) -> Settings {
    if incoming.profile != current.profile {
        Place::capture(current).apply(&mut incoming);
        incoming.profile = current.profile.clone();
    }
    incoming
}

/// Wie gut ein gespeichertes Netzwerk zum aktuellen passt
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Match {
    /// Gleiche Gateway-Adresse im gleichen Subnetz, MAC unbekannt (z. B. VPN)
    Address,
    /// Gleiches Gateway (MAC-Adresse): eindeutig, auch wenn zwei Router dieselbe IP haben
    Gateway,
}

/// Merkmale eines Netzwerks, an das ein Profil gebunden ist, bzw. des aktuellen Netzwerks
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(default)]
pub struct NetworkMark {
    /// Zur Anzeige: Name des Netzwerks, sonst das Subnetz
    pub label: String,
    /// IPv4-Adresse des Gateways
    pub gateway: String,
    /// MAC-Adresse des Gateways, z. B. `3c:a6:2f:0a:1b:2c`
    pub gateway_mac: Option<String>,
    /// Eigenes Subnetz, z. B. `192.168.178.0/24`
    pub subnet: Option<String>,
    /// Netzwerkkarte, nur zur Anzeige
    pub interface: String,
}

impl NetworkMark {
    pub fn of(network: &qross_platform::network::Network) -> Self {
        let subnet = network.subnet();
        let label = network
            .name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| subnet.clone())
            .unwrap_or_else(|| format!("Gateway {}", network.gateway));
        Self {
            label,
            gateway: network.gateway.to_string(),
            gateway_mac: network.gateway_mac.map(|mac| mac.to_string()),
            subnet,
            interface: network.interface.clone(),
        }
    }

    /// Passt dieses (gespeicherte) Netzwerk zum aktuellen? Kennen beide die MAC des Gateways,
    /// entscheidet allein sie: Viele Router haben ab Werk dieselbe Adresse (z. B. 192.168.178.1).
    pub fn matches(&self, current: &NetworkMark) -> Option<Match> {
        if self.gateway.is_empty() {
            return None;
        }
        match (&self.gateway_mac, &current.gateway_mac) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b).then_some(Match::Gateway),
            _ => {
                let same_subnet = match (&self.subnet, &current.subnet) {
                    (Some(a), Some(b)) => a == b,
                    _ => true,
                };
                (self.gateway == current.gateway && same_subnet).then_some(Match::Address)
            }
        }
    }
}

/// Kennung der aktuellen Netzwerke, um Wechsel zu erkennen (ohne Namen, die nicht jede Abfrage liefert)
pub fn signature(networks: &[NetworkMark]) -> String {
    let mut parts: Vec<String> = networks
        .iter()
        .map(|n| {
            let mac = n.gateway_mac.as_deref().unwrap_or("-");
            format!("{}|{}|{}", n.gateway, mac.to_ascii_lowercase(), n.subnet.as_deref().unwrap_or("-"))
        })
        .collect();
    parts.sort();
    parts.dedup();
    parts.join(";")
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Automatisch aktivieren, wenn eines der Netzwerke erkannt wird
    pub auto: bool,
    pub networks: Vec<NetworkMark>,
    /// Werte je Ort. Beim aktiven Profil der Stand beim letzten Speichern; maßgeblich ist dann `gui.json`.
    pub place: Place,
}

impl Default for Profile {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), auto: true, networks: Vec::new(), place: Place::default() }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(default)]
pub struct Profiles {
    /// Profil, dessen Werte gerade gelten (wie `Settings::profile`)
    pub active: String,
    /// Profil automatisch nach Netzwerk wählen
    pub auto: bool,
    /// Passt kein Profil zum Netzwerk, dieses aktivieren; leer: das aktuelle behalten
    pub fallback: String,
    pub list: Vec<Profile>,
}

impl Default for Profiles {
    fn default() -> Self {
        Self { active: String::new(), auto: true, fallback: String::new(), list: Vec::new() }
    }
}

/// Ergebnis der automatischen Wahl
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Beim aktuellen Profil bleiben
    Keep,
    /// Dieses Profil passt zum Netzwerk
    Network(String),
    /// Kein Profil passt; das Profil für unbekannte Netzwerke
    Fallback(String),
}

/// Änderungen aus den Einstellungen der Oberfläche (aktivieren geht über die Laufzeit)
#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ProfileEdit {
    /// Neues Profil mit Startwerten: dieser Rechner allein als Server
    Create { name: String },
    /// Kopie mit allen Werten je Ort, aber ohne Netzwerke
    Duplicate { id: String },
    Rename { id: String, name: String },
    /// Löschen; das aktive Profil nicht
    Remove { id: String },
    /// Das Profil automatisch aktivieren, wenn eines seiner Netzwerke erkannt wird
    SetAuto { id: String, auto: bool },
    /// Automatische Wahl insgesamt ein- oder ausschalten
    SetAutomatic { enabled: bool },
    /// Profil für unbekannte Netzwerke; leer: das aktuelle behalten
    SetFallback { id: String },
    /// Netzwerk zuordnen; ein anderes Profil verliert es dabei
    AddNetwork { id: String, network: NetworkMark },
    RemoveNetwork { id: String, index: usize },
}

impl Profiles {
    /// Lädt `profiles.json`; `None`, wenn es die Datei noch nicht gibt.
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(PROFILES_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("{} lesen", path.display()))?;
        let profiles = serde_json::from_str(&text).with_context(|| format!("{} ist ungültig", path.display()))?;
        Ok(Some(profiles))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir).with_context(|| format!("{} anlegen", dir.display()))?;
        let path = dir.join(PROFILES_FILE);
        fs::write(&path, serde_json::to_string_pretty(self)?).with_context(|| format!("{} schreiben", path.display()))
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.list.iter().find(|p| p.id == id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut Profile> {
        self.list.iter_mut().find(|p| p.id == id)
    }

    pub fn active_profile(&self) -> Option<&Profile> {
        self.get(&self.active)
    }

    /// Beim Start: Profile und geladene Einstellungen zusammenführen. Ohne Profile (erster Start mit
    /// dieser Version) wandern die Einstellungen verlustfrei in „Standard“. Sonst gehören die Werte
    /// aus `gui.json` zu `Settings::profile`; fehlt das (eine ältere Version hat gespeichert), zum
    /// zuletzt aktiven Profil. Liefert `true`, wenn sich etwas geändert hat und gespeichert werden sollte.
    pub fn attach(&mut self, settings: &mut Settings) -> bool {
        let before = (self.clone(), settings.profile.clone());
        self.repair();
        if self.list.is_empty() {
            let id = self.new_id();
            let name = FIRST_PROFILE.to_owned();
            self.list.push(Profile { id: id.clone(), name, place: Place::capture(settings), ..Profile::default() });
            // In Netzwerken ohne eigenes Profil gilt das bisherige.
            self.fallback = id.clone();
            self.active = id;
        }
        let target = [settings.profile.as_str(), self.active.as_str()]
            .into_iter()
            .find(|id| self.get(id).is_some())
            .unwrap_or(self.list[0].id.as_str())
            .to_owned();
        self.active = target.clone();
        settings.profile = target;
        self.sync(settings);
        before != (self.clone(), settings.profile.clone())
    }

    /// Fehlt `gui.json` oder ist sie unlesbar, kommen die Werte je Ort aus dem zuletzt aktiven Profil
    /// statt aus den Startwerten (die sonst beim [`Profiles::attach`] das Profil überschrieben).
    pub fn restore(&self, settings: &mut Settings) {
        if let Some(profile) = self.active_profile() {
            profile.place.apply(settings);
            settings.profile = profile.id.clone();
        }
    }

    /// Kennungen eindeutig und nicht leer, Namen nicht leer, Verweise gültig (z. B. von Hand bearbeitet).
    fn repair(&mut self) {
        let mut seen = HashSet::new();
        let duplicate: Vec<usize> =
            (0..self.list.len()).filter(|&i| self.list[i].id.is_empty() || !seen.insert(self.list[i].id.clone())).collect();
        for i in duplicate {
            self.list[i].id = self.new_id();
        }
        for i in 0..self.list.len() {
            if self.list[i].name.trim().is_empty() {
                let name = (1..).map(|n| if n == 1 { "Profil".to_owned() } else { format!("Profil {n}") });
                self.list[i].name = self.free_name(name);
            }
        }
        if self.get(&self.fallback).is_none() {
            self.fallback.clear();
        }
    }

    fn new_id(&self) -> String {
        (1..).map(|n| format!("p{n}")).find(|id| self.get(id).is_none()).expect("freie Kennung")
    }

    fn taken(&self, name: &str, except: Option<&str>) -> bool {
        self.list.iter().any(|p| Some(p.id.as_str()) != except && p.name.trim().to_lowercase() == name.trim().to_lowercase())
    }

    /// Der erste noch nicht vergebene Name
    fn free_name(&self, mut candidates: impl Iterator<Item = String>) -> String {
        candidates.find(|name| !self.taken(name, None)).expect("freier Name")
    }

    /// „Büro (Kopie)“, „Büro (Kopie 2)“ …, gekürzt auf die erlaubte Länge
    fn copy_name(&self, name: &str) -> String {
        self.free_name((1..).map(|n| {
            let suffix = if n == 1 { " (Kopie)".to_owned() } else { format!(" (Kopie {n})") };
            let base: String = name.chars().take(MAX_NAME.saturating_sub(suffix.chars().count())).collect();
            format!("{}{suffix}", base.trim_end())
        }))
    }

    fn checked_name(&self, name: &str, except: Option<&str>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Das Profil braucht einen Namen.".into());
        }
        if name.chars().count() > MAX_NAME {
            return Err(format!("Der Name ist zu lang (höchstens {MAX_NAME} Zeichen)."));
        }
        if self.taken(name, except) {
            return Err(format!("Ein Profil „{name}“ gibt es schon."));
        }
        Ok(name.to_owned())
    }

    fn require(&self, id: &str) -> Result<&Profile, String> {
        self.get(id).ok_or_else(|| "Dieses Profil gibt es nicht mehr.".to_owned())
    }

    /// Übernimmt die Werte je Ort aus den Einstellungen ins aktive Profil; `true` bei einer Änderung.
    pub fn sync(&mut self, settings: &Settings) -> bool {
        if settings.profile != self.active {
            return false;
        }
        let place = Place::capture(settings);
        match self.get_mut(&settings.profile) {
            Some(profile) if profile.place != place => {
                profile.place = place;
                true
            }
            _ => false,
        }
    }

    /// Wechselt das Profil: Werte des bisherigen sichern, die des neuen in die Einstellungen setzen.
    pub fn activate(&mut self, id: &str, settings: &mut Settings) -> Result<(), String> {
        let place = self.require(id)?.place.clone();
        self.sync(settings);
        place.apply(settings);
        settings.profile = id.to_owned();
        self.active = id.to_owned();
        Ok(())
    }

    /// Wählt Qross überhaupt automatisch? Nur, wenn eingeschaltet und ein Profil Netzwerke hat.
    pub fn automatic(&self) -> bool {
        self.auto && self.list.iter().any(|p| p.auto && !p.networks.is_empty())
    }

    /// Wie gut passt das Profil zu den aktuellen Netzwerken (nur Profile mit Automatik)
    fn strength(profile: &Profile, current: &[NetworkMark]) -> Option<Match> {
        if !profile.auto {
            return None;
        }
        profile.networks.iter().flat_map(|n| current.iter().filter_map(|c| n.matches(c))).max()
    }

    /// Profil für die aktuellen Netzwerke. Das aktive bleibt, solange kein anderes besser passt;
    /// ohne Netzwerk (offline) ändert sich nichts.
    pub fn choose(&self, current: &[NetworkMark]) -> Choice {
        if !self.automatic() || current.is_empty() {
            return Choice::Keep;
        }
        let best = self.list.iter().filter_map(|p| Self::strength(p, current)).max();
        let Some(best) = best else {
            let fallback = &self.fallback;
            return if !fallback.is_empty() && *fallback != self.active && self.get(fallback).is_some() {
                Choice::Fallback(fallback.clone())
            } else {
                Choice::Keep
            };
        };
        if self.active_profile().and_then(|p| Self::strength(p, current)) == Some(best) {
            return Choice::Keep;
        }
        let profile = self.list.iter().find(|p| Self::strength(p, current) == Some(best)).expect("bestes Profil");
        Choice::Network(profile.id.clone())
    }

    /// Das aktuelle Netzwerk, über das ein Profil gewählt wurde (für die Benachrichtigung)
    pub fn matched<'a>(&self, id: &str, current: &'a [NetworkMark]) -> Option<&'a NetworkMark> {
        let profile = self.get(id)?;
        let best = Self::strength(profile, current)?;
        current.iter().find(|c| profile.networks.iter().any(|n| n.matches(c) == Some(best)))
    }

    /// Führt eine Änderung aus den Einstellungen aus. `current` liefert die Werte je Ort des aktiven
    /// Profils (für „Neu“ und „Duplizieren“). `Ok(Some(text))`: Hinweis für die Oberfläche.
    pub fn edit(&mut self, edit: ProfileEdit, current: &Settings) -> Result<Option<String>, String> {
        self.sync(current);
        match edit {
            ProfileEdit::Create { name } => {
                let name = self.checked_name(&name, None)?;
                let id = self.new_id();
                self.list.push(Profile { id, name, place: Place::fresh(current), ..Profile::default() });
            }
            ProfileEdit::Duplicate { id } => {
                let source = self.require(&id)?.clone();
                let name = self.copy_name(&source.name);
                let copy = Profile { id: self.new_id(), name, auto: source.auto, networks: Vec::new(), place: source.place };
                self.list.push(copy);
            }
            ProfileEdit::Rename { id, name } => {
                let name = self.checked_name(&name, Some(&id))?;
                self.require(&id)?;
                self.get_mut(&id).expect("geprüft").name = name;
            }
            ProfileEdit::Remove { id } => {
                self.require(&id)?;
                if id == self.active {
                    return Err("Das aktive Profil lässt sich nicht löschen. Wechsle vorher zu einem anderen.".into());
                }
                self.list.retain(|p| p.id != id);
                if self.fallback == id {
                    self.fallback.clear();
                }
            }
            ProfileEdit::SetAuto { id, auto } => {
                self.require(&id)?;
                self.get_mut(&id).expect("geprüft").auto = auto;
            }
            ProfileEdit::SetAutomatic { enabled } => self.auto = enabled,
            ProfileEdit::SetFallback { id } => {
                if !id.is_empty() {
                    self.require(&id)?;
                }
                self.fallback = id;
            }
            ProfileEdit::AddNetwork { id, network } => {
                self.require(&id)?;
                if network.gateway.is_empty() {
                    return Err("Kein Netzwerk erkannt.".into());
                }
                let mut moved = None;
                for profile in &mut self.list {
                    let before = profile.networks.len();
                    profile.networks.retain(|n| n.matches(&network).is_none());
                    if profile.id != id && profile.networks.len() < before {
                        moved = Some(profile.name.clone());
                    }
                }
                let label = network.label.clone();
                self.get_mut(&id).expect("geprüft").networks.push(network);
                if let Some(other) = moved {
                    return Ok(Some(format!("„{label}“ gehörte bisher zum Profil „{other}“.")));
                }
            }
            ProfileEdit::RemoveNetwork { id, index } => {
                let profile = self.get_mut(&id).ok_or("Dieses Profil gibt es nicht mehr.")?;
                if index >= profile.networks.len() {
                    return Err("Dieses Netzwerk ist nicht mehr zugeordnet.".into());
                }
                profile.networks.remove(index);
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(gateway: &str, mac: Option<&str>, subnet: &str) -> NetworkMark {
        NetworkMark {
            label: subnet.into(),
            gateway: gateway.into(),
            gateway_mac: mac.map(Into::into),
            subnet: Some(subnet.into()),
            interface: "WLAN".into(),
        }
    }

    const HOME_MAC: &str = "3c:a6:2f:0a:1b:2c";
    const OFFICE_MAC: &str = "e8:1c:ba:f2:a6:b3";

    fn home_net() -> NetworkMark {
        mark("192.168.178.1", Some(HOME_MAC), "192.168.178.0/24")
    }
    /// Gleicher Router-Typ, gleiche Adressen, anderes Gerät
    fn office_net() -> NetworkMark {
        mark("192.168.178.1", Some(OFFICE_MAC), "192.168.178.0/24")
    }
    fn vpn_net() -> NetworkMark {
        mark("10.8.0.1", None, "10.8.0.0/24")
    }

    /// Zwei Profile: „Standard“ (aus den alten Einstellungen) und „Büro“ als Client
    fn setup() -> (Profiles, Settings) {
        let mut settings = Settings::for_host("pc");
        let mut profiles = Profiles::default();
        profiles.attach(&mut settings);
        profiles.edit(ProfileEdit::Create { name: "Büro".into() }, &settings).unwrap();
        let office_id = profiles.list[1].id.clone();
        profiles.activate(&office_id, &mut settings).unwrap();
        settings.role = Role::Client;
        settings.client.address = "10.1.2.3".into();
        profiles.sync(&settings);
        (profiles, settings)
    }

    fn id_of(profiles: &Profiles, name: &str) -> String {
        profiles.list.iter().find(|p| p.name == name).unwrap().id.clone()
    }

    #[test]
    fn existing_settings_move_into_a_first_profile_without_loss() {
        let mut settings = Settings::for_host("pc");
        settings.role = Role::Client;
        settings.client.address = "192.168.1.10".into();
        settings.server.add_screen("laptop");
        settings.server.corner_size = 12;
        settings.game_mode = false;
        let before = settings.clone();

        let mut profiles = Profiles::default();
        assert!(profiles.attach(&mut settings), "Migration soll gespeichert werden");
        assert_eq!(profiles.list.len(), 1);
        let first = &profiles.list[0];
        assert_eq!(first.name, FIRST_PROFILE);
        assert_eq!(first.place, Place::capture(&before));
        assert_eq!((profiles.active.as_str(), profiles.fallback.as_str()), (first.id.as_str(), first.id.as_str()));
        assert_eq!(settings.profile, first.id);
        settings.profile.clear();
        assert_eq!(settings, before, "nur die Zuordnung zum Profil kommt dazu");

        // Zweiter Start: nichts mehr zu tun
        let mut settings = before.clone();
        settings.profile = profiles.active.clone();
        assert!(!profiles.clone().attach(&mut settings));
    }

    #[test]
    fn values_from_gui_json_win_after_a_downgrade() {
        let (mut profiles, mut settings) = setup();
        let office = id_of(&profiles, "Büro");
        // Eine ältere Version hat gui.json ohne `profile` gespeichert und dabei die Adresse geändert.
        settings.profile.clear();
        settings.client.address = "10.9.9.9".into();
        assert!(profiles.attach(&mut settings));
        assert_eq!(settings.profile, office, "gehört zum zuletzt aktiven Profil");
        assert_eq!(profiles.get(&office).unwrap().place.client.address, "10.9.9.9");
        assert_eq!(profiles.list.len(), 2, "die übrigen Profile bleiben erhalten");
    }

    #[test]
    fn values_belong_to_the_profile_named_in_gui_json() {
        // Absturz zwischen dem Speichern beider Dateien: gui.json gehört schon zu „Standard“, profiles.json
        // nennt noch „Büro“ als aktiv. Die Werte dürfen „Büro“ nicht überschreiben.
        let (mut profiles, mut settings) = setup();
        let standard = id_of(&profiles, "Standard");
        let office_place = profiles.active_profile().unwrap().place.clone();
        profiles.get(&standard).unwrap().place.apply(&mut settings);
        settings.profile = standard.clone();
        profiles.attach(&mut settings);
        assert_eq!(profiles.active, standard);
        assert_eq!(profiles.get(&id_of(&profiles, "Büro")).unwrap().place, office_place);
    }

    #[test]
    fn a_missing_gui_json_is_restored_from_the_active_profile() {
        let (profiles, settings) = setup();
        let mut fresh = Settings::for_host("pc");
        fresh.game_mode = false;
        let mut loaded = profiles.clone();
        loaded.restore(&mut fresh);
        assert!(!loaded.attach(&mut fresh), "nichts überschrieben");
        assert_eq!(Place::capture(&fresh), Place::capture(&settings));
        assert_eq!((fresh.role, fresh.client.address.as_str()), (Role::Client, "10.1.2.3"));
        assert!(!fresh.game_mode, "Rechnerweites bleibt, wie es geladen wurde");
    }

    #[test]
    fn broken_profile_lists_are_repaired() {
        let mut profiles: Profiles = serde_json::from_str(
            r#"{"active":"x","fallback":"weg","list":[{"id":"","name":"A"},{"id":"p1","name":" "},{"id":"p1","name":"C"}]}"#,
        )
        .unwrap();
        let mut settings = Settings::for_host("pc");
        profiles.attach(&mut settings);
        let ids: HashSet<&str> = profiles.list.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "eindeutige Kennungen: {:?}", profiles.list);
        assert!(profiles.list.iter().all(|p| !p.name.trim().is_empty()));
        assert_eq!(profiles.fallback, "");
        assert_eq!(profiles.active, profiles.list[0].id, "unbekanntes aktives Profil: das erste");
        assert_eq!(settings.profile, profiles.active);
    }

    #[test]
    fn activating_keeps_the_values_of_both_profiles() {
        let (mut profiles, mut settings) = setup();
        let standard = id_of(&profiles, "Standard");
        let office = id_of(&profiles, "Büro");
        settings.game_mode = false; // gilt für alle Profile
        profiles.activate(&standard, &mut settings).unwrap();
        assert_eq!((settings.role, settings.profile.as_str()), (Role::Server, standard.as_str()));
        assert!(!settings.game_mode, "Rechnerweites bleibt");
        settings.server.add_screen("laptop");
        profiles.activate(&office, &mut settings).unwrap();
        assert_eq!((settings.role, settings.client.address.as_str()), (Role::Client, "10.1.2.3"));
        assert_eq!(settings.server.screens.len(), 1, "Anordnung von „Büro“");
        assert_eq!(profiles.get(&standard).unwrap().place.server.screens.len(), 2, "„laptop“ in „Standard“ gesichert");
        assert!(profiles.activate("gibt-es-nicht", &mut settings).is_err());
    }

    #[test]
    fn keys_stay_global_while_layouts_differ() {
        let (mut profiles, mut settings) = setup();
        settings.server.keys.lock = "Ctrl+Alt+KeyL".into();
        profiles.activate(&id_of(&profiles, "Standard"), &mut settings).unwrap();
        assert_eq!(settings.server.keys.lock, "Ctrl+Alt+KeyL");
    }

    #[test]
    fn delayed_saves_do_not_leak_into_another_profile() {
        let (mut profiles, mut settings) = setup();
        let stale = settings.clone(); // Fenster bearbeitet noch „Büro“
        profiles.activate(&id_of(&profiles, "Standard"), &mut settings).unwrap();
        let mut edited = stale;
        edited.client.address = "10.7.7.7".into();
        edited.clipboard_files_mb = 5;
        let saved = rebase(edited, &settings);
        assert_eq!(saved.client.address, settings.client.address, "Werte je Ort von „Standard“ bleiben");
        assert_eq!(saved.role, Role::Server);
        assert_eq!(saved.clipboard_files_mb, 5, "rechnerweite Änderung kommt an");
        assert_eq!(saved.profile, settings.profile);
    }

    #[test]
    fn gateway_mac_decides_between_identical_routers() {
        assert_eq!(home_net().matches(&home_net()), Some(Match::Gateway));
        assert_eq!(home_net().matches(&office_net()), None, "gleiche IP, anderer Router");
        let mut no_mac = home_net();
        no_mac.gateway_mac = None;
        assert_eq!(home_net().matches(&no_mac), Some(Match::Address), "MAC noch nicht im ARP-Cache");
        assert_eq!(vpn_net().matches(&vpn_net()), Some(Match::Address));
        let mut other_subnet = no_mac.clone();
        other_subnet.subnet = Some("192.168.0.0/16".into());
        assert_eq!(home_net().matches(&other_subnet), None);
        let mut upper = home_net();
        upper.gateway_mac = Some(HOME_MAC.to_uppercase());
        assert_eq!(home_net().matches(&upper), Some(Match::Gateway));
        assert_eq!(NetworkMark::default().matches(&NetworkMark::default()), None);
    }

    #[test]
    fn network_signature_ignores_order_and_names() {
        let mut named = home_net();
        named.label = "FRITZ!Box".into();
        assert_eq!(signature(&[home_net(), vpn_net()]), signature(&[vpn_net(), named]));
        assert_ne!(signature(&[home_net()]), signature(&[office_net()]));
        assert_eq!(signature(&[]), "");
    }

    #[test]
    fn mark_from_a_detected_network() {
        use qross_platform::network::{Mac, Network};
        let mut network = Network {
            interface: "WLAN".into(),
            gateway: "192.168.178.1".parse().unwrap(),
            gateway_mac: Some(Mac([0x3c, 0xa6, 0x2f, 0x0a, 0x1b, 0x2c])),
            address: Some(("192.168.178.23".parse().unwrap(), 24)),
            name: Some("FRITZ!Box 7590".into()),
            metric: 25,
        };
        let mark = NetworkMark::of(&network);
        assert_eq!(mark.label, "FRITZ!Box 7590");
        assert_eq!(mark.gateway_mac.as_deref(), Some(HOME_MAC));
        assert_eq!(mark.matches(&home_net()), Some(Match::Gateway));
        network.name = None;
        assert_eq!(NetworkMark::of(&network).label, "192.168.178.0/24", "macOS: kein Name ohne Ortungsdienste");
    }

    #[test]
    fn automatic_choice_by_network() {
        let (mut profiles, _) = setup();
        let standard = id_of(&profiles, "Standard");
        let office = id_of(&profiles, "Büro");
        assert!(!profiles.automatic(), "ohne zugeordnete Netzwerke wählt Qross nichts");
        assert_eq!(profiles.choose(&[office_net()]), Choice::Keep);

        let settings = Settings::for_host("pc");
        profiles.edit(ProfileEdit::AddNetwork { id: office.clone(), network: office_net() }, &settings).unwrap();
        assert!(profiles.automatic());
        profiles.active = standard.clone();
        assert_eq!(profiles.choose(&[office_net()]), Choice::Network(office.clone()));
        assert_eq!(profiles.choose(&[vpn_net(), office_net()]), Choice::Network(office.clone()), "VPN zusätzlich");
        assert_eq!(profiles.choose(&[]), Choice::Keep, "offline: nichts ändern");

        // Kein Treffer: Profil für unbekannte Netzwerke (nach der Migration „Standard“)
        profiles.active = office.clone();
        assert_eq!(profiles.choose(&[home_net()]), Choice::Fallback(standard.clone()));
        profiles.edit(ProfileEdit::SetFallback { id: String::new() }, &settings).unwrap();
        assert_eq!(profiles.choose(&[home_net()]), Choice::Keep, "aktuelles behalten");
        profiles.active = standard.clone();
        profiles.edit(ProfileEdit::SetFallback { id: standard.clone() }, &settings).unwrap();
        assert_eq!(profiles.choose(&[home_net()]), Choice::Keep, "schon aktiv");

        // Automatik je Profil und insgesamt abschaltbar
        profiles.edit(ProfileEdit::SetAuto { id: office.clone(), auto: false }, &settings).unwrap();
        assert!(!profiles.automatic());
        assert_eq!(profiles.choose(&[office_net()]), Choice::Keep);
        profiles.edit(ProfileEdit::SetAuto { id: office.clone(), auto: true }, &settings).unwrap();
        profiles.edit(ProfileEdit::SetAutomatic { enabled: false }, &settings).unwrap();
        assert_eq!(profiles.choose(&[office_net()]), Choice::Keep);
    }

    #[test]
    fn stronger_match_wins_and_the_active_profile_is_kept_on_a_tie() {
        let (mut profiles, _) = setup();
        let standard = id_of(&profiles, "Standard");
        let office = id_of(&profiles, "Büro");
        let mut office_without_mac = office_net();
        office_without_mac.gateway_mac = None;
        // „Standard“ kennt das Netz nur ohne MAC (z. B. vor dem ersten ARP), „Büro“ mit
        profiles.list[0].networks.push(office_without_mac);
        profiles.list[1].networks.push(office_net());
        profiles.active = standard.clone();
        assert_eq!(profiles.choose(&[office_net()]), Choice::Network(office.clone()));
        assert_eq!(profiles.matched(&office, &[vpn_net(), office_net()]), Some(&office_net()));
        // Ohne MAC passen beide gleich gut: das aktive bleibt
        let mut current = office_net();
        current.gateway_mac = None;
        assert_eq!(profiles.choose(&[current.clone()]), Choice::Keep);
        profiles.active = office.clone();
        assert_eq!(profiles.choose(&[current]), Choice::Keep);
    }

    #[test]
    fn adding_a_network_moves_it_from_another_profile() {
        let (mut profiles, settings) = setup();
        let standard = id_of(&profiles, "Standard");
        let office = id_of(&profiles, "Büro");
        profiles.edit(ProfileEdit::AddNetwork { id: standard.clone(), network: office_net() }, &settings).unwrap();
        let mut renamed = office_net();
        renamed.label = "Firma".into();
        let note = profiles.edit(ProfileEdit::AddNetwork { id: office.clone(), network: renamed }, &settings).unwrap();
        assert!(note.unwrap().contains("Standard"));
        assert!(profiles.get(&standard).unwrap().networks.is_empty());
        assert_eq!(profiles.get(&office).unwrap().networks.len(), 1);
        // Erneut übernehmen ersetzt den Eintrag (z. B. jetzt mit MAC)
        profiles.edit(ProfileEdit::AddNetwork { id: office.clone(), network: office_net() }, &settings).unwrap();
        assert_eq!(profiles.get(&office).unwrap().networks, vec![office_net()]);
        profiles.edit(ProfileEdit::AddNetwork { id: office.clone(), network: home_net() }, &settings).unwrap();
        profiles.edit(ProfileEdit::RemoveNetwork { id: office.clone(), index: 0 }, &settings).unwrap();
        assert_eq!(profiles.get(&office).unwrap().networks, vec![home_net()]);
        assert!(profiles.edit(ProfileEdit::RemoveNetwork { id: office, index: 5 }, &settings).is_err());
    }

    #[test]
    fn create_rename_duplicate_remove() {
        let (mut profiles, settings) = setup();
        let office = id_of(&profiles, "Büro");
        let standard = id_of(&profiles, "Standard");
        assert!(profiles.edit(ProfileEdit::Create { name: " büro ".into() }, &settings).is_err(), "Name schon vergeben");
        assert!(profiles.edit(ProfileEdit::Create { name: "  ".into() }, &settings).is_err());
        assert!(profiles.edit(ProfileEdit::Create { name: "x".repeat(MAX_NAME + 1) }, &settings).is_err());

        profiles.edit(ProfileEdit::Create { name: "Zuhause".into() }, &settings).unwrap();
        let home_profile = profiles.list.last().unwrap();
        assert_eq!(home_profile.place.role, Role::Server, "neu: dieser Rechner allein als Server");
        assert_eq!(home_profile.place.server.screens.len(), 1);
        assert_eq!(home_profile.place.server.name, settings.server.name);

        profiles.edit(ProfileEdit::AddNetwork { id: office.clone(), network: office_net() }, &settings).unwrap();
        profiles.edit(ProfileEdit::Duplicate { id: office.clone() }, &settings).unwrap();
        profiles.edit(ProfileEdit::Duplicate { id: office.clone() }, &settings).unwrap();
        let names: Vec<&str> = profiles.list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Standard", "Büro", "Zuhause", "Büro (Kopie)", "Büro (Kopie 2)"]);
        let copy = &profiles.list[3];
        assert_eq!(copy.place, profiles.get(&office).unwrap().place);
        assert!(copy.networks.is_empty(), "Netzwerke gehören nur einem Profil");

        let copy = copy.id.clone();
        profiles.edit(ProfileEdit::Rename { id: copy.clone(), name: "Kunde".into() }, &settings).unwrap();
        assert!(profiles.edit(ProfileEdit::Rename { id: copy.clone(), name: "Zuhause".into() }, &settings).is_err());
        profiles.edit(ProfileEdit::Rename { id: copy.clone(), name: "kunde".into() }, &settings).unwrap();
        assert_eq!(profiles.get(&copy).unwrap().name, "kunde", "eigener Name in anderer Schreibweise");

        assert!(profiles.edit(ProfileEdit::Remove { id: office.clone() }, &settings).is_err(), "aktiv");
        profiles.edit(ProfileEdit::Remove { id: standard.clone() }, &settings).unwrap();
        assert_eq!(profiles.fallback, "", "Profil für unbekannte Netzwerke gelöscht");
        assert!(profiles.edit(ProfileEdit::Remove { id: standard }, &settings).is_err());

        let long = "y".repeat(MAX_NAME);
        profiles.edit(ProfileEdit::Rename { id: copy.clone(), name: long.clone() }, &settings).unwrap();
        profiles.edit(ProfileEdit::Duplicate { id: copy }, &settings).unwrap();
        let name = &profiles.list.last().unwrap().name;
        assert_eq!(name.chars().count(), MAX_NAME);
        assert!(name.ends_with(" (Kopie)"));
    }

    #[test]
    fn edits_from_the_window_arrive_as_json() {
        let edit: ProfileEdit = serde_json::from_str(r#"{"action":"rename","id":"p2","name":"Büro"}"#).unwrap();
        assert_eq!(edit, ProfileEdit::Rename { id: "p2".into(), name: "Büro".into() });
        let edit: ProfileEdit =
            serde_json::from_str(r#"{"action":"add_network","id":"p1","network":{"gateway":"10.0.0.1","subnet":null}}"#)
                .unwrap();
        assert!(matches!(edit, ProfileEdit::AddNetwork { network, .. } if network.gateway == "10.0.0.1"));
    }

    #[test]
    fn profiles_roundtrip_and_tolerate_missing_fields() {
        let dir = std::env::temp_dir().join(format!("qross-profiles-test-{}", std::process::id()));
        assert!(Profiles::load(&dir).unwrap().is_none());
        let (mut profiles, settings) = setup();
        profiles.edit(ProfileEdit::AddNetwork { id: profiles.active.clone(), network: office_net() }, &settings).unwrap();
        profiles.save(&dir).unwrap();
        assert_eq!(Profiles::load(&dir).unwrap(), Some(profiles));
        let _ = fs::remove_dir_all(&dir);

        // Felder einer neueren Version stören nicht, fehlende bekommen Standardwerte.
        let newer: Profiles =
            serde_json::from_str(r#"{"active":"p1","zukunft":1,"list":[{"id":"p1","name":"A","place":{"role":"client","neu":true}}]}"#)
                .unwrap();
        assert!(newer.auto);
        let place = &newer.list[0].place;
        assert_eq!(place.role, Role::Client);
        assert_eq!(place.server.port, qross_proto::DEFAULT_PORT);
        assert!(place.server.discovery && newer.list[0].auto);
    }
}
