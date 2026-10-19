//! Cursor-Finder auf diesem Rechner: Kommt der Cursor hier an, blitzt etwa eine Sekunde lang ein
//! Ring um ihn auf und darunter der Name des Rechners. Dieselbe Anzeige zeigt beim Drag & Drop über
//! die Kante neben dem Cursor, was gezogen wird ([`card`]), bis abgelegt oder abgebrochen wird. Sie
//! liegt über allen Fenstern, nimmt keinen Fokus, lässt Klicks hindurch und steht nicht in der
//! Taskleiste. Läuft im Vordergrund ein Vollbildprogramm (siehe [`crate::game_state`]), zeigt sie
//! nichts.
//!
//! Gezeichnet wird auf allen Systemen gleich ([`draw`], [`card`]); jedes System stellt nur das
//! durchsichtige Fenster dazu ([`Surface`]): Windows ein Layered-Fenster, macOS ein randloses
//! `NSWindow` auf hoher Ebene in allen Spaces, X11 ein Override-Redirect-Fenster mit Alphakanal ohne
//! Eingabebereich. Unter Wayland lassen sich keine Fenster über andere Programme legen; dort gibt
//! es den Cursor-Finder und die Drag-Vorschau nicht.
//!
//! Alles läuft in einem eigenen Thread, der beim ersten Zeigen startet. Er folgt dem Cursor, solange
//! die Anzeige zu sehen ist, und richtet Größe und Lage nach dem Monitor unter dem Cursor. Ring und
//! Drag-Vorschau teilen sich das Fenster; solange die Vorschau zu sehen ist, bleibt der Ring weg.

pub mod card;
pub mod draw;
mod font;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use qross_core::finder::DragPreview;
use qross_proto::CursorFinder;
use tracing::{debug, warn};

pub use draw::{Display, Frame, DURATION};
use font::Shorten;

/// Zeigt den Cursor-Finder und die Drag-Vorschau auf diesem Rechner (siehe [`crate::finder_overlay`]).
pub trait FinderOverlay: Send + Sync {
    /// Etwa eine Sekunde lang zeigen, was `style` verlangt, um den Cursor, wo er gerade steht;
    /// `name`: Name dieses Rechners. Ein erneuter Aufruf beginnt von vorn.
    fn show(&self, style: CursorFinder, name: &str);
    /// Sofort ausblenden, etwa weil der Cursor den Rechner verlassen hat
    fn hide(&self);
    /// Gezogene Dateien neben dem Cursor zeigen, bis [`FinderOverlay::hide_drag`]; die Vorschau
    /// folgt ihm so lange. Ein erneuter Aufruf ersetzt sie.
    fn show_drag(&self, preview: &DragPreview);
    /// Die Drag-Vorschau ausblenden: abgelegt, abgebrochen oder der Cursor ist fort
    fn hide_drag(&self);
}

/// Das durchsichtige Fenster eines Systems. Lebt im Thread des Cursor-Finders.
pub(crate) trait Surface {
    /// Bereit machen, bevor die Anzeige beginnt. `false`: Hier lässt sich gerade nichts zeigen
    /// (z. B. X11 ohne Compositor, dort bliebe der Hintergrund schwarz).
    fn prepare(&mut self) -> bool;
    /// Position des Cursors in den Koordinaten des Systems
    fn cursor(&mut self) -> Option<(i32, i32)>;
    /// Monitor an dieser Stelle
    fn display_at(&mut self, x: i32, y: i32) -> Option<Display>;
    /// Bild anzeigen (das erste macht das Fenster sichtbar)
    fn present(&mut self, frame: &Frame);
    /// Ausblenden
    fn hide(&mut self);
    /// Zwischen zwei Bildern, z. B. Fensternachrichten abholen
    fn pump(&mut self) {}
}

/// Abstand zwischen zwei Bildern
const FRAME: Duration = Duration::from_millis(16);
/// Kam eine Anfrage so spät im Thread an, ist der Moment vorbei
const TOO_LATE: Duration = Duration::from_millis(300);

enum Command {
    Show { style: CursorFinder, name: String, at: Instant },
    Hide,
    ShowDrag(DragPreview),
    HideDrag,
}

/// Erzeugt das Fenster eines Systems, im Thread des Cursor-Finders
pub(crate) type MakeSurface = fn() -> anyhow::Result<Box<dyn Surface>>;

