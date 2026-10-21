//! Qross-Oberfläche: Server/Client im selben Prozess, Fenster als Web-Frontend (Tauri 2).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod logs;
mod main_window;
mod netwatch;
mod runtime;
mod tray;
mod updates;

use std::path::PathBuf;
use std::sync::Arc;

use qross_gui::autostart::Autostart;
use qross_gui::changelog::{self, Release};
use qross_gui::i18n::{self, Lang};
use qross_gui::lifecycle;
use qross_gui::profiles::{NetworkMark, ProfileEdit, Profiles};
use qross_gui::{Role, Settings, GRID_COLS, GRID_ROWS};
use qross_platform::{Desktop, Permission, PortalProblem};
use serde::Serialize;
use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;
use tracing::{info, warn};

use crate::logs::{LogBuffer, LogLine};
use crate::runtime::{FoundServer, Runtime, ServerProbe, Status, UsageEvent};
use crate::updates::{UpdateStatus, Updates};

type Rt<'a> = State<'a, Arc<Runtime>>;

/// Mit diesem Argument startet das System Qross bei der Anmeldung (unsichtbar im Tray).
const PROJECT_URL: &str = "https://github.com/xozy22/qross";

/// Hinweis, warum sich der Autostart im Entwicklungsbuild nicht einschalten lässt
fn dev_autostart(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "Im Entwicklungsbuild trägt sich Qross nicht in den Autostart ein.",
        Lang::En => "Development builds of Qross don't add themselves to autostart.",
    }
}

/// Rechnername in Kleinbuchstaben (Windows liefert ihn in Großbuchstaben).
pub fn hostname() -> String {
    gethostname::gethostname().to_string_lossy().to_lowercase()
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

/// Hat das Hauptfenster gerade den Fokus? Dann reicht ein Hinweis in der App. Ohne Fenster (im Tray)
/// nie.
pub fn window_in_front(app: &AppHandle) -> bool {
    app.get_webview_window(qross_gui::window::LABEL).is_some_and(|w| {
        w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
    })
}

/// Systembenachrichtigung (Windows-Infocenter, macOS-Mitteilungen, Linux über D-Bus).
/// Schlägt sie fehl, bleibt es beim Hinweis in der App.
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    tracing::debug!("system notification: {title}");
    let result = app.notification().builder().title(title).body(body).show();
    if let Err(e) = result {
        warn!("system notification failed: {e}");
    }
}

#[derive(Serialize)]
struct Snapshot {
    settings: Settings,
    profiles: Profiles,
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
    /// Sprache des Systems wie „de-DE“ (leer, wenn unbekannt); gilt bei der Einstellung „auto“
    system_language: String,
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
        profiles: rt.profiles(),
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
        system_language: i18n::system_locale().unwrap_or_default(),
        logs: logs.snapshot(),
    })
}

/// Einstellungen, die nicht in `gui.json` stehen, sondern vom System kommen
#[derive(Serialize)]
struct Prefs {
    /// Echter Zustand des Autostarts im System
    autostart: bool,
    /// Nur Release-Builds tragen sich in den Autostart ein, und nur von einem festen Ort aus
    /// (ausschalten geht immer)
    autostart_editable: bool,
    /// Warum sich Qross nicht eintragen kann (Entwicklungsbuild, macOS-DMG, …)
    autostart_note: Option<String>,
    /// Wo man den Eintrag im System findet
    autostart_place: &'static str,
    update: UpdateStatus,
    data_dir: String,
}

#[tauri::command]
fn get_prefs(app: AppHandle, rt: Rt<'_>, autostart: State<'_, Autostart>) -> Prefs {
    let lang = i18n::lang();
    let enabled = autostart.is_enabled();
    let note = if cfg!(debug_assertions) {
        Some(dev_autostart(lang).to_owned())
    } else {
        autostart.unsuitable(lang).map(str::to_owned)
    };
    Prefs {
        autostart: enabled,
        autostart_editable: !cfg!(debug_assertions) && (note.is_none() || enabled),
        autostart_note: note,
        autostart_place: autostart.place(lang),
        update: app.state::<Arc<Updates>>().status(),
        data_dir: rt.data_dir.display().to_string(),
    }
}

/// „Mit dem System starten“; liefert den Zustand danach.
#[tauri::command]
fn set_autostart(rt: Rt<'_>, autostart: State<'_, Autostart>, enabled: bool) -> Result<bool, String> {
    let lang = i18n::lang();
    if cfg!(debug_assertions) {
        return Err(dev_autostart(lang).into());
    }
    autostart.set(enabled).map_err(|e| match lang {
        Lang::De => format!("Autostart nicht geändert: {e:#}"),
        Lang::En => format!("Autostart not changed: {e:#}"),
    })?;
    // Ab jetzt entscheidet der Nutzer; der erste Start schaltet nichts mehr ein
    rt.update_app_state(|s| s.autostart_configured = true);
    Ok(autostart.is_enabled())
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
        other => return Err(format!("unknown permission: {other}")),
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
    info!("forgot the remembered Wayland portal permissions");
    Ok(())
}

