//! Eingabe-Erfassung unter X11.
//!
//! Lokaler Modus: Zeigerposition alle 4 ms abfragen und Kanten melden. Gegriffen werden nur die
//! Kürzel (Fang-Taste, Wechsel-Kürzel; passiver Tastengriff), sonst verhält sich das System
//! völlig normal.
//! Remote-Modus: Zeiger und Tastatur auf dem Root-Fenster grabben, Cursor durch einen leeren
//! ersetzen und nach jeder Bewegung in die Mitte zurücksetzen.
//!
//! Medientasten gehen unter X11 immer an den aktiven Rechner: Während des Tastatur-Grabs lässt
//! sich eine einzelne Taste nicht lokal durchreichen.
//!
//! Im Remote-Modus meldet die Erfassung zu jeder Zeichentaste, was sie in der Belegung des
//! X-Servers ergibt (`CaptureEvent::Typed`, für Clients im Zeichenmodus).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use qross_core::chars::is_typing_key;
use qross_core::keys::{HotkeyEvent, HotkeyTracker, KeyOptions, Mods};
use qross_core::CaptureEvent;
use qross_proto::{Edge, KeyCode, MouseButton};
use tokio::sync::mpsc::UnboundedSender;
use x11rb::connection::Connection;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, Cursor, EventMask, GrabMode, GrabStatus, KeyButMask, ModMask, Window};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use super::keymap::Keymap;
use crate::keymap_linux::{evdev_from_hid, hid_from_evdev, X11_OFFSET};
use crate::Capture;

/// evdev-Codes für den Notausgang
const KEY_ESC: u16 = 1;
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
    keys: Mutex<KeyOptions>,
    /// Passiv gegriffene Kürzel (X11-Keycode, Zusatztasten)
    grabbed: Mutex<Vec<(u8, ModMask)>>,
    /// Dateien vom Client warten: das nächste Loslassen meldet `LocalDrop`
    await_drop: AtomicBool,
}

/// Feststell- und Num-Taste sollen die Kürzel nicht verhindern.
fn lock_variants() -> [ModMask; 4] {
    [ModMask::from(0u16), ModMask::LOCK, ModMask::M2, ModMask::LOCK | ModMask::M2]
}

