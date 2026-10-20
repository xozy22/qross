//! Server-Laufzeit: nimmt Clients an, verbindet Eingabe-Erfassung, `ServerState` und Netzwerk.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use qross_core::finder::{self as cursor_finder, Cue};
use qross_core::keys::{KeyConfig, KeyMode};
use qross_core::server::{Action, Hold, ServerEvent, ServerState, SwitchOptions};
use qross_core::sharing::{describe as describe_sharing, SharingRules};
use qross_core::{CaptureEvent, Layout};
use qross_platform::{Capture, Clipboard, FinderOverlay, SecureInput};
use qross_proto::{
    encode_datagram, is_sampled_input, is_sampled_move, read_frame, read_frame_up_to, write_frame, Clipboard as ClipboardMsg,
    ClipboardData, Control, CursorFinder, Datagram, DragMessage, FileEntry, GameState, Input, Latency, LatencySummary,
    Platform, Rect, SampleId, Sharing, StreamKind, MAX_MONITORS, PROTOCOL_VERSION, SERVER_VERSION,
};
use quinn::{Connection, Endpoint, Incoming, RecvStream, SendStream};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::clipboard::ClipboardHub;
use crate::clipfiles::{self, FilesNotice, Notify, Ready};
use crate::drag_preview::DragView;
use crate::files::{FileOutcome, Receiver, Staging};
use crate::game::{self, Detector};
use crate::latency::{self, Meter};
use crate::ondemand::{self, Link, Offered, Offers, Pastes};
use crate::progress::{Direction, Kind, Progress, Transfers};
use crate::quality::{self, Quality};
use crate::rights::{self, Allowed};
use crate::screensaver;
use crate::secure_input;
use crate::settle::{self, MouseActivity};
use crate::{discovery, files};
use crate::streams::{open_input, read_kind, send_drag_message};
use crate::switch_trace::{EdgeHits, ServerTrace};
use crate::tls::{peer_fingerprint, server_config};
use crate::wake::{self, SystemEvent};
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
    /// Cursor-Finder dieses Rechners, wenn der Cursor zurückkommt (üblich:
    /// `qross_platform::finder_overlay`); `None`: hier nichts zeigen, Clients bekommen ihn trotzdem
    pub finder: Option<Arc<dyn FinderOverlay>>,
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
    /// Ruhezustand, Aufwachen, Netzwechsel; ohne Vorgabe vom Betriebssystem
    system_events: Option<mpsc::UnboundedReceiver<SystemEvent>>,
    /// Sichere Tastatureingabe abfragen (macOS)
    secure_input: Option<secure_input::Detector>,
    /// Cursor-Finder für alle Rechner, auch während des Laufs änderbar
    cursor_finder: CursorFinderHandle,
    /// Bildschirmschoner mit den Clients abgleichen; mit `lock_clients` sperren sie sich stattdessen
    screen_saver: bool,
    lock_clients: bool,
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

/// Ändert den Cursor-Finder eines laufenden Servers (siehe [`Server::cursor_finder`]). Er gilt sofort
/// für alle Rechner: für den Server selbst und für verbundene Clients ab Qross 0.8, die ihn ohne
/// neue Verbindung erfahren.
#[derive(Clone)]
pub struct CursorFinderHandle(Arc<watch::Sender<CursorFinder>>);

impl CursorFinderHandle {
    pub fn set(&self, finder: CursorFinder) {
        self.0.send_if_modified(|current| {
            let changed = *current != finder;
            if changed {
                info!("Cursor-Finder: {}", cursor_finder::describe(finder));
            }
            *current = finder;
            changed
        });
    }

