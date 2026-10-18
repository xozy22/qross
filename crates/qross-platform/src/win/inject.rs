//! Eingaben über `SendInput` einspeisen.

use std::mem::size_of;

use qross_core::Inject;
use qross_proto::{KeyCode, MouseButton};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MOUSE_EVENT_FLAGS,
};

use super::{screen_bounds, INJECT_MAGIC};
use crate::keymap_windows::windows_from_hid;
use crate::Injector;

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

pub fn injector() -> anyhow::Result<Box<dyn Injector>> {
    Ok(Box::new(WinInjector))
}

struct WinInjector;

impl Injector for WinInjector {
    fn inject(&mut self, event: &Inject) {
        match *event {
            Inject::MoveTo { x, y } => {
                let b = screen_bounds();
                let flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
                send(&[mouse(normalize(x - b.x, b.width), normalize(y - b.y, b.height), 0, flags)]);
            }
            Inject::Button { button, down } => {
                let (flags, data) = match (button, down) {
                    (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                    (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
                    (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                    (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
                    (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                    (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                    (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
                    (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
                    (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
                    (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
                };
                send(&[mouse(0, 0, data, flags)]);
            }
            Inject::Wheel { dx, dy } => {
                let mut inputs = Vec::with_capacity(2);
                if dy != 0 {
                    inputs.push(mouse(0, 0, dy as i32 as u32, MOUSEEVENTF_WHEEL));
                }
                if dx != 0 {
                    inputs.push(mouse(0, 0, dx as i32 as u32, MOUSEEVENTF_HWHEEL));
                }
                send(&inputs);
            }
            Inject::Key { key, down } => key_event(key, down),
        }
    }

    fn drop_folder(&mut self, x: i32, y: i32) -> Option<std::path::PathBuf> {
        super::shell::folder_at(x, y)
    }
}

/// Esc drücken und loslassen – bricht einen laufenden OLE-Drag ab. Markiert, damit der eigene
/// Hook es durchlässt.
pub(crate) fn tap_escape() {
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: 0x01,
                dwFlags: KEYEVENTF_SCANCODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: INJECT_MAGIC,
            },
        },
    };
    send(&[key(false), key(true)]);
}

fn key_event(key: KeyCode, down: bool) {
    let Some(win) = windows_from_hid(key) else {
        tracing::debug!(?key, "Taste ohne Windows-Zuordnung");
        return;
    };
    let mut flags = if down { 0 } else { KEYEVENTF_KEYUP };
    if win.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let ki = match win.vk {
        Some(vk) => KEYBDINPUT { wVk: vk, wScan: win.scan, dwFlags: flags, time: 0, dwExtraInfo: INJECT_MAGIC },
        None => KEYBDINPUT {
            wVk: 0,
            wScan: win.scan,
            dwFlags: flags | KEYEVENTF_SCANCODE,
            time: 0,
            dwExtraInfo: INJECT_MAGIC,
        },
    };
    send(&[INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki } }]);
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: INJECT_MAGIC },
        },
    }
}

fn send(inputs: &[INPUT]) {
    if inputs.is_empty() {
        return;
    }
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        // Passiert z. B. wenn ein Fenster mit höherer Integritätsstufe (UAC) im Vordergrund ist.
        tracing::warn!("SendInput hat {sent} von {} Ereignissen angenommen", inputs.len());
    }
}

/// Pixel-Offset → absolute Koordinate 0..=65535. Windows rechnet zurück per
/// `pixel = floor(n * len / 65536)`, daher wird hier aufgerundet.
fn normalize(offset: i32, len: u32) -> i32 {
    if len <= 1 {
        return 0;
    }
    let offset = offset.clamp(0, len as i32 - 1) as i64;
    ((offset * 65536 + len as i64 - 1) / len as i64).min(65535) as i32
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn normalize_hits_every_pixel() {
        for len in [1080u32, 1920, 2560, 3840 + 1920] {
            for px in 0..len as i32 {
                let n = normalize(px, len) as i64;
                assert_eq!((n * len as i64 / 65536) as i32, px, "len {len}");
            }
        }
    }
}
