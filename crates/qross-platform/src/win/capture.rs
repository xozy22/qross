//! Eingabe-Erfassung über Low-Level-Hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`).
//!
//! Lokaler Modus: Alles läuft normal durch, nur Kantenberührungen werden gemeldet.
//! Remote-Modus: Alle Eingaben werden verschluckt und weitergemeldet. Der Cursor steht
//! dabei fest in der Mitte des primären Monitors; Bewegung = Hook-Position minus aktuelle
//! Cursor-Position.
//!
//! Die Entscheidung „verschlucken oder nicht“ muss synchron im Hook fallen, deshalb lebt
//! der Modus als Atomic hier und nicht im asynchronen Server-Kern.

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use anyhow::{anyhow, bail, Context};
use qross_core::keys::{HotkeyEvent, HotkeyTracker, KeyOptions, MediaKeys};
use qross_core::CaptureEvent;
use qross_proto::{Edge, KeyCode, MouseButton};
use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::Foundation::{GetLastError, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, GetCurrentThreadId, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MBUTTON, VK_RBUTTON,
    VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_XBUTTON1, VK_XBUTTON2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, GetMessageW, PostThreadMessageW, SetCursorPos,
    SetWindowsHookExW, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_UP, MSG, MSLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1,
};

use super::{cursor, dnd, inject, primary_center, INJECT_MAGIC};
use crate::keymap_windows::hid_from_windows;
use crate::Capture;

struct Shared {
    remote: AtomicBool,
    events: UnboundedSender<CaptureEvent>,
    keys: Mutex<KeyOptions>,
    /// Dateien vom Client warten: das nächste Loslassen einer Maustaste meldet `LocalDrop`
    await_drop: AtomicBool,
}

/// Hook-Prozeduren sind freie Funktionen ohne Kontext – daher ein prozessweiter Zustand.
static SHARED: OnceLock<Shared> = OnceLock::new();

/// Nur vom Hook-Thread benutzt.
struct HookState {
    /// Physisch gedrückte Tasten (nach Virtual-Key)
    pressed: [bool; 256],
    /// Tasten, die schon vor dem Wechsel in den Remote-Modus lokal gedrückt waren.
    /// Ihr Loslassen muss lokal durchgehen, sonst bleiben sie auf dem Server hängen.
    local_down: [bool; 256],
    /// Dasselbe für Maustasten (wichtig beim Ziehen von Dateien über die Kante)
    buttons_down: [bool; 5],
    /// Erkennt Fang-Taste und Wechsel-Kürzel
    hotkey: HotkeyTracker,
}

thread_local! {
    static STATE: RefCell<HookState> = const {
        RefCell::new(HookState {
            pressed: [false; 256],
            local_down: [false; 256],
            buttons_down: [false; 5],
            hotkey: HotkeyTracker::new(),
        })
    };
}

impl HookState {
    fn any(&self, a: u16, b: u16) -> bool {
        self.pressed[a as usize] || self.pressed[b as usize]
    }

    fn panic_modifiers(&self) -> bool {
        self.any(VK_LCONTROL, VK_RCONTROL) && self.any(VK_LMENU, VK_RMENU) && self.any(VK_LSHIFT, VK_RSHIFT)
    }

    /// Alt oder Windows-Taste gedrückt: Ihr Loslassen öffnet die Menüleiste bzw. das Startmenü,
    /// wenn dazwischen keine andere Taste ankam.
    fn menu_modifiers(&self) -> bool {
        self.any(VK_LMENU, VK_RMENU) || self.any(VK_LWIN, VK_RWIN)
    }
}

