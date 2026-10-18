//! Erfassung über einen aktiven CGEventTap. Lokal läuft alles durch (nur Kanten werden
//! gemeldet); im Remote-Modus wird alles verschluckt, der Cursor vom Maus-Eingang entkoppelt
//! (`CGAssociateMouseAndMouseCursorPosition(false)`) und die Bewegung aus den Delta-Feldern gelesen.
//!
//! Medientasten sind unter macOS Systemereignisse, die dieser Tap nicht abfängt – sie wirken
//! deshalb immer auf dem Mac selbst.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop};
use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CallbackResult, CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    EventField,
};
use core_graphics::geometry::CGPoint;
use qross_core::keys::{HotkeyEvent, HotkeyTracker, KeyOptions};
use qross_core::CaptureEvent;
use qross_proto::{Edge, MouseButton};
use tokio::sync::mpsc::UnboundedSender;

use crate::keymap_mac::{hid_from_mac, is_modifier};
use crate::Capture;

const KVK_ESCAPE: u16 = 0x35;
const KVK_CAPS_LOCK: u16 = 0x39;
const CONTROL: [u16; 2] = [0x3B, 0x3E];
const OPTION: [u16; 2] = [0x3A, 0x3D];
const SHIFT: [u16; 2] = [0x38, 0x3C];

struct Shared {
    remote: AtomicBool,
    tap_disabled: AtomicBool,
    events: UnboundedSender<CaptureEvent>,
    keys: Mutex<KeyOptions>,
    /// Dateien vom Client warten: das nächste Loslassen meldet `LocalDrop`
    await_drop: AtomicBool,
}

#[derive(Default)]
struct TapState {
    /// Physisch gedrückte Tasten (macOS-Keycodes)
    pressed: HashSet<u16>,
    /// Vor dem Wechsel lokal gedrückte Tasten – ihr Loslassen geht lokal durch
    local_keys: HashSet<u16>,
    local_buttons: [bool; 5],
    /// Erkennt die Fang-Taste
    hotkey: HotkeyTracker,
}

pub fn start(events: UnboundedSender<CaptureEvent>) -> Result<Arc<dyn Capture>> {
    let shared = Arc::new(Shared {
        remote: AtomicBool::new(false),
        tap_disabled: AtomicBool::new(false),
        events,
        keys: Mutex::new(KeyOptions::default()),
        await_drop: AtomicBool::new(false),
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let worker = shared.clone();
    thread::Builder::new().name("qross-mac-tap".into()).spawn(move || run(worker, ready_tx))?;
    ready_rx.recv()?.map_err(|e| anyhow!(e))?;
    Ok(Arc::new(MacCapture(shared)))
}

fn run(shared: Arc<Shared>, ready: Sender<Result<(), String>>) {
    use CGEventType::*;
    let interest = vec![
        MouseMoved,
        LeftMouseDown,
        LeftMouseUp,
        RightMouseDown,
        RightMouseUp,
        OtherMouseDown,
        OtherMouseUp,
        LeftMouseDragged,
        RightMouseDragged,
        OtherMouseDragged,
        ScrollWheel,
        KeyDown,
        KeyUp,
        FlagsChanged,
    ];
    let state = RefCell::new(TapState::default());
    let callback_shared = shared.clone();
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        interest,
        move |_proxy, kind, event| handle(&callback_shared, &state, kind, event),
    );
    let Ok(tap) = tap else {
        let _ = ready.send(Err("CGEventTap nicht erlaubt – Qross unter Systemeinstellungen → Datenschutz & \
                                Sicherheit für „Bedienungshilfen“ und „Eingabeüberwachung“ freigeben. Ist es \
                                dort schon eingeschaltet, den Eintrag entfernen und neu hinzufügen"
            .into()));
        return;
    };
    let Ok(source) = tap.mach_port().create_runloop_source(0) else {
        let _ = ready.send(Err("Run-Loop-Quelle für den Event-Tap fehlt".into()));
        return;
    };
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();
    let _ = ready.send(Ok(()));
    loop {
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, Duration::from_millis(500), false);
        // macOS schaltet langsame Taps ab; dann wieder einschalten.
        if shared.tap_disabled.swap(false, Ordering::AcqRel) {
            tap.enable();
        }
    }
}

