//! Eingabe-Erfassung über Low-Level-Hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`).
//!
//! Lokaler Modus: Alles läuft normal durch, nur Kantenberührungen werden gemeldet.
//! Remote-Modus: Alle Eingaben werden verschluckt und weitergemeldet. Der Cursor steht
//! dabei fest in der Mitte des primären Monitors; Bewegung = Hook-Position minus aktuelle
//! Cursor-Position.
//!
//! Die Entscheidung „verschlucken oder nicht“ muss synchron im Hook fallen, deshalb lebt
//! der Modus als Atomic hier und nicht im asynchronen Server-Kern.
//!
//! Im Remote-Modus liest der Hook zu jeder Zeichentaste, was sie im Tastaturlayout des
//! Vordergrundfensters ergibt (`CaptureEvent::Typed`, für Clients im Zeichenmodus).
//!
//! Windows entfernt Low-Level-Hooks ohne jede Meldung, wenn ein Aufruf länger als
//! `LowLevelHooksTimeout` braucht. Deshalb tut der Hook so wenig wie möglich: kein Lock, kein Kanal
//! von tokio (der sperrt beim Aufwecken des Empfängers und legt Speicher an), nichts Langsames.
//! Ereignisse gehen über eine sperrfreie Warteschlange an einen eigenen Thread (`forward`), der sie
//! an den Server-Kern weiterreicht; Tastenoptionen kommen per Nachricht statt über ein Lock. Im Hook
//! bleiben nur kurze Systemaufrufe ohne Warten (Monitor an der Kante, Cursorposition, Tastaturlayout)
//! und kleine Zeichenketten für den Zeichenmodus. Gehen die Hooks trotzdem verloren, merkt das der
//! Wächter (`watchdog`) und lässt sie auf ihrem Thread neu setzen (`Hooks::reinstall`).

use std::cell::RefCell;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};
use std::thread::{self, Thread};

use anyhow::{anyhow, bail, Context};
use qross_core::chars::is_typing_key;
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
    GetCurrentThread, GetCurrentThreadId, SetThreadInformation, SetThreadPriority, ThreadPowerThrottling,
    THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE,
    THREAD_PRIORITY_HIGHEST, THREAD_PRIORITY_TIME_CRITICAL,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, VK_CAPITAL, VK_ESCAPE, VK_LBUTTON, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN,
    VK_MBUTTON, VK_RBUTTON, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_XBUTTON1, VK_XBUTTON2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, GetMessageW, PostThreadMessageW, SetCursorPos,
    SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, LLKHF_UP, MSG, MSLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    XBUTTON1,
};

use super::{cursor, dnd, inject, layout, primary_center, watchdog, INJECT_MAGIC};
use crate::hook_watch::Calls;
use crate::keymap_windows::{hid_from_windows, WinModifiers};
use crate::spsc::{self, Consumer, Producer};
use crate::Capture;

struct Shared {
    remote: AtomicBool,
    /// Für Antworten von anderen Threads; der Hook selbst meldet über `emit`.
    events: UnboundedSender<CaptureEvent>,
    /// Dateien vom Client warten: das nächste Loslassen einer Maustaste meldet `LocalDrop`
    await_drop: AtomicBool,
}

/// Hook-Prozeduren sind freie Funktionen ohne Kontext – daher ein prozessweiter Zustand.
static SHARED: OnceLock<Shared> = OnceLock::new();

/// Jeder Aufruf der Hooks, auch für eigene Eingaben: Daran sieht der Wächter, dass sie leben.
static MOUSE_CALLS: AtomicU32 = AtomicU32::new(0);
static KEY_CALLS: AtomicU32 = AtomicU32::new(0);
/// Wie oft die Hooks neu gesetzt wurden
static REHOOKS: AtomicU32 = AtomicU32::new(0);
/// Der Hook-Thread läuft (solange arbeitet der Wächter)
static HOOKS_RUNNING: AtomicBool = AtomicBool::new(false);
/// Verworfen, weil die Warteschlange voll war (meldet `forward`)
static DROPPED: AtomicU32 = AtomicU32::new(0);

/// An den Hook-Thread: Hooks neu setzen
const WM_REHOOK: u32 = WM_APP + 0x51;
/// An den Hook-Thread: neue Tastenoptionen, lParam ist eine `Box<KeyOptions>`
const WM_KEY_OPTIONS: u32 = WM_APP + 0x52;
/// Plätze der Warteschlange zum Server-Kern: einige Sekunden Mausbewegung
const QUEUE_LEN: usize = 4096;

