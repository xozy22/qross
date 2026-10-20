//! Linux: Bildschirmschoner über den Dienst des Desktops auf dem D-Bus, in einer X11-Sitzung ohne
//! solchen Dienst über den Bildschirmschoner des X-Servers.
//!
//! Unter GNOME und KDE ist der „Bildschirmschoner“ die Bildschirmsperre: GNOME sperrt dabei, wenn
//! die automatische Sperre an ist, KDE sperrt immer. Qross startet ihn deshalb nur, wenn der
//! Desktop ihn auch selbst nach einer Wartezeit startet.

use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use ashpd::zbus::Connection;
use x11rb::connection::Connection as _;
use x11rb::protocol::screensaver::{ConnectionExt as _, State};
use x11rb::protocol::xproto::{ConnectionExt as _, ScreenSaver, Window};
use x11rb::rust_connection::RustConnection;

use super::Outcome;
use crate::wayland_logic::LinuxSession;

/// Bildschirmschoner-Dienste: Name (zugleich Schnittstelle) und Objektpfad, in dieser Reihenfolge
/// versucht. GNOME bietet `org.freedesktop.ScreenSaver` nur zum Verhindern an, deshalb zuerst.
const SERVICES: [(&str, &str); 5] = [
    ("org.gnome.ScreenSaver", "/org/gnome/ScreenSaver"),
    ("org.cinnamon.ScreenSaver", "/org/cinnamon/ScreenSaver"),
    ("org.mate.ScreenSaver", "/org/mate/ScreenSaver"),
    ("org.freedesktop.ScreenSaver", "/org/freedesktop/ScreenSaver"),
    ("org.freedesktop.ScreenSaver", "/ScreenSaver"),
];
/// Antwortet der Bus nicht, nicht länger warten (die Abfrage läuft alle paar hundert Millisekunden)
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// Ohne Dienst erst nach dieser Zeit wieder suchen
const RETRY: Duration = Duration::from_secs(30);
/// Unter Wayland gibt es ohne Dienst keinen Bildschirmschoner, den Qross erreicht
const NO_SERVICE: &str = "without a screen saver service on D-Bus";

/// Der gefundene Dienst samt Verbindung
#[derive(Clone)]
struct Service {
    conn: Connection,
    name: &'static str,
    path: &'static str,
}

impl Service {
    async fn call_bool(&self, method: &'static str, arg: Option<bool>) -> ashpd::zbus::Result<bool> {
        let reply = match arg {
            Some(arg) => self.conn.call_method(Some(self.name), self.path, Some(self.name), method, &(arg,)).await?,
            None => self.conn.call_method(Some(self.name), self.path, Some(self.name), method, &()).await?,
        };
        reply.body().deserialize::<bool>()
    }

    async fn call(&self, method: &'static str) -> ashpd::zbus::Result<()> {
        self.conn.call_method(Some(self.name), self.path, Some(self.name), method, &()).await.map(drop)
    }

    fn get_active(&self) -> Option<bool> {
        let service = self.clone();
        crate::wayland::block_on(CALL_TIMEOUT, async move { service.call_bool("GetActive", None).await.ok() }).flatten()
    }

    fn set_active(&self, active: bool) -> Option<bool> {
        let service = self.clone();
        crate::wayland::block_on(CALL_TIMEOUT, async move { service.call_bool("SetActive", Some(active)).await.ok() })
            .flatten()
    }

    /// Methode ohne Antwortwert (`Lock`, `SimulateUserActivity`); `true`, wenn der Dienst sie kennt
    fn invoke(&self, method: &'static str) -> bool {
        let service = self.clone();
        crate::wayland::block_on(CALL_TIMEOUT, async move { service.call(method).await.is_ok() }).unwrap_or(false)
    }

    fn is_gnome(&self) -> bool {
        self.name == "org.gnome.ScreenSaver"
    }
}

/// Sucht den Dienst des Desktops; ohne Session-Bus oder Dienst `None` (dann erst nach `RETRY` neu).
fn service() -> Option<Service> {
    static FOUND: Mutex<(Option<Service>, Option<Instant>)> = Mutex::new((None, None));
    let mut found = FOUND.lock().unwrap();
    if let Some(service) = &found.0 {
        return Some(service.clone());
    }
    if found.1.is_some_and(|next| Instant::now() < next) {
        return None;
    }
    found.1 = Some(Instant::now() + RETRY);
    let service = crate::wayland::block_on(CALL_TIMEOUT * 3, async {
        let conn = Connection::session().await.ok()?;
        for (name, path) in SERVICES {
            let service = Service { conn: conn.clone(), name, path };
            if service.call_bool("GetActive", None).await.is_ok() {
                return Some(service);
            }
        }
        None
    })
    .flatten();
    if let Some(service) = &service {
        tracing::debug!("screen saver: {} on the session bus", service.name);
    }
    found.0.clone_from(&service);
    service
}

