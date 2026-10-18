//! Cursor-Finder auf diesem Rechner: Kommt der Cursor hier an, blitzt etwa eine Sekunde lang ein
//! Ring um ihn auf und darunter der Name des Rechners. Die Anzeige liegt über allen Fenstern, nimmt
//! keinen Fokus, lässt Klicks hindurch und steht nicht in der Taskleiste. Läuft im Vordergrund ein
//! Vollbildprogramm (siehe [`crate::game_state`]), zeigt sie nichts.
//!
//! Gezeichnet wird auf allen Systemen gleich ([`draw`]); jedes System stellt nur das durchsichtige
//! Fenster dazu ([`Surface`]): Windows ein Layered-Fenster, macOS ein randloses `NSWindow` auf
//! hoher Ebene in allen Spaces, X11 ein Override-Redirect-Fenster mit Alphakanal ohne
//! Eingabebereich. Unter Wayland lassen sich keine Fenster über andere Programme legen; dort gibt
//! es den Cursor-Finder nicht.
//!
//! Alles läuft in einem eigenen Thread, der beim ersten Zeigen startet. Er folgt dem Cursor, solange
//! die Anzeige zu sehen ist, und richtet Größe und Lage nach dem Monitor unter dem Cursor.

pub mod draw;
mod font;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use qross_proto::CursorFinder;
use tracing::{debug, warn};

pub use draw::{Display, Frame, DURATION};

/// Zeigt den Cursor-Finder auf diesem Rechner (siehe [`crate::finder_overlay`]).
pub trait FinderOverlay: Send + Sync {
    /// Etwa eine Sekunde lang zeigen, was `style` verlangt, um den Cursor, wo er gerade steht;
    /// `name`: Name dieses Rechners. Ein erneuter Aufruf beginnt von vorn.
    fn show(&self, style: CursorFinder, name: &str);
    /// Sofort ausblenden, etwa weil der Cursor den Rechner verlassen hat
    fn hide(&self);
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
    fn frame(&mut self, surface: &mut dyn Surface, font: &mut Option<Option<font::Font>>) -> Option<Frame> {
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

    fn text(&mut self, display: &Display, font: &mut Option<Option<font::Font>>) -> Option<&draw::Text> {
        let scale = display.pixel_scale();
        if !matches!(&self.text, Some((s, _)) if *s == scale) {
            let font = font.get_or_insert_with(|| {
                let found = font::Font::system();
                if found.is_none() {
                    warn!("Cursor-Finder: keine Schrift gefunden, der Name fehlt");
                }
                found
            });
            let px = draw::FONT_SIZE * scale;
            let rendered = font.as_ref().and_then(|f| f.render(&self.name, px, draw::MAX_LABEL_WIDTH * scale - px));
            self.text = Some((scale, rendered));
        }
        self.text.as_ref().and_then(|(_, text)| text.as_ref())
    }
}

/// Schleife des Cursor-Finder-Threads; endet, sobald niemand mehr etwas schicken kann.
fn run(mut surface: Box<dyn Surface>, commands: Receiver<Command>, fullscreen: fn() -> bool) {
    let mut font = None;
    let mut showing: Option<Showing> = None;
    loop {
        let command = if showing.is_some() {
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
        match command {
            Some(Command::Show { style, name, at }) => {
                let shown = showing.take().is_some();
                if at.elapsed() > TOO_LATE {
                    debug!("Cursor-Finder: zu spät, nichts gezeigt");
                } else if fullscreen() {
                    debug!("Cursor-Finder: Vollbild im Vordergrund, nichts gezeigt");
                } else if surface.prepare() {
                    debug!("Cursor-Finder: {} für „{name}“", qross_core::finder::describe(style));
                    showing = Some(Showing { style, name, since: at, text: None, presented: 0 });
                }
                if shown && showing.is_none() {
                    surface.hide();
                }
            }
            Some(Command::Hide) if showing.take().is_some() => surface.hide(),
            Some(Command::Hide) | None => {}
        }
        if let Some(current) = showing.as_mut() {
            match current.frame(&mut *surface, &mut font) {
                Some(frame) => {
                    surface.present(&frame);
                    current.presented += 1;
                }
                None if current.since.elapsed() >= DURATION => {
                    if current.presented == 0 {
                        debug!("Cursor-Finder: nichts gezeigt, Cursor oder Monitor nicht abfragbar");
                    }
                    showing = None;
                    surface.hide();
                }
                // Cursor oder Monitor gerade nicht abfragbar: beim nächsten Bild erneut
                None => {}
            }
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

    struct FakeSurface(Calls);

    impl Surface for FakeSurface {
        fn prepare(&mut self) -> bool {
            true
        }

        fn cursor(&mut self) -> Option<(i32, i32)> {
            Some((300, 200))
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

    fn surface_from(slot: &'static Mutex<Option<Calls>>) -> Calls {
        slot.lock().unwrap().get_or_insert_with(Calls::default).clone()
    }

    #[test]
    fn it_follows_the_cursor_for_a_second_and_disappears() {
        let calls = surface_from(&SHOWN);
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&SHOWN)))), || false);
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
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&HIDDEN)))), || false);
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
        let overlay = Overlay::with_fullscreen(|| Ok(Box::new(FakeSurface(surface_from(&FULLSCREEN)))), || true);
        overlay.show(RING, "mac");
        wait(150);
        assert!(calls.lock().unwrap().is_empty(), "{:?}", calls.lock().unwrap());
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
