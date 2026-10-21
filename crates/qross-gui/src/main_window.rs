//! Hauptfenster bei Bedarf (Logik in `qross_gui::window`): Zeigen legt es an, im Tray versteckt endet es
//! nach 30 s samt den Prozessen der Web-Oberfläche, Lage und Größe bleiben dazwischen gemerkt. Aufträge
//! an die Seite (Einstellungen oder Neuigkeiten öffnen, Start aus dem Tray) warten, bis sie geladen ist
//! und sich mit `ui_ready` meldet.

#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result};
use qross_gui::autostart;
use qross_gui::window::{self, AtStart, Bounds, Life, Placement, Show, UiAction, CLOSE_AFTER, LABEL};
use serde::Serialize;
use tauri::webview::{PageLoadEvent, PageLoadPayload};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Position, Size, Webview,
    WebviewWindow, WebviewWindowBuilder, Window,
};
use tracing::{info, warn};

use crate::tray;

/// Zweiter Versuch, wenn das Anlegen scheitert (etwa während der alte Browser-Prozess noch endet)
const RETRY_AFTER: Duration = Duration::from_secs(1);

struct State {
    life: Life,
    placement: Placement,
}

/// Nur kurz sperren und nie, während ein Fenster-Aufruf auf den Hauptthread wartet: Aufrufe aus anderen
/// Threads laufen dort, und der Hauptthread sperrt ebenfalls.
static STATE: Mutex<State> = Mutex::new(State { life: Life::new(), placement: Placement::new() });

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap()
}

/// Programmstart: normal das Fenster zeigen; per Autostart im Tray bleiben, ganz ohne Fenster (ohne Tray
/// minimiert zeigen).
pub fn at_start(app: &AppHandle) -> Result<()> {
    let autostarted = std::env::args().any(|arg| arg == autostart::ARG);
    let start = window::at_start(autostarted, tray::available());
    if start == AtStart::Tray {
        info!("autostart: Qross is running in the tray, the window opens on demand");
        return Ok(());
    }
    // Anlegen wie beim Zeigen, aber gleich hier im Hauptthread, wie Tauri die Fenster aus tauri.conf.json
    // beim Start selbst anlegt
    state().life.show(false);
    let window = create(app).inspect_err(|_| state().life.create_failed())?;
    state().life.created();
    if start == AtStart::Minimized {
        // Erst minimieren, dann zeigen: umgekehrt geht das Minimieren beim frisch gezeigten Fenster verloren.
        let _ = window.minimize();
    }
    let _ = window.show();
    Ok(())
}

/// Fenster zeigen und nach vorn holen; gibt es keins, wird es angelegt.
pub fn show(app: &AppHandle) {
    let window = app.get_webview_window(LABEL);
    let decision = state().life.show(window.is_some());
    match (decision, window) {
        (Show::Reveal, Some(window)) => reveal(app, &window),
        (Show::Create, _) => spawn_create(app),
        _ => {}
    }
}

/// Fenster zeigen (bei Bedarf anlegen), und die Seite führt `event` aus, sobald sie bereit ist.
pub fn open(app: &AppHandle, event: &'static str, payload: impl Serialize) {
    action(app, event, payload);
    show(app);
}

/// Auftrag an die Seite, sobald sie geladen und das Fenster zu sehen ist; öffnet das Fenster nicht.
pub fn action(app: &AppHandle, event: &'static str, payload: impl Serialize) {
    let payload = serde_json::to_value(payload).unwrap_or_default();
    let actions = state().life.action(UiAction { event, payload });
    deliver(app, actions);
}

fn deliver(app: &AppHandle, actions: Vec<UiAction>) {
    for action in actions {
        if let Err(e) = app.emit(action.event, action.payload) {
            warn!("failed to send '{}' to the window: {e}", action.event);
        }
    }
}

/// Die Seite ist geladen und hört auf ihre Ereignisse (Ende von `init` in main.js): Was auf sie wartet,
/// kommt jetzt.
#[tauri::command]
pub fn ui_ready(app: AppHandle) {
    let actions = state().life.page_ready();
    deliver(&app, actions);
}

