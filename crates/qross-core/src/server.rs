//! Zustandsmaschine des Servers: Wo ist der Cursor gerade, und was muss wohin?

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use qross_proto::{Edge, GameState, Input, KeyCode, Lock, LockState, MouseButton, MouseDelta, MouseMove, Rect};

use crate::chars::{KeyChars, Typist};
use crate::input::CaptureEvent;
use crate::keys::{KeyMap, KeyMode, SwitchTarget};
use crate::layout::Layout;
use crate::screen::{beyond, Screen};
use crate::sharing::SharingRules;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Capture(CaptureEvent),
    ClientConnected { name: String, screen: Rect },
    ClientScreenChanged { name: String, screen: Rect },
    /// Die einzelnen Monitore eines Clients (seit Qross 0.5). Ohne diese Meldung gilt seine
    /// Fläche als ein einziger Monitor.
    ClientMonitors { name: String, monitors: Vec<Rect> },
    ClientDisconnected { name: String },
    LocalScreenChanged(Rect),
    /// Antwort des Clients auf `Action::QueryRemoteDrag`: Werden dort Dateien gezogen?
    RemoteDragOffer { client: String, probe: u32, transfer: u64, has_files: bool },
    /// Die eigenen Monitore des Servers
    LocalMonitors(Vec<Rect>),
    /// Spielemodus: Was im Vordergrund des Servers selbst läuft
    LocalGame(GameState),
    /// Spielemodus: Was im Vordergrund eines Clients läuft (seit Qross 0.6)
    ClientGame { name: String, game: GameState },
    /// Der Client kann Zeichen eingeben (`Control::TypesText`, seit Qross 0.7)
    ClientTypesText { name: String },
    /// Auf dem Server (macOS) ist die „Sichere Tastatureingabe“ an bzw. wieder aus: Tasten erreichen
    /// die Erfassung dann nicht, sondern das Programm im Vordergrund des Servers. Der Cursor kommt
    /// deshalb zurück und bleibt auf dem Server, bis sie endet.
    LocalSecureInput(bool),
    /// Feststelltaste, Num und Rollen des Servers beim Start: welche Qross hier lesen kann und wie
    /// sie stehen. Danach meldet die Erfassung Änderungen mit `CaptureEvent::Locks`.
    LocalLocks(LockState),
    /// Der Client kann seine Sperrtasten auf den Stand des Servers bringen (`Control::SyncsLocks`,
    /// seit Qross 0.8)
    ClientSyncsLocks { name: String },
    /// Der Bildschirmschoner des Servers hat begonnen bzw. geendet (nur, wenn er abgeglichen wird)
    LocalScreenSaver(bool),
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
    /// Als Datagramm senden: relative Bewegung statt Position (Spielemodus)
    MoveBy { to: String, delta: MouseDelta },
    /// Nur melden: Wegen der sicheren Tastatureingabe bleibt der Cursor auf dem Server. `from`: von
    /// diesem Client zurückgeholt; `None`: ein Wechsel wurde verhindert (einmal, solange sie an ist)
    HeldBySecureInput { from: Option<String> },
    /// Feststelltaste, Num und Rollen des Servers jetzt lesen und an diesen Client schicken
    /// (`Input::Locks`), gleich hinter dem, was schon unterwegs ist
    SyncLocks { to: String },
    /// Allen Clients, die ihm folgen können, den Bildschirmschoner des Servers melden: Sie starten
    /// bzw. beenden ihren eigenen; mit `lock` sperren sie sich beim Start stattdessen.
    ScreenSaver { active: bool, lock: bool },
    /// Den Bildschirmschoner des Servers beenden: Jemand ist zurück, aber seine Eingaben gehen gerade
    /// an einen Client und erreichen den Bildschirmschoner hier nicht.
    StopLocalScreenSaver,
}

/// Warum der Cursor gerade auf seinem Bildschirm bleibt
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// Er darf wechseln
    Free,
    /// Fang-Taste
    Locked,
    /// Spielemodus: Auf dem Bildschirm läuft ein Vollbildprogramm oder ein Spiel hat den Zeiger
    /// eingefangen. Die Fang-Taste löst den Halt, bis der Cursor den Bildschirm wechselt.
    Game(GameState),
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
    local: Screen,
    clients: HashMap<String, Screen>,
    active: Active,
    epoch: u16,
    seq: u32,
    /// Cursor bleibt auf dem aktuellen Bildschirm (Rollen-Taste), z. B. für Spiele
    locked: bool,
    /// Spielemodus: bei Vollbild oder eingefangenem Zeiger auf dem aktiven Bildschirm festhalten
    /// und Clients dort relative Bewegungen schicken
    game_mode: bool,
    /// Was im Vordergrund des Servers bzw. der Clients läuft
    local_game: GameState,
    games: HashMap<String, GameState>,
    /// Die Fang-Taste hat den Halt des Spielemodus gelöst: bis der Cursor den Bildschirm wechselt
    /// oder dort ein Vollbild beginnt bzw. endet
    game_released: bool,
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
    /// Tastenposition oder Zeichen je Rechner (fehlt: Tastenposition)
    key_modes: HashMap<String, KeyMode>,
    /// Verbundene Clients, die Zeichen eingeben können
    typing: HashSet<String>,
    /// Zeichenmodus für den Client mit dem Cursor; `None` auf dem Server und bei Tastenposition
    typist: Option<Typist>,
    /// Auf dem aktiven Client gedrückte Maustasten (Bit je `MouseButton`)
    remote_buttons: u8,
    /// Nachfrage beim Client läuft: (Nummer, Kante, Position)
    remote_probe: Option<(u32, Edge, i32, i32)>,
    next_probe: u32,
    /// Ohne Dateien erst wieder nachfragen, wenn alle Tasten losgelassen wurden
    remote_probe_failed: bool,
    /// Dateien vom Client warten auf das Ablegen hier
    incoming: Option<u64>,
    /// Rechte je Rechner; hier zählt nur Drag & Drop (Zwischenablage und Dateien prüft die Netzschicht)
    sharing: SharingRules,
    /// Sichere Tastatureingabe auf dem Server: Der Cursor bleibt dort
    secure_input: bool,
    /// Seit sie an ist, wurde schon ein Wechsel verhindert und gemeldet
    secure_refused: bool,
    /// Zuletzt bekannter Stand der Sperrtasten des Servers; `None` je Taste: hier nicht lesbar
    locks: LockState,
    /// Verbundene Clients, die ihre Sperrtasten auf den Stand des Servers bringen können
    syncs_locks: HashSet<String>,
    /// Clients sperren sich, wenn der Bildschirmschoner des Servers beginnt
    lock_clients: bool,
    /// Der Bildschirmschoner des Servers läuft (nur, wenn er abgeglichen wird)
    saver_running: bool,
    /// Ihn zu beenden ist schon veranlasst
    saver_waking: bool,
}

impl ServerState {
    pub fn new(me: impl Into<String>, layout: Layout, local: Rect) -> Self {
        Self {
            me: me.into(),
            layout,
            local: Screen::new(local),
            clients: HashMap::new(),
            active: Active::Local,
            epoch: 0,
            seq: 0,
            locked: false,
            game_mode: true,
            local_game: GameState::default(),
            games: HashMap::new(),
            game_released: false,
            probe: None,
            drag: None,
            next_transfer: 1,
            options: SwitchOptions::IMMEDIATE,
            push: None,
            key_maps: HashMap::new(),
            key_modes: HashMap::new(),
            typing: HashSet::new(),
            typist: None,
            remote_buttons: 0,
            remote_probe: None,
            next_probe: 1,
            remote_probe_failed: false,
            incoming: None,
            sharing: SharingRules::default(),
            secure_input: false,
            secure_refused: false,
            locks: LockState::default(),
            syncs_locks: HashSet::new(),
            lock_clients: false,
            saver_running: false,
            saver_waking: false,
        }
    }

    /// Bildschirmschoner abgleichen: Clients sperren sich, sobald er beginnt (Standard: nein, sie
    /// starten nur ihren eigenen).
    pub fn set_lock_clients(&mut self, lock: bool) {
        self.lock_clients = lock;
    }

    /// Rechte je Rechner (Standard: alles erlaubt). Gilt ab dem nächsten Ereignis: Wohin Drag &
    /// Drop nicht darf, wird mit gedrückter Taste gar nicht erst gewechselt.
    pub fn set_sharing(&mut self, sharing: SharingRules) {
        self.sharing = sharing;
    }

    /// Tasten je Rechner umbelegen (Name des Rechners → gedrückte Taste → gesendete Taste).
    pub fn set_key_maps(&mut self, maps: HashMap<String, KeyMap>) {
        self.key_maps = maps;
    }

