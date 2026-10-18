//! Drag & Drop: Welcher Ordner liegt unter dem Cursor? Schreibtisch → `~/Desktop`, Finder-Fenster →
//! der Ordner, den es zeigt. Alles andere (Programme, Dock, Menüleiste, Suchfenster) → `None`.
//!
//! Welches Fenster an der Stelle oben liegt, sagt die Fensterliste von CoreGraphics; dafür braucht es
//! keine Rechte. Den Ordner eines Finder-Fensters lesen die Bedienungshilfen (Accessibility), deren
//! Recht Qross ohnehin hat. Der Finder per AppleScript (`target of window`) wüsste ihn genau, verlangt
//! aber das Recht „Automation“ – eine weitere Abfrage des Systems, ausgerechnet mitten im Ablegen.
//! `AXDocument` lässt der Finder unter macOS 26 leer; der Ordner ergibt sich deshalb aus Titel,
//! Pfadleiste und den Datei-URLs einiger sichtbarer Einträge (siehe `dnd_logic::finder_folder`).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::time::{Duration, Instant};

use core_foundation::array::CFArray;
use core_foundation::base::{kCFAllocatorDefault, CFType, CFTypeID, CFTypeRef, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::url::{CFURLCreateFilePathURL, CFURLCreateWithString, CFURL};
use core_foundation::{declare_TCFType, impl_TCFType};
use core_graphics::geometry::CGRect;
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowListOptionOnScreenOnly,
    kCGWindowOwnerPID,
};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{NSFileManager, NSString};

use crate::dnd_logic::{finder_folder, FinderView};

const FINDER: &str = "com.apple.finder";
/// Höchstens so lange darf eine Abfrage beim Finder dauern (er kann gerade beschäftigt sein) …
const TIMEOUT_S: f32 = 0.2;
/// … und das Lesen eines Fensters insgesamt; sonst gilt „Downloads/Qross“.
const READ_LIMIT: Duration = Duration::from_millis(500);
/// Obergrenze der gelesenen Elemente, falls ein Fenster einmal ganz anders aussieht
const MAX_ELEMENTS: usize = 400;
/// Einträge je Liste: genug, um zu sehen, in welchem Ordner sie liegen
const ITEMS_PER_LIST: usize = 3;
/// In der Spaltenansicht nur die letzten Spalten; die weiter links reichen bis zum Volume zurück.
const COLUMNS: usize = 3;
/// Ansichten des Ordnerinhalts, nach `AXIdentifier`
const CONTENT_VIEWS: [&str; 4] = ["ListView", "IconView", "GalleryView", "ColumnView"];

pub fn folder_at(x: i32, y: i32) -> Option<PathBuf> {
    inspect(x, y, &mut Trace(None))
}

/// Wie `folder_at`, dazu jeder Schritt als Text (Diagnose: `folder_at --trace`, Tests)
pub fn trace_folder_at(x: i32, y: i32) -> (Option<PathBuf>, Vec<String>) {
    let mut trace = Trace(Some(Vec::new()));
    let folder = inspect(x, y, &mut trace);
    (folder, trace.0.unwrap_or_default())
}

/// Ablaufspur; `None`: keine (beim gewöhnlichen Ablegen)
struct Trace(Option<Vec<String>>);

impl Trace {
    fn note(&mut self, line: impl FnOnce() -> String) {
        if let Some(lines) = &mut self.0 {
            lines.push(line());
        }
    }

    fn on(&self) -> bool {
        self.0.is_some()
    }
}

fn inspect(x: i32, y: i32, trace: &mut Trace) -> Option<PathBuf> {
    autoreleasepool(|_| {
        let (pid, layer) = top_window_at(f64::from(x), f64::from(y), trace)?;
        let bundle = bundle_id(pid);
        trace.note(|| format!("oberstes Fenster: pid {pid}, Ebene {layer}, Programm {bundle:?}"));
        if bundle.as_deref() != Some(FINDER) {
            tracing::debug!("unter dem Cursor liegt kein Finder-Fenster");
            return None;
        }
        let folder = if layer < 0 {
            // Unterhalb der normalen Fenster: der Schreibtisch, den der Finder zeichnet
            dirs::desktop_dir().filter(|dir| dir.is_dir())
        } else if layer == 0 {
            finder_window_at(pid, x, y, trace)
        } else {
            None
        };
        tracing::debug!(?folder, "Ablageordner bei {x},{y}");
        trace.note(|| format!("Ergebnis: {folder:?}"));
        folder
    })
}

// ------------------------------------------------------------------ Fensterliste

