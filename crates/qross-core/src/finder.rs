//! Cursor-Finder: Wann blitzt er wo auf? Immer auf dem Rechner, auf dem der Cursor ankommt – auf
//! einem Client bei `Input::Enter`, auf dem Server, wenn der Cursor zurückkommt. Verlässt der Cursor
//! einen Rechner, verschwindet die Anzeige dort sofort. Eingestellt wird er am Server; Clients ab
//! Qross 0.8 erfahren es über `Control::CursorFinder`.
//!
//! Dieselbe Anzeige zeigt beim Drag & Drop über die Kante, was gezogen wird ([`DragPreview`]): auf
//! dem Rechner, auf dem der Cursor mit den Dateien gerade ist, bis abgelegt oder abgebrochen wird.

pub use qross_proto::CursorFinder;
use qross_proto::FileEntry;

use crate::client::Transition;
use crate::lang::{ui_lang, Lang};
use crate::server::Action;
use crate::text::size_text_in;

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

/// Fürs Log, z. B. „ring and name“
pub fn describe(finder: CursorFinder) -> &'static str {
    match (finder.ring, finder.name) {
        (true, true) => "ring and name",
        (true, false) => "ring only",
        (false, true) => "name only",
        (false, false) => "off",
    }
}

/// Symbol der Drag-Vorschau
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragIcon {
    File,
    Folder,
    /// Mehrere Dateien bzw. Ordner
    Several,
}

/// Was neben dem Cursor steht, solange Dateien über die Kante gezogen werden: Symbol, Name (bei
/// mehreren „3 Dateien“) und Gesamtgröße
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragPreview {
    pub icon: DragIcon,
    pub title: String,
    /// z. B. „3,1 MB“
    pub size: String,
}

impl DragPreview {
    /// Aus den Einträgen einer Übertragung (`Input::DragStart`, `DragMessage::Offer`): Ordner
    /// stehen dort samt Inhalt, gezogen wurden die obersten Einträge. `None` ohne Einträge.
    /// Texte in der Oberflächensprache des Prozesses ([`ui_lang`]).
    pub fn of(entries: &[FileEntry]) -> Option<Self> {
        Self::of_in(entries, ui_lang())
    }

    /// Wie [`Self::of`], mit Texten in `lang`
    pub fn of_in(entries: &[FileEntry], lang: Lang) -> Option<Self> {
        // Name und ob es ein Ordner ist, in der Reihenfolge der Einträge
        let mut tops: Vec<(&str, bool)> = Vec::new();
        for entry in entries {
            let (top, inside) = match entry.path.split_once('/') {
                Some((top, _)) => (top, true),
                None => (entry.path.as_str(), false),
            };
            let folder = inside || entry.is_dir;
            match tops.iter_mut().find(|(name, _)| *name == top) {
                Some((_, is_folder)) => *is_folder |= folder,
                None => tops.push((top, folder)),
            }
        }
        let (icon, title) = match tops.as_slice() {
            [] => return None,
            [(name, true)] => (DragIcon::Folder, (*name).to_owned()),
            [(name, false)] => (DragIcon::File, (*name).to_owned()),
            several => {
                let folders = several.iter().filter(|(_, folder)| *folder).count();
                let what = match (folders, lang) {
                    (0, Lang::De) => "Dateien",
                    (0, Lang::En) => "files",
                    (n, Lang::De) if n == several.len() => "Ordner",
                    (n, Lang::En) if n == several.len() => "folders",
                    (_, Lang::De) => "Elemente",
                    (_, Lang::En) => "items",
                };
                (DragIcon::Several, format!("{} {what}", several.len()))
            }
        };
        let size = size_text_in(entries.iter().map(|e| e.size).sum(), lang);
        Some(Self { icon, title, size })
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
        assert_eq!(describe(BOTH), "ring and name");
        assert_eq!(describe(NAME), "name only");
        assert_eq!(describe(CursorFinder { ring: true, name: false }), "ring only");
        assert_eq!(describe(CursorFinder::OFF), "off");
    }

    fn file(path: &str, size: u64) -> FileEntry {
        FileEntry { path: path.into(), size, is_dir: false }
    }

    fn folder(path: &str) -> FileEntry {
        FileEntry { path: path.into(), size: 0, is_dir: true }
    }

    fn preview(icon: DragIcon, title: &str, size: &str) -> Option<DragPreview> {
        Some(DragPreview { icon, title: title.into(), size: size.into() })
    }

    #[test]
    fn the_drag_preview_names_what_was_picked_up() {
        let of = |entries: &[FileEntry]| DragPreview::of_in(entries, Lang::De);
        assert_eq!(of(&[file("Bericht.pdf", 3_141_592)]), preview(DragIcon::File, "Bericht.pdf", "3,1 MB"));
        // Ein Ordner kommt samt Inhalt; gezählt wird er einmal, seine Größe ist die seines Inhalts
        let project = [folder("Projekt"), folder("Projekt/sub"), file("Projekt/sub/notiz.txt", 600), file("Projekt/a.rs", 900)];
        assert_eq!(of(&project), preview(DragIcon::Folder, "Projekt", "1,5 KB"));
        assert_eq!(of(&[folder("leer")]), preview(DragIcon::Folder, "leer", "0 B"));

        let files = [file("a.jpg", 2_000_000), file("b.jpg", 2_000_000), file("c.jpg", 1_000_000)];
        assert_eq!(of(&files), preview(DragIcon::Several, "3 Dateien", "5 MB"));
        let folders = [folder("x"), file("x/1", 10), folder("y")];
        assert_eq!(of(&folders), preview(DragIcon::Several, "2 Ordner", "10 B"));
        let mixed = [file("Bericht.pdf", 1000), folder("Projekt"), file("Projekt/a.rs", 1000)];
        assert_eq!(of(&mixed), preview(DragIcon::Several, "2 Elemente", "2 KB"));
        assert_eq!(of(&[]), None);

        let of = |entries: &[FileEntry]| DragPreview::of_in(entries, Lang::En);
        assert_eq!(of(&[file("Bericht.pdf", 3_141_592)]), preview(DragIcon::File, "Bericht.pdf", "3.1 MB"));
        assert_eq!(of(&project), preview(DragIcon::Folder, "Projekt", "1.5 KB"));
        assert_eq!(of(&files), preview(DragIcon::Several, "3 files", "5 MB"));
        assert_eq!(of(&folders), preview(DragIcon::Several, "2 folders", "10 B"));
        assert_eq!(of(&mixed), preview(DragIcon::Several, "2 items", "2 KB"));
        assert_eq!(of(&[]), None);
    }

    /// Der einzige Test, der die Oberflächensprache des Prozesses umstellt (die Tests laufen
    /// parallel); alle anderen wählen die Sprache ausdrücklich.
    #[test]
    fn the_drag_preview_follows_the_ui_language() {
        let files = [file("a.jpg", 2_000_000), file("b.jpg", 1_100_000)];
        assert_eq!(crate::lang::ui_lang(), Lang::En, "Vorgabe, etwa für die Kommandozeile");
        assert_eq!(DragPreview::of(&files), preview(DragIcon::Several, "2 files", "3.1 MB"));
        crate::lang::set_ui_lang(Lang::De);
        assert_eq!(crate::lang::ui_lang(), Lang::De);
        assert_eq!(DragPreview::of(&files), preview(DragIcon::Several, "2 Dateien", "3,1 MB"));
        crate::lang::set_ui_lang(Lang::En);
        assert_eq!(DragPreview::of(&files), preview(DragIcon::Several, "2 files", "3.1 MB"));
    }
}
