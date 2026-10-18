//! Eingabe-Erfassung unter X11.
//!
//! Lokaler Modus: Zeigerposition alle 4 ms abfragen und Kanten melden (keine Grabs, das System
//! verhält sich völlig normal). Remote-Modus: Zeiger und Tastatur auf dem Root-Fenster grabben,
//! Cursor durch einen leeren ersetzen und nach jeder Bewegung in die Mitte zurücksetzen.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glide_core::CaptureEvent;
use glide_proto::{Edge, MouseButton};
use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Cursor, EventMask, GrabMode, GrabStatus, Window};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::keymap_linux::{hid_from_evdev, X11_OFFSET};
use crate::Capture;

/// evdev-Codes für Notausgang und Rollen-Taste
const KEY_ESC: u16 = 1;
const KEY_SCROLLLOCK: u16 = 70;
const CTRL: [u16; 2] = [29, 97];
const ALT: [u16; 2] = [56, 100];
const SHIFT: [u16; 2] = [42, 54];
/// Button1Mask … Button5Mask
const ANY_BUTTON_MASK: u16 = 0x1F00;
const POLL: Duration = Duration::from_millis(4);

struct Shared {
    conn: RustConnection,
    root: Window,
    blank: Cursor,
    remote: AtomicBool,
    events: UnboundedSender<CaptureEvent>,
}

impl Shared {
    fn center(&self) -> (i16, i16) {
        let (w, h) = super::root_size(&self.conn, self.root).unwrap_or((1920, 1080));
        ((w / 2) as i16, (h / 2) as i16)
    }

    fn warp(&self, x: i16, y: i16) {
        let _ = self.conn.warp_pointer(NONE, self.root, 0, 0, 0, 0, x, y);
        let _ = self.conn.flush();
    }

    fn ungrab(&self) {
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
    }
}

pub fn start(events: UnboundedSender<CaptureEvent>) -> Result<Arc<dyn Capture>> {
    let (conn, root) = super::connect()?;
    let blank = blank_cursor(&conn, root).context("leeren Cursor anlegen")?;
    let shared = Arc::new(Shared { conn, root, blank, remote: AtomicBool::new(false), events });
    let worker = shared.clone();
    thread::Builder::new().name("glide-x11-capture".into()).spawn(move || {
        if let Err(e) = run(&worker) {
            tracing::error!("X11-Erfassung beendet: {e:#}");
        }
    })?;
    Ok(Arc::new(X11Capture(shared)))
}

fn blank_cursor(conn: &RustConnection, root: Window) -> Result<Cursor> {
    let pixmap = conn.generate_id()?;
    conn.create_pixmap(1, pixmap, root, 1, 1)?;
    let cursor = conn.generate_id()?;
    conn.create_cursor(cursor, pixmap, pixmap, 0, 0, 0, 0, 0, 0, 0, 0)?;
    conn.free_pixmap(pixmap)?;
    conn.flush()?;
    Ok(cursor)
}

struct X11Capture(Arc<Shared>);

impl Capture for X11Capture {
    fn capture_remote(&self) {
        let s = &self.0;
        let mask = EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE;
        let pointer = s
            .conn
            .grab_pointer(false, s.root, mask, GrabMode::ASYNC, GrabMode::ASYNC, s.root, s.blank, CURRENT_TIME)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.status);
        let keyboard = s
            .conn
            .grab_keyboard(false, s.root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.status);
        if pointer != Some(GrabStatus::SUCCESS) || keyboard != Some(GrabStatus::SUCCESS) {
            // Ein anderes Programm hält gerade einen Grab (z. B. ein offenes Menü): nicht wechseln.
            tracing::warn!("Eingaben lassen sich gerade nicht abfangen (Zeiger: {pointer:?}, Tastatur: {keyboard:?})");
            s.ungrab();
            let _ = s.events.send(CaptureEvent::PanicReturn);
            return;
        }
        s.remote.store(true, Ordering::Release);
        let (cx, cy) = s.center();
        s.warp(cx, cy);
    }

    fn release(&self, x: i32, y: i32) {
        let s = &self.0;
        s.remote.store(false, Ordering::Release);
        s.ungrab();
        s.warp(x as i16, y as i16);
    }
}

