//! Aufwachen aus dem Ruhezustand und Netzwechsel, nachgestellt über einen UDP-Vermittler zwischen
//! Client und Server: Funkloch, Ruhezustand, neue Adresse, Neuanmeldung mit gleichem Namen.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qross_core::{CaptureEvent, Layout, LinkConfig};
use qross_net::client::{self, ClientConfig, ClientNotice, ClientPlatform};
use qross_net::progress::Transfers;
use qross_net::server::{Server, ServerConfig, ServerNotice, ServerPlatform};
use qross_net::wake::SystemEvent;
use qross_net::{Identity, TrustStore};
use qross_platform::dryrun::{RecordingCapture, RecordingInjector, SharedClipboard};
use qross_proto::{Edge, GameState, Rect};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinHandle;

/// Alles kommt durch
const PASS: u8 = 0;
/// Nichts kommt durch (Funkloch, Ruhezustand)
const DROP_ALL: u8 = 1;
/// Nur der Server kommt noch durch (siehe `server_forgets_client`)
const DROP_UP: u8 = 2;

/// UDP-Vermittler vor dem Server. Der Client verbindet sich mit `addr`; je Absender des Clients
/// gibt es ein eigenes Socket zum Server, so wie hinter einem NAT.
struct Proxy {
    addr: SocketAddr,
    shared: Shared,
    task: JoinHandle<()>,
}

#[derive(Clone, Default)]
struct Shared {
    mode: Arc<AtomicU8>,
    generation: Arc<AtomicU64>,
    /// Absender, die es nicht mehr gibt (beendeter Client)
    gone: Arc<Mutex<HashSet<SocketAddr>>>,
    /// Bisherige Absender des Clients
    seen: Arc<Mutex<HashSet<SocketAddr>>>,
    /// Wann Initial-Pakete (Verbindungsversuche) vom Client kamen, auch wenn sie nicht durchkamen
    initials: Arc<Mutex<Vec<Instant>>>,
}

/// QUIC-Paket mit langem Kopf vom Typ Initial (RFC 9000, 17.2)
fn is_initial(packet: &[u8]) -> bool {
    packet.first().is_some_and(|b| b & 0x80 != 0 && b & 0x30 == 0)
}

impl Proxy {
    async fn start(server: SocketAddr) -> Self {
        let front = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = front.local_addr().unwrap();
        let shared = Shared::default();
        let task = tokio::spawn(relay(front, server, shared.clone()));
        Self { addr, shared, task }
    }

    fn set(&self, mode: u8) {
        self.shared.mode.store(mode, Ordering::SeqCst);
    }

    /// Der Client erscheint beim Server unter einer neuen Adresse (neues WLAN, NAT setzt um).
    fn new_address(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Alle bisherigen Absender verschwinden spurlos, so wie ein abgestürzter Client: Nichts von
    /// ihnen kommt mehr an, nichts erreicht sie mehr.
    fn forget_clients(&self) {
        let seen = self.shared.seen.lock().unwrap().clone();
        self.shared.gone.lock().unwrap().extend(seen);
    }

    /// Zeitpunkte der Verbindungsversuche (Initial-Pakete) ab `since`
    fn initials_since(&self, since: Instant) -> Vec<Instant> {
        self.shared.initials.lock().unwrap().iter().copied().filter(|t| *t >= since).collect()
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Unter Windows meldet `recv` nach einem ICMP „Port nicht erreichbar“ einen Fehler; weiter geht es trotzdem.
fn fatal(e: &std::io::Error) -> bool {
    e.kind() != std::io::ErrorKind::ConnectionReset
}

async fn relay(front: Arc<UdpSocket>, server: SocketAddr, shared: Shared) {
    let mut upstream: HashMap<SocketAddr, (u64, Arc<UdpSocket>, JoinHandle<()>)> = HashMap::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let (n, from) = match front.recv_from(&mut buf).await {
            Ok(received) => received,
            Err(e) if fatal(&e) => return,
            Err(_) => continue,
        };
        if is_initial(&buf[..n]) {
            shared.initials.lock().unwrap().push(Instant::now());
        }
        if shared.mode.load(Ordering::SeqCst) != PASS || shared.gone.lock().unwrap().contains(&from) {
            continue;
        }
        shared.seen.lock().unwrap().insert(from);
        let current = shared.generation.load(Ordering::SeqCst);
        if upstream.get(&from).is_none_or(|(g, ..)| *g != current) {
            let up = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
            up.connect(server).await.unwrap();
            let back = tokio::spawn(relay_back(up.clone(), front.clone(), from, shared.clone()));
            // Die alte Adresse gibt es nicht mehr: Antworten dorthin gehen verloren.
            if let Some((_, _, old)) = upstream.insert(from, (current, up, back)) {
                old.abort();
            }
        }
        let _ = upstream[&from].1.send(&buf[..n]).await;
    }
}

async fn relay_back(up: Arc<UdpSocket>, front: Arc<UdpSocket>, client: SocketAddr, shared: Shared) {
    let mut buf = vec![0u8; 65536];
    loop {
        let n = match up.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) if fatal(&e) => return,
            Err(_) => continue,
        };
        if shared.mode.load(Ordering::SeqCst) != DROP_ALL && !shared.gone.lock().unwrap().contains(&client) {
            let _ = front.send_to(&buf[..n], client).await;
        }
    }
}