/// Fehlende Rechte gleich beim Start ins Protokoll schreiben, nicht erst beim ersten Ereignis.
fn log_permissions() {
    let missing: Vec<&str> = [(Permission::Accessibility, "Accessibility"), (Permission::InputMonitoring, "Input Monitoring")]
        .into_iter()
        .filter(|&(permission, _)| !qross_platform::has_permission(permission))
        .map(|(_, name)| name)
        .collect();
    if !missing.is_empty() {
        warn!("macOS has not yet granted Qross: {}", missing.join(", "));
    }
}

#[tauri::command]
fn preview_layout(settings: Settings) -> LayoutPreview {
    let s = &settings.server;
    LayoutPreview { toml: settings.server_toml(), error: s.validate(i18n::lang()).err(), isolated: s.isolated() }
}

/// Speichert die Einstellungen (samt aktivem Profil). Was ein laufender Server oder Client erst
/// beim Start übernimmt (Layout, Port, Wechsel, Grenze für kopierte Dateien), führt zu einem Neustart.
#[tauri::command]
async fn save_settings(rt: Rt<'_>, settings: Settings) -> Result<(), String> {
    let rt = rt.inner().clone();
    let Some((old, settings)) = rt.store_settings(settings).map_err(|e| format!("{e:#}"))? else {
        return Ok(());
    };
    // Andere Sprache: Tray, Status und alles Folgende gleich in der neuen
    if settings.language != old.language && i18n::apply(&settings.language) {
        info!("language: {:?} (setting '{}')", i18n::lang(), settings.language);
        tray::language_changed(rt.app_handle());
        rt.language_changed();
    }

    let status = rt.status();
    let server_changed = settings.server.needs_restart(&old.server);
    // Rechte je Rechner und den Cursor-Finder übernimmt ein laufender Server ohne Neustart.
    let sharing_changed = settings.server.effective_sharing() != old.server.effective_sharing();
    let finder_changed = settings.cursor_finder != old.cursor_finder;
    // Kopierte Dateien, vertrauliche Inhalte und Spielemodus gelten für beide Rollen
    let limit_changed = settings.clipboard_files_mb != old.clipboard_files_mb
        || settings.clipboard_share_concealed != old.clipboard_share_concealed
        || settings.game_mode != old.game_mode;
    // Den Bildschirmschoner gleicht nur der Server ab
    let saver_changed = settings.screensaver_sync != old.screensaver_sync
        || settings.screensaver_lock_clients != old.screensaver_lock_clients;
    let layout_valid = settings.server.validate(i18n::lang()).is_ok();
    match status.role {
        Some(role) if role != settings.role => rt.stop().await,
        Some(Role::Server) if (server_changed || limit_changed || saver_changed) && layout_valid => {
            info!("settings changed, restarting the server");
            rt.start().await.map_err(|e| format!("{e:#}"))?;
        }
        Some(Role::Server) if sharing_changed || finder_changed => {
            rt.set_sharing(settings.server.effective_sharing()).await;
            rt.set_cursor_finder(settings.cursor_finder).await;
        }
        Some(Role::Client) if limit_changed => {
            info!("settings changed, reconnecting the client");
            rt.start().await.map_err(|e| format!("{e:#}"))?;
        }
        _ => {}
    }
    Ok(())
}

/// „Ausprobieren“: den Cursor-Finder auf diesem Rechner gleich zeigen. `false` unter Wayland.
#[tauri::command]
fn try_cursor_finder(rt: Rt<'_>, ring: bool, name: bool) -> bool {
    rt.try_cursor_finder(qross_proto::CursorFinder { ring, name })
}