/// Cursor-Finder mit dem Fenster eines Systems; der Thread startet beim ersten Zeigen.
pub(crate) struct Overlay {
    make: MakeSurface,
    /// Läuft im Vordergrund ein Vollbildprogramm? (dann nichts zeigen)
    fullscreen: fn() -> bool,
    thread: Mutex<Option<Sender<Command>>>,
    /// Schon gewarnt, dass sich das Fenster nicht anlegen lässt
    warned: AtomicBool,
}

impl Overlay {
    pub(crate) fn new(make: MakeSurface) -> Self {
        Self::with_fullscreen(make, || crate::game_state().is_active())
    }

    pub(crate) fn with_fullscreen(make: MakeSurface, fullscreen: fn() -> bool) -> Self {
        Self { make, fullscreen, thread: Mutex::new(None), warned: AtomicBool::new(false) }
    }

    fn send(&self, command: Command, start: bool) {
        let mut thread = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        let command = match thread.as_ref() {
            Some(tx) => match tx.send(command) {
                Ok(()) => return,
                // Der Thread ist beendet (Fenster ließ sich nicht anlegen): beim nächsten Zeigen neu versuchen
                Err(mpsc::SendError(command)) => command,
            },
            None => command,
        };
        *thread = None;
        if !start {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (make, fullscreen) = (self.make, self.fullscreen);
        let warn_once = !self.warned.swap(true, Ordering::Relaxed);
        let spawned = std::thread::Builder::new().name("qross-finder".into()).spawn(move || match make() {
            Ok(surface) => run(surface, rx, fullscreen),
            Err(e) if warn_once => warn!("Cursor-Finder nicht verfügbar: {e:#}"),
            Err(e) => debug!("Cursor-Finder nicht verfügbar: {e:#}"),
        });
        match spawned {
            Ok(_) => {
                let _ = tx.send(command);
                *thread = Some(tx);
            }
            Err(e) => warn!("Cursor-Finder: Thread nicht gestartet: {e}"),
        }
    }
}

impl FinderOverlay for Overlay {
    fn show(&self, style: CursorFinder, name: &str) {
        if style.is_on() {
            self.send(Command::Show { style, name: name.to_owned(), at: Instant::now() }, true);
        }
    }

    fn hide(&self) {
        // Läuft noch nichts, ist auch nichts zu sehen.
        self.send(Command::Hide, false);
    }

    fn show_drag(&self, preview: &DragPreview) {
        self.send(Command::ShowDrag(preview.clone()), true);
    }

    fn hide_drag(&self) {
        self.send(Command::HideDrag, false);
    }
}

/// Schrift für Namen und Größen, beim ersten Gebrauch geladen (`None` darin: keine gefunden)
type FontSlot = Option<Option<font::Font>>;

fn font(slot: &mut FontSlot) -> Option<&font::Font> {
    slot.get_or_insert_with(|| {
        let found = font::Font::system();
        if found.is_none() {
            warn!("Cursor-Finder: keine Schrift gefunden, Namen fehlen");
        }
        found
    })
    .as_ref()
}

/// Eine laufende Anzeige
struct Showing {
    style: CursorFinder,
    name: String,
    since: Instant,
    /// Gerasterter Name zur Bildpunkt-Größe (ändert sich nur, wenn der Cursor auf einen Monitor mit
    /// anderer Skalierung wandert)
    text: Option<(f32, Option<draw::Text>)>,
    /// So viele Bilder wurden gezeigt
    presented: u32,
}

impl Showing {
    fn frame(&mut self, surface: &mut dyn Surface, font: &mut FontSlot) -> Option<Frame> {
        let (style, t) = (self.style, self.since.elapsed());
        if t >= DURATION {
            return None;
        }
        let cursor = surface.cursor()?;
        let display = surface.display_at(cursor.0, cursor.1)?;
        let text = if style.name { self.text(&display, font) } else { None };
        let label = text.map(|t| draw::label_size(t, &display));
        let scene = draw::scene(style, cursor, &display, t, label)?;
        Some(draw::render(&scene, &display, text))
    }