type Log<N> = Arc<Mutex<Vec<(Instant, N)>>>;

/// Sammelt alle Meldungen eines Kanals mit Zeitpunkt.
fn collect<N: Send + 'static>(mut rx: mpsc::UnboundedReceiver<N>) -> Log<N> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let collected = log.clone();
    tokio::spawn(async move {
        while let Some(notice) = rx.recv().await {
            collected.lock().unwrap().push((Instant::now(), notice));
        }
    });
    log
}

/// Ein Client „laptop“ hinter dem Vermittler
struct Laptop {
    injected: RecordingInjector,
    notices: Log<ClientNotice>,
    /// Ruhezustand, Aufwachen, Netzwechsel des Laptops
    system: UnboundedSender<SystemEvent>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Drop for Laptop {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Laptop {
    fn connected(&self) -> usize {
        self.notices.lock().unwrap().iter().filter(|(_, n)| matches!(n, ClientNotice::Connected { .. })).count()
    }

    fn disconnected(&self) -> Vec<String> {
        let notices = self.notices.lock().unwrap();
        notices.iter().filter_map(|(_, n)| if let ClientNotice::Disconnected { reason } = n { Some(reason.clone()) } else { None }).collect()
    }

    fn send(&self, event: SystemEvent) {
        self.system.send(event).unwrap();
    }
}

struct Rig {
    events: UnboundedSender<CaptureEvent>,
    server_notices: Log<ServerNotice>,
    /// Ruhezustand, Aufwachen, Netzwechsel des Servers
    server_system: UnboundedSender<SystemEvent>,
    proxy: Proxy,
    /// Hier liegt die Identität des Clients, damit ein zweiter Start dieselbe hat.
    client_dir: PathBuf,
    server_fingerprint: qross_net::Fingerprint,
    server_task: JoinHandle<anyhow::Result<()>>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server_task.abort();
        let _ = std::fs::remove_dir_all(&self.client_dir);
    }
}

async fn rig(test: &str) -> Rig {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let client_dir = std::env::temp_dir().join(format!("qross-resume-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&client_dir);
    let client_id = Identity::load_or_create(&client_dir).unwrap();
    let server_id = Identity::generate().unwrap();
    let mut trust = TrustStore::in_memory();
    trust.trust("laptop", &client_id.fingerprint()).unwrap();
    let layout =
        Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }]).unwrap();
    let config = ServerConfig { name: "pc".into(), listen: "127.0.0.1:0".parse().unwrap(), layout };
    let (notice_tx, notice_rx) = mpsc::unbounded_channel();
    let (server_system, system_rx) = mpsc::unbounded_channel();
    let server = Server::bind(config, &server_id, trust)
        .unwrap()
        .with_notices(notice_tx)
        .with_system_events(system_rx);
    let proxy = Proxy::start(server.local_addr().unwrap()).await;
    let (events, events_rx) = mpsc::unbounded_channel();
    let server_task = tokio::spawn(server.run(ServerPlatform {
        capture: Arc::new(RecordingCapture::default()),
        events: events_rx,
        clipboard: Box::new(SharedClipboard::default()),
        screen: Box::new(|| Rect::new(0, 0, 1920, 1080)),
        monitors: Box::new(Vec::new),
        game: Box::new(GameState::default),
    }));
    Rig {
        events,
        server_notices: collect(notice_rx),
        server_system,
        proxy,
        client_dir,
        server_fingerprint: server_id.fingerprint(),
        server_task,
    }
}