pub fn start_capture(events: UnboundedSender<CaptureEvent>) -> anyhow::Result<Arc<dyn Capture>> {
    let shared =
        Shared { remote: AtomicBool::new(false), events, keys: Mutex::new(KeyOptions::default()), await_drop: AtomicBool::new(false) };
    if SHARED.set(shared).is_err() {
        bail!("die Eingabe-Erfassung läuft in diesem Prozess bereits");
    }
    // Falls ein früherer Lauf im Remote-Modus abgestürzt ist
    cursor::restore();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("qross-hooks".into())
        .spawn(move || unsafe { run_hook_thread(ready_tx) })?;
    let thread_id = ready_rx
        .recv()
        .context("Hook-Thread hat sich unerwartet beendet")?
        .map_err(|code| anyhow!("SetWindowsHookExW fehlgeschlagen (Fehlercode {code})"))?;
    if let Err(e) = dnd::start() {
        tracing::warn!("{e:#} – Dateien lassen sich nicht über die Kante ziehen");
    }
    Ok(Arc::new(WinCapture { thread_id }))
}

unsafe fn run_hook_thread(ready: Sender<Result<u32, u32>>) {
    // Hooks blockieren die gesamte Eingabe des Systems, bis sie zurückkehren.
    SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    let module = GetModuleHandleW(null());
    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
    let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
    if mouse.is_null() || keyboard.is_null() {
        let code = GetLastError();
        for hook in [mouse, keyboard] {
            if !hook.is_null() {
                UnhookWindowsHookEx(hook);
            }
        }
        let _ = ready.send(Err(code));
        return;
    }
    let _ = ready.send(Ok(GetCurrentThreadId()));

    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        DispatchMessageW(&msg);
    }
    UnhookWindowsHookEx(mouse);
    UnhookWindowsHookEx(keyboard);
}

struct WinCapture {
    thread_id: u32,
}

impl Capture for WinCapture {
    fn set_key_options(&self, options: KeyOptions) {
        if let Some(shared) = SHARED.get() {
            *shared.keys.lock().unwrap_or_else(|e| e.into_inner()) = options;
        }
    }

    fn capture_remote(&self) {
        if let Some(shared) = SHARED.get() {
            shared.remote.store(true, Ordering::Release);
            // Weg vom Rand, damit Bewegungen in alle Richtungen messbar bleiben.
            let (x, y) = primary_center();
            unsafe { SetCursorPos(x, y) };
            cursor::hide();
        }
    }

    fn release(&self, x: i32, y: i32) {
        if let Some(shared) = SHARED.get() {
            shared.remote.store(false, Ordering::Release);
            unsafe { SetCursorPos(x, y) };
            cursor::restore();
        }
    }

    fn probe_drag(&self, x: i32, y: i32) {
        let Some(shared) = SHARED.get() else { return };
        let events = shared.events.clone();
        let sent = dnd::probe(x, y, true, move |files| {
            let _ = events.send(CaptureEvent::DragProbeResult { files });
        });
        if !sent {
            let _ = shared.events.send(CaptureEvent::DragProbeResult { files: Vec::new() });
        }
    }

    fn cancel_local_drag(&self) {
        inject::tap_escape();
    }

    fn await_local_drop(&self) {
        if let Some(shared) = SHARED.get() {
            shared.await_drop.store(true, Ordering::Release);
        }
    }

    fn drop_folder(&self, x: i32, y: i32) -> Option<std::path::PathBuf> {
        super::shell::folder_at(x, y)
    }
}