fn x11_session() -> bool {
    crate::linux::session() == LinuxSession::X11
}

/// Verbindung zum X-Server für die regelmäßige Abfrage
fn with_x11<T>(f: impl FnOnce(&RustConnection, Window) -> Option<T>) -> Option<T> {
    static X11: OnceLock<Mutex<Option<(RustConnection, Window)>>> = OnceLock::new();
    let slot = X11.get_or_init(|| Mutex::new(crate::x11::connect().ok()));
    let guard = slot.lock().unwrap();
    let (conn, root) = guard.as_ref()?;
    f(conn, *root)
}

/// Bildschirmschoner des X-Servers an (Erweiterung MIT-SCREEN-SAVER)?
fn x11_active() -> bool {
    with_x11(|conn, root| {
        let state = conn.screensaver_query_info(root).ok()?.reply().ok()?.state;
        Some(state == u8::from(State::ON) || state == u8::from(State::CYCLE))
    })
    .unwrap_or(false)
}

fn x11_force(mode: ScreenSaver) -> bool {
    with_x11(|conn, _| {
        conn.force_screen_saver(mode).ok()?;
        conn.flush().ok()
    })
    .is_some()
}

/// Der X-Server startet seinen Bildschirmschoner nur mit einer Wartezeit über 0 (`xset s`).
fn x11_configured() -> bool {
    with_x11(|conn, _| Some(conn.get_screen_saver().ok()?.reply().ok()?.timeout > 0)).unwrap_or(false)
}

pub fn running() -> bool {
    match service() {
        Some(service) => service.get_active().unwrap_or(false),
        None => x11_session() && x11_active(),
    }
}

/// Startet der Desktop selbst einen Bildschirmschoner (bzw. sperrt er nach einer Wartezeit)?
fn desktop_configured(service: &Service) -> bool {
    if service.is_gnome() {
        let out = Command::new("gsettings").args(["get", "org.gnome.desktop.session", "idle-delay"]).output();
        return out.map_or(true, |out| super::gnome_configured(&String::from_utf8_lossy(&out.stdout)));
    }
    let kde = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("KDE")));
    if kde {
        let rc = dirs::config_dir().map(|dir| dir.join("kscreenlockerrc"));
        return super::kde_configured(&rc.and_then(|rc| std::fs::read_to_string(rc).ok()).unwrap_or_default());
    }
    true
}

pub fn start() -> Outcome {
    if let Some(service) = service() {
        if service.get_active() == Some(true) {
            return Outcome::Unchanged;
        }
        if !desktop_configured(&service) {
            return Outcome::NotConfigured;
        }
        return match service.set_active(true) {
            Some(true) => Outcome::Started,
            _ => Outcome::Failed(format!("{} refused", service.name)),
        };
    }
    if !x11_session() {
        return Outcome::Unsupported(NO_SERVICE);
    }
    if x11_active() {
        return Outcome::Unchanged;
    }
    if !x11_configured() {
        return Outcome::NotConfigured;
    }
    if x11_force(ScreenSaver::ACTIVE) {
        Outcome::Started
    } else {
        Outcome::Failed("X server not reachable".into())
    }
}

pub fn stop() -> Outcome {
    if let Some(service) = service() {
        if service.get_active() == Some(false) {
            return Outcome::Unchanged;
        }
        // KDE kennt kein SetActive(false); dort weckt nur eine Aktivität, die Sperre bleibt.
        if service.set_active(false) == Some(true) || service.invoke("SimulateUserActivity") {
            return Outcome::Stopped;
        }
        return Outcome::Failed(format!("{} refused", service.name));
    }
    if !x11_session() {
        return Outcome::Unsupported(NO_SERVICE);
    }
    if !x11_active() {
        return Outcome::Unchanged;
    }
    if x11_force(ScreenSaver::RESET) {
        Outcome::Stopped
    } else {
        Outcome::Failed("X server not reachable".into())
    }
}

/// Über systemd-logind, dem alle großen Desktops folgen; sonst über den Bildschirmschoner-Dienst.
pub fn lock() -> Outcome {
    if Command::new("loginctl").arg("lock-session").output().is_ok_and(|out| out.status.success()) {
        return Outcome::Locked;
    }
    match service() {
        Some(service) if service.invoke("Lock") => Outcome::Locked,
        _ => Outcome::Unsupported("without loginctl or a screen saver service"),
    }
}