    /// Tastenposition oder Zeichen je Rechner (fehlt einer: Tastenposition). Zeichen bekommt ein
    /// Client nur, wenn er sie auch eingeben kann (`ServerEvent::ClientTypesText`).
    pub fn set_key_modes(&mut self, modes: HashMap<String, KeyMode>) {
        self.key_modes = modes;
    }

    /// Was `name` gerade bekommt: Zeichen nur, wenn eingestellt und der Client sie eingeben kann
    pub fn key_mode(&self, name: &str) -> KeyMode {
        match self.key_modes.get(name) {
            Some(KeyMode::Chars) if self.typing.contains(name) => KeyMode::Chars,
            _ => KeyMode::Position,
        }
    }

    pub fn set_switch_options(&mut self, options: SwitchOptions) {
        self.options = options;
    }

    /// Spielemodus ein- oder ausschalten (Standard: an).
    pub fn set_game_mode(&mut self, enabled: bool) {
        self.game_mode = enabled;
    }

    /// Per Fang-Taste festgehalten?
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// Warum der Cursor gerade nicht wechselt (die Fang-Taste geht dem Spielemodus vor)
    pub fn hold(&self) -> Hold {
        if self.locked {
            Hold::Locked
        } else {
            self.game_hold().map_or(Hold::Free, Hold::Game)
        }
    }

    fn held(&self) -> bool {
        self.locked || self.game_hold().is_some()
    }

    /// Hält der Spielemodus den Cursor fest? Dann mit dem Zustand des aktiven Bildschirms.
    /// Steht er dabei auf einem Client, bekommt der relative Bewegungen.
    fn game_hold(&self) -> Option<GameState> {
        let game = match &self.active {
            Active::Local => self.local_game,
            Active::Remote { name, .. } => self.games.get(name).copied().unwrap_or_default(),
        };
        (self.game_mode && game.is_active() && !self.game_released).then_some(game)
    }

