//! Startet und stoppt Server bzw. Client im eigenen Prozess und führt den Status für das Fenster.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use qross_core::sharing::SharingRules;
use qross_core::CaptureEvent;
use qross_gui::lifecycle::AppState;
use qross_gui::profiles::{self, Place, ProfileEdit, Profiles};
use qross_gui::secure_input::{self, SecureInputHint};
use qross_gui::transfers::{self, LONG_TRANSFER};
use qross_gui::{clipboard_files, Role, Settings, EXPORT_FILE};
use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
use qross_net::clipfiles::FilesNotice;
use qross_net::progress::{Progress, Transfers};
use qross_net::quality::Quality;
use qross_net::server::{CursorFinderHandle, Server, ServerConfig, ServerNotice, ServerPlatform, SharingHandle, StopHandle};
use qross_net::{Fingerprint, Identity, TrustOutcome, TrustStore, KNOWN_SERVERS_FILE, TRUSTED_CLIENTS_FILE};
use qross_platform::dryrun::{LogInjector, MemoryClipboard};
use qross_platform::{Capture, FinderOverlay, SecureInput};
use qross_proto::{CursorFinder, GameState, Platform, Rect, Sharing, DEFAULT_PORT};
use serde::Serialize;
use tauri::async_runtime::{self, JoinHandle};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tracing::{info, warn};

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Stopped,
    Starting,
    /// Server lauscht
    Listening,
    /// Client versucht sich zu verbinden
    Connecting,
    /// Client ist verbunden
    Connected,
    Error,
}

#[derive(Serialize, Clone, Debug)]
pub struct ClientInfo {
    pub name: String,
    pub addr: String,
    pub width: u32,
    pub height: u32,
    /// Einzelne Monitore (leer: Client vor Qross 0.5 oder noch nicht gemeldet)
    pub monitors: Vec<Rect>,
    /// Kann Zeichen eingeben (Zeichenmodus); `false` bei Qross vor 0.7 und unter Wayland
    pub types_text: bool,
}

/// Bildschirm dieses Rechners, wenn er Server ist
#[derive(Serialize, Clone, Debug)]
pub struct LocalScreen {
    pub width: u32,
    pub height: u32,
    pub monitors: Vec<Rect>,
}

/// Ein abgewiesener Client, der auf Freigabe wartet (oder einen Hinweis braucht).
#[derive(Serialize, Clone, Debug)]
pub struct PendingClient {
    pub name: String,
    pub addr: String,
    pub fingerprint: String,
    /// `unknown`, `mismatch` oder `known`
    pub trust: &'static str,
    /// Bereits freigegebener Fingerprint (bei `mismatch`)
    pub expected: Option<String>,
    pub reason: String,
    pub in_layout: bool,
    pub last_seen: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Status {
    /// Rolle der laufenden Instanz (`None`, wenn gestoppt)
    pub role: Option<Role>,
    pub phase: Phase,
    pub message: String,
    /// Server: Bildschirm, auf dem der Cursor gerade ist
    pub cursor_on: Option<String>,
    /// Server: Cursor per Rollen-Taste auf `cursor_on` festgehalten
    pub locked: bool,
    /// Server: Der Spielemodus hält den Cursor auf `cursor_on` fest, weil dort ein Programm im
    /// Vollbild läuft (`fullscreen`) oder ein Spiel den Zeiger eingefangen hat (`captured`)
    pub game_hold: Option<&'static str>,
    pub clients: Vec<ClientInfo>,
    pub pending: Vec<PendingClient>,
    /// Client: Name des verbundenen Servers
    pub server_name: Option<String>,
    /// Client: Cursor ist gerade auf diesem Rechner
    pub cursor_here: bool,
    /// Server: eigener Bildschirm samt Monitoren
    pub local_screen: Option<LocalScreen>,
    /// Client: was der Server mit diesem Rechner teilt (`None`: noch nicht gemeldet, etwa von
    /// einem Server bis Qross 0.6, der alles erlaubt)
    pub sharing: Option<Sharing>,
    /// Server (macOS): Hinweis, dass die sichere Tastatureingabe den Cursor auf dem Mac hält
    /// (nur mit verbundenem Client, siehe [`secure_input::hint`])
    pub secure_input: Option<SecureInputHint>,
    /// Server (macOS): wer die sichere Tastatureingabe gerade hält
    #[serde(skip)]
    pub secure_holder: Option<SecureInput>,
    /// Client: was der Server für den Cursor-Finder eingestellt hat (`None`: nichts gemeldet, dann
    /// ist er aus, etwa bei einem Server bis Qross 0.7)
    pub cursor_finder: Option<CursorFinder>,
}

impl Status {
    pub fn running(&self) -> bool {
        self.role.is_some()
    }
}

/// Fingerprint eines Servers, den der Nutzer vor dem ersten Verbinden bestätigen soll.
#[derive(Serialize, Clone, Debug)]
pub struct ServerProbe {
    pub address: String,
    pub addr: String,
    pub fingerprint: String,
    /// Bisher gespeicherter Fingerprint, falls er abweicht
    pub expected: Option<String>,
    /// Fingerprint ist bereits gespeichert und stimmt
    pub known: bool,
}

/// Ein per LAN-Suche gefundener Server. Die Angaben sind unbestätigt; geprüft wird beim Verbinden.
#[derive(Serialize, Clone, Debug)]
pub struct FoundServer {
    pub name: String,
    /// So in das Adressfeld übernehmen (IP, mit Port nur wenn abweichend)
    pub address: String,
    pub fingerprint: String,
    /// `windows`, `macos`, `linux` oder `other`
    pub platform: &'static str,
    /// Strg und Cmd werden beim Verbinden getauscht (Mac ↔ PC)
    pub swaps_modifiers: bool,
    /// `known`, `unknown` oder `mismatch` gegenüber dem gespeicherten Fingerprint
    pub trust: &'static str,
}

/// Messpunkt der Verbindungsqualität für das Fenster (Ereignis `quality`).
#[derive(Serialize, Clone, Debug)]
pub struct QualityEvent {
    /// Server: Name des Clients; Client: `None` (die Verbindung zum Server)
    pub peer: Option<String>,
    /// „Sehr gut“, „Gut“, „Mäßig“ oder „Schlecht“
    pub label: &'static str,
    #[serde(flatten)]
    pub quality: Quality,
}

pub fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "windows",
        Platform::MacOs => "macos",
        Platform::Linux => "linux",
        _ => "other",
    }
}

