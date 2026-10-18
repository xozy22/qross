//! Qross-Oberfläche: Server/Client im selben Prozess, Fenster als Web-Frontend (Tauri 2).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod logs;
mod runtime;
mod tray;
mod updates;

use std::path::PathBuf;
use std::sync::Arc;

use qross_gui::changelog::{self, Release};
use qross_gui::lifecycle;
use qross_gui::{Role, Settings, EXPORT_FILE, GRID_COLS, GRID_ROWS};
use qross_platform::{Desktop, Permission, PortalProblem};
use serde::Serialize;
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_opener::OpenerExt;
use tracing::{info, warn};

use crate::logs::{LogBuffer, LogLine};
use crate::runtime::{FoundServer, Runtime, ServerProbe, Status};
use crate::updates::{UpdateStatus, Updates};

type Rt<'a> = State<'a, Arc<Runtime>>;

/// Mit diesem Argument startet das System Qross bei der Anmeldung (unsichtbar im Tray).
const AUTOSTART_ARG: &str = "--autostart";
const PROJECT_URL: &str = "https://github.com/xozy22/qross";

/// Rechnername in Kleinbuchstaben (Windows liefert ihn in Großbuchstaben).
pub fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().to_lowercase()
}

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Stoppt Server bzw. Client (gibt Maus und Tastatur frei) und beendet die App.
pub fn quit(app: &AppHandle) {
    let app = app.clone();
    let rt = app.state::<Arc<Runtime>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        rt.stop_for_exit().await;
        app.exit(0);
    });
}

/// Hat das Hauptfenster gerade den Fokus? Dann reicht ein Hinweis in der App.
pub fn window_in_front(app: &AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|w| {
        w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
    })
}

/// Systembenachrichtigung (Windows-Infocenter, macOS-Mitteilungen, Linux über D-Bus).
/// Schlägt sie fehl, bleibt es beim Hinweis in der App.
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    tracing::debug!("Systembenachrichtigung: {title}");
    let result = app.notification().builder().title(title).body(body).show();
    if let Err(e) = result {
        warn!("Systembenachrichtigung nicht möglich: {e}");
    }
}

#[derive(Serialize)]
struct Snapshot {
    settings: Settings,
    status: Status,
    fingerprint: String,
    hostname: String,
    /// IPv4-Adresse im lokalen Netz, unter der Clients den Server erreichen
    local_ip: Option<String>,
    data_dir: String,
    grid: (i32, i32),
    /// Betriebssystem dieses Rechners (`windows`, `macos`, `linux`)
    platform: &'static str,
    /// Grafische Sitzung (`windows`, `macos`, `x11`, `wayland`, `other`)
    desktop: &'static str,
    /// Version dieser App (für „Neuigkeiten“)
    version: &'static str,
    logs: Vec<LogLine>,
}

#[derive(Serialize)]
struct LayoutPreview {
    toml: String,
    error: Option<String>,
    isolated: Vec<String>,
}

#[tauri::command]
fn get_state(rt: Rt<'_>, logs: State<'_, Arc<LogBuffer>>) -> Result<Snapshot, String> {
    let fingerprint = rt.identity().map_err(|e| format!("{e:#}"))?.fingerprint().to_string();
    Ok(Snapshot {
        settings: rt.settings.lock().unwrap().clone(),
        status: rt.status(),
        fingerprint,
        hostname: hostname(),
        local_ip: local_ip(),
        data_dir: rt.data_dir.display().to_string(),
        grid: (GRID_COLS, GRID_ROWS),
        platform: runtime::platform_name(qross_proto::Platform::current()),
        desktop: match qross_platform::desktop() {
            Desktop::Windows => "windows",
            Desktop::MacOs => "macos",
            Desktop::X11 => "x11",
            Desktop::Wayland => "wayland",
            Desktop::Other => "other",
        },
        version: env!("CARGO_PKG_VERSION"),
        logs: logs.snapshot(),
    })
}

