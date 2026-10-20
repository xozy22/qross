//! Client-Laufzeit: verbindet sich (mit automatischer Wiederverbindung) und speist ein.

use std::fmt;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use qross_core::client::{ClientState, Transition};
use qross_core::finder::{self as cursor_finder, Cue};
use qross_core::sharing::describe as describe_sharing;
use qross_core::Inject;
use qross_platform::{Clipboard, FinderOverlay, Injector};
use qross_proto::{
    decode_datagram, read_frame, write_frame, Clipboard as ClipboardMsg, ClipboardData, Control, CursorFinder, Datagram,
    DragMessage, FileEntry, FrameError, GameState, Input, Latency, LatencySummary, MouseDelta, MouseMove, Platform, Rect,
    SampleId, ScreenInfo, Sharing, StreamKind, DEFAULT_PORT, GAME_MODE_SINCE, LATENCY_SINCE, LOCKS_SINCE, MONITORS_SINCE,
    PASTE_ON_DEMAND_SINCE, PROTOCOL_VERSION, SCREEN_SAVER_SINCE, SHARING_SINCE, TEXT_SINCE, USAGE_SINCE,
};
use quinn::{Connection, ConnectionError, Endpoint, RecvStream};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::clipboard::ClipboardHub;
use crate::clipfiles::{self, FilesNotice, Incoming, Notify, Ready};
use crate::drag_preview::DragView;
use crate::files::{self, FileOutcome, Receiver, Staging};
use crate::game::{self, Detector};
use crate::latency::{self, Collector, Measured, Stamp};
use crate::ondemand::{self, Link, Offered, Offers, Pastes};
use crate::progress::{Direction, Progress, Transfers};
use crate::quality::{self, Quality};
use crate::rights::{self, Allowed};
use crate::screensaver;
use crate::streams::{read_kind, send_drag_message};
use crate::switch_trace::{ClientTrace, EarlyMoves};
use crate::tls::{client_config, peer_fingerprint, SERVER_NAME};
use crate::usage;
use crate::wake::{self, Backoff, SystemEvent};
use crate::{Fingerprint, Identity, TrustOutcome, TrustStore, KNOWN_SERVERS_FILE};

/// Nach einer Ablehnung (etwa: noch nicht freigegeben) so lange warten; schneller hilft da nicht.
const AFTER_REJECT: Duration = Duration::from_secs(3);
/// Höchstens so lange auf die Namensauflösung warten; nach dem Aufwachen hängt sie gern noch.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);

pub struct ClientConfig {
    /// Name dieses Rechners im Layout des Servers
    pub name: String,
    /// `host`, `host:port` oder IP-Adresse
    pub server: String,
    /// Fest vorgegebener Server-Fingerprint; sonst Trust on First Use
    pub server_fingerprint: Option<Fingerprint>,
    /// Kopierte Dateien bis zu dieser Gesamtgröße in Bytes mitgeben (0 = nie);
    /// üblich: `clipfiles::DEFAULT_LIMIT`
    pub clipboard_file_limit: u64,
    /// Inhalte der Zwischenablage, die ein Passwortmanager als vertraulich markiert, trotzdem an
    /// den Server geben (üblich: nein; dann meldet `ClientNotice::ClipboardConcealed` das Zurückhalten)
    pub share_concealed: bool,
}

pub struct ClientPlatform {
    pub injector: Box<dyn Injector>,
    pub clipboard: Box<dyn Clipboard>,
    pub screen: Box<dyn Fn() -> Rect + Send + Sync>,
    /// Die einzelnen Monitore in den Koordinaten von `screen` (leer: unbekannt)
    pub monitors: Box<dyn Fn() -> Vec<Rect> + Send + Sync>,
    /// Spielemodus: was im Vordergrund läuft (üblich: `qross_platform::game_state`; ohne
    /// Spielemodus `GameState::default`, dann meldet der Client nie etwas)
    pub game: Detector,
    /// Cursor-Finder dieses Rechners, wenn der Cursor ankommt (üblich: `qross_platform::finder_overlay`).
    /// Was er zeigt, stellt der Server ein; `None`: hier nie etwas zeigen.
    pub finder: Option<Arc<dyn FinderOverlay>>,
}

/// Abfrage der eigenen Bildschirmfläche und des Vordergrunds (aus `ClientPlatform`)
struct ScreenQuery {
    bounds: Box<dyn Fn() -> Rect + Send + Sync>,
    monitors: Box<dyn Fn() -> Vec<Rect> + Send + Sync>,
    game: watch::Receiver<GameState>,
    /// Das Einspeisen kann Zeichen eingeben (Zeichenmodus)
    types_text: bool,
    /// Das Einspeisen kann Feststelltaste, Num und Rollen setzen
    syncs_locks: bool,
    /// Cursor-Finder dieses Rechners
    finder: Option<Arc<dyn FinderOverlay>>,
    /// Das Einspeisen kann den Bildschirmschoner starten und beenden
    follows_saver: bool,
}

/// Fehler, bei denen eine Wiederverbindung sinnlos ist.
#[derive(Debug)]
struct Fatal(String);

impl fmt::Display for Fatal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Fatal {}

/// Warum eine Sitzung bzw. ein Verbindungsversuch endete, wenn das die nächste Wartezeit bestimmt
#[derive(Debug)]
enum Break {
    /// Dieser Rechner geht in den Ruhezustand: getrennt, neu verbinden erst nach dem Aufwachen
    Sleep,
    /// Aufgewacht, anderes Netz oder die Verbindung antwortet danach nicht mehr: sofort neu
    /// verbinden. Mit Grund, wenn eine Verbindung bestand.
    Now(Option<String>),
    /// Der Server geht in den Ruhezustand
    ServerSleeps,
    /// Der Server hat abgelehnt (etwa: noch nicht freigegeben)
    Rejected(String),
}

impl fmt::Display for Break {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Break::Sleep => f.write_str("going to sleep; reconnecting after waking up"),
            Break::Now(Some(reason)) => f.write_str(reason),
            Break::Now(None) => f.write_str("connection attempt interrupted"),
            Break::ServerSleeps => f.write_str("server is going to sleep"),
            Break::Rejected(reason) => write!(f, "server rejected the connection: {reason}"),
        }
    }
}

impl std::error::Error for Break {}

impl Break {
    /// Ein Systemereignis unterbricht den Verbindungsaufbau.
    fn interrupt(event: SystemEvent) -> anyhow::Error {
        match event {
            SystemEvent::Sleeping => Break::Sleep,
            SystemEvent::Woke | SystemEvent::NetworkChanged => Break::Now(None),
        }
        .into()
    }
}

/// Systemereignisse (vom Betriebssystem oder aus einem Test)
type Events = mpsc::UnboundedReceiver<SystemEvent>;

/// Was von einer Sitzung zur nächsten bleibt
struct Reconnect {
    tls: quinn::ClientConfig,
    events: Events,
    /// Zuletzt erreichte Adresse des Servers, falls die Namensauflösung einmal hängt
    last_addr: Option<SocketAddr>,
}

/// Lebendprüfung nach dem Aufwachen oder einem Netzwechsel (siehe [`wake::answers`])
type Probe = Pin<Box<dyn Future<Output = bool> + Send>>;