    /// Neuer Spielemodus-Zustand von `screen` (`None`: der Server selbst).
    fn set_game(&mut self, screen: Option<String>, game: GameState) {
        let (old, on_it) = match &screen {
            None => (self.local_game, self.active == Active::Local),
            Some(name) => (
                self.games.get(name).copied().unwrap_or_default(),
                matches!(&self.active, Active::Remote { name: active, .. } if active == name),
            ),
        };
        // Beginnt oder endet dort, wo der Cursor ist, ein Vollbild, gilt wieder der Spielemodus.
        if on_it && old.is_active() != game.is_active() {
            self.game_released = false;
        }
        match screen {
            None => self.local_game = game,
            Some(name) => {
                self.games.insert(name, game);
            }
        }
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
            ServerEvent::Capture(c) => {
                self.wake_screen_saver(&mut out);
                self.on_capture(c, now, &mut out)
            }
            ServerEvent::ClientConnected { name, screen } => {
                self.clients.insert(name, Screen::new(screen));
            }
            ServerEvent::ClientScreenChanged { name, screen } => {
                if let Some(s) = self.clients.get_mut(&name) {
                    s.set_bounds(screen);
                }
                self.keep_cursor_on_monitor(&name);
            }
            ServerEvent::ClientMonitors { name, monitors } => {
                if let Some(s) = self.clients.get_mut(&name) {
                    s.set_monitors(monitors);
                }
                self.keep_cursor_on_monitor(&name);
            }
            ServerEvent::ClientDisconnected { name } => {
                self.clients.remove(&name);
                self.games.remove(&name);
                self.typing.remove(&name);
                self.syncs_locks.remove(&name);
                if matches!(&self.active, Active::Remote { name: active, .. } if *active == name) {
                    self.go_local(self.local.center(), &mut out);
                }
            }
            ServerEvent::LocalScreenChanged(rect) => self.local.set_bounds(rect),
            ServerEvent::LocalMonitors(monitors) => self.local.set_monitors(monitors),
            ServerEvent::RemoteDragOffer { client, probe, transfer, has_files } => {
                self.on_remote_offer(client, probe, transfer, has_files, &mut out)
            }
            ServerEvent::LocalGame(game) => self.set_game(None, game),
            ServerEvent::ClientGame { name, game } => {
                if self.clients.contains_key(&name) {
                    self.set_game(Some(name), game);
                }
            }
            ServerEvent::ClientTypesText { name } => {
                if self.clients.contains_key(&name) {
                    self.typing.insert(name);
                    // Die Meldung kam erst, als der Cursor schon dort war
                    if self.typist.is_none() && self.key_mode(self.active_screen()) == KeyMode::Chars {
                        self.typist = Some(Typist::new());
                    }
                }
            }
            ServerEvent::LocalSecureInput(on) => self.set_secure_input(on, &mut out),
            ServerEvent::LocalLocks(locks) => self.locks = locks,
            ServerEvent::ClientSyncsLocks { name } => {
                if self.clients.contains_key(&name) {
                    // Die Meldung kam erst, als der Cursor schon dort war
                    if matches!(&self.active, Active::Remote { name: active, .. } if *active == name) {
                        out.push(Action::SyncLocks { to: name.clone() });
                    }
                    self.syncs_locks.insert(name);
                }
            }
            ServerEvent::LocalScreenSaver(active) => self.on_screen_saver(active, &mut out),
        }
        out
    }

    /// Sichere Tastatureingabe auf dem Server an oder aus. Steht der Cursor dabei auf einem Client,
    /// kommt er zurück – auch gegen die Fang-Taste und den Spielemodus: Was dort getippt würde,
    /// landete sonst unbemerkt im Programm im Vordergrund des Servers, schlimmstenfalls in einem
    /// Terminal oder Chat.
    fn set_secure_input(&mut self, on: bool, out: &mut Vec<Action>) {
        if on == self.secure_input {
            return;
        }
        self.secure_input = on;
        self.secure_refused = false;
        if !on {
            return;
        }
        // Eine laufende Drag-Prüfung an der Kante wechselt nicht mehr
        self.probe = None;
        self.push = None;
        if let Active::Remote { name, .. } = &self.active {
            let from = name.clone();
            self.locked = false;
            out.push(Action::Send { to: from.clone(), input: Input::Leave });
            self.go_local(self.local.center(), out);
            out.push(Action::HeldBySecureInput { from: Some(from) });
        }
    }

    /// Ein Wechsel weg vom Server unterbleibt wegen der sicheren Tastatureingabe; einmal melden,
    /// solange sie an ist.
    fn refuse_for_secure_input(&mut self, out: &mut Vec<Action>) {
        if !self.secure_refused {
            self.secure_refused = true;
            out.push(Action::HeldBySecureInput { from: None });
        }
    }

    /// Bekommt der Client mit dem Cursor den Stand der Sperrtasten statt ihrer Tastendrücke?
    fn syncs_locks_on(&self, name: &str) -> bool {
        self.syncs_locks.contains(name)
    }

    /// Bildschirmschoner des Servers: Beginnt er, starten die Clients ihren, endet er, beenden sie
    /// ihn. Der Cursor bleibt dabei, wo er ist; jede Bewegung des Zeigers würde ihn sofort beenden.
    fn on_screen_saver(&mut self, active: bool, out: &mut Vec<Action>) {
        self.saver_running = active;
        self.saver_waking = false;
        out.push(Action::ScreenSaver { active, lock: active && self.lock_clients });
    }

    /// Eine Eingabe am Server, während der Bildschirmschoner läuft: Steht der Cursor auf einem
    /// Client, fängt der Server sie ab, und sie erreicht den Bildschirmschoner nicht. Dann beendet
    /// der Server ihn selbst (einmal; sein Ende melden die Clients wie sonst auch).
    fn wake_screen_saver(&mut self, out: &mut Vec<Action>) {
        if self.saver_running && !self.saver_waking && self.active != Active::Local {
            self.saver_waking = true;
            out.push(Action::StopLocalScreenSaver);
        }
    }

    fn on_remote_offer(&mut self, client: String, probe: u32, transfer: u64, has_files: bool, out: &mut Vec<Action>) {
        let Some((pending, edge, x, y)) = self.remote_probe else { return };
        let current = matches!(&self.active, Active::Remote { name, .. } if *name == client);
        if pending != probe || !current {
            return;
        }
        self.remote_probe = None;
        // Ohne Dateien (z. B. ein Fenster oder markierter Text) oder ohne Rechte, sie abzugeben:
        // mit gedrückter Taste nicht wechseln
        if !has_files || !self.sharing.for_client(&client).drag.from_client() {
            self.remote_probe_failed = true;
            return;
        }
        let Some(screen) = self.clients.get(&client) else { return };
        let rect = screen.hull();
        // Der Client hat seinen Drag schon abgebrochen; `Leave` lässt nur noch die Taste los.
        out.push(Action::Send { to: client.clone(), input: Input::Leave });
        let point = enter_point(rect, edge, x, y, &self.local);
        self.go_local(point, out);
        self.incoming = Some(transfer);
        out.push(Action::AwaitLocalDrop { from: client, transfer });
    }

    /// Nach einer Änderung der Fläche von `name`: steht der Cursor dort, auf einen Monitor holen.
    fn keep_cursor_on_monitor(&mut self, name: &str) {
        if let (Active::Remote { name: active, x, y }, Some(screen)) = (&mut self.active, self.clients.get(name)) {
            if active == name {
                (*x, *y) = screen.snap(*x, *y);
            }
        }
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
                if self.active != Active::Local || self.held() {
                    return;
                }
                // Auf den Monitor: Bei schneller Bewegung meldet die Erfassung womöglich, wohin der
                // Cursor ohne Rand käme, also einen Punkt hinter der Kante (Windows). Er zählte sonst
                // nicht als äußere Kante, und gewechselt würde erst, wenn die Hand abbremst.
                let (x, y) = self.local.snap(x, y);
                let Some(target) = self.layout.neighbor(&self.me, edge).map(str::to_owned) else {
                    return;
                };
                if !self.clients.contains_key(&target) {
                    return;
                }
                // Das System meldet jede Monitorkante ohne Nachbarmonitor; gewechselt wird nur
                // dort, wo ein Monitor an der äußeren Kante liegt.
                if self.local.knows_monitors() && !self.local.on_outer_edge(edge, x, y) {
                    return;
                }
                if !self.may_switch(self.local.monitor_at(x, y), edge, x, y, now) {
                    return;
                }
                if self.secure_input {
                    return self.refuse_for_secure_input(out);
                }
                if buttons_down {
                    // Mit gedrückter Taste nur wechseln, wenn Dateien gezogen werden
                    // (nicht z. B. beim Verschieben eines Fensters an den Rand) und der Client
                    // sie bekommen darf.
                    if self.probe.is_none() && self.sharing.for_client(&target).drag.to_client() {
                        self.probe = Some((edge, x, y));
                        out.push(Action::ProbeDrag { x, y });
                    }
                    return;
                }
                let (nx, ny) = enter_point(self.local.hull(), edge, x, y, &self.clients[&target]);
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
                let Some(screen) = self.clients.get(&target) else { return };
                // Rechte inzwischen entzogen: Der Drag bleibt hier.
                if !self.sharing.for_client(&target).drag.to_client() {
                    return;
                }
                let (nx, ny) = enter_point(self.local.hull(), edge, x, y, screen);
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
                    // Zeichenmodus: zurückgehaltene Zusatztasten für Umschalt+Klick, Strg+Klick
                    let modifiers = self.typist.as_mut().map(Typist::flush).unwrap_or_default();
                    for input in modifiers.into_iter().chain([input]) {
                        out.push(Action::Send { to: name.clone(), input });
                    }
                }
            }
            CaptureEvent::Wheel { dx, dy } => {
                // Zeichenmodus: Strg+Rad zoomt
                for input in self.typist.as_mut().map(Typist::flush).unwrap_or_default() {
                    self.send_remote(input, out);
                }
                self.send_remote(Input::Wheel { dx, dy }, out)
            }
            CaptureEvent::SwitchTo(target) => self.switch_to(target, out),
            CaptureEvent::Key { key, down } => self.on_key(key, down, None, out),
            CaptureEvent::Typed { key, chars } => self.on_key(key, true, Some(&chars), out),
            CaptureEvent::PanicReturn => {
                self.locked = false;
                if let Active::Remote { name, .. } = &self.active {
                    out.push(Action::Send { to: name.clone(), input: Input::Leave });
                    self.go_local(self.local.center(), out);
                }
            }
            CaptureEvent::ToggleLock => {
                if self.held() {
                    // Löst beides: die Fang-Taste und den Halt des Spielemodus
                    self.game_released = self.game_hold().is_some();
                    self.locked = false;
                } else {
                    self.locked = true;
                    self.game_released = false;
                }
            }
            CaptureEvent::LocalDrop { x, y } => {
                if let (Some(transfer), Active::Local) = (self.incoming.take(), &self.active) {
                    out.push(Action::DropIncoming { transfer, x, y });
                }
            }
            CaptureEvent::Locks(locks) => {
                self.locks = locks;
                if let Active::Remote { name, .. } = &self.active {
                    if self.syncs_locks_on(name) {
                        out.push(Action::Send { to: name.clone(), input: Input::Locks(locks) });
                    }
                }
            }
        }
    }

    /// Wechsel per Kürzel: zu einem Rechner (Eintritt in der Mitte) oder zum Nachbarn in einer
    /// Richtung (Eintritt an der gegenüberliegenden Kante, auf Höhe des Cursors; vom Server aus,
    /// dessen Cursorposition der Kern nicht kennt, in der Mitte der Kante). Ist der Cursor
    /// festgehalten oder das Ziel nicht verbunden, passiert nichts.
    fn switch_to(&mut self, target: SwitchTarget, out: &mut Vec<Action>) {
        if self.locked {
            return;
        }
        let (from, rect, x, y) = match &self.active {
            Active::Local => {
                let (x, y) = self.local.center();
                (self.me.clone(), self.local.hull(), x, y)
            }
            Active::Remote { name, x, y } => {
                let Some(screen) = self.clients.get(name) else { return };
                (name.clone(), screen.hull(), *x, *y)
            }
        };
        let (name, edge) = match target {
            SwitchTarget::Screen(name) => (name, None),
            SwitchTarget::Direction(edge) => match self.layout.neighbor(&from, edge) {
                Some(name) => (name.to_owned(), Some(edge)),
                None => return,
            },
        };
        if name == from {
            return;
        }
        if self.secure_input && name != self.me {
            if self.clients.contains_key(&name) {
                self.refuse_for_secure_input(out);
            }
            return;
        }
        let Some(screen) = (if name == self.me { Some(&self.local) } else { self.clients.get(&name) }) else {
            return;
        };
        let (tx, ty) = match edge {
            Some(edge) => enter_point(rect, edge, x, y, screen),
            None => screen.center(),
        };
        if name == self.me {
            self.send_remote(Input::Leave, out);
            self.go_local((tx, ty), out);
        } else {
            self.enter(name, tx, ty, out);
        }
    }

    /// Taste an den Client mit dem Cursor: umbelegt, im Zeichenmodus als Text oder Kürzel
    fn on_key(&mut self, key: KeyCode, down: bool, chars: Option<&KeyChars>, out: &mut Vec<Action>) {
        let Active::Remote { name, .. } = &self.active else { return };
        let remap = self.key_maps.get(name).and_then(|map| map.get(&key)).copied();
        // Feststelltaste & Co.: Der Server schaltet mit und schickt seinen neuen Stand
        // (`CaptureEvent::Locks`). Ein Tastendruck schaltete den Client ein zweites Mal um. Kennt der
        // Server den Stand nicht oder ist die Taste umbelegt, geht sie wie jede andere.
        let synced = Lock::of_key(key).is_some_and(|lock| self.locks.get(lock).is_some());
        if synced && remap.is_none() && self.syncs_locks_on(name) {
            return;
        }
        let inputs = match &mut self.typist {
            Some(typist) if down => typist.press(key, remap, chars),
            Some(typist) => typist.release(key, remap.unwrap_or(key)),
            None => vec![Input::Key { key: remap.unwrap_or(key), down }],
        };
        out.extend(inputs.into_iter().map(|input| Action::Send { to: name.clone(), input }));
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
        let Some(screen) = self.clients.get(&name) else { return };
        let rect = screen.hull();
        let (nx, ny) = (x.saturating_add(dx), y.saturating_add(dy));
        // Nur auf Monitorflächen bewegen (ohne Monitorliste: im Rechteck klemmen)
        let (cx, cy) = screen.step((x, y), (nx, ny));

        if self.game_hold().is_some() {
            // Spielemodus: die Bewegung selbst schicken, damit ein Spiel mit eingefangenem Zeiger
            // auch am Rand des Bildschirms weiterdreht. Die Position läuft geklemmt mit, für Klicks
            // und für die Zeit danach. Nur Clients, die `Control::Game` senden, kommen hierher.
            self.active = Active::Remote { name: name.clone(), x: cx, y: cy };
            if (dx, dy) != (0, 0) {
                self.seq = self.seq.wrapping_add(1);
                let delta = MouseDelta { epoch: self.epoch, seq: self.seq, dx, dy };
                out.push(Action::MoveBy { to: name, delta });
            }
            return;
        }

        // Wechsel über die äußere Kante, und nur dort, wo ein Monitor an ihr liegt
        let crossing = if self.locked {
            None
        } else {
            Edge::ALL.into_iter().find(|&edge| beyond(rect, edge, nx, ny) && screen.on_outer_edge(edge, cx, cy))
        };
        let monitor = screen.monitor_at(cx, cy);

        let crossing = crossing.filter(|&edge| {
            let available =
                self.layout.neighbor(&name, edge).is_some_and(|t| t == self.me || self.clients.contains_key(t));
            available && self.may_switch(monitor, edge, cx, cy, now)
        });
        if let Some(edge) = crossing {
            if let Some(target) = self.layout.neighbor(&name, edge).map(str::to_owned) {
                if self.remote_buttons != 0 {
                    // Mit gedrückter Taste nur wechseln, wenn der Client Dateien zum Server zieht
                    // und sie abgeben darf.
                    if target == self.me
                        && self.remote_probe.is_none()
                        && !self.remote_probe_failed
                        && self.sharing.for_client(&name).drag.from_client()
                    {
                        let probe = self.next_probe;
                        self.next_probe = self.next_probe.wrapping_add(1);
                        self.remote_probe = Some((probe, edge, cx, cy));
                        out.push(Action::QueryRemoteDrag { to: name.clone(), probe });
                    }
                } else if target == self.me {
                    out.push(Action::Send { to: name, input: Input::Leave });
                    self.go_local(enter_point(rect, edge, cx, cy, &self.local), out);
                    return;
                } else if let Some(target_screen) = self.clients.get(&target) {
                    let (tx, ty) = enter_point(rect, edge, cx, cy, target_screen);
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
        self.game_released = false;
        self.typist = (self.key_mode(&target) == KeyMode::Chars).then(Typist::new);
        out.push(Action::Send { to: target.clone(), input: Input::Enter { epoch: self.epoch, x, y } });
        if self.syncs_locks_on(&target) {
            out.push(Action::SyncLocks { to: target.clone() });
        }
        out.push(Action::SyncClipboard { to: target.clone() });
        self.active = Active::Remote { name: target, x, y };
    }

    fn go_local(&mut self, (x, y): (i32, i32), out: &mut Vec<Action>) {
        self.active = Active::Local;
        self.game_released = false;
        self.typist = None;
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

/// Position auf `to` beim Übertritt über `edge` der Fläche `from`: anteilig entlang der Kante
/// umgerechnet, dann auf den nächstgelegenen Monitor an der Eintrittskante gelegt.
fn enter_point(from: Rect, edge: Edge, x: i32, y: i32, to: &Screen) -> (i32, i32) {
    let (tx, ty) = map_across(from, edge, x, y, to.hull());
    to.entry_point(edge.opposite(), tx, ty)
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
    use crate::sharing::{Share, Sharing};
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
    fn a_fast_push_reported_behind_the_edge_switches_at_once() {
        // Bei schneller Bewegung meldet der Windows-Hook, wohin der Cursor ohne Rand käme: hinter die
        // Kante. Das zählt wie die Kante selbst, sonst hinge der Cursor dort, bis die Hand abbremst.
        let mut s = state();
        s.handle(ServerEvent::LocalMonitors(vec![PC]));
        assert!(s.handle(edge_hit(Edge::Right, 1949, 540)).contains(&Action::Send {
            to: "laptop".into(),
            input: Input::Enter { epoch: 1, x: 1, y: 399 }
        }));
        // Auch schräg über die Ecke hinaus: Die Höhe bleibt auf dem Monitor.
        let mut s = state();
        s.handle(ServerEvent::LocalMonitors(vec![PC]));
        assert!(s.handle(edge_hit(Edge::Right, 1990, 1100)).contains(&Action::Send {
            to: "laptop".into(),
            input: Input::Enter { epoch: 1, x: 1, y: 799 }
        }));
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

    /// Rechte nur für „laptop“, alles andere wie gehabt
    fn laptop_may(drag: Share) -> SharingRules {
        let laptop = Sharing { drag, ..Sharing::ALL };
        SharingRules { default: Sharing::ALL, clients: [("laptop".to_owned(), laptop)].into() }
    }

    #[test]
    fn without_drag_rights_to_the_client_a_held_button_never_switches() {
        let mut s = state();
        s.set_sharing(laptop_may(Share::FromClient));
        assert!(s.handle(held_edge_hit()).is_empty(), "nicht einmal nachsehen, ob Dateien gezogen werden");
        assert_eq!(s.active_screen(), "pc");
        // Ohne gedrückte Taste wechselt der Cursor wie immer
        assert!(s.handle(edge_hit(Edge::Right, 1919, 540)).contains(&Action::CaptureRemote));

        // Rechte entzogen, während die Prüfung lief: Der Drag bleibt auf dem Server.
        let mut s = state();
        assert_eq!(s.handle(held_edge_hit()), vec![Action::ProbeDrag { x: 1919, y: 540 }]);
        s.set_sharing(laptop_may(Share::Off));
        assert!(s.handle(probe_result(&["a.txt"])).is_empty());
        assert_eq!(s.active_screen(), "pc");
    }

    #[test]
    fn without_drag_rights_from_the_client_a_held_button_stays_there() {
        let mut s = state();
        s.set_sharing(laptop_may(Share::ToClient));
        let actions = drag_on_laptop_to_pc(&mut s);
        let switching = |a: &Action| matches!(a, Action::QueryRemoteDrag { .. } | Action::ReleaseCapture { .. });
        assert!(!actions.iter().any(switching), "keine Nachfrage, kein Wechsel: {actions:?}");
        assert_eq!(s.active_screen(), "laptop");
        // Eine Antwort, um die niemand gebeten hat, ändert nichts.
        assert!(s.handle(offer(1, true)).is_empty());
        assert_eq!(s.active_screen(), "laptop");

        s.handle(button(false));
        assert!(s.handle(delta(-10, 0)).contains(&Action::Send { to: "laptop".into(), input: Input::Leave }));
        assert_eq!(s.active_screen(), "pc");

        // Rechte entzogen, während die Nachfrage lief: Die Dateien bleiben drüben.
        let mut s = state();
        assert_eq!(drag_on_laptop_to_pc(&mut s)[0], Action::QueryRemoteDrag { to: "laptop".into(), probe: 1 });
        s.set_sharing(laptop_may(Share::Off));
        assert!(s.handle(offer(1, true)).is_empty());
        assert!(s.handle(delta(-10, 0)).is_empty(), "solange die Taste gedrückt ist");
        assert_eq!(s.active_screen(), "laptop");
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

    const KEY_Q: KeyCode = KeyCode(0x14);
    const SHIFT: KeyCode = KeyCode::LEFT_SHIFT;

    /// Cursor auf dem Laptop, der auf `mode` steht und (`can_type`) Zeichen eingeben kann
    fn on_laptop(mode: KeyMode, can_type: bool) -> ServerState {
        let mut s = state();
        s.set_key_modes(HashMap::from([("laptop".to_owned(), mode)]));
        if can_type {
            s.handle(ServerEvent::ClientTypesText { name: "laptop".into() });
        }
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s
    }

    fn to_laptop(inputs: Vec<Input>) -> Vec<Action> {
        inputs.into_iter().map(|input| Action::Send { to: "laptop".into(), input }).collect()
    }

    fn typed_at() -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::Typed { key: KEY_Q, chars: KeyChars::text("@", 'q') })
    }

    fn key(key: KeyCode, down: bool) -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::Key { key, down })
    }

    #[test]
    fn characters_go_only_to_clients_set_to_and_able_to_type_them() {
        let mut s = on_laptop(KeyMode::Chars, true);
        assert_eq!(s.key_mode("laptop"), KeyMode::Chars);
        assert_eq!(s.handle(typed_at()), to_laptop(vec![Input::Text { key: KEY_Q, text: "@".into() }]));
        assert_eq!(s.handle(key(KEY_Q, false)), to_laptop(vec![Input::Key { key: KEY_Q, down: false }]));

        // Auf Tastenposition, oder er kann es nicht (älteres Qross, Wayland): die Taste wie bisher
        for mut s in [on_laptop(KeyMode::Position, true), on_laptop(KeyMode::Chars, false)] {
            assert_eq!(s.key_mode("laptop"), KeyMode::Position);
            assert_eq!(s.handle(key(SHIFT, true)), to_laptop(vec![Input::Key { key: SHIFT, down: true }]));
            assert_eq!(s.handle(typed_at()), to_laptop(vec![Input::Key { key: KEY_Q, down: true }]));
        }
        // Auf dem Server selbst geht nichts raus
        let mut s = on_laptop(KeyMode::Chars, true);
        s.handle(ServerEvent::Capture(CaptureEvent::PanicReturn));
        assert!(s.handle(typed_at()).is_empty());
    }

    #[test]
    fn held_modifiers_reach_the_client_before_clicks_and_wheels() {
        let mut s = on_laptop(KeyMode::Chars, true);
        assert!(s.handle(key(SHIFT, true)).is_empty(), "Umschalt wartet");
        let click = ServerEvent::Capture(CaptureEvent::Button { button: MouseButton::Left, down: true });
        assert_eq!(
            s.handle(click),
            to_laptop(vec![
                Input::Key { key: SHIFT, down: true },
                Input::Button { button: MouseButton::Left, down: true, x: 1, y: 399 },
            ])
        );
        let wheel = ServerEvent::Capture(CaptureEvent::Wheel { dx: 0, dy: 120 });
        assert_eq!(s.handle(wheel), to_laptop(vec![Input::Wheel { dx: 0, dy: 120 }]));
        assert_eq!(s.handle(key(SHIFT, false)), to_laptop(vec![Input::Key { key: SHIFT, down: false }]));
    }

    #[test]
    fn the_ability_to_type_counts_from_its_report_until_disconnecting() {
        // Die Meldung kommt erst, als der Cursor schon auf dem Laptop ist
        let mut s = on_laptop(KeyMode::Chars, false);
        s.handle(ServerEvent::ClientTypesText { name: "laptop".into() });
        assert_eq!(s.handle(typed_at()), to_laptop(vec![Input::Text { key: KEY_Q, text: "@".into() }]));
        // Neu verbunden: wieder Tastenposition, bis er es erneut meldet
        s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() });
        s.handle(ServerEvent::ClientConnected { name: "laptop".into(), screen: LAPTOP });
        assert_eq!(s.key_mode("laptop"), KeyMode::Position);
        // Unbekannte Rechner melden nichts an
        s.handle(ServerEvent::ClientTypesText { name: "tablet".into() });
        assert!(!s.typing.contains("tablet"));
    }

    #[test]
    fn remapping_applies_in_character_mode_too() {
        let mut s = on_laptop(KeyMode::Chars, true);
        s.set_key_maps(HashMap::from([("laptop".to_owned(), HashMap::from([(KEY_Q, KeyCode::A)]))]));
        assert_eq!(s.handle(typed_at()), to_laptop(vec![Input::Key { key: KeyCode::A, down: true }]));
        assert_eq!(s.handle(key(KEY_Q, false)), to_laptop(vec![Input::Key { key: KeyCode::A, down: false }]));
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

    fn switch_to(target: &str) -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::SwitchTo(target.parse().unwrap()))
    }

    #[test]
    fn hotkeys_jump_to_the_middle_of_a_screen_and_back() {
        let mut s = state();
        assert_eq!(
            s.handle(switch_to("laptop")),
            vec![
                Action::CaptureRemote,
                Action::Send { to: "laptop".into(), input: Input::Enter { epoch: 1, x: 640, y: 400 } },
                Action::SyncClipboard { to: "laptop".into() },
            ]
        );
        assert_eq!(s.active_screen(), "laptop");
        assert!(s.handle(switch_to("laptop")).is_empty(), "schon dort");
        assert_eq!(
            s.handle(switch_to("pc")),
            vec![Action::Send { to: "laptop".into(), input: Input::Leave }, Action::ReleaseCapture { x: 960, y: 540 }]
        );
        assert_eq!(s.active_screen(), "pc");
    }

    #[test]
    fn direction_hotkeys_enter_at_the_opposite_edge() {
        let mut s = state();
        // Vom Server aus: Mitte der Kante
        assert!(s.handle(switch_to("right")).contains(&Action::Send {
            to: "laptop".into(),
            input: Input::Enter { epoch: 1, x: 1, y: 399 }
        }));
        // Vom Client aus: auf Höhe des Cursors
        s.handle(delta(100, -299));
        assert!(s.handle(switch_to("up")).is_empty(), "oben ist niemand");
        assert_eq!(
            s.handle(switch_to("left")),
            vec![Action::Send { to: "laptop".into(), input: Input::Leave }, Action::ReleaseCapture { x: 1918, y: 135 }]
        );
        assert!(s.handle(switch_to("left")).is_empty(), "links vom Server ist niemand");
    }

    #[test]
    fn hotkeys_do_nothing_for_absent_screens_or_a_held_cursor() {
        let mut s = state();
        assert!(s.handle(switch_to("tablet")).is_empty(), "nicht im Layout");
        assert!(s.handle(switch_to("pc")).is_empty(), "schon auf dem Server");
        s.handle(ServerEvent::Capture(CaptureEvent::ToggleLock));
        assert!(s.handle(switch_to("laptop")).is_empty(), "Fang-Taste geht vor");
        assert!(s.handle(switch_to("right")).is_empty());
        s.handle(ServerEvent::Capture(CaptureEvent::ToggleLock));
        s.handle(switch_to("laptop"));
        s.handle(ServerEvent::Capture(CaptureEvent::ToggleLock));
        assert!(s.handle(switch_to("pc")).is_empty(), "auch auf dem Client festgehalten");
        assert_eq!(s.active_screen(), "laptop");

        s.handle(ServerEvent::Capture(CaptureEvent::ToggleLock));
        s.handle(switch_to("pc"));
        s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() });
        assert!(s.handle(switch_to("laptop")).is_empty(), "nicht verbunden");
        assert!(s.handle(switch_to("right")).is_empty());
    }

    #[test]
    fn hotkeys_switch_between_clients() {
        let layout = Layout::from_links(&[
            LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() },
            LinkConfig { from: "tablet".into(), edge: Edge::Right, to: "pc".into() },
        ])
        .unwrap();
        let mut s = ServerState::new("pc", layout, PC);
        s.handle(ServerEvent::ClientConnected { name: "laptop".into(), screen: LAPTOP });
        s.handle(ServerEvent::ClientConnected { name: "tablet".into(), screen: Rect::new(0, 0, 800, 600) });
        s.handle(switch_to("laptop"));
        assert_eq!(
            s.handle(switch_to("tablet")),
            vec![
                Action::Send { to: "laptop".into(), input: Input::Leave },
                Action::Send { to: "tablet".into(), input: Input::Enter { epoch: 2, x: 400, y: 300 } },
                Action::SyncClipboard { to: "tablet".into() },
            ]
        );
        // Rechts vom Tablet liegt der Server, nicht der Laptop
        assert!(s.handle(switch_to("right")).contains(&Action::ReleaseCapture { x: 1, y: 540 }));
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

    /// MacBook links (unten bündig), daneben ein höherer externer Monitor, der weiter oben beginnt
    const MAC: Rect = Rect::new(0, -400, 5256, 1800);
    const MAC_BUILTIN: Rect = Rect::new(0, 418, 1512, 982);
    const MAC_EXTERNAL: Rect = Rect::new(1512, -400, 3744, 1800);

    fn mac_state() -> ServerState {
        let layout =
            Layout::from_links(&[LinkConfig { from: "pc".into(), edge: Edge::Right, to: "mac".into() }]).unwrap();
        let mut s = ServerState::new("pc", layout, PC);
        s.handle(ServerEvent::ClientConnected { name: "mac".into(), screen: MAC });
        s.handle(ServerEvent::ClientMonitors { name: "mac".into(), monitors: vec![MAC_BUILTIN, MAC_EXTERNAL] });
        s
    }

    fn enter_mac(epoch: u16, x: i32, y: i32) -> Action {
        Action::Send { to: "mac".into(), input: Input::Enter { epoch, x, y } }
    }

    fn move_mac(seq: u32, x: i32, y: i32) -> Action {
        Action::Move { to: "mac".into(), mv: MouseMove { epoch: 1, seq, x, y } }
    }

    #[test]
    fn entering_lands_on_the_nearest_monitor_at_the_edge() {
        let mut s = mac_state();
        // Neben dem oberen Teil des PCs liegt beim Mac links kein Monitor: Oberkante des MacBooks
        assert!(s.handle(edge_hit(Edge::Right, 1919, 100)).contains(&enter_mac(1, 1, 418)));
        s.handle(delta(-5, 0));
        assert_eq!(s.active_screen(), "pc");
        // Weiter unten anteilig wie bisher, denn dort liegt das MacBook
        assert!(s.handle(edge_hit(Edge::Right, 1919, 900)).contains(&enter_mac(2, 1, 1100)));
    }

    #[test]
    fn cursor_stays_on_the_monitors_of_a_client() {
        let mut s = mac_state();
        s.handle(edge_hit(Edge::Right, 1919, 100));
        // Hinüber auf den externen Monitor, oberhalb des MacBooks
        assert_eq!(s.handle(delta(1600, -100)), vec![move_mac(1, 1601, 318)]);
        // Nach links in die tote Fläche über dem MacBook: bleibt am Rand des externen Monitors
        assert_eq!(s.handle(delta(-200, 0)), vec![move_mac(2, 1512, 318)]);
        // Dort ist keine äußere Kante, also auch kein Wechsel zum PC
        assert!(s.handle(delta(-5000, 0)).is_empty());
        assert_eq!(s.active_screen(), "mac");
        // Hinunter aufs MacBook und über dessen linke Kante zurück
        assert_eq!(s.handle(delta(-100, 500)), vec![move_mac(3, 1412, 818)]);
        assert_eq!(
            s.handle(delta(-5000, 0)),
            vec![Action::Send { to: "mac".into(), input: Input::Leave }, Action::ReleaseCapture { x: 1918, y: 730 }]
        );
    }

    #[test]
    fn corner_dead_zone_belongs_to_each_monitor() {
        let mut s = mac_state();
        s.set_switch_options(SwitchOptions { corner_size: 10, delay: Duration::ZERO });
        s.handle(edge_hit(Edge::Right, 1919, 100));
        // Obere linke Ecke des MacBooks, weit unterhalb der Oberkante des Rechtecks
        assert_eq!(s.handle(delta(-50, 2)), vec![move_mac(1, 0, 420)]);
        assert_eq!(s.active_screen(), "mac");
        assert_eq!(
            s.handle(delta(-50, 20)),
            vec![Action::Send { to: "mac".into(), input: Input::Leave }, Action::ReleaseCapture { x: 1918, y: 503 }]
        );
    }

    #[test]
    fn server_switches_only_where_its_own_monitors_reach_the_edge() {
        let mut s = state();
        // Rechter Monitor 300 px tiefer: rechts neben dem oberen Teil des linken Monitors ist nichts
        s.handle(ServerEvent::LocalScreenChanged(Rect::new(0, 0, 3840, 1380)));
        s.handle(ServerEvent::LocalMonitors(vec![PC, Rect::new(1920, 300, 1920, 1080)]));
        assert!(s.handle(edge_hit(Edge::Right, 1919, 100)).is_empty(), "Monitorkante zur toten Fläche");
        assert!(s.handle(edge_hit(Edge::Right, 3839, 800)).contains(&Action::Send {
            to: "laptop".into(),
            input: Input::Enter { epoch: 1, x: 1, y: 463 }
        }));
        // Zurück landet der Cursor auf dem Monitor an der Kante, nicht in der toten Fläche
        assert_eq!(
            s.handle(delta(-50, -463)),
            vec![Action::Send { to: "laptop".into(), input: Input::Leave }, Action::ReleaseCapture { x: 3838, y: 300 }]
        );
    }

    #[test]
    fn changed_monitors_keep_the_cursor_on_a_monitor() {
        let mut s = mac_state();
        s.handle(edge_hit(Edge::Right, 1919, 100));
        s.handle(delta(1600, -100));
        // Externer Monitor abgezogen: macOS legt das MacBook nach (0, 0)
        s.handle(ServerEvent::ClientScreenChanged { name: "mac".into(), screen: Rect::new(0, 0, 1512, 982) });
        s.handle(ServerEvent::ClientMonitors { name: "mac".into(), monitors: vec![Rect::new(0, 0, 1512, 982)] });
        assert_eq!(s.handle(delta(-11, 0)), vec![move_mac(2, 1500, 418)]);
    }

    const FULLSCREEN: GameState = GameState { fullscreen: true, captured: false };
    const CAPTURED: GameState = GameState { fullscreen: false, captured: true };

    fn laptop_game(game: GameState) -> ServerEvent {
        ServerEvent::ClientGame { name: "laptop".into(), game }
    }

    fn move_by(seq: u32, dx: i32, dy: i32) -> Action {
        Action::MoveBy { to: "laptop".into(), delta: MouseDelta { epoch: 1, seq, dx, dy } }
    }

    #[test]
    fn fullscreen_on_the_server_keeps_the_cursor_there() {
        let mut s = state();
        s.handle(ServerEvent::LocalGame(FULLSCREEN));
        assert_eq!(s.hold(), Hold::Game(FULLSCREEN));
        assert!(s.handle(edge_hit(Edge::Right, 1919, 540)).is_empty(), "Vollbild: kein Wechsel");
        assert!(!s.is_locked(), "die Fang-Taste bleibt unberührt");

        // Das Spiel endet: wieder frei
        s.handle(ServerEvent::LocalGame(GameState::default()));
        assert_eq!(s.hold(), Hold::Free);
        assert!(!s.handle(edge_hit(Edge::Right, 1919, 540)).is_empty());
        assert_eq!(s.active_screen(), "laptop");
    }

    #[test]
    fn game_mode_can_be_switched_off() {
        let mut s = state();
        s.set_game_mode(false);
        s.handle(ServerEvent::LocalGame(CAPTURED));
        assert_eq!(s.hold(), Hold::Free);
        assert!(!s.handle(edge_hit(Edge::Right, 1919, 540)).is_empty());
        s.handle(laptop_game(FULLSCREEN));
        assert!(matches!(s.handle(delta(5, 0)).as_slice(), [Action::Move { .. }]), "absolute Positionen wie bisher");
    }

    #[test]
    fn game_on_a_client_holds_the_cursor_and_sends_deltas() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s.handle(laptop_game(CAPTURED));
        assert_eq!(s.hold(), Hold::Game(CAPTURED));
        // Weit über die linke Kante: kein Wechsel, und das Spiel bekommt die volle Bewegung
        assert_eq!(s.handle(delta(-5000, 3)), vec![move_by(1, -5000, 3)]);
        assert_eq!(s.active_screen(), "laptop");
        assert_eq!(s.handle(delta(-7, 0)), vec![move_by(2, -7, 0)], "auch am Rand geht nichts verloren");
        assert!(s.handle(delta(0, 0)).is_empty());
        // Klicks tragen die mitgeführte, geklemmte Position
        let click = ServerEvent::Capture(CaptureEvent::Button { button: MouseButton::Left, down: true });
        assert_eq!(
            s.handle(click),
            vec![Action::Send {
                to: "laptop".into(),
                input: Input::Button { button: MouseButton::Left, down: true, x: 0, y: 402 }
            }]
        );
        s.handle(ServerEvent::Capture(CaptureEvent::Button { button: MouseButton::Left, down: false }));

        // Das Spiel endet: wieder absolute Positionen, und die linke Kante führt zurück
        s.handle(laptop_game(GameState::default()));
        assert_eq!(s.hold(), Hold::Free);
        assert_eq!(s.handle(delta(5, 0)), vec![move_laptop(3, 5, 402)]);
        assert_eq!(
            s.handle(delta(-50, 0)),
            vec![Action::Send { to: "laptop".into(), input: Input::Leave }, Action::ReleaseCapture { x: 1918, y: 542 }]
        );
    }

    fn move_laptop(seq: u32, x: i32, y: i32) -> Action {
        Action::Move { to: "laptop".into(), mv: MouseMove { epoch: 1, seq, x, y } }
    }

    #[test]
    fn lock_key_releases_the_game_hold_until_the_screen_changes() {
        let lock = ServerEvent::Capture(CaptureEvent::ToggleLock);
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s.handle(laptop_game(FULLSCREEN));
        s.handle(lock.clone());
        assert_eq!(s.hold(), Hold::Free, "Fang-Taste löst den Spielemodus");
        assert!(!s.is_locked());
        // Frei: absolute Positionen, damit der sichtbare Cursor zur Kante findet
        assert!(s.handle(delta(-5000, 0)).contains(&Action::Send { to: "laptop".into(), input: Input::Leave }));
        assert_eq!(s.active_screen(), "pc");

        // Zurück auf den Laptop: Das Vollbild läuft noch, also wieder festgehalten
        s.handle(edge_hit(Edge::Right, 1919, 540));
        assert_eq!(s.hold(), Hold::Game(FULLSCREEN));
        // Ein zweiter Druck hält per Fang-Taste fest, die relativen Bewegungen bleiben
        s.handle(lock.clone());
        s.handle(lock.clone());
        assert_eq!(s.hold(), Hold::Locked);
        assert!(matches!(s.handle(delta(-9, 0)).as_slice(), [Action::MoveBy { .. }]));
        s.handle(lock);
        assert_eq!(s.hold(), Hold::Free);
        assert!(matches!(s.handle(delta(1, 0)).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn a_new_fullscreen_holds_again_after_releasing() {
        let lock = ServerEvent::Capture(CaptureEvent::ToggleLock);
        let mut s = state();
        s.handle(ServerEvent::LocalGame(FULLSCREEN));
        s.handle(lock.clone());
        assert_eq!(s.hold(), Hold::Free);
        // Wechsel innerhalb des Vollbilds (Menü → Spiel mit eingefangenem Zeiger): bleibt gelöst
        s.handle(ServerEvent::LocalGame(GameState { fullscreen: true, captured: true }));
        assert_eq!(s.hold(), Hold::Free);
        // Ende und neuer Start: wieder festgehalten
        s.handle(ServerEvent::LocalGame(GameState::default()));
        s.handle(ServerEvent::LocalGame(FULLSCREEN));
        assert_eq!(s.hold(), Hold::Game(FULLSCREEN));
        // Ohne Vollbild sperrt und löst die Fang-Taste wie bisher
        s.handle(ServerEvent::LocalGame(GameState::default()));
        s.handle(lock.clone());
        assert_eq!(s.hold(), Hold::Locked);
        s.handle(lock);
        assert_eq!(s.hold(), Hold::Free);
    }

    #[test]
    fn game_states_of_other_screens_do_not_hold() {
        let mut s = state();
        s.handle(laptop_game(FULLSCREEN));
        assert_eq!(s.hold(), Hold::Free, "Vollbild auf dem Laptop, Cursor auf dem PC");
        s.handle(ServerEvent::ClientGame { name: "unbekannt".into(), game: FULLSCREEN });
        s.handle(edge_hit(Edge::Right, 1919, 540));
        assert_eq!(s.hold(), Hold::Game(FULLSCREEN));
        // Getrennt und neu verbunden: der alte Zustand gilt nicht mehr
        s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() });
        s.handle(ServerEvent::ClientConnected { name: "laptop".into(), screen: LAPTOP });
        s.handle(edge_hit(Edge::Right, 1919, 540));
        assert_eq!(s.hold(), Hold::Free);
        assert!(matches!(s.handle(delta(1, 0)).as_slice(), [Action::Move { .. }]));
    }

    #[test]
    fn panic_return_leaves_a_game_on_the_client() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s.handle(laptop_game(CAPTURED));
        let actions = s.handle(ServerEvent::Capture(CaptureEvent::PanicReturn));
        assert!(actions.contains(&Action::ReleaseCapture { x: 960, y: 540 }), "{actions:?}");
        assert_eq!((s.active_screen(), s.hold()), ("pc", Hold::Free));
    }

    #[test]
    fn hotkeys_land_on_a_monitor() {
        let mut s = mac_state();
        // Neben der Mitte des PCs liegt das MacBook
        assert!(s.handle(switch_to("right")).contains(&enter_mac(1, 1, 500)));
        s.handle(switch_to("pc"));
        // Die Mitte der Gesamtfläche liegt auf dem externen Monitor
        assert!(s.handle(switch_to("mac")).contains(&enter_mac(2, 2628, 500)));
    }

    fn secure_input(on: bool) -> ServerEvent {
        ServerEvent::LocalSecureInput(on)
    }

    const REFUSED: Action = Action::HeldBySecureInput { from: None };

    #[test]
    fn secure_input_pulls_the_cursor_back_from_a_client() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        assert_eq!(
            s.handle(secure_input(true)),
            vec![
                Action::Send { to: "laptop".into(), input: Input::Leave },
                Action::ReleaseCapture { x: 960, y: 540 },
                Action::HeldBySecureInput { from: Some("laptop".into()) },
            ]
        );
        assert_eq!(s.active_screen(), "pc");
        // Dieselbe Meldung noch einmal ändert nichts
        assert!(s.handle(secure_input(true)).is_empty());
    }

    #[test]
    fn secure_input_keeps_the_cursor_on_the_server_until_it_ends() {
        let mut s = state();
        assert!(s.handle(secure_input(true)).is_empty(), "auf dem Server: nichts zu tun");
        // Der erste verhinderte Wechsel wird gemeldet, weitere nicht
        assert_eq!(s.handle(edge_hit(Edge::Right, 1919, 540)), vec![REFUSED]);
        assert!(s.handle(edge_hit(Edge::Right, 1919, 540)).is_empty());
        assert!(s.handle(held_edge_hit()).is_empty(), "auch nicht mit gedrückter Taste (Drag & Drop)");
        assert!(s.handle(switch_to("laptop")).is_empty());
        assert!(s.handle(switch_to("right")).is_empty());
        assert_eq!(s.active_screen(), "pc");
        // Kanten ohne Nachbarn oder ohne verbundenen Client sind kein verhinderter Wechsel
        let mut s = state();
        s.handle(secure_input(true));
        assert!(s.handle(edge_hit(Edge::Left, 0, 540)).is_empty());
        assert!(s.handle(switch_to("tablet")).is_empty());
        s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() });
        assert!(s.handle(edge_hit(Edge::Right, 1919, 540)).is_empty());

        // Vorbei: Es wechselt wieder
        let mut s = state();
        s.handle(secure_input(true));
        s.handle(edge_hit(Edge::Right, 1919, 540));
        assert!(s.handle(secure_input(false)).is_empty());
        assert!(s.handle(edge_hit(Edge::Right, 1919, 540)).contains(&Action::CaptureRemote));
        assert_eq!(s.active_screen(), "laptop");
    }

    #[test]
    fn each_secure_input_phase_reports_one_refusal() {
        let mut s = state();
        s.handle(secure_input(true));
        assert_eq!(s.handle(switch_to("laptop")), vec![REFUSED], "auch per Kürzel");
        assert!(s.handle(edge_hit(Edge::Right, 1919, 540)).is_empty());
        s.handle(secure_input(false));
        s.handle(secure_input(true));
        assert_eq!(s.handle(edge_hit(Edge::Right, 1919, 540)), vec![REFUSED]);
    }

    #[test]
    fn secure_input_refuses_only_once_the_switch_delay_has_passed() {
        let mut s = state();
        s.set_switch_options(SwitchOptions { corner_size: 0, delay: Duration::from_millis(300) });
        s.handle(secure_input(true));
        let t0 = Instant::now();
        assert!(s.handle_at(edge_hit(Edge::Right, 1919, 540), t0).is_empty(), "nur kurz an die Kante");
        let later = t0 + Duration::from_millis(100);
        assert!(s.handle_at(edge_hit(Edge::Right, 1919, 540), later).is_empty());
        let pushed = t0 + Duration::from_millis(200);
        assert!(s.handle_at(edge_hit(Edge::Right, 1919, 540), pushed).is_empty());
        let long = t0 + Duration::from_millis(320);
        assert_eq!(s.handle_at(edge_hit(Edge::Right, 1919, 540), long), vec![REFUSED]);
    }

    #[test]
    fn secure_input_overrides_lock_key_and_game_mode() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s.handle(ServerEvent::Capture(CaptureEvent::ToggleLock));
        assert!(s.is_locked());
        assert!(s.handle(secure_input(true)).contains(&Action::ReleaseCapture { x: 960, y: 540 }));
        assert!(!s.is_locked(), "zurück auf dem Server ist die Sperre gelöst");

        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s.handle(laptop_game(CAPTURED));
        assert!(s.handle(secure_input(true)).contains(&Action::ReleaseCapture { x: 960, y: 540 }));
        assert_eq!((s.active_screen(), s.hold()), ("pc", Hold::Free));
    }

    #[test]
    fn secure_input_drops_a_pending_drag_probe() {
        let mut s = state();
        assert_eq!(s.handle(held_edge_hit()), vec![Action::ProbeDrag { x: 1919, y: 540 }]);
        s.handle(secure_input(true));
        let files = vec![PathBuf::from("/tmp/a.txt")];
        assert!(s.handle(ServerEvent::Capture(CaptureEvent::DragProbeResult { files })).is_empty());
        assert_eq!(s.active_screen(), "pc");
    }

    const CAPS: KeyCode = KeyCode::CAPS_LOCK;
    const NUM: KeyCode = KeyCode::NUM_LOCK;
    /// Wie unter Windows: alle drei bekannt
    const ALL_OFF: LockState = LockState { caps: Some(false), num: Some(true), scroll: Some(false) };

    fn locks(locks: LockState) -> ServerEvent {
        ServerEvent::Capture(CaptureEvent::Locks(locks))
    }

    /// Server mit Sperrtasten `local`, „laptop“ kann sie abgleichen (`syncs`)
    fn lock_state(local: LockState, syncs: bool) -> ServerState {
        let mut s = state();
        s.handle(ServerEvent::LocalLocks(local));
        if syncs {
            s.handle(ServerEvent::ClientSyncsLocks { name: "laptop".into() });
        }
        s
    }

    #[test]
    fn a_client_that_syncs_gets_the_lock_state_instead_of_the_keys() {
        let mut s = lock_state(ALL_OFF, true);
        assert_eq!(
            s.handle(edge_hit(Edge::Right, 1919, 540)),
            vec![
                Action::CaptureRemote,
                Action::Send { to: "laptop".into(), input: Input::Enter { epoch: 1, x: 1, y: 399 } },
                Action::SyncLocks { to: "laptop".into() },
                Action::SyncClipboard { to: "laptop".into() },
            ],
            "beim Betreten übernimmt der Client den Stand des Servers"
        );
        // Feststelltaste auf dem Client: Der Server schaltet mit und schickt seinen neuen Stand,
        // die Taste selbst schaltete den Client ein zweites Mal um.
        assert!(s.handle(key(CAPS, true)).is_empty());
        let on = LockState { caps: Some(true), ..ALL_OFF };
        assert_eq!(s.handle(locks(on)), to_laptop(vec![Input::Locks(on)]));
        assert!(s.handle(key(CAPS, false)).is_empty());
        assert!(s.handle(key(NUM, true)).is_empty());
        assert!(s.handle(key(KeyCode::SCROLL_LOCK, true)).is_empty());
        assert_eq!(s.handle(key(KeyCode::A, true)), to_laptop(vec![Input::Key { key: KeyCode::A, down: true }]));

        // Zurück auf dem Server: Der hat den Stand schon; nichts zu schicken
        let back = s.handle(delta(-5000, 0));
        assert!(back.iter().any(|a| matches!(a, Action::ReleaseCapture { .. })), "{back:?}");
        assert!(!back.iter().any(|a| matches!(a, Action::SyncLocks { .. })), "{back:?}");
        assert!(s.handle(locks(ALL_OFF)).is_empty(), "lokal geht nichts an Clients");
    }

    #[test]
    fn older_clients_keep_getting_lock_keys() {
        let mut s = lock_state(ALL_OFF, false);
        let enter = s.handle(edge_hit(Edge::Right, 1919, 540));
        assert!(!enter.iter().any(|a| matches!(a, Action::SyncLocks { .. })), "{enter:?}");
        assert_eq!(s.handle(key(CAPS, true)), to_laptop(vec![Input::Key { key: CAPS, down: true }]));
        assert!(s.handle(locks(LockState { caps: Some(true), ..ALL_OFF })).is_empty(), "kann Locks nicht lesen");
        assert_eq!(s.handle(key(CAPS, false)), to_laptop(vec![Input::Key { key: CAPS, down: false }]));
    }

    #[test]
    fn locks_the_server_cannot_read_go_as_keys() {
        // Wie am Mac: nur die Feststelltaste, Num und Rollen gibt es dort nicht
        let mac = LockState { caps: Some(false), num: None, scroll: None };
        let mut s = lock_state(mac, true);
        s.handle(edge_hit(Edge::Right, 1919, 540));
        assert!(s.handle(key(CAPS, true)).is_empty());
        assert_eq!(s.handle(key(NUM, true)), to_laptop(vec![Input::Key { key: NUM, down: true }]));

        // Server ohne lesbare Sperrtasten (Wayland): alles wie bisher, auch beim Betreten
        let mut s = lock_state(LockState::default(), true);
        let enter = s.handle(edge_hit(Edge::Right, 1919, 540));
        assert!(enter.contains(&Action::SyncLocks { to: "laptop".into() }), "die Netzschicht schickt nur Bekanntes");
        assert_eq!(s.handle(key(CAPS, true)), to_laptop(vec![Input::Key { key: CAPS, down: true }]));
    }

    #[test]
    fn remapped_lock_keys_go_as_the_key_they_became() {
        let mut s = lock_state(ALL_OFF, true);
        s.set_key_maps(HashMap::from([("laptop".to_owned(), HashMap::from([(CAPS, KeyCode::ESCAPE)]))]));
        s.handle(edge_hit(Edge::Right, 1919, 540));
        assert_eq!(s.handle(key(CAPS, true)), to_laptop(vec![Input::Key { key: KeyCode::ESCAPE, down: true }]));
    }

    #[test]
    fn syncing_counts_from_its_report_until_disconnecting() {
        let mut s = lock_state(ALL_OFF, false);
        s.handle(edge_hit(Edge::Right, 1919, 540));
        // Die Meldung kam erst, als der Cursor schon dort war: gleich abgleichen
        assert_eq!(s.handle(ServerEvent::ClientSyncsLocks { name: "laptop".into() }), vec![Action::SyncLocks { to: "laptop".into() }]);
        assert!(s.handle(key(CAPS, true)).is_empty());
        assert!(s.handle(ServerEvent::ClientSyncsLocks { name: "unbekannt".into() }).is_empty());

        s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() });
        s.handle(ServerEvent::ClientConnected { name: "laptop".into(), screen: LAPTOP });
        let enter = s.handle(edge_hit(Edge::Right, 1919, 540));
        assert!(!enter.iter().any(|a| matches!(a, Action::SyncLocks { .. })), "neue Verbindung, noch keine Meldung: {enter:?}");
        assert_eq!(s.handle(key(CAPS, true)), to_laptop(vec![Input::Key { key: CAPS, down: true }]));
    }

    #[test]
    fn switching_between_clients_syncs_the_next_one() {
        let layout = Layout::from_links(&[
            LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() },
            LinkConfig { from: "laptop".into(), edge: Edge::Right, to: "tablet".into() },
        ])
        .unwrap();
        let mut s = ServerState::new("pc", layout, PC);
        s.handle(ServerEvent::LocalLocks(ALL_OFF));
        for name in ["laptop", "tablet"] {
            s.handle(ServerEvent::ClientConnected { name: name.into(), screen: LAPTOP });
        }
        s.handle(ServerEvent::ClientSyncsLocks { name: "tablet".into() });
        s.handle(switch_to("laptop"));
        assert_eq!(
            s.handle(switch_to("tablet")),
            vec![
                Action::Send { to: "laptop".into(), input: Input::Leave },
                Action::Send { to: "tablet".into(), input: Input::Enter { epoch: 2, x: 640, y: 400 } },
                Action::SyncLocks { to: "tablet".into() },
                Action::SyncClipboard { to: "tablet".into() },
            ]
        );
        let on = LockState { num: Some(false), ..ALL_OFF };
        assert_eq!(s.handle(locks(on)), vec![Action::Send { to: "tablet".into(), input: Input::Locks(on) }]);
        assert!(!s.handle(switch_to("laptop")).iter().any(|a| matches!(a, Action::SyncLocks { .. })));
    }

    fn saver(active: bool) -> ServerEvent {
        ServerEvent::LocalScreenSaver(active)
    }

    #[test]
    fn screen_saver_on_the_server_reaches_the_clients() {
        let mut s = state();
        assert_eq!(s.handle(saver(true)), vec![Action::ScreenSaver { active: true, lock: false }]);
        assert_eq!(s.handle(saver(false)), vec![Action::ScreenSaver { active: false, lock: false }]);
        assert_eq!(s.active_screen(), "pc");
        // Mitsperren gilt nur für den Beginn
        s.set_lock_clients(true);
        assert_eq!(s.handle(saver(true)), vec![Action::ScreenSaver { active: true, lock: true }]);
        assert_eq!(s.handle(saver(false)), vec![Action::ScreenSaver { active: false, lock: false }]);
    }

    #[test]
    fn screen_saver_leaves_the_cursor_where_it_is() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        // Keine Bewegung des Zeigers: Die würde ihn sofort wieder beenden.
        assert_eq!(s.handle(saver(true)), vec![Action::ScreenSaver { active: true, lock: false }]);
        assert_eq!(s.active_screen(), "laptop");
        assert_eq!(s.handle(saver(false)), vec![Action::ScreenSaver { active: false, lock: false }]);
        assert_eq!(s.active_screen(), "laptop");
    }

    #[test]
    fn input_for_a_client_ends_the_screen_saver_of_the_server() {
        let mut s = state();
        s.handle(edge_hit(Edge::Right, 1919, 540));
        s.handle(saver(true));
        // Die erste abgefangene Eingabe beendet ihn, sie selbst geht weiter an den Laptop.
        assert_eq!(s.handle(delta(5, 0)), vec![Action::StopLocalScreenSaver, move_laptop(1, 6, 399)]);
        assert_eq!(s.handle(delta(5, 0)), vec![move_laptop(2, 11, 399)], "nur einmal");
        assert_eq!(s.handle(saver(false)), vec![Action::ScreenSaver { active: false, lock: false }]);
        assert_eq!(s.handle(delta(5, 0)), vec![move_laptop(3, 16, 399)]);

        // Beginnt er erneut, gilt das wieder, auch für Tasten
        s.handle(saver(true));
        let key = ServerEvent::Capture(CaptureEvent::Key { key: KeyCode::A, down: true });
        assert_eq!(s.handle(key)[0], Action::StopLocalScreenSaver);
    }

    #[test]
    fn input_on_the_server_itself_reaches_its_screen_saver() {
        // Steht der Cursor auf dem Server, beendet die Eingabe ihn wie ohne Qross.
        let mut s = state();
        s.handle(saver(true));
        assert!(s.handle(edge_hit(Edge::Right, 1919, 540)).iter().all(|a| *a != Action::StopLocalScreenSaver));
    }

    #[test]
    fn map_across_scales_along_the_edge() {
        let from = Rect::new(-1920, 0, 1920, 1080);
        assert_eq!(map_across(from, Edge::Bottom, -1, 1079, LAPTOP), (1279, 1));
        assert_eq!(map_across(from, Edge::Top, -1920, 0, LAPTOP), (0, 798));
        assert_eq!(map_across(PC, Edge::Left, 0, 540, LAPTOP), (1278, 399));
    }
}