/// Wechselt das Profil; ein laufender Server oder Client startet mit dessen Werten neu. Liefert
/// beim Client einen zu bestätigenden Fingerprint, wenn der Server des Profils noch unbekannt ist.
#[tauri::command]
async fn activate_profile(rt: Rt<'_>, id: String) -> Result<Option<ServerProbe>, String> {
    rt.inner().clone().activate_profile(&id).await.map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
struct ProfilesEdited {
    profiles: Profiles,
    /// Hinweis, z. B. dass ein Netzwerk bisher zu einem anderen Profil gehörte
    note: Option<String>,
}

/// Profile anlegen, umbenennen, duplizieren, löschen, Netzwerke zuordnen, Automatik schalten
#[tauri::command]
fn edit_profiles(rt: Rt<'_>, edit: ProfileEdit) -> Result<ProfilesEdited, String> {
    let automatic_on = edit == ProfileEdit::SetAutomatic { enabled: true };
    let note = rt.edit_profiles(edit)?;
    if automatic_on {
        // Eingeschaltet: gleich nach dem aktuellen Netzwerk wählen, nicht erst beim nächsten Wechsel
        let rt = rt.inner().clone();
        tauri::async_runtime::spawn(async move { netwatch::check_now(&rt).await });
    }
    Ok(ProfilesEdited { profiles: rt.profiles(), note })
}

/// Netzwerke, in denen dieser Rechner gerade ist (bevorzugtes zuerst), für „Aktuelles Netzwerk übernehmen“
#[tauri::command]
async fn current_networks() -> Vec<NetworkMark> {
    netwatch::networks(true).await
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
    /// Beschriftung in der Sprache der Oberfläche, z. B. „Alt links“ bzw. „Left Alt“
    label: &'static str,
}

/// Alle Tasten, die sich als Fang-Taste oder für die Umbelegung eintragen lassen
#[tauri::command]
fn key_names() -> Vec<KeyName> {
    qross_core::keys::all_keys_in(i18n::lang()).map(|(_, name, label)| KeyName { name, label }).collect()
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
        return Err("only available in debug builds".into());
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
        "failed" => rt.on_files_notice(FilesNotice::Failed { peer, reason: "connection lost".into() }),
        other => return Err(format!("unknown kind: {other}")),
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

/// CPU und Arbeitsspeicher von Qross auf diesem Rechner, samt Web-Oberfläche. Das Fenster fragt alle 2 s,
/// solange es die Werte zeigt; die erste Abfrage nach einer Pause dauert gut eine halbe Sekunde.
#[tauri::command]
async fn local_usage(rt: Rt<'_>) -> Result<UsageEvent, String> {
    Ok(rt.local_usage().await)
}

/// Server: Wessen CPU und Arbeitsspeicher das Fenster zeigt (`None`: niemandes). Der Client meldet sie,
/// solange, etwa alle 2 s (Ereignis `usage`).
#[tauri::command]
async fn watch_usage(rt: Rt<'_>, name: Option<String>) -> Result<(), String> {
    rt.watch_usage(name).await;
    Ok(())
}

/// Adresse der Netzwerkkarte mit der Standardroute. `connect` auf einem UDP-Socket sendet nichts.
fn local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then(|| ip.to_string())
}

/// Nur Release-Builds: Beim ersten Start von einem festen Ort den Autostart einschalten, danach
/// entscheidet der Nutzer. Ein vorhandener Eintrag folgt dem Programm, wenn es verschoben wurde.
fn configure_autostart(autostart: &Autostart, rt: &Runtime) {
    if cfg!(debug_assertions) {
        return;
    }
    let configured = rt.app_state().autostart_configured;
    if autostart.sync(configured) && !configured {
        rt.update_app_state(|s| s.autostart_configured = true);
    }
}

/// Lief beim letzten Beenden ein Server oder Client, wieder starten (falls gewünscht).
pub fn resume(app: &AppHandle) {
    let rt = app.state::<Arc<Runtime>>().inner().clone();
    let settings = rt.settings.lock().unwrap().clone();
    let Some(role) = lifecycle::resume_role(settings.resume_role, rt.app_state().last_running, settings.role) else {
        return;
    };
    info!("resuming the {}", if role == Role::Server { "server" } else { "client" });
    tauri::async_runtime::spawn(async move {
        match rt.start().await {
            Ok(None) => {}
            Ok(Some(_)) => warn!("resume: the server is unknown and must be confirmed first"),
            Err(e) => warn!("failed to resume: {e:#}"),
        }
    });
}

/// Lädt die Profile und führt sie mit den Einstellungen zusammen; beim ersten Start mit Profilen
/// werden die bisherigen Einstellungen zum Profil „Standard“. `loaded`: `gui.json` war lesbar.
fn load_profiles(data_dir: &std::path::Path, settings: &mut Settings, loaded: bool) -> Profiles {
    let mut profiles = match Profiles::load(data_dir) {
        Ok(profiles) => profiles.unwrap_or_default(),
        Err(e) => {
            // Nicht überschreiben: Vielleicht lässt sich die Datei von Hand retten.
            let broken = data_dir.join(format!("{}.defekt", qross_gui::profiles::PROFILES_FILE));
            warn!("{e:#}; starting with new profiles, the old file was moved to {}", broken.display());
            let _ = std::fs::rename(data_dir.join(qross_gui::profiles::PROFILES_FILE), &broken);
            Profiles::default()
        }
    };
    if !loaded {
        profiles.restore(settings);
    }
    if profiles.attach(settings, i18n::lang()) {
        let saved = profiles.save(data_dir).and_then(|()| settings.save(data_dir));
        if let Err(e) = saved {
            warn!("failed to save profiles: {e:#}");
        }
    }
    let active = profiles.active_profile().map(|p| p.name.as_str()).unwrap_or_default();
    info!("profile '{active}' ({} profiles)", profiles.list.len());
    profiles
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
    let (mut settings, loaded) = match Settings::load(&data_dir) {
        Ok(Some(settings)) => (settings, true),
        Ok(None) => (Settings::for_host(&hostname()), false),
        Err(e) => {
            warn!("{e:#}; using default settings");
            (Settings::for_host(&hostname()), false)
        }
    };
    // Vor Profilen, Tray und erstem Status: alles für Menschen gleich in der richtigen Sprache
    let system = i18n::system_locale();
    i18n::set_lang(i18n::resolve(&settings.language, system.as_deref()));
    info!("language: {:?} (setting '{}', system '{}')", i18n::lang(), settings.language, system.unwrap_or_default());
    let profiles = load_profiles(&data_dir, &mut settings, loaded);
    info!("data in {}", data_dir.display());
    log_permissions();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    // Nur eine Qross-Instanz: Ein zweiter Start (z. B. aus dem Startmenü, während Qross per
    // Autostart im Tray läuft) zeigt das Fenster und legt es dafür bei Bedarf an. Debug-Builds
    // bleiben mehrfach startbar, für Tests mit Server und Client auf einem Rechner.
    #[cfg(not(debug_assertions))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| main_window::show(app)));
    }
    #[cfg(debug_assertions)]
    {
        builder = builder.plugin(tauri_plugin_mcp_bridge::init_with_config(
            tauri_plugin_mcp_bridge::Config::new("127.0.0.1"),
        ));
    }
    builder
        .plugin(tauri_plugin_notification::init())
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
            activate_profile,
            edit_profiles,
            current_networks,
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
            try_cursor_finder,
            local_usage,
            watch_usage,
            main_window::ui_ready,
        ])
        .on_page_load(main_window::page_load)
        .setup(move |app| {
            logs.attach(app.handle().clone());
            let rt = Arc::new(Runtime::new(app.handle().clone(), data_dir.clone(), settings.clone(), profiles.clone()));
            app.manage(rt.clone());
            app.manage(Arc::new(Updates::new(&rt.app_state())));
            tray::init(app.handle());
            // Unter Linux zeigt der Eintrag auf die AppImage, nicht auf ihren flüchtigen Einhängepunkt
            #[cfg(target_os = "linux")]
            let appimage = app.env().appimage;
            #[cfg(not(target_os = "linux"))]
            let appimage: Option<std::ffi::OsString> = None;
            let autostart = Autostart::new(appimage.as_deref().map(std::path::Path::new));
            configure_autostart(&autostart, &rt);
            app.manage(autostart);
            // Das Fenster steht in tauri.conf.json mit „create: false“: Per Autostart im Tray gibt es
            // keins, bis es jemand öffnet.
            main_window::at_start(app.handle())?;
            let handle = app.handle().clone();
            let watch = rt.clone();
            tauri::async_runtime::spawn(async move {
                // Erst das Profil zum Netzwerk wählen, dann Server oder Client fortsetzen
                netwatch::check_at_start(&watch).await;
                resume(&handle);
                netwatch::spawn(watch);
            });
            updates::spawn_scheduler(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if tray::available() {
                    // Mit Tray versteckt Schließen das Fenster; nach einer Weile im Tray endet es ganz
                    // (main_window). Beendet wird über das Tray-Menü.
                    let _ = window.hide();
                    main_window::sync_page(window.app_handle());
                    main_window::hidden(window.app_handle());
                } else {
                    // Ohne Tray wäre ein verstecktes Fenster nicht mehr erreichbar.
                    quit(window.app_handle());
                }
            }
            WindowEvent::Resized(_) => {
                main_window::sync_page(window.app_handle());
                main_window::track(window);
            }
            WindowEvent::Moved(_) => main_window::track(window),
            WindowEvent::Focused(true) => main_window::sync_page(window.app_handle()),
            WindowEvent::Destroyed => main_window::destroyed(window.app_handle()),
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("failed to start the Qross GUI")
        .run(on_run_event);
}

fn on_run_event(app: &AppHandle, event: RunEvent) {
    match event {
        // Das Fenster endet im Tray nach einer Weile (main_window); Qross läuft ohne weiter. Beendet
        // wird nur über quit() und Updates, beide mit Exit-Code.
        RunEvent::ExitRequested { code: None, api, .. } if tray::available() => api.prevent_exit(),
        // macOS: Klick aufs Dock-Symbol oder erneutes Öffnen der App
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => main_window::show(app),
        _ => {}
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}
