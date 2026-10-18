//! Zustandsmaschine des Servers: Wo ist der Cursor gerade, und was muss wohin?

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use qross_proto::{Edge, Input, MouseButton, MouseMove, Rect};

use crate::input::CaptureEvent;
use crate::keys::KeyMap;
use crate::layout::Layout;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Capture(CaptureEvent),
    ClientConnected { name: String, screen: Rect },
    ClientScreenChanged { name: String, screen: Rect },
    ClientDisconnected { name: String },
    LocalScreenChanged(Rect),
    /// Antwort des Clients auf `Action::QueryRemoteDrag`: Werden dort Dateien gezogen?
    RemoteDragOffer { client: String, probe: u32, transfer: u64, has_files: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Lokale Eingaben ab jetzt abfangen und als Deltas melden
    CaptureRemote,
    /// Abfangen beenden und den lokalen Cursor an diese Position setzen
    ReleaseCapture { x: i32, y: i32 },
    /// Zuverlässig auf dem Input-Stream senden
    Send { to: String, input: Input },
    /// Als Datagramm senden
    Move { to: String, mv: MouseMove },
    /// Lokales Clipboard an diesen Client schicken, falls es sich seit dem letzten Mal geändert hat
    SyncClipboard { to: String },
    /// Mit gedrückter Taste an der Kante: nachsehen, ob gerade Dateien gezogen werden
    /// (Antwort: `CaptureEvent::DragProbeResult`)
    ProbeDrag { x: i32, y: i32 },
    /// Den lokalen Drag abbrechen – die Dateien wandern stattdessen zum Client
    CancelLocalDrag,
    /// `DragStart` an den Client senden und die Dateien übertragen
    StartFileDrag { to: String, transfer: u64, files: Vec<PathBuf> },
    /// Der Cursor steht auf dem Client mit gedrückter Taste an der Kante zum Server:
    /// nachfragen, ob dort Dateien gezogen werden (Antwort: `ServerEvent::RemoteDragOffer`)
    QueryRemoteDrag { to: String, probe: u32 },
    /// Dateien vom Client kommen; das nächste Loslassen hier legt sie ab (`CaptureEvent::LocalDrop`)
    AwaitLocalDrop { from: String, transfer: u64 },
    /// Losgelassen an (x, y): die Dateien dieser Übertragung dort ablegen
    DropIncoming { transfer: u64, x: i32, y: i32 },
    /// Nicht abgelegt (Cursor ging zurück, Client getrennt): Übertragung verwerfen
    CancelIncoming { transfer: u64 },
}

/// Wann der Cursor den Bildschirm wechseln darf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchOptions {
    /// Ecken-Totzone in Pixeln: In den Ecken wird nicht gewechselt (Startmenü, Schließen-Knöpfe,
    /// Hot Corners). 0 = aus.
    pub corner_size: u32,
    /// So lange muss gegen die Kante gedrückt werden, bevor gewechselt wird. 0 = sofort.
    pub delay: Duration,
}

impl SwitchOptions {
    /// Empfohlene Werte: kleine Ecken-Totzone, sofortiger Wechsel
    pub const RECOMMENDED: Self = Self { corner_size: 8, delay: Duration::ZERO };
    /// Jede Kantenberührung wechselt sofort
    pub const IMMEDIATE: Self = Self { corner_size: 0, delay: Duration::ZERO };
}

impl Default for SwitchOptions {
    fn default() -> Self {
        Self::RECOMMENDED
    }
}

/// Lücke zwischen zwei Kantenberührungen, ab der ein neuer Anlauf beginnt
const PUSH_GAP: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Active {
    Local,
    Remote { name: String, x: i32, y: i32 },
}

