//! Zustand des Clients: veraltete Datagramme verwerfen und gedrückte Tasten verfolgen,
//! damit beim Verlassen oder Verbindungsabbruch nichts „hängen“ bleibt.

use std::collections::{BTreeMap, BTreeSet};

use qross_proto::{Input, KeyCode, MouseButton, MouseDelta, MouseMove};

use crate::input::Inject;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Entered,
    Left,
}

#[derive(Debug, Default)]
pub struct ClientState {
    epoch: Option<u16>,
    last_seq: Option<u32>,
    held_keys: BTreeSet<KeyCode>,
    held_buttons: BTreeSet<MouseButton>,
    /// Zeichenmodus: Tasten des Servers, die hier als Zeichen bzw. Kürzel gedrückt sind, und wie
    /// sie wieder losgelassen werden
    typed: BTreeMap<KeyCode, Inject>,
    /// Strg ↔ Cmd tauschen (PC steuert Mac oder umgekehrt), damit Strg+C dort Kopieren bleibt
    swap_ctrl_cmd: bool,
    /// Der Server schickt gerade relative Bewegungen (Spielemodus). Klicks springen dann nicht
    /// an ihre Position: Ein Spiel mit eingefangenem Zeiger sähe darin einen Ruck.
    relative: bool,
}

/// Strg und Windows-/Cmd-Taste vertauschen (links und rechts getrennt)
pub fn swap_ctrl_cmd(key: KeyCode) -> KeyCode {
    match key.0 {
        0xE0 => KeyCode(0xE3),
        0xE3 => KeyCode(0xE0),
        0xE4 => KeyCode(0xE7),
        0xE7 => KeyCode(0xE4),
        _ => key,
    }
}