enum Running {
    /// `sharing`, `cursor_finder`: Rechte je Rechner und Cursor-Finder, im Betrieb änderbar
    Server { task: JoinHandle<()>, stop: StopHandle, name: String, sharing: SharingHandle, cursor_finder: CursorFinderHandle },
    Client { task: JoinHandle<()> },
}

/// Die Eingabe-Erfassung lässt sich pro Prozess nur einmal starten. Sie bleibt deshalb bestehen,
/// und ihre Ereignisse werden an den jeweils laufenden Server weitergereicht.
struct CaptureHub {
    capture: Arc<dyn Capture>,
    target: Arc<Mutex<Option<mpsc::UnboundedSender<CaptureEvent>>>>,
}

pub struct Runtime {
    app: AppHandle,
    pub data_dir: PathBuf,
    pub settings: Mutex<Settings>,
    /// Alle Profile je Ort. Wer beides sperrt, sperrt zuerst `settings`.
    pub profiles: Mutex<Profiles>,
    status: Mutex<Status>,
    running: tokio::sync::Mutex<Option<Running>>,
    capture: Mutex<Option<CaptureHub>>,
    /// Jede gestartete Instanz bekommt eine neue Nummer; Meldungen alter Instanzen werden ignoriert.
    generation: AtomicU64,
    /// In dieser Sitzung ignorierte Clients (Name, Fingerprint)
    dismissed: Mutex<Vec<(String, String)>>,
    /// Seit wann kopierte Dateien von einem Rechner ankommen (für die Systembenachrichtigung)
    receiving_since: Mutex<HashMap<String, Instant>>,
    /// Dateiübertragungen der laufenden Instanz (zum Abbrechen)
    transfers: Mutex<Option<Transfers>>,
    /// Was über einen Programmlauf hinaus gilt (laufende Rolle, letzte Update-Prüfung …)
    app_state: Mutex<AppState>,
    /// Cursor-Finder dieses Rechners, für Server und Client derselbe (beim ersten Gebrauch angelegt)
    finder: OnceLock<Option<Arc<dyn FinderOverlay>>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Runtime {
    pub fn new(app: AppHandle, data_dir: PathBuf, settings: Settings, profiles: Profiles) -> Self {
        Self {
            app,
            settings: Mutex::new(settings),
            profiles: Mutex::new(profiles),
            status: Mutex::new(Status::default()),
            running: tokio::sync::Mutex::new(None),
            capture: Mutex::new(None),
            generation: AtomicU64::new(0),
            dismissed: Mutex::new(Vec::new()),
            receiving_since: Mutex::new(HashMap::new()),
            transfers: Mutex::new(None),
            app_state: Mutex::new(AppState::load(&data_dir)),
            finder: OnceLock::new(),
            data_dir,
        }
    }

    pub fn app_handle(&self) -> &AppHandle {
        &self.app
    }

    pub fn app_state(&self) -> AppState {
        self.app_state.lock().unwrap().clone()
    }

    /// Ändert den App-Zustand und speichert ihn sofort.
    pub fn update_app_state(&self, change: impl FnOnce(&mut AppState)) {
        let mut state = self.app_state.lock().unwrap();
        change(&mut state);
        if let Err(e) = state.save(&self.data_dir) {
            warn!("App-Zustand nicht gespeichert: {e:#}");
        }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Cursor-Finder dieses Rechners; `None` unter Wayland
    fn finder_overlay(&self) -> Option<Arc<dyn FinderOverlay>> {
        self.finder.get_or_init(qross_platform::finder_overlay).clone()
    }

    /// „Ausprobieren“ in den Einstellungen: den Cursor-Finder hier gleich zeigen, mit dem Namen,
    /// unter dem dieser Rechner im Layout steht. `false`, wo es ihn nicht gibt (Wayland).
    pub fn try_cursor_finder(&self, style: CursorFinder) -> bool {
        let Some(overlay) = self.finder_overlay() else { return false };
        let settings = self.settings.lock().unwrap().clone();
        let name = match settings.role {
            Role::Server => settings.server.name.trim().to_owned(),
            Role::Client => settings.client.name.trim().to_owned(),
        };
        overlay.show(style, if name.is_empty() { crate::hostname() } else { name }.as_str());
        true
    }

    pub fn identity(&self) -> Result<Identity> {
        Identity::load_or_create(&self.data_dir)
    }

    /// Ändert den Status und meldet ihn an Fenster und Tray.
    fn update(&self, change: impl FnOnce(&mut Status)) {
        let status = {
            let mut status = self.status.lock().unwrap();
            change(&mut status);
            status.secure_input = secure_input::hint(status.secure_holder.as_ref(), status.clients.len());
            status.clone()
        };
        crate::tray::update(&self.app, &status);
        let _ = self.app.emit("status", status);
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    /// Startet die gespeicherte Rolle. Liefert beim Client einen zu bestätigenden Fingerprint,
    /// wenn der Server noch unbekannt ist; dann wird nicht gestartet.
    pub async fn start(self: &Arc<Self>) -> Result<Option<ServerProbe>> {
        let settings = self.settings.lock().unwrap().clone();
        if settings.role == Role::Client {
            if let Some(probe) = self.needs_confirmation(&settings).await? {
                return Ok(Some(probe));
            }
        }
        let mut running = self.running.lock().await;
        if let Some(old) = running.take() {
            self.shutdown(old).await;
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.update(|s| {
            *s = Status { role: Some(settings.role), phase: Phase::Starting, message: "Startet …".into(), ..Status::default() }
        });
        let result = match settings.role {
            Role::Server => self.start_server(&settings, generation).await,
            Role::Client => self.start_client(&settings, generation),
        };
        match result {
            Ok(r) => {
                *running = Some(r);
                self.update_app_state(|s| s.last_running = Some(settings.role));
                Ok(None)
            }
            Err(e) => {
                warn!("Start fehlgeschlagen: {e:#}");
                let message = format!("{e:#}");
                self.update(|s| *s = Status { phase: Phase::Error, message, ..Status::default() });
                Err(e)
            }
        }
    }

    /// Stoppen durch den Nutzer: beim nächsten Start nicht fortsetzen.
    pub async fn stop(&self) {
        self.update_app_state(|s| s.last_running = None);
        self.stop_for_exit().await;
    }

    /// Stoppen, weil die App endet (Beenden, Update): die Rolle bleibt zum Fortsetzen gemerkt.
    pub async fn stop_for_exit(&self) {
        let mut running = self.running.lock().await;
        if let Some(old) = running.take() {
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.shutdown(old).await;
            info!("gestoppt");
        }
        self.update(|s| *s = Status::default());
    }

    async fn shutdown(&self, running: Running) {
        *self.transfers.lock().unwrap() = None;
        match running {
            Running::Server { task, stop, name, .. } => {
                let _ = tokio::time::timeout(Duration::from_secs(2), stop.stop()).await;
                task.abort();
                if let Some(hub) = self.capture.lock().unwrap().as_ref() {
                    *hub.target.lock().unwrap() = None;
                    // War der Cursor gerade auf einem Client, Maus und Tastatur zurückholen.
                    let cursor_on = self.status.lock().unwrap().cursor_on.clone();
                    if cursor_on.is_some_and(|screen| screen != name) {
                        let b = qross_platform::screen_bounds();
                        hub.capture.release(b.x + b.width as i32 / 2, b.y + b.height as i32 / 2);
                    }
                }
            }
            Running::Client { task } => task.abort(),
        }
    }

    // ---------------------------------------------------------------- Profile

    /// Speichert Einstellungen aus dem Fenster samt aktivem Profil und `qross.toml`. Werte je Ort,
    /// die das Fenster noch für ein inzwischen gewechseltes Profil bearbeitet hat, fallen weg (siehe
    /// [`profiles::rebase`]). Liefert die bisherigen und die neuen Einstellungen; `None`, wenn sich
    /// nichts ändert.
    pub fn store_settings(&self, incoming: Settings) -> Result<Option<(Settings, Settings)>> {
        let mut settings = self.settings.lock().unwrap();
        let incoming = profiles::rebase(incoming, &settings);
        if incoming == *settings {
            return Ok(None);
        }
        incoming.save(&self.data_dir)?;
        self.export(&incoming);
        let old = std::mem::replace(&mut *settings, incoming);
        let mut profiles = self.profiles.lock().unwrap();
        if profiles.sync(&settings) {
            if let Err(e) = profiles.save(&self.data_dir) {
                warn!("Profil nicht gespeichert: {e:#}");
            }
        }
        Ok(Some((old, settings.clone())))
    }

    /// `qross.toml` für `qross server --config`, sofern das Layout gültig ist
    fn export(&self, settings: &Settings) {
        if settings.server.validate().is_ok() {
            if let Err(e) = std::fs::write(self.data_dir.join(EXPORT_FILE), settings.server_toml()) {
                warn!("{EXPORT_FILE} nicht geschrieben: {e}");
            }
        }
    }

    pub fn profiles(&self) -> Profiles {
        self.profiles.lock().unwrap().clone()
    }

    /// Meldet geänderte Profile an Fenster und Tray.
    fn profiles_changed(&self) {
        let profiles = self.profiles();
        crate::tray::update_profiles(&self.app, &profiles);
        let _ = self.app.emit("profiles", profiles);
    }

    /// Änderung aus den Einstellungen (anlegen, umbenennen, Netzwerke …), sofort gespeichert.
    /// `Ok(Some(text))`: Hinweis für das Fenster.
    pub fn edit_profiles(&self, edit: ProfileEdit) -> Result<Option<String>, String> {
        let note = {
            let settings = self.settings.lock().unwrap();
            let mut profiles = self.profiles.lock().unwrap();
            let mut changed = profiles.clone();
            let note = changed.edit(edit, &settings)?;
            changed.save(&self.data_dir).map_err(|e| format!("{e:#}"))?;
            *profiles = changed;
            note
        };
        self.profiles_changed();
        Ok(note)
    }

    /// Aktiviert ein Profil. Lief Server oder Client und ändert sich etwas je Ort, startet er mit
    /// den Werten des Profils neu; als Client kann dafür erst der Server zu bestätigen sein.
    pub async fn activate_profile(self: &Arc<Self>, id: &str) -> Result<Option<ServerProbe>> {
        let (old, new, name) = {
            let mut settings = self.settings.lock().unwrap();
            let mut profiles = self.profiles.lock().unwrap();
            let (mut next_settings, mut next_profiles) = (settings.clone(), profiles.clone());
            next_profiles.activate(id, &mut next_settings).map_err(|e| anyhow!(e))?;
            // Erst gui.json: Sie nennt das Profil, zu dem ihre Werte gehören (siehe Profiles::attach).
            next_settings.save(&self.data_dir)?;
            next_profiles.save(&self.data_dir)?;
            self.export(&next_settings);
            let name = next_profiles.active_profile().map(|p| p.name.clone()).unwrap_or_default();
            *profiles = next_profiles;
            (std::mem::replace(&mut *settings, next_settings.clone()), next_settings, name)
        };
        info!("Profil „{name}“ aktiv");
        self.profiles_changed();
        let _ = self.app.emit("settings", &new);
        if self.status().running() && Place::capture(&old) != Place::capture(&new) {
            info!("{} startet mit dem Profil neu", if new.role == Role::Server { "Server" } else { "Client" });
            return self.start().await;
        }
        Ok(None)
    }

    // ---------------------------------------------------------------- Server

    fn capture_events(&self) -> Result<(Arc<dyn Capture>, mpsc::UnboundedReceiver<CaptureEvent>)> {
        let mut slot = self.capture.lock().unwrap();
        if slot.is_none() {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let capture = qross_platform::start_capture(tx)?;
            let target: Arc<Mutex<Option<mpsc::UnboundedSender<CaptureEvent>>>> = Arc::default();
            let forward = target.clone();
            async_runtime::spawn(async move {
                while let Some(event) = rx.recv().await {
                    if let Some(tx) = forward.lock().unwrap().as_ref() {
                        let _ = tx.send(event);
                    }
                }
            });
            *slot = Some(CaptureHub { capture, target });
        }
        let hub = slot.as_ref().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        *hub.target.lock().unwrap() = Some(tx);
        Ok((hub.capture.clone(), rx))
    }

    async fn start_server(self: &Arc<Self>, settings: &Settings, generation: u64) -> Result<Running> {
        let s = &settings.server;
        let layout = s.validate().map_err(|e| anyhow!(e))?;
        let keys = s.effective_keys().config().map_err(|e| anyhow!(e))?;
        let identity = self.identity()?;
        info!("Fingerprint dieses Servers: {}", identity.fingerprint());

        // Nach einem Neustart kann der Port noch kurz belegt sein.
        let mut attempt = 0;
        let server = loop {
            let trust = TrustStore::load(&self.data_dir.join(TRUSTED_CLIENTS_FILE))?;
            let config = ServerConfig { name: s.name.clone(), listen: s.listen(), layout: layout.clone() };
            match Server::bind(config, &identity, trust) {
                Ok(server) => break server,
                Err(e) if attempt < 5 => {
                    attempt += 1;
                    warn!("{e:#} – neuer Versuch");
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                Err(e) => return Err(e),
            }
        };
        for name in s.isolated() {
            warn!("„{name}“ hat im Layout keinen Nachbarn – der Cursor kann ihn nicht erreichen");
        }

        let (capture, events) = self.capture_events()?;
        let (notice_tx, mut notice_rx) = mpsc::unbounded_channel();
        let server = server
            .with_notices(notice_tx)
            .with_switch_options(s.switch_options())
            .with_discovery(s.discovery)
            .with_clipboard_file_limit(settings.clipboard_file_limit())
            .with_share_concealed(settings.clipboard_share_concealed)
            .with_keys(keys)
            .with_game_mode(settings.game_mode)
            .with_sharing(s.effective_sharing())
            .with_cursor_finder(settings.cursor_finder);
        // Hält ein Programm die sichere Tastatureingabe, bleibt der Cursor auf dem Mac (Hinweis im Fenster)
        #[cfg(target_os = "macos")]
        let server = server.with_secure_input(Box::new(qross_platform::secure_input));
        let (stop, sharing, cursor_finder) = (server.stop_handle(), server.sharing(), server.cursor_finder());
        *self.transfers.lock().unwrap() = Some(server.transfers());
        let platform = ServerPlatform {
            capture,
            events,
            clipboard: qross_platform::clipboard(),
            screen: Box::new(qross_platform::screen_bounds),
            monitors: Box::new(qross_platform::monitors),
            game: Box::new(qross_platform::game_state),
            finder: self.finder_overlay(),
        };

        let rt = self.clone();
        async_runtime::spawn(async move {
            while let Some(notice) = notice_rx.recv().await {
                if !rt.is_current(generation) {
                    break;
                }
                rt.on_server_notice(notice);
            }
        });
        let rt = self.clone();
        let task = async_runtime::spawn(async move {
            let result = server.run(platform).await;
            rt.finished(generation, result);
        });
        Ok(Running::Server { task, stop, name: s.name.clone(), sharing, cursor_finder })
    }

    /// Rechte je Rechner im laufenden Server ändern. Sie gelten sofort, ohne die Clients zu trennen.
    pub async fn set_sharing(&self, rules: SharingRules) {
        if let Some(Running::Server { sharing, .. }) = self.running.lock().await.as_ref() {
            sharing.set(rules);
        }
    }

    /// Cursor-Finder im laufenden Server ändern; er gilt sofort, auch für verbundene Clients.
    pub async fn set_cursor_finder(&self, finder: CursorFinder) {
        if let Some(Running::Server { cursor_finder, .. }) = self.running.lock().await.as_ref() {
            cursor_finder.set(finder);
        }
    }

    fn on_server_notice(&self, notice: ServerNotice) {
        let settings = self.settings.lock().unwrap().server.clone();
        match notice {
            ServerNotice::Listening { addr } => self.update(|s| {
                s.phase = Phase::Listening;
                s.message = format!("Wartet auf Clients (UDP {})", addr.port());
                s.cursor_on = Some(settings.name.clone());
            }),
            ServerNotice::ClientConnected { name, addr, screen } => self.update(|s| {
                s.clients.retain(|c| c.name != name);
                s.pending.retain(|p| p.name != name);
                let (width, height) = (screen.width, screen.height);
                s.clients.push(ClientInfo {
                    name,
                    addr: addr.to_string(),
                    width,
                    height,
                    monitors: Vec::new(),
                    types_text: false,
                });
                s.clients.sort_by(|a, b| a.name.cmp(&b.name));
            }),
            ServerNotice::ClientDisconnected { name } => self.update(|s| {
                s.clients.retain(|c| c.name != name);
                if s.cursor_on.as_deref() == Some(name.as_str()) {
                    s.cursor_on = Some(settings.name.clone());
                }
            }),
            ServerNotice::ClientRejected { name, addr, fingerprint, trust, reason } => {
                let fingerprint = fingerprint.to_string();
                if self.dismissed.lock().unwrap().iter().any(|(n, f)| *n == name && *f == fingerprint) {
                    return;
                }
                let expected = match trust {
                    TrustOutcome::Mismatch => TrustStore::load(&self.data_dir.join(TRUSTED_CLIENTS_FILE))
                        .ok()
                        .and_then(|t| t.expected(&name).map(str::to_owned)),
                    _ => None,
                };
                let pending = PendingClient {
                    in_layout: settings.contains(&name),
                    name,
                    addr: addr.to_string(),
                    fingerprint,
                    trust: match trust {
                        TrustOutcome::Known => "known",
                        TrustOutcome::Unknown => "unknown",
                        TrustOutcome::Mismatch => "mismatch",
                    },
                    expected,
                    reason,
                    last_seen: now_ms(),
                };
                self.update(|s| {
                    s.pending.retain(|p| p.name != pending.name);
                    s.pending.push(pending);
                });
            }
            ServerNotice::CursorOn { screen } => self.update(|s| s.cursor_on = Some(screen)),
            ServerNotice::CursorLocked { locked, screen } => self.update(|s| {
                s.locked = locked;
                s.cursor_on = Some(screen);
            }),
            ServerNotice::ClipboardFiles(notice) => self.on_files_notice(notice),
            ServerNotice::ClientScreenChanged { name, screen, monitors } => self.update(|s| {
                if let Some(c) = s.clients.iter_mut().find(|c| c.name == name) {
                    (c.width, c.height, c.monitors) = (screen.width, screen.height, monitors);
                }
            }),
            ServerNotice::LocalScreenChanged { screen, monitors } => self.update(|s| {
                s.local_screen = Some(LocalScreen { width: screen.width, height: screen.height, monitors });
            }),
            ServerNotice::Quality { name, quality } => self.emit_quality(Some(name), quality),
            ServerNotice::GameHold { screen, game } => self.update(|s| {
                s.game_hold = game.map(|g| if g.captured { "captured" } else { "fullscreen" });
                s.cursor_on = Some(screen);
            }),
            ServerNotice::Transfer(progress) => self.on_transfer(progress),
            ServerNotice::ClientTypesText { name } => self.update(|s| {
                if let Some(c) = s.clients.iter_mut().find(|c| c.name == name) {
                    c.types_text = true;
                }
            }),
            ServerNotice::ClipboardConcealed => self.on_concealed(),
            ServerNotice::SecureInput { holder } => self.update(|s| s.secure_holder = holder),
            ServerNotice::SecureInputHold { from } => self.on_secure_input_hold(from),
        }
    }

    /// Die sichere Tastatureingabe hält den Cursor auf dem Mac (vom Client zurückgeholt oder ein
    /// Wechsel verhindert, beides einmal je Mal): Systembenachrichtigung, sofern das Fenster mit dem
    /// Hinweis nicht ohnehin vorn ist.
    fn on_secure_input_hold(&self, from: Option<String>) {
        let app = self.status.lock().unwrap().secure_holder.as_ref().and_then(|h| h.app.clone());
        let (title, body) = secure_input::notification(from.as_deref(), app.as_deref());
        if !crate::window_in_front(&self.app) {
            crate::notify(&self.app, &title, &body);
        }
    }

    /// Gibt einen wartenden Client frei. Er wird beim nächsten Verbindungsversuch angenommen.
    pub fn approve_client(&self, name: &str, fingerprint: &str) -> Result<()> {
        let fingerprint: Fingerprint = fingerprint.parse()?;
        let path = self.data_dir.join(TRUSTED_CLIENTS_FILE);
        TrustStore::load(&path)?.trust(name, &fingerprint)?;
        info!("„{name}“ ist freigegeben ({fingerprint})");
        self.update(|s| s.pending.retain(|p| p.name != name));
        Ok(())
    }

    pub fn dismiss_client(&self, name: &str) {
        let fingerprint = self.status.lock().unwrap().pending.iter().find(|p| p.name == name).map(|p| p.fingerprint.clone());
        if let Some(fingerprint) = fingerprint {
            self.dismissed.lock().unwrap().push((name.to_owned(), fingerprint));
        }
        self.update(|s| s.pending.retain(|p| p.name != name));
    }

    // ---------------------------------------------------------------- Client

    async fn needs_confirmation(&self, settings: &Settings) -> Result<Option<ServerProbe>> {
        let address = settings.client.address.trim();
        if address.is_empty() {
            return Err(anyhow!("Bitte die Adresse des Servers eingeben."));
        }
        let trust = TrustStore::load(&self.data_dir.join(KNOWN_SERVERS_FILE))?;
        if trust.expected(address).is_some() {
            // Bekannt; eine Abweichung meldet die Client-Laufzeit selbst.
            return Ok(None);
        }
        Ok(Some(self.probe_server(address).await?))
    }

    /// Fragt den Fingerprint des Servers ab, ohne sich anzumelden.
    pub async fn probe_server(&self, address: &str) -> Result<ServerProbe> {
        let identity = self.identity()?;
        let (addr, fingerprint) = tokio::time::timeout(Duration::from_secs(8), client::probe_fingerprint(address, &identity))
            .await
            .map_err(|_| anyhow!("Server {address} antwortet nicht"))?
            .with_context(|| format!("Server {address} nicht erreichbar"))?;
        let trust = TrustStore::load(&self.data_dir.join(KNOWN_SERVERS_FILE))?;
        let outcome = trust.check(address, &fingerprint);
        let expected = match outcome {
            TrustOutcome::Mismatch => trust.expected(address).map(str::to_owned),
            _ => None,
        };
        Ok(ServerProbe {
            address: address.to_owned(),
            addr: addr.to_string(),
            fingerprint: fingerprint.to_string(),
            expected,
            known: outcome == TrustOutcome::Known,
        })
    }

    /// Sucht Qross-Server im LAN (dauert etwa `wait`).
    pub async fn discover_servers(&self, wait: Duration) -> Result<Vec<FoundServer>> {
        let found = qross_net::discovery::discover(wait).await?;
        let trust = TrustStore::load(&self.data_dir.join(KNOWN_SERVERS_FILE))?;
        let mut servers: Vec<FoundServer> = found
            .into_iter()
            .map(|s| {
                let address = if s.addr.port() == DEFAULT_PORT { s.addr.ip().to_string() } else { s.addr.to_string() };
                let trust = match s.fingerprint.parse::<Fingerprint>().map(|fp| trust.check(&address, &fp)) {
                    Ok(TrustOutcome::Known) => "known",
                    Ok(TrustOutcome::Mismatch) => "mismatch",
                    _ => "unknown",
                };
                FoundServer {
                    swaps_modifiers: (s.platform == Platform::MacOs) != (Platform::current() == Platform::MacOs),
                    platform: platform_name(s.platform),
                    name: s.name,
                    address,
                    fingerprint: s.fingerprint,
                    trust,
                }
            })
            .collect();
        servers.sort_by(|a, b| a.name.cmp(&b.name));
        info!("Server-Suche: {} gefunden", servers.len());
        Ok(servers)
    }

    /// Speichert den bestätigten Fingerprint des Servers (ersetzt einen alten Eintrag).
    pub fn trust_server(&self, address: &str, fingerprint: &str) -> Result<()> {
        let fingerprint: Fingerprint = fingerprint.parse()?;
        TrustStore::load(&self.data_dir.join(KNOWN_SERVERS_FILE))?.trust(address.trim(), &fingerprint)?;
        info!("Server {address} bestätigt ({fingerprint})");
        Ok(())
    }

    fn start_client(self: &Arc<Self>, settings: &Settings, generation: u64) -> Result<Running> {
        let c = &settings.client;
        let identity = self.identity()?;
        let name = if c.name.trim().is_empty() { crate::hostname() } else { c.name.trim().to_owned() };
        info!("Dieser Rechner: „{name}“, Fingerprint {}", identity.fingerprint());
        let trust = TrustStore::load(&self.data_dir.join(KNOWN_SERVERS_FILE))?;
        let config = ClientConfig {
            name,
            server: c.address.trim().to_owned(),
            server_fingerprint: None,
            clipboard_file_limit: settings.clipboard_file_limit(),
            share_concealed: settings.clipboard_share_concealed,
        };
        let platform = if c.dry_run {
            info!("Testmodus: Eingaben werden nur protokolliert");
            ClientPlatform {
                injector: Box::new(LogInjector),
                clipboard: Box::new(MemoryClipboard::default()),
                screen: Box::new(|| Rect::new(0, 0, 1920, 1080)),
                monitors: Box::new(Vec::new),
                game: Box::new(GameState::default),
                finder: None,
            }
        } else {
            ClientPlatform {
                injector: qross_platform::injector()?,
                clipboard: qross_platform::clipboard(),
                screen: Box::new(qross_platform::screen_bounds),
                monitors: Box::new(qross_platform::monitors),
                game: if settings.game_mode { Box::new(qross_platform::game_state) } else { Box::new(GameState::default) },
                finder: self.finder_overlay(),
            }
        };

        let (notice_tx, mut notice_rx) = mpsc::unbounded_channel();
        let rt = self.clone();
        async_runtime::spawn(async move {
            while let Some(notice) = notice_rx.recv().await {
                if !rt.is_current(generation) {
                    break;
                }
                rt.on_client_notice(notice);
            }
        });
        let transfers = Transfers::new();
        *self.transfers.lock().unwrap() = Some(transfers.clone());
        let rt = self.clone();
        let task = async_runtime::spawn(async move {
            let result = client::run_with_transfers(config, identity, trust, platform, notice_tx, transfers).await;
            rt.finished(generation, result);
        });
        Ok(Running::Client { task })
    }

    fn on_client_notice(&self, notice: ClientNotice) {
        match notice {
            ClientNotice::Connecting { addr } => self.update(|s| {
                s.phase = Phase::Connecting;
                s.message = format!("Verbinde mit {addr} …");
            }),
            ClientNotice::Connected { server_name, addr } => self.update(|s| {
                s.phase = Phase::Connected;
                s.message = format!("Verbunden mit „{server_name}“ ({addr})");
                s.server_name = Some(server_name);
                // Die Rechte meldet ein Server ab Qross 0.7 gleich danach, den Cursor-Finder ab 0.8.
                s.sharing = None;
                s.cursor_finder = None;
            }),
            ClientNotice::Disconnected { reason } => self.update(|s| {
                s.phase = Phase::Connecting;
                s.message = format!("{reason} – neuer Versuch in Kürze");
                s.server_name = None;
                s.cursor_here = false;
                s.sharing = None;
                s.cursor_finder = None;
            }),
            ClientNotice::CursorEntered => self.update(|s| s.cursor_here = true),
            ClientNotice::CursorLeft => self.update(|s| s.cursor_here = false),
            ClientNotice::ClipboardFiles(notice) => self.on_files_notice(notice),
            ClientNotice::Quality(quality) => self.emit_quality(None, quality),
            ClientNotice::Transfer(progress) => self.on_transfer(progress),
            ClientNotice::Sharing(sharing) => self.update(|s| s.sharing = Some(sharing)),
            ClientNotice::ClipboardConcealed => self.on_concealed(),
            ClientNotice::CursorFinder(finder) => self.update(|s| s.cursor_finder = Some(finder)),
        }
    }

    // ---------------------------------------------------------------- beide

    /// Messpunkt der Verbindungsqualität direkt ans Fenster (etwa 1×/s je Verbindung, deshalb
    /// nicht über den Status, der jedes Mal auch das Tray neu zeichnet).
    fn emit_quality(&self, peer: Option<String>, quality: Quality) {
        let _ = self.app.emit("quality", QualityEvent { peer, label: quality.level.label(), quality });
    }

    /// Stand einer Dateiübertragung direkt ans Fenster (etwa 4×/s, deshalb nicht über den Status).
    /// Am Ende einer längeren Übertragung zusätzlich eine Systembenachrichtigung, wenn das Fenster
    /// im Hintergrund ist.
    pub fn on_transfer(&self, progress: Progress) {
        if let Some((title, body)) = transfers::end_notice(&progress) {
            if !crate::window_in_front(&self.app) {
                crate::notify(&self.app, &title, &body);
            }
        }
        let _ = self.app.emit("transfer", transfers::view(&progress));
    }

    /// „Abbrechen“ in der Fortschrittsanzeige. `false`, wenn die Übertragung schon zu Ende ist.
    pub fn cancel_transfer(&self, id: &str) -> Result<bool> {
        let (direction, transfer) = transfers::parse_id(id).with_context(|| format!("unbekannte Übertragung {id}"))?;
        let running = self.transfers.lock().unwrap().clone();
        Ok(running.is_some_and(|t| t.cancel(direction, transfer)))
    }

    /// Kopierte Dateien: Hinweis im Fenster und, wenn es im Hintergrund ist, eine
    /// Systembenachrichtigung für „bereit“ (nach längerer Übertragung) und „zu groß“.
    /// Das Protokoll schreibt `qross_net` selbst.
    pub fn on_files_notice(&self, notice: FilesNotice) {
        let message = clipboard_files::describe(&notice, Platform::current() == Platform::MacOs);
        let system = {
            let mut since = self.receiving_since.lock().unwrap();
            match &notice {
                FilesNotice::Receiving { peer, .. } => {
                    since.insert(peer.clone(), Instant::now());
                    false
                }
                FilesNotice::Ready { peer, .. } => since.remove(peer).is_some_and(|t| t.elapsed() >= LONG_TRANSFER),
                FilesNotice::TooLarge { .. } => true,
                FilesNotice::Failed { peer, .. } => {
                    since.remove(peer);
                    false
                }
                _ => false,
            }
        };
        if message.text.is_none() {
            return;
        }
        if system && !crate::window_in_front(&self.app) {
            crate::notify(&self.app, &message.title, message.text.as_deref().unwrap_or_default());
        }
        let _ = self.app.emit("clipboard-files", &message);
    }

    /// Ein vertraulich markierter Inhalt blieb hier: dezenter Hinweis im Fenster, ohne den Inhalt.
    /// Das Protokoll schreibt `qross_net` selbst.
    fn on_concealed(&self) {
        let text = "Vertraulicher Inhalt der Zwischenablage bleibt auf diesem Rechner".to_owned();
        let _ = self.app.emit("app-notice", crate::updates::AppNotice { tone: "info", text, action: None });
    }

    fn finished(&self, generation: u64, result: Result<()>) {
        if !self.is_current(generation) {
            return;
        }
        let (phase, message) = match result {
            Ok(()) => (Phase::Stopped, "Beendet".to_owned()),
            Err(e) => {
                warn!("{e:#}");
                (Phase::Error, format!("{e:#}"))
            }
        };
        // Die Instanz läuft nicht mehr; ein erneuter Start räumt `running` auf.
        self.update(|s| *s = Status { phase, message, ..Status::default() });
    }
}
