//! Server-Laufzeit: nimmt Clients an, verbindet Eingabe-Erfassung, `ServerState` und Netzwerk.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use qross_core::keys::{KeyConfig, KeyMode};
use qross_core::server::{Action, Hold, ServerEvent, ServerState, SwitchOptions};
use qross_core::sharing::{describe as describe_sharing, SharingRules};
use qross_core::{CaptureEvent, Layout};
use qross_platform::{Capture, Clipboard, SecureInput};
use qross_proto::{
    encode_datagram, read_frame, write_frame, Clipboard as ClipboardMsg, ClipboardData, Control, Datagram, DragMessage,
    FileEntry, GameState, Input, Platform, Rect, Sharing, StreamKind, MAX_MONITORS, PROTOCOL_VERSION, SERVER_VERSION,
};
use quinn::{Connection, Endpoint, Incoming, RecvStream, SendStream};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::clipboard::ClipboardHub;
use crate::clipfiles::{self, FilesNotice, Notify, Ready};
use crate::files::{FileOutcome, Receiver, Staging};
use crate::game::{self, Detector};
use crate::progress::{Direction, Kind, Progress, Transfers};
use crate::quality::{self, Quality};
use crate::rights::{self, Allowed};
use crate::secure_input;
use crate::{discovery, files};
use crate::streams::{open_input, read_kind, send_drag_message};
use crate::tls::{peer_fingerprint, server_config};
use crate::{Fingerprint, Identity, TrustOutcome, TrustStore};

pub struct ServerConfig {
    /// Name dieses Rechners im Layout
    pub name: String,
    pub listen: SocketAddr,
    pub layout: Layout,
}

pub struct ServerPlatform {
    pub capture: Arc<dyn Capture>,
    pub events: mpsc::UnboundedReceiver<CaptureEvent>,
    pub clipboard: Box<dyn Clipboard>,
    /// Liefert die aktuelle Bildschirmfläche (wird regelmäßig abgefragt).
    pub screen: Box<dyn Fn() -> Rect + Send + Sync>,
    /// Die einzelnen Monitore in den Koordinaten von `screen` (leer: unbekannt)
    pub monitors: Box<dyn Fn() -> Vec<Rect> + Send + Sync>,
    /// Spielemodus: was im Vordergrund läuft (üblich: `qross_platform::game_state`)
    pub game: Detector,
}

pub struct Server {
    endpoint: Endpoint,
    config: ServerConfig,
    trust: TrustStore,
    notices: Option<mpsc::UnboundedSender<ServerNotice>>,
    switching: SwitchOptions,
    /// Auf Suchanfragen im LAN antworten (Standard, wenn auf allen Adressen gelauscht wird)
    discovery: bool,
    /// Kopierte Dateien bis zu dieser Gesamtgröße mitgeben (0 = nie)
    clipboard_file_limit: u64,
    /// Vertraulich markierte Inhalte der Zwischenablage trotzdem teilen
    share_concealed: bool,
    /// Fang-Taste, Wechsel-Kürzel, Medientasten, Umbelegung je Rechner
    keys: KeyConfig,
    /// Bei Vollbild bzw. eingefangenem Zeiger festhalten und relative Bewegungen schicken
    game_mode: bool,
    fingerprint: Fingerprint,
    /// Laufende Dateiübertragungen (Fortschritt, Abbrechen)
    transfers: Transfers,
    /// Rechte je Rechner, auch während des Laufs änderbar
    sharing: SharingHandle,
    /// Sichere Tastatureingabe abfragen (macOS)
    secure_input: Option<secure_input::Detector>,
}

/// Ändert die Rechte je Rechner eines laufenden Servers (siehe [`Server::sharing`]). Sie gelten
/// sofort, auch für bestehende Verbindungen: Was nicht mehr erlaubt ist, bricht ab, und Clients
/// ab Qross 0.7 erfahren ihre neuen Rechte.
#[derive(Clone)]
pub struct SharingHandle(Arc<watch::Sender<SharingRules>>);

impl SharingHandle {
    pub fn set(&self, rules: SharingRules) {
        self.0.send_if_modified(|current| {
            let changed = *current != rules;
            *current = rules;
            changed
        });
    }

    pub fn get(&self) -> SharingRules {
        self.0.borrow().clone()
    }
}

/// Bricht eine Hintergrundaufgabe ab, sobald der Server (bzw. `run`) beendet wird.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Zustandsmeldungen für eine Oberfläche (siehe [`Server::with_notices`]).
#[derive(Debug, Clone)]
pub enum ServerNotice {
    Listening { addr: SocketAddr },
    ClientConnected { name: String, addr: SocketAddr, screen: Rect },
    ClientDisconnected { name: String },
    /// Ein Client wurde abgewiesen. Bei `trust == Unknown` wartet er auf Freigabe
    /// (er versucht es alle paar Sekunden erneut; die Vertrauensliste wird dann neu gelesen).
    ClientRejected { name: String, addr: SocketAddr, fingerprint: Fingerprint, trust: TrustOutcome, reason: String },
    /// Der Cursor ist jetzt auf diesem Bildschirm (Server oder Client).
    CursorOn { screen: String },
    /// Rollen-Taste: Cursor auf `screen` festgehalten (`locked`) bzw. wieder frei.
    CursorLocked { locked: bool, screen: String },
    /// Kopierte Dateien in der Zwischenablage (senden, empfangen, einfügbar, zu groß)
    ClipboardFiles(FilesNotice),
    /// Bildschirm eines verbundenen Clients: Gesamtfläche und einzelne Monitore (leer bei Clients
    /// vor Qross 0.5). Kommt nach `ClientConnected`, sobald er Monitore meldet, und bei Änderungen.
    ClientScreenChanged { name: String, screen: Rect, monitors: Vec<Rect> },
    /// Bildschirm des Servers selbst, nach `Listening` und bei Änderungen
    LocalScreenChanged { screen: Rect, monitors: Vec<Rect> },
    /// Verbindungsqualität zu einem Client, etwa jede Sekunde (siehe [`quality`]).
    Quality { name: String, quality: Quality },
    /// Spielemodus: Der Cursor bleibt auf `screen`, weil dort ein Vollbildprogramm läuft bzw. ein
    /// Spiel den Zeiger eingefangen hat (`game`), oder ist wieder frei (`game: None`). Die
    /// Fang-Taste löst den Halt; ist sie selbst gedrückt, meldet das `CursorLocked`.
    GameHold { screen: String, game: Option<GameState> },
    /// Stand einer Dateiübertragung (Drag & Drop, kopierte Dateien) etwa viermal pro Sekunde und
    /// zum Schluss ihr Ergebnis (siehe [`crate::progress`]). Abbrechen über [`Server::transfers`].
    Transfer(Progress),
    /// Der Client kann Zeichen eingeben (Zeichenmodus, ab Qross 0.7). Kommt kurz nach
    /// `ClientConnected`; bleibt sie aus, bekommt er nur Tastenpositionen.
    ClientTypesText { name: String },
    /// Ein Passwortmanager hat den Inhalt der Zwischenablage als vertraulich markiert; er bleibt
    /// hier (einmal je Inhalt, siehe [`Server::with_share_concealed`]).
    ClipboardConcealed,
    /// Auf diesem Rechner (macOS) hält ein Programm die „Sichere Tastatureingabe“ (`holder`) bzw.
    /// wieder keines (`None`); kommt auch, wenn ein anderes Programm sie übernimmt. Solange sie an
    /// ist, bleibt der Cursor hier (siehe [`Server::with_secure_input`]).
    SecureInput { holder: Option<SecureInput> },
    /// Wegen der sicheren Tastatureingabe bleibt der Cursor hier: vom Client `from` zurückgeholt
    /// bzw. ein Wechsel wurde verhindert (`from: None`, einmal, solange sie an ist).
    SecureInputHold { from: Option<String> },
}