/// `Drop` verschluckt das Ereignis.
fn handle(s: &Shared, state: &RefCell<TapState>, kind: CGEventType, event: &CGEvent) -> CallbackResult {
    use CGEventType::*;
    if matches!(kind, TapDisabledByTimeout | TapDisabledByUserInput) {
        s.tap_disabled.store(true, Ordering::Release);
        return CallbackResult::Keep;
    }
    let remote = s.remote.load(Ordering::Acquire);
    let mut st = state.borrow_mut();
    let send = |e| {
        let _ = s.events.send(e);
    };

    if let Some((button, down)) = button_of(kind, event) {
        let slot = button as usize;
        if !remote {
            st.local_buttons[slot] = down;
        } else if !down && st.local_buttons[slot] {
            st.local_buttons[slot] = false;
            send(CaptureEvent::Button { button, down: false });
            return CallbackResult::Keep;
        }
    }

    let keycode = || event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    let key_change = match kind {
        KeyDown => Some((keycode(), true)),
        KeyUp => Some((keycode(), false)),
        // Modifier: Zustand umschalten; Caps Lock meldet nur das Drücken
        FlagsChanged => {
            let code = keycode();
            Some((code, code == KVK_CAPS_LOCK || !st.pressed.contains(&code)))
        }
        _ => None,
    };
    if let Some((code, down)) = key_change {
        if down {
            st.pressed.insert(code);
        } else {
            st.pressed.remove(&code);
        }
        // Fang-Taste: hält den Cursor auf dem aktuellen Bildschirm fest bzw. gibt ihn frei
        let options = *s.keys.lock().unwrap_or_else(|e| e.into_inner());
        if let (Some(key), Some(lock)) = (hid_from_mac(code), options.lock.as_ref()) {
            let result = st.hotkey.on_key(key, down, Some(lock));
            if code == KVK_CAPS_LOCK {
                // Caps Lock meldet nur das Drücken
                st.hotkey.on_key(key, false, Some(lock));
            }
            if result == HotkeyEvent::Triggered {
                send(CaptureEvent::ToggleLock);
            }
            if result != HotkeyEvent::None {
                return if remote || lock.swallow_locally() { CallbackResult::Drop } else { CallbackResult::Keep };
            }
        }
    }

    if !remote {
        if matches!(kind, LeftMouseUp | RightMouseUp | OtherMouseUp) && s.await_drop.swap(false, Ordering::AcqRel) {
            // Dateien vom Client: hier losgelassen = hier ablegen
            let p = event.location();
            send(CaptureEvent::LocalDrop { x: p.x as i32, y: p.y as i32 });
        }
        if let Some((code, down)) = key_change {
            if down && code != KVK_CAPS_LOCK {
                st.local_keys.insert(code);
            } else {
                st.local_keys.remove(&code);
            }
        }
        if matches!(kind, MouseMoved | LeftMouseDragged | RightMouseDragged | OtherMouseDragged) {
            let p = event.location();
            if let Some(edge) = outer_edge_at(p) {
                let buttons_down = !matches!(kind, MouseMoved);
                send(CaptureEvent::EdgeHit { edge, x: p.x as i32, y: p.y as i32, buttons_down });
            }
        }
        return CallbackResult::Keep;
    }

    let report = match kind {
        MouseMoved | LeftMouseDragged | RightMouseDragged | OtherMouseDragged => {
            let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as i32;
            let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as i32;
            (dx != 0 || dy != 0).then_some(CaptureEvent::MouseDelta { dx, dy })
        }
        ScrollWheel => {
            let lines = |f| event.get_integer_value_field(f) as i32;
            let points = |f| event.get_integer_value_field(f) as i32;
            let (mut dy, mut dx) = (
                lines(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1) * 120,
                lines(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2) * 120,
            );
            if dy == 0 && dx == 0 {
                // Trackpad: feine Punkt-Deltas, ca. 10 Punkte je Raste
                dy = points(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1) * 12;
                dx = points(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2) * 12;
            }
            let clamp = |v: i32| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
            (dx != 0 || dy != 0).then_some(CaptureEvent::Wheel { dx: clamp(dx), dy: clamp(dy) })
        }
        KeyDown | KeyUp | FlagsChanged => {
            let (code, down) = key_change.expect("Tastenereignis");
            if st.local_keys.contains(&code) {
                if !down {
                    st.local_keys.remove(&code);
                    return CallbackResult::Keep;
                }
                return CallbackResult::Drop;
            }
            let held = |keys: [u16; 2]| keys.iter().any(|k| st.pressed.contains(k));
            if code == KVK_ESCAPE && down && held(CONTROL) && held(OPTION) && held(SHIFT) {
                // Notausgang Strg+Option+Shift+Esc
                s.remote.store(false, Ordering::Release);
                set_detached(false);
                send(CaptureEvent::PanicReturn);
                return CallbackResult::Drop;
            }
            match hid_from_mac(code) {
                Some(key) if code == KVK_CAPS_LOCK => {
                    send(CaptureEvent::Key { key, down: true });
                    Some(CaptureEvent::Key { key, down: false })
                }
                Some(key) if is_modifier(code) || matches!(kind, KeyDown | KeyUp) => Some(CaptureEvent::Key { key, down }),
                _ => None,
            }
        }
        _ => button_of(kind, event).map(|(button, down)| CaptureEvent::Button { button, down }),
    };
    if let Some(event) = report {
        send(event);
    }
    CallbackResult::Drop
}

