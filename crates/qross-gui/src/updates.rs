//! Automatische Updates (tauri-plugin-updater): erste Prüfung etwa 15 s nach dem Start, danach
//! täglich. Stündlich wird nachgesehen, ob die letzte erfolgreiche Prüfung älter als 24 h ist, so
//! übersteht der Rhythmus auch Schlaf und Ruhezustand.
//!
//! Gefundene Updates werden im Hintergrund geladen (das Plugin prüft die Signatur) und in einem
//! ruhigen Moment installiert: Rolle gestoppt, als Server der Cursor auf dem Server, als Client der
//! Cursor nicht auf diesem Rechner. Danach startet Qross neu und setzt die Rolle fort.
//!
//! Still installiert wird nur, wo keine Rückfrage kommt (NSIS, AppImage, macOS-App). MSI (UAC),
//! Linux-Pakete (pkexec), unbekannte Installationsart und abgeschaltete Automatik bekommen nur den
//! Hinweis „Update verfügbar“ mit „Jetzt aktualisieren“. Fehler (offline, 404) stehen nur im
//! Protokoll und in der Statuszeile der Einstellungen.
//!
//! Für Tests in Debug-Builds (in Release-Builds wirkungslos):
//! - `QROSS_UPDATE_TEST=1` schaltet die automatische Prüfung ein, erste Prüfung nach 3 s
//! - `QROSS_UPDATE_URL=http://…/latest.json` ersetzt den Endpoint

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use qross_gui::lifecycle::{self, AppState};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};
use tracing::{info, warn};

use crate::runtime::Runtime;

const FIRST_CHECK: Duration = Duration::from_secs(15);
const WAKE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const QUIET_POLL: Duration = Duration::from_secs(5);
/// So lange steht der Hinweis „wird aktualisiert“, bevor installiert wird
const NOTICE_LEAD: Duration = Duration::from_secs(4);

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// Noch nicht geprüft
    #[default]
    Idle,
    Checking,
    /// Keine neuere Version
    Current,
    Downloading,
    /// Geladen, wartet auf einen ruhigen Moment
    Waiting,
    Installing,
    /// Gefunden, Installation braucht eine Bestätigung
    Available,
    Error,
}

#[derive(Serialize, Clone, Debug)]
pub struct UpdateStatus {
    pub phase: Phase,
    /// Version dieser App
    pub current: &'static str,
    /// Gefundene neuere Version
    pub version: Option<String>,
    /// Letzte erfolgreiche Prüfung, Unix-Millisekunden
    pub checked_at: Option<u64>,
    /// Fortschritt beim Laden in Prozent
    pub progress: Option<u8>,
    pub message: Option<String>,
    /// Automatische Prüfung aktiv (Release-Build oder `QROSS_UPDATE_TEST`)
    pub automatic: bool,
    /// Installationsart (nsis, msi, appimage, deb, app …); `None` im Entwicklungsbuild
    pub bundle: Option<String>,
}

/// Hinweis im Fenster; `action` ergibt einen Knopf (z. B. `install-update`).
#[derive(Serialize, Clone, Debug)]
pub struct AppNotice {
    pub tone: &'static str,
    pub text: String,
    pub action: Option<&'static str>,
}

pub struct Updates {
    status: Mutex<UpdateStatus>,
    /// Prüfung, Laden oder Installation läuft
    busy: AtomicBool,
    /// Gefundenes Update, das auf „Jetzt aktualisieren“ wartet
    pending: Mutex<Option<Update>>,
    /// Version, zu der in diesem Lauf schon „verfügbar“ gemeldet wurde
    announced: Mutex<Option<String>>,
}

impl Updates {
    pub fn new(state: &AppState) -> Self {
        Self {
            status: Mutex::new(UpdateStatus {
                phase: Phase::Idle,
                current: env!("CARGO_PKG_VERSION"),
                version: None,
                checked_at: state.last_update_check.map(|s| s * 1000),
                progress: None,
                message: None,
                automatic: automatic(),
                bundle: bundle_name(),
            }),
            busy: AtomicBool::new(false),
            pending: Mutex::new(None),
            announced: Mutex::new(None),
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, app: &AppHandle, change: impl FnOnce(&mut UpdateStatus)) {
        let status = {
            let mut status = self.status.lock().unwrap();
            change(&mut status);
            status.clone()
        };
        let _ = app.emit("update-status", status);
    }
}

/// Gibt `busy` frei, wenn eine Prüfung oder Installation endet (auch bei Fehlern).
struct Busy<'a>(&'a AtomicBool);

