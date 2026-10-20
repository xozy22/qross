//! Feststelltaste unter macOS: Stand lesen und setzen über IOHIDSystem
//! (`IOHIDGetModifierLockState`/`IOHIDSetModifierLockState`). Ein künstliches CGEvent schaltet sie
//! nicht um. Num und Rollen gibt es auf Mac-Tastaturen nicht; die „Clear“-Taste des Ziffernblocks
//! liegt zwar auf der Position von Num, schaltet aber nichts.

use std::ffi::{c_char, c_void};
use std::sync::Mutex;

use qross_proto::LockState;

type KernReturn = i32;
type MachPort = u32;

const KERN_SUCCESS: KernReturn = 0;
/// `kIOHIDParamConnectType` aus `IOKit/hidsystem/IOHIDShared.h`
const HID_PARAM_CONNECT_TYPE: u32 = 1;
/// `kIOHIDCapsLockState`
const CAPS_LOCK_STATE: i32 = 1;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> MachPort;
    fn IOServiceOpen(service: MachPort, owning_task: MachPort, kind: u32, connect: *mut MachPort) -> KernReturn;
    fn IOServiceClose(connect: MachPort) -> KernReturn;
    fn IOObjectRelease(object: MachPort) -> KernReturn;
    fn IOHIDGetModifierLockState(handle: MachPort, selector: i32, state: *mut bool) -> KernReturn;
    fn IOHIDSetModifierLockState(handle: MachPort, selector: i32, state: bool) -> KernReturn;
}

extern "C" {
    /// Hinter dem Makro `mach_task_self()`
    static mach_task_self_: MachPort;
}

/// Offene Verbindung zu IOHIDSystem (0 = noch keine). Einmal geöffnet und behalten: Den Dienst bei jedem
/// Betreten des Clients neu zu suchen und zu öffnen kostet spürbar Zeit, und so lange warten im
/// Einspeise-Thread alle folgenden Mausbewegungen – bei schnellem Wechsel hakte der Cursor.
static CONNECTION: Mutex<MachPort> = Mutex::new(0);

fn open() -> Option<MachPort> {
    unsafe {
        // Verbraucht von IOServiceGetMatchingService; 0 = Standard-Port
        let matching = IOServiceMatching(c"IOHIDSystem".as_ptr());
        if matching.is_null() {
            return None;
        }
        let service = IOServiceGetMatchingService(0, matching);
        if service == 0 {
            tracing::debug!("IOHIDSystem not found");
            return None;
        }
        let mut connect: MachPort = 0;
        let opened = IOServiceOpen(service, mach_task_self_, HID_PARAM_CONNECT_TYPE, &mut connect);
        IOObjectRelease(service);
        if opened != KERN_SUCCESS {
            tracing::debug!("failed to open IOHIDSystem (error {opened:#x})");
            return None;
        }
        Some(connect)
    }
}

/// `f` mit der offenen Verbindung. Schlägt es fehl (etwa weil IOHIDSystem nach dem Ruhezustand neu
/// angelegt wurde), einmal neu öffnen und wiederholen.
fn with_hid_system<T>(f: impl Fn(MachPort) -> Option<T>) -> Option<T> {
    let mut connection = CONNECTION.lock().unwrap_or_else(|e| e.into_inner());
    if *connection == 0 {
        *connection = open()?;
    }
    if let Some(result) = f(*connection) {
        return Some(result);
    }
    unsafe { IOServiceClose(*connection) };
    *connection = 0;
    *connection = open()?;
    f(*connection)
}

/// Verbindung schon beim Start öffnen, damit auch das erste Betreten nicht darauf wartet
pub fn warm_up() {
    let _ = caps_lock();
}

/// Ist die Feststelltaste an? `None`: nicht lesbar
pub fn caps_lock() -> Option<bool> {
    with_hid_system(|connect| {
        let mut on = false;
        let result = unsafe { IOHIDGetModifierLockState(connect, CAPS_LOCK_STATE, &mut on) };
        (result == KERN_SUCCESS).then_some(on)
    })
}

pub fn current() -> LockState {
    LockState { caps: caps_lock(), num: None, scroll: None }
}

/// Feststelltaste auf `on` stellen, falls sie anders steht. Num und Rollen gibt es hier nicht.
pub fn set(target: LockState) {
    let Some(want) = target.caps else { return };
    if caps_lock() == Some(want) {
        return;
    }
    let done = with_hid_system(|connect| {
        let result = unsafe { IOHIDSetModifierLockState(connect, CAPS_LOCK_STATE, want) };
        (result == KERN_SUCCESS).then_some(())
    });
    if done.is_none() {
        tracing::warn!("failed to toggle Caps Lock (IOHIDSetModifierLockState)");
    }
}
