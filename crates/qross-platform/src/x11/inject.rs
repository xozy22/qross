//! Eingaben über die XTest-Erweiterung einspeisen.

use anyhow::Result;
use qross_core::Inject;
use qross_proto::MouseButton;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Window, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use crate::keymap_linux::{evdev_from_hid, X11_OFFSET};
use crate::Injector;

/// 120 = eine Raste (wie Windows); X11 kennt nur ganze Rasten.
const NOTCH: i32 = 120;

pub struct X11Injector {
    conn: RustConnection,
    root: Window,
    /// Noch nicht ausgegebene Teil-Rasten (hochauflösende Räder)
    wheel_rest: (i32, i32),
}

impl X11Injector {
    pub fn new() -> Result<Self> {
        let (conn, root) = super::connect()?;
        Ok(Self { conn, root, wheel_rest: (0, 0) })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
        if let Err(e) = self.conn.xtest_fake_input(kind, detail, CURRENT_TIME, self.root, x, y, 0) {
            tracing::warn!("XTest fehlgeschlagen: {e}");
        }
    }

    fn click(&self, button: u8) {
        self.fake(BUTTON_PRESS_EVENT, button, 0, 0);
        self.fake(BUTTON_RELEASE_EVENT, button, 0, 0);
    }
}

impl Injector for X11Injector {
    fn inject(&mut self, event: &Inject) {
        match *event {
            Inject::MoveTo { x, y } => {
                let clamp = |v: i32| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
                // detail 0 = absolute Position
                self.fake(MOTION_NOTIFY_EVENT, 0, clamp(x), clamp(y));
            }
            Inject::MoveBy { dx, dy } => {
                // detail 1 = relativ, wie XTestFakeRelativeMotionEvent (dort ohne Root-Fenster)
                let clamp = |v: i32| v.clamp(i16::MIN.into(), i16::MAX.into()) as i16;
                if let Err(e) = self.conn.xtest_fake_input(MOTION_NOTIFY_EVENT, 1, CURRENT_TIME, NONE, clamp(dx), clamp(dy), 0) {
                    tracing::warn!("XTest fehlgeschlagen: {e}");
                }
            }
            Inject::Button { button, down } => {
                let detail = match button {
                    MouseButton::Left => 1,
                    MouseButton::Middle => 2,
                    MouseButton::Right => 3,
                    MouseButton::Back => 8,
                    MouseButton::Forward => 9,
                };
                self.fake(if down { BUTTON_PRESS_EVENT } else { BUTTON_RELEASE_EVENT }, detail, 0, 0);
            }
            Inject::Wheel { dx, dy } => {
                self.wheel_rest.0 += i32::from(dx);
                self.wheel_rest.1 += i32::from(dy);
                // X11: 4 = hoch, 5 = runter, 6 = links, 7 = rechts
                for (rest, positive, negative) in [(1, 4u8, 5u8), (0, 7, 6)] {
                    loop {
                        let value = if rest == 1 { &mut self.wheel_rest.1 } else { &mut self.wheel_rest.0 };
                        let button = if *value >= NOTCH {
                            *value -= NOTCH;
                            positive
                        } else if *value <= -NOTCH {
                            *value += NOTCH;
                            negative
                        } else {
                            break;
                        };
                        self.click(button);
                    }
                }
            }
            Inject::Key { key, down } => match evdev_from_hid(key) {
                Some(code) => {
                    let keycode = (code + X11_OFFSET).min(255) as u8;
                    self.fake(if down { KEY_PRESS_EVENT } else { KEY_RELEASE_EVENT }, keycode, 0, 0);
                }
                None => tracing::debug!(?key, "Taste ohne X11-Zuordnung"),
            },
        }
        let _ = self.conn.flush();
    }
}