/// Besitzer (pid) und Ebene des obersten sichtbaren Fensters an (x, y), Koordinaten wie CGEvent
fn top_window_at(x: f64, y: f64, trace: &mut Trace) -> Option<(i32, i32)> {
    let Some(windows) = copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID) else {
        trace.note(|| "keine Fensterliste".into());
        return None;
    };
    // Von vorn nach hinten sortiert
    for raw in windows.get_all_values() {
        let info: CFDictionary = unsafe { CFDictionary::wrap_under_get_rule(raw as CFDictionaryRef) };
        let value = |key: CFStringRef| -> Option<CFType> {
            let item = info.find(key as *const c_void)?;
            Some(unsafe { CFType::wrap_under_get_rule(*item as CFTypeRef) })
        };
        let number = |key| value(key).and_then(|v| v.downcast::<CFNumber>());
        let (Some(pid), Some(layer)) = (
            number(unsafe { kCGWindowOwnerPID }).and_then(|n| n.to_i32()),
            number(unsafe { kCGWindowLayer }).and_then(|n| n.to_i32()),
        ) else {
            continue;
        };
        let Some(bounds) = value(unsafe { kCGWindowBounds })
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|dict| CGRect::from_dict_representation(&dict))
        else {
            continue;
        };
        let inside = x >= bounds.origin.x
            && x < bounds.origin.x + bounds.size.width
            && y >= bounds.origin.y
            && y < bounds.origin.y + bounds.size.height;
        if !inside {
            continue;
        }
        let alpha = number(unsafe { kCGWindowAlpha }).and_then(|n| n.to_f64());
        trace.note(|| {
            let (o, s) = (bounds.origin, bounds.size);
            format!("Fenster an der Stelle: pid {pid}, Ebene {layer}, Alpha {alpha:?}, {}×{} bei {},{}", s.width, s.height, o.x, o.y)
        });
        // Unsichtbare Fenster (z. B. Hilfsflächen von Programmen) fangen keine Klicks
        if alpha.is_some_and(|alpha| alpha <= 0.0) {
            continue;
        }
        return Some((pid, layer));
    }
    trace.note(|| format!("kein Fenster bei {x},{y}"));
    None
}

fn bundle_id(pid: i32) -> Option<String> {
    // `processIdentifier` u. Ä. gibt es in objc2-app-kit nur mit dem Feature „libc“.
    let app: Option<Retained<AnyObject>> =
        unsafe { msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid] };
    let id: Option<Retained<NSString>> = unsafe { msg_send![&*app?, bundleIdentifier] };
    id.map(|id| id.to_string())
}

// ------------------------------------------------------------------ Bedienungshilfen

#[repr(C)]
pub struct __AXUIElement(c_void);
pub type AXUIElementRef = *const __AXUIElement;

declare_TCFType!(AxElement, AXUIElementRef);
impl_TCFType!(AxElement, AXUIElementRef, AXUIElementGetTypeID);

// Elemente als `CFTypeRef` wie in `game.rs` (gleiche Deklaration, sonst warnt der Compiler)
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyElementAtPosition(application: CFTypeRef, x: f32, y: f32, element: *mut CFTypeRef) -> i32;
    fn AXUIElementCopyAttributeValue(element: CFTypeRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> i32;
}

impl AxElement {
    /// Eigene Zeitgrenze je Element: Sonst wartet jede Abfrage bis zu 6 s auf einen hängenden Finder.
    fn limited(self) -> Self {
        unsafe { AXUIElementSetMessagingTimeout(self.as_CFTypeRef(), TIMEOUT_S) };
        self
    }

    fn attribute(&self, name: &'static str) -> Option<CFType> {
        let name = CFString::from_static_string(name);
        let mut value: CFTypeRef = null();
        let error = unsafe { AXUIElementCopyAttributeValue(self.as_CFTypeRef(), name.as_concrete_TypeRef(), &mut value) };
        (error == 0 && !value.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(value) })
    }

    fn string(&self, name: &'static str) -> Option<String> {
        self.attribute(name)?.downcast::<CFString>().map(|s| s.to_string())
    }

    fn element(&self, name: &'static str) -> Option<AxElement> {
        self.attribute(name)?.downcast::<AxElement>().map(AxElement::limited)
    }

    fn elements(&self, name: &'static str) -> Vec<AxElement> {
        let Some(array) = self.attribute(name).and_then(|v| v.downcast::<CFArray>()) else { return Vec::new() };
        array
            .get_all_values()
            .into_iter()
            .filter(|raw| !raw.is_null())
            .filter_map(|raw| unsafe { CFType::wrap_under_get_rule(raw as CFTypeRef) }.downcast::<AxElement>())
            .map(AxElement::limited)
            .collect()
    }

    /// Dateipfad aus einer URL (als `CFURL` oder als Text); `None` für andere URLs wie `nwnode:`
    fn path(&self, name: &'static str) -> Option<PathBuf> {
        let value = self.attribute(name)?;
        let url = match value.downcast::<CFURL>() {
            Some(url) => url,
            None => {
                let text = value.downcast::<CFString>()?;
                let raw = unsafe { CFURLCreateWithString(kCFAllocatorDefault, text.as_concrete_TypeRef(), null()) };
                if raw.is_null() {
                    return None;
                }
                unsafe { CFURL::wrap_under_create_rule(raw) }
            }
        };
        // Der Finder liefert Dateiverweise (`file:///.file/id=…`); daraus wird eine URL mit Pfad.
        let raw = unsafe { CFURLCreateFilePathURL(kCFAllocatorDefault, url.as_concrete_TypeRef(), std::ptr::null_mut()) };
        if raw.is_null() {
            return None;
        }
        unsafe { CFURL::wrap_under_create_rule(raw) }.to_path()
    }

    fn role(&self) -> String {
        self.string("AXRole").unwrap_or_default()
    }
}