/// Einstellungen, die nicht in `gui.json` stehen, sondern vom System kommen
#[derive(Serialize)]
struct Prefs {
    /// Echter Zustand des Autostarts im System
    autostart: bool,
    /// Nur Release-Builds tragen sich in den Autostart ein
    autostart_editable: bool,
    update: UpdateStatus,
    data_dir: String,
}

#[tauri::command]
fn get_prefs(app: AppHandle, rt: Rt<'_>) -> Prefs {
    Prefs {
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        autostart_editable: !cfg!(debug_assertions),
        update: app.state::<Arc<Updates>>().status(),
        data_dir: rt.data_dir.display().to_string(),
    }
}

/// „Mit dem System starten“; liefert den Zustand danach.
#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    if cfg!(debug_assertions) {
        return Err("Im Entwicklungsbuild trägt sich Qross nicht in den Autostart ein.".into());
    }
    let autolaunch = app.autolaunch();
    let result = if enabled { autolaunch.enable() } else { autolaunch.disable() };
    result.map_err(|e| format!("Autostart nicht geändert: {e}"))?;
    info!("Autostart {}", if enabled { "eingeschaltet" } else { "ausgeschaltet" });
    autolaunch.is_enabled().map_err(|e| e.to_string())
}

/// „Jetzt nach Updates suchen“; das Ergebnis kommt als Ereignis `update-status`.
#[tauri::command]
fn check_updates(app: AppHandle) {
    tauri::async_runtime::spawn(async move { updates::check(&app).await });
}

/// „Jetzt aktualisieren“ für ein angebotenes Update
#[tauri::command]
fn install_update(app: AppHandle) {
    tauri::async_runtime::spawn(async move { updates::install_now(&app).await });
}

