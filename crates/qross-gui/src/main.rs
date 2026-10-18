//! Qross-Oberfläche: Server/Client im selben Prozess, Fenster als Web-Frontend (Tauri 2).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod logs;
mod runtime;
mod tray;

use std::path::PathBuf;
use std::sync::Arc;

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
        logs: logs.snapshot(),
    })
}

#[tauri::command]
fn preview_layout(settings: Settings) -> LayoutPreview {
    let s = &settings.server;
    LayoutPreview { toml: s.to_toml(), error: s.validate().err(), isolated: s.isolated() }
}

/// Speichert die Einstellungen. Ändert sich das Layout eines laufenden Servers, startet er neu.
#[tauri::command]
async fn save_settings(rt: Rt<'_>, settings: Settings) -> Result<(), String> {
    let rt = rt.inner().clone();
    let old = rt.settings.lock().unwrap().clone();
    if settings == old {
        return Ok(());
    }
    settings.save(&rt.data_dir).map_err(|e| format!("{e:#}"))?;
    if settings.server.validate().is_ok() {
        if let Err(e) = std::fs::write(rt.data_dir.join(EXPORT_FILE), settings.server.to_toml()) {
            warn!("{EXPORT_FILE} nicht geschrieben: {e}");
        }
    }
    *rt.settings.lock().unwrap() = settings.clone();

    let status = rt.status();
    let server_changed = settings.server.needs_restart(&old.server);
    match status.role {
        Some(role) if role != settings.role => rt.stop().await,
        Some(Role::Server) if server_changed && settings.server.validate().is_ok() => {
            info!("Einstellungen geändert – Server startet neu");
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

/// Sucht Qross-Server im LAN (ca. 1,2 s).
#[tauri::command]
async fn discover_servers(rt: Rt<'_>) -> Result<Vec<FoundServer>, String> {
    rt.discover_servers(std::time::Duration::from_millis(1200)).await.map_err(|e| format!("{e:#}"))
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
            trust_server,
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
