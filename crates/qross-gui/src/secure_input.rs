//! „Sichere Tastatureingabe“ am Mac als Server: Texte und Regeln für den Hinweis im Fenster, im
//! Tray und als Systembenachrichtigung.
//!
//! Solange ein Programm sie hält, bekommt Qross keine Tasten mit; der Kern holt den Cursor deshalb
//! von einem Client zurück und hält ihn auf dem Mac, bis sie endet (siehe
//! `qross_core::server::ServerEvent::LocalSecureInput`). Ohne verbundenen Client stört sie nicht –
//! ein Passwortfeld auf dem Mac ist dann kein Grund für einen Hinweis.

use qross_platform::SecureInput;
use serde::Serialize;

use crate::i18n::Lang;

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
/// verbunden ist; endet sie oder geht der letzte Client, verschwindet er. Texte in `lang`.
pub fn hint(holder: Option<&SecureInput>, clients: usize, lang: Lang) -> Option<SecureInputHint> {
    let holder = holder.filter(|_| clients > 0)?;
    let app = holder.app.clone();
    let who = match (&app, lang) {
        (Some(app), Lang::De) => format!("„{app}“ hat sie eingeschaltet."),
        (Some(app), Lang::En) => format!("“{app}” turned it on."),
        (None, Lang::De) => "Ein Programm hat sie eingeschaltet; welches, verrät macOS nicht.".to_owned(),
        (None, Lang::En) => "An app turned it on; macOS doesn't say which one.".to_owned(),
    };
    let (title, text) = match lang {
        Lang::De => (
            "macOS: Sichere Tastatureingabe ist an",
            format!(
                "{who} Solange sie an ist, bekommt Qross keine Tasten mit – was du tippst, würde auf dem Mac landen statt \
                 auf dem anderen Rechner. Deshalb bleibt der Cursor auf diesem Mac, bis sie wieder aus ist."
            ),
        ),
        Lang::En => (
            "macOS: Secure Keyboard Entry is on",
            format!(
                "{who} While it's on, Qross doesn't receive any keystrokes – what you type would end up on the Mac \
                 instead of the other computer. That's why the cursor stays on this Mac until it's off again."
            ),
        ),
    };
    Some(SecureInputHint { title: title.to_owned(), text, steps: steps(app.as_deref(), lang), app })
}

/// Was man tun kann, je nach Programm
pub fn steps(app: Option<&str>, lang: Lang) -> Vec<String> {
    match lang {
        Lang::De => steps_de(app),
        Lang::En => steps_en(app),
    }
}