impl Rig {
    fn laptop(&self) -> Laptop {
        let injected = RecordingInjector::default();
        let config = ClientConfig {
            name: "laptop".into(),
            server: self.proxy.addr.to_string(),
            server_fingerprint: Some(self.server_fingerprint),
            clipboard_file_limit: 0,
            share_concealed: false,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let (system, system_rx) = mpsc::unbounded_channel();
        let platform = ClientPlatform {
            injector: Box::new(injected.clone()),
            clipboard: Box::new(SharedClipboard::default()),
            screen: Box::new(|| Rect::new(0, 0, 1280, 800)),
            monitors: Box::new(Vec::new),
            game: Box::new(GameState::default),
        };
        let identity = Identity::load_or_create(&self.client_dir).unwrap();
        let task = tokio::spawn(client::run_with_system_events(
            config,
            identity,
            TrustStore::in_memory(),
            platform,
            tx,
            Transfers::new(),
            system_rx,
        ));
        Laptop { injected, notices: collect(rx), system, task }
    }

    fn server_saw(&self, what: impl Fn(&ServerNotice) -> bool) -> Option<Instant> {
        self.server_notices.lock().unwrap().iter().rev().find(|(_, n)| what(n)).map(|(t, _)| *t)
    }

    fn disconnects(&self) -> usize {
        let notices = self.server_notices.lock().unwrap();
        notices.iter().filter(|(_, n)| matches!(n, ServerNotice::ClientDisconnected { .. })).count()
    }

    /// Eingaben wie ein Nutzer, der den Laptop benutzen will: gegen die rechte Kante schieben
    /// (wechselt, sobald er verbunden ist) und die Maus bewegen (sobald der Cursor drüben ist).
    /// Liefert, wann beim Laptop die erste neue Eingabe ankam.
    async fn first_input(&self, laptop: &Laptop, limit: Duration) -> Option<Instant> {
        let before = laptop.injected.0.lock().unwrap().len();
        let started = Instant::now();
        while started.elapsed() < limit {
            let _ = self.events.send(CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false });
            let _ = self.events.send(CaptureEvent::MouseDelta { dx: 1, dy: 0 });
            tokio::time::sleep(Duration::from_millis(10)).await;
            if laptop.injected.0.lock().unwrap().len() > before {
                return Some(Instant::now());
            }
        }
        None
    }

    /// Verbunden und der Cursor ist drüben
    async fn ready(&self, laptop: &Laptop) {
        self.first_input(laptop, Duration::from_secs(10)).await.expect("Laptop verbindet sich");
    }

    /// Wartet, bis der Server einen Client mehr aufgegeben hat als `before`, und liefert den Zeitpunkt.
    async fn server_drops(&self, before: usize, limit: Duration) -> Instant {
        until("Server gibt den Client auf", limit, || self.disconnects() > before).await;
        self.server_saw(|n| matches!(n, ServerNotice::ClientDisconnected { .. })).unwrap()
    }
}

async fn until(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < limit, "Zeitüberschreitung beim Warten auf: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn report(scenario: &str, since: Instant, at: Option<Instant>) -> Duration {
    let took = at.map(|at| at.saturating_duration_since(since));
    match took {
        Some(d) => eprintln!("MESSUNG {scenario}: {} ms", d.as_millis()),
        None => eprintln!("MESSUNG {scenario}: keine Eingabe angekommen"),
    }
    took.unwrap_or(Duration::MAX)
}

/// Wie nach dem Ruhezustand eines MacBooks: Der Server hat die Verbindung längst aufgegeben, der
/// Client hält sie noch für gesund (seine Uhr stand still). Nachgestellt, indem nur der Client
/// nicht mehr durchkommt: Der Server gibt nach seiner Zeitüberschreitung auf, der Client hört bis
/// dahin noch die Keep-alives des Servers und merkt nichts.
async fn server_forgets_client(rig: &Rig) -> Instant {
    let before = rig.disconnects();
    rig.proxy.set(DROP_UP);
    rig.server_drops(before, Duration::from_secs(15)).await;
    Instant::now()
}

