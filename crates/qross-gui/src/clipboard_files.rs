//! Texte zu kopierten Dateien (Strg+C auf Dateien, drüben Strg+V) für Hinweise in der App und
//! Systembenachrichtigungen. Das Protokoll schreibt `qross_net` selbst. Während der Übertragung
//! zeigt die Fortschrittsanzeige (`transfers`) den Stand; „bereit“ und Fehler landen dann in deren
//! Zeile statt in einem zweiten Hinweis.

use qross_core::text::size_text_in;
use qross_net::clipfiles::FilesNotice;
use serde::Serialize;

use crate::i18n::Lang;

/// Wie der Hinweis aussieht
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Info,
    Success,
    Warn,
    Error,
}

/// Welche Meldung es war
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Sending,
    Receiving,
    Ready,
    TooLarge,
    Failed,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct FilesMessage {
    pub event: Event,
    /// Der andere Rechner (findet die passende Zeile der Fortschrittsanzeige)
    pub peer: String,
    pub tone: Tone,
    /// Überschrift einer Systembenachrichtigung
    pub title: String,
    /// Text des Hinweises; `None` = kein Hinweis (Senden und Ankommen zeigt die Fortschrittsanzeige)
    pub text: Option<String>,
}

/// Beschreibt eine Meldung in Worten (in `lang`). `mac` wählt Cmd statt Strg.
pub fn describe(notice: &FilesNotice, mac: bool, lang: Lang) -> FilesMessage {
    let paste = match (mac, lang) {
        (true, _) => "Cmd+V",
        (false, Lang::De) => "Strg+V",
        (false, Lang::En) => "Ctrl+V",
    };
    let peer = match notice {
        FilesNotice::Sending { peer, .. }
        | FilesNotice::TooLarge { peer, .. }
        | FilesNotice::Receiving { peer, .. }
        | FilesNotice::Ready { peer, .. }
        | FilesNotice::Failed { peer, .. } => peer.clone(),
    };
    match notice {
        FilesNotice::Sending { .. } => FilesMessage {
            event: Event::Sending,
            peer,
            tone: Tone::Info,
            title: match lang {
                Lang::De => "Kopierte Dateien werden gesendet".into(),
                Lang::En => "Sending copied files".into(),
            },
            text: None,
        },
        FilesNotice::Receiving { .. } => FilesMessage {
            event: Event::Receiving,
            peer,
            tone: Tone::Info,
            title: match lang {
                Lang::De => "Kopierte Dateien kommen an".into(),
                Lang::En => "Copied files are arriving".into(),
            },
            text: None,
        },
        FilesNotice::Ready { names, .. } => FilesMessage {
            event: Event::Ready,
            peer,
            tone: Tone::Success,
            title: match lang {
                Lang::De => "Kopierte Dateien sind bereit".into(),
                Lang::En => "Copied files are ready".into(),
            },
            text: Some(match lang {
                Lang::De => format!(
                    "{} {} bereit – mit {paste} einfügen",
                    names_text(names, lang),
                    if names.len() == 1 { "ist" } else { "sind" }
                ),
                Lang::En => format!(
                    "{} {} ready – press {paste} to paste",
                    names_text(names, lang),
                    if names.len() == 1 { "is" } else { "are" }
                ),
            }),
        },
        FilesNotice::TooLarge { limit, .. } => FilesMessage {
            event: Event::TooLarge,
            peer,
            tone: Tone::Warn,
            title: match lang {
                Lang::De => "Kopierte Dateien zu groß".into(),
                Lang::En => "Copied files too large".into(),
            },
            text: Some(match lang {
                Lang::De => {
                    format!("Kopierte Dateien sind größer als {} – bitte per Drag & Drop ziehen", size_text_in(*limit, lang))
                }
                Lang::En => format!("Copied files are larger than {} – please use drag and drop", size_text_in(*limit, lang)),
            }),
        },
        FilesNotice::Failed { reason, .. } => FilesMessage {
            event: Event::Failed,
            tone: Tone::Error,
            title: match lang {
                Lang::De => "Kopierte Dateien nicht übertragen".into(),
                Lang::En => "Copied files not transferred".into(),
            },
            text: Some(match lang {
                Lang::De => format!("Kopierte Dateien mit „{peer}“ nicht übertragen: {reason}"),
                Lang::En => format!("Couldn't transfer copied files with “{peer}”: {reason}"),
            }),
            peer,
        },
    }
}

