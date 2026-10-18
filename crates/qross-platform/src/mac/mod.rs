//! macOS über Quartz Event Services (CGEventTap / CGEventPost).
//!
//! Qross braucht dafür unter Systemeinstellungen → Datenschutz & Sicherheit die Rechte
//! „Bedienungshilfen“ (Einspeisen) und „Eingabeüberwachung“ (Erfassen).
//! Koordinaten sind macOS-Punkte (nicht Pixel) – für das Protokoll egal, da jeder Rechner
//! sein eigenes, in sich stimmiges Koordinatensystem meldet.

mod capture;
mod drag;
mod finder;
mod game;
mod inject;
mod layout;
mod media;
mod pasteboard;
mod permissions;

use std::sync::Arc;

use anyhow::Result;
use core_graphics::display::CGDisplay;
use core_graphics::geometry::CGRect;
use qross_core::CaptureEvent;
use qross_proto::Rect;
use tokio::sync::mpsc::UnboundedSender;

use crate::arboard_clipboard::ArboardClipboard;
use crate::{Capture, Clipboard, Injector, Permission};

pub use drag::DragWatch;
pub use finder::trace_folder_at;
pub use game::game_state;
pub(crate) use pasteboard::concealed;
pub use permissions::{granted as has_permission, request as request_permission};

pub fn init_process() {}

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
            "„Bedienungshilfen“ ist für Qross nicht erlaubt – eingespeiste Maus- und Tastaturereignisse kommen \
             nicht an. Unter Systemeinstellungen → Datenschutz & Sicherheit freigeben; ist Qross dort schon \
             eingeschaltet, den Eintrag entfernen und neu hinzufügen."
        );
    }
    Ok(Box::new(inject::MacInjector::new()?))
}

pub fn clipboard() -> Box<dyn Clipboard> {
    Box::new(ArboardClipboard::default())
}
