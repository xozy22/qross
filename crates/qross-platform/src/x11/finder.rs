//! Cursor-Finder unter X11: ein Override-Redirect-Fenster (der Fenstermanager verwaltet es nicht,
//! es bekommt keinen Fokus und steht in keiner Leiste) mit 32-Bit-Visual für den Alphakanal und
//! leerem Eingabebereich (Shape-Erweiterung), sodass Klicks hindurchgehen. Durchsichtig wird es nur
//! mit einem Compositor; ohne ihn bliebe der Hintergrund schwarz, dann zeigt Qross nichts.
//!
//! Die Skalierung kommt aus `Xft.dpi` (wie bei GTK und Qt), denn X11 kennt keine je Monitor.

use anyhow::{bail, Context, Result};
use qross_proto::Rect;
use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
    ImageFormat, ImageOrder, PropMode, StackMode, VisualClass, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::NONE;

use crate::finder::{Display, Frame, Surface};

pub(crate) struct X11Surface {
    conn: RustConnection,
    root: Window,
    /// Visual mit Alphakanal (Tiefe 32)
    visual: Visualid,
    /// `_NET_WM_CM_S<n>`: Wem die Auswahl gehört, der ist der Compositor
    compositor: u32,
    /// Fenster und Grafikkontext, beim ersten Zeigen angelegt
    window: Option<(Window, u32)>,
    mapped: bool,
    /// Logische Pixel → Pixel (`Xft.dpi` / 96)
    scale: f32,
    /// Bytes in der Reihenfolge des X-Servers (fast immer LSB zuerst wie das Bild)
    msb_first: bool,
    told_no_compositor: bool,
}

pub(crate) fn surface() -> Result<Box<dyn Surface>> {
    let (conn, screen_num) = x11rb::connect(None).context("keine Verbindung zum X-Server")?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let visual = screen
        .allowed_depths
        .iter()
        .filter(|d| d.depth == 32)
        .flat_map(|d| d.visuals.iter())
        .find(|v| v.class == VisualClass::TRUE_COLOR)
        .map(|v| v.visual_id);
    let Some(visual) = visual else { bail!("der X-Server bietet kein Visual mit Alphakanal") };
    let compositor = conn.intern_atom(false, format!("_NET_WM_CM_S{screen_num}").as_bytes())?.reply()?.atom;
    let scale = xft_dpi(&conn, root).map_or(1.0, |dpi| dpi / 96.0).clamp(1.0, 4.0);
    let msb_first = conn.setup().image_byte_order == ImageOrder::MSB_FIRST;
    Ok(Box::new(X11Surface {
        conn,
        root,
        visual,
        compositor,
        window: None,
        mapped: false,
        scale,
        msb_first,
        told_no_compositor: false,
    }))
}

/// `Xft.dpi` aus den Ressourcen des X-Servers (setzen GNOME, KDE und Xfce bei Skalierung)
fn xft_dpi(conn: &RustConnection, root: Window) -> Option<f32> {
    let reply = conn.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16).ok()?.reply().ok()?;
    let text = String::from_utf8_lossy(&reply.value).into_owned();
    text.lines().find_map(|line| line.strip_prefix("Xft.dpi:").and_then(|v| v.trim().parse().ok()))
}

