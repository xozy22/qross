//! Fenster mit Administratorrechten am Windows-Server, etwa der Task-Manager: Texte für den Hinweis im
//! Fenster, im Tray und als Systembenachrichtigung.
//!
//! Liegt so ein Fenster vorne oder unter dem Cursor, lässt Windows Qross weder Maus noch Tastatur
//! sehen; der Kern wechselt dann nicht zu Clients (siehe
//! `qross_core::server::ServerEvent::LocalElevatedWindow`). Anders als bei der sicheren
//! Tastatureingabe gilt der Hinweis auch ohne verbundenen Client: Er erklärt dann schon, warum der
//! Cursor an der Kante hängen bliebe. Die Benachrichtigung kommt nur, wenn der Nutzer wechseln wollte.

use qross_platform::ElevatedWindow;
use serde::Serialize;

use crate::i18n::Lang;

/// Hinweis oben im Fenster, solange so ein Fenster im Weg ist und der Server läuft
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ElevatedHint {
    pub title: String,
    /// Was los ist und was hilft
    pub text: String,
}

/// Höchstens so viele Zeichen hat der Name in Hinweisen; längere Fenstertitel werden gekürzt.
const NAME_CHARS: usize = 40;

/// Wer im Weg ist
#[derive(Debug, PartialEq, Eq)]
enum Blocker {
    TaskManager,
    /// Fenstertitel oder Programmname, z. B. „Administrator: Eingabeaufforderung“ oder „regedit“
    Named(String),
    Unknown,
}

fn blocker(window: &ElevatedWindow) -> Blocker {
    if window.is_task_manager() {
        return Blocker::TaskManager;
    }
    let title = window.title.as_deref().map(str::trim).filter(|title| !title.is_empty());
    match title.or(window.program()) {
        Some(name) => Blocker::Named(shorten(name)),
        None => Blocker::Unknown,
    }
}

fn shorten(name: &str) -> String {
    if name.chars().count() <= NAME_CHARS {
        return name.to_owned();
    }
    let mut short: String = name.chars().take(NAME_CHARS - 1).collect();
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

/// Name für Hinweise: der Task-Manager in der Sprache der Oberfläche, sonst Fenstertitel oder
/// Programmname; `None`, wenn nichts davon bekannt ist.
pub fn name(window: &ElevatedWindow, lang: Lang) -> Option<String> {
    match (blocker(window), lang) {
        (Blocker::TaskManager, Lang::De) => Some("Task-Manager".to_owned()),
        (Blocker::TaskManager, Lang::En) => Some("Task Manager".to_owned()),
        (Blocker::Named(name), _) => Some(name),
        (Blocker::Unknown, _) => None,
    }
}

/// Wer mit welchen Rechten läuft, als Satz: „Der Task-Manager läuft mit Administratorrechten“
fn who(window: &ElevatedWindow, lang: Lang) -> String {
    let (de, en) = if window.admin {
        ("mit Administratorrechten", "as administrator")
    } else {
        ("mit höheren Rechten als Qross", "with higher privileges than Qross")
    };
    match (blocker(window), lang) {
        (Blocker::TaskManager, Lang::De) => format!("Der Task-Manager läuft {de}"),
        (Blocker::TaskManager, Lang::En) => format!("Task Manager is running {en}"),
        (Blocker::Named(name), Lang::De) => format!("„{name}“ läuft {de}"),
        (Blocker::Named(name), Lang::En) => format!("“{name}” is running {en}"),
        (Blocker::Unknown, Lang::De) => format!("Ein Fenster läuft {de}"),
        (Blocker::Unknown, Lang::En) => format!("A window is running {en}"),
    }
}

/// „Solange er vorne ist oder unter dem Cursor liegt“
fn meanwhile(window: &ElevatedWindow, lang: Lang) -> &'static str {
    match (blocker(window), lang) {
        (Blocker::TaskManager, Lang::De) => "Solange er vorne ist oder unter dem Cursor liegt",
        (Blocker::Named(_), Lang::De) => "Solange das Fenster vorne ist oder unter dem Cursor liegt",
        (Blocker::Unknown, Lang::De) => "Solange es vorne ist oder unter dem Cursor liegt",
        (Blocker::Named(_), Lang::En) => "While this window is in front or under the cursor",
        (_, Lang::En) => "While it's in front or under the cursor",
    }
}

/// Was hilft
fn remedy(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "Klick kurz auf ein anderes Fenster oder den Desktop.",
        Lang::En => "Just click another window or the desktop.",
    }
}