/// Die Seite lädt (neu, etwa nach einem Sprachwechsel): Aufträge warten auf ihr nächstes `ui_ready`.
pub fn page_load(webview: &Webview, payload: &PageLoadPayload<'_>) {
    if webview.label() == LABEL && payload.event() == PageLoadEvent::Started {
        state().life.page_loading();
    }
}

fn reveal(app: &AppHandle, window: &WebviewWindow) {
    // Die Seite zuerst, sonst blitzt das Fenster kurz leer auf
    page_shown(window, true);
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    tray::window_shown(app);
    let actions = state().life.deliverable();
    deliver(app, actions);
}

/// Legt das Fenster in einem eigenen Thread an und zeigt es. Nicht im Hauptthread: Dorthin kommt das
/// Zeigen auch beim zweiten Programmstart, unter Windows mitten in der Nachricht des anderen Prozesses,
/// und darin startet WebView2 nicht. Aus anderen Threads legt Tauri Fenster über die Ereignisschleife an.
fn spawn_create(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new().name("qross-window".into()).spawn(move || {
        let result = create(&app).or_else(|e| {
            warn!("failed to open the window, trying again: {e:#}");
            std::thread::sleep(RETRY_AFTER);
            create(&app)
        });
        match result {
            Ok(window) => {
                state().life.created();
                reveal(&app, &window);
            }
            Err(e) => {
                warn!("failed to open the window: {e:#}");
                state().life.create_failed();
            }
        }
    });
    if let Err(e) = spawned {
        warn!("failed to open the window: {e}");
        state().life.create_failed();
    }
}

/// Neues, noch unsichtbares Fenster aus tauri.conf.json, mit eigener Titelleiste und der Lage von zuletzt
fn create(app: &AppHandle) -> Result<WebviewWindow> {
    info!("opening the window");
    let config = app.config().app.windows.iter().find(|w| w.label == LABEL).context("no main window in tauri.conf.json")?;
    // So legt Tauri die Seite eines neuen Fensters an
    #[cfg(windows)]
    PAGE_SHOWN.store(true, Ordering::Relaxed);
    let window = WebviewWindowBuilder::from_config(app, config)?.build()?;
    // Eigene Titelleiste: Die Kopfzeile der Seite zieht das Fenster und trägt Minimieren, Maximieren und
    // Schließen. Unter macOS bleiben die Ampel-Knöpfe des Systems und liegen in ihr (titleBarStyle
    // „Overlay“ in tauri.conf.json).
    #[cfg(not(target_os = "macos"))]
    if let Err(e) = window.set_decorations(false) {
        warn!("failed to turn off the window frame: {e}");
    }
    restore(&window);
    Ok(window)
}

/// Schließen hat das Fenster in den Tray versteckt: nach [`CLOSE_AFTER`] ganz zerstören, wenn es so bleibt.
pub fn hidden(app: &AppHandle) {
    let Some(ticket) = state().life.hidden() else { return };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(CLOSE_AFTER).await;
        // Im Hauptthread lesen sich Lage und Größe ohne Umweg
        let handle = app.clone();
        if let Err(e) = app.run_on_main_thread(move || close_if_due(&handle, ticket)) {
            warn!("failed to close the hidden window: {e}");
        }
    });
}

fn close_if_due(app: &AppHandle, ticket: u64) {
    let window = app.get_webview_window(LABEL);
    if window.as_ref().is_some_and(|w| w.is_visible().unwrap_or(false)) {
        // Auf anderem Weg wieder zu sehen (etwa über das Fenster-Menü von macOS): bleibt
        state().life.show(true);
        return;
    }
    if !state().life.close_due(ticket) {
        return;
    }
    let Some(window) = window else {
        // Schon weg
        destroyed(app);
        return;
    };
    let page: &Webview = window.as_ref();
    track(&page.window());
    info!("closing the window after {} s in the tray to free the web view's memory", CLOSE_AFTER.as_secs());
    if let Err(e) = window.destroy() {
        warn!("failed to close the hidden window: {e}");
        state().life.close_failed();
    }
}

/// Das Fenster ist weg (Ereignis `Destroyed`). Wurde es inzwischen wieder verlangt, neu anlegen.
pub fn destroyed(app: &AppHandle) {
    let reopen = state().life.destroyed();
    if reopen {
        show(app);
    }
}