pub struct ServerState {
    me: String,
    layout: Layout,
    local: Rect,
    clients: HashMap<String, Rect>,
    active: Active,
    epoch: u16,
    seq: u32,
    /// Cursor bleibt auf dem aktuellen Bildschirm (Rollen-Taste), z. B. für Spiele
    locked: bool,
    /// Laufende Drag-Prüfung an dieser Kantenposition
    probe: Option<(Edge, i32, i32)>,
    /// Dateien werden gerade auf den aktiven Client gezogen
    drag: Option<u64>,
    next_transfer: u64,
    options: SwitchOptions,
    /// Gegen welche Kante seit wann (erste, letzte Berührung) gedrückt wird
    push: Option<(Edge, Instant, Instant)>,
    /// Umbelegte Tasten je Rechner
    key_maps: HashMap<String, KeyMap>,
    /// Auf dem aktiven Client gedrückte Maustasten (Bit je `MouseButton`)
    remote_buttons: u8,
    /// Nachfrage beim Client läuft: (Nummer, Kante, Position)
    remote_probe: Option<(u32, Edge, i32, i32)>,
    next_probe: u32,
    /// Ohne Dateien erst wieder nachfragen, wenn alle Tasten losgelassen wurden
    remote_probe_failed: bool,
    /// Dateien vom Client warten auf das Ablegen hier
    incoming: Option<u64>,
}

impl ServerState {
    pub fn new(me: impl Into<String>, layout: Layout, local: Rect) -> Self {
        Self {
            me: me.into(),
            layout,
            local,
            clients: HashMap::new(),
            active: Active::Local,
            epoch: 0,
            seq: 0,
            locked: false,
            probe: None,
            drag: None,
            next_transfer: 1,
            options: SwitchOptions::IMMEDIATE,
            push: None,
            key_maps: HashMap::new(),
            remote_buttons: 0,
            remote_probe: None,
            next_probe: 1,
            remote_probe_failed: false,
            incoming: None,
        }
    }

    /// Tasten je Rechner umbelegen (Name des Rechners → gedrückte Taste → gesendete Taste).
    pub fn set_key_maps(&mut self, maps: HashMap<String, KeyMap>) {
        self.key_maps = maps;
    }

