//! Client-Laufzeit: verbindet sich (mit automatischer Wiederverbindung) und speist ein.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use qross_core::client::{ClientState, Transition};
use qross_core::Inject;
use qross_platform::{Clipboard, Injector};
use qross_proto::{
    decode_datagram, read_frame, write_frame, Clipboard as ClipboardMsg, ClipboardData, Control, Datagram, DragMessage,
    FileEntry, Input, MouseMove, Platform, Rect, ScreenInfo, StreamKind, DEFAULT_PORT, MONITORS_SINCE, PROTOCOL_VERSION,
};
use quinn::{Connection, Endpoint, RecvStream};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::clipboard::ClipboardHub;
use crate::clipfiles::{self, FilesNotice, Incoming, Notify, Ready};
use crate::files::{self, FileOutcome, Receiver, Staging};
use crate::quality::{self, Quality};
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
}

pub struct ClientPlatform {
    pub injector: Box<dyn Injector>,
    pub clipboard: Box<dyn Clipboard>,
    pub screen: Box<dyn Fn() -> Rect + Send + Sync>,
    /// Die einzelnen Monitore in den Koordinaten von `screen` (leer: unbekannt)
    pub monitors: Box<dyn Fn() -> Vec<Rect> + Send + Sync>,
}

/// Abfrage der eigenen Bildschirmfläche (aus `ClientPlatform`)
struct ScreenQuery {
    bounds: Box<dyn Fn() -> Rect + Send + Sync>,
    monitors: Box<dyn Fn() -> Vec<Rect> + Send + Sync>,
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

enum FromServer {
    Move(MouseMove),
    Input(Input),
    Clipboard(ClipboardData),
    File(FileOutcome),
    ClipboardFiles { transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    ClipboardFile(FileOutcome),
    /// Server fragt: Werden hier gerade Dateien gezogen? (Drag & Drop zum Server)
    DragQuery { probe: u32 },
}

/// Zwischenablage dieses Clients: Zugriff, ankommende kopierte Dateien, Meldungen dazu.
struct ClipboardSide {
    hub: ClipboardHub,
    incoming: Incoming,
    notify: Notify,
}

/// Aufträge an den Einspeise-Thread. Die Ablageordner-Abfrage läuft dort, damit sie nach den
/// vorher eingespeisten Bewegungen ausgewertet wird.
enum ToInjector {
    Inject(Inject),
    DropFolder { x: i32, y: i32, reply: oneshot::Sender<Option<PathBuf>> },
    /// Gezogene Dateien übernehmen (der lokale Drag wird dabei abgebrochen)
    TakeDragged { x: i32, y: i32, reply: oneshot::Sender<Vec<PathBuf>> },
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
}

type Notices<'a> = Option<&'a mpsc::UnboundedSender<ClientNotice>>;

fn notify(notices: Notices<'_>, notice: ClientNotice) {
    if let Some(tx) = notices {
        let _ = tx.send(notice);
    }
}

/// Läuft, bis ein nicht behebbarer Fehler auftritt (z. B. falscher Server-Fingerprint).
pub async fn run(config: ClientConfig, identity: Identity, trust: TrustStore, platform: ClientPlatform) -> Result<()> {
    run_inner(config, identity, trust, platform, None).await
}

/// Wie [`run`], meldet den Verbindungszustand zusätzlich über `notices`.
pub async fn run_with_notices(
    config: ClientConfig,
    identity: Identity,
    trust: TrustStore,
    platform: ClientPlatform,
    notices: mpsc::UnboundedSender<ClientNotice>,
) -> Result<()> {
    run_inner(config, identity, trust, platform, Some(&notices)).await
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
) -> Result<()> {
    let ClientPlatform { mut injector, clipboard, screen, monitors } = platform;
    let screen = ScreenQuery { bounds: screen, monitors };
    let mut clipboard = ClipboardSide {
        hub: ClipboardHub::spawn(clipboard),
        incoming: Incoming::new(Staging::persistent(&clipfiles::default_store())?),
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
                        Inject::MoveTo { .. } => tracing::trace!(?event, "einspeisen"),
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
    // Die einzelnen Monitore nur an Server, die sie kennen: ältere trennen bei unbekannten Nachrichten.
    let report_monitors = server_version >= MONITORS_SINCE;
    let mut monitors = (screen.monitors)();
    if report_monitors && !monitors.is_empty() {
        debug!(?monitors, "Monitore an den Server gemeldet");
        write_frame(&mut control, &Control::Monitors(monitors.clone())).await?;
    }
    // Läuft bis zum Ende dieser Sitzung
    let _quality = notices.cloned().map(|tx| {
        quality::monitor(conn.clone(), move |q| {
            let _ = tx.send(ClientNotice::Quality(q));
        })
    });

    let mut receiver = Receiver::new(Staging::create(&files::drag_staging_base())?);

    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(receive_datagrams(conn.clone(), tx.clone()));
    tokio::spawn(receive_streams(conn.clone(), tx, receiver.staging(), clip.incoming.staging()));
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

    let outcome: Result<()> = async {
        loop {
            tokio::select! {
                Some(msg) = rx.recv() => match msg {
                    FromServer::Move(mv) => {
                        if let Some(event) = state.on_move(mv) {
                            moves += 1;
                            last_pos = (mv.x, mv.y);
                            let _ = inject.send(ToInjector::Inject(event));
                        }
                    }
                    FromServer::Input(input) => {
                        let dropped = match &input {
                            Input::Enter { x, y, .. } => {
                                (moves, last_pos) = (0, (*x, *y));
                                None
                            }
                            Input::DragStart { transfer, entries } => {
                                receiver.on_drag_start(*transfer, entries.clone());
                                None
                            }
                            Input::Drop { transfer, x, y } => Some((*transfer, *x, *y)),
                            _ => None,
                        };
                        let (events, transition) = state.on_input(input);
                        inject_all(inject, events);
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
                                // Beim Verlassen das eigene Clipboard mitgeben, falls es sich geändert hat.
                                let (hub, synced, conn) = (clip.hub.clone(), clip_synced.clone(), conn.clone());
                                let (limit, peer, notify) = (config.clipboard_file_limit, server_name.clone(), clip.notify.clone());
                                tokio::spawn(async move {
                                    let known = *synced.lock().unwrap();
                                    let (seq, content) = hub.changed_since(Some(known)).await;
                                    *synced.lock().unwrap() = seq;
                                    if let Some(content) = content {
                                        clipfiles::send(conn, content, limit, peer, notify).await;
                                    }
                                });
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
                        let _ = inject.send(ToInjector::TakeDragged { x: last_pos.0, y: last_pos.1, reply });
                        tokio::spawn(offer_dragged_files(conn.clone(), probe, dragged));
                    }
                    FromServer::ClipboardFile(outcome) => match clip.incoming.on_file(outcome) {
                        Ok(Some(ready)) => deliver_files(ready, clip, &clip_synced),
                        Ok(None) => {}
                        Err(failed) => (clip.notify)(FilesNotice::Failed { peer: failed.peer, reason: failed.reason }),
                    },
                },
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
                reason = conn.closed() => return Err(anyhow!("Verbindung getrennt: {reason}")),
            }
        }
    }
    .await;

    // Nichts gedrückt lassen, wenn die Verbindung mitten in einer Eingabe abreißt.
    inject_all(inject, state.release_all());
    receiver.cancel_undropped();
    outcome
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

async fn receive_datagrams(conn: Connection, tx: mpsc::UnboundedSender<FromServer>) {
    while let Ok(bytes) = conn.read_datagram().await {
        match decode_datagram(&bytes) {
            Ok(Datagram::MouseMove(mv)) => {
                let _ = tx.send(FromServer::Move(mv));
            }
            Err(e) => debug!("ungültiges Datagramm: {e}"),
        }
    }
}

/// Antwort auf die Drag-Nachfrage des Servers: Werden hier Dateien gezogen, gehen sie zum Server.
async fn offer_dragged_files(conn: Connection, probe: u32, dragged: oneshot::Receiver<Vec<PathBuf>>) {
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
    if let Err(e) = files::send_files(conn, StreamKind::File, transfer, entries).await {
        warn!("Übertragung zum Server abgebrochen: {e:#}");
    }
}

async fn receive_streams(
    conn: Connection,
    tx: mpsc::UnboundedSender<FromServer>,
    drag: Arc<Staging>,
    clipboard: Arc<Staging>,
) {
    while let Ok(mut stream) = conn.accept_uni().await {
        let (tx, drag, clipboard) = (tx.clone(), drag.clone(), clipboard.clone());
        tokio::spawn(async move {
            if let Err(e) = read_server_stream(&mut stream, &tx, &drag, &clipboard).await {
                debug!("Stream vom Server beendet: {e:#}");
            }
        });
    }
}

async fn read_server_stream(
    stream: &mut RecvStream,
    tx: &mpsc::UnboundedSender<FromServer>,
    drag: &Staging,
    clipboard: &Staging,
) -> Result<()> {
    match read_kind(stream).await? {
        StreamKind::Input => {
            while let Some(input) = read_frame::<_, Input>(stream).await? {
                let _ = tx.send(FromServer::Input(input));
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
            let outcome = files::receive_file(stream, drag).await?;
            let _ = tx.send(FromServer::File(outcome));
        }
        StreamKind::ClipboardFile => {
            let outcome = files::receive_file(stream, clipboard).await?;
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
