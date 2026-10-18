//! Server-Laufzeit: nimmt Clients an, verbindet Eingabe-Erfassung, `ServerState` und Netzwerk.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use qross_core::server::{Action, ServerEvent, ServerState, SwitchOptions};
use qross_core::{CaptureEvent, Layout};
use qross_platform::{Capture, Clipboard};
use qross_proto::{
    encode_datagram, read_frame, write_frame, Clipboard as ClipboardMsg, ClipboardData, Control, Datagram, FileEntry,
    Input, Platform, Rect, StreamKind, PROTOCOL_VERSION,
};
use quinn::{Connection, Endpoint, Incoming};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::clipboard::ClipboardHub;
use crate::clipfiles::{self, FilesNotice, Notify, Ready};
use crate::files::{FileOutcome, Staging};
use crate::quality::{self, Quality};
use crate::{discovery, files};
use crate::streams::{open_input, read_kind};
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
    fingerprint: Fingerprint,
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
    /// Verbindungsqualität zu einem Client, etwa jede Sekunde (siehe [`quality`]).
    Quality { name: String, quality: Quality },
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
        reply: oneshot::Sender<Result<String, String>>,
    },
    ScreenChanged { name: String, id: usize, screen: Rect },
    Clipboard { name: String, id: usize, data: ClipboardData },
    ClipboardFiles { name: String, id: usize, transfer: u64, entries: Vec<FileEntry>, data: ClipboardData },
    ClipboardFile { name: String, id: usize, outcome: FileOutcome },
    Closed { name: String, id: usize },
}

