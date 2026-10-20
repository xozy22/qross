//! Laufendes Drag & Drop unter X11 (XDND) erkennen, am Server (Dateien an der Kante zum Client) wie
//! am Client (Dateien zurück zum Server). Wer zieht, wird Besitzer der Auswahl `XdndSelection` und
//! bietet darüber die Daten an, Dateien als `text/uri-list`. Jeden Besitzwechsel meldet XFixes; ist
//! ihre Zahl seit dem Drücken der Maustaste gestiegen, läuft ein neues Ziehen (`DragTracker`).
//!
//! Abgebrochen wird es wie unter Windows und macOS mit Esc (per XTest): GTK, Qt und Chromium beenden
//! das Ziehen dann, ohne abzulegen, und geben Zeiger und Tastatur wieder frei.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, Window, WindowClass, KEY_PRESS_EVENT, KEY_RELEASE_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

use crate::dnd_logic::{parse_uri_list, DragTracker};
use crate::keymap_linux::X11_OFFSET;

/// So lange darf das ziehende Programm für die Dateiliste brauchen
const ANSWER_TIMEOUT: Duration = Duration::from_millis(300);

/// Eigene Verbindung mit einem unsichtbaren Fenster, an das die Dateiliste geht
pub struct Xdnd {
    conn: RustConnection,
    window: Window,
    selection: Atom,
    uri_list: Atom,
    incr: Atom,
    property: Atom,
    /// Besitzwechsel von `XdndSelection` seit dem Start
    changes: u32,
}

impl Xdnd {
    pub fn new() -> Result<Self> {
        let (conn, root) = super::connect()?;
        let version = conn.xfixes_query_version(5, 0)?.reply()?;
        if version.major_version < 1 {
            bail!("the X server does not support XFixes");
        }
        let atom = |name: &str| -> Result<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        let (selection, uri_list, incr, property) =
            (atom("XdndSelection")?, atom("text/uri-list")?, atom("INCR")?, atom("QROSS_XDND")?);
        let window = conn.generate_id()?;
        let aux = CreateWindowAux::new();
        conn.create_window(COPY_DEPTH_FROM_PARENT, window, root, -10, -10, 1, 1, 0, WindowClass::INPUT_ONLY, COPY_FROM_PARENT, &aux)?;
        conn.xfixes_select_selection_input(window, selection, SelectionEventMask::SET_SELECTION_OWNER)?.check()?;
        Ok(Self { conn, window, selection, uri_list, incr, property, changes: 0 })
    }

    /// Zahl der Besitzwechsel von `XdndSelection` bis jetzt (ändert sich mit jedem neuen Ziehen)
    pub fn generation(&mut self) -> Result<i64> {
        // Rundreise: Danach liegen alle Meldungen bis zu diesem Moment vor.
        self.conn.get_input_focus()?.reply()?;
        while let Some(event) = self.conn.poll_for_event()? {
            self.count(&event);
        }
        Ok(i64::from(self.changes))
    }

    fn count(&mut self, event: &Event) {
        if let Event::XfixesSelectionNotify(e) = event {
            if e.selection == self.selection && e.owner != NONE {
                self.changes = self.changes.wrapping_add(1);
            }
        }
    }

    /// Dateien des laufenden Ziehens; leer, wenn keins läuft oder es keine Dateien anbietet
    pub fn files(&mut self) -> Vec<PathBuf> {
        match self.request_files() {
            Ok(files) => files,
            Err(e) => {
                tracing::debug!("cannot read the file list of the drag: {e:#}");
                Vec::new()
            }
        }
    }

    fn request_files(&mut self) -> Result<Vec<PathBuf>> {
        if self.conn.get_selection_owner(self.selection)?.reply()?.owner == NONE {
            return Ok(Vec::new());
        }
        self.conn.delete_property(self.window, self.property)?;
        self.conn.convert_selection(self.window, self.selection, self.uri_list, self.property, CURRENT_TIME)?;
        self.conn.flush()?;
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        let property = loop {
            match self.conn.poll_for_event()? {
                Some(Event::SelectionNotify(e)) if e.requestor == self.window && e.selection == self.selection => {
                    break e.property;
                }
                Some(event) => self.count(&event),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                None => bail!("no answer from the dragging program"),
            }
        };
        if property == NONE {
            // Das Programm bietet keine Dateien an (z. B. gezogener Text)
            return Ok(Vec::new());
        }
        // Bis 4 MB; größere Listen kämen stückweise (INCR), das braucht es für Dateinamen nicht.
        let reply = self.conn.get_property(true, self.window, property, AtomEnum::ANY, 0, 1 << 20)?.reply()?;
        if reply.type_ == self.incr {
            bail!("file list too large");
        }
        Ok(parse_uri_list(&reply.value))
    }
}

/// Zustand der Erkennung: Verbindung (fehlt, wenn XFixes fehlt) und Zählerstand beim Drücken
pub struct DragState {
    xdnd: Option<Xdnd>,
    tracker: DragTracker,
}

impl DragState {
    pub fn new() -> Self {
        let xdnd = Xdnd::new()
            .map_err(|e| tracing::warn!("{e:#}; ongoing drag and drop will not be detected"))
            .ok();
        Self { xdnd, tracker: DragTracker::default() }
    }

    pub fn press(&mut self) {
        if let Some(generation) = self.xdnd.as_mut().and_then(|x| x.generation().ok()) {
            self.tracker.press(generation);
        }
    }

    pub fn release(&mut self) {
        self.tracker.release();
    }

    /// Hat seit dem Drücken ein Ziehen begonnen, dessen Dateien noch nicht gelesen wurden?
    /// Danach gilt es als erledigt (siehe `DragTracker::take`).
    pub fn take(&mut self) -> bool {
        if !self.tracker.is_pressed() {
            return false;
        }
        match self.xdnd.as_mut().map(Xdnd::generation) {
            Some(Ok(generation)) => self.tracker.take(generation),
            Some(Err(e)) => {
                tracing::debug!("cannot query XDND: {e:#}");
                false
            }
            None => false,
        }
    }

    pub fn files(&mut self) -> Vec<PathBuf> {
        self.xdnd.as_mut().map(Xdnd::files).unwrap_or_default()
    }
}

/// Esc drücken und loslassen (XTest): bricht ein laufendes Ziehen ab.
pub fn tap_escape(conn: &impl Connection, root: Window) {
    const KEY_ESC: u16 = 1;
    let code = (KEY_ESC + X11_OFFSET) as u8;
    for kind in [KEY_PRESS_EVENT, KEY_RELEASE_EVENT] {
        if let Err(e) = conn.xtest_fake_input(kind, code, CURRENT_TIME, root, 0, 0, 0) {
            tracing::warn!("XTest failed: {e}");
        }
    }
    let _ = conn.flush();
}
