//! „Sichere Tastatureingabe“ am Mac als Server: Texte und Regeln für den Hinweis im Fenster, im
//! Tray und als Systembenachrichtigung.
//!
//! Solange ein Programm sie hält, bekommt Qross keine Tasten mit; der Kern holt den Cursor deshalb
//! von einem Client zurück und hält ihn auf dem Mac, bis sie endet (siehe
//! `qross_core::server::ServerEvent::LocalSecureInput`). Ohne verbundenen Client stört sie nicht –
//! ein Passwortfeld auf dem Mac ist dann kein Grund für einen Hinweis.

use qross_platform::SecureInput;
use serde::Serialize;

/// Hinweis oben im Fenster (und kurz im Tray), solange er gilt
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SecureInputHint {
    /// Programm, das sie hält (`None`: verrät macOS nicht)
    pub app: Option<String>,
    pub title: String,
    /// Was los ist und was Qross tut
    pub text: String,
    /// Was man tun kann, das Naheliegende zuerst
    pub steps: Vec<String>,
}

/// Hinweis zeigen? Nur solange die sichere Tastatureingabe an ist und mindestens ein Client
/// verbunden ist; endet sie oder geht der letzte Client, verschwindet er.
pub fn hint(holder: Option<&SecureInput>, clients: usize) -> Option<SecureInputHint> {
    let holder = holder.filter(|_| clients > 0)?;
    let app = holder.app.clone();
    let who = match &app {
        Some(app) => format!("„{app}“ hat sie eingeschaltet."),
        None => "Ein Programm hat sie eingeschaltet; welches, verrät macOS nicht.".to_owned(),
    };
    Some(SecureInputHint {
        title: "macOS: Sichere Tastatureingabe ist an".to_owned(),
        text: format!(
            "{who} Solange sie an ist, bekommt Qross keine Tasten mit – was du tippst, würde auf dem Mac landen statt \
             auf dem anderen Rechner. Deshalb bleibt der Cursor auf diesem Mac, bis sie wieder aus ist."
        ),
        steps: steps(app.as_deref()),
        app,
    })
}

/// Was man tun kann, je nach Programm
pub fn steps(app: Option<&str>) -> Vec<String> {
    let password_field = "Ist ein Passwortfeld aktiv, in ein anderes Feld oder Programm klicken.".to_owned();
    match app {
        Some("Terminal") => vec![
            "In Terminal im Menü „Terminal“ die „Sichere Tastatureingabe“ ausschalten – oder ein anderes Programm \
             nach vorn holen."
                .to_owned(),
        ],
        Some("iTerm2" | "iTerm") => vec![
            "In iTerm2 im Menü „iTerm2“ „Secure Keyboard Entry“ ausschalten – oder ein anderes Programm nach vorn holen."
                .to_owned(),
        ],
        Some("loginwindow") => vec![
            "macOS selbst hält sie fest, ein bekannter Fehler nach dem Sperren oder Anmelden. Den Bildschirm sperren \
             (⌃⌘Q) und wieder entsperren."
                .to_owned(),
            "Hilft das nicht: ab- und wieder anmelden.".to_owned(),
        ],
        Some("SecurityAgent") => vec!["Eine Passwortabfrage von macOS ist offen: sie beantworten oder abbrechen.".to_owned()],
        Some(app) => vec![
            password_field,
            format!("Hilft das nicht: „{app}“ beenden. Passwortmanager und Terminals halten sie manchmal fest."),
        ],
        None => vec![
            password_field,
            "Hilft das nicht: Passwortmanager und Terminals beenden. Welches Programm es ist, zeigt im Terminal \
             „ioreg -l -w 0 | grep SecureInput“ (Prozessnummer hinter kCGSSessionSecureInputPID)."
                .to_owned(),
        ],
    }
}

/// Zusatz für den Tooltip des Tray-Icons, z. B. „ · Sichere Tastatureingabe (Terminal): Cursor bleibt auf dem Mac“
pub fn tray_note(hint: &SecureInputHint) -> String {
    let app = hint.app.as_deref().map(|app| format!(" ({app})")).unwrap_or_default();
    format!(" · Sichere Tastatureingabe{app}: Cursor bleibt auf dem Mac")
}