#[tauri::command]
fn open_data_dir(app: AppHandle, rt: Rt<'_>) -> Result<(), String> {
    app.opener().open_path(rt.data_dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_project_page(app: AppHandle) -> Result<(), String> {
    app.opener().open_url(PROJECT_URL, None::<&str>).map_err(|e| e.to_string())
}

/// Rechte unter macOS → Datenschutz & Sicherheit; Windows und Linux haben immer alle.
#[derive(Serialize)]
struct Permissions {
    accessibility: bool,
    input_monitoring: bool,
}

#[tauri::command]
fn permissions() -> Permissions {
    Permissions {
        accessibility: qross_platform::has_permission(Permission::Accessibility),
        input_monitoring: qross_platform::has_permission(Permission::InputMonitoring),
    }
}

/// Trägt Qross in die Liste des Rechts ein (macOS fragt beim ersten Mal selbst nach) und öffnet die
/// passende Seite der Systemeinstellungen. `permission`: `accessibility` oder `input_monitoring`.
#[tauri::command]
fn open_permission_settings(app: AppHandle, permission: String) -> Result<(), String> {
    let (permission, pane) = match permission.as_str() {
        "accessibility" => (Permission::Accessibility, "Privacy_Accessibility"),
        "input_monitoring" => (Permission::InputMonitoring, "Privacy_ListenEvent"),
        other => return Err(format!("unbekanntes Recht: {other}")),
    };
    qross_platform::request_permission(permission);
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

/// Wayland: Stand der Desktop-Portale für Hinweise in der Oberfläche
#[derive(Serialize)]
struct PortalInfo {
    /// Versionen der Portal-Schnittstellen, `None`: fehlt
    remote_desktop: Option<u32>,
    input_capture: Option<u32>,
    /// Letztes Problem beim Einspeisen (Client) bzw. Erfassen (Server)
    injector: Option<ProblemInfo>,
    capture: Option<ProblemInfo>,
    /// Freigabe gemerkt: Das System fragt beim nächsten Start nicht erneut
    remembered_injector: bool,
    remembered_capture: bool,
}

#[derive(Serialize)]
struct ProblemInfo {
    /// `missing`, `denied` oder `failed`
    kind: &'static str,
    text: String,
}

impl From<PortalProblem> for ProblemInfo {
    fn from(problem: PortalProblem) -> Self {
        let kind = match &problem {
            PortalProblem::Missing(_) => "missing",
            PortalProblem::Denied => "denied",
            PortalProblem::Failed(_) => "failed",
        };
        Self { kind, text: problem.to_string() }
    }
}

/// `None` außerhalb von Wayland. Fragt beim ersten Mal den Portal-Dienst (bis zu 5 s), deshalb
/// nicht im Hauptthread.
#[tauri::command]
async fn portal_state() -> Option<PortalInfo> {
    let state = tauri::async_runtime::spawn_blocking(qross_platform::portal_state).await.ok().flatten()?;
    Some(PortalInfo {
        remote_desktop: state.remote_desktop,
        input_capture: state.input_capture,
        injector: state.injector.map(ProblemInfo::from),
        capture: state.capture.map(ProblemInfo::from),
        remembered_injector: state.remembered_injector,
        remembered_capture: state.remembered_capture,
    })
}

/// Gemerkte Wayland-Freigaben löschen: Beim nächsten Start fragt das System wieder.
#[tauri::command]
fn forget_portal_permissions() -> Result<(), String> {
    qross_platform::forget_portal_permissions().map_err(|e| format!("{e:#}"))?;
    info!("Gemerkte Wayland-Freigaben gelöscht");
    Ok(())
}

/// Fehlende Rechte gleich beim Start ins Protokoll schreiben, nicht erst beim ersten Ereignis.
fn log_permissions() {
    let missing: Vec<&str> = [(Permission::Accessibility, "Bedienungshilfen"), (Permission::InputMonitoring, "Eingabeüberwachung")]
        .into_iter()
        .filter(|&(permission, _)| !qross_platform::has_permission(permission))
        .map(|(_, name)| name)
        .collect();
    if !missing.is_empty() {
        warn!("macOS erlaubt Qross noch nicht: {}", missing.join(", "));
    }
}

#[tauri::command]
fn preview_layout(settings: Settings) -> LayoutPreview {
    let s = &settings.server;
    LayoutPreview { toml: settings.server_toml(), error: s.validate().err(), isolated: s.isolated() }
}

/// Speichert die Einstellungen. Was ein laufender Server oder Client erst beim Start übernimmt
/// (Layout, Port, Wechsel, Grenze für kopierte Dateien), führt zu einem Neustart.
#[tauri::command]
async fn save_settings(rt: Rt<'_>, settings: Settings) -> Result<(), String> {
    let rt = rt.inner().clone();
    let old = rt.settings.lock().unwrap().clone();
    if settings == old {
        return Ok(());
    }
    settings.save(&rt.data_dir).map_err(|e| format!("{e:#}"))?;
    if settings.server.validate().is_ok() {
        if let Err(e) = std::fs::write(rt.data_dir.join(EXPORT_FILE), settings.server_toml()) {
            warn!("{EXPORT_FILE} nicht geschrieben: {e}");
        }
    }
    *rt.settings.lock().unwrap() = settings.clone();

    let status = rt.status();
    let server_changed = settings.server.needs_restart(&old.server);
    // Kopierte Dateien, vertrauliche Inhalte und Spielemodus gelten für beide Rollen
    let limit_changed = settings.clipboard_files_mb != old.clipboard_files_mb
        || settings.clipboard_share_concealed != old.clipboard_share_concealed
        || settings.game_mode != old.game_mode;
    match status.role {
        Some(role) if role != settings.role => rt.stop().await,
        Some(Role::Server) if (server_changed || limit_changed) && settings.server.validate().is_ok() => {
            info!("Einstellungen geändert – Server startet neu");
            rt.start().await.map_err(|e| format!("{e:#}"))?;
        }
        Some(Role::Client) if limit_changed => {
            info!("Einstellungen geändert – Client verbindet neu");
            rt.start().await.map_err(|e| format!("{e:#}"))?;
        }
        _ => {}
    }
    Ok(())
}

#[tauri::command]
async fn start(rt: Rt<'_>) -> Result<Option<ServerProbe>, String> {
    rt.inner().clone().start().await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn stop(rt: Rt<'_>) -> Result<(), String> {
    rt.stop().await;
    Ok(())
}

#[tauri::command]
fn approve_client(rt: Rt<'_>, name: String, fingerprint: String) -> Result<(), String> {
    rt.approve_client(&name, &fingerprint).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn dismiss_client(rt: Rt<'_>, name: String) {
    rt.dismiss_client(&name);
}

#[tauri::command]
async fn probe_server(rt: Rt<'_>, address: String) -> Result<ServerProbe, String> {
    rt.probe_server(address.trim()).await.map_err(|e| format!("{e:#}"))
}

/// Benennbare Taste für Listen und Aufnahme in der Oberfläche
#[derive(Serialize)]
struct KeyName {
    /// Wie `KeyboardEvent.code`, z. B. „AltLeft“
    name: &'static str,
    /// Deutsche Beschriftung, z. B. „Alt links“
    label: &'static str,
}

/// Alle Tasten, die sich als Fang-Taste oder für die Umbelegung eintragen lassen
#[tauri::command]
fn key_names() -> Vec<KeyName> {
    qross_core::keys::all_keys().map(|(_, name, label)| KeyName { name, label }).collect()
}

/// Neuigkeiten aus dem eingebetteten CHANGELOG.md, neueste Version zuerst.
#[tauri::command]
fn changelog() -> &'static [Release] {
    changelog::releases()
}

/// Sucht Qross-Server im LAN (ca. 1,2 s).
#[tauri::command]
async fn discover_servers(rt: Rt<'_>) -> Result<Vec<FoundServer>, String> {
    rt.discover_servers(std::time::Duration::from_millis(1200)).await.map_err(|e| format!("{e:#}"))
}

/// Nur Debug-Builds: spielt die Meldungen einer Dateiübertragung ab, um Hinweise und
/// Systembenachrichtigungen ohne zweiten Rechner zu prüfen. `kind`: ready, slow, large, failed.
#[tauri::command]
async fn simulate_clipboard_files(rt: Rt<'_>, kind: String) -> Result<(), String> {
    use qross_net::clipfiles::FilesNotice;
    if !cfg!(debug_assertions) {
        return Err("nur in Debug-Builds verfügbar".into());
    }
    let rt = rt.inner().clone();
    let (peer, names) = ("pc".to_owned(), vec!["Bericht.pdf".to_owned()]);
    match kind.as_str() {
        "ready" | "slow" => {
            rt.on_files_notice(FilesNotice::Receiving { peer: peer.clone(), names: names.clone(), bytes: 3_141_592 });
            let wait = if kind == "slow" { 3500 } else { 300 };
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
            rt.on_files_notice(FilesNotice::Ready { peer, names, bytes: 3_141_592 });
        }
        "large" => rt.on_files_notice(FilesNotice::TooLarge { peer, names: vec!["Film.mkv".into()], limit: 1_000_000_000 }),
        "failed" => rt.on_files_notice(FilesNotice::Failed { peer, reason: "Verbindung getrennt".into() }),
        other => return Err(format!("unbekannt: {other}")),
    }
    Ok(())
}

/// „Abbrechen“ einer laufenden Dateiübertragung (`id` aus dem Ereignis `transfer`). `false`, wenn
/// sie inzwischen zu Ende ist.
#[tauri::command]
fn cancel_transfer(rt: Rt<'_>, id: String) -> Result<bool, String> {
    rt.cancel_transfer(&id).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn trust_server(rt: Rt<'_>, address: String, fingerprint: String) -> Result<(), String> {
    rt.trust_server(&address, &fingerprint).map_err(|e| format!("{e:#}"))
}

/// Adresse der Netzwerkkarte mit der Standardroute. `connect` auf einem UDP-Socket sendet nichts.
fn local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then(|| ip.to_string())
}

/// Beim allerersten Start (nur Release-Builds) Autostart einschalten; danach entscheidet der Nutzer.
fn configure_autostart(app: &AppHandle, rt: &Runtime) {
    if cfg!(debug_assertions) || rt.app_state().autostart_configured {
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => info!("Autostart eingerichtet: Qross startet bei der Anmeldung"),
        Err(e) => warn!("Autostart nicht eingerichtet: {e}"),
    }
    rt.update_app_state(|s| s.autostart_configured = true);
}

/// Das Fenster startet unsichtbar (tauri.conf.json). Normal zeigen; beim Autostart im Tray
/// bleiben, ohne Tray minimiert zeigen.
fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else { return };
    let autostarted = std::env::args().any(|arg| arg == AUTOSTART_ARG);
    if !autostarted {
        let _ = window.show();
    } else if tray::available() {
        info!("Autostart: Qross läuft im Tray");
    } else {
        // Erst minimieren, dann zeigen: umgekehrt geht das Minimieren beim frisch gezeigten Fenster verloren.
        let _ = window.minimize();
        let _ = window.show();
    }
}

/// Lief beim letzten Beenden ein Server oder Client, wieder starten (falls gewünscht).
pub fn resume(app: &AppHandle) {
    let rt = app.state::<Arc<Runtime>>().inner().clone();
    let settings = rt.settings.lock().unwrap().clone();
    let Some(role) = lifecycle::resume_role(settings.resume_role, rt.app_state().last_running, settings.role) else {
        return;
    };
    info!("Setze {} fort", if role == Role::Server { "den Server" } else { "den Client" });
    tauri::async_runtime::spawn(async move {
        match rt.start().await {
            Ok(None) => {}
            Ok(Some(_)) => warn!("Fortsetzen: Der Server ist unbekannt und muss erst bestätigt werden"),
            Err(e) => warn!("Fortsetzen fehlgeschlagen: {e:#}"),
        }
    });
}

fn data_dir() -> PathBuf {
    std::env::var_os("QROSS_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|d| d.join("Qross")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn main() {
    let logs = logs::init();
    qross_platform::init_process();

    let data_dir = data_dir();
    // Dort merkt sich Qross unter Wayland die Freigaben der Desktop-Portale
    qross_platform::set_data_dir(&data_dir);
    let settings = match Settings::load(&data_dir) {
        Ok(Some(settings)) => settings,
        Ok(None) => Settings::for_host(&hostname()),
        Err(e) => {
            warn!("{e:#} – verwende Standardeinstellungen");
            Settings::for_host(&hostname())
        }
    };
    info!("Daten in {}", data_dir.display());
    log_permissions();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // Nur eine Qross-Instanz: Ein zweiter Start (z. B. aus dem Startmenü, während Qross per
    // Autostart im Tray läuft) zeigt das vorhandene Fenster. Debug-Builds bleiben mehrfach
    // startbar, für Tests mit Server und Client auf einem Rechner.
    #[cfg(not(debug_assertions))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)));
    }
    #[cfg(debug_assertions)]
    {
        builder = builder.plugin(tauri_plugin_mcp_bridge::init_with_config(
            tauri_plugin_mcp_bridge::Config::new("127.0.0.1"),
        ));
    }
    builder
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_ARG])))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .manage(logs.clone())
        .invoke_handler(tauri::generate_handler![
            get_state,
            preview_layout,
            save_settings,
            start,
            stop,
            approve_client,
            dismiss_client,
            probe_server,
            discover_servers,
            changelog,
            key_names,
            trust_server,
            simulate_clipboard_files,
            cancel_transfer,
            get_prefs,
            set_autostart,
            check_updates,
            install_update,
            open_data_dir,
            open_project_page,
            permissions,
            open_permission_settings,
            portal_state,
            forget_portal_permissions,
        ])
        .setup(move |app| {
            logs.attach(app.handle().clone());
            let rt = Arc::new(Runtime::new(app.handle().clone(), data_dir.clone(), settings.clone()));
            app.manage(rt.clone());
            app.manage(Arc::new(Updates::new(&rt.app_state())));
            tray::init(app.handle());
            configure_autostart(app.handle(), &rt);
            show_window(app.handle());
            resume(app.handle());
            updates::spawn_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if tray::available() {
                    // Mit Tray versteckt Schließen das Fenster nur; beendet wird über das Tray-Menü.
                    let _ = window.hide();
                } else {
                    // Ohne Tray wäre ein verstecktes Fenster nicht mehr erreichbar.
                    quit(window.app_handle());
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Qross-Oberfläche konnte nicht starten");
}