/// „Bericht.pdf“, „a und b“, „a, b und c“, ab vier: „„a“ und 3 weitere“ (englisch „a, b, and c“,
/// „“a” and 3 more“)
pub fn names_text(names: &[String], lang: Lang) -> String {
    match (names, lang) {
        ([], Lang::De) => "Dateien".into(),
        ([], Lang::En) => "Files".into(),
        ([one], _) => one.clone(),
        ([first, second], Lang::De) => format!("{first} und {second}"),
        ([first, second], Lang::En) => format!("{first} and {second}"),
        ([first, second, third], Lang::De) => format!("{first}, {second} und {third}"),
        ([first, second, third], Lang::En) => format!("{first}, {second}, and {third}"),
        ([first, rest @ ..], Lang::De) => format!("„{first}“ und {} weitere", rest.len()),
        ([first, rest @ ..], Lang::En) => format!("“{first}” and {} more", rest.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_are_shortened_from_four_on() {
        assert_eq!(names_text(&[], Lang::De), "Dateien");
        assert_eq!(names_text(&names(&["Bericht.pdf"]), Lang::De), "Bericht.pdf");
        assert_eq!(names_text(&names(&["a", "b"]), Lang::De), "a und b");
        assert_eq!(names_text(&names(&["a", "b", "c"]), Lang::De), "a, b und c");
        assert_eq!(names_text(&names(&["a", "b", "c", "d"]), Lang::De), "„a“ und 3 weitere");

        assert_eq!(names_text(&[], Lang::En), "Files");
        assert_eq!(names_text(&names(&["Report.pdf"]), Lang::En), "Report.pdf");
        assert_eq!(names_text(&names(&["a", "b"]), Lang::En), "a and b");
        assert_eq!(names_text(&names(&["a", "b", "c"]), Lang::En), "a, b, and c");
        assert_eq!(names_text(&names(&["a", "b", "c", "d"]), Lang::En), "“a” and 3 more");
    }

    #[test]
    fn messages_match_the_events() {
        let ready = FilesNotice::Ready { peer: "pc".into(), names: names(&["Bericht.pdf"]), bytes: 3_100_000 };
        assert_eq!(describe(&ready, false, Lang::De).text.as_deref(), Some("Bericht.pdf ist bereit – mit Strg+V einfügen"));
        assert_eq!(describe(&ready, true, Lang::De).text.as_deref(), Some("Bericht.pdf ist bereit – mit Cmd+V einfügen"));
        assert_eq!(describe(&ready, false, Lang::De).tone, Tone::Success);

        let many = FilesNotice::Ready { peer: "pc".into(), names: names(&["a", "b", "c", "d"]), bytes: 1 };
        assert_eq!(
            describe(&many, false, Lang::De).text.as_deref(),
            Some("„a“ und 3 weitere sind bereit – mit Strg+V einfügen")
        );

        let message = describe(&ready, false, Lang::De);
        assert_eq!((message.event, message.peer.as_str()), (Event::Ready, "pc"));

        let receiving = FilesNotice::Receiving { peer: "pc".into(), names: names(&["Bericht.pdf"]), bytes: 3_141_592 };
        assert_eq!(describe(&receiving, false, Lang::De).text, None, "das zeigt die Fortschrittsanzeige");

        let large = FilesNotice::TooLarge { peer: "pc".into(), names: names(&["Film.mkv"]), limit: 1_000_000_000 };
        let message = describe(&large, false, Lang::De);
        assert_eq!(message.text.as_deref(), Some("Kopierte Dateien sind größer als 1 GB – bitte per Drag & Drop ziehen"));
        assert_eq!(message.tone, Tone::Warn);

        let failed = FilesNotice::Failed { peer: "pc".into(), reason: "Prüfsumme falsch".into() };
        assert_eq!(describe(&failed, false, Lang::De).tone, Tone::Error);

        let sending = FilesNotice::Sending { peer: "pc".into(), names: names(&["a"]), bytes: 1 };
        assert_eq!(describe(&sending, false, Lang::De).text, None, "Senden erscheint nur im Protokoll");
    }

    #[test]
    fn messages_in_english() {
        let ready = FilesNotice::Ready { peer: "pc".into(), names: names(&["Report.pdf"]), bytes: 3_100_000 };
        let message = describe(&ready, false, Lang::En);
        assert_eq!(message.title, "Copied files are ready");
        assert_eq!(message.text.as_deref(), Some("Report.pdf is ready – press Ctrl+V to paste"));
        assert_eq!(describe(&ready, true, Lang::En).text.as_deref(), Some("Report.pdf is ready – press Cmd+V to paste"));
        let many = FilesNotice::Ready { peer: "pc".into(), names: names(&["a", "b"]), bytes: 1 };
        assert_eq!(describe(&many, false, Lang::En).text.as_deref(), Some("a and b are ready – press Ctrl+V to paste"));

        let large = FilesNotice::TooLarge { peer: "pc".into(), names: names(&["Movie.mkv"]), limit: 2_500_000_000 };
        let message = describe(&large, false, Lang::En);
        assert_eq!(message.title, "Copied files too large");
        assert_eq!(message.text.as_deref(), Some("Copied files are larger than 2.5 GB – please use drag and drop"));
        let german = describe(&large, false, Lang::De);
        assert_eq!(german.text.as_deref(), Some("Kopierte Dateien sind größer als 2,5 GB – bitte per Drag & Drop ziehen"));

        let failed = FilesNotice::Failed { peer: "pc".into(), reason: "checksum mismatch".into() };
        let message = describe(&failed, false, Lang::En);
        assert_eq!(message.title, "Copied files not transferred");
        assert_eq!(message.text.as_deref(), Some("Couldn't transfer copied files with “pc”: checksum mismatch"));
        let german = describe(&failed, false, Lang::De);
        assert_eq!(german.text.as_deref(), Some("Kopierte Dateien mit „pc“ nicht übertragen: checksum mismatch"));
    }
}
