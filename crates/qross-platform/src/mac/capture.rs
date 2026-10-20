//! Erfassung über einen aktiven CGEventTap. Lokal läuft alles durch (nur Kanten werden
//! gemeldet); im Remote-Modus wird alles verschluckt, der Cursor vom Maus-Eingang entkoppelt
//! (`CGAssociateMouseAndMouseCursorPosition(false)`) und die Bewegung aus den Delta-Feldern gelesen.
//!
//! Medientasten sind unter macOS Systemereignisse, die dieser Tap nicht abfängt – sie wirken
//! deshalb immer auf dem Mac selbst.
//!
//! Im Remote-Modus meldet der Tap zu jeder Zeichentaste, was sie im Tastaturlayout der aktuellen
//! Eingabequelle ergibt (`CaptureEvent::Typed`, für Clients im Zeichenmodus).
//!
//! Die Feststelltaste schaltet der Mac selbst um, bevor der Tap sie sieht; ihr neuer Stand geht als
//! `CaptureEvent::Locks` an den Client. Num und Rollen gibt es auf Mac-Tastaturen nicht.
//!
//! Drag & Drop zum Client: Erreicht der Cursor mit gedrückter Taste die Kante, prüft der Server, ob
//! seit dem Drücken ein Drag mit Dateien begonnen hat (Drag-Zwischenablage, siehe `drag`). Dann bricht
//! ein eingespeistes Esc den Drag im Finder ab, und die Dateien gehen an den Client.

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
    CallbackResult, CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use qross_core::chars::is_typing_key;
use qross_core::keys::{HotkeyEvent, HotkeyTracker, KeyOptions};
use qross_core::CaptureEvent;
use qross_proto::{Edge, LockState, MouseButton};
use tokio::sync::mpsc::UnboundedSender;

use super::drag::DragWatch;
use super::layout;
use crate::keymap_mac::{hid_from_mac, is_modifier};
use crate::Capture;

const KVK_ESCAPE: u16 = 0x35;
const KVK_CAPS_LOCK: u16 = 0x39;
const CONTROL: [u16; 2] = [0x3B, 0x3E];
const OPTION: [u16; 2] = [0x3A, 0x3D];
const SHIFT: [u16; 2] = [0x38, 0x3C];
/// Kennung selbst eingespeister Ereignisse im Feld „Source User Data“: Der Tap lässt sie durch.
const OWN_EVENT: i64 = 0x5152_4F53;

struct Shared {
    remote: AtomicBool,
    tap_disabled: AtomicBool,
    events: UnboundedSender<CaptureEvent>,
    keys: Mutex<KeyOptions>,
    /// Dateien vom Client warten: das nächste Loslassen meldet `LocalDrop`
    await_drop: AtomicBool,
    /// Erkennt einen Drag, der nach dem Drücken der linken Maustaste begonnen hat
    drag: Mutex<DragWatch>,
}

#[derive(Default)]
struct TapState {
    /// Physisch gedrückte Tasten (macOS-Keycodes)
    pressed: HashSet<u16>,
    /// Vor dem Wechsel lokal gedrückte Tasten – ihr Loslassen geht lokal durch
    local_keys: HashSet<u16>,
    local_buttons: [bool; 5],
    /// Erkennt Fang-Taste und Wechsel-Kürzel
    hotkey: HotkeyTracker,
}

