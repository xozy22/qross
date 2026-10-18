//! Eingaben unter Wayland einspeisen: Portal `RemoteDesktop` (Freigabe-Dialog, Restore-Token),
//! dann libei als Sender. Ist `LIBEI_SOCKET` gesetzt, geht es ohne Portal direkt dorthin (so prüfen
//! es die Tests gegen eine eigene EIS-Gegenstelle).
//!
//! Absolute Bewegung über ein Gerät mit `ei_pointer_absolute`; dessen Regionen sind zugleich die
//! Monitore, die der Client meldet. Fehlt so ein Gerät, wird relativ von der zuletzt gesetzten
//! Position aus bewegt.

use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::Result;
use ashpd::desktop::remote_desktop::{DeviceType, RemoteDesktop, SelectDevicesOptions, StartOptions};
use ashpd::desktop::{PersistMode, Session};
use futures_util::StreamExt;
use qross_core::Inject;
use qross_proto::Rect;
use reis::ei;
use reis::event::{DeviceCapability, EiEvent};
use tokio::sync::mpsc;

use super::tokens::{self, Kind};
use super::{now_micros, register_app, set_geometry, set_problem, spawn_session, start_error, Fail, Ready, Role};
use crate::keymap_linux::evdev_from_hid;
use crate::wayland_logic::{ei_discrete, evdev_button, snap};
use crate::{Injector, PortalProblem};

/// So lange wartet der Start nach dem Handshake auf ein Gerät für absolute Bewegung
const DEVICE_WAIT: Duration = Duration::from_millis(1500);
const RETRY_MAX: Duration = Duration::from_secs(30);

pub struct WaylandInjector {
    tx: mpsc::UnboundedSender<Inject>,
}

impl Injector for WaylandInjector {
    fn follows_screen_saver(&self) -> bool {
        true
    }

    fn screen_saver(&mut self, active: bool, lock: bool) -> crate::screensaver::Outcome {
        crate::screensaver::follow(active, lock)
    }

    fn inject(&mut self, event: &Inject) {
        let _ = self.tx.send(*event);
    }
}

/// Öffnet die Sitzung (beim ersten Mal mit Freigabe-Dialog) und kehrt zurück, sobald libei bereit
/// ist. Bricht die Verbindung später ab (z. B. „Freigabe beenden“ im Systemmenü), baut der
/// Hintergrund sie mit dem gemerkten Token neu auf.
pub fn start() -> Result<Box<dyn Injector>> {
    start_with(None)
}

/// Wie [`start`], aber mit einer schon geöffneten libei-Verbindung (Tests gegen mutter).
pub fn from_fd(fd: OwnedFd) -> Result<Box<dyn Injector>> {
    start_with(Some(fd))
}

fn start_with(fd: Option<OwnedFd>) -> Result<Box<dyn Injector>> {
    let (tx, rx) = mpsc::unbounded_channel();
    spawn_session("qross-wayland-inject", Role::Injector, move |ready| run(rx, ready, fd))?;
    Ok(Box::new(WaylandInjector { tx }))
}

/// Wie eine Sitzung endete
enum Ended {
    /// Der Injector wurde verworfen: nichts mehr zu tun
    Closed,
    /// Verbindung verloren (Grund)
    Lost(String),
}