/// Beendet einen laufenden Server von außen: schließt alle Verbindungen und gibt den Port frei.
#[derive(Clone)]
pub struct StopHandle(Endpoint);

impl StopHandle {
    pub async fn stop(&self) {
        self.0.close(0u32.into(), b"server stopped");
        self.0.wait_idle().await;
    }
}

enum NetEvent {
    Hello {
        name: String,
        screen: Rect,
        fingerprint: Fingerprint,
        conn: Connection,
        reply: oneshot::Sender<Result<Admitted, String>>,
    },
    ScreenChanged { name: String, id: usize, screen: Rect },
    Monitors { name: String, id: usize, monitors: Vec<Rect> },
    Game { name: String, id: usize, game: GameState },
    TypesText { name: String, id: usize },
    Clipboard { name: String, id: usize, data: ClipboardData },
    ClipboardFiles { name: String, id: usize, transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    ClipboardFile { name: String, id: usize, outcome: FileOutcome },
    /// Antwort auf eine Drag-Nachfrage (Drag & Drop vom Client zum Server)
    DragOffer { name: String, id: usize, probe: u32, transfer: u64, entries: Vec<FileEntry> },
    DragFile { name: String, id: usize, outcome: FileOutcome },
    Closed { name: String, id: usize },
}

/// Angenommen: Name des Servers fürs `Welcome` und die Rechte des Clients, die sich mit den Regeln ändern
struct Admitted {
    server_name: String,
    sharing: watch::Receiver<Sharing>,
}

/// Zwischenordner für ankommende Dateien
#[derive(Clone)]
struct Stagings {
    clipboard: Arc<Staging>,
    drag: Arc<Staging>,
    /// Zählt die ankommenden Bytes
    transfers: Transfers,
}

struct ClientHandle {
    /// Unterscheidet eine neue Verbindung vom selben Client von der alten.
    id: usize,
    conn: Connection,
    input: mpsc::UnboundedSender<Input>,
    /// Zuletzt gemeldete Fläche und Monitore (für die Oberfläche)
    screen: Rect,
    monitors: Vec<Rect>,
    /// Misst die Verbindungsqualität, solange der Client verbunden ist (nur mit Oberfläche)
    _quality: Option<quality::Monitor>,
    /// Was der Server mit ihm teilt; die Verbindung liest mit (Streams prüfen, Client informieren).
    sharing: watch::Sender<Sharing>,
    /// Kann Zeichen eingeben (`Control::TypesText`)
    types_text: bool,
    /// Schon ins Log geschrieben, dass er trotz Zeichenmodus Tastenpositionen bekommt
    told_positions: std::sync::atomic::AtomicBool,
}

impl Server {
    pub fn bind(config: ServerConfig, identity: &Identity, trust: TrustStore) -> Result<Self> {
        let endpoint = Endpoint::server(server_config(identity)?, config.listen)
            .with_context(|| format!("UDP {} öffnen", config.listen))?;
        let discovery = config.listen.ip().is_unspecified();
        Ok(Self {
            endpoint,
            config,
            trust,
            notices: None,
            switching: SwitchOptions::RECOMMENDED,
            discovery,
            clipboard_file_limit: clipfiles::DEFAULT_LIMIT,
            share_concealed: false,
            keys: KeyConfig::default(),
            game_mode: true,
            fingerprint: identity.fingerprint(),
            transfers: Transfers::new(),
            sharing: SharingHandle(Arc::new(watch::channel(SharingRules::default()).0)),
            secure_input: None,
        })
    }

    /// Sichere Tastatureingabe abfragen (macOS; üblich: `qross_platform::secure_input`, Standard:
    /// gar nicht). Solange ein Programm sie hält, erreichen Tasten die Erfassung nicht, sondern
    /// dieses Programm: Der Cursor kommt dann von einem Client zurück und bleibt hier, bis sie
    /// endet. Das melden `ServerNotice::SecureInput` und `ServerNotice::SecureInputHold`.
    pub fn with_secure_input(mut self, detect: secure_input::Detector) -> Self {
        self.secure_input = Some(detect);
        self
    }

    /// Rechte je Rechner (Standard: alles in beide Richtungen). Ändern im Betrieb über [`Server::sharing`].
    pub fn with_sharing(self, rules: SharingRules) -> Self {
        self.sharing.set(rules);
        self
    }

    /// Griff, um die Rechte je Rechner im Betrieb zu ändern
    pub fn sharing(&self) -> SharingHandle {
        self.sharing.clone()
    }

    /// Spielemodus (Standard: an): Läuft auf dem Rechner mit dem Cursor ein Vollbildprogramm oder
    /// hat dort ein Spiel den Zeiger eingefangen, bleibt der Cursor dort, und ein Client bekommt
    /// relative Bewegungen. Clients melden das erst ab Qross 0.6.
    pub fn with_game_mode(mut self, enabled: bool) -> Self {
        self.game_mode = enabled;
        self
    }

    /// Laufende Dateiübertragungen, etwa zum Abbrechen aus der Oberfläche
    pub fn transfers(&self) -> Transfers {
        self.transfers.clone()
    }

