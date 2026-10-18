//! Eingaben über `SendInput` einspeisen.

use std::mem::size_of;
use std::path::PathBuf;
use std::time::Duration;

use qross_core::Inject;
use qross_proto::{KeyCode, MouseButton};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyExW, SendInput, VkKeyScanExW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC_EX, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MOUSE_EVENT_FLAGS,
};

use super::layout::foreground_layout;
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
            // Ohne MOUSEEVENTF_ABSOLUTE wie eine echte Maus: Raw Input meldet die Bewegung, und ein
            // per ClipCursor eingesperrter Zeiger bleibt, wo das Spiel ihn haben will.
            Inject::MoveBy { dx, dy } => send(&[mouse(dx, dy, 0, MOUSEEVENTF_MOVE)]),
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
            Inject::Char { ch, down } => type_char(ch, down),
            Inject::Shortcut { ch, key, down } => shortcut(ch, key, down),
        }
    }

    fn types_text(&self) -> bool {
        true
    }

    fn drop_folder(&mut self, x: i32, y: i32) -> Option<std::path::PathBuf> {
        super::shell::folder_at(x, y)
    }

    fn take_dragged_files(&mut self, x: i32, y: i32) -> Vec<PathBuf> {
        if let Err(e) = super::dnd::start() {
            tracing::debug!("{e:#}");
            return Vec::new();
        }
        let (tx, rx) = std::sync::mpsc::channel();
        if !super::dnd::probe(x, y, false, move |files| {
            let _ = tx.send(files);
        }) {
            return Vec::new();
        }
        // Der laufende OLE-Drag bemerkt das Drop-Ziel erst bei einer Mausbewegung – daher
        // auf der Stelle „wackeln“, bis die Prüfung antwortet (sie gibt nach 400 ms auf).
        let files = (0..10)
            .find_map(|_| {
                self.inject(&Inject::MoveTo { x, y });
                rx.recv_timeout(Duration::from_millis(60)).ok()
            })
            .unwrap_or_default();
        if !files.is_empty() {
            tap_escape();
        }
        files
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

/// Unbelegte Taste (VK 0xE8) drücken und loslassen, markiert für den eigenen Hook. Lässt man
/// danach Alt oder die Windows-Taste los, öffnet Windows weder Menüleiste noch Startmenü.
pub(crate) fn tap_menu_mask() {
    let key = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0xE8,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
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

/// Ein Zeichen unabhängig vom Tastaturlayout eingeben (`VK_PACKET`); außerhalb der Basisebene,
/// z. B. Emoji, als Ersatzzeichenpaar.
fn type_char(ch: char, down: bool) {
    let flags = KEYEVENTF_UNICODE | if down { 0 } else { KEYEVENTF_KEYUP };
    let mut units = [0u16; 2];
    let inputs: Vec<INPUT> = ch
        .encode_utf16(&mut units)
        .iter()
        .map(|&unit| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: 0, wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: INJECT_MAGIC },
            },
        })
        .collect();
    send(&inputs);
}

/// Kürzel im Zeichenmodus: die Taste, die im Layout des Vordergrundfensters `ch` ergibt. Programme
/// prüfen den Virtual-Key (Strg+Z: `VK_Z`), Umschalt für das Zeichen ist dabei egal. Gibt es das
/// Zeichen dort nicht, die Taste an der Position `key`.
fn shortcut(ch: char, key: KeyCode, down: bool) {
    let layout = foreground_layout();
    let found = u16::try_from(u32::from(ch)).ok().map(|unit| unsafe { VkKeyScanExW(unit, layout) }).filter(|&r| r != -1);
    let Some(found) = found else { return key_event(key, down) };
    let vk = (found as u16) & 0xFF;
    let scan = unsafe { MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_VSC_EX, layout) };
    let mut flags = if down { 0 } else { KEYEVENTF_KEYUP };
    if scan & 0xFF00 == 0xE000 {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let ki = KEYBDINPUT { wVk: vk, wScan: (scan & 0xFF) as u16, dwFlags: flags, time: 0, dwExtraInfo: INJECT_MAGIC };
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
    use super::*;
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_PACKET;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT,
        LLKHF_UP, MSG, WH_KEYBOARD_LL, WM_QUIT,
    };

    /// Von Qross eingespeiste Tastenereignisse, die der Test-Hook gesehen hat: (Virtual-Key, Scancode, losgelassen)
    static SEEN: Mutex<Vec<(u32, u32, bool)>> = Mutex::new(Vec::new());

    unsafe extern "system" fn swallow(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let info = &*(lparam as *const KBDLLHOOKSTRUCT);
        if code >= 0 && info.dwExtraInfo == INJECT_MAGIC {
            SEEN.lock().unwrap().push((info.vkCode, info.scanCode, info.flags & LLKHF_UP != 0));
            return 1;
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// Echtes Einspeisen über `SendInput`, ohne dass ein Programm etwas abbekommt: Der zuletzt
    /// eingehängte Low-Level-Hook kommt zuerst dran, liest mit und verschluckt die Ereignisse.
    /// Von Hand: `cargo test -p qross-platform -- --ignored injected_characters`
    #[test]
    #[ignore = "speist echte Tastenereignisse ins System ein (der Test verschluckt sie gleich wieder)"]
    fn injected_characters_reach_the_input_system() {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let hook_thread = std::thread::spawn(move || unsafe {
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(swallow), GetModuleHandleW(std::ptr::null()), 0);
            ready_tx.send((!hook.is_null()).then(|| GetCurrentThreadId())).unwrap();
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
            UnhookWindowsHookEx(hook);
        });
        let thread_id = ready_rx.recv().unwrap().expect("Tastatur-Hook");

        let mut injector = WinInjector;
        let y = KeyCode(0x1C);
        for event in [
            Inject::Char { ch: 'ä', down: true },
            Inject::Char { ch: 'ä', down: false },
            Inject::Char { ch: '😀', down: true },
            Inject::Char { ch: '😀', down: false },
            Inject::Shortcut { ch: 'z', key: y, down: true },
            Inject::Shortcut { ch: 'z', key: y, down: false },
        ] {
            injector.inject(&event);
        }
        std::thread::sleep(Duration::from_millis(300));
        unsafe { PostThreadMessageW(thread_id, WM_QUIT, 0, 0) };
        hook_thread.join().unwrap();

        let seen = SEEN.lock().unwrap().clone();
        assert!(
            !seen.is_empty(),
            "nichts angekommen: Bildschirm gesperrt, Sandbox ohne Zugriff auf die Eingabe oder ein Fenster mit \
             Administratorrechten im Vordergrund"
        );
        assert_eq!(seen.len(), 8, "{seen:x?}");
        let packet = u32::from(VK_PACKET);
        assert_eq!(
            seen[..8],
            [
                (packet, 0xE4, false),
                (packet, 0xE4, true),
                (packet, 0xD83D, false),
                (packet, 0xDE00, false),
                (packet, 0xD83D, true),
                (packet, 0xDE00, true),
                // Kürzel: Virtual-Key des Zeichens im aktuellen Layout, VK_Z ist in allen lateinischen gleich
                (u32::from(b'Z'), seen[6].1, false),
                (u32::from(b'Z'), seen[6].1, true),
            ],
            "{seen:x?}"
        );
    }

    #[test]
    fn injector_types_text() {
        assert!(WinInjector.types_text());
    }

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