/// Systembenachrichtigung, wenn der Cursor deswegen auf dem Mac bleibt: vom Client `from`
/// zurückgeholt oder ein Wechsel verhindert (`from: None`). Liefert Titel und Text.
pub fn notification(from: Option<&str>, app: Option<&str>) -> (String, String) {
    let who = app.map(|app| format!("„{app}“ hat")).unwrap_or_else(|| "Ein Programm hat".to_owned());
    let first_step = steps(app).swap_remove(0);
    match from {
        Some(client) => (
            "Cursor zurück auf dem Mac".to_owned(),
            format!(
                "{who} die sichere Tastatureingabe eingeschaltet. Tasten wären nicht auf „{client}“ angekommen, sondern \
                 auf dem Mac. {first_step}"
            ),
        ),
        None => (
            "Cursor bleibt auf dem Mac".to_owned(),
            format!("{who} die sichere Tastatureingabe eingeschaltet; Tasten kämen auf den anderen Rechnern nicht an. {first_step}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal() -> SecureInput {
        SecureInput { pid: Some(412), app: Some("Terminal".into()) }
    }

    #[test]
    fn the_hint_shows_only_with_a_connected_client() {
        assert_eq!(hint(None, 2), None, "aus: kein Hinweis");
        assert_eq!(hint(Some(&terminal()), 0), None, "ohne Client stört sie nicht");
        let shown = hint(Some(&terminal()), 1).expect("an und ein Client verbunden");
        assert_eq!(shown.app.as_deref(), Some("Terminal"));
        assert!(shown.text.starts_with("„Terminal“ hat sie eingeschaltet."), "{}", shown.text);
        assert!(shown.text.contains("bleibt der Cursor auf diesem Mac"), "{}", shown.text);
        assert!(shown.steps[0].contains("„Sichere Tastatureingabe“ ausschalten"), "{:?}", shown.steps);
        // Endet sie oder geht der letzte Client, verschwindet er
        assert_eq!(hint(None, 1), None);
        assert_eq!(hint(Some(&terminal()), 0), None);
    }

    #[test]
    fn unknown_programs_get_general_advice() {
        let shown = hint(Some(&SecureInput::default()), 1).unwrap();
        assert_eq!(shown.app, None);
        assert!(shown.text.starts_with("Ein Programm hat sie eingeschaltet"), "{}", shown.text);
        assert_eq!(shown.steps.len(), 2);
        assert!(shown.steps[1].contains("ioreg"), "{:?}", shown.steps);
    }

    #[test]
    fn advice_fits_the_program() {
        assert!(steps(Some("iTerm2"))[0].contains("Secure Keyboard Entry"));
        assert!(steps(Some("loginwindow"))[0].contains("sperren"));
        assert!(steps(Some("SecurityAgent"))[0].contains("Passwortabfrage"));
        let other = steps(Some("1Password 7"));
        assert!(other[0].contains("Passwortfeld"), "{other:?}");
        assert!(other[1].contains("„1Password 7“ beenden"), "{other:?}");
    }

    #[test]
    fn tray_and_notifications_name_the_program() {
        let shown = hint(Some(&terminal()), 1).unwrap();
        assert_eq!(tray_note(&shown), " · Sichere Tastatureingabe (Terminal): Cursor bleibt auf dem Mac");
        let unknown = hint(Some(&SecureInput::default()), 1).unwrap();
        assert_eq!(tray_note(&unknown), " · Sichere Tastatureingabe: Cursor bleibt auf dem Mac");

        let (title, body) = notification(Some("pc"), Some("Terminal"));
        assert_eq!(title, "Cursor zurück auf dem Mac");
        assert!(body.starts_with("„Terminal“ hat die sichere Tastatureingabe eingeschaltet."), "{body}");
        assert!(body.contains("nicht auf „pc“ angekommen"), "{body}");
        assert!(body.ends_with("nach vorn holen."), "mit dem ersten Schritt: {body}");

        let (title, body) = notification(None, None);
        assert_eq!(title, "Cursor bleibt auf dem Mac");
        assert!(body.starts_with("Ein Programm hat die sichere Tastatureingabe eingeschaltet;"), "{body}");
        assert!(body.ends_with("in ein anderes Feld oder Programm klicken."), "{body}");
    }
}