/// Was vom Server kommt; Bewegungen mit dem Zeitpunkt ihres Empfangs, Eingaben mit ihrer Stichprobe
/// der Eingabekette
enum FromServer {
    Move(MouseMove, Instant),
    /// Relative Bewegung (Spielemodus)
    Delta(MouseDelta, Instant),
    Input(Input, Option<Stamp>),
    Clipboard(ClipboardData),
    File(FileOutcome),
    ClipboardFiles { transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    ClipboardFile(FileOutcome),
    /// Kopierte Dateien des Servers auf Abruf: nur die Liste (seit Qross 0.13)
    ClipboardFilesOnDemand { transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    /// Der Server fügt kopierte Dateien auf Abruf ein (seit Qross 0.13)
    PastesOnDemand,
    /// Server fragt: Werden hier gerade Dateien gezogen? (Drag & Drop zum Server)
    DragQuery { probe: u32 },
    /// Was der Server mit diesem Rechner teilt (seit Qross 0.7)
    Sharing(Sharing),
    /// Was der Cursor-Finder hier zeigt (seit Qross 0.8)
    CursorFinder(CursorFinder),
    /// Bildschirmschoner des Servers begonnen bzw. beendet (seit Qross 0.8)
    ScreenSaver { active: bool, lock: bool },
    /// Ergebnis der Messung der Eingabekette (ab `LATENCY_SINCE`)
    Latency(LatencySummary),
    /// Der Server zeigt CPU und Arbeitsspeicher dieses Rechners an bzw. nicht mehr (seit Qross 0.14)
    WatchUsage(bool),
}

/// Zwischenablage dieses Clients: Zugriff, ankommende kopierte Dateien, Meldungen dazu.
struct ClipboardSide {
    hub: ClipboardHub,
    incoming: Incoming,
    notify: Notify,
    /// Fortschritt und Abbrechen aller Dateiübertragungen (auch Drag & Drop)
    transfers: Transfers,
    /// Kopierte Dateien auf Abruf: was der Server hier abrufen darf, seine Abrufe und die hiesigen bei ihm.
    /// Bleibt über Neuverbindungen bestehen; `link` nennt die jeweils aktuelle Verbindung.
    offers: Offers,
    serving: Pastes,
    pasting: Pastes,
    link: watch::Sender<Option<Connection>>,
}

/// Aufträge an den Einspeise-Thread. Die Ablageordner-Abfrage läuft dort, damit sie nach den
/// vorher eingespeisten Bewegungen ausgewertet wird.
enum ToInjector {
    Inject(Inject),
    DropFolder { x: i32, y: i32, reply: oneshot::Sender<Option<PathBuf>> },
    /// Gezogene Dateien übernehmen (der lokale Drag wird dabei abgebrochen)
    TakeDragged { x: i32, y: i32, reply: oneshot::Sender<Vec<PathBuf>> },
    /// Dem Bildschirmschoner des Servers folgen
    ScreenSaver { active: bool, lock: bool },
    /// Stichprobe der Eingabekette: Alles davor ist eingespeist, jetzt die Zeit nehmen.
    Measured { stamp: Stamp, done: mpsc::UnboundedSender<Measured> },
}

/// Ab so lange gilt ein Auftrag im Einspeise-Thread als Hänger: Der Cursor stand dann sichtbar still.
const SLOW_INJECT: Duration = Duration::from_millis(20);

/// Was der Einspeise-Thread gerade erledigt hat, fürs Log, wenn es zu lange dauerte
enum InjectStep {
    Inject(Inject),
    Named(&'static str),
}

impl fmt::Display for InjectStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inject(event) => write!(f, "{event:?}"),
            Self::Named(name) => f.write_str(name),
        }
    }
}

fn inject_all(inject: &Sender<ToInjector>, events: impl IntoIterator<Item = Inject>) {
    for event in events {
        let _ = inject.send(ToInjector::Inject(event));
    }
}

/// Einspeise-Thread: erledigt die Aufträge der Reihe nach. Warten schon mehrere Bewegungen, etwa
/// weil nach einer Lücke im Netz viele auf einmal ankamen, gehen sie zusammen hinaus (siehe
/// [`coalesce`]): Der Cursor springt an die neueste Stelle, statt die alte Bahn nachzufahren.
fn inject_loop(mut injector: Box<dyn Injector>, jobs: std::sync::mpsc::Receiver<ToInjector>) {
    let mut next = None;
    while let Some(job) = next.take().or_else(|| jobs.recv().ok()) {
        let started = Instant::now();
        let step = match job {
            ToInjector::Inject(event @ (Inject::MoveTo { .. } | Inject::MoveBy { .. })) => {
                let (event, marks, after) = coalesce(event, &jobs);
                next = after;
                tracing::trace!(?event, "inject");
                injector.inject(&event);
                let done = Instant::now();
                for (stamp, to) in marks {
                    let _ = to.send(Measured { id: stamp.id, received: stamp.received, done });
                }
                InjectStep::Inject(event)
            }
            ToInjector::Inject(event) => {
                debug!(?event, "inject");
                injector.inject(&event);
                InjectStep::Inject(event)
            }
            ToInjector::DropFolder { x, y, reply } => {
                let _ = reply.send(injector.drop_folder(x, y));
                InjectStep::Named("find drop folder")
            }
            ToInjector::TakeDragged { x, y, reply } => {
                let _ = reply.send(injector.take_dragged_files(x, y));
                InjectStep::Named("take dragged files")
            }
            ToInjector::ScreenSaver { active, lock } => {
                screensaver::report(active, &injector.screen_saver(active, lock));
                InjectStep::Named("sync screen saver")
            }
            ToInjector::Measured { stamp, done } => {
                let _ = done.send(Measured { id: stamp.id, received: stamp.received, done: Instant::now() });
                continue;
            }
        };
        let took = started.elapsed();
        if took >= SLOW_INJECT {
            warn!("injecting stalled for {} ms ({step}); all following input waited that long", took.as_millis());
        }
    }
}

/// Höchstens so viele wartende Aufträge fasst das Einspeisen auf einmal zusammen
const COALESCE_MAX: usize = 1024;

/// Stichprobe der Eingabekette mit dem Kanal, an den die Rückmeldung geht
type Mark = (Stamp, mpsc::UnboundedSender<Measured>);

/// Nimmt die Bewegungen dazu, die hinter `first` schon warten: Von Positionen zählt nur die letzte,
/// relative Bewegungen (Spielemodus) werden zusammengezählt. Stichproben dazwischen kommen mit; sie
/// werden nach dem Einspeisen beantwortet. Liefert auch den ersten Auftrag, der nicht dazugehört.
fn coalesce(first: Inject, jobs: &std::sync::mpsc::Receiver<ToInjector>) -> (Inject, Vec<Mark>, Option<ToInjector>) {
    let (mut event, mut marks) = (first, Vec::new());
    for _ in 0..COALESCE_MAX {
        let Ok(job) = jobs.try_recv() else { break };
        match (event, job) {
            (Inject::MoveTo { .. }, ToInjector::Inject(next @ Inject::MoveTo { .. })) => event = next,
            (Inject::MoveBy { dx, dy }, ToInjector::Inject(Inject::MoveBy { dx: x, dy: y })) => {
                event = Inject::MoveBy { dx: dx.saturating_add(x), dy: dy.saturating_add(y) };
            }
            (_, ToInjector::Measured { stamp, done }) => marks.push((stamp, done)),
            (_, other) => return (event, marks, Some(other)),
        }
    }
    (event, marks, None)
}