async fn run(mut rx: mpsc::UnboundedReceiver<Inject>, mut ready: Ready, mut fd: Option<OwnedFd>) {
    let mut delay = Duration::from_secs(1);
    loop {
        let link = match fd.take() {
            // Schon geöffnet (Tests gegen mutter): nur beim ersten Mal, danach wie sonst
            Some(fd) => ei::Context::new(UnixStream::from(fd))
                .map(|context| Link { context, session: None })
                .map_err(Fail::from),
            None => connect().await,
        };
        match link {
            Ok(link) => {
                let ended = session(&link.context, &mut rx, &mut ready).await;
                if let Some(session) = &link.session {
                    let _ = session.close().await;
                }
                match ended {
                    Ended::Closed => return,
                    Ended::Lost(reason) => {
                        tracing::warn!("Wayland-Einspeisung unterbrochen: {reason}");
                        let problem = PortalProblem::Failed(format!("Verbindung verloren: {reason}"));
                        set_problem(Role::Injector, Some(problem));
                        delay = Duration::from_secs(1);
                    }
                }
            }
            Err(fail) => {
                let Some(fail) = ready.fail(fail) else { return };
                set_problem(Role::Injector, Some(fail.problem()));
                if matches!(fail, Fail::Denied) {
                    // Nicht wieder und wieder fragen; ein Neustart des Clients fragt erneut.
                    tracing::error!("Freigabe zum Einspeisen abgelehnt – Eingaben kommen nicht mehr an");
                    while rx.recv().await.is_some() {}
                    return;
                }
                tracing::warn!("Wayland-Einspeisung: {}", start_error(Role::Injector, &fail));
            }
        }
        // Bis zum nächsten Versuch eintreffende Eingaben verwerfen
        let pause = tokio::time::sleep(delay);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                _ = &mut pause => break,
                event = rx.recv() => if event.is_none() { return },
            }
        }
        delay = (delay * 2).min(RETRY_MAX);
    }
}

struct Link {
    context: ei::Context,
    /// Hält die Portal-Sitzung offen (fehlt bei `LIBEI_SOCKET`)
    session: Option<Session<RemoteDesktop>>,
}

async fn connect() -> Result<Link, Fail> {
    if let Some(context) = ei::Context::connect_to_env()? {
        tracing::info!("libei direkt über LIBEI_SOCKET (ohne Portal)");
        return Ok(Link { context, session: None });
    }
    register_app().await;
    let portal = RemoteDesktop::new().await?;
    if portal.version() < 2 {
        return Err(Fail::Missing(format!(
            "Das Portal RemoteDesktop hat Version {} und kann noch kein libei (ConnectToEIS)",
            portal.version()
        )));
    }
    let session = portal.create_session(Default::default()).await?;
    let token = tokens::load(Kind::RemoteDesktop);
    let options = SelectDevicesOptions::default()
        .set_devices(DeviceType::Keyboard | DeviceType::Pointer)
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(token.as_deref());
    portal.select_devices(&session, options).await?.response()?;
    if token.is_none() {
        tracing::info!("Das System fragt jetzt, ob Qross Maus und Tastatur steuern darf …");
    }
    let started = portal.start(&session, None, StartOptions::default()).await?.response()?;
    tokens::save(Kind::RemoteDesktop, started.restore_token());
    let devices = started.devices();
    if !devices.contains(DeviceType::Keyboard) || !devices.contains(DeviceType::Pointer) {
        tracing::warn!("Freigegeben wurden nur {devices:?} – Tastatur oder Maus fehlen");
    }
    let fd = portal.connect_to_eis(&session, Default::default()).await?;
    let context = ei::Context::new(UnixStream::from(fd))?;
    tracing::info!(
        "Wayland-Einspeisung über das Portal{}",
        if started.restore_token().is_some() { " (Freigabe gemerkt)" } else { "" }
    );
    Ok(Link { context, session: Some(session) })
}

async fn session(context: &ei::Context, rx: &mut mpsc::UnboundedReceiver<Inject>, ready: &mut Ready) -> Ended {
    let (connection, mut events) = match context.handshake_tokio("Qross", ei::handshake::ContextType::Sender).await {
        Ok(handshake) => handshake,
        Err(e) => {
            let reason = format!("libei-Handshake: {e}");
            if ready.waiting() {
                ready.fail(Fail::Other(anyhow::anyhow!(reason.clone())));
            }
            return Ended::Lost(reason);
        }
    };
    set_problem(Role::Injector, None);
    let mut sender = Sender::new(connection);
    // Kurz auf die Geräte warten, damit die erste Größenabfrage schon die Monitore kennt.
    let device_wait = tokio::time::sleep(DEVICE_WAIT);
    tokio::pin!(device_wait);
    loop {
        tokio::select! {
            // Erst den Stand des Compositors (Geräte, Freigaben), dann die Eingaben
            biased;
            event = events.next() => match event {
                Some(Ok(event)) => {
                    if let Some(ended) = sender.on_event(event) {
                        return ended;
                    }
                    if sender.absolute().is_some() {
                        ready.ok();
                    }
                }
                Some(Err(e)) => return Ended::Lost(format!("libei: {e}")),
                None => return Ended::Lost("libei-Verbindung geschlossen".into()),
            },
            _ = &mut device_wait, if ready.waiting() => {
                tracing::warn!("libei bietet kein Gerät für absolute Bewegung – bewege relativ");
                ready.ok();
            }
            event = rx.recv() => match event {
                Some(event) => sender.inject(&event),
                None => return Ended::Closed,
            },
        }
    }
}