/// Hinweis zeigen? Solange ein Fenster mit höheren Rechten im Weg ist (der Server meldet das nur,
/// während er läuft). Texte in `lang`.
pub fn hint(window: Option<&ElevatedWindow>, lang: Lang) -> Option<ElevatedHint> {
    let window = window?;
    let (meanwhile, remedy) = (meanwhile(window, lang), remedy(lang));
    let text = match lang {
        Lang::De => format!(
            "{meanwhile}, lässt Windows Qross weder Maus noch Tastatur sehen – zum anderen Rechner geht es dann \
             nicht. {remedy}"
        ),
        Lang::En => format!(
            "{meanwhile}, Windows doesn't let Qross see the mouse or keyboard – so the cursor can't move to the \
             other computer. {remedy}"
        ),
    };
    Some(ElevatedHint { title: who(window, lang), text })
}

/// Zusatz für den Tooltip des Tray-Icons, z. B. „ · Task-Manager mit Administratorrechten: Cursor bleibt hier“
pub fn tray_note(window: &ElevatedWindow, lang: Lang) -> String {
    let name = name(window, lang);
    let rights = match (window.admin, lang) {
        (true, Lang::De) => "mit Administratorrechten",
        (false, Lang::De) => "mit höheren Rechten",
        (true, Lang::En) => "running as administrator",
        (false, Lang::En) => "running with higher privileges",
    };
    match (name, window.is_task_manager(), lang) {
        (Some(name), true, _) => format!(" · {name} {rights}: {}", stays(lang)),
        (Some(name), false, Lang::De) => format!(" · „{name}“ {rights}: {}", stays(lang)),
        (Some(name), false, Lang::En) => format!(" · “{name}” {rights}: {}", stays(lang)),
        (None, _, Lang::De) => format!(" · Fenster {rights}: {}", stays(lang)),
        (None, _, Lang::En) => format!(" · window {rights}: {}", stays(lang)),
    }
}

fn stays(lang: Lang) -> &'static str {
    match lang {
        Lang::De => "Cursor bleibt hier",
        Lang::En => "cursor stays here",
    }
}

