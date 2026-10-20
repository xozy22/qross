//! macOS: Ruhezustand über IOKit (`IORegisterForSystemPower`), Netzwerk über SCDynamicStore (eigene
//! Adressen und Standardroute, wie sie `configd` pflegt). Beides läuft in einer eigenen Run-Loop auf
//! einem eigenen Thread, also auch in der Kommandozeile, wo keine App-Ereignisschleife läuft
//! (`NSWorkspaceDidWakeNotification` käme dort nie an).

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{kCFAllocatorDefault, CFAllocatorRef, CFIndex, TCFType};
use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRun, CFRunLoopSourceRef};
use core_foundation::string::{CFString, CFStringRef};
use tracing::warn;

use super::{dispatch, ready, SystemEvent, BEFORE_SLEEP};

/// `io_connect_t`, `io_object_t` (Mach-Ports)
type IoPort = u32;
type IoNotificationPortRef = *mut c_void;
type IoServiceInterestCallback =
    extern "C" fn(refcon: *mut c_void, service: IoPort, message_type: u32, message_argument: *mut c_void);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IORegisterForSystemPower(
        refcon: *mut c_void,
        port: *mut IoNotificationPortRef,
        callback: IoServiceInterestCallback,
        notifier: *mut IoPort,
    ) -> IoPort;
    fn IONotificationPortGetRunLoopSource(port: IoNotificationPortRef) -> CFRunLoopSourceRef;
    fn IOAllowPowerChange(kernel_port: IoPort, notification_id: isize) -> i32;
}

/// `iokit_common_msg(…)` aus `IOMessage.h`
const CAN_SYSTEM_SLEEP: u32 = 0xE000_0270;
const SYSTEM_WILL_SLEEP: u32 = 0xE000_0280;
const SYSTEM_HAS_POWERED_ON: u32 = 0xE000_0300;

type DynamicStoreRef = *const c_void;
type DynamicStoreCallback = extern "C" fn(store: DynamicStoreRef, changed_keys: CFArrayRef, info: *mut c_void);

#[link(name = "SystemConfiguration", kind = "framework")]
extern "C" {
    fn SCDynamicStoreCreate(
        allocator: CFAllocatorRef,
        name: CFStringRef,
        callout: Option<DynamicStoreCallback>,
        context: *mut c_void,
    ) -> DynamicStoreRef;
    fn SCDynamicStoreSetNotificationKeys(store: DynamicStoreRef, keys: CFArrayRef, patterns: CFArrayRef) -> u8;
    fn SCDynamicStoreCreateRunLoopSource(
        allocator: CFAllocatorRef,
        store: DynamicStoreRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
}

/// Verbindung zur Energieverwaltung, für die Antworten in `on_power`
static ROOT_PORT: AtomicU32 = AtomicU32::new(0);

pub fn start() {
    if let Err(e) = std::thread::Builder::new().name("qross-sysevents".into()).spawn(run) {
        warn!("not watching system events: {e}");
    }
}

fn run() {
    let run_loop = unsafe { CFRunLoopGetCurrent() };

    let mut port: IoNotificationPortRef = null_mut();
    let mut notifier: IoPort = 0;
    let root = unsafe { IORegisterForSystemPower(null_mut(), &mut port, on_power, &mut notifier) };
    if root == 0 {
        warn!("not watching for sleep (IORegisterForSystemPower)");
    } else {
        ROOT_PORT.store(root, Ordering::Release);
        unsafe { CFRunLoopAddSource(run_loop, IONotificationPortGetRunLoopSource(port), kCFRunLoopDefaultMode) };
        ready(true);
    }

    let name = CFString::new("Qross");
    let store = unsafe { SCDynamicStoreCreate(kCFAllocatorDefault, name.as_concrete_TypeRef(), Some(on_network), null_mut()) };
    if store.is_null() {
        warn!("not watching the network (SCDynamicStoreCreate)");
    } else {
        // Standardroute und Adressen jeder Schnittstelle (WLAN gewechselt, Kabel, VPN)
        let keys = CFArray::from_CFTypes(&[CFString::new("State:/Network/Global/IPv4"), CFString::new("State:/Network/Global/IPv6")]);
        let patterns = CFArray::from_CFTypes(&[
            CFString::new("State:/Network/Interface/[^/]+/IPv4"),
            CFString::new("State:/Network/Interface/[^/]+/IPv6"),
        ]);
        let watched =
            unsafe { SCDynamicStoreSetNotificationKeys(store, keys.as_concrete_TypeRef(), patterns.as_concrete_TypeRef()) };
        let source = unsafe { SCDynamicStoreCreateRunLoopSource(kCFAllocatorDefault, store, 0) };
        if watched == 0 || source.is_null() {
            warn!("not watching the network (SCDynamicStore)");
        } else {
            unsafe { CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode) };
            ready(false);
        }
    }
    // Läuft bis zum Prozessende; Store und Quellen bleiben so lange bestehen.
    unsafe { CFRunLoopRun() };
}

extern "C" fn on_power(_refcon: *mut c_void, _service: IoPort, message: u32, argument: *mut c_void) {
    let root = ROOT_PORT.load(Ordering::Acquire);
    match message {
        // Darf es schlafen? Ja – wach halten will Qross den Rechner nicht.
        CAN_SYSTEM_SLEEP => unsafe {
            IOAllowPowerChange(root, argument as isize);
        },
        SYSTEM_WILL_SLEEP => {
            dispatch(SystemEvent::Sleeping);
            // macOS wartet auf die Antwort (bis 30 s); so lange können Verbindungen schließen.
            std::thread::sleep(BEFORE_SLEEP);
            unsafe { IOAllowPowerChange(root, argument as isize) };
        }
        SYSTEM_HAS_POWERED_ON => dispatch(SystemEvent::Woke),
        _ => {}
    }
}

extern "C" fn on_network(_store: DynamicStoreRef, _changed: CFArrayRef, _info: *mut c_void) {
    dispatch(SystemEvent::NetworkChanged);
}
