//! Eingaben unter Wayland erfassen: Portal `InputCapture` mit Zeigerbarrieren an den äußeren
//! Monitorkanten, Eingaben danach über libei (Empfänger).
//!
//! Anders als unter X11 und Windows entscheidet hier der Compositor, wann erfasst wird: Stößt der
//! Cursor an eine Barriere, ist die Erfassung schon aktiv („Activated“), bevor der Server von der
//! Kante weiß. Qross meldet dann `EdgeHit` und wartet auf die Entscheidung:
//! - Der Server wechselt (`capture_remote`): ab jetzt gehen die Eingaben an den Client.
//! - Der Server wechselt nicht (Ecke, Verzögerung, kein Nachbar): Bewegt sich die Maus von der
//!   Kante weg oder kommt 300 ms lang kein Druck gegen die Kante, gibt Qross den Cursor frei.
//!   Drückt der Nutzer weiter gegen die Kante, kommt erneut `EdgeHit` (für die Wechselverzögerung).
//!
//! Die Fang-Taste wirkt nur, während der Cursor auf einem Client ist: Lokal sieht Qross unter
//! Wayland keine Tasten.

use std::collections::HashSet;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use ashpd::desktop::input_capture::{
    ActivatedBarrier, Barrier as PortalBarrier, BarrierID, BarrierPosition, Capabilities, CreateSessionOptions, InputCapture,
    ReleaseOptions, StartOptions,
};
use ashpd::desktop::{PersistMode, Session};
use futures_util::StreamExt;
use qross_core::keys::{HotkeyEvent, HotkeyTracker, KeyOptions};
use qross_core::CaptureEvent;
use qross_proto::Rect;
use reis::ei;
use reis::event::{DeviceCapability, EiEvent};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::tokens::{self, Kind};
use super::{describe, set_geometry, set_problem, spawn_session, start_error, Fail, Ready, Role};
use crate::keymap_linux::hid_from_evdev;
use crate::wayland_logic::{
    self, barrier_near, button_from_evdev, position_on, pushes_into, wheel_from_ei_discrete, Barrier, Motion,
    SmoothWheel,
};
use crate::{Capture, PortalProblem};

/// So lange darf eine ausgelöste Barriere ohne Druck gegen die Kante auf den Server warten
const PENDING: Duration = Duration::from_millis(300);
const RETRY_MAX: Duration = Duration::from_secs(30);
/// evdev-Codes für den Notausgang Strg+Alt+Shift+Esc
const KEY_ESC: u32 = 1;
const CTRL: [u32; 2] = [29, 97];
const ALT: [u32; 2] = [56, 100];
const SHIFT: [u32; 2] = [42, 54];

enum Command {
    Remote,
    Release { x: i32, y: i32 },
    Keys(KeyOptions),
}

struct WaylandCapture {
    tx: UnboundedSender<Command>,
}

impl Capture for WaylandCapture {
    fn screen_saver_active(&self) -> bool {
        crate::screensaver::running()
    }

    fn stop_screen_saver(&self) {
        crate::screensaver::stop_in_background();
    }

    fn capture_remote(&self) {
        let _ = self.tx.send(Command::Remote);
    }

    fn release(&self, x: i32, y: i32) {
        let _ = self.tx.send(Command::Release { x, y });
    }

    fn set_key_options(&self, options: KeyOptions) {
        let _ = self.tx.send(Command::Keys(options));
    }
}

/// Öffnet die Sitzung (je nach Desktop mit Freigabe-Dialog), setzt die Barrieren und kehrt zurück,
/// sobald erfasst werden kann.
pub fn start(events: UnboundedSender<CaptureEvent>) -> Result<Arc<dyn Capture>> {
    let (tx, rx) = mpsc::unbounded_channel();
    spawn_session("qross-wayland-capture", Role::Capture, move |ready| run(events, rx, ready))?;
    Ok(Arc::new(WaylandCapture { tx }))
}

enum Ended {
    /// Die Erfassung wurde verworfen
    Closed,
    Lost(String),
}