/// Zustandsmeldungen für eine Oberfläche (siehe [`run_with_notices`]).
#[derive(Debug, Clone)]
pub enum ClientNotice {
    Connecting { addr: SocketAddr },
    Connected { server_name: String, addr: SocketAddr },
    /// Verbindung beendet oder nicht zustande gekommen; es folgt ein neuer Versuch.
    Disconnected { reason: String },
    CursorEntered,
    CursorLeft,
    /// Kopierte Dateien in der Zwischenablage (senden, empfangen, einfügbar, zu groß)
    ClipboardFiles(FilesNotice),
    /// Verbindungsqualität zum Server, etwa jede Sekunde (siehe [`quality`]).
    Quality(Quality),
    /// Stand einer Dateiübertragung etwa viermal pro Sekunde und zum Schluss ihr Ergebnis
    /// (siehe [`crate::progress`]). Abbrechen über die `Transfers` aus [`run_with_transfers`].
    Transfer(Progress),
    /// Was der Server mit diesem Rechner teilt (Rechte je Rechner): nach dem Verbinden und bei
    /// jeder Änderung. Server bis Qross 0.6 melden das nie; dort ist alles erlaubt.
    Sharing(Sharing),
    /// Ein Passwortmanager hat den Inhalt der Zwischenablage als vertraulich markiert; er bleibt
    /// hier (einmal je Inhalt, siehe [`ClientConfig::share_concealed`]).
    ClipboardConcealed,
    /// Was der Server für den Cursor-Finder eingestellt hat: nach dem Verbinden, sofern er etwas
    /// zeigt, und bei jeder Änderung. Server bis Qross 0.7 melden das nie; dort bleibt er aus.
    CursorFinder(CursorFinder),
    /// Verzögerung der Eingabekette vom Server bis hier, etwa jede Sekunde, solange sich etwas ändert
    /// (siehe [`latency`]). Server vor `LATENCY_SINCE` melden das nie.
    Latency(LatencySummary),
}

type Notices<'a> = Option<&'a mpsc::UnboundedSender<ClientNotice>>;

fn notify(notices: Notices<'_>, notice: ClientNotice) {
    if let Some(tx) = notices {
        let _ = tx.send(notice);
    }
}

/// Läuft, bis ein nicht behebbarer Fehler auftritt (z. B. falscher Server-Fingerprint). Ruhezustand,
/// Aufwachen und Netzwechsel meldet das Betriebssystem (siehe [`wake`]).
pub async fn run(config: ClientConfig, identity: Identity, trust: TrustStore, platform: ClientPlatform) -> Result<()> {
    run_inner(config, identity, trust, platform, None, Transfers::new(), None).await
}

/// Wie [`run`], meldet den Verbindungszustand zusätzlich über `notices`.
pub async fn run_with_notices(
    config: ClientConfig,
    identity: Identity,
    trust: TrustStore,
    platform: ClientPlatform,
    notices: mpsc::UnboundedSender<ClientNotice>,
) -> Result<()> {
    run_inner(config, identity, trust, platform, Some(&notices), Transfers::new(), None).await
}

/// Wie [`run_with_notices`]; laufende Dateiübertragungen lassen sich über `transfers` abbrechen.
pub async fn run_with_transfers(
    config: ClientConfig,
    identity: Identity,
    trust: TrustStore,
    platform: ClientPlatform,
    notices: mpsc::UnboundedSender<ClientNotice>,
    transfers: Transfers,
) -> Result<()> {
    run_inner(config, identity, trust, platform, Some(&notices), transfers, None).await
}

/// Wie [`run_with_transfers`], aber Ruhezustand, Aufwachen und Netzwechsel kommen aus `events`
/// statt vom Betriebssystem (für Tests).
pub async fn run_with_system_events(
    config: ClientConfig,
    identity: Identity,
    trust: TrustStore,
    platform: ClientPlatform,
    notices: mpsc::UnboundedSender<ClientNotice>,
    transfers: Transfers,
    events: mpsc::UnboundedReceiver<SystemEvent>,
) -> Result<()> {
    run_inner(config, identity, trust, platform, Some(&notices), transfers, Some(events)).await
}

/// Baut nur den TLS-Handshake auf und liefert den Fingerprint des Servers, ohne sich anzumelden
/// (z. B. um ihn vor dem ersten Verbinden bestätigen zu lassen).
pub async fn probe_fingerprint(server: &str, identity: &Identity) -> Result<(SocketAddr, Fingerprint)> {
    let addr = resolve(server).await?;
    let mut endpoint = Endpoint::client(wake::unspecified(addr))?;
    endpoint.set_default_client_config(client_config(identity)?);
    let conn = endpoint
        .connect(addr, SERVER_NAME)?
        .await
        .with_context(|| format!("failed to connect to {addr}"))?;
    let fingerprint = peer_fingerprint(&conn)?;
    conn.close(0u32.into(), b"probe");
    let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    Ok((addr, fingerprint))
}

async fn run_inner(
    config: ClientConfig,
    identity: Identity,
    mut trust: TrustStore,
    platform: ClientPlatform,
    notices: Notices<'_>,
    transfers: Transfers,
    events: Option<Events>,
) -> Result<()> {
    let ClientPlatform { injector, clipboard, screen, monitors, game, finder } = platform;
    let (types_text, syncs_locks, follows_saver) =
        (injector.types_text(), injector.syncs_locks(), injector.follows_screen_saver());
    let screen =
        ScreenQuery { bounds: screen, monitors, game: game::watch(game), types_text, syncs_locks, follows_saver, finder };
    let _progress = {
        let notices = notices.cloned();
        transfers.spawn_reporter(move |progress| {
            if let Some(tx) = &notices {
                let _ = tx.send(ClientNotice::Transfer(progress));
            }
        })
    };
    let mut clipboard = ClipboardSide {
        hub: ClipboardHub::spawn(clipboard, config.share_concealed, {
            let notices = notices.cloned();
            move || {
                if let Some(tx) = &notices {
                    let _ = tx.send(ClientNotice::ClipboardConcealed);
                }
            }
        }),
        incoming: Incoming::new(Staging::persistent(&clipfiles::default_store())?, transfers.clone()),
        offers: Offers::default(),
        serving: Pastes::new(Direction::Sending, transfers.clone()),
        pasting: Pastes::new(Direction::Receiving, transfers.clone()),
        link: watch::channel(None).0,
        transfers,
        notify: {
            let notices = notices.cloned();
            Arc::new(move |notice| {
                if let Some(tx) = &notices {
                    let _ = tx.send(ClientNotice::ClipboardFiles(notice));
                }
            })
        },
    };

    // Eigener Thread, damit SendInput nie die Netzwerk-Tasks blockiert.
    let (inject_tx, inject_rx) = std::sync::mpsc::channel::<ToInjector>();
    std::thread::Builder::new().name("qross-inject".into()).spawn(move || inject_loop(injector, inject_rx))?;

    let mut backoff = Backoff::new();
    let events = events.unwrap_or_else(wake::system_events);
    let mut reconnect = Reconnect { tls: client_config(&identity)?, events, last_addr: None };
    loop {
        let started = Instant::now();
        let result = session(&config, &mut trust, &inject_tx, &mut clipboard, &screen, notices, &mut reconnect).await;
        let mut wait = backoff.after(started.elapsed(), Instant::now());
        let mut asleep = false;
        match result {
            Ok(()) => {
                info!("connection to the server ended");
                notify(notices, ClientNotice::Disconnected { reason: "connection to the server ended".into() });
            }
            Err(e) if e.downcast_ref::<Fatal>().is_some() => return Err(e),
            Err(e) => {
                // Ruhezustand ist kein Fehler
                let mut expected = false;
                match e.downcast_ref::<Break>() {
                    Some(Break::Sleep) => (asleep, wait, expected) = (true, wake::SLEEP_PAUSE, true),
                    Some(Break::Now(None)) => {
                        // Nur ein Verbindungsversuch, der neu beginnt: nichts zu melden
                        debug!("{e}");
                        backoff.reset();
                        continue;
                    }
                    Some(Break::Now(Some(_))) => {
                        backoff.reset();
                        wait = Duration::ZERO;
                    }
                    // Er schläft gleich: nicht vorher noch einmal anmelden
                    Some(Break::ServerSleeps) => (wait, expected) = (Backoff::FAST_MAX, true),
                    Some(Break::Rejected(_)) => wait = AFTER_REJECT,
                    None => {}
                }
                if expected {
                    info!("{e:#}");
                } else {
                    warn!("{e:#}");
                }
                notify(notices, ClientNotice::Disconnected { reason: format!("{e:#}") });
            }
        }
        pause(wait, asleep, &mut reconnect.events, &mut backoff).await;
    }
}