/// Systembenachrichtigung, wenn der Nutzer zu einem Client wollte, während so ein Fenster im Weg war
/// (einmal je Mal). Liefert Titel und Text in `lang`; ohne bekanntes Fenster allgemein.
pub fn notification(window: Option<&ElevatedWindow>, lang: Lang) -> (String, String) {
    let unknown = ElevatedWindow { admin: true, ..ElevatedWindow::default() };
    let window = window.unwrap_or(&unknown);
    let (who, meanwhile, remedy) = (who(window, lang), meanwhile(window, lang), remedy(lang));
    match lang {
        Lang::De => (
            "Cursor bleibt auf diesem PC".to_owned(),
            format!("{who}. {meanwhile}, sieht Qross weder Maus noch Tastatur. {remedy}"),
        ),
        Lang::En => (
            "Cursor stays on this PC".to_owned(),
            format!("{who}. {meanwhile}, Qross can't see the mouse or keyboard. {remedy}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taskmgr() -> ElevatedWindow {
        ElevatedWindow { pid: 4711, exe: Some("Taskmgr.exe".into()), title: Some("Task-Manager".into()), admin: true }
    }

    fn console() -> ElevatedWindow {
        ElevatedWindow {
            pid: 815,
            exe: Some("cmd.exe".into()),
            title: Some("Administrator: Eingabeaufforderung".into()),
            admin: true,
        }
    }

    /// Bildschirmtastatur: UI-Zugriff, nur etwas über Qross, ohne Titel
    fn keyboard() -> ElevatedWindow {
        ElevatedWindow { pid: 42, exe: Some("osk.exe".into()), title: None, admin: false }
    }

    #[test]
    fn names_follow_program_title_and_file() {
        assert_eq!(name(&taskmgr(), Lang::De).as_deref(), Some("Task-Manager"));
        // Unabhängig von der Sprache von Windows (dessen Titel wäre „Task Manager“)
        let english_windows = ElevatedWindow { title: Some("Task Manager".into()), ..taskmgr() };
        assert_eq!(name(&english_windows, Lang::De).as_deref(), Some("Task-Manager"));
        assert_eq!(name(&taskmgr(), Lang::En).as_deref(), Some("Task Manager"));
        assert_eq!(name(&console(), Lang::De).as_deref(), Some("Administrator: Eingabeaufforderung"));
        assert_eq!(name(&keyboard(), Lang::En).as_deref(), Some("osk"), "ohne Titel der Programmname");
        let blank = ElevatedWindow { title: Some("  ".into()), ..keyboard() };
        assert_eq!(name(&blank, Lang::De).as_deref(), Some("osk"));
        assert_eq!(name(&ElevatedWindow::default(), Lang::De), None);
        let long = ElevatedWindow { title: Some(format!("C:\\Projekte\\{} - Editor", "x".repeat(60))), ..console() };
        let short = name(&long, Lang::De).unwrap();
        assert_eq!(short.chars().count(), 40, "{short}");
        assert!(short.starts_with("C:\\Projekte\\xxx") && short.ends_with('…'), "{short}");
    }

    #[test]
    fn the_hint_names_the_task_manager() {
        assert_eq!(hint(None, Lang::De), None, "nichts im Weg: kein Hinweis");
        let shown = hint(Some(&taskmgr()), Lang::De).expect("Task-Manager im Weg");
        assert_eq!(shown.title, "Der Task-Manager läuft mit Administratorrechten");
        assert_eq!(
            shown.text,
            "Solange er vorne ist oder unter dem Cursor liegt, lässt Windows Qross weder Maus noch Tastatur sehen \
             – zum anderen Rechner geht es dann nicht. Klick kurz auf ein anderes Fenster oder den Desktop."
        );
        let shown = hint(Some(&taskmgr()), Lang::En).unwrap();
        assert_eq!(shown.title, "Task Manager is running as administrator");
        assert_eq!(
            shown.text,
            "While it's in front or under the cursor, Windows doesn't let Qross see the mouse or keyboard – so the \
             cursor can't move to the other computer. Just click another window or the desktop."
        );
    }

    #[test]
    fn other_windows_get_a_general_hint() {
        let shown = hint(Some(&console()), Lang::De).unwrap();
        assert_eq!(shown.title, "„Administrator: Eingabeaufforderung“ läuft mit Administratorrechten");
        assert!(shown.text.starts_with("Solange das Fenster vorne ist oder unter dem Cursor liegt, lässt"), "{}", shown.text);
        let shown = hint(Some(&console()), Lang::En).unwrap();
        assert_eq!(shown.title, "“Administrator: Eingabeaufforderung” is running as administrator");
        assert!(shown.text.starts_with("While this window is in front or under the cursor, Windows"), "{}", shown.text);

        let unknown = ElevatedWindow { admin: true, ..ElevatedWindow::default() };
        assert_eq!(hint(Some(&unknown), Lang::De).unwrap().title, "Ein Fenster läuft mit Administratorrechten");
        assert!(hint(Some(&unknown), Lang::De).unwrap().text.starts_with("Solange es vorne ist"));
        assert_eq!(hint(Some(&unknown), Lang::En).unwrap().title, "A window is running as administrator");

        // Nur etwas über Qross (UI-Zugriff): keine Administratorrechte behaupten
        assert_eq!(hint(Some(&keyboard()), Lang::De).unwrap().title, "„osk“ läuft mit höheren Rechten als Qross");
        assert_eq!(hint(Some(&keyboard()), Lang::En).unwrap().title, "“osk” is running with higher privileges than Qross");
    }

    #[test]
    fn tray_notes_name_the_window() {
        assert_eq!(tray_note(&taskmgr(), Lang::De), " · Task-Manager mit Administratorrechten: Cursor bleibt hier");
        assert_eq!(tray_note(&taskmgr(), Lang::En), " · Task Manager running as administrator: cursor stays here");
        assert_eq!(
            tray_note(&console(), Lang::De),
            " · „Administrator: Eingabeaufforderung“ mit Administratorrechten: Cursor bleibt hier"
        );
        assert_eq!(tray_note(&keyboard(), Lang::En), " · “osk” running with higher privileges: cursor stays here");
        let unknown = ElevatedWindow { admin: true, ..ElevatedWindow::default() };
        assert_eq!(tray_note(&unknown, Lang::De), " · Fenster mit Administratorrechten: Cursor bleibt hier");
        assert_eq!(tray_note(&unknown, Lang::En), " · window running as administrator: cursor stays here");
    }

    #[test]
    fn notifications_say_why_and_what_helps() {
        let (title, body) = notification(Some(&taskmgr()), Lang::De);
        assert_eq!(title, "Cursor bleibt auf diesem PC");
        assert_eq!(
            body,
            "Der Task-Manager läuft mit Administratorrechten. Solange er vorne ist oder unter dem Cursor liegt, sieht \
             Qross weder Maus noch Tastatur. Klick kurz auf ein anderes Fenster oder den Desktop."
        );
        let (title, body) = notification(Some(&taskmgr()), Lang::En);
        assert_eq!(title, "Cursor stays on this PC");
        assert_eq!(
            body,
            "Task Manager is running as administrator. While it's in front or under the cursor, Qross can't see the \
             mouse or keyboard. Just click another window or the desktop."
        );
        let (_, body) = notification(Some(&console()), Lang::De);
        assert!(body.starts_with("„Administrator: Eingabeaufforderung“ läuft mit Administratorrechten. Solange das Fenster"));
        // Schon wieder frei, als die Meldung ankam: allgemein
        let (_, body) = notification(None, Lang::De);
        assert!(body.starts_with("Ein Fenster läuft mit Administratorrechten. Solange es vorne ist"), "{body}");
        let (_, body) = notification(None, Lang::En);
        assert!(body.starts_with("A window is running as administrator. While it's in front"), "{body}");
    }
}