    /// Fang-Taste, Medientasten und Umbelegung je Rechner (Standard: Rollen-Taste,
    /// Medientasten an den aktiven Rechner, keine Umbelegung), Wechsel-Kürzel (Standard: keine)
    /// und Tastenposition oder Zeichen je Rechner (Standard: Tastenposition).
    pub fn with_keys(mut self, keys: KeyConfig) -> Self {
        self.keys = keys;
        self
    }

    /// Kopierte Dateien bis zu dieser Gesamtgröße in Bytes mitgeben (0 = nie;
    /// Standard: `clipfiles::DEFAULT_LIMIT`). Größeres bleibt Drag & Drop vorbehalten.
    pub fn with_clipboard_file_limit(mut self, bytes: u64) -> Self {
        self.clipboard_file_limit = bytes;
        self
    }

    /// Inhalte der Zwischenablage, die ein Passwortmanager als vertraulich markiert, trotzdem an
    /// Clients geben (Standard: nein; dann meldet `ServerNotice::ClipboardConcealed` das Zurückhalten).
    pub fn with_share_concealed(mut self, share: bool) -> Self {
        self.share_concealed = share;
        self
    }

    /// Ecken-Totzone und Wechselverzögerung (Standard: `SwitchOptions::RECOMMENDED`).
    pub fn with_switch_options(mut self, options: SwitchOptions) -> Self {
        self.switching = options;
        self
    }

    /// Server-Suche im LAN ein-/ausschalten (Standard: an, wenn auf allen Adressen gelauscht wird).
    pub fn with_discovery(mut self, enabled: bool) -> Self {
        self.discovery = enabled;
        self
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Meldet Verbindungen, Ablehnungen und Cursorwechsel zusätzlich über `notices`.
    pub fn with_notices(mut self, notices: mpsc::UnboundedSender<ServerNotice>) -> Self {
        self.notices = Some(notices);
        self
    }

    /// Griff zum Beenden; danach die Aufgabe mit `run` abbrechen.
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle(self.endpoint.clone())
    }

    fn notify(&self, notice: ServerNotice) {
        if let Some(tx) = &self.notices {
            let _ = tx.send(notice);
        }
    }

    /// Meldet die Verbindungsqualität eines Clients, sofern jemand zuhört.
    fn monitor_quality(&self, conn: &Connection, name: &str) -> Option<quality::Monitor> {
        let (tx, name) = (self.notices.clone()?, name.to_owned());
        Some(quality::monitor(conn.clone(), move |quality| {
            let _ = tx.send(ServerNotice::Quality { name: name.clone(), quality });
        }))
    }

    fn notify_action(&self, action: &Action) {
        match action {
            Action::Send { to, input: Input::Enter { .. } } => self.notify(ServerNotice::CursorOn { screen: to.clone() }),
            Action::ReleaseCapture { .. } => self.notify(ServerNotice::CursorOn { screen: self.config.name.clone() }),
            Action::HeldBySecureInput { from } => self.notify(ServerNotice::SecureInputHold { from: from.clone() }),
            _ => {}
        }
    }