/// MacBook wacht auf, das Netz ist erst nach `network_after` wieder da. Wie das System meldet der
/// Test das Aufwachen sofort und den Netzwechsel, sobald das Netz da ist.
async fn wake_after_sleep(network_after: Duration) -> Duration {
    let rig = rig(&format!("wake{}", network_after.as_millis())).await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    let woke = server_forgets_client(&rig).await;
    if network_after.is_zero() {
        rig.proxy.set(PASS);
        laptop.send(SystemEvent::Woke);
    } else {
        rig.proxy.set(DROP_ALL);
        laptop.send(SystemEvent::Woke);
        tokio::time::sleep(network_after).await;
        rig.proxy.set(PASS);
        laptop.send(SystemEvent::NetworkChanged);
    }
    let at = rig.first_input(&laptop, Duration::from_secs(20)).await;
    report(&format!("Aufwachen, Netz nach {} s", network_after.as_secs_f32()), woke, at)
}

#[tokio::test]
async fn wake_with_network_at_once() {
    let took = wake_after_sleep(Duration::ZERO).await;
    // Vorher bis zum nächsten Keep-alive, also bis gut 1 s
    assert!(took < Duration::from_millis(500), "{took:?}");
}

#[tokio::test]
async fn wake_with_network_after_two_seconds() {
    let took = wake_after_sleep(Duration::from_secs(2)).await;
    assert!(took < Duration::from_millis(2600), "{took:?}");
}

#[tokio::test]
async fn wake_with_network_after_five_seconds() {
    let took = wake_after_sleep(Duration::from_secs(5)).await;
    assert!(took < Duration::from_millis(5600), "{took:?}");
}

/// Kurzes Funkloch (WLAN hakt, Kabel kurz ab), kürzer als die Zeitüberschreitung: Die Verbindung
/// bleibt, danach kommen sofort wieder Eingaben an.
#[tokio::test]
async fn short_outage_keeps_the_connection() {
    let rig = rig("outage").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    rig.proxy.set(DROP_ALL);
    tokio::time::sleep(Duration::from_secs(3)).await;
    rig.proxy.set(PASS);
    let back = Instant::now();
    let at = rig.first_input(&laptop, Duration::from_secs(20)).await;
    let took = report("Funkloch 3 s", back, at);
    assert!(took < Duration::from_millis(1500), "{took:?}");
    assert_eq!(laptop.connected(), 1, "kein neuer Verbindungsaufbau");
}

/// Anderes WLAN, Kabel statt WLAN, VPN an: Der Laptop meldet sich mit neuer Adresse. QUIC zieht mit
/// der Verbindung um, ein Neuaufbau ist nicht nötig – auch nicht, wenn der Laptop dafür auf ein
/// neues Socket wechselt.
#[tokio::test]
async fn a_new_address_keeps_the_connection() {
    let rig = rig("address").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    rig.proxy.set(DROP_ALL);
    rig.proxy.new_address();
    tokio::time::sleep(Duration::from_secs(1)).await;
    rig.proxy.set(PASS);
    laptop.send(SystemEvent::NetworkChanged);
    let back = Instant::now();
    let at = rig.first_input(&laptop, Duration::from_secs(20)).await;
    let took = report("neue Adresse nach 1 s Funkloch", back, at);
    assert!(took < Duration::from_millis(1000), "{took:?}");
    // Der Laptop wechselt noch einmal das Socket, ohne dass sich sonst etwas ändert.
    laptop.send(SystemEvent::NetworkChanged);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let at = rig.first_input(&laptop, Duration::from_secs(5)).await;
    assert!(at.is_some(), "Eingaben nach dem Socket-Wechsel");
    assert_eq!(laptop.connected(), 1, "kein neuer Verbindungsaufbau");
    assert_eq!(rig.disconnects(), 0);
}

