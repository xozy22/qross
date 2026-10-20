//! Sprache der Texte, die Menschen lesen: Tray-Menü und Tooltip, Systembenachrichtigungen, Hinweise
//! und Meldungen ans Fenster. Logs und technische Fehler bleiben englisch.
//!
//! Die Sprache gilt für den ganzen Prozess; gespeichert ist sie in `qross_core` (siehe
//! [`qross_core::lang::ui_lang`]), denn auch die Vorschau beim Ziehen in `qross_net` richtet sich
//! danach. Funktionen, die Texte bilden, bekommen die Sprache als Parameter; ihre Aufrufer geben
//! [`lang`] mit.

pub use qross_core::Lang;

/// Sprache für Texte an Menschen (bis zum ersten [`set_lang`] Englisch)
pub fn lang() -> Lang {
    qross_core::lang::ui_lang()
}

pub fn set_lang(lang: Lang) {
    qross_core::lang::set_ui_lang(lang);
}

/// Sprache nach der Einstellung `setting` (`"auto"`, `"de"`, `"en"`) und der Systemsprache
/// `system` (z. B. `"de-DE"`): „de“ und „en“ gelten direkt, alles andere (auch leer oder unbekannt)
/// folgt dem System – Deutsch bei „de“, „de-DE“, „de_AT“ …, sonst Englisch. Dieselbe Regel wie
/// `resolveLanguage` in `ui/js/i18n.js`, damit Fenster und Tray dieselbe Sprache sprechen.
pub fn resolve(setting: &str, system: Option<&str>) -> Lang {
    match setting {
        "de" => Lang::De,
        "en" => Lang::En,
        _ if system.is_some_and(is_german) => Lang::De,
        _ => Lang::En,
    }
}

/// „de“ allein oder mit Region dahinter („de-DE“, „de_AT“), Groß- und Kleinschreibung egal
fn is_german(locale: &str) -> bool {
    let bytes = locale.as_bytes();
    bytes.len() >= 2 && bytes[..2].eq_ignore_ascii_case(b"de") && matches!(bytes.get(2), None | Some(b'-' | b'_'))
}

/// Sprache des Systems wie „de-DE“; `None`, wenn sie sich nicht ermitteln lässt
pub fn system_locale() -> Option<String> {
    sys_locale::get_locale().filter(|locale| !locale.trim().is_empty())
}

/// Setzt die Sprache nach der Einstellung (bei „auto“ nach der Systemsprache). Liefert `true`, wenn
/// sich die Sprache damit ändert.
pub fn apply(setting: &str) -> bool {
    let next = resolve(setting, system_locale().as_deref());
    let changed = lang() != next;
    set_lang(next);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_wins_over_the_system() {
        assert_eq!(resolve("de", Some("en-US")), Lang::De);
        assert_eq!(resolve("en", Some("de-DE")), Lang::En);
        assert_eq!(resolve("de", None), Lang::De);
    }

    #[test]
    fn auto_follows_the_system_language() {
        for system in ["de", "de-DE", "de_AT", "DE-ch", "de-Latn-DE"] {
            assert_eq!(resolve("auto", Some(system)), Lang::De, "{system}");
        }
        for system in ["en-US", "fr-FR", "C", "", "dev", "deu", "es-DE"] {
            assert_eq!(resolve("auto", Some(system)), Lang::En, "{system}");
        }
        assert_eq!(resolve("auto", None), Lang::En, "Systemsprache unbekannt");
        // Leer (ältere gui.json) oder unbekannt wie „auto“
        assert_eq!(resolve("", Some("de-DE")), Lang::De);
        assert_eq!(resolve("fr", Some("de-DE")), Lang::De);
        assert_eq!(resolve("fr", Some("en-GB")), Lang::En);
    }
}