    pub fn get(&self) -> CursorFinder {
        *self.0.borrow()
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
    /// Verzögerung der Eingabekette zu einem Client, etwa jede Sekunde, solange sich etwas ändert
    /// (siehe [`latency`]). Ohne `client_reports` (Client älter als `LATENCY_SINCE`) gibt es nur den
    /// Server-Anteil.
    Latency { name: String, summary: LatencySummary, client_reports: bool },
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
    SyncsLocks { name: String, id: usize },
    FollowsScreenSaver { name: String, id: usize },
    Clipboard { name: String, id: usize, data: ClipboardData },
    ClipboardFiles { name: String, id: usize, transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    ClipboardFile { name: String, id: usize, outcome: FileOutcome },
    /// Der Client fügt kopierte Dateien auf Abruf ein (`Control::PastesOnDemand`)
    PastesOnDemand { name: String, id: usize },
    /// Kopierte Dateien des Clients auf Abruf: nur die Liste
    ClipboardFilesOnDemand { name: String, id: usize, transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    /// Antwort auf eine Drag-Nachfrage (Drag & Drop vom Client zum Server)
    DragOffer { name: String, id: usize, probe: u32, transfer: u64, entries: Vec<FileEntry> },
    DragFile { name: String, id: usize, outcome: FileOutcome },
    Closed { name: String, id: usize },
}

/// Angenommen: Name des Servers fürs `Welcome`, die Rechte des Clients, die sich mit den Regeln ändern,
/// der Cursor-Finder, weitere Nachrichten an ihn auf dem Control-Stream, die Messung der Eingabekette und
/// was er beim Einfügen abrufen darf
struct Admitted {
    server_name: String,
    sharing: watch::Receiver<Sharing>,
    cursor_finder: watch::Receiver<CursorFinder>,
    control: mpsc::UnboundedReceiver<Control>,
    latency: latency::Shared,
    /// Der Server fügt kopierte Dateien auf Abruf ein (er sagt es dem Client).
    pastes_on_demand: bool,
    offers: Offers,
    /// Seine Abrufe beim Server
    serving: Pastes,
}

/// Verbindungen und Rechte der Clients, für Abrufe kopierter Dateien beim Einfügen (aus Systemthreads)
#[derive(Clone, Default)]
struct Peers(Arc<Mutex<HashMap<String, Peer>>>);

/// Verbindung eines Clients und was er teilen darf
type Peer = (Connection, watch::Receiver<Sharing>);

impl Peers {
    fn set(&self, name: &str, conn: Connection, sharing: watch::Receiver<Sharing>) {
        self.0.lock().unwrap().insert(name.to_owned(), (conn, sharing));
    }

    /// Nur, wenn es noch dieselbe Verbindung ist
    fn remove(&self, name: &str, id: usize) {
        let mut peers = self.0.lock().unwrap();
        if peers.get(name).is_some_and(|(conn, _)| conn.stable_id() == id) {
            peers.remove(name);
        }
    }

    /// Die jeweils aktuelle Verbindung zu `name`, solange er kopierte Dateien abgeben darf
    fn link(&self, name: &str) -> Link {
        let (peers, name) = (self.clone(), name.to_owned());
        Arc::new(move || {
            let peers = peers.0.lock().unwrap();
            let (conn, sharing) = peers.get(&name).ok_or_else(|| format!("„{name}“ ist nicht verbunden"))?;
            let allowed = sharing.borrow().files.from_client();
            if !allowed {
                return Err(format!("„{name}“ darf keine kopierten Dateien mehr abgeben"));
            }
            Ok(conn.clone())
        })
    }
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
    input: InputTx,
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
    /// Nachrichten auf dem Control-Stream nach dem `Welcome` (außer den Rechten)
    control: mpsc::UnboundedSender<Control>,
    /// Folgt dem Bildschirmschoner des Servers (`Control::FollowsScreenSaver`)
    follows_saver: bool,
    /// Fügt kopierte Dateien auf Abruf ein (`Control::PastesOnDemand`)
    pastes_on_demand: bool,
    /// Verzögerung der Eingabekette; das Ergebnis geht jede Sekunde hinaus, solange er verbunden ist
    /// (an die Oberfläche und über den Control-Stream an ihn)
    latency: latency::Shared,
    _latency_reports: AbortOnDrop,
}

/// Eingaben an den Input-Stream eines Clients, mit dem Zeitpunkt ihrer Erfassung (Stichproben der
/// Eingabekette; `None`, wenn sie nicht aus der Erfassung stammen)
type InputTx = mpsc::UnboundedSender<(Input, Option<Instant>)>;

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
            system_events: None,
            secure_input: None,
            cursor_finder: CursorFinderHandle(Arc::new(watch::channel(CursorFinder::OFF).0)),
            screen_saver: true,
            lock_clients: false,
        })
    }

    /// Ruhezustand, Aufwachen und Netzwechsel aus `events` statt vom Betriebssystem (für Tests).
    /// Vor dem Ruhezustand trennt der Server alle Clients; nach dem Aufwachen und bei einem Netzwechsel
    /// trennt er jeden, von dem eine Weile nichts mehr kommt.
    pub fn with_system_events(mut self, events: mpsc::UnboundedReceiver<SystemEvent>) -> Self {
        self.system_events = Some(events);
        self
    }

    /// Sichere Tastatureingabe abfragen (macOS; üblich: `qross_platform::secure_input`, Standard:
    /// gar nicht). Solange ein Programm sie hält, erreichen Tasten die Erfassung nicht, sondern
    /// dieses Programm: Der Cursor kommt dann von einem Client zurück und bleibt hier, bis sie
    /// endet. Das melden `ServerNotice::SecureInput` und `ServerNotice::SecureInputHold`.
    pub fn with_secure_input(mut self, detect: secure_input::Detector) -> Self {
        self.secure_input = Some(detect);
        self
    }

    /// Cursor-Finder (Standard: aus): Beim Wechsel blitzt auf dem Rechner, auf dem der Cursor
    /// ankommt, ein Ring um den Cursor bzw. sein Name auf. Clients ab Qross 0.8 bekommen das mitgeteilt;
    /// ändern im Betrieb über [`Server::cursor_finder`].
    pub fn with_cursor_finder(self, finder: CursorFinder) -> Self {
        self.cursor_finder.0.send_replace(finder);
        self
    }

    /// Griff, um den Cursor-Finder im Betrieb zu ändern
    pub fn cursor_finder(&self) -> CursorFinderHandle {
        self.cursor_finder.clone()
    }

    /// Bildschirmschoner abgleichen (Standard: an, ohne Sperren): Beginnt er hier, starten Clients
    /// ab Qross 0.8 ihren, mit `lock_clients` sperren sie sich stattdessen; endet er, beenden sie
    /// ihren. Ein Client, den das sperrt, wird an seiner eigenen Tastatur entsperrt.
    pub fn with_screen_saver(mut self, sync: bool, lock_clients: bool) -> Self {
        self.screen_saver = sync;
        self.lock_clients = lock_clients;
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

    /// Gibt das Ergebnis der Messung der Eingabekette jede Sekunde heraus, wenn sich etwas geändert
    /// hat: an die Oberfläche und, sobald er mitmisst, an den Client.
    fn report_latency(&self, name: &str, latency: &latency::Shared, control: mpsc::UnboundedSender<Control>) -> AbortOnDrop {
        let (notices, name, meter) = (self.notices.clone(), name.to_owned(), latency.clone());
        AbortOnDrop(tokio::spawn(async move {
            let mut tick = tokio::time::interval(latency::SUMMARY_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let (slow, summary) = {
                    let mut meter = meter.lock().unwrap();
                    (meter.take_slow(), meter.take_summary(Instant::now()).map(|s| (s, meter.client_reports())))
                };
                // Höchstens einmal je Sekunde: der langsamste mit seinen Teilen
                if let Some(slow) = slow {
                    warn!("Eingabe an „{name}“ verzögert: {}", slow.describe());
                }
                let Some((summary, client_reports)) = summary else { continue };
                if let Some(tx) = &notices {
                    let _ = tx.send(ServerNotice::Latency { name: name.clone(), summary, client_reports });
                }
                if client_reports && control.send(Control::Latency(Latency::Summary(summary))).is_err() {
                    return;
                }
            }
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
        // Kopierte Dateien auf Abruf: was Clients hier abrufen dürfen, ihre Abrufe hier und die hiesigen bei ihnen
        let offers = Offers::default();
        let serving = Pastes::new(Direction::Sending, transfers.clone());
        let pasting = Pastes::new(Direction::Receiving, transfers.clone());
        let peers = Peers::default();
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

        let ServerPlatform { capture, mut events, clipboard, screen, monitors, game, finder } = platform;
        // Ohne Spielemodus wird gar nicht erst nachgesehen; der Sender hält den Kanal dann offen.
        let (_no_game, mut local_game) = tokio::sync::watch::channel(GameState::default());
        if self.game_mode {
            local_game = game::watch(game);
        }
        let (_no_secure_input, mut secure) = watch::channel(None);
        if let Some(detect) = self.secure_input.take() {
            secure = secure_input::watch(detect);
        }
        // Ebenso beim Bildschirmschoner
        let (_no_saver, mut local_saver) = watch::channel(false);
        if self.screen_saver {
            let capture = capture.clone();
            local_saver = screensaver::watch(Box::new(move || capture.screen_saver_active()));
        }
        let clipboard = ClipboardHub::spawn(clipboard, self.share_concealed, {
            let notices = self.notices.clone();
            move || {
                if let Some(tx) = &notices {
                    let _ = tx.send(ServerNotice::ClipboardConcealed);
                }
            }
        });
        // Auf Abruf nur mit einem einzigen Client: Was ein Client kopiert, gibt der Server sonst als
        // Datei an den nächsten weiter, und virtuelle Dateien hat er dafür nicht.
        let clients_in_layout = self.config.layout.screens().filter(|s| *s != self.config.name).count();
        let paste_here = clipboard.pastes_on_demand() && clients_in_layout == 1;
        if paste_here {
            info!("Kopierte Dateien vom Client kommen erst beim Einfügen");
        } else if clipboard.pastes_on_demand() {
            info!("Kopierte Dateien von Clients kommen vorab: Mit mehreren Clients gibt der Server sie weiter");
        }
        let files_notify: Notify = {
            let notices = self.notices.clone();
            Arc::new(move |notice| {
                if let Some(tx) = &notices {
                    let _ = tx.send(ServerNotice::ClipboardFiles(notice));
                }
            })
        };
        // Große Inhalte der Zwischenablage warten nach einem Wechsel, bis die Maus ruht.
        let mouse = MouseActivity::default();
        let mut local_screen = screen();
        let mut local_monitors = monitors();
        let mut state = ServerState::new(self.config.name.clone(), self.config.layout.clone(), local_screen);
        state.handle(ServerEvent::LocalMonitors(local_monitors.clone()));
        state.set_switch_options(self.switching);
        state.set_game_mode(self.game_mode);
        state.set_lock_clients(self.lock_clients);
        state.set_key_maps(self.keys.remap.clone());
        state.set_key_modes(self.keys.modes.clone());
        // Welche Sperrtasten der Server lesen kann; Clients übernehmen dann seinen Stand
        let locks = capture.locks();
        if locks.is_known() {
            debug!("Sperrtasten hier: {}", locks.describe());
        } else {
            info!("Feststelltaste und Num sind hier nicht lesbar – Clients gleichen sie nicht ab");
        }
        state.handle(ServerEvent::LocalLocks(locks));
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
        let mut system_events = self.system_events.take().unwrap_or_else(wake::system_events);
        let finder_style = self.cursor_finder.0.subscribe();
        if finder_style.borrow().is_on() {
            info!("Cursor-Finder: {}", cursor_finder::describe(*finder_style.borrow()));
        }
        // Was ein Client hierher zieht, steht am Cursor, bis es abgelegt oder verworfen ist.
        let mut drag_view = DragView::new(finder.clone());
        info!("Server „{}“ lauscht auf {}", self.config.name, self.local_addr()?);
        if !local_monitors.is_empty() {
            info!("eigene Monitore: {}", describe_monitors(&local_monitors));
        }
        self.notify(ServerNotice::Listening { addr: self.local_addr()? });
        self.notify(ServerNotice::LocalScreenChanged { screen: local_screen, monitors: local_monitors.clone() });

        // Protokoll des letzten Wechsels auf einen Client (siehe `switch_trace`)
        let mut edge_hits = EdgeHits::default();
        let mut switching: Option<ServerTrace> = None;

        loop {
            if switching.as_ref().is_some_and(|trace| trace.is_due(Instant::now())) {
                switching.take().expect("geprüft").log();
            }
            // Wann das auslösende Ereignis aus der Erfassung abgeholt wurde (Eingabekette)
            let mut captured = None;
            let actions = tokio::select! {
                event = events.recv() => match event {
                    Some(CaptureEvent::MouseDelta { mut dx, mut dy }) => {
                        captured = Some(Instant::now());
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
                        if let Some(at) = captured {
                            mouse.moved(at);
                            if let Some(trace) = &mut switching {
                                trace.on_move(at, dx, dy);
                            }
                        }
                        let mut actions = state.handle(ServerEvent::Capture(CaptureEvent::MouseDelta { dx, dy }));
                        if let Some(event) = next {
                            if matches!(event, CaptureEvent::EdgeHit { .. }) {
                                edge_hits.hit(Instant::now());
                            }
                            actions.extend(self.on_capture(&mut state, event));
                        }
                        actions
                    }
                    Some(event) => {
                        captured = Some(Instant::now());
                        if matches!(event, CaptureEvent::EdgeHit { .. }) {
                            edge_hits.hit(Instant::now());
                        }
                        self.on_capture(&mut state, event)
                    }
                    None => bail!("Eingabe-Erfassung wurde beendet"),
                },
                Some(event) = net_rx.recv() => match event {
                    NetEvent::Hello { name, screen, fingerprint, conn, reply } => {
                        let addr = conn.remote_address();
                        match self.admit(&name, &fingerprint) {
                            Ok(()) => {
                                // Derselbe Rechner (Name und Schlüssel) meldet sich neu, etwa nach dem
                                // Aufwachen: Die alte Verbindung ist tot oder gleich überholt. Nicht
                                // abweisen, bis sie ausläuft, sondern gleich ersetzen.
                                let mut actions = Vec::new();
                                if let Some(old) = clients.remove(&name) {
                                    info!("„{name}“ meldet sich neu an – alte Verbindung ersetzt");
                                    old.conn.close(wake::CLOSE_REPLACED, b"replaced");
                                    transfers.abort_receiving(Some(&name), "Verbindung ersetzt");
                                    self.notify(ServerNotice::ClientDisconnected { name: name.clone() });
                                    actions = state.handle(ServerEvent::ClientDisconnected { name: name.clone() });
                                }
                                self.notify(ServerNotice::ClientConnected { name: name.clone(), addr, screen });
                                let latency = Meter::shared(Instant::now());
                                let input = spawn_input_writer(conn.clone(), name.clone(), latency.clone());
                                let quality = self.monitor_quality(&conn, &name);
                                let sharing = rules.borrow().for_client(&name);
                                let (sharing_tx, sharing_rx) = watch::channel(sharing);
                                let (control_tx, control_rx) = mpsc::unbounded_channel();
                                let latency_reports = self.report_latency(&name, &latency, control_tx.clone());
                                peers.set(&name, conn.clone(), sharing_rx.clone());
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
                                    control: control_tx,
                                    follows_saver: false,
                                    pastes_on_demand: false,
                                    latency: latency.clone(),
                                    _latency_reports: latency_reports,
                                };
                                clients.insert(name.clone(), handle);
                                clip_synced.lock().unwrap().remove(&name);
                                let _ = reply.send(Ok(Admitted {
                                    server_name: self.config.name.clone(),
                                    sharing: sharing_rx,
                                    cursor_finder: self.cursor_finder.0.subscribe(),
                                    control: control_rx,
                                    latency,
                                    pastes_on_demand: paste_here,
                                    offers: offers.clone(),
                                    serving: serving.clone(),
                                }));
                                info!("Client „{name}“ verbunden ({}×{})", screen.width, screen.height);
                                if sharing != Sharing::ALL {
                                    info!("Rechte für „{name}“: {}", describe_sharing(sharing));
                                }
                                actions.extend(state.handle(ServerEvent::ClientConnected { name, screen }));
                                actions
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
                    NetEvent::SyncsLocks { name, id } if is_current(&clients, &name, id) => {
                        debug!("„{name}“ übernimmt Feststelltaste, Num und Rollen vom Server");
                        state.handle(ServerEvent::ClientSyncsLocks { name })
                    }
                    NetEvent::FollowsScreenSaver { name, id } if is_current(&clients, &name, id) => {
                        if let Some(client) = clients.get_mut(&name) {
                            client.follows_saver = true;
                        }
                        debug!("„{name}“ folgt dem Bildschirmschoner");
                        Vec::new()
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
                    NetEvent::PastesOnDemand { name, id } if is_current(&clients, &name, id) => {
                        if let Some(client) = clients.get_mut(&name) {
                            client.pastes_on_demand = true;
                        }
                        debug!("„{name}“ fügt kopierte Dateien auf Abruf ein");
                        Vec::new()
                    }
                    NetEvent::ClipboardFilesOnDemand { name, id, transfer, entries, data } if is_current(&clients, &name, id) => {
                        // Was vorab unterwegs war, ist überholt.
                        incoming.supersede();
                        match Offered::new(name.clone(), transfer, entries, peers.link(&name), pasting.clone()) {
                            Ok(offered) => {
                                let (clipboard, synced, notify) = (clipboard.clone(), clip_synced.clone(), files_notify.clone());
                                tokio::spawn(async move {
                                    if let Some(seq) = ondemand::deliver(offered, data, &clipboard, &notify).await {
                                        // Nicht an denselben Client zurückschicken
                                        synced.lock().unwrap().insert(name, seq);
                                    }
                                });
                            }
                            Err(e) => {
                                warn!("kopierte Dateien von „{name}“ abgelehnt: {e:#}");
                                files_notify(FilesNotice::Failed { peer: name, reason: format!("{e:#}") });
                            }
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
                        if let Some(client) = clients.remove(&name) {
                            let summary = client.latency.lock().unwrap().summary(Instant::now());
                            if summary.mouse.is_some() || summary.keys.is_some() {
                                info!("Verzögerung der Eingabekette zu „{name}“: {}", latency::describe(&summary));
                            }
                        }
                        info!("Client „{name}“ getrennt");
                        peers.remove(&name, id);
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
                Ok(()) = local_saver.changed() => {
                    let active = *local_saver.borrow_and_update();
                    let following = clients.values().filter(|c| c.follows_saver).count();
                    match (active, self.lock_clients) {
                        (true, false) => info!("Bildschirmschoner gestartet – {following} Client(s) starten ihren"),
                        (true, true) => info!("Bildschirmschoner gestartet – {following} Client(s) sperren sich"),
                        (false, _) => info!("Bildschirmschoner beendet – {following} Client(s) beenden ihren"),
                    }
                    state.handle(ServerEvent::LocalScreenSaver(active))
                }
                Ok(()) = local_game.changed() => {
                    let game = *local_game.borrow_and_update();
                    debug!("Spielemodus: auf diesem Rechner {}", game::describe(game));
                    state.handle(ServerEvent::LocalGame(game))
                }
                Some(event) = system_events.recv() => {
                    on_system_event(event, &clients);
                    Vec::new()
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
                match &action {
                    Action::Send { to, input: Input::Enter { .. } } => {
                        if let Some(earlier) = switching.take() {
                            earlier.log();
                        }
                        let at = captured.unwrap_or_else(Instant::now);
                        switching = Some(ServerTrace::start(to.clone(), at, &mut edge_hits));
                    }
                    Action::ReleaseCapture { .. } => {
                        if let Some(mut trace) = switching.take() {
                            trace.on_back(Instant::now());
                            trace.log();
                        }
                    }
                    _ => {}
                }
                self.notify_action(&action);
                self.explain_key_mode(&action, &clients);
                if let Some(overlay) = &finder {
                    match cursor_finder::on_server(&action, *finder_style.borrow()) {
                        Some(Cue::Show(style)) => overlay.show(style, &self.config.name),
                        Some(Cue::Hide) => overlay.hide(),
                        None => {}
                    }
                }
                match &action {
                    Action::AwaitLocalDrop { transfer, .. } => drag_view.show(*transfer, &drops),
                    Action::DropIncoming { .. } | Action::CancelIncoming { .. } => drag_view.hide(),
                    _ => {}
                }
                let files =
                    FileSide { limit: self.clipboard_file_limit, notify: &files_notify, transfers: &transfers, mouse: &mouse, offers: &offers };
                let (step, started) = (platform_step(&action), Instant::now());
                apply(action, &*capture, &clipboard, &clients, &clip_synced, files, &mut drops, captured);
                let took = started.elapsed();
                if let Some(step) = step.filter(|_| took >= SLOW_STEP) {
                    warn!("„{step}“ hielt die Weitergabe {} ms auf – so lange stand der Cursor auf dem Client", took.as_millis());
                }
            }
            self.report_hold(&state, &mut hold);
            // Abgebrochen (hier, vom Client, weil Rechte entzogen wurden oder er fort ist) oder fehlgeschlagen
            drag_view.check(&drops);
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

    /// Darf `name` mit diesem Schlüssel herein? Ist er schon verbunden, ersetzt die neue Verbindung die
    /// alte – denselben Namen mit einem anderen Schlüssel weist die Vertrauensliste ab.
    fn admit(&mut self, name: &str, fingerprint: &Fingerprint) -> Result<(), String> {
        if name == self.config.name {
            return Err(format!("„{name}“ ist der Name des Servers"));
        }
        if !self.config.layout.contains(name) {
            return Err(format!("„{name}“ steht nicht im Layout des Servers"));
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

/// Vor dem Ruhezustand alle Clients trennen: Sie merken es sofort, statt erst nach der
/// Zeitüberschreitung. Nach dem Aufwachen und bei einem Netzwechsel jeden trennen, von dem nichts
/// mehr kommt – unter macOS und Linux stand die Uhr im Ruhezustand still, quinn hielte tote
/// Verbindungen sonst noch bis zu 5 s.
fn on_system_event(event: SystemEvent, clients: &HashMap<String, ClientHandle>) {
    match event {
        SystemEvent::Sleeping => {
            for (name, client) in clients {
                debug!("Ruhezustand: trenne „{name}“");
                client.conn.close(wake::CLOSE_SLEEPING, b"server sleeping");
            }
        }
        SystemEvent::Woke | SystemEvent::NetworkChanged => {
            for (name, client) in clients {
                let (conn, name) = (client.conn.clone(), name.clone());
                tokio::spawn(async move {
                    if !wake::answers(conn.clone(), wake::SERVER_PROBE).await && conn.close_reason().is_none() {
                        info!("„{name}“ antwortet nicht mehr – getrennt");
                        conn.close(0u32.into(), b"no answer");
                    }
                });
            }
        }
    }
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

/// Für die ausgehende Zwischenablage und Dateien: Grenze für kopierte Dateien (0 = nie), Meldungen,
/// Fortschritt, wann sich die Maus zuletzt bewegt hat und was Clients auf Abruf holen dürfen
struct FileSide<'a> {
    limit: u64,
    notify: &'a Notify,
    transfers: &'a Transfers,
    mouse: &'a MouseActivity,
    offers: &'a Offers,
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

/// Ab so lange gilt ein Schritt der Ereignisschleife als Hänger: Solange gehen keine Bewegungen hinaus.
const SLOW_STEP: Duration = Duration::from_millis(20);

/// Schritte, die das System direkt aufrufen und dabei die Ereignisschleife aufhalten können, fürs Log
fn platform_step(action: &Action) -> Option<&'static str> {
    Some(match action {
        Action::CaptureRemote => "Cursor einfangen",
        Action::ReleaseCapture { .. } => "Cursor freigeben",
        Action::SyncLocks { .. } => "Sperrtasten lesen",
        Action::ProbeDrag { .. } => "Ziehen prüfen",
        Action::CancelLocalDrag => "Ziehen abbrechen",
        Action::AwaitLocalDrop { .. } => "Ablegen vorbereiten",
        Action::DropIncoming { .. } => "Ablageordner suchen",
        Action::StopLocalScreenSaver => "Bildschirmschoner beenden",
        _ => return None,
    })
}

/// `captured`: wann das auslösende Ereignis aus der Erfassung kam (Stichproben der Eingabekette)
#[allow(clippy::too_many_arguments)]
fn apply(
    action: Action,
    capture: &dyn Capture,
    clipboard: &ClipboardHub,
    clients: &HashMap<String, ClientHandle>,
    clip_synced: &ClipSynced,
    files: FileSide<'_>,
    drops: &mut Receiver,
    captured: Option<Instant>,
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
                let _ = client.input.send((input, captured));
            }
        }
        Action::SyncLocks { to } => {
            // Erst jetzt lesen: Lokal kann sich der Stand geändert haben, ohne dass es jemand sah.
            let locks = capture.locks();
            if let (true, Some(client)) = (locks.is_known(), clients.get(&to)) {
                debug!("Sperrtasten → „{to}“: {}", locks.describe());
                let _ = client.input.send((Input::Locks(locks), None));
            }
        }
        Action::Move { to, mv } => send_datagram(clients, &to, Datagram::MouseMove(mv), captured),
        Action::MoveBy { to, delta } => send_datagram(clients, &to, Datagram::MouseDelta(delta), captured),
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
            let (mouse, since) = (files.mouse.clone(), Instant::now());
            let on_demand = client.pastes_on_demand.then(|| files.offers.clone());
            // Lesen und PNG-Kodierung im Clipboard-Thread; die Ereignisschleife läuft weiter.
            tokio::spawn(async move {
                let known = synced.lock().unwrap().get(&to).copied();
                let (seq, content) = clipboard.changed_since(known).await;
                synced.lock().unwrap().insert(to.clone(), seq);
                let Some(content) = content else { return };
                let Some(content) = rights::content(content, allowed) else {
                    return debug!("Zwischenablage nicht an „{to}“: nicht erlaubt");
                };
                // Bilder und Dateien erst, wenn die Maus drüben zur Ruhe kommt (siehe `settle`)
                if settle::is_bulky(&content, limit) {
                    settle::settled(&mouse, since).await;
                    // Inzwischen kam Neueres an ihn oder von ihm: das Ältere nicht hinterherschicken
                    if synced.lock().unwrap().get(&to) != Some(&seq) || conn.close_reason().is_some() {
                        return debug!("Zwischenablage an „{to}“ entfällt: überholt oder getrennt, bevor die Maus ruhte");
                    }
                    debug!("Zwischenablage an „{to}“ geht {} ms nach dem Wechsel hinaus", since.elapsed().as_millis());
                }
                clipfiles::send(conn, content, limit, to, notify, transfers, on_demand).await
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
        Action::ScreenSaver { active, lock } => {
            for client in clients.values().filter(|c| c.follows_saver) {
                let _ = client.control.send(Control::ScreenSaver { active, lock });
            }
        }
        Action::StopLocalScreenSaver => {
            info!("Eingabe für einen Client – Bildschirmschoner hier wird beendet");
            capture.stop_screen_saver();
        }
    }
}

fn send_datagram(clients: &HashMap<String, ClientHandle>, to: &str, datagram: Datagram, captured: Option<Instant>) {
    let Some(client) = clients.get(to) else { return };
    if let Err(e) = client.conn.send_datagram(encode_datagram(&datagram).into()) {
        return debug!("Datagramm an „{to}“ nicht gesendet: {e}");
    }
    let (epoch, seq) = match datagram {
        Datagram::MouseMove(mv) => (mv.epoch, mv.seq),
        Datagram::MouseDelta(delta) => (delta.epoch, delta.seq),
    };
    if let Some(captured) = captured.filter(|_| is_sampled_move(seq)) {
        client.latency.lock().unwrap().sent(SampleId::Mouse { epoch, seq }, captured, Instant::now());
    }
}

/// Dateien auflisten, `DragStart` senden und die Inhalte übertragen.
async fn start_file_drag(
    conn: Connection,
    input: InputTx,
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
    let _ = input.send((Input::DragStart { transfer, entries: entries.iter().map(|e| e.entry.clone()).collect() }, None));
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

/// Schreibt die Eingaben auf den Input-Stream. Ihre laufende Nummer dort kennt auch der Client; daran
/// ordnet die Messung der Eingabekette seine Rückmeldungen zu.
fn spawn_input_writer(conn: Connection, name: String, latency: latency::Shared) -> InputTx {
    let (tx, mut rx) = mpsc::unbounded_channel::<(Input, Option<Instant>)>();
    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut stream = open_input(&conn).await?;
            let mut index = 0u64;
            while let Some((input, captured)) = rx.recv().await {
                index += 1;
                write_frame(&mut stream, &input).await?;
                if let Some(captured) = captured.filter(|_| is_sampled_input(&input)) {
                    latency.lock().unwrap().sent(SampleId::Input { index }, captured, Instant::now());
                }
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
    let (mut send, mut recv, hello) = match receive_hello(&conn).await {
        Ok(received) => received,
        Err(e) => {
            conn.close(0u32.into(), b"no hello");
            return Err(e.context("kein gültiges Hello"));
        }
    };
    let Control::Hello { version, name, screen, platform } = hello else {
        conn.close(0u32.into(), b"no hello");
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
    let (sharing, latency) = match verdict {
        Ok(Admitted { server_name, sharing, cursor_finder, control, latency, pastes_on_demand, offers, serving }) => {
            debug!("„{name}“ läuft unter {platform:?}");
            // Ältere Clients werten die Version nicht aus; neuere melden daraufhin ihre Monitore,
            // ob sie Zeichen eingeben, die Sperrtasten setzen und dem Bildschirmschoner folgen können
            // und, sobald dort etwas im Vollbild läuft, den Spielemodus.
            let welcome = Control::Welcome { version: SERVER_VERSION, server_name, platform: Platform::current() };
            write_frame(&mut send, &welcome).await?;
            tokio::spawn(send_control(send, sharing.clone(), cursor_finder, control, pastes_on_demand, name.clone()));
            // Abrufe kopierter Dateien (weitere bidirektionale Streams), nur solange er sie bekommen darf
            let rights = sharing.clone();
            let allowed = move || rights.borrow().files.to_client();
            tokio::spawn(ondemand::accept_fetches(conn.clone(), name.clone(), offers, serving, allowed));
            (sharing, latency)
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
            Ok(Some(Control::SyncsLocks)) => {
                let _ = tx.send(NetEvent::SyncsLocks { name: name.clone(), id });
            }
            Ok(Some(Control::FollowsScreenSaver)) => {
                let _ = tx.send(NetEvent::FollowsScreenSaver { name: name.clone(), id });
            }
            Ok(Some(Control::PastesOnDemand)) => {
                let _ = tx.send(NetEvent::PastesOnDemand { name: name.clone(), id });
            }
            Ok(Some(Control::Latency(Latency::Report(report)))) => {
                latency.lock().unwrap().report(&report, Instant::now());
            }
            Ok(Some(other)) => debug!(?other, "unerwartete Control-Nachricht"),
            Ok(None) | Err(_) => break,
        }
    }
    conn.close(0u32.into(), b"");
    let _ = tx.send(NetEvent::Closed { name, id });
    Ok(())
}

/// Höchstens so groß darf das `Hello` sein; es braucht ein paar Dutzend Bytes.
const HELLO_MAX: usize = 16 * 1024;
/// So lange darf es nach dem Handshake auf sich warten lassen
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);

/// Das `Hello` auf dem Control-Stream, den der Client öffnet. Bis hierher kommt jeder mit irgendeinem
/// Zertifikat; vertraut wird erst danach. Deshalb gilt eine eigene kleine Grenze statt der für Frames
/// (64 MB, der Puffer entsteht schon mit dem Längenfeld), und es muss bald kommen.
async fn receive_hello(conn: &Connection) -> Result<(SendStream, RecvStream, Control)> {
    let receive = async {
        let (send, mut recv) = conn.accept_bi().await?;
        let hello = read_frame_up_to(&mut recv, HELLO_MAX).await?.context("Control-Stream ohne Nachricht beendet")?;
        anyhow::Ok((send, recv, hello))
    };
    tokio::time::timeout(HELLO_TIMEOUT, receive)
        .await
        .map_err(|_| anyhow!("nichts binnen {} s nach dem Handshake", HELLO_TIMEOUT.as_secs()))?
}

/// Schickt dem Client nach dem `Welcome` seine Rechte, den Cursor-Finder (diesen nur, wenn er
/// eingeschaltet ist) und ob der Server kopierte Dateien auf Abruf einfügt, danach jede Änderung und was
/// sonst ansteht (Bildschirmschoner, Ergebnis der Messung der Eingabekette). Clients bis Qross 0.6 lesen
/// den Control-Stream nach dem `Welcome` nicht mehr, bei ihnen bleibt das ungelesen liegen; Clients ab
/// 0.7 überspringen, was sie nicht kennen.
async fn send_control(
    mut control: SendStream,
    mut sharing: watch::Receiver<Sharing>,
    mut finder: watch::Receiver<CursorFinder>,
    mut extra: mpsc::UnboundedReceiver<Control>,
    pastes_on_demand: bool,
    name: String,
) {
    let mut messages = vec![Control::Sharing(*sharing.borrow_and_update())];
    let style = *finder.borrow_and_update();
    if style.is_on() {
        messages.push(Control::CursorFinder(style));
    }
    if pastes_on_demand {
        messages.push(Control::PastesOnDemand);
    }
    loop {
        for message in messages.drain(..) {
            if let Err(e) = write_frame(&mut control, &message).await {
                return debug!("Control-Nachricht an „{name}“ nicht gesendet: {e}");
            }
        }
        // Endet, sobald der Client getrennt ist (bzw. der Server beendet)
        tokio::select! {
            changed = sharing.changed() => match changed {
                Ok(()) => messages.push(Control::Sharing(*sharing.borrow_and_update())),
                Err(_) => return,
            },
            changed = finder.changed() => match changed {
                Ok(()) => messages.push(Control::CursorFinder(*finder.borrow_and_update())),
                Err(_) => return,
            },
            next = extra.recv() => match next {
                Some(message) => messages.push(message),
                None => return,
            },
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
                        if matches!(message, ClipboardMsg::Files { .. } | ClipboardMsg::FilesOnDemand { .. }) && !allowed.files {
                            info!("kopierte Dateien von „{name}“ abgewiesen: nicht erlaubt");
                        }
                        match rights::message(message, allowed) {
                            Some(ClipboardMsg::Data(data)) => {
                                let _ = tx.send(NetEvent::Clipboard { name, id, data });
                            }
                            Some(ClipboardMsg::Files { transfer, entries, data }) => {
                                let _ = tx.send(NetEvent::ClipboardFiles { name, id, transfer, entries, data });
                            }
                            Some(ClipboardMsg::FilesOnDemand { transfer, entries, data }) => {
                                let _ = tx.send(NetEvent::ClipboardFilesOnDemand { name, id, transfer, entries, data });
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
