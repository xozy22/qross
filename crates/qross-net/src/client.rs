//! Client-Laufzeit: verbindet sich (mit automatischer Wiederverbindung) und speist ein.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use qross_core::client::{ClientState, Transition};
use qross_core::sharing::describe as describe_sharing;
use qross_core::Inject;
use qross_platform::{Clipboard, Injector};
use qross_proto::{
    decode_datagram, read_frame, write_frame, Clipboard as ClipboardMsg, ClipboardData, Control, Datagram, DragMessage,
    FileEntry, FrameError, GameState, Input, Latency, LatencySummary, MouseDelta, MouseMove, Platform, Rect, ScreenInfo,
    Sharing, StreamKind, DEFAULT_PORT, GAME_MODE_SINCE, LATENCY_SINCE, MONITORS_SINCE, PROTOCOL_VERSION, SHARING_SINCE,
    TEXT_SINCE,
};
use quinn::{Connection, Endpoint, RecvStream};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::clipboard::ClipboardHub;
use crate::clipfiles::{self, FilesNotice, Incoming, Notify, Ready};
use crate::files::{self, FileOutcome, Receiver, Staging};
use crate::game::{self, Detector};
use crate::latency::{self, Collector, Measured, Stamp};
use crate::progress::{Progress, Transfers};
use crate::quality::{self, Quality};
use crate::rights::{self, Allowed};
use crate::streams::{read_kind, send_drag_message};
use crate::tls::{client_config, peer_fingerprint, SERVER_NAME};
use crate::{Fingerprint, Identity, TrustOutcome, TrustStore, KNOWN_SERVERS_FILE};

/// Wiederverbindung: schnell beginnen (Server-Neustart dauert oft nur Sekundenbruchteile),
/// dann bis zur Obergrenze verdoppeln.
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(3);
/// Eine Sitzung, die so lange lief, gilt als erfolgreich – danach wieder schnell versuchen.
const STABLE_SESSION: Duration = Duration::from_secs(10);

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
}