/// Ordner des Finder-Fensters an (x, y). Ohne das Recht „Bedienungshilfen“ antwortet der Finder nicht.
fn finder_window_at(pid: i32, x: i32, y: i32, trace: &mut Trace) -> Option<PathBuf> {
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return None;
    }
    let app = unsafe { AxElement::wrap_under_create_rule(app as AXUIElementRef) }.limited();
    let mut hit: CFTypeRef = null();
    let error = unsafe { AXUIElementCopyElementAtPosition(app.as_CFTypeRef(), x as f32, y as f32, &mut hit) };
    if error != 0 || hit.is_null() {
        tracing::debug!("Finder-Fenster nicht lesbar (Fehler {error}; fehlt das Recht „Bedienungshilfen“?)");
        trace.note(|| format!("AXUIElementCopyElementAtPosition: Fehler {error}"));
        return None;
    }
    let hit = unsafe { AxElement::wrap_under_create_rule(hit as AXUIElementRef) }.limited();
    trace.note(|| format!("Element an der Stelle: {}", describe(&hit)));
    let Some(window) = window_of(hit) else {
        trace.note(|| "kein Fenster zum Element".into());
        return None;
    };
    trace.note(|| format!("Fenster: {}, Dokument {:?}", describe(&window), window.string("AXDocument")));
    if trace.on() {
        outline(&window, 0, &mut 0, trace);
    }
    // Nur gewöhnliche Fenster; Informationsfenster und Dialoge zeigen keinen Ordner.
    if window.string("AXSubrole").as_deref() != Some("AXStandardWindow") {
        return None;
    }
    let mut reader = Reader { elements: 0, deadline: Instant::now() + READ_LIMIT };
    let view = reader.read(&window);
    tracing::debug!(?view, elements = reader.elements, "Finder-Fenster");
    trace.note(|| format!("gelesen ({} Elemente): {view:?}", reader.elements));
    let names = |path: &Path| {
        let shown = NSFileManager::defaultManager().displayNameAtPath(&NSString::from_str(&path.to_string_lossy()));
        let last = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        vec![nfc(&shown.to_string()), nfc(&last)]
    };
    finder_folder(&view, names, |path| path.is_dir())
}

/// Rolle, Unterrolle, Kennung, Titel, Wert und URL eines Elements, für die Ablaufspur
fn describe(element: &AxElement) -> String {
    let mut text = element.role();
    for (label, name) in [("", "AXSubrole"), ("id=", "AXIdentifier"), ("Titel=", "AXTitle"), ("Wert=", "AXValue")] {
        if let Some(value) = element.string(name).filter(|v| !v.is_empty()) {
            text.push_str(&format!(" {label}{value:?}"));
        }
    }
    if element.attribute("AXURL").is_some() {
        text.push_str(&format!(" URL={:?}", element.path("AXURL")));
    }
    text
}

/// Der Elementbaum eines Fensters, eingerückt (höchstens 8 Ebenen, 12 Kinder je Element, 200 Zeilen)
fn outline(element: &AxElement, depth: usize, lines: &mut usize, trace: &mut Trace) {
    if depth >= 8 || *lines >= 200 {
        return;
    }
    *lines += 1;
    trace.note(|| format!("{}{}", "  ".repeat(depth + 1), describe(element)));
    for child in element.elements("AXChildren").iter().take(12) {
        outline(child, depth + 1, lines, trace);
    }
}

