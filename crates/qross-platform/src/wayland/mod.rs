//! Linux mit Wayland: Eingaben über die Desktop-Portale (xdg-desktop-portal) und libei.
//!
//! - Einspeisen (Client): Portal `RemoteDesktop` mit `ConnectToEIS`, danach libei als Sender.
//! - Erfassen (Server): Portal `InputCapture` mit Zeigerbarrieren an den äußeren Monitorkanten,
//!   danach libei als Empfänger.
//! - Geometrie: die libei-Regionen (Client) bzw. die Zonen des Portals (Server), beide in logischen
//!   Pixeln des Compositors. So passen Einspeisen, Erfassen und gemeldete Fläche zusammen.
//!
//! Die einmalige Freigabe fragt das Portal selbst ab. Wo es das kann, merkt sich Qross das
//! Restore-Token im Datenverzeichnis, damit beim nächsten Start nicht wieder gefragt wird.
//! Hintergrund und Stand der Compositoren: `docs/wayland.md`.

mod capture;
mod inject;
mod tokens;

use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use ashpd::desktop::input_capture::InputCapture;
use ashpd::desktop::remote_desktop::RemoteDesktop;
use ashpd::desktop::ResponseError;
use qross_proto::Rect;
use tokio::runtime::{Builder, Handle, Runtime, RuntimeFlavor};

pub(crate) use capture::start as start_capture;
pub(crate) use inject::{from_fd as injector_from_fd, start as injector};
pub(crate) use tokens::{forget as forget_tokens, set_data_dir};

use crate::{PortalProblem, PortalState};

/// Kennung, unter der sich Qross beim Portal anmeldet (wie in `tauri.conf.json`)
const APP_ID: &str = "app.qross.desktop";
/// So lange wartet der Start auf die Antwort im Freigabe-Dialog
const PROMPT_TIMEOUT: Duration = Duration::from_secs(180);

/// Eigene Laufzeit für alle Portal- und libei-Verbindungen. Sie lebt bis zum Prozessende: ashpd hält
/// eine gemeinsame D-Bus-Verbindung, deren Hintergrundaufgaben in der Laufzeit laufen, in der sie
/// entstand. Mit einer kurzlebigen Laufzeit würde jeder spätere Portal-Aufruf hängen bleiben.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("qross-wayland")
            .enable_all()
            .build()
            .expect("failed to build the runtime for the Wayland portals")
    })
}

/// Wartet blockierend höchstens `timeout` auf `future`, das in der Portal-Laufzeit läuft. Darf aus
/// einer anderen Tokio-Laufzeit heraus aufgerufen werden; läuft es zu lange, wird es abgebrochen.
pub(crate) fn block_on<T: Send + 'static>(timeout: Duration, future: impl Future<Output = T> + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let task = runtime().spawn(async move {
        let _ = tx.send(future.await);
    });
    let result = wait_blocking(|| rx.recv_timeout(timeout).ok());
    if result.is_none() {
        task.abort();
    }
    result
}

/// Blockierend warten, ohne einen Arbeits-Thread der aufrufenden Tokio-Laufzeit lahmzulegen
fn wait_blocking<T>(wait: impl FnOnce() -> T) -> T {
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => tokio::task::block_in_place(wait),
        _ => wait(),
    }
}

/// Meldet dem Start einmal, ob die Sitzung steht
pub(crate) struct Ready(Option<std::sync::mpsc::Sender<Result<(), Fail>>>);

impl Ready {
    fn waiting(&self) -> bool {
        self.0.is_some()
    }

    fn ok(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(Ok(()));
        }
    }

    /// Fehler an den wartenden Start; `Some` zurück, wenn niemand mehr wartet
    fn fail(&mut self, fail: Fail) -> Option<Fail> {
        match self.0.take() {
            Some(tx) => {
                let _ = tx.send(Err(fail));
                None
            }
            None => Some(fail),
        }
    }
}