impl<'a> Busy<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        (!flag.swap(true, Ordering::SeqCst)).then_some(Self(flag))
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Testschalter, nur in Debug-Builds
fn test_env(name: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Automatische Prüfung nur in Release-Builds (oder zum Testen per `QROSS_UPDATE_TEST`).
pub fn automatic() -> bool {
    !cfg!(debug_assertions) || test_env("QROSS_UPDATE_TEST").is_some()
}

fn bundle_name() -> Option<String> {
    tauri::utils::platform::bundle_type().map(|b| b.to_string())
}

fn runtime(app: &AppHandle) -> Arc<Runtime> {
    app.state::<Arc<Runtime>>().inner().clone()
}

fn updates(app: &AppHandle) -> Arc<Updates> {
    app.state::<Arc<Updates>>().inner().clone()
}

fn notice(app: &AppHandle, tone: &'static str, text: String, action: Option<&'static str>) {
    let _ = app.emit("app-notice", AppNotice { tone, text, action });
}

/// Startet den Planer (nur wenn automatisch geprüft wird).
pub fn spawn_scheduler(app: AppHandle) {
    if !automatic() {
        info!("Automatische Update-Prüfung nur in installierten Versionen (Release-Build)");
        return;
    }
    tauri::async_runtime::spawn(async move {
        let first = if test_env("QROSS_UPDATE_TEST").is_some() { Duration::from_secs(3) } else { FIRST_CHECK };
        tokio::time::sleep(first).await;
        check(&app).await;
        loop {
            tokio::time::sleep(WAKE_INTERVAL).await;
            if lifecycle::update_due(runtime(&app).app_state().last_update_check, now_secs()) {
                check(&app).await;
            }
        }
    });
}

fn build_updater(app: &AppHandle) -> Result<Updater> {
    let mut builder = app.updater_builder();
    if let Some(url) = test_env("QROSS_UPDATE_URL") {
        builder = builder.endpoints(vec![url.parse()?])?;
    }
    Ok(builder.build()?)
}

/// Fehler in Worten für die Statuszeile
fn describe_error(e: &anyhow::Error) -> String {
    let text = format!("{e:#}");
    if text.contains("minisign") || text.contains("signature") {
        "Signatur ungültig, Update verworfen".into()
    } else if text.contains("valid release JSON") {
        "Keine Update-Informationen gefunden".into()
    } else if text.contains("error sending request") || text.contains("dns") || text.contains("connect") {
        "Update-Server nicht erreichbar".into()
    } else {
        text
    }
}

/// Prüft jetzt (Planer oder „Jetzt nach Updates suchen“) und führt ein gefundenes Update weiter.
pub async fn check(app: &AppHandle) {
    let updates = updates(app);
    let Some(_busy) = Busy::acquire(&updates.busy) else { return };
    updates.set(app, |s| {
        s.phase = Phase::Checking;
        s.message = None;
        s.progress = None;
    });
    let result: Result<Option<Update>> = async { Ok(build_updater(app)?.check().await?) }.await;
    let update = match result {
        Ok(update) => {
            let now = now_secs();
            runtime(app).update_app_state(|s| s.last_update_check = Some(now));
            updates.set(app, |s| s.checked_at = Some(now * 1000));
            update
        }
        Err(e) => {
            warn!("Update-Prüfung fehlgeschlagen: {e:#}");
            let message = describe_error(&e);
            updates.set(app, |s| {
                s.phase = Phase::Error;
                s.message = Some(message);
            });
            return;
        }
    };
    let Some(update) = update else {
        info!("Qross {} ist aktuell", env!("CARGO_PKG_VERSION"));
        updates.set(app, |s| {
            s.phase = Phase::Current;
            s.version = None;
        });
        return;
    };
    info!("Update {} gefunden", update.version);
    let automatic = runtime(app).settings.lock().unwrap().auto_update;
    if automatic && lifecycle::silent_install_possible(bundle_name().as_deref()) {
        install_when_quiet(app, update).await;
    } else {
        offer(app, update);
    }
}

/// Merkt sich das Update und bietet es an (einmal je Version und Programmlauf als Hinweis).
fn offer(app: &AppHandle, update: Update) {
    let updates = updates(app);
    let version = update.version.clone();
    *updates.pending.lock().unwrap() = Some(update);
    updates.set(app, |s| {
        s.phase = Phase::Available;
        s.version = Some(version.clone());
        s.progress = None;
        s.message = None;
    });
    let mut announced = updates.announced.lock().unwrap();
    if announced.as_deref() != Some(version.as_str()) {
        *announced = Some(version.clone());
        notice(app, "info", format!("Update {version} verfügbar"), Some("install-update"));
    }
}

async fn download(app: &AppHandle, update: &Update) -> Result<Vec<u8>> {
    let updates = updates(app);
    let version = update.version.clone();
    updates.set(app, |s| {
        s.phase = Phase::Downloading;
        s.version = Some(version);
        s.progress = Some(0);
        s.message = None;
    });
    let mut received = 0u64;
    let mut shown = 0u8;
    let bytes = update
        .download(
            |chunk, total| {
                received += chunk as u64;
                if let Some(total) = total.filter(|t| *t > 0) {
                    let percent = (received * 100 / total).min(100) as u8;
                    if percent != shown {
                        shown = percent;
                        updates.set(app, |s| s.progress = Some(percent));
                    }
                }
            },
            || {},
        )
        .await?;
    Ok(bytes)
}

/// Lädt im Hintergrund und installiert, sobald niemand Maus und Tastatur braucht.
async fn install_when_quiet(app: &AppHandle, update: Update) {
    let bytes = match download(app, &update).await {
        Ok(bytes) => bytes,
        Err(e) => return failed(app, "Update nicht geladen", e),
    };
    updates(app).set(app, |s| {
        s.phase = Phase::Waiting;
        s.progress = None;
    });
    info!("Update {} geladen – wird in einem ruhigen Moment installiert", update.version);
    loop {
        let rt = runtime(app);
        if !rt.settings.lock().unwrap().auto_update {
            // Automatik inzwischen abgeschaltet: nur noch anbieten
            return offer(app, update);
        }
        let status = rt.status();
        let server = rt.settings.lock().unwrap().server.name.clone();
        if lifecycle::is_quiet(status.role, &server, status.cursor_on.as_deref(), status.cursor_here) {
            break;
        }
        tokio::time::sleep(QUIET_POLL).await;
    }
    install(app, update, bytes).await;
}

/// „Jetzt aktualisieren“: lädt das angebotene Update und installiert es sofort.
pub async fn install_now(app: &AppHandle) {
    let updates = updates(app);
    let Some(_busy) = Busy::acquire(&updates.busy) else { return };
    let Some(update) = updates.pending.lock().unwrap().take() else { return };
    match download(app, &update).await {
        Ok(bytes) => install(app, update, bytes).await,
        Err(e) => {
            let version = update.version.clone();
            *updates.pending.lock().unwrap() = Some(update);
            failed(app, "Update nicht geladen", e);
            updates.set(app, |s| s.version = Some(version));
        }
    }
}

/// Hinweis, Rolle anhalten (bleibt zum Fortsetzen gemerkt), installieren, neu starten.
async fn install(app: &AppHandle, update: Update, bytes: Vec<u8>) {
    let version = update.version.clone();
    updates(app).set(app, |s| {
        s.phase = Phase::Installing;
        s.version = Some(version.clone());
        s.progress = None;
    });
    let text = format!("Qross wird auf {version} aktualisiert und startet neu");
    info!("{text}");
    notice(app, "info", text.clone(), None);
    crate::notify(app, "Qross wird aktualisiert", &text);
    tokio::time::sleep(NOTICE_LEAD).await;
    runtime(app).stop_for_exit().await;
    // Windows: install() startet den Installer und beendet die App; der Installer startet sie neu.
    if let Err(e) = update.install(&bytes) {
        failed(app, "Update nicht installiert", e.into());
        // Die Rolle war schon angehalten: wieder aufnehmen
        crate::resume(app);
        return;
    }
    app.restart();
}

fn failed(app: &AppHandle, what: &str, e: anyhow::Error) {
    warn!("{what}: {e:#}");
    let message = format!("{what}: {}", describe_error(&e));
    updates(app).set(app, |s| {
        s.phase = Phase::Error;
        s.progress = None;
        s.message = Some(message);
    });
}