fn button_of(kind: CGEventType, event: &CGEvent) -> Option<(MouseButton, bool)> {
    use CGEventType::*;
    let other = || match event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER) {
        3 => MouseButton::Back,
        4 => MouseButton::Forward,
        _ => MouseButton::Middle,
    };
    Some(match kind {
        LeftMouseDown => (MouseButton::Left, true),
        LeftMouseUp => (MouseButton::Left, false),
        RightMouseDown => (MouseButton::Right, true),
        RightMouseUp => (MouseButton::Right, false),
        OtherMouseDown => (other(), true),
        OtherMouseUp => (other(), false),
        _ => return None,
    })
}

/// Kante eines Monitors, hinter der kein weiterer Monitor liegt
fn outer_edge_at(p: CGPoint) -> Option<Edge> {
    let displays = super::displays();
    let inside = |x: f64, y: f64| {
        displays.iter().any(|r| {
            x >= r.origin.x && x < r.origin.x + r.size.width && y >= r.origin.y && y < r.origin.y + r.size.height
        })
    };
    let monitor = displays.iter().find(|r| {
        p.x >= r.origin.x && p.x < r.origin.x + r.size.width && p.y >= r.origin.y && p.y < r.origin.y + r.size.height
    })?;
    let candidates = [
        (p.x <= monitor.origin.x, Edge::Left, (p.x - 1.0, p.y)),
        (p.x >= monitor.origin.x + monitor.size.width - 1.0, Edge::Right, (p.x + 1.0, p.y)),
        (p.y <= monitor.origin.y, Edge::Top, (p.x, p.y - 1.0)),
        (p.y >= monitor.origin.y + monitor.size.height - 1.0, Edge::Bottom, (p.x, p.y + 1.0)),
    ];
    candidates.into_iter().find(|&(at, _, (bx, by))| at && !inside(bx, by)).map(|(_, edge, _)| edge)
}

/// Cursor vom Maus-Eingang entkoppeln (Remote) bzw. wieder koppeln (lokal).
fn set_detached(detached: bool) {
    let _ = CGDisplay::associate_mouse_and_mouse_cursor_position(!detached);
    let main = CGDisplay::main();
    let _ = if detached { main.hide_cursor() } else { main.show_cursor() };
}

struct MacCapture(Arc<Shared>);

impl Capture for MacCapture {
    fn set_key_options(&self, options: KeyOptions) {
        *self.0.keys.lock().unwrap_or_else(|e| e.into_inner()) = options;
    }

    fn await_local_drop(&self) {
        self.0.await_drop.store(true, Ordering::Release);
    }

    fn capture_remote(&self) {
        self.0.remote.store(true, Ordering::Release);
        let b = CGDisplay::main().bounds();
        let center = CGPoint::new(b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0);
        let _ = CGDisplay::warp_mouse_cursor_position(center);
        set_detached(true);
    }

    fn release(&self, x: i32, y: i32) {
        self.0.remote.store(false, Ordering::Release);
        let _ = CGDisplay::warp_mouse_cursor_position(CGPoint::new(x.into(), y.into()));
        set_detached(false);
    }
}

impl Drop for MacCapture {
    fn drop(&mut self) {
        self.0.remote.store(false, Ordering::Release);
        set_detached(false);
    }
}