/// Wartet bis zum nächsten Verbindungsversuch. Aufwachen und Netzwechsel beenden das Warten sofort,
/// im Ruhezustand (`asleep`) nur das Aufwachen.
async fn pause(wait: Duration, mut asleep: bool, events: &mut Events, backoff: &mut Backoff) {
    let mut until = tokio::time::Instant::now() + wait;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(until) => return,
            Some(event) = events.recv() => match event {
                SystemEvent::Sleeping => {
                    asleep = true;
                    until = tokio::time::Instant::now() + wake::SLEEP_PAUSE;
                }
                SystemEvent::Woke => {
                    info!("woke up; reconnecting");
                    backoff.reset();
                    return;
                }
                SystemEvent::NetworkChanged if !asleep => {
                    debug!("network changed; reconnecting");
                    backoff.reset();
                    return;
                }
                SystemEvent::NetworkChanged => {}
            },
        }
    }
}

async fn session(
    config: &ClientConfig,
    trust: &mut TrustStore,
    inject: &Sender<ToInjector>,
    clip: &mut ClipboardSide,
    screen: &ScreenQuery,
    notices: Notices<'_>,
    reconnect: &mut Reconnect,
) -> Result<()> {
    let addr = tokio::select! {
        resolved = tokio::time::timeout(RESOLVE_TIMEOUT, resolve(&config.server)) => {
            match (resolved, reconnect.last_addr) {
                (Ok(Ok(addr)), _) => addr,
                (Ok(Err(e)), Some(last)) => {
                    debug!("{e:#}; trying the last reached address {last}");
                    last
                }
                (Err(_), Some(last)) => {
                    debug!("'{}' not resolved in time; trying {last}", config.server);
                    last
                }
                (Ok(Err(e)), None) => return Err(e),
                (Err(_), None) => bail!("failed to resolve '{}' in time", config.server),
            }
        }
        Some(event) = reconnect.events.recv() => return Err(Break::interrupt(event)),
    };
    let mut endpoint = Endpoint::client(wake::unspecified(addr))?;
    endpoint.set_default_client_config(reconnect.tls.clone());

    debug!("connecting to {addr}");
    notify(notices, ClientNotice::Connecting { addr });
    let connecting = endpoint.connect(addr, SERVER_NAME)?;
    // Aufwachen oder ein Netzwechsel unterbricht den Versuch: Der nächste beginnt sofort mit frischem
    // Socket auf dem neuen Weg.
    let conn = tokio::select! {
        connected = tokio::time::timeout(wake::CONNECT_TIMEOUT, connecting) => match connected {
            Ok(conn) => conn.with_context(|| format!("failed to connect to {addr}"))?,
            Err(_) => bail!("failed to connect to {addr}: no response"),
        },
        Some(event) = reconnect.events.recv() => return Err(Break::interrupt(event)),
    };
    verify_server(config, trust, &peer_fingerprint(&conn)?)?;

    let (mut control, mut control_rx) = conn.open_bi().await?;
    let mut bounds = (screen.bounds)();
    let hello = Control::Hello {
        version: PROTOCOL_VERSION,
        name: config.name.clone(),
        screen: ScreenInfo { bounds },
        platform: Platform::current(),
    };
    write_frame(&mut control, &hello).await?;
    let (server_name, server_platform, server_version) = match read_frame::<_, Control>(&mut control_rx).await? {
        Some(Control::Welcome { version, server_name, platform }) => {
            info!("connected to server '{server_name}' ({addr})");
            notify(notices, ClientNotice::Connected { server_name: server_name.clone(), addr });
            (server_name, platform, version)
        }
        Some(Control::Reject { reason }) => return Err(Break::Rejected(reason).into()),
        other => bail!("unexpected response from the server: {other:?}"),
    };
    reconnect.last_addr = Some(addr);
    // Zeichen statt Tastenpositionen kann nur ein Server schicken, der sie kennt; ältere trennen bei
    // unbekannten Nachrichten. Ohne diese Meldung bleibt es bei Tastenpositionen.
    if server_version >= TEXT_SINCE && screen.types_text {
        write_frame(&mut control, &Control::TypesText).await?;
    }
    // Den Stand der Sperrtasten schickt nur ein Server, der ihn kennt; sonst bleibt es bei den Tasten.
    if server_version >= LOCKS_SINCE && screen.syncs_locks {
        write_frame(&mut control, &Control::SyncsLocks).await?;
    }
    // Ebenso den Bildschirmschoner: Nur wer sich meldet, bekommt `Control::ScreenSaver`.
    if server_version >= SCREEN_SAVER_SINCE && screen.follows_saver {
        write_frame(&mut control, &Control::FollowsScreenSaver).await?;
    }
    // Kopierte Dateien erst beim Einfügen: nur mit Servern, die das kennen, und wenn dieses System es kann
    if server_version >= PASTE_ON_DEMAND_SINCE && clip.hub.pastes_on_demand() {
        info!("copied files from the server are transferred only when pasted");
        write_frame(&mut control, &Control::PastesOnDemand).await?;
    }
    // CPU und Arbeitsspeicher auf Nachfrage: nur an Server, die danach fragen können (ältere trennen bei
    // unbekannten Nachrichten). Gemessen wird nur, solange er sie anzeigt.
    let report_usage = server_version >= USAGE_SINCE && qross_platform::usage::supported();
    if report_usage {
        write_frame(&mut control, &Control::ReportsUsage).await?;
    }
    let (usage_watch, watched) = watch::channel(false);
    let (usage_tx, mut usage_rx) = mpsc::unbounded_channel();
    if report_usage {
        tokio::spawn(usage::report_while_watched(watched, usage_tx));
    }
    // Die einzelnen Monitore nur an Server, die sie kennen: ältere trennen bei unbekannten Nachrichten.
    let report_monitors = server_version >= MONITORS_SINCE;
    let mut monitors = (screen.monitors)();
    if report_monitors && !monitors.is_empty() {
        debug!(?monitors, "monitors reported to the server");
        write_frame(&mut control, &Control::Monitors(monitors.clone())).await?;
    }
    // Spielemodus nur an Server, die ihn kennen; anfangs nur, wenn schon etwas im Vollbild läuft.
    let report_game = server_version >= GAME_MODE_SINCE;
    let mut game_rx = screen.game.clone();
    let mut game = *game_rx.borrow_and_update();
    if report_game && game.is_active() {
        info!("game mode: {} in the foreground, reported to the server", game::describe(game));
        write_frame(&mut control, &Control::Game(game)).await?;
    }
    // Verzögerung der Eingabekette: nur mit Servern, die die Rückmeldung kennen (ältere trennen bei
    // unbekannten Nachrichten). Die erste, leere sagt ihm, dass dieser Client mitmisst.
    let measure = server_version >= LATENCY_SINCE;
    let mut collector = Collector::new(Instant::now());
    if measure {
        write_frame(&mut control, &Control::Latency(Latency::Report(collector.report(Instant::now())))).await?;
    }
    let (measured_tx, mut measured_rx) = mpsc::unbounded_channel();
    let marks = measure.then_some(measured_tx);
    let flush = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(flush);
    // Läuft bis zum Ende dieser Sitzung
    let _quality = notices.cloned().map(|tx| {
        quality::monitor(conn.clone(), move |q| {
            let _ = tx.send(ClientNotice::Quality(q));
        })
    });

    let mut receiver = Receiver::new(Staging::create(&files::drag_staging_base())?, clip.transfers.clone());
    // Was vom Server hierher gezogen wird, steht am Cursor, bis es abgelegt oder verworfen ist.
    let mut drag_view = DragView::new(screen.finder.clone());

    let (tx, mut rx) = mpsc::unbounded_channel();
    // Rechte je Rechner: nur neuere Server schicken nach dem Welcome noch etwas.
    if server_version >= SHARING_SINCE {
        tokio::spawn(receive_control(control_rx, tx.clone()));
    }
    tokio::spawn(receive_datagrams(conn.clone(), tx.clone()));
    let stagings = (receiver.staging(), clip.incoming.staging(), clip.transfers.clone());
    tokio::spawn(receive_streams(conn.clone(), tx, stagings));
    // Kopierte Dateien auf Abruf: Der Server ruft hier ab, und hiesige Abrufe gehen über diese Verbindung.
    tokio::spawn(ondemand::accept_fetches(conn.clone(), server_name.clone(), clip.offers.clone(), clip.serving.clone(), || true));
    clip.link.send_replace(Some(conn.clone()));
    let link: Link = {
        let current = clip.link.subscribe();
        Arc::new(move || current.borrow().clone().ok_or_else(|| "not connected to the server".to_owned()))
    };
    // Übertragungen der vorigen Verbindung kommen nie mehr vollständig an.
    clip.incoming.supersede();

    let mut state = ClientState::new();
    // PC steuert Mac (oder umgekehrt): Strg und Cmd tauschen, damit z. B. Strg+C kopiert
    let swap = (server_platform == Platform::MacOs) != (Platform::current() == Platform::MacOs);
    if swap {
        info!("swapping Ctrl and Cmd (server: {server_platform:?})");
    }
    state.set_swap_ctrl_cmd(swap);
    // Zuletzt abgeglichene Clipboard-Sequenznummer (auch aus Hintergrund-Aufgaben aktualisiert)
    let clip_synced = Arc::new(std::sync::Mutex::new(clip.hub.sequence().await));
    // Für das Log: Bewegungen seit dem letzten Enter und letzte Position
    let mut moves = 0u32;
    let mut last_pos = (0, 0);
    let mut screen_poll = tokio::time::interval(Duration::from_secs(2));
    // Was der Server mit diesem Rechner teilt; ältere Server melden nichts und erlauben alles.
    let mut sharing = Sharing::ALL;
    // Läuft nach dem Aufwachen bzw. einem Netzwechsel
    let mut probe: Option<Probe> = None;
    // Was der Cursor-Finder hier zeigt; ältere Server melden nichts, dann bleibt er aus.
    let mut finder_style = CursorFinder::OFF;
    // Fügt der Server kopierte Dateien auf Abruf ein? Ältere melden nichts und bekommen sie vorab.
    let mut server_on_demand = false;
    // Protokoll des Betretens (siehe `switch_trace`): Bewegungen vor dem `Enter`, das laufende Protokoll
    // und die Rückmeldungen des Einspeise-Threads dazu
    let mut early_moves = EarlyMoves::default();
    let mut entering: Option<ClientTrace> = None;
    let (traced_tx, mut traced_rx) = mpsc::unbounded_channel::<Measured>();

    let outcome: Result<()> = async {
        loop {
            if entering.as_ref().is_some_and(|trace| trace.is_due(Instant::now())) {
                entering.take().expect("checked above").log();
            }
            tokio::select! {
                Some(msg) = rx.recv() => match msg {
                    FromServer::Move(mv, received) => match state.on_move(mv) {
                        Some(event) => {
                            moves += 1;
                            last_pos = (mv.x, mv.y);
                            let _ = inject.send(ToInjector::Inject(event));
                            mark(inject, Stamp::mouse(mv.epoch, mv.seq, received), marks.as_ref());
                            let id = SampleId::Mouse { epoch: mv.epoch, seq: mv.seq };
                            trace_move(&mut entering, inject, &traced_tx, id, received);
                        }
                        None => early_moves.add(mv.epoch, received),
                    },
                    FromServer::Delta(delta, received) => match state.on_delta(delta) {
                        Some(event) => {
                            moves += 1;
                            let _ = inject.send(ToInjector::Inject(event));
                            mark(inject, Stamp::mouse(delta.epoch, delta.seq, received), marks.as_ref());
                            let id = SampleId::Mouse { epoch: delta.epoch, seq: delta.seq };
                            trace_move(&mut entering, inject, &traced_tx, id, received);
                        }
                        None => early_moves.add(delta.epoch, received),
                    },
                    FromServer::Input(input, stamp) => {
                        let entered = match &input {
                            Input::Enter { epoch, .. } => Some(*epoch),
                            _ => None,
                        };
                        let dropped = match &input {
                            Input::Enter { x, y, .. } => {
                                (moves, last_pos) = (0, (*x, *y));
                                None
                            }
                            Input::DragStart { transfer, entries } => {
                                receiver.on_drag_start(&server_name, *transfer, entries.clone());
                                // `Drop` bzw. `Leave` können schon vorher angekommen sein: Dann nichts zeigen.
                                if state.is_active() {
                                    drag_view.show(*transfer, &receiver);
                                }
                                None
                            }
                            Input::Drop { transfer, x, y } => {
                                drag_view.hide();
                                Some((*transfer, *x, *y))
                            }
                            _ => None,
                        };
                        let (events, transition) = state.on_input(input);
                        // Eine Bewegung, die ihr Enter überholt hatte, folgt ihm gleich.
                        if entered.is_some() && events.len() > 1 {
                            moves += 1;
                            if let Some(&Inject::MoveTo { x, y }) = events.last() {
                                last_pos = (x, y);
                            }
                        }
                        let injects = !events.is_empty();
                        inject_all(inject, events);
                        if injects {
                            mark(inject, stamp, marks.as_ref());
                        }
                        if let Some((transfer, x, y)) = dropped {
                            let (reply, folder) = oneshot::channel();
                            let _ = inject.send(ToInjector::DropFolder { x, y, reply });
                            let target = folder.await.ok().flatten().unwrap_or_else(files::fallback_drop_folder);
                            receiver.on_drop(transfer, target);
                        }
                        if let (Some(overlay), Some(transition)) = (&screen.finder, transition) {
                            match cursor_finder::on_client(transition, finder_style) {
                                Some(Cue::Show(style)) => overlay.show(style, &config.name),
                                Some(Cue::Hide) => overlay.hide(),
                                None => {}
                            }
                        }
                        match transition {
                            Some(Transition::Entered) => {
                                info!("cursor entered this computer at {},{}", last_pos.0, last_pos.1);
                                notify(notices, ClientNotice::CursorEntered);
                                if let Some(earlier) = entering.take() {
                                    earlier.log();
                                }
                                if let Some(epoch) = entered {
                                    entering = Some(ClientTrace::start(epoch, Instant::now(), &mut early_moves));
                                }
                            }
                            Some(Transition::Left) => {
                                if let Some(trace) = entering.take() {
                                    trace.log();
                                }
                                drag_view.hide();
                                receiver.cancel_undropped();
                                notify(notices, ClientNotice::CursorLeft);
                                info!(
                                    "cursor left this computer ({moves} moves, last at {},{})",
                                    last_pos.0, last_pos.1
                                );
                                // Beim Verlassen das eigene Clipboard mitgeben, falls es sich geändert hat
                                // und der Server es annimmt.
                                let allowed = Allowed { clipboard: sharing.clipboard.from_client(), files: sharing.files.from_client() };
                                if allowed.any() {
                                    let (hub, synced, conn) = (clip.hub.clone(), clip_synced.clone(), conn.clone());
                                    let (limit, peer, notify) = (config.clipboard_file_limit, server_name.clone(), clip.notify.clone());
                                    let transfers = clip.transfers.clone();
                                    let on_demand = server_on_demand.then(|| clip.offers.clone());
                                    tokio::spawn(async move {
                                        let known = *synced.lock().unwrap();
                                        let (seq, content) = hub.changed_since(Some(known)).await;
                                        *synced.lock().unwrap() = seq;
                                        if let Some(content) = content.and_then(|c| rights::content(c, allowed)) {
                                            clipfiles::send(conn, content, limit, peer, notify, transfers, on_demand).await;
                                        }
                                    });
                                } else {
                                    debug!("clipboard stays here: the server doesn't accept it");
                                }
                            }
                            None => {}
                        }
                    }
                    FromServer::Clipboard(data) => {
                        info!("clipboard from the server: {}", data.summary());
                        clip.incoming.supersede();
                        let (hub, synced) = (clip.hub.clone(), clip_synced.clone());
                        tokio::spawn(async move {
                            match hub.write(data).await {
                                Ok(seq) => *synced.lock().unwrap() = seq,
                                Err(e) => warn!("failed to apply clipboard: {e:#}"),
                            }
                        });
                    }
                    FromServer::File(outcome) => receiver.on_file(outcome),
                    FromServer::ClipboardFiles { transfer, entries, data } => {
                        let (names, bytes) = clipfiles::summary(&entries);
                        (clip.notify)(FilesNotice::Receiving { peer: server_name.clone(), names, bytes });
                        match clip.incoming.announce(server_name.clone(), transfer, entries, data) {
                            Ok(Some(ready)) => deliver_files(ready, clip, &clip_synced),
                            Ok(None) => {}
                            Err(e) => {
                                warn!("copied files from the server rejected: {e:#}");
                                (clip.notify)(FilesNotice::Failed { peer: server_name.clone(), reason: format!("{e:#}") });
                            }
                        }
                    }
                    FromServer::ClipboardFilesOnDemand { transfer, entries, data } => {
                        // Was vorab unterwegs war, ist überholt.
                        clip.incoming.supersede();
                        match Offered::new(server_name.clone(), transfer, entries, link.clone(), clip.pasting.clone()) {
                            Ok(offered) => {
                                let (hub, synced, notify) = (clip.hub.clone(), clip_synced.clone(), clip.notify.clone());
                                tokio::spawn(async move {
                                    if let Some(seq) = ondemand::deliver(offered, data, &hub, &notify).await {
                                        *synced.lock().unwrap() = seq;
                                    }
                                });
                            }
                            Err(e) => {
                                warn!("copied files from the server rejected: {e:#}");
                                (clip.notify)(FilesNotice::Failed { peer: server_name.clone(), reason: format!("{e:#}") });
                            }
                        }
                    }
                    FromServer::PastesOnDemand => {
                        debug!("the server pastes copied files on demand");
                        server_on_demand = true;
                    }
                    FromServer::DragQuery { probe } => {
                        let (reply, dragged) = oneshot::channel();
                        if sharing.drag.from_client() {
                            let _ = inject.send(ToInjector::TakeDragged { x: last_pos.0, y: last_pos.1, reply });
                        } else {
                            // Nicht erlaubt: Der Drag bleibt hier, die Antwort lautet „keine Dateien“.
                            let _ = reply.send(Vec::new());
                        }
                        let (peer, transfers) = (server_name.clone(), clip.transfers.clone());
                        tokio::spawn(offer_dragged_files(conn.clone(), probe, dragged, peer, transfers));
                    }
                    FromServer::ClipboardFile(outcome) => match clip.incoming.on_file(outcome) {
                        Ok(Some(ready)) => deliver_files(ready, clip, &clip_synced),
                        Ok(None) => {}
                        Err(failed) => (clip.notify)(FilesNotice::Failed { peer: failed.peer, reason: failed.reason }),
                    },
                    FromServer::Sharing(now) => {
                        if now != sharing {
                            info!("permissions of this computer on the server: {}", describe_sharing(now));
                        }
                        sharing = now;
                        notify(notices, ClientNotice::Sharing(now));
                    }
                    FromServer::CursorFinder(now) => {
                        if now != finder_style {
                            info!("cursor finder from the server: {}", cursor_finder::describe(now));
                        }
                        finder_style = now;
                        notify(notices, ClientNotice::CursorFinder(now));
                    }
                    // Im Einspeise-Thread: Start und Ende dürfen dort kurz warten.
                    FromServer::ScreenSaver { active, lock } => {
                        let _ = inject.send(ToInjector::ScreenSaver { active, lock });
                    }
                    FromServer::Latency(summary) => notify(notices, ClientNotice::Latency(summary)),
                    FromServer::WatchUsage(on) => {
                        if on != *usage_watch.borrow() {
                            if on {
                                debug!("The server shows this computer's CPU and memory usage – reporting every 2 s");
                            } else {
                                debug!("The server no longer shows this computer's CPU and memory usage");
                            }
                        }
                        usage_watch.send_replace(on);
                    }
                },
                Some(report) = usage_rx.recv(), if report_usage => {
                    write_frame(&mut control, &Control::Usage(report)).await?;
                }
                Some(measured) = measured_rx.recv(), if measure => {
                    if collector.add(measured) {
                        flush.as_mut().reset(tokio::time::Instant::now() + latency::REPORT_AFTER);
                    }
                }
                _ = &mut flush, if !collector.is_empty() => {
                    let report = collector.report(Instant::now());
                    write_frame(&mut control, &Control::Latency(Latency::Report(report))).await?;
                }
                _ = screen_poll.tick() => {
                    let now = (screen.bounds)();
                    if now != bounds {
                        bounds = now;
                        write_frame(&mut control, &Control::ScreenChanged(ScreenInfo { bounds })).await?;
                    }
                    if report_monitors {
                        let now = (screen.monitors)();
                        if now != monitors {
                            monitors = now;
                            debug!(?monitors, "monitors changed");
                            write_frame(&mut control, &Control::Monitors(monitors.clone())).await?;
                        }
                    }
                }
                Ok(()) = game_rx.changed(), if report_game => {
                    let now = *game_rx.borrow_and_update();
                    if now != game {
                        game = now;
                        debug!("game mode: {} in the foreground, reported to the server", game::describe(game));
                        write_frame(&mut control, &Control::Game(game)).await?;
                    }
                }
                Some(event) = reconnect.events.recv() => match event {
                    SystemEvent::Sleeping => {
                        // Der Server gibt den Cursor gleich zurück, statt 5 s ins Leere zu schicken.
                        conn.close(wake::CLOSE_SLEEPING, b"sleeping");
                        return Err(Break::Sleep.into());
                    }
                    SystemEvent::Woke | SystemEvent::NetworkChanged => {
                        // Auf neuem Socket weiter (siehe `wake::rebind`); lebt die Verbindung noch,
                        // kommt die Antwort innerhalb einer Laufzeit.
                        if let Err(e) = wake::rebind(&endpoint, addr) {
                            debug!("failed to bind a new socket: {e}");
                        }
                        debug!(?event, "checking the connection to the server");
                        probe = Some(Box::pin(wake::answers(conn.clone(), wake::CLIENT_PROBE)));
                    }
                },
                Some(injected) = traced_rx.recv() => {
                    if let Some(trace) = &mut entering {
                        trace.on_injected(injected.received, injected.done);
                    }
                }
                alive = async { probe.as_mut().expect("only while a check is running").await }, if probe.is_some() => {
                    probe = None;
                    if !alive {
                        conn.close(0u32.into(), b"no answer");
                        return Err(Break::Now(Some("server stopped responding; reconnecting".into())).into());
                    }
                    debug!("connection to the server is alive");
                }
                reason = conn.closed() => return Err(closed(reason)),
            }
            // Abgebrochen (hier, vom Server oder weil Rechte entzogen wurden) oder fehlgeschlagen
            drag_view.check(&receiver);
        }
    }
    .await;

    // Nichts gedrückt lassen, wenn die Verbindung mitten in einer Eingabe abreißt.
    inject_all(inject, state.release_all());
    if let Some(overlay) = &screen.finder {
        overlay.hide();
    }
    drag_view.hide();
    receiver.cancel_undropped();
    clip.link.send_replace(None);
    // Was noch ankam, wird nie vollständig.
    clip.transfers.abort_receiving(None, "disconnected");
    outcome
}