async fn run(events: UnboundedSender<CaptureEvent>, mut commands: UnboundedReceiver<Command>, mut ready: Ready) {
    let mut keys = KeyOptions::default();
    let mut delay = Duration::from_secs(1);
    loop {
        match session(&events, &mut commands, &mut keys, &mut ready).await {
            Ok(Ended::Closed) => return,
            Ok(Ended::Lost(reason)) => {
                tracing::warn!("Wayland capture interrupted: {reason}");
                let problem = PortalProblem::Failed(format!("connection lost: {reason}"));
                set_problem(Role::Capture, Some(problem));
                delay = Duration::from_secs(1);
            }
            Err(fail) => {
                let Some(fail) = ready.fail(fail) else { return };
                set_problem(Role::Capture, Some(fail.problem()));
                if matches!(fail, Fail::Denied) {
                    tracing::error!(
                        "permission to capture input denied; the cursor stays on this computer until the server restarts"
                    );
                    // Nicht wieder und wieder fragen. Ein neu gestarteter Server setzt die Tasten
                    // (`set_key_options`) – das gilt als Bitte, erneut zu fragen.
                    loop {
                        match commands.recv().await {
                            None => return,
                            Some(Command::Keys(options)) => {
                                keys = options;
                                break;
                            }
                            Some(command) => idle_command(command, &events, &mut keys),
                        }
                    }
                    delay = Duration::from_secs(1);
                    continue;
                }
                tracing::warn!("Wayland capture: {}", start_error(Role::Capture, &fail));
            }
        }
        let pause = tokio::time::sleep(delay);
        tokio::pin!(pause);
        loop {
            tokio::select! {
                _ = &mut pause => break,
                command = commands.recv() => match command {
                    Some(command) => idle_command(command, &events, &mut keys),
                    None => return,
                },
            }
        }
        delay = (delay * 2).min(RETRY_MAX);
    }
}

/// Befehle ohne Sitzung: Erfassen geht gerade nicht, der Server soll lokal bleiben.
fn idle_command(command: Command, events: &UnboundedSender<CaptureEvent>, keys: &mut KeyOptions) {
    match command {
        Command::Remote => {
            let _ = events.send(CaptureEvent::PanicReturn);
        }
        Command::Release { .. } => {}
        Command::Keys(options) => *keys = options,
    }
}

async fn session(
    events: &UnboundedSender<CaptureEvent>,
    commands: &mut UnboundedReceiver<Command>,
    keys: &mut KeyOptions,
    ready: &mut Ready,
) -> Result<Ended, Fail> {
    let portal = InputCapture::new().await?;
    let capabilities = Capabilities::Keyboard | Capabilities::Pointer;
    let token = tokens::load(Kind::InputCapture);
    if token.is_none() || portal.version() < 2 {
        tracing::info!("The system is now asking whether Qross may capture the mouse and keyboard …");
    }
    let session = if portal.version() >= 2 {
        // Ab Version 2 mit gemerkter Freigabe
        let session = portal.create_session2(Default::default()).await?;
        let options = StartOptions::default()
            .set_capabilities(capabilities)
            .set_persist_mode(PersistMode::ExplicitlyRevoked)
            .set_restore_token(token);
        let started = portal.start(&session, None, options).await?.response()?;
        tokens::save(Kind::InputCapture, started.restore_token());
        session
    } else {
        let options = CreateSessionOptions::default().set_capabilities(capabilities);
        let (session, granted) = portal.create_session(None, options).await?;
        if !granted.contains(Capabilities::Pointer) {
            return Err(Fail::Other(anyhow::anyhow!("the portal only allows {granted:?}, the mouse is missing")));
        }
        session
    };
    let result = run_session(&portal, &session, events, commands, keys, ready).await;
    let _ = session.close().await;
    result
}