    pub async fn run(mut self, platform: ServerPlatform) -> Result<()> {
        // Kopierte Dateien aller Clients landen hier, bis sie in die Zwischenablage kommen.
        let transfers = self.transfers.clone();
        let mut incoming = clipfiles::Incoming::new(Staging::persistent(&clipfiles::default_store())?, transfers.clone());
        // Vom Client gezogene Dateien warten hier, bis sie auf dem Server abgelegt werden.
        let mut drops = Receiver::new(Staging::create(&files::drag_staging_base())?, transfers.clone());
        let stagings = Stagings { clipboard: incoming.staging(), drag: drops.staging(), transfers: transfers.clone() };
        let _progress = {
            let notices = self.notices.clone();
            transfers.spawn_reporter(move |progress| {
                if let Some(tx) = &notices {
                    let _ = tx.send(ServerNotice::Transfer(progress));
                }
            })
        };
        let (net_tx, mut net_rx) = mpsc::unbounded_channel();
        tokio::spawn(accept_loop(self.endpoint.clone(), net_tx, stagings));

        let ServerPlatform { capture, mut events, clipboard, screen, monitors, game } = platform;
        // Ohne Spielemodus wird gar nicht erst nachgesehen; der Sender hält den Kanal dann offen.
        let (_no_game, mut local_game) = tokio::sync::watch::channel(GameState::default());
        if self.game_mode {
            local_game = game::watch(game);
        }
        let (_no_secure_input, mut secure) = watch::channel(None);
        if let Some(detect) = self.secure_input.take() {
            secure = secure_input::watch(detect);
        }
        let clipboard = ClipboardHub::spawn(clipboard, self.share_concealed, {
            let notices = self.notices.clone();
            move || {
                if let Some(tx) = &notices {
                    let _ = tx.send(ServerNotice::ClipboardConcealed);
                }
            }
        });
        let files_notify: Notify = {
            let notices = self.notices.clone();
            Arc::new(move |notice| {
                if let Some(tx) = &notices {
                    let _ = tx.send(ServerNotice::ClipboardFiles(notice));
                }
            })
        };
        let mut local_screen = screen();
        let mut local_monitors = monitors();
        let mut state = ServerState::new(self.config.name.clone(), self.config.layout.clone(), local_screen);
        state.handle(ServerEvent::LocalMonitors(local_monitors.clone()));
        state.set_switch_options(self.switching);
        state.set_game_mode(self.game_mode);
        state.set_key_maps(self.keys.remap.clone());
        state.set_key_modes(self.keys.modes.clone());
        for (hotkey, target) in &self.keys.options.switch {
            if let qross_core::keys::SwitchTarget::Screen(name) = target {
                if *name != self.config.name && !self.config.layout.contains(name) {
                    warn!("Kürzel „{hotkey}“ führt zu „{name}“, der nicht im Layout steht");
                }
            }
        }
        capture.set_key_options(self.keys.options.clone());
        let _discovery = self.discovery.then(|| {
            let port = self.local_addr().map(|a| a.port()).unwrap_or_default();
            AbortOnDrop(tokio::spawn(discovery::respond(self.config.name.clone(), port, self.fingerprint.to_string())))
        });
        let mut clients: HashMap<String, ClientHandle> = HashMap::new();
        // Clipboard-Sequenznummer, die ein Client zuletzt bekommen hat
        let clip_synced: ClipSynced = Arc::new(Mutex::new(HashMap::new()));
        let mut rules = self.sharing.0.subscribe();
        state.set_sharing(rules.borrow_and_update().clone());
        let mut screen_poll = tokio::time::interval(Duration::from_secs(2));
        // Zuletzt gemeldetes Festhalten und der Bildschirm dazu
        let mut hold = (Hold::Free, self.config.name.clone());
        info!("Server „{}“ lauscht auf {}", self.config.name, self.local_addr()?);
        if !local_monitors.is_empty() {
            info!("eigene Monitore: {}", describe_monitors(&local_monitors));
        }
        self.notify(ServerNotice::Listening { addr: self.local_addr()? });
        self.notify(ServerNotice::LocalScreenChanged { screen: local_screen, monitors: local_monitors.clone() });

        loop {
            let actions = tokio::select! {
                event = events.recv() => match event {
                    Some(CaptureEvent::MouseDelta { mut dx, mut dy }) => {
                        // Schon wartende Bewegungen zusammenfassen (z. B. 8-kHz-Mäuse oder kurz
                        // ausgelasteter Rechner): ein Datagramm statt vieler, ohne Wartezeit.
                        let mut next = None;
                        while let Ok(event) = events.try_recv() {
                            match event {
                                CaptureEvent::MouseDelta { dx: x, dy: y } => {
                                    dx = dx.saturating_add(x);
                                    dy = dy.saturating_add(y);
                                }
                                other => {
                                    next = Some(other);
                                    break;
                                }
                            }
                        }
                        let mut actions = state.handle(ServerEvent::Capture(CaptureEvent::MouseDelta { dx, dy }));
                        if let Some(event) = next {
                            actions.extend(self.on_capture(&mut state, event));
                        }
                        actions
                    }
                    Some(event) => self.on_capture(&mut state, event),
                    None => bail!("Eingabe-Erfassung wurde beendet"),
                },
                Some(event) = net_rx.recv() => match event {
                    NetEvent::Hello { name, screen, fingerprint, conn, reply } => {
                        let addr = conn.remote_address();
                        match self.admit(&name, &fingerprint, &clients) {
                            Ok(()) => {
                                self.notify(ServerNotice::ClientConnected { name: name.clone(), addr, screen });
                                let input = spawn_input_writer(conn.clone(), name.clone());
                                let quality = self.monitor_quality(&conn, &name);
                                let sharing = rules.borrow().for_client(&name);
                                let (sharing_tx, sharing_rx) = watch::channel(sharing);
                                let handle = ClientHandle {
                                    id: conn.stable_id(),
                                    conn,
                                    input,
                                    screen,
                                    monitors: Vec::new(),
                                    _quality: quality,
                                    sharing: sharing_tx,
                                    types_text: false,
                                    told_positions: Default::default(),
                                };
                                clients.insert(name.clone(), handle);
                                clip_synced.lock().unwrap().remove(&name);
                                let _ = reply.send(Ok(Admitted { server_name: self.config.name.clone(), sharing: sharing_rx }));
                                info!("Client „{name}“ verbunden ({}×{})", screen.width, screen.height);
                                if sharing != Sharing::ALL {
                                    info!("Rechte für „{name}“: {}", describe_sharing(sharing));
                                }
                                state.handle(ServerEvent::ClientConnected { name, screen })
                            }
                            Err(reason) => {
                                warn!("Client „{name}“ abgelehnt: {reason}");
                                let trust = self.trust.check(&name, &fingerprint);
                                self.notify(ServerNotice::ClientRejected {
                                    name: name.clone(),
                                    addr,
                                    fingerprint,
                                    trust,
                                    reason: reason.clone(),
                                });
                                let _ = reply.send(Err(reason));
                                Vec::new()
                            }
                        }
                    }
                    NetEvent::ScreenChanged { name, id, screen } if is_current(&clients, &name, id) => {
                        info!("Bildschirm von „{name}“ geändert: {}×{}", screen.width, screen.height);
                        if let Some(client) = clients.get_mut(&name) {
                            client.screen = screen;
                            let monitors = client.monitors.clone();
                            self.notify(ServerNotice::ClientScreenChanged { name: name.clone(), screen, monitors });
                        }
                        state.handle(ServerEvent::ClientScreenChanged { name, screen })
                    }
                    NetEvent::Monitors { name, id, monitors } if is_current(&clients, &name, id) => {
                        info!("„{name}“ meldet {}", describe_monitors(&monitors));
                        if let Some(client) = clients.get_mut(&name) {
                            client.monitors = monitors.clone();
                            let (screen, monitors) = (client.screen, monitors.clone());
                            self.notify(ServerNotice::ClientScreenChanged { name: name.clone(), screen, monitors });
                        }
                        state.handle(ServerEvent::ClientMonitors { name, monitors })
                    }
                    NetEvent::Game { name, id, game } if is_current(&clients, &name, id) => {
                        debug!("Spielemodus: „{name}“ meldet {}", game::describe(game));
                        state.handle(ServerEvent::ClientGame { name, game })
                    }
                    NetEvent::TypesText { name, id } if is_current(&clients, &name, id) => {
                        if let Some(client) = clients.get_mut(&name) {
                            client.types_text = true;
                        }
                        if self.keys.modes.get(&name) == Some(&KeyMode::Chars) {
                            info!("„{name}“ bekommt Zeichen statt Tastenpositionen");
                        } else {
                            debug!("„{name}“ kann Zeichen eingeben");
                        }
                        self.notify(ServerNotice::ClientTypesText { name: name.clone() });
                        state.handle(ServerEvent::ClientTypesText { name })
                    }
                    NetEvent::Clipboard { name, id, data } if is_current(&clients, &name, id) => {
                        info!("Zwischenablage von „{name}“: {}", data.summary());
                        incoming.supersede();
                        let (clipboard, synced) = (clipboard.clone(), clip_synced.clone());
                        tokio::spawn(async move {
                            match clipboard.write(data).await {
                                // Nicht an denselben Client zurückschicken
                                Ok(seq) => {
                                    synced.lock().unwrap().insert(name, seq);
                                }
                                Err(e) => warn!("Zwischenablage von „{name}“ nicht übernommen: {e:#}"),
                            }
                        });
                        Vec::new()
                    }
                    NetEvent::ClipboardFiles { name, id, transfer, entries, data } if is_current(&clients, &name, id) => {
                        let (names, bytes) = clipfiles::summary(&entries);
                        files_notify(FilesNotice::Receiving { peer: name.clone(), names, bytes });
                        match incoming.announce(name.clone(), transfer, entries, data) {
                            Ok(Some(ready)) => deliver_files(ready, &clipboard, &clip_synced, &files_notify),
                            Ok(None) => {}
                            Err(e) => {
                                warn!("kopierte Dateien von „{name}“ abgelehnt: {e:#}");
                                files_notify(FilesNotice::Failed { peer: name, reason: format!("{e:#}") });
                            }
                        }
                        Vec::new()
                    }
                    NetEvent::ClipboardFile { name, id, outcome } if is_current(&clients, &name, id) => {
                        match incoming.on_file(outcome) {
                            Ok(Some(ready)) => deliver_files(ready, &clipboard, &clip_synced, &files_notify),
                            Ok(None) => {}
                            Err(failed) => files_notify(FilesNotice::Failed { peer: failed.peer, reason: failed.reason }),
                        }
                        Vec::new()
                    }
                    NetEvent::DragOffer { name, id, probe, transfer, entries } if is_current(&clients, &name, id) => {
                        let has_files = !entries.is_empty();
                        if has_files {
                            drops.on_drag_start(&name, transfer, entries);
                        }
                        state.handle(ServerEvent::RemoteDragOffer { client: name, probe, transfer, has_files })
                    }
                    NetEvent::DragFile { name, id, outcome } if is_current(&clients, &name, id) => {
                        drops.on_file(outcome);
                        Vec::new()
                    }
                    NetEvent::Closed { name, id } if is_current(&clients, &name, id) => {
                        clients.remove(&name);
                        info!("Client „{name}“ getrennt");
                        transfers.abort_receiving(Some(&name), "Verbindung getrennt");
                        self.notify(ServerNotice::ClientDisconnected { name: name.clone() });
                        state.handle(ServerEvent::ClientDisconnected { name })
                    }
                    _ => Vec::new(),
                },
                Ok(()) = rules.changed() => {
                    let rules = rules.borrow_and_update().clone();
                    for (name, client) in &clients {
                        let sharing = rules.for_client(name);
                        if client.sharing.send_replace(sharing) != sharing {
                            info!("Rechte für „{name}“ geändert: {}", describe_sharing(sharing));
                            cancel_forbidden(&transfers, name, sharing);
                        }
                    }
                    state.set_sharing(rules);
                    Vec::new()
                }
                Ok(()) = local_game.changed() => {
                    let game = *local_game.borrow_and_update();
                    debug!("Spielemodus: auf diesem Rechner {}", game::describe(game));
                    state.handle(ServerEvent::LocalGame(game))
                }
                Ok(()) = secure.changed() => {
                    let holder = secure.borrow_and_update().clone();
                    match &holder {
                        Some(h) => info!(
                            "Sichere Tastatureingabe ist an ({}) – Tasten erreichen Qross nicht, der Cursor bleibt auf \
                             diesem Rechner",
                            h.describe()
                        ),
                        None => info!("Sichere Tastatureingabe ist wieder aus"),
                    }
                    let on = holder.is_some();
                    self.notify(ServerNotice::SecureInput { holder });
                    state.handle(ServerEvent::LocalSecureInput(on))
                }
                _ = screen_poll.tick() => {
                    let (now, now_monitors) = (screen(), monitors());
                    let mut actions = Vec::new();
                    if now != local_screen {
                        info!("lokaler Bildschirm geändert: {}×{}", now.width, now.height);
                        actions.extend(state.handle(ServerEvent::LocalScreenChanged(now)));
                    }
                    if now_monitors != local_monitors {
                        info!("eigene Monitore: {}", describe_monitors(&now_monitors));
                        actions.extend(state.handle(ServerEvent::LocalMonitors(now_monitors.clone())));
                    }
                    if now != local_screen || now_monitors != local_monitors {
                        (local_screen, local_monitors) = (now, now_monitors);
                        let monitors = local_monitors.clone();
                        self.notify(ServerNotice::LocalScreenChanged { screen: local_screen, monitors });
                    }
                    actions
                }
            };
            for action in actions {
                self.notify_action(&action);
                self.explain_key_mode(&action, &clients);
                let files = FileSide { limit: self.clipboard_file_limit, notify: &files_notify, transfers: &transfers };
                apply(action, &*capture, &clipboard, &clients, &clip_synced, files, &mut drops);
            }
            self.report_hold(&state, &mut hold);
        }
    }

