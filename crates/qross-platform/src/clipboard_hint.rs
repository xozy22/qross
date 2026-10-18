//! Markierungen der Passwortmanager unter Linux (siehe `crate::concealed`): der MIME-Typ
//! `x-kde-passwordManagerHint` mit dem Inhalt „secret“, den KeePassXC setzt. `arboard` liest nur
//! Text, HTML, Bilder und Dateien; deshalb fragt Qross selbst nach – auf demselben Weg wie `arboard`:
//! über das Wayland-Protokoll data-control (KDE, Sway, Hyprland), sonst über X11 (auch XWayland,
//! etwa unter GNOME).

use std::io::Read;
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::debug;
use wl_clipboard_rs::paste::{self, ClipboardType, MimeType, Seat};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, GetPropertyReply, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

use crate::concealed;

/// So lange darf der Besitzer der Zwischenablage für eine Antwort brauchen
const ANSWER: Duration = Duration::from_millis(300);
/// Mehr als ein kurzes Wort steht in einer Markierung nie
const MARKER_MAX: u64 = 64;
/// Größte gelesene X11-Eigenschaft in 32-Bit-Einheiten; reicht für jede Liste der Ziele
const PROPERTY_MAX: u32 = 4096;

#[derive(Default)]
pub struct Hint {
    /// Eigene X11-Verbindung mit unsichtbarem Fenster für die Anfragen; erst bei Bedarf
    x11: Option<X11>,
    /// Der Compositor kennt data-control nicht (GNOME); `arboard` nimmt dann auch X11.
    no_data_control: bool,
}

impl Hint {
    pub fn concealed(&mut self) -> Option<&'static str> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() && !self.no_data_control {
            match wayland() {
                Ok(found) => return found,
                Err(e) => {
                    debug!("Zwischenablage über Wayland nicht prüfbar ({e}) – frage über X11");
                    self.no_data_control = matches!(e, paste::Error::MissingProtocol { .. });
                }
            }
        }
        self.x11_concealed()
    }

    fn x11_concealed(&mut self) -> Option<&'static str> {
        if self.x11.is_none() {
            self.x11 = X11::connect().map_err(|e| debug!("Zwischenablage über X11 nicht prüfbar: {e:#}")).ok();
        }
        match self.x11.as_ref()?.concealed() {
            Ok(found) => found,
            Err(e) => {
                debug!("Markierungen der Zwischenablage über X11 nicht lesbar: {e:#}");
                // Beim nächsten Mal neu verbinden
                self.x11 = None;
                None
            }
        }
    }
}

fn wayland() -> Result<Option<&'static str>, paste::Error> {
    let types = match paste::get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
        Ok(types) => types,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoSeats) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(concealed::find(concealed::LINUX, |name| types.contains(name).then(|| wayland_contents(name))))
}

fn wayland_contents(mime: &str) -> Option<Vec<u8>> {
    let (pipe, _) = paste::get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Specific(mime)).ok()?;
    let mut data = Vec::new();
    pipe.take(MARKER_MAX).read_to_end(&mut data).ok()?;
    Some(data)
}

struct X11 {
    conn: RustConnection,
    window: Window,
    clipboard: Atom,
    targets: Atom,
    property: Atom,
    incr: Atom,
}

impl X11 {
    fn connect() -> Result<Self> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let window = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )?;
        let atom = |name: &str| -> Result<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        let (clipboard, targets, property, incr) =
            (atom("CLIPBOARD")?, atom("TARGETS")?, atom("QROSS_MARKER")?, atom("INCR")?);
        Ok(Self { conn, window, clipboard, targets, property, incr })
    }

    fn concealed(&self) -> Result<Option<&'static str>> {
        if self.conn.get_selection_owner(self.clipboard)?.reply()?.owner == x11rb::NONE {
            return Ok(None);
        }
        let Some(targets) = self.convert(self.targets)? else { return Ok(None) };
        let targets: Vec<Atom> = targets.value32().map(Iterator::collect).unwrap_or_default();
        let mut atoms = Vec::new();
        for (name, _) in concealed::LINUX {
            atoms.push((*name, self.conn.intern_atom(true, name.as_bytes())?.reply()?.atom));
        }
        Ok(concealed::find(concealed::LINUX, |name| {
            let atom = atoms.iter().find(|(n, _)| *n == name).map(|(_, atom)| *atom)?;
            if atom == x11rb::NONE || !targets.contains(&atom) {
                return None;
            }
            // Nicht lesbar (abgelehnt, keine Antwort, INCR): im Zweifel vertraulich
            Some(self.convert(atom).ok().flatten().filter(|r| r.type_ != self.incr).map(|r| r.value))
        }))
    }

    /// Fragt den Besitzer der Zwischenablage nach `target`; `None`, wenn er ablehnt oder nicht
    /// rechtzeitig antwortet.
    fn convert(&self, target: Atom) -> Result<Option<GetPropertyReply>> {
        // Verspätete Antworten früherer Anfragen verwerfen
        while self.conn.poll_for_event()?.is_some() {}
        self.conn.convert_selection(self.window, self.clipboard, target, self.property, x11rb::CURRENT_TIME)?;
        self.conn.flush()?;
        let deadline = Instant::now() + ANSWER;
        loop {
            while let Some(event) = self.conn.poll_for_event()? {
                let Event::SelectionNotify(notify) = event else { continue };
                if notify.requestor != self.window || notify.target != target {
                    continue;
                }
                if notify.property == x11rb::NONE {
                    return Ok(None);
                }
                let reply =
                    self.conn.get_property(true, self.window, self.property, AtomEnum::ANY, 0, PROPERTY_MAX)?.reply()?;
                return Ok(Some(reply));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