/// Startet eine Portal-Sitzung in einem eigenen Thread (die libei-Ströme von reis sind nicht
/// `Send`) und wartet, bis sie steht oder scheitert – höchstens so lange, wie ein Nutzer für den
/// Freigabe-Dialog braucht.
fn spawn_session<Fut>(name: &str, role: Role, session: impl FnOnce(Ready) -> Fut + Send + 'static) -> anyhow::Result<()>
where
    Fut: Future<Output = ()> + 'static,
{
    // Die gemeinsame D-Bus-Verbindung von ashpd in der dauerhaften Laufzeit anlegen, nicht in der
    // des Sitzungs-Threads (die endet mit der Sitzung).
    block_on(Duration::from_secs(5), register_app());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::Builder::new().name(name.into()).spawn(move || {
        let rt = match Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => return drop(ready_tx.send(Err(Fail::Other(e.into())))),
        };
        rt.block_on(async move {
            tokio::select! {
                // Nur ein ausdrücklicher Abbruch zählt; fällt der Sender einfach weg, läuft es weiter.
                Ok(()) = &mut cancel_rx => {}
                () = session(Ready(Some(ready_tx))) => {}
            }
        });
    })?;
    let fail = match wait_blocking(|| ready_rx.recv_timeout(PROMPT_TIMEOUT)) {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(fail)) => fail,
        Err(_) => Fail::Other(anyhow::anyhow!("no answer to the permission prompt")),
    };
    // Sonst wartete der Thread weiter auf den Dialog und verbände sich später ohne Abnehmer
    let _ = cancel_tx.send(());
    set_problem(role, Some(fail.problem()));
    Err(start_error(role, &fail))
}

/// Bei Hostprogrammen (nicht Flatpak) kennt das Portal sonst keine App-Kennung: Der Dialog zeigt
/// dann keinen Namen, und manche Portale merken sich Freigaben nur je Kennung. Ältere Portale
/// (vor xdg-desktop-portal 1.19.1) kennen die Anmeldung nicht – dann eben ohne.
async fn register_app() {
    static DONE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    DONE.get_or_init(|| async {
        let id = match APP_ID.parse() {
            Ok(id) => id,
            Err(e) => return tracing::debug!("invalid app ID {APP_ID}: {e}"),
        };
        match ashpd::register_host_app(id).await {
            Ok(()) => tracing::debug!("registered with the portal as {APP_ID}"),
            Err(e) => tracing::debug!("cannot register with the portal: {e}"),
        }
    })
    .await;
}

// ------------------------------------------------------------------ Portal-Erkennung

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Portals {
    pub remote_desktop: Option<u32>,
    pub input_capture: Option<u32>,
}

/// Welche Portale gibt es, in welcher Version? Gefundene merkt sich der Prozess; fehlende werden
/// höchstens alle 10 s neu gesucht (der Portal-Dienst startet nach der Anmeldung manchmal spät).
pub(crate) fn portals() -> Portals {
    static CACHE: Mutex<Option<(Portals, Instant)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((portals, at)) = *cache {
        let complete = portals.remote_desktop.is_some() && portals.input_capture.is_some();
        if complete || at.elapsed() < Duration::from_secs(10) {
            return portals;
        }
    }
    let probe = async {
        register_app().await;
        let remote_desktop = match RemoteDesktop::new().await {
            Ok(p) => Some(p.version()),
            Err(e) => {
                tracing::debug!("RemoteDesktop portal: {e}");
                None
            }
        };
        let input_capture = match InputCapture::new().await {
            Ok(p) => Some(p.version()),
            Err(e) => {
                tracing::debug!("InputCapture portal: {e}");
                None
            }
        };
        Portals { remote_desktop, input_capture }
    };
    let found = block_on(Duration::from_secs(5), probe).unwrap_or_default();
    *cache = Some((found, Instant::now()));
    found
}

// ------------------------------------------------------------------ Geometrie

static GEOMETRY: Mutex<Option<Vec<Rect>>> = Mutex::new(None);

/// Monitore der laufenden Portal-Sitzung (logische Pixel), `None` ohne Sitzung
pub(crate) fn geometry() -> Option<Vec<Rect>> {
    GEOMETRY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_geometry(monitors: Vec<Rect>) {
    let mut slot = GEOMETRY.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref() != Some(&monitors) {
        tracing::info!("Wayland monitors: {}", describe(&monitors));
        *slot = Some(monitors);
    }
}

fn describe(monitors: &[Rect]) -> String {
    let list: Vec<String> = monitors.iter().map(|m| format!("{}×{} at ({}, {})", m.width, m.height, m.x, m.y)).collect();
    if list.is_empty() {
        "none".into()
    } else {
        list.join(", ")
    }
}

// ------------------------------------------------------------------ Freigaben und Probleme

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Injector,
    Capture,
}