    /// Steht ein Client auf „Zeichen“, kann sie aber nicht eingeben: einmal je Verbindung ins Log,
    /// sobald der Cursor ihn betritt.
    fn explain_key_mode(&self, action: &Action, clients: &HashMap<String, ClientHandle>) {
        let Action::Send { to, input: Input::Enter { .. } } = action else { return };
        let Some(client) = clients.get(to) else { return };
        if self.keys.modes.get(to) == Some(&KeyMode::Chars)
            && !client.types_text
            && !client.told_positions.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            info!(
                "„{to}“ steht auf „Zeichen“, kann sie aber nicht eingeben (Qross vor 0.7 oder Wayland) – \
                 er bekommt Tastenpositionen"
            );
        }
    }

    /// Fang-Taste und Spielemodus: Hat sich geändert, ob und warum der Cursor festgehalten wird?
    fn report_hold(&self, state: &ServerState, last: &mut (Hold, String)) {
        let now = (state.hold(), state.active_screen().to_owned());
        if now == *last {
            return;
        }
        let (old, old_screen) = std::mem::replace(last, now.clone());
        let (hold, screen) = now;
        let locked = hold == Hold::Locked;
        if locked != (old == Hold::Locked) {
            if locked {
                info!("Cursor auf „{screen}“ festgehalten (Fang-Taste löst)");
            } else {
                info!("Cursor wieder frei");
            }
            self.notify(ServerNotice::CursorLocked { locked, screen: screen.clone() });
        }
        let game_of = |hold: Hold| match hold {
            Hold::Game(game) => Some(game),
            _ => None,
        };
        let (game, before) = (game_of(hold), game_of(old));
        // Neu, beendet, anders (Vollbild ↔ eingefangen) oder auf einem anderen Bildschirm
        if game != before || (game.is_some() && screen != old_screen) {
            match game {
                Some(g) => info!("Spielemodus: Cursor bleibt auf „{screen}“ ({}; Fang-Taste löst)", game::describe(g)),
                // Mit der Fang-Taste festgehalten: Das hat `CursorLocked` schon gemeldet.
                None if !locked => info!("Spielemodus: Cursor wieder frei"),
                None => {}
            }
            self.notify(ServerNotice::GameHold { screen, game });
        }
    }

