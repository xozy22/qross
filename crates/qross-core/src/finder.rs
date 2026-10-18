//! Cursor-Finder: Wann blitzt er wo auf? Immer auf dem Rechner, auf dem der Cursor ankommt – auf
//! einem Client bei `Input::Enter`, auf dem Server, wenn der Cursor zurückkommt. Verlässt der Cursor
//! einen Rechner, verschwindet die Anzeige dort sofort. Eingestellt wird er am Server; Clients ab
//! Qross 0.8 erfahren es über `Control::CursorFinder`.

pub use qross_proto::CursorFinder;

use crate::client::Transition;
use crate::server::Action;

/// Was der Cursor-Finder auf diesem Rechner tun soll
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// Kurz aufblitzen lassen, wie eingestellt
    Show(CursorFinder),
    /// Der Cursor hat diesen Rechner verlassen: sofort ausblenden
    Hide,
}

/// Server: Was eine Aktion des Kerns für den Cursor-Finder auf dem Server selbst bedeutet. Wechselt
/// der Cursor von Client zu Client, zeigt der Server nichts, das übernimmt der Client.
pub fn on_server(action: &Action, finder: CursorFinder) -> Option<Cue> {
    match action {
        Action::ReleaseCapture { .. } => finder.is_on().then_some(Cue::Show(finder)),
        Action::CaptureRemote => Some(Cue::Hide),
        _ => None,
    }
}

/// Client: Was das Betreten bzw. Verlassen für den Cursor-Finder auf dem Client bedeutet
pub fn on_client(transition: Transition, finder: CursorFinder) -> Option<Cue> {
    match transition {
        Transition::Entered => finder.is_on().then_some(Cue::Show(finder)),
        Transition::Left => Some(Cue::Hide),
    }
}

/// Fürs Log, z. B. „Ring und Name“
pub fn describe(finder: CursorFinder) -> &'static str {
    match (finder.ring, finder.name) {
        (true, true) => "Ring und Name",
        (true, false) => "nur Ring",
        (false, true) => "nur Name",
        (false, false) => "aus",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientState;
    use crate::server::{ServerEvent, ServerState};
    use crate::{CaptureEvent, Layout, LinkConfig};
    use qross_proto::{Edge, Input, Rect};

    const BOTH: CursorFinder = CursorFinder { ring: true, name: true };
    const NAME: CursorFinder = CursorFinder { ring: false, name: true };

    fn server() -> ServerState {
        let layout = Layout::from_links(&[
            LinkConfig { from: "pc".into(), edge: Edge::Right, to: "laptop".into() },
            LinkConfig { from: "laptop".into(), edge: Edge::Right, to: "mac".into() },
        ])
        .unwrap();
        let mut s = ServerState::new("pc", layout, Rect::new(0, 0, 1920, 1080));
        for name in ["laptop", "mac"] {
            s.handle(ServerEvent::ClientConnected { name: name.into(), screen: Rect::new(0, 0, 1280, 800) });
        }
        s
    }

    /// Was der Server bei diesem Ereignis selbst zeigt
    fn cues(s: &mut ServerState, event: CaptureEvent, finder: CursorFinder) -> Vec<Cue> {
        s.handle(ServerEvent::Capture(event)).iter().filter_map(|a| on_server(a, finder)).collect()
    }

    fn delta(dx: i32) -> CaptureEvent {
        CaptureEvent::MouseDelta { dx, dy: 0 }
    }

    #[test]
    fn the_server_shows_it_only_when_the_cursor_comes_back() {
        let mut s = server();
        let hit = CaptureEvent::EdgeHit { edge: Edge::Right, x: 1919, y: 540, buttons_down: false };
        assert_eq!(cues(&mut s, hit, BOTH), [Cue::Hide], "zum Client: hier ausblenden");
        assert_eq!(cues(&mut s, delta(5000), BOTH), [], "vom Laptop zum Mac: das zeigt der Mac");
        assert_eq!(s.active_screen(), "mac");
        assert_eq!(cues(&mut s, delta(-5000), BOTH), [], "zurück zum Laptop");
        assert_eq!(cues(&mut s, delta(-5000), NAME), [Cue::Show(NAME)], "zurück auf dem Server");
        assert_eq!(s.active_screen(), "pc");

        // Auch per Kürzel, Notausgang und wenn der Client mit dem Cursor wegfällt
        let switch = |to: &str| CaptureEvent::SwitchTo(to.parse().unwrap());
        assert_eq!(cues(&mut s, switch("mac"), BOTH), [Cue::Hide]);
        assert_eq!(cues(&mut s, switch("pc"), BOTH), [Cue::Show(BOTH)]);
        cues(&mut s, switch("laptop"), BOTH);
        assert_eq!(cues(&mut s, CaptureEvent::PanicReturn, BOTH), [Cue::Show(BOTH)]);
        cues(&mut s, switch("laptop"), BOTH);
        let gone = s.handle(ServerEvent::ClientDisconnected { name: "laptop".into() });
        assert_eq!(gone.iter().filter_map(|a| on_server(a, BOTH)).collect::<Vec<_>>(), [Cue::Show(BOTH)]);
    }

    #[test]
    fn switched_off_it_never_shows() {
        let mut s = server();
        cues(&mut s, CaptureEvent::SwitchTo("laptop".parse().unwrap()), CursorFinder::OFF);
        assert_eq!(cues(&mut s, delta(-5000), CursorFinder::OFF), []);
        assert_eq!(s.active_screen(), "pc");

        let mut c = ClientState::new();
        let (_, entered) = c.on_input(Input::Enter { epoch: 1, x: 0, y: 0 });
        assert_eq!(on_client(entered.unwrap(), CursorFinder::OFF), None);
        // Ausblenden schadet nie, etwa wenn er gerade erst ausgeschaltet wurde
        let (_, left) = c.on_input(Input::Leave);
        assert_eq!(on_client(left.unwrap(), CursorFinder::OFF), Some(Cue::Hide));
    }

    #[test]
    fn a_client_shows_it_on_every_arrival() {
        let mut c = ClientState::new();
        for epoch in 1..=2 {
            let (_, entered) = c.on_input(Input::Enter { epoch, x: 1, y: 399 });
            assert_eq!(on_client(entered.unwrap(), BOTH), Some(Cue::Show(BOTH)));
            let (_, left) = c.on_input(Input::Leave);
            assert_eq!(on_client(left.unwrap(), BOTH), Some(Cue::Hide));
        }
        assert_eq!(describe(BOTH), "Ring und Name");
        assert_eq!(describe(NAME), "nur Name");
        assert_eq!(describe(CursorFinder::OFF), "aus");
    }
}
