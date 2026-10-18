//! Eingaben per CGEventPost einspeisen. Damit Doppelklicks und Ziehen funktionieren, setzt
//! macOS eigene Angaben voraus: Klickzähler, „Dragged“-Bewegungen bei gedrückter Taste und
//! Modifier-Flags an jedem Tastenereignis.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use qross_core::Inject;
use qross_proto::MouseButton;

use super::layout;

use crate::keymap_mac::mac_from_hid;
use crate::Injector;

const DOUBLE_CLICK: Duration = Duration::from_millis(500);
const NOTCH: i32 = 120;

pub struct MacInjector {
    source: CGEventSource,
    pos: CGPoint,
    buttons: [bool; 5],
    flags: CGEventFlags,
    /// Letzter Klick: Taste, Zeitpunkt, Zähler (für Doppel-/Dreifachklick)
    last_click: Option<(MouseButton, Instant, i64)>,
    wheel_rest: (i32, i32),
    /// Stand der Drag-Zwischenablage beim Drücken der linken Maustaste
    drag_mark: Option<isize>,
}

// SAFETY: Der Injector wird einmal in den Einspeise-Thread verschoben und nur dort benutzt.
// CGEventSource ist ein CoreFoundation-Objekt mit Referenzzählung; ein Wechsel des
// besitzenden Threads ohne gleichzeitige Nutzung ist zulässig.
unsafe impl Send for MacInjector {}

impl MacInjector {
    pub fn new() -> Result<Self> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("CGEventSource nicht verfügbar (Bedienungshilfen-Recht fehlt?)"))?;
        // Tastaturlayout für Kürzel im Zeichenmodus
        layout::refresh(Duration::ZERO);
        Ok(Self {
            source,
            pos: CGPoint::new(0.0, 0.0),
            buttons: [false; 5],
            flags: CGEventFlags::CGEventFlagNull,
            last_click: None,
            wheel_rest: (0, 0),
            drag_mark: None,
        })
    }

    fn mouse(&self, kind: CGEventType, button: CGMouseButton) -> Option<CGEvent> {
        CGEvent::new_mouse_event(self.source.clone(), kind, self.pos, button).ok()
    }

    fn post(&self, event: Option<CGEvent>) {
        match event {
            Some(event) => event.post(CGEventTapLocation::HID),
            None => tracing::warn!("CGEvent konnte nicht erzeugt werden"),
        }
    }

    fn flag_for(code: u16) -> CGEventFlags {
        match code {
            0x38 | 0x3C => CGEventFlags::CGEventFlagShift,
            0x3B | 0x3E => CGEventFlags::CGEventFlagControl,
            0x3A | 0x3D => CGEventFlags::CGEventFlagAlternate,
            0x36 | 0x37 => CGEventFlags::CGEventFlagCommand,
            0x39 => CGEventFlags::CGEventFlagAlphaShift,
            _ => CGEventFlags::CGEventFlagNull,
        }
    }
}

impl MacInjector {
    /// Bewegung an `self.pos`; mit gedrückter Taste als „Dragged“. Die Deltas lesen Spiele, die
    /// den Zeiger vom Cursor gelöst haben (Spielemodus).
    fn post_move(&self, dx: i32, dy: i32) {
        let (kind, button) = if self.buttons[MouseButton::Left as usize] {
            (CGEventType::LeftMouseDragged, CGMouseButton::Left)
        } else if self.buttons[MouseButton::Right as usize] {
            (CGEventType::RightMouseDragged, CGMouseButton::Right)
        } else if self.buttons.iter().any(|b| *b) {
            (CGEventType::OtherMouseDragged, CGMouseButton::Center)
        } else {
            (CGEventType::MouseMoved, CGMouseButton::Left)
        };
        let event = self.mouse(kind, button);
        if let Some(e) = &event {
            if (dx, dy) != (0, 0) {
                e.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, dx.into());
                e.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, dy.into());
            }
        }
        self.post(event);
    }

    fn tap_escape(&self) {
        const KVK_ESCAPE: u16 = 0x35;
        for down in [true, false] {
            self.post(CGEvent::new_keyboard_event(self.source.clone(), KVK_ESCAPE, down).ok());
        }
    }
}

impl Injector for MacInjector {
    fn follows_screen_saver(&self) -> bool {
        true
    }

    fn screen_saver(&mut self, active: bool, lock: bool) -> crate::screensaver::Outcome {
        crate::screensaver::follow(active, lock)
    }

    fn types_text(&self) -> bool {
        true
    }