impl X11Surface {
    fn create(&mut self) -> Result<(Window, u32)> {
        let conn = &self.conn;
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, self.root, self.visual)?;
        let window = conn.generate_id()?;
        let aux = CreateWindowAux::new().override_redirect(1).background_pixel(0).border_pixel(0).colormap(colormap);
        conn.create_window(32, window, self.root, 0, 0, 1, 1, 0, WindowClass::INPUT_OUTPUT, self.visual, &aux)?;
        // Kein Eingabebereich: Klicks und Bewegungen gehen an das Fenster darunter
        conn.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &[])?;
        let atom = |name: &str| -> Result<u32> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
        // Für Compositoren: wie ein gezogenes Symbol, also ohne Schatten und Einblend-Animation
        let (window_type, dnd) = (atom("_NET_WM_WINDOW_TYPE")?, atom("_NET_WM_WINDOW_TYPE_DND")?);
        conn.change_property32(PropMode::REPLACE, window, window_type, AtomEnum::ATOM, &[dnd])?;
        conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, b"Qross Cursor-Finder")?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;
        conn.flush()?;
        Ok((window, gc))
    }

    fn compositing(&mut self) -> bool {
        let owner = self.conn.get_selection_owner(self.compositor).ok().and_then(|c| c.reply().ok()).map(|r| r.owner);
        let running = owner.is_some_and(|o| o != NONE);
        if !running && !self.told_no_compositor {
            tracing::info!("Cursor-Finder: kein Compositor aktiv, ohne ihn ließe sich nichts durchsichtig zeigen");
        }
        self.told_no_compositor = !running;
        running
    }

    fn draw(&mut self, frame: &Frame) -> Result<()> {
        let Some((window, gc)) = self.window else { return Ok(()) };
        let r = frame.rect;
        let size = ConfigureWindowAux::new()
            .x(r.x)
            .y(r.y)
            .width(frame.width)
            .height(frame.height)
            .stack_mode(StackMode::ABOVE);
        self.conn.configure_window(window, &size)?;
        if !self.mapped {
            self.conn.map_window(window)?;
            self.mapped = true;
        }
        let mut pixels = std::borrow::Cow::Borrowed(&frame.pixels);
        if self.msb_first {
            pixels = std::borrow::Cow::Owned(frame.pixels.chunks(4).flat_map(|p| [p[3], p[2], p[1], p[0]]).collect());
        }
        // In Streifen, die in eine Anfrage passen
        let row = frame.width as usize * 4;
        let rows = ((self.conn.maximum_request_bytes().saturating_sub(64)) / row.max(1)).max(1);
        for (i, band) in pixels.chunks(row * rows).enumerate() {
            let y = (i * rows) as i16;
            let height = (band.len() / row) as u16;
            self.conn.put_image(ImageFormat::Z_PIXMAP, window, gc, frame.width as u16, height, 0, y, 0, 32, band)?;
        }
        self.conn.flush()?;
        Ok(())
    }
}

impl Surface for X11Surface {
    fn prepare(&mut self) -> bool {
        if !self.compositing() {
            return false;
        }
        if self.window.is_none() {
            match self.create() {
                Ok(window) => self.window = Some(window),
                Err(e) => tracing::warn!("Cursor-Finder: Fenster nicht angelegt: {e:#}"),
            }
        }
        self.window.is_some()
    }

    fn cursor(&mut self) -> Option<(i32, i32)> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((reply.root_x.into(), reply.root_y.into()))
    }

    fn display_at(&mut self, x: i32, y: i32) -> Option<Display> {
        let monitors: Vec<Rect> = match self.conn.randr_get_monitors(self.root, true).ok().and_then(|c| c.reply().ok()) {
            Some(reply) => {
                reply.monitors.iter().map(|m| Rect::new(m.x.into(), m.y.into(), m.width.into(), m.height.into())).collect()
            }
            None => Vec::new(),
        };
        let inside = |m: &&Rect| x >= m.x && x <= m.right() && y >= m.y && y <= m.bottom();
        let area = match monitors.iter().find(inside).or(monitors.first()) {
            Some(monitor) => *monitor,
            None => {
                let g = self.conn.get_geometry(self.root).ok()?.reply().ok()?;
                Rect::new(0, 0, g.width.into(), g.height.into())
            }
        };
        Some(Display { area, scale: self.scale, density: 1.0 })
    }

    fn present(&mut self, frame: &Frame) {
        if let Err(e) = self.draw(frame) {
            tracing::debug!("Cursor-Finder: Bild nicht gezeigt: {e}");
        }
    }

    fn hide(&mut self) {
        if let (Some((window, _)), true) = (self.window, self.mapped) {
            let _ = self.conn.unmap_window(window);
            let _ = self.conn.flush();
            self.mapped = false;
        }
    }
}