/// Das Fenster, zu dem ein Element gehört
fn window_of(mut element: AxElement) -> Option<AxElement> {
    for _ in 0..20 {
        if element.role() == "AXWindow" {
            return Some(element);
        }
        if let Some(window) = element.element("AXWindow") {
            return Some(window);
        }
        element = element.element("AXParent")?;
    }
    None
}

/// Einheitliche Unicode-Form, damit „ü“ aus dem Titel und aus dem Dateisystem gleich sind
fn nfc(text: &str) -> String {
    NSString::from_str(text).precomposedStringWithCanonicalMapping().to_string()
}

struct Reader {
    elements: usize,
    deadline: Instant,
}

impl Reader {
    fn exhausted(&self) -> bool {
        self.elements >= MAX_ELEMENTS || Instant::now() >= self.deadline
    }

    fn read(&mut self, window: &AxElement) -> FinderView {
        let mut view = FinderView {
            document: window.path("AXDocument"),
            title: nfc(&window.string("AXTitle").unwrap_or_default()),
            ..FinderView::default()
        };
        let mut content = None;
        self.walk(window, 0, &mut view, &mut content);
        // Die Einträge kosten am meisten; ohne Pfadleiste geht es nicht ohne sie. Ohne erkannte
        // Inhaltsansicht (etwa eine andere Version des Finders) aus dem ganzen Fenster: Einträge der
        // Seitenleiste stören nicht, denn es zählt nur ein Ordner, der so heißt wie das Fenster.
        let resolved = finder_folder(&view, |_| Vec::new(), |path| path.is_dir()).is_some();
        if !resolved {
            let content = content.unwrap_or_else(|| window.clone());
            self.collect_items(&content, 0, &mut view.items);
        }
        view
    }

    /// Sucht die Pfadleiste und die Ansicht des Ordnerinhalts – nicht in Seitenleiste, Symbolleiste,
    /// Tableiste oder Rollbalken.
    fn walk(&mut self, element: &AxElement, depth: usize, view: &mut FinderView, content: &mut Option<AxElement>) {
        if depth >= 10 || self.exhausted() {
            return;
        }
        self.elements += 1;
        let role = element.role();
        let identifier = element.string("AXIdentifier");
        if matches!(role.as_str(), "AXToolbar" | "AXTabGroup" | "AXScrollBar" | "AXButton") {
            return;
        }
        if role == "AXOutline" && identifier.as_deref() != Some("ListView") {
            // die Seitenleiste
            return;
        }
        if identifier.as_deref().is_some_and(|id| CONTENT_VIEWS.contains(&id)) {
            *content = Some(element.clone());
            return;
        }
        let children = element.elements("AXChildren");
        if role == "AXList" {
            if let Some(crumbs) = path_bar(&children) {
                view.crumbs = crumbs;
                return;
            }
        }
        for child in &children {
            self.walk(child, depth + 1, view, content);
        }
    }

    /// Pfade der ersten Einträge jeder Liste der Inhaltsansicht
    fn collect_items(&mut self, element: &AxElement, depth: usize, items: &mut Vec<PathBuf>) {
        if depth >= 12 || self.exhausted() {
            return;
        }
        self.elements += 1;
        if let Some(path) = element.path("AXURL") {
            items.push(path);
            return;
        }
        let role = element.role();
        let children: Vec<AxElement> = match role.as_str() {
            "AXScrollBar" | "AXToolbar" | "AXTabGroup" => return,
            "AXBrowser" => {
                let columns = element.elements("AXColumns");
                columns.into_iter().rev().take(COLUMNS).collect()
            }
            // Eine Tabelle nennt unter ihren Kindern auch die Spalten; Einträge stehen nur in Zeilen.
            "AXOutline" | "AXTable" => {
                element.elements("AXChildren").into_iter().filter(|c| c.role() == "AXRow").take(ITEMS_PER_LIST).collect()
            }
            "AXList" => element.elements("AXChildren").into_iter().take(ITEMS_PER_LIST).collect(),
            _ => element.elements("AXChildren"),
        };
        for child in &children {
            let before = items.len();
            self.collect_items(child, depth + 1, items);
            // Symbol und Name eines Eintrags tragen dieselbe URL; eine genügt.
            if role == "AXGroup" && items.len() > before {
                break;
            }
        }
    }
}

/// Die Pfadleiste ist eine Liste von Texten, jeder mit der URL des Ordners, den er nennt.
fn path_bar(children: &[AxElement]) -> Option<Vec<(String, Option<PathBuf>)>> {
    let first = children.first()?;
    if first.role() != "AXStaticText" || first.attribute("AXURL").is_none() {
        return None;
    }
    Some(children.iter().map(|c| (nfc(&c.string("AXValue").unwrap_or_default()), c.path("AXURL"))).collect())
}
