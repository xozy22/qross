//! Tray-Icon: zeigt den Zustand als farbigen Punkt und im Tooltip; im Menü auch die Profile.
//! Laufen Dateiübertragungen, steht stattdessen ihr Gesamtfortschritt als Balken im Symbol und im
//! Tooltip, dazu als Balken in der Taskleiste (Windows) bzw. im Dock (macOS).
//!
//! Nicht jedes System zeigt Tray-Icons an (z. B. GNOME ohne AppIndicator-Erweiterung). Dann
//! beendet Schließen die App, statt das Fenster unerreichbar zu verstecken.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use qross_gui::i18n::{self, Lang};
use qross_gui::profiles::{ProfileEdit, Profiles};
use qross_gui::transfers::{shorten, Overall};
use qross_gui::tray_icon::{bar_fill, draw_bar, draw_dot};
use qross_gui::Role;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::{ProgressBarState, ProgressBarStatus};
use tauri::{AppHandle, Emitter, Manager, Wry};
use tracing::{debug, info, warn};

use crate::runtime::{Phase, Runtime, Status};

const TRAY_ID: &str = "main";

/// Einträge des Menüs, deren Text sich ändert (Starten/Stoppen, Sprache)
struct Items {
    show: MenuItem<Wry>,
    news: MenuItem<Wry>,
    settings: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    profiles: Submenu<Wry>,
}

static ITEMS: OnceLock<Items> = OnceLock::new();
/// Name des aktiven Profils für den Tooltip, sofern es mehrere Profile gibt
static PROFILE_NAME: Mutex<Option<String>> = Mutex::new(None);
static AVAILABLE: AtomicBool = AtomicBool::new(false);
const PROFILE_PREFIX: &str = "profile:";

/// Texte des Menüs in einer Sprache
struct MenuTexts {
    show: &'static str,
    news: &'static str,
    settings: &'static str,
    start: &'static str,
    stop: &'static str,
    quit: &'static str,
    /// Untermenü „Profil: Büro“
    profile: &'static str,
    auto: &'static str,
    manage: &'static str,
}

fn menu_texts(lang: Lang) -> MenuTexts {
    match lang {
        Lang::De => MenuTexts {
            show: "Qross öffnen",
            news: "Neuigkeiten",
            settings: "Einstellungen",
            start: "Starten",
            stop: "Stoppen",
            quit: "Beenden",
            profile: "Profil",
            auto: "Automatisch nach Netzwerk",
            manage: "Profile verwalten …",
        },
        Lang::En => MenuTexts {
            show: "Open Qross",
            news: "What's new",
            settings: "Settings",
            start: "Start",
            stop: "Stop",
            quit: "Quit",
            profile: "Profile",
            auto: "Switch automatically by network",
            manage: "Manage profiles …",
        },
    }
}

/// Ist ein Tray-Icon sichtbar, über das sich das versteckte Fenster zurückholen lässt?
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

/// Legt das Tray-Icon an, sofern das System eines anzeigen kann. Fehler beenden die App nicht.
pub fn init(app: &AppHandle) {
    if !host_present() {
        info!("no tray available; closing the window quits Qross");
        return;
    }
    // Unter Linux wird libappindicator erst jetzt geladen und kann dabei mit einem Panic scheitern.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| create(app))) {
        Ok(Ok(())) => AVAILABLE.store(true, Ordering::Relaxed),
        Ok(Err(e)) => warn!("tray icon unavailable ({e}); closing the window quits Qross"),
        Err(_) => warn!("tray icon unavailable; closing the window quits Qross"),
    }
}

fn host_present() -> bool {
    // Zum Ausprobieren des Verhaltens ohne Tray (nur Debug-Builds)
    if cfg!(debug_assertions) && std::env::var_os("QROSS_NO_TRAY").is_some() {
        return false;
    }
    platform_host_present()
}

