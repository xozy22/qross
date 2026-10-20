//! Sprache der Oberfläche

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

/// Sprache für Texte, die Menschen in der Oberfläche lesen; Logs und Fehler sind immer englisch.
/// Gespeichert als `"en"` bzw. `"de"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// Englisch (Standard)
    #[default]
    En,
    /// Deutsch
    De,
}

/// Oberflächensprache dieses Prozesses, siehe [`ui_lang`]
static UI_LANG: AtomicU8 = AtomicU8::new(Lang::En as u8);

/// Oberflächensprache dieses Prozesses (Vorgabe: Englisch). Nur für Texte, die untere Schichten
/// selbst Menschen zeigen, etwa die Vorschau beim Ziehen von Dateien; Logs und Fehler bleiben
/// englisch. Die App setzt sie beim Start und bei jedem Wechsel, die Kommandozeile lässt die Vorgabe.
pub fn ui_lang() -> Lang {
    if UI_LANG.load(Ordering::Relaxed) == Lang::De as u8 {
        Lang::De
    } else {
        Lang::En
    }
}

/// Setzt die Oberflächensprache dieses Prozesses (siehe [`ui_lang`]).
pub fn set_ui_lang(lang: Lang) {
    UI_LANG.store(lang as u8, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_read_like_the_settings() {
        use serde::de::value::Error;
        use serde::de::IntoDeserializer;
        let lang = |text: &str| Lang::deserialize(IntoDeserializer::<Error>::into_deserializer(text)).ok();
        assert_eq!(lang("en"), Some(Lang::En));
        assert_eq!(lang("de"), Some(Lang::De));
        assert_eq!(lang("De"), None, "nur klein geschrieben");
        assert_eq!(lang("fr"), None);
        assert_eq!(Lang::default(), Lang::En);
    }

    // `ui_lang` und `set_ui_lang` prüft `finder::tests::the_drag_preview_follows_the_ui_language`:
    // Nur ein Test darf die Sprache des Prozesses umstellen, die Tests laufen parallel.
}