/// Unter macOS liegen alle Bildschirme in Punkten nebeneinander, unter Windows und Linux in Pixeln (Windows
/// auch bei verschiedener Skalierung je Bildschirm). In diesen Einheiten merkt sich Qross die Lage.
const POINTS: bool = cfg!(target_os = "macos");

fn desktop(position: PhysicalPosition<i32>, size: PhysicalSize<u32>, scale: f64) -> Bounds {
    let unit = |value: f64| if POINTS { value / scale } else { value };
    Bounds {
        x: unit(position.x.into()).round() as i32,
        y: unit(position.y.into()).round() as i32,
        width: unit(size.width.into()).round() as u32,
        height: unit(size.height.into()).round() as u32,
    }
}

/// Nach Verschieben oder Größenänderung und vor dem Zerstören: Lage fürs Neuanlegen merken
pub fn track(window: &Window) {
    let Ok(minimized) = window.is_minimized() else { return };
    let maximized = window.is_maximized().unwrap_or(false);
    let bounds = match (window.outer_position(), window.inner_size(), window.scale_factor()) {
        (Ok(position), Ok(size), Ok(scale)) => Some(desktop(position, size, scale)),
        _ => None,
    };
    state().placement.track(bounds, maximized, minimized);
}

/// Lage, Größe und Maximiert-Zustand von zuletzt; beim ersten Mal bleibt das Fenster zentriert.
fn restore(window: &WebviewWindow) {
    let screens: Vec<Bounds> = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| desktop(monitor.work_area().position, monitor.work_area().size, monitor.scale_factor()))
        .collect();
    let restore = state().placement.restore(&screens);
    if let Some(Bounds { x, y, width, height }) = restore.bounds {
        let (position, size): (Position, Size) = if POINTS {
            (LogicalPosition::new(x, y).into(), LogicalSize::new(width, height).into())
        } else {
            (PhysicalPosition::new(x, y).into(), PhysicalSize::new(width, height).into())
        };
        // Erst die Lage, dann die Größe: Auf einen Bildschirm mit anderer Skalierung verschoben, passt
        // Windows die Größe an.
        if let Err(e) = window.set_position(position).and_then(|()| window.set_size(size)) {
            warn!("failed to restore the window's position: {e}");
        }
    }
    if restore.maximized {
        if let Err(e) = window.maximize() {
            warn!("failed to maximize the window: {e}");
        }
    }
}

/// Zeigt die Seite gerade etwas? Nur Wechsel gehen an WebView2 (Resized kommt beim Ziehen oft).
#[cfg(windows)]
static PAGE_SHOWN: AtomicBool = AtomicBool::new(true);

/// Sagt der Seite, ob das Fenster zu sehen ist. WebView2 merkt Verstecken (Tray) und Minimieren nicht
/// von selbst und zeichnet weiter, jede laufende Animation (pulsierender Punkt, Verbindungslinien) mit
/// 60 Bildern pro Sekunde. Versteckt ruht die Seite: Ereignisse kommen weiter an, gezeichnet wird erst
/// wieder beim Zeigen. macOS und Linux erkennen das selbst.
fn page_shown(window: &WebviewWindow, shown: bool) {
    #[cfg(windows)]
    if PAGE_SHOWN.swap(shown, Ordering::Relaxed) != shown {
        let page: &Webview = window.as_ref();
        if let Err(e) = if shown { page.show() } else { page.hide() } {
            warn!("failed to {} the page: {e}", if shown { "show" } else { "hide" });
        }
    }
    #[cfg(not(windows))]
    let _ = (window, shown);
}

/// Nach Verstecken, Minimieren, Wiederherstellen oder Fokus: Zeigt das Fenster gerade etwas?
pub fn sync_page(app: &AppHandle) {
    #[cfg(windows)]
    if let Some(window) = app.get_webview_window(LABEL) {
        let shown = window.is_visible().unwrap_or(true) && !window.is_minimized().unwrap_or(false);
        page_shown(&window, shown);
    }
    #[cfg(not(windows))]
    let _ = app;
}