/// Abfrage der eigenen Bildschirmfläche und des Vordergrunds (aus `ClientPlatform`)
struct ScreenQuery {
    bounds: Box<dyn Fn() -> Rect + Send + Sync>,
    monitors: Box<dyn Fn() -> Vec<Rect> + Send + Sync>,
    game: watch::Receiver<GameState>,
    /// Das Einspeisen kann Zeichen eingeben (Zeichenmodus)
    types_text: bool,
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

/// Was vom Server kommt; Stichproben der Eingabekette mit dem Zeitpunkt ihres Empfangs
enum FromServer {
    Move(MouseMove, Option<Stamp>),
    /// Relative Bewegung (Spielemodus)
    Delta(MouseDelta, Option<Stamp>),
    Input(Input, Option<Stamp>),
    Clipboard(ClipboardData),
    File(FileOutcome),
    ClipboardFiles { transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    ClipboardFile(FileOutcome),
    /// Server fragt: Werden hier gerade Dateien gezogen? (Drag & Drop zum Server)
    DragQuery { probe: u32 },
    /// Was der Server mit diesem Rechner teilt (seit Qross 0.7)
    Sharing(Sharing),
    /// Ergebnis der Messung der Eingabekette (ab `LATENCY_SINCE`)
    Latency(LatencySummary),
}

/// Zwischenablage dieses Clients: Zugriff, ankommende kopierte Dateien, Meldungen dazu.
struct ClipboardSide {
    hub: ClipboardHub,
    incoming: Incoming,
    notify: Notify,
    /// Fortschritt und Abbrechen aller Dateiübertragungen (auch Drag & Drop)
    transfers: Transfers,
}

/// Aufträge an den Einspeise-Thread. Die Ablageordner-Abfrage läuft dort, damit sie nach den
/// vorher eingespeisten Bewegungen ausgewertet wird.
enum ToInjector {
    Inject(Inject),
    DropFolder { x: i32, y: i32, reply: oneshot::Sender<Option<PathBuf>> },
    /// Gezogene Dateien übernehmen (der lokale Drag wird dabei abgebrochen)
    TakeDragged { x: i32, y: i32, reply: oneshot::Sender<Vec<PathBuf>> },
    /// Stichprobe der Eingabekette: Alles davor ist eingespeist, jetzt die Zeit nehmen.
    Measured { stamp: Stamp, done: mpsc::UnboundedSender<Measured> },
}

fn inject_all(inject: &Sender<ToInjector>, events: impl IntoIterator<Item = Inject>) {
    for event in events {
        let _ = inject.send(ToInjector::Inject(event));
    }
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

/// Läuft, bis ein nicht behebbarer Fehler auftritt (z. B. falscher Server-Fingerprint).
pub async fn run(config: ClientConfig, identity: Identity, trust: TrustStore, platform: ClientPlatform) -> Result<()> {
    run_inner(config, identity, trust, platform, None, Transfers::new()).await
}

/// Wie [`run`], meldet den Verbindungszustand zusätzlich über `notices`.
pub async fn run_with_notices(
    config: ClientConfig,
    identity: Identity,
    trust: TrustStore,
    platform: ClientPlatform,
    notices: mpsc::UnboundedSender<ClientNotice>,
) -> Result<()> {
    run_inner(config, identity, trust, platform, Some(&notices), Transfers::new()).await
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
    run_inner(config, identity, trust, platform, Some(&notices), transfers).await
}

/// Baut nur den TLS-Handshake auf und liefert den Fingerprint des Servers, ohne sich anzumelden
/// (z. B. um ihn vor dem ersten Verbinden bestätigen zu lassen).
pub async fn probe_fingerprint(server: &str, identity: &Identity) -> Result<(SocketAddr, Fingerprint)> {
    let addr = resolve(server).await?;
    let bind: SocketAddr = match addr.ip() {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let mut endpoint = Endpoint::client(bind)?;
    endpoint.set_default_client_config(client_config(identity)?);
    let conn = endpoint
        .connect(addr, SERVER_NAME)?
        .await
        .with_context(|| format!("keine Verbindung zu {addr}"))?;
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
) -> Result<()> {
    let ClientPlatform { mut injector, clipboard, screen, monitors, game } = platform;
    let types_text = injector.types_text();
    let screen = ScreenQuery { bounds: screen, monitors, game: game::watch(game), types_text };
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
    std::thread::Builder::new().name("qross-inject".into()).spawn(move || {
        while let Ok(job) = inject_rx.recv() {
            match job {
                ToInjector::Inject(event) => {
                    match event {
                        Inject::MoveTo { .. } | Inject::MoveBy { .. } => tracing::trace!(?event, "einspeisen"),
                        _ => debug!(?event, "einspeisen"),
                    }
                    injector.inject(&event);
                }
                ToInjector::DropFolder { x, y, reply } => {
                    let _ = reply.send(injector.drop_folder(x, y));
                }
                ToInjector::TakeDragged { x, y, reply } => {
                    let _ = reply.send(injector.take_dragged_files(x, y));
                }
                ToInjector::Measured { stamp, done } => {
                    let _ = done.send(Measured { id: stamp.id, received: stamp.received, done: Instant::now() });
                }
            }
        }
    })?;

    let tls = client_config(&identity)?;
    let mut retry = RETRY_MIN;
    loop {
        let started = std::time::Instant::now();
        let result = session(&config, &tls, &mut trust, &inject_tx, &mut clipboard, &screen, notices).await;
        retry = if started.elapsed() > STABLE_SESSION { RETRY_MIN } else { (retry * 2).min(RETRY_MAX) };
        match result {
            Ok(()) => {
                info!("Verbindung zum Server beendet");
                notify(notices, ClientNotice::Disconnected { reason: "Verbindung zum Server beendet".into() });
            }
            Err(e) if e.downcast_ref::<Fatal>().is_some() => return Err(e),
            Err(e) => {
                warn!("{e:#}");
                notify(notices, ClientNotice::Disconnected { reason: format!("{e:#}") });
            }
        }
        tokio::time::sleep(retry).await;
    }
}

async fn session(
    config: &ClientConfig,
    tls: &quinn::ClientConfig,
    trust: &mut TrustStore,
    inject: &Sender<ToInjector>,
    clip: &mut ClipboardSide,
    screen: &ScreenQuery,
    notices: Notices<'_>,
) -> Result<()> {
    let addr = resolve(&config.server).await?;
    let bind: SocketAddr = match addr.ip() {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let mut endpoint = Endpoint::client(bind)?;
    endpoint.set_default_client_config(tls.clone());

    debug!("verbinde mit {addr}");
    notify(notices, ClientNotice::Connecting { addr });
    let conn = endpoint
        .connect(addr, SERVER_NAME)?
        .await
        .with_context(|| format!("keine Verbindung zu {addr}"))?;
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
            info!("verbunden mit Server „{server_name}“ ({addr})");
            notify(notices, ClientNotice::Connected { server_name: server_name.clone(), addr });
            (server_name, platform, version)
        }
        Some(Control::Reject { reason }) => bail!("Server hat die Verbindung abgelehnt: {reason}"),
        other => bail!("unerwartete Antwort vom Server: {other:?}"),
    };
    // Zeichen statt Tastenpositionen kann nur ein Server schicken, der sie kennt; ältere trennen bei
    // unbekannten Nachrichten. Ohne diese Meldung bleibt es bei Tastenpositionen.
    if server_version >= TEXT_SINCE && screen.types_text {
        write_frame(&mut control, &Control::TypesText).await?;
    }
    // Die einzelnen Monitore nur an Server, die sie kennen: ältere trennen bei unbekannten Nachrichten.
    let report_monitors = server_version >= MONITORS_SINCE;
    let mut monitors = (screen.monitors)();
    if report_monitors && !monitors.is_empty() {
        debug!(?monitors, "Monitore an den Server gemeldet");
        write_frame(&mut control, &Control::Monitors(monitors.clone())).await?;
    }
    // Spielemodus nur an Server, die ihn kennen; anfangs nur, wenn schon etwas im Vollbild läuft.
    let report_game = server_version >= GAME_MODE_SINCE;
    let mut game_rx = screen.game.clone();
    let mut game = *game_rx.borrow_and_update();
    if report_game && game.is_active() {
        info!("Spielemodus: {} im Vordergrund, an den Server gemeldet", game::describe(game));
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

    let (tx, mut rx) = mpsc::unbounded_channel();
    // Rechte je Rechner: nur neuere Server schicken nach dem Welcome noch etwas.
    if server_version >= SHARING_SINCE {
        tokio::spawn(receive_control(control_rx, tx.clone()));
    }
    tokio::spawn(receive_datagrams(conn.clone(), tx.clone()));
    let stagings = (receiver.staging(), clip.incoming.staging(), clip.transfers.clone());
    tokio::spawn(receive_streams(conn.clone(), tx, stagings));
    // Übertragungen der vorigen Verbindung kommen nie mehr vollständig an.
    clip.incoming.supersede();

    let mut state = ClientState::new();
    // PC steuert Mac (oder umgekehrt): Strg und Cmd tauschen, damit z. B. Strg+C kopiert
    let swap = (server_platform == Platform::MacOs) != (Platform::current() == Platform::MacOs);
    if swap {
        info!("Strg und Cmd werden getauscht (Server: {server_platform:?})");
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

    let outcome: Result<()> = async {
        loop {
            tokio::select! {
                Some(msg) = rx.recv() => match msg {
                    FromServer::Move(mv, stamp) => {
                        if let Some(event) = state.on_move(mv) {
                            moves += 1;
                            last_pos = (mv.x, mv.y);
                            let _ = inject.send(ToInjector::Inject(event));
                            mark(inject, stamp, marks.as_ref());
                        }
                    }
                    FromServer::Delta(delta, stamp) => {
                        if let Some(event) = state.on_delta(delta) {
                            moves += 1;
                            let _ = inject.send(ToInjector::Inject(event));
                            mark(inject, stamp, marks.as_ref());
                        }
                    }
                    FromServer::Input(input, stamp) => {
                        let dropped = match &input {
                            Input::Enter { x, y, .. } => {
                                (moves, last_pos) = (0, (*x, *y));
                                None
                            }
                            Input::DragStart { transfer, entries } => {
                                receiver.on_drag_start(&server_name, *transfer, entries.clone());
                                None
                            }
                            Input::Drop { transfer, x, y } => Some((*transfer, *x, *y)),
                            _ => None,
                        };
                        let (events, transition) = state.on_input(input);
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
                        match transition {
                            Some(Transition::Entered) => {
                                info!("Cursor betritt diesen Rechner bei {},{}", last_pos.0, last_pos.1);
                                notify(notices, ClientNotice::CursorEntered);
                            }
                            Some(Transition::Left) => {
                                receiver.cancel_undropped();
                                notify(notices, ClientNotice::CursorLeft);
                                info!(
                                    "Cursor verlässt diesen Rechner ({moves} Bewegungen, zuletzt bei {},{})",
                                    last_pos.0, last_pos.1
                                );
                                // Beim Verlassen das eigene Clipboard mitgeben, falls es sich geändert hat
                                // und der Server es annimmt.
                                let allowed = Allowed { clipboard: sharing.clipboard.from_client(), files: sharing.files.from_client() };
                                if allowed.any() {
                                    let (hub, synced, conn) = (clip.hub.clone(), clip_synced.clone(), conn.clone());
                                    let (limit, peer, notify) = (config.clipboard_file_limit, server_name.clone(), clip.notify.clone());
                                    let transfers = clip.transfers.clone();
                                    tokio::spawn(async move {
                                        let known = *synced.lock().unwrap();
                                        let (seq, content) = hub.changed_since(Some(known)).await;
                                        *synced.lock().unwrap() = seq;
                                        if let Some(content) = content.and_then(|c| rights::content(c, allowed)) {
                                            clipfiles::send(conn, content, limit, peer, notify, transfers).await;
                                        }
                                    });
                                } else {
                                    debug!("Zwischenablage bleibt hier: der Server nimmt sie nicht an");
                                }
                            }
                            None => {}
                        }
                    }
                    FromServer::Clipboard(data) => {
                        info!("Zwischenablage vom Server: {}", data.summary());
                        clip.incoming.supersede();
                        let (hub, synced) = (clip.hub.clone(), clip_synced.clone());
                        tokio::spawn(async move {
                            match hub.write(data).await {
                                Ok(seq) => *synced.lock().unwrap() = seq,
                                Err(e) => warn!("Zwischenablage nicht übernommen: {e:#}"),
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
                                warn!("kopierte Dateien vom Server abgelehnt: {e:#}");
                                (clip.notify)(FilesNotice::Failed { peer: server_name.clone(), reason: format!("{e:#}") });
                            }
                        }
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
                            info!("Rechte dieses Rechners beim Server: {}", describe_sharing(now));
                        }
                        sharing = now;
                        notify(notices, ClientNotice::Sharing(now));
                    }
                    FromServer::Latency(summary) => notify(notices, ClientNotice::Latency(summary)),
                },
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
                            debug!(?monitors, "Monitore geändert");
                            write_frame(&mut control, &Control::Monitors(monitors.clone())).await?;
                        }
                    }
                }
                Ok(()) = game_rx.changed(), if report_game => {
                    let now = *game_rx.borrow_and_update();
                    if now != game {
                        game = now;
                        debug!("Spielemodus: {} im Vordergrund, an den Server gemeldet", game::describe(game));
                        write_frame(&mut control, &Control::Game(game)).await?;
                    }
                }
                reason = conn.closed() => return Err(anyhow!("Verbindung getrennt: {reason}")),
            }
        }
    }
    .await;

    // Nichts gedrückt lassen, wenn die Verbindung mitten in einer Eingabe abreißt.
    inject_all(inject, state.release_all());
    receiver.cancel_undropped();
    // Was noch ankam, wird nie vollständig.
    clip.transfers.abort_receiving(None, "Verbindung getrennt");
    outcome
}

/// Stichprobe der Eingabekette: Hinter den Einspeise-Aufträgen der Eingabe meldet der Einspeise-Thread,
/// wann sie erledigt waren (nur, wenn der Server mitmisst).
fn mark(inject: &Sender<ToInjector>, stamp: Option<Stamp>, marks: Option<&mpsc::UnboundedSender<Measured>>) {
    if let (Some(stamp), Some(done)) = (stamp, marks) {
        let _ = inject.send(ToInjector::Measured { stamp, done: done.clone() });
    }
}

/// Fertig übertragene Dateien vom Server in die Zwischenablage legen.
fn deliver_files(ready: Ready, clip: &ClipboardSide, synced: &Arc<std::sync::Mutex<u64>>) {
    let (hub, synced, notify) = (clip.hub.clone(), synced.clone(), clip.notify.clone());
    tokio::spawn(async move {
        let Ready { peer, paths, data, names, bytes } = ready;
        match hub.write_files(paths, data).await {
            Ok(seq) => {
                info!("kopierte Dateien liegen in der Zwischenablage");
                *synced.lock().unwrap() = seq;
                notify(FilesNotice::Ready { peer, names, bytes });
            }
            Err(e) => {
                warn!("kopierte Dateien nicht in die Zwischenablage gelegt: {e:#}");
                notify(FilesNotice::Failed { peer, reason: format!("{e:#}") });
            }
        }
    });
}

fn verify_server(config: &ClientConfig, trust: &mut TrustStore, fingerprint: &Fingerprint) -> Result<()> {
    if let Some(pinned) = &config.server_fingerprint {
        if pinned != fingerprint {
            return Err(Fatal(format!(
                "Fingerprint des Servers stimmt nicht: erwartet {pinned}, erhalten {fingerprint}"
            ))
            .into());
        }
        return Ok(());
    }
    match trust.check(&config.server, fingerprint) {
        TrustOutcome::Known => Ok(()),
        TrustOutcome::Unknown => {
            warn!(
                "Erster Kontakt mit {}: Fingerprint {fingerprint} wird gespeichert. \
                 Vergleiche ihn mit der Ausgabe von `qross fingerprint` auf dem Server.",
                config.server
            );
            trust.trust(&config.server, fingerprint)
        }
        TrustOutcome::Mismatch => Err(Fatal(format!(
            "Der Server {} hat einen anderen Fingerprint als beim letzten Mal \
             (gespeichert: {}, jetzt: {fingerprint}). Wurde er neu eingerichtet, den Eintrag in {KNOWN_SERVERS_FILE} löschen.",
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
        .with_context(|| format!("„{server}“ nicht auflösbar"))?
        .collect();
    // Der Server lauscht standardmäßig auf IPv4; Windows liefert oft zuerst IPv6.
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .with_context(|| format!("„{server}“ hat keine Adresse"))
}

/// Was der Server nach dem `Welcome` auf dem Control-Stream schickt (seit Qross 0.7: die Rechte).
async fn receive_control(mut control: RecvStream, tx: mpsc::UnboundedSender<FromServer>) {
    loop {
        match read_frame::<_, Control>(&mut control).await {
            Ok(Some(Control::Sharing(sharing))) => {
                let _ = tx.send(FromServer::Sharing(sharing));
            }
            Ok(Some(Control::Latency(Latency::Summary(summary)))) => {
                let _ = tx.send(FromServer::Latency(summary));
            }
            Ok(Some(other)) => debug!(?other, "unerwartete Control-Nachricht vom Server"),
            // Von einer neueren Version: überspringen, der Frame ist gelesen
            Err(FrameError::Codec(e)) => debug!("unbekannte Control-Nachricht vom Server: {e}"),
            Ok(None) | Err(_) => return,
        }
    }
}

async fn receive_datagrams(conn: Connection, tx: mpsc::UnboundedSender<FromServer>) {
    while let Ok(bytes) = conn.read_datagram().await {
        let received = Instant::now();
        match decode_datagram(&bytes) {
            Ok(Datagram::MouseMove(mv)) => {
                let _ = tx.send(FromServer::Move(mv, Stamp::mouse(mv.epoch, mv.seq, received)));
            }
            Ok(Datagram::MouseDelta(delta)) => {
                let _ = tx.send(FromServer::Delta(delta, Stamp::mouse(delta.epoch, delta.seq, received)));
            }
            Err(e) => debug!("ungültiges Datagramm: {e}"),
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
                warn!("Gezogene Dateien nicht lesbar: {e:#}");
                Vec::new()
            }
            Err(_) => Vec::new(),
        }
    };
    let transfer = clipfiles::new_transfer_id();
    let offer = DragMessage::Offer { probe, transfer, entries: entries.iter().map(|e| e.entry.clone()).collect() };
    if let Err(e) = send_drag_message(&conn, &offer).await {
        return warn!("Drag-Antwort an den Server nicht gesendet: {e:#}");
    }
    if entries.is_empty() {
        return;
    }
    let total: u64 = entries.iter().map(|e| e.entry.size).sum();
    info!("übertrage {} gezogene Einträge ({}) zum Server", entries.len(), files::human_size(total));
    if let Err(e) = files::send_tracked(conn, StreamKind::File, transfer, entries, &server_name, &transfers).await {
        warn!("Übertragung zum Server abgebrochen: {e:#}");
    }
}

/// Zwischenordner für gezogene und kopierte Dateien, Fortschritt
type Stagings = (Arc<Staging>, Arc<Staging>, Transfers);

async fn receive_streams(conn: Connection, tx: mpsc::UnboundedSender<FromServer>, stagings: Stagings) {
    while let Ok(mut stream) = conn.accept_uni().await {
        let (tx, stagings) = (tx.clone(), stagings.clone());
        tokio::spawn(async move {
            if let Err(e) = read_server_stream(&mut stream, &tx, &stagings).await {
                debug!("Stream vom Server beendet: {e:#}");
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
    }
    Ok(())
}