    pub fn set_switch_options(&mut self, options: SwitchOptions) {
        self.options = options;
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Name des Bildschirms, auf dem sich der Cursor gerade befindet
    pub fn active_screen(&self) -> &str {
        match &self.active {
            Active::Local => &self.me,
            Active::Remote { name, .. } => name,
        }
    }

    pub fn handle(&mut self, event: ServerEvent) -> Vec<Action> {
        self.handle_at(event, Instant::now())
    }

    /// Wie `handle`, mit vorgegebener Uhrzeit (für Tests der Wechselverzögerung).
    pub fn handle_at(&mut self, event: ServerEvent, now: Instant) -> Vec<Action> {
        let mut out = Vec::new();
        match event {
            ServerEvent::Capture(c) => self.on_capture(c, now, &mut out),
            ServerEvent::ClientConnected { name, screen } => {
                self.clients.insert(name, screen);
            }
            ServerEvent::ClientScreenChanged { name, screen } => {
                if let Some(rect) = self.clients.get_mut(&name) {
                    *rect = screen;
                }
                if let Active::Remote { name: active, x, y } = &mut self.active {
                    if *active == name {
                        (*x, *y) = screen.clamp(*x, *y);
                    }
                }
            }
            ServerEvent::ClientDisconnected { name } => {
                self.clients.remove(&name);
                if matches!(&self.active, Active::Remote { name: active, .. } if *active == name) {
                    self.go_local(self.local.center(), &mut out);
                }
            }
            ServerEvent::LocalScreenChanged(rect) => self.local = rect,
            ServerEvent::RemoteDragOffer { client, probe, transfer, has_files } => {
                self.on_remote_offer(client, probe, transfer, has_files, &mut out)
            }
        }
        out
    }

    fn on_remote_offer(&mut self, client: String, probe: u32, transfer: u64, has_files: bool, out: &mut Vec<Action>) {
        let Some((pending, edge, x, y)) = self.remote_probe else { return };
        let current = matches!(&self.active, Active::Remote { name, .. } if *name == client);
        if pending != probe || !current {
            return;
        }
        self.remote_probe = None;
        if !has_files {
            // Z. B. ein Fenster oder markierter Text: mit gedrückter Taste nicht wechseln
            self.remote_probe_failed = true;
            return;
        }
        let Some(&rect) = self.clients.get(&client) else { return };
        // Der Client hat seinen Drag schon abgebrochen; `Leave` lässt nur noch die Taste los.
        out.push(Action::Send { to: client.clone(), input: Input::Leave });
        self.go_local(map_across(rect, edge, x, y, self.local), out);
        self.incoming = Some(transfer);
        out.push(Action::AwaitLocalDrop { from: client, transfer });
    }

    /// Darf über `edge` an Position (x, y) von `rect` gewechselt werden?
    fn may_switch(&mut self, rect: Rect, edge: Edge, x: i32, y: i32, now: Instant) -> bool {
        let corner = self.options.corner_size as i32;
        let in_corner = corner > 0
            && match edge {
                Edge::Left | Edge::Right => y < rect.y + corner || y > rect.bottom() - corner,
                Edge::Top | Edge::Bottom => x < rect.x + corner || x > rect.right() - corner,
            };
        if in_corner {
            self.push = None;
            return false;
        }
        if self.options.delay.is_zero() {
            return true;
        }
        match self.push {
            Some((e, since, last)) if e == edge && now.duration_since(last) < PUSH_GAP => {
                self.push = Some((e, since, now));
                now.duration_since(since) >= self.options.delay
            }
            _ => {
                self.push = Some((edge, now, now));
                false
            }
        }
    }

    fn on_capture(&mut self, event: CaptureEvent, now: Instant, out: &mut Vec<Action>) {
        match event {
            CaptureEvent::EdgeHit { edge, x, y, buttons_down } => {
                if self.active != Active::Local || self.locked {
                    return;
                }
                let Some(target) = self.layout.neighbor(&self.me, edge).map(str::to_owned) else {
                    return;
                };
                let Some(&rect) = self.clients.get(&target) else { return };
                if !self.may_switch(self.local, edge, x, y, now) {
                    return;
                }
                if buttons_down {
                    // Mit gedrückter Taste nur wechseln, wenn Dateien gezogen werden
                    // (nicht z. B. beim Verschieben eines Fensters an den Rand).
                    if self.probe.is_none() {
                        self.probe = Some((edge, x, y));
                        out.push(Action::ProbeDrag { x, y });
                    }
                    return;
                }
                let (nx, ny) = map_across(self.local, edge, x, y, rect);
                self.enter(target, nx, ny, out);
            }
            CaptureEvent::DragProbeResult { files } => {
                let Some((edge, x, y)) = self.probe.take() else { return };
                if files.is_empty() || self.active != Active::Local {
                    return;
                }
                let Some(target) = self.layout.neighbor(&self.me, edge).map(str::to_owned) else {
                    return;
                };
                let Some(&rect) = self.clients.get(&target) else { return };
                let (nx, ny) = map_across(self.local, edge, x, y, rect);
                out.push(Action::CancelLocalDrag);
                self.enter(target.clone(), nx, ny, out);
                let transfer = self.next_transfer;
                self.next_transfer += 1;
                self.drag = Some(transfer);
                out.push(Action::StartFileDrag { to: target, transfer, files });
            }
            CaptureEvent::MouseDelta { dx, dy } => self.on_delta(dx, dy, now, out),
            CaptureEvent::Button { button, down } => {
                if let Active::Remote { name, x, y } = &self.active {
                    if self.drag.is_none() {
                        let bit = 1 << button_bit(button);
                        if down {
                            self.remote_buttons |= bit;
                        } else {
                            self.remote_buttons &= !bit;
                        }
                        if self.remote_buttons == 0 {
                            self.remote_probe_failed = false;
                        }
                    }
                    let input = match self.drag {
                        // Die Taste, mit der gezogen wird, wurde nie an den Client geschickt:
                        // Loslassen bedeutet hier „ablegen“.
                        Some(transfer) if !down => {
                            self.drag = None;
                            Input::Drop { transfer, x: *x, y: *y }
                        }
                        Some(_) => return,
                        None => Input::Button { button, down, x: *x, y: *y },
                    };
                    out.push(Action::Send { to: name.clone(), input });
                }
            }
            CaptureEvent::Wheel { dx, dy } => self.send_remote(Input::Wheel { dx, dy }, out),
            CaptureEvent::Key { key, down } => {
                if let Active::Remote { name, .. } = &self.active {
                    let key = self.key_maps.get(name).and_then(|map| map.get(&key)).copied().unwrap_or(key);
                    out.push(Action::Send { to: name.clone(), input: Input::Key { key, down } });
                }
            }
            CaptureEvent::PanicReturn => {
                self.locked = false;
                if let Active::Remote { name, .. } = &self.active {
                    out.push(Action::Send { to: name.clone(), input: Input::Leave });
                    self.go_local(self.local.center(), out);
                }
            }
            CaptureEvent::ToggleLock => self.locked = !self.locked,
            CaptureEvent::LocalDrop { x, y } => {
                if let (Some(transfer), Active::Local) = (self.incoming.take(), &self.active) {
                    out.push(Action::DropIncoming { transfer, x, y });
                }
            }
        }
    }

    fn send_remote(&self, input: Input, out: &mut Vec<Action>) {
        if let Active::Remote { name, .. } = &self.active {
            out.push(Action::Send { to: name.clone(), input });
        }
    }

    fn on_delta(&mut self, dx: i32, dy: i32, now: Instant, out: &mut Vec<Action>) {
        let (name, x, y) = match &self.active {
            Active::Remote { name, x, y } => (name.clone(), *x, *y),
            Active::Local => return,
        };
        let Some(&rect) = self.clients.get(&name) else { return };
        let (nx, ny) = (x.saturating_add(dx), y.saturating_add(dy));

        let crossing = if self.locked {
            None
        } else if nx < rect.x {
            Some(Edge::Left)
        } else if nx > rect.right() {
            Some(Edge::Right)
        } else if ny < rect.y {
            Some(Edge::Top)
        } else if ny > rect.bottom() {
            Some(Edge::Bottom)
        } else {
            None
        };
        let (cx, cy) = rect.clamp(nx, ny);

        let crossing = crossing.filter(|&edge| {
            let available =
                self.layout.neighbor(&name, edge).is_some_and(|t| t == self.me || self.clients.contains_key(t));
            available && self.may_switch(rect, edge, cx, cy, now)
        });
        if let Some(edge) = crossing {
            if let Some(target) = self.layout.neighbor(&name, edge).map(str::to_owned) {
                if self.remote_buttons != 0 {
                    // Mit gedrückter Taste nur wechseln, wenn der Client Dateien zum Server zieht.
                    if target == self.me && self.remote_probe.is_none() && !self.remote_probe_failed {
                        let probe = self.next_probe;
                        self.next_probe = self.next_probe.wrapping_add(1);
                        self.remote_probe = Some((probe, edge, cx, cy));
                        out.push(Action::QueryRemoteDrag { to: name.clone(), probe });
                    }
                } else if target == self.me {
                    out.push(Action::Send { to: name, input: Input::Leave });
                    self.go_local(map_across(rect, edge, cx, cy, self.local), out);
                    return;
                } else if let Some(&target_rect) = self.clients.get(&target) {
                    let (tx, ty) = map_across(rect, edge, cx, cy, target_rect);
                    self.enter(target, tx, ty, out);
                    return;
                }
            }
        }

        if (cx, cy) == (x, y) {
            return;
        }
        self.active = Active::Remote { name: name.clone(), x: cx, y: cy };
        self.seq = self.seq.wrapping_add(1);
        out.push(Action::Move { to: name, mv: MouseMove { epoch: self.epoch, seq: self.seq, x: cx, y: cy } });
    }

    fn enter(&mut self, target: String, x: i32, y: i32, out: &mut Vec<Action>) {
        // Ein Drag endet, sobald er den Client ohne Ablegen verlässt (der Client verwirft ihn).
        self.drag = None;
        self.reset_remote_drag(out);
        match &self.active {
            Active::Local => out.push(Action::CaptureRemote),
            Active::Remote { name, .. } => out.push(Action::Send { to: name.clone(), input: Input::Leave }),
        }
        self.epoch = self.epoch.wrapping_add(1);
        out.push(Action::Send { to: target.clone(), input: Input::Enter { epoch: self.epoch, x, y } });
        out.push(Action::SyncClipboard { to: target.clone() });
        self.active = Active::Remote { name: target, x, y };
    }

    fn go_local(&mut self, (x, y): (i32, i32), out: &mut Vec<Action>) {
        self.active = Active::Local;
        self.drag = None;
        self.reset_remote_drag(out);
        out.push(Action::ReleaseCapture { x, y });
    }

    /// Neuer Bildschirm: Tastenzustand und Nachfragen des alten Clients gelten nicht mehr;
    /// noch nicht abgelegte Dateien vom Client werden verworfen.
    fn reset_remote_drag(&mut self, out: &mut Vec<Action>) {
        self.remote_buttons = 0;
        self.remote_probe = None;
        self.remote_probe_failed = false;
        if let Some(transfer) = self.incoming.take() {
            out.push(Action::CancelIncoming { transfer });
        }
    }
}

fn button_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
    }
}

