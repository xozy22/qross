//! Zeichenmodus unter macOS: das Tastaturlayout (`uchr`) der aktuellen Eingabequelle und
//! `UCKeyTranslate` dazu.
//!
//! Die Eingabequelle (Text Input Sources) darf seit macOS 14 nur der Hauptthread abfragen. Qross
//! liest sie deshalb dort, über die Haupt-Queue, die in der App ihr Event-Loop abarbeitet, und legt
//! eine Kopie ab. Erfassung und Einspeisen arbeiten mit der Kopie und lösen nur das Neulesen aus.
//! Ohne solche Schleife (`qross server` im Terminal) gilt das Layout vom Start, sofern der auf dem
//! Hauptthread geschieht; sonst bleibt es bei Tastenpositionen.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::data::{CFData, CFDataRef};
use core_foundation::string::CFStringRef;
use core_graphics::event::CGEventFlags;
use qross_core::chars::{KeyChars, Produces};

use crate::keymap_mac::uc_modifiers;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> CFTypeRef;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn LMGetKbdType() -> u8;
    #[allow(clippy::too_many_arguments)]
    fn UCKeyTranslate(
        layout: *const c_void,
        code: u16,
        action: u16,
        modifiers: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        actual_length: *mut usize,
        chars: *mut u16,
    ) -> i32;
}

extern "C" {
    /// Die Haupt-Queue (`dispatch_get_main_queue()` ist in C nur ein Makro dafür)
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
    fn pthread_main_np() -> i32;
}

const KEY_ACTION_DOWN: u16 = 0;
const NO_DEAD_KEYS: u32 = 1;

/// Kopie eines Tastaturlayouts
pub(crate) struct Layout {
    /// Die `uchr`-Daten, auf 4 Byte ausgerichtet
    data: Vec<u32>,
    keyboard_type: u32,
    /// Zeichen → (Keycode, mit Umschalt), beim ersten Kürzel aufgebaut
    reverse: OnceLock<HashMap<char, (u16, bool)>>,
}

impl Layout {
    fn translate(&self, code: u16, modifiers: u32, options: u32, dead_key_state: &mut u32) -> String {
        let mut buf = [0u16; 8];
        let mut len = 0usize;
        let status = unsafe {
            UCKeyTranslate(
                self.data.as_ptr().cast(),
                code,
                KEY_ACTION_DOWN,
                modifiers,
                self.keyboard_type,
                options,
                dead_key_state,
                buf.len(),
                &mut len,
                buf.as_mut_ptr(),
            )
        };
        if status != 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len.min(buf.len())])
    }

    /// Das Zeichen ohne Rücksicht auf tote Tasten (bei einer toten Taste: ihr Akzent)
    fn plain(&self, code: u16, modifiers: u32) -> Option<char> {
        self.translate(code, modifiers, NO_DEAD_KEYS, &mut 0).chars().next().filter(|c| !c.is_control())
    }

    /// Was die Taste mit den Zusatztasten eines Tastenereignisses ergibt. Am Mac tippt Option
    /// Zeichen (z. B. @ mit Option+L im deutschen Layout); Kürzel sind Cmd und Ctrl.
    pub fn key_chars(&self, code: u16, flags: CGEventFlags) -> KeyChars {
        let has = |flag| flags.contains(flag);
        let base = self.plain(code, 0);
        if has(CGEventFlags::CGEventFlagCommand) || has(CGEventFlags::CGEventFlagControl) {
            return KeyChars { produces: Produces::Nothing, base, shortcut: true };
        }
        let modifiers = uc_modifiers(
            has(CGEventFlags::CGEventFlagShift),
            has(CGEventFlags::CGEventFlagAlphaShift),
            has(CGEventFlags::CGEventFlagAlternate),
        );
        let mut dead_key_state = 0;
        let text = self.translate(code, modifiers, 0, &mut dead_key_state);
        let produces = if text.is_empty() && dead_key_state != 0 {
            self.plain(code, modifiers).or(base).map_or(Produces::Nothing, Produces::Dead)
        } else {
            Produces::from_text(text)
        };
        KeyChars { produces, base, shortcut: false }
    }

    /// Taste, die `ch` ergibt: ohne Zusatztasten, sonst mit Umschalt
    pub fn find(&self, ch: char) -> Option<(u16, bool)> {
        let reverse = self.reverse.get_or_init(|| {
            let mut map = HashMap::new();
            for shift in [false, true] {
                for code in 0..128 {
                    if let Some(c) = self.plain(code, uc_modifiers(shift, false, false)) {
                        map.entry(c).or_insert((code, shift));
                    }
                }
            }
            map
        });
        reverse.get(&ch).copied()
    }
}

/// Zuletzt gelesenes Layout und wann
static CURRENT: Mutex<Option<(Arc<Layout>, Instant)>> = Mutex::new(None);
/// Ein Auftrag an die Haupt-Queue läuft (in einem Programm ohne Hauptschleife für immer)
static PENDING: AtomicBool = AtomicBool::new(false);

pub(crate) fn current() -> Option<Arc<Layout>> {
    CURRENT.lock().ok()?.as_ref().map(|(layout, _)| layout.clone())
}

/// Das Layout neu lesen, wenn die Kopie älter als `max_age` ist (bzw. fehlt). Kehrt sofort zurück;
/// die neue Kopie folgt, sobald der Hauptthread dazu kommt.
pub(crate) fn refresh(max_age: Duration) {
    let fresh = CURRENT.lock().is_ok_and(|current| current.as_ref().is_some_and(|(_, at)| at.elapsed() < max_age));
    if fresh {
        return;
    }
    if unsafe { pthread_main_np() } != 0 {
        return store(load());
    }
    if PENDING.swap(true, Ordering::AcqRel) {
        return;
    }
    extern "C" fn on_main(_: *mut c_void) {
        store(load());
        PENDING.store(false, Ordering::Release);
    }
    unsafe { dispatch_async_f(std::ptr::addr_of!(_dispatch_main_q).cast(), std::ptr::null_mut(), on_main) };
}

fn store(layout: Option<Layout>) {
    match layout {
        Some(layout) => {
            if let Ok(mut current) = CURRENT.lock() {
                *current = Some((Arc::new(layout), Instant::now()));
            }
        }
        None => tracing::debug!("cannot read the keyboard layout of the input source"),
    }
}

/// Nur auf dem Hauptthread
fn load() -> Option<Layout> {
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let layout = (!data.is_null()).then(|| {
            let data = CFData::wrap_under_get_rule(data as CFDataRef);
            let bytes = data.bytes();
            let mut words = vec![0u32; bytes.len().div_ceil(4)];
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), words.as_mut_ptr().cast::<u8>(), bytes.len());
            Layout { data: words, keyboard_type: u32::from(LMGetKbdType()), reverse: OnceLock::new() }
        });
        CFRelease(source);
        layout
    }
}