impl ClientState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_swap_ctrl_cmd(&mut self, swap: bool) {
        self.swap_ctrl_cmd = swap;
    }

    pub fn is_active(&self) -> bool {
        self.epoch.is_some()
    }

    /// Kommen gerade relative Bewegungen (Spielemodus)?
    pub fn is_relative(&self) -> bool {
        self.relative
    }

    pub fn on_input(&mut self, input: Input) -> (Vec<Inject>, Option<Transition>) {
        match input {
            Input::Enter { epoch, x, y } => {
                self.epoch = Some(epoch);
                self.last_seq = None;
                self.relative = false;
                (vec![Inject::MoveTo { x, y }], Some(Transition::Entered))
            }
            Input::Leave => {
                self.epoch = None;
                (self.release_all(), Some(Transition::Left))
            }
            _ if !self.is_active() => (Vec::new(), None),
            Input::Key { key, down: false } if self.typed.contains_key(&key) => {
                (self.typed.remove(&key).into_iter().collect(), None)
            }
            Input::Text { key, text } => (self.type_text(key, &text), None),
            Input::Shortcut { key, ch } => {
                let mut out = self.release_typed(key, Inject::Shortcut { ch, key, down: false });
                out.push(Inject::Shortcut { ch, key, down: true });
                self.typed.insert(key, Inject::Shortcut { ch, key, down: false });
                (out, None)
            }
            Input::Key { key, down } => {
                let key = if self.swap_ctrl_cmd { swap_ctrl_cmd(key) } else { key };
                if down {
                    self.held_keys.insert(key);
                } else {
                    self.held_keys.remove(&key);
                }
                (vec![Inject::Key { key, down }], None)
            }
            Input::Button { button, down, x, y } => {
                if down {
                    self.held_buttons.insert(button);
                } else {
                    self.held_buttons.remove(&button);
                }
                if self.relative {
                    (vec![Inject::Button { button, down }], None)
                } else {
                    (vec![Inject::MoveTo { x, y }, Inject::Button { button, down }], None)
                }
            }
            Input::Wheel { dx, dy } => (vec![Inject::Wheel { dx, dy }], None),
            // Dateiübertragung: übernimmt die Laufzeit; die Position kommt mit dem Drop.
            Input::DragStart { .. } => (Vec::new(), None),
            Input::Drop { x, y, .. } => (vec![Inject::MoveTo { x, y }], None),
            // Stand der Sperrtasten des Servers übernehmen (beim Verlassen bleibt er, wie er ist)
            Input::Locks(locks) => (vec![Inject::Locks(locks)], None),
        }
    }

    /// Zeichenmodus: `text` eingeben. Das letzte Zeichen bleibt gedrückt, bis die Taste des Servers
    /// losgelassen wird; eine Wiederholung drückt es erneut, wie eine gehaltene Taste.
    fn type_text(&mut self, key: KeyCode, text: &str) -> Vec<Inject> {
        let chars: Vec<char> = text.chars().collect();
        let Some((&last, first)) = chars.split_last() else { return Vec::new() };
        let release = Inject::Char { ch: last, down: false };
        let mut out = self.release_typed(key, release);
        for &ch in first {
            out.extend([Inject::Char { ch, down: true }, Inject::Char { ch, down: false }]);
        }
        out.push(Inject::Char { ch: last, down: true });
        self.typed.insert(key, release);
        out
    }

    /// Ist für `key` gerade etwas anderes als `next` gedrückt, erst das loslassen.
    fn release_typed(&mut self, key: KeyCode, next: Inject) -> Vec<Inject> {
        match self.typed.get(&key) {
            Some(&held) if held != next => self.typed.remove(&key).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Nur neuere Positionen des aktuellen Besuchs übernehmen.
    pub fn on_move(&mut self, mv: MouseMove) -> Option<Inject> {
        if !self.accept(mv.epoch, mv.seq) {
            return None;
        }
        self.relative = false;
        Some(Inject::MoveTo { x: mv.x, y: mv.y })
    }

    /// Relative Bewegung (Spielemodus). Positionen und Bewegungen zählen gemeinsam: Was älter ist
    /// als das zuletzt übernommene Datagramm, fällt weg wie ein verlorenes.
    pub fn on_delta(&mut self, delta: MouseDelta) -> Option<Inject> {
        if !self.accept(delta.epoch, delta.seq) {
            return None;
        }
        self.relative = true;
        Some(Inject::MoveBy { dx: delta.dx, dy: delta.dy })
    }

    /// Gehört das Datagramm zum aktuellen Besuch und ist es neuer als das zuletzt übernommene?
    fn accept(&mut self, epoch: u16, seq: u32) -> bool {
        if self.epoch != Some(epoch) {
            return false;
        }
        if let Some(last) = self.last_seq {
            if (seq.wrapping_sub(last) as i32) <= 0 {
                return false;
            }
        }
        self.last_seq = Some(seq);
        true
    }

    /// Alle noch gedrückten Tasten und Maustasten loslassen.
    pub fn release_all(&mut self) -> Vec<Inject> {
        let typed = std::mem::take(&mut self.typed);
        let keys = std::mem::take(&mut self.held_keys);
        let buttons = std::mem::take(&mut self.held_buttons);
        typed
            .into_values()
            .chain(keys.into_iter().rev().map(|key| Inject::Key { key, down: false }))
            .chain(buttons.into_iter().map(|button| Inject::Button { button, down: false }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qross_proto::LockState;

    fn mv(epoch: u16, seq: u32, x: i32) -> MouseMove {
        MouseMove { epoch, seq, x, y: 0 }
    }

    #[test]
    fn stale_and_foreign_moves_are_dropped() {
        let mut c = ClientState::new();
        assert_eq!(c.on_move(mv(1, 1, 5)), None, "vor Enter");
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        assert_eq!(c.on_move(mv(1, 5, 5)), Some(Inject::MoveTo { x: 5, y: 0 }));
        assert_eq!(c.on_move(mv(1, 4, 4)), None, "überholt");
        assert_eq!(c.on_move(mv(0, 9, 9)), None, "alte Epoche");
        assert_eq!(c.on_move(mv(1, 6, 6)), Some(Inject::MoveTo { x: 6, y: 0 }));
    }

    #[test]
    fn sequence_wraps_around() {
        let mut c = ClientState::new();
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        assert!(c.on_move(mv(1, u32::MAX, 1)).is_some());
        assert!(c.on_move(mv(1, 0, 2)).is_some());
    }

    #[test]
    fn leave_releases_everything_held() {
        let mut c = ClientState::new();
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        c.on_input(Input::Key { key: KeyCode::LEFT_SHIFT, down: true });
        c.on_input(Input::Key { key: KeyCode::A, down: true });
        c.on_input(Input::Key { key: KeyCode::A, down: false });
        c.on_input(Input::Button { button: MouseButton::Left, down: true, x: 3, y: 4 });

        let (injects, transition) = c.on_input(Input::Leave);
        assert_eq!(transition, Some(Transition::Left));
        assert_eq!(
            injects,
            vec![
                Inject::Key { key: KeyCode::LEFT_SHIFT, down: false },
                Inject::Button { button: MouseButton::Left, down: false },
            ]
        );
        assert!(c.release_all().is_empty());
    }

    #[test]
    fn ctrl_and_cmd_can_be_swapped() {
        let mut c = ClientState::new();
        c.set_swap_ctrl_cmd(true);
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        let (injects, _) = c.on_input(Input::Key { key: KeyCode::LEFT_CTRL, down: true });
        assert_eq!(injects, vec![Inject::Key { key: KeyCode(0xE3), down: true }]);
        // Losgelassen wird beim Verlassen die tatsächlich gedrückte (getauschte) Taste
        let (injects, _) = c.on_input(Input::Leave);
        assert_eq!(injects, vec![Inject::Key { key: KeyCode(0xE3), down: false }]);
        assert_eq!(swap_ctrl_cmd(KeyCode::A), KeyCode::A);
    }

    const KEY_Q: KeyCode = KeyCode(0x14);
    const KEY_Y: KeyCode = KeyCode(0x1C);

    #[test]
    fn typed_characters_are_held_until_their_key_is_released() {
        let mut c = ClientState::new();
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        let (injects, _) = c.on_input(Input::Text { key: KEY_Q, text: "@".into() });
        assert_eq!(injects, vec![Inject::Char { ch: '@', down: true }]);
        // Gehalten: noch einmal drücken, ohne loszulassen
        let (injects, _) = c.on_input(Input::Text { key: KEY_Q, text: "@".into() });
        assert_eq!(injects, vec![Inject::Char { ch: '@', down: true }]);
        let (injects, _) = c.on_input(Input::Key { key: KEY_Q, down: false });
        assert_eq!(injects, vec![Inject::Char { ch: '@', down: false }]);
        assert!(c.typed.is_empty());

        // Mehrere Zeichen (z. B. „^x“ nach einer toten Taste): alle bis aufs letzte tippen
        let (injects, _) = c.on_input(Input::Text { key: KEY_Q, text: "^x".into() });
        assert_eq!(
            injects,
            vec![Inject::Char { ch: '^', down: true }, Inject::Char { ch: '^', down: false }, Inject::Char { ch: 'x', down: true }]
        );
        let (injects, _) = c.on_input(Input::Leave);
        assert_eq!(injects, vec![Inject::Char { ch: 'x', down: false }], "beim Verlassen losgelassen");
    }

    #[test]
    fn shortcuts_press_the_key_for_their_character() {
        let mut c = ClientState::new();
        c.set_swap_ctrl_cmd(true);
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        let (injects, _) = c.on_input(Input::Key { key: KeyCode::LEFT_CTRL, down: true });
        assert_eq!(injects, vec![Inject::Key { key: KeyCode(0xE3), down: true }], "Strg wird am Mac zu Cmd");
        let (injects, _) = c.on_input(Input::Shortcut { key: KEY_Y, ch: 'z' });
        assert_eq!(injects, vec![Inject::Shortcut { ch: 'z', key: KEY_Y, down: true }]);
        let (injects, _) = c.on_input(Input::Key { key: KEY_Y, down: false });
        assert_eq!(injects, vec![Inject::Shortcut { ch: 'z', key: KEY_Y, down: false }]);
        // Ohne Shortcut gedrückt: eine ganz normale Taste
        let (injects, _) = c.on_input(Input::Key { key: KEY_Y, down: false });
        assert_eq!(injects, vec![Inject::Key { key: KEY_Y, down: false }]);
        let (injects, _) = c.on_input(Input::Leave);
        assert_eq!(injects, vec![Inject::Key { key: KeyCode(0xE3), down: false }]);
        let (injects, _) = c.on_input(Input::Text { key: KEY_Q, text: "a".into() });
        assert!(injects.is_empty(), "nicht aktiv");
    }

    #[test]
    fn input_is_ignored_while_inactive() {
        let mut c = ClientState::new();
        let (injects, _) = c.on_input(Input::Key { key: KeyCode::A, down: true });
        assert!(injects.is_empty());
    }

    #[test]
    fn the_lock_state_of_the_server_is_taken_over_while_active() {
        let locks = LockState { caps: Some(true), num: Some(true), scroll: None };
        let mut c = ClientState::new();
        assert!(c.on_input(Input::Locks(locks)).0.is_empty(), "nicht aktiv");
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        assert_eq!(c.on_input(Input::Locks(locks)), (vec![Inject::Locks(locks)], None));
        // Beim Verlassen bleibt der Stand: Er ist ja der des Servers
        let (injects, _) = c.on_input(Input::Leave);
        assert!(injects.is_empty(), "{injects:?}");
    }

    fn delta(epoch: u16, seq: u32, dx: i32) -> MouseDelta {
        MouseDelta { epoch, seq, dx, dy: -dx }
    }

    #[test]
    fn deltas_become_relative_moves_in_order() {
        let mut c = ClientState::new();
        assert_eq!(c.on_delta(delta(1, 1, 5)), None, "vor Enter");
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        assert!(!c.is_relative());
        assert_eq!(c.on_delta(delta(1, 2, 5)), Some(Inject::MoveBy { dx: 5, dy: -5 }));
        assert!(c.is_relative());
        assert_eq!(c.on_delta(delta(1, 1, 9)), None, "überholt");
        assert_eq!(c.on_delta(delta(0, 9, 9)), None, "alte Epoche");
        // Eine Position, die älter ist als die letzte Bewegung, kommt nicht mehr zum Zug
        assert_eq!(c.on_move(mv(1, 2, 7)), None);
        assert_eq!(c.on_move(mv(1, 3, 7)), Some(Inject::MoveTo { x: 7, y: 0 }));
        assert!(!c.is_relative(), "wieder absolute Positionen");
        assert_eq!(c.on_delta(delta(1, 4, -1)), Some(Inject::MoveBy { dx: -1, dy: 1 }));
        // Ein neuer Besuch beginnt wieder absolut
        c.on_input(Input::Enter { epoch: 2, x: 3, y: 4 });
        assert!(!c.is_relative());
    }

    #[test]
    fn clicks_stay_put_while_moving_relatively() {
        let mut c = ClientState::new();
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        c.on_delta(delta(1, 1, 3));
        let (injects, _) = c.on_input(Input::Button { button: MouseButton::Left, down: true, x: 10, y: 20 });
        assert_eq!(injects, vec![Inject::Button { button: MouseButton::Left, down: true }], "kein Sprung im Spiel");
        // Losgelassen wird die Taste trotzdem beim Verlassen
        let (injects, _) = c.on_input(Input::Leave);
        assert_eq!(injects, vec![Inject::Button { button: MouseButton::Left, down: false }]);
    }

    #[test]
    fn click_moves_to_its_position_first() {
        let mut c = ClientState::new();
        c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        let (injects, _) = c.on_input(Input::Button { button: MouseButton::Right, down: true, x: 10, y: 20 });
        assert_eq!(
            injects,
            vec![Inject::MoveTo { x: 10, y: 20 }, Inject::Button { button: MouseButton::Right, down: true }]
        );
    }
}
