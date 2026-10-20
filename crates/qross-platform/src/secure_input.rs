//! „Sichere Tastatureingabe“ unter macOS (Secure Event Input), plattformneutraler Teil.
//!
//! Hat ein Programm sie eingeschaltet – ein aktives Passwortfeld, Terminal mit „Sichere
//! Tastatureingabe“, ein Passwortmanager –, liefert macOS Tastaturereignisse keinem Event-Tap mehr,
//! auch nicht dem von Qross. Mausereignisse kommen weiter an. Ein Mac als Server erfasst dann zwar
//! die Maus, aber keine Tasten: Die landen im Programm im Vordergrund des Macs. Abfrage über
//! [`fn@crate::secure_input`].

/// Die sichere Tastatureingabe ist an.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecureInput {
    /// Prozess, der sie eingeschaltet hat (`None`: verrät das System nicht)
    pub pid: Option<u32>,
    /// Name des Programms, z. B. „Terminal“, „Google Chrome“ oder „loginwindow“
    pub app: Option<String>,
}

impl SecureInput {
    /// Fürs Log, z. B. „Terminal (PID 412)“ oder „unbekanntes Programm“
    pub fn describe(&self) -> String {
        match (&self.app, self.pid) {
            (Some(app), Some(pid)) => format!("{app} (PID {pid})"),
            (Some(app), None) => app.clone(),
            (None, Some(pid)) => format!("PID {pid}"),
            (None, None) => "unknown app".to_owned(),
        }
    }
}

/// Programmname aus dem Pfad der ausführbaren Datei: das äußerste `.app`-Bündel (Hilfsprozesse
/// liegen in dem ihres Programms), sonst der Dateiname. `None` bei leerem Pfad.
pub fn app_name(path: &str) -> Option<String> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let bundle = parts.clone().find_map(|part| part.strip_suffix(".app")).filter(|name| !name.is_empty());
    bundle.or_else(|| parts.next_back()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_come_from_the_outermost_bundle() {
        let name = |path| app_name(path);
        assert_eq!(name("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal").as_deref(), Some("Terminal"));
        assert_eq!(
            name(
                "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/\
                 Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)"
            )
            .as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(name("/System/Library/CoreServices/loginwindow.app/Contents/MacOS/loginwindow").as_deref(), Some("loginwindow"));
        // Ohne Bündel: der Dateiname
        assert_eq!(
            name("/System/Library/Frameworks/Security.framework/Versions/A/MachServices/SecurityAgent.bundle/Contents/MacOS/SecurityAgent")
                .as_deref(),
            Some("SecurityAgent")
        );
        assert_eq!(name("/usr/bin/sudo").as_deref(), Some("sudo"));
        assert_eq!(name(""), None);
        assert_eq!(name("/"), None);
    }

    #[test]
    fn descriptions_name_what_is_known() {
        let full = SecureInput { pid: Some(412), app: Some("Terminal".into()) };
        assert_eq!(full.describe(), "Terminal (PID 412)");
        assert_eq!(SecureInput { pid: Some(7), app: None }.describe(), "PID 7");
        assert_eq!(SecureInput::default().describe(), "unknown app");
    }
}