/// Linux: Tray-Icons laufen über StatusNotifierItem. Ohne `org.kde.StatusNotifierWatcher` auf dem
/// Session-Bus zeigt niemand das Icon an. Lässt sich das nicht prüfen, gilt: kein Tray.
#[cfg(target_os = "linux")]
fn platform_host_present() -> bool {
    use std::process::Command;

    let gdbus = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.DBus",
            "--object-path",
            "/org/freedesktop/DBus",
            "--method",
            "org.freedesktop.DBus.NameHasOwner",
            "org.kde.StatusNotifierWatcher",
        ])
        .output();
    if let Ok(out) = gdbus {
        if out.status.success() {
            return String::from_utf8_lossy(&out.stdout).contains("true");
        }
    }
    let dbus_send = Command::new("dbus-send")
        .args([
            "--session",
            "--dest=org.freedesktop.DBus",
            "--type=method_call",
            "--print-reply",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.NameHasOwner",
            "string:org.kde.StatusNotifierWatcher",
        ])
        .output();
    match dbus_send {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).contains("boolean true"),
        _ => false,
    }
}

/// Windows und macOS haben immer einen Infobereich bzw. eine Menüleiste.
#[cfg(not(target_os = "linux"))]
fn platform_host_present() -> bool {
    true
}