    /// Erfassung an den Kern geben und Wechsel per Tastenkürzel ins Protokoll schreiben
    /// (ob und warum der Cursor festgehalten wird, meldet `report_hold`).
    fn on_capture(&self, state: &mut ServerState, event: CaptureEvent) -> Vec<Action> {
        let switch = match &event {
            CaptureEvent::SwitchTo(target) => Some(target.label()),
            _ => None,
        };
        let actions = state.handle(ServerEvent::Capture(event));
        if let Some(target) = switch {
            if actions.is_empty() {
                info!("Kürzel {target}: kein Wechsel (Ziel nicht verbunden, Cursor festgehalten oder schon dort)");
            } else {
                info!("Kürzel {target}: Cursor jetzt auf „{}“", state.active_screen());
            }
        }
        actions
    }

    fn admit(&mut self, name: &str, fingerprint: &Fingerprint, clients: &HashMap<String, ClientHandle>) -> Result<(), String> {
        if name == self.config.name {
            return Err(format!("„{name}“ ist der Name des Servers"));
        }
        if !self.config.layout.contains(name) {
            return Err(format!("„{name}“ steht nicht im Layout des Servers"));
        }
        if clients.contains_key(name) {
            return Err(format!("ein Client namens „{name}“ ist bereits verbunden"));
        }
        let mut outcome = self.trust.check(name, fingerprint);
        if outcome == TrustOutcome::Unknown {
            if let Err(e) = self.trust.reload() {
                warn!("Vertrauensliste konnte nicht neu geladen werden: {e:#}");
            }
            outcome = self.trust.check(name, fingerprint);
        }
        match outcome {
            TrustOutcome::Known => Ok(()),
            TrustOutcome::Unknown => {
                warn!(
                    "Unbekannter Client „{name}“ mit Fingerprint {fingerprint}. \
                     Zum Freigeben auf diesem Rechner ausführen: qross trust {name} {fingerprint}"
                );
                Err("dieser Rechner ist auf dem Server noch nicht freigegeben (qross trust)".into())
            }
            TrustOutcome::Mismatch => {
                warn!(
                    "Client „{name}“ meldet sich mit Fingerprint {fingerprint}, erwartet war {}",
                    self.trust.expected(name).unwrap_or("?")
                );
                Err("Fingerprint passt nicht zum freigegebenen Rechner dieses Namens".into())
            }
        }
    }
}

fn is_current(clients: &HashMap<String, ClientHandle>, name: &str, id: usize) -> bool {
    clients.get(name).is_some_and(|c| c.id == id)
}

/// Fürs Log, z. B. „2 Monitore (1512×982, 3744×1800)“
fn describe_monitors(monitors: &[Rect]) -> String {
    let sizes: Vec<String> = monitors.iter().map(|m| format!("{}×{}", m.width, m.height)).collect();
    match sizes.len() {
        0 => "keine Monitore".to_owned(),
        1 => format!("1 Monitor ({})", sizes[0]),
        n => format!("{n} Monitore ({})", sizes.join(", ")),
    }
}

/// Je Client die zuletzt übertragene Clipboard-Sequenznummer
type ClipSynced = Arc<Mutex<HashMap<String, u64>>>;

/// Für ausgehende Dateien: Grenze für kopierte Dateien (0 = nie), Meldungen, Fortschritt
struct FileSide<'a> {
    limit: u64,
    notify: &'a Notify,
    transfers: &'a Transfers,
}

/// Fertig übertragene Dateien eines Clients in die Zwischenablage legen.
fn deliver_files(ready: Ready, clipboard: &ClipboardHub, clip_synced: &ClipSynced, notify: &Notify) {
    let (clipboard, synced, notify) = (clipboard.clone(), clip_synced.clone(), notify.clone());
    tokio::spawn(async move {
        let Ready { peer, paths, data, names, bytes } = ready;
        match clipboard.write_files(paths, data).await {
            Ok(seq) => {
                info!("kopierte Dateien von „{peer}“ liegen in der Zwischenablage");
                // Nicht an denselben Client zurückschicken
                synced.lock().unwrap().insert(peer.clone(), seq);
                notify(FilesNotice::Ready { peer, names, bytes });
            }
            Err(e) => {
                warn!("kopierte Dateien von „{peer}“ nicht in die Zwischenablage gelegt: {e:#}");
                notify(FilesNotice::Failed { peer, reason: format!("{e:#}") });
            }
        }
    });
}

fn apply(
    action: Action,
    capture: &dyn Capture,
    clipboard: &ClipboardHub,
    clients: &HashMap<String, ClientHandle>,
    clip_synced: &ClipSynced,
    files: FileSide<'_>,
    drops: &mut Receiver,
) {
    match action {
        Action::CaptureRemote => capture.capture_remote(),
        Action::ReleaseCapture { x, y } => {
            capture.release(x, y);
            info!("Cursor zurück auf diesem Rechner");
        }
        Action::Send { to, input } => {
            match input {
                Input::Enter { .. } => info!("Cursor → „{to}“"),
                _ => debug!(?input, "→ „{to}“"),
            }
            if let Some(client) = clients.get(&to) {
                let _ = client.input.send(input);
            }
        }
        Action::Move { to, mv } => send_datagram(clients, &to, Datagram::MouseMove(mv)),
        Action::MoveBy { to, delta } => send_datagram(clients, &to, Datagram::MouseDelta(delta)),
        Action::SyncClipboard { to } => {
            let Some(client) = clients.get(&to) else { return };
            let rights = *client.sharing.borrow();
            let allowed = Allowed { clipboard: rights.clipboard.to_client(), files: rights.files.to_client() };
            if !allowed.any() {
                // Gar nicht erst lesen; wird es wieder erlaubt, geht der dann aktuelle Inhalt mit.
                return debug!("Zwischenablage nicht an „{to}“: nicht erlaubt");
            }
            let (clipboard, synced, conn) = (clipboard.clone(), clip_synced.clone(), client.conn.clone());
            let (limit, notify, transfers) = (files.limit, files.notify.clone(), files.transfers.clone());
            // Lesen und PNG-Kodierung im Clipboard-Thread; die Ereignisschleife läuft weiter.
            tokio::spawn(async move {
                let known = synced.lock().unwrap().get(&to).copied();
                let (seq, content) = clipboard.changed_since(known).await;
                synced.lock().unwrap().insert(to.clone(), seq);
                let Some(content) = content else { return };
                match rights::content(content, allowed) {
                    Some(content) => clipfiles::send(conn, content, limit, to, notify, transfers).await,
                    None => debug!("Zwischenablage nicht an „{to}“: nicht erlaubt"),
                }
            });
        }
        Action::ProbeDrag { x, y } => capture.probe_drag(x, y),
        Action::CancelLocalDrag => capture.cancel_local_drag(),
        Action::StartFileDrag { to, transfer, files: roots } => {
            if let Some(client) = clients.get(&to) {
                // Der Kern wechselt ohne Rechte gar nicht erst; nur zur Sicherheit
                if !client.sharing.borrow().drag.to_client() {
                    return warn!("Drag & Drop an „{to}“ ist nicht erlaubt");
                }
                let (conn, input, transfers) = (client.conn.clone(), client.input.clone(), files.transfers.clone());
                tokio::spawn(start_file_drag(conn, input, to, transfer, roots, transfers));
            }
        }
        Action::QueryRemoteDrag { to, probe } => {
            if let Some(client) = clients.get(&to) {
                let conn = client.conn.clone();
                tokio::spawn(async move {
                    if let Err(e) = send_drag_message(&conn, &DragMessage::Query { probe }).await {
                        debug!("Drag-Nachfrage an „{to}“ nicht gesendet: {e:#}");
                    }
                });
            }
        }
        Action::AwaitLocalDrop { from, transfer } => {
            info!("Dateien von „{from}“ werden hierher gezogen – Loslassen legt sie ab (Übertragung {transfer})");
            capture.await_local_drop();
        }
        Action::DropIncoming { transfer, x, y } => {
            let folder = capture.drop_folder(x, y).unwrap_or_else(files::fallback_drop_folder);
            drops.on_drop(transfer, folder);
        }
        Action::CancelIncoming { transfer } => {
            debug!("Übertragung {transfer} nicht abgelegt");
            drops.cancel_undropped();
        }
        Action::HeldBySecureInput { from: Some(from) } => {
            warn!("Sichere Tastatureingabe: Cursor von „{from}“ zurückgeholt – Tasten wären dort nicht angekommen")
        }
        Action::HeldBySecureInput { from: None } => info!("Sichere Tastatureingabe: kein Wechsel, der Cursor bleibt hier"),
    }
}