fn steps_de(app: Option<&str>) -> Vec<String> {
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

fn steps_en(app: Option<&str>) -> Vec<String> {
    let password_field = "If a password field is active, click into another field or app.".to_owned();
    match app {
        Some("Terminal") => vec![
            "In Terminal, turn off “Secure Keyboard Entry” in the “Terminal” menu – or bring another app to the front."
                .to_owned(),
        ],
        Some("iTerm2" | "iTerm") => vec![
            "In iTerm2, turn off “Secure Keyboard Entry” in the “iTerm2” menu – or bring another app to the front."
                .to_owned(),
        ],
        Some("loginwindow") => vec![
            "macOS itself is holding it, a known bug after locking the screen or logging in. Lock the screen (⌃⌘Q) \
             and unlock it again."
                .to_owned(),
            "If that doesn't help, log out and back in.".to_owned(),
        ],
        Some("SecurityAgent") => vec!["A macOS password prompt is open: answer or cancel it.".to_owned()],
        Some(app) => vec![
            password_field,
            format!("If that doesn't help, quit “{app}”. Password managers and terminals sometimes hold on to it."),
        ],
        None => vec![
            password_field,
            "If that doesn't help, quit password managers and terminals. To find out which app it is, run \
             “ioreg -l -w 0 | grep SecureInput” in Terminal (the process ID follows kCGSSessionSecureInputPID)."
                .to_owned(),
        ],
    }
}

/// Zusatz für den Tooltip des Tray-Icons, z. B. „ · Sichere Tastatureingabe (Terminal): Cursor bleibt auf dem Mac“
pub fn tray_note(hint: &SecureInputHint, lang: Lang) -> String {
    let app = hint.app.as_deref().map(|app| format!(" ({app})")).unwrap_or_default();
    match lang {
        Lang::De => format!(" · Sichere Tastatureingabe{app}: Cursor bleibt auf dem Mac"),
        Lang::En => format!(" · Secure Keyboard Entry{app}: cursor stays on the Mac"),
    }
}

/// Systembenachrichtigung, wenn der Cursor deswegen auf dem Mac bleibt: vom Client `from`
/// zurückgeholt oder ein Wechsel verhindert (`from: None`). Liefert Titel und Text in `lang`.
pub fn notification(from: Option<&str>, app: Option<&str>, lang: Lang) -> (String, String) {
    let first_step = steps(app, lang).swap_remove(0);
    match lang {
        Lang::De => {
            let who = app.map(|app| format!("„{app}“ hat")).unwrap_or_else(|| "Ein Programm hat".to_owned());
            match from {
                Some(client) => (
                    "Cursor zurück auf dem Mac".to_owned(),
                    format!(
                        "{who} die sichere Tastatureingabe eingeschaltet. Tasten wären nicht auf „{client}“ angekommen, \
                         sondern auf dem Mac. {first_step}"
                    ),
                ),
                None => (
                    "Cursor bleibt auf dem Mac".to_owned(),
                    format!(
                        "{who} die sichere Tastatureingabe eingeschaltet; Tasten kämen auf den anderen Rechnern nicht an. \
                         {first_step}"
                    ),
                ),
            }
        }
        Lang::En => {
            let who = app.map(|app| format!("“{app}”")).unwrap_or_else(|| "An app".to_owned());
            match from {
                Some(client) => (
                    "Cursor back on the Mac".to_owned(),
                    format!(
                        "{who} turned on Secure Keyboard Entry. Keystrokes would have arrived on the Mac instead of \
                         on “{client}”. {first_step}"
                    ),
                ),
                None => (
                    "Cursor stays on the Mac".to_owned(),
                    format!(
                        "{who} turned on Secure Keyboard Entry; keystrokes wouldn't arrive on the other computers. \
                         {first_step}"
                    ),
                ),
            }
        }
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
        assert_eq!(hint(None, 2, Lang::De), None, "aus: kein Hinweis");
        assert_eq!(hint(Some(&terminal()), 0, Lang::De), None, "ohne Client stört sie nicht");
        let shown = hint(Some(&terminal()), 1, Lang::De).expect("an und ein Client verbunden");
        assert_eq!(shown.app.as_deref(), Some("Terminal"));
        assert_eq!(shown.title, "macOS: Sichere Tastatureingabe ist an");
        assert!(shown.text.starts_with("„Terminal“ hat sie eingeschaltet."), "{}", shown.text);
        assert!(shown.text.contains("bleibt der Cursor auf diesem Mac"), "{}", shown.text);
        assert!(shown.steps[0].contains("„Sichere Tastatureingabe“ ausschalten"), "{:?}", shown.steps);
        // Endet sie oder geht der letzte Client, verschwindet er
        assert_eq!(hint(None, 1, Lang::De), None);
        assert_eq!(hint(Some(&terminal()), 0, Lang::De), None);
    }

    #[test]
    fn the_hint_in_english() {
        assert_eq!(hint(Some(&terminal()), 0, Lang::En), None);
        let shown = hint(Some(&terminal()), 1, Lang::En).unwrap();
        assert_eq!(shown.title, "macOS: Secure Keyboard Entry is on");
        assert!(shown.text.starts_with("“Terminal” turned it on. While it's on, Qross"), "{}", shown.text);
        assert!(shown.text.ends_with("the cursor stays on this Mac until it's off again."), "{}", shown.text);
        assert!(shown.steps[0].starts_with("In Terminal, turn off “Secure Keyboard Entry”"), "{:?}", shown.steps);
        let unknown = hint(Some(&SecureInput::default()), 1, Lang::En).unwrap();
        assert!(unknown.text.starts_with("An app turned it on; macOS doesn't say which one."), "{}", unknown.text);
        assert_eq!(unknown.steps.len(), 2);
        assert!(unknown.steps[1].contains("ioreg"), "{:?}", unknown.steps);
    }

    #[test]
    fn unknown_programs_get_general_advice() {
        let shown = hint(Some(&SecureInput::default()), 1, Lang::De).unwrap();
        assert_eq!(shown.app, None);
        assert!(shown.text.starts_with("Ein Programm hat sie eingeschaltet"), "{}", shown.text);
        assert_eq!(shown.steps.len(), 2);
        assert!(shown.steps[1].contains("ioreg"), "{:?}", shown.steps);
    }

    #[test]
    fn advice_fits_the_program() {
        assert!(steps(Some("iTerm2"), Lang::De)[0].contains("Secure Keyboard Entry"));
        assert!(steps(Some("loginwindow"), Lang::De)[0].contains("sperren"));
        assert!(steps(Some("SecurityAgent"), Lang::De)[0].contains("Passwortabfrage"));
        let other = steps(Some("1Password 7"), Lang::De);
        assert!(other[0].contains("Passwortfeld"), "{other:?}");
        assert!(other[1].contains("„1Password 7“ beenden"), "{other:?}");

        assert!(steps(Some("iTerm"), Lang::En)[0].contains("“iTerm2” menu"));
        assert_eq!(steps(Some("loginwindow"), Lang::En).len(), 2);
        assert!(steps(Some("SecurityAgent"), Lang::En)[0].contains("password prompt"));
        let other = steps(Some("1Password 7"), Lang::En);
        assert!(other[0].contains("password field"), "{other:?}");
        assert!(other[1].contains("quit “1Password 7”"), "{other:?}");
    }

    #[test]
    fn tray_and_notifications_name_the_program() {
        let shown = hint(Some(&terminal()), 1, Lang::De).unwrap();
        assert_eq!(tray_note(&shown, Lang::De), " · Sichere Tastatureingabe (Terminal): Cursor bleibt auf dem Mac");
        assert_eq!(tray_note(&shown, Lang::En), " · Secure Keyboard Entry (Terminal): cursor stays on the Mac");
        let unknown = hint(Some(&SecureInput::default()), 1, Lang::De).unwrap();
        assert_eq!(tray_note(&unknown, Lang::De), " · Sichere Tastatureingabe: Cursor bleibt auf dem Mac");

        let (title, body) = notification(Some("pc"), Some("Terminal"), Lang::De);
        assert_eq!(title, "Cursor zurück auf dem Mac");
        assert!(body.starts_with("„Terminal“ hat die sichere Tastatureingabe eingeschaltet."), "{body}");
        assert!(body.contains("nicht auf „pc“ angekommen"), "{body}");
        assert!(body.ends_with("nach vorn holen."), "mit dem ersten Schritt: {body}");

        let (title, body) = notification(None, None, Lang::De);
        assert_eq!(title, "Cursor bleibt auf dem Mac");
        assert!(body.starts_with("Ein Programm hat die sichere Tastatureingabe eingeschaltet;"), "{body}");
        assert!(body.ends_with("in ein anderes Feld oder Programm klicken."), "{body}");
    }

    #[test]
    fn notifications_in_english() {
        let (title, body) = notification(Some("pc"), Some("Terminal"), Lang::En);
        assert_eq!(title, "Cursor back on the Mac");
        assert!(body.starts_with("“Terminal” turned on Secure Keyboard Entry. Keystrokes would have"), "{body}");
        assert!(body.contains("instead of on “pc”."), "{body}");
        assert!(body.ends_with("bring another app to the front."), "mit dem ersten Schritt: {body}");

        let (title, body) = notification(None, None, Lang::En);
        assert_eq!(title, "Cursor stays on the Mac");
        assert!(body.starts_with("An app turned on Secure Keyboard Entry;"), "{body}");
        assert!(body.ends_with("click into another field or app."), "{body}");
    }
}
