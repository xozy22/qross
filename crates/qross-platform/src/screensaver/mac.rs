//! macOS: Bildschirmschoner über die verteilten Benachrichtigungen und `ScreenSaverEngine`.
//!
//! macOS liefert verteilte Benachrichtigungen über einen Run Loop: in der Oberfläche über den des
//! Haupt-Threads, sonst über den eigenen Thread, der sich angemeldet hat. Der Rückruf setzt nur einen
//! Merker, egal in welchem Thread.

use std::ffi::{c_char, c_int, c_void};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};

use super::Outcome;

const DID_START: &str = "com.apple.screensaver.didstart";
const DID_STOP: &str = "com.apple.screensaver.didstop";
/// Prozess des Bildschirmschoners (`/System/Library/CoreServices/ScreenSaverEngine.app`)
const ENGINE: &str = "ScreenSaverEngine";
/// `CFNotificationSuspensionBehaviorDeliverImmediately`
const DELIVER_IMMEDIATELY: isize = 4;
const RTLD_LAZY: c_int = 1;

type NotificationCallback =
    extern "C" fn(center: *mut c_void, observer: *mut c_void, name: CFStringRef, object: *const c_void, info: CFDictionaryRef);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFNotificationCenterGetDistributedCenter() -> *mut c_void;
    fn CFNotificationCenterAddObserver(
        center: *mut c_void,
        observer: *const c_void,
        callback: NotificationCallback,
        name: CFStringRef,
        object: *const c_void,
        suspension: isize,
    );
    fn CFRunLoopRun();
    fn CFPreferencesCopyValue(key: CFStringRef, app: CFStringRef, user: CFStringRef, host: CFStringRef) -> CFTypeRef;
    static kCFPreferencesCurrentUser: CFStringRef;
    static kCFPreferencesCurrentHost: CFStringRef;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMCopyAssertionsStatus(status: *mut CFDictionaryRef) -> i32;
}

extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static WATCH: Once = Once::new();
/// Kennung des Beobachters bei der Notification Center
static OBSERVER: u8 = 0;

pub fn running() -> bool {
    WATCH.call_once(|| {
        RUNNING.store(engine_running(), Ordering::Relaxed);
        let spawned = std::thread::Builder::new().name("qross-screensaver".into()).spawn(|| unsafe {
            let center = CFNotificationCenterGetDistributedCenter();
            for name in [DID_START, DID_STOP] {
                let name = CFString::new(name);
                let observer = &OBSERVER as *const u8 as *const c_void;
                CFNotificationCenterAddObserver(
                    center,
                    observer,
                    on_notification,
                    name.as_concrete_TypeRef(),
                    std::ptr::null(),
                    DELIVER_IMMEDIATELY,
                );
            }
            CFRunLoopRun();
        });
        if let Err(e) = spawned {
            tracing::warn!("screen saver: failed to start the observer: {e}");
        }
    });
    RUNNING.load(Ordering::Relaxed)
}

extern "C" fn on_notification(
    _center: *mut c_void,
    _observer: *mut c_void,
    name: CFStringRef,
    _object: *const c_void,
    _info: CFDictionaryRef,
) {
    if name.is_null() {
        return;
    }
    let name = unsafe { CFString::wrap_under_get_rule(name) }.to_string();
    match name.as_str() {
        DID_START => RUNNING.store(true, Ordering::Relaxed),
        DID_STOP => RUNNING.store(false, Ordering::Relaxed),
        _ => {}
    }
}

/// Läuft `ScreenSaverEngine`? Für Start und Ende, die selten vorkommen; die regelmäßige Abfrage
/// nimmt die Benachrichtigungen.
fn engine_running() -> bool {
    Command::new("/usr/bin/pgrep").args(["-x", ENGINE]).output().is_ok_and(|out| out.status.success())
}

/// „Bildschirmschoner starten nach“ in Sekunden, `None`: nicht gesetzt (Vorgabe des Systems)
fn idle_time() -> Option<i64> {
    let key = CFString::new("idleTime");
    let app = CFString::new("com.apple.screensaver");
    let value = unsafe {
        CFPreferencesCopyValue(
            key.as_concrete_TypeRef(),
            app.as_concrete_TypeRef(),
            kCFPreferencesCurrentUser,
            kCFPreferencesCurrentHost,
        )
    };
    if value.is_null() {
        return None;
    }
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    value.downcast::<CFNumber>().and_then(|n| n.to_i64())
}

/// Hält ein Programm den Bildschirm wach (`PreventUserIdleDisplaySleep`, etwa ein Video)?
fn kept_awake() -> bool {
    let mut status: CFDictionaryRef = std::ptr::null();
    if unsafe { IOPMCopyAssertionsStatus(&mut status) } != 0 || status.is_null() {
        return false;
    }
    let status: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(status) };
    let key = CFString::new("PreventUserIdleDisplaySleep");
    status.find(&key).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i64()).is_some_and(|n| n > 0)
}

pub fn start() -> Outcome {
    if engine_running() {
        return Outcome::Unchanged;
    }
    if !super::mac_configured(idle_time()) {
        return Outcome::NotConfigured;
    }
    if kept_awake() {
        return Outcome::KeptAwake;
    }
    match Command::new("/usr/bin/open").args(["-a", ENGINE]).output() {
        Ok(out) if out.status.success() => Outcome::Started,
        Ok(out) => Outcome::Failed(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// `ScreenSaverEngine` beenden (wie `killall`), so wie es eine Bewegung der Maus täte. Das wirkt auch
/// auf einem Server, dessen Erfassung die Eingaben gerade für einen Client abfängt. War der Mac
/// gesperrt, zeigt er danach die Anmeldung.
pub fn stop() -> Outcome {
    if !engine_running() {
        return Outcome::Unchanged;
    }
    match Command::new("/usr/bin/killall").arg(ENGINE).output() {
        Ok(out) if out.status.success() => Outcome::Stopped,
        Ok(out) => Outcome::Failed(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// Sperrt sofort, wie „Bildschirm sperren“ im Apple-Menü. Die Funktion dafür ist nicht
/// dokumentiert; fehlt sie, meldet das `Unsupported`, und es startet nur der Bildschirmschoner.
pub fn lock() -> Outcome {
    let framework = c"/System/Library/PrivateFrameworks/login.framework/Versions/Current/login";
    let handle = unsafe { dlopen(framework.as_ptr(), RTLD_LAZY) };
    if handle.is_null() {
        return Outcome::Unsupported("without login.framework");
    }
    let symbol = unsafe { dlsym(handle, c"SACLockScreenImmediate".as_ptr()) };
    if symbol.is_null() {
        return Outcome::Unsupported("without SACLockScreenImmediate");
    }
    // SAFETY: `int SACLockScreenImmediate(void)` aus login.framework
    let lock_now: extern "C" fn() -> c_int = unsafe { std::mem::transmute(symbol) };
    match lock_now() {
        0 => Outcome::Locked,
        code => Outcome::Failed(format!("error code {code}")),
    }
}