    fn text(&mut self, display: &Display, slot: &mut FontSlot) -> Option<&draw::Text> {
        let scale = display.pixel_scale();
        if !matches!(&self.text, Some((s, _)) if *s == scale) {
            let px = draw::FONT_SIZE * scale;
            let max = draw::MAX_LABEL_WIDTH * scale - px;
            let rendered = font(slot).and_then(|f| f.render(&self.name, px, max, Shorten::End));
            self.text = Some((scale, rendered));
        }
        self.text.as_ref().and_then(|(_, text)| text.as_ref())
    }
}

/// Eine laufende Drag-Vorschau
struct Dragging {
    preview: DragPreview,
    since: Instant,
    /// Gezeichnetes Kärtchen, je Skalierung und Dichte des Monitors unter dem Cursor
    card: Option<((f32, f32), card::Card)>,
}

impl Dragging {
    fn frame(&mut self, surface: &mut dyn Surface, slot: &mut FontSlot) -> Option<Frame> {
        let cursor = surface.cursor()?;
        let display = surface.display_at(cursor.0, cursor.1)?;
        let key = (display.scale, display.density);
        if !matches!(&self.card, Some((k, _)) if *k == key) {
            let scale = display.pixel_scale();
            let max = card::MAX_TEXT_WIDTH * scale;
            let font = font(slot);
            let title = font.and_then(|f| f.render(&self.preview.title, card::TITLE_SIZE * scale, max, Shorten::Middle));
            let size = font.and_then(|f| f.render(&self.preview.size, card::DETAIL_SIZE * scale, max, Shorten::End));
            self.card = Some((key, card::render(self.preview.icon, title.as_ref(), size.as_ref(), &display)));
        }
        let (_, card) = self.card.as_ref()?;
        Some(card.frame(cursor, &display, card::alpha_at(self.since.elapsed())))
    }
}

/// Schleife des Cursor-Finder-Threads; endet, sobald niemand mehr etwas schicken kann.
fn run(mut surface: Box<dyn Surface>, commands: Receiver<Command>, fullscreen: fn() -> bool) {
    let mut font = None;
    let mut showing: Option<Showing> = None;
    let mut dragging: Option<Dragging> = None;
    // Das Fenster ist bereit (und womöglich zu sehen), und welches Bild es zeigt
    let mut prepared = false;
    let mut last: Option<Frame> = None;
    loop {
        let mut command = if showing.is_some() || dragging.is_some() {
            match commands.recv_timeout(FRAME) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        // Was schon wartet, gleich mit: Folgt die Vorschau direkt auf den Ring (der Cursor kommt mit
        // Dateien zum Server zurück), blitzt der Ring nicht erst ein Bild lang auf.
        while let Some(current) = command.take() {
            match current {
                Command::Show { style, name, at } => {
                    showing = None;
                    if dragging.is_some() {
                        debug!("Cursor-Finder: die Drag-Vorschau ist zu sehen, nichts weiter gezeigt");
                    } else if at.elapsed() > TOO_LATE {
                        debug!("Cursor-Finder: zu spät, nichts gezeigt");
                    } else if fullscreen() {
                        debug!("Cursor-Finder: Vollbild im Vordergrund, nichts gezeigt");
                    } else if surface.prepare() {
                        prepared = true;
                        debug!("Cursor-Finder: {} für „{name}“", qross_core::finder::describe(style));
                        showing = Some(Showing { style, name, since: at, text: None, presented: 0 });
                    }
                }
                Command::Hide => showing = None,
                Command::ShowDrag(preview) => {
                    // Die Vorschau hat Vorrang, Ring und Name machen ihr Platz
                    (showing, dragging) = (None, None);
                    if fullscreen() {
                        debug!("Drag-Vorschau: Vollbild im Vordergrund, nichts gezeigt");
                    } else if surface.prepare() {
                        prepared = true;
                        debug!("Drag-Vorschau: {} ({})", preview.title, preview.size);
                        dragging = Some(Dragging { preview, since: Instant::now(), card: None });
                    }
                }
                Command::HideDrag => dragging = None,
            }
            command = commands.try_recv().ok();
        }
        let mut finished = false;
        let frame = match (dragging.as_mut(), showing.as_mut()) {
            (Some(drag), _) => drag.frame(&mut *surface, &mut font),
            (None, Some(current)) => {
                let frame = current.frame(&mut *surface, &mut font);
                if frame.is_some() {
                    current.presented += 1;
                } else if current.since.elapsed() >= DURATION {
                    if current.presented == 0 {
                        debug!("Cursor-Finder: nichts gezeigt, Cursor oder Monitor nicht abfragbar");
                    }
                    finished = true;
                }
                // Sonst ist Cursor oder Monitor gerade nicht abfragbar: beim nächsten Bild erneut
                frame
            }
            (None, None) => None,
        };
        if finished {
            showing = None;
        }
        // Steht der Cursor still und ändert sich nichts, bleibt das Fenster, wie es ist.
        if let Some(frame) = frame.filter(|frame| last.as_ref() != Some(frame)) {
            surface.present(&frame);
            last = Some(frame);
        }
        if prepared && showing.is_none() && dragging.is_none() {
            surface.hide();
            (prepared, last) = (false, None);
        }
        surface.pump();
    }
    surface.hide();
}

#[cfg(test)]
mod tests {
    use super::*;
    use qross_proto::Rect;
    use std::sync::Arc;

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Present(Rect),
        Hide,
    }

    /// Aufzeichnung des Test-Fensters (der Thread legt es selbst an, daher global je Test)
    type Calls = Arc<Mutex<Vec<Call>>>;

    /// Test-Fenster mit dem Cursor dort, wo ihn der Test gerade hinstellt
    struct FakeSurface(Calls, &'static Mutex<(i32, i32)>);

    /// Cursor der Tests, die ihn nicht bewegen
    static STILL: Mutex<(i32, i32)> = Mutex::new((300, 200));

    impl Surface for FakeSurface {
        fn prepare(&mut self) -> bool {
            true
        }

        fn cursor(&mut self) -> Option<(i32, i32)> {
            Some(*self.1.lock().unwrap())
        }

        fn display_at(&mut self, _x: i32, _y: i32) -> Option<Display> {
            Some(Display { area: Rect::new(0, 0, 1920, 1080), scale: 1.0, density: 1.0 })
        }

        fn present(&mut self, frame: &Frame) {
            self.0.lock().unwrap().push(Call::Present(frame.rect));
        }

        fn hide(&mut self) {
            self.0.lock().unwrap().push(Call::Hide);
        }
    }

    const RING: CursorFinder = CursorFinder { ring: true, name: false };

    fn presents(calls: &Calls) -> usize {
        calls.lock().unwrap().iter().filter(|c| matches!(c, Call::Present(_))).count()
    }

    fn wait(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    // Je Test ein eigenes Fenster: `MakeSurface` ist ein Funktionszeiger, die Aufzeichnung daher statisch.
    static SHOWN: Mutex<Option<Calls>> = Mutex::new(None);
    static HIDDEN: Mutex<Option<Calls>> = Mutex::new(None);
    static FULLSCREEN: Mutex<Option<Calls>> = Mutex::new(None);
    static DRAGGED: Mutex<Option<Calls>> = Mutex::new(None);
    static DRAG_CURSOR: Mutex<(i32, i32)> = Mutex::new((300, 200));
    static RING_THEN_DRAG: Mutex<Option<Calls>> = Mutex::new(None);

    fn surface_from(slot: &'static Mutex<Option<Calls>>) -> Calls {
        slot.lock().unwrap().get_or_insert_with(Calls::default).clone()
    }

    #[test]
    fn it_follows_the_cursor_for_a_second_and_disappears() {
        let calls = surface_from(&SHOWN);
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&SHOWN), &STILL))), || false);
        overlay.hide();
        assert!(calls.lock().unwrap().is_empty(), "Ausblenden startet nichts");
        overlay.show(CursorFinder::OFF, "pc");
        wait(100);
        assert_eq!(presents(&calls), 0, "ausgeschaltet zeigt er nichts");

        overlay.show(RING, "pc");
        wait(DURATION.as_millis() as u64 + 300);
        // Etwa 60 Bilder je Sekunde; auf einem ausgelasteten Rechner im Debug-Build weniger
        let shown = presents(&calls);
        assert!(shown >= 5, "laufend neue Bilder: {shown}");
        let first = calls.lock().unwrap()[0].clone();
        let Call::Present(rect) = first else { panic!("{first:?}") };
        assert!(rect.x < 300 && rect.right() > 300 && rect.y < 200 && rect.bottom() > 200, "um den Cursor: {rect:?}");
        assert_eq!(calls.lock().unwrap().last(), Some(&Call::Hide), "nach einer Sekunde ausgeblendet");
        let after = presents(&calls);
        wait(100);
        assert_eq!(presents(&calls), after, "danach keine Bilder mehr");
    }

    #[test]
    fn leaving_hides_it_at_once() {
        let calls = surface_from(&HIDDEN);
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&HIDDEN), &STILL))), || false);
        overlay.show(RING, "laptop");
        wait(100);
        overlay.hide();
        wait(50);
        assert_eq!(calls.lock().unwrap().last(), Some(&Call::Hide));
        let count = calls.lock().unwrap().len();
        wait(100);
        assert_eq!(calls.lock().unwrap().len(), count, "nichts mehr nach dem Ausblenden");
    }

    #[test]
    fn nothing_over_a_fullscreen_program() {
        let calls = surface_from(&FULLSCREEN);
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&FULLSCREEN), &STILL))), || true);
        overlay.show(RING, "mac");
        overlay.show_drag(&report());
        wait(150);
        assert!(calls.lock().unwrap().is_empty(), "{:?}", calls.lock().unwrap());
    }

    fn report() -> DragPreview {
        DragPreview { icon: qross_core::finder::DragIcon::File, title: "Bericht.pdf".into(), size: "3,1 MB".into() }
    }

    fn last_present(calls: &Calls) -> Option<Rect> {
        calls.lock().unwrap().iter().rev().find_map(|c| match c {
            Call::Present(rect) => Some(*rect),
            Call::Hide => None,
        })
    }

    fn hidden_last(calls: &Calls) -> bool {
        calls.lock().unwrap().last() == Some(&Call::Hide)
    }

    /// Wartet bis zu zwei Sekunden, bis `condition` gilt (auch ein ausgelasteter Rechner kommt mit)
    fn eventually(what: &str, condition: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < until, "Zeitüberschreitung beim Warten auf: {what}");
            wait(10);
        }
    }

    #[test]
    fn the_drag_preview_follows_the_cursor_until_it_is_hidden() {
        let calls = surface_from(&DRAGGED);
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&DRAGGED), &DRAG_CURSOR))), || false);
        overlay.hide_drag();
        assert!(calls.lock().unwrap().is_empty(), "Ausblenden startet nichts");
        overlay.show_drag(&report());
        // Länger als der Cursor-Finder: Die Vorschau bleibt, bis sie ausgeblendet wird
        wait(DURATION.as_millis() as u64 + 300);
        let rect = last_present(&calls).expect("gezeigt");
        assert!(rect.x > 300 && rect.y > 200 && rect.x < 300 + 40 && rect.y < 200 + 40, "rechts unter dem Cursor: {rect:?}");
        assert!(!calls.lock().unwrap().contains(&Call::Hide), "{:?}", calls.lock().unwrap());
        // Eingeblendet und dann still: Solange der Cursor steht, kommen keine neuen Bilder
        let still = presents(&calls);
        assert!(still < 20, "{still} Bilder für einen stehenden Cursor");
        wait(100);
        assert_eq!(presents(&calls), still);

        *DRAG_CURSOR.lock().unwrap() = (900, 600);
        eventually("Vorschau am neuen Ort", || last_present(&calls) != Some(rect));
        let moved = last_present(&calls).unwrap();
        assert_eq!((moved.x - rect.x, moved.y - rect.y), (600, 400), "folgt dem Cursor: {moved:?}");
        assert_eq!((moved.width, moved.height), (rect.width, rect.height));

        overlay.hide_drag();
        eventually("ausgeblendet", || hidden_last(&calls));
        let count = calls.lock().unwrap().len();
        *DRAG_CURSOR.lock().unwrap() = (100, 100);
        wait(100);
        assert_eq!(calls.lock().unwrap().len(), count, "nichts mehr nach dem Ausblenden");
    }

    #[test]
    fn the_drag_preview_takes_the_place_of_the_ring() {
        let calls = surface_from(&RING_THEN_DRAG);
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&RING_THEN_DRAG), &STILL))), || false);
        overlay.show(RING, "laptop");
        eventually("Ring", || last_present(&calls).is_some());
        let ring = last_present(&calls).unwrap();
        assert!(ring.x < 300 && ring.right() > 300, "um den Cursor: {ring:?}");
        overlay.show_drag(&report());
        eventually("Kärtchen", || last_present(&calls).is_some_and(|rect| rect.x > 300));
        let card = last_present(&calls).unwrap();
        assert!(card.y > 200, "rechts unter dem Cursor: {card:?}");
        // Ein Ring während der Vorschau zeigt nichts; der Cursor-Finder blendet sie auch nicht aus
        overlay.show(RING, "laptop");
        overlay.hide();
        wait(100);
        assert_eq!(last_present(&calls), Some(card));
        assert!(!calls.lock().unwrap().contains(&Call::Hide), "{:?}", calls.lock().unwrap());
        overlay.hide_drag();
        eventually("ausgeblendet", || hidden_last(&calls));
    }

    #[test]
    fn a_missing_window_is_tried_again_later() {
        let overlay = Overlay::with_fullscreen(|| Err(anyhow::anyhow!("kein Bildschirm")), || false);
        overlay.show(RING, "pc");
        wait(50);
        overlay.show(RING, "pc");
        overlay.hide();
    }
}