struct ClientHandle {
    /// Unterscheidet eine neue Verbindung vom selben Client von der alten.
    id: usize,
    conn: Connection,
    input: mpsc::UnboundedSender<Input>,
    /// Misst die Verbindungsqualität, solange der Client verbunden ist (nur mit Oberfläche)
    _quality: Option<quality::Monitor>,
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
            fingerprint: identity.fingerprint(),
        })
    }

    /// Kopierte Dateien bis zu dieser Gesamtgröße in Bytes mitgeben (0 = nie;
    /// Standard: `clipfiles::DEFAULT_LIMIT`). Größeres bleibt Drag & Drop vorbehalten.
    pub fn with_clipboard_file_limit(mut self, bytes: u64) -> Self {
        self.clipboard_file_limit = bytes;
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
            _ => {}
        }
    }

    pub async fn run(mut self, platform: ServerPlatform) -> Result<()> {
        // Kopierte Dateien aller Clients landen hier, bis sie in die Zwischenablage kommen.
        let mut incoming = clipfiles::Incoming::new(Staging::persistent(&clipfiles::default_store())?);
        let (net_tx, mut net_rx) = mpsc::unbounded_channel();
        tokio::spawn(accept_loop(self.endpoint.clone(), net_tx, incoming.staging()));

        let ServerPlatform { capture, mut events, clipboard, screen } = platform;
        let clipboard = ClipboardHub::spawn(clipboard);
        let files_notify: Notify = {
            let notices = self.notices.clone();
            Arc::new(move |notice| {
                if let Some(tx) = &notices {
                    let _ = tx.send(ServerNotice::ClipboardFiles(notice));
                }
            })
        };
        let mut local_screen = screen();
        let mut state = ServerState::new(self.config.name.clone(), self.config.layout.clone(), local_screen);
        state.set_switch_options(self.switching);
        let _discovery = self.discovery.then(|| {
            let port = self.local_addr().map(|a| a.port()).unwrap_or_default();
            AbortOnDrop(tokio::spawn(discovery::respond(self.config.name.clone(), port, self.fingerprint.to_string())))
        });
        let mut clients: HashMap<String, ClientHandle> = HashMap::new();
        // Clipboard-Sequenznummer, die ein Client zuletzt bekommen hat
        let clip_synced: ClipSynced = Arc::new(Mutex::new(HashMap::new()));
        let mut screen_poll = tokio::time::interval(Duration::from_secs(2));
        info!("Server „{}“ lauscht auf {}", self.config.name, self.local_addr()?);
        self.notify(ServerNotice::Listening { addr: self.local_addr()? });

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
                                clients.insert(name.clone(), ClientHandle { id: conn.stable_id(), conn, input, _quality: quality });
                                clip_synced.lock().unwrap().remove(&name);
                                let _ = reply.send(Ok(self.config.name.clone()));
                                info!("Client „{name}“ verbunden ({}×{})", screen.width, screen.height);
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
                        state.handle(ServerEvent::ClientScreenChanged { name, screen })
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
                    NetEvent::Closed { name, id } if is_current(&clients, &name, id) => {
                        clients.remove(&name);
                        info!("Client „{name}“ getrennt");
                        self.notify(ServerNotice::ClientDisconnected { name: name.clone() });
                        state.handle(ServerEvent::ClientDisconnected { name })
                    }
                    _ => Vec::new(),
                },
                _ = screen_poll.tick() => {
                    let now = screen();
                    if now == local_screen {
                        Vec::new()
                    } else {
                        local_screen = now;
                        info!("lokaler Bildschirm geändert: {}×{}", now.width, now.height);
                        state.handle(ServerEvent::LocalScreenChanged(now))
                    }
                }
            };
            for action in actions {
                self.notify_action(&action);
                let files = (self.clipboard_file_limit, &files_notify);
                apply(action, &*capture, &clipboard, &clients, &clip_synced, files);
            }
        }
    }

    fn on_capture(&self, state: &mut ServerState, event: CaptureEvent) -> Vec<Action> {
        let toggle = event == CaptureEvent::ToggleLock;
        let actions = state.handle(ServerEvent::Capture(event));
        if toggle {
            if state.is_locked() {
                info!("Cursor auf „{}“ festgehalten (Rollen-Taste löst)", state.active_screen());
            } else {
                info!("Cursor wieder frei");
            }
            self.notify(ServerNotice::CursorLocked { locked: state.is_locked(), screen: state.active_screen().to_owned() });
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

/// Je Client die zuletzt übertragene Clipboard-Sequenznummer
type ClipSynced = Arc<Mutex<HashMap<String, u64>>>;

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
    (file_limit, files_notify): (u64, &Notify),
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
        Action::Move { to, mv } => {
            if let Some(client) = clients.get(&to) {
                if let Err(e) = client.conn.send_datagram(encode_datagram(&Datagram::MouseMove(mv)).into()) {
                    debug!("Datagramm an „{to}“ nicht gesendet: {e}");
                }
            }
        }
        Action::SyncClipboard { to } => {
            let Some(client) = clients.get(&to) else { return };
            let (clipboard, synced, conn) = (clipboard.clone(), clip_synced.clone(), client.conn.clone());
            let notify = files_notify.clone();
            // Lesen und PNG-Kodierung im Clipboard-Thread; die Ereignisschleife läuft weiter.
            tokio::spawn(async move {
                let known = synced.lock().unwrap().get(&to).copied();
                let (seq, content) = clipboard.changed_since(known).await;
                synced.lock().unwrap().insert(to.clone(), seq);
                if let Some(content) = content {
                    clipfiles::send(conn, content, file_limit, to, notify).await;
                }
            });
        }
        Action::ProbeDrag { x, y } => capture.probe_drag(x, y),
        Action::CancelLocalDrag => capture.cancel_local_drag(),
        Action::StartFileDrag { to, transfer, files } => {
            if let Some(client) = clients.get(&to) {
                tokio::spawn(start_file_drag(client.conn.clone(), client.input.clone(), to, transfer, files));
            }
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
    match files::send_files(conn, StreamKind::File, transfer, entries).await {
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

async fn accept_loop(endpoint: Endpoint, tx: mpsc::UnboundedSender<NetEvent>, staging: Arc<Staging>) {
    while let Some(incoming) = endpoint.accept().await {
        let (tx, staging) = (tx.clone(), staging.clone());
        tokio::spawn(async move {
            let remote = incoming.remote_address();
            if let Err(e) = handle_connection(incoming, tx, staging).await {
                debug!("Verbindung von {remote} beendet: {e:#}");
            }
        });
    }
}

async fn handle_connection(
    incoming: Incoming,
    tx: mpsc::UnboundedSender<NetEvent>,
    staging: Arc<Staging>,
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
    match verdict {
        Ok(server_name) => {
            debug!("„{name}“ läuft unter {platform:?}");
            let welcome = Control::Welcome { version: PROTOCOL_VERSION, server_name, platform: Platform::current() };
            write_frame(&mut send, &welcome).await?;
        }
        Err(reason) => {
            write_frame(&mut send, &Control::Reject { reason }).await?;
            let _ = send.finish();
            // Dem Client Zeit geben, die Ablehnung zu lesen, bevor die Verbindung zugeht.
            let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
            return Ok(());
        }
    }

    let id = conn.stable_id();
    tokio::spawn(receive_client_streams(conn.clone(), name.clone(), id, tx.clone(), staging));
    loop {
        match read_frame::<_, Control>(&mut recv).await {
            Ok(Some(Control::ScreenChanged(info))) => {
                let _ = tx.send(NetEvent::ScreenChanged { name: name.clone(), id, screen: info.bounds });
            }
            Ok(Some(other)) => debug!(?other, "unerwartete Control-Nachricht"),
            Ok(None) | Err(_) => break,
        }
    }
    conn.close(0u32.into(), b"");
    let _ = tx.send(NetEvent::Closed { name, id });
    Ok(())
}

/// Streams vom Client: nur Zwischenablage (Inhalte und kopierte Dateien).
async fn receive_client_streams(
    conn: Connection,
    name: String,
    id: usize,
    tx: mpsc::UnboundedSender<NetEvent>,
    staging: Arc<Staging>,
) {
    while let Ok(mut stream) = conn.accept_uni().await {
        let (name, tx, staging) = (name.clone(), tx.clone(), staging.clone());
        tokio::spawn(async move {
            let result: Result<()> = async {
                match read_kind(&mut stream).await? {
                    StreamKind::Clipboard => match read_frame(&mut stream).await? {
                        Some(ClipboardMsg::Data(data)) => {
                            let _ = tx.send(NetEvent::Clipboard { name, id, data });
                        }
                        Some(ClipboardMsg::Files { transfer, entries, data }) => {
                            let _ = tx.send(NetEvent::ClipboardFiles { name, id, transfer, entries, data });
                        }
                        None => {}
                    },
                    StreamKind::ClipboardFile => {
                        let outcome = files::receive_file(&mut stream, &staging).await?;
                        let _ = tx.send(NetEvent::ClipboardFile { name, id, outcome });
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