fn send_datagram(clients: &HashMap<String, ClientHandle>, to: &str, datagram: Datagram) {
    if let Some(client) = clients.get(to) {
        if let Err(e) = client.conn.send_datagram(encode_datagram(&datagram).into()) {
            debug!("Datagramm an „{to}“ nicht gesendet: {e}");
        }
    }
}

/// Dateien auflisten, `DragStart` senden und die Inhalte übertragen.
async fn start_file_drag(
    conn: Connection,
    input: mpsc::UnboundedSender<Input>,
    to: String,
    transfer: u64,
    roots: Vec<PathBuf>,
    transfers: Transfers,
) {
    let entries = match tokio::task::spawn_blocking(move || files::collect(&roots)).await {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => return warn!("Dateien für „{to}“ nicht lesbar: {e:#}"),
        Err(e) => return warn!("Dateien für „{to}“ nicht lesbar: {e}"),
    };
    let total: u64 = entries.iter().map(|e| e.entry.size).sum();
    info!("übertrage {} Einträge ({}) an „{to}“", entries.len(), files::human_size(total));
    let _ = input.send(Input::DragStart { transfer, entries: entries.iter().map(|e| e.entry.clone()).collect() });
    let started = std::time::Instant::now();
    match files::send_tracked(conn, StreamKind::File, transfer, entries, &to, &transfers).await {
        Ok(()) => {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            info!(
                "Übertragung an „{to}“ fertig in {secs:.1} s ({}/s)",
                files::human_size((total as f64 / secs) as u64)
            );
        }
        Err(e) => warn!("Übertragung an „{to}“ abgebrochen: {e:#}"),
    }
}

fn spawn_input_writer(conn: Connection, name: String) -> mpsc::UnboundedSender<Input> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Input>();
    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut stream = open_input(&conn).await?;
            while let Some(input) = rx.recv().await {
                write_frame(&mut stream, &input).await?;
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            debug!("Input-Stream zu „{name}“ beendet: {e:#}");
        }
    });
    tx
}

async fn accept_loop(endpoint: Endpoint, tx: mpsc::UnboundedSender<NetEvent>, stagings: Stagings) {
    while let Some(incoming) = endpoint.accept().await {
        let (tx, stagings) = (tx.clone(), stagings.clone());
        tokio::spawn(async move {
            let remote = incoming.remote_address();
            if let Err(e) = handle_connection(incoming, tx, stagings).await {
                debug!("Verbindung von {remote} beendet: {e:#}");
            }
        });
    }
}

async fn handle_connection(
    incoming: Incoming,
    tx: mpsc::UnboundedSender<NetEvent>,
    stagings: Stagings,
) -> Result<()> {
    let remote = incoming.remote_address();
    let conn = incoming.await.context("QUIC-Handshake")?;
    let fingerprint = peer_fingerprint(&conn)?;
    let (mut send, mut recv) = conn.accept_bi().await?;
    let Some(Control::Hello { version, name, screen, platform }) = read_frame(&mut recv).await? else {
        bail!("{remote} hat kein Hello gesendet");
    };

    let verdict = if version != PROTOCOL_VERSION {
        Err(format!("Protokollversion {version} wird nicht unterstützt (Server: {PROTOCOL_VERSION})"))
    } else {
        let (reply_tx, reply_rx) = oneshot::channel();
        let hello = NetEvent::Hello { name: name.clone(), screen: screen.bounds, fingerprint, conn: conn.clone(), reply: reply_tx };
        tx.send(hello).map_err(|_| anyhow!("Server wurde beendet"))?;
        reply_rx.await.map_err(|_| anyhow!("Server wurde beendet"))?
    };
    let sharing = match verdict {
        Ok(Admitted { server_name, sharing }) => {
            debug!("„{name}“ läuft unter {platform:?}");
            // Ältere Clients werten die Version nicht aus; neuere melden daraufhin ihre Monitore,
            // ob sie Zeichen eingeben können und, sobald dort etwas im Vollbild läuft, den Spielemodus.
            let welcome = Control::Welcome { version: SERVER_VERSION, server_name, platform: Platform::current() };
            write_frame(&mut send, &welcome).await?;
            tokio::spawn(send_sharing(send, sharing.clone(), name.clone()));
            sharing
        }
        Err(reason) => {
            write_frame(&mut send, &Control::Reject { reason }).await?;
            let _ = send.finish();
            // Dem Client Zeit geben, die Ablehnung zu lesen, bevor die Verbindung zugeht.
            let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
            return Ok(());
        }
    };

    let id = conn.stable_id();
    tokio::spawn(receive_client_streams(conn.clone(), name.clone(), id, tx.clone(), stagings, sharing));
    loop {
        match read_frame::<_, Control>(&mut recv).await {
            Ok(Some(Control::ScreenChanged(info))) => {
                let _ = tx.send(NetEvent::ScreenChanged { name: name.clone(), id, screen: info.bounds });
            }
            Ok(Some(Control::Monitors(mut monitors))) => {
                monitors.truncate(MAX_MONITORS);
                let _ = tx.send(NetEvent::Monitors { name: name.clone(), id, monitors });
            }
            Ok(Some(Control::Game(game))) => {
                let _ = tx.send(NetEvent::Game { name: name.clone(), id, game });
            }
            Ok(Some(Control::TypesText)) => {
                let _ = tx.send(NetEvent::TypesText { name: name.clone(), id });
            }
            Ok(Some(other)) => debug!(?other, "unerwartete Control-Nachricht"),
            Ok(None) | Err(_) => break,
        }
    }
    conn.close(0u32.into(), b"");
    let _ = tx.send(NetEvent::Closed { name, id });
    Ok(())
}