/// Position beim Übertritt über `edge` von `from` nach `to`: auf der gegenüberliegenden
/// Kante, einen Pixel nach innen, entlang der Kante proportional skaliert.
pub fn map_across(from: Rect, edge: Edge, x: i32, y: i32, to: Rect) -> (i32, i32) {
    match edge {
        Edge::Left => (to.right() - 1, scale(y, from.y, from.height, to.y, to.height)),
        Edge::Right => (to.x + 1, scale(y, from.y, from.height, to.y, to.height)),
        Edge::Top => (scale(x, from.x, from.width, to.x, to.width), to.bottom() - 1),
        Edge::Bottom => (scale(x, from.x, from.width, to.x, to.width), to.y + 1),
    }
}

fn scale(v: i32, from_start: i32, from_len: u32, to_start: i32, to_len: u32) -> i32 {
    if from_len <= 1 || to_len <= 1 {
        return to_start;
    }
    let rel = (v - from_start).clamp(0, from_len as i32 - 1) as i64;
    to_start + (rel * (to_len as i64 - 1) / (from_len as i64 - 1)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LinkConfig;
    use qross_proto::{KeyCode, MouseButton};
    use std::path::PathBuf;

    const PC: Rect = Rect::new(0, 0, 1920, 1080);
    const LAPTOP: Rect = Rect::new(0, 0, 1280, 800);

    fn state() -> ServerState {
        let layout = Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() }])
            .unwrap();
        let mut s = ServerState::new("pc", layout, PC);
        s.handle(ServerEvent::ClientConnected { name: "laptop".into(), screen: LAPTOP });
        s
    }

    fn edge_hit(edge: Edge, x: i32, y: i32) -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::EdgeHit { edge, x, y, buttons_down: false })
    }

    fn delta(dx: i32, dy: i32) -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::MouseDelta { dx, dy })
    }

    #[test]
    fn edge_hit_enters_neighbor_proportionally() {
        let mut s = state();
        let actions = s.handle(edge_hit(Edge::Right, 1919, 540));
        assert_eq!(
            actions,
            vec![
                Action::CaptureRemote,
                Action::Send { to: "laptop".into(), input: Input::Enter { epoch: 1, x: 1, y: 399 } },
                Action::SyncClipboard { to: "laptop".into() },
            ]
        );
        assert_eq!(s.active_screen(), "laptop");
    }

    #[test]
    fn edges_without_connected_neighbor_are_ignored() {
        let mut s = state();
        assert!(s.handle(edge_hit(Edge::Left, 0, 10)).is_empty());
        s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() });
        assert!(s.handle(edge_hit(Edge::Right, 1919, 10)).is_empty());
    }

    fn held_edge_hit() -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: true })
    }

    fn probe_result(files: &[&str]) -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::DragProbeResult { files: files.iter().map(PathBuf::from).collect() })
    }

    #[test]
    fn held_button_without_files_does_not_switch() {
        let mut s = state();
        assert_eq!(s.handle(held_edge_hit()), vec![Action::ProbeDrag { x: 1919, y: 540 }]);
        assert!(s.handle(held_edge_hit()).is_empty(), "nur eine Prüfung gleichzeitig");
        assert!(s.handle(probe_result(&[])).is_empty());
        assert_eq!(s.active_screen(), "pc");
        // Nach dem Ergebnis darf erneut geprüft werden
        assert_eq!(s.handle(held_edge_hit()), vec![Action::ProbeDrag { x: 1919, y: 540 }]);
    }

    #[test]
    fn dragging_files_switches_and_drops_on_release() {
        let mut s = state();
        s.handle(held_edge_hit());
        let actions = s.handle(probe_result(&["C:\\daten\\video.mp4"]));
        assert_eq!(
            actions,
            vec![
                Action::CancelLocalDrag,
                Action::CaptureRemote,
                Action::Send { to: "laptop".into(), input: Input::Enter { epoch: 1, x: 1, y: 399 } },
                Action::SyncClipboard { to: "laptop".into() },
                Action::StartFileDrag {
                    to: "laptop".into(),
                    transfer: 1,
                    files: vec![PathBuf::from("C:\\daten\\video.mp4")]
                },
            ]
        );
        s.handle(delta(100, 0));
        let release = ServerEvent::Capture(CaptureEvent::Button { button: MouseButton::Left, down: false });
        assert_eq!(
            s.handle(release.clone()),
            vec![Action::Send { to: "laptop".into(), input: Input::Drop { transfer: 1, x: 101, y: 399 } }]
        );
        // Danach sind Klicks wieder normale Klicks
        assert_eq!(
            s.handle(release),
            vec![Action::Send {
                to: "laptop".into(),
                input: Input::Button { button: MouseButton::Left, down: false, x: 101, y: 399 }
            }]
        );
    }

    #[test]
    fn leaving_during_drag_cancels_it() {
        let mut s = state();
        s.handle(held_edge_hit());
        s.handle(probe_result(&["a.txt"]));
        s.handle(delta(-50, 0));
        assert_eq!(s.active_screen(), "pc");
        s.handle(edge_hit(Edge::Right, 1919, 0));
        let release = ServerEvent::Capture(CaptureEvent::Button { button: MouseButton::Left, down: false });
        assert!(matches!(&s.handle(release)[0], Action::Send { input: Input::Button { .. }, .. }));
    }

    #[test]
    fn deltas_become_clamped_datagrams() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 0));
        assert_eq!(
            s.handle(delta(10, -5)),
            vec![Action::Move { to: "laptop".into(), mv: MouseMove { epoch: 1, seq: 1, x: 11, y: 0 } }]
        );
        // Unten gibt es keinen Nachbarn: klemmen statt wechseln
        let actions = s.handle(delta(0, 5000));
        assert_eq!(
            actions,
            vec![Action::Move { to: "laptop".into(), mv: MouseMove { epoch: 1, seq: 2, x: 11, y: 799 } }]
        );
        // Keine Bewegung mehr möglich → nichts senden
        assert!(s.handle(delta(0, 10)).is_empty());
    }

    #[test]
    fn crossing_back_releases_capture() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 1079));
        let actions = s.handle(delta(-50, 0));
        assert_eq!(
            actions,
            vec![
                Action::Send { to: "laptop".into(), input: Input::Leave },
                Action::ReleaseCapture { x: 1918, y: 1079 },
            ]
        );
        assert_eq!(s.active_screen(), "pc");
    }

    fn button(down: bool) -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::Button { button: MouseButton::Left, down })
    }

    fn offer(probe: u32, has_files: bool) -> ServerEvent {
        ServerEvent::RemoteDragOffer { client: "laptop".into(), probe, transfer: 7, has_files }
    }

    /// Auf dem Laptop (rechts vom PC) mit gedrückter Taste nach links an die Kante zum PC
    fn drag_on_laptop_to_pc(s: &mut ServerState) -> Vec<Action> {
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s.handle(button(true));
        s.handle(delta(-100, 0))
    }

    #[test]
    fn files_dragged_on_the_client_come_back_to_the_server() {
        let mut s = state();
        let actions = drag_on_laptop_to_pc(&mut s);
        assert_eq!(actions[0], Action::QueryRemoteDrag { to: "laptop".into(), probe: 1 }, "erst nachfragen");
        assert!(!actions.iter().any(|a| matches!(a, Action::ReleaseCapture { .. })), "{actions:?}");
        assert_eq!(s.active_screen(), "laptop");
        assert!(s.handle(delta(-10, 0)).is_empty(), "keine zweite Nachfrage");

        assert!(s.handle(offer(99, true)).is_empty(), "veraltete Antwort");
        let actions = s.handle(offer(1, true));
        assert_eq!(actions[0], Action::Send { to: "laptop".into(), input: Input::Leave });
        assert!(matches!(actions[1], Action::ReleaseCapture { x: 1918, .. }), "{actions:?}");
        assert_eq!(actions[2], Action::AwaitLocalDrop { from: "laptop".into(), transfer: 7 });
        assert_eq!(s.active_screen(), "pc");
        let drop = ServerEvent::Capture(CaptureEvent::LocalDrop { x: 500, y: 300 });
        assert_eq!(s.handle(drop.clone()), vec![Action::DropIncoming { transfer: 7, x: 500, y: 300 }]);
        assert!(s.handle(drop).is_empty(), "nur einmal ablegen");
    }

    #[test]
    fn held_button_without_files_stays_on_the_client_until_released() {
        let mut s = state();
        drag_on_laptop_to_pc(&mut s);
        assert!(s.handle(offer(1, false)).is_empty());
        assert!(s.handle(delta(-10, 0)).is_empty(), "z. B. Fenster an den Rand geschoben: kein Wechsel, keine Nachfrage");
        assert_eq!(s.active_screen(), "laptop");

        s.handle(button(false));
        let actions = s.handle(delta(-10, 0));
        assert!(actions.contains(&Action::Send { to: "laptop".into(), input: Input::Leave }), "{actions:?}");
        assert_eq!(s.active_screen(), "pc");
    }

    #[test]
    fn going_back_before_dropping_discards_the_files() {
        let mut s = state();
        drag_on_laptop_to_pc(&mut s);
        s.handle(offer(1, true));
        let actions = s.handle(edge_hit(Edge::Right, 1919, 540));
        assert!(actions.contains(&Action::CancelIncoming { transfer: 7 }), "{actions:?}");
        assert!(s.handle(ServerEvent::Capture(CaptureEvent::LocalDrop { x: 1, y: 1 })).is_empty());
    }

    #[test]
    fn keys_are_remapped_per_client() {
        let mut s = state();
        let alt = KeyCode::LEFT_ALT;
        let meta = KeyCode(0xE3);
        s.set_key_maps(HashMap::from([("laptop".to_owned(), HashMap::from([(alt, meta)]))]));
        s.handle(edge_hit(Edge::Right, 1919, 0));
        let send = |s: &mut ServerState, key| s.handle(ServerEvent::Capture(CaptureEvent::Key { key, down: true }));
        assert_eq!(send(&mut s, alt), vec![Action::Send { to: "laptop".into(), input: Input::Key { key: meta, down: true } }]);
        assert_eq!(
            send(&mut s, KeyCode::A),
            vec![Action::Send { to: "laptop".into(), input: Input::Key { key: KeyCode::A, down: true } }],
            "nicht umbelegte Tasten bleiben"
        );
    }

    #[test]
    fn input_goes_to_active_client_only() {
        let mut s = state();
        let key = ServerEvent::Capture(CaptureEvent::Key { key: KeyCode::A, down: true });
        assert!(s.handle(key.clone()).is_empty());

        s.handle(edge_hit(Edge::Right, 1919, 0));
        assert_eq!(
            s.handle(key),
            vec![Action::Send { to: "laptop".into(), input: Input::Key { key: KeyCode::A, down: true } }]
        );
        let click = ServerEvent::Capture(CaptureEvent::Button { button: MouseButton::Left, down: true });
        assert_eq!(
            s.handle(click),
            vec![Action::Send {
                to: "laptop".into(),
                input: Input::Button { button: MouseButton::Left, down: true, x: 1, y: 0 }
            }]
        );
    }

    #[test]
    fn disconnect_and_panic_return_to_local() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 0));
        assert_eq!(
            s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() }),
            vec![Action::ReleaseCapture { x: 960, y: 540 }]
        );

        s.handle(ServerEvent::ClientConnected { name: "laptop".into(), screen: LAPTOP });
        s.handle(edge_hit(Edge::Right, 1919, 0));
        assert_eq!(
            s.handle(ServerEvent::Capture(CaptureEvent::PanicReturn)),
            vec![
                Action::Send { to: "laptop".into(), input: Input::Leave },
                Action::ReleaseCapture { x: 960, y: 540 },
            ]
        );
    }

    #[test]
    fn lock_keeps_cursor_on_current_screen() {
        let lock = ServerEvent::Capture(CaptureEvent::ToggleLock);
        let mut s = state();
        s.handle(lock.clone());
        assert!(s.is_locked());
        assert!(s.handle(edge_hit(Edge::Right, 1919, 0)).is_empty(), "gesperrt: kein Wechsel");

        s.handle(lock.clone());
        s.handle(edge_hit(Edge::Right, 1919, 0));
        s.handle(lock);
        // Gesperrt auf dem Laptop: die linke Kante klemmt, statt zurückzuwechseln
        assert_eq!(
            s.handle(delta(-500, 0)),
            vec![Action::Move { to: "laptop".into(), mv: MouseMove { epoch: 1, seq: 1, x: 0, y: 0 } }]
        );
        assert_eq!(s.active_screen(), "laptop");
        // Der Notausgang hebt die Sperre auf
        s.handle(ServerEvent::Capture(CaptureEvent::PanicReturn));
        assert!(!s.is_locked());
    }

    #[test]
    fn corners_do_not_switch() {
        let mut s = state();
        s.set_switch_options(SwitchOptions { corner_size: 10, delay: Duration::ZERO });
        assert!(s.handle(edge_hit(Edge::Right, 1919, 5)).is_empty(), "obere Ecke");
        assert!(s.handle(edge_hit(Edge::Right, 1919, 1075)).is_empty(), "untere Ecke");
        assert!(!s.handle(edge_hit(Edge::Right, 1919, 10)).is_empty(), "knapp außerhalb der Ecke");
        // Auch beim Zurückwechseln vom Client
        let actions = s.handle(delta(-50, -9999));
        assert!(matches!(actions.as_slice(), [Action::Move { .. }]), "Ecke links oben klemmt: {actions:?}");
        assert_eq!(s.active_screen(), "laptop");
    }

    #[test]
    fn switch_delay_requires_pushing() {
        let mut s = state();
        s.set_switch_options(SwitchOptions { corner_size: 0, delay: Duration::from_millis(200) });
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!(s.handle_at(edge_hit(Edge::Right, 1919, 500), at(0)).is_empty());
        assert!(s.handle_at(edge_hit(Edge::Right, 1919, 500), at(100)).is_empty());
        // Pause > PUSH_GAP: neuer Anlauf
        assert!(s.handle_at(edge_hit(Edge::Right, 1919, 500), at(400)).is_empty());
        assert!(s.handle_at(edge_hit(Edge::Right, 1919, 500), at(500)).is_empty());
        assert!(!s.handle_at(edge_hit(Edge::Right, 1919, 500), at(610)).is_empty(), "200 ms gedrückt");
        assert_eq!(s.active_screen(), "laptop");
    }

    #[test]
    fn each_visit_gets_a_new_epoch() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 0));
        s.handle(delta(-5, 0));
        let actions = s.handle(edge_hit(Edge::Right, 1919, 0));
        assert!(actions.contains(&Action::Send { to: "laptop".into(), input: Input::Enter { epoch: 2, x: 1, y: 0 } }));
    }

    #[test]
    fn map_across_scales_along_the_edge() {
        let from = Rect::new(-1920, 0, 1920, 1080);
        assert_eq!(map_across(from, Edge::Bottom, -1, 1079, LAPTOP), (1279, 1));
        assert_eq!(map_across(from, Edge::Top, -1920, 0, LAPTOP), (0, 798));
        assert_eq!(map_across(PC, Edge::Left, 0, 540, LAPTOP), (1278, 399));
    }
}