/// Was der Hook loswerden will, ohne selbst zu warten
enum FromHook {
    Event(CaptureEvent),
    /// Systemcursor wieder einblenden: lädt das Schema aus der Registry, das dauert.
    RestoreCursor,
    /// Taste ohne Zuordnung, fürs Debug-Log
    Unmapped { vk: u32, scan: u32 },
}

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
    /// Das künstliche Strg links von AltGr ist gedrückt
    altgr: bool,
    /// Feststelltaste im Remote-Modus: Dort schaltet sie das System nicht um (verschluckt), das
    /// Layout des Servers soll sie für den Zeichenmodus aber berücksichtigen.
    caps: bool,
    was_remote: bool,
    /// Als gedrückt an den Client gemeldete Tasten (nach Virtual-Key) und Maustasten: Gehen die
    /// Hooks verloren, lässt `resync` sie dort los.
    sent: [Option<KeyCode>; 256],
    sent_buttons: [bool; 5],
}

thread_local! {
    static STATE: RefCell<HookState> = const {
        RefCell::new(HookState {
            pressed: [false; 256],
            local_down: [false; 256],
            buttons_down: [false; 5],
            hotkey: HotkeyTracker::new(),
            altgr: false,
            caps: false,
            was_remote: false,
            sent: [None; 256],
            sent_buttons: [false; 5],
        })
    };
}

thread_local! {
    /// Warteschlange zu `forward` und dessen Thread zum Wecken; nur auf dem Hook-Thread gesetzt.
    static OUT: RefCell<Option<(Producer<FromHook>, Thread)>> = const { RefCell::new(None) };
    /// Kommt per `WM_KEY_OPTIONS` statt über ein Lock, denn der Hook darf nie warten.
    static KEY_OPTIONS: RefCell<Rc<KeyOptions>> = RefCell::new(Rc::new(KeyOptions::default()));
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

    /// Zusatztasten für den Zeichenmodus. Das künstliche Strg von AltGr zählt nicht als Strg.
    fn modifiers(&self) -> WinModifiers {
        WinModifiers {
            shift: self.any(VK_LSHIFT, VK_RSHIFT),
            ctrl: self.pressed[VK_RCONTROL as usize] || (self.pressed[VK_LCONTROL as usize] && !self.altgr),
            alt: self.any(VK_LMENU, VK_RMENU),
            altgr: self.altgr && self.pressed[VK_RMENU as usize],
            win: self.any(VK_LWIN, VK_RWIN),
            caps: self.caps,
        }
    }
}

pub fn start_capture(events: UnboundedSender<CaptureEvent>) -> anyhow::Result<Arc<dyn Capture>> {
    Ok(Arc::new(start(events)?))
}

fn start(events: UnboundedSender<CaptureEvent>) -> anyhow::Result<WinCapture> {
    let shared = Shared { remote: AtomicBool::new(false), events: events.clone(), await_drop: AtomicBool::new(false) };
    if SHARED.set(shared).is_err() {
        bail!("die Eingabe-Erfassung läuft in diesem Prozess bereits");
    }
    // Falls ein früherer Lauf im Remote-Modus abgestürzt ist
    cursor::restore();
    let (queue, from_hook) = spsc::queue(QUEUE_LEN);
    let forwarder = thread::Builder::new().name("qross-hook-events".into()).spawn(move || forward(from_hook, events))?;
    let forwarder = forwarder.thread().clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("qross-hooks".into())
        .spawn(move || unsafe { run_hook_thread(queue, forwarder, ready_tx) })?;
    let thread_id = ready_rx
        .recv()
        .context("Hook-Thread hat sich unerwartet beendet")?
        .map_err(|code| anyhow!("SetWindowsHookExW fehlgeschlagen (Fehlercode {code})"))?;
    let rehook = move || unsafe {
        PostThreadMessageW(thread_id, WM_REHOOK, 0, 0);
    };
    if let Err(e) = watchdog::spawn(hook_calls, &HOOKS_RUNNING, rehook) {
        tracing::warn!("Wächter für die Eingabe-Hooks läuft nicht: {e}");
    }
    if let Err(e) = dnd::start() {
        tracing::warn!("{e:#} – Dateien lassen sich nicht über die Kante ziehen");
    }
    Ok(WinCapture { thread_id })
}

