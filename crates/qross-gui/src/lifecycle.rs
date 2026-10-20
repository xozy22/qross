//! Was über einen Programmlauf hinaus gilt: Fortsetzen nach Neustart oder Update, fällige
//! Update-Prüfung, „ruhiger Moment“ für die Installation. Reine Logik ohne Tauri.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::Role;

/// Zustand der App (nicht vom Nutzer eingestellt), im Datenverzeichnis neben `gui.json`.
pub const STATE_FILE: &str = "app-state.json";

/// Abstand zwischen zwei automatischen Update-Prüfungen
pub const UPDATE_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(default)]
pub struct AppState {
    /// Rolle, die beim Beenden lief. Ein ausdrückliches Stoppen löscht sie.
    pub last_running: Option<Role>,
    /// Letzte Update-Prüfung (Unix-Sekunden)
    pub last_update_check: Option<u64>,
    /// Autostart wurde beim ersten Start eingerichtet; danach entscheidet nur noch der Nutzer.
    pub autostart_configured: bool,
    /// Netzwerk, nach dem zuletzt das Profil gewählt wurde (siehe `profiles::signature`). Ist es beim
    /// Start dasselbe, bleibt ein von Hand gewähltes Profil aktiv.
    pub network: Option<String>,
}

impl AppState {
    /// Lädt den Zustand; eine fehlende oder kaputte Datei ergibt den Ausgangszustand.
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(STATE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join(STATE_FILE);
        fs::write(&path, serde_json::to_string_pretty(self)?).with_context(|| format!("failed to write {}", path.display()))
    }
}

/// Ist eine Update-Prüfung fällig? Auch wenn die Uhr zurückgestellt wurde (letzte Prüfung „in der Zukunft“).
pub fn update_due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(last) => now < last || now - last >= UPDATE_INTERVAL_SECS,
    }
}

/// Welche Rolle beim Start fortgesetzt wird: nur wenn gewünscht, beim Beenden eine lief und
/// sie noch zur eingestellten Rolle passt.
pub fn resume_role(enabled: bool, last_running: Option<Role>, configured: Role) -> Option<Role> {
    if !enabled {
        return None;
    }
    last_running.filter(|role| *role == configured)
}

/// Kann jetzt installiert werden, ohne jemandem Maus oder Tastatur wegzunehmen?
/// Gestoppt: ja. Server: nur wenn der Cursor auf dem Server selbst ist. Client: nur wenn der
/// Cursor gerade nicht auf diesem Rechner ist.
pub fn is_quiet(running: Option<Role>, server_name: &str, cursor_on: Option<&str>, cursor_here: bool) -> bool {
    match running {
        None => true,
        Some(Role::Server) => cursor_on.is_none_or(|screen| screen == server_name),
        Some(Role::Client) => !cursor_here,
    }
}

/// Darf ein Update ohne Rückfrage installiert werden? Nicht bei MSI (UAC-Abfrage) und Linux-
/// Paketen (pkexec-Passwort), auch nicht ohne bekannte Installationsart (z. B. Entwicklungsbuild).
/// `bundle` ist der Name aus `tauri::utils::config::BundleType` in Kleinbuchstaben.
pub fn silent_install_possible(bundle: Option<&str>) -> bool {
    matches!(bundle, Some("nsis" | "appimage" | "app"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_is_due_after_a_day_or_without_any_check() {
        let now = 1_800_000_000;
        assert!(update_due(None, now));
        assert!(!update_due(Some(now), now));
        assert!(!update_due(Some(now - UPDATE_INTERVAL_SECS + 60), now), "noch keine 24 h");
        assert!(update_due(Some(now - UPDATE_INTERVAL_SECS), now), "genau 24 h");
        assert!(update_due(Some(now - 3 * UPDATE_INTERVAL_SECS), now), "nach langem Schlaf");
        assert!(update_due(Some(now + 3600), now), "Uhr zurückgestellt");
    }

    #[test]
    fn quiet_moment_depends_on_role_and_cursor() {
        assert!(is_quiet(None, "pc", None, false));
        assert!(is_quiet(Some(Role::Server), "pc", Some("pc"), false));
        assert!(is_quiet(Some(Role::Server), "pc", None, false), "noch kein Cursor gemeldet");
        assert!(!is_quiet(Some(Role::Server), "pc", Some("laptop"), false), "Cursor auf einem Client");
        assert!(is_quiet(Some(Role::Client), "", None, false));
        assert!(!is_quiet(Some(Role::Client), "", None, true), "Cursor auf diesem Client");
    }

    #[test]
    fn only_the_role_that_was_running_is_resumed() {
        assert_eq!(resume_role(true, Some(Role::Server), Role::Server), Some(Role::Server));
        assert_eq!(resume_role(true, Some(Role::Client), Role::Client), Some(Role::Client));
        assert_eq!(resume_role(false, Some(Role::Server), Role::Server), None, "abgeschaltet");
        assert_eq!(resume_role(true, None, Role::Server), None, "lief nicht");
        assert_eq!(resume_role(true, Some(Role::Client), Role::Server), None, "Rolle inzwischen gewechselt");
    }

    #[test]
    fn silent_install_only_without_prompts() {
        assert!(silent_install_possible(Some("nsis")));
        assert!(silent_install_possible(Some("appimage")));
        assert!(silent_install_possible(Some("app")));
        assert!(!silent_install_possible(Some("msi")), "UAC");
        assert!(!silent_install_possible(Some("deb")), "pkexec");
        assert!(!silent_install_possible(Some("rpm")));
        assert!(!silent_install_possible(None), "Entwicklungsbuild");
    }

    #[test]
    fn state_roundtrip_and_broken_file() {
        let dir = std::env::temp_dir().join(format!("qross-state-test-{}", std::process::id()));
        assert_eq!(AppState::load(&dir), AppState::default());
        let state = AppState {
            last_running: Some(Role::Client),
            last_update_check: Some(42),
            autostart_configured: true,
            network: Some("192.168.178.1|3c:a6:2f:0a:1b:2c|192.168.178.0/24".into()),
        };
        state.save(&dir).unwrap();
        assert_eq!(AppState::load(&dir), state);
        fs::write(dir.join(STATE_FILE), "{ kaputt").unwrap();
        assert_eq!(AppState::load(&dir), AppState::default());
        let _ = fs::remove_dir_all(&dir);
    }
}