static PROBLEMS: Mutex<[Option<PortalProblem>; 2]> = Mutex::new([None, None]);

fn set_problem(role: Role, problem: Option<PortalProblem>) {
    PROBLEMS.lock().unwrap_or_else(|e| e.into_inner())[role as usize] = problem;
}

/// Stand für die Oberfläche: vorhandene Portale, letzte Probleme, gemerkte Freigaben
pub(crate) fn state() -> PortalState {
    let portals = portals();
    let problems = PROBLEMS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let [injector, capture] = problems;
    PortalState {
        remote_desktop: portals.remote_desktop,
        input_capture: portals.input_capture,
        injector,
        capture,
        remembered_injector: tokens::exists(tokens::Kind::RemoteDesktop),
        remembered_capture: tokens::exists(tokens::Kind::InputCapture),
    }
}

/// Warum eine Portal-Sitzung nicht zustande kam
#[derive(Debug)]
enum Fail {
    /// Portal fehlt oder ist zu alt (Text: was genau)
    Missing(String),
    /// Der Nutzer hat die Freigabe abgelehnt
    Denied,
    Other(anyhow::Error),
}

impl Fail {
    fn problem(&self) -> PortalProblem {
        match self {
            Fail::Missing(what) => PortalProblem::Missing(what.clone()),
            Fail::Denied => PortalProblem::Denied,
            Fail::Other(e) => PortalProblem::Failed(format!("{e:#}")),
        }
    }
}

impl From<ashpd::Error> for Fail {
    fn from(e: ashpd::Error) -> Self {
        match e {
            ashpd::Error::Response(ResponseError::Cancelled) => Fail::Denied,
            ashpd::Error::PortalNotFound(name) => Fail::Missing(format!("The desktop portal does not offer {name}")),
            ashpd::Error::RequiresVersion(need, have) => {
                Fail::Missing(format!("The desktop portal is too old (version {have}, needs {need})"))
            }
            ashpd::Error::Zbus(e) if no_portal_service(&e) => Fail::Missing("xdg-desktop-portal is not running".into()),
            ashpd::Error::Zbus(ashpd::zbus::Error::Address(_)) => {
                Fail::Missing("There is no D-Bus session bus (DBUS_SESSION_BUS_ADDRESS)".into())
            }
            other => Fail::Other(other.into()),
        }
    }
}

/// Fehlt der Dienst des Portals ganz (nicht installiert, nicht startbar)?
fn no_portal_service(e: &ashpd::zbus::Error) -> bool {
    use ashpd::zbus::{fdo, Error};
    match e {
        Error::MethodError(name, _, _) => name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown",
        Error::FDO(e) => matches!(**e, fdo::Error::ServiceUnknown(_)),
        _ => false,
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::Other(e.into())
    }
}

impl From<reis::Error> for Fail {
    fn from(e: reis::Error) -> Self {
        Fail::Other(anyhow::anyhow!("libei: {e}"))
    }
}

/// Fehlermeldung für den Start, mit Hinweis, was zu tun ist
fn start_error(role: Role, fail: &Fail) -> anyhow::Error {
    let what = match role {
        Role::Injector => "Injecting input (client)",
        Role::Capture => "Capturing input (server)",
    };
    match fail {
        Fail::Missing(detail) => anyhow::anyhow!(
            "{what} does not work under Wayland: {detail}. It requires xdg-desktop-portal with {} \
             ({}) or an X11 session; details in docs/wayland.md in the Qross repository.",
            match role {
                Role::Injector => "RemoteDesktop version 2 or later",
                Role::Capture => "InputCapture",
            },
            match role {
                Role::Injector => "GNOME 45 or later, KDE Plasma 6.1 or later",
                Role::Capture => "GNOME 45 or later, KDE Plasma 6.1 or later, Hyprland 0.56 or later",
            }
        ),
        Fail::Denied => anyhow::anyhow!(
            "{what}: the permission was denied. The system will ask again on the next start."
        ),
        Fail::Other(e) => anyhow::anyhow!("{what} via the desktop portal: {e:#}"),
    }
}

/// Mikrosekunden der monotonen Uhr, wie libei sie für Zeitstempel erwartet
fn now_micros() -> u64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    t.tv_sec as u64 * 1_000_000 + t.tv_nsec as u64 / 1_000
}