unsafe fn run_hook_thread(queue: Producer<FromHook>, forwarder: Thread, ready: Sender<Result<u32, u32>>) {
    OUT.set(Some((queue, forwarder)));
    hook_loop(ready);
    HOOKS_RUNNING.store(false, Ordering::Release);
    // Warteschlange schließen: `forward` gibt den Rest weiter und endet.
    if let Some((queue, forwarder)) = OUT.take() {
        drop(queue);
        forwarder.unpark();
    }
}

unsafe fn hook_loop(ready: Sender<Result<u32, u32>>) {
    // Hooks blockieren die gesamte Eingabe des Systems, bis sie zurückkehren, und Windows entfernt
    // sie, wenn das zu lange dauert: höchste Priorität, auch wenn Qross im Hintergrund läuft.
    SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    full_speed();
    // Hier anlegen, nicht beim ersten Tastendruck im Hook
    KEY_OPTIONS.with(|_| ());
    let mut hooks = match Hooks::install() {
        Ok(hooks) => hooks,
        Err(code) => {
            let _ = ready.send(Err(code));
            return;
        }
    };
    HOOKS_RUNNING.store(true, Ordering::Release);
    let _ = ready.send(Ok(GetCurrentThreadId()));

    // Auch hier nichts Langsames: Solange eine Nachricht läuft, warten die Hooks.
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        match msg.message {
            WM_REHOOK => hooks.reinstall(),
            WM_KEY_OPTIONS => {
                let options = Box::from_raw(msg.lParam as *mut KeyOptions);
                KEY_OPTIONS.set(Rc::new(*options));
            }
            #[cfg(test)]
            tests::WM_DROP_HOOKS => hooks.remove(),
            _ => {
                DispatchMessageW(&msg);
            }
        }
    }
    hooks.remove();
}

/// Nimmt den Thread von der Drosselung im Hintergrund aus (EcoQoS: langsame Kerne, wenig Takt).
/// Ältere Windows-Versionen kennen das nicht und lehnen ab, das schadet nicht.
unsafe fn full_speed() {
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    let size = size_of::<THREAD_POWER_THROTTLING_STATE>() as u32;
    SetThreadInformation(GetCurrentThread(), ThreadPowerThrottling, (&state as *const THREAD_POWER_THROTTLING_STATE).cast(), size);
}

/// Die beiden Low-Level-Hooks des Hook-Threads
struct Hooks {
    mouse: HHOOK,
    keyboard: HHOOK,
}

impl Hooks {
    /// Setzt beide oder keinen; sonst der Fehlercode.
    unsafe fn install() -> Result<Self, u32> {
        let module = GetModuleHandleW(null());
        let hooks = Hooks {
            mouse: SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0),
            keyboard: SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0),
        };
        if hooks.mouse.is_null() || hooks.keyboard.is_null() {
            let code = GetLastError();
            hooks.remove();
            return Err(code);
        }
        Ok(hooks)
    }

    /// Für einen Hook, den Windows schon entfernt hat, schlägt das fehl – ohne Folgen.
    unsafe fn remove(&self) {
        for hook in [self.mouse, self.keyboard] {
            if !hook.is_null() {
                UnhookWindowsHookEx(hook);
            }
        }
    }

    /// Setzt beide neu, nachdem Windows (mindestens) einen stillschweigend entfernt hat. Erst beide
    /// entfernen: Der übrige liefe sonst doppelt.
    unsafe fn reinstall(&mut self) {
        self.remove();
        match Hooks::install() {
            Ok(hooks) => {
                *self = hooks;
                REHOOKS.fetch_add(1, Ordering::Relaxed);
                resync();
            }
            Err(code) => {
                *self = Hooks { mouse: null_mut(), keyboard: null_mut() };
                // Ohne Hooks wartet hier nichts auf das Log; der Wächter versucht es wieder.
                tracing::error!("Eingabe-Hooks lassen sich nicht neu setzen (Fehlercode {code})");
            }
        }
    }
}