fn mod_mask(mods: Mods) -> ModMask {
    let mut mask = ModMask::from(0u16);
    for (on, bit) in [(mods.ctrl, ModMask::CONTROL), (mods.shift, ModMask::SHIFT), (mods.alt, ModMask::M1), (mods.meta, ModMask::M4)] {
        if on {
            mask |= bit;
        }
    }
    mask
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
    // Gehaltene Tasten wiederholen nur KeyPress, ohne künstliche KeyRelease dazwischen
    let xkb_ok = conn.xkb_use_extension(1, 0).ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.supported);
    if xkb_ok {
        let flag = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
        let device = xkb::DeviceSpec::from(xkb::ID::USE_CORE_KBD);
        let none = xkb::BoolCtrl::from(0u32);
        let _ = conn.xkb_per_client_flags(device, flag, flag, none, none, none);
    }
    let shared = Arc::new(Shared {
        conn,
        root,
        blank,
        remote: AtomicBool::new(false),
        events,
        keys: Mutex::new(KeyOptions::default()),
        grabbed: Mutex::new(Vec::new()),
        await_drop: AtomicBool::new(false),
    });
    X11Capture(shared.clone()).set_key_options(KeyOptions::default());
    let worker = shared.clone();
    thread::Builder::new().name("qross-x11-capture".into()).spawn(move || {
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
    fn set_key_options(&self, options: KeyOptions) {
        let s = &self.0;
        let mut grabbed = s.grabbed.lock().unwrap_or_else(|e| e.into_inner());
        for (code, mods) in grabbed.drain(..) {
            for extra in lock_variants() {
                let _ = s.conn.ungrab_key(code, s.root, mods | extra);
            }
        }
        for hotkey in options.hotkeys() {
            let Some(code) = evdev_from_hid(hotkey.key).and_then(|code| u8::try_from(code + X11_OFFSET).ok()) else {
                tracing::warn!("Kürzel „{hotkey}“: Diese Taste gibt es unter X11 nicht");
                continue;
            };
            let mods = mod_mask(hotkey.mods);
            if grabbed.contains(&(code, mods)) {
                continue;
            }
            // Erst alle Varianten greifen, dann prüfen, ob ein anderes Programm schneller war
            let cookies: Vec<_> = lock_variants()
                .into_iter()
                .filter_map(|extra| s.conn.grab_key(true, s.root, mods | extra, code, GrabMode::ASYNC, GrabMode::ASYNC).ok())
                .collect();
            let taken = cookies.into_iter().any(|cookie| cookie.check().is_err());
            if taken {
                tracing::warn!(
                    "Kürzel „{hotkey}“ ist schon von einem anderen Programm belegt; es wirkt nur, solange der Cursor \
                     auf einem anderen Rechner ist"
                );
            }
            grabbed.push((code, mods));
        }
        *s.keys.lock().unwrap_or_else(|e| e.into_inner()) = options;
        let _ = s.conn.flush();
    }

    fn await_local_drop(&self) {
        self.0.await_drop.store(true, Ordering::Release);
    }

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
    let mut hotkey = HotkeyTracker::new();
    let mut was_remote = false;
    // Belegung für den Zeichenmodus; ohne sie bleibt es bei Tastenpositionen
    let mut keymap = Keymap::fetch(&s.conn).map_err(|e| tracing::warn!("Tastaturbelegung nicht lesbar: {e:#}")).ok();

    loop {
        let remote = s.remote.load(Ordering::Acquire);
        if remote && !was_remote {
            // Was lokal an Zusatztasten passierte, hat die Erfassung nicht gesehen.
            sync_modifiers(&s.conn, &mut hotkey)?;
        } else if !remote && was_remote {
            // Das Loslassen gedrückter Kürzel-Tasten geht jetzt an andere Programme.
            hotkey.forget_keys();
        }
        was_remote = remote;
        if !remote {
            if size_checked.elapsed() > Duration::from_secs(1) {
                size = super::root_size(&s.conn, s.root)?;
                size_checked = Instant::now();
            }
            let pointer = s.conn.query_pointer(s.root)?.reply()?;
            let (x, y) = (i32::from(pointer.root_x), i32::from(pointer.root_y));
            let buttons = u16::from(pointer.mask) & ANY_BUTTON_MASK != 0;
            if !buttons && s.await_drop.swap(false, Ordering::AcqRel) {
                // Dateien vom Client: hier losgelassen = hier ablegen
                let _ = s.events.send(CaptureEvent::LocalDrop { x, y });
            }
            if let Some(edge) = edge_at(x, y, size) {
                let buttons_down = buttons;
                let _ = s.events.send(CaptureEvent::EdgeHit { edge, x, y, buttons_down });
            }
            // Lokal kommen nur die passiv gegriffenen Kürzel an; Reste der Remote-Phase verwerfen.
            while let Some(event) = s.conn.poll_for_event()? {
                if let Event::MappingNotify(_) = event {
                    keymap = Keymap::fetch(&s.conn).ok();
                    continue;
                }
                let (Event::KeyPress(e) | Event::KeyRelease(e)) = &event else { continue };
                let down = matches!(event, Event::KeyPress(_));
                let grabbed = s.grabbed.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|&(code, _)| code == e.detail);
                let Some(key) = hid_from_evdev(u16::from(e.detail).wrapping_sub(X11_OFFSET)).filter(|_| grabbed) else {
                    continue;
                };
                if down {
                    // Zusatztasten so, wie sie beim Drücken waren: Bis die Erfassung das Ereignis liest
                    // (alle 4 ms), können sie schon wieder losgelassen sein.
                    hotkey.sync_modifiers(modifiers_in(e.state));
                }
                let options = s.keys.lock().unwrap_or_else(|e| e.into_inner());
                if let HotkeyEvent::Triggered { event, .. } = hotkey.on_key(key, down, &options) {
                    let _ = s.events.send(event);
                }
            }
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
                if down {
                    pressed.insert(code);
                } else {
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
                } else if let Some(key) = hid_from_evdev(code) {
                    let options = s.keys.lock().unwrap_or_else(|e| e.into_inner());
                    match hotkey.on_key(key, down, &options) {
                        HotkeyEvent::Triggered { event, .. } => Some(event),
                        HotkeyEvent::Consumed { .. } => None,
                        HotkeyEvent::None => Some(match keymap.as_ref().filter(|_| down && is_typing_key(key)) {
                            Some(keymap) => CaptureEvent::Typed { key, chars: keymap.chars(e.detail, u16::from(e.state)) },
                            None => CaptureEvent::Key { key, down },
                        }),
                    }
                } else {
                    None
                }
            }
            Event::MappingNotify(_) => {
                keymap = Keymap::fetch(&s.conn).ok();
                None
            }
            _ => None,
        };
        if let Some(event) = report {
            let _ = s.events.send(event);
        }
    }
}

/// Beim Wechsel in den Remote-Modus die gerade gedrückten Zusatztasten vom X-Server übernehmen:
/// Im lokalen Modus sieht die Erfassung nur die gegriffenen Kürzel, nicht Strg, Alt und Co.
fn sync_modifiers(conn: &RustConnection, hotkey: &mut HotkeyTracker) -> Result<()> {
    let keys = conn.query_keymap()?.reply()?.keys;
    hotkey.sync_modifiers(|key: KeyCode| {
        let Some(code) = evdev_from_hid(key).map(|code| usize::from(code + X11_OFFSET)) else { return false };
        keys.get(code / 8).is_some_and(|byte| byte & (1 << (code % 8)) != 0)
    });
    Ok(())
}

/// Zusatztasten im Zustand eines Tastenereignisses. Links und rechts sind dort nicht zu
/// unterscheiden; für den Vergleich mit einem Kürzel genügt das (gezählt wird die linke).
fn modifiers_in(state: KeyButMask) -> impl Fn(KeyCode) -> bool {
    let state = u16::from(state);
    move |key| {
        let mask = match key.0 {
            0xE0 => KeyButMask::CONTROL,
            0xE1 => KeyButMask::SHIFT,
            0xE2 => KeyButMask::MOD1,
            0xE3 => KeyButMask::MOD4,
            _ => return false,
        };
        state & u16::from(mask) != 0
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
