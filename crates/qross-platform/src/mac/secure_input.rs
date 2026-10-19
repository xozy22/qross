//! „Sichere Tastatureingabe“ abfragen (siehe [`mod@crate::secure_input`]).
//!
//! Ob sie an ist, sagt `IsSecureEventInputEnabled` (Carbon); das gilt systemweit, egal welcher
//! Prozess sie eingeschaltet hat. Wer sie hält, steht nicht dokumentiert unter
//! `kCGSSessionSecureInputPID` in der Beschreibung der Sitzung – dieselbe Angabe, die
//! `ioreg -l -w 0 | grep SecureInput` zeigt. Schaltet sie ein Prozess ein, der kein Programm mit
//! Fenstern ist (etwa ein Kommandozeilenwerkzeug), trägt macOS dort das Programm ein, das gerade
//! vorn ist (in der CI: Finder). Für Terminal, Browser, Passwortmanager und loginwindow ist es das
//! Programm selbst.

use std::ffi::c_void;

use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

use crate::secure_input::{app_name, SecureInput};

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    /// `Boolean`, also `unsigned char`
    fn IsSecureEventInputEnabled() -> u8;
    #[cfg(test)]
    fn EnableSecureEventInput() -> i32;
    #[cfg(test)]
    fn DisableSecureEventInput() -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// `NULL` ohne Verbindung zum Fensterserver
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

// libproc, Teil von libSystem
extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
    fn proc_name(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

/// `PROC_PIDPATHINFO_MAXSIZE`
const PATH_MAX: usize = 4096;

pub fn state() -> Option<SecureInput> {
    if unsafe { IsSecureEventInputEnabled() } == 0 {
        return None;
    }
    let pid = holder();
    Some(SecureInput { pid, app: pid.and_then(app_of) })
}

/// Prozess, der die sichere Tastatureingabe hält
fn holder() -> Option<u32> {
    let session = unsafe { CGSessionCopyCurrentDictionary() };
    if session.is_null() {
        return None;
    }
    let session: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_create_rule(session) };
    let pid = session.find(CFString::from_static_string("kCGSSessionSecureInputPID"))?.downcast::<CFNumber>()?.to_i64()?;
    u32::try_from(pid).ok().filter(|&pid| pid > 0)
}

/// Name des Programms: aus dem Pfad (Bündel statt Hilfsprozess), sonst der Kurzname des Prozesses
fn app_of(pid: u32) -> Option<String> {
    let pid = i32::try_from(pid).ok()?;
    let mut buffer = vec![0u8; PATH_MAX];
    let len = unsafe { proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if len > 0 {
        if let Some(name) = app_name(&String::from_utf8_lossy(&buffer[..len as usize])) {
            return Some(name);
        }
    }
    let len = unsafe { proc_name(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    (len > 0).then(|| String::from_utf8_lossy(&buffer[..len as usize]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Schaltet die sichere Tastatureingabe für diesen Prozess ein und wieder aus, auch wenn der
    /// Test scheitert (sonst bliebe sie bis zum Ende des Prozesses an).
    struct Enabled;

    impl Enabled {
        fn new() -> Self {
            assert_eq!(unsafe { EnableSecureEventInput() }, 0, "EnableSecureEventInput");
            Self
        }
    }

    impl Drop for Enabled {
        fn drop(&mut self) {
            unsafe { DisableSecureEventInput() };
        }
    }

    /// Ein- und Ausschalten in einem Test: Parallel laufende Tests sähen sonst den Zustand der anderen.
    #[test]
    fn detects_secure_input_of_this_process() {
        let before = state();
        if let Some(other) = &before {
            // Auf einem Arbeitsrechner kann sie gerade an sein (Passwortfeld, Terminal); in der CI nie.
            assert!(std::env::var_os("CI").is_none(), "in der CI schon an: {}", other.describe());
            eprintln!("Sichere Tastatureingabe ist schon an ({}) – übersprungen", other.describe());
            return;
        }

        let on = Enabled::new();
        let held = state().expect("nach EnableSecureEventInput an");
        eprintln!("gehalten von {}", held.describe());
        // Dieser Test ist kein Programm mit Fenstern: macOS nennt dann das Programm vorn (in der CI
        // der Finder), nicht ihn selbst. Es muss aber ein laufender Prozess mit Namen sein.
        let pid = held.pid.expect("kCGSSessionSecureInputPID in der Sitzungsbeschreibung");
        let app = held.app.as_deref().expect("Programmname zum Halter");
        assert!(pid > 0 && !app.is_empty(), "{}", held.describe());

        drop(on);
        assert_eq!(state(), None, "nach DisableSecureEventInput wieder aus");
    }

    #[test]
    fn names_a_process_by_its_executable() {
        let exe = std::env::current_exe().unwrap();
        let exe = exe.file_name().unwrap().to_string_lossy();
        let app = app_of(std::process::id()).expect("Name des eigenen Prozesses");
        // Kein Bündel: der Dateiname (`proc_name` kürzt auf 32 Zeichen)
        assert!(!app.is_empty() && exe.starts_with(&app), "Programm „{app}“, erwartet „{exe}“");
        assert_eq!(app_of(i32::MAX as u32), None, "kein solcher Prozess");
    }
}
