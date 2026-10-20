//! macOS über Quartz Event Services (CGEventTap / CGEventPost).
//!
//! Qross braucht dafür unter Systemeinstellungen → Datenschutz & Sicherheit die Rechte
//! „Bedienungshilfen“ (Einspeisen) und „Eingabeüberwachung“ (Erfassen).
//! Koordinaten sind macOS-Punkte (nicht Pixel) – für das Protokoll egal, da jeder Rechner
//! sein eigenes, in sich stimmiges Koordinatensystem meldet.

mod capture;
mod drag;
mod finder;
mod finder_folder;
mod game;
mod inject;
mod layout;
mod locks;
mod media;
mod pasteboard;
mod permissions;
mod secure_input;

use std::sync::Arc;

use anyhow::Result;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::CGRect;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
use qross_core::CaptureEvent;
use qross_proto::Rect;
use tokio::sync::mpsc::UnboundedSender;

use crate::arboard_clipboard::ArboardClipboard;
use crate::{Capture, Clipboard, FinderOverlay, Injector, Permission};

pub use drag::DragWatch;
pub use finder_folder::trace_folder_at;
pub use game::game_state;
pub(crate) use pasteboard::concealed;
pub use permissions::{granted as has_permission, request as request_permission};
pub use secure_input::state as secure_input;

/// Einmal beim Programmstart; unter macOS gibt es dabei nichts zu tun (App Nap: siehe [`Activity`]).
pub fn init_process() {}

/// App Nap aus, solange der Wert lebt: für die Zeit, in der Server oder Client laufen. Läuft Qross
/// ohne sichtbares Fenster (im Tray, als `qross client`), bremst macOS es sonst aus: Timer und Netzwerk
/// werden gebündelt, und die ersten Eingaben nach einer Pause kommen spürbar verzögert an. Gestoppt
/// darf macOS wieder sparen, das schont den Akku. Synergy macht es genauso. Bildschirmschoner,
/// Display- und Systemschlaf bleiben erlaubt.
pub struct Activity(Retained<ProtocolObject<dyn NSObjectProtocol>>);

// SAFETY: Das Token ist ein unveränderliches NSObject, und NSProcessInfo ist thread-sicher: Beenden
// geht von jedem Thread aus.
unsafe impl Send for Activity {}
unsafe impl Sync for Activity {}

impl Activity {
    pub fn begin() -> Self {
        let options = NSActivityOptions::UserInitiatedAllowingIdleSystemSleep | NSActivityOptions::LatencyCritical;
        let reason = NSString::from_str("Qross forwards mouse and keyboard without delay");
        let token = NSProcessInfo::processInfo().beginActivityWithOptions_reason(options, &reason);
        tracing::debug!("App Nap off while Qross is running");
        Self(token)
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        // SAFETY: Das Token stammt aus beginActivityWithOptions_reason, hat also den erwarteten Typ.
        unsafe { NSProcessInfo::processInfo().endActivity(&self.0) };
        tracing::debug!("App Nap allowed again");
    }
}

pub(crate) fn displays() -> Vec<CGRect> {
    CGDisplay::active_displays()
        .map(|ids| ids.into_iter().map(|id| CGDisplay::new(id).bounds()).collect())
        .unwrap_or_default()
}

pub fn screen_bounds() -> Rect {
    let rects = displays();
    if rects.is_empty() {
        return Rect::new(0, 0, 1920, 1080);
    }
    let left = rects.iter().map(|r| r.origin.x).fold(f64::MAX, f64::min);
    let top = rects.iter().map(|r| r.origin.y).fold(f64::MAX, f64::min);
    let right = rects.iter().map(|r| r.origin.x + r.size.width).fold(f64::MIN, f64::max);
    let bottom = rects.iter().map(|r| r.origin.y + r.size.height).fold(f64::MIN, f64::max);
    Rect::new(left as i32, top as i32, (right - left) as u32, (bottom - top) as u32)
}

/// `CGDisplayBounds` je aktivem Display, in Punkten wie `screen_bounds()`
pub fn monitors() -> Vec<Rect> {
    displays()
        .iter()
        .map(|r| Rect::new(r.origin.x as i32, r.origin.y as i32, r.size.width as u32, r.size.height as u32))
        .collect()
}

pub fn start_capture(events: UnboundedSender<CaptureEvent>) -> Result<Arc<dyn Capture>> {
    capture::start(events)
}

pub fn injector() -> Result<Box<dyn Injector>> {
    // Ohne das Recht verwirft macOS eingespeiste Ereignisse stillschweigend.
    if !has_permission(Permission::Accessibility) {
        tracing::warn!(
            "Qross does not have the 'Accessibility' permission, so injected mouse and keyboard events do not \
             arrive. Grant it under System Settings → Privacy & Security; if Qross is already turned on there, \
             remove the entry and add it again."
        );
    }
    let injector = inject::MacInjector::new()?;
    locks::warm_up();
    Ok(Box::new(injector))
}

pub fn clipboard() -> Box<dyn Clipboard> {
    Box::new(ArboardClipboard::default())
}

pub fn finder_overlay() -> Option<Arc<dyn FinderOverlay>> {
    Some(Arc::new(crate::finder::Overlay::new(finder::surface)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_nap_stays_off_while_an_activity_lives() {
        // Beginnen, beenden, und zwei zugleich (etwa Neustart des Servers): kein Absturz
        drop(super::Activity::begin());
        let first = super::Activity::begin();
        let second = super::Activity::begin();
        drop(first);
        drop(second);
    }
}
