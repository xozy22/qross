//! Rechte unter Systemeinstellungen → Datenschutz & Sicherheit.
//!
//! macOS merkt sich zu jedem Eintrag die designierte Anforderung der Signatur und prüft bei jedem
//! Zugriff, ob die laufende App sie erfüllt. Erfüllt sie sie nicht mehr (etwa nach einem Update von
//! einer nur ad hoc signierten Version), bleibt der Eintrag sichtbar und eingeschaltet, wirkt aber
//! nicht: Ereignisse kommen nicht an, ohne Fehlermeldung. Deshalb hier ausdrücklich nachfragen.

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};

use crate::Permission;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    /// `Boolean` aus CoreFoundation, also `unsigned char`
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

// Seit macOS 10.15; Qross setzt 13.0 voraus.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

pub fn granted(permission: Permission) -> bool {
    match permission {
        Permission::Accessibility => unsafe { AXIsProcessTrusted() != 0 },
        Permission::InputMonitoring => unsafe { CGPreflightListenEventAccess() },
    }
}

/// Trägt Qross in die Liste ein (ausgeschaltet) und zeigt beim ersten Mal die Abfrage des Systems.
pub fn request(permission: Permission) {
    match permission {
        Permission::Accessibility => {
            let prompt = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
            let options = CFDictionary::from_CFType_pairs(&[(prompt.as_CFType(), CFBoolean::true_value().as_CFType())]);
            unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) };
        }
        Permission::InputMonitoring => {
            unsafe { CGRequestListenEventAccess() };
        }
    }
}