struct Device {
    device: reis::event::Device,
    /// Der Compositor hat das Gerät freigegeben, und es emuliert gerade
    emulating: bool,
}

struct Sender {
    connection: reis::event::Connection,
    devices: Vec<Device>,
    sequence: u32,
    /// Zuletzt gesetzte Position, für relative Bewegung ohne Absolut-Gerät
    last: Option<(i32, i32)>,
}

impl Sender {
    fn new(connection: reis::event::Connection) -> Self {
        Self { connection, devices: Vec::new(), sequence: 0, last: None }
    }

    fn on_event(&mut self, event: EiEvent) -> Option<Ended> {
        match event {
            EiEvent::SeatAdded(e) => {
                e.seat.bind_capabilities(
                    DeviceCapability::Pointer
                        | DeviceCapability::PointerAbsolute
                        | DeviceCapability::Keyboard
                        | DeviceCapability::Button
                        | DeviceCapability::Scroll,
                );
                self.flush();
            }
            EiEvent::SeatRemoved(e) => {
                self.devices.retain(|d| d.device.seat() != &e.seat);
                self.update_geometry();
            }
            EiEvent::DeviceAdded(e) => {
                tracing::debug!("libei-Gerät {:?}: {}", e.device.name(), describe_regions(e.device.regions()));
                self.devices.push(Device { device: e.device, emulating: false });
                self.update_geometry();
            }
            EiEvent::DeviceRemoved(e) => {
                self.devices.retain(|d| d.device != e.device);
                self.update_geometry();
            }
            EiEvent::DeviceResumed(e) => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.device == e.device) {
                    d.device.device().start_emulating(self.connection.serial(), self.sequence);
                    self.sequence = self.sequence.wrapping_add(1);
                    d.emulating = true;
                }
                self.flush();
            }
            EiEvent::DevicePaused(e) => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.device == e.device) {
                    d.emulating = false;
                }
            }
            EiEvent::Disconnected(e) => {
                let why = e.explanation.unwrap_or_else(|| format!("{:?}", e.reason));
                return Some(Ended::Lost(format!("vom Compositor getrennt: {why}")));
            }
            _ => {}
        }
        None
    }

    fn flush(&self) {
        if let Err(e) = self.connection.flush() {
            tracing::debug!("libei senden: {e}");
        }
    }

    /// Ein emulierendes Gerät mit dieser Fähigkeit; bevorzugt das Absolut-Gerät, damit Bewegung und
    /// Tasten möglichst vom selben Gerät kommen.
    fn device(&self, capability: DeviceCapability) -> Option<&reis::event::Device> {
        let usable = |d: &&Device| d.emulating && d.device.has_capability(capability);
        let absolute = |d: &&Device| d.device.has_capability(DeviceCapability::PointerAbsolute);
        let mut candidates = self.devices.iter().filter(usable);
        let first = candidates.clone().next();
        candidates.find(absolute).or(first).map(|d| &d.device)
    }

    fn absolute(&self) -> Option<&reis::event::Device> {
        self.devices
            .iter()
            .find(|d| d.emulating && d.device.has_capability(DeviceCapability::PointerAbsolute) && !d.device.regions().is_empty())
            .map(|d| &d.device)
    }

    /// Monitore = Regionen der Absolut-Geräte
    fn update_geometry(&self) {
        let mut monitors: Vec<Rect> = Vec::new();
        for d in self.devices.iter().filter(|d| d.device.has_capability(DeviceCapability::PointerAbsolute)) {
            for r in d.device.regions() {
                let rect = region_rect(r);
                if !monitors.contains(&rect) {
                    monitors.push(rect);
                }
            }
        }
        if !monitors.is_empty() {
            set_geometry(monitors);
        }
    }

    fn frame(&self, device: &reis::event::Device) {
        device.device().frame(self.connection.serial(), now_micros());
        self.flush();
    }

    fn inject(&mut self, event: &Inject) {
        match *event {
            Inject::MoveTo { x, y } => self.move_to(x, y),
            Inject::MoveBy { dx, dy } => {
                // Relative Bewegung (Spielemodus): über ein Gerät für relative Zeigerbewegung
                let Some(device) = self.device(DeviceCapability::Pointer) else {
                    return tracing::debug!("libei: kein Gerät für relative Bewegung");
                };
                let Some(pointer) = device.interface::<ei::Pointer>() else { return };
                pointer.motion_relative(dx as f32, dy as f32);
                self.frame(device);
            }
            Inject::Button { button, down } => {
                let Some(device) = self.device(DeviceCapability::Button) else {
                    return tracing::debug!("libei: kein Gerät für Maustasten");
                };
                let Some(buttons) = device.interface::<ei::Button>() else { return };
                let state = if down { ei::button::ButtonState::Press } else { ei::button::ButtonState::Released };
                buttons.button(evdev_button(button), state);
                self.frame(device);
            }
            Inject::Wheel { dx, dy } => {
                let Some(device) = self.device(DeviceCapability::Scroll) else {
                    return tracing::debug!("libei: kein Gerät für das Mausrad");
                };
                let Some(scroll) = device.interface::<ei::Scroll>() else { return };
                let (x, y) = ei_discrete(dx, dy);
                scroll.scroll_discrete(x, y);
                self.frame(device);
            }
            Inject::Key { key, down } => {
                let Some(code) = evdev_from_hid(key) else {
                    return tracing::debug!(?key, "Taste ohne evdev-Zuordnung");
                };
                let Some(device) = self.device(DeviceCapability::Keyboard) else {
                    return tracing::debug!("libei: keine Tastatur");
                };
                let Some(keyboard) = device.interface::<ei::Keyboard>() else { return };
                let state = if down { ei::keyboard::KeyState::Press } else { ei::keyboard::KeyState::Released };
                keyboard.key(u32::from(code), state);
                self.frame(device);
            }
            // libei kennt nur Tasten. Der Client meldet dem Server deshalb keinen Zeichenmodus
            // (`types_text`), und das kommt nicht vor; ein Kürzel ginge notfalls als Taste.
            Inject::Char { ch, .. } => tracing::debug!(?ch, "libei kann keine Zeichen eingeben"),
            Inject::Shortcut { key, down, .. } => self.inject(&Inject::Key { key, down }),
        }
    }

    fn move_to(&mut self, x: i32, y: i32) {
        if let Some(device) = self.absolute() {
            // Außerhalb aller Regionen verwirft der Compositor die Bewegung: auf die nächste legen.
            let regions: Vec<Rect> = device.regions().iter().map(region_rect).collect();
            let (x, y) = snap(&regions, x, y);
            if let Some(pointer) = device.interface::<ei::PointerAbsolute>() {
                pointer.motion_absolute(x as f32, y as f32);
                self.frame(device);
                self.last = Some((x, y));
            }
            return;
        }
        let Some(device) = self.device(DeviceCapability::Pointer) else {
            return tracing::debug!("libei: kein Zeigergerät");
        };
        let Some(pointer) = device.interface::<ei::Pointer>() else { return };
        // Ohne bekannte Position erst in die linke obere Ecke schieben, dann von dort aus zählen
        let (from_x, from_y) = match self.last {
            Some(last) => last,
            None => {
                pointer.motion_relative(-100_000.0, -100_000.0);
                (0, 0)
            }
        };
        pointer.motion_relative((x - from_x) as f32, (y - from_y) as f32);
        self.frame(device);
        self.last = Some((x, y));
    }
}

fn region_rect(r: &reis::event::Region) -> Rect {
    let coord = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
    Rect::new(coord(r.x), coord(r.y), r.width, r.height)
}

fn describe_regions(regions: &[reis::event::Region]) -> String {
    if regions.is_empty() {
        return "ohne Regionen".into();
    }
    let list: Vec<String> =
        regions.iter().map(|r| format!("{}×{} bei ({}, {}), Skalierung {}", r.width, r.height, r.x, r.y, r.scale)).collect();
    list.join("; ")
}
