//! Startet und stoppt Server bzw. Client im eigenen Prozess und führt den Status für das Fenster.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use qross_core::CaptureEvent;
use qross_gui::lifecycle::AppState;
use qross_gui::transfers::{self, LONG_TRANSFER};
use qross_gui::{clipboard_files, Role, Settings};
use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
use qross_net::clipfiles::FilesNotice;
use qross_net::progress::{Progress, Transfers};
use qross_net::quality::Quality;
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform, StopHandle};
use qross_net::{Fingerprint, Identity, TrustOutcome, TrustStore, KNOWN_SERVERS_FILE, TRUSTED_CLIENTS_FILE};
use qross_platform::dryrun::{LogInjector, MemoryClipboard};
use qross_platform::Capture;
use qross_proto::{GameState, Platform, Rect, DEFAULT_PORT};
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
    Server { task: JoinHandle<()>, stop: StopHandle, name: String },
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
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Runtime {
    pub fn new(app: AppHandle, data_dir: PathBuf, settings: Settings) -> Self {
        Self {
            app,
            settings: Mutex::new(settings),
            status: Mutex::new(Status::default()),
            running: tokio::sync::Mutex::new(None),
            capture: Mutex::new(None),
            generation: AtomicU64::new(0),
            dismissed: Mutex::new(Vec::new()),
            receiving_since: Mutex::new(HashMap::new()),
            transfers: Mutex::new(None),
            app_state: Mutex::new(AppState::load(&data_dir)),
            data_dir,
        }
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

    pub fn identity(&self) -> Result<Identity> {
        Identity::load_or_create(&self.data_dir)
    }

    /// Ändert den Status und meldet ihn an Fenster und Tray.
    fn update(&self, change: impl FnOnce(&mut Status)) {
        let status = {
            let mut status = self.status.lock().unwrap();
            change(&mut status);
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
            Running::Server { task, stop, name } => {
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
            .with_keys(keys)
            .with_game_mode(settings.game_mode);
        let stop = server.stop_handle();
        *self.transfers.lock().unwrap() = Some(server.transfers());
        let platform = ServerPlatform {
            capture,
            events,
            clipboard: qross_platform::clipboard(),
            screen: Box::new(qross_platform::screen_bounds),
            monitors: Box::new(qross_platform::monitors),
            game: Box::new(qross_platform::game_state),
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
        Ok(Running::Server { task, stop, name: s.name.clone() })
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
        };
        let platform = if c.dry_run {
            info!("Testmodus: Eingaben werden nur protokolliert");
            ClientPlatform {
                injector: Box::new(LogInjector),
                clipboard: Box::new(MemoryClipboard::default()),
                screen: Box::new(|| Rect::new(0, 0, 1920, 1080)),
                monitors: Box::new(Vec::new),
                game: Box::new(GameState::default),
            }
        } else {
            ClientPlatform {
                injector: qross_platform::injector()?,
                clipboard: qross_platform::clipboard(),
                screen: Box::new(qross_platform::screen_bounds),
                monitors: Box::new(qross_platform::monitors),
                game: if settings.game_mode { Box::new(qross_platform::game_state) } else { Box::new(GameState::default) },
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
            }),
            ClientNotice::Disconnected { reason } => self.update(|s| {
                s.phase = Phase::Connecting;
                s.message = format!("{reason} – neuer Versuch in Kürze");
                s.server_name = None;
                s.cursor_here = false;
            }),
            ClientNotice::CursorEntered => self.update(|s| s.cursor_here = true),
            ClientNotice::CursorLeft => self.update(|s| s.cursor_here = false),
            ClientNotice::ClipboardFiles(notice) => self.on_files_notice(notice),
            ClientNotice::Quality(quality) => self.emit_quality(None, quality),
            ClientNotice::Transfer(progress) => self.on_transfer(progress),
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