impl Drop for X11Capture {
    fn drop(&mut self) {
        self.0.ungrab();
        let _ = self.0.conn.flush();
    }
}

fn run(s: &Shared) -> Result<()> {
    let mut pressed: HashSet<u16> = HashSet::new();
    let mut size = super::root_size(&s.conn, s.root)?;
    let mut size_checked = Instant::now();

    loop {
        if !s.remote.load(Ordering::Acquire) {
            if size_checked.elapsed() > Duration::from_secs(1) {
                size = super::root_size(&s.conn, s.root)?;
                size_checked = Instant::now();
            }
            let pointer = s.conn.query_pointer(s.root)?.reply()?;
            let (x, y) = (i32::from(pointer.root_x), i32::from(pointer.root_y));
            if let Some(edge) = edge_at(x, y, size) {
                let buttons_down = u16::from(pointer.mask) & ANY_BUTTON_MASK != 0;
                let _ = s.events.send(CaptureEvent::EdgeHit { edge, x, y, buttons_down });
            }
            // Ereignisse aus der letzten Remote-Phase verwerfen
            while s.conn.poll_for_event()?.is_some() {}
            thread::sleep(POLL);
            continue;
        }

        let Some(event) = s.conn.poll_for_event()? else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        let (cx, cy) = s.center();
        let report = match event {
            Event::MotionNotify(e) => {
                let (dx, dy) = (i32::from(e.root_x) - i32::from(cx), i32::from(e.root_y) - i32::from(cy));
                if dx == 0 && dy == 0 {
                    continue;
                }
                s.warp(cx, cy);
                Some(CaptureEvent::MouseDelta { dx, dy })
            }
            Event::ButtonPress(e) => button_event(e.detail, true),
            Event::ButtonRelease(e) => button_event(e.detail, false),
            Event::KeyPress(e) | Event::KeyRelease(e) if e.detail as u16 >= X11_OFFSET => {
                let down = matches!(event, Event::KeyPress(_));
                let code = e.detail as u16 - X11_OFFSET;
                let repeat = down && !pressed.insert(code);
                if !down {
                    pressed.remove(&code);
                }
                let held = |keys: [u16; 2]| keys.iter().any(|k| pressed.contains(k));
                if code == KEY_ESC && down && held(CTRL) && held(ALT) && held(SHIFT) {
                    // Notausgang Strg+Alt+Shift+Esc
                    s.remote.store(false, Ordering::Release);
                    s.ungrab();
                    s.warp(cx, cy);
                    pressed.clear();
                    Some(CaptureEvent::PanicReturn)
                } else if code == KEY_SCROLLLOCK {
                    (down && !repeat).then_some(CaptureEvent::ToggleLock)
                } else {
                    hid_from_evdev(code).map(|key| CaptureEvent::Key { key, down })
                }
            }
            _ => None,
        };
        if let Some(event) = report {
            let _ = s.events.send(event);
        }
    }
}

fn button_event(detail: u8, down: bool) -> Option<CaptureEvent> {
    let button = |button| Some(CaptureEvent::Button { button, down });
    match detail {
        1 => button(MouseButton::Left),
        2 => button(MouseButton::Middle),
        3 => button(MouseButton::Right),
        8 => button(MouseButton::Back),
        9 => button(MouseButton::Forward),
        // Räder kommen als Tasten 4–7; das Loslassen trägt keine Information
        4 if down => Some(CaptureEvent::Wheel { dx: 0, dy: 120 }),
        5 if down => Some(CaptureEvent::Wheel { dx: 0, dy: -120 }),
        6 if down => Some(CaptureEvent::Wheel { dx: -120, dy: 0 }),
        7 if down => Some(CaptureEvent::Wheel { dx: 120, dy: 0 }),
        _ => None,
    }
}

fn edge_at(x: i32, y: i32, (width, height): (u16, u16)) -> Option<Edge> {
    if x <= 0 {
        Some(Edge::Left)
    } else if x >= i32::from(width) - 1 {
        Some(Edge::Right)
    } else if y <= 0 {
        Some(Edge::Top)
    } else if y >= i32::from(height) - 1 {
        Some(Edge::Bottom)
    } else {
        None
    }
}