/// Nach dem Neusetzen: Was in der Lücke geschah, hat der Hook nicht gesehen. Was der Client noch
/// gedrückt glaubt, dort loslassen, und den eigenen Stand neu aus dem System lesen. Was jetzt
/// gedrückt ist, ging in der Lücke lokal durch – sein Loslassen muss es auch.
fn resync() {
    const BUTTONS: [(MouseButton, u16); 5] = [
        (MouseButton::Left, VK_LBUTTON),
        (MouseButton::Right, VK_RBUTTON),
        (MouseButton::Middle, VK_MBUTTON),
        (MouseButton::Back, VK_XBUTTON1),
        (MouseButton::Forward, VK_XBUTTON2),
    ];
    let remote = SHARED.get().is_some_and(|shared| shared.remote.load(Ordering::Acquire));
    let down = |vk: usize| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0;
    STATE.with_borrow_mut(|st| {
        for vk in 0..256 {
            if let Some(key) = st.sent[vk].take() {
                emit(CaptureEvent::Key { key, down: false });
            }
            st.pressed[vk] = down(vk);
            st.local_down[vk] = st.pressed[vk];
        }
        for (button, vk) in BUTTONS {
            let slot = button as usize;
            if std::mem::take(&mut st.sent_buttons[slot]) {
                emit(CaptureEvent::Button { button, down: false });
            }
            let now_down = down(usize::from(vk));
            if st.buttons_down[slot] && !now_down {
                // Lokal gedrückt und in der Lücke losgelassen: wie `local_release` in `handle_mouse`
                dnd::BUTTON_RELEASES.fetch_add(1, Ordering::AcqRel);
                if remote {
                    emit(CaptureEvent::Button { button, down: false });
                }
            }
            st.buttons_down[slot] = now_down;
        }
        st.hotkey = HotkeyTracker::new();
        st.altgr = false;
    });
    if remote {
        // In der Lücke lief der versteckte Cursor frei herum: zurück in die Mitte, damit Bewegungen
        // in alle Richtungen messbar bleiben.
        let (x, y) = primary_center();
        unsafe { SetCursorPos(x, y) };
    }
}

fn hook_calls() -> Calls {
    Calls { mouse: MOUSE_CALLS.load(Ordering::Relaxed), keyboard: KEY_CALLS.load(Ordering::Relaxed) }
}

/// Meldet ein Ereignis an den Server-Kern, ohne zu warten. Nur auf dem Hook-Thread.
fn emit(event: CaptureEvent) {
    pass(FromHook::Event(event));
}

fn pass(item: FromHook) {
    OUT.with_borrow_mut(|out| {
        if let Some((queue, forwarder)) = out {
            if queue.push(item).is_err() {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
            forwarder.unpark();
        }
    });
}

/// Eigener Thread: gibt weiter, was der Hook meldet. Hier darf es warten und dauern.
fn forward(mut queue: Consumer<FromHook>, events: UnboundedSender<CaptureEvent>) {
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };
    loop {
        let closed = queue.is_closed();
        while let Some(item) = queue.pop() {
            match item {
                FromHook::Event(event) => {
                    let _ = events.send(event);
                }
                FromHook::RestoreCursor => cursor::restore(),
                FromHook::Unmapped { vk, scan } => tracing::debug!(vk, scan, "Taste ohne Zuordnung"),
            }
        }
        let dropped = DROPPED.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            tracing::warn!("{dropped} Eingaben verworfen: Der Server-Kern kam nicht hinterher");
        }
        if closed {
            return;
        }
        thread::park();
    }
}

struct WinCapture {
    thread_id: u32,
}

impl Capture for WinCapture {
    fn set_key_options(&self, options: KeyOptions) {
        let options = Box::into_raw(Box::new(options));
        if unsafe { PostThreadMessageW(self.thread_id, WM_KEY_OPTIONS, 0, options as LPARAM) } == 0 {
            drop(unsafe { Box::from_raw(options) });
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
    MOUSE_CALLS.fetch_add(1, Ordering::Relaxed);
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
    KEY_CALLS.fetch_add(1, Ordering::Relaxed);
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
            emit(CaptureEvent::LocalDrop { x: info.pt.x, y: info.pt.y });
        }
        let slot = button as usize;
        let local_release = STATE.with_borrow_mut(|st| {
            if !remote {
                st.buttons_down[slot] = down;
                st.sent_buttons[slot] = false;
            } else if !down && st.buttons_down[slot] {
                st.buttons_down[slot] = false;
                return true;
            } else {
                st.sent_buttons[slot] = down;
            }
            false
        });
        if !down && (!remote || local_release) {
            dnd::BUTTON_RELEASES.fetch_add(1, Ordering::AcqRel);
        }
        if local_release {
            // Lokal gedrückt (z. B. beim Ziehen über die Kante): das Loslassen braucht das
            // lokale System, und der Server-Kern macht daraus ggf. den Drop.
            emit(CaptureEvent::Button { button, down: false });
            return false;
        }
    }