/// Warum die Verbindung endete, mit den Fehlercodes aus [`wake`]
fn closed(reason: ConnectionError) -> anyhow::Error {
    match &reason {
        ConnectionError::ApplicationClosed(close) if close.error_code == wake::CLOSE_SLEEPING => Break::ServerSleeps.into(),
        ConnectionError::ApplicationClosed(close) if close.error_code == wake::CLOSE_REPLACED => Fatal(
            "another Qross with the same name and key connected to the server; this client stops"
                .into(),
        )
        .into(),
        // Stateless Reset: Der Server ist erreichbar, kennt die Verbindung aber nicht mehr (er hat sie
        // im Ruhezustand dieses Rechners aufgegeben oder wurde neu gestartet). Gleich neu anmelden.
        ConnectionError::Reset => Break::Now(Some("the server no longer knows the connection; reconnecting".into())).into(),
        _ => anyhow!("disconnected: {reason}"),
    }
}

/// Stichprobe der Eingabekette: Hinter den Einspeise-Aufträgen der Eingabe meldet der Einspeise-Thread,
/// wann sie erledigt waren (nur, wenn der Server mitmisst).
fn mark(inject: &Sender<ToInjector>, stamp: Option<Stamp>, marks: Option<&mpsc::UnboundedSender<Measured>>) {
    if let (Some(stamp), Some(done)) = (stamp, marks) {
        let _ = inject.send(ToInjector::Measured { stamp, done: done.clone() });
    }
}

