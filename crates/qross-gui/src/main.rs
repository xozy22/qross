//! Qross-Oberfläche: Server/Client im selben Prozess, Fenster als Web-Frontend (Tauri 2).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod logs;
mod runtime;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

use qross_gui::changelog::{self, Release};
use qross_gui::clipboard_files::FilesMessage;
use qross_gui::{Role, Settings, EXPORT_FILE, GRID_COLS, GRID_ROWS};
use serde::Serialize;
use tauri::{AppHandle, Manager, State, WindowEvent};
use tracing::{info, warn};

use crate::logs::{LogBuffer, LogLine};
use crate::runtime::{FoundServer, Runtime, ServerProbe, Status};

type Rt<'a> = State<'a, Arc<Runtime>>;

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
        rt.stop().await;
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
pub fn notify(app: &AppHandle, message: &FilesMessage) {
    use tauri_plugin_notification::NotificationExt;
    tracing::debug!("Systembenachrichtigung: {}", message.title);
    let result = app
        .notification()
        .builder()
        .title(&message.title)
        .body(message.text.as_deref().unwrap_or_default())
        .show();
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
        version: env!("CARGO_PKG_VERSION"),
        logs: logs.snapshot(),
    })
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
    let limit_changed = settings.clipboard_files_mb != old.clipboard_files_mb;
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
            let wait = if kind == "slow" { 2500 } else { 300 };
            tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
            rt.on_files_notice(FilesNotice::Ready { peer, names, bytes: 3_141_592 });
        }
        "large" => rt.on_files_notice(FilesNotice::TooLarge { peer, names: vec!["Film.mkv".into()], limit: 1_000_000_000 }),
        "failed" => rt.on_files_notice(FilesNotice::Failed { peer, reason: "Verbindung getrennt".into() }),
        other => return Err(format!("unbekannt: {other}")),
    }
    Ok(())
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
    let settings = match Settings::load(&data_dir) {
        Ok(Some(settings)) => settings,
        Ok(None) => Settings::for_host(&hostname()),
        Err(e) => {
            warn!("{e:#} – verwende Standardeinstellungen");
            Settings::for_host(&hostname())
        }
    };
    info!("Daten in {}", data_dir.display());

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    #[cfg(debug_assertions)]
    {
        builder = builder.plugin(tauri_plugin_mcp_bridge::init_with_config(
            tauri_plugin_mcp_bridge::Config::new("127.0.0.1"),
        ));
    }
    builder
        .plugin(tauri_plugin_notification::init())
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
            trust_server,
            simulate_clipboard_files,
        ])
        .setup(move |app| {
            logs.attach(app.handle().clone());
            app.manage(Arc::new(Runtime::new(app.handle().clone(), data_dir.clone(), settings.clone())));
            tray::init(app.handle());
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