pub fn start(events: UnboundedSender<CaptureEvent>) -> Result<Arc<dyn Capture>> {
    let shared = Arc::new(Shared {
        remote: AtomicBool::new(false),
        tap_disabled: AtomicBool::new(false),
        events,
        keys: Mutex::new(KeyOptions::default()),
        await_drop: AtomicBool::new(false),
        drag: Mutex::new(DragWatch::new()),
    });
    // Tastaturlayout für den Zeichenmodus
    layout::refresh(Duration::ZERO);
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
    if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == OWN_EVENT {
        // Esc zum Abbrechen eines Drags, der zum Client wandert: gilt immer lokal
        return CallbackResult::Keep;
    }
    let remote = s.remote.load(Ordering::Acquire);
    let mut st = state.borrow_mut();
    let send = |e| {
        let _ = s.events.send(e);
    };

    if let Some((button, down)) = button_of(kind, event) {
        if button == MouseButton::Left {
            // Ein Drag im Finder beginnt erst nach dem Drücken; hier den Stand der Drag-Zwischenablage merken.
            let mut drag = s.drag.lock().unwrap_or_else(|e| e.into_inner());
            if !down {
                drag.release();
            } else if !remote {
                drag.press();
            }
        }
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
        // Fang-Taste (hält den Cursor fest bzw. gibt ihn frei) und Wechsel-Kürzel
        if let Some(key) = hid_from_mac(code) {
            let options = s.keys.lock().unwrap_or_else(|e| e.into_inner());
            let hotkey = st.hotkey.on_key(key, down, &options);
            if code == KVK_CAPS_LOCK {
                // Caps Lock meldet nur das Drücken
                st.hotkey.on_key(key, false, &options);
            }
            if let HotkeyEvent::Tapped { event } = hotkey {
                // Zusatztaste allein getippt: melden; ihr Loslassen geht weiter, wohin ihr Drücken ging
                send(event);
            } else if hotkey != HotkeyEvent::None {
                let swallow = hotkey.swallow(remote);
                if let HotkeyEvent::Triggered { event, .. } = hotkey {
                    send(event);
                }
                return if swallow { CallbackResult::Drop } else { CallbackResult::Keep };
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
                    // Schon umgeschaltet, die Flags tragen den neuen Stand. Clients ab Qross 0.8
                    // übernehmen ihn, ältere bekommen die Taste. Durchlassen, damit auch die
                    // Programme hier ihn sehen (wie unter Windows).
                    let caps = event.get_flags().contains(CGEventFlags::CGEventFlagAlphaShift);
                    send(CaptureEvent::Locks(LockState { caps: Some(caps), num: None, scroll: None }));
                    send(CaptureEvent::Key { key, down: true });
                    send(CaptureEvent::Key { key, down: false });
                    return CallbackResult::Keep;
                }
                // Zeichenmodus: was die Taste im Layout ergibt (ohne gelesenes Layout: Position)
                Some(key) if matches!(kind, KeyDown) && is_typing_key(key) => Some(match layout::current() {
                    Some(layout) => CaptureEvent::Typed { key, chars: layout.key_chars(code, event.get_flags()) },
                    None => CaptureEvent::Key { key, down },
                }),
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
    fn screen_saver_active(&self) -> bool {
        crate::screensaver::running()
    }

    fn stop_screen_saver(&self) {
        crate::screensaver::stop_in_background();
    }

    fn set_key_options(&self, options: KeyOptions) {
        *self.0.keys.lock().unwrap_or_else(|e| e.into_inner()) = options;
    }

    fn await_local_drop(&self) {
        self.0.await_drop.store(true, Ordering::Release);
    }

    fn locks(&self) -> LockState {
        super::locks::current()
    }

    fn probe_drag(&self, _x: i32, _y: i32) {
        let files = self.0.drag.lock().unwrap_or_else(|e| e.into_inner()).probe();
        if !files.is_empty() {
            tracing::info!("{} Datei(en) werden über die Kante gezogen", files.len());
        }
        let _ = self.0.events.send(CaptureEvent::DragProbeResult { files });
    }

    fn cancel_local_drag(&self) {
        // Esc bricht den Drag im Finder ab; die Taste bleibt gedrückt, ihr Loslassen legt dann beim Client ab.
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else { return };
        for down in [true, false] {
            if let Ok(event) = CGEvent::new_keyboard_event(source.clone(), KVK_ESCAPE, down) {
                event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, OWN_EVENT);
                event.post(CGEventTapLocation::HID);
            }
        }
    }

    fn drop_folder(&self, x: i32, y: i32) -> Option<std::path::PathBuf> {
        super::finder_folder::folder_at(x, y)
    }

    fn capture_remote(&self) {
        self.0.remote.store(true, Ordering::Release);
        // Die Eingabequelle kann sich seit dem letzten Mal geändert haben
        layout::refresh(Duration::ZERO);
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