/// Bewegung fürs Protokoll des Betretens; im Beobachtungsfenster meldet der Einspeise-Thread auch, wann
/// sie eingespeist ist (jede, nicht nur die Stichproben der Eingabekette).
fn trace_move(
    trace: &mut Option<ClientTrace>,
    inject: &Sender<ToInjector>,
    done: &mpsc::UnboundedSender<Measured>,
    id: SampleId,
    received: Instant,
) {
    let Some(trace) = trace.as_mut().filter(|trace| trace.watches(received)) else { return };
    trace.on_move(received);
    let _ = inject.send(ToInjector::Measured { stamp: Stamp { id, received }, done: done.clone() });
}

/// Fertig übertragene Dateien vom Server in die Zwischenablage legen.
fn deliver_files(ready: Ready, clip: &ClipboardSide, synced: &Arc<std::sync::Mutex<u64>>) {
    let (hub, synced, notify) = (clip.hub.clone(), synced.clone(), clip.notify.clone());
    tokio::spawn(async move {
        let Ready { peer, paths, data, names, bytes } = ready;
        match hub.write_files(paths, data).await {
            Ok(seq) => {
                info!("copied files are in the clipboard");
                *synced.lock().unwrap() = seq;
                notify(FilesNotice::Ready { peer, names, bytes });
            }
            Err(e) => {
                warn!("failed to put copied files into the clipboard: {e:#}");
                notify(FilesNotice::Failed { peer, reason: format!("{e:#}") });
            }
        }
    });
}

