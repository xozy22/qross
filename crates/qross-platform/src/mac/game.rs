//! Spielemodus: Ist das fokussierte Fenster im Vollbild?
//!
//! Gefragt wird über die Bedienungshilfen-Schnittstelle (dasselbe Recht, das Qross zum Einspeisen
//! und Erfassen ohnehin braucht). Ob ein Spiel den Zeiger vom Cursor gelöst hat
//! (`CGAssociateMouseAndMouseCursorPosition`), verrät macOS nicht; das bleibt deshalb `false`.

use std::ffi::c_void;

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use qross_proto::GameState;

type AXError = i32;
const AX_SUCCESS: AXError = 0;
/// `kAXValueCGPointType` bzw. `kAXValueCGSizeType`
const AX_VALUE_CG_POINT: u32 = 1;
const AX_VALUE_CG_SIZE: u32 = 2;
/// Antwortet ein Programm nicht, nicht die vollen 6 s des Systems warten
const TIMEOUT_SECS: f32 = 0.25;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(element: CFTypeRef, attribute: CFStringRef, value: *mut CFTypeRef) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> AXError;
    /// `Boolean` aus CoreFoundation, also `unsigned char`
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
}

pub fn game_state() -> GameState {
    GameState { fullscreen: focused_window_fullscreen().unwrap_or(false), captured: false }
}

/// Vollbild (grüner Knopf, eigener Space) oder ein randloses Fenster, das einen Bildschirm samt
/// Menüleiste bedeckt, wie es viele Spiele öffnen. Ein gezoomtes Fenster lässt die Menüleiste frei.
fn focused_window_fullscreen() -> Option<bool> {
    let system = unsafe { CFType::wrap_under_create_rule(AXUIElementCreateSystemWide()) };
    unsafe { AXUIElementSetMessagingTimeout(system.as_CFTypeRef(), TIMEOUT_SECS) };
    let app = attribute(&system, "AXFocusedApplication")?;
    let window = attribute(&app, "AXFocusedWindow")?;
    if attribute(&window, "AXFullScreen").and_then(|v| v.downcast::<CFBoolean>()).is_some_and(bool::from) {
        return Some(true);
    }
    let origin: CGPoint = value(&attribute(&window, "AXPosition")?, AX_VALUE_CG_POINT)?;
    let size: CGSize = value(&attribute(&window, "AXSize")?, AX_VALUE_CG_SIZE)?;
    let frame = CGRect::new(&origin, &size);
    Some(super::displays().iter().any(|display| covers(&frame, display)))
}

fn attribute(element: &CFType, name: &str) -> Option<CFType> {
    let name = CFString::new(name);
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(element.as_CFTypeRef(), name.as_concrete_TypeRef(), &mut out) };
    (err == AX_SUCCESS && !out.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(out) })
}

/// Inhalt eines `AXValue` (Punkt oder Größe)
fn value<T: Default>(value: &CFType, kind: u32) -> Option<T> {
    let mut out = T::default();
    let ok = unsafe { AXValueGetValue(value.as_CFTypeRef(), kind, &mut out as *mut T as *mut c_void) };
    (ok != 0).then_some(out)
}

/// Bedeckt `frame` den Bildschirm `display` ganz (ein Punkt Spielraum für Rundungen)?
fn covers(frame: &CGRect, display: &CGRect) -> bool {
    const SLACK: f64 = 1.0;
    frame.origin.x <= display.origin.x + SLACK
        && frame.origin.y <= display.origin.y + SLACK
        && frame.origin.x + frame.size.width >= display.origin.x + display.size.width - SLACK
        && frame.origin.y + frame.size.height >= display.origin.y + display.size.height - SLACK
}
