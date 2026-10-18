//! Zustandsmaschine des Servers: Wo ist der Cursor gerade, und was muss wohin?

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use qross_proto::{Edge, Input, MouseMove, Rect};

use crate::input::CaptureEvent;
use crate::layout::Layout;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Capture(CaptureEvent),
    ClientConnected { name: String, screen: Rect },
    ClientScreenChanged { name: String, screen: Rect },
    ClientDisconnected { name: String },
    LocalScreenChanged(Rect),
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
        }
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
        }
        out
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
            CaptureEvent::Key { key, down } => self.send_remote(Input::Key { key, down }, out),
            CaptureEvent::PanicReturn => {
                self.locked = false;
                if let Active::Remote { name, .. } = &self.active {
                    out.push(Action::Send { to: name.clone(), input: Input::Leave });
                    self.go_local(self.local.center(), out);
                }
            }
            CaptureEvent::ToggleLock => self.locked = !self.locked,
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
                if target == self.me {
                    out.push(Action::Send { to: name, input: Input::Leave });
                    self.go_local(map_across(rect, edge, cx, cy, self.local), out);
                    return;
                }
                if let Some(&target_rect) = self.clients.get(&target) {
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
        out.push(Action::ReleaseCapture { x, y });
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