fn verify_server(config: &ClientConfig, trust: &mut TrustStore, fingerprint: &Fingerprint) -> Result<()> {
    if let Some(pinned) = &config.server_fingerprint {
        if pinned != fingerprint {
            return Err(Fatal(format!(
                "server fingerprint does not match: expected {pinned}, got {fingerprint}"
            ))
            .into());
        }
        return Ok(());
    }
    match trust.check(&config.server, fingerprint) {
        TrustOutcome::Known => Ok(()),
        TrustOutcome::Unknown => {
            warn!(
                "first contact with {}: storing fingerprint {fingerprint}. \
                 Compare it with the output of `qross fingerprint` on the server.",
                config.server
            );
            trust.trust(&config.server, fingerprint)
        }
        TrustOutcome::Mismatch => Err(Fatal(format!(
            "the server {} has a different fingerprint than last time \
             (stored: {}, now: {fingerprint}). If it was set up again, delete its entry in {KNOWN_SERVERS_FILE}.",
            config.server,
            trust.expected(&config.server).unwrap_or("?"),
        ))
        .into()),
    }
}

async fn resolve(server: &str) -> Result<SocketAddr> {
    if let Ok(addr) = server.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = server.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let target = match server.rsplit_once(':') {
        Some((_, port)) if port.parse::<u16>().is_ok() => server.to_owned(),
        _ => format!("{server}:{DEFAULT_PORT}"),
    };
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&target)
        .await
        .with_context(|| format!("failed to resolve '{server}'"))?
        .collect();
    // Der Server lauscht standardmäßig auf IPv4; Windows liefert oft zuerst IPv6.
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .with_context(|| format!("'{server}' has no address"))
}

/// Was der Server nach dem `Welcome` auf dem Control-Stream schickt (seit Qross 0.7: die Rechte,
/// seit 0.8 den Cursor-Finder und den Bildschirmschoner).
async fn receive_control(mut control: RecvStream, tx: mpsc::UnboundedSender<FromServer>) {
    loop {
        match read_frame::<_, Control>(&mut control).await {
            Ok(Some(Control::Sharing(sharing))) => {
                let _ = tx.send(FromServer::Sharing(sharing));
            }
            Ok(Some(Control::CursorFinder(finder))) => {
                let _ = tx.send(FromServer::CursorFinder(finder));
            }
            Ok(Some(Control::ScreenSaver { active, lock })) => {
                let _ = tx.send(FromServer::ScreenSaver { active, lock });
            }
            Ok(Some(Control::Latency(Latency::Summary(summary)))) => {
                let _ = tx.send(FromServer::Latency(summary));
            }
            Ok(Some(Control::PastesOnDemand)) => {
                let _ = tx.send(FromServer::PastesOnDemand);
            }
            Ok(Some(Control::WatchUsage(on))) => {
                let _ = tx.send(FromServer::WatchUsage(on));
            }
            Ok(Some(other)) => debug!(?other, "unexpected control message from the server"),
            // Von einer neueren Version: überspringen, der Frame ist gelesen
            Err(FrameError::Codec(e)) => debug!("unknown control message from the server: {e}"),
            Ok(None) | Err(_) => return,
        }
    }
}

async fn receive_datagrams(conn: Connection, tx: mpsc::UnboundedSender<FromServer>) {
    while let Ok(bytes) = conn.read_datagram().await {
        let received = Instant::now();
        match decode_datagram(&bytes) {
            Ok(Datagram::MouseMove(mv)) => {
                let _ = tx.send(FromServer::Move(mv, received));
            }
            Ok(Datagram::MouseDelta(delta)) => {
                let _ = tx.send(FromServer::Delta(delta, received));
            }
            Err(e) => debug!("invalid datagram: {e}"),
        }
    }
}