fn create(app: &AppHandle) -> tauri::Result<()> {
    let lang = i18n::lang();
    let texts = menu_texts(lang);
    let show = MenuItem::with_id(app, "show", texts.show, true, None::<&str>)?;
    let news = MenuItem::with_id(app, "news", texts.news, true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", texts.settings, true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", texts.start, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", texts.quit, true, None::<&str>)?;
    let profiles = Submenu::with_id(app, "profiles", texts.profile, true)?;
    let current = app.state::<Arc<Runtime>>().profiles();
    fill_profiles(app, &profiles, &current, lang)?;
    set_profile_name(&current);
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&show, &news, &settings, &profiles, &toggle, &separator, &quit])?;
    let _ = ITEMS.set(Items { show, news, settings, toggle, quit, profiles });

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(None, None))
        .tooltip(tooltip(&Status::default(), None, None, lang))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => crate::show_main(app),
            "news" => {
                crate::show_main(app);
                let _ = app.emit("show-news", ());
            }
            "settings" => {
                crate::show_main(app);
                let _ = app.emit("show-settings", ());
            }
            "toggle" => {
                let app = app.clone();
                let rt = app.state::<Arc<Runtime>>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    if rt.status().running() {
                        rt.stop().await;
                    } else if !matches!(rt.start().await, Ok(None)) {
                        // Fehler oder unbekannter Server: im Fenster zeigen
                        crate::show_main(&app);
                        let _ = app.emit("start-from-tray", ());
                    }
                });
            }
            "quit" => crate::quit(app),
            "profiles-manage" => {
                crate::show_main(app);
                let _ = app.emit("show-settings", "set-profiles");
            }
            "profiles-auto" => {
                let rt = app.state::<Arc<Runtime>>().inner().clone();
                let enabled = !rt.profiles().auto;
                if let Err(e) = rt.edit_profiles(ProfileEdit::SetAutomatic { enabled }) {
                    warn!("failed to toggle automatic profile selection: {e}");
                } else if enabled {
                    tauri::async_runtime::spawn(async move { crate::netwatch::check_now(&rt).await });
                }
            }
            id if id.starts_with(PROFILE_PREFIX) => {
                let id = id[PROFILE_PREFIX.len()..].to_owned();
                let app = app.clone();
                let rt = app.state::<Arc<Runtime>>().inner().clone();
                if changes_running_role(&rt, &id) {
                    // Server schaltet sich aus bzw. Client trennt: erst im Fenster nachfragen. Das
                    // Häkchen bleibt bis dahin beim aktiven Profil.
                    update_profiles(&app, &rt.profiles());
                    crate::show_main(&app);
                    let _ = app.emit("confirm-profile", id);
                    return;
                }
                tauri::async_runtime::spawn(async move {
                    if !matches!(rt.activate_profile(&id).await, Ok(None)) {
                        // Fehler oder unbekannter Server: im Fenster zeigen
                        crate::show_main(&app);
                        let _ = app.emit("start-from-tray", ());
                    }
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Würde das Profil die Rolle wechseln, während der Server läuft oder der Client verbunden ist?
fn changes_running_role(rt: &Runtime, id: &str) -> bool {
    let status = rt.status();
    let running = match status.role {
        Some(Role::Server) => true,
        Some(Role::Client) => status.phase == Phase::Connected,
        None => false,
    };
    let profiles = rt.profiles();
    running
        && id != profiles.active
        && profiles.list.iter().find(|p| p.id == id).is_some_and(|p| Some(p.place.role) != status.role)
}

/// Profile als Häkchen-Einträge, dazu die Automatik und der Weg in die Einstellungen
fn fill_profiles(app: &AppHandle, menu: &Submenu<Wry>, profiles: &Profiles, lang: Lang) -> tauri::Result<()> {
    let texts = menu_texts(lang);
    while menu.remove_at(0)?.is_some() {}
    // „&“ markiert sonst ein Tastenkürzel im Menü
    let text = |name: &str| name.replace('&', "&&");
    for profile in &profiles.list {
        let id = format!("{PROFILE_PREFIX}{}", profile.id);
        let active = profile.id == profiles.active;
        menu.append(&CheckMenuItem::with_id(app, id, text(&profile.name), true, active, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&CheckMenuItem::with_id(app, "profiles-auto", texts.auto, true, profiles.auto, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "profiles-manage", texts.manage, true, None::<&str>)?)?;
    let active = profiles.active_profile().map(|p| p.name.as_str()).unwrap_or("–");
    menu.set_text(format!("{}: {}", texts.profile, text(active)))
}

fn set_profile_name(profiles: &Profiles) {
    let name = profiles.active_profile().filter(|_| profiles.list.len() > 1).map(|p| p.name.clone());
    *PROFILE_NAME.lock().unwrap() = name;
}

/// Profile haben sich geändert (anderes aktiv, umbenannt, Automatik …)
pub fn update_profiles(app: &AppHandle, profiles: &Profiles) {
    set_profile_name(profiles);
    if let Some(items) = ITEMS.get() {
        if let Err(e) = fill_profiles(app, &items.profiles, profiles, i18n::lang()) {
            warn!("failed to update profiles in the tray menu: {e}");
        }
    }
    update(app);
}

/// Die Sprache hat sich geändert: Menü und Tooltip neu beschriften
pub fn language_changed(app: &AppHandle) {
    let Some(items) = ITEMS.get() else { return };
    if let Err(e) = relabel(app, items, i18n::lang()) {
        warn!("failed to relabel the tray menu: {e}");
    }
    // Beim nächsten Zeigen alles neu setzen, auch Tooltip und „Starten“/„Stoppen“
    *SHOWN.lock().unwrap() = None;
    update(app);
}

fn relabel(app: &AppHandle, items: &Items, lang: Lang) -> tauri::Result<()> {
    let texts = menu_texts(lang);
    items.show.set_text(texts.show)?;
    items.news.set_text(texts.news)?;
    items.settings.set_text(texts.settings)?;
    items.quit.set_text(texts.quit)?;
    let profiles = app.state::<Arc<Runtime>>().profiles();
    fill_profiles(app, &items.profiles, &profiles, lang)
}

/// Was der Tray gerade zeigt. Der Status ändert sich bei jedem Wechsel des Cursors, Punkt, Tooltip
/// und Menü dagegen selten; ans System geht nur, was sich geändert hat.
struct Shown {
    dot: Option<[u8; 3]>,
    /// Gefüllte Spalten des Fortschrittsbalkens, solange Dateien übertragen werden
    bar: Option<u8>,
    tooltip: String,
    running: bool,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);
/// Gesamtstand der laufenden Dateiübertragungen (siehe [`set_transfers`])
static TRANSFERS: Mutex<Option<Overall>> = Mutex::new(None);
/// Prozent im Balken der Taskleiste bzw. des Docks; `None`: beim nächsten Mal neu setzen
static TASKBAR: Mutex<Option<Option<u8>>> = Mutex::new(Some(None));

/// Zeigt den Status im Tray, auf dem Hauptthread und mit dem Stand von dann: Wer den Status meldet,
/// wartet nicht auf das System, und dicht aufeinander folgende Meldungen enden beim neuesten Stand.
pub fn update(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show(&handle));
}

/// Laufende Dateiübertragungen, zusammengefasst; `None`, wenn keine (mehr) läuft.
pub fn set_transfers(app: &AppHandle, overall: Option<Overall>) {
    {
        let mut current = TRANSFERS.lock().unwrap();
        if *current == overall {
            return;
        }
        *current = overall;
    }
    update(app);
}

/// Das Fenster ist (wieder) zu sehen: Windows legt seine Schaltfläche in der Taskleiste dabei neu
/// an, ohne den Balken einer laufenden Übertragung.
pub fn window_shown(app: &AppHandle) {
    *TASKBAR.lock().unwrap() = None;
    update(app);
}

fn show(app: &AppHandle) {
    let transfer = TRANSFERS.lock().unwrap().clone();
    show_taskbar(app, transfer.as_ref().map(|t| t.percent));
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let Some(rt) = app.try_state::<Arc<Runtime>>() else { return };
    let status = rt.status();
    let profile = PROFILE_NAME.lock().unwrap().clone();
    let lang = i18n::lang();
    let next = Shown {
        dot: dot(&status),
        bar: transfer.as_ref().map(|t| bar_fill(t.percent)),
        tooltip: tooltip(&status, profile.as_deref(), transfer.as_ref().map(|t| t.text.as_str()), lang),
        running: status.running(),
    };
    let mut shown = SHOWN.lock().unwrap();
    let before = shown.as_ref();
    if before.is_none_or(|s| s.dot != next.dot || s.bar != next.bar) {
        let _ = tray.set_icon(Some(icon(next.dot, next.bar)));
    }
    if before.is_none_or(|s| s.tooltip != next.tooltip) {
        let _ = tray.set_tooltip(Some(&next.tooltip));
    }
    if before.is_none_or(|s| s.running != next.running) {
        if let Some(items) = ITEMS.get() {
            let texts = menu_texts(lang);
            let _ = items.toggle.set_text(if next.running { texts.stop } else { texts.start });
        }
    }
    *shown = Some(next);
}

/// Balken in der Taskleiste (Windows) bzw. im Dock (macOS, Linux mit libunity); `None` nimmt ihn weg.
/// Unter Windows nur zu sehen, solange das Fenster eine Schaltfläche hat, also nicht im Tray.
fn show_taskbar(app: &AppHandle, percent: Option<u8>) {
    let mut shown = TASKBAR.lock().unwrap();
    if *shown == Some(percent) {
        return;
    }
    let Some(window) = app.get_webview_window("main") else { return };
    let state = match percent {
        Some(p) => ProgressBarState { status: Some(ProgressBarStatus::Normal), progress: Some(p.into()) },
        None => ProgressBarState { status: Some(ProgressBarStatus::None), progress: None },
    };
    if let Err(e) = window.set_progress_bar(state) {
        debug!("failed to set taskbar progress: {e}");
    }
    *shown = Some(percent);
}

/// Windows nimmt höchstens 127 Zeichen
const TOOLTIP_CHARS: usize = 120;

/// `transfer`: Gesamtstand laufender Übertragungen, steht dann statt der Verbindung
fn tooltip(status: &Status, profile: Option<&str>, transfer: Option<&str>, lang: Lang) -> String {
    let profile = profile.map(|name| format!(" ({name})")).unwrap_or_default();
    let role = match status.role {
        Some(Role::Server) => "Server",
        Some(Role::Client) => "Client",
        None => {
            let state = match (status.phase, lang) {
                (Phase::Error, Lang::De) => "Fehler",
                (Phase::Error, Lang::En) => "error",
                (_, Lang::De) => "gestoppt",
                (_, Lang::En) => "stopped",
            };
            return format!("Qross{profile} – {state}");
        }
    };
    let server = status.server_name.as_deref();
    let retrying = status.retry_reason.is_some();
    let detail = match (transfer, status.role, status.phase, lang) {
        (Some(transfer), ..) => transfer.to_owned(),
        (_, Some(Role::Server), Phase::Listening, _) => clients_text(status.clients.len(), lang),
        (_, _, Phase::Connected, Lang::De) => format!("verbunden mit {}", server.unwrap_or("Server")),
        (_, _, Phase::Connected, Lang::En) => format!("connected to {}", server.unwrap_or("the server")),
        (_, _, Phase::Connecting, Lang::De) if retrying => "Server nicht erreichbar, versucht es weiter".to_owned(),
        (_, _, Phase::Connecting, Lang::En) if retrying => "server unreachable, still trying".to_owned(),
        (_, _, Phase::Connecting, Lang::De) => "verbindet …".to_owned(),
        (_, _, Phase::Connecting, Lang::En) => "connecting …".to_owned(),
        (_, _, Phase::Starting, Lang::De) => "startet …".to_owned(),
        (_, _, Phase::Starting, Lang::En) => "starting …".to_owned(),
        _ => status.message.clone(),
    };
    let lock = match (&status.secure_input, status.locked, status.game_hold.is_some(), lang) {
        (Some(hint), ..) => qross_gui::secure_input::tray_note(hint, lang),
        (None, true, _, Lang::De) => " · Cursor gesperrt".to_owned(),
        (None, true, _, Lang::En) => " · cursor locked".to_owned(),
        (None, false, true, Lang::De) => " · Spielemodus hält den Cursor".to_owned(),
        (None, false, true, Lang::En) => " · game mode is holding the cursor".to_owned(),
        (None, false, false, _) => String::new(),
    };
    shorten(&format!("Qross {role}{profile} – {detail}{lock}"), TOOLTIP_CHARS)
}

/// „wartet auf Clients“, „1 Client verbunden“, „3 Clients verbunden“
fn clients_text(count: usize, lang: Lang) -> String {
    match (count, lang) {
        (0, Lang::De) => "wartet auf Clients".to_owned(),
        (0, Lang::En) => "waiting for clients".to_owned(),
        (1, Lang::De) => "1 Client verbunden".to_owned(),
        (1, Lang::En) => "1 client connected".to_owned(),
        (n, Lang::De) => format!("{n} Clients verbunden"),
        (n, Lang::En) => format!("{n} clients connected"),
    }
}

/// Farbe des Statuspunkts; gestoppt ohne Punkt
fn dot(status: &Status) -> Option<[u8; 3]> {
    match status.phase {
        Phase::Stopped => None,
        Phase::Error => Some([220, 38, 38]),
        Phase::Starting | Phase::Connecting => Some([234, 179, 8]),
        Phase::Listening if status.clients.is_empty() => Some([234, 179, 8]),
        // Sichere Tastatureingabe: Der Cursor kommt nicht zu den Clients
        Phase::Listening if status.secure_input.is_some() => Some([234, 179, 8]),
        Phase::Listening | Phase::Connected => Some([22, 163, 74]),
    }
}

/// Basis-Icon mit Statuspunkt unten links; laufen Übertragungen, stattdessen mit ihrem Balken.
fn icon(dot: Option<[u8; 3]>, bar: Option<u8>) -> Image<'static> {
    let base = Image::from_bytes(include_bytes!("../icons/32x32.png")).expect("embedded icon is a valid PNG");
    let (w, h) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    match (bar, dot) {
        (Some(filled), _) => draw_bar(&mut rgba, w, h, filled),
        (None, Some(color)) => draw_dot(&mut rgba, w, h, color),
        (None, None) => {}
    }
    Image::new_owned(rgba, w, h)
}
