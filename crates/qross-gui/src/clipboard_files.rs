//! Texte zu kopierten Dateien (Strg+C auf Dateien, drüben Strg+V) für Hinweise in der App und
//! Systembenachrichtigungen. Das Protokoll schreibt `qross_net` selbst.

use qross_net::clipfiles::FilesNotice;
use serde::Serialize;

/// Wie der Hinweis aussieht
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct FilesMessage {
    pub tone: Tone,
    /// Überschrift einer Systembenachrichtigung
    pub title: String,
    /// Text des Hinweises; `None` = kein Hinweis (beim Senden, das wäre zu laut)
    pub text: Option<String>,
}

/// Beschreibt eine Meldung in Worten. `mac` wählt Cmd statt Strg.
pub fn describe(notice: &FilesNotice, mac: bool) -> FilesMessage {
    let paste = if mac { "Cmd+V" } else { "Strg+V" };
    match notice {
        FilesNotice::Sending { .. } => {
            FilesMessage { tone: Tone::Info, title: "Kopierte Dateien werden gesendet".into(), text: None }
        }
        FilesNotice::Receiving { peer, names, bytes } => FilesMessage {
            tone: Tone::Info,
            title: "Kopierte Dateien kommen an".into(),
            text: Some(format!("Dateien von „{peer}“ kommen an: {} ({})", names_text(names), size_text(*bytes))),
        },
        FilesNotice::Ready { names, .. } => FilesMessage {
            tone: Tone::Success,
            title: "Kopierte Dateien sind bereit".into(),
            text: Some(format!(
                "{} {} bereit – mit {paste} einfügen",
                names_text(names),
                if names.len() == 1 { "ist" } else { "sind" }
            )),
        },
        FilesNotice::TooLarge { limit, .. } => FilesMessage {
            tone: Tone::Warn,
            title: "Kopierte Dateien zu groß".into(),
            text: Some(format!("Kopierte Dateien sind größer als {} – bitte per Drag & Drop ziehen", size_text(*limit))),
        },
        FilesNotice::Failed { peer, reason } => FilesMessage {
            tone: Tone::Error,
            title: "Kopierte Dateien nicht übertragen".into(),
            text: Some(format!("Kopierte Dateien mit „{peer}“ nicht übertragen: {reason}")),
        },
    }
}

/// „Bericht.pdf“, „a und b“, „a, b und c“, ab vier: „„a“ und 3 weitere“
pub fn names_text(names: &[String]) -> String {
    match names {
        [] => "Dateien".into(),
        [one] => one.clone(),
        [first, second] => format!("{first} und {second}"),
        [first, second, third] => format!("{first}, {second} und {third}"),
        [first, rest @ ..] => format!("„{first}“ und {} weitere", rest.len()),
    }
}

/// Dezimale Einheiten wie bei der Größengrenze (1 GB = 1000 MB), deutsch mit Komma:
/// „512 B“, „3,1 MB“, „1 GB“
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let rounded = format!("{value:.1}");
    let number = rounded.strip_suffix(".0").unwrap_or(&rounded).replace('.', ",");
    format!("{number} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sizes_are_german() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1000), "1 KB");
        assert_eq!(size_text(3_141_592), "3,1 MB");
        assert_eq!(size_text(999_960), "1 MB");
        assert_eq!(size_text(1_000_000_000), "1 GB");
        assert_eq!(size_text(2_500_000_000), "2,5 GB");
    }

    #[test]
    fn names_are_shortened_from_four_on() {
        assert_eq!(names_text(&[]), "Dateien");
        assert_eq!(names_text(&names(&["Bericht.pdf"])), "Bericht.pdf");
        assert_eq!(names_text(&names(&["a", "b"])), "a und b");
        assert_eq!(names_text(&names(&["a", "b", "c"])), "a, b und c");
        assert_eq!(names_text(&names(&["a", "b", "c", "d"])), "„a“ und 3 weitere");
    }

    #[test]
    fn messages_match_the_events() {
        let ready = FilesNotice::Ready { peer: "pc".into(), names: names(&["Bericht.pdf"]), bytes: 3_100_000 };
        assert_eq!(describe(&ready, false).text.as_deref(), Some("Bericht.pdf ist bereit – mit Strg+V einfügen"));
        assert_eq!(describe(&ready, true).text.as_deref(), Some("Bericht.pdf ist bereit – mit Cmd+V einfügen"));
        assert_eq!(describe(&ready, false).tone, Tone::Success);

        let many = FilesNotice::Ready { peer: "pc".into(), names: names(&["a", "b", "c", "d"]), bytes: 1 };
        assert_eq!(describe(&many, false).text.as_deref(), Some("„a“ und 3 weitere sind bereit – mit Strg+V einfügen"));

        let receiving = FilesNotice::Receiving { peer: "pc".into(), names: names(&["Bericht.pdf"]), bytes: 3_141_592 };
        assert_eq!(describe(&receiving, false).text.as_deref(), Some("Dateien von „pc“ kommen an: Bericht.pdf (3,1 MB)"));

        let large = FilesNotice::TooLarge { peer: "pc".into(), names: names(&["Film.mkv"]), limit: 1_000_000_000 };
        let message = describe(&large, false);
        assert_eq!(message.text.as_deref(), Some("Kopierte Dateien sind größer als 1 GB – bitte per Drag & Drop ziehen"));
        assert_eq!(message.tone, Tone::Warn);

        let failed = FilesNotice::Failed { peer: "pc".into(), reason: "Prüfsumme falsch".into() };
        assert_eq!(describe(&failed, false).tone, Tone::Error);

        let sending = FilesNotice::Sending { peer: "pc".into(), names: names(&["a"]), bytes: 1 };
        assert_eq!(describe(&sending, false).text, None, "Senden erscheint nur im Protokoll");
    }
}
