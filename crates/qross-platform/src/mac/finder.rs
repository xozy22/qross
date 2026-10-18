//! Cursor-Finder unter macOS: ein randloses, durchsichtiges `NSWindow` auf der Ebene des
//! Bildschirmschoners, in allen Spaces und neben Vollbild-Apps, das Mausereignisse durchlässt
//! (`ignoresMouseEvents`) und nie zum Hauptfenster wird. Das Bild liegt als `CGImage` im Layer der
//! Inhaltsansicht.
//!
//! AppKit-Fenster gehören dem Hauptthread. Cursor, Monitore und Bild ermittelt der Thread des
//! Cursor-Finders selbst (Quartz ist dafür thread-sicher) und reicht nur das fertige Bild über die
//! Hauptwarteschlange (libdispatch) weiter. Läuft dort keine Ereignisschleife (Kommandozeile), bleibt
//! höchstens ein Auftrag liegen, und es ist nichts zu sehen.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use core_graphics::base::{kCGBitmapByteOrder32Little, kCGImageAlphaPremultipliedFirst, kCGRenderingIntentDefault};
use core_graphics::color_space::CGColorSpace;
use core_graphics::data_provider::CGDataProvider;
use core_graphics::display::CGDisplay;
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::image::CGImage;
use foreign_types::ForeignType;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSScreenSaverWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use qross_proto::Rect;

use crate::finder::{Display, Frame, Surface};

/// Was das Fenster als Nächstes zeigen soll
enum Update {
    Show(Frame, f64),
    Hide,
}

/// Neuester Stand für den Hauptthread und ob schon ein Auftrag dorthin unterwegs ist. Es zählt
/// immer nur der neueste Stand; ältere Bilder fallen weg, wenn der Hauptthread nicht nachkommt.
struct Pending {
    latest: Option<Update>,
    queued: bool,
}

static PENDING: Mutex<Pending> = Mutex::new(Pending { latest: None, queued: false });

thread_local! {
    /// Das Fenster (nur im Hauptthread angelegt und benutzt)
    static WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
}

// Wie in `layout.rs` (gleiche Signatur, sonst meldet der Compiler eine abweichende Deklaration)
extern "C" {
    /// Die Hauptwarteschlange (`dispatch_get_main_queue()` ist in C nur ein Makro darauf)
    static _dispatch_main_q: u8;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

extern "C" fn on_main(_context: *mut c_void) {
    // Keine Panik über die C-Grenze tragen
    let _ = std::panic::catch_unwind(apply_pending);
}

fn post(update: Update) {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    pending.latest = Some(update);
    if !pending.queued {
        pending.queued = true;
        unsafe { dispatch_async_f(std::ptr::addr_of!(_dispatch_main_q).cast(), std::ptr::null_mut(), on_main) };
    }
}

/// Im Hauptthread: alles Aufgelaufene anwenden
fn apply_pending() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    loop {
        let update = {
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            match pending.latest.take() {
                Some(update) => update,
                None => {
                    pending.queued = false;
                    return;
                }
            }
        };
        match update {
            Update::Show(frame, primary_height) => show(mtm, &frame, primary_height),
            Update::Hide => WINDOW.with_borrow(|window| {
                if let Some(window) = window {
                    window.orderOut(None);
                }
            }),
        }
    }
}

fn window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    WINDOW.with_borrow_mut(|slot| {
        slot.get_or_insert_with(|| {
            let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    rect,
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(false);
            window.setIgnoresMouseEvents(true);
            window.setLevel(NSScreenSaverWindowLevel);
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            if let Some(view) = window.contentView() {
                view.setWantsLayer(true);
            }
            window
        })
        .clone()
    })
}

/// Im Hauptthread: Bild ins Fenster und Fenster an seinen Platz
fn show(mtm: MainThreadMarker, frame: &Frame, primary_height: f64) {
    let window = window(mtm);
    let Some(view) = window.contentView() else { return };
    let r = frame.rect;
    // Cocoa zählt von unten links des Hauptbildschirms, Quartz von oben links
    let origin = NSPoint::new(r.x as f64, primary_height - r.y as f64 - r.height as f64);
    window.setFrame_display(NSRect::new(origin, NSSize::new(r.width as f64, r.height as f64)), false);
    let image = CGImage::new(
        frame.width as usize,
        frame.height as usize,
        8,
        32,
        frame.width as usize * 4,
        &CGColorSpace::create_device_rgb(),
        kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little,
        &CGDataProvider::from_buffer(Arc::new(frame.pixels.clone())),
        false,
        kCGRenderingIntentDefault,
    );
    unsafe {
        let layer: *mut AnyObject = msg_send![&*view, layer];
        if !layer.is_null() {
            let scale = frame.width as f64 / r.width.max(1) as f64;
            let _: () = msg_send![layer, setContentsScale: scale];
            let _: () = msg_send![layer, setContents: image.as_ptr() as *mut AnyObject];
        }
    }
    // Zeigt das Fenster, ohne Qross in den Vordergrund zu holen
    window.orderFrontRegardless();
}

pub(crate) struct MacSurface {
    source: Option<CGEventSource>,
}

pub(crate) fn surface() -> anyhow::Result<Box<dyn Surface>> {
    Ok(Box::new(MacSurface { source: CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok() }))
}

impl Surface for MacSurface {
    fn prepare(&mut self) -> bool {
        true
    }

    fn cursor(&mut self) -> Option<(i32, i32)> {
        let event = CGEvent::new(self.source.clone()?).ok()?;
        let p = event.location();
        Some((p.x.round() as i32, p.y.round() as i32))
    }

    fn display_at(&mut self, x: i32, y: i32) -> Option<Display> {
        let ids = CGDisplay::active_displays().ok()?;
        let displays: Vec<CGDisplay> = ids.into_iter().map(CGDisplay::new).collect();
        let contains = |d: &&CGDisplay| {
            let b = d.bounds();
            let (fx, fy) = (x as f64, y as f64);
            fx >= b.origin.x && fx < b.origin.x + b.size.width && fy >= b.origin.y && fy < b.origin.y + b.size.height
        };
        let display = displays.iter().find(contains).or(displays.first())?;
        let b = display.bounds();
        // Retina: Bildpunkte je Punkt aus dem Anzeigemodus
        let density = display
            .display_mode()
            .filter(|mode| mode.width() > 0)
            .map(|mode| mode.pixel_width() as f32 / mode.width() as f32)
            .unwrap_or(1.0)
            .clamp(1.0, 4.0);
        let area = Rect::new(b.origin.x as i32, b.origin.y as i32, b.size.width as u32, b.size.height as u32);
        Some(Display { area, scale: 1.0, density })
    }

    fn present(&mut self, frame: &Frame) {
        post(Update::Show(frame.clone(), CGDisplay::main().bounds().size.height));
    }

    fn hide(&mut self) {
        post(Update::Hide);
    }
}