    if !remote {
        if msg == WM_MOUSEMOVE {
            let (x, y) = (info.pt.x, info.pt.y);
            if let Some(edge) = outer_edge_at(x, y) {
                emit(CaptureEvent::EdgeHit { edge, x, y, buttons_down: buttons_down() });
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
    emit(event);
    true
}

fn handle_key(shared: &Shared, info: &KBDLLHOOKSTRUCT) -> bool {
    let up = info.flags & LLKHF_UP != 0;
    let vk = (info.vkCode & 0xFF) as usize;
    // Von AltGr erzeugtes künstliches LCtrl zählt nicht als Strg (sonst wäre AltGr+L = Strg+Alt+L).
    let fake_ctrl = info.scanCode & 0x200 != 0;
    let extended = info.flags & LLKHF_EXTENDED != 0;
    let key = if fake_ctrl { None } else { hid_from_windows(info.scanCode, extended, info.vkCode) };
    let options = KEY_OPTIONS.with_borrow(Rc::clone);
    STATE.with_borrow_mut(|st| {
        let was_down = std::mem::replace(&mut st.pressed[vk], !up);
        if fake_ctrl {
            st.altgr = !up;
        }
        let remote = shared.remote.load(Ordering::Acquire);
        if remote && !st.was_remote {
            // Stand der Feststelltaste beim Wechsel; danach zählt der Hook selbst mit.
            st.caps = unsafe { GetKeyState(i32::from(VK_CAPITAL)) } & 1 != 0;
        }
        st.was_remote = remote;
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
                    emit(event);
                }
                return swallow;
            }
        }
        if !remote {
            st.local_down[vk] = !up;
            st.sent[vk] = None;
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
            pass(FromHook::RestoreCursor);
            emit(CaptureEvent::PanicReturn);
            return true;
        }
        if fake_ctrl {
            // Künstliches LCtrl von AltGr – der Client erzeugt es selbst.
            return true;
        }
        if vk == VK_CAPITAL as usize && !up && !was_down {
            st.caps = !st.caps;
        }
        if options.media == MediaKeys::Local && key.is_some_and(KeyCode::is_media) {
            // Medientasten wirken auf Wunsch immer hier (z. B. Lautsprecher am Server)
            return false;
        }
        match key {
            Some(key) if !up && is_typing_key(key) => {
                // Zeichenmodus: was die Taste im Layout des Vordergrundfensters ergibt
                let chars = layout::key_chars(info.vkCode, info.scanCode & 0xFF, st.modifiers(), layout::foreground_layout());
                st.sent[vk] = Some(key);
                emit(CaptureEvent::Typed { key, chars });
            }
            Some(key) => {
                st.sent[vk] = (!up).then_some(key);
                emit(CaptureEvent::Key { key, down: !up });
            }
            None => pass(FromHook::Unmapped { vk: info.vkCode, scan: info.scanCode }),
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
    };

    use super::*;

    /// Entfernt die Hooks so, wie Windows es nach einer Zeitüberschreitung tut: Der Hook-Thread merkt
    /// nichts davon und behält die alten Handles.
    pub(super) const WM_DROP_HOOKS: u32 = WM_APP + 0x5F;

    /// Eigene Eingabe für beide Hooks (die Probe des Wächters)
    fn nudge() {
        assert!(
            watchdog::probe(),
            "Windows lehnt SendInput ab – der Test braucht eine aktive Sitzung (nicht gesperrt, RDP-Fenster nicht \
             minimiert)"
        );
    }

    /// Erzeugt eigene Eingaben, bis `done` gilt
    fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) -> Duration {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < timeout,
                "{what}: nach {timeout:?} nicht eingetreten (vorne ein Fenster mit Administratorrechten? Dann wartet \
                 der Wächter absichtlich)"
            );
            nudge();
            thread::sleep(Duration::from_millis(50));
        }
        start.elapsed()
    }

    /// Beide Hooks wurden seit `before` aufgerufen.
    fn both_called_since(before: Calls) -> impl Fn() -> bool {
        move || {
            let now = hook_calls();
            now.mouse != before.mouse && now.keyboard != before.keyboard
        }
    }

    /// Ein Punkt an einer äußeren Kante des primären Monitors
    fn outer_edge_point() -> Option<(i32, i32)> {
        let (cx, cy) = primary_center();
        let m = monitor_rect(cx, cy)?;
        [(m.left, cy), (m.right - 1, cy), (cx, m.top), (cx, m.bottom - 1)]
            .into_iter()
            .find(|&(x, y)| outer_edge_at(x, y).is_some())
    }

    /// Steht der Cursor kurz an der Kante, meldet der Hook `EdgeHit`: Der ganze Weg bis zum Server-Kern
    /// geht, über Warteschlange und `forward`.
    fn edge_hit_arrives(events: &mut tokio::sync::mpsc::UnboundedReceiver<CaptureEvent>) {
        let Some((x, y)) = outer_edge_point() else {
            return eprintln!("keine äußere Kante am primären Monitor – Weg zum Server-Kern ungeprüft");
        };
        // Wie eine echte Maus (nicht markiert): ein Pixel hin und zurück an die Kante
        let shake = |dx| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 },
            },
        };
        let mut was = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut was) };
        while events.try_recv().is_ok() {}
        unsafe { SetCursorPos(x, y) };
        let start = Instant::now();
        let mut hit = false;
        while !hit && start.elapsed() < Duration::from_secs(2) {
            let inputs = [shake(1), shake(-1)];
            unsafe { SendInput(2, inputs.as_ptr(), size_of::<INPUT>() as i32) };
            thread::sleep(Duration::from_millis(30));
            while let Ok(event) = events.try_recv() {
                hit |= matches!(event, CaptureEvent::EdgeHit { .. });
            }
        }
        unsafe { SetCursorPos(was.x, was.y) };
        assert!(hit, "kein EdgeHit bei ({x}, {y})");
    }

    /// Echte Hooks, echter Wächter: Windows entfernt die Hooks (hier nachgestellt), der Wächter merkt es
    /// und lässt sie neu setzen. Erzeugt echte Eingaben (die Probe des Wächters; der Cursor steht kurz an
    /// einer Bildschirmkante), deshalb nur mit `QROSS_INPUT_TEST=1`; braucht eine aktive Sitzung.
    #[test]
    fn lost_hooks_are_set_again_by_the_watchdog() {
        if std::env::var_os("QROSS_INPUT_TEST").is_none() {
            return eprintln!("erzeugt echte Eingaben – nur mit QROSS_INPUT_TEST=1, übersprungen");
        }
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let capture = start(tx).unwrap();
        wait_for("Hooks sehen Eingaben", Duration::from_secs(2), both_called_since(hook_calls()));
        edge_hit_arrives(&mut events);

        // Lebende Hooks und eigene Eingaben: kein Fehlalarm
        let rehooks = REHOOKS.load(Ordering::Relaxed);
        let calm = Instant::now();
        while calm.elapsed() < Duration::from_secs(3) {
            nudge();
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(REHOOKS.load(Ordering::Relaxed), rehooks, "Fehlalarm bei lebenden Hooks");

        unsafe { PostThreadMessageW(capture.thread_id, WM_DROP_HOOKS, 0, 0) };
        thread::sleep(Duration::from_millis(200));
        let dead = hook_calls();
        for _ in 0..5 {
            nudge();
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(hook_calls(), dead, "die Hooks sind noch da");

        let took = wait_for("Wächter setzt die Hooks neu", Duration::from_secs(10), || {
            REHOOKS.load(Ordering::Relaxed) > rehooks
        });
        eprintln!("neu gesetzt nach {took:?}");
        wait_for("Hooks sehen wieder Eingaben", Duration::from_secs(2), both_called_since(hook_calls()));
        edge_hit_arrives(&mut events);
        assert_eq!(REHOOKS.load(Ordering::Relaxed), rehooks + 1, "genau einmal neu gesetzt");
        drop(capture);
    }

    #[test]
    fn resync_releases_what_the_client_still_holds() {
        thread::spawn(|| {
            let (queue, mut from_hook) = spsc::queue(16);
            OUT.set(Some((queue, thread::current())));
            STATE.with_borrow_mut(|st| {
                st.sent[0x41] = Some(KeyCode::A);
                st.sent_buttons[MouseButton::Right as usize] = true;
                st.altgr = true;
            });
            resync();
            let mut released = Vec::new();
            while let Some(FromHook::Event(event)) = from_hook.pop() {
                released.push(event);
            }
            assert!(released.contains(&CaptureEvent::Key { key: KeyCode::A, down: false }), "{released:?}");
            assert!(released.contains(&CaptureEvent::Button { button: MouseButton::Right, down: false }), "{released:?}");
            STATE.with_borrow(|st| {
                assert!(st.sent.iter().all(Option::is_none) && !st.sent_buttons.contains(&true) && !st.altgr);
            });
            resync();
            assert!(from_hook.pop().is_none(), "nichts doppelt loslassen");
        })
        .join()
        .unwrap();
    }
}
