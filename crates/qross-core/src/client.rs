//! Zustand des Clients: veraltete Datagramme verwerfen und gedrückte Tasten verfolgen,
//! damit beim Verlassen oder Verbindungsabbruch nichts „hängen“ bleibt.

use std::collections::BTreeSet;

use qross_proto::{Input, KeyCode, MouseButton, MouseMove};

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
    /// Strg ↔ Cmd tauschen (PC steuert Mac oder umgekehrt), damit Strg+C dort Kopieren bleibt
    swap_ctrl_cmd: bool,
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

    pub fn on_input(&mut self, input: Input) -> (Vec<Inject>, Option<Transition>) {
        match input {
            Input::Enter { epoch, x, y } => {
                self.epoch = Some(epoch);
                self.last_seq = None;
                (vec![Inject::MoveTo { x, y }], Some(Transition::Entered))
            }
            Input::Leave => {
                self.epoch = None;
                (self.release_all(), Some(Transition::Left))
            }
            _ if !self.is_active() => (Vec::new(), None),
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
                (vec![Inject::MoveTo { x, y }, Inject::Button { button, down }], None)
            }
            Input::Wheel { dx, dy } => (vec![Inject::Wheel { dx, dy }], None),
            // Dateiübertragung: übernimmt die Laufzeit; die Position kommt mit dem Drop.
            Input::DragStart { .. } => (Vec::new(), None),
            Input::Drop { x, y, .. } => (vec![Inject::MoveTo { x, y }], None),
        }
    }

    /// Nur neuere Positionen des aktuellen Besuchs übernehmen.
    pub fn on_move(&mut self, mv: MouseMove) -> Option<Inject> {
        if self.epoch != Some(mv.epoch) {
            return None;
        }
        if let Some(last) = self.last_seq {
            if (mv.seq.wrapping_sub(last) as i32) <= 0 {
                return None;
            }
        }
        self.last_seq = Some(mv.seq);
        Some(Inject::MoveTo { x: mv.x, y: mv.y })
    }

    /// Alle noch gedrückten Tasten und Maustasten loslassen.
    pub fn release_all(&mut self) -> Vec<Inject> {
        let keys = std::mem::take(&mut self.held_keys);
        let buttons = std::mem::take(&mut self.held_buttons);
        keys.into_iter()
            .rev()
            .map(|key| Inject::Key { key, down: false })
            .chain(buttons.into_iter().map(|button| Inject::Button { button, down: false }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn input_is_ignored_while_inactive() {
        let mut c = ClientState::new();
        let (injects, _) = c.on_input(Input::Key { key: KeyCode::A, down: true });
        assert!(injects.is_empty());
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