impl Drop for WinCapture {
    fn drop(&mut self) {
        if let Some(shared) = SHARED.get() {
            shared.remote.store(false, Ordering::Release);
        }
        cursor::restore();
        unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) };
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(shared) = SHARED.get() {
            let info = &*(lparam as *const MSLLHOOKSTRUCT);
            if info.dwExtraInfo != INJECT_MAGIC && handle_mouse(shared, wparam as u32, info) {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        if let Some(shared) = SHARED.get() {
            let info = &*(lparam as *const KBDLLHOOKSTRUCT);
            if info.dwExtraInfo != INJECT_MAGIC && handle_key(shared, info) {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// Gibt zurück, ob das Ereignis verschluckt werden soll.
fn handle_mouse(shared: &Shared, msg: u32, info: &MSLLHOOKSTRUCT) -> bool {
    let remote = shared.remote.load(Ordering::Acquire);
    if let Some((button, down)) = button_of(msg, info) {
        if !remote && !down && shared.await_drop.swap(false, Ordering::AcqRel) {
            // Dateien vom Client: hier losgelassen = hier ablegen
            let _ = shared.events.send(CaptureEvent::LocalDrop { x: info.pt.x, y: info.pt.y });
        }
        let slot = button as usize;
        let local_release = STATE.with_borrow_mut(|st| {
            if !remote {
                st.buttons_down[slot] = down;
            } else if !down && st.buttons_down[slot] {
                st.buttons_down[slot] = false;
                return true;
            }
            false
        });
        if !down && (!remote || local_release) {
            dnd::BUTTON_RELEASES.fetch_add(1, Ordering::AcqRel);
        }
        if local_release {
            // Lokal gedrückt (z. B. beim Ziehen über die Kante): das Loslassen braucht das
            // lokale System, und der Server-Kern macht daraus ggf. den Drop.
            let _ = shared.events.send(CaptureEvent::Button { button, down: false });
            return false;
        }
    }

    if !remote {
        if msg == WM_MOUSEMOVE {
            let (x, y) = (info.pt.x, info.pt.y);
            if let Some(edge) = outer_edge_at(x, y) {
                let _ = shared.events.send(CaptureEvent::EdgeHit { edge, x, y, buttons_down: buttons_down() });
            }
        }
        return false;
    }

    let button = |button, down| CaptureEvent::Button { button, down };
    let wheel = (info.mouseData >> 16) as u16 as i16;
    let event = match msg {
        WM_MOUSEMOVE => {
            let mut cur = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut cur) };
            let (dx, dy) = (info.pt.x - cur.x, info.pt.y - cur.y);
            if dx == 0 && dy == 0 {
                return true;
            }
            CaptureEvent::MouseDelta { dx, dy }
        }
        WM_LBUTTONDOWN => button(MouseButton::Left, true),
        WM_LBUTTONUP => button(MouseButton::Left, false),
        WM_RBUTTONDOWN => button(MouseButton::Right, true),
        WM_RBUTTONUP => button(MouseButton::Right, false),
        WM_MBUTTONDOWN => button(MouseButton::Middle, true),
        WM_MBUTTONUP => button(MouseButton::Middle, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let which = if (info.mouseData >> 16) as u16 == XBUTTON1 { MouseButton::Back } else { MouseButton::Forward };
            button(which, msg == WM_XBUTTONDOWN)
        }
        WM_MOUSEWHEEL => CaptureEvent::Wheel { dx: 0, dy: wheel },
        WM_MOUSEHWHEEL => CaptureEvent::Wheel { dx: wheel, dy: 0 },
        _ => return true,
    };
    let _ = shared.events.send(event);
    true
}

fn handle_key(shared: &Shared, info: &KBDLLHOOKSTRUCT) -> bool {
    let up = info.flags & LLKHF_UP != 0;
    let vk = (info.vkCode & 0xFF) as usize;
    // Von AltGr erzeugtes künstliches LCtrl zählt nicht als Strg (sonst wäre AltGr+L = Strg+Alt+L).
    let fake_ctrl = info.scanCode & 0x200 != 0;
    let extended = info.flags & LLKHF_EXTENDED != 0;
    let key = if fake_ctrl { None } else { hid_from_windows(info.scanCode, extended, info.vkCode) };
    let options = shared.keys.lock().unwrap_or_else(|e| e.into_inner());
    STATE.with_borrow_mut(|st| {
        st.pressed[vk] = !up;
        let remote = shared.remote.load(Ordering::Acquire);
        if let Some(key) = key {
            // Fang-Taste (Standard: Rollen) und Wechsel-Kürzel
            let hotkey = st.hotkey.on_key(key, !up, &options);
            if hotkey != HotkeyEvent::None {
                let swallow = hotkey.swallow(remote);
                if let HotkeyEvent::Triggered { event, .. } = hotkey {
                    if swallow && st.menu_modifiers() {
                        // Das verschluckte Kürzel war sonst die einzige Taste zwischen Drücken und
                        // Loslassen von Alt bzw. der Windows-Taste.
                        inject::tap_menu_mask();
                    }
                    let _ = shared.events.send(event);
                }
                return swallow;
            }
        }
        if !remote {
            st.local_down[vk] = !up;
            return false;
        }
        if st.local_down[vk] {
            // Lokal gedrückte Taste: Loslassen lokal durchlassen, Auto-Repeat verwerfen.
            if up {
                st.local_down[vk] = false;
                return false;
            }
            return true;
        }
        if !up && vk == VK_ESCAPE as usize && st.panic_modifiers() {
            // Notausgang Strg+Alt+Shift+Esc – funktioniert auch, wenn der Server-Kern hängt.
            shared.remote.store(false, Ordering::Release);
            let (x, y) = primary_center();
            unsafe { SetCursorPos(x, y) };
            cursor::restore();
            let _ = shared.events.send(CaptureEvent::PanicReturn);
            return true;
        }
        if fake_ctrl {
            // Künstliches LCtrl von AltGr – der Client erzeugt es selbst.
            return true;
        }
        if options.media == MediaKeys::Local && key.is_some_and(KeyCode::is_media) {
            // Medientasten wirken auf Wunsch immer hier (z. B. Lautsprecher am Server)
            return false;
        }
        match key {
            Some(key) => {
                let _ = shared.events.send(CaptureEvent::Key { key, down: !up });
            }
            None => tracing::debug!(vk = info.vkCode, scan = info.scanCode, "Taste ohne Zuordnung"),
        }
        true
    })
}

fn button_of(msg: u32, info: &MSLLHOOKSTRUCT) -> Option<(MouseButton, bool)> {
    Some(match msg {
        WM_LBUTTONDOWN => (MouseButton::Left, true),
        WM_LBUTTONUP => (MouseButton::Left, false),
        WM_RBUTTONDOWN => (MouseButton::Right, true),
        WM_RBUTTONUP => (MouseButton::Right, false),
        WM_MBUTTONDOWN => (MouseButton::Middle, true),
        WM_MBUTTONUP => (MouseButton::Middle, false),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let which = if (info.mouseData >> 16) as u16 == XBUTTON1 { MouseButton::Back } else { MouseButton::Forward };
            (which, msg == WM_XBUTTONDOWN)
        }
        _ => return None,
    })
}

/// Liegt (x, y) an einer Monitorkante, hinter der kein weiterer Monitor folgt?
/// Pro Monitor statt über die Bounding-Box: Bei unterschiedlich großen Monitoren ist so
/// z. B. auch die Unterkante des kleineren Monitors eine echte Außenkante.
fn outer_edge_at(x: i32, y: i32) -> Option<Edge> {
    let monitor = monitor_rect(x, y)?;
    let candidates = [
        (x <= monitor.left, Edge::Left, (x - 1, y)),
        (x >= monitor.right - 1, Edge::Right, (x + 1, y)),
        (y <= monitor.top, Edge::Top, (x, y - 1)),
        (y >= monitor.bottom - 1, Edge::Bottom, (x, y + 1)),
    ];
    candidates
        .into_iter()
        .find(|&(at_edge, _, (bx, by))| at_edge && !has_monitor(bx, by))
        .map(|(_, edge, _)| edge)
}

fn monitor_rect(x: i32, y: i32) -> Option<RECT> {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        (GetMonitorInfoW(monitor, &mut info) != 0).then_some(info.rcMonitor)
    }
}

fn has_monitor(x: i32, y: i32) -> bool {
    unsafe { !MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONULL).is_null() }
}

fn buttons_down() -> bool {
    [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2]
        .iter()
        .any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0)
}