    fn take_dragged_files(&mut self, _x: i32, _y: i32) -> Vec<std::path::PathBuf> {
        // Nur ein Drag, der nach dem Drücken der Taste begonnen hat, zählt.
        let Some(mark) = self.drag_mark.filter(|_| self.buttons[MouseButton::Left as usize]) else { return Vec::new() };
        if super::drag::change_count() == mark {
            return Vec::new();
        }
        let files = super::drag::dragged_files();
        if !files.is_empty() {
            // Esc bricht den Drag im Finder ab – die Dateien wandern stattdessen zum Server.
            self.tap_escape();
            self.drag_mark = None;
        }
        files
    }

    fn inject(&mut self, event: &Inject) {
        match *event {
            Inject::MoveTo { x, y } => {
                self.pos = CGPoint::new(x.into(), y.into());
                self.post_move(0, 0);
            }
            Inject::MoveBy { dx, dy } => {
                // Vom tatsächlichen Cursor aus: Ein Spiel setzt ihn womöglich selbst zurück.
                let here = CGEvent::new(self.source.clone()).map(|e| e.location()).unwrap_or(self.pos);
                let b = super::screen_bounds();
                let x = (here.x + f64::from(dx)).clamp(b.x.into(), b.right().into());
                let y = (here.y + f64::from(dy)).clamp(b.y.into(), b.bottom().into());
                self.pos = CGPoint::new(x, y);
                self.post_move(dx, dy);
            }
            Inject::Button { button, down } => {
                self.buttons[button as usize] = down;
                if button == MouseButton::Left {
                    self.drag_mark = down.then(super::drag::change_count);
                }
                let (kind, cg_button, number) = match (button, down) {
                    (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left, 0),
                    (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left, 0),
                    (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right, 1),
                    (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right, 1),
                    (other, down) => {
                        let number = match other {
                            MouseButton::Back => 3,
                            MouseButton::Forward => 4,
                            _ => 2,
                        };
                        let kind = if down { CGEventType::OtherMouseDown } else { CGEventType::OtherMouseUp };
                        (kind, CGMouseButton::Center, number)
                    }
                };
                if down {
                    let count = match self.last_click {
                        Some((b, at, n)) if b == button && at.elapsed() < DOUBLE_CLICK => n + 1,
                        _ => 1,
                    };
                    self.last_click = Some((button, Instant::now(), count));
                }
                let count = self.last_click.map_or(1, |(_, _, n)| n);
                let event = self.mouse(kind, cg_button);
                if let Some(e) = &event {
                    e.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, count);
                    e.set_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER, number);
                }
                self.post(event);
            }
            Inject::Wheel { dx, dy } => {
                self.wheel_rest.0 += i32::from(dx);
                self.wheel_rest.1 += i32::from(dy);
                let lines_y = self.wheel_rest.1 / NOTCH;
                let lines_x = self.wheel_rest.0 / NOTCH;
                self.wheel_rest.1 -= lines_y * NOTCH;
                self.wheel_rest.0 -= lines_x * NOTCH;
                if lines_x != 0 || lines_y != 0 {
                    let event =
                        CGEvent::new_scroll_event(self.source.clone(), ScrollEventUnit::LINE, 2, lines_y, lines_x, 0).ok();
                    self.post(event);
                }
            }
            Inject::Key { key, down } => {
                if super::media::post(key, down) {
                    return;
                }
                let Some(code) = mac_from_hid(key) else {
                    tracing::debug!(?key, "Taste ohne macOS-Zuordnung");
                    return;
                };
                let flag = Self::flag_for(code);
                if flag != CGEventFlags::CGEventFlagNull {
                    if down {
                        self.flags |= flag;
                    } else {
                        self.flags.remove(flag);
                    }
                }
                let event = CGEvent::new_keyboard_event(self.source.clone(), code, down).ok();
                if let Some(e) = &event {
                    e.set_flags(self.flags);
                }
                self.post(event);
            }
            Inject::Char { ch, down } => {
                let event = CGEvent::new_keyboard_event(self.source.clone(), 0, down).ok();
                if let Some(e) = &event {
                    let mut buf = [0u16; 2];
                    e.set_string_from_utf16_unchecked(ch.encode_utf16(&mut buf));
                    // Das Zeichen steht fest; mit Option o. Ä. wäre es womöglich ein Kürzel.
                    e.set_flags(CGEventFlags::CGEventFlagNull);
                }
                self.post(event);
            }
            Inject::Shortcut { ch, key, down } => {
                // Menüs prüfen das Zeichen der Taste: Cmd+Z braucht die Taste, die hier z ergibt.
                layout::refresh(Duration::from_secs(2));
                match layout::current().and_then(|layout| layout.find(ch)) {
                    Some((code, shift)) => {
                        let event = CGEvent::new_keyboard_event(self.source.clone(), code, down).ok();
                        if let Some(e) = &event {
                            e.set_flags(if shift { self.flags | CGEventFlags::CGEventFlagShift } else { self.flags });
                        }
                        self.post(event);
                    }
                    None => self.inject(&Inject::Key { key, down }),
                }
            }
        }
    }
}