async fn run_session(
    portal: &InputCapture,
    session: &Session<InputCapture>,
    events: &UnboundedSender<CaptureEvent>,
    commands: &mut UnboundedReceiver<Command>,
    keys: &mut KeyOptions,
    ready: &mut Ready,
) -> Result<Ended, Fail> {
    let fd = portal.connect_to_eis(session, Default::default()).await?;
    let context = ei::Context::new(UnixStream::from(fd))?;
    let (connection, mut ei_events) = context.handshake_tokio("Qross", ei::handshake::ContextType::Receiver).await?;
    // Vor dem Einschalten abonnieren, damit kein Signal verloren geht
    let mut activated = portal.receive_activated().await?;
    let mut deactivated = portal.receive_deactivated().await?;
    let mut disabled = portal.receive_disabled().await?;
    let mut zones_changed = portal.receive_zones_changed().await?;
    let (mut barriers, mut monitors) = setup(portal, session).await?;
    ready.ok();
    set_problem(Role::Capture, None);

    let mut state = State::new(events.clone(), connection);
    let mut resets: Vec<Instant> = Vec::new();
    loop {
        let deadline = state.deadline();
        let release = tokio::select! {
            // Signale gehen nur an den Besitzer der Sitzung, und Qross hat höchstens eine offen.
            signal = activated.next() => {
                let Some(signal) = signal else { return Ok(Ended::Lost("portal signals ended".into())) };
                let barrier = match signal.barrier_id() {
                    Some(ActivatedBarrier::Barrier(id)) => Some(id.get()),
                    _ => None,
                };
                state.on_activated(signal.activation_id(), signal.cursor_position(), barrier, &barriers, &monitors)
            }
            signal = deactivated.next() => {
                let Some(signal) = signal else { return Ok(Ended::Lost("portal signals ended".into())) };
                state.on_deactivated(signal.activation_id());
                None
            }
            signal = disabled.next() => {
                // mutter lässt eine ausgeschaltete Sitzung nicht zuverlässig wieder einschalten
                // (xdg-desktop-portal-gnome #126): die Sitzung neu aufbauen.
                state.on_deactivated(None);
                return Ok(Ended::Lost(match signal {
                    Some(_) => "the compositor disabled the capture".into(),
                    None => "portal signals ended".into(),
                }));
            }
            signal = zones_changed.next() => {
                if signal.is_none() {
                    return Ok(Ended::Lost("portal signals ended".into()));
                }
                tracing::info!("monitors changed, setting the barriers again");
                state.on_deactivated(None);
                (barriers, monitors) = match reset(portal, session, &mut resets).await {
                    Ok(setup) => setup,
                    Err(reason) => return Ok(Ended::Lost(reason)),
                };
                None
            }
            event = ei_events.next() => match event {
                Some(Ok(event)) => match state.on_ei(event, keys, &monitors) {
                    Outcome::Nothing => None,
                    Outcome::Release(release) => Some(release),
                    Outcome::Lost(reason) => return Ok(Ended::Lost(reason)),
                },
                Some(Err(e)) => return Ok(Ended::Lost(format!("libei: {e}"))),
                None => return Ok(Ended::Lost("libei connection closed".into())),
            },
            command = commands.recv() => match command {
                Some(Command::Keys(options)) => {
                    *keys = options;
                    None
                }
                Some(command) => state.on_command(command),
                None => return Ok(Ended::Closed),
            },
            _ = sleep_until(deadline), if deadline.is_some() => state.on_timeout(),
        };
        if let Some(release) = release {
            let options = ReleaseOptions::default()
                .set_activation_id(release.id)
                .set_cursor_position((f64::from(release.x) + 0.5, f64::from(release.y) + 0.5));
            if let Err(e) = portal.release(session, options).await {
                tracing::warn!("failed to release the capture: {e}");
            }
        }
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    if let Some(deadline) = deadline {
        tokio::time::sleep_until(deadline.into()).await;
    }
}

/// Zonen holen, Barrieren setzen, einschalten. Liefert die angenommenen Barrieren und die Monitore.
async fn setup(portal: &InputCapture, session: &Session<InputCapture>) -> Result<(Vec<Barrier>, Vec<Rect>), Fail> {
    let zones = portal.zones(session, Default::default()).await?.response()?;
    let monitors: Vec<Rect> =
        zones.regions().iter().map(|r| Rect::new(r.x_offset(), r.y_offset(), r.width(), r.height())).collect();
    if monitors.is_empty() {
        return Err(Fail::Other(anyhow::anyhow!("the portal reports no monitors (zones)")));
    }
    set_geometry(monitors.clone());
    let wanted = wayland_logic::barriers(&monitors);
    let request: Vec<PortalBarrier> = wanted
        .iter()
        .filter_map(|b| {
            let (x1, y1, x2, y2) = b.line;
            Some(PortalBarrier::new(BarrierID::new(b.id)?, BarrierPosition::new(x1, y1, x2, y2)))
        })
        .collect();
    let response = portal.set_pointer_barriers(session, &request, zones.zone_set(), Default::default()).await?.response()?;
    let failed: Vec<u32> = response.failed_barriers().iter().map(|id| id.get()).collect();
    let (accepted, refused): (Vec<Barrier>, Vec<Barrier>) = wanted.into_iter().partition(|b| !failed.contains(&b.id));
    if !refused.is_empty() {
        let list: Vec<String> = refused.iter().map(|b| format!("{:?} {:?}", b.edge, b.line)).collect();
        tracing::warn!("the portal refused barriers: {}", list.join(", "));
    }
    if accepted.is_empty() {
        return Err(Fail::Other(anyhow::anyhow!("the portal refused all pointer barriers")));
    }
    portal.enable(session, Default::default()).await?;
    let edges: Vec<String> = accepted.iter().map(|b| format!("{:?}", b.edge)).collect();
    tracing::info!("Wayland capture ready, monitors {}, edges {}", describe(&monitors), edges.join(", "));
    Ok((accepted, monitors))
}

/// Nach geänderten Monitoren neu einrichten; mehr als dreimal in 10 s gilt als verloren (dann
/// baut `run` die ganze Sitzung neu auf).
async fn reset(
    portal: &InputCapture,
    session: &Session<InputCapture>,
    resets: &mut Vec<Instant>,
) -> Result<(Vec<Barrier>, Vec<Rect>), String> {
    resets.retain(|at| at.elapsed() < Duration::from_secs(10));
    resets.push(Instant::now());
    if resets.len() > 3 {
        return Err("cannot re-enable the capture".into());
    }
    setup(portal, session).await.map_err(|e| format!("failed to set the barriers again: {}", e.problem()))
}

/// Erfassung freigeben: Aktivierung und Cursorposition danach
struct Release {
    id: Option<u32>,
    x: i32,
    y: i32,
}

enum Outcome {
    Nothing,
    Release(Release),
    Lost(String),
}

enum Mode {
    Local,
    /// Barriere ausgelöst, der Server entscheidet noch
    Pending { id: Option<u32>, barrier: Barrier, at: (i32, i32), last_push: Instant },
    Remote { id: Option<u32> },
}

struct State {
    events: UnboundedSender<CaptureEvent>,
    connection: reis::event::Connection,
    mode: Mode,
    motion: Motion,
    wheel: SmoothWheel,
    /// Rad innerhalb des laufenden libei-Frames: Rasten und stufenlos
    frame_discrete: (i32, i32),
    frame_smooth: (f64, f64),
    hotkey: HotkeyTracker,
    pressed: HashSet<u32>,
}

impl State {
    fn new(events: UnboundedSender<CaptureEvent>, connection: reis::event::Connection) -> Self {
        Self {
            events,
            connection,
            mode: Mode::Local,
            motion: Motion::default(),
            wheel: SmoothWheel::default(),
            frame_discrete: (0, 0),
            frame_smooth: (0.0, 0.0),
            hotkey: HotkeyTracker::new(),
            pressed: HashSet::new(),
        }
    }

    fn send(&self, event: CaptureEvent) {
        let _ = self.events.send(event);
    }

    fn deadline(&self) -> Option<Instant> {
        match &self.mode {
            Mode::Pending { last_push, .. } => Some(*last_push + PENDING),
            _ => None,
        }
    }

    fn on_activated(
        &mut self,
        id: Option<u32>,
        cursor: Option<(f32, f32)>,
        barrier: Option<u32>,
        barriers: &[Barrier],
        monitors: &[Rect],
    ) -> Option<Release> {
        let (cx, cy) = cursor.map(|(x, y)| (f64::from(x), f64::from(y))).unwrap_or_else(|| {
            let (x, y) = wayland_logic::center(monitors);
            (f64::from(x), f64::from(y))
        });
        let found = barrier
            .and_then(|id| barriers.iter().find(|b| b.id == id))
            .or_else(|| barrier_near(barriers, cx as i32, cy as i32))
            .copied();
        let Some(barrier) = found else {
            // Ohne Barriere ausgelöst (sollte nicht vorkommen): sofort zurückgeben
            let (x, y) = wayland_logic::center(monitors);
            return Some(Release { id, x, y });
        };
        let at = position_on(&barrier, cx, cy);
        self.mode = Mode::Pending { id, barrier, at, last_push: Instant::now() };
        self.send(CaptureEvent::EdgeHit { edge: barrier.edge, x: at.0, y: at.1, buttons_down: false });
        None
    }

    /// Der Compositor hat die Erfassung beendet (`id`: welche Aktivierung; `None`: jede). Das
    /// Signal kommt auch nach einem eigenen `Release` – dann läuft vielleicht schon die nächste
    /// Aktivierung, und die darf nicht verloren gehen.
    fn on_deactivated(&mut self, id: Option<u32>) {
        let current = match self.mode {
            Mode::Pending { id, .. } | Mode::Remote { id } => id,
            Mode::Local => return,
        };
        if id.is_some() && current.is_some() && id != current {
            return;
        }
        if matches!(self.mode, Mode::Remote { .. }) {
            self.send(CaptureEvent::PanicReturn);
        }
        self.mode = Mode::Local;
    }

    fn on_command(&mut self, command: Command) -> Option<Release> {
        match command {
            Command::Remote => match self.mode {
                Mode::Pending { id, .. } => {
                    self.mode = Mode::Remote { id };
                    self.hotkey = HotkeyTracker::new();
                    self.motion.reset();
                    self.wheel.reset();
                    self.pressed.clear();
                    None
                }
                Mode::Remote { .. } => None,
                // Schon wieder freigegeben (Maus ging zurück): Server soll lokal bleiben
                Mode::Local => {
                    self.send(CaptureEvent::PanicReturn);
                    None
                }
            },
            Command::Release { x, y } => match std::mem::replace(&mut self.mode, Mode::Local) {
                Mode::Pending { id, .. } | Mode::Remote { id } => Some(Release { id, x, y }),
                Mode::Local => None,
            },
            Command::Keys(_) => None,
        }
    }

    fn on_timeout(&mut self) -> Option<Release> {
        match self.mode {
            Mode::Pending { id, at, last_push, .. } if last_push.elapsed() >= PENDING => {
                self.mode = Mode::Local;
                Some(Release { id, x: at.0, y: at.1 })
            }
            _ => None,
        }
    }

    /// Eine Eingabe beim Warten auf den Server: Druck gegen die Kante meldet sie erneut, alles
    /// andere gibt den Cursor sofort frei (die Eingabe selbst geht dabei verloren).
    fn pending_input(&mut self, push: Option<(f64, f64)>) -> Option<Release> {
        let Mode::Pending { id, barrier, at, ref mut last_push } = self.mode else { return None };
        if let Some((dx, dy)) = push {
            if pushes_into(barrier.edge, dx, dy) {
                *last_push = Instant::now();
                self.send(CaptureEvent::EdgeHit { edge: barrier.edge, x: at.0, y: at.1, buttons_down: false });
                return None;
            }
            self.mode = Mode::Local;
            let (x, y) = barrier.monitor.clamp(at.0 + dx.round() as i32, at.1 + dy.round() as i32);
            return Some(Release { id, x, y });
        }
        self.mode = Mode::Local;
        Some(Release { id, x: at.0, y: at.1 })
    }

    fn on_ei(&mut self, event: EiEvent, keys: &KeyOptions, monitors: &[Rect]) -> Outcome {
        let remote = matches!(self.mode, Mode::Remote { .. });
        let pending = matches!(self.mode, Mode::Pending { .. });
        let release = match event {
            EiEvent::SeatAdded(e) => {
                e.seat.bind_capabilities(
                    DeviceCapability::Pointer
                        | DeviceCapability::PointerAbsolute
                        | DeviceCapability::Keyboard
                        | DeviceCapability::Button
                        | DeviceCapability::Scroll,
                );
                if let Err(e) = self.connection.flush() {
                    tracing::debug!("failed to send via libei: {e}");
                }
                None
            }
            EiEvent::Disconnected(e) => {
                let why = e.explanation.unwrap_or_else(|| format!("{:?}", e.reason));
                return Outcome::Lost(format!("disconnected by the compositor: {why}"));
            }
            EiEvent::PointerMotion(e) => {
                let (dx, dy) = (f64::from(e.dx), f64::from(e.dy));
                if pending {
                    self.pending_input(Some((dx, dy)))
                } else {
                    if remote {
                        if let Some((dx, dy)) = self.motion.add(dx, dy) {
                            self.send(CaptureEvent::MouseDelta { dx, dy });
                        }
                    }
                    None
                }
            }
            EiEvent::Button(_) if pending => self.pending_input(None),
            EiEvent::Button(e) if remote => {
                if let Some(button) = button_from_evdev(e.button) {
                    let down = e.state == ei::button::ButtonState::Press;
                    self.send(CaptureEvent::Button { button, down });
                }
                None
            }
            EiEvent::ScrollDiscrete(e) if remote => {
                self.frame_discrete.0 += e.discrete_dx;
                self.frame_discrete.1 += e.discrete_dy;
                None
            }
            EiEvent::ScrollDelta(e) if remote => {
                self.frame_smooth.0 += f64::from(e.dx);
                self.frame_smooth.1 += f64::from(e.dy);
                None
            }
            EiEvent::ScrollStop(_) | EiEvent::ScrollCancel(_) => {
                self.wheel.reset();
                None
            }
            EiEvent::Frame(_) => {
                self.flush_wheel();
                None
            }
            EiEvent::KeyboardKey(_) if pending => self.pending_input(None),
            EiEvent::KeyboardKey(e) if remote => {
                let down = e.state == ei::keyboard::KeyState::Press;
                self.on_key(e.key, down, keys, monitors)
            }
            _ => None,
        };
        match release {
            Some(release) => Outcome::Release(release),
            None => Outcome::Nothing,
        }
    }

    /// Pro Frame entweder Rasten oder stufenloses Scrollen weitergeben, nie beides doppelt
    fn flush_wheel(&mut self) {
        let discrete = std::mem::take(&mut self.frame_discrete);
        let smooth = std::mem::take(&mut self.frame_smooth);
        if !matches!(self.mode, Mode::Remote { .. }) {
            return;
        }
        let wheel = if discrete != (0, 0) {
            self.wheel.reset();
            Some(wheel_from_ei_discrete(discrete.0, discrete.1))
        } else if smooth != (0.0, 0.0) {
            self.wheel.add(smooth.0, smooth.1)
        } else {
            None
        };
        if let Some((dx, dy)) = wheel {
            self.send(CaptureEvent::Wheel { dx, dy });
        }
    }

    fn on_key(&mut self, code: u32, down: bool, keys: &KeyOptions, monitors: &[Rect]) -> Option<Release> {
        if down {
            self.pressed.insert(code);
        } else {
            self.pressed.remove(&code);
        }
        let held = |set: [u32; 2]| set.iter().any(|k| self.pressed.contains(k));
        if code == KEY_ESC && down && held(CTRL) && held(ALT) && held(SHIFT) {
            // Notausgang: sofort lokal, Cursor in die Mitte
            let Mode::Remote { id } = std::mem::replace(&mut self.mode, Mode::Local) else { return None };
            self.pressed.clear();
            self.send(CaptureEvent::PanicReturn);
            let (x, y) = wayland_logic::center(monitors);
            return Some(Release { id, x, y });
        }
        let key = u16::try_from(code).ok().and_then(hid_from_evdev)?;
        // Fang-Taste und Wechsel-Kürzel; das gemeldete Ereignis wertet der Server-Kern aus.
        match self.hotkey.on_key(key, down, keys) {
            HotkeyEvent::Triggered { event, .. } => self.send(event),
            HotkeyEvent::Consumed { .. } => {}
            // Zusatztaste allein getippt: melden; ihr Loslassen geht weiter wie ihr Drücken
            HotkeyEvent::Tapped { event } => {
                self.send(event);
                self.send(CaptureEvent::Key { key, down });
            }
            HotkeyEvent::None => self.send(CaptureEvent::Key { key, down }),
        }
        None
    }
}