/// Antwort auf die Drag-Nachfrage des Servers: Werden hier Dateien gezogen, gehen sie zum Server.
async fn offer_dragged_files(
    conn: Connection,
    probe: u32,
    dragged: oneshot::Receiver<Vec<PathBuf>>,
    server_name: String,
    transfers: Transfers,
) {
    let paths = dragged.await.unwrap_or_default();
    let entries = if paths.is_empty() {
        Vec::new()
    } else {
        match tokio::task::spawn_blocking(move || files::collect(&paths)).await {
            Ok(Ok(entries)) => entries,
            Ok(Err(e)) => {
                warn!("failed to read dragged files: {e:#}");
                Vec::new()
            }
            Err(_) => Vec::new(),
        }
    };
    let transfer = clipfiles::new_transfer_id();
    let offer = DragMessage::Offer { probe, transfer, entries: entries.iter().map(|e| e.entry.clone()).collect() };
    if let Err(e) = send_drag_message(&conn, &offer).await {
        return warn!("failed to send drag reply to the server: {e:#}");
    }
    if entries.is_empty() {
        return;
    }
    let total: u64 = entries.iter().map(|e| e.entry.size).sum();
    info!("transferring {} dragged entries ({}) to the server", entries.len(), files::human_size(total));
    if let Err(e) = files::send_tracked(conn, StreamKind::File, transfer, entries, &server_name, &transfers).await {
        warn!("transfer to the server aborted: {e:#}");
    }
}

/// Zwischenordner für gezogene und kopierte Dateien, Fortschritt
type Stagings = (Arc<Staging>, Arc<Staging>, Transfers);

async fn receive_streams(conn: Connection, tx: mpsc::UnboundedSender<FromServer>, stagings: Stagings) {
    while let Ok(mut stream) = conn.accept_uni().await {
        let (tx, stagings) = (tx.clone(), stagings.clone());
        tokio::spawn(async move {
            if let Err(e) = read_server_stream(&mut stream, &tx, &stagings).await {
                debug!("stream from the server ended: {e:#}");
            }
        });
    }
}

async fn read_server_stream(
    stream: &mut RecvStream,
    tx: &mpsc::UnboundedSender<FromServer>,
    (drag, clipboard, transfers): &Stagings,
) -> Result<()> {
    match read_kind(stream).await? {
        StreamKind::Input => {
            // Laufende Nummer wie beim Server: daran erkennt er die Stichproben wieder
            let mut index = 0u64;
            while let Some(input) = read_frame::<_, Input>(stream).await? {
                let received = Instant::now();
                index += 1;
                let stamp = Stamp::input(&input, index, received);
                let _ = tx.send(FromServer::Input(input, stamp));
            }
        }
        StreamKind::Clipboard => match read_frame(stream).await? {
            Some(ClipboardMsg::Data(data)) => {
                let _ = tx.send(FromServer::Clipboard(data));
            }
            Some(ClipboardMsg::Files { transfer, entries, data }) => {
                let _ = tx.send(FromServer::ClipboardFiles { transfer, entries, data });
            }
            Some(ClipboardMsg::FilesOnDemand { transfer, entries, data }) => {
                let _ = tx.send(FromServer::ClipboardFilesOnDemand { transfer, entries, data });
            }
            None => {}
        },
        StreamKind::File => {
            let outcome = files::receive_file(stream, drag, transfers).await?;
            let _ = tx.send(FromServer::File(outcome));
        }
        StreamKind::ClipboardFile => {
            let outcome = files::receive_file(stream, clipboard, transfers).await?;
            let _ = tx.send(FromServer::ClipboardFile(outcome));
        }
        StreamKind::Drag => {
            if let Some(DragMessage::Query { probe }) = read_frame(stream).await? {
                let _ = tx.send(FromServer::DragQuery { probe });
            }
        }
        // Abrufe kommen auf bidirektionalen Streams (siehe `ondemand::accept_fetches`).
        StreamKind::Fetch => bail!("fetch on a unidirectional stream"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use qross_platform::dryrun::RecordingInjector;
    use qross_proto::KeyCode;

    use super::*;

    fn mark(seq: u32, done: &mpsc::UnboundedSender<Measured>) -> ToInjector {
        ToInjector::Measured { stamp: Stamp { id: SampleId::Mouse { epoch: 1, seq }, received: Instant::now() }, done: done.clone() }
    }

    fn move_to(x: i32) -> ToInjector {
        ToInjector::Inject(Inject::MoveTo { x, y: 0 })
    }

    #[test]
    fn waiting_moves_go_out_together() {
        let (jobs, waiting) = std::sync::mpsc::channel();
        let (done, _answers) = mpsc::unbounded_channel();
        // Von Positionen zählt nur die letzte; die Stichproben dazwischen kommen mit.
        for job in [mark(1, &done), move_to(2), mark(2, &done), move_to(3)] {
            jobs.send(job).unwrap();
        }
        jobs.send(ToInjector::Inject(Inject::Key { key: KeyCode::A, down: true })).unwrap();
        let (event, marks, next) = coalesce(Inject::MoveTo { x: 1, y: 0 }, &waiting);
        assert_eq!((event, marks.len()), (Inject::MoveTo { x: 3, y: 0 }, 2));
        assert!(matches!(next, Some(ToInjector::Inject(Inject::Key { .. }))), "danach der Reihe nach weiter");

        // Relative Bewegungen zählen alle, eine Position beendet sie.
        for (dx, dy) in [(3, -1), (4, -2)] {
            jobs.send(ToInjector::Inject(Inject::MoveBy { dx, dy })).unwrap();
        }
        jobs.send(move_to(9)).unwrap();
        let (event, _, next) = coalesce(Inject::MoveBy { dx: 1, dy: 0 }, &waiting);
        assert_eq!(event, Inject::MoveBy { dx: 8, dy: -3 });
        assert!(matches!(next, Some(ToInjector::Inject(Inject::MoveTo { x: 9, y: 0 }))));

        let (event, marks, next) = coalesce(Inject::MoveTo { x: 9, y: 0 }, &waiting);
        assert_eq!((event, marks.len()), (Inject::MoveTo { x: 9, y: 0 }, 0));
        assert!(next.is_none(), "nichts wartet");
    }

    #[test]
    fn a_burst_of_moves_is_injected_once_and_every_sample_answered() {
        let (jobs, waiting) = std::sync::mpsc::channel();
        let (done, mut answers) = mpsc::unbounded_channel();
        // Nach einem Hänger kommen viele Bewegungen auf einmal an.
        for x in 1..=100 {
            jobs.send(move_to(x)).unwrap();
            if x % 32 == 0 {
                jobs.send(mark(x as u32, &done)).unwrap();
            }
        }
        jobs.send(ToInjector::Inject(Inject::Key { key: KeyCode::A, down: true })).unwrap();
        drop(jobs);
        let injected = RecordingInjector::default();
        inject_loop(Box::new(injected.clone()), waiting);
        assert_eq!(
            *injected.0.lock().unwrap(),
            [Inject::MoveTo { x: 100, y: 0 }, Inject::Key { key: KeyCode::A, down: true }],
            "die alte Bahn wird nicht nachgefahren"
        );
        let answered: Vec<_> = std::iter::from_fn(|| answers.try_recv().ok()).collect();
        assert_eq!(answered.len(), 3, "jede Stichprobe bekommt ihre Antwort");
    }
}