/// Der Client startet neu (oder baut nach dem Aufwachen sofort neu auf), während der Server die
/// alte Verbindung noch hält: Die neue ersetzt sie sofort.
#[tokio::test]
async fn the_same_client_reconnecting_replaces_its_old_connection() {
    let rig = rig("samename").await;
    let first = rig.laptop();
    rig.ready(&first).await;
    // Die alte Verbindung verschwindet spurlos (abgestürzt, oder der Client hat sie selbst aufgegeben).
    rig.proxy.forget_clients();
    drop(first);
    let started = Instant::now();
    let second = rig.laptop();
    let at = rig.first_input(&second, Duration::from_secs(20)).await;
    let took = report("Neuanmeldung mit gleichem Namen", started, at);
    // Vorher: abgewiesen („bereits verbunden“), bis die alte Verbindung nach 5 s auslief
    assert!(took < Duration::from_millis(1000), "{took:?}");
    assert_eq!(rig.disconnects(), 1, "die alte Verbindung ist getrennt");
}

/// Laufen versehentlich zwei Clients mit demselben Namen und Schlüssel, gewinnt der neuere; der
/// ältere hört auf, statt sich ständig gegenseitig hinauszuwerfen.
#[tokio::test]
async fn a_second_live_client_with_the_same_identity_wins() {
    let rig = rig("twice").await;
    let mut first = rig.laptop();
    rig.ready(&first).await;
    let second = rig.laptop();
    rig.ready(&second).await;
    let ended = tokio::time::timeout(Duration::from_secs(5), &mut first.task).await;
    let error = ended.expect("der ältere hört auf").unwrap().unwrap_err();
    assert!(format!("{error:#}").contains("anderes Qross"), "{error:#}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(second.connected(), 1, "der neuere bleibt verbunden");
    assert!(!second.task.is_finished());
}

/// Der Laptop geht in den Ruhezustand: Er meldet sich vorher ab, der Server holt den Cursor sofort
/// zurück. Bis zum Aufwachen versucht er nicht, sich neu zu verbinden.
#[tokio::test]
async fn a_sleeping_client_signs_off_and_returns_on_waking() {
    let rig = rig("sleep").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    let before = rig.disconnects();
    let sleeping = Instant::now();
    laptop.send(SystemEvent::Sleeping);
    let dropped = rig.server_drops(before, Duration::from_secs(3)).await;
    let took = report("Ruhezustand des Clients: Server gibt Cursor zurück", sleeping, Some(dropped));
    // Vorher erst nach der Zeitüberschreitung (5 s)
    assert!(took < Duration::from_millis(500), "{took:?}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(laptop.connected(), 1, "im Ruhezustand keine neue Verbindung");
    assert!(laptop.disconnected().iter().any(|r| r.contains("Ruhezustand")), "{:?}", laptop.disconnected());

    let woke = Instant::now();
    laptop.send(SystemEvent::Woke);
    let at = rig.first_input(&laptop, Duration::from_secs(5)).await;
    let took = report("nach dem Aufwachen wieder da", woke, at);
    assert!(took < Duration::from_millis(500), "{took:?}");
}

/// Der Client verschwindet ohne Abschied (Akku leer, Kabel ab): So lange hält der Server ihn noch.
/// Hier hilft nur die Zeitüberschreitung.
#[tokio::test]
async fn a_vanished_client_is_dropped_after_the_idle_timeout() {
    let rig = rig("vanished").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    let before = rig.disconnects();
    rig.proxy.set(DROP_ALL);
    let gone = Instant::now();
    let dropped = rig.server_drops(before, Duration::from_secs(15)).await;
    let took = report("Server hält verschwundenen Client", gone, Some(dropped));
    assert!(took < Duration::from_millis(6000), "{took:?}");
}

/// Der Server wacht auf und seine Clients sind inzwischen weg: Er wirft sie hinaus, statt ihnen bis
/// zur Zeitüberschreitung Eingaben zu schicken.
#[tokio::test]
async fn a_waking_server_drops_silent_clients() {
    let rig = rig("serverwake").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    let before = rig.disconnects();
    rig.proxy.set(DROP_ALL);
    let woke = Instant::now();
    rig.server_system.send(SystemEvent::Woke).unwrap();
    let dropped = rig.server_drops(before, Duration::from_secs(10)).await;
    let took = report("Server wacht auf: stummer Client getrennt", woke, Some(dropped));
    assert!(took < Duration::from_millis(3500), "{took:?}");
}

/// Wacht der Server auf und der Client ist noch da, bleibt die Verbindung.
#[tokio::test]
async fn a_waking_server_keeps_live_clients() {
    let rig = rig("serverkeeps").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    rig.server_system.send(SystemEvent::Woke).unwrap();
    rig.server_system.send(SystemEvent::NetworkChanged).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(rig.disconnects(), 0);
    assert_eq!(laptop.connected(), 1);
}

/// Der Server geht in den Ruhezustand: Der Client erfährt es sofort und ist nach dem Aufwachen
/// des Servers schnell wieder da.
#[tokio::test]
async fn a_sleeping_server_signs_off_its_clients() {
    let rig = rig("serversleep").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    let sleeping = Instant::now();
    rig.server_system.send(SystemEvent::Sleeping).unwrap();
    until("Client merkt den Ruhezustand des Servers", Duration::from_secs(3), || !laptop.disconnected().is_empty()).await;
    // Jetzt schläft er wirklich.
    rig.proxy.set(DROP_ALL);
    let at = laptop.notices.lock().unwrap().iter().find(|(_, n)| matches!(n, ClientNotice::Disconnected { .. })).map(|(t, _)| *t);
    let took = report("Ruhezustand des Servers: Client weiß Bescheid", sleeping, at);
    assert!(took < Duration::from_millis(500), "{took:?}");
    assert!(laptop.disconnected()[0].contains("Ruhezustand"), "{:?}", laptop.disconnected());

    tokio::time::sleep(Duration::from_secs(10)).await;
    rig.proxy.set(PASS);
    let back = Instant::now();
    let at = rig.first_input(&laptop, Duration::from_secs(10)).await;
    let took = report("Server nach 10 s Ruhezustand wieder da", back, at);
    assert!(took < Duration::from_millis(2500), "{took:?}");
}

/// Beide Seiten geben auf (Ruhezustand des Servers ohne Abmeldung, Windows-Client nach dem
/// Aufwachen, langes Funkloch); danach ist der Server wieder erreichbar.
async fn long_outage(secs: f32) -> Duration {
    let rig = rig(&format!("long{}", (secs * 10.0) as u32)).await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    rig.proxy.set(DROP_ALL);
    tokio::time::sleep(Duration::from_secs_f32(secs)).await;
    rig.proxy.set(PASS);
    let back = Instant::now();
    let at = rig.first_input(&laptop, Duration::from_secs(20)).await;
    report(&format!("langes Funkloch {secs} s"), back, at)
}

#[tokio::test]
async fn long_outage_7s() {
    let took = long_outage(7.0).await;
    assert!(took < Duration::from_millis(2500), "{took:?}");
}

#[tokio::test]
async fn long_outage_9s() {
    let took = long_outage(9.0).await;
    assert!(took < Duration::from_millis(2500), "{took:?}");
}

#[tokio::test]
async fn long_outage_11s() {
    let took = long_outage(11.5).await;
    assert!(took < Duration::from_millis(2500), "{took:?}");
}

/// Server nicht erreichbar (schläft, anderes Netz): So oft versucht es der Client. Die größte
/// Lücke ist die längste Wartezeit, nachdem der Server wieder da ist.
#[tokio::test]
async fn attempts_while_unreachable() {
    let rig = rig("unreachable").await;
    let laptop = rig.laptop();
    rig.ready(&laptop).await;
    rig.proxy.set(DROP_ALL);
    let since = Instant::now();
    tokio::time::sleep(Duration::from_secs(25)).await;
    let initials = rig.proxy.initials_since(since);
    let gap = initials.windows(2).map(|w| w[1] - w[0]).max().unwrap_or_default();
    let span = initials.last().zip(initials.first()).map(|(l, f)| *l - *f).unwrap_or_default();
    eprintln!(
        "MESSUNG Server unerreichbar: {} Initial-Pakete in {} ms, größte Lücke {} ms",
        initials.len(),
        span.as_millis(),
        gap.as_millis()
    );
    // Vorher bis zu 4 s
    assert!(gap < Duration::from_millis(2500), "{gap:?}");
    drop(laptop);
}