/// Schickt dem Client seine Rechte: gleich nach dem `Welcome` und nach jeder Änderung. Clients bis
/// Qross 0.6 lesen den Control-Stream danach nicht mehr; bei ihnen bleibt das ungelesen liegen.
async fn send_sharing(mut control: SendStream, mut sharing: watch::Receiver<Sharing>, name: String) {
    loop {
        let current = *sharing.borrow_and_update();
        if let Err(e) = write_frame(&mut control, &Control::Sharing(current)).await {
            return debug!("Rechte an „{name}“ nicht gesendet: {e}");
        }
        // Endet, sobald der Client getrennt ist
        if sharing.changed().await.is_err() {
            return;
        }
    }
}

/// Rechte entzogen: Laufende Übertragungen, die jetzt nicht mehr erlaubt sind, brechen ab, als hätte
/// man „Abbrechen“ gedrückt. Angekommenes wird dabei aus dem Staging gelöscht.
fn cancel_forbidden(transfers: &Transfers, name: &str, sharing: Sharing) {
    let forbidden = [
        (sharing.files.to_client(), Direction::Sending, Kind::Clipboard),
        (sharing.files.from_client(), Direction::Receiving, Kind::Clipboard),
        (sharing.drag.to_client(), Direction::Sending, Kind::Drag),
        (sharing.drag.from_client(), Direction::Receiving, Kind::Drag),
    ];
    for (_, direction, kind) in forbidden.into_iter().filter(|(allowed, ..)| !allowed) {
        let cancelled = transfers.cancel_with(name, direction, kind);
        if cancelled > 0 {
            info!("{cancelled} laufende Übertragung(en) mit „{name}“ abgebrochen: nicht mehr erlaubt");
        }
    }
}

/// Lehnt einen Datei-Stream ab, bevor etwas davon im Staging landet (`STOP_SENDING`; der Sender
/// hört beim nächsten Schreiben auf).
fn refuse(stream: &mut RecvStream, what: &str) -> anyhow::Error {
    let _ = stream.stop(0u32.into());
    anyhow!("{what} nicht erlaubt")
}

/// Streams vom Client: Zwischenablage (Inhalte und kopierte Dateien) und Drag & Drop zum Server.
/// Was der Client nicht abgeben darf, wird hier abgewiesen, auch bei Clients, die ihre Rechte nicht
/// kennen (bis Qross 0.6) oder sich nicht daran halten.
async fn receive_client_streams(
    conn: Connection,
    name: String,
    id: usize,
    tx: mpsc::UnboundedSender<NetEvent>,
    stagings: Stagings,
    sharing: watch::Receiver<Sharing>,
) {
    while let Ok(mut stream) = conn.accept_uni().await {
        let (name, tx, stagings, sharing) = (name.clone(), tx.clone(), stagings.clone(), sharing.clone());
        tokio::spawn(async move {
            let result: Result<()> = async {
                match read_kind(&mut stream).await? {
                    StreamKind::Clipboard => {
                        let Some(message) = read_frame::<_, ClipboardMsg>(&mut stream).await? else { return Ok(()) };
                        let rights = *sharing.borrow();
                        let allowed = Allowed { clipboard: rights.clipboard.from_client(), files: rights.files.from_client() };
                        if matches!(message, ClipboardMsg::Files { .. }) && !allowed.files {
                            info!("kopierte Dateien von „{name}“ abgewiesen: nicht erlaubt");
                        }
                        match rights::message(message, allowed) {
                            Some(ClipboardMsg::Data(data)) => {
                                let _ = tx.send(NetEvent::Clipboard { name, id, data });
                            }
                            Some(ClipboardMsg::Files { transfer, entries, data }) => {
                                let _ = tx.send(NetEvent::ClipboardFiles { name, id, transfer, entries, data });
                            }
                            None if !allowed.clipboard => info!("Zwischenablage von „{name}“ abgewiesen: nicht erlaubt"),
                            None => {}
                        }
                    }
                    StreamKind::ClipboardFile => {
                        if !sharing.borrow().files.from_client() {
                            return Err(refuse(&mut stream, "kopierte Datei"));
                        }
                        let outcome = files::receive_file(&mut stream, &stagings.clipboard, &stagings.transfers).await?;
                        let _ = tx.send(NetEvent::ClipboardFile { name, id, outcome });
                    }
                    StreamKind::Drag => {
                        if let Some(DragMessage::Offer { probe, transfer, mut entries }) = read_frame(&mut stream).await? {
                            if !entries.is_empty() && !sharing.borrow().drag.from_client() {
                                // Wie „keine Dateien“: Der Server wechselt nicht und nimmt nichts an.
                                info!("gezogene Dateien von „{name}“ abgewiesen: nicht erlaubt");
                                entries.clear();
                            }
                            let _ = tx.send(NetEvent::DragOffer { name, id, probe, transfer, entries });
                        }
                    }
                    StreamKind::File => {
                        if !sharing.borrow().drag.from_client() {
                            return Err(refuse(&mut stream, "gezogene Datei"));
                        }
                        let outcome = files::receive_file(&mut stream, &stagings.drag, &stagings.transfers).await?;
                        let _ = tx.send(NetEvent::DragFile { name, id, outcome });
                    }
                    other => bail!("Client darf keine Streams vom Typ {other:?} öffnen"),
                }
                Ok(())
            }
            .await;
            if let Err(e) = result {
                debug!("Stream vom Client verworfen: {e:#}");
            }
        });
    }
}
