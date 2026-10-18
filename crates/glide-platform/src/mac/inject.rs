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
use glide_core::Inject;
use glide_proto::MouseButton;

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
}

// SAFETY: Der Injector wird einmal in den Einspeise-Thread verschoben und nur dort benutzt.
// CGEventSource ist ein CoreFoundation-Objekt mit Referenzzählung; ein Wechsel des
// besitzenden Threads ohne gleichzeitige Nutzung ist zulässig.
unsafe impl Send for MacInjector {}

impl MacInjector {
    pub fn new() -> Result<Self> {
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .map_err(|_| anyhow!("CGEventSource nicht verfügbar (Bedienungshilfen-Recht fehlt?)"))?;
        Ok(Self {
            source,
            pos: CGPoint::new(0.0, 0.0),
            buttons: [false; 5],
            flags: CGEventFlags::CGEventFlagNull,
            last_click: None,
            wheel_rest: (0, 0),
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

impl Injector for MacInjector {
    fn inject(&mut self, event: &Inject) {
        match *event {
            Inject::MoveTo { x, y } => {
                self.pos = CGPoint::new(x.into(), y.into());
                let (kind, button) = if self.buttons[MouseButton::Left as usize] {
                    (CGEventType::LeftMouseDragged, CGMouseButton::Left)
                } else if self.buttons[MouseButton::Right as usize] {
                    (CGEventType::RightMouseDragged, CGMouseButton::Right)
                } else if self.buttons.iter().any(|b| *b) {
                    (CGEventType::OtherMouseDragged, CGMouseButton::Center)
                } else {
                    (CGEventType::MouseMoved, CGMouseButton::Left)
                };
                self.post(self.mouse(kind, button));
            }
            Inject::Button { button, down } => {
                self.buttons[button as usize] = down;
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
        }
    }
}
